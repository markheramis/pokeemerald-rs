//! A badly poisoned battler: the end-of-turn residual between poison and burn,
//! the counter's increment and cap, and Shed Skin's cure. `Status1::BadlyPoisoned`
//! is `STATUS1_TOXIC_POISON` plus `STATUS1_TOXIC_COUNTER`
//! (`pokeemerald/include/constants/battle.h:120-123`).

use crate::common::{max_iv_mon, SequenceRng};
use assets::{AbilityId, MoveId};
use battle::status1::bad_poison_tick;
use battle::{Battle, BattleEvent, Dex, PlayerAction, Status1};

const TACKLE: MoveId = MoveId::TACKLE;

/// `SPECIES_RATTATA`: faster than [`ZIGZAGOON`] at equal levels.
const RATTATA: u16 = 19;
/// `SPECIES_ZIGZAGOON`: a Normal-type defender with no damage-reading ability.
const ZIGZAGOON: u16 = 288;
/// `SPECIES_SEVIPER`: Shed Skin in its only ability slot.
const SEVIPER: u16 = 379;
const LEVEL: u8 = 20;

/// Draws for a turn where both battlers use Tackle. A bad-poison tick draws
/// nothing, so the script is exact without a residual value.
const BOTH_BATTLERS_ATTACK: [u16; 11] = [0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0];

fn mon(species: u16, status: Status1) -> battle::BattlePokemon {
    let mut built = max_iv_mon(&Dex::new(), species, LEVEL, vec![TACKLE]);
    built.set_status1(status);
    built
}

fn take_tackle_turn(
    player: battle::BattlePokemon,
    enemy: battle::BattlePokemon,
) -> (Battle, Vec<BattleEvent>) {
    let mut rng = SequenceRng::new(BOTH_BATTLERS_ATTACK);
    let mut battle = Battle::new(Dex::new(), player, enemy, false, &mut rng).unwrap();
    let events = battle
        .take_turn(PlayerAction::UseMove(0), &mut rng)
        .unwrap();
    rng.assert_exhausted();
    (battle, events)
}

#[test]
fn a_fresh_toxic_counter_deals_one_sixteenth_and_becomes_one() {
    let player = mon(RATTATA, Status1::BadlyPoisoned { counter: 0 });
    let max_hp = player.stats().max_hp;
    let (damage, next) = bad_poison_tick(0, max_hp);

    let (battle, events) = take_tackle_turn(player, mon(ZIGZAGOON, Status1::Healthy));

    assert_eq!(damage, max_hp / 16);
    assert_eq!(next, 1);
    assert!(
        events.contains(&BattleEvent::HurtByBadPoison {
            by_player: true,
            damage,
        }),
        "{events:?}"
    );
    assert!(!events.iter().any(|event| {
        matches!(
            event,
            BattleEvent::HurtByPoison { .. } | BattleEvent::HurtByBurn { .. }
        )
    }));
    assert_eq!(
        battle.player().status1(),
        Status1::BadlyPoisoned { counter: 1 }
    );
}

#[test]
fn the_counter_caps_at_fifteen_and_keeps_dealing_that_multiple() {
    let dex = Dex::new();
    // Splash deals nothing, so the tick is not capped by a Tackle's chip
    // and a knockout cannot clear the status before the assertion.
    let mut player = max_iv_mon(&dex, RATTATA, LEVEL, vec![MoveId::SPLASH]);
    player.set_status1(Status1::BadlyPoisoned { counter: 15 });
    let enemy = max_iv_mon(&dex, ZIGZAGOON, LEVEL, vec![MoveId::SPLASH]);
    let max_hp = player.stats().max_hp;
    let (damage, next) = bad_poison_tick(15, max_hp);

    // Battle start, the turn-number seed, and the enemy's one-move pick.
    // Splash draws nothing, and the toxic tick draws nothing.
    let mut rng = SequenceRng::new([0, 0, 0]);
    let mut battle = Battle::new(dex, player, enemy, false, &mut rng).unwrap();
    let events = battle
        .take_turn(PlayerAction::UseMove(0), &mut rng)
        .unwrap();
    rng.assert_exhausted();

    assert_eq!(next, 15);
    assert_eq!(damage, (max_hp / 16) * 15);
    assert!(damage < max_hp, "fifteen sixteenths cannot itself faint");
    assert!(
        events.contains(&BattleEvent::HurtByBadPoison {
            by_player: true,
            damage,
        }),
        "{events:?}"
    );
    assert_eq!(
        battle.player().status1(),
        Status1::BadlyPoisoned { counter: 15 }
    );
    assert_eq!(battle.player().current_hp(), max_hp - damage);
}

