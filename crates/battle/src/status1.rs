//! Primary status ([`Status1`]), the full-paralysis gate, poison's, bad
//! poison's and burn's end-turn damage, and Shed Skin's end-turn cure draw.
//!
//! `status1` persists across a switch, unlike the volatile bits in
//! [`crate::volatile::Volatiles`]. Confusion belongs to those volatile bits
//! ([`crate::volatile::Volatiles::confusion_turns`]), not to `Status1`.
//!
//! Upstream's `status1` is a bitfield with one flag per status
//! (`pokeemerald/include/constants/battle.h:112`-`:125`), but every
//! infliction path this crate models refuses a nonzero field
//! (`pokeemerald/src/battle_script_commands.c:2334`-`:2335`), so an enum with
//! one variant per status loses nothing. Sleep keeps its remaining turn count
//! in the low three bits of that field (`STATUS1_SLEEP`,
//! `pokeemerald/include/constants/battle.h:112`), which [`Status1::Asleep`]
//! carries as `turns`.

use assets::AbilityId;

use crate::damage::BattleRng;

/// One battler's primary status condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Status1 {
    /// No primary status.
    #[default]
    Healthy,
    /// Paralysed: [`draws_full_paralysis`] may cancel this battler's chosen
    /// move before it acts (`pokeemerald/src/battle_util.c:2188`-`:2199`),
    /// and turn order quarters its effective Speed
    /// (`pokeemerald/src/battle_main.c:4650`-`:4651`).
    Paralysed,
    /// Poisoned: [`poison_residual_damage`] fires every end of turn
    /// (`ENDTURN_POISON`, `pokeemerald/src/battle_util.c:1525`-`:1535`).
    Poisoned,
    /// Burned: [`burn_residual_damage`] fires every end of turn
    /// (`ENDTURN_BURN`, `pokeemerald/src/battle_util.c:1550`-`:1560`), and
    /// the attacker's physical damage is halved unless it has Guts
    /// ([`crate::ability::burn_halves_damage`]).
    Burned,
    /// Badly poisoned: [`bad_poison_tick`] fires every end of turn
    /// (`ENDTURN_BAD_POISON`, `pokeemerald/src/battle_util.c:1536`-`:1546`).
    /// `counter` is `STATUS1_TOXIC_COUNTER` (bits 8–11, `0..=15`). The
    /// residual advances it on the battler only; the party word is unchanged
    /// until a whole-status emit, and a switch reloads that word.
    BadlyPoisoned {
        /// Toxic turns already stacked, before this residual's increment.
        counter: u8,
    },
    /// Asleep with `turns` actions left before waking: the canceller
    /// ([`tick_sleep`]) decrements it ahead of every chosen move
    /// (`CANCELER_ASLEEP`, `pokeemerald/src/battle_util.c:2015`-`:2057`).
    /// Every value [`draw_sleep_duration`] returns is in `2..=5`. Zero
    /// remaining turns is upstream's cleared sleep bits, a healthy battler:
    /// [`Status1::is_asleep`] is false for it, [`tick_sleep`] ignores it, and
    /// `BattlePokemon::set_status1` stores it as [`Status1::Healthy`].
    Asleep {
        /// Remaining sleep turns; zero reads as not asleep.
        turns: u8,
    },
}

impl Status1 {
    /// Whether this status is [`Status1::Paralysed`].
    #[must_use]
    pub const fn is_paralysed(self) -> bool {
        matches!(self, Self::Paralysed)
    }

    /// Whether this status is [`Status1::Poisoned`].
    #[must_use]
    pub const fn is_poisoned(self) -> bool {
        matches!(self, Self::Poisoned)
    }

    /// Whether this status is [`Status1::Burned`].
    #[must_use]
    pub const fn is_burned(self) -> bool {
        matches!(self, Self::Burned)
    }

    /// Whether this status is [`Status1::BadlyPoisoned`].
    #[must_use]
    pub const fn is_badly_poisoned(self) -> bool {
        matches!(self, Self::BadlyPoisoned { .. })
    }

