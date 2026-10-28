//! Voluntary trainer switches chosen before any move is scored.
//!
//! `ShouldSwitch` (`pokeemerald/src/battle_ai_switch_items.c:429`) runs from
//! `AI_TrySwitchOrUseItem` (`:528`) at action selection, ahead of
//! `BattleAI_ChooseMoveOrAction`. This module is the singles half of that
//! decision: the trapping prelude (wrap, root, Shadow Tag, Arena Trap, Magnet
//! Pull against Steel, arena battles) is not modelled, and neither is
//! `ShouldUseItem` or any double-battle branch. A bench with no healthy
//! member returns before the first probe, so a lone Pokémon draws nothing
//! here.
//!
//! Probes read the active battler's real ability and the bench member's real
//! ability. Type matchups are `AI_TypeCalc` flags (`battle_script_commands.c:
//! 1594`), not the scoring chart. `ShouldSwitchIfNaturalCure`'s else-if reads
//! `gBattleMoves[MOVE_UNAVAILABLE]` when the last landed move is that
//! sentinel and the first coin fails; that index is not in the move table, so
//! the read is refused and logged instead of guessed.

use assets::{AbilityId, Effectiveness, MoveId, Type, TypeChart};

use super::super::battler_state::{BattlerSide, BattlerState, LastLandedMove, PerishSongTurns};
use super::super::trainer::TrainerContext;
use crate::damage::BattleRng;
use crate::dex::Dex;
use crate::error::BattleError;
use crate::pokemon::BattlePokemon;

const MISSED: u8 = 1 << 0;
const SUPER_EFFECTIVE: u8 = 1 << 1;
const NOT_VERY_EFFECTIVE: u8 = 1 << 2;
const DOESNT_AFFECT: u8 = 1 << 3;
const FAILED: u8 = 1 << 5;
const NO_EFFECT: u8 = MISSED | DOESNT_AFFECT | FAILED;

const WONDER_GUARD_MODULUS: u16 = 3;
const WONDER_GUARD_ACCEPT_BELOW: u16 = 2;
const ABSORBER_STAY_MODULUS: u16 = 3;
const OWN_SUPER_MODULUS: u16 = 10;
const IMMUNITY_MODULUS: u16 = 2;
const RESISTED_MODULUS: u16 = 3;
const GUARANTEED_MODULUS: u16 = 1;
const RAISED_STAGE_LIMIT: u8 = 3;

/// The party and field history a voluntary switch reads.
///
/// `history` is the active enemy's `gLastLandedMoves` / `gLastHitBy` /
/// Perish Song timer. `party` is the trainer roster child 1 already keeps in
/// original-slot order.
pub(crate) struct VoluntarySwitch<'a> {
    party: &'a TrainerContext,
    history: &'a BattlerState,
}

impl<'a> VoluntarySwitch<'a> {
    /// Borrows the roster and the active enemy's field history.
    #[must_use]
    pub(crate) const fn new(party: &'a TrainerContext, history: &'a BattlerState) -> Self {
        Self { party, history }
    }
}

/// A bench slot a probe named, or upstream's `PARTY_SIZE` "any mon" sentinel.
#[derive(Clone, Copy)]
enum Replacement {
    Slot(usize),
    Any,
}

struct Probe<'a> {
    dex: &'a Dex,
    party: &'a TrainerContext,
    active: &'a BattlePokemon,
    player: &'a BattlePokemon,
    history: &'a BattlerState,
}

/// Runs the switch half of `AI_TrySwitchOrUseItem` before move scoring.
///
/// `None` for `switch_view`, or a party whose healthy bench is empty, returns
/// `Ok(None)` without drawing. `Ok(Some(slot))` is the original party slot to
/// send in. [`BattleError::UndefinedNaturalCureMove`] is the refused
/// `MOVE_UNAVAILABLE` read, after the coin that reaches it and before any
/// move-table lookup.
///
/// # Errors
///
/// Returns [`BattleError::UndefinedNaturalCureMove`] or a move-data error
/// from a probe that does look a real move up.
pub(super) fn try_voluntary_switch(
    dex: &Dex,
    active: &BattlePokemon,
    player: &BattlePokemon,
    switch_view: Option<VoluntarySwitch<'_>>,
    rng: &mut impl BattleRng,
) -> Result<Option<usize>, BattleError> {
    let Some(switch_view) = switch_view else {
        return Ok(None);
    };
    if switch_view.party.bench_len() == 0 {
        return Ok(None);
    }
    let probe = Probe {
        dex,
        party: switch_view.party,
        active,
        player,
        history: switch_view.history,
    };
    match probe.decide(rng)? {
        None => Ok(None),
        Some(replacement) => Ok(Some(probe.resolve(replacement)?)),
    }
}

