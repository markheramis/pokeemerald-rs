use std::ops::Range;

use super::super::{
    evs_from_substruct2, from_save_pokemon, pack_ivs, to_save_pokemon, unpack_ivs, LoadedLead,
    PartyError, MAIL_NONE,
};
use super::common::{
    saved_treecko_holding, torchic_before_learning_peck, treecko_fixture,
    EXPECTED_ATTACK_PP_OFFSET, EXPECTED_GROWTH_EXPERIENCE, EXPECTED_GROWTH_FRIENDSHIP,
    EXPECTED_GROWTH_HELD_ITEM, EXPECTED_IS_EGG_BIT, FIXTURE_PERSONALITY, GROWL, POUND, SCRATCH,
    TREECKO,
};
use assets::items::ItemId;
use battle::{BattlePokemon, Dex, Ivs};
use engine::save::{BoxPokemon, Pokemon};

const TENTACOOL: assets::SpeciesId = assets::SpeciesId(72);

const TACKLE: assets::MoveId = assets::MoveId::TACKLE;

const PECK: assets::MoveId = assets::MoveId::PECK;

const EXPECTED_GROWTH_SPECIES: Range<usize> = 0..2;

const EXPECTED_GROWTH_PP_BONUSES: usize = 8;

const EXPECTED_IV_FIELD_WIDTH: usize = 5;

const EXPECTED_IV_FIELD_MASK: u32 = 0x1F;

const EXPECTED_ABILITY_SLOT_SHIFT: usize = 31;

#[test]
fn ivs_pack_into_five_bit_fields_in_declaration_order() {
    let ivs = Ivs {
        hp: 1,
        attack: 2,
        defense: 3,
        speed: 4,
        sp_attack: 5,
        sp_defense: 6,
    };
    let word = pack_ivs(ivs);
    assert_eq!(word & EXPECTED_IV_FIELD_MASK, 1);
    assert_eq!(
        (word >> EXPECTED_IV_FIELD_WIDTH) & EXPECTED_IV_FIELD_MASK,
        2
    );
    assert_eq!(
        (word >> (2 * EXPECTED_IV_FIELD_WIDTH)) & EXPECTED_IV_FIELD_MASK,
        3
    );
    assert_eq!(
        (word >> (3 * EXPECTED_IV_FIELD_WIDTH)) & EXPECTED_IV_FIELD_MASK,
        4
    );
    assert_eq!(
        (word >> (4 * EXPECTED_IV_FIELD_WIDTH)) & EXPECTED_IV_FIELD_MASK,
        5
    );
    assert_eq!(
        (word >> (5 * EXPECTED_IV_FIELD_WIDTH)) & EXPECTED_IV_FIELD_MASK,
        6
    );
    assert_eq!(word >> (6 * EXPECTED_IV_FIELD_WIDTH), 0);
    assert_eq!(unpack_ivs(word), ivs);
}

#[test]
fn evs_from_substruct2_maps_each_byte_to_its_named_field() {
    let evs_and_condition: [u8; engine::save::SUBSTRUCTURE_LEN] =
        [10, 20, 30, 40, 50, 60, 0, 0, 0, 0, 0, 0];
    assert_eq!(
        evs_from_substruct2(&evs_and_condition),
        battle::Evs {
            hp: 10,
            attack: 20,
            defense: 30,
            speed: 40,
            sp_attack: 50,
            sp_defense: 60,
        }
    );
}

#[test]
fn the_egg_and_ability_bits_do_not_leak_into_the_ivs() {
    let ivs = Ivs {
        hp: 31,
        attack: 31,
        defense: 31,
        speed: 31,
        sp_attack: 31,
        sp_defense: 31,
    };
    let egg_and_ability_bits = EXPECTED_IS_EGG_BIT | (1 << EXPECTED_ABILITY_SLOT_SHIFT);
    assert_eq!(unpack_ivs(pack_ivs(ivs) | egg_and_ability_bits), ivs);
}