    /// The toxic counter when this is [`Status1::BadlyPoisoned`].
    #[must_use]
    pub const fn toxic_counter(self) -> Option<u8> {
        match self {
            Self::BadlyPoisoned { counter } => Some(counter),
            _ => None,
        }
    }

    /// Ordinary or bad poison: `STATUS1_PSN_ANY`
    /// (`pokeemerald/include/constants/battle.h:124`).
    #[must_use]
    pub const fn is_psn_any(self) -> bool {
        self.is_poisoned() || self.is_badly_poisoned()
    }

    /// Whether this status is [`Status1::Asleep`] with at least one turn left;
    /// zero turns are upstream's cleared sleep bits, a healthy battler.
    #[must_use]
    pub const fn is_asleep(self) -> bool {
        matches!(self, Self::Asleep { turns } if turns != 0)
    }

    /// Maps the unreachable `Asleep { turns: 0 }` to [`Status1::Healthy`],
    /// upstream's cleared sleep bits; every other value is unchanged.
    #[must_use]
    pub const fn normalised(self) -> Self {
        if let Self::Asleep { turns: 0 } = self {
            Self::Healthy
        } else {
            self
        }
    }

    /// Whether no primary status is set, the guard every infliction path this
    /// crate models shares (`pokeemerald/src/battle_script_commands.c:2334`-`:2335`).
    #[must_use]
    pub const fn is_healthy(self) -> bool {
        matches!(self.normalised(), Self::Healthy)
    }
}

/// What [`tick_sleep`] decided for one action of the sleeping battler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SleepTick {
    /// The battler was not asleep; nothing changed.
    NotAsleep,
    /// The battler stays asleep with the carried status and its chosen move
    /// is cancelled (`BattleScript_MoveUsedIsAsleep`).
    StillAsleep(Status1),
    /// The duration ran out: the battler is [`Status1::Healthy`] and its
    /// chosen move proceeds (`BattleScript_MoveUsedWokeUp`).
    WokeUp,
}

/// `CANCELER_ASLEEP`'s decrement: one turn, or two for an Early Bird holder
/// (`pokeemerald/src/battle_util.c:2029`-`:2043`). A remaining count below the
/// decrement clears the sleep bits outright rather than underflowing.
///
/// Draws nothing. Uproar's wake-up (`:2017`-`:2025`) and the Snore and Sleep
/// Talk exemption (`:2038`) are not modelled.
#[must_use]
pub const fn tick_sleep(status1: Status1, ability: AbilityId) -> SleepTick {
    let Status1::Asleep { turns } = status1.normalised() else {
        return SleepTick::NotAsleep;
    };
    let decrement = if ability.0 == AbilityId::EARLY_BIRD.0 {
        2
    } else {
        1
    };
    if turns <= decrement {
        SleepTick::WokeUp
    } else {
        SleepTick::StillAsleep(Status1::Asleep {
            turns: turns - decrement,
        })
    }
}

/// Smallest sleep duration `SetMoveEffect` writes: `(Random() & 3) + 2`
/// (`pokeemerald/src/battle_script_commands.c:2487`).
pub const SLEEP_TURNS_MIN: u8 = 2;
/// Largest sleep duration: `(3) + 2`.
pub const SLEEP_TURNS_MAX: u8 = 5;

/// Draws a sleep duration of two to five turns with one `Random()` call
/// (`pokeemerald/src/battle_script_commands.c:2487`).
#[must_use]
pub fn draw_sleep_duration(rng: &mut impl BattleRng) -> u8 {
    // `& 3` leaves at most three, so the narrowing cannot truncate.
    (rng.next_u16() & 3) as u8 + SLEEP_TURNS_MIN
}

