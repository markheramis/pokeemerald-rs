use std::ops::Range;

use super::super::{
    from_save_pokemon, hp_hidden_by_load, merge_into_save_pokemon, to_save_pokemon, unpack_ivs,
    LoadedLead, MAIL_NONE,
};
use super::common::{
    retained_evs, stored_record_with_retained_fields, treecko_fixture, EXPECTED_ATTACK_PP_OFFSET,
    EXPECTED_GROWTH_EXPERIENCE, EXPECTED_GROWTH_FRIENDSHIP, EXPECTED_GROWTH_HELD_ITEM,
    EXPECTED_IS_EGG_BIT, EXPECTED_MISC_IV_WORD, MISC_RIBBONS, POUND, RETAINED_EVS_AND_CONDITION,
    RETAINED_FRIENDSHIP, RETAINED_MAIL, RETAINED_STATUS, TREECKO,
};
use battle::{BattlePokemon, Dex, Ivs};
use engine::save::Pokemon;

const MAX_TOTAL_EVS: u16 = 510;

const MISC_ENCOUNTER_DATA: Range<usize> = 0..4;

const BOX_RETAINED_HEADER: Range<usize> = 8..28;

#[test]
fn the_retained_evs_stay_inside_a_total_an_upstream_save_can_hold() {
    let evs = retained_evs();
    let total = u16::from(evs.hp)
        + u16::from(evs.attack)
        + u16::from(evs.defense)
        + u16::from(evs.speed)
        + u16::from(evs.sp_attack)
        + u16::from(evs.sp_defense);
    assert!(
        total <= MAX_TOTAL_EVS,
        "fixture sanity: {total} EVs is a spread no upstream save can hold"
    );
}

#[test]
fn re_saving_a_loaded_mon_keeps_every_field_the_battle_model_does_not_carry() {
    let dex = Dex::new();
    let stored = stored_record_with_retained_fields();
    let mut loaded =
        LoadedLead::load(&dex, std::slice::from_ref(&stored)).expect("the fixture must decode");
    loaded.battler_mut().apply_damage(9);
    loaded.battler_mut().deduct_pp(0).unwrap();

    let merged = loaded.merge_and_save(&dex);

    let before = stored.box_data.substructures().unwrap();
    let after = merged
        .box_data
        .substructures()
        .expect("the merge must leave the checksum valid");
    assert_eq!(
        &after.growth[EXPECTED_GROWTH_HELD_ITEM], &before.growth[EXPECTED_GROWTH_HELD_ITEM],
        "heldItem is the save's"
    );
    assert_eq!(
        after.growth[EXPECTED_GROWTH_FRIENDSHIP], RETAINED_FRIENDSHIP,
        "accumulated friendship is the save's, not the species' base value"
    );
    assert_ne!(
        after.growth[EXPECTED_GROWTH_FRIENDSHIP],
        dex.species(loaded.battler().species())
            .unwrap()
            .base_friendship,
        "fixture sanity: a re-derived friendship would differ from this"
    );
    assert_eq!(
        after.evs_and_condition, RETAINED_EVS_AND_CONDITION,
        "EVs and contest condition are the save's, whole"
    );
    assert_eq!(
        &after.misc[MISC_ENCOUNTER_DATA], &before.misc[MISC_ENCOUNTER_DATA],
        "pokérus and the met/ball/OT-gender bytes are the save's"
    );
    assert_eq!(
        &after.misc[MISC_RIBBONS], &before.misc[MISC_RIBBONS],
        "the ribbon word is the save's"
    );
    assert_eq!(
        merged.status, RETAINED_STATUS,
        "paralysis loaded into the battler is filed back unchanged"
    );
    assert_eq!(merged.mail, RETAINED_MAIL, "the mail slot is the save's");
    assert_eq!(
        merged.box_data.to_bytes()[BOX_RETAINED_HEADER],
        stored.box_data.to_bytes()[BOX_RETAINED_HEADER],
        "nickname, language, OT name and markings are the save's"
    );
    assert_eq!(merged.box_data.personality(), stored.box_data.personality());
    assert_eq!(merged.box_data.ot_id(), stored.box_data.ot_id());

    assert_eq!(
        [
            merged.max_hp,
            merged.attack,
            merged.defense,
            merged.speed,
            merged.special_attack,
            merged.special_defense,
        ],
        [
            stored.max_hp,
            stored.attack,
            stored.defense,
            stored.speed,
            stored.special_attack,
            stored.special_defense,
        ],
        "the EV-trained stat block is the save's, not the 0-EV block this \
         port recomputes"
    );
    assert_ne!(
        merged.max_hp,
        u16::try_from(loaded.battler().stats().max_hp).unwrap(),
        "fixture sanity: recomputing the block really would have moved it"
    );
    assert_eq!(
        merged.hp,
        u16::try_from(loaded.battler().current_hp()).unwrap(),
        "current HP is battle state, so it is the battler's either way"
    );
    assert!(
        merged.hp <= merged.max_hp,
        "and cannot contradict a retained maximum: the model's own maximum \
         is the 0-EV one, and EVs only add"
    );
}

