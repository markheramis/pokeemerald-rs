//! Both constructors refuse move-end-cured status/ability pairs
//! (`battle_util.c:2873-2951`) and always-refused abilities (Truant, Intimidate, Speed Boost,
//! Color Change, Trace, Flame Body, Cute Charm, Drizzle, Drought, Sand Stream), before any RNG draw.

use crate::common::{admission_script, max_iv_mon, SequenceRng};
use assets::trainers::TrainerId;
use assets::{AbilityId, MoveId, SpeciesId};
use battle::{Battle, BattleError, BattlePokemon, Dex, Status1};

const MAY_ROUTE_103_MUDKIP: TrainerId = TrainerId(529);

fn mon(species: SpeciesId, slot: u8, status: Status1) -> BattlePokemon {
    let dex = Dex::new();
    let mut mon = max_iv_mon(&dex, species.0, 20, vec![MoveId::TACKLE]).with_ability_slot(slot);
    mon.set_status1(status);
    mon
}

fn healthy(species: SpeciesId) -> BattlePokemon {
    mon(species, 0, Status1::Healthy)
}

fn refused() -> [(BattlePokemon, AbilityId); 27] {
    [
        (healthy(SpeciesId::MIGHTYENA), AbilityId::INTIMIDATE),
        (healthy(SpeciesId::EKANS), AbilityId::INTIMIDATE),
        (
            mon(SpeciesId::MAWILE, 1, Status1::Healthy),
            AbilityId::INTIMIDATE,
        ),
        (healthy(SpeciesId::SLAKOTH), AbilityId::TRUANT),
        (healthy(SpeciesId::SLAKING), AbilityId::TRUANT),
        (healthy(SpeciesId::NINJASK), AbilityId::SPEED_BOOST),
        (healthy(SpeciesId::YANMA), AbilityId::SPEED_BOOST),
        (healthy(SpeciesId::KECLEON), AbilityId::COLOR_CHANGE),
        (healthy(SpeciesId::PORYGON), AbilityId::TRACE),
        (healthy(SpeciesId::PORYGON2), AbilityId::TRACE),
        (mon(SpeciesId::RALTS, 1, Status1::Healthy), AbilityId::TRACE),
        (
            mon(SpeciesId::KIRLIA, 1, Status1::Healthy),
            AbilityId::TRACE,
        ),
        (
            mon(SpeciesId::GARDEVOIR, 1, Status1::Healthy),
            AbilityId::TRACE,
        ),
        (healthy(SpeciesId::MAGMAR), AbilityId::FLAME_BODY),
        (
            mon(SpeciesId::SLUGMA, 1, Status1::Healthy),
            AbilityId::FLAME_BODY,
        ),
        (healthy(SpeciesId::CLEFAIRY), AbilityId::CUTE_CHARM),
        (healthy(SpeciesId::SKITTY), AbilityId::CUTE_CHARM),
        (healthy(SpeciesId::KYOGRE), AbilityId::DRIZZLE),
        (healthy(SpeciesId::GROUDON), AbilityId::DROUGHT),
        (healthy(SpeciesId::TYRANITAR), AbilityId::SAND_STREAM),
        (
            mon(SpeciesId::PERSIAN, 0, Status1::Paralysed),
            AbilityId::LIMBER,
        ),
        (
            mon(SpeciesId::SNORLAX, 0, Status1::Poisoned),
            AbilityId::IMMUNITY,
        ),
        (
            mon(SpeciesId::SNORLAX, 0, Status1::BadlyPoisoned { counter: 4 }),
            AbilityId::IMMUNITY,
        ),
        (
            mon(SpeciesId::GOLDEEN, 1, Status1::Burned),
            AbilityId::WATER_VEIL,
        ),
        (
            mon(SpeciesId::HOOTHOOT, 0, Status1::Asleep { turns: 3 }),
            AbilityId::INSOMNIA,
        ),
        (
            mon(SpeciesId::MANKEY, 0, Status1::Asleep { turns: 3 }),
            AbilityId::VITAL_SPIRIT,
        ),
        {
            let mut spinda = healthy(SpeciesId::SPINDA);
            assert_eq!(spinda.ability(), AbilityId::OWN_TEMPO);
            spinda.volatiles_mut().set_confusion(3);
            (spinda, AbilityId::OWN_TEMPO)
        },
    ]
}

