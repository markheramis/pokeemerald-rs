//! `ShouldSwitch` and the seven probes it runs before a trainer scores a move
//! (`pokeemerald/src/battle_ai_switch_items.c:20`-`:526`).
//!
//! A bench of at least one healthy original slot is required before any probe
//! draws. Probes then run in upstream order and may return
//! [`crate::battle::opponent_ai::EnemyAction::Switch`] ahead of move scoring.
//! Doubles, party-half splits, and the trapping early-outs (wrap, Ingrain,
//! Shadow Tag, Arena Trap, Magnet Pull, Arena) are not modelled.
//! `ShouldUseItem` is not modelled.

use assets::{AbilityId, Effectiveness, MoveId, Type, TypeChart};

use super::super::battler_state::{BattlerSide, BattlerState, LastLandedMove, PerishSongTurns};
use super::super::trainer::TrainerContext;
use crate::damage::BattleRng;
use crate::dex::Dex;
use crate::error::BattleError;
use crate::pokemon::{BattlePokemon, MOVE_NONE};

/// The inputs a voluntary switch reads off the live battle.
pub(crate) struct SwitchFacts<'a> {
    /// The trainer party, in original slots.
    pub party: &'a TrainerContext,
    /// The active enemy's landed-move, hitter, and Perish Song state.
    pub enemy_state: &'a BattlerState,
    /// `gCurrentMove` as the suitability fallback still sees it.
    pub resolving_move: MoveId,
}

#[derive(Debug, Clone, Copy, Default)]
struct TypeFlags {
    doesnt_affect: bool,
    not_very: bool,
    super_effective: bool,
}

/// Runs `ShouldSwitch`. `Ok(Some(slot))` is a switch to that original party
/// slot. `Ok(None)` means score a move, after whatever draws the probes took.
///
/// # Errors
///
/// Returns [`BattleError::UndefinedNaturalCureLandedMove`] when Natural Cure
/// would read `gBattleMoves[MOVE_UNAVAILABLE]`, or a missing-move error from a
/// type calculation.
pub(crate) fn consider_switch(
    dex: &Dex,
    active: &BattlePokemon,
    player: &BattlePokemon,
    facts: &SwitchFacts<'_>,
    rng: &mut impl BattleRng,
) -> Result<Option<usize>, BattleError> {
    // `availableToSwitch == 0` returns before any probe (`:507`-`:508`).
    if facts.party.bench_len() == 0 {
        return Ok(None);
    }
    if perish_song_ends(*facts.enemy_state) {
        return most_suitable(dex, active, player, facts);
    }
    if let Some(slot) = wonder_guard(dex, active, player, facts.party, rng)? {
        return Ok(Some(slot));
    }
    if let Some(slot) =
        absorbs_last_move(dex, active, player, *facts.enemy_state, facts.party, rng)?
    {
        return Ok(Some(slot));
    }
    if let Some(slot) = natural_cure(dex, active, player, facts, rng)? {
        return Ok(Some(slot));
    }
    if has_super_effective_move(dex, active, player, false, rng)? {
        return Ok(None);
    }
    if stats_are_raised(active) {
        return Ok(None);
    }
    if let Some(slot) = find_mon_with_flags(
        dex,
        player,
        *facts.enemy_state,
        facts.party,
        TypeFlags {
            doesnt_affect: true,
            ..TypeFlags::default()
        },
        2,
        rng,
    )? {
        return Ok(Some(slot));
    }
    find_mon_with_flags(
        dex,
        player,
        *facts.enemy_state,
        facts.party,
        TypeFlags {
            not_very: true,
            ..TypeFlags::default()
        },
        3,
        rng,
    )
}

fn perish_song_ends(state: BattlerState) -> bool {
    // Timer 0 is the turn the countdown ends (`:22`-`:23`).
    matches!(state.perish_song(), Some(PerishSongTurns(0)))
}

fn most_suitable(
    dex: &Dex,
    active: &BattlePokemon,
    player: &BattlePokemon,
    facts: &SwitchFacts<'_>,
) -> Result<Option<usize>, BattleError> {
    // `PARTY_SIZE` defers to `GetMostSuitableMonToSwitchInto` (`:545`-`:547`).
    facts
        .party
        .select_replacement(dex, active, facts.resolving_move, player)
}

