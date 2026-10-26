//! Admission and resolution for ordinary single-target damaging moves.
//!
//! Resolution consumes accuracy, critical-hit, damage-variance, and trailing
//! effect-chance draws in that order. Accuracy bypasses and critical-hit
//! suppression omit their respective draws. A failed accuracy roll stops
//! immediately, classified by [`classify_accuracy_failure`], while a type
//! immunity on a landed roll still consumes the damage and effect-chance
//! draws. Struggle skips the trailing effect-chance draw.

use assets::{AbilityId, Effectiveness, MoveEffect, MoveId, Type};

use crate::ability::{
    huge_power_attack, pinch_boosts_power, plus_minus_special_attack, suppresses_critical_hits,
    thick_fat_special_attack,
};
use crate::accuracy::accuracy_check;
use crate::critical::{crit_adjusted_stages, crit_roll, crit_stage};
use crate::damage::{
    aggregate_type_effectiveness, apply_damage_roll, apply_dual_type_effectiveness, apply_stab,
    base_damage_with_flash_fire, has_stab, BattleRng, DamageInput, MoveCategory, Weather, STRUGGLE,
};
use crate::dex::Dex;
use crate::error::BattleError;
use crate::pokemon::BattlePokemon;
use crate::secondary::{
    is_confuse_hit_effect, is_poison_hit_effect, is_speed_down_hit_effect,
    spend_effect_chance_draw, SecondaryApplication,
};

const EFFECT_HIT: MoveEffect = MoveEffect(0);
const EFFECT_SPEED_UP: MoveEffect = MoveEffect(12);
const EFFECT_SPECIAL_DEFENSE_UP: MoveEffect = MoveEffect(14);
const EFFECT_ACCURACY_UP: MoveEffect = MoveEffect(15);
const EFFECT_ALWAYS_HIT: MoveEffect = MoveEffect(17);
const EFFECT_SPECIAL_ATTACK_DOWN: MoveEffect = MoveEffect(21);
const EFFECT_SPECIAL_DEFENSE_DOWN: MoveEffect = MoveEffect(22);
const EFFECT_HIGH_CRITICAL: MoveEffect = MoveEffect(43);
const EFFECT_ACCURACY_UP_2: MoveEffect = MoveEffect(55);
const EFFECT_EVASION_UP_2: MoveEffect = MoveEffect(56);
const EFFECT_SPECIAL_ATTACK_DOWN_2: MoveEffect = MoveEffect(61);
const EFFECT_ACCURACY_DOWN_2: MoveEffect = MoveEffect(63);
const EFFECT_EVASION_DOWN_2: MoveEffect = MoveEffect(64);
const EFFECT_EVASION_DOWN_HIT: MoveEffect = MoveEffect(74);
const EFFECT_VITAL_THROW: MoveEffect = MoveEffect(78);
const EFFECT_UNUSED_60: MoveEffect = MoveEffect(96);
const EFFECT_QUICK_ATTACK: MoveEffect = MoveEffect(103);
const EFFECT_UNUSED_6E: MoveEffect = MoveEffect(110);
const EFFECT_UNUSED_83: MoveEffect = MoveEffect(131);
const EFFECT_UNUSED_8D: MoveEffect = MoveEffect(141);
const EFFECT_UNUSED_A3: MoveEffect = MoveEffect(163);

const ORDINARY_HIT_EFFECTS: [MoveEffect; 21] = [
    EFFECT_HIT,
    EFFECT_SPEED_UP,
    EFFECT_SPECIAL_DEFENSE_UP,
    EFFECT_ACCURACY_UP,
    EFFECT_ALWAYS_HIT,
    EFFECT_SPECIAL_ATTACK_DOWN,
    EFFECT_SPECIAL_DEFENSE_DOWN,
    EFFECT_HIGH_CRITICAL,
    EFFECT_ACCURACY_UP_2,
    EFFECT_EVASION_UP_2,
    EFFECT_SPECIAL_ATTACK_DOWN_2,
    EFFECT_ACCURACY_DOWN_2,
    EFFECT_EVASION_DOWN_2,
    EFFECT_EVASION_DOWN_HIT,
    EFFECT_VITAL_THROW,
    EFFECT_UNUSED_60,
    EFFECT_QUICK_ATTACK,
    EFFECT_UNUSED_6E,
    EFFECT_UNUSED_83,
    EFFECT_UNUSED_8D,
    EFFECT_UNUSED_A3,
];