fn assert_refused_without_draws(
    result: &Result<Battle, BattleError>,
    ability: AbilityId,
    rng: &SequenceRng,
) {
    assert!(matches!(
        result,
        Err(BattleError::UnportedAbilityInteraction(found)) if *found == ability
    ));
    assert_eq!(rng.draws(), 0, "refusal precedes every constructor draw");
}

fn wild(
    player: BattlePokemon,
    reserves: Vec<BattlePokemon>,
    enemy: BattlePokemon,
    first_battle: bool,
    rng: &mut SequenceRng,
) -> Result<Battle, BattleError> {
    Battle::new_with_player_reserves(Dex::new(), player, reserves, enemy, first_battle, rng)
}

fn trainer(
    player: BattlePokemon,
    reserves: Vec<BattlePokemon>,
    party: Vec<BattlePokemon>,
    rng: &mut SequenceRng,
) -> Result<Battle, BattleError> {
    Battle::new_trainer_with_player_reserves(
        Dex::new(),
        player,
        reserves,
        MAY_ROUTE_103_MUDKIP,
        party,
        rng,
    )
}

#[test]
fn wild_constructor_refuses_an_active_player_before_drawing() {
    for first_battle in [false, true] {
        for (bad, ability) in refused() {
            let mut rng = SequenceRng::new([]);
            let result = wild(
                bad,
                vec![],
                healthy(SpeciesId::ZIGZAGOON),
                first_battle,
                &mut rng,
            );
            assert_refused_without_draws(&result, ability, &rng);
        }
    }
}

#[test]
fn wild_constructor_refuses_an_active_enemy_before_drawing() {
    for (bad, ability) in refused() {
        let mut rng = SequenceRng::new([]);
        let result = wild(healthy(SpeciesId::ZIGZAGOON), vec![], bad, false, &mut rng);
        assert_refused_without_draws(&result, ability, &rng);
    }
}

#[test]
fn wild_constructor_refuses_a_later_player_reserve_before_drawing() {
    for (bad, ability) in refused() {
        let mut rng = SequenceRng::new([]);
        let reserves = vec![healthy(SpeciesId::TREECKO), bad];
        let result = wild(
            healthy(SpeciesId::MUDKIP),
            reserves,
            healthy(SpeciesId::ZIGZAGOON),
            false,
            &mut rng,
        );
        assert_refused_without_draws(&result, ability, &rng);
    }
}

#[test]
fn trainer_constructor_refuses_an_active_player_before_drawing() {
    for (bad, ability) in refused() {
        let mut rng = SequenceRng::new([]);
        let result = trainer(bad, vec![], vec![healthy(SpeciesId::TREECKO)], &mut rng);
        assert_refused_without_draws(&result, ability, &rng);
    }
}

#[test]
fn trainer_constructor_refuses_the_active_enemy_before_drawing() {
    for (bad, ability) in refused() {
        let mut rng = SequenceRng::new([]);
        let result = trainer(healthy(SpeciesId::MUDKIP), vec![], vec![bad], &mut rng);
        assert_refused_without_draws(&result, ability, &rng);
    }
}

#[test]
fn trainer_constructor_refuses_a_later_bench_member_before_drawing() {
    for (bad, ability) in refused() {
        let mut rng = SequenceRng::new([]);
        let party = vec![
            healthy(SpeciesId::TREECKO),
            healthy(SpeciesId::TREECKO),
            bad,
        ];
        let result = trainer(healthy(SpeciesId::MUDKIP), vec![], party, &mut rng);
        assert_refused_without_draws(&result, ability, &rng);
    }
}

#[test]
fn trainer_constructor_refuses_a_later_player_reserve_before_drawing() {
    for (bad, ability) in refused() {
        let mut rng = SequenceRng::new([]);
        let reserves = vec![healthy(SpeciesId::TORCHIC), bad];
        let result = trainer(
            healthy(SpeciesId::MUDKIP),
            reserves,
            vec![healthy(SpeciesId::TREECKO)],
            &mut rng,
        );
        assert_refused_without_draws(&result, ability, &rng);
    }
}