/// Applies `STATUS1_SLEEP` to a target in `SetMoveEffect`
/// (`pokeemerald/src/battle_script_commands.c:2273`-`:2298`, `:2480`-`:2487`),
/// returning the target's new status, or `None` when upstream's guards
/// refuse it: any existing primary status, Vital Spirit, or Insomnia.
///
/// The duration draw happens only once every guard has passed. The active
/// Uproar scan (`:2275`-`:2289`) is not modelled.
#[must_use]
pub fn try_inflict_sleep(
    current: Status1,
    target_ability: AbilityId,
    rng: &mut impl BattleRng,
) -> Option<Status1> {
    if !current.is_healthy()
        || target_ability == AbilityId::VITAL_SPIRIT
        || target_ability == AbilityId::INSOMNIA
    {
        return None;
    }
    Some(Status1::Asleep {
        turns: draw_sleep_duration(rng),
    })
}

/// Full paralysis is `Random() % 4 == 0` (`pokeemerald/src/battle_util.c:2189`).
const FULL_PARALYSIS_CHANCE_DENOMINATOR: u16 = 4;

/// Draws whether a paralysed battler is fully unable to act this turn —
/// `CANCELER_PARALYZED` (`pokeemerald/src/battle_util.c:2188`-`:2199`).
///
/// Draws nothing, and returns `false`, for a battler that is not paralysed:
/// upstream's `&&` short-circuits before its own `Random()` call.
#[must_use]
pub fn draws_full_paralysis(status1: Status1, rng: &mut impl BattleRng) -> bool {
    status1.is_paralysed()
        && rng
            .next_u16()
            .is_multiple_of(FULL_PARALYSIS_CHANCE_DENOMINATOR)
}

/// `ENDTURN_POISON` deals max HP / 8 (`pokeemerald/src/battle_util.c:1528`).
const POISON_DAMAGE_DENOMINATOR: u32 = 8;

/// `ENDTURN_POISON`'s damage for a battler with `max_hp`: an eighth of it,
/// never less than one, with no RNG draw
/// (`pokeemerald/src/battle_util.c:1528-1530`).
#[must_use]
pub const fn poison_residual_damage(max_hp: u32) -> u32 {
    let damage = max_hp / POISON_DAMAGE_DENOMINATOR;
    if damage == 0 {
        1
    } else {
        damage
    }
}

/// Largest `STATUS1_TOXIC_COUNTER` (`STATUS1_TOXIC_TURN(15)`).
pub const TOXIC_COUNTER_MAX: u8 = 15;

/// `ENDTURN_BAD_POISON`'s base is max HP / 16, never less than one. The
/// counter increments first unless it is already [`TOXIC_COUNTER_MAX`], then
/// the base is multiplied by that counter. No RNG draw
/// (`pokeemerald/src/battle_util.c:1536`-`:1546`).
#[must_use]
pub const fn bad_poison_tick(counter: u8, max_hp: u32) -> (u32, u8) {
    let next = if counter >= TOXIC_COUNTER_MAX {
        TOXIC_COUNTER_MAX
    } else {
        counter + 1
    };
    let base = max_hp / BAD_POISON_DAMAGE_DENOMINATOR;
    let base = if base == 0 { 1 } else { base };
    (base * next as u32, next)
}

/// `ENDTURN_BAD_POISON` divides max HP by 16 (`pokeemerald/src/battle_util.c:1538`).
const BAD_POISON_DAMAGE_DENOMINATOR: u32 = 16;

/// `ENDTURN_BURN` deals max HP / 8 (`pokeemerald/src/battle_util.c:1553`).
const BURN_DAMAGE_DENOMINATOR: u32 = 8;

/// `ENDTURN_BURN`'s damage for a battler with `max_hp`: an eighth of it,
/// never less than one, with no RNG draw
/// (`pokeemerald/src/battle_util.c:1553-1555`).
#[must_use]
pub const fn burn_residual_damage(max_hp: u32) -> u32 {
    let damage = max_hp / BURN_DAMAGE_DENOMINATOR;
    if damage == 0 {
        1
    } else {
        damage
    }
}

/// Shed Skin cures on `Random() % 3 == 0` (`pokeemerald/src/battle_util.c:2621`).
const SHED_SKIN_CURE_CHANCE_DENOMINATOR: u16 = 3;

