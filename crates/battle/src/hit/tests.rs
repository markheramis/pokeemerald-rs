use super::{
    classify_accuracy_failure, damage_before_roll, damage_core, ensure_resolvable,
    is_ordinary_hit_effect, resolve_hit, HitOutcome,
};
use crate::ability::{suppresses_critical_hits, GUTS, HUGE_POWER, MARVEL_SCALE, PURE_POWER};
use crate::accuracy::always_hits;
use crate::damage::{
    apply_dual_type_effectiveness, base_damage, DamageInput, MoveCategory, Weather, STRUGGLE,
};
use crate::dex::Dex;
use crate::error::BattleError;
use crate::pokemon::{BattlePokemon, Ivs};
use crate::script_rng::SequenceRng;
use crate::secondary::SecondaryApplication;
use crate::stat_stage::StatStage;
use crate::status1::Status1;
use assets::species::AbilityId;
use assets::{MoveId, SpeciesId, Type};

const BULBASAUR: SpeciesId = SpeciesId(1);
const SQUIRTLE: SpeciesId = SpeciesId(7);
const RATTATA: SpeciesId = SpeciesId(19);
const ABRA: SpeciesId = SpeciesId(63);
const SHELLDER: SpeciesId = SpeciesId(90);
const GASTLY: SpeciesId = SpeciesId(92);
const MARILL: SpeciesId = SpeciesId(183);
const MEDITITE: SpeciesId = SpeciesId(356);
const ANORITH: SpeciesId = SpeciesId(390);
/// `SPECIES_MAKUHITA`: Guts in ability slot 1 (Thick Fat is slot 0).
const MAKUHITA: SpeciesId = SpeciesId(335);
/// `SPECIES_MILOTIC`: Marvel Scale in its primary (and only) ability slot.
const MILOTIC: SpeciesId = SpeciesId(329);
/// `SPECIES_BUTTERFREE`: Compound Eyes in its primary (and only) ability slot.
const BUTTERFREE: SpeciesId = SpeciesId(12);
/// `SPECIES_REMORAID`: Hustle in its primary (and only) ability slot.
const REMORAID: SpeciesId = SpeciesId(223);
/// `SPECIES_DELIBIRD`: Vital Spirit in slot 0, Hustle in slot 1 -- unlike
/// Remoraid, a same-species non-Hustle control is reachable through
/// [`BattlePokemon::with_ability_slot`].
const DELIBIRD: SpeciesId = SpeciesId(225);

const DOUBLE_SLAP: MoveId = MoveId::DOUBLE_SLAP;
const HORN_DRILL: MoveId = MoveId::HORN_DRILL;
const TACKLE: MoveId = MoveId::TACKLE;
/// `MOVE_BONE_RUSH` -- `EFFECT_MULTI_HIT`, Ground. Used below only for its
/// typing: no admitted `is_ordinary_hit_effect` move is Ground-type, so
/// [`damage_core`] is exercised directly instead of through [`resolve_hit`],
/// which would reject Bone Rush's effect at [`ensure_resolvable`].
const BONE_RUSH: MoveId = MoveId::BONE_RUSH;
/// `MOVE_ICICLE_SPEAR` -- `EFFECT_MULTI_HIT`, Ice; its loop reaches Thick Fat
/// through [`damage_before_roll`] alone.
const ICICLE_SPEAR: MoveId = MoveId::ICICLE_SPEAR;
const GROWL: MoveId = MoveId::GROWL;
const SONIC_BOOM: MoveId = MoveId::SONIC_BOOM;
const WATER_GUN: MoveId = MoveId::WATER_GUN;
const COUNTER: MoveId = MoveId::COUNTER;
const SEISMIC_TOSS: MoveId = MoveId::SEISMIC_TOSS;
const ABSORB: MoveId = MoveId::ABSORB;
const QUICK_ATTACK: MoveId = MoveId::QUICK_ATTACK;
const SWIFT: MoveId = MoveId::SWIFT;
const SLASH: MoveId = MoveId::SLASH;
const CURSE: MoveId = MoveId::CURSE;
const FALSE_SWIPE: MoveId = MoveId::FALSE_SWIPE;
const PURSUIT: MoveId = MoveId::PURSUIT;
const UNKNOWN_MOVE: MoveId = MoveId(60_000);
/// Normal-type (physical), 75 accuracy.
const SLAM: MoveId = MoveId::SLAM;
/// `MOVE_MUD_SHOT` -- `EFFECT_SPEED_DOWN_HIT`, Ground, certain secondary.
const MUD_SHOT: MoveId = MoveId::MUD_SHOT;

const THICK_FAT: AbilityId = AbilityId::THICK_FAT;
/// `SPECIES_SHEDINJA`: Wonder Guard in its primary (and only) ability slot.
const SHEDINJA: SpeciesId = SpeciesId(303);
const FAINT_ATTACK: MoveId = MoveId::FAINT_ATTACK;

const MAX_IVS: Ivs = Ivs {
    hp: 31,
    attack: 31,
    defense: 31,
    speed: 31,
    sp_attack: 31,
    sp_defense: 31,
};

const ACCURACY_HIT_DRAW: u16 = 0;
const TACKLE_MISS_DRAW: u16 = 95;
/// Slam's plain 75 threshold misses roll 75 for an attacker with no accuracy
/// modifier.
const SLAM_MISS_DRAW: u16 = 75;
/// Slam's plain 75 threshold misses roll 91, but Compound Eyes raises the
/// threshold to `75 * 130 / 100 = 97`, which the same roll clears
/// (`battle_script_commands.c:1152-1153`).
const COMPOUND_EYES_ONLY_HIT_DRAW: u16 = 90;
/// Slam's plain 75 threshold hits roll 66, but Hustle lowers a physical
/// move's threshold to `75 * 80 / 100 = 60`, which the same roll exceeds
/// (`battle_script_commands.c:1156-1157`).
const HUSTLE_ONLY_MISS_DRAW: u16 = 65;
const ORDINARY_CRIT_DRAW: u16 = 0;
const ORDINARY_NO_CRIT_DRAW: u16 = 1;
const HIGH_CRIT_ONLY_DRAW: u16 = 8;
const BEST_DAMAGE_DRAW: u16 = 0;
const DISCARDED_EFFECT_DRAW: u16 = 0;

const ORDINARY_NON_CRITICAL_DRAWS: [u16; 4] = [
    ACCURACY_HIT_DRAW,
    ORDINARY_NO_CRIT_DRAW,
    BEST_DAMAGE_DRAW,
    DISCARDED_EFFECT_DRAW,
];
const ORDINARY_CRITICAL_DRAWS: [u16; 4] = [
    ACCURACY_HIT_DRAW,
    ORDINARY_CRIT_DRAW,
    BEST_DAMAGE_DRAW,
    DISCARDED_EFFECT_DRAW,
];
const ALWAYS_HIT_NON_CRITICAL_DRAWS: [u16; 3] = [
    ORDINARY_NO_CRIT_DRAW,
    BEST_DAMAGE_DRAW,
    DISCARDED_EFFECT_DRAW,
];
const CRITICAL_SUPPRESSED_DRAWS: [u16; 3] =
    [ACCURACY_HIT_DRAW, BEST_DAMAGE_DRAW, DISCARDED_EFFECT_DRAW];
const STRUGGLE_NON_CRITICAL_DRAWS: [u16; 3] =
    [ACCURACY_HIT_DRAW, ORDINARY_NO_CRIT_DRAW, BEST_DAMAGE_DRAW];

const TACKLE_DAMAGE: u32 = 4;
const TACKLE_CRITICAL_DAMAGE: u32 = 8;
const MODEST_WATER_GUN_DAMAGE: u32 = 10;
const STRUGGLE_DAMAGE_TO_GASTLY: u32 = 6;
const THICK_FAT_MARILL_TACKLE_DAMAGE: u32 = 3;
const HUGE_POWER_MARILL_TACKLE_DAMAGE: u32 = 5;
const PURE_POWER_MEDITITE_TACKLE_DAMAGE: u32 = 6;
const HUGE_POWER_BEFORE_STAGE_DAMAGE: u32 = 8;
/// L5 Makuhita (12 raw Attack) versus L5 Milotic (14 raw Defense), Tackle,
/// best roll, no boost active on either side.
const HEALTHY_MAKUHITA_TACKLE_DAMAGE: u32 = 4;
/// The same matchup with the Makuhita attacker paralysed or poisoned, so
/// Guts raises its raw Attack from 12 to 18 before the stage multiply.
const GUTS_MAKUHITA_TACKLE_DAMAGE: u32 = 5;
/// The same base matchup with the Milotic defender paralysed or poisoned, so
/// Marvel Scale raises its raw Defense from 14 to 21 before the stage
/// multiply.
const MARVEL_SCALE_MILOTIC_TACKLE_DAMAGE: u32 = 3;

