//! Single-battle turn resolution.
//!
//! [`Battle`] supports ordinary wild encounters, the scripted first battle,
//! and trainer parties. An accepted turn chooses the opponent's action before
//! resolving a run or move order, skips queued actions once either battler
//! faints, settles the knockouts that leaves, and then applies end-of-turn
//! residuals in an end-turn order rebuilt from current Speed.
//!
//! Construction consumes a turn-number draw and a conditional Speed-tie draw.
//! Each accepted turn consumes another turn-number draw before opponent action
//! selection. These positions preserve the shared upstream RNG sequence
//! (`src/battle_main.c:3140`, `:3852`-`:3861`, `:3923`, `:4013`).

use assets::trainers::TrainerId;
use assets::{AbilityId, MoveId, MoveTarget};

use crate::damage::{BattleRng, STRUGGLE};
use crate::dex::Dex;
use crate::error::BattleError;
use crate::escape::{ensure_admissible, try_run_from_battle};
use crate::exp::{trainer_faint_exp, wild_faint_exp};
use crate::pokemon::{BattlePokemon, MoveLearnDecision, PendingMoveLearn, MAX_LEVEL, MOVE_NONE};
use crate::secondary;
use crate::stat_change;
use crate::status1::{
    draws_full_paralysis, draws_shed_skin_cure, poison_residual_damage, tick_sleep, SleepTick,
    Status1,
};
use crate::turn_order::{resolve_order, Order};
use crate::volatile::draws_confusion_self_hit;

pub(crate) mod admission;
pub mod battler_state;
mod events;
mod execute;
pub(crate) mod held_item;
pub(crate) mod opponent_ai;
#[cfg(test)]
mod switch_tests;
pub mod trainer;
pub(crate) mod trainer_ai;

pub use battler_state::{BattlerSide, BattlerState, LastLandedMove, PerishSongTurns};
pub use events::{BattleEvent, TurnError};

use opponent_ai::{
    all_known_moves_are_spent, choose_enemy_action_first_battle, choose_enemy_move,
    selectable_slot, EnemyAction,
};
use trainer::TrainerContext;
use trainer_ai::{choose_trainer_action_in_battle, VoluntarySwitch};

const NO_MOVE_PRIORITY: i8 = 0;

/// A player's selected turn action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayerAction {
    /// Use the move at this index in the active Pokémon's moveset.
    UseMove(usize),
    /// Attempt to run away.
    Run,
}

/// The result of a finished battle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BattleOutcome {
    /// The opposing side has no usable Pokémon.
    PlayerWon,
    /// The player's party has no usable Pokémon: the active member and every
    /// reserve have fainted (`Cmd_checkteamslost`,
    /// `src/battle_script_commands.c:3534`-`:3564`).
    PlayerLost,
    /// The player successfully ran away.
    PlayerRan,
    /// The opponent fled from the scripted first battle.
    WildFled,
}

/// Validates that one complete move-effect pipeline can execute a move.
pub(crate) fn ensure_executable(dex: &Dex, move_id: MoveId) -> Result<(), BattleError> {
    execute::MovePipeline::for_move(dex, move_id).map(|_| ())
}

/// An owned single battle driven one turn at a time.
#[derive(Debug, Clone)]
pub struct Battle {
    dex: Dex,
    player: BattlePokemon,
    /// The player's party members other than [`Battle::player`], always in
    /// the order the owning flow passed them.
    /// [`Battle::send_out_next_player_reserve`] returns a fainted active
    /// member to its own position here rather than swapping it into the
    /// sent-out reserve's, so the party order never changes across
    /// replacements, as upstream's `gPlayerParty` records keep their slots
    /// while `gBattlerPartyIndexes` names the active one.
    player_reserves: Vec<BattlePokemon>,
    /// The position of [`Battle::player`] in the party as passed to
    /// [`Battle::new_with_player_reserves`] or
    /// [`Battle::new_trainer_with_player_reserves`]: `0` until a replacement,
    /// and never beyond `player_reserves.len()`. The player-side analogue of
    /// upstream's `gBattlerPartyIndexes` entry for the player's battler.
    player_slot: usize,
    enemy: BattlePokemon,
    run_attempts: u8,
    random_turn_number: u16,
    outcome: Option<BattleOutcome>,
    kind: BattleKind,
    turn_counter: u8,
    turn_has_started: bool,
    pending_residual_selection: Option<ChosenMoves>,
    /// Each battler's last actually chosen move slot, `[player, enemy]`:
    /// upstream's `chosenMovePositions`: zero-initialized and never assigned
    /// for a forced Struggle (`pokeemerald/src/battle_main.c:4183`-`:4189`).
    retained_slots: [usize; 2],
    /// `gCurrentMove` (`pokeemerald/include/battle.h`): the most recently
    /// attempted move, `MOVE_NONE` before any has been. A forced trainer
    /// replacement's most-damage fallback reads this stale value as its
    /// base-damage move, matching upstream
    /// (`pokeemerald/src/battle_ai_switch_items.c:772`-`:779`).
    last_move_used: MoveId,
    /// Move history and Perish Song state per battler, `[player, enemy]`,
    /// reset whenever a battler leaves or enters the field.
    battler_states: [BattlerState; 2],
}

#[derive(Debug, Clone)]
enum BattleKind {
    Wild,
    FirstBattle,
    Trainer(TrainerContext),
}

/// What one battler selected this turn, as `GetWhoStrikesFirst(.., FALSE)`
/// still sees it when the end-turn pass rebuilds its order: the chosen move
/// position, read from whichever battler holds it at that point, so a faint
/// replacement or a spent slot changes the priority
/// (`pokeemerald/src/battle_util.c:1199`-`:1210`,
/// `pokeemerald/src/battle_main.c:4690`-`:4748`).
#[derive(Debug, Clone, Copy)]
enum Selection {
    /// A run or flee: no move is selected.
    NoMove,
    /// The move slot chosen at selection.
    Slot(usize),
    /// A forced Struggle: the end-turn comparison reads the battler's
    /// retained slot (`pokeemerald/src/battle_util.c:100`-`:104`).
    Struggle,
}

/// Both battlers' selections. Only the selection is retained from the action
/// phase, never which battler won it.
#[derive(Debug, Clone, Copy)]
struct ChosenMoves {
    player: Selection,
    enemy: Selection,
}

/// A resolved priority pair for one comparison.
#[derive(Debug, Clone, Copy)]
struct ResidualPriorities {
    player: i8,
    enemy: i8,
}

#[derive(Debug, Clone, Copy)]
enum ValidatedPlayerAction {
    /// `slot` is `None` for a forced Struggle: every known move is spent, so
    /// upstream substitutes `MOVE_STRUGGLE` at selection with no PP slot to
    /// deduct from (`AreAllMovesUnusable`, `pokeemerald/src/battle_util.c:1125`-
    /// `:1139`, `:100`-`:104`).
    UseMove {
        slot: Option<usize>,
        move_id: MoveId,
    },
    Run,
}

fn initialize_turn_rng_state(
    player: &BattlePokemon,
    enemy: &BattlePokemon,
    rng: &mut impl BattleRng,
) -> u16 {
    let random_turn_number = rng.next_u16();
    // Upstream seeds a MOVE_NONE order here. A Speed tie consumes RNG even
    // though the order is discarded (`src/battle_main.c:3852`-`:3861`).
    let _initial_order = resolve_order(
        NO_MOVE_PRIORITY,
        NO_MOVE_PRIORITY,
        player.speed_for_turn_order(),
        enemy.speed_for_turn_order(),
        rng,
    );
    random_turn_number
}

/// Re-reads Deoxys's Speed Forme stats from the EVs the player's side carries
/// in. [`BattlePokemon::gain_evs`] leaves the earning battle's stats alone, as
/// `MonGainEVs` writes only the party copy (`pokemon.c:5988`); upstream's
/// battle-start copy then reads them back through `GetMonData` and
/// `GetDeoxysStat` (`battle_controller_player.c:1641`-`:1645`).
fn refresh_carried_speed_forme_stats(
    player: &mut BattlePokemon,
    player_reserves: &mut [BattlePokemon],
) {
    for mon in std::iter::once(player).chain(player_reserves.iter_mut()) {
        mon.apply_speed_forme_evs();
    }
}

