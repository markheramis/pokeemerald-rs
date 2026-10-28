//! Trainer AI admission and move scoring.
//!
//! Route 103 trainers use four scoring scripts over a finite set of move
//! effects. The admission functions reject other flags and effects (per
//! enabled script, see [`ensure_scoreable`]) before a battle starts because
//! unmodelled scoring branches can consume different RNG draws.
//!
//! `CheckBadMove`'s target-ability guards read `get_ability AI_TARGET`, which
//! upstream answers from battle history, a trapping ability, or a fresh
//! `Random() & 1` guess between a species' two ability slots
//! (`src/battle_ai_script_commands.c:1350-1396`). This engine models no
//! ability history, so it never guesses: a target whose true ability is
//! Shadow Tag, Magnet Pull, or Arena Trap reads it directly, as upstream does
//! ahead of the guess (`src/battle_ai_script_commands.c:1368-1376`); any other
//! scoring target whose species has two ability slots is refused with
//! [`BattleError::AmbiguousPlayerAbility`] before any draw, and a
//! single-ability target reads its sole ability without drawing.

use assets::trainers::AiFlags;
use assets::{AbilityId, MoveEffect, MoveId, Type, TypeChart};

use super::opponent_ai::{all_known_moves_are_spent, selectable_slot, EnemyAction};
use crate::damage::{apply_stab, apply_type_effectiveness, base_damage, has_stab, BattleRng};
use crate::damage::{DamageInput, MoveCategory, Weather};
use crate::dex::Dex;
use crate::error::BattleError;
use crate::hit::defender_levitate_blocked;
use crate::pokemon::{BattlePokemon, MAX_MON_MOVES};
use crate::stat_change::{
    stage_of, stat_change_for_effect, ChangedStat, StatChangeDirection, StatChangeEffect,
    StatChangeMagnitude, CLEAR_BODY, HYPER_CUTTER, KEEN_EYE, SOUNDPROOF, WHITE_SMOKE,
};
use crate::stat_stage::StatStage;
use crate::status1::Status1;

const PERCENT_SCALE: u32 = 100;
const AI_RANDOM_ROLL_MODULUS: u16 = 256;
const SIMULATED_DAMAGE_VARIANCE: u16 = 16;
const MINIMUM_DAMAGE: u32 = 1;
const MINIMUM_DAMAGING_MOVE_POWER: u8 = 2;

const INITIAL_MOVE_SCORE: i8 = 100;
const UNUSABLE_MOVE_SCORE: i8 = 0;
const STRONGLY_DISCOURAGE: i8 = -10;
const REDUNDANT_CONFUSION_PENALTY: i8 = -5;
const SLIGHTLY_DISCOURAGE: i8 = -1;
const ABSORBED_MOVE_PENALTY: i8 = -12;
const DISCOURAGE: i8 = -2;
const ENCOURAGE: i8 = 2;
const STRONGLY_ENCOURAGE: i8 = 4;

const QUADRUPLE_EFFECTIVENESS_BONUS_THRESHOLD: u16 = 80;
const STAT_DROP_DISCOURAGEMENT_THRESHOLD: u16 = 50;
const FIRST_TURN_SETUP_BONUS_THRESHOLD: u16 = 80;
const ATTACK_DROP_USER_HP_THRESHOLD: u32 = 90;
const DEFENSE_DROP_USER_HP_THRESHOLD: u32 = 70;
const STAT_DROP_TARGET_HP_THRESHOLD: u32 = 70;
const HEAVILY_LOWERED_STAT_STAGE: i8 = -3;
const FIRST_TURN: u8 = 0;

const AI_EFFECTIVENESS_NEUTRAL: u32 = 40;
const AI_EFFECTIVENESS_SUPER: u32 = 80;
const AI_EFFECTIVENESS_SUPER_WITH_STAB: u32 = 120;
const AI_EFFECTIVENESS_QUADRUPLE: u32 = 160;
const AI_EFFECTIVENESS_QUADRUPLE_WITH_STAB: u32 = 240;

const SUPPORTED_AI_FLAGS: u32 = AiFlags::CHECK_BAD_MOVE.bits()
    | AiFlags::TRY_TO_FAINT.bits()
    | AiFlags::CHECK_VIABILITY.bits()
    | AiFlags::SETUP_FIRST_TURN.bits();

/// Effect IDs `AI_TryToFaint` and `AI_CV_Sweeper`'s most-powerful-move
/// comparison treats as powerless (`sIgnoredPowerfulMoveEffects`,
/// `src/battle_ai_script_commands.c:266-280`): Explosion, Dream Eater, Razor
/// Wind, Sky Attack, Recharge, Skull Bash, Solar Beam, Spit Up, Focus Punch,
/// Superpower, Eruption, Overheat. A depleted slot holding one is admitted but
/// still sits in the comparison, so it must be skipped there.
const IGNORED_POWERFUL_MOVE_EFFECTS: [MoveEffect; 12] = [
    MoveEffect::EXPLOSION,
    MoveEffect::DREAM_EATER,
    MoveEffect::RAZOR_WIND,
    MoveEffect::SKY_ATTACK,
    MoveEffect::RECHARGE,
    MoveEffect::SKULL_BASH,
    MoveEffect::SOLAR_BEAM,
    MoveEffect::SPIT_UP,
    MoveEffect::FOCUS_PUNCH,
    MoveEffect::SUPERPOWER,
    MoveEffect::ERUPTION,
    MoveEffect::OVERHEAT,
];

fn is_ignored_powerful_move_effect(effect: MoveEffect) -> bool {
    IGNORED_POWERFUL_MOVE_EFFECTS.contains(&effect)
}

/// The audited effects `AI_CheckBadMove` leaves unchanged once its prologue
/// passes: none appears in its `if_effect` chain
/// (`data/battle_ai_scripts.s:105-214`). `EFFECT_HIT` has its own variant. Any
/// other effect is never an implicit no-op.
const UNCHANGED_AFTER_PROLOGUE_EFFECTS: [MoveEffect; 13] = [
    MoveEffect::ABSORB,
    MoveEffect::POISON_HIT,
    MoveEffect::ALWAYS_HIT,
    MoveEffect::MULTI_HIT,
    MoveEffect::DRAGON_RAGE,
    MoveEffect::HIGH_CRITICAL,
    MoveEffect::SPEED_DOWN_HIT,
    MoveEffect::EVASION_DOWN_HIT,
    MoveEffect::CONFUSE_HIT,
    MoveEffect::VITAL_THROW,
    MoveEffect::SPLASH,
    MoveEffect::QUICK_ATTACK,
    MoveEffect::CHARGE,
];

/// Rows whose `AI_CheckViability` branch is not ported
/// (`data/battle_ai_scripts.s:655,666,686,706,708`).
const UNPORTED_VIABILITY_EFFECTS: [MoveEffect; 5] = [
    MoveEffect::ABSORB,
    MoveEffect::ALWAYS_HIT,
    MoveEffect::HIGH_CRITICAL,
    MoveEffect::SPEED_DOWN_HIT,
    MoveEffect::VITAL_THROW,
];

/// Effects `AI_TryToFaint` refuses until its extra scoring is ported.
///
/// `Charge` doubles the next Electric move's damage, which upstream's
/// `AI_CalcDmg` folds into every simulated-damage read
/// (`src/battle_script_commands.c:1306-1316`); `estimated_damage` does not, so
/// the one damage-reading script, `TRY_TO_FAINT` (`if_can_faint`,
/// `data/battle_ai_scripts.s:2618`), refuses it. `AI_CheckViability` neither
/// dispatches `EFFECT_CHARGE` nor reads damage (`:619-743`).
///
/// `QUICK_ATTACK` earns an extra +2 from `AI_TryToFaint` when it can faint the
/// target (`data/battle_ai_scripts.s:2629-2634`).
const UNPORTED_TRY_TO_FAINT_EFFECTS: [MoveEffect; 2] =
    [MoveEffect::CHARGE, MoveEffect::QUICK_ATTACK];

/// AI-only: effects `AI_CheckBadMove` routes to an `AI_CBM_*Up` / `*Down` label
/// that execution does not map (`data/battle_ai_scripts.s:108-121,133-146,163,186`).
const AI_ONLY_STAT_LABELS: [(
    MoveEffect,
    ChangedStat,
    StatChangeMagnitude,
    StatChangeDirection,
); 12] = {
    use ChangedStat::{Accuracy, Defense, Evasion, SpAttack, SpDefense, Speed};
    use StatChangeDirection::{Lower, Raise};
    use StatChangeMagnitude::{One, Two};
    [
        (MoveEffect::SPEED_UP, Speed, One, Raise),
        (MoveEffect::SPECIAL_DEFENSE_UP, SpDefense, One, Raise),
        (MoveEffect::ACCURACY_UP, Accuracy, One, Raise),
        (MoveEffect::SPECIAL_ATTACK_DOWN, SpAttack, One, Lower),
        (MoveEffect::SPECIAL_DEFENSE_DOWN, SpDefense, One, Lower),
        (MoveEffect::ACCURACY_UP_2, Accuracy, Two, Raise),
        (MoveEffect::EVASION_UP_2, Evasion, Two, Raise),
        (MoveEffect::SPECIAL_ATTACK_DOWN_2, SpAttack, Two, Lower),
        (MoveEffect::ACCURACY_DOWN_2, Accuracy, Two, Lower),
        (MoveEffect::EVASION_DOWN_2, Evasion, Two, Lower),
        (MoveEffect::MINIMIZE, Evasion, One, Raise),
        (MoveEffect::DEFENSE_CURL, Defense, One, Raise),
    ]
};