const CRITICAL_DAMAGE_MULTIPLIER: u32 = 2;
const CHARGE_DAMAGE_MULTIPLIER: u32 = 2;

/// The result of resolving one move against one target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HitOutcome {
    /// The accuracy check failed.
    Miss,
    /// The move connected, but the target was immune.
    NoEffect,
    /// A Ground move was blocked by the target's Levitate
    /// (`battle_script_commands.c:1375-1383`).
    LevitateBlocked,
    /// A powered move other than Struggle that was not strictly super
    /// effective was blocked by the target's Wonder Guard
    /// (`battle_script_commands.c:1409-1418`; Struggle bypasses `typecalc`).
    WonderGuardBlocked,
    /// The target's Volt Absorb or Water Absorb took the move
    /// (`ABILITYEFFECT_ABSORBING`, `battle_util.c:2681-2701`, `:2730-2744`).
    Absorbed {
        /// HP the target heals: a quarter of max HP floored to one, before the
        /// HP cap. Zero when the target is already at full HP, where upstream
        /// reports the move useless instead (`BattleScript_MonMadeMoveUseless`).
        healing: u32,
    },
    /// The target's Flash Fire took a Fire move (`battle_util.c:2703-2726`).
    ///
    /// Upstream also lets a frozen holder be hit. `Status1` cannot represent
    /// freeze yet, so that exemption has nothing to test.
    FlashFireBlocked {
        /// Whether this activation latches the boost for the first time.
        first_activation: bool,
    },
    /// The move connected and dealt damage.
    Hit {
        /// HP of damage dealt.
        damage: u32,
        /// Whether the hit was critical.
        is_critical: bool,
    },
}

/// Returns whether `effect` uses the ordinary-hit script without an engine
/// special case.
///
/// False Swipe and Pursuit share the script but are excluded because upstream
/// clamps or redirects them outside it (`battle_script_commands.c:1683`,
/// `:8745`, `:9854`).
#[must_use]
pub fn is_ordinary_hit_effect(effect: MoveEffect) -> bool {
    ORDINARY_HIT_EFFECTS.contains(&effect)
}

/// Validates that `move_id` can use [`resolve_hit`].
///
/// Validation completes before any state or RNG is touched.
///
/// # Errors
///
/// Returns [`BattleError::UnknownMove`], [`BattleError::NonDamagingMove`],
/// [`BattleError::UnsupportedMoveType`], or
/// [`BattleError::UnsupportedMoveEffect`] for the corresponding unsupported
/// move property.
pub fn ensure_resolvable(dex: &Dex, move_id: MoveId) -> Result<(), BattleError> {
    let move_data = dex.move_data(move_id)?;
    if move_data.power == 0 {
        return Err(BattleError::NonDamagingMove(move_id));
    }
    if move_data.move_type.battle_type().is_none() {
        return Err(BattleError::UnsupportedMoveType(move_id));
    }
    // `EFFECT_POISON_HIT`, `EFFECT_CONFUSE_HIT` and `EFFECT_SPEED_DOWN_HIT`
    // resolve through this same damage script while keeping their own
    // trampoline dispatch in `spend_effect_chance_draw`.
    if !is_ordinary_hit_effect(move_data.effect)
        && !is_poison_hit_effect(move_data.effect)
        && !is_confuse_hit_effect(move_data.effect)
        && !is_speed_down_hit_effect(move_data.effect)
        && move_id != STRUGGLE
    {
        return Err(BattleError::UnsupportedMoveEffect(move_id));
    }
    Ok(())
}