/// Draws whether a Shed Skin holder cures its primary status this residual
/// pass — `ABILITY_SHED_SKIN`'s `ABILITYEFFECT_ENDTURN` case
/// (`pokeemerald/src/battle_util.c:2620`-`:2621`).
///
/// Draws nothing, and returns `false`, for a healthy battler: upstream's
/// `&&` short-circuits before its own `Random()` call. The caller owns the
/// `hp != 0` guard (`pokeemerald/src/battle_util.c:2601`-`:2602`) and the
/// ability check.
#[must_use]
pub fn draws_shed_skin_cure(status1: Status1, rng: &mut impl BattleRng) -> bool {
    !status1.is_healthy()
        && rng
            .next_u16()
            .is_multiple_of(SHED_SKIN_CURE_CHANCE_DENOMINATOR)
}

#[cfg(test)]
mod tests {
    use super::{
        bad_poison_tick, burn_residual_damage, draw_sleep_duration, draws_full_paralysis,
        draws_shed_skin_cure, poison_residual_damage, tick_sleep, try_inflict_sleep, SleepTick,
        Status1, TOXIC_COUNTER_MAX,
    };
    use crate::damage::BattleRng;
    use assets::AbilityId;

    struct FixedRng(u16);
    impl BattleRng for FixedRng {
        fn next_u16(&mut self) -> u16 {
            self.0
        }
    }

    struct CountingRng {
        value: u16,
        draws: u32,
    }
    impl BattleRng for CountingRng {
        fn next_u16(&mut self) -> u16 {
            self.draws += 1;
            self.value
        }
    }

    #[test]
    fn a_healthy_battler_draws_nothing_and_is_never_fully_paralysed() {
        let mut rng = CountingRng { value: 0, draws: 0 };
        assert!(!draws_full_paralysis(Status1::Healthy, &mut rng));
        assert_eq!(rng.draws, 0, "the healthy case must not touch the RNG");
    }

    #[test]
    fn a_paralysed_battler_draws_exactly_once() {
        let mut rng = CountingRng { value: 1, draws: 0 };
        let _ = draws_full_paralysis(Status1::Paralysed, &mut rng);
        assert_eq!(rng.draws, 1);
    }

    #[test]
    fn one_in_four_values_trigger_full_paralysis() {
        assert!(draws_full_paralysis(Status1::Paralysed, &mut FixedRng(0)));
        assert!(draws_full_paralysis(Status1::Paralysed, &mut FixedRng(4)));
        assert!(!draws_full_paralysis(Status1::Paralysed, &mut FixedRng(1)));
        assert!(!draws_full_paralysis(Status1::Paralysed, &mut FixedRng(2)));
        assert!(!draws_full_paralysis(Status1::Paralysed, &mut FixedRng(3)));
    }

    #[test]
    fn status1_defaults_to_healthy() {
        assert_eq!(Status1::default(), Status1::Healthy);
        assert!(!Status1::default().is_paralysed());
        assert!(Status1::Paralysed.is_paralysed());
    }

    #[test]
    fn each_variant_answers_exactly_its_own_predicates() {
        for (status, healthy, paralysed, poisoned, burned, badly) in [
            (Status1::Healthy, true, false, false, false, false),
            (Status1::Paralysed, false, true, false, false, false),
            (Status1::Poisoned, false, false, true, false, false),
            (Status1::Burned, false, false, false, true, false),
            (
                Status1::Asleep { turns: 3 },
                false,
                false,
                false,
                false,
                false,
            ),
            (
                Status1::BadlyPoisoned { counter: 0 },
                false,
                false,
                false,
                false,
                true,
            ),
            (
                Status1::BadlyPoisoned { counter: 4 },
                false,
                false,
                false,
                false,
                true,
            ),
        ] {
            assert_eq!(status.is_healthy(), healthy, "{status:?}");
            assert_eq!(status.is_paralysed(), paralysed, "{status:?}");
            assert_eq!(status.is_poisoned(), poisoned, "{status:?}");
            assert_eq!(status.is_burned(), burned, "{status:?}");
            assert_eq!(status.is_badly_poisoned(), badly, "{status:?}");
            assert_eq!(status.is_psn_any(), poisoned || badly, "{status:?}");
            assert_eq!(
                status.toxic_counter(),
                match status {
                    Status1::BadlyPoisoned { counter } => Some(counter),
                    _ => None,
                },
                "{status:?}"
            );
            assert_eq!(
                status.is_asleep(),
                matches!(status, Status1::Asleep { .. }),
                "{status:?}"
            );
        }
    }