fn mon(dex: &Dex, species: SpeciesId, level: u8, moves: Vec<MoveId>) -> BattlePokemon {
    BattlePokemon::new(dex, species, level, MAX_IVS, 0, moves).unwrap()
}

#[test]
fn a_miss_draws_only_for_accuracy() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 5, vec![TACKLE]);
    let defender = mon(&dex, SQUIRTLE, 5, vec![TACKLE]);
    let mut rng = SequenceRng::new([TACKLE_MISS_DRAW]);

    let resolution = resolve_hit(&dex, TACKLE, &attacker, &defender, false, &mut rng).unwrap();

    assert_eq!(resolution.outcome, HitOutcome::Miss);
    assert_eq!(resolution.secondary_effect, SecondaryApplication::None);
    assert_eq!(rng.draws(), 1);
}

#[test]
fn an_ordinary_hit_draws_accuracy_critical_damage_and_effect_chance() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 5, vec![TACKLE]);
    let defender = mon(&dex, SQUIRTLE, 5, vec![TACKLE]);
    let mut rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);

    let resolution = resolve_hit(&dex, TACKLE, &attacker, &defender, false, &mut rng).unwrap();

    assert!(matches!(resolution.outcome, HitOutcome::Hit { .. }));
    assert_eq!(
        resolution.secondary_effect,
        SecondaryApplication::None,
        "Tackle has no secondary effect"
    );
    assert_eq!(rng.draws(), ORDINARY_NON_CRITICAL_DRAWS.len());
}

#[test]
fn the_discarded_effect_chance_value_never_changes_an_ordinary_hit() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 5, vec![TACKLE]);
    let defender = mon(&dex, SQUIRTLE, 5, vec![TACKLE]);

    let mut outcomes = Vec::new();
    for effect_chance_draw in [0u16, 99, u16::MAX] {
        let draws = [
            ACCURACY_HIT_DRAW,
            ORDINARY_NO_CRIT_DRAW,
            BEST_DAMAGE_DRAW,
            effect_chance_draw,
        ];
        let mut rng = SequenceRng::new(draws);
        outcomes.push(resolve_hit(&dex, TACKLE, &attacker, &defender, false, &mut rng).unwrap());
        assert_eq!(rng.draws(), draws.len());
    }

    assert!(outcomes.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn best_roll_non_critical_damage_matches_the_independent_pin() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 5, vec![TACKLE]);
    let defender = mon(&dex, SQUIRTLE, 5, vec![TACKLE]);
    let mut rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);

    let resolution = resolve_hit(&dex, TACKLE, &attacker, &defender, false, &mut rng).unwrap();

    assert_eq!(
        resolution.outcome,
        HitOutcome::Hit {
            damage: TACKLE_DAMAGE,
            is_critical: false,
        }
    );
    assert_eq!(rng.draws(), ORDINARY_NON_CRITICAL_DRAWS.len());
}

#[test]
fn a_confirmed_critical_hit_doubles_the_pinned_damage_and_is_reported() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 5, vec![TACKLE]);
    let defender = mon(&dex, SQUIRTLE, 5, vec![TACKLE]);
    let mut rng = SequenceRng::new(ORDINARY_CRITICAL_DRAWS);

    let resolution = resolve_hit(&dex, TACKLE, &attacker, &defender, false, &mut rng).unwrap();

    assert_eq!(
        resolution.outcome,
        HitOutcome::Hit {
            damage: TACKLE_CRITICAL_DAMAGE,
            is_critical: true,
        }
    );
    assert_eq!(TACKLE_CRITICAL_DAMAGE, 2 * TACKLE_DAMAGE);
    assert_eq!(rng.draws(), ORDINARY_CRITICAL_DRAWS.len());
}

#[test]
fn a_special_move_uses_nature_adjusted_special_stats() {
    let dex = Dex::new();
    let modest_personality = 15;
    let calm_personality = 20;
    let attacker = BattlePokemon::new(
        &dex,
        SQUIRTLE,
        10,
        MAX_IVS,
        modest_personality,
        vec![WATER_GUN],
    )
    .unwrap();
    let defender =
        BattlePokemon::new(&dex, RATTATA, 10, MAX_IVS, calm_personality, vec![TACKLE]).unwrap();
    assert_eq!(attacker.stats().sp_attack, 19);
    assert_eq!(defender.stats().sp_defense, 16);
    let mut rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);

    let resolution = resolve_hit(&dex, WATER_GUN, &attacker, &defender, false, &mut rng).unwrap();

    assert_eq!(
        resolution.outcome,
        HitOutcome::Hit {
            damage: MODEST_WATER_GUN_DAMAGE,
            is_critical: false,
        }
    );
    assert_eq!(rng.draws(), ORDINARY_NON_CRITICAL_DRAWS.len());
}

#[test]
fn struggle_bypasses_stab_type_effectiveness_and_the_effect_chance_draw() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 5, vec![STRUGGLE]);
    let ghost_defender = mon(&dex, GASTLY, 5, vec![TACKLE]);
    let mut rng = SequenceRng::new(STRUGGLE_NON_CRITICAL_DRAWS);

    let resolution =
        resolve_hit(&dex, STRUGGLE, &attacker, &ghost_defender, false, &mut rng).unwrap();

    assert_eq!(
        resolution.outcome,
        HitOutcome::Hit {
            damage: STRUGGLE_DAMAGE_TO_GASTLY,
            is_critical: false,
        }
    );
    assert_eq!(rng.draws(), STRUGGLE_NON_CRITICAL_DRAWS.len());

    let ordinary_attacker = mon(&dex, BULBASAUR, 5, vec![TACKLE]);
    let mut rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);
    let resolution = resolve_hit(
        &dex,
        TACKLE,
        &ordinary_attacker,
        &ghost_defender,
        false,
        &mut rng,
    )
    .unwrap();
    assert_eq!(resolution.outcome, HitOutcome::NoEffect);
}

#[test]
fn type_immunity_still_draws_critical_damage_and_effect_chance() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 20, vec![TACKLE]);
    let ghost_defender = mon(&dex, GASTLY, 20, vec![TACKLE]);
    let mut rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);

    let resolution =
        resolve_hit(&dex, TACKLE, &attacker, &ghost_defender, false, &mut rng).unwrap();

    assert_eq!(resolution.outcome, HitOutcome::NoEffect);
    assert_eq!(rng.draws(), ORDINARY_NON_CRITICAL_DRAWS.len());
}

/// `Cmd_typecalc`'s Levitate branch (`battle_script_commands.c:1375-1383`).
/// Pins the ordinary-pipeline half of `damage_before_roll`'s shared boundary
/// directly via `damage_core`, since no admitted ordinary-hit move is
/// Ground-type to exercise it through `resolve_hit` end to end (see the
/// multi-hit turn-level regression for that half via Bone Rush in
/// `crates/battle/tests/turn_engine/move_resolution.rs`).
#[test]
fn levitate_blocks_a_ground_move_before_stab_and_type_effectiveness() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 20, vec![BONE_RUSH]);
    let levitate_defender = mon(&dex, GASTLY, 20, vec![TACKLE]);
    let mut rng = SequenceRng::new([ORDINARY_NO_CRIT_DRAW, BEST_DAMAGE_DRAW]);

    let outcome = damage_core(
        &dex,
        BONE_RUSH,
        &attacker,
        &levitate_defender,
        false,
        &mut rng,
    )
    .unwrap();

    assert_eq!(outcome, HitOutcome::LevitateBlocked);
    assert_eq!(
        rng.draws(),
        2,
        "a Levitate block still spends the critical and damage-variance \
         draws, exactly like an ordinary type immunity"
    );
}

/// `Cmd_typecalc`'s Wonder Guard branch (`battle_script_commands.c:1409-1418`)
/// blocks any powered, not-strictly-super-effective hit, distinctly from a
/// [`HitOutcome::NoEffect`] typing immunity.
#[test]
fn wonder_guard_blocks_a_neutral_hit_and_still_draws_normally() {
    let dex = Dex::new();
    let attacker = mon(&dex, RATTATA, 10, vec![WATER_GUN]);
    let shedinja_defender = mon(&dex, SHEDINJA, 5, vec![TACKLE]);
    assert_eq!(shedinja_defender.ability(), AbilityId::WONDER_GUARD);
    let mut rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);

    let resolution = resolve_hit(
        &dex,
        WATER_GUN,
        &attacker,
        &shedinja_defender,
        false,
        &mut rng,
    )
    .unwrap();

    assert_eq!(resolution.outcome, HitOutcome::WonderGuardBlocked);
    assert_eq!(
        resolution.secondary_effect,
        SecondaryApplication::None,
        "a Wonder Guard block must not permit a secondary effect"
    );
    assert_eq!(
        rng.draws(),
        ORDINARY_NON_CRITICAL_DRAWS.len(),
        "a Wonder Guard block still spends the same draws as an ordinary hit"
    );
}