#[test]
fn a_battler_round_trips_through_the_save_layout() {
    let dex = Dex::new();
    let mut mon = treecko_fixture();
    mon.apply_damage(7);
    mon.deduct_pp(0).unwrap();
    mon.deduct_pp(0).unwrap();

    let saved = to_save_pokemon(&dex, &mon);
    let restored = from_save_pokemon(&dex, &saved).expect("what we just wrote must decode");

    assert_eq!(restored.species(), mon.species());
    assert_eq!(restored.level(), mon.level());
    assert_eq!(restored.personality(), mon.personality());
    assert_eq!(restored.original_trainer_id(), mon.original_trainer_id());
    assert_eq!(restored.nature(), mon.nature());
    assert_eq!(restored.ivs(), mon.ivs());
    assert_eq!(restored.stats(), mon.stats());
    assert_eq!(
        restored.current_hp(),
        mon.current_hp(),
        "damage taken before saving must survive the save"
    );
    assert_ne!(
        restored.current_hp(),
        restored.stats().max_hp,
        "the fixture must save a damaged mon, or full-HP restore would pass"
    );
    assert_eq!(restored.moves(), mon.moves(), "moves and PP, slot for slot");
}

#[test]
fn a_saved_held_item_decodes_onto_the_battler() {
    let dex = Dex::new();
    for item in [
        ItemId::NONE,
        ItemId::NUGGET,
        ItemId::ORAN_BERRY,
        ItemId(0xFFFF),
    ] {
        let restored = from_save_pokemon(&dex, &saved_treecko_holding(item))
            .expect("a record naming any u16 item decodes");
        assert_eq!(restored.held_item(), item, "{item:?}");
    }
}

#[test]
fn a_held_item_survives_the_battler_round_trip() {
    let dex = Dex::new();
    let mon = treecko_fixture().with_held_item(ItemId::NUGGET);

    let saved = to_save_pokemon(&dex, &mon);

    assert_eq!(
        saved.box_data.substructures().unwrap().growth[EXPECTED_GROWTH_HELD_ITEM],
        ItemId::NUGGET.0.to_le_bytes(),
        "a record encoded from a battler files the item it holds"
    );
    assert_eq!(
        from_save_pokemon(&dex, &saved).unwrap().held_item(),
        ItemId::NUGGET
    );
}

#[test]
fn sub_level_experience_survives_the_round_trip() {
    let dex = Dex::new();
    let mut mon = treecko_fixture();
    assert!(mon.apply_experience(&dex, 10).unwrap().is_none());
    let treecko = dex.species(mon.species()).unwrap();
    assert_eq!(
        mon.experience(),
        assets::experience_for_level(treecko.growth_rate, 12).unwrap() + 10,
        "the fixture must sit strictly between two thresholds, or a \
         level-derived re-encode would pass"
    );
    assert_eq!(mon.level(), 12);

    let restored = from_save_pokemon(&dex, &to_save_pokemon(&dex, &mon))
        .expect("what we just wrote must decode");
    assert_eq!(restored.experience(), mon.experience());
    assert_eq!(restored.level(), mon.level());
    assert_eq!(restored.stats(), mon.stats());
}

#[test]
fn a_move_learned_by_levelling_up_survives_the_round_trip() {
    let dex = Dex::new();
    let mut mon = torchic_before_learning_peck();

    let torchic = dex.species(mon.species()).unwrap();
    let level_16 = assets::experience_for_level(torchic.growth_rate, 16).unwrap();
    assert!(
        mon.apply_experience(&dex, level_16 - mon.experience())
            .unwrap()
            .is_none(),
        "two of the four slots are free, so Peck is learned without asking"
    );
    assert_eq!(
        mon.moves()
            .iter()
            .map(|slot| slot.move_id)
            .collect::<Vec<_>>(),
        vec![SCRATCH, GROWL, PECK],
        "fixture sanity: the level-up must actually have taught Peck \
         (MOVE_PECK, Torchic's level-16 learnset entry) before the round \
         trip can prove anything about it"
    );

    let restored = from_save_pokemon(&dex, &to_save_pokemon(&dex, &mon))
        .expect("what we just wrote must decode");
    assert_eq!(
        restored.moves(),
        mon.moves(),
        "the taught move -- and its freshly rolled PP -- survives the save \
         round trip like any other moveset slot"
    );
    assert_eq!(restored.level(), mon.level());
}