    #[test]
    fn sleep_duration_is_the_low_two_bits_plus_two() {
        for (raw, expected) in [(0, 2), (1, 3), (2, 4), (3, 5), (4, 2), (0xFFFF, 5)] {
            assert_eq!(draw_sleep_duration(&mut FixedRng(raw)), expected, "{raw}");
        }
    }

    #[test]
    fn a_sleep_duration_draws_exactly_once() {
        let mut rng = CountingRng { value: 2, draws: 0 };
        let _ = draw_sleep_duration(&mut rng);
        assert_eq!(rng.draws, 1);
    }

    #[test]
    fn a_normal_sleeper_loses_one_turn_and_wakes_when_none_remain() {
        let ability = AbilityId::NONE;
        assert_eq!(
            tick_sleep(Status1::Asleep { turns: 3 }, ability),
            SleepTick::StillAsleep(Status1::Asleep { turns: 2 })
        );
        assert_eq!(
            tick_sleep(Status1::Asleep { turns: 2 }, ability),
            SleepTick::StillAsleep(Status1::Asleep { turns: 1 })
        );
        assert_eq!(
            tick_sleep(Status1::Asleep { turns: 1 }, ability),
            SleepTick::WokeUp
        );
    }

    #[test]
    fn early_bird_loses_two_turns_and_clears_rather_than_underflowing() {
        let ability = AbilityId::EARLY_BIRD;
        assert_eq!(
            tick_sleep(Status1::Asleep { turns: 5 }, ability),
            SleepTick::StillAsleep(Status1::Asleep { turns: 3 })
        );
        assert_eq!(
            tick_sleep(Status1::Asleep { turns: 3 }, ability),
            SleepTick::StillAsleep(Status1::Asleep { turns: 1 })
        );
        assert_eq!(
            tick_sleep(Status1::Asleep { turns: 2 }, ability),
            SleepTick::WokeUp
        );
        assert_eq!(
            tick_sleep(Status1::Asleep { turns: 1 }, ability),
            SleepTick::WokeUp,
            "one turn left is below the two-turn decrement"
        );
    }

    #[test]
    fn a_battler_that_is_not_asleep_does_not_tick() {
        for status in [
            Status1::Healthy,
            Status1::Paralysed,
            Status1::Poisoned,
            Status1::Burned,
            Status1::BadlyPoisoned { counter: 2 },
        ] {
            assert_eq!(
                tick_sleep(status, AbilityId::EARLY_BIRD),
                SleepTick::NotAsleep
            );
        }
    }

    #[test]
    fn sleep_lands_on_a_healthy_target_with_one_duration_draw() {
        let mut rng = CountingRng { value: 1, draws: 0 };
        assert_eq!(
            try_inflict_sleep(Status1::Healthy, AbilityId::NONE, &mut rng),
            Some(Status1::Asleep { turns: 3 })
        );
        assert_eq!(rng.draws, 1);
    }

    #[test]
    fn sleep_is_refused_without_a_draw_by_status_or_sleep_preventing_ability() {
        for (status, ability) in [
            (Status1::Paralysed, AbilityId::NONE),
            (Status1::Poisoned, AbilityId::NONE),
            (Status1::Burned, AbilityId::NONE),
            (Status1::BadlyPoisoned { counter: 1 }, AbilityId::NONE),
            (Status1::Asleep { turns: 2 }, AbilityId::NONE),
            (Status1::Healthy, AbilityId::VITAL_SPIRIT),
            (Status1::Healthy, AbilityId::INSOMNIA),
        ] {
            let mut rng = CountingRng { value: 0, draws: 0 };
            assert_eq!(try_inflict_sleep(status, ability, &mut rng), None);
            assert_eq!(rng.draws, 0, "{status:?} {ability:?}");
        }
    }