/// The merge writes the battler's own primary status
/// (`SetPlayerMonData`'s `MON_DATA_STATUS`,
/// `pokeemerald/src/battle_controller_player.c:1983-1987`), whatever word the
/// backing record held: an inflicted condition reaches the save and a cure
/// clears it. Word values are `STATUS1_*`
/// (`pokeemerald/include/constants/battle.h:114-121`).
#[test]
fn a_merge_files_the_battlers_primary_status_over_the_backing_records_word() {
    const STATUS1_NONE: u32 = 0;
    const STATUS1_POISON: u32 = 8;
    const STATUS1_BURN: u32 = 16;
    const STATUS1_PARALYSIS: u32 = 64;
    let dex = Dex::new();
    let stored = stored_record_with_retained_fields();
    assert_eq!(stored.status, STATUS1_PARALYSIS, "fixture sanity");
    for (in_battle_status, expected_word) in [
        (battle::Status1::Poisoned, STATUS1_POISON),
        (battle::Status1::Burned, STATUS1_BURN),
        (battle::Status1::Paralysed, STATUS1_PARALYSIS),
        (battle::Status1::BadlyPoisoned { counter: 0 }, 0x80),
        (battle::Status1::BadlyPoisoned { counter: 5 }, 0x580),
        (battle::Status1::Asleep { turns: 3 }, 3),
        (battle::Status1::Asleep { turns: 0 }, STATUS1_NONE),
        (battle::Status1::Healthy, STATUS1_NONE),
    ] {
        let mut loaded =
            LoadedLead::load(&dex, std::slice::from_ref(&stored)).expect("the fixture must decode");
        assert_eq!(loaded.battler().status1(), battle::Status1::Paralysed);
        loaded.battler_mut().set_status1(in_battle_status);

        let merged = loaded.merge_and_save(&dex);

        assert_eq!(
            merged.status, expected_word,
            "{in_battle_status:?} must be the merged record's status word"
        );
        assert_eq!(
            merged.box_data.substructures().map(|s| s.growth),
            stored.box_data.substructures().map(|s| s.growth),
            "the status write leaves the secure region's growth bytes alone"
        );
    }
}