fn wonder_guard(
    dex: &Dex,
    active: &BattlePokemon,
    player: &BattlePokemon,
    party: &TrainerContext,
    rng: &mut impl BattleRng,
) -> Result<Option<usize>, BattleError> {
    if player.ability() != AbilityId::WONDER_GUARD {
        return Ok(None);
    }
    if active_has_super_effective(dex, active, player)? {
        return Ok(None);
    }
    for (slot, candidate) in party.candidates() {
        if candidate.is_fainted() {
            continue;
        }
        for known in candidate.moves() {
            if !type_flags(dex, known.move_id, player)?.super_effective {
                continue;
            }
            // `Random() % 3 < 2` (`:106`).
            if rng.next_u16() % 3 < 2 {
                return Ok(Some(slot));
            }
        }
    }
    Ok(None)
}

fn absorbs_last_move(
    dex: &Dex,
    active: &BattlePokemon,
    player: &BattlePokemon,
    state: BattlerState,
    party: &TrainerContext,
    rng: &mut impl BattleRng,
) -> Result<Option<usize>, BattleError> {
    if has_super_effective_move(dex, active, player, true, rng)?
        && !rng.next_u16().is_multiple_of(3)
    {
        return Ok(None);
    }
    let LastLandedMove::Move(landed) = state.last_landed_move() else {
        return Ok(None);
    };
    let data = dex.move_data(landed)?;
    if data.power == 0 {
        return Ok(None);
    }
    let Some(move_type) = data.move_type.battle_type() else {
        return Ok(None);
    };
    let absorbing = match move_type {
        Type::Fire => AbilityId::FLASH_FIRE,
        Type::Water => AbilityId::WATER_ABSORB,
        Type::Electric => AbilityId::VOLT_ABSORB,
        _ => return Ok(None),
    };
    if active.ability() == absorbing {
        return Ok(None);
    }
    for (slot, candidate) in party.candidates() {
        if candidate.is_fainted() || candidate.ability() != absorbing {
            continue;
        }
        // `Random() & 1` (`:206`).
        if rng.next_u16() & 1 == 1 {
            return Ok(Some(slot));
        }
    }
    Ok(None)
}

fn natural_cure(
    dex: &Dex,
    active: &BattlePokemon,
    player: &BattlePokemon,
    facts: &SwitchFacts<'_>,
    rng: &mut impl BattleRng,
) -> Result<Option<usize>, BattleError> {
    if !active.status1().is_asleep()
        || active.ability() != AbilityId::NATURAL_CURE
        || active.current_hp() < active.stats().max_hp / 2
    {
        return Ok(None);
    }
    let landed = facts.enemy_state.last_landed_move();
    let undefined_or_none = matches!(landed, LastLandedMove::None | LastLandedMove::Unavailable);
    if undefined_or_none && rng.next_u16() & 1 == 1 {
        return most_suitable(dex, active, player, facts);
    }
    if matches!(landed, LastLandedMove::Unavailable) {
        return Err(BattleError::UndefinedNaturalCureLandedMove);
    }
    let move_id = match landed {
        LastLandedMove::Move(move_id) => move_id,
        LastLandedMove::None => MOVE_NONE,
        LastLandedMove::Unavailable => unreachable!("refused before the move-table read"),
    };
    if dex.move_data(move_id)?.power == 0 && rng.next_u16() & 1 == 1 {
        return most_suitable(dex, active, player, facts);
    }
    if let Some(slot) = find_mon_with_flags(
        dex,
        player,
        *facts.enemy_state,
        facts.party,
        TypeFlags {
            doesnt_affect: true,
            ..TypeFlags::default()
        },
        1,
        rng,
    )? {
        return Ok(Some(slot));
    }
    if let Some(slot) = find_mon_with_flags(
        dex,
        player,
        *facts.enemy_state,
        facts.party,
        TypeFlags {
            not_very: true,
            ..TypeFlags::default()
        },
        1,
        rng,
    )? {
        return Ok(Some(slot));
    }
    if rng.next_u16() & 1 == 1 {
        return most_suitable(dex, active, player, facts);
    }
    Ok(None)
}