    #[test]
    fn poisoned_draws_nothing_for_full_paralysis() {
        let mut rng = CountingRng { value: 0, draws: 0 };
        assert!(!draws_full_paralysis(Status1::Poisoned, &mut rng));
        assert_eq!(
            rng.draws, 0,
            "only Status1::Paralysed drives the full-paralysis draw"
        );
    }

    #[test]
    fn a_healthy_battler_draws_nothing_for_the_shed_skin_cure() {
        let mut rng = CountingRng { value: 0, draws: 0 };
        assert!(!draws_shed_skin_cure(Status1::Healthy, &mut rng));
        assert_eq!(rng.draws, 0, "the healthy case must not touch the RNG");
    }

    #[test]
    fn a_statused_battler_draws_exactly_once_for_the_shed_skin_cure() {
        let mut rng = CountingRng { value: 1, draws: 0 };
        let _ = draws_shed_skin_cure(Status1::Paralysed, &mut rng);
        assert_eq!(rng.draws, 1);
    }

    #[test]
    fn one_in_three_values_cure_a_statused_shed_skin_holder() {
        for status in [
            Status1::Paralysed,
            Status1::Poisoned,
            Status1::Burned,
            Status1::BadlyPoisoned { counter: 7 },
        ] {
            assert!(draws_shed_skin_cure(status, &mut FixedRng(0)), "{status:?}");
            assert!(draws_shed_skin_cure(status, &mut FixedRng(3)), "{status:?}");
            assert!(
                !draws_shed_skin_cure(status, &mut FixedRng(1)),
                "{status:?}"
            );
            assert!(
                !draws_shed_skin_cure(status, &mut FixedRng(2)),
                "{status:?}"
            );
        }
    }

    #[test]
    fn bad_poison_ticks_the_counter_before_multiplying_and_caps_at_fifteen() {
        assert_eq!(bad_poison_tick(0, 160), (10, 1));
        assert_eq!(bad_poison_tick(1, 160), (20, 2));
        assert_eq!(bad_poison_tick(14, 160), (150, 15));
        assert_eq!(
            bad_poison_tick(15, 160),
            (150, 15),
            "fifteen does not become sixteen"
        );
        assert_eq!(bad_poison_tick(0, 15), (1, 1), "a zero base becomes one");
        assert_eq!(bad_poison_tick(4, 15), (5, 5));
        assert_eq!(TOXIC_COUNTER_MAX, 15);
    }

    #[test]
    fn poison_residual_damage_is_an_eighth_of_max_hp_floored_to_one() {
        assert_eq!(poison_residual_damage(80), 10);
        assert_eq!(poison_residual_damage(79), 9, "integer division truncates");
        assert_eq!(poison_residual_damage(8), 1);
        assert_eq!(
            poison_residual_damage(7),
            1,
            "a fraction below one HP is floored up to one, not zero"
        );
        assert_eq!(poison_residual_damage(1), 1);
        assert_eq!(
            poison_residual_damage(0),
            1,
            "even a zero max HP still floors to one (`battle_util.c:1529-1530`)"
        );
    }

    #[test]
    fn burn_residual_damage_is_an_eighth_of_max_hp_floored_to_one() {
        assert_eq!(burn_residual_damage(80), 10);
        assert_eq!(burn_residual_damage(79), 9, "integer division truncates");
        assert_eq!(burn_residual_damage(8), 1);
        assert_eq!(
            burn_residual_damage(7),
            1,
            "a fraction below one HP is floored up to one, not zero"
        );
        assert_eq!(burn_residual_damage(0), 1);
    }

    #[test]
    fn a_burned_battler_draws_nothing_for_full_paralysis() {
        let mut rng = CountingRng { value: 0, draws: 0 };
        assert!(!draws_full_paralysis(Status1::Burned, &mut rng));
        assert_eq!(rng.draws, 0);
    }
}