/// Runs the accuracy stage of a damaging move.
///
/// Always-hit effects consume no RNG value. Other effects consume one.
///
/// # Errors
///
/// Returns [`BattleError::UnknownMove`] before drawing when `move_id` is not
/// in `dex`.
pub fn accuracy_roll(
    dex: &Dex,
    move_id: MoveId,
    attacker: &BattlePokemon,
    defender: &BattlePokemon,
    rng: &mut impl BattleRng,
) -> Result<bool, BattleError> {
    let move_data = dex.move_data(move_id)?;
    Ok(accuracy_check(
        move_data.accuracy,
        move_data.effect,
        move_data.move_type,
        attacker.ability(),
        attacker.stages().accuracy,
        defender.stages().evasion,
        rng,
    ))
}

/// Whether `defender`'s typing gives `move_type` no effect at all.
///
/// This is the `TYPE_MUL_NO_EFFECT` scan shared by `Cmd_typecalc` and
/// `CheckWonderGuardAndLevitate` (`battle_script_commands.c:1454-1469`), which
/// flags `MOVE_RESULT_DOESNT_AFFECT_FOE` when either defender type carries an
/// immunity row.
#[must_use]
pub(crate) fn defender_is_immune(move_type: Type, defender: &BattlePokemon) -> bool {
    aggregate_type_effectiveness(move_type, defender.types()) == Effectiveness::NoEffect
}

/// Whether a Ground `move_type` is blocked by Levitate on `ability` -- the
/// defender's true ability at execution, or trainer AI's guessed ability for
/// an unrevealed target.
///
/// Levitate outranks the type scan in both `Cmd_typecalc`
/// (`battle_script_commands.c:1375-1383`) and `CheckWonderGuardAndLevitate`
/// (`:1435-1443`), each of which returns as soon as it matches. Callers apply
/// the Struggle exemption themselves.
#[must_use]
pub(crate) fn defender_levitate_blocked(move_type: Type, ability: AbilityId) -> bool {
    move_type == Type::Ground && ability == AbilityId::LEVITATE
}

/// Whether `defender`'s Wonder Guard blocks a not-strictly-super-effective
/// `move_type`.
///
/// Both `Cmd_typecalc` (`battle_script_commands.c:1409-1418`) and
/// `CheckWonderGuardAndLevitate` (`:1490-1497`) read
/// `ModulateDmgByType`'s effectiveness flags rather than floored running
/// damage, so [`aggregate_type_effectiveness`] is the right input. Callers
/// apply the Struggle and zero-power exemptions themselves.
#[must_use]
pub(crate) fn defender_wonder_guard_blocked(move_type: Type, defender: &BattlePokemon) -> bool {
    defender.ability() == AbilityId::WONDER_GUARD
        && aggregate_type_effectiveness(move_type, defender.types())
            != Effectiveness::SuperEffective
}

