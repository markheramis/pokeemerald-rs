//! Scripted-RNG coverage for each voluntary-switch probe: the selected
//! original slot, the draws it consumes, and the fact that a switch returns
//! before move scoring.

use assets::trainers::{AiFlags, TrainerId};
use assets::{MoveId, SpeciesId};

use super::super::super::battler_state::{
    BattlerSide, BattlerState, LastLandedMove, PerishSongTurns,
};
use super::super::super::opponent_ai::EnemyAction;
use super::super::super::trainer::{trainer_data, TrainerContext};
use super::VoluntarySwitch;
use crate::dex::Dex;
use crate::error::BattleError;
use crate::pokemon::{BattlePokemon, Ivs};
use crate::script_rng::SequenceRng;
use crate::stat_stage::StatStage;
use crate::status1::Status1;

const MAY: TrainerId = TrainerId(529);
const SCORING_DRAWS: usize = 5;

fn mon(species: SpeciesId, moves: &[MoveId]) -> BattlePokemon {
    BattlePokemon::new(&Dex::new(), species, 20, Ivs::default(), 0, moves.to_vec())
        .expect("dex-resident")
}

fn roster(bench: Vec<BattlePokemon>) -> TrainerContext {
    TrainerContext::new(MAY, trainer_data(MAY).expect("a real trainer"), bench)
}

fn landed(move_id: MoveId, remember_hitter: bool) -> BattlerState {
    let mut state = BattlerState::default();
    state.record_move_end(
        BattlerSide::Player,
        remember_hitter,
        LastLandedMove::Move(move_id),
    );
    state
}

fn unavailable() -> BattlerState {
    let mut state = BattlerState::default();
    state.record_move_end(BattlerSide::Player, true, LastLandedMove::Unavailable);
    state
}

fn perish(turns: u8) -> BattlerState {
    let mut state = BattlerState::default();
    state.set_perish_song(Some(PerishSongTurns(turns)));
    state
}

fn asleep(mut pokemon: BattlePokemon) -> BattlePokemon {
    pokemon.set_status1(Status1::Asleep { turns: 3 });
    pokemon
}

fn below_half(mut pokemon: BattlePokemon) -> BattlePokemon {
    let max_hp = pokemon.stats().max_hp;
    pokemon.apply_damage(max_hp - max_hp / 2 + 1);
    pokemon
}

fn script(prefix: &[u16]) -> Vec<u16> {
    let mut draws = prefix.to_vec();
    draws.extend(std::iter::repeat_n(0, SCORING_DRAWS));
    draws
}

fn choose(
    enemy: &BattlePokemon,
    player: &BattlePokemon,
    party: Option<&TrainerContext>,
    state: BattlerState,
    flags: AiFlags,
    draws: &[u16],
) -> Result<EnemyAction, BattleError> {
    let mut rng = SequenceRng::new(draws.iter().copied());
    let view = party.map(|party| VoluntarySwitch::new(party, &state));
    let action = super::super::choose_trainer_action_in_battle(
        &Dex::new(),
        enemy,
        player,
        flags,
        0,
        view,
        &mut rng,
    );
    rng.assert_exhausted();
    action
}

fn score_only(
    enemy: &BattlePokemon,
    player: &BattlePokemon,
    party: Option<&TrainerContext>,
    state: BattlerState,
) -> EnemyAction {
    choose(enemy, player, party, state, AiFlags::NONE, &script(&[])).expect("scoring draws only")
}

#[test]
fn no_healthy_bench_draws_only_for_scoring_even_on_perish_song() {
    let enemy = mon(SpeciesId::TREECKO, &[MoveId::TACKLE]);
    let player = mon(SpeciesId::MEOWTH, &[MoveId::TACKLE]);
    let state = perish(0);
    let lone = score_only(&enemy, &player, None, state);
    let empty = roster(vec![]);
    let fainted_member = {
        let mut member = mon(SpeciesId::TORCHIC, &[MoveId::TACKLE]);
        member.apply_damage(member.stats().max_hp);
        member
    };
    let fainted = roster(vec![fainted_member]);
    assert_eq!(lone, EnemyAction::Move(0));
    assert_eq!(score_only(&enemy, &player, Some(&empty), state), lone);
    assert_eq!(score_only(&enemy, &player, Some(&fainted), state), lone);
}

#[test]
fn perish_song_ending_switches_before_any_scoring_draw() {
    let enemy = mon(SpeciesId::TREECKO, &[MoveId::SING]);
    let player = mon(SpeciesId::CHANSEY, &[MoveId::TACKLE]);
    let party = roster(vec![mon(SpeciesId::TORCHIC, &[MoveId::TACKLE])]);
    let action = choose(
        &enemy,
        &player,
        Some(&party),
        perish(0),
        AiFlags::CHECK_BAD_MOVE,
        &[],
    )
    .expect("a perish switch draws nothing");
    assert_eq!(action, EnemyAction::Switch(1));
}