impl Probe<'_> {
    fn decide(&self, rng: &mut impl BattleRng) -> Result<Option<Replacement>, BattleError> {
        if let Some(replacement) = self.perish_song() {
            return Ok(Some(replacement));
        }
        if let Some(replacement) = self.wonder_guard(rng)? {
            return Ok(Some(replacement));
        }
        if let Some(replacement) = self.absorber(rng)? {
            return Ok(Some(replacement));
        }
        if let Some(replacement) = self.natural_cure(rng)? {
            return Ok(Some(replacement));
        }
        if self.keeps_own_super_effective_move(rng)? || self.stats_are_raised() {
            return Ok(None);
        }
        if let Some(slot) = self.find_replacement(rng, DOESNT_AFFECT, IMMUNITY_MODULUS)? {
            return Ok(Some(Replacement::Slot(slot)));
        }
        if let Some(slot) = self.find_replacement(rng, NOT_VERY_EFFECTIVE, RESISTED_MODULUS)? {
            return Ok(Some(Replacement::Slot(slot)));
        }
        Ok(None)
    }

    /// `ShouldSwitchIfPerishSong`: a countdown of zero names any bench member
    /// and draws nothing. Any other timer, including no song, falls through.
    fn perish_song(&self) -> Option<Replacement> {
        match self.history.perish_song() {
            Some(PerishSongTurns(0)) => Some(Replacement::Any),
            Some(PerishSongTurns(_)) | None => None,
        }
    }

    /// `ShouldSwitchIfWonderGuard`, singles only. No draw unless a bench move
    /// is super effective; that move accepts on `Random() % 3 < 2`.
    fn wonder_guard(&self, rng: &mut impl BattleRng) -> Result<Option<Replacement>, BattleError> {
        if self.player.ability() != AbilityId::WONDER_GUARD {
            return Ok(None);
        }
        if self.active_has_super_effective_move()? {
            return Ok(None);
        }
        for (slot, candidate) in self.party.candidates() {
            if candidate.is_fainted() {
                continue;
            }
            for known in candidate.moves() {
                let flags = ai_move_result_flags(self.dex, known.move_id, self.player)?;
                if (flags & SUPER_EFFECTIVE) != 0
                    && rng.next_u16() % WONDER_GUARD_MODULUS < WONDER_GUARD_ACCEPT_BELOW
                {
                    return Ok(Some(Replacement::Slot(slot)));
                }
            }
        }
        Ok(None)
    }

    /// `FindMonThatAbsorbsOpponentsMove`. A super-effective move of our own
    /// draws `Random() % 3` first and stays unless the roll is zero. The last
    /// landed move is not read when it was never recorded or did not land.
    fn absorber(&self, rng: &mut impl BattleRng) -> Result<Option<Replacement>, BattleError> {
        if self.active_has_super_effective_move()?
            && !rng.next_u16().is_multiple_of(ABSORBER_STAY_MODULUS)
        {
            return Ok(None);
        }
        let LastLandedMove::Move(move_id) = self.history.last_landed_move() else {
            return Ok(None);
        };
        let data = self.dex.move_data(move_id)?;
        if data.power == 0 {
            return Ok(None);
        }
        let Some(move_type) = data.move_type.battle_type() else {
            return Ok(None);
        };
        let Some(absorbing) = absorbing_ability(move_type) else {
            return Ok(None);
        };
        if self.active.ability() == absorbing {
            return Ok(None);
        }
        for (slot, candidate) in self.party.candidates() {
            if candidate.is_fainted() {
                continue;
            }
            if candidate.ability() == absorbing && (rng.next_u16() & 1) != 0 {
                return Ok(Some(Replacement::Slot(slot)));
            }
        }
        Ok(None)
    }

    /// `ShouldSwitchIfNaturalCure`. The `MOVE_UNAVAILABLE` else-if is refused
    /// before `move_data` once its opening coin fails.
    fn natural_cure(&self, rng: &mut impl BattleRng) -> Result<Option<Replacement>, BattleError> {
        if !self.active.status1().is_asleep()
            || self.active.ability() != AbilityId::NATURAL_CURE
            || self.active.current_hp() < self.active.stats().max_hp / 2
        {
            return Ok(None);
        }
        let last = self.history.last_landed_move();
        let unreadable = matches!(last, LastLandedMove::None | LastLandedMove::Unavailable);
        if unreadable && (rng.next_u16() & 1) != 0 {
            return Ok(Some(Replacement::Any));
        }
        let move_id = match last {
            LastLandedMove::Unavailable => {
                eprintln!(
                    "refusing trainer switch: Natural Cure would read gBattleMoves[MOVE_UNAVAILABLE], which the move table does not define"
                );
                return Err(BattleError::UndefinedNaturalCureMove);
            }
            LastLandedMove::None => MoveId::NONE,
            LastLandedMove::Move(move_id) => move_id,
        };
        if self.dex.move_data(move_id)?.power == 0 && (rng.next_u16() & 1) != 0 {
            return Ok(Some(Replacement::Any));
        }
        if let Some(slot) = self.find_replacement(rng, DOESNT_AFFECT, GUARANTEED_MODULUS)? {
            return Ok(Some(Replacement::Slot(slot)));
        }
        if let Some(slot) = self.find_replacement(rng, NOT_VERY_EFFECTIVE, GUARANTEED_MODULUS)? {
            return Ok(Some(Replacement::Slot(slot)));
        }
        if (rng.next_u16() & 1) != 0 {
            return Ok(Some(Replacement::Any));
        }
        Ok(None)
    }

    /// `HasSuperEffectiveMoveAgainstOpponents(FALSE)`: stay (do not switch)
    /// when a super-effective move rolls `Random() % 10 != 0`.
    fn keeps_own_super_effective_move(
        &self,
        rng: &mut impl BattleRng,
    ) -> Result<bool, BattleError> {
        for known in self.active.moves() {
            let flags = ai_move_result_flags(self.dex, known.move_id, self.player)?;
            if (flags & SUPER_EFFECTIVE) != 0 && !rng.next_u16().is_multiple_of(OWN_SUPER_MODULUS) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn active_has_super_effective_move(&self) -> Result<bool, BattleError> {
        for known in self.active.moves() {
            let flags = ai_move_result_flags(self.dex, known.move_id, self.player)?;
            if (flags & SUPER_EFFECTIVE) != 0 {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `AreStatsRaised`: the sum of positive stage offsets above 3 stays,
    /// with no draw. Neutral and lowered stages add nothing.
    fn stats_are_raised(&self) -> bool {
        let stages = self.active.stages();
        let mut total = 0u8;
        for offset in [
            stages.attack.offset(),
            stages.defense.offset(),
            stages.speed.offset(),
            stages.sp_attack.offset(),
            stages.sp_defense.offset(),
            stages.accuracy.offset(),
            stages.evasion.offset(),
        ] {
            if let Ok(raised) = u8::try_from(offset) {
                total += raised;
            }
        }
        total > RAISED_STAGE_LIMIT
    }

    /// `FindMonWithFlagsAndSuperEffective`. `None` and `Unavailable` return
    /// before the move table is read, as does a missing hitter. A zero-power
    /// move returns after that lookup. A matching bench move still draws when
    /// `modulus` is 1.
    fn find_replacement(
        &self,
        rng: &mut impl BattleRng,
        required: u8,
        modulus: u16,
    ) -> Result<Option<usize>, BattleError> {
        let LastLandedMove::Move(move_id) = self.history.last_landed_move() else {
            return Ok(None);
        };
        let Some(hitter_side) = self.history.last_hitter() else {
            return Ok(None);
        };
        if self.dex.move_data(move_id)?.power == 0 {
            return Ok(None);
        }
        let hitter = match hitter_side {
            BattlerSide::Player => self.player,
            BattlerSide::Enemy => self.active,
        };
        for (slot, candidate) in self.party.candidates() {
            if candidate.is_fainted() {
                continue;
            }
            let landed_flags = ai_move_result_flags(self.dex, move_id, candidate)?;
            if (landed_flags & required) == 0 {
                continue;
            }
            for known in candidate.moves() {
                let answer = ai_move_result_flags(self.dex, known.move_id, hitter)?;
                if (answer & SUPER_EFFECTIVE) != 0 && rng.next_u16().is_multiple_of(modulus) {
                    return Ok(Some(slot));
                }
            }
        }
        Ok(None)
    }

    fn resolve(&self, replacement: Replacement) -> Result<usize, BattleError> {
        match replacement {
            Replacement::Slot(slot) => Ok(slot),
            Replacement::Any => {
                let resolving_move = match self.history.last_landed_move() {
                    LastLandedMove::Move(move_id) => move_id,
                    LastLandedMove::None | LastLandedMove::Unavailable => self
                        .active
                        .moves()
                        .first()
                        .map_or(MoveId::NONE, |slot| slot.move_id),
                };
                self.party
                    .select_replacement(self.dex, self.active, resolving_move, self.player)?
                    .ok_or(BattleError::InvalidTrainerSwitch(self.party.active_slot()))
            }
        }
    }
}

fn absorbing_ability(move_type: Type) -> Option<AbilityId> {
    match move_type {
        Type::Fire => Some(AbilityId::FLASH_FIRE),
        Type::Water => Some(AbilityId::WATER_ABSORB),
        Type::Electric => Some(AbilityId::VOLT_ABSORB),
        Type::Normal
        | Type::Fighting
        | Type::Flying
        | Type::Poison
        | Type::Ground
        | Type::Rock
        | Type::Bug
        | Type::Ghost
        | Type::Steel
        | Type::Grass
        | Type::Psychic
        | Type::Ice
        | Type::Dragon
        | Type::Dark => None,
    }
}

/// `AI_TypeCalc` (`battle_script_commands.c:1594`): chart flags, then Wonder
/// Guard. Levitate on a Ground move skips the chart. Struggle sets nothing.
fn ai_move_result_flags(
    dex: &Dex,
    move_id: MoveId,
    defender: &BattlePokemon,
) -> Result<u8, BattleError> {
    if move_id == MoveId::STRUGGLE {
        return Ok(0);
    }
    let data = dex.move_data(move_id)?;
    let ability = defender.ability();
    let Some(move_type) = data.move_type.battle_type() else {
        return Ok(apply_wonder_guard(0, ability, data.power));
    };
    if ability == AbilityId::LEVITATE && move_type == Type::Ground {
        return Ok(MISSED | DOESNT_AFFECT);
    }
    let mut flags = 0;
    let defender_types = defender.types();
    let distinct = defender_types[1] != defender_types[0];
    for &(attacker, defender_type, effectiveness) in TypeChart::rows() {
        if attacker != move_type {
            continue;
        }
        if defender_type == defender_types[0] {
            modulate(&mut flags, data.power, effectiveness);
        }
        if distinct && defender_type == defender_types[1] {
            modulate(&mut flags, data.power, effectiveness);
        }
    }
    Ok(apply_wonder_guard(flags, ability, data.power))
}

fn apply_wonder_guard(mut flags: u8, ability: AbilityId, power: u8) -> u8 {
    let super_effective = (flags & SUPER_EFFECTIVE) != 0;
    let not_very = (flags & NOT_VERY_EFFECTIVE) != 0;
    if ability == AbilityId::WONDER_GUARD && power != 0 && (!super_effective || not_very) {
        flags |= DOESNT_AFFECT;
    }
    flags
}

fn modulate(flags: &mut u8, power: u8, effectiveness: Effectiveness) {
    match effectiveness {
        Effectiveness::NoEffect => {
            *flags |= DOESNT_AFFECT;
            *flags &= !SUPER_EFFECTIVE;
            *flags &= !NOT_VERY_EFFECTIVE;
        }
        Effectiveness::NotVeryEffective => {
            if power != 0 && (*flags & NO_EFFECT) == 0 {
                if (*flags & SUPER_EFFECTIVE) != 0 {
                    *flags &= !SUPER_EFFECTIVE;
                } else {
                    *flags |= NOT_VERY_EFFECTIVE;
                }
            }
        }
        Effectiveness::SuperEffective => {
            if power != 0 && (*flags & NO_EFFECT) == 0 {
                if (*flags & NOT_VERY_EFFECTIVE) != 0 {
                    *flags &= !NOT_VERY_EFFECTIVE;
                } else {
                    *flags |= SUPER_EFFECTIVE;
                }
            }
        }
        Effectiveness::Normal => {}
    }
}

#[cfg(test)]
mod tests;