/// Classifies a failed accuracy roll without consuming an RNG draw.
///
/// `Cmd_accuracycheck` does not end a failed roll as a generic miss: after
/// setting `MOVE_RESULT_MISSED` it calls `CheckWonderGuardAndLevitate`
/// (`battle_script_commands.c:1175-1186`). That helper returns early for
/// Struggle and zero-power moves (`:1432-1433`), reports Levitate against a
/// Ground move (`:1435-1443`), flags `MOVE_RESULT_DOESNT_AFFECT_FOE` for a
/// typing immunity (`:1454-1469`), and lets Wonder Guard overwrite
/// `MISS_TYPE` with `B_MSG_AVOIDED_DMG` (`:1490-1497`). `Cmd_resultmessage`
/// then prefers Wonder Guard's message over the typing-immunity one
/// (`:2055-2059`, with `gMissStringIds` in `battle_message.c:890-897`), so the
/// observable order is Levitate, then Wonder Guard, then typing immunity, then
/// the generic miss.
///
/// The helper reads only move data and the defender, so classification adds no
/// draw to the single accuracy draw that failed.
///
/// # Errors
///
/// Returns [`BattleError::UnknownMove`] when `move_id` is not in `dex`, or
/// [`BattleError::UnsupportedMoveType`] when its type cannot participate in
/// battle calculations.
pub fn classify_accuracy_failure(
    dex: &Dex,
    move_id: MoveId,
    defender: &BattlePokemon,
) -> Result<HitOutcome, BattleError> {
    let move_data = dex.move_data(move_id)?;
    if move_id == STRUGGLE || move_data.power == 0 {
        return Ok(HitOutcome::Miss);
    }
    let move_type = move_data
        .move_type
        .battle_type()
        .ok_or(BattleError::UnsupportedMoveType(move_id))?;

    if defender_levitate_blocked(move_type, defender.ability()) {
        Ok(HitOutcome::LevitateBlocked)
    } else if defender_wonder_guard_blocked(move_type, defender) {
        Ok(HitOutcome::WonderGuardBlocked)
    } else if defender_is_immune(move_type, defender) {
        Ok(HitOutcome::NoEffect)
    } else {
        Ok(HitOutcome::Miss)
    }
}

/// Damage after critical-hit, STAB, and type rules but before damage variance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RawDamage {
    /// Damage before variance, or zero when the target is immune.
    pub damage: u32,
    /// Whether the hit was critical.
    pub is_critical: bool,
    /// Whether `damage` is zero because a Ground move met a Levitate holder,
    /// rather than an ordinary type immunity.
    pub levitate_blocked: bool,
    /// Whether `damage` is zero because Wonder Guard blocked a hit that was
    /// not strictly super effective, rather than an ordinary type immunity.
    pub wonder_guard_blocked: bool,
}

fn roll_critical_hit(
    move_effect: MoveEffect,
    attacker: &BattlePokemon,
    defender: &BattlePokemon,
    critical_hits_suppressed: bool,
    rng: &mut impl BattleRng,
) -> bool {
    let defender_suppresses_critical_hits = suppresses_critical_hits(defender.ability());
    if critical_hits_suppressed || defender_suppresses_critical_hits {
        return false;
    }

    let critical_hit_stage = crit_stage(move_effect, attacker.volatiles().focus_energy);
    crit_roll(critical_hit_stage, rng)
}

fn damage_input(
    move_power: u8,
    move_type: Type,
    attacker: &BattlePokemon,
    defender: &BattlePokemon,
    is_critical: bool,
) -> DamageInput {
    let move_category = MoveCategory::for_type(move_type);
    let (raw_attack, attack_stage) = attacker.attacking_stat(move_category);
    let ability_adjusted_attack = thick_fat_special_attack(
        defender.ability(),
        move_type,
        huge_power_attack(attacker.ability(), move_category, raw_attack),
    );
    let ability_adjusted_attack = plus_minus_special_attack(
        attacker.ability(),
        defender.ability(),
        move_category,
        ability_adjusted_attack,
    );
    let (defense, defense_stage) = defender.defending_stat(move_category);
    let (attack_stage, defense_stage) =
        crit_adjusted_stages(attack_stage, defense_stage, is_critical);

    DamageInput {
        attacker_level: attacker.level(),
        power: u32::from(move_power),
        move_type,
        attack_stat: ability_adjusted_attack,
        attack_stage,
        defense_stat: defense,
        defense_stage,
        attacker_burned: false,
        reflect: false,
        light_screen: false,
        weather: Weather::None,
        is_solar_beam: false,
        attacker_pinch_boost: pinch_boosts_power(
            attacker.ability(),
            move_type,
            attacker.current_hp(),
            attacker.stats().max_hp,
        ),
    }
}