/// Levitate sets `MOVE_RESULT_MISSED | MOVE_RESULT_DOESNT_AFFECT_FOE`
/// (`battle_script_commands.c:1375-1378`), so `seteffectwithchance` spends
/// its draw but skips `SetMoveEffect` (`:2923-2930`).
#[test]
fn levitate_blocked_mud_shot_applies_no_speed_drop() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 20, vec![MUD_SHOT]);
    let levitate_defender = mon(&dex, GASTLY, 20, vec![TACKLE]);
    assert_eq!(levitate_defender.ability(), AbilityId::LEVITATE);
    let mut rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);

    let resolution = resolve_hit(
        &dex,
        MUD_SHOT,
        &attacker,
        &levitate_defender,
        false,
        &mut rng,
    )
    .unwrap();

    assert_eq!(resolution.outcome, HitOutcome::LevitateBlocked);
    assert_eq!(rng.draws(), ORDINARY_NON_CRITICAL_DRAWS.len());
    assert_eq!(resolution.secondary_effect, SecondaryApplication::None);
}

/// Wonder Guard's own message (`B_MSG_AVOIDED_DMG`, index 3) outranks the
/// ordinary typing-immunity message (`B_MSG_AVOIDED_ATK`, index 2) in
/// `Cmd_resultmessage` (`battle_script_commands.c:2048-2059`), so a move that
/// is independently type-immune must still report the Wonder Guard outcome.
#[test]
fn wonder_guard_outranks_an_independent_typing_immunity() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 20, vec![TACKLE]);
    let shedinja_defender = mon(&dex, SHEDINJA, 20, vec![WATER_GUN]);
    let mut rng = SequenceRng::new([ORDINARY_NO_CRIT_DRAW, BEST_DAMAGE_DRAW]);

    let outcome =
        damage_core(&dex, TACKLE, &attacker, &shedinja_defender, false, &mut rng).unwrap();

    assert_eq!(outcome, HitOutcome::WonderGuardBlocked);
}

/// Wonder Guard permits a strictly super-effective hit, so the block above
/// is a targeted admission check rather than a general immunity.
#[test]
fn wonder_guard_permits_a_super_effective_hit() {
    let dex = Dex::new();
    let attacker = mon(&dex, RATTATA, 10, vec![FAINT_ATTACK]);
    let shedinja_defender = mon(&dex, SHEDINJA, 5, vec![TACKLE]);
    let mut rng = SequenceRng::new(ALWAYS_HIT_NON_CRITICAL_DRAWS);

    let resolution = resolve_hit(
        &dex,
        FAINT_ATTACK,
        &attacker,
        &shedinja_defender,
        false,
        &mut rng,
    )
    .unwrap();

    assert!(matches!(resolution.outcome, HitOutcome::Hit { .. }));
}

/// Struggle bypasses `Cmd_typecalc` entirely (`battle_script_commands.c:1360-1364`),
/// so it must reach a Wonder Guard holder exactly like any other defender.
#[test]
fn wonder_guard_never_blocks_struggle() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 5, vec![STRUGGLE]);
    let shedinja_defender = mon(&dex, SHEDINJA, 5, vec![TACKLE]);
    let mut rng = SequenceRng::new(STRUGGLE_NON_CRITICAL_DRAWS);

    let resolution = resolve_hit(
        &dex,
        STRUGGLE,
        &attacker,
        &shedinja_defender,
        false,
        &mut rng,
    )
    .unwrap();

    assert!(matches!(resolution.outcome, HitOutcome::Hit { .. }));
}

#[test]
fn an_accuracy_bypassing_hit_starts_with_the_critical_draw() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 5, vec![SWIFT]);
    let defender = mon(&dex, SQUIRTLE, 5, vec![TACKLE]);
    assert!(always_hits(dex.move_data(SWIFT).unwrap().effect));
    let mut rng = SequenceRng::new(ALWAYS_HIT_NON_CRITICAL_DRAWS);

    let resolution = resolve_hit(&dex, SWIFT, &attacker, &defender, false, &mut rng).unwrap();

    assert!(matches!(resolution.outcome, HitOutcome::Hit { .. }));
    assert_eq!(rng.draws(), ALWAYS_HIT_NON_CRITICAL_DRAWS.len());

    let draws = [TACKLE_MISS_DRAW, BEST_DAMAGE_DRAW, DISCARDED_EFFECT_DRAW];
    let mut rng = SequenceRng::new(draws);
    let resolution = resolve_hit(&dex, SWIFT, &attacker, &defender, false, &mut rng).unwrap();
    assert!(matches!(resolution.outcome, HitOutcome::Hit { .. }));
    assert_eq!(rng.draws(), draws.len());
}

#[test]
fn a_high_crit_move_crits_on_a_draw_an_ordinary_move_does_not() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 5, vec![SLASH, TACKLE]);
    let defender = mon(&dex, SQUIRTLE, 5, vec![TACKLE]);
    let separating_draws = [
        ACCURACY_HIT_DRAW,
        HIGH_CRIT_ONLY_DRAW,
        BEST_DAMAGE_DRAW,
        DISCARDED_EFFECT_DRAW,
    ];

    let mut slash_rng = SequenceRng::new(separating_draws);
    let slash = resolve_hit(&dex, SLASH, &attacker, &defender, false, &mut slash_rng).unwrap();
    assert!(matches!(
        slash.outcome,
        HitOutcome::Hit {
            is_critical: true,
            ..
        }
    ));

    let mut tackle_rng = SequenceRng::new(separating_draws);
    let tackle = resolve_hit(&dex, TACKLE, &attacker, &defender, false, &mut tackle_rng).unwrap();
    assert!(matches!(
        tackle.outcome,
        HitOutcome::Hit {
            is_critical: false,
            ..
        }
    ));
}

#[test]
fn caller_critical_suppression_skips_the_critical_draw_and_never_crits() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 5, vec![SLASH]);
    let defender = mon(&dex, SQUIRTLE, 5, vec![TACKLE]);
    let mut rng = SequenceRng::new(CRITICAL_SUPPRESSED_DRAWS);

    let resolution = resolve_hit(&dex, SLASH, &attacker, &defender, true, &mut rng).unwrap();

    assert!(matches!(
        resolution.outcome,
        HitOutcome::Hit {
            is_critical: false,
            ..
        }
    ));
    assert_eq!(rng.draws(), CRITICAL_SUPPRESSED_DRAWS.len());
}

#[test]
fn critical_suppression_removes_only_the_critical_draw() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 5, vec![TACKLE, SWIFT]);
    let defender = mon(&dex, SQUIRTLE, 5, vec![TACKLE]);

    let mut ordinary_rng = SequenceRng::new(CRITICAL_SUPPRESSED_DRAWS);
    let ordinary =
        resolve_hit(&dex, TACKLE, &attacker, &defender, true, &mut ordinary_rng).unwrap();
    assert!(matches!(ordinary.outcome, HitOutcome::Hit { .. }));
    assert_eq!(ordinary_rng.draws(), CRITICAL_SUPPRESSED_DRAWS.len());

    let always_hit_draws = [BEST_DAMAGE_DRAW, DISCARDED_EFFECT_DRAW];
    let mut always_hit_rng = SequenceRng::new(always_hit_draws);
    let always_hit =
        resolve_hit(&dex, SWIFT, &attacker, &defender, true, &mut always_hit_rng).unwrap();
    assert!(matches!(always_hit.outcome, HitOutcome::Hit { .. }));
    assert_eq!(always_hit_rng.draws(), always_hit_draws.len());

    let mut miss_rng = SequenceRng::new([TACKLE_MISS_DRAW]);
    let miss = resolve_hit(&dex, TACKLE, &attacker, &defender, true, &mut miss_rng).unwrap();
    assert_eq!(miss.outcome, HitOutcome::Miss);
    assert_eq!(miss_rng.draws(), 1);
}

