//! Wild-opponent move and action selection.

use assets::MoveId;

use crate::damage::BattleRng;
use crate::pokemon::{BattlePokemon, MAX_MON_MOVES, MOVE_NONE};

const PERCENT_SCALE: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnemyAction {
    Move(usize),
    Struggle,
    Flee,
    /// Switch to the healthy non-active trainer party member in this
    /// original slot. `ShouldSwitch` selects it before move scoring.
    Switch(usize),
}

/// Whether the slot holds a real move, regardless of remaining PP.
pub(crate) fn selectable_slot(slot_move: Option<MoveId>) -> bool {
    slot_move.is_some_and(|move_id| move_id != MOVE_NONE)
}

/// Whether every known move slot is out of PP: `AreAllMovesUnusable`'s
/// `MOVE_LIMITATION_PP`/`MOVE_LIMITATION_ZEROMOVE` case
/// (`pokeemerald/src/battle_util.c:1098`, `:1101`), shared by the player, the
/// wild AI, and the trainer AI's own forced-Struggle diversion.
pub(crate) fn all_known_moves_are_spent(pokemon: &BattlePokemon) -> bool {
    pokemon.moves().iter().all(|slot| slot.pp == 0)
}

fn draw_move_slot(rng: &mut impl BattleRng) -> usize {
    usize::from(rng.next_u16()) % MAX_MON_MOVES
}

/// Returns `None` when every known move is spent and Struggle is forced.
pub(crate) fn choose_enemy_move(enemy: &BattlePokemon, rng: &mut impl BattleRng) -> Option<usize> {
    if all_known_moves_are_spent(enemy) {
        return None;
    }

    loop {
        let candidate_slot = draw_move_slot(rng);
        if selectable_slot(enemy.move_at(candidate_slot)) {
            return Some(candidate_slot);
        }
    }
}

pub(crate) const FIRST_BATTLE_FLEE_HP_PERCENT: u32 = 20;

fn current_hp_percent(pokemon: &BattlePokemon) -> u32 {
    PERCENT_SCALE * pokemon.current_hp() / pokemon.stats().max_hp
}

fn move_slots_with_pp(pokemon: &BattlePokemon) -> Vec<usize> {
    pokemon
        .moves()
        .iter()
        .enumerate()
        .filter_map(|(slot, move_slot)| (move_slot.pp > 0).then_some(slot))
        .collect()
}

/// Consumes Emerald's unused first-battle `simulatedRNG` fill before the flee
/// check (`src/battle_ai_script_commands.c:312`-`:341`).
fn consume_first_battle_setup_draws(rng: &mut impl BattleRng) {
    for _ in 0..MAX_MON_MOVES {
        let _ = rng.next_u16();
    }
}

/// Chooses the wild opponent's action on the one AI script a first battle ever
/// runs (`AI_SCRIPT_FIRST_BATTLE`): Struggle without a PP-positive move, Flee
/// when the player's HP percent is at or below [`FIRST_BATTLE_FLEE_HP_PERCENT`]
/// after the unused setup draws, else a uniform draw over PP-positive slots.
pub(crate) fn choose_enemy_action_first_battle(
    enemy: &BattlePokemon,
    player: &BattlePokemon,
    rng: &mut impl BattleRng,
) -> EnemyAction {
    let usable_move_slots = move_slots_with_pp(enemy);
    if usable_move_slots.is_empty() {
        return EnemyAction::Struggle;
    }

    consume_first_battle_setup_draws(rng);
    if current_hp_percent(player) <= FIRST_BATTLE_FLEE_HP_PERCENT {
        return EnemyAction::Flee;
    }

    let selected_slot = usize::from(rng.next_u16()) % usable_move_slots.len();
    EnemyAction::Move(usable_move_slots[selected_slot])
}