/// Calculates damage through type effectiveness, stopping before damage
/// variance.
///
/// The critical-hit draw is skipped when either the caller or defender's
/// ability suppresses critical hits. A Ground move against a Levitate holder
/// is zeroed before the type chart is consulted, matching
/// `Cmd_typecalc`'s dedicated Levitate branch upstream.
///
/// # Errors
///
/// Returns [`BattleError::UnknownMove`] or
/// [`BattleError::UnsupportedMoveType`] before drawing.
pub fn damage_before_roll(
    dex: &Dex,
    move_id: MoveId,
    attacker: &BattlePokemon,
    defender: &BattlePokemon,
    critical_hits_suppressed: bool,
    rng: &mut impl BattleRng,
) -> Result<RawDamage, BattleError> {
    let move_data = dex.move_data(move_id)?;
    let move_type = move_data
        .move_type
        .battle_type()
        .ok_or(BattleError::UnsupportedMoveType(move_id))?;

    let is_critical = roll_critical_hit(
        move_data.effect,
        attacker,
        defender,
        critical_hits_suppressed,
        rng,
    );
    let input = damage_input(move_data.power, move_type, attacker, defender, is_critical);

    let base_damage = base_damage_with_flash_fire(&input, attacker.volatiles().flash_fire);
    let damage_after_critical = if is_critical {
        base_damage * CRITICAL_DAMAGE_MULTIPLIER
    } else {
        base_damage
    };
    let damage_after_charge = if attacker.volatiles().charged_up() && move_type == Type::Electric {
        damage_after_critical * CHARGE_DAMAGE_MULTIPLIER
    } else {
        damage_after_critical
    };
    let levitate_blocked =
        move_id != STRUGGLE && defender_levitate_blocked(move_type, defender.ability());
    let wonder_guard_blocked =
        move_id != STRUGGLE && defender_wonder_guard_blocked(move_type, defender);
    let damage = if move_id == STRUGGLE {
        damage_after_charge
    } else if levitate_blocked || wonder_guard_blocked {
        0
    } else {
        let damage_after_stab = apply_stab(
            damage_after_charge,
            has_stab(attacker.types(), move_id, move_type),
        );
        apply_dual_type_effectiveness(damage_after_stab, move_type, defender.types())
    };

    Ok(RawDamage {
        damage,
        is_critical,
        levitate_blocked,
        wonder_guard_blocked,
    })
}

/// `ABILITYEFFECT_ABSORBING` for the defender's Volt Absorb, Water Absorb, or
/// Flash Fire (`battle_util.c:2676-2744`).
///
/// It runs after accuracy and before the critical-hit draw, so an absorbed
/// move consumes no further draws. The caller applies the heal or latch.
fn absorbing_outcome(
    dex: &Dex,
    move_id: MoveId,
    defender: &BattlePokemon,
) -> Result<Option<HitOutcome>, BattleError> {
    let move_data = dex.move_data(move_id)?;
    let move_type = move_data
        .move_type
        .battle_type()
        .ok_or(BattleError::UnsupportedMoveType(move_id))?;
    let absorbs_hp = match defender.ability() {
        AbilityId::VOLT_ABSORB => move_type == Type::Electric,
        AbilityId::WATER_ABSORB => move_type == Type::Water,
        AbilityId::FLASH_FIRE => {
            return Ok(
                (move_type == Type::Fire).then_some(HitOutcome::FlashFireBlocked {
                    first_activation: !defender.volatiles().flash_fire,
                }),
            );
        }
        _ => return Ok(None),
    };
    if !absorbs_hp || move_data.power == 0 {
        return Ok(None);
    }
    let max_hp = defender.stats().max_hp;
    let healing = if defender.current_hp() == max_hp {
        0
    } else {
        (max_hp / 4).max(1)
    };
    Ok(Some(HitOutcome::Absorbed { healing }))
}