#[test]
fn an_experience_total_past_the_next_threshold_levels_the_decoded_mon_up() {
    let dex = Dex::new();
    let mon = treecko_fixture();
    let mut saved = to_save_pokemon(&dex, &mon);

    let treecko = dex.species(mon.species()).unwrap();
    let level_13 = assets::experience_for_level(treecko.growth_rate, 13).unwrap();
    let mut substructures = saved.box_data.substructures().unwrap();
    substructures.growth[EXPECTED_GROWTH_EXPERIENCE].copy_from_slice(&level_13.to_le_bytes());
    saved.box_data.set_substructures(&substructures);

    let restored = from_save_pokemon(&dex, &saved).expect("valid bytes must decode");
    assert_eq!(restored.level(), 13, "the level follows the experience");
    assert_eq!(restored.experience(), level_13);
}

#[test]
fn decoding_an_inconsistent_save_levels_up_without_teaching_moves() {
    let dex = Dex::new();
    let mon = torchic_before_learning_peck();
    let mut saved = to_save_pokemon(&dex, &mon);

    let torchic = dex.species(mon.species()).unwrap();
    let level_16 = assets::experience_for_level(torchic.growth_rate, 16).unwrap();
    let mut substructures = saved.box_data.substructures().unwrap();
    substructures.growth[EXPECTED_GROWTH_EXPERIENCE].copy_from_slice(&level_16.to_le_bytes());
    saved.box_data.set_substructures(&substructures);

    let restored = from_save_pokemon(&dex, &saved).expect("valid bytes must decode");
    assert_eq!(
        restored.level(),
        16,
        "the level still follows the experience"
    );
    assert_eq!(
        restored
            .moves()
            .iter()
            .map(|slot| slot.move_id)
            .collect::<Vec<_>>(),
        vec![SCRATCH, GROWL],
        "but the moveset stays exactly the saved attacks substructure -- \
         no Peck: load is not a level-up"
    );
}

#[test]
fn save_fields_are_encoded_at_their_layout_offsets() {
    let dex = Dex::new();
    let mon = treecko_fixture();
    let saved = to_save_pokemon(&dex, &mon);

    assert_eq!(saved.box_data.ot_id(), mon.original_trainer_id());
    assert_eq!(saved.level, 12);
    assert_eq!(saved.mail, MAIL_NONE);
    assert_eq!(saved.max_hp, u16::try_from(mon.stats().max_hp).unwrap());

    let substructures = saved.box_data.substructures().unwrap();
    assert_eq!(
        u16::from_le_bytes(
            substructures.growth[EXPECTED_GROWTH_SPECIES]
                .try_into()
                .unwrap()
        ),
        mon.species().0
    );
    let treecko = dex.species(mon.species()).unwrap();
    assert_eq!(
        u32::from_le_bytes(
            substructures.growth[EXPECTED_GROWTH_EXPERIENCE]
                .try_into()
                .unwrap()
        ),
        mon.experience(),
        "the growth word holds the mon's own accumulated experience"
    );
    assert_eq!(
        mon.experience(),
        assets::experience_for_level(treecko.growth_rate, 12).unwrap(),
        "a freshly built mon begins at its level's growth threshold"
    );
    assert_eq!(
        substructures.growth[EXPECTED_GROWTH_FRIENDSHIP],
        treecko.base_friendship
    );
    assert_eq!(
        substructures.evs_and_condition,
        [0; engine::save::SUBSTRUCTURE_LEN],
        "this fixture never gained EVs, so the EV substructure is written \
         all-zero"
    );
    assert_eq!(
        u16::from_le_bytes(
            substructures.attacks[..std::mem::size_of::<u16>()]
                .try_into()
                .unwrap()
        ),
        mon.moves()[0].move_id.0
    );
    assert_eq!(
        substructures.attacks[EXPECTED_ATTACK_PP_OFFSET],
        mon.moves()[0].pp
    );
}