impl Battle {
    /// Starts an ordinary wild battle or the scripted first battle.
    ///
    /// The enemy's full moveset is validated before RNG is consumed because
    /// its action selection can choose any slot. Player moves are validated
    /// when selected. The scripted first battle suppresses critical hits,
    /// refuses Run from Pokémon without Run Away, and uses its dedicated
    /// opponent AI.
    ///
    /// A depleted enemy slot needs only real move data, not an executable
    /// effect: `Battle::act` fails it as [`BattleEvent::FailedNoPp`] before
    /// running one, matching `Cmd_attackcanceler`'s no-PP jump
    /// (`src/battle_script_commands.c:934`-`:939`). Soundproof's own block
    /// runs earlier still (`:932`-`:933`), but needs only the already-checked
    /// move data, not an executable effect, so it cannot make this unsafe.
    /// Struggle is that jump's one exemption (`:934`) and so the only move a
    /// depleted slot still executes; it has an executable pipeline of its own,
    /// so the relaxation still reaches `Battle::execute_move` with nothing
    /// unresolvable.
    ///
    /// # Errors
    ///
    /// Returns [`BattleError::FaintedBattler`] for a fainted participant or
    /// the first move or ability validation error from the enemy's moveset.
    /// Errors leave the RNG untouched.
    pub fn new(
        dex: Dex,
        player: BattlePokemon,
        enemy: BattlePokemon,
        first_battle: bool,
        rng: &mut impl BattleRng,
    ) -> Result<Self, BattleError> {
        Self::new_with_player_reserves(dex, player, Vec::new(), enemy, first_battle, rng)
    }

    /// Starts an ordinary wild battle or the scripted first battle with a
    /// player party larger than the active member.
    ///
    /// `player_reserves` is consumed in order under a headless party-order
    /// policy: an active faint sends out the first entry that is not itself
    /// fainted, with no player choice and no party-screen UI
    /// (`Cmd_checkteamslost`, `src/battle_script_commands.c:3534`-`:3564`;
    /// `BattleScript_HandleFaintedMon`'s send-out branch,
    /// `data/battle_scripts_1.s:2830`-`:2896`). [`Battle::new`] is this
    /// constructor with an empty reserve list. A reserve's own moves are
    /// validated as the player selects them, exactly like the active
    /// member's; the enemy's moveset is validated against every non-fainted
    /// reserve up front too, since any of them may face it as a defender
    /// with no further checkpoint before that turn. A fainted reserve is
    /// admitted without that check: `Battle::send_out_next_player_reserve`
    /// never selects it, so it can never become the enemy's defender.
    ///
    /// # Errors
    ///
    /// Returns [`BattleError::FaintedBattler`] for a fainted active
    /// participant, or the first move or ability validation error from the
    /// enemy's moveset against the active member or any non-fainted reserve.
    /// A fainted reserve is admitted: it models a player party that already
    /// lost a member before the battle. A non-fainted participant that is
    /// Paralysed with Limber, Poisoned with Immunity, asleep with Insomnia or
    /// Vital Spirit, confused with Own Tempo, or holds Truant, Intimidate,
    /// Speed Boost, Color Change, Trace, Effect Spore, Flame Body, Cute Charm,
    /// Drizzle, Drought or Sand Stream fails with
    /// [`BattleError::UnportedAbilityInteraction`].
    /// Errors leave the RNG untouched.
    pub fn new_with_player_reserves(
        dex: Dex,
        mut player: BattlePokemon,
        mut player_reserves: Vec<BattlePokemon>,
        enemy: BattlePokemon,
        first_battle: bool,
        rng: &mut impl BattleRng,
    ) -> Result<Self, BattleError> {
        refresh_carried_speed_forme_stats(&mut player, &mut player_reserves);
        if player.is_fainted() {
            return Err(BattleError::FaintedBattler(true));
        }
        if enemy.is_fainted() {
            return Err(BattleError::FaintedBattler(false));
        }
        for (index, slot) in enemy.moves().iter().enumerate() {
            // A depleted slot must still be real move data -- not the
            // empty-slot placeholder or an id outside the dex -- but its
            // effect need not be executable; see the no-PP note above.
            if slot.move_id == MOVE_NONE {
                return Err(BattleError::PlaceholderMove(index));
            }
            dex.move_data(slot.move_id)?;
            // Zero-PP moves stop before applying effects, Struggle
            // excepted (`src/battle_script_commands.c:934`-`:939`). A
            // non-fainted reserve becomes this same enemy's defender the
            // moment it is sent out, with no further validation at that
            // point, so every non-fainted reserve is checked here too. A
            // fainted reserve is skipped: `send_out_next_player_reserve`
            // never selects it.
            if slot.pp > 0 {
                ensure_executable(&dex, slot.move_id)?;
                for defender in std::iter::once(&player).chain(
                    player_reserves
                        .iter()
                        .filter(|reserve| !reserve.is_fainted()),
                ) {
                    secondary::ensure_admissible(&dex, slot.move_id, &enemy, defender)?;
                }
            }
        }
        admission::ensure_participants_admissible(
            [&player, &enemy].into_iter().chain(
                player_reserves
                    .iter()
                    .filter(|reserve| !reserve.is_fainted()),
            ),
        )?;
        held_item::ensure_participants_hold_playable_items(
            [&player, &enemy].into_iter().chain(
                player_reserves
                    .iter()
                    .filter(|reserve| !reserve.is_fainted()),
            ),
        )?;
        let random_turn_number = initialize_turn_rng_state(&player, &enemy, rng);
        Ok(Self {
            dex,
            player,
            player_reserves,
            player_slot: 0,
            enemy,
            run_attempts: 0,
            random_turn_number,
            outcome: None,
            kind: if first_battle {
                BattleKind::FirstBattle
            } else {
                BattleKind::Wild
            },
            turn_counter: 0,
            turn_has_started: false,
            pending_residual_selection: None,
            retained_slots: [0; 2],
            last_move_used: MOVE_NONE,
            battler_states: [BattlerState::default(); 2],
        })
    }

    /// Starts a trainer battle with `party[0]` active and the rest on the
    /// bench. Equivalent to [`Battle::new_trainer_with_player_reserves`] with
    /// no player reserves.
    ///
    /// # Errors
    ///
    /// Returns [`BattleError::EmptyTrainerParty`] for an empty party,
    /// [`BattleError::UnknownTrainer`] for an unknown trainer,
    /// [`BattleError::FaintedBattler`] for a fainted active battler, or the
    /// first AI, move, or ability validation error. Errors leave the RNG untouched.
    pub fn new_trainer(
        dex: Dex,
        player: BattlePokemon,
        trainer: TrainerId,
        party: Vec<BattlePokemon>,
        rng: &mut impl BattleRng,
    ) -> Result<Self, BattleError> {
        Self::new_trainer_with_player_reserves(dex, player, Vec::new(), trainer, party, rng)
    }