#[cfg(test)]
mod tests {
    use super::{
        choose_enemy_action_first_battle, choose_enemy_move, current_hp_percent, selectable_slot,
        EnemyAction, FIRST_BATTLE_FLEE_HP_PERCENT, PERCENT_SCALE,
    };
    use crate::pokemon::{BattlePokemon, Ivs, MAX_MON_MOVES, MOVE_NONE};
    use crate::script_rng::SequenceRng;
    use assets::{MoveId, SpeciesId};

    const BULBASAUR: SpeciesId = SpeciesId(1);
    const ZIGZAGOON: SpeciesId = SpeciesId(288);
    const TACKLE: MoveId = MoveId::TACKLE;
    const GROWL: MoveId = MoveId::GROWL;
    const MAX_IVS: Ivs = Ivs {
        hp: 31,
        attack: 31,
        defense: 31,
        speed: 31,
        sp_attack: 31,
        sp_defense: 31,
    };

    fn mon(species: SpeciesId, level: u8, moves: Vec<MoveId>) -> BattlePokemon {
        BattlePokemon::new(&crate::dex::Dex::new(), species, level, MAX_IVS, 0, moves).unwrap()
    }

    fn spend_move(pokemon: &mut BattlePokemon, slot: usize) {
        while pokemon.moves()[slot].pp > 0 {
            pokemon.deduct_pp(slot).unwrap();
        }
    }

    #[test]
    fn selectable_slots_hold_real_moves() {
        assert!(selectable_slot(Some(TACKLE)));
        assert!(!selectable_slot(Some(MOVE_NONE)));
        assert!(!selectable_slot(None));
    }

    #[test]
    fn ordinary_choice_returns_none_without_drawing_when_every_move_is_spent() {
        let mut enemy = mon(ZIGZAGOON, 2, vec![TACKLE]);
        spend_move(&mut enemy, 0);
        let mut rng = SequenceRng::new([]);

        assert_eq!(choose_enemy_move(&enemy, &mut rng), None);
        assert_eq!(rng.draws(), 0);
    }

    #[test]
    fn first_battle_choice_forces_struggle_without_drawing_when_every_move_is_spent() {
        let mut enemy = mon(ZIGZAGOON, 2, vec![TACKLE, GROWL]);
        spend_move(&mut enemy, 0);
        spend_move(&mut enemy, 1);
        let player = mon(BULBASAUR, 5, vec![TACKLE]);
        let mut rng = SequenceRng::new([]);

        assert_eq!(
            choose_enemy_action_first_battle(&enemy, &player, &mut rng),
            EnemyAction::Struggle
        );
        assert_eq!(rng.draws(), 0);
    }

    #[test]
    fn first_battle_choice_flees_at_or_below_twenty_percent_after_setup_draws() {
        let enemy = mon(ZIGZAGOON, 50, vec![TACKLE, GROWL]);
        let mut player = mon(BULBASAUR, 50, vec![TACKLE]);
        let max_hp = player.stats().max_hp;
        player.apply_damage(max_hp - max_hp * FIRST_BATTLE_FLEE_HP_PERCENT / PERCENT_SCALE);
        assert!(current_hp_percent(&player) <= FIRST_BATTLE_FLEE_HP_PERCENT);
        let mut rng = SequenceRng::new([0; MAX_MON_MOVES]);

        assert_eq!(
            choose_enemy_action_first_battle(&enemy, &player, &mut rng),
            EnemyAction::Flee
        );
        assert_eq!(rng.draws(), MAX_MON_MOVES);
    }

    #[test]
    fn first_battle_choice_never_selects_a_spent_slot() {
        let mut enemy = mon(ZIGZAGOON, 50, vec![TACKLE, GROWL]);
        spend_move(&mut enemy, 0);
        let player = mon(BULBASAUR, 50, vec![TACKLE]);
        let mut rng = SequenceRng::new([0; MAX_MON_MOVES + 1]);

        assert_eq!(
            choose_enemy_action_first_battle(&enemy, &player, &mut rng),
            EnemyAction::Move(1)
        );
        assert_eq!(rng.draws(), MAX_MON_MOVES + 1);
    }
}