/// A trailing `MOVE_NONE` slot is an *empty* slot upstream, not a known
/// move -- a decoder that carried it through would build a battler
/// `BattlePokemon::new` refuses outright.
#[test]
fn empty_move_slots_are_dropped_rather_than_decoded_as_moves() {
    let dex = Dex::new();
    let mon = BattlePokemon::new(&dex, TREECKO, 5, Ivs::default(), 0, vec![POUND]).unwrap();
    let saved = to_save_pokemon(&dex, &mon);
    let restored = from_save_pokemon(&dex, &saved).expect("a one-move mon must decode");
    assert_eq!(restored.moves().len(), 1);
}

/// A checksum-valid record with a move after an empty slot must be refused:
/// decoding the prefix would let the next save erase the later move and PP.
#[test]
fn an_interior_empty_move_slot_is_refused_rather_than_truncated() {
    const SECOND_MOVE_SLOT: Range<usize> = 2..4;
    const THIRD_MOVE_SLOT: Range<usize> = 4..6;

    let dex = Dex::new();
    let mon = BattlePokemon::new(
        &dex,
        TREECKO,
        5,
        Ivs::default(),
        0,
        vec![POUND, SCRATCH, TACKLE],
    )
    .unwrap();
    let mut saved = to_save_pokemon(&dex, &mon);
    let mut substructures = saved.box_data.substructures().unwrap();
    substructures.attacks[SECOND_MOVE_SLOT].fill(0);
    saved.box_data.set_substructures(&substructures);
    assert_eq!(
        saved.box_data.substructures().unwrap().attacks[THIRD_MOVE_SLOT],
        TACKLE.0.to_le_bytes(),
        "fixture sanity: the checksum is valid and the third move is still saved"
    );

    assert!(matches!(
        from_save_pokemon(&dex, &saved),
        Err(PartyError::InteriorEmptyMove)
    ));
    assert!(LoadedLead::load(&dex, std::slice::from_ref(&saved)).is_err());
}

#[test]
fn a_corrupt_secure_region_is_reported_not_guessed_at() {
    const SECURE_REGION_BYTE: usize = 40;
    const CORRUPTION_MASK: u8 = 0x80;

    let dex = Dex::new();
    let mut saved = to_save_pokemon(&dex, &treecko_fixture());
    let mut bytes = saved.box_data.to_bytes();
    bytes[SECURE_REGION_BYTE] ^= CORRUPTION_MASK;
    saved.box_data = BoxPokemon::from_bytes(bytes);

    assert!(matches!(
        from_save_pokemon(&dex, &saved),
        Err(PartyError::Substructures(_))
    ));
}

#[test]
fn an_empty_party_slot_does_not_decode_into_a_battler() {
    let err = from_save_pokemon(&Dex::new(), &Pokemon::default())
        .expect_err("SPECIES_NONE is not a fightable mon");
    assert!(matches!(err, PartyError::Battler(_)), "{err}");
    assert!(err.to_string().starts_with("saved party member:"));
}

#[test]
fn pp_ups_survive_the_round_trip_byte_for_byte() {
    let dex = Dex::new();
    let bonuses = battle::PpBonuses::from_bits(0b0000_0111);
    let mut mon = treecko_fixture().with_pp_bonuses(&dex, bonuses).unwrap();
    let slot_0_max = mon.max_pp(&dex, 0).unwrap();
    let base_pp = dex.move_data(mon.moves()[0].move_id).unwrap().pp;
    assert!(
        slot_0_max > base_pp,
        "fixture sanity: the upgraded slot must hold more than base PP"
    );
    mon.deduct_pp(0).unwrap();
    mon.deduct_pp(0).unwrap();

    let saved = to_save_pokemon(&dex, &mon);
    assert_eq!(
        saved.box_data.substructures().unwrap().growth[EXPECTED_GROWTH_PP_BONUSES],
        bonuses.bits(),
        "the growth substructure stores ppBonuses itself"
    );

    let restored = from_save_pokemon(&dex, &saved).expect("what we just wrote must decode");
    assert_eq!(restored.pp_bonuses(), bonuses);
    assert_eq!(restored.max_pp(&dex, 0).unwrap(), slot_0_max);
    assert_eq!(
        restored.max_pp(&dex, 1).unwrap(),
        mon.max_pp(&dex, 1).unwrap()
    );
    assert_eq!(
        restored.moves()[0].pp,
        slot_0_max - 2,
        "remaining PP is measured from the PP-Up-adjusted maximum"
    );
    assert_eq!(restored.moves(), mon.moves(), "moves and PP, slot for slot");

    let resaved = to_save_pokemon(&dex, &restored);
    assert_eq!(
        resaved.box_data.substructures().unwrap().growth[EXPECTED_GROWTH_PP_BONUSES],
        bonuses.bits(),
        "re-serialising must emit the identical byte, not zero"
    );
    assert_eq!(resaved, saved, "and the whole 100-byte value is unchanged");
}