#[test]
fn perish_song_with_turns_remaining_does_not_draw() {
    let enemy = mon(SpeciesId::TREECKO, &[MoveId::TACKLE]);
    let player = mon(SpeciesId::MEOWTH, &[MoveId::TACKLE]);
    let party = roster(vec![mon(SpeciesId::TORCHIC, &[MoveId::TACKLE])]);
    let still_counting = score_only(&enemy, &player, Some(&party), perish(3));
    let absent = score_only(&enemy, &player, Some(&party), BattlerState::default());
    assert_eq!(still_counting, EnemyAction::Move(0));
    assert_eq!(absent, still_counting);
}

#[test]
fn wonder_guard_takes_the_first_accepted_super_effective_bench_move() {
    let enemy = mon(SpeciesId::TREECKO, &[MoveId::TACKLE]);
    let player = mon(SpeciesId::SHEDINJA, &[MoveId::TACKLE]);
    let party = roster(vec![mon(
        SpeciesId::FLAREON,
        &[MoveId::EMBER, MoveId::FLAMETHROWER],
    )]);
    let first = choose(
        &enemy,
        &player,
        Some(&party),
        BattlerState::default(),
        AiFlags::NONE,
        &[0],
    )
    .expect("one acceptance draw");
    let second = choose(
        &enemy,
        &player,
        Some(&party),
        BattlerState::default(),
        AiFlags::NONE,
        &[2, 0],
    )
    .expect("a rejected roll then an acceptance");
    assert_eq!(first, EnemyAction::Switch(1));
    assert_eq!(second, EnemyAction::Switch(1));
}

#[test]
fn wonder_guard_stays_when_the_only_super_effective_roll_fails() {
    let enemy = mon(SpeciesId::TREECKO, &[MoveId::TACKLE]);
    let player = mon(SpeciesId::SHEDINJA, &[MoveId::TACKLE]);
    let party = roster(vec![mon(SpeciesId::FLAREON, &[MoveId::EMBER])]);
    let action = choose(
        &enemy,
        &player,
        Some(&party),
        BattlerState::default(),
        AiFlags::NONE,
        &script(&[2]),
    )
    .expect("a failed roll then scoring");
    assert_eq!(action, EnemyAction::Move(0));
}

#[test]
fn wonder_guard_skips_a_fainted_bench_member() {
    let enemy = mon(SpeciesId::TREECKO, &[MoveId::TACKLE]);
    let player = mon(SpeciesId::SHEDINJA, &[MoveId::TACKLE]);
    let mut fainted = mon(SpeciesId::FLAREON, &[MoveId::EMBER]);
    fainted.apply_damage(fainted.stats().max_hp);
    let party = roster(vec![fainted, mon(SpeciesId::FLAREON, &[MoveId::EMBER])]);
    let action = choose(
        &enemy,
        &player,
        Some(&party),
        BattlerState::default(),
        AiFlags::NONE,
        &[0],
    )
    .expect("only the living bench move draws");
    assert_eq!(action, EnemyAction::Switch(2));
}

#[test]
fn wonder_guard_does_not_scan_the_bench_when_the_active_move_is_super_effective() {
    let enemy = mon(SpeciesId::FLAREON, &[MoveId::EMBER]);
    let player = mon(SpeciesId::SHEDINJA, &[MoveId::TACKLE]);
    let party = roster(vec![mon(SpeciesId::TREECKO, &[MoveId::EMBER])]);
    let action = choose(
        &enemy,
        &player,
        Some(&party),
        BattlerState::default(),
        AiFlags::NONE,
        &script(&[0, 1]),
    )
    .expect("absorber and own-super draws, then scoring");
    assert_eq!(action, EnemyAction::Move(0));
}

#[test]
fn absorber_switches_on_an_odd_roll_for_each_absorbing_type() {
    let cases = [
        (MoveId::EMBER, SpeciesId::FLAREON),
        (MoveId::WATER_GUN, SpeciesId::VAPOREON),
        (MoveId::THUNDER_SHOCK, SpeciesId::JOLTEON),
    ];
    for (last_move, species) in cases {
        let enemy = mon(SpeciesId::TREECKO, &[MoveId::TACKLE]);
        let player = mon(SpeciesId::MEOWTH, &[MoveId::TACKLE]);
        let party = roster(vec![mon(species, &[MoveId::TACKLE])]);
        let action = choose(
            &enemy,
            &player,
            Some(&party),
            landed(last_move, false),
            AiFlags::NONE,
            &[1],
        )
        .expect("one ability coin");
        assert_eq!(action, EnemyAction::Switch(1));
    }
}