const EFFECT_SPORE_HOLDERS: [SpeciesId; 4] = [
    SpeciesId::PARAS,
    SpeciesId::PARASECT,
    SpeciesId::SHROOMISH,
    SpeciesId::BRELOOM,
];

/// The constructor script for `player` against `enemy`: the order tiebreak is
/// drawn only when their turn-order Speeds tie.
fn admission_script_for(player: &BattlePokemon, enemy: &BattlePokemon) -> Vec<u16> {
    admission_script(player.speed_for_turn_order() == enemy.speed_for_turn_order())
}

#[test]
fn effect_spore_holders_are_admitted_on_every_participant_path() {
    for species in EFFECT_SPORE_HOLDERS {
        for status in [Status1::Healthy, Status1::Paralysed, Status1::Poisoned] {
            let holder = || {
                let holder = mon(species, 0, status);
                assert_eq!(holder.ability(), AbilityId::EFFECT_SPORE);
                holder
            };
            let opponent = || healthy(SpeciesId::ZIGZAGOON);
            let lead = || healthy(SpeciesId::MUDKIP);

            let mut rng = SequenceRng::new(admission_script_for(&holder(), &opponent()));
            wild(holder(), vec![], opponent(), false, &mut rng).unwrap();
            rng.assert_exhausted();

            let mut rng = SequenceRng::new(admission_script_for(&opponent(), &holder()));
            wild(opponent(), vec![], holder(), false, &mut rng).unwrap();
            rng.assert_exhausted();

            let mut rng = SequenceRng::new(admission_script_for(&lead(), &opponent()));
            wild(lead(), vec![holder()], opponent(), false, &mut rng).unwrap();
            rng.assert_exhausted();

            let mut rng = SequenceRng::new(admission_script_for(&holder(), &opponent()));
            trainer(holder(), vec![], vec![opponent()], &mut rng).unwrap();
            rng.assert_exhausted();

            let mut rng = SequenceRng::new(admission_script_for(&lead(), &holder()));
            trainer(lead(), vec![], vec![holder()], &mut rng).unwrap();
            rng.assert_exhausted();

            let mut rng = SequenceRng::new(admission_script_for(&lead(), &opponent()));
            trainer(
                lead(),
                vec![],
                vec![opponent(), opponent(), holder()],
                &mut rng,
            )
            .unwrap();
            rng.assert_exhausted();

            let mut rng = SequenceRng::new(admission_script_for(&lead(), &opponent()));
            trainer(lead(), vec![holder()], vec![opponent()], &mut rng).unwrap();
            rng.assert_exhausted();
        }
    }
}

#[test]
fn a_fainted_reserve_is_admitted_because_it_can_never_enter() {
    assert_ne!(
        healthy(SpeciesId::MUDKIP).speed_for_turn_order(),
        healthy(SpeciesId::ZIGZAGOON).speed_for_turn_order(),
        "the active pair must not tie on Speed, so construction draws no tiebreak and admission_script(false) fits"
    );
    for (index, (mut bad, _)) in refused().into_iter().enumerate() {
        bad.apply_damage(u32::MAX);
        assert!(bad.is_fainted());
        let mut rng = SequenceRng::new(admission_script(false));
        wild(
            healthy(SpeciesId::MUDKIP),
            vec![bad],
            healthy(SpeciesId::ZIGZAGOON),
            false,
            &mut rng,
        )
        .unwrap();
        rng.assert_exhausted();
        let (mut bad, _) = refused().into_iter().nth(index).unwrap();
        bad.apply_damage(u32::MAX);
        let mut rng = SequenceRng::new(admission_script(false));
        trainer(
            healthy(SpeciesId::MUDKIP),
            vec![],
            vec![healthy(SpeciesId::ZIGZAGOON), bad],
            &mut rng,
        )
        .unwrap();
        rng.assert_exhausted();
    }
}