#[test]
fn zero_power_moves_are_reported_as_non_damaging() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 5, vec![GROWL]);
    let defender = mon(&dex, SQUIRTLE, 5, vec![TACKLE]);
    let mut rng = SequenceRng::new([]);

    assert_eq!(
        resolve_hit(&dex, GROWL, &attacker, &defender, false, &mut rng),
        Err(BattleError::NonDamagingMove(GROWL))
    );
    assert_eq!(rng.draws(), 0);
}

#[test]
fn powered_moves_with_other_execution_rules_are_rejected_before_rng() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 5, vec![TACKLE]);
    let defender = mon(&dex, SQUIRTLE, 5, vec![TACKLE]);
    let unsupported_moves = [
        SONIC_BOOM,
        DOUBLE_SLAP,
        HORN_DRILL,
        COUNTER,
        SEISMIC_TOSS,
        ABSORB,
        FALSE_SWIPE,
        PURSUIT,
    ];

    for move_id in unsupported_moves {
        assert!(dex.move_data(move_id).unwrap().power > 0, "{move_id:?}");
        let mut rng = SequenceRng::new([]);
        assert_eq!(
            resolve_hit(&dex, move_id, &attacker, &defender, false, &mut rng),
            Err(BattleError::UnsupportedMoveEffect(move_id)),
            "{move_id:?}"
        );
        assert_eq!(rng.draws(), 0, "{move_id:?}");
    }
}

#[test]
fn ordinary_hit_shaped_moves_and_struggle_are_accepted() {
    let dex = Dex::new();
    for move_id in [TACKLE, SLASH, SWIFT, QUICK_ATTACK] {
        let effect = dex.move_data(move_id).unwrap().effect;
        assert!(is_ordinary_hit_effect(effect), "{move_id:?}");
        assert_eq!(ensure_resolvable(&dex, move_id), Ok(()));
    }

    assert!(!is_ordinary_hit_effect(
        dex.move_data(STRUGGLE).unwrap().effect
    ));
    assert_eq!(ensure_resolvable(&dex, STRUGGLE), Ok(()));
}

/// `MOVE_POISON_STING`: `EFFECT_POISON_HIT`.
const POISON_STING: MoveId = MoveId::POISON_STING;
/// `MOVE_SMOG`: `EFFECT_POISON_HIT`.
const SMOG: MoveId = MoveId::SMOG;
/// `MOVE_SLUDGE`: `EFFECT_POISON_HIT`.
const SLUDGE: MoveId = MoveId::SLUDGE;
/// `MOVE_SLUDGE_BOMB`: `EFFECT_POISON_HIT`.
const SLUDGE_BOMB: MoveId = MoveId::SLUDGE_BOMB;
/// `MOVE_POISON_TAIL`: `EFFECT_POISON_TAIL`, an unported trampoline.
const POISON_TAIL: MoveId = MoveId::POISON_TAIL;
/// `SPECIES_EKANS`: mono Poison-type.
const EKANS: SpeciesId = SpeciesId(23);
/// A draw that clears [`POISON_STING`]'s 30% secondary chance.
const POISON_CHANCE_HIT_DRAW: u16 = 29;
/// A draw that misses it.
const POISON_CHANCE_MISS_DRAW: u16 = 30;

#[test]
fn every_effect_poison_hit_move_is_admitted_though_not_ordinary() {
    let dex = Dex::new();
    for move_id in [POISON_STING, SMOG, SLUDGE, SLUDGE_BOMB] {
        let effect = dex.move_data(move_id).unwrap().effect;
        assert!(
            !is_ordinary_hit_effect(effect),
            "{move_id:?} needs the poison trampoline, not the plain hit script"
        );
        assert_eq!(ensure_resolvable(&dex, move_id), Ok(()), "{move_id:?}");
    }
}

#[test]
fn poison_tail_is_still_rejected_like_every_other_unported_trampoline() {
    let dex = Dex::new();
    assert_eq!(
        ensure_resolvable(&dex, POISON_TAIL),
        Err(BattleError::UnsupportedMoveEffect(POISON_TAIL))
    );
}

#[test]
fn a_landed_poison_sting_reports_poisons_defender_only_on_a_successful_roll() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 10, vec![POISON_STING]);
    let defender = mon(&dex, SQUIRTLE, 10, vec![TACKLE]);
    assert_eq!(
        dex.move_data(POISON_STING).unwrap().secondary_effect_chance,
        30
    );

    let mut succeeds = SequenceRng::new([
        ACCURACY_HIT_DRAW,
        ORDINARY_NO_CRIT_DRAW,
        BEST_DAMAGE_DRAW,
        POISON_CHANCE_HIT_DRAW,
    ]);
    let succeeded = resolve_hit(
        &dex,
        POISON_STING,
        &attacker,
        &defender,
        false,
        &mut succeeds,
    )
    .unwrap();
    assert!(matches!(succeeded.outcome, HitOutcome::Hit { .. }));
    assert_eq!(succeeded.secondary_effect, SecondaryApplication::Poison);
    assert_eq!(succeeds.draws(), 4);

    let mut fails = SequenceRng::new([
        ACCURACY_HIT_DRAW,
        ORDINARY_NO_CRIT_DRAW,
        BEST_DAMAGE_DRAW,
        POISON_CHANCE_MISS_DRAW,
    ]);
    let failed = resolve_hit(&dex, POISON_STING, &attacker, &defender, false, &mut fails).unwrap();
    assert!(matches!(failed.outcome, HitOutcome::Hit { .. }));
    assert_eq!(failed.secondary_effect, SecondaryApplication::None);
}

#[test]
fn a_poison_type_or_steel_type_defender_never_reports_poisons_defender() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 10, vec![POISON_STING]);
    let poison_type_defender = mon(&dex, EKANS, 10, vec![TACKLE]);
    let mut rng = SequenceRng::new([
        ACCURACY_HIT_DRAW,
        ORDINARY_NO_CRIT_DRAW,
        BEST_DAMAGE_DRAW,
        POISON_CHANCE_HIT_DRAW,
    ]);
    let resolution = resolve_hit(
        &dex,
        POISON_STING,
        &attacker,
        &poison_type_defender,
        false,
        &mut rng,
    )
    .unwrap();
    assert!(
        matches!(resolution.outcome, HitOutcome::Hit { .. }),
        "fixture sanity: Poison is only not-very-effective against Poison, not immune, \
         so the hit must land for this to test the status guard rather than a coincidental miss"
    );
    assert_eq!(resolution.secondary_effect, SecondaryApplication::None);
}

/// Poison Sting's secondary is modelled (unlike Water Gun's, which has no
/// trampoline at all and so cannot distinguish this suppression from a
/// coincidental `false`). Wonder Guard must suppress it even on a roll that
/// would otherwise succeed, matching `MOVE_RESULT_MISSED`'s membership in
/// `MOVE_RESULT_NO_EFFECT` (`include/constants/battle.h:220-228`).
#[test]
fn wonder_guard_suppresses_poison_stings_secondary_on_a_successful_chance_roll() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 10, vec![POISON_STING]);
    let shedinja_defender = mon(&dex, SHEDINJA, 10, vec![TACKLE]);
    assert_eq!(shedinja_defender.ability(), AbilityId::WONDER_GUARD);
    let mut rng = SequenceRng::new([
        ACCURACY_HIT_DRAW,
        ORDINARY_NO_CRIT_DRAW,
        BEST_DAMAGE_DRAW,
        POISON_CHANCE_HIT_DRAW,
    ]);

    let resolution = resolve_hit(
        &dex,
        POISON_STING,
        &attacker,
        &shedinja_defender,
        false,
        &mut rng,
    )
    .unwrap();

    assert_eq!(
        resolution.outcome,
        HitOutcome::WonderGuardBlocked,
        "Poison is not-very-effective against Ghost and has no row against \
         Bug, so only Wonder Guard blocks this hit"
    );
    assert_eq!(
        resolution.secondary_effect,
        SecondaryApplication::None,
        "Wonder Guard must suppress the secondary effect even when the \
         chance roll would otherwise succeed"
    );
}

/// `MOVE_PSYBEAM`: `EFFECT_CONFUSE_HIT`, 100 accuracy, 10% chance.
const PSYBEAM: MoveId = MoveId::PSYBEAM;
/// `SPECIES_SPINDA`: Own Tempo in its only ability slot.
const SPINDA: SpeciesId = SpeciesId(308);
/// A draw that clears [`PSYBEAM`]'s 10% secondary chance.
const CONFUSE_CHANCE_HIT_DRAW: u16 = 9;
/// A draw that misses it.
const CONFUSE_CHANCE_MISS_DRAW: u16 = 10;