    /// Starts a trainer battle with `party[0]` active and the rest on the
    /// bench, and `player_reserves` under the same ordered, headless
    /// replacement policy as [`Battle::new_with_player_reserves`]
    /// (`Cmd_checkteamslost`, `src/battle_script_commands.c:3534`-`:3564`).
    /// Every trainer party move is additionally validated against every
    /// non-fainted reserve, not just `player`.
    ///
    /// # Errors
    ///
    /// Returns [`BattleError::EmptyTrainerParty`] for an empty party,
    /// [`BattleError::UnknownTrainer`] for an unknown trainer,
    /// [`BattleError::FaintedBattler`] for a fainted active battler, or the
    /// first AI, move, or ability validation error against the active member
    /// or any non-fainted reserve, including
    /// [`BattleError::AmbiguousPlayerAbility`] for a two-ability player
    /// member without a trapping ability when the trainer scores with
    /// `CheckBadMove` and some non-fainted party member still has a
    /// PP-positive move. Errors leave the RNG untouched.
    pub fn new_trainer_with_player_reserves(
        dex: Dex,
        mut player: BattlePokemon,
        mut player_reserves: Vec<BattlePokemon>,
        trainer: TrainerId,
        mut party: Vec<BattlePokemon>,
        rng: &mut impl BattleRng,
    ) -> Result<Self, BattleError> {
        refresh_carried_speed_forme_stats(&mut player, &mut player_reserves);
        if party.is_empty() {
            return Err(BattleError::EmptyTrainerParty(trainer));
        }
        let data = trainer::trainer_data(trainer)?;
        trainer_ai::ensure_supported_flags(data.ai_flags)?;
        held_item::ensure_trainer_bag_unused(&data.items)?;
        if player.is_fainted() {
            return Err(BattleError::FaintedBattler(true));
        }
        if party[0].is_fainted() {
            return Err(BattleError::FaintedBattler(false));
        }
        for mon in &party {
            for (index, slot) in mon.moves().iter().enumerate() {
                // A depleted slot must still be real move data, but the AI
                // discards zero-PP slots before scoring and the no-PP abort
                // precedes any effect, so executable/scoreable admission
                // applies only to slots with PP remaining.
                if slot.move_id == MOVE_NONE {
                    return Err(BattleError::PlaceholderMove(index));
                }
                dex.move_data(slot.move_id)?;
                if slot.pp > 0 {
                    trainer::ensure_move_playable(&dex, slot.move_id, data.ai_flags)?;
                    for defender in std::iter::once(&player).chain(
                        player_reserves
                            .iter()
                            .filter(|reserve| !reserve.is_fainted()),
                    ) {
                        secondary::ensure_admissible(&dex, slot.move_id, mon, defender)?;
                    }
                }
            }
        }

        admission::ensure_participants_admissible(
            std::iter::once(&player)
                .chain(party.iter().filter(|mon| !mon.is_fainted()))
                .chain(
                    player_reserves
                        .iter()
                        .filter(|reserve| !reserve.is_fainted()),
                ),
        )?;

        held_item::ensure_participants_hold_playable_items(
            std::iter::once(&player)
                .chain(party.iter().filter(|mon| !mon.is_fainted()))
                .chain(
                    player_reserves
                        .iter()
                        .filter(|reserve| !reserve.is_fainted()),
                ),
        )?;

        // An all-spent battler is diverted to Struggle before the AI picks a
        // move (`battle_main.c:4183`-`:4191`), and PP never refills in
        // battle, so only a party that can still score reads the target's
        // ability.
        let party_can_score = party
            .iter()
            .any(|mon| !mon.is_fainted() && !all_known_moves_are_spent(mon));
        if party_can_score
            && data
                .ai_flags
                .contains(assets::trainers::AiFlags::CHECK_BAD_MOVE)
        {
            for target in std::iter::once(&player).chain(
                player_reserves
                    .iter()
                    .filter(|reserve| !reserve.is_fainted()),
            ) {
                trainer_ai::ensure_target_ability_unambiguous(target)?;
            }
        }

        let enemy = party.remove(0);
        let random_turn_number = initialize_turn_rng_state(&player, &enemy, rng);
        Ok(Self {
            dex,
            player,
            player_reserves,
            player_slot: 0,
            enemy,
            run_attempts: 0,
            random_turn_number,
            outcome: None,
            kind: BattleKind::Trainer(TrainerContext::new(trainer, data, party)),
            turn_counter: 0,
            turn_has_started: false,
            pending_residual_selection: None,
            retained_slots: [0; 2],
            last_move_used: MOVE_NONE,
            battler_states: [BattlerState::default(); 2],
        })
    }

    /// Returns the opponent's trainer context, if this is a trainer battle.
    #[must_use]
    pub const fn trainer(&self) -> Option<&TrainerContext> {
        match &self.kind {
            BattleKind::Trainer(context) => Some(context),
            BattleKind::Wild | BattleKind::FirstBattle => None,
        }
    }

    /// Returns a battler's move history and Perish Song state.
    #[must_use]
    pub const fn battler_state(&self, side: BattlerSide) -> &BattlerState {
        &self.battler_states[side.index()]
    }

    const fn is_first_battle(&self) -> bool {
        matches!(self.kind, BattleKind::FirstBattle)
    }

    /// Returns the zero-based battle-turn counter used by trainer AI.
    #[must_use]
    pub const fn turn_counter(&self) -> u8 {
        self.turn_counter
    }

    /// Returns the player's active Pokémon.
    #[must_use]
    pub const fn player(&self) -> &BattlePokemon {
        &self.player
    }

    /// Returns every player-side party member in the order it was passed to
    /// [`Battle::new_with_player_reserves`] or
    /// [`Battle::new_trainer_with_player_reserves`] -- the starting active
    /// member first, then each reserve -- whichever member is active now.
    ///
    /// The position is the stable link back to the caller's party slot: it
    /// survives every replacement, so two members that share species,
    /// personality, and original trainer id still reconcile to their own
    /// slots. Each entry carries its final HP, PP, and experience state;
    /// its identity ([`BattlePokemon::personality`],
    /// [`BattlePokemon::original_trainer_id`]) remains available for the
    /// owning flow to check each slot's record against, as it already does
    /// for a single lead.
    pub fn player_members(&self) -> impl Iterator<Item = &BattlePokemon> {
        let (before, after) = self.player_reserves.split_at(self.player_slot);
        before
            .iter()
            .chain(std::iter::once(&self.player))
            .chain(after.iter())
    }

    /// Returns the opponent's active Pokémon.
    #[must_use]
    pub const fn enemy(&self) -> &BattlePokemon {
        &self.enemy
    }

    /// Returns the outcome once the battle has ended.
    #[must_use]
    pub const fn outcome(&self) -> Option<BattleOutcome> {
        self.outcome
    }

    /// Returns the level-up move waiting for a learn or decline decision.
    ///
    /// A pending decision blocks another turn and defers the battle outcome or
    /// trainer replacement until [`Battle::resolve_move_learn`] resolves it.
    #[must_use]
    pub const fn pending_move_learn(&self) -> Option<PendingMoveLearn> {
        self.player.pending_move_learn()
    }

    /// Resolves the pending move-learning decision and returns its ordered events.
    ///
    /// Another prompt may follow immediately. Resolving the final prompt also
    /// releases any deferred replacement, prize money, and battle outcome, and
    /// then runs the residual pass the prompt held back — `rng` feeds that
    /// released pass, at the stream position it would have drawn from had no
    /// prompt deferred it.
    ///
    /// # Errors
    ///
    /// Returns [`BattleError::NoMoveLearnPending`] when no decision is waiting,
    /// [`BattleError::InvalidMoveSlot`] for an invalid replacement slot, or
    /// [`BattleError::HmMoveCantBeForgotten`] for an HM replacement; all three
    /// preserve the pending decision. Returns [`BattleError::UnknownSpecies`]
    /// when the released residual pass fells a battler whose reward needs a
    /// dex lookup.
    pub fn resolve_move_learn(
        &mut self,
        decision: MoveLearnDecision,
        rng: &mut impl BattleRng,
    ) -> Result<Vec<BattleEvent>, BattleError> {
        let asked_move = self
            .player
            .pending_move_learn()
            .ok_or(BattleError::NoMoveLearnPending)?
            .move_id();
        let resolution = self.player.resolve_move_learn(&self.dex, decision)?;

        let mut events = Vec::new();
        match resolution.learned {
            Some(learned) => events.push(BattleEvent::MoveReplaced {
                learned: learned.move_id,
                forgotten: learned.forgotten,
                slot: learned.slot,
            }),
            None => events.push(BattleEvent::MoveLearnDeclined {
                move_id: asked_move,
            }),
        }
        if let Some(next) = resolution.next {
            events.push(BattleEvent::MoveLearnPrompt {
                move_id: next.move_id(),
            });
        } else {
            self.settle_fainted_enemy(&mut events)?;
            // Upstream's yes/no box sits inside `HandleFaintedMonActions`,
            // which every path to the residual pass crosses first
            // (`src/battle_util.c:1912`-`:1923`).
            if let Some(chosen) = self.pending_residual_selection.take() {
                // Upstream zeroes `gCurrentMove` as each action closes, before
                // the residual pass runs, regardless of a move-learn prompt
                // deferring that pass to this later call
                // (`pokeemerald/src/battle_util.c:658`-`:671`).
                self.last_move_used = MOVE_NONE;
                self.residual_effects(chosen, &mut events, rng)?;
                self.handle_fainted_mons(&mut events)?;
            }
        }
        Ok(events)
    }