/// `HasSuperEffectiveMoveAgainstOpponents`. `no_rng` is the absorber's gate;
/// the later call passes `false` and draws `Random() % 10`.
fn has_super_effective_move(
    dex: &Dex,
    active: &BattlePokemon,
    player: &BattlePokemon,
    no_rng: bool,
    rng: &mut impl BattleRng,
) -> Result<bool, BattleError> {
    for known in active.moves() {
        if !type_flags(dex, known.move_id, player)?.super_effective {
            continue;
        }
        if no_rng {
            return Ok(true);
        }
        if !rng.next_u16().is_multiple_of(10) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn active_has_super_effective(
    dex: &Dex,
    active: &BattlePokemon,
    player: &BattlePokemon,
) -> Result<bool, BattleError> {
    for known in active.moves() {
        if type_flags(dex, known.move_id, player)?.super_effective {
            return Ok(true);
        }
    }
    Ok(false)
}

fn stats_are_raised(active: &BattlePokemon) -> bool {
    let stages = active.stages();
    let raised = [
        stages.attack,
        stages.defense,
        stages.speed,
        stages.sp_attack,
        stages.sp_defense,
        stages.accuracy,
        stages.evasion,
    ]
    .into_iter()
    .map(|stage| i32::from(stage.offset()))
    .filter(|offset| *offset > 0)
    .sum::<i32>();
    // `buffedStatsValue > 3` (`:325`). Upstream's unused HP stage stays at
    // default and adds nothing, so it is absent here.
    raised > 3
}

fn find_mon_with_flags(
    dex: &Dex,
    player: &BattlePokemon,
    state: BattlerState,
    party: &TrainerContext,
    wanted: TypeFlags,
    modulo: u16,
    rng: &mut impl BattleRng,
) -> Result<Option<usize>, BattleError> {
    let LastLandedMove::Move(landed) = state.last_landed_move() else {
        return Ok(None);
    };
    if state.last_hitter().is_none() {
        return Ok(None);
    }
    if dex.move_data(landed)?.power == 0 {
        return Ok(None);
    }
    // Singles: the last hitter is the player (`gLastHitBy`).
    if state.last_hitter() != Some(BattlerSide::Player) {
        return Ok(None);
    }
    for (slot, candidate) in party.candidates() {
        if candidate.is_fainted() {
            continue;
        }
        let resisted = type_flags(dex, landed, candidate)?;
        let matches_wanted = (wanted.doesnt_affect && resisted.doesnt_affect)
            || (wanted.not_very && resisted.not_very)
            || (wanted.super_effective && resisted.super_effective);
        if !matches_wanted {
            continue;
        }
        for known in candidate.moves() {
            if !type_flags(dex, known.move_id, player)?.super_effective {
                continue;
            }
            if rng.next_u16().is_multiple_of(modulo) {
                return Ok(Some(slot));
            }
        }
    }
    Ok(None)
}

fn type_flags(
    dex: &Dex,
    move_id: MoveId,
    defender: &BattlePokemon,
) -> Result<TypeFlags, BattleError> {
    // `AI_TypeCalc` (`pokeemerald/src/battle_script_commands.c:1594`-`:1635`).
    if move_id == crate::damage::STRUGGLE {
        return Ok(TypeFlags::default());
    }
    let data = dex.move_data(move_id)?;
    let mut flags = TypeFlags::default();
    let Some(move_type) = data.move_type.battle_type() else {
        apply_wonder_guard(defender.ability(), data.power, &mut flags);
        return Ok(flags);
    };
    if defender.ability() == AbilityId::LEVITATE && move_type == Type::Ground {
        flags.doesnt_affect = true;
    } else {
        let [type1, type2] = defender.types();
        for &(attacker, defending, effectiveness) in TypeChart::rows() {
            if attacker != move_type {
                continue;
            }
            if defending == type1 {
                modulate(&mut flags, effectiveness, data.power);
            }
            if defending == type2 && type1 != type2 {
                modulate(&mut flags, effectiveness, data.power);
            }
        }
    }
    apply_wonder_guard(defender.ability(), data.power, &mut flags);
    Ok(flags)
}

fn apply_wonder_guard(ability: AbilityId, power: u8, flags: &mut TypeFlags) {
    if ability != AbilityId::WONDER_GUARD || power == 0 {
        return;
    }
    let not_cleanly_super = !flags.super_effective || flags.not_very;
    if not_cleanly_super {
        flags.doesnt_affect = true;
    }
}

fn modulate(flags: &mut TypeFlags, effectiveness: Effectiveness, power: u8) {
    // `ModulateDmgByType2` (`battle_script_commands.c:1502`-`:1533`).
    let no_effect = flags.doesnt_affect;
    match effectiveness {
        Effectiveness::NoEffect => {
            flags.doesnt_affect = true;
            flags.not_very = false;
            flags.super_effective = false;
        }
        Effectiveness::NotVeryEffective if power > 0 && !no_effect => {
            if flags.super_effective {
                flags.super_effective = false;
            } else {
                flags.not_very = true;
            }
        }
        Effectiveness::SuperEffective if power > 0 && !no_effect => {
            if flags.not_very {
                flags.not_very = false;
            } else {
                flags.super_effective = true;
            }
        }
        Effectiveness::NotVeryEffective | Effectiveness::SuperEffective | Effectiveness::Normal => {
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{consider_switch, SwitchFacts};
    use crate::battle::battler_state::{
        BattlerSide, BattlerState, LastLandedMove, PerishSongTurns,
    };
    use crate::battle::opponent_ai::EnemyAction;
    use crate::battle::trainer::{trainer_data, TrainerContext};
    use crate::battle::trainer_ai::choose_trainer_action_with_switch;
    use crate::dex::Dex;
    use crate::error::BattleError;
    use crate::pokemon::{BattlePokemon, Ivs, MAX_MON_MOVES};
    use crate::script_rng::SequenceRng;
    use crate::stat_stage::StatStage;
    use crate::status1::Status1;
    use assets::trainers::{AiFlags, TrainerId};
    use assets::{MoveId, SpeciesId};

    const LEVEL: u8 = 20;
    const PERSONALITY: u32 = 0;
    const TREECKO: SpeciesId = SpeciesId(277);
    const MUDKIP: SpeciesId = SpeciesId(283);
    const SHEDINJA: SpeciesId = SpeciesId(303);
    const CHANSEY: SpeciesId = SpeciesId(113);
    const VAPOREON: SpeciesId = SpeciesId(134);
    const FLAREON: SpeciesId = SpeciesId(136);
    const PIDGEY: SpeciesId = SpeciesId(16);
    const VOLTORB: SpeciesId = SpeciesId(100);
    const TACKLE: MoveId = MoveId::TACKLE;
    const EMBER: MoveId = MoveId::EMBER;
    const GROWL: MoveId = MoveId::GROWL;
    const EARTHQUAKE: MoveId = MoveId::EARTHQUAKE;
    const WING_ATTACK: MoveId = MoveId::WING_ATTACK;
    const POUND: MoveId = MoveId::POUND;
    const SEISMIC_TOSS: MoveId = MoveId::SEISMIC_TOSS;
    const SONIC_BOOM: MoveId = MoveId::SONIC_BOOM;
    /// Four simulated-damage draws plus the tie break, with no switch draws.
    const SCORING_DRAWS: usize = MAX_MON_MOVES + 1;

    fn mon(species: SpeciesId, moves: Vec<MoveId>) -> BattlePokemon {
        BattlePokemon::new(
            &Dex::new(),
            species,
            LEVEL,
            Ivs::default(),
            PERSONALITY,
            moves,
        )
        .expect("species and moves are in the dex")
    }

    fn party(bench: Vec<BattlePokemon>) -> TrainerContext {
        let id = TrainerId::MAY_ROUTE_103_MUDKIP;
        let data = trainer_data(id).expect("may exists");
        TrainerContext::new(id, data, bench)
    }

    fn facts<'a>(party: &'a TrainerContext, state: &'a BattlerState) -> SwitchFacts<'a> {
        SwitchFacts {
            party,
            enemy_state: state,
            resolving_move: MoveId::NONE,
        }
    }

    fn probe(
        active: &BattlePokemon,
        player: &BattlePokemon,
        bench: Vec<BattlePokemon>,
        state: BattlerState,
        script: &[u16],
    ) -> (Result<Option<usize>, BattleError>, usize) {
        let held = party(bench);
        let mut rng = SequenceRng::new(script.iter().copied());
        let result = consider_switch(&Dex::new(), active, player, &facts(&held, &state), &mut rng);
        let draws = rng.draws();
        if result.is_ok() {
            rng.assert_exhausted();
        }
        (result, draws)
    }

    #[test]
    fn no_healthy_bench_draws_nothing_even_when_perish_song_ends() {
        let active = mon(TREECKO, vec![TACKLE]);
        let player = mon(MUDKIP, vec![TACKLE]);
        let mut state = BattlerState::default();
        state.set_perish_song(Some(PerishSongTurns(0)));
        let fainted = {
            let mut bench = mon(FLAREON, vec![EMBER]);
            bench.apply_damage(u32::MAX);
            bench
        };
        let (result, draws) = probe(&active, &player, vec![fainted], state, &[]);
        assert_eq!(result, Ok(None));
        assert_eq!(draws, 0);
    }

    #[test]
    fn perish_song_switches_before_any_draw() {
        let active = mon(TREECKO, vec![TACKLE]);
        let player = mon(MUDKIP, vec![TACKLE]);
        let mut state = BattlerState::default();
        state.set_perish_song(Some(PerishSongTurns(0)));
        let (result, draws) = probe(
            &active,
            &player,
            vec![mon(FLAREON, vec![EMBER])],
            state,
            &[],
        );
        assert_eq!(result, Ok(Some(1)));
        assert_eq!(draws, 0);
    }

    #[test]
    fn wonder_guard_rolls_percent_three_and_names_the_original_slot() {
        let active = mon(TREECKO, vec![TACKLE]);
        let player = mon(SHEDINJA, vec![POUND]);
        let bench = vec![mon(FLAREON, vec![EMBER])];
        let state = BattlerState::default();
        let (hit, draws) = probe(&active, &player, bench.clone(), state, &[0]);
        assert_eq!(hit, Ok(Some(1)));
        assert_eq!(draws, 1);
        let (miss, draws) = probe(&active, &player, bench, state, &[2]);
        assert_eq!(miss, Ok(None));
        assert_eq!(draws, 1);
    }

    #[test]
    fn absorber_coin_selects_the_flash_fire_bench() {
        let active = mon(TREECKO, vec![TACKLE]);
        let player = mon(MUDKIP, vec![TACKLE]);
        let mut state = BattlerState::default();
        state.set_last_landed_move(LastLandedMove::Move(EMBER), None);
        let bench = vec![mon(FLAREON, vec![EMBER])];
        let (switched, draws) = probe(&active, &player, bench.clone(), state, &[1]);
        assert_eq!(switched, Ok(Some(1)));
        assert_eq!(draws, 1);
        let (stayed, draws) = probe(&active, &player, bench, state, &[0]);
        assert_eq!(stayed, Ok(None));
        assert_eq!(draws, 1);
    }

    #[test]
    fn absorber_percent_three_and_own_super_effective_percent_ten_share_one_sequence() {
        let active = mon(FLAREON, vec![EMBER]);
        assert_eq!(active.ability(), assets::AbilityId::FLASH_FIRE);
        let player = mon(TREECKO, vec![TACKLE]);
        let mut state = BattlerState::default();
        state.set_last_landed_move(LastLandedMove::Move(EMBER), Some(BattlerSide::Player));
        let bench = vec![mon(VAPOREON, vec![WING_ATTACK])];
        // %3 != 0 aborts the absorber, then %10 != 0 refuses the switch.
        let (blocked, draws) = probe(&active, &player, bench.clone(), state, &[1, 1]);
        assert_eq!(blocked, Ok(None));
        assert_eq!(draws, 2);
        // %3 == 0 continues, %10 == 0 does not refuse, then the resisted
        // candidate's %3 == 0 switches to slot 1.
        let (switched, draws) = probe(&active, &player, bench, state, &[0, 0, 0]);
        assert_eq!(switched, Ok(Some(1)));
        assert_eq!(draws, 3);
    }

    #[test]
    fn natural_cure_coin_switches_and_move_unavailable_is_refused() {
        let mut active = mon(CHANSEY, vec![TACKLE]);
        assert_eq!(active.ability(), assets::AbilityId::NATURAL_CURE);
        active.set_status1(Status1::Asleep { turns: 3 });
        let player = mon(MUDKIP, vec![TACKLE]);
        let bench = vec![mon(TREECKO, vec![TACKLE])];
        let mut missing = BattlerState::default();
        missing.set_last_landed_move(LastLandedMove::None, None);
        let (switched, draws) = probe(&active, &player, bench.clone(), missing, &[1]);
        assert_eq!(switched, Ok(Some(1)));
        assert_eq!(draws, 1);

        let mut undefined = BattlerState::default();
        undefined.set_last_landed_move(LastLandedMove::Unavailable, None);
        let (refused, draws) = probe(&active, &player, bench, undefined, &[0]);
        assert_eq!(refused, Err(BattleError::UndefinedNaturalCureLandedMove));
        assert_eq!(draws, 1);
    }

    #[test]
    fn natural_cure_reads_a_defined_status_move_before_its_second_coin() {
        let mut active = mon(CHANSEY, vec![TACKLE]);
        active.set_status1(Status1::Asleep { turns: 2 });
        let player = mon(MUDKIP, vec![TACKLE]);
        let mut state = BattlerState::default();
        state.set_last_landed_move(LastLandedMove::Move(GROWL), None);
        let (switched, draws) = probe(
            &active,
            &player,
            vec![mon(TREECKO, vec![TACKLE])],
            state,
            &[1],
        );
        assert_eq!(switched, Ok(Some(1)));
        assert_eq!(draws, 1);
    }

    #[test]
    fn raised_stats_draw_nothing_and_block_the_resisted_candidate() {
        let mut active = mon(TREECKO, vec![TACKLE]);
        active.stages_mut().attack = StatStage::new(4).expect("stage 4");
        let player = mon(TREECKO, vec![TACKLE]);
        let mut state = BattlerState::default();
        state.set_last_landed_move(LastLandedMove::Move(EARTHQUAKE), Some(BattlerSide::Player));
        let bench = vec![mon(PIDGEY, vec![WING_ATTACK])];
        let (blocked, draws) = probe(&active, &player, bench.clone(), state, &[]);
        assert_eq!(blocked, Ok(None));
        assert_eq!(draws, 0);

        let plain = mon(TREECKO, vec![TACKLE]);
        let (switched, draws) = probe(&plain, &player, bench, state, &[0]);
        assert_eq!(switched, Ok(Some(1)));
        assert_eq!(draws, 1);
    }

    #[test]
    fn a_switch_returns_before_scoring_and_a_lone_lead_keeps_scoring_draws() {
        let dex = Dex::new();
        let enemy = mon(TREECKO, vec![SEISMIC_TOSS, SONIC_BOOM, POUND]);
        let player = mon(VOLTORB, vec![POUND]);
        let held = party(vec![mon(MUDKIP, vec![TACKLE])]);
        let mut ending = BattlerState::default();
        ending.set_perish_song(Some(PerishSongTurns(0)));
        let mut switch_rng = SequenceRng::new([]);
        let switched = choose_trainer_action_with_switch(
            &dex,
            &enemy,
            &player,
            AiFlags::NONE,
            0,
            Some(facts(&held, &ending)),
            &mut switch_rng,
        );
        assert_eq!(switched, Ok(EnemyAction::Switch(1)));
        assert_eq!(switch_rng.draws(), 0);

        let mut score_rng = SequenceRng::new([0, 1, 2, 3, 0]);
        let scored = choose_trainer_action_with_switch(
            &dex,
            &enemy,
            &player,
            AiFlags::NONE,
            0,
            Some(facts(&held, &BattlerState::default())),
            &mut score_rng,
        );
        assert_eq!(scored, Ok(EnemyAction::Move(0)));
        assert_eq!(score_rng.draws(), SCORING_DRAWS);
        score_rng.assert_exhausted();

        let lone = party(vec![]);
        let mut lone_rng = SequenceRng::new([0, 1, 2, 3, 0]);
        let lone_choice = choose_trainer_action_with_switch(
            &dex,
            &enemy,
            &player,
            AiFlags::NONE,
            0,
            Some(facts(&lone, &ending)),
            &mut lone_rng,
        );
        assert_eq!(lone_choice, Ok(EnemyAction::Move(0)));
        assert_eq!(lone_rng.draws(), SCORING_DRAWS);
    }
}