#[test]
fn effect_confuse_hit_is_admitted_though_not_ordinary() {
    let dex = Dex::new();
    let effect = dex.move_data(PSYBEAM).unwrap().effect;
    assert!(
        !is_ordinary_hit_effect(effect),
        "Psybeam needs the confuse trampoline, not the plain hit script"
    );
    assert_eq!(ensure_resolvable(&dex, PSYBEAM), Ok(()));
}

#[test]
fn a_landed_psybeam_reports_confuse_only_on_a_successful_roll() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 10, vec![PSYBEAM]);
    let defender = mon(&dex, SQUIRTLE, 10, vec![TACKLE]);
    assert_eq!(dex.move_data(PSYBEAM).unwrap().secondary_effect_chance, 10);

    let mut succeeds = SequenceRng::new([
        ACCURACY_HIT_DRAW,
        ORDINARY_NO_CRIT_DRAW,
        BEST_DAMAGE_DRAW,
        CONFUSE_CHANCE_HIT_DRAW,
    ]);
    let succeeded = resolve_hit(&dex, PSYBEAM, &attacker, &defender, false, &mut succeeds).unwrap();
    assert!(matches!(succeeded.outcome, HitOutcome::Hit { .. }));
    assert_eq!(succeeded.secondary_effect, SecondaryApplication::Confuse);
    assert_eq!(
        succeeds.draws(),
        4,
        "the duration draw is not part of resolve_hit; the caller draws it \
         only once the target's post-damage faint check clears"
    );

    let mut fails = SequenceRng::new([
        ACCURACY_HIT_DRAW,
        ORDINARY_NO_CRIT_DRAW,
        BEST_DAMAGE_DRAW,
        CONFUSE_CHANCE_MISS_DRAW,
    ]);
    let failed = resolve_hit(&dex, PSYBEAM, &attacker, &defender, false, &mut fails).unwrap();
    assert!(matches!(failed.outcome, HitOutcome::Hit { .. }));
    assert_eq!(failed.secondary_effect, SecondaryApplication::None);
}

#[test]
fn an_own_tempo_defender_never_reports_confuse() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 10, vec![PSYBEAM]);
    let defender = mon(&dex, SPINDA, 10, vec![TACKLE]);
    assert_eq!(defender.ability(), AbilityId::OWN_TEMPO);
    let mut rng = SequenceRng::new([
        ACCURACY_HIT_DRAW,
        ORDINARY_NO_CRIT_DRAW,
        BEST_DAMAGE_DRAW,
        CONFUSE_CHANCE_HIT_DRAW,
    ]);
    let resolution = resolve_hit(&dex, PSYBEAM, &attacker, &defender, false, &mut rng).unwrap();
    assert!(matches!(resolution.outcome, HitOutcome::Hit { .. }));
    assert_eq!(resolution.secondary_effect, SecondaryApplication::None);
    assert_eq!(rng.draws(), 4, "the chance draw still happens, discarded");
}

#[test]
fn admission_reports_unknown_and_non_damaging_moves_in_order() {
    let dex = Dex::new();
    assert_eq!(
        ensure_resolvable(&dex, UNKNOWN_MOVE),
        Err(BattleError::UnknownMove(UNKNOWN_MOVE))
    );
    assert_eq!(
        ensure_resolvable(&dex, GROWL),
        Err(BattleError::NonDamagingMove(GROWL))
    );
    assert_eq!(
        ensure_resolvable(&dex, CURSE),
        Err(BattleError::NonDamagingMove(CURSE))
    );
}

#[test]
fn armor_abilities_skip_the_critical_draw_and_prevent_critical_hits() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 5, vec![SLASH]);

    for armor_species in [ANORITH, SHELLDER] {
        let defender = mon(&dex, armor_species, 5, vec![TACKLE]);
        assert!(suppresses_critical_hits(defender.ability()));
        let mut rng = SequenceRng::new(CRITICAL_SUPPRESSED_DRAWS);

        let resolution = resolve_hit(&dex, SLASH, &attacker, &defender, false, &mut rng).unwrap();

        assert!(matches!(
            resolution.outcome,
            HitOutcome::Hit {
                is_critical: false,
                ..
            }
        ));
        assert_eq!(rng.draws(), CRITICAL_SUPPRESSED_DRAWS.len());
    }
}

#[test]
fn huge_power_in_ability_slot_two_doubles_a_physical_hit() {
    let dex = Dex::new();
    let defender = mon(&dex, SQUIRTLE, 5, vec![TACKLE]);
    let thick_fat = BattlePokemon::new(&dex, MARILL, 5, MAX_IVS, 0, vec![TACKLE])
        .unwrap()
        .with_ability_slot(0);
    let huge_power = BattlePokemon::new(&dex, MARILL, 5, MAX_IVS, 0, vec![TACKLE])
        .unwrap()
        .with_ability_slot(1);
    assert_eq!(thick_fat.ability(), THICK_FAT);
    assert_eq!(huge_power.ability(), HUGE_POWER);

    let mut thick_fat_rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);
    assert_eq!(
        resolve_hit(
            &dex,
            TACKLE,
            &thick_fat,
            &defender,
            false,
            &mut thick_fat_rng,
        )
        .unwrap()
        .outcome,
        HitOutcome::Hit {
            damage: THICK_FAT_MARILL_TACKLE_DAMAGE,
            is_critical: false,
        }
    );

    let mut huge_power_rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);
    assert_eq!(
        resolve_hit(
            &dex,
            TACKLE,
            &huge_power,
            &defender,
            false,
            &mut huge_power_rng,
        )
        .unwrap()
        .outcome,
        HitOutcome::Hit {
            damage: HUGE_POWER_MARILL_TACKLE_DAMAGE,
            is_critical: false,
        }
    );
}

#[test]
fn pure_power_doubles_a_physical_hit() {
    let dex = Dex::new();
    let attacker = mon(&dex, MEDITITE, 5, vec![TACKLE]);
    let defender = mon(&dex, SQUIRTLE, 5, vec![TACKLE]);
    assert_eq!(attacker.ability(), PURE_POWER);
    let mut rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);

    let resolution = resolve_hit(&dex, TACKLE, &attacker, &defender, false, &mut rng).unwrap();

    assert_eq!(
        resolution.outcome,
        HitOutcome::Hit {
            damage: PURE_POWER_MEDITITE_TACKLE_DAMAGE,
            is_critical: false,
        }
    );
}

#[test]
fn huge_power_never_touches_a_special_move() {
    let dex = Dex::new();
    let defender = mon(&dex, SQUIRTLE, 5, vec![TACKLE]);
    let thick_fat = BattlePokemon::new(&dex, MARILL, 5, MAX_IVS, 0, vec![WATER_GUN])
        .unwrap()
        .with_ability_slot(0);
    let huge_power = BattlePokemon::new(&dex, MARILL, 5, MAX_IVS, 0, vec![WATER_GUN])
        .unwrap()
        .with_ability_slot(1);

    let mut thick_fat_rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);
    let thick_fat_outcome = resolve_hit(
        &dex,
        WATER_GUN,
        &thick_fat,
        &defender,
        false,
        &mut thick_fat_rng,
    )
    .unwrap();
    let mut huge_power_rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);
    let huge_power_outcome = resolve_hit(
        &dex,
        WATER_GUN,
        &huge_power,
        &defender,
        false,
        &mut huge_power_rng,
    )
    .unwrap();

    assert_eq!(thick_fat_outcome.outcome, huge_power_outcome.outcome);
}

#[test]
fn huge_power_doubles_raw_attack_before_stat_stage_scaling() {
    let dex = Dex::new();
    let mut attacker = BattlePokemon::new(&dex, MARILL, 15, MAX_IVS, 0, vec![TACKLE])
        .unwrap()
        .with_ability_slot(1);
    attacker.stages_mut().attack = StatStage::new(-2).unwrap();
    let defender = mon(&dex, ABRA, 15, vec![TACKLE]);
    let mut rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);

    let resolution = resolve_hit(&dex, TACKLE, &attacker, &defender, false, &mut rng).unwrap();

    assert_eq!(
        resolution.outcome,
        HitOutcome::Hit {
            damage: HUGE_POWER_BEFORE_STAGE_DAMAGE,
            is_critical: false,
        }
    );
}