#[test]
fn bad_poison_lands_after_ordinary_poison_and_before_burn() {
    let player = mon(RATTATA, Status1::Poisoned);
    let enemy = mon(ZIGZAGOON, Status1::BadlyPoisoned { counter: 2 });
    let (_, poisoned_then_toxic) = take_tackle_turn(player, enemy);

    let poison_at = poisoned_then_toxic
        .iter()
        .position(|event| {
            matches!(
                event,
                BattleEvent::HurtByPoison {
                    by_player: true,
                    ..
                }
            )
        })
        .expect("the faster poisoned battler ticks");
    let toxic_at = poisoned_then_toxic
        .iter()
        .position(|event| {
            matches!(
                event,
                BattleEvent::HurtByBadPoison {
                    by_player: false,
                    ..
                }
            )
        })
        .expect("the slower toxic battler ticks");
    assert!(
        poison_at < toxic_at,
        "poison precedes bad poison across the turn order: {poisoned_then_toxic:?}"
    );

    let player = mon(RATTATA, Status1::BadlyPoisoned { counter: 1 });
    let enemy = mon(ZIGZAGOON, Status1::Burned);
    let (_, toxic_then_burn) = take_tackle_turn(player, enemy);
    let toxic_at = toxic_then_burn
        .iter()
        .position(|event| {
            matches!(
                event,
                BattleEvent::HurtByBadPoison {
                    by_player: true,
                    ..
                }
            )
        })
        .expect("the faster toxic battler ticks");
    let burn_at = toxic_then_burn
        .iter()
        .position(|event| {
            matches!(
                event,
                BattleEvent::HurtByBurn {
                    by_player: false,
                    ..
                }
            )
        })
        .expect("the slower burned battler ticks");
    assert!(
        toxic_at < burn_at,
        "bad poison precedes burn: {toxic_then_burn:?}"
    );
}

#[test]
fn shed_skin_cures_bad_poison_before_the_tick_and_draws_once() {
    let dex = Dex::new();
    let mut player = max_iv_mon(&dex, SEVIPER, 10, vec![TACKLE]);
    assert_eq!(player.ability(), AbilityId::SHED_SKIN);
    player.set_status1(Status1::BadlyPoisoned { counter: 3 });
    let enemy = max_iv_mon(&dex, ZIGZAGOON, 10, vec![TACKLE]);

    let mut rng = SequenceRng::new(
        BOTH_BATTLERS_ATTACK
            .into_iter()
            .chain([0])
            .collect::<Vec<_>>(),
    );
    let mut battle = Battle::new(dex, player, enemy, false, &mut rng).unwrap();
    let events = battle
        .take_turn(PlayerAction::UseMove(0), &mut rng)
        .unwrap();
    rng.assert_exhausted();

    assert!(
        events.contains(&BattleEvent::ShedSkinCured {
            by_player: true,
            status: Status1::BadlyPoisoned { counter: 3 },
        }),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, BattleEvent::HurtByBadPoison { .. })),
        "the cure precedes ENDTURN_BAD_POISON: {events:?}"
    );
    assert_eq!(battle.player().status1(), Status1::Healthy);
}