/// The stat change an `AI_CBM_*` stat label scores for `effect`.
fn ai_stat_label(effect: MoveEffect) -> Option<StatChangeEffect> {
    stat_change_for_effect(effect).or_else(|| {
        AI_ONLY_STAT_LABELS
            .iter()
            .find(|(candidate, ..)| *candidate == effect)
            .map(|&(_, stat, magnitude, direction)| StatChangeEffect {
                stat,
                magnitude,
                direction,
            })
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScoreableEffect {
    Hit,
    AttackDown,
    DefenseDown,
    /// `AI_CBM_Paralyze` (`data/battle_ai_scripts.s:397-403`).
    Paralyze,
    /// `AI_CBM_Confuse` (`data/battle_ai_scripts.s:386-395`). `EFFECT_SWAGGER`
    /// and `EFFECT_FLATTER` also reach it but stay unadmitted.
    Confuse,
    /// `AI_CBM_FocusEnergy` (`data/battle_ai_scripts.s:382-384`).
    FocusEnergy,
    /// `AI_CBM_HighRiskForDamage`, reached from Sonic Boom
    /// (`data/battle_ai_scripts.s:177,368-376`).
    HighRiskForDamage,
    /// One of [`UNCHANGED_AFTER_PROLOGUE_EFFECTS`].
    UnchangedAfterPrologue(MoveEffect),
    /// Any other shared stat-stage label (see [`ai_stat_label`]). Its
    /// `AI_CheckViability` and `AI_SetupFirstTurn` branches are not ported,
    /// except that Defense Curl has no viability branch.
    StatLabel(MoveEffect, StatChangeEffect),
}

impl ScoreableEffect {
    fn from_raw(effect: MoveEffect) -> Option<Self> {
        if effect == MoveEffect::HIT {
            Some(Self::Hit)
        } else if effect == MoveEffect::ATTACK_DOWN {
            Some(Self::AttackDown)
        } else if effect == MoveEffect::DEFENSE_DOWN {
            Some(Self::DefenseDown)
        } else if effect == MoveEffect::PARALYZE {
            Some(Self::Paralyze)
        } else if effect == MoveEffect::CONFUSE {
            Some(Self::Confuse)
        } else if effect == MoveEffect::FOCUS_ENERGY {
            Some(Self::FocusEnergy)
        } else if effect == MoveEffect::SONICBOOM {
            Some(Self::HighRiskForDamage)
        } else if let Some(label) = ai_stat_label(effect) {
            Some(Self::StatLabel(effect, label))
        } else {
            UNCHANGED_AFTER_PROLOGUE_EFFECTS
                .contains(&effect)
                .then_some(Self::UnchangedAfterPrologue(effect))
        }
    }

    /// `AI_CheckViability` branches on Paralyze and Confuse
    /// (`data/battle_ai_scripts.s:687,704`), and `AI_SetupFirstTurn` rolls for
    /// all three (`:2667,2668,2685`); neither is ported. Focus Energy has no
    /// viability branch.
    const fn supported_with_status_handler(self, flags: AiFlags) -> bool {
        let viability_branch = !matches!(self, Self::FocusEnergy);
        !(flags.contains(AiFlags::SETUP_FIRST_TURN)
            || viability_branch && flags.contains(AiFlags::CHECK_VIABILITY))
    }

    const fn is_stat_drop(self) -> bool {
        matches!(self, Self::AttackDown | Self::DefenseDown)
    }

    /// Whether every enabled script has a ported branch for this effect.
    fn supported_by(self, flags: AiFlags) -> bool {
        let effect = match self {
            Self::Paralyze | Self::Confuse | Self::FocusEnergy => {
                return self.supported_with_status_handler(flags);
            }
            Self::UnchangedAfterPrologue(effect) => effect,
            Self::StatLabel(effect, _) => {
                return !flags.contains(AiFlags::SETUP_FIRST_TURN)
                    && (effect == MoveEffect::DEFENSE_CURL
                        || !flags.contains(AiFlags::CHECK_VIABILITY));
            }
            Self::HighRiskForDamage => return flags == AiFlags::CHECK_BAD_MOVE,
            Self::Hit | Self::AttackDown | Self::DefenseDown => return true,
        };
        if flags.contains(AiFlags::CHECK_VIABILITY) && UNPORTED_VIABILITY_EFFECTS.contains(&effect)
        {
            return false;
        }
        !(flags.contains(AiFlags::TRY_TO_FAINT) && UNPORTED_TRY_TO_FAINT_EFFECTS.contains(&effect))
    }
}

fn scoreable_effect(dex: &Dex, move_id: MoveId) -> Result<ScoreableEffect, BattleError> {
    ScoreableEffect::from_raw(dex.move_data(move_id)?.effect)
        .ok_or(BattleError::UnscoreableMoveEffect(move_id))
}

/// Rejects a move whose effect the enabled trainer AI scripts cannot score.
///
/// A flagless trainer runs no scoring script (`ChooseMoveOrAction_Singles`
/// skips its flag loop, `src/battle_ai_script_commands.c:405-415`), so only the
/// move's dex residency is checked.
///
/// The unchanged-after-prologue rows are admitted when `CHECK_BAD_MOVE` is the
/// only script that would reach them, and refused when `CHECK_VIABILITY` or
/// `TRY_TO_FAINT` would take an unported branch. `SETUP_FIRST_TURN` has no
/// branch for any of them.
///
/// # Errors
///
/// Returns [`BattleError::UnknownMove`] or
/// [`BattleError::UnscoreableMoveEffect`].
pub(crate) fn ensure_scoreable(
    dex: &Dex,
    move_id: MoveId,
    flags: AiFlags,
) -> Result<(), BattleError> {
    if flags == AiFlags::NONE {
        return dex.move_data(move_id).map(|_| ());
    }
    if scoreable_effect(dex, move_id)?.supported_by(flags) {
        Ok(())
    } else {
        Err(BattleError::UnscoreableMoveEffect(move_id))
    }
}

/// Rejects trainer AI flags without a modelled scoring script.
///
/// # Errors
///
/// Returns [`BattleError::UnsupportedAiFlags`] containing only the unsupported
/// bits.
pub(crate) const fn ensure_supported_flags(flags: AiFlags) -> Result<(), BattleError> {
    let unsupported_bits = flags.bits() & !SUPPORTED_AI_FLAGS;
    if unsupported_bits == 0 {
        Ok(())
    } else {
        Err(BattleError::UnsupportedAiFlags(AiFlags(unsupported_bits)))
    }
}

struct MoveScores {
    values: [i8; MAX_MON_MOVES],
    simulated_damage_percent: [u32; MAX_MON_MOVES],
}

impl MoveScores {
    fn initialize(enemy: &BattlePokemon, rng: &mut impl BattleRng) -> Self {
        let mut values = [UNUSABLE_MOVE_SCORE; MAX_MON_MOVES];
        for (slot, score) in values.iter_mut().enumerate() {
            if move_slot_can_be_scored(enemy, slot) {
                *score = INITIAL_MOVE_SCORE;
            }
        }

        let mut simulated_damage_percent = [0; MAX_MON_MOVES];
        for percent in &mut simulated_damage_percent {
            *percent = PERCENT_SCALE - u32::from(rng.next_u16() % SIMULATED_DAMAGE_VARIANCE);
        }

        Self {
            values,
            simulated_damage_percent,
        }
    }

    /// Emerald adds score bytes as wrapping `i8` values before flooring a
    /// negative result at zero (`src/battle_ai_script_commands.c:703`).
    fn adjust(&mut self, slot: usize, delta: i8) {
        self.values[slot] = self.values[slot]
            .wrapping_add(delta)
            .max(UNUSABLE_MOVE_SCORE);
    }

    fn discard(&mut self, slot: usize) {
        self.values[slot] = UNUSABLE_MOVE_SCORE;
    }
}

#[derive(Debug, Clone, Copy)]
enum ScoringScript {
    CheckBadMove,
    TryToFaint,
    CheckViability,
    SetupFirstTurn,
}

const SCORING_SCRIPTS_IN_FLAG_ORDER: [(AiFlags, ScoringScript); 4] = [
    (AiFlags::CHECK_BAD_MOVE, ScoringScript::CheckBadMove),
    (AiFlags::TRY_TO_FAINT, ScoringScript::TryToFaint),
    (AiFlags::CHECK_VIABILITY, ScoringScript::CheckViability),
    (AiFlags::SETUP_FIRST_TURN, ScoringScript::SetupFirstTurn),
];

/// Chooses a trainer opponent's action after validating its AI flags.
///
/// Scoring consumes four simulated-damage draws, runs enabled scripts in
/// ascending flag order, and consumes one final tie-break draw. A two-ability
/// `CheckBadMove` target is refused before any draw (see the module docs). A trainer
/// with no usable move returns Struggle without drawing. This entry has no
/// party, so it never runs the voluntary-switch probes; battles with a bench
/// use [`choose_trainer_action_in_battle`].
///
/// # Errors
///
/// Returns an unsupported flag, move, or move-effect error if the caller did
/// not run the trainer admission checks first, or
/// [`BattleError::AmbiguousPlayerAbility`] for a two-ability `player` scored by
/// `CheckBadMove`.
#[cfg(test)]
pub(crate) fn choose_trainer_action(
    dex: &Dex,
    enemy: &BattlePokemon,
    player: &BattlePokemon,
    ai_flags: AiFlags,
    turn_counter: u8,
    rng: &mut impl BattleRng,
) -> Result<EnemyAction, BattleError> {
    choose_trainer_action_in_battle(dex, enemy, player, ai_flags, turn_counter, None, rng)
}

/// [`choose_trainer_action`] after the switch half of `AI_TrySwitchOrUseItem`.
///
/// A selected bench slot returns before Struggle, score admission, the
/// `CheckBadMove` ability refusal, and the four simulated-damage draws. No
/// healthy bench member skips every probe without drawing.
///
/// # Errors
///
/// Returns the errors of [`choose_trainer_action`], or
/// [`BattleError::UndefinedNaturalCureMove`] when Natural Cure would read
/// `MOVE_UNAVAILABLE`.
pub(crate) fn choose_trainer_action_in_battle(
    dex: &Dex,
    enemy: &BattlePokemon,
    player: &BattlePokemon,
    ai_flags: AiFlags,
    turn_counter: u8,
    switch_view: Option<voluntary_switch::VoluntarySwitch<'_>>,
    rng: &mut impl BattleRng,
) -> Result<EnemyAction, BattleError> {
    ensure_supported_flags(ai_flags)?;
    if let Some(slot) =
        voluntary_switch::try_voluntary_switch(dex, enemy, player, switch_view, rng)?
    {
        return Ok(EnemyAction::Switch(slot));
    }
    score_trainer_action(dex, enemy, player, ai_flags, turn_counter, rng)
}

fn score_trainer_action(
    dex: &Dex,
    enemy: &BattlePokemon,
    player: &BattlePokemon,
    ai_flags: AiFlags,
    turn_counter: u8,
    rng: &mut impl BattleRng,
) -> Result<EnemyAction, BattleError> {
    ensure_supported_flags(ai_flags)?;

    if all_known_moves_are_spent(enemy) {
        return Ok(EnemyAction::Struggle);
    }
    for slot in 0..MAX_MON_MOVES {
        if move_slot_can_be_scored(enemy, slot) {
            ensure_scoreable(dex, enemy.moves()[slot].move_id, ai_flags)?;
        }
    }
    if ai_flags.contains(AiFlags::CHECK_BAD_MOVE) {
        ensure_target_ability_unambiguous(player)?;
    }

    let mut scores = MoveScores::initialize(enemy, rng);
    for (flag, script) in SCORING_SCRIPTS_IN_FLAG_ORDER {
        if !ai_flags.contains(flag) {
            continue;
        }

        for slot in 0..MAX_MON_MOVES {
            if !move_slot_can_be_scored(enemy, slot) {
                scores.discard(slot);
                continue;
            }

            let move_id = enemy.moves()[slot].move_id;
            match script {
                ScoringScript::CheckBadMove => {
                    score_bad_move(dex, &mut scores, slot, move_id, enemy, player)?;
                }
                ScoringScript::TryToFaint => {
                    score_try_to_faint(dex, &mut scores, slot, move_id, enemy, player, rng)?;
                }
                ScoringScript::CheckViability => {
                    score_viability(dex, &mut scores, slot, move_id, enemy, player, rng)?;
                }
                ScoringScript::SetupFirstTurn => {
                    score_first_turn_setup(dex, &mut scores, slot, move_id, turn_counter, rng)?;
                }
            }
        }
    }

    let selected_slot = select_highest_scoring_move(enemy, scores.values, rng);
    Ok(EnemyAction::Move(selected_slot))
}

fn move_slot_can_be_scored(pokemon: &BattlePokemon, slot: usize) -> bool {
    selectable_slot(pokemon.move_at(slot)) && pokemon.moves()[slot].pp > 0
}

/// Deliberately differs from `battle_ai_script_commands.c:423-429` (slot 0 admitted
/// unconditionally, later slots only on `MOVE_NONE`): only PP-positive real moves tie (#1865).
fn select_highest_scoring_move(
    enemy: &BattlePokemon,
    scores: [i8; MAX_MON_MOVES],
    rng: &mut impl BattleRng,
) -> usize {
    let mut highest_score = i8::MIN;
    let mut highest_scoring_slots = Vec::new();

    for (slot, score) in scores.iter().copied().enumerate() {
        if !move_slot_can_be_scored(enemy, slot) {
            continue;
        }
        if score == highest_score {
            highest_scoring_slots.push(slot);
        }
        if score > highest_score {
            highest_score = score;
            highest_scoring_slots.clear();
            highest_scoring_slots.push(slot);
        }
    }

    let selected = usize::from(rng.next_u16()) % highest_scoring_slots.len();
    highest_scoring_slots[selected]
}

fn current_hp_percent(pokemon: &BattlePokemon) -> u32 {
    PERCENT_SCALE * pokemon.current_hp() / pokemon.stats().max_hp
}

fn random_roll_is_at_least(rng: &mut impl BattleRng, threshold: u16) -> bool {
    rng.next_u16() % AI_RANDOM_ROLL_MODULUS >= threshold
}

/// Preserves Emerald's dual non-immunity glitch: its AI type command folds
/// rows without carrying an immunity flag, so a later non-immune row can lift
/// zero damage back to one (`src/battle_ai_script_commands.c:1515`-`:1556`).
fn fold_ai_type_effectiveness(damage: u32, move_type: Type, defender_types: [Type; 2]) -> u32 {
    let defender_has_two_types = defender_types[1] != defender_types[0];
    let mut damage = damage;

    for &(attacker_type, defender_type, effectiveness) in TypeChart::rows() {
        if attacker_type != move_type {
            continue;
        }
        let applies_to_defender = defender_type == defender_types[0]
            || (defender_has_two_types && defender_type == defender_types[1]);
        if applies_to_defender {
            damage = apply_type_effectiveness(damage, effectiveness);
        }
    }

    damage
}

fn ai_type_effectiveness(
    dex: &Dex,
    move_id: MoveId,
    attacker: &BattlePokemon,
    defender: &BattlePokemon,
) -> Result<Option<u32>, BattleError> {
    let Some(move_type) = dex.move_data(move_id)?.move_type.battle_type() else {
        return Ok(None);
    };
    let neutral_with_stab = apply_stab(
        AI_EFFECTIVENESS_NEUTRAL,
        has_stab(attacker.types(), move_id, move_type),
    );
    Ok(Some(fold_ai_type_effectiveness(
        neutral_with_stab,
        move_type,
        defender.types(),
    )))
}

fn is_quadruple_effective(
    dex: &Dex,
    move_id: MoveId,
    attacker: &BattlePokemon,
    defender: &BattlePokemon,
) -> Result<bool, BattleError> {
    let Some(effectiveness) = ai_type_effectiveness(dex, move_id, attacker, defender)? else {
        return Ok(false);
    };
    Ok(matches!(
        effectiveness,
        AI_EFFECTIVENESS_QUADRUPLE | AI_EFFECTIVENESS_QUADRUPLE_WITH_STAB
    ))
}

fn has_no_effect(
    dex: &Dex,
    move_id: MoveId,
    attacker: &BattlePokemon,
    defender: &BattlePokemon,
) -> Result<bool, BattleError> {
    Ok(ai_type_effectiveness(dex, move_id, attacker, defender)? == Some(0))
}

/// The trapping abilities upstream reads off the target directly, ahead of
/// the two-slot guess and with no draw
/// (`src/battle_ai_script_commands.c:1368-1376`).
const TRAPPING_ABILITIES: [AbilityId; 3] = [
    AbilityId::SHADOW_TAG,
    AbilityId::MAGNET_PULL,
    AbilityId::ARENA_TRAP,
];

/// Refuses a scoring target whose species has two ability slots and whose
/// true ability is not a trapping one, because upstream's ability read for it
/// depends on battle history or a random guess the engine does not model
/// (`src/battle_ai_script_commands.c:1350-1396`). A true Shadow Tag, Magnet
/// Pull, or Arena Trap is admitted: upstream returns it before the guess
/// (`src/battle_ai_script_commands.c:1368-1376`).
///
/// # Errors
///
/// Returns [`BattleError::AmbiguousPlayerAbility`] for a two-ability species
/// whose selected ability is not a trapping one.
pub(crate) fn ensure_target_ability_unambiguous(target: &BattlePokemon) -> Result<(), BattleError> {
    let [_, second] = target.ability_slots();
    if second.0 == 0 || TRAPPING_ABILITIES.contains(&target.ability()) {
        Ok(())
    } else {
        Err(BattleError::AmbiguousPlayerAbility(target.species()))
    }
}

/// `Cmd_get_ability`'s target-ability read for the AI, restricted to the
/// targets admission allows: a true trapping ability, read directly
/// (`src/battle_ai_script_commands.c:1368-1376`), or a single-ability
/// species' sole ability. No history lookup, no draw.
///
/// # Errors
///
/// Returns [`BattleError::AmbiguousPlayerAbility`] for a two-ability target
/// whose selected ability is not a trapping one.
fn ai_get_ability(target: &BattlePokemon) -> Result<AbilityId, BattleError> {
    ensure_target_ability_unambiguous(target)?;
    Ok(target.ability())
}

/// The nine moves `AI_CheckBadMove` compares against Soundproof
/// (`data/battle_ai_scripts.s:95-103`); unlike execution's `SOUND_MOVES` in
/// `stat_change.rs`, this list omits Hyper Voice.
const AI_SOUND_MOVES: [MoveId; 9] = [
    MoveId::GROWL,
    MoveId::ROAR,
    MoveId::SING,
    MoveId::SUPERSONIC,
    MoveId::SCREECH,
    MoveId::SNORE,
    MoveId::UPROAR,
    MoveId::METAL_SOUND,
    MoveId::GRASS_WHISTLE,
];

/// Whether `AI_CheckBadMove` discourages `move_id` for Soundproof, given the
/// target ability read (`data/battle_ai_scripts.s:92-103`).
///
/// # Errors
///
/// Propagates a missing move entry from `dex`.
fn ai_soundproof_discourages(
    dex: &Dex,
    move_id: MoveId,
    target_ability: AbilityId,
) -> Result<bool, BattleError> {
    dex.move_data(move_id)?;
    Ok(target_ability == SOUNDPROOF && AI_SOUND_MOVES.contains(&move_id))
}

/// `AI_CBM_AttackDown`'s own `get_ability` call, before the shared Clear
/// Body/White Smoke check (`data/battle_ai_scripts.s:279-280`).
fn ai_hyper_cutter_blocks(target_ability: AbilityId) -> bool {
    target_ability == HYPER_CUTTER
}

/// `CheckIfAbilityBlocksStatChange`'s `get_ability` call, shared by
/// `AI_CBM_AttackDown` and `AI_CBM_DefenseDown`
/// (`data/battle_ai_scripts.s:308-311`).
fn ai_clear_body_or_white_smoke_blocks(target_ability: AbilityId) -> bool {
    target_ability == CLEAR_BODY || target_ability == WHITE_SMOKE
}

/// Unlike [`crate::hit::defender_wonder_guard_blocked`], only an
/// exactly-super-effective hit escapes here (`data/battle_ai_scripts.s:82-84`,
/// `src/battle_ai_script_commands.c:1528-1556`).
fn ai_wonder_guard_blocks(
    dex: &Dex,
    move_id: MoveId,
    attacker: &BattlePokemon,
    defender: &BattlePokemon,
    target_ability: AbilityId,
) -> Result<bool, BattleError> {
    if target_ability != AbilityId::WONDER_GUARD {
        return Ok(false);
    }
    Ok(!matches!(
        ai_type_effectiveness(dex, move_id, attacker, defender)?,
        Some(AI_EFFECTIVENESS_SUPER | AI_EFFECTIVENESS_SUPER_WITH_STAB)
    ))
}

/// `AI_CheckBadMove`'s prologue up to its `if_effect` chain
/// (`data/battle_ai_scripts.s:51-103`). Returns whether the script terminated
/// with a penalty. A move whose power the AI does not rank skips straight to
/// Soundproof, except Fissure and Horn Drill, which always run the checks. No
/// step draws: the target's ability is read without a guess.
fn score_bad_move_prologue(
    dex: &Dex,
    scores: &mut MoveScores,
    slot: usize,
    move_id: MoveId,
    user: &BattlePokemon,
    target: &BattlePokemon,
) -> Result<bool, BattleError> {
    let move_data = dex.move_data(move_id)?;
    let runs_negation_checks = move_id == MoveId::FISSURE
        || move_id == MoveId::HORN_DRILL
        || move_is_power_comparable(move_data);

    if runs_negation_checks {
        if has_no_effect(dex, move_id, user, target)? {
            scores.adjust(slot, STRONGLY_DISCOURAGE);
            return Ok(true);
        }
        let ability = ai_get_ability(target)?;
        let move_type = move_data.move_type.battle_type();
        let penalty = if ability == AbilityId::VOLT_ABSORB && move_type == Some(Type::Electric)
            || ability == AbilityId::WATER_ABSORB && move_type == Some(Type::Water)
            || ability == AbilityId::FLASH_FIRE && move_type == Some(Type::Fire)
        {
            Some(ABSORBED_MOVE_PENALTY)
        } else if ability == AbilityId::WONDER_GUARD
            && ai_wonder_guard_blocks(dex, move_id, user, target, ability)?
            || move_type.is_some_and(|move_type| defender_levitate_blocked(move_type, ability))
        {
            Some(STRONGLY_DISCOURAGE)
        } else {
            None
        };
        if let Some(penalty) = penalty {
            scores.adjust(slot, penalty);
            return Ok(true);
        }
    }

    // Soundproof reads the ability at its own call site
    // (`data/battle_ai_scripts.s:92-103`).
    let soundproof_ability = ai_get_ability(target)?;
    if ai_soundproof_discourages(dex, move_id, soundproof_ability)? {
        scores.adjust(slot, STRONGLY_DISCOURAGE);
        return Ok(true);
    }
    Ok(false)
}

fn estimated_damage(
    dex: &Dex,
    move_id: MoveId,
    attacker: &BattlePokemon,
    defender: &BattlePokemon,
    simulated_damage_percent: u32,
) -> Result<u32, BattleError> {
    let move_data = dex.move_data(move_id)?;
    let Some(move_type) = move_data.move_type.battle_type() else {
        return Ok(MINIMUM_DAMAGE);
    };
    let category = MoveCategory::for_type(move_type);
    let (attack_stat, attack_stage) = attacker.attacking_stat(category);
    let attack_stat = crate::ability::huge_power_attack(attacker.ability(), category, attack_stat);
    // Upstream's `CalculateBaseDamage` halves raw Special Attack for Fire/Ice
    // against Thick Fat before stage scaling (`pokemon.c:3202-3204`); the AI
    // estimates every populated slot, depleted or not.
    let attack_stat =
        crate::ability::thick_fat_special_attack(defender.ability(), move_type, attack_stat);
    // Plus/Minus raise raw Special Attack 150% before stages when the other
    // active mon has the complement (`pokemon.c:3207-3210`).
    let attack_stat = crate::ability::plus_minus_special_attack(
        attacker.ability(),
        defender.ability(),
        category,
        attack_stat,
    );
    let (defense_stat, defense_stage) = defender.defending_stat(category);
    let input = DamageInput {
        attacker_level: attacker.level(),
        power: u32::from(move_data.power),
        move_type,
        attack_stat,
        attack_stage,
        defense_stat,
        defense_stage,
        attacker_burned: false,
        reflect: false,
        light_screen: false,
        weather: Weather::None,
        is_solar_beam: false,
        attacker_pinch_boost: crate::ability::pinch_boosts_power(
            attacker.ability(),
            move_type,
            attacker.current_hp(),
            attacker.stats().max_hp,
        ),
    };
    let damage = base_damage(&input);
    // `TypeCalc` returns for Struggle before STAB and the chart (`battle_script_commands.c:1541`);
    // the caller's percentage and floor still apply (`battle_ai_script_commands.c:1208`-`:1213`).
    if move_id == MoveId::STRUGGLE {
        return Ok((damage * simulated_damage_percent / PERCENT_SCALE).max(MINIMUM_DAMAGE));
    }
    let damage = apply_stab(damage, has_stab(attacker.types(), move_id, move_type));
    let damage = if defender_levitate_blocked(move_type, defender.ability()) {
        damage
    } else {
        fold_ai_type_effectiveness(damage, move_type, defender.types())
    };
    Ok((damage * simulated_damage_percent / PERCENT_SCALE).max(MINIMUM_DAMAGE))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PowerComparison {
    NotComparable,
    WeakerThanAnotherMove,
    Strongest,
}

/// Whether `get_how_powerful_move_is` ranks the considered move rather than
/// answering `MOVE_POWER_OTHER`: power above one and not an ignored effect
/// (`src/battle_ai_script_commands.c:1186-1189`).
fn move_is_power_comparable(move_data: &assets::MoveData) -> bool {
    move_data.power >= MINIMUM_DAMAGING_MOVE_POWER
        && !is_ignored_powerful_move_effect(move_data.effect)
}

fn compare_move_power(
    dex: &Dex,
    scores: &MoveScores,
    considered_slot: usize,
    move_id: MoveId,
    attacker: &BattlePokemon,
    defender: &BattlePokemon,
) -> Result<PowerComparison, BattleError> {
    if !move_is_power_comparable(dex.move_data(move_id)?) {
        return Ok(PowerComparison::NotComparable);
    }

    let mut damage_by_slot = [0; MAX_MON_MOVES];
    for (slot, damage) in damage_by_slot.iter_mut().enumerate() {
        let Some(candidate_move) = attacker.move_at(slot) else {
            continue;
        };
        let candidate_data = dex.move_data(candidate_move)?;
        if candidate_data.power < MINIMUM_DAMAGING_MOVE_POWER
            || is_ignored_powerful_move_effect(candidate_data.effect)
        {
            continue;
        }
        *damage = estimated_damage(
            dex,
            candidate_move,
            attacker,
            defender,
            scores.simulated_damage_percent[slot],
        )?;
    }

    if damage_by_slot
        .iter()
        .any(|damage| *damage > damage_by_slot[considered_slot])
    {
        Ok(PowerComparison::WeakerThanAnotherMove)
    } else {
        Ok(PowerComparison::Strongest)
    }
}

fn score_bad_move(
    dex: &Dex,
    scores: &mut MoveScores,
    slot: usize,
    move_id: MoveId,
    user: &BattlePokemon,
    target: &BattlePokemon,
) -> Result<(), BattleError> {
    let effect = scoreable_effect(dex, move_id)?;

    if score_bad_move_prologue(dex, scores, slot, move_id, user, target)? {
        return Ok(());
    }

    let penalty = match effect {
        ScoreableEffect::Hit | ScoreableEffect::UnchangedAfterPrologue(_) => None,
        ScoreableEffect::AttackDown => stat_label_penalty(
            user,
            target,
            StatChangeEffect {
                stat: ChangedStat::Attack,
                magnitude: StatChangeMagnitude::One,
                direction: StatChangeDirection::Lower,
            },
        )?,
        ScoreableEffect::DefenseDown => stat_label_penalty(
            user,
            target,
            StatChangeEffect {
                stat: ChangedStat::Defense,
                magnitude: StatChangeMagnitude::One,
                direction: StatChangeDirection::Lower,
            },
        )?,
        ScoreableEffect::StatLabel(_, label) => stat_label_penalty(user, target, label)?,
        ScoreableEffect::Paralyze => paralyze_penalty(dex, move_id, user, target)?,
        ScoreableEffect::Confuse => confuse_penalty(target)?,
        ScoreableEffect::FocusEnergy => {
            user.volatiles().focus_energy.then_some(STRONGLY_DISCOURAGE)
        }
        ScoreableEffect::HighRiskForDamage => {
            high_risk_for_damage_penalty(dex, move_id, user, target)?
        }
    };
    if let Some(penalty) = penalty {
        scores.adjust(slot, penalty);
    }
    Ok(())
}

/// `AI_CBM_*Up` / `AI_CBM_*Down` (`data/battle_ai_scripts.s:248-312`): the
/// first guard that fires ends the script.
fn stat_label_penalty(
    user: &BattlePokemon,
    target: &BattlePokemon,
    label: StatChangeEffect,
) -> Result<Option<i8>, BattleError> {
    let holder = if label.affects_user() { user } else { target };
    if stage_of(holder, label.stat) == label.cap() {
        return Ok(Some(STRONGLY_DISCOURAGE));
    }
    if label.affects_user() {
        return Ok(None);
    }
    let blocked = stat_specific_drop_guard(target, label.stat)?
        || ai_clear_body_or_white_smoke_blocks(ai_get_ability(target)?);
    Ok(blocked.then_some(STRONGLY_DISCOURAGE))
}

/// The guard a drop label runs before `CheckIfAbilityBlocksStatChange`
/// (`data/battle_ai_scripts.s:277-281,287-290,300-304`).
fn stat_specific_drop_guard(
    target: &BattlePokemon,
    stat: ChangedStat,
) -> Result<bool, BattleError> {
    Ok(match stat {
        ChangedStat::Attack => ai_hyper_cutter_blocks(ai_get_ability(target)?),
        ChangedStat::Speed => ai_get_ability(target)? == AbilityId::SPEED_BOOST,
        ChangedStat::Accuracy => ai_get_ability(target)? == KEEN_EYE,
        _ => false,
    })
}

/// `AI_CBM_Paralyze`: each guard ends the script with its own single penalty,
/// so a type-immune target never also takes the Limber or status `-10`
/// (`data/battle_ai_scripts.s:397-403`). Safeguard is not modelled, and no
/// admitted move can set it.
fn paralyze_penalty(
    dex: &Dex,
    move_id: MoveId,
    user: &BattlePokemon,
    target: &BattlePokemon,
) -> Result<Option<i8>, BattleError> {
    if has_no_effect(dex, move_id, user, target)?
        || ai_get_ability(target)? == AbilityId::LIMBER
        || target.status1() != Status1::Healthy
    {
        Ok(Some(STRONGLY_DISCOURAGE))
    } else {
        Ok(None)
    }
}

/// `AI_CBM_HighRiskForDamage`: a chart-immune target or a Wonder Guard target
/// the move does not hit for exactly double damage ends at `-10`
/// (`data/battle_ai_scripts.s:368-376`). Unlike the prologue, it runs for
/// power-1 moves too, and it draws nothing.
fn high_risk_for_damage_penalty(
    dex: &Dex,
    move_id: MoveId,
    user: &BattlePokemon,
    target: &BattlePokemon,
) -> Result<Option<i8>, BattleError> {
    let blocked = has_no_effect(dex, move_id, user, target)?
        || ai_wonder_guard_blocks(dex, move_id, user, target, ai_get_ability(target)?)?;
    Ok(blocked.then_some(STRONGLY_DISCOURAGE))
}

/// `AI_CBM_Confuse`: active confusion ends at `-5` before any ability read;
/// Own Tempo is `-10` (`data/battle_ai_scripts.s:386-395`). Safeguard is not
/// modelled.
fn confuse_penalty(target: &BattlePokemon) -> Result<Option<i8>, BattleError> {
    if target.volatiles().confused() {
        Ok(Some(REDUNDANT_CONFUSION_PENALTY))
    } else if ai_get_ability(target)? == AbilityId::OWN_TEMPO {
        Ok(Some(STRONGLY_DISCOURAGE))
    } else {
        Ok(None)
    }
}

fn score_try_to_faint(
    dex: &Dex,
    scores: &mut MoveScores,
    slot: usize,
    move_id: MoveId,
    user: &BattlePokemon,
    target: &BattlePokemon,
    rng: &mut impl BattleRng,
) -> Result<(), BattleError> {
    let move_data = dex.move_data(move_id)?;
    if move_data.power >= MINIMUM_DAMAGING_MOVE_POWER {
        let damage = estimated_damage(
            dex,
            move_id,
            user,
            target,
            scores.simulated_damage_percent[slot],
        )?;
        if target.current_hp() <= damage {
            scores.adjust(slot, STRONGLY_ENCOURAGE);
            return Ok(());
        }
    }

    if compare_move_power(dex, scores, slot, move_id, user, target)?
        == PowerComparison::WeakerThanAnotherMove
    {
        scores.adjust(slot, SLIGHTLY_DISCOURAGE);
        return Ok(());
    }

    if is_quadruple_effective(dex, move_id, user, target)?
        && random_roll_is_at_least(rng, QUADRUPLE_EFFECTIVENESS_BONUS_THRESHOLD)
    {
        scores.adjust(slot, ENCOURAGE);
    }
    Ok(())
}

fn score_viability(
    dex: &Dex,
    scores: &mut MoveScores,
    slot: usize,
    move_id: MoveId,
    user: &BattlePokemon,
    target: &BattlePokemon,
    rng: &mut impl BattleRng,
) -> Result<(), BattleError> {
    match scoreable_effect(dex, move_id)? {
        ScoreableEffect::Hit
        | ScoreableEffect::UnchangedAfterPrologue(_)
        | ScoreableEffect::FocusEnergy => {}
        ScoreableEffect::HighRiskForDamage => {
            return Err(BattleError::UnscoreableMoveEffect(move_id))
        }
        // Defense Curl is absent from `AI_CheckViability`'s dispatch
        // (`data/battle_ai_scripts.s:652-776`); every other label is unported.
        ScoreableEffect::StatLabel(effect, _) if effect == MoveEffect::DEFENSE_CURL => {}
        ScoreableEffect::Paralyze | ScoreableEffect::Confuse | ScoreableEffect::StatLabel(..) => {
            return Err(BattleError::UnscoreableMoveEffect(move_id));
        }
        ScoreableEffect::AttackDown => {
            score_attack_drop_viability(scores, slot, user, target, rng);
        }
        ScoreableEffect::DefenseDown => {
            score_defense_drop_viability(scores, slot, user, target, rng);
        }
    }
    Ok(())
}

/// Exact six-entry `AI_CV_AttackDown_PhysicalTypeList`. Emerald omits Flying,
/// Poison, and Ghost (`data/battle_ai_scripts.s:1115`-`:1121`).
const PHYSICAL_TYPE_LIST: [Type; 6] = [
    Type::Normal,
    Type::Fighting,
    Type::Ground,
    Type::Rock,
    Type::Bug,
    Type::Steel,
];

fn has_physical_type(pokemon: &BattlePokemon) -> bool {
    pokemon
        .types()
        .iter()
        .any(|pokemon_type| PHYSICAL_TYPE_LIST.contains(pokemon_type))
}

fn score_attack_drop_viability(
    scores: &mut MoveScores,
    slot: usize,
    user: &BattlePokemon,
    target: &BattlePokemon,
    rng: &mut impl BattleRng,
) {
    let attack_stage = target.stages().attack;
    if attack_stage != StatStage::NEUTRAL {
        scores.adjust(slot, SLIGHTLY_DISCOURAGE);
        if current_hp_percent(user) <= ATTACK_DROP_USER_HP_THRESHOLD {
            scores.adjust(slot, SLIGHTLY_DISCOURAGE);
        }
        if attack_stage.offset() <= HEAVILY_LOWERED_STAT_STAGE
            && random_roll_is_at_least(rng, STAT_DROP_DISCOURAGEMENT_THRESHOLD)
        {
            scores.adjust(slot, DISCOURAGE);
        }
    }

    if current_hp_percent(target) <= STAT_DROP_TARGET_HP_THRESHOLD {
        scores.adjust(slot, DISCOURAGE);
    }
    if !has_physical_type(target)
        && random_roll_is_at_least(rng, STAT_DROP_DISCOURAGEMENT_THRESHOLD)
    {
        scores.adjust(slot, DISCOURAGE);
    }
}

fn score_defense_drop_viability(
    scores: &mut MoveScores,
    slot: usize,
    user: &BattlePokemon,
    target: &BattlePokemon,
    rng: &mut impl BattleRng,
) {
    let user_is_healthy = current_hp_percent(user) >= DEFENSE_DROP_USER_HP_THRESHOLD;
    let defense_is_not_heavily_lowered =
        target.stages().defense.offset() > HEAVILY_LOWERED_STAT_STAGE;
    let skip_discouragement_roll = user_is_healthy && defense_is_not_heavily_lowered;

    if !skip_discouragement_roll && random_roll_is_at_least(rng, STAT_DROP_DISCOURAGEMENT_THRESHOLD)
    {
        scores.adjust(slot, DISCOURAGE);
    }
    if current_hp_percent(target) <= STAT_DROP_TARGET_HP_THRESHOLD {
        scores.adjust(slot, DISCOURAGE);
    }
}

fn score_first_turn_setup(
    dex: &Dex,
    scores: &mut MoveScores,
    slot: usize,
    move_id: MoveId,
    turn_counter: u8,
    rng: &mut impl BattleRng,
) -> Result<(), BattleError> {
    if turn_counter != FIRST_TURN {
        return Ok(());
    }
    if !scoreable_effect(dex, move_id)?.is_stat_drop() {
        return Ok(());
    }
    if random_roll_is_at_least(rng, FIRST_TURN_SETUP_BONUS_THRESHOLD) {
        scores.adjust(slot, ENCOURAGE);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        choose_trainer_action, compare_move_power, ensure_scoreable, ensure_supported_flags,
        estimated_damage, score_try_to_faint, select_highest_scoring_move, MoveScores,
        PowerComparison, FIRST_TURN, FIRST_TURN_SETUP_BONUS_THRESHOLD, PERCENT_SCALE,
        STAT_DROP_DISCOURAGEMENT_THRESHOLD,
    };
    use crate::battle::opponent_ai::EnemyAction;
    use crate::dex::Dex;
    use crate::error::BattleError;
    use crate::pokemon::{BattlePokemon, Ivs, MAX_MON_MOVES};
    use crate::script_rng::SequenceRng;
    use assets::trainers::AiFlags;
    use assets::{MoveId, SpeciesId};

    const ROUTE_103_LEVEL: u8 = 5;
    const DEFAULT_PERSONALITY: u32 = 0;
    const SECOND_TURN: u8 = 1;
    const HUGE_POWER_ABILITY_SLOT: u8 = 1;
    const GUTS_ABILITY_SLOT: u8 = 1;
    const HUSTLE_ABILITY_SLOT: u8 = 1;
    const MAXIMUM_SIMULATED_DAMAGE_DRAW: u16 = 0;
    const SELECT_FIRST_TIED_MOVE: u16 = 0;
    const SELECT_SECOND_TIED_MOVE: u16 = 1;
    const MAXIMUM_DAMAGE_ROLL: u16 = 0;

    const TREECKO: SpeciesId = SpeciesId(277);
    const TORCHIC: SpeciesId = SpeciesId(280);
    const MUDKIP: SpeciesId = SpeciesId(283);
    const MARILL: SpeciesId = SpeciesId(183);
    const SQUIRTLE: SpeciesId = SpeciesId(7);
    /// `SPECIES_MAKUHITA`: Guts in ability slot 1 (Thick Fat is slot 0).
    const MAKUHITA: SpeciesId = SpeciesId(335);
    /// `SPECIES_MILOTIC`: Marvel Scale in its primary (and only) ability slot.
    const MILOTIC: SpeciesId = SpeciesId(329);
    /// `SPECIES_DELIBIRD`: Vital Spirit in slot 0, Hustle in slot 1 -- unlike
    /// a single-ability species, a same-species non-Hustle control is
    /// reachable through `with_ability_slot`.
    const DELIBIRD: SpeciesId = SpeciesId(225);
    /// `SPECIES_SHEDINJA`: Wonder Guard in its only ability slot.
    const SHEDINJA: SpeciesId = SpeciesId(303);
    /// `SPECIES_VOLTORB`: Soundproof in slot 0 (Static is slot 1).
    const VOLTORB: SpeciesId = SpeciesId(100);
    /// `SPECIES_PINSIR`: Hyper Cutter is its only ability.
    const PINSIR: SpeciesId = SpeciesId(127);
    /// `SPECIES_REGIROCK`: Clear Body is its only ability.
    const REGIROCK: SpeciesId = SpeciesId(401);
    /// `SPECIES_MR_MIME`: Soundproof is its only ability.
    const MR_MIME: SpeciesId = SpeciesId(122);
    /// `SPECIES_GASTLY`: Levitate in its only ability slot.
    const GASTLY: SpeciesId = SpeciesId(92);
    /// `SPECIES_TRAPINCH`: Hyper Cutter in slot 0, Arena Trap (a trapping
    /// ability) in slot 1.
    const TRAPINCH: SpeciesId = SpeciesId(332);

    const POUND: MoveId = MoveId::POUND;
    const SCRATCH: MoveId = MoveId::SCRATCH;
    const WING_ATTACK: MoveId = MoveId::WING_ATTACK;
    const TACKLE: MoveId = MoveId::TACKLE;
    const LEER: MoveId = MoveId::LEER;
    const GROWL: MoveId = MoveId::GROWL;
    const EMBER: MoveId = MoveId::EMBER;
    const WATER_GUN: MoveId = MoveId::WATER_GUN;
    const EARTHQUAKE: MoveId = MoveId::EARTHQUAKE;
    /// `MOVE_HYPER_VOICE`: absent from the AI's own Soundproof list
    /// (`data/battle_ai_scripts.s:95-103`) despite belonging to execution's
    /// `SOUND_MOVES` (`crates/battle/src/stat_change.rs:83-94`).
    const HYPER_VOICE: MoveId = MoveId::HYPER_VOICE;

    const SEISMIC_TOSS: MoveId = MoveId::SEISMIC_TOSS;
    const SONIC_BOOM: MoveId = MoveId::SONIC_BOOM;
    /// Upstream draws one setup value per move slot, then one tie-break value
    /// (`src/battle_ai_script_commands.c:322-342,445`).
    const FLAGLESS_DRAWS: usize = MAX_MON_MOVES + 1;

    #[test]
    fn a_flagless_trainer_chooses_among_executable_fixed_damage_moves() {
        let dex = Dex::new();
        let mut enemy = pokemon(TREECKO, vec![SEISMIC_TOSS, SONIC_BOOM, POUND]);
        spend_move(&mut enemy, 2);
        let player = pokemon(VOLTORB, vec![POUND]);
        for (tie, slot) in [(0, 0), (1, 1), (2, 0)] {
            let mut rng = SequenceRng::new([0, 1, 2, 3, tie]);
            assert_eq!(
                choose_trainer_action(&dex, &enemy, &player, AiFlags::NONE, FIRST_TURN, &mut rng),
                Ok(EnemyAction::Move(slot))
            );
            assert_eq!(rng.draws(), FLAGLESS_DRAWS);
            rng.assert_exhausted();
        }
    }

    #[test]
    fn flagless_admission_still_requires_a_dex_resident_move() {
        let dex = Dex::new();
        assert_eq!(ensure_scoreable(&dex, SEISMIC_TOSS, AiFlags::NONE), Ok(()));
        assert!(ensure_scoreable(&dex, MoveId(u16::MAX), AiFlags::NONE).is_err());
    }

    #[test]
    fn scoring_flags_still_refuse_fixed_damage_moves() {
        let dex = Dex::new();
        for flags in [
            AiFlags::CHECK_BAD_MOVE,
            AiFlags::TRY_TO_FAINT,
            AiFlags::CHECK_VIABILITY,
            AiFlags::SETUP_FIRST_TURN,
        ] {
            assert_eq!(
                ensure_scoreable(&dex, SEISMIC_TOSS, flags),
                Err(BattleError::UnscoreableMoveEffect(SEISMIC_TOSS))
            );
        }
    }

    fn route_103_flags() -> AiFlags {
        AiFlags::CHECK_BAD_MOVE
            .union(AiFlags::TRY_TO_FAINT)
            .union(AiFlags::CHECK_VIABILITY)
    }

    fn first_turn_setup_flags() -> AiFlags {
        AiFlags::CHECK_BAD_MOVE
            .union(AiFlags::TRY_TO_FAINT)
            .union(AiFlags::SETUP_FIRST_TURN)
    }

    fn pokemon(species: SpeciesId, moves: Vec<MoveId>) -> BattlePokemon {
        BattlePokemon::new(
            &Dex::new(),
            species,
            ROUTE_103_LEVEL,
            Ivs::default(),
            DEFAULT_PERSONALITY,
            moves,
        )
        .expect("test Pokemon must be dex-resident")
    }

    fn spend_move(pokemon: &mut BattlePokemon, slot: usize) {
        while pokemon.moves()[slot].pp > 0 {
            pokemon.deduct_pp(slot).unwrap();
        }
    }

    fn rng_with_maximum_simulated_damage(
        subsequent_draws: impl IntoIterator<Item = u16>,
    ) -> SequenceRng {
        SequenceRng::new(
            [MAXIMUM_SIMULATED_DAMAGE_DRAW; MAX_MON_MOVES]
                .into_iter()
                .chain(subsequent_draws),
        )
    }

    /// `SPECIES_JYNX`: an Ice type whose Special Attack far exceeds its
    /// Attack, so a weak Ice move can rival a resisted physical one.
    const JYNX: SpeciesId = SpeciesId(124);

    /// Upstream's `AI_CalcDmg` runs `CalculateBaseDamage` for every nonempty
    /// slot, depleted or not (`battle_ai_script_commands.c:1204-1213`), and it
    /// halves raw Special Attack for Fire/Ice against Thick Fat
    /// (`pokemon.c:3202-3204`). Icicle Spear is the one Fire/Ice move that
    /// trainer admission accepts, so the depleted slot is reachable.
    #[test]
    fn depleted_ice_move_respects_thick_fat_in_power_comparison() {
        let dex = Dex::new();
        let moves = vec![MoveId::ROCK_THROW, GROWL, MoveId::ICICLE_SPEAR];
        for move_id in &moves {
            assert_eq!(
                crate::battle::trainer::ensure_move_playable(&dex, *move_id, AiFlags::TRY_TO_FAINT),
                Ok(()),
                "{move_id:?}"
            );
        }
        let build = |species, moves| {
            BattlePokemon::new(
                &dex,
                species,
                20,
                Ivs {
                    hp: 31,
                    attack: 31,
                    defense: 31,
                    speed: 31,
                    sp_attack: 31,
                    sp_defense: 31,
                },
                DEFAULT_PERSONALITY,
                moves,
            )
            .unwrap()
        };
        let mut enemy = build(JYNX, moves);
        spend_move(&mut enemy, 2);
        let player = build(MAKUHITA, vec![TACKLE]);
        assert_eq!(player.ability(), crate::ability::THICK_FAT);

        assert_eq!(
            estimated_damage(&dex, MoveId::ROCK_THROW, &enemy, &player, PERCENT_SCALE).unwrap(),
            7
        );
        assert_eq!(
            estimated_damage(&dex, MoveId::ICICLE_SPEAR, &enemy, &player, PERCENT_SCALE).unwrap(),
            6
        );

        let mut rng = rng_with_maximum_simulated_damage([]);
        let mut scores = MoveScores::initialize(&enemy, &mut rng);
        assert_eq!(
            compare_move_power(&dex, &scores, 0, MoveId::ROCK_THROW, &enemy, &player),
            Ok(PowerComparison::Strongest)
        );
        for slot in 0..2 {
            let move_id = enemy.moves()[slot].move_id;
            score_try_to_faint(&dex, &mut scores, slot, move_id, &enemy, &player, &mut rng)
                .unwrap();
        }
        assert_eq!(scores.values, [100, 100, 0, 0]);

        let mut rng = rng_with_maximum_simulated_damage([SELECT_FIRST_TIED_MOVE]);
        assert_eq!(
            choose_trainer_action(
                &dex,
                &enemy,
                &player,
                AiFlags::TRY_TO_FAINT,
                FIRST_TURN,
                &mut rng,
            )
            .unwrap(),
            EnemyAction::Move(0)
        );
        assert_eq!(rng.draws(), MAX_MON_MOVES + 1);
        rng.assert_exhausted();
    }

    #[test]
    fn exhausted_ground_slots_keep_prechart_damage_against_levitate() {
        let dex = Dex::new();
        let target = pokemon(GASTLY, vec![POUND]);
        assert_eq!(target.ability(), assets::AbilityId::LEVITATE);
        let cases = [
            (vec![WING_ATTACK, GROWL], EnemyAction::Move(0)),
            (
                vec![WING_ATTACK, GROWL, MoveId::MUD_SLAP],
                EnemyAction::Move(0),
            ),
            (vec![WING_ATTACK, GROWL, EARTHQUAKE], EnemyAction::Move(1)),
        ];
        for (moves, expected) in cases {
            let mut enemy = pokemon(TRAPINCH, moves);
            if enemy.moves().len() == 3 {
                spend_move(&mut enemy, 2);
            }
            let mut rng = rng_with_maximum_simulated_damage([SELECT_FIRST_TIED_MOVE]);
            let action = choose_trainer_action(
                &dex,
                &enemy,
                &target,
                AiFlags::TRY_TO_FAINT,
                FIRST_TURN,
                &mut rng,
            )
            .unwrap();
            assert_eq!(action, expected);
            assert_eq!(rng.draws(), MAX_MON_MOVES + 1);
        }
    }

    #[test]
    fn route_103_move_effects_are_scoreable_and_ember_is_not() {
        let dex = Dex::new();
        let flags = route_103_flags();

        assert!(ensure_scoreable(&dex, POUND, flags).is_ok());
        assert!(ensure_scoreable(&dex, GROWL, flags).is_ok());
        assert!(ensure_scoreable(&dex, LEER, flags).is_ok());
        assert_eq!(
            ensure_scoreable(&dex, EMBER, flags),
            Err(BattleError::UnscoreableMoveEffect(EMBER))
        );
    }

    #[test]
    fn only_the_four_lowest_ai_script_bits_are_supported() {
        assert!(ensure_supported_flags(route_103_flags()).is_ok());
        assert!(ensure_supported_flags(first_turn_setup_flags()).is_ok());
        assert_eq!(
            ensure_supported_flags(AiFlags::CHECK_BAD_MOVE.union(AiFlags::RISKY)),
            Err(BattleError::UnsupportedAiFlags(AiFlags::RISKY))
        );
    }

    #[test]
    fn every_spent_move_forces_struggle_without_drawing() {
        let mut enemy = pokemon(TORCHIC, vec![SCRATCH, GROWL]);
        for slot in 0..enemy.moves().len() {
            spend_move(&mut enemy, slot);
        }
        let player = pokemon(TREECKO, vec![POUND]);
        let mut rng = SequenceRng::new([]);

        assert_eq!(
            choose_trainer_action(
                &Dex::new(),
                &enemy,
                &player,
                route_103_flags(),
                FIRST_TURN,
                &mut rng,
            )
            .unwrap(),
            EnemyAction::Struggle
        );
        assert_eq!(rng.draws(), 0);
        rng.assert_exhausted();
    }

    #[test]
    fn opening_tackle_and_growl_cost_setup_viability_and_tie_break_draws() {
        let enemy = pokemon(MUDKIP, vec![TACKLE, GROWL]);
        let player = pokemon(TORCHIC, vec![SCRATCH]);
        let mut rng = rng_with_maximum_simulated_damage([
            STAT_DROP_DISCOURAGEMENT_THRESHOLD,
            SELECT_FIRST_TIED_MOVE,
        ]);

        let action = choose_trainer_action(
            &Dex::new(),
            &enemy,
            &player,
            route_103_flags(),
            FIRST_TURN,
            &mut rng,
        )
        .unwrap();

        assert_eq!(action, EnemyAction::Move(0));
        assert_eq!(rng.draws(), MAX_MON_MOVES + 2);
    }

    #[test]
    fn opening_leer_skips_the_viability_roll_and_ties_with_pound() {
        let enemy = pokemon(TREECKO, vec![POUND, LEER]);
        let player = pokemon(MUDKIP, vec![TACKLE]);
        let mut choose_pound = rng_with_maximum_simulated_damage([SELECT_FIRST_TIED_MOVE]);
        let mut choose_leer = rng_with_maximum_simulated_damage([SELECT_SECOND_TIED_MOVE]);

        assert_eq!(
            choose_trainer_action(
                &Dex::new(),
                &enemy,
                &player,
                route_103_flags(),
                FIRST_TURN,
                &mut choose_pound,
            )
            .unwrap(),
            EnemyAction::Move(0)
        );
        assert_eq!(choose_pound.draws(), MAX_MON_MOVES + 1);
        assert_eq!(
            choose_trainer_action(
                &Dex::new(),
                &enemy,
                &player,
                route_103_flags(),
                FIRST_TURN,
                &mut choose_leer,
            )
            .unwrap(),
            EnemyAction::Move(1)
        );
        assert_eq!(choose_leer.draws(), MAX_MON_MOVES + 1);
    }

    #[test]
    fn a_finishable_target_makes_tackle_outscore_growl() {
        let dex = Dex::new();
        let enemy = pokemon(MUDKIP, vec![TACKLE, GROWL]);
        let mut player = pokemon(TORCHIC, vec![SCRATCH]);
        let estimate = estimated_damage(&dex, TACKLE, &enemy, &player, PERCENT_SCALE).unwrap();
        player.apply_damage(player.stats().max_hp - estimate);
        assert_eq!(player.current_hp(), estimate);

        let mut rng = rng_with_maximum_simulated_damage([
            STAT_DROP_DISCOURAGEMENT_THRESHOLD,
            SELECT_FIRST_TIED_MOVE,
        ]);

        assert_eq!(
            choose_trainer_action(
                &dex,
                &enemy,
                &player,
                route_103_flags(),
                FIRST_TURN,
                &mut rng,
            )
            .unwrap(),
            EnemyAction::Move(0)
        );
    }

    #[test]
    fn setup_first_turn_scores_stat_moves_only_on_turn_one() {
        let enemy = pokemon(TREECKO, vec![POUND, LEER]);
        let player = pokemon(MUDKIP, vec![TACKLE]);
        let mut first_turn_rng = rng_with_maximum_simulated_damage([
            FIRST_TURN_SETUP_BONUS_THRESHOLD,
            SELECT_FIRST_TIED_MOVE,
        ]);

        assert_eq!(
            choose_trainer_action(
                &Dex::new(),
                &enemy,
                &player,
                first_turn_setup_flags(),
                FIRST_TURN,
                &mut first_turn_rng,
            )
            .unwrap(),
            EnemyAction::Move(1)
        );
        assert_eq!(first_turn_rng.draws(), MAX_MON_MOVES + 2);

        let mut second_turn_rng = rng_with_maximum_simulated_damage([SELECT_FIRST_TIED_MOVE]);
        assert_eq!(
            choose_trainer_action(
                &Dex::new(),
                &enemy,
                &player,
                first_turn_setup_flags(),
                SECOND_TURN,
                &mut second_turn_rng,
            )
            .unwrap(),
            EnemyAction::Move(0)
        );
        assert_eq!(second_turn_rng.draws(), MAX_MON_MOVES + 1);
    }

    #[test]
    fn a_spent_move_is_neither_scored_nor_selected() {
        let mut enemy = pokemon(MUDKIP, vec![TACKLE, GROWL]);
        spend_move(&mut enemy, 0);
        let player = pokemon(TORCHIC, vec![SCRATCH]);
        let mut rng = rng_with_maximum_simulated_damage([
            STAT_DROP_DISCOURAGEMENT_THRESHOLD,
            SELECT_FIRST_TIED_MOVE,
        ]);

        assert_eq!(
            choose_trainer_action(
                &Dex::new(),
                &enemy,
                &player,
                route_103_flags(),
                FIRST_TURN,
                &mut rng,
            )
            .unwrap(),
            EnemyAction::Move(1)
        );
        assert_eq!(rng.draws(), MAX_MON_MOVES + 2);
        rng.assert_exhausted();
    }

    #[test]
    fn zero_score_usable_move_excludes_spent_slot_zero() {
        let mut enemy = pokemon(MUDKIP, vec![TACKLE, GROWL]);
        spend_move(&mut enemy, 0);
        let mut rng = SequenceRng::new([SELECT_FIRST_TIED_MOVE]);

        assert_eq!(
            select_highest_scoring_move(&enemy, [0; MAX_MON_MOVES], &mut rng),
            1
        );
        assert_eq!(rng.draws(), 1);
        rng.assert_exhausted();
    }

    #[test]
    fn zero_score_tie_includes_only_pp_positive_real_moves() {
        let mut enemy = pokemon(MUDKIP, vec![TACKLE, GROWL, SCRATCH]);
        spend_move(&mut enemy, 2);

        for (draw, expected_slot) in [(2_u16, 0_usize), (3, 1)] {
            let mut rng = SequenceRng::new([draw]);
            assert_eq!(
                select_highest_scoring_move(&enemy, [0; MAX_MON_MOVES], &mut rng),
                expected_slot
            );
            assert_eq!(rng.draws(), 1);
            rng.assert_exhausted();
        }
    }

    #[test]
    fn huge_power_estimated_damage_matches_the_real_damage_step() {
        let dex = Dex::new();
        let attacker = BattlePokemon::new(
            &dex,
            MARILL,
            ROUTE_103_LEVEL,
            Ivs::default(),
            DEFAULT_PERSONALITY,
            vec![TACKLE],
        )
        .unwrap()
        .with_ability_slot(HUGE_POWER_ABILITY_SLOT);
        assert_eq!(attacker.ability(), crate::ability::HUGE_POWER);
        let defender = pokemon(SQUIRTLE, vec![TACKLE]);

        let critical_hits_suppressed = true;
        let mut no_draws = SequenceRng::new([]);
        let damage_before_roll = crate::hit::damage_before_roll(
            &dex,
            TACKLE,
            &attacker,
            &defender,
            critical_hits_suppressed,
            &mut no_draws,
        )
        .unwrap();
        let mut best_damage_roll = SequenceRng::new([MAXIMUM_DAMAGE_ROLL]);
        let real_damage =
            crate::damage::apply_damage_roll(damage_before_roll.damage, &mut best_damage_roll);
        let estimated_damage =
            estimated_damage(&dex, TACKLE, &attacker, &defender, PERCENT_SCALE).unwrap();

        assert_eq!(estimated_damage, real_damage);
    }

    #[test]
    fn hustle_estimated_damage_matches_the_real_damage_step() {
        let dex = Dex::new();
        let attacker = BattlePokemon::new(
            &dex,
            DELIBIRD,
            ROUTE_103_LEVEL,
            Ivs::default(),
            DEFAULT_PERSONALITY,
            vec![TACKLE],
        )
        .unwrap()
        .with_ability_slot(HUSTLE_ABILITY_SLOT);
        assert_eq!(attacker.ability(), crate::ability::HUSTLE);
        let defender = pokemon(SQUIRTLE, vec![TACKLE]);

        let control = BattlePokemon::new(
            &dex,
            DELIBIRD,
            ROUTE_103_LEVEL,
            Ivs::default(),
            DEFAULT_PERSONALITY,
            vec![TACKLE],
        )
        .unwrap()
        .with_ability_slot(0);
        assert_ne!(control.ability(), crate::ability::HUSTLE);
        let control_estimate =
            estimated_damage(&dex, TACKLE, &control, &defender, PERCENT_SCALE).unwrap();

        let critical_hits_suppressed = true;
        let mut no_draws = SequenceRng::new([]);
        let damage_before_roll = crate::hit::damage_before_roll(
            &dex,
            TACKLE,
            &attacker,
            &defender,
            critical_hits_suppressed,
            &mut no_draws,
        )
        .unwrap();
        let mut best_damage_roll = SequenceRng::new([MAXIMUM_DAMAGE_ROLL]);
        let real_damage =
            crate::damage::apply_damage_roll(damage_before_roll.damage, &mut best_damage_roll);
        let estimated_damage =
            estimated_damage(&dex, TACKLE, &attacker, &defender, PERCENT_SCALE).unwrap();

        assert_eq!(estimated_damage, real_damage);
        assert!(
            estimated_damage > control_estimate,
            "the Hustle attacker's estimate must exceed the unboosted control's"
        );
    }

    #[test]
    fn a_statused_guts_attackers_estimated_damage_matches_the_real_damage_step() {
        let dex = Dex::new();
        let mut attacker = BattlePokemon::new(
            &dex,
            MAKUHITA,
            ROUTE_103_LEVEL,
            Ivs::default(),
            DEFAULT_PERSONALITY,
            vec![TACKLE],
        )
        .unwrap()
        .with_ability_slot(GUTS_ABILITY_SLOT);
        assert_eq!(attacker.ability(), crate::ability::GUTS);
        attacker.set_status1(crate::status1::Status1::Paralysed);
        let defender = pokemon(MILOTIC, vec![TACKLE]);

        let critical_hits_suppressed = true;
        let mut no_draws = SequenceRng::new([]);
        let damage_before_roll = crate::hit::damage_before_roll(
            &dex,
            TACKLE,
            &attacker,
            &defender,
            critical_hits_suppressed,
            &mut no_draws,
        )
        .unwrap();
        let mut best_damage_roll = SequenceRng::new([MAXIMUM_DAMAGE_ROLL]);
        let real_damage =
            crate::damage::apply_damage_roll(damage_before_roll.damage, &mut best_damage_roll);
        let estimated_damage =
            estimated_damage(&dex, TACKLE, &attacker, &defender, PERCENT_SCALE).unwrap();

        assert_eq!(estimated_damage, real_damage);
        assert!(
            estimated_damage
                > estimated_damage_of_a_healthy_attacker(&dex, &defender)
                    .expect("healthy control must resolve"),
            "the statused Guts attacker's estimate must exceed the healthy control's"
        );
    }

    fn estimated_damage_of_a_healthy_attacker(
        dex: &Dex,
        defender: &BattlePokemon,
    ) -> Result<u32, BattleError> {
        let healthy_attacker = BattlePokemon::new(
            dex,
            MAKUHITA,
            ROUTE_103_LEVEL,
            Ivs::default(),
            DEFAULT_PERSONALITY,
            vec![TACKLE],
        )
        .unwrap()
        .with_ability_slot(GUTS_ABILITY_SLOT);
        estimated_damage(dex, TACKLE, &healthy_attacker, defender, PERCENT_SCALE)
    }

    #[test]
    fn a_statused_marvel_scale_defenders_estimated_damage_matches_the_real_damage_step() {
        let dex = Dex::new();
        let attacker = pokemon(MAKUHITA, vec![TACKLE]);
        let mut defender = pokemon(MILOTIC, vec![TACKLE]);
        assert_eq!(defender.ability(), crate::ability::MARVEL_SCALE);
        defender.set_status1(crate::status1::Status1::Poisoned);

        let critical_hits_suppressed = true;
        let mut no_draws = SequenceRng::new([]);
        let damage_before_roll = crate::hit::damage_before_roll(
            &dex,
            TACKLE,
            &attacker,
            &defender,
            critical_hits_suppressed,
            &mut no_draws,
        )
        .unwrap();
        let mut best_damage_roll = SequenceRng::new([MAXIMUM_DAMAGE_ROLL]);
        let real_damage =
            crate::damage::apply_damage_roll(damage_before_roll.damage, &mut best_damage_roll);
        let statused_estimate =
            estimated_damage(&dex, TACKLE, &attacker, &defender, PERCENT_SCALE).unwrap();

        assert_eq!(statused_estimate, real_damage);
        let healthy_defender = pokemon(MILOTIC, vec![TACKLE]);
        let healthy_estimate =
            estimated_damage(&dex, TACKLE, &attacker, &healthy_defender, PERCENT_SCALE).unwrap();
        assert!(
            statused_estimate < healthy_estimate,
            "the statused Marvel Scale defender's estimate must be lower than the healthy control's"
        );
    }

    /// `AI_CheckBadMove` reads `get_ability AI_TARGET` on a powered hit
    /// (`data/battle_ai_scripts.s:59-64`): a Wonder Guard target keeps a
    /// merely-neutral hit's -10 score, while an exactly-super-effective hit
    /// keeps its full score, matching the same ability at execution
    /// (`crates/battle/src/hit.rs:194-198`).
    #[test]
    fn wonder_guard_discourages_a_neutral_hit_but_not_a_super_effective_one() {
        let enemy = pokemon(MUDKIP, vec![WATER_GUN, WING_ATTACK]);
        let player = pokemon(SHEDINJA, vec![POUND]);
        assert_eq!(player.ability(), assets::AbilityId::WONDER_GUARD);
        let mut rng = rng_with_maximum_simulated_damage([SELECT_FIRST_TIED_MOVE]);

        let action = choose_trainer_action(
            &Dex::new(),
            &enemy,
            &player,
            AiFlags::CHECK_BAD_MOVE,
            FIRST_TURN,
            &mut rng,
        )
        .unwrap();

        assert_eq!(action, EnemyAction::Move(1));
        assert_eq!(rng.draws(), MAX_MON_MOVES + 1);
    }

    /// `AI_CBM_AttackDown` reads `get_ability AI_TARGET` after the stage
    /// floor check (`data/battle_ai_scripts.s:277-280`): Hyper Cutter scores
    /// the drop -10 at a stage the target could otherwise still lose,
    /// matching the `AbilityProtected` outcome at execution
    /// (`crates/battle/src/stat_change.rs:374-379`).
    #[test]
    fn hyper_cutter_discourages_an_attack_drop_at_an_unlowered_stage() {
        let enemy = pokemon(TREECKO, vec![POUND, GROWL]);
        // Pinsir's only ability is Hyper Cutter, so the read is unambiguous.
        let player = pokemon(PINSIR, vec![POUND]);
        assert_eq!(player.ability(), assets::AbilityId::HYPER_CUTTER);
        // Both moves tie at 100 without the guard, and the tie-break draw
        // below would already pick slot 0 -- select the SECOND tied slot so
        // this fails without the guard instead of passing by coincidence.
        // The ability reads draw nothing.
        let mut rng = rng_with_maximum_simulated_damage([SELECT_SECOND_TIED_MOVE]);

        let action = choose_trainer_action(
            &Dex::new(),
            &enemy,
            &player,
            AiFlags::CHECK_BAD_MOVE,
            FIRST_TURN,
            &mut rng,
        )
        .unwrap();

        assert_eq!(action, EnemyAction::Move(0));
        assert_eq!(rng.draws(), MAX_MON_MOVES + 1);
    }

    /// `AI_CBM_DefenseDown` shares the Clear Body / White Smoke guard with
    /// `AI_CBM_AttackDown` (`data/battle_ai_scripts.s:308-311`).
    #[test]
    fn clear_body_discourages_a_defense_drop_at_an_unlowered_stage() {
        let enemy = pokemon(TREECKO, vec![POUND, LEER]);
        // Regirock's only ability is Clear Body.
        let player = pokemon(REGIROCK, vec![POUND]);
        assert_eq!(player.ability(), crate::stat_change::CLEAR_BODY);
        // See the SECOND-tied-slot note above: both moves tie at 100 without
        // the guard, and the ability reads draw nothing.
        let mut rng = rng_with_maximum_simulated_damage([SELECT_SECOND_TIED_MOVE]);

        let action = choose_trainer_action(
            &Dex::new(),
            &enemy,
            &player,
            AiFlags::CHECK_BAD_MOVE,
            FIRST_TURN,
            &mut rng,
        )
        .unwrap();

        assert_eq!(action, EnemyAction::Move(0));
        assert_eq!(rng.draws(), MAX_MON_MOVES + 1);
    }

    /// The Soundproof guard (`data/battle_ai_scripts.s:92-103`) omits Hyper
    /// Voice unlike execution's ten-move `SOUND_MOVES`
    /// (`crates/battle/src/stat_change.rs:83-94`).
    #[test]
    fn soundproof_discourages_growl_but_not_hyper_voice() {
        let enemy = pokemon(TREECKO, vec![HYPER_VOICE, GROWL]);
        // Mr. Mime's only ability is Soundproof.
        let player = pokemon(MR_MIME, vec![POUND]);
        assert_eq!(player.ability(), assets::AbilityId::SOUNDPROOF);
        // Both moves tie at 100 without the guard; see the SECOND-tied-slot
        // note above. Hyper Voice is absent from the AI's own list, so it
        // scores unaffected either way; the ability reads draw nothing.
        let mut rng = rng_with_maximum_simulated_damage([SELECT_SECOND_TIED_MOVE]);

        let action = choose_trainer_action(
            &Dex::new(),
            &enemy,
            &player,
            AiFlags::CHECK_BAD_MOVE,
            FIRST_TURN,
            &mut rng,
        )
        .unwrap();

        assert_eq!(action, EnemyAction::Move(0));
        assert_eq!(rng.draws(), MAX_MON_MOVES + 1);
    }

    /// Upstream's `get_ability AI_TARGET` reads history, a trapping ability,
    /// or draws a fresh `Random() & 1` guess between two species slots
    /// (`src/battle_ai_script_commands.c:1350-1396`). The engine models no
    /// history, so a two-ability target without a trapping ability is refused
    /// before any draw, whichever slot it holds: Voltorb in either slot and a
    /// Hyper Cutter Trapinch.
    #[test]
    fn a_two_ability_target_is_refused_before_any_draw() {
        let dex = Dex::new();
        let enemy = pokemon(TREECKO, vec![HYPER_VOICE, GROWL]);
        for (species, slot) in [(VOLTORB, 0), (VOLTORB, 1), (TRAPINCH, 0)] {
            let target = pokemon(species, vec![POUND]).with_ability_slot(slot);
            let mut rng = rng_with_maximum_simulated_damage([]);
            let draws_before = rng.draws();

            let refused = choose_trainer_action(
                &dex,
                &enemy,
                &target,
                AiFlags::CHECK_BAD_MOVE,
                FIRST_TURN,
                &mut rng,
            );

            assert_eq!(refused, Err(BattleError::AmbiguousPlayerAbility(species)));
            assert_eq!(rng.draws(), draws_before);
        }
    }

    /// Upstream returns a true trapping ability before the two-slot guess,
    /// with no draw (`src/battle_ai_script_commands.c:1368-1376`), so an Arena
    /// Trap Trapinch is admitted and read directly while a Hyper Cutter
    /// Trapinch stays refused.
    #[test]
    fn an_arena_trap_target_is_admitted_and_read_without_a_draw() {
        let dex = Dex::new();
        let enemy = pokemon(TREECKO, vec![HYPER_VOICE, GROWL]);
        let arena_trap = pokemon(TRAPINCH, vec![POUND]).with_ability_slot(1);
        let hyper_cutter = pokemon(TRAPINCH, vec![POUND]).with_ability_slot(0);

        assert_eq!(
            super::ai_get_ability(&arena_trap),
            Ok(assets::AbilityId::ARENA_TRAP)
        );
        assert_eq!(
            super::ensure_target_ability_unambiguous(&arena_trap),
            Ok(())
        );
        assert_eq!(
            super::ensure_target_ability_unambiguous(&hyper_cutter),
            Err(BattleError::AmbiguousPlayerAbility(TRAPINCH))
        );

        let mut admitted = rng_with_maximum_simulated_damage([SELECT_FIRST_TIED_MOVE]);
        let mut single = rng_with_maximum_simulated_damage([SELECT_FIRST_TIED_MOVE]);
        let trapping_choice = choose_trainer_action(
            &dex,
            &enemy,
            &arena_trap,
            AiFlags::CHECK_BAD_MOVE,
            FIRST_TURN,
            &mut admitted,
        );
        let single_ability_choice = choose_trainer_action(
            &dex,
            &enemy,
            &pokemon(TREECKO, vec![POUND]),
            AiFlags::CHECK_BAD_MOVE,
            FIRST_TURN,
            &mut single,
        );
        assert!(trapping_choice.is_ok());
        assert_eq!(trapping_choice, single_ability_choice);
        assert_eq!(admitted.draws(), single.draws());
    }

    /// A single-ability target reads its sole ability and draws nothing.
    #[test]
    fn a_single_ability_target_is_read_without_a_draw() {
        let target = pokemon(SHEDINJA, vec![POUND]);
        assert_eq!(
            super::ai_get_ability(&target),
            Ok(assets::AbilityId::WONDER_GUARD)
        );
    }

    /// `TypeCalc` returns for Struggle before the chart, so a depleted
    /// Struggle outranks an immune or resisted Take Down (power 90, resisted ~45 against 50) in the `TryToFaint`
    /// power comparison instead of flooring to one alongside it.
    #[test]
    fn a_depleted_struggle_keeps_untyped_damage_in_the_power_comparison() {
        let dex = Dex::new();
        let attacker_with = |moves: Vec<MoveId>| {
            let mut attacker = pokemon(TRAPINCH, moves);
            spend_move(&mut attacker, 1);
            attacker
        };
        let attacker = attacker_with(vec![MoveId::TAKE_DOWN, MoveId::STRUGGLE]);
        // Gastly (Ghost), Onix (Rock/Ground), Magnemite (Electric/Steel), Aron.
        for species in [GASTLY, SpeciesId(95), SpeciesId(81), SpeciesId(382)] {
            let defender = pokemon(species, vec![POUND]);
            let mut rng = rng_with_maximum_simulated_damage([]);
            let scores = MoveScores::initialize(&attacker, &mut rng);
            let struggle = estimated_damage(
                &dex,
                MoveId::STRUGGLE,
                &attacker,
                &defender,
                scores.simulated_damage_percent[1],
            )
            .unwrap();
            assert!(struggle > 1, "species {species:?}: {struggle}");
            assert_eq!(
                compare_move_power(&dex, &scores, 0, MoveId::TAKE_DOWN, &attacker, &defender),
                Ok(PowerComparison::WeakerThanAnotherMove),
                "species {species:?}"
            );
        }
    }

    #[test]
    fn plus_minus_estimated_damage_matches_the_real_damage_step() {
        let dex = Dex::new();
        let plusle = pokemon(SpeciesId(353), vec![MoveId::WATER_GUN]);
        let minun = pokemon(SpeciesId(354), vec![MoveId::WATER_GUN]);
        let squirtle = pokemon(SQUIRTLE, vec![MoveId::WATER_GUN]);
        assert_eq!(plusle.ability(), assets::AbilityId::PLUS);
        assert_eq!(minun.ability(), assets::AbilityId::MINUS);

        for (attacker, defender) in [(&plusle, &minun), (&minun, &plusle), (&plusle, &squirtle)] {
            let mut no_draws = SequenceRng::new([]);
            let before_roll = crate::hit::damage_before_roll(
                &dex,
                MoveId::WATER_GUN,
                attacker,
                defender,
                true,
                &mut no_draws,
            )
            .unwrap();
            let mut best_roll = SequenceRng::new([MAXIMUM_DAMAGE_ROLL]);
            let real = crate::damage::apply_damage_roll(before_roll.damage, &mut best_roll);
            let estimate =
                estimated_damage(&dex, MoveId::WATER_GUN, attacker, defender, PERCENT_SCALE)
                    .unwrap();
            assert_eq!(estimate, real);
        }
        // The boost itself must show: the complement out-damages a same-ability bystander.
        let boosted =
            estimated_damage(&dex, MoveId::WATER_GUN, &plusle, &minun, PERCENT_SCALE).unwrap();
        let control =
            estimated_damage(&dex, MoveId::WATER_GUN, &plusle, &plusle, PERCENT_SCALE).unwrap();
        assert!(boosted > control, "{boosted} vs {control}");
    }
}

#[cfg(test)]
mod prologue_tests;
#[cfg(test)]
mod stat_stage_tests;
mod voluntary_switch;
pub(crate) use voluntary_switch::VoluntarySwitch;