#[test]
fn confusion_is_refused_only_for_own_tempo() {
    let mut zigzagoon = healthy(SpeciesId::ZIGZAGOON);
    zigzagoon.volatiles_mut().set_confusion(3);
    let enemy = healthy(SpeciesId::ZIGZAGOON);
    assert_eq!(
        zigzagoon.speed_for_turn_order(),
        enemy.speed_for_turn_order(),
        "the mirror fixture ties on Speed, so construction draws the seed and the order tiebreak"
    );
    let mut rng = SequenceRng::new(admission_script(true));
    let battle = wild(zigzagoon, vec![], enemy, false, &mut rng).unwrap();
    assert!(battle.player().volatiles().confused());
    rng.assert_exhausted();

    let spinda = healthy(SpeciesId::SPINDA);
    let enemy = healthy(SpeciesId::ZIGZAGOON);
    assert_eq!(
        spinda.speed_for_turn_order(),
        enemy.speed_for_turn_order(),
        "Spinda ties Zigzagoon on Speed, so construction draws the seed and the order tiebreak"
    );
    let mut rng = SequenceRng::new(admission_script(true));
    wild(spinda, vec![], enemy, false, &mut rng).unwrap();
    rng.assert_exhausted();
}

#[test]
fn an_always_refused_ability_is_refused_whatever_the_status() {
    for (species, ability) in [
        (SpeciesId::MIGHTYENA, AbilityId::INTIMIDATE),
        (SpeciesId::NINJASK, AbilityId::SPEED_BOOST),
        (SpeciesId::KECLEON, AbilityId::COLOR_CHANGE),
        (SpeciesId::PORYGON, AbilityId::TRACE),
        (SpeciesId::MAGMAR, AbilityId::FLAME_BODY),
        (SpeciesId::CLEFAIRY, AbilityId::CUTE_CHARM),
        (SpeciesId::KYOGRE, AbilityId::DRIZZLE),
        (SpeciesId::GROUDON, AbilityId::DROUGHT),
        (SpeciesId::TYRANITAR, AbilityId::SAND_STREAM),
    ] {
        for status in [Status1::Healthy, Status1::Paralysed, Status1::Poisoned] {
            always_refused_whatever(species, ability, status);
        }
    }
}

fn always_refused_whatever(species: SpeciesId, ability: AbilityId, status: Status1) {
    let mut rng = SequenceRng::new([]);
    let result = wild(
        mon(species, 0, status),
        vec![],
        healthy(SpeciesId::ZIGZAGOON),
        false,
        &mut rng,
    );
    assert_refused_without_draws(&result, ability, &rng);
    let mut rng = SequenceRng::new([]);
    let result = trainer(
        mon(species, 0, status),
        vec![],
        vec![healthy(SpeciesId::ZIGZAGOON)],
        &mut rng,
    );
    assert_refused_without_draws(&result, ability, &rng);
}

#[test]
fn the_selected_ability_slot_decides_not_the_species_pool() {
    for (species, slot, ability) in [
        (SpeciesId::EKANS, 1, AbilityId::SHED_SKIN),
        (SpeciesId::MAWILE, 0, AbilityId::HYPER_CUTTER),
        (SpeciesId::YANMA, 1, AbilityId::COMPOUND_EYES),
        (SpeciesId::RALTS, 0, AbilityId::SYNCHRONIZE),
        (SpeciesId::KIRLIA, 0, AbilityId::SYNCHRONIZE),
        (SpeciesId::GARDEVOIR, 0, AbilityId::SYNCHRONIZE),
    ] {
        let holder = mon(species, slot, Status1::Healthy);
        assert_eq!(holder.ability(), ability);
        let enemy = healthy(SpeciesId::ZIGZAGOON);
        assert_ne!(holder.speed_for_turn_order(), enemy.speed_for_turn_order());
        let mut rng = SequenceRng::new(admission_script(false));
        wild(holder, vec![], enemy, false, &mut rng).unwrap();
        rng.assert_exhausted();
    }

    let snorlax = mon(SpeciesId::SNORLAX, 1, Status1::Poisoned);
    assert_eq!(snorlax.ability(), AbilityId::THICK_FAT);
    let enemy = healthy(SpeciesId::ZIGZAGOON);
    assert_ne!(snorlax.speed_for_turn_order(), enemy.speed_for_turn_order());
    let mut rng = SequenceRng::new(admission_script(false));
    wild(snorlax, vec![], enemy, false, &mut rng).unwrap();
    rng.assert_exhausted();
}