    /// Returns the number of ordinary run attempts made in this battle. A
    /// Run Away holder's escape does not advance this counter
    /// (`pokeemerald/src/battle_util.c:427`-`:447`, `:475`).
    #[must_use]
    pub const fn run_tries(&self) -> u8 {
        self.run_attempts
    }

    /// Returns the turn-number draw captured during construction or turn start.
    #[must_use]
    pub const fn random_turn_number(&self) -> u16 {
        self.random_turn_number
    }

    fn validate_player_move(&self, index: usize) -> Result<MoveId, BattleError> {
        let slot = self
            .player
            .moves()
            .get(index)
            .ok_or(BattleError::InvalidMoveSlot(index))?;
        if !selectable_slot(Some(slot.move_id)) {
            return Err(BattleError::PlaceholderMove(index));
        }
        // `Cmd_attackcanceler`'s no-PP abort exempts Struggle
        // (`battle_script_commands.c:934`).
        if slot.pp == 0 && slot.move_id != STRUGGLE {
            return Err(BattleError::NoPpRemaining(index));
        }
        ensure_executable(&self.dex, slot.move_id)?;
        secondary::ensure_admissible(&self.dex, slot.move_id, &self.player, &self.enemy)?;
        Ok(slot.move_id)
    }

    /// Re-screens the enemy's moveset with [`secondary::ensure_admissible`],
    /// the same PP gate construction used, against the current battlers
    /// rather than the ones construction saw; see the crate root docs for why.
    fn revalidate_enemy_admission(&self) -> Result<(), BattleError> {
        for slot in self.enemy.moves() {
            if slot.pp > 0 {
                secondary::ensure_admissible(&self.dex, slot.move_id, &self.enemy, &self.player)?;
            }
        }
        Ok(())
    }

    /// Resolves one player action and returns the resulting events in order.
    ///
    /// The action is validated before RNG is consumed. The opponent then chooses
    /// an action before a run attempt or move ordering resolves.
    ///
    /// # Errors
    ///
    /// Returns [`TurnError`] for an invalid state or action. Pre-turn failures
    /// consume no RNG and contain no events. A pending move-learning decision
    /// takes precedence over an existing outcome.
    pub fn take_turn(
        &mut self,
        player_action: PlayerAction,
        rng: &mut impl BattleRng,
    ) -> Result<Vec<BattleEvent>, TurnError> {
        let mut events = Vec::new();
        match self.resolve_turn(player_action, rng, &mut events) {
            Ok(()) => Ok(events),
            Err(error) => Err(TurnError { events, error }),
        }
    }

    /// Run Away or equal-or-greater raw Speed escapes without a draw and ends
    /// the turn before the enemy acts (`pokeemerald/src/battle_util.c:427-447,463-481`).
    fn escape_is_guaranteed(&self, action: ValidatedPlayerAction) -> bool {
        matches!(action, ValidatedPlayerAction::Run)
            && (self.player.ability() == AbilityId::RUN_AWAY
                || self.player.stats().speed >= self.enemy.stats().speed)
    }

    fn resolve_turn(
        &mut self,
        player_action: PlayerAction,
        rng: &mut impl BattleRng,
        events: &mut Vec<BattleEvent>,
    ) -> Result<(), BattleError> {
        let player_action = self.validate_player_action(player_action)?;
        if !self.escape_is_guaranteed(player_action) {
            self.revalidate_enemy_admission()?;
        }
        self.start_turn(rng);
        let enemy_action = self.choose_enemy_action(rng)?;
        self.resolve_selected_actions(player_action, enemy_action, rng, events)
    }

    /// Resolves the turn's two selected actions, then the end-of-turn pass.
    fn resolve_selected_actions(
        &mut self,
        player_action: ValidatedPlayerAction,
        enemy_action: EnemyAction,
        rng: &mut impl BattleRng,
        events: &mut Vec<BattleEvent>,
    ) -> Result<(), BattleError> {
        if let (ValidatedPlayerAction::UseMove { move_id, .. }, EnemyAction::Switch(slot)) =
            (player_action, enemy_action)
        {
            // The player's move was admitted against the outgoing defender.
            let incoming = match &self.kind {
                BattleKind::Trainer(context) => context.switch_target(slot)?,
                BattleKind::Wild | BattleKind::FirstBattle => {
                    return Err(BattleError::InvalidTrainerSwitch(slot));
                }
            };
            secondary::ensure_admissible(&self.dex, move_id, &self.player, incoming)?;
        }
        let chosen = ChosenMoves {
            player: match player_action {
                ValidatedPlayerAction::UseMove {
                    slot: Some(slot), ..
                } => Selection::Slot(slot),
                ValidatedPlayerAction::UseMove { slot: None, .. } => Selection::Struggle,
                ValidatedPlayerAction::Run => Selection::NoMove,
            },
            enemy: match enemy_action {
                EnemyAction::Move(slot) => Selection::Slot(slot),
                EnemyAction::Struggle => Selection::Struggle,
                EnemyAction::Flee | EnemyAction::Switch(_) => Selection::NoMove,
            },
        };
        if let Selection::Slot(slot) = chosen.player {
            self.retained_slots[0] = slot;
        }
        if let Selection::Slot(slot) = chosen.enemy {
            self.retained_slots[1] = slot;
        }
        let priorities = self.priorities_of(chosen, false)?;
        match player_action {
            ValidatedPlayerAction::UseMove { slot, move_id, .. } => {
                self.resolve_move_exchange(slot, move_id, priorities, enemy_action, rng, events)?;
            }
            ValidatedPlayerAction::Run => {
                self.resolve_run_attempt(enemy_action, rng, events)?;
                if self.outcome.is_some() {
                    return Ok(());
                }
            }
        }

        self.pass_turn(chosen, events, rng)
    }

    fn validate_player_action(
        &self,
        action: PlayerAction,
    ) -> Result<ValidatedPlayerAction, BattleError> {
        if let Some(pending) = self.player.pending_move_learn() {
            return Err(BattleError::MoveLearnPending(pending.move_id()));
        }
        if self.outcome.is_some() {
            return Err(BattleError::BattleAlreadyOver);
        }

        match action {
            PlayerAction::Run => match self.kind {
                BattleKind::Trainer(_) => Err(BattleError::NoRunningFromTrainer),
                // Run Away is admitted before this refusal
                // (`pokeemerald/src/battle_main.c:4038`-`:4039`, `:4078`-`:4082`).
                BattleKind::FirstBattle => {
                    if self.player.ability() == AbilityId::RUN_AWAY {
                        Ok(ValidatedPlayerAction::Run)
                    } else {
                        Err(BattleError::RunForbidden)
                    }
                }
                BattleKind::Wild => {
                    ensure_admissible(&self.player, &self.enemy)?;
                    Ok(ValidatedPlayerAction::Run)
                }
            },
            // `AreAllMovesUnusable` (`pokeemerald/src/battle_util.c:1125`-
            // `:1139`) forces Struggle before the chosen slot is read; the
            // cursor can only name an existing slot
            // (`src/battle_controller_player.c:552`-`:597`).
            PlayerAction::UseMove(slot) if all_known_moves_are_spent(&self.player) => {
                if slot >= self.player.moves().len() {
                    return Err(BattleError::InvalidMoveSlot(slot));
                }
                Ok(ValidatedPlayerAction::UseMove {
                    slot: None,
                    move_id: STRUGGLE,
                })
            }
            PlayerAction::UseMove(slot) => Ok(ValidatedPlayerAction::UseMove {
                slot: Some(slot),
                move_id: self.validate_player_move(slot)?,
            }),
        }
    }