#[test]
fn pp_bonus_bits_for_unknown_slots_are_not_stripped() {
    let dex = Dex::new();
    let bonuses = battle::PpBonuses::from_bits(0b1111_1111);
    let mon = BattlePokemon::new(
        &dex,
        TREECKO,
        12,
        Ivs::default(),
        FIXTURE_PERSONALITY,
        vec![TACKLE],
    )
    .unwrap()
    .with_pp_bonuses(&dex, bonuses)
    .unwrap();
    assert!(
        mon.moves().len() < battle::MAX_MON_MOVES,
        "fixture sanity: the fixture must leave at least one slot empty"
    );

    let saved = to_save_pokemon(&dex, &mon);
    let restored = from_save_pokemon(&dex, &saved).unwrap();

    assert_eq!(restored.pp_bonuses().bits(), 0b1111_1111);
    assert_eq!(
        to_save_pokemon(&dex, &restored)
            .box_data
            .substructures()
            .unwrap()
            .growth[EXPECTED_GROWTH_PP_BONUSES],
        0b1111_1111
    );
}

#[test]
fn healing_a_restored_mon_refills_to_the_upgraded_maximum() {
    let dex = Dex::new();
    let bonuses = battle::PpBonuses::from_bits(0b0000_0011);
    let mut mon = treecko_fixture().with_pp_bonuses(&dex, bonuses).unwrap();
    for _ in 0..5 {
        mon.deduct_pp(0).unwrap();
    }
    let saved = to_save_pokemon(&dex, &mon);

    let mut restored = from_save_pokemon(&dex, &saved).unwrap();
    restored.heal(&dex).unwrap();

    let base_pp = dex.move_data(restored.moves()[0].move_id).unwrap().pp;
    assert_eq!(restored.moves()[0].pp, restored.max_pp(&dex, 0).unwrap());
    assert!(
        restored.moves()[0].pp > base_pp,
        "a heal that stopped at base PP would strip the PP Ups again"
    );
}

#[test]
fn a_disagreeing_ability_slot_survives_the_save_round_trip() {
    const CLEAR_BODY: u16 = 29;
    const LIQUID_OOZE: u16 = 64;
    const EVEN_PERSONALITY: u32 = 0x1234_ABCC;

    let dex = Dex::new();
    let mon = BattlePokemon::new(
        &dex,
        TENTACOOL,
        20,
        Ivs::default(),
        EVEN_PERSONALITY,
        vec![TACKLE],
    )
    .expect("Tentacool is in the dex")
    .with_ability_slot(1);
    assert_eq!(
        mon.ability().0,
        LIQUID_OOZE,
        "fixture sanity: the override, not personality parity, decides"
    );
    assert_ne!(
        mon.ability().0,
        CLEAR_BODY,
        "fixture sanity: personality parity alone would have picked this"
    );

    let restored = from_save_pokemon(&dex, &to_save_pokemon(&dex, &mon))
        .expect("what we just wrote must decode");
    assert_eq!(restored.ability_slot(), 1);
    assert_eq!(
        restored.ability().0,
        LIQUID_OOZE,
        "the disagreeing slot survives the round trip instead of being \
         re-derived from the (even) personality"
    );
}