#[test]
fn sub_level_experience_does_not_flatten_the_retained_stat_block() {
    let dex = Dex::new();
    let stored = stored_record_with_retained_fields();
    let mut loaded =
        LoadedLead::load(&dex, std::slice::from_ref(&stored)).expect("the fixture must decode");

    let level_13 = assets::experience_for_level(
        dex.species(loaded.battler().species()).unwrap().growth_rate,
        13,
    )
    .unwrap();
    let award = level_13 - 1 - loaded.battler().experience();
    let _ = loaded
        .battler_mut()
        .apply_experience(&dex, award)
        .expect("an award short of the threshold is in range");
    assert_eq!(
        loaded.battler().level(),
        12,
        "fixture sanity: no level was crossed"
    );
    assert_ne!(
        loaded.battler().experience(),
        u32::from_le_bytes(
            stored.box_data.substructures().unwrap().growth[EXPECTED_GROWTH_EXPERIENCE]
                .try_into()
                .unwrap()
        ),
        "fixture sanity: the experience word really moved"
    );

    let merged = loaded.merge_and_save(&dex);
    let after = merged.box_data.substructures().unwrap();
    assert_eq!(
        u32::from_le_bytes(after.growth[EXPECTED_GROWTH_EXPERIENCE].try_into().unwrap()),
        loaded.battler().experience(),
        "the awarded experience is saved"
    );
    assert_eq!(
        [merged.max_hp, merged.attack, merged.defense],
        [stored.max_hp, stored.attack, stored.defense],
        "and the EV-trained block is retained: sub-level experience is not \
         an input to the stat formula"
    );
}

#[test]
fn re_saving_an_untouched_lead_writes_the_record_back_byte_for_byte() {
    let dex = Dex::new();
    let stored = stored_record_with_retained_fields();
    let mut loaded =
        LoadedLead::load(&dex, std::slice::from_ref(&stored)).expect("the fixture must decode");
    assert_ne!(
        stored.max_hp,
        u16::try_from(loaded.battler().stats().max_hp).unwrap(),
        "fixture sanity: the stored block carries an EV contribution the \
         model cannot rebuild, so a re-derived block would differ"
    );

    let merged = loaded.merge_and_save(&dex);

    let (merged_bytes, stored_bytes) = (merged.to_bytes(), stored.to_bytes());
    let moved: Vec<usize> = (0..merged_bytes.len())
        .filter(|index| merged_bytes[*index] != stored_bytes[*index])
        .collect();
    assert_eq!(
        moved,
        Vec::<usize>::new(),
        "an untouched lead must re-save as the same 100 bytes"
    );

    assert_eq!(
        loaded.merge_and_save(&dex).to_bytes(),
        stored.to_bytes(),
        "a second save on the same aggregate is idempotent"
    );
    let mut reloaded = LoadedLead::load(&dex, std::slice::from_ref(&merged))
        .expect("the re-saved record must decode");
    assert_eq!(reloaded.merge_and_save(&dex).to_bytes(), stored.to_bytes());
}