    fn start_turn(&mut self, rng: &mut impl BattleRng) {
        if self.turn_has_started {
            self.turn_counter = self.turn_counter.saturating_add(1);
        }
        self.turn_has_started = true;
        self.random_turn_number = rng.next_u16();
    }

    fn choose_enemy_action(&self, rng: &mut impl BattleRng) -> Result<EnemyAction, BattleError> {
        match &self.kind {
            BattleKind::FirstBattle => Ok(choose_enemy_action_first_battle(
                &self.enemy,
                &self.player,
                rng,
            )),
            BattleKind::Trainer(context) => choose_trainer_action_in_battle(
                &self.dex,
                &self.enemy,
                &self.player,
                context.ai_flags(),
                self.turn_counter,
                Some(VoluntarySwitch::new(
                    context,
                    self.battler_state(BattlerSide::Enemy),
                )),
                rng,
            ),
            BattleKind::Wild => Ok(match choose_enemy_move(&self.enemy, rng) {
                Some(index) => EnemyAction::Move(index),
                None => EnemyAction::Struggle,
            }),
        }
    }

    fn resolve_run_attempt(
        &mut self,
        enemy_action: EnemyAction,
        rng: &mut impl BattleRng,
        events: &mut Vec<BattleEvent>,
    ) -> Result<(), BattleError> {
        // Run Away escapes with no draw and no `run_tries` increment
        // (`pokeemerald/src/battle_util.c:427`-`:447`, `:475`).
        let success = if self.player.ability() == AbilityId::RUN_AWAY {
            true
        } else {
            // Escape uses raw battle stats, not stage-modified turn-order
            // Speed (`src/battle_util.c:463`-`:465`).
            let success = try_run_from_battle(
                self.player.stats().speed,
                self.enemy.stats().speed,
                self.run_attempts,
                rng,
            );
            self.run_attempts = self.run_attempts.wrapping_add(1);
            success
        };
        events.push(BattleEvent::RunAttempt {
            by_player: true,
            success,
        });
        if success {
            self.finish(events, BattleOutcome::PlayerRan);
            return Ok(());
        }
        self.resolve_enemy_action(enemy_action, rng, events)
    }

    /// Priorities of the two selections against the battlers as they stand
    /// now; a missing slot reads as no move.
    ///
    /// `end_turn` is the rebuilt end-turn comparison, which reads a forced
    /// Struggle's retained slot instead of Struggle.
    fn priorities_of(
        &self,
        chosen: ChosenMoves,
        end_turn: bool,
    ) -> Result<ResidualPriorities, BattleError> {
        let slot_priority = |battler: &BattlePokemon, slot: usize| {
            battler.moves().get(slot).map_or(Ok(NO_MOVE_PRIORITY), |m| {
                self.dex.move_data(m.move_id).map(|data| data.priority)
            })
        };
        let priority =
            |selection: Selection, battler: &BattlePokemon, retained: usize| match selection {
                Selection::NoMove => Ok(NO_MOVE_PRIORITY),
                Selection::Slot(slot) => slot_priority(battler, slot),
                Selection::Struggle if end_turn => slot_priority(battler, retained),
                Selection::Struggle => self.dex.move_data(STRUGGLE).map(|data| data.priority),
            };
        Ok(ResidualPriorities {
            player: priority(chosen.player, &self.player, self.retained_slots[0])?,
            enemy: priority(chosen.enemy, &self.enemy, self.retained_slots[1])?,
        })
    }

    fn resolve_move_exchange(
        &mut self,
        player_slot: Option<usize>,
        player_move: MoveId,
        priorities: ResidualPriorities,
        enemy_action: EnemyAction,
        rng: &mut impl BattleRng,
        events: &mut Vec<BattleEvent>,
    ) -> Result<(), BattleError> {
        // `SetActionsAndBattlersTurnOrder` queues switch actions ahead of
        // every move action, so a switch consumes no order comparison
        // (`pokeemerald/src/battle_main.c:4817`-`:4847`).
        if matches!(enemy_action, EnemyAction::Switch(_)) {
            self.resolve_enemy_action(enemy_action, rng, events)?;
            self.act(true, player_move, player_slot, rng, events)?;
            return Ok(());
        }
        let order = resolve_order(
            priorities.player,
            priorities.enemy,
            self.player.speed_for_turn_order(),
            self.enemy.speed_for_turn_order(),
            rng,
        );

        match order {
            Order::AttackerFirst => {
                self.act(true, player_move, player_slot, rng, events)?;
                if self.both_battlers_can_act() {
                    self.resolve_enemy_action(enemy_action, rng, events)?;
                }
            }
            Order::DefenderFirst => {
                self.resolve_enemy_action(enemy_action, rng, events)?;
                if self.both_battlers_can_act() {
                    self.act(true, player_move, player_slot, rng, events)?;
                }
            }
        }
        Ok(())
    }

    fn both_battlers_can_act(&self) -> bool {
        self.outcome.is_none() && !self.player.is_fainted() && !self.enemy.is_fainted()
    }

    fn pass_turn(
        &mut self,
        chosen: ChosenMoves,
        events: &mut Vec<BattleEvent>,
        rng: &mut impl BattleRng,
    ) -> Result<(), BattleError> {
        // Upstream reaches `HandleFaintedMonActions` twice a turn: from each
        // action's own `Cmd_end`, and again behind `BattleTurnPassed`'s
        // residual pass (`src/battle_main.c:549`, `:3960`-`:3968`).
        self.handle_fainted_mons(events)?;
        if self.player.pending_move_learn().is_some() {
            self.pending_residual_selection = Some(chosen);
            return Ok(());
        }
        // Upstream zeroes `gCurrentMove` as each action closes, before the
        // residual pass runs (`pokeemerald/src/battle_util.c:658`-`:671`).
        self.last_move_used = MOVE_NONE;
        self.residual_effects(chosen, events, rng)?;
        self.handle_fainted_mons(events)
    }

    fn residual_effects(
        &mut self,
        chosen: ChosenMoves,
        events: &mut Vec<BattleEvent>,
        rng: &mut impl BattleRng,
    ) -> Result<(), BattleError> {
        if self.outcome.is_some() {
            return Ok(());
        }
        // `DoFieldEndTurnEffects` rebuilds `gBattlerByTurnOrder` from battler
        // identity and compares the pair afresh, at the battlers' current
        // Speed, so a Speed-stage change or a surviving tie is re-decided
        // rather than inherited from the action phase
        // (`src/battle_util.c:1199`-`:1210`).
        let priorities = self.priorities_of(chosen, true)?;
        let order = resolve_order(
            priorities.player,
            priorities.enemy,
            self.player.speed_for_turn_order(),
            self.enemy.speed_for_turn_order(),
            rng,
        );
        // `DoBattlerEndTurnEffects` walks that rebuilt order and within each battler's pass reaches
        // ENDTURN_ABILITIES (Shed Skin) before ENDTURN_POISON, and
        // ENDTURN_POISON before ENDTURN_CHARGE
        // (`src/battle_util.c:1442`-`:1474`, `:1494`-`:1535`).
        let player_first = matches!(order, Order::AttackerFirst);
        for is_player in [player_first, !player_first] {
            let already_fainted = if is_player {
                self.player.is_fainted()
            } else {
                self.enemy.is_fainted()
            };
            if already_fainted {
                continue;
            }
            self.apply_shed_skin_residual(is_player, rng, events);
            self.apply_poison_residual(is_player, events);
            if is_player {
                self.player.volatiles_mut().tick_charge();
            } else {
                self.enemy.volatiles_mut().tick_charge();
            }
            let now_fainted = if is_player {
                self.player.is_fainted()
            } else {
                self.enemy.is_fainted()
            };
            if now_fainted {
                self.settle_faint(is_player, events);
                if self.fainting_decides_the_battle(is_player) {
                    break;
                }
            }
        }
        Ok(())
    }