#[test]
fn a_statused_guts_attacker_raises_physical_damage() {
    let dex = Dex::new();
    let healthy = BattlePokemon::new(&dex, MAKUHITA, 5, MAX_IVS, 0, vec![TACKLE])
        .unwrap()
        .with_ability_slot(1);
    assert_eq!(healthy.ability(), GUTS);
    let mut statused = healthy.clone();
    statused.set_status1(Status1::Paralysed);
    let defender = mon(&dex, MILOTIC, 5, vec![TACKLE]);

    let mut healthy_rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);
    let healthy_outcome =
        resolve_hit(&dex, TACKLE, &healthy, &defender, false, &mut healthy_rng).unwrap();
    assert_eq!(
        healthy_outcome.outcome,
        HitOutcome::Hit {
            damage: HEALTHY_MAKUHITA_TACKLE_DAMAGE,
            is_critical: false,
        }
    );

    let mut statused_rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);
    let statused_outcome =
        resolve_hit(&dex, TACKLE, &statused, &defender, false, &mut statused_rng).unwrap();
    assert_eq!(
        statused_outcome.outcome,
        HitOutcome::Hit {
            damage: GUTS_MAKUHITA_TACKLE_DAMAGE,
            is_critical: false,
        }
    );
}

#[test]
fn a_statused_marvel_scale_defender_lowers_physical_damage() {
    let dex = Dex::new();
    let attacker = mon(&dex, MAKUHITA, 5, vec![TACKLE]);
    let healthy_defender = mon(&dex, MILOTIC, 5, vec![TACKLE]);
    assert_eq!(healthy_defender.ability(), MARVEL_SCALE);
    let mut statused_defender = healthy_defender.clone();
    statused_defender.set_status1(Status1::Poisoned);

    let mut healthy_rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);
    let healthy_outcome = resolve_hit(
        &dex,
        TACKLE,
        &attacker,
        &healthy_defender,
        false,
        &mut healthy_rng,
    )
    .unwrap();
    assert_eq!(
        healthy_outcome.outcome,
        HitOutcome::Hit {
            damage: HEALTHY_MAKUHITA_TACKLE_DAMAGE,
            is_critical: false,
        }
    );

    let mut statused_rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);
    let statused_outcome = resolve_hit(
        &dex,
        TACKLE,
        &attacker,
        &statused_defender,
        false,
        &mut statused_rng,
    )
    .unwrap();
    assert_eq!(
        statused_outcome.outcome,
        HitOutcome::Hit {
            damage: MARVEL_SCALE_MILOTIC_TACKLE_DAMAGE,
            is_critical: false,
        }
    );
}

#[test]
fn guts_never_touches_special_attack() {
    let dex = Dex::new();
    let mut attacker = BattlePokemon::new(&dex, MAKUHITA, 5, MAX_IVS, 0, vec![WATER_GUN])
        .unwrap()
        .with_ability_slot(1);
    assert_eq!(attacker.ability(), GUTS);
    let healthy_special_attack = attacker.attacking_stat(MoveCategory::Special);

    attacker.set_status1(Status1::Paralysed);

    assert_eq!(
        attacker.attacking_stat(MoveCategory::Special),
        healthy_special_attack,
        "Guts must not touch Special Attack even once its holder is statused"
    );
}

#[test]
fn marvel_scale_never_touches_special_defense() {
    let dex = Dex::new();
    let mut defender = mon(&dex, MILOTIC, 5, vec![WATER_GUN]);
    assert_eq!(defender.ability(), MARVEL_SCALE);
    let healthy_special_defense = defender.defending_stat(MoveCategory::Special);

    defender.set_status1(Status1::Poisoned);

    assert_eq!(
        defender.defending_stat(MoveCategory::Special),
        healthy_special_defense,
        "Marvel Scale must not touch Special Defense even once its holder is statused"
    );
}

#[test]
fn compound_eyes_raises_the_accuracy_threshold_of_an_executable_move() {
    let dex = Dex::new();
    let attacker = mon(&dex, BUTTERFREE, 10, vec![SLAM]);
    assert_eq!(attacker.ability(), AbilityId::COMPOUND_EYES);
    let defender = mon(&dex, SQUIRTLE, 10, vec![TACKLE]);
    let mut rng = SequenceRng::new([
        COMPOUND_EYES_ONLY_HIT_DRAW,
        ORDINARY_NO_CRIT_DRAW,
        BEST_DAMAGE_DRAW,
        DISCARDED_EFFECT_DRAW,
    ]);

    let resolution = resolve_hit(&dex, SLAM, &attacker, &defender, false, &mut rng).unwrap();

    assert!(
        matches!(resolution.outcome, HitOutcome::Hit { .. }),
        "roll 91 is within Compound Eyes' 97 threshold: {:?}",
        resolution.outcome
    );
    assert_eq!(
        rng.draws(),
        4,
        "a Compound Eyes hit continues through the remaining move draws"
    );
}

#[test]
fn hustle_lowers_the_accuracy_threshold_of_a_physical_executable_move() {
    let dex = Dex::new();
    let attacker = mon(&dex, REMORAID, 10, vec![SLAM]);
    assert_eq!(attacker.ability(), AbilityId::HUSTLE);
    let defender = mon(&dex, SQUIRTLE, 10, vec![TACKLE]);
    let mut rng = SequenceRng::new([HUSTLE_ONLY_MISS_DRAW]);

    let resolution = resolve_hit(&dex, SLAM, &attacker, &defender, false, &mut rng).unwrap();

    assert_eq!(
        resolution.outcome,
        HitOutcome::Miss,
        "roll 66 exceeds Hustle's 60 threshold"
    );
    assert_eq!(rng.draws(), 1, "a miss stops immediately");
}

#[test]
fn hustle_raises_a_physical_attackers_landed_damage() {
    let dex = Dex::new();
    let hustle_attacker = BattlePokemon::new(&dex, DELIBIRD, 10, MAX_IVS, 0, vec![TACKLE])
        .unwrap()
        .with_ability_slot(1);
    assert_eq!(hustle_attacker.ability(), AbilityId::HUSTLE);
    let control = BattlePokemon::new(&dex, DELIBIRD, 10, MAX_IVS, 0, vec![TACKLE])
        .unwrap()
        .with_ability_slot(0);
    assert_ne!(control.ability(), AbilityId::HUSTLE);
    let defender = mon(&dex, SQUIRTLE, 10, vec![TACKLE]);

    let mut control_rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);
    let HitOutcome::Hit {
        damage: control_damage,
        ..
    } = resolve_hit(&dex, TACKLE, &control, &defender, false, &mut control_rng)
        .unwrap()
        .outcome
    else {
        panic!("Tackle must land on a Water/Water matchup");
    };

    // Pinned independently through `base_damage` with the control's raw
    // Attack scaled 150%, rather than derived from `control_damage`, since
    // `pokemon.c:3205-3206`'s truncation happens before the stage multiply
    // and would not generally commute with scaling the already-rounded
    // control figure.
    let (raw_attack, attack_stage) = control.attacking_stat(MoveCategory::Physical);
    let (defense_stat, defense_stage) = defender.defending_stat(MoveCategory::Physical);
    let expected_hustle_damage = base_damage(&DamageInput {
        attacker_level: control.level(),
        power: u32::from(dex.move_data(TACKLE).unwrap().power),
        move_type: Type::Normal,
        attack_stat: 150 * raw_attack / 100,
        attack_stage,
        defense_stat,
        defense_stage,
        attacker_burned: false,
        reflect: false,
        light_screen: false,
        weather: Weather::None,
        is_solar_beam: false,
        attacker_pinch_boost: false,
    });
    assert!(
        expected_hustle_damage > control_damage,
        "fixture must be sensitive to the boost: {expected_hustle_damage} vs {control_damage}"
    );

    let mut hustle_rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);
    let resolution = resolve_hit(
        &dex,
        TACKLE,
        &hustle_attacker,
        &defender,
        false,
        &mut hustle_rng,
    )
    .unwrap();

    assert_eq!(
        resolution.outcome,
        HitOutcome::Hit {
            damage: expected_hustle_damage,
            is_critical: false,
        },
        "a Hustle attacker's physical damage must carry the 150% raw-Attack boost"
    );
}

#[test]
fn hustle_never_touches_a_special_move() {
    let dex = Dex::new();
    let defender = mon(&dex, SQUIRTLE, 10, vec![TACKLE]);
    let hustle_attacker = BattlePokemon::new(&dex, DELIBIRD, 10, MAX_IVS, 0, vec![WATER_GUN])
        .unwrap()
        .with_ability_slot(1);
    let control = BattlePokemon::new(&dex, DELIBIRD, 10, MAX_IVS, 0, vec![WATER_GUN])
        .unwrap()
        .with_ability_slot(0);
    assert_eq!(hustle_attacker.ability(), AbilityId::HUSTLE);
    assert_ne!(control.ability(), AbilityId::HUSTLE);

    let mut hustle_rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);
    let hustle_outcome = resolve_hit(
        &dex,
        WATER_GUN,
        &hustle_attacker,
        &defender,
        false,
        &mut hustle_rng,
    )
    .unwrap();
    let mut control_rng = SequenceRng::new(ORDINARY_NON_CRITICAL_DRAWS);
    let control_outcome = resolve_hit(
        &dex,
        WATER_GUN,
        &control,
        &defender,
        false,
        &mut control_rng,
    )
    .unwrap();

    assert_eq!(hustle_outcome.outcome, control_outcome.outcome);
}