#[test]
#[allow(clippy::too_many_lines)] // one continuous level-up, damage, PP, and re-save scenario
fn re_saving_a_loaded_mon_overlays_what_the_session_changed() {
    let dex = Dex::new();
    let stored = stored_record_with_retained_fields();
    let mut loaded =
        LoadedLead::load(&dex, std::slice::from_ref(&stored)).expect("the fixture must decode");

    let treecko = dex.species(loaded.battler().species()).unwrap();
    let level_13 = assets::experience_for_level(treecko.growth_rate, 13).unwrap();
    let award = level_13 - loaded.battler().experience();
    let _ = loaded
        .battler_mut()
        .apply_experience(&dex, award)
        .expect("a level-13 award is in range");
    assert_eq!(loaded.battler().level(), 13, "fixture: it levelled up");
    loaded.battler_mut().apply_damage(11);
    loaded.battler_mut().deduct_pp(1).unwrap();
    loaded.battler_mut().deduct_pp(1).unwrap();
    let merged = loaded.merge_and_save(&dex);
    let after = merged.box_data.substructures().unwrap();

    assert_eq!(
        u32::from_le_bytes(after.growth[EXPECTED_GROWTH_EXPERIENCE].try_into().unwrap()),
        loaded.battler().experience(),
        "the growth word carries the experience the battle awarded"
    );
    assert_eq!(merged.level, 13, "and the level that came with it");
    let recompute = |level: u8, evs: battle::Evs| {
        battle::compute_stats_with_evs(
            loaded.battler().species(),
            treecko,
            level,
            loaded.battler().nature(),
            loaded.battler().ivs(),
            evs,
        )
    };
    let old_floor = recompute(stored.level, battle::Evs::default()).max_hp;
    let gap_old = u32::from(stored.max_hp) - old_floor;
    let gap_new = u32::from(merged.max_hp) - loaded.battler().stats().max_hp;
    let rebased_offset =
        u16::try_from(gap_new - gap_old).expect("the fixture's EVs keep this well under u16::MAX");
    assert_eq!(
        merged.hp,
        u16::try_from(loaded.battler().current_hp()).unwrap() + rebased_offset,
        "the level-up moved the EV-aware gap, so the saved HP carries that \
         movement even though nothing was clamped at load"
    );
    assert_ne!(merged.hp, stored.hp, "fixture sanity: the damage is real");

    // The recomputed block is EV-aware -- fed the fixture's own retained EV
    // bytes through `CalculateMonStats`'s formula, not the battler's `0`-EV
    // `loaded.battler().stats()` cache: only this save-time recompute is EV-aware, the
    // live cache stays `0`-EV for the whole battle.
    let expected = recompute(loaded.battler().level(), retained_evs());
    assert_eq!(
        merged.max_hp,
        u16::try_from(expected.max_hp).unwrap(),
        "a level-up moved what the cached block is a function of, so the \
         block is recomputed from the record's retained EV bytes"
    );
    assert_ne!(
        merged.max_hp,
        u16::try_from(loaded.battler().stats().max_hp).unwrap(),
        "fixture sanity: the retained hp EV (252) really does raise the \
         saved block above the battler's own 0-EV cache"
    );
    assert_ne!(
        merged.max_hp, stored.max_hp,
        "fixture sanity: the retained block would have been the level-12 one"
    );
    assert_eq!(
        [
            merged.attack,
            merged.defense,
            merged.speed,
            merged.special_attack,
            merged.special_defense,
        ],
        [
            u16::try_from(expected.attack).unwrap(),
            u16::try_from(expected.defense).unwrap(),
            u16::try_from(expected.speed).unwrap(),
            u16::try_from(expected.sp_attack).unwrap(),
            u16::try_from(expected.sp_defense).unwrap(),
        ],
        "the whole block, not just the maximum HP"
    );
    assert_eq!(
        after.attacks,
        super::encode_attacks(loaded.battler()),
        "moves and per-slot PP, slot for slot"
    );
    assert_ne!(
        &after.attacks[EXPECTED_ATTACK_PP_OFFSET..],
        &stored.box_data.substructures().unwrap().attacks[EXPECTED_ATTACK_PP_OFFSET..],
        "fixture sanity: the spent PP is real"
    );
    assert_eq!(
        after.evs_and_condition[0..6],
        RETAINED_EVS_AND_CONDITION[0..6],
        "the record's own retained EVs round-trip back out \
         unchanged -- nothing in this session called `gain_evs`"
    );

    let mut expected_reloaded = loaded.battler().clone();
    expected_reloaded.heal_hp(u32::from(rebased_offset));
    let reloaded = from_save_pokemon(&dex, &merged).expect("the merge must decode again");
    assert_eq!(
        reloaded, expected_reloaded,
        "and back out as the same battler, plus the rebased offset's point"
    );
}