    fn fainting_decides_the_battle(&self, is_player: bool) -> bool {
        // `Cmd_checkteamslost` totals a whole party's HP
        // (`src/battle_script_commands.c:3534`-`:3577`); the player side is
        // exhausted only once its active member and every reserve have
        // fainted.
        if is_player {
            return self.usable_player_reserves() == 0;
        }
        match &self.kind {
            BattleKind::Trainer(context) => context.bench().iter().all(BattlePokemon::is_fainted),
            BattleKind::Wild | BattleKind::FirstBattle => true,
        }
    }

    /// `ABILITYEFFECT_ENDTURN`'s `ABILITY_SHED_SKIN` case: a living battler
    /// whose ability is Shed Skin draws to cure its own primary status
    /// (`pokeemerald/src/battle_util.c:2601`-`:2602`, `:2620`-`:2640`).
    fn apply_shed_skin_residual(
        &mut self,
        is_player: bool,
        rng: &mut impl BattleRng,
        events: &mut Vec<BattleEvent>,
    ) {
        let battler = if is_player { &self.player } else { &self.enemy };
        if battler.is_fainted() || battler.ability() != AbilityId::SHED_SKIN {
            return;
        }
        let status = battler.status1();
        if !draws_shed_skin_cure(status, rng) {
            return;
        }
        let target = if is_player {
            &mut self.player
        } else {
            &mut self.enemy
        };
        target.set_status1(Status1::Healthy);
        events.push(BattleEvent::ShedSkinCured {
            by_player: is_player,
            status,
        });
    }

    fn apply_poison_residual(&mut self, is_player: bool, events: &mut Vec<BattleEvent>) {
        let battler = if is_player { &self.player } else { &self.enemy };
        if battler.is_fainted() || !battler.status1().is_poisoned() {
            return;
        }
        let damage = poison_residual_damage(battler.stats().max_hp);
        let target = if is_player {
            &mut self.player
        } else {
            &mut self.enemy
        };
        let dealt = damage.min(target.current_hp());
        target.apply_damage(dealt);
        events.push(BattleEvent::HurtByPoison {
            by_player: is_player,
            damage: dealt,
        });
    }

    fn handle_fainted_mons(&mut self, events: &mut Vec<BattleEvent>) -> Result<(), BattleError> {
        if self.outcome.is_some() || self.player.pending_move_learn().is_some() {
            return Ok(());
        }
        // `Cmd_checkteamslost` ORs both sides' outcome bits from independent
        // whole-party HP totals (`src/battle_script_commands.c:3534`-
        // `:3577`); an exhausted player party sets the loss regardless of
        // the enemy's own total, so this precedes any enemy-side handling.
        if self.player.is_fainted() && self.usable_player_reserves() == 0 {
            self.finish(events, BattleOutcome::PlayerLost);
            return Ok(());
        }
        if !self.enemy.is_fainted() {
            if self.player.is_fainted() {
                self.send_out_next_player_reserve(events);
            }
            return Ok(());
        }
        // Case 1 runs `BattleScript_GiveExp` to completion, the yes/no box
        // included, before case 4 replaces or pays out
        // (`src/battle_util.c:1912`-`:1946`). This runs against whichever
        // member was active during the exchange, ahead of any reserve
        // replacement below.
        self.settle_enemy_reward(events)?;
        if self.player.pending_move_learn().is_some() {
            return Ok(());
        }
        // `HandleFaintedMonActions` case 4 walks battlers from 0
        // (`src/battle_util.c:1924`-`:1935`), so the player's replacement is
        // out before the trainer picks its own against it. A battle the
        // enemy's faint ends never reaches the player's send-out branch
        // (`data/battle_scripts_1.s:2830`-`:2832`).
        if self.player.is_fainted() && self.trainer_has_usable_bench() {
            self.send_out_next_player_reserve(events);
        }
        self.settle_fainted_enemy(events)
    }

    fn trainer_has_usable_bench(&self) -> bool {
        matches!(&self.kind, BattleKind::Trainer(context)
            if context.bench().iter().any(|mon| !mon.is_fainted()))
    }

    /// The player-side analogue of [`Battle::settle_fainted_enemy`]:
    /// `Cmd_checkteamslost`'s player-side HP total
    /// (`src/battle_script_commands.c:3534`-`:3564`) reads zero only once
    /// every party member has fainted, so `BattleScript_HandleFaintedMon`
    /// reaches its send-out branch first
    /// (`data/battle_scripts_1.s:2830`-`:2896`). This crate's headless
    /// party-order policy always takes the first non-fainted reserve, with
    /// no player choice and no party-screen UI.
    ///
    /// The fainted member goes back to its own party position among the
    /// reserves, so [`Battle::player_members`] keeps the caller's order:
    /// upstream likewise leaves every `gPlayerParty` record in its slot and
    /// only repoints `gBattlerPartyIndexes` at the replacement
    /// (`src/battle_script_commands.c:4613`).
    ///
    /// Returns `false`, leaving `self.player` untouched, once every reserve
    /// is fainted too.
    fn send_out_next_player_reserve(&mut self, events: &mut Vec<BattleEvent>) -> bool {
        let Some(index) = self
            .player_reserves
            .iter()
            .position(|reserve| !reserve.is_fainted())
        else {
            return false;
        };
        // `player_reserves` holds every party position but `player_slot`, in
        // order, so reserve `index` sits at party position `index` below the
        // active member's and one past it above.
        let replacement_slot = if index < self.player_slot {
            index
        } else {
            index + 1
        };
        let replacement = self.player_reserves.remove(index);
        let fainted = std::mem::replace(&mut self.player, replacement);
        // Likewise, with both members out of the list, the fainted member's
        // party position is its reserve index below the replacement's and
        // one less above it.
        let fainted_index = if self.player_slot < replacement_slot {
            self.player_slot
        } else {
            self.player_slot - 1
        };
        self.player_reserves.insert(fainted_index, fainted);
        self.player_slot = replacement_slot;
        // `SwitchInClearSetData` for the incoming battler
        // (`pokeemerald/src/battle_main.c:3152`-`:3261`), which zeroes
        // `gCurrentMove` too: after a double faint the player's send-out runs
        // first, so the trainer's replacement scores against `MOVE_NONE`
        // (`battle_util.c:1936`-`:1946`, `battle_ai_switch_items.c:772`-`:779`).
        self.player.clear_battle_scratch();
        self.battler_states[BattlerSide::Player.index()] = BattlerState::default();
        self.last_move_used = MOVE_NONE;
        events.push(BattleEvent::PlayerSentOut {
            species: self.player.species(),
            reserves_remaining: self.usable_player_reserves(),
        });
        true
    }

    /// The number of player-side reserves that have not fainted.
    fn usable_player_reserves(&self) -> usize {
        self.player_reserves
            .iter()
            .filter(|reserve| !reserve.is_fainted())
            .count()
    }

    fn settle_enemy_reward(&mut self, events: &mut Vec<BattleEvent>) -> Result<(), BattleError> {
        // `Cmd_getexp` case 2 zeroes the award and jumps past both the string
        // and `MonGainEVs` for a recipient already at the cap
        // (`src/battle_script_commands.c:3351`-`:3356`), and does the same
        // for a recipient at zero HP -- a simultaneous double faint's own
        // victor included (`:3367`, `:3431`).
        if self.player.level() >= MAX_LEVEL || self.player.is_fainted() {
            return Ok(());
        }
        let defeated = self.dex.species(self.enemy.species())?;
        let level = self.enemy.level();
        let exp = if self.trainer().is_some() {
            trainer_faint_exp(defeated.base_exp, level)
        } else {
            wild_faint_exp(defeated.base_exp, level)
        };
        // `MonGainEVs` runs ahead of the exp and level-up sequence, so a
        // level crossed this turn snapshots the gain
        // (`src/battle_script_commands.c:3420`).
        self.player.gain_evs(defeated.ev_yield);
        let pending = self.player.apply_experience(&self.dex, exp)?;
        events.push(BattleEvent::ExpGained(exp));
        if let Some(prompt) = pending {
            events.push(BattleEvent::MoveLearnPrompt {
                move_id: prompt.move_id(),
            });
        }
        Ok(())
    }