/// `Cmd_accuracycheck` does not stop a failed roll at the generic miss: it
/// calls `CheckWonderGuardAndLevitate` (`battle_script_commands.c:1175-1186`),
/// whose type scan flags `MOVE_RESULT_DOESNT_AFFECT_FOE` (`:1454-1469`) so
/// `Cmd_resultmessage` prints `STRINGID_ITDOESNTAFFECT` instead of
/// `STRINGID_ATTACKMISSED` (`:2055-2059`).
#[test]
fn a_failed_accuracy_roll_against_an_immune_target_reports_the_immunity() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 20, vec![TACKLE]);
    let ghost_defender = mon(&dex, GASTLY, 20, vec![TACKLE]);
    let mut rng = SequenceRng::new([TACKLE_MISS_DRAW]);

    let resolution =
        resolve_hit(&dex, TACKLE, &attacker, &ghost_defender, false, &mut rng).unwrap();

    assert_eq!(resolution.outcome, HitOutcome::NoEffect);
    assert_eq!(resolution.secondary_effect, SecondaryApplication::None);
    assert_eq!(
        rng.draws(),
        1,
        "reclassifying the failed roll must not spend a critical, damage, or \
         effect-chance draw"
    );
}

/// The same helper lets Wonder Guard overwrite `MISS_TYPE` with
/// `B_MSG_AVOIDED_DMG` (`battle_script_commands.c:1490-1497`), which
/// `Cmd_resultmessage` prefers over the typing-immunity message
/// (`:2055-2059`). Slam is Normal, so Shedinja's Ghost half makes it
/// independently immune as well: Wonder Guard still wins.
#[test]
fn a_failed_accuracy_roll_against_wonder_guard_reports_wonder_guard() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 20, vec![SLAM]);
    let shedinja_defender = mon(&dex, SHEDINJA, 20, vec![TACKLE]);
    assert_eq!(shedinja_defender.ability(), AbilityId::WONDER_GUARD);
    let mut rng = SequenceRng::new([SLAM_MISS_DRAW]);

    let resolution =
        resolve_hit(&dex, SLAM, &attacker, &shedinja_defender, false, &mut rng).unwrap();

    assert_eq!(resolution.outcome, HitOutcome::WonderGuardBlocked);
    assert_eq!(
        rng.draws(),
        1,
        "reclassifying the failed roll must not spend a critical, damage, or \
         effect-chance draw"
    );
}

/// Levitate returns from `CheckWonderGuardAndLevitate` before its type scan
/// (`battle_script_commands.c:1435-1443`), so a Ground move that misses a
/// Levitate holder reports the Ground miss. No admitted ordinary-hit move is
/// Ground-type, so the classifier is exercised directly here; the multi-hit
/// pipeline covers it end to end with Bone Rush.
#[test]
fn a_failed_accuracy_roll_against_levitate_reports_levitate() {
    let dex = Dex::new();
    let levitate_defender = mon(&dex, GASTLY, 20, vec![TACKLE]);
    assert_eq!(levitate_defender.ability(), AbilityId::LEVITATE);

    assert_eq!(
        classify_accuracy_failure(&dex, BONE_RUSH, &levitate_defender),
        Ok(HitOutcome::LevitateBlocked)
    );
}

/// `CheckWonderGuardAndLevitate` returns immediately for Struggle and for a
/// zero-power move (`battle_script_commands.c:1432-1433`), leaving
/// `MISS_TYPE = B_MSG_MISSED`. An ordinary matchup keeps the generic miss for
/// the same reason: nothing in the helper matches.
#[test]
fn struggle_and_an_ordinary_matchup_keep_the_generic_miss() {
    let dex = Dex::new();
    let ghost_defender = mon(&dex, GASTLY, 20, vec![TACKLE]);
    let ordinary_defender = mon(&dex, SQUIRTLE, 20, vec![TACKLE]);

    assert_eq!(
        classify_accuracy_failure(&dex, STRUGGLE, &ghost_defender),
        Ok(HitOutcome::Miss),
        "Struggle skips the helper entirely, so its immunity never applies"
    );
    assert_eq!(
        classify_accuracy_failure(&dex, GROWL, &ordinary_defender),
        Ok(HitOutcome::Miss),
        "a zero-power move returns before the helper reads any typing"
    );
    assert_eq!(
        classify_accuracy_failure(&dex, TACKLE, &ordinary_defender),
        Ok(HitOutcome::Miss)
    );
}

/// The classifier reads only move data and the defender, so it can never
/// disturb the draw sequence the failed accuracy roll left behind.
#[test]
fn classifying_a_failed_roll_rejects_an_unknown_move_and_draws_nothing() {
    let dex = Dex::new();
    let defender = mon(&dex, SQUIRTLE, 20, vec![TACKLE]);

    assert_eq!(
        classify_accuracy_failure(&dex, UNKNOWN_MOVE, &defender),
        Err(BattleError::UnknownMove(UNKNOWN_MOVE))
    );
}

/// Upstream halves the raw (odd-truncated) Special Attack for Thick Fat before
/// stage scaling (`pokemon.c:3202-3204`), so a +1 stage scales the halved stat.
#[test]
fn thick_fat_halves_raw_special_attack_before_stage_scaling_in_the_raw_damage_input() {
    let dex = Dex::new();
    let mut attacker = mon(&dex, ABRA, 15, vec![ICICLE_SPEAR]);
    attacker.stages_mut().sp_attack = StatStage::new(1).unwrap();
    let raw_special_attack = attacker.stats().sp_attack;
    assert_eq!(
        raw_special_attack % 2,
        1,
        "the stat must be odd to pin truncation"
    );
    let thick_fat = BattlePokemon::new(&dex, MARILL, 5, MAX_IVS, 0, vec![TACKLE])
        .unwrap()
        .with_ability_slot(0);
    assert_eq!(thick_fat.ability(), THICK_FAT);
    let (defense, defense_stage) = thick_fat.defending_stat(MoveCategory::Special);
    let expected_base = base_damage(&DamageInput {
        attacker_level: attacker.level(),
        power: 10,
        move_type: Type::Ice,
        attack_stat: raw_special_attack / 2,
        attack_stage: StatStage::new(1).unwrap(),
        defense_stat: defense,
        defense_stage,
        attacker_burned: false,
        reflect: false,
        light_screen: false,
        weather: Weather::None,
        is_solar_beam: false,
        attacker_pinch_boost: false,
    });
    let mut rng = SequenceRng::new([ORDINARY_NO_CRIT_DRAW]);

    let raw =
        damage_before_roll(&dex, ICICLE_SPEAR, &attacker, &thick_fat, false, &mut rng).unwrap();

    assert_eq!(
        raw.damage,
        apply_dual_type_effectiveness(expected_base, Type::Ice, thick_fat.types())
    );
    assert_eq!(rng.draws(), 1, "Thick Fat spends no extra draw");
}

/// Thick Fat halves raw 7 to 3 and -6 truncates it to 0, which upstream never
/// floors (`pokemon.c:3100-3104`, `:3298-3315`): 2 HP per hit, not 3.
#[test]
fn icicle_spear_into_thick_fat_keeps_a_zero_stage_adjusted_special_attack() {
    const SMEARGLE: SpeciesId = SpeciesId(235);
    const ADAMANT_PERSONALITY: u32 = 3;
    const NAIVE_PERSONALITY: u32 = 14;
    const ZERO_IVS: Ivs = Ivs {
        hp: 0,
        attack: 0,
        defense: 0,
        speed: 0,
        sp_attack: 0,
        sp_defense: 0,
    };
    let dex = Dex::new();
    let mut attacker = BattlePokemon::new(
        &dex,
        SMEARGLE,
        8,
        ZERO_IVS,
        ADAMANT_PERSONALITY,
        vec![ICICLE_SPEAR],
    )
    .unwrap();
    attacker.stages_mut().sp_attack = StatStage::MIN;
    let mut defender =
        BattlePokemon::new(&dex, MAKUHITA, 5, ZERO_IVS, NAIVE_PERSONALITY, vec![TACKLE])
            .unwrap()
            .with_ability_slot(0);
    defender.stages_mut().sp_defense = StatStage::MIN;
    assert_eq!(attacker.stats().sp_attack, 7);
    assert_eq!(defender.stats().sp_defense, 7);
    assert_eq!(defender.ability(), THICK_FAT);
    let mut rng = SequenceRng::new([ORDINARY_NO_CRIT_DRAW, BEST_DAMAGE_DRAW]);

    let outcome = damage_core(&dex, ICICLE_SPEAR, &attacker, &defender, false, &mut rng).unwrap();

    assert_eq!(
        outcome,
        HitOutcome::Hit {
            damage: 2,
            is_critical: false
        }
    );
    assert_eq!(rng.draws(), 2);
}