/// `STATUS1_*` (`pokeemerald/include/constants/battle.h:114-125`): the three
/// sleep bits count turns left, poison is `0x08`, burn is `0x10`, paralysis
/// is `0x40`, and bad poison is `0x80` with the counter in bits 8–11.
#[test]
fn every_modelled_status_word_decodes_and_encodes_back_to_itself() {
    let dex = Dex::new();
    let mut cases = vec![
        (0, battle::Status1::Healthy),
        (1, battle::Status1::Asleep { turns: 1 }),
        (2, battle::Status1::Asleep { turns: 2 }),
        (3, battle::Status1::Asleep { turns: 3 }),
        (4, battle::Status1::Asleep { turns: 4 }),
        (5, battle::Status1::Asleep { turns: 5 }),
        (6, battle::Status1::Asleep { turns: 6 }),
        (7, battle::Status1::Asleep { turns: 7 }),
        (8, battle::Status1::Poisoned),
        (16, battle::Status1::Burned),
        (64, battle::Status1::Paralysed),
    ];
    for counter in 0..=15 {
        cases.push((
            0x80 | (u32::from(counter) << 8),
            battle::Status1::BadlyPoisoned { counter },
        ));
    }
    for (word, expected) in cases {
        let mut record = to_save_pokemon(&dex, &treecko_fixture());
        record.status = word;

        let mut loaded = LoadedLead::load(&dex, &[record]).expect("a modelled status decodes");
        assert_eq!(loaded.battler().status1(), expected, "word {word:#x}");

        assert_eq!(loaded.merge_and_save(&dex).status, word, "word {word:#x}");
        let mut fresh = treecko_fixture();
        fresh.set_status1(expected);
        assert_eq!(to_save_pokemon(&dex, &fresh).status, word, "word {word:#x}");
    }
}

/// Freeze `0x20`, a toxic counter without bad poison (`0x100`), several
/// conditions at once, a sleep count beside poison, and bits above
/// `STATUS1_ANY` are not modelled
/// (`pokeemerald/include/constants/battle.h:117-125`): decoding fails closed
/// and names the original word, never masking it to a neighbour.
#[test]
fn an_unmodelled_status_word_fails_closed_with_the_original_word() {
    let dex = Dex::new();
    for word in [
        0x20,
        0x100,
        0x9,
        0x48,
        0x88,
        0x18,
        0x50,
        0x11,
        0x1080,
        0x8000_0000,
        0x1_0000,
    ] {
        let mut record = to_save_pokemon(&dex, &treecko_fixture());
        record.status = word;

        assert_eq!(
            from_save_pokemon(&dex, &record).unwrap_err(),
            PartyError::UnsupportedStatus(word),
            "word {word:#x}"
        );
        assert_eq!(
            LoadedLead::load(&dex, &[record]).unwrap_err(),
            PartyError::UnsupportedStatus(word),
            "a lone undecodable slot has nothing to fall back to: word {word:#x}"
        );
        assert_eq!(record.status, word, "the record's bytes are untouched");
    }
}

#[test]
fn an_internal_toxic_counter_beyond_the_status_word_is_refused_rather_than_truncated() {
    let mut lead = treecko_fixture();
    lead.set_status1(battle::Status1::BadlyPoisoned { counter: 16 });

    let outcome = std::panic::catch_unwind(|| to_save_pokemon(&Dex::new(), &lead));

    assert!(
        outcome.is_err(),
        "counter 16 would otherwise spill into the bits above STATUS1_TOXIC_COUNTER"
    );
}

#[test]
fn an_internal_sleep_count_beyond_the_status_word_is_refused_rather_than_truncated() {
    let mut lead = treecko_fixture();
    lead.set_status1(battle::Status1::Asleep { turns: 8 });

    let outcome = std::panic::catch_unwind(|| to_save_pokemon(&Dex::new(), &lead));

    assert!(
        outcome.is_err(),
        "turns 8 would otherwise encode as STATUS1_POISON"
    );
}