/// Calculates a landed move's outcome through damage variance.
///
/// An absorbing ability ends the move first, before any draw.
/// Otherwise this always consumes a damage-variance draw, including for an immune
/// target. It consumes a preceding critical-hit draw unless critical hits are
/// suppressed.
///
/// # Errors
///
/// Returns the errors from [`damage_before_roll`] before the damage-variance
/// draw.
pub fn damage_core(
    dex: &Dex,
    move_id: MoveId,
    attacker: &BattlePokemon,
    defender: &BattlePokemon,
    critical_hits_suppressed: bool,
    rng: &mut impl BattleRng,
) -> Result<HitOutcome, BattleError> {
    if let Some(absorbed) = absorbing_outcome(dex, move_id, defender)? {
        return Ok(absorbed);
    }
    let raw_damage = damage_before_roll(
        dex,
        move_id,
        attacker,
        defender,
        critical_hits_suppressed,
        rng,
    )?;
    let damage = apply_damage_roll(raw_damage.damage, rng);

    if damage == 0 {
        if raw_damage.wonder_guard_blocked {
            Ok(HitOutcome::WonderGuardBlocked)
        } else if raw_damage.levitate_blocked {
            Ok(HitOutcome::LevitateBlocked)
        } else {
            Ok(HitOutcome::NoEffect)
        }
    } else {
        Ok(HitOutcome::Hit {
            damage,
            is_critical: raw_damage.is_critical,
        })
    }
}

/// [`resolve_hit`]'s full result: the damage verdict, plus what the trailing
/// effect-chance draw wants the caller to apply to `defender`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HitResolution {
    /// The damage verdict.
    pub outcome: HitOutcome,
    /// What the caller should apply to `defender`, subject to the caller's
    /// own post-damage faint check ([`spend_effect_chance_draw`]).
    pub secondary_effect: SecondaryApplication,
}

/// Resolves an ordinary hit against one target.
///
/// A landed ordinary move consumes accuracy, critical-hit, damage-variance,
/// and effect-chance draws in order, subject to the documented bypasses.
/// Struggle omits the effect-chance stage because its recoil effect is outside
/// this function's damage-only contract.
///
/// # Errors
///
/// Returns the errors from [`ensure_resolvable`], [`accuracy_roll`],
/// [`classify_accuracy_failure`], [`damage_core`], or
/// [`spend_effect_chance_draw`]. Admission completes before any draw.
pub fn resolve_hit(
    dex: &Dex,
    move_id: MoveId,
    attacker: &BattlePokemon,
    defender: &BattlePokemon,
    critical_hits_suppressed: bool,
    rng: &mut impl BattleRng,
) -> Result<HitResolution, BattleError> {
    ensure_resolvable(dex, move_id)?;

    if !accuracy_roll(dex, move_id, attacker, defender, rng)? {
        return Ok(HitResolution {
            outcome: classify_accuracy_failure(dex, move_id, defender)?,
            secondary_effect: SecondaryApplication::None,
        });
    }

    let outcome = damage_core(
        dex,
        move_id,
        attacker,
        defender,
        critical_hits_suppressed,
        rng,
    )?;

    let secondary_effect = if move_id == STRUGGLE
        || matches!(
            outcome,
            HitOutcome::Absorbed { .. } | HitOutcome::FlashFireBlocked { .. }
        ) {
        SecondaryApplication::None
    } else {
        // Wonder Guard and Levitate both carry `MOVE_RESULT_MISSED`, part of
        // upstream's `MOVE_RESULT_NO_EFFECT` bitmask
        // (`include/constants/battle.h:220-228`), so each suppresses a
        // secondary effect exactly like an ordinary `NoEffect` immunity.
        // `seteffectwithchance` still spends the chance draw on that path
        // (`battle_script_commands.c:2923-2930`).
        let hit_had_effect = !matches!(
            outcome,
            HitOutcome::NoEffect | HitOutcome::WonderGuardBlocked | HitOutcome::LevitateBlocked
        );
        spend_effect_chance_draw(dex, move_id, hit_had_effect, defender, rng)?
    };

    Ok(HitResolution {
        outcome,
        secondary_effect,
    })
}

#[cfg(test)]
#[path = "hit/tests.rs"]
mod tests;