    fn settle_fainted_enemy(&mut self, events: &mut Vec<BattleEvent>) -> Result<(), BattleError> {
        if self.outcome.is_some() || !self.enemy.is_fainted() {
            return Ok(());
        }
        let (replacement, money) = match &self.kind {
            BattleKind::Trainer(context) => (
                context.select_replacement(
                    &self.dex,
                    &self.enemy,
                    self.last_move_used,
                    &self.player,
                )?,
                context.money(),
            ),
            BattleKind::Wild | BattleKind::FirstBattle => {
                self.finish(events, BattleOutcome::PlayerWon);
                return Ok(());
            }
        };
        if let Some(slot) = replacement {
            return self.switch_enemy_to(slot, events);
        }
        events.push(BattleEvent::MoneyGained(money));
        self.finish(events, BattleOutcome::PlayerWon);
        Ok(())
    }

    /// Makes original party slot `slot` the active enemy: the outgoing
    /// member returns to its own slot and the incoming one enters with
    /// `SwitchInClearSetData`'s reset (`pokeemerald/src/battle_main.c:3152`-
    /// `:3261`). A prechosen voluntary switch and a forced post-faint
    /// replacement both land here; neither draws from `rng`.
    ///
    /// # Errors
    ///
    /// Returns [`BattleError::InvalidTrainerSwitch`] when this is not a
    /// trainer battle or `slot` is not a healthy non-active member; nothing
    /// changes then.
    fn switch_enemy_to(
        &mut self,
        slot: usize,
        events: &mut Vec<BattleEvent>,
    ) -> Result<(), BattleError> {
        let BattleKind::Trainer(context) = &mut self.kind else {
            return Err(BattleError::InvalidTrainerSwitch(slot));
        };
        context.switch_target(slot)?;
        // `Cmd_switchoutabilities` cures the leaving member before it returns
        // to its party record (`battle_script_commands.c:9590`-`:9603`). A
        // fainted member leaving for a forced replacement is already healthy.
        if self.enemy.ability() == AbilityId::NATURAL_CURE {
            self.enemy.set_status1(Status1::Healthy);
        }
        self.enemy.clear_battle_scratch();
        context.switch_active(&mut self.enemy, slot)?;
        self.enemy.clear_battle_scratch();
        self.battler_states[BattlerSide::Enemy.index()] = BattlerState::default();
        // `SwitchInClearSetData` zeroes `gCurrentMove` (`battle_main.c:3257`).
        self.last_move_used = MOVE_NONE;
        events.push(BattleEvent::TrainerSentOut {
            species: self.enemy.species(),
            bench_remaining: context.bench_len(),
        });
        Ok(())
    }

    /// Runs one battler's turn-order slot, then closes the action as
    /// `HandleAction_ActionFinished` does: the acting battler's own
    /// last-landed move returns to none (`pokeemerald/src/battle_util.c:671`).
    /// Its last hitter and the move it landed on its target remain.
    fn act(
        &mut self,
        player_is_attacker: bool,
        move_id: MoveId,
        slot: Option<usize>,
        rng: &mut impl BattleRng,
        events: &mut Vec<BattleEvent>,
    ) -> Result<(), BattleError> {
        let result = self.run_action(player_is_attacker, move_id, slot, rng, events);
        self.battler_states[BattlerSide::from_is_player(player_is_attacker).index()]
            .clear_last_landed_move();
        result
    }

    /// Runs one battler's turn-order slot: the confusion decrement, the
    /// full-paralysis draw, the Soundproof block, PP handling, then
    /// execution.
    ///
    /// `slot` is `None` for a forced Struggle, which spends no PP
    /// (`HITMARKER_NO_PPDEDUCT`, `pokeemerald/src/battle_util.c:100`-`:104`).
    fn run_action(
        &mut self,
        player_is_attacker: bool,
        move_id: MoveId,
        slot: Option<usize>,
        rng: &mut impl BattleRng,
        events: &mut Vec<BattleEvent>,
    ) -> Result<(), BattleError> {
        // `gCurrentMove` is set for the chosen action before anything can
        // gate its execution (`HandleAction_UseMove`,
        // `pokeemerald/src/battle_util.c:78`-`:137`), so it holds the move
        // even when full paralysis or empty PP stop this one from landing.
        self.last_move_used = move_id;
        // `CANCELER_ASLEEP` decrements sleep ahead of every other modelled
        // canceller (`src/battle_util.c:2015`-`:2057`): a duration that
        // survives cancels the move, expiry lets it proceed.
        let attacker = if player_is_attacker {
            &mut self.player
        } else {
            &mut self.enemy
        };
        match tick_sleep(attacker.status1(), attacker.ability()) {
            SleepTick::NotAsleep => {}
            SleepTick::StillAsleep(status) => {
                attacker.set_status1(status);
                events.push(BattleEvent::FastAsleep {
                    by_player: player_is_attacker,
                    move_id,
                });
                self.record_cancelled_move(player_is_attacker, move_id)?;
                return Ok(());
            }
            SleepTick::WokeUp => {
                attacker.set_status1(Status1::Healthy);
                events.push(BattleEvent::WokeUp {
                    by_player: player_is_attacker,
                });
            }
        }
        // `CANCELER_CONFUSED` decrements the attacker's confusion duration
        // ahead of `CANCELER_PARALYZED` (`src/battle_util.c:2157`-`:2199`).
        // A duration that survives the decrement draws once more before the
        // paralysis draw and PP handling: a self-hit ends the action here.
        let attacker = if player_is_attacker {
            &mut self.player
        } else {
            &mut self.enemy
        };
        if attacker.volatiles_mut().tick_confusion() {
            events.push(BattleEvent::SnappedOutOfConfusion {
                by_player: player_is_attacker,
            });
        } else if attacker.volatiles().confused() {
            events.push(BattleEvent::Confused {
                by_player: player_is_attacker,
            });
            if draws_confusion_self_hit(rng) {
                self.apply_confusion_self_hit(player_is_attacker, rng, events);
                self.record_confusion_self_hit(player_is_attacker);
                return Ok(());
            }
        }
        let attacker_status1 = if player_is_attacker {
            self.player.status1()
        } else {
            self.enemy.status1()
        };
        if draws_full_paralysis(attacker_status1, rng) {
            events.push(BattleEvent::FullyParalyzed {
                by_player: player_is_attacker,
                move_id,
            });
            self.record_cancelled_move(player_is_attacker, move_id)?;
            return Ok(());
        }
        let pp_cost = self.move_pp_cost(player_is_attacker, move_id)?;
        // `attackcanceler` runs the Soundproof move-block ahead of its no-PP
        // test (`battle_script_commands.c:932-939`); the blocked script's
        // `ppreduce` spends nothing from an empty slot (`:1230`).
        let (attacker, defender) = if player_is_attacker {
            (&mut self.player, &self.enemy)
        } else {
            (&mut self.enemy, &self.player)
        };
        if stat_change::soundproof_block(&self.dex, move_id, defender)? {
            if let Some(slot) = slot {
                if attacker.moves()[slot].pp > 0 {
                    attacker.deduct_pp_by(slot, pp_cost)?;
                }
            }
            events.push(BattleEvent::SoundproofProtected {
                by_player: player_is_attacker,
                move_id,
            });
            self.record_cancelled_move(player_is_attacker, move_id)?;
            return Ok(());
        }
        if let Some(slot) = slot {
            let slot_pp = if player_is_attacker {
                self.player.moves()[slot].pp
            } else {
                self.enemy.moves()[slot].pp
            };
            if slot_pp == 0 {
                // `Cmd_attackcanceler` exempts Struggle from the no-PP abort
                // (`battle_script_commands.c:934`); `Cmd_ppreduce` spends
                // nothing from an empty slot (`:1230`).
                if move_id != STRUGGLE {
                    events.push(BattleEvent::FailedNoPp {
                        by_player: player_is_attacker,
                        move_id,
                    });
                    self.record_cancelled_move(player_is_attacker, move_id)?;
                    return Ok(());
                }
            } else if player_is_attacker {
                self.player.deduct_pp_by(slot, pp_cost)?;
            } else {
                self.enemy.deduct_pp_by(slot, pp_cost)?;
            }
        }
        self.execute_move(player_is_attacker, move_id, rng, events)
    }