#[test]
fn absorber_even_roll_falls_through_to_scoring() {
    let enemy = mon(SpeciesId::TREECKO, &[MoveId::TACKLE]);
    let player = mon(SpeciesId::MEOWTH, &[MoveId::TACKLE]);
    let party = roster(vec![mon(SpeciesId::JOLTEON, &[MoveId::TACKLE])]);
    let action = choose(
        &enemy,
        &player,
        Some(&party),
        landed(MoveId::THUNDER_SHOCK, false),
        AiFlags::NONE,
        &script(&[0]),
    )
    .expect("the even coin then scoring");
    assert_eq!(action, EnemyAction::Move(0));
}

#[test]
fn absorber_draws_nothing_when_the_active_mon_already_has_the_ability() {
    let enemy = mon(SpeciesId::JOLTEON, &[MoveId::TACKLE]);
    let player = mon(SpeciesId::MEOWTH, &[MoveId::TACKLE]);
    let party = roster(vec![mon(SpeciesId::JOLTEON, &[MoveId::TACKLE])]);
    let action = score_only(
        &enemy,
        &player,
        Some(&party),
        landed(MoveId::THUNDER_SHOCK, false),
    );
    assert_eq!(action, EnemyAction::Move(0));
}

#[test]
fn own_super_effective_move_keeps_the_active_mon_before_a_bench_probe() {
    let enemy = mon(SpeciesId::FLAREON, &[MoveId::EMBER]);
    let player = mon(SpeciesId::TREECKO, &[MoveId::TACKLE]);
    let party = roster(vec![mon(SpeciesId::GASTLY, &[MoveId::EMBER])]);
    let action = choose(
        &enemy,
        &player,
        Some(&party),
        landed(MoveId::TACKLE, true),
        AiFlags::NONE,
        &script(&[0, 1]),
    )
    .expect("stay, then score");
    assert_eq!(action, EnemyAction::Move(0));
}

#[test]
fn raised_stats_block_a_bench_switch_without_drawing() {
    let mut enemy = mon(SpeciesId::TREECKO, &[MoveId::TACKLE]);
    enemy.stages_mut().attack = StatStage::new(4).expect("stage");
    let player = mon(SpeciesId::MEOWTH, &[MoveId::TACKLE]);
    let party = roster(vec![mon(SpeciesId::GASTLY, &[MoveId::KARATE_CHOP])]);
    let state = landed(MoveId::TACKLE, true);
    let blocked = score_only(&enemy, &player, Some(&party), state);
    assert_eq!(blocked, EnemyAction::Move(0));

    enemy.stages_mut().attack = StatStage::new(3).expect("stage");
    let allowed = choose(&enemy, &player, Some(&party), state, AiFlags::NONE, &[0])
        .expect("the immunity roll");
    assert_eq!(allowed, EnemyAction::Switch(1));
}

#[test]
fn resisted_bench_member_switches_only_on_a_zero_residue() {
    let enemy = mon(SpeciesId::MEOWTH, &[MoveId::TACKLE]);
    let player = mon(SpeciesId::TREECKO, &[MoveId::TACKLE]);
    let party = roster(vec![mon(SpeciesId::TREECKO, &[MoveId::EMBER])]);
    let state = landed(MoveId::THUNDER_SHOCK, true);
    let switched = choose(&enemy, &player, Some(&party), state, AiFlags::NONE, &[0])
        .expect("one resisted roll");
    let stayed = choose(
        &enemy,
        &player,
        Some(&party),
        state,
        AiFlags::NONE,
        &script(&[1]),
    )
    .expect("a failed resisted roll then scoring");
    assert_eq!(switched, EnemyAction::Switch(1));
    assert_eq!(stayed, EnemyAction::Move(0));
}

#[test]
fn levitate_makes_a_ground_move_count_as_immunity() {
    let enemy = mon(SpeciesId::TREECKO, &[MoveId::TACKLE]);
    let player = mon(SpeciesId::MEOWTH, &[MoveId::TACKLE]);
    let party = roster(vec![mon(SpeciesId::GASTLY, &[MoveId::KARATE_CHOP])]);
    let action = choose(
        &enemy,
        &player,
        Some(&party),
        landed(MoveId::EARTHQUAKE, true),
        AiFlags::NONE,
        &[0],
    )
    .expect("one immunity roll");
    assert_eq!(action, EnemyAction::Switch(1));
}