#[test]
fn a_slot_holding_a_different_pokemon_is_rebuilt_rather_than_overlaid() {
    const DIFFERENT_PERSONALITY_BITS: u32 = 0x0F0F_0F0F;
    const DIFFERENT_ORIGINAL_TRAINER_ID: u32 = 0x0BAD_0BAD;

    let dex = Dex::new();
    let stored = stored_record_with_retained_fields();
    let lead = from_save_pokemon(&dex, &stored).expect("the fixture must decode");

    let swapped_personality = BattlePokemon::new(
        &dex,
        lead.species(),
        lead.level(),
        lead.ivs(),
        lead.personality() ^ DIFFERENT_PERSONALITY_BITS,
        lead.moves().iter().map(|slot| slot.move_id).collect(),
    )
    .unwrap()
    .with_original_trainer_id(lead.original_trainer_id());
    assert_eq!(
        merge_into_save_pokemon(&dex, &swapped_personality, &stored, &mut 0),
        to_save_pokemon(&dex, &swapped_personality),
        "a different personality is a different mon"
    );

    let traded_away = lead
        .clone()
        .with_original_trainer_id(DIFFERENT_ORIGINAL_TRAINER_ID);
    assert_eq!(
        merge_into_save_pokemon(&dex, &traded_away, &stored, &mut 0),
        to_save_pokemon(&dex, &traded_away),
        "so is a different original trainer -- it is half the XOR key"
    );
}

#[test]
fn an_empty_slot_is_built_from_scratch() {
    let dex = Dex::new();
    let mon = BattlePokemon::new(&dex, TREECKO, 5, Ivs::default(), 0, vec![POUND]).unwrap();
    let empty = Pokemon::default();
    assert_eq!(
        empty.box_data.personality(),
        mon.personality(),
        "fixture sanity: the identity gate alone would let this through, so \
         the species check is what decides"
    );
    let built = merge_into_save_pokemon(&dex, &mon, &empty, &mut 0);
    assert_eq!(built, to_save_pokemon(&dex, &mon));
    assert_eq!(built.mail, MAIL_NONE, "an empty slot has no mail to keep");
}

#[test]
fn the_merge_rewrites_the_iv_word_around_the_egg_bit() {
    let dex = Dex::new();
    let mut stored = stored_record_with_retained_fields();
    let mut substructures = stored.box_data.substructures().unwrap();
    let iv_word = u32::from_le_bytes(
        substructures.misc[EXPECTED_MISC_IV_WORD]
            .try_into()
            .unwrap(),
    );
    substructures.misc[EXPECTED_MISC_IV_WORD]
        .copy_from_slice(&(iv_word | EXPECTED_IS_EGG_BIT).to_le_bytes());
    stored.box_data.set_substructures(&substructures);

    let lead = from_save_pokemon(&dex, &stored)
        .expect("the fixture must decode")
        .with_ability_slot(1);
    let merged = merge_into_save_pokemon(
        &dex,
        &lead,
        &stored,
        &mut hp_hidden_by_load(&dex, &stored, &lead),
    );

    let merged_word = u32::from_le_bytes(
        merged.box_data.substructures().unwrap().misc[EXPECTED_MISC_IV_WORD]
            .try_into()
            .unwrap(),
    );
    assert_eq!(
        merged_word & EXPECTED_IS_EGG_BIT,
        EXPECTED_IS_EGG_BIT,
        "the egg bit this port does not model stays exactly as it was"
    );
    assert_eq!(merged_word >> 31, 1, "abilityNum is the battler's");
    assert_eq!(unpack_ivs(merged_word), lead.ivs());
}

/// A merge over a record a pre-#1208 build saved with `hasSpecies` wrongly
/// clear must repair the bit, not just carry it forward, matching how this
/// branch already re-files growth species every save.
#[test]
fn merge_into_save_pokemon_heals_a_stale_clear_has_species_bit() {
    let dex = Dex::new();
    let mon = treecko_fixture();
    let mut stale = to_save_pokemon(&dex, &mon);
    stale.box_data.set_has_species(false);

    let mut offset = hp_hidden_by_load(&dex, &stale, &mon);
    let merged = merge_into_save_pokemon(&dex, &mon, &stale, &mut offset);

    assert!(
        merged.box_data.has_species(),
        "a merge over a stale record must set hasSpecies, or the slot stays \
         checksum-valid but empty to Emerald-compatible readers"
    );
}