/// `SPECIES_VULPIX`: Flash Fire in its primary (and only) ability slot.
const VULPIX: SpeciesId = SpeciesId(37);
/// `MOVE_EMBER` -- `EFFECT_BURN_HIT`, so `resolve_hit` would refuse it at
/// admission; the damage entry accepts its move data directly.
const EMBER: MoveId = MoveId::EMBER;

#[test]
fn flash_fire_absorbs_a_fire_move_without_drawing_and_latches_once() {
    let dex = Dex::new();
    let attacker = mon(&dex, BULBASAUR, 20, vec![EMBER]);
    let mut vulpix = mon(&dex, VULPIX, 20, vec![TACKLE]);
    assert_eq!(vulpix.ability(), AbilityId::FLASH_FIRE);
    let mut rng = SequenceRng::new([]);

    let first = damage_core(&dex, EMBER, &attacker, &vulpix, false, &mut rng).unwrap();
    assert_eq!(
        first,
        HitOutcome::FlashFireBlocked {
            first_activation: true
        }
    );
    assert!(vulpix.volatiles_mut().activate_flash_fire());

    // Later activations still cancel the move but report no boost, and the
    // latch is not a counter.
    let second = damage_core(&dex, EMBER, &attacker, &vulpix, false, &mut rng).unwrap();
    assert_eq!(
        second,
        HitOutcome::FlashFireBlocked {
            first_activation: false
        }
    );
    assert!(!vulpix.volatiles_mut().activate_flash_fire());
    assert_eq!(rng.draws(), 0, "absorption precedes every draw");

    // Unlike Volt Absorb and Water Absorb, there is no power gate and no
    // `MOVE_RESULT_NO_EFFECT` write; non-Fire moves are unaffected.
    let mut rng = SequenceRng::new([ORDINARY_NO_CRIT_DRAW, BEST_DAMAGE_DRAW]);
    let tackle = damage_core(&dex, TACKLE, &attacker, &vulpix, false, &mut rng).unwrap();
    assert!(matches!(tackle, HitOutcome::Hit { .. }));
}

#[test]
fn a_latched_flash_fire_boosts_ember_before_the_final_plus_two() {
    let dex = Dex::new();
    let control = mon(&dex, VULPIX, 20, vec![EMBER]);
    let mut latched = mon(&dex, VULPIX, 20, vec![EMBER]);
    latched.volatiles_mut().activate_flash_fire();
    let defender = mon(&dex, BULBASAUR, 20, vec![TACKLE]);
    let draws = [ORDINARY_NO_CRIT_DRAW, BEST_DAMAGE_DRAW];

    let unboosted = damage_before_roll(
        &dex,
        EMBER,
        &control,
        &defender,
        false,
        &mut SequenceRng::new(draws),
    )
    .unwrap()
    .damage;
    let boosted = damage_before_roll(
        &dex,
        EMBER,
        &latched,
        &defender,
        false,
        &mut SequenceRng::new(draws),
    )
    .unwrap()
    .damage;

    // Reproduce the pre-STAB base so the expectation fixes the order: the
    // 15/10 multiplier truncates before the constant 2 is added.
    let input = |boost: bool| {
        let move_data = dex.move_data(EMBER).unwrap();
        let category = MoveCategory::Special;
        let (attack, attack_stage) = control.attacking_stat(category);
        let (defense, defense_stage) = defender.defending_stat(category);
        let input = DamageInput {
            attacker_level: 20,
            power: u32::from(move_data.power),
            move_type: Type::Fire,
            attack_stat: attack,
            attack_stage,
            defense_stat: defense,
            defense_stage,
            attacker_burned: false,
            reflect: false,
            light_screen: false,
            weather: Weather::None,
            is_solar_beam: false,
            attacker_pinch_boost: false,
        };
        crate::damage::base_damage_with_flash_fire(&input, boost)
    };
    let plain = input(false);
    let with_boost = input(true);
    assert_eq!(with_boost, (plain - 2) * 15 / 10 + 2);
    assert!(with_boost > plain);
    assert!(boosted > unboosted, "{boosted} vs {unboosted}");

    // Only Fire moves are boosted.
    let tackle_control = damage_before_roll(
        &dex,
        TACKLE,
        &control,
        &defender,
        false,
        &mut SequenceRng::new(draws),
    )
    .unwrap()
    .damage;
    let tackle_latched = damage_before_roll(
        &dex,
        TACKLE,
        &latched,
        &defender,
        false,
        &mut SequenceRng::new(draws),
    )
    .unwrap()
    .damage;
    assert_eq!(tackle_latched, tackle_control);
}

/// Upstream raises raw Special Attack 150% for Plus against Minus and for
/// Minus against Plus, opponents included, before stage scaling
/// (`pokemon.c:3207-3210`, `battle_util.c:3128-3137`).
fn plus_minus_expected_damage(
    dex: &Dex,
    attacker: &BattlePokemon,
    defender: &BattlePokemon,
    boosted: bool,
) -> u32 {
    let raw = attacker.stats().sp_attack;
    let (defense, defense_stage) = defender.defending_stat(MoveCategory::Special);
    let expected_base = base_damage(&DamageInput {
        attacker_level: attacker.level(),
        power: u32::from(dex.move_data(WATER_GUN).unwrap().power),
        move_type: Type::Water,
        attack_stat: if boosted { raw * 150 / 100 } else { raw },
        attack_stage: attacker.stages().sp_attack,
        defense_stat: defense,
        defense_stage,
        attacker_burned: false,
        reflect: false,
        light_screen: false,
        weather: Weather::None,
        is_solar_beam: false,
        attacker_pinch_boost: false,
    });
    apply_dual_type_effectiveness(expected_base, Type::Water, defender.types())
}

#[test]
fn plus_and_minus_boost_special_attack_against_the_complementary_opponent() {
    let dex = Dex::new();
    let mut plusle = mon(&dex, SpeciesId(353), 20, vec![WATER_GUN]);
    let mut minun = mon(&dex, SpeciesId(354), 20, vec![WATER_GUN]);
    assert_eq!(plusle.ability(), AbilityId::PLUS);
    assert_eq!(minun.ability(), AbilityId::MINUS);
    // An odd raw stat at stage +2 separates "boost then stage" from any other order.
    plusle.stages_mut().sp_attack = StatStage::new(2).unwrap();
    minun.stages_mut().sp_attack = StatStage::new(2).unwrap();
    assert_eq!(plusle.stats().sp_attack % 2, 1, "pin truncation");
    assert_eq!(minun.stats().sp_attack % 2, 1, "pin truncation");

    for (attacker, defender) in [(&plusle, &minun), (&minun, &plusle)] {
        let mut rng = SequenceRng::new([ORDINARY_NO_CRIT_DRAW]);
        let raw = damage_before_roll(&dex, WATER_GUN, attacker, defender, false, &mut rng).unwrap();
        assert_eq!(
            raw.damage,
            plus_minus_expected_damage(&dex, attacker, defender, true)
        );
        assert_ne!(
            raw.damage,
            plus_minus_expected_damage(&dex, attacker, defender, false)
        );
    }
}

#[test]
fn plus_without_a_complementary_opponent_gets_no_special_attack_boost() {
    let dex = Dex::new();
    let plusle = mon(&dex, SpeciesId(353), 20, vec![WATER_GUN]);
    let squirtle = mon(&dex, SQUIRTLE, 20, vec![WATER_GUN]);
    let other_plusle = mon(&dex, SpeciesId(353), 20, vec![WATER_GUN]);

    for defender in [&squirtle, &other_plusle] {
        let mut rng = SequenceRng::new([ORDINARY_NO_CRIT_DRAW]);
        let raw = damage_before_roll(&dex, WATER_GUN, &plusle, defender, false, &mut rng).unwrap();
        assert_eq!(
            raw.damage,
            plus_minus_expected_damage(&dex, &plusle, defender, false)
        );
    }
}