#[test]
fn natural_cure_refuses_move_unavailable_after_an_even_coin() {
    let enemy = asleep(mon(SpeciesId::CHANSEY, &[MoveId::TACKLE]));
    let player = mon(SpeciesId::MEOWTH, &[MoveId::TACKLE]);
    let party = roster(vec![mon(SpeciesId::TORCHIC, &[MoveId::TACKLE])]);
    let error = choose(
        &enemy,
        &player,
        Some(&party),
        unavailable(),
        AiFlags::NONE,
        &[0],
    )
    .expect_err("the undefined read is refused");
    assert_eq!(error, BattleError::UndefinedNaturalCureMove);
}

#[test]
fn natural_cure_odd_coin_switches_without_reading_move_unavailable() {
    let enemy = asleep(mon(SpeciesId::CHANSEY, &[MoveId::TACKLE]));
    let player = mon(SpeciesId::MEOWTH, &[MoveId::TACKLE]);
    let party = roster(vec![mon(SpeciesId::TORCHIC, &[MoveId::TACKLE])]);
    let action = choose(
        &enemy,
        &player,
        Some(&party),
        unavailable(),
        AiFlags::NONE,
        &[1],
    )
    .expect("one coin");
    assert_eq!(action, EnemyAction::Switch(1));
}

#[test]
fn natural_cure_below_half_hp_never_reaches_the_unavailable_read() {
    let enemy = asleep(below_half(mon(SpeciesId::CHANSEY, &[MoveId::TACKLE])));
    assert!(enemy.current_hp() < enemy.stats().max_hp / 2);
    let player = mon(SpeciesId::MEOWTH, &[MoveId::TACKLE]);
    let party = roster(vec![mon(SpeciesId::TORCHIC, &[MoveId::TACKLE])]);
    let action = score_only(&enemy, &player, Some(&party), unavailable());
    assert_eq!(action, EnemyAction::Move(0));
}

#[test]
fn natural_cure_requires_sleep_and_the_ability() {
    let player = mon(SpeciesId::MEOWTH, &[MoveId::TACKLE]);
    let party = roster(vec![mon(SpeciesId::TORCHIC, &[MoveId::TACKLE])]);
    let awake = score_only(
        &mon(SpeciesId::CHANSEY, &[MoveId::TACKLE]),
        &player,
        Some(&party),
        unavailable(),
    );
    let serene = asleep(mon(SpeciesId::CHANSEY, &[MoveId::TACKLE]).with_ability_slot(1));
    let other_ability = score_only(&serene, &player, Some(&party), unavailable());
    assert_eq!(awake, EnemyAction::Move(0));
    assert_eq!(other_ability, EnemyAction::Move(0));
}

#[test]
fn natural_cure_switches_on_a_powerless_move_coin() {
    let enemy = asleep(mon(SpeciesId::CHANSEY, &[MoveId::GROWL]));
    let player = mon(SpeciesId::MEOWTH, &[MoveId::TACKLE]);
    let party = roster(vec![mon(SpeciesId::TORCHIC, &[MoveId::TACKLE])]);
    let action = choose(
        &enemy,
        &player,
        Some(&party),
        landed(MoveId::GROWL, false),
        AiFlags::NONE,
        &[1],
    )
    .expect("one power coin");
    assert_eq!(action, EnemyAction::Switch(1));
}

#[test]
fn natural_cure_none_falls_through_two_even_coins_then_takes_the_last() {
    let enemy = asleep(mon(SpeciesId::CHANSEY, &[MoveId::TACKLE]));
    let player = mon(SpeciesId::MEOWTH, &[MoveId::TACKLE]);
    let party = roster(vec![mon(SpeciesId::TORCHIC, &[MoveId::TACKLE])]);
    let action = choose(
        &enemy,
        &player,
        Some(&party),
        BattlerState::default(),
        AiFlags::NONE,
        &[0, 0, 1],
    )
    .expect("three coins");
    assert_eq!(action, EnemyAction::Switch(1));
}

#[test]
fn natural_cure_finds_an_immune_bench_member_and_still_draws_for_modulus_one() {
    let enemy = asleep(mon(SpeciesId::CHANSEY, &[MoveId::TACKLE]));
    let player = mon(SpeciesId::MEOWTH, &[MoveId::TACKLE]);
    let party = roster(vec![
        mon(SpeciesId::TORCHIC, &[MoveId::GROWL]),
        mon(SpeciesId::GASTLY, &[MoveId::KARATE_CHOP]),
    ]);
    let action = choose(
        &enemy,
        &player,
        Some(&party),
        landed(MoveId::TACKLE, true),
        AiFlags::NONE,
        &[9],
    )
    .expect("the guaranteed modulus still draws");
    assert_eq!(action, EnemyAction::Switch(2));
}