    /// PP a move spends: two against a distinct target with Pressure, one
    /// otherwise; self-targeted moves are exempt
    /// (`battle_script_commands.c:1205`-`:1228`).
    fn move_pp_cost(&self, player_is_attacker: bool, move_id: MoveId) -> Result<u8, BattleError> {
        let target = self.dex.move_data(move_id)?.target;
        if target == MoveTarget::USER {
            return Ok(1);
        }
        let defender = if player_is_attacker {
            &self.enemy
        } else {
            &self.player
        };
        if defender.ability() == AbilityId::PRESSURE {
            Ok(2)
        } else {
            Ok(1)
        }
    }

    fn resolve_enemy_action(
        &mut self,
        action: EnemyAction,
        rng: &mut impl BattleRng,
        events: &mut Vec<BattleEvent>,
    ) -> Result<(), BattleError> {
        match action {
            EnemyAction::Move(slot) => self.act(
                false,
                self.enemy.moves()[slot].move_id,
                Some(slot),
                rng,
                events,
            ),
            // An all-spent opponent's forced Struggle runs through the same
            // slot as an ordinary move, just with no PP to deduct
            // (`AreAllMovesUnusable`, `pokeemerald/src/battle_util.c:1125`-
            // `:1139`, `:100`-`:104`).
            EnemyAction::Struggle => self.act(false, STRUGGLE, None, rng, events),
            EnemyAction::Switch(slot) => self.switch_enemy_to(slot, events),
            EnemyAction::Flee => {
                events.push(BattleEvent::WildFled);
                self.finish(events, BattleOutcome::WildFled);
                Ok(())
            }
        }
    }

    fn finish(&mut self, events: &mut Vec<BattleEvent>, outcome: BattleOutcome) {
        self.outcome = Some(outcome);
        events.push(BattleEvent::Ended(outcome));
    }
}

#[cfg(test)]
mod tests {
    use assets::trainers::AiFlags;
    use assets::SpeciesId;

    use super::*;
    use crate::pokemon::Ivs;
    use crate::script_rng::SequenceRng;

    const MAX_IVS: Ivs = Ivs {
        hp: 31,
        attack: 31,
        defense: 31,
        speed: 31,
        sp_attack: 31,
        sp_defense: 31,
    };
    const ABSORB: MoveId = MoveId::ABSORB;
    const TACKLE: MoveId = MoveId::TACKLE;
    const BULBASAUR: SpeciesId = SpeciesId(1);
    const SQUIRTLE: SpeciesId = SpeciesId(7);
    const TENTACOOL: SpeciesId = SpeciesId(72);
    const MAY_ROUTE_103_MUDKIP: TrainerId = TrainerId(529);
    const ENEMY_LEVEL: u8 = 50;
    const PLAYER_LEVEL: u8 = 30;
    const ENEMY_REMAINING_HP: u32 = 5;
    const ENEMY_SPEED: u32 = 65;
    const PLAYER_SPEED: u32 = 56;
    const LIQUID_OOZE_PERSONALITY: u32 = 1;
    const PLAYER_MOVE_SLOT: usize = 0;
    const DRAWS_THROUGH_ENEMY_ABSORB: usize = 9;

    fn trainer_battle_with_faster_absorb_user() -> (Battle, u32, u8) {
        let dex = Dex::new();
        let mut enemy = BattlePokemon::new(&dex, BULBASAUR, ENEMY_LEVEL, MAX_IVS, 0, vec![ABSORB])
            .expect("valid enemy");
        assert_eq!(enemy.stats().speed, ENEMY_SPEED);
        enemy.apply_damage(enemy.stats().max_hp - ENEMY_REMAINING_HP);
        let player = BattlePokemon::new(
            &dex,
            TENTACOOL,
            PLAYER_LEVEL,
            MAX_IVS,
            LIQUID_OOZE_PERSONALITY,
            vec![TACKLE],
        )
        .expect("valid player");
        assert_eq!(player.stats().speed, PLAYER_SPEED);
        assert_eq!(player.ability(), crate::ability::LIQUID_OOZE);
        let player_max_hp = player.stats().max_hp;

        let mut trainer_without_ai =
            *trainer::trainer_data(MAY_ROUTE_103_MUDKIP).expect("known trainer");
        trainer_without_ai.ai_flags = AiFlags::NONE;
        let bench =
            vec![
                BattlePokemon::new(&dex, SQUIRTLE, PLAYER_LEVEL, MAX_IVS, 0, vec![TACKLE])
                    .expect("valid bench mon"),
            ];
        let trainer = TrainerContext::new(MAY_ROUTE_103_MUDKIP, &trainer_without_ai, bench);
        let player_move_max_pp = dex.move_data(TACKLE).expect("known move").pp;

        let battle = Battle {
            dex,
            player,
            player_reserves: Vec::new(),
            player_slot: 0,
            enemy,
            run_attempts: 0,
            random_turn_number: 0,
            outcome: None,
            kind: BattleKind::Trainer(trainer),
            turn_counter: 0,
            turn_has_started: false,
            pending_residual_selection: None,
            retained_slots: [0; 2],
            last_move_used: MOVE_NONE,
            battler_states: [BattlerState::default(); 2],
        };
        (battle, player_max_hp, player_move_max_pp)
    }

    #[test]
    fn liquid_ooze_recoil_ko_skips_the_players_queued_move() {
        let (mut battle, player_max_hp, player_move_max_pp) =
            trainer_battle_with_faster_absorb_user();

        let mut rng = SequenceRng::new([0; DRAWS_THROUGH_ENEMY_ABSORB]);
        let events = battle
            .take_turn(PlayerAction::UseMove(PLAYER_MOVE_SLOT), &mut rng)
            .unwrap();

        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, BattleEvent::Fainted { by_player: false }))
                .count(),
            1,
            "the enemy should faint exactly once: {events:?}"
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, BattleEvent::ExpGained(_)))
                .count(),
            1,
            "experience should be awarded exactly once: {events:?}"
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(
                    e,
                    BattleEvent::Hit {
                        by_player: true,
                        ..
                    }
                ))
                .count(),
            0,
            "the player's queued move should not execute: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, BattleEvent::Fainted { by_player: true })),
            "the player never fainted here: {events:?}"
        );
        let player_damage_taken: u32 = events
            .iter()
            .filter_map(|e| match e {
                BattleEvent::Hit {
                    by_player: false,
                    damage,
                    ..
                } => Some(*damage),
                _ => None,
            })
            .sum();
        assert_eq!(
            battle.player().current_hp(),
            player_max_hp - player_damage_taken,
            "only the enemy's hit should reduce player HP: {events:?}"
        );
        assert_eq!(
            battle.player().moves()[PLAYER_MOVE_SLOT].pp,
            player_move_max_pp,
            "the skipped move should preserve PP"
        );
        assert_eq!(
            battle.outcome(),
            None,
            "the trainer's bench is not empty, so the battle continues"
        );
        assert!(
            events.contains(&BattleEvent::TrainerSentOut {
                species: SQUIRTLE,
                bench_remaining: 0,
            }),
            "the replacement should enter after the turn: {events:?}"
        );
        assert_eq!(
            battle.enemy().species(),
            SQUIRTLE,
            "the replacement is active by the time `take_turn` returns"
        );
    }
}
