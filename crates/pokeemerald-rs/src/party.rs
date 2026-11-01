//! Conversion between battle-ready party members and Emerald save records.
//!
//! [`battle::BattlePokemon`] models the state used in battle, while
//! [`engine::save::Pokemon`] preserves Emerald's complete 100-byte party
//! record. Both encoders write the live species, experience, PP bonuses,
//! moves and PP, IVs, ability slot, level, and current HP. A new record uses
//! save-format defaults for everything else; [`merge_into_save_pokemon`]
//! overlays those fields onto the record that was loaded, retaining its
//! header, friendship, contest condition, egg flag, encounter
//! data, ribbons, and mail. EVs are the battler's: the merge writes
//! them back from [`battle::BattlePokemon::evs`].
//!
//! # Held item
//!
//! [`from_save_pokemon`] reads the growth substructure's held item
//! (`MON_DATA_HELD_ITEM`, `pokeemerald/src/pokemon.c:3833`) onto the battler
//! through [`battle::BattlePokemon::with_held_item`], whatever its id.
//! `battle` carries the item but models none of its effects, so
//! [`battle::ensure_held_item_playable`] refuses a battle whose player-side
//! item has one; each flow's refusal is documented where it is made. A fresh
//! [`to_save_pokemon`] record files the battler's item. The merge leaves the
//! backing record's item bytes alone, as nothing in the port changes a held
//! item after load.
//!
//! Deliberately not modelled by `battle`, and so retained from the backing
//! record by [`merge_into_save_pokemon`]: friendship, pokérus,
//! met data, poké ball, OT gender, ribbons, markings, nickname, OT name,
//! language, mail, the egg bit, and contest condition (`PokemonSubstruct2`'s
//! trailing six bytes). [`to_save_pokemon`], which has no record to retain
//! them from, writes upstream's own `CreateMon` default for whichever of
//! those it can derive from its own two arguments: nickname
//! (the species' display name), language (`GAME_LANGUAGE`), met level
//! ([`battle::BattlePokemon::created_at_level`], not the live level -- a
//! mon that levels up before its first save still files the level it was
//! created at, matching `CreateBoxMon`'s one-time stamp), met game
//! (`VERSION_EMERALD`), and poké ball (`ITEM_POKE_BALL`) -- friendship is
//! derived from the species' base
//! friendship the same way, matching `CreateBoxMon`. OT name, met location,
//! and OT gender instead come from the current save's player identity and
//! map upstream, which `to_save_pokemon` has no way to see; those three stay
//! zero, same as every other field `battle` does not model, until a caller
//! that holds that state is threaded through -- [`starter`]'s level-five grant
//! is the first such caller and stamps all three.
//!
//! Primary status is the battler's, in both directions
//! (`pokeemerald/src/battle_controller_player.c:1636-1640,1983-1987`): both
//! encoders write [`battle::BattlePokemon::status1`] into the record's status
//! word, so an inflicted or cured condition reaches the save, and
//! [`from_save_pokemon`] reads it back. Only healthy, ordinary poison,
//! paralysis, and sleep (`1..=7` turns left) are modelled
//! (`pokeemerald/include/constants/battle.h:114-125`); a record whose word is
//! anything else -- burn, freeze, bad poison, several conditions, or stray
//! bits -- fails to decode with [`PartyError::UnsupportedStatus`] rather than
//! reading as healthy and losing the condition on the next save.
//!
//! Effort values are the one battle-authoritative field with its own
//! adoption/gain contract -- see [`battle::pokemon::evs`] for that. Both
//! encoders write the live value into `PokemonSubstruct2`'s first six bytes
//! ([`evs_from_substruct2`], [`evs_to_substruct2`]) unconditionally, whether
//! or not the session's KOs also crossed a level.
//!
//! The saved stat block is a cache derived from species, level, IVs, EVs, and
//! nature. Emerald's save/load path copies it without recalculating it
//! (`pokeemerald/src/load_save.c:160-178`), so a merge retains the block while
//! species and level are unchanged; if either changes, the block is
//! recomputed EV-aware -- see [`compute_levelled_up_stats`] for which EVs
//! that recompute uses, and why. [`to_save_pokemon`] runs the same
//! species/level-moved test against
//! [`battle::BattlePokemon::created_at_level`] instead of a stored record's
//! byte, since it has no record of its own to compare against.
//!
//! Loading an EV-trained record can clamp current HP to the live model's
//! lower maximum ([`battle::pokemon::evs`] owns why that maximum never
//! moves). [`hp_hidden_by_load`] records the hidden points; a later merge
//! translates the live HP back into the retained or recomputed range and
//! rebases the offset (signed -- it can shrink across a level-up) when the
//! EV-derived maximum changes ([`overlay_current_hp_with_hidden_points`]).
//! Shedinja is the one exception the recompute cannot fix on the retained
//! branch: [`normalize_retained_shedinja_max_hp`] pins its maximum back to
//! `1` and rebases the offset by the points it removes.

use std::ops::Range;

use battle::{BattlePokemon, Dex, Ivs, MAX_MON_MOVES};
use engine::save::{BoxPokemon, Pokemon, PokemonSubstructures, BOX_NICKNAME_LEN, SUBSTRUCTURE_LEN};

const MAIL_NONE: u8 = u8::MAX;
const SPECIES_NONE: u16 = 0;

const GROWTH_SPECIES: Range<usize> = 0..2;
const GROWTH_HELD_ITEM: Range<usize> = 2..4;
const GROWTH_EXPERIENCE: Range<usize> = 4..8;
const GROWTH_PP_BONUSES: usize = 8;
const GROWTH_FRIENDSHIP: usize = 9;

const STAT_VALUE_COUNT: usize = 6;

const MISC_IV_WORD: Range<usize> = 4..8;
const IV_FIELD_WIDTH: usize = 5;
const IV_FIELD_MASK: u32 = 0x1F;
const IS_EGG_BIT: u32 = 1 << 30;

/// `PokemonSubstruct3`'s origins word (`pokeemerald/include/pokemon.h:131-149`):
/// met level in bits 0-6, met game in bits 7-10, poké ball in bits 11-14, OT
/// gender in bit 15.
const MISC_MET_DATA: Range<usize> = 2..4;
const MET_LEVEL_MASK: u16 = 0x7F;
const MET_GAME_SHIFT: u16 = 7;
const POKE_BALL_SHIFT: u16 = 11;
const OT_GENDER_SHIFT: u16 = 15;
/// `PokemonSubstruct3::metLocation`, the misc substructure's second byte.
const MISC_MET_LOCATION: usize = 1;

/// `VERSION_EMERALD` (`pokeemerald/include/constants/global.h:8-16`) --
/// `gGameVersion`'s fixed value for this build, not player state, so
/// [`to_save_pokemon`] can write it unconditionally.
const VERSION_EMERALD: u16 = 3;
/// `ITEM_POKE_BALL` (`pokeemerald/include/constants/items.h:10`) --
/// `CreateBoxMon`'s own unconditional default ball.
const ITEM_POKE_BALL: u16 = 4;
/// `GAME_LANGUAGE` (`pokeemerald/include/constants/global.h:21-30`,
/// `LANGUAGE_ENGLISH`) -- `gGameLanguage`'s fixed value for this build, not
/// player state.
const LANGUAGE_ENGLISH: u8 = 2;
const ABILITY_SLOT_SHIFT: usize = 31;

const MOVE_ID_WIDTH: usize = size_of::<u16>();
const ATTACK_PP_OFFSET: usize = MAX_MON_MOVES * MOVE_ID_WIDTH;

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "no production caller until the starter chooser (#1663)"
    )
)]
mod starter;

/// Why a saved party member could not be converted into a battler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PartyError {
    /// The encrypted substructures failed checksum validation.
    Substructures(engine::save::PokemonError),
    /// The decoded species, level, or moveset was not battle-ready.
    Battler(battle::BattleError),
    /// The record is an egg or Bad Egg -- never a battler, upstream's own
    /// `SetBattlePartyIds` egg exclusion (`pokeemerald/src/battle_controllers.c:601-602`).
    Egg,
    /// A known move follows an empty (`MOVE_NONE`) slot. Upstream keeps such
    /// a gap as a real, unselectable slot (`MOVE_LIMITATION_ZEROMOVE`,
    /// `pokeemerald/src/battle_util.c:1098`; the move menu skips it,
    /// `pokeemerald/src/battle_controller_player.c:1468`), but the battle
    /// model only holds a contiguous moveset, so decoding it would drop the
    /// later moves and PP on the next save.
    InteriorEmptyMove,
    /// The saved primary-status word is not one the battle model carries --
    /// freeze, bad poison (with or without its counter), several
    /// conditions at once, or bits outside `STATUS1_ANY`. Decoding it as
    /// healthy would let the next save erase the condition, so the record
    /// stays undecoded and its bytes untouched.
    UnsupportedStatus(u32),
}

impl std::fmt::Display for PartyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Substructures(err) => write!(f, "saved party member: {err}"),
            Self::Battler(err) => write!(f, "saved party member: {err}"),
            Self::Egg => write!(f, "saved party member: is an egg"),
            Self::InteriorEmptyMove => {
                write!(
                    f,
                    "saved party member: a known move follows an empty move slot"
                )
            }
            Self::UnsupportedStatus(word) => {
                write!(
                    f,
                    "saved party member: primary status word {word:#x} is not modelled"
                )
            }
        }
    }
}

impl std::error::Error for PartyError {}

impl From<engine::save::PokemonError> for PartyError {
    fn from(err: engine::save::PokemonError) -> Self {
        Self::Substructures(err)
    }
}

impl From<battle::BattleError> for PartyError {
    fn from(err: battle::BattleError) -> Self {
        Self::Battler(err)
    }
}

/// `STATUS1_SLEEP`, `STATUS1_POISON`, `STATUS1_BURN`, `STATUS1_PARALYSIS`,
/// `STATUS1_TOXIC_POISON` and `STATUS1_TOXIC_COUNTER` -- the primary status
/// word's encodings for the conditions `battle::Status1` carries
/// (`pokeemerald/include/constants/battle.h:114-125`). The sleep bits are a
/// three-bit remaining-turn count, not a flag. The toxic counter is bits 8–11.
const STATUS1_SLEEP_MASK: u32 = 7;
const STATUS1_POISON: u32 = 1 << 3;
const STATUS1_BURN: u32 = 1 << 4;
const STATUS1_PARALYSIS: u32 = 1 << 6;
const STATUS1_TOXIC_POISON: u32 = 1 << 7;
const STATUS1_TOXIC_COUNTER_MASK: u32 = 0xF00;

/// Decodes the save record's primary status word (`MON_DATA_STATUS`, copied
/// into the battler by `CopyPlayerMonData`,
/// `pokeemerald/src/battle_controller_player.c:1636-1640`) into the battle
/// model's [`battle::Status1`].
///
/// Exactly one modelled condition decodes; freeze, a toxic counter without
/// `STATUS1_TOXIC_POISON`, any combination, and anything above `STATUS1_ANY`
/// (`battle.h:125`) fail closed rather than being masked into a neighbour.
fn status1_from_word(word: u32) -> Result<battle::Status1, PartyError> {
    if let Some(counter) = toxic_counter(word) {
        return Ok(battle::Status1::BadlyPoisoned { counter });
    }
    match word {
        0 => Ok(battle::Status1::Healthy),
        1..=STATUS1_SLEEP_MASK => Ok(battle::Status1::Asleep {
            // At most seven: the mask above bounds it.
            turns: u8::try_from(word).unwrap_or(u8::MAX),
        }),
        STATUS1_POISON => Ok(battle::Status1::Poisoned),
        STATUS1_BURN => Ok(battle::Status1::Burned),
        STATUS1_PARALYSIS => Ok(battle::Status1::Paralysed),
        unsupported => Err(PartyError::UnsupportedStatus(unsupported)),
    }
}

/// `STATUS1_TOXIC_POISON` with nothing but a four-bit counter beside it.
fn toxic_counter(word: u32) -> Option<u8> {
    if word & !STATUS1_TOXIC_COUNTER_MASK != STATUS1_TOXIC_POISON {
        return None;
    }
    u8::try_from((word & STATUS1_TOXIC_COUNTER_MASK) >> 8).ok()
}

/// Encodes the battler's primary status as the save record's status word
/// (`SetPlayerMonData`'s `MON_DATA_STATUS` write-back,
/// `pokeemerald/src/battle_controller_player.c:1983-1987`).
///
/// A healthy battler files `STATUS1_NONE`, which is how a cure reaches the
/// save.
///
/// # Panics
///
/// Panics if a sleep count exceeds the three bits the word holds; a modelled
/// infliction draws two to five turns, so a larger one is a broken invariant
/// that would otherwise corrupt the neighbouring status bits.
fn status1_to_word(status1: battle::Status1) -> u32 {
    match status1.normalised() {
        battle::Status1::Healthy | battle::Status1::Asleep { turns: 0 } => 0,
        battle::Status1::Paralysed => STATUS1_PARALYSIS,
        battle::Status1::Poisoned => STATUS1_POISON,
        battle::Status1::Burned => STATUS1_BURN,
        battle::Status1::BadlyPoisoned { counter } => {
            assert!(
                u32::from(counter) <= (STATUS1_TOXIC_COUNTER_MASK >> 8),
                "a toxic counter of {counter} does not fit STATUS1_TOXIC_COUNTER's four bits"
            );
            STATUS1_TOXIC_POISON | (u32::from(counter) << 8)
        }
        battle::Status1::Asleep { turns } => {
            let word = u32::from(turns);
            assert!(
                word <= STATUS1_SLEEP_MASK,
                "a sleep count of {turns} does not fit STATUS1_SLEEP's three bits"
            );
            word
        }
    }
}

/// Packs the six five-bit IVs, leaving the egg and ability flags clear.
fn pack_ivs(ivs: Ivs) -> u32 {
    ivs.as_array()
        .iter()
        .enumerate()
        .fold(0u32, |word, (index, value)| {
            word | (u32::from(*value) & IV_FIELD_MASK) << (index * IV_FIELD_WIDTH)
        })
}

fn unpack_ivs(word: u32) -> Ivs {
    let [hp, attack, defense, speed, sp_attack, sp_defense]: [u8; STAT_VALUE_COUNT] =
        std::array::from_fn(|index| {
            u8::try_from((word >> (index * IV_FIELD_WIDTH)) & IV_FIELD_MASK).unwrap_or(0)
        });
    Ivs {
        hp,
        attack,
        defense,
        speed,
        sp_attack,
        sp_defense,
    }
}

/// `PokemonSubstruct2`'s first six bytes, one whole byte per EV, in the same
/// HP/Attack/Defense/Speed/SpAttack/SpDefense order [`pack_ivs`]/
/// [`unpack_ivs`] use, unlike the IVs' packed five-bit fields. Used by
/// [`from_save_pokemon`] to seed [`battle::BattlePokemon::with_evs`] --
/// [`compute_levelled_up_stats`]'s own stat-block recompute reads
/// [`battle::BattlePokemon::evs_at_last_level_up`] instead, not these bytes
/// directly. [`evs_to_substruct2`] is the inverse.
fn evs_from_substruct2(evs_and_condition: &[u8; SUBSTRUCTURE_LEN]) -> battle::Evs {
    let [hp, attack, defense, speed, sp_attack, sp_defense, ..] = *evs_and_condition;
    battle::Evs {
        hp,
        attack,
        defense,
        speed,
        sp_attack,
        sp_defense,
    }
}

/// [`evs_from_substruct2`]'s inverse: packs a [`battle::Evs`]
/// back into `PokemonSubstruct2`'s first six bytes, in the same order. Used
/// by both encoders to file the battler's own (possibly KO-incremented) EVs
/// -- [`to_save_pokemon`] for a mon with no backing record,
/// [`merge_into_save_pokemon`] for one there is.
fn evs_to_substruct2(evs: battle::Evs) -> [u8; 6] {
    evs.as_array()
}

/// Recomputes a changed stat cache EV-aware, fed `mon`'s own IVs and nature
/// alongside `evs`, for a lead whose species or level moved since the
/// reference point each caller compares against.
///
/// `evs` must be [`battle::BattlePokemon::evs_at_last_level_up`], not the
/// live [`battle::BattlePokemon::evs`]: upstream's `CalculateMonStats` only
/// runs inside the level-up sequence that moved `mon` here in the first
/// place, so a KO's `MonGainEVs` bytes gained *after* that level-up must not
/// retroactively inflate the block it cached (regression:
/// `save_time_recompute_uses_the_evs_the_level_up_saw_not_later_gains`).
///
/// A dex mismatch falls back to the battler's existing `0`-EV cache instead
/// of making save conversion panic.
fn compute_levelled_up_stats(dex: &Dex, mon: &BattlePokemon, evs: battle::Evs) -> battle::Stats {
    match dex.species(mon.species()) {
        Ok(base) => battle::compute_stats_with_evs(
            mon.species(),
            base,
            mon.level(),
            mon.nature(),
            mon.ivs(),
            evs,
        ),
        Err(_) => mon.stats(),
    }
}

fn zero_ev_max_hp(dex: &Dex, species: u16, level: u8, mon: &BattlePokemon) -> u32 {
    match dex.species(assets::SpeciesId(species)) {
        Ok(base) => {
            battle::compute_stats_with_evs(
                assets::SpeciesId(species),
                base,
                level,
                mon.nature(),
                mon.ivs(),
                battle::Evs::default(),
            )
            .max_hp
        }
        Err(_) => mon.stats().max_hp,
    }
}

/// Creates a complete save record from a battler and save-format defaults.
pub(crate) fn to_save_pokemon(dex: &Dex, mon: &BattlePokemon) -> Pokemon {
    let mut growth = [0u8; SUBSTRUCTURE_LEN];
    growth[GROWTH_SPECIES].copy_from_slice(&mon.species().0.to_le_bytes());
    growth[GROWTH_HELD_ITEM].copy_from_slice(&mon.held_item().0.to_le_bytes());
    growth[GROWTH_EXPERIENCE].copy_from_slice(&mon.experience().to_le_bytes());
    growth[GROWTH_PP_BONUSES] = mon.pp_bonuses().bits();
    growth[GROWTH_FRIENDSHIP] = dex
        .species(mon.species())
        .map_or(0, |species| species.base_friendship);

    let mut misc = [0u8; SUBSTRUCTURE_LEN];
    let iv_word = pack_ivs(mon.ivs()) | (u32::from(mon.ability_slot()) << ABILITY_SLOT_SHIFT);
    misc[MISC_IV_WORD].copy_from_slice(&iv_word.to_le_bytes());
    // Met level is the level the record was first built at, not the live
    // level: `CreateBoxMon` stamps it once and later saves copy it unchanged
    // (`pokeemerald/src/pokemon.c:2259`). Met location and OT gender stay
    // clear; see the module docs.
    let origins = (u16::from(mon.created_at_level()) & MET_LEVEL_MASK)
        | (VERSION_EMERALD << MET_GAME_SHIFT)
        | (ITEM_POKE_BALL << POKE_BALL_SHIFT);
    misc[MISC_MET_DATA].copy_from_slice(&origins.to_le_bytes());

    // `PokemonSubstruct2` -- the mon's own EVs (`0` for a mon
    // nothing has ever called `with_evs`/`gain_evs` on, matching `CreateMon`'s
    // own default) in the first six bytes, contest condition zero in the
    // trailing six (module docs).
    let mut evs_and_condition = [0u8; SUBSTRUCTURE_LEN];
    evs_and_condition[0..6].copy_from_slice(&evs_to_substruct2(mon.evs()));

    let mut box_data = BoxPokemon::new(mon.personality(), mon.original_trainer_id());
    box_data.set_substructures(&PokemonSubstructures {
        growth,
        attacks: encode_attacks(mon),
        evs_and_condition,
        misc,
    });
    // Unencrypted header bytes `set_substructures` never touches --
    // `CreateBoxMon` stamps both unconditionally
    // (`pokeemerald/src/pokemon.c:2251-2252`). OT name stays clear: it needs
    // the current save's player identity, which this function does not have
    // (module docs).
    box_data.set_nickname(create_mon_nickname(mon.species()));
    box_data.set_language(LANGUAGE_ENGLISH);
    // `SetBoxMonData(MON_DATA_SPECIES)` raises `hasSpecies` for a nonzero
    // species (`pokeemerald/src/pokemon.c:4220-4227`).
    box_data.set_has_species(mon.species().0 != SPECIES_NONE);

    let mut record = Pokemon {
        box_data,
        mail: MAIL_NONE,
        status: status1_to_word(mon.status1()),
        ..Pokemon::default()
    };
    // Recomputed only when `mon` has levelled up since `BattlePokemon::new`
    // built it; see `compute_levelled_up_stats`'s own doc for which EVs
    // that recompute uses, and why.
    //
    // The unchanged-level branch recomputes from the raw dex row at zero EVs
    // rather than filing `mon.stats()`: for Deoxys the live cache carries the
    // single-player Speed Forme substitution, which upstream's
    // `CalculateMonStats` never writes into the stored block.
    let stats = if mon.level() == mon.created_at_level() {
        compute_levelled_up_stats(dex, mon, battle::Evs::default())
    } else {
        compute_levelled_up_stats(dex, mon, mon.evs_at_last_level_up())
    };
    overlay_battle_stats(&mut record, mon, stats);
    // Translate `record.hp` across whatever gap the branch above opened -- a
    // no-op when it did not (`stats == mon.stats()` there), so safe to run
    // unconditionally.
    let gap = clamp_i32(stats.max_hp.saturating_sub(mon.stats().max_hp));
    overlay_current_hp_with_hidden_points(&mut record, mon, gap);
    record
}

/// [`to_save_pokemon`]'s nickname: `species`'s display name, game-text
/// encoded and copied into the fixed-width header field exactly as
/// `SetBoxMonData(MON_DATA_NICKNAME, speciesName)` does
/// (`pokeemerald/src/pokemon.c:2249-2250`, `GetSpeciesName`). A species id or
/// name this port cannot resolve or encode falls back to an all-clear
/// nickname -- the record's own already-zeroed bytes -- rather than panic.
fn create_mon_nickname(species: assets::SpeciesId) -> [u8; BOX_NICKNAME_LEN] {
    let mut nickname = [0u8; BOX_NICKNAME_LEN];
    if let Ok(name) = assets::SpeciesNames::new().name(species) {
        if let Ok(encoded) = engine::text::encode_str(name) {
            let copy_len = encoded.len().min(nickname.len());
            nickname[..copy_len].copy_from_slice(&encoded[..copy_len]);
        }
    }
    nickname
}

fn encode_attacks(mon: &BattlePokemon) -> [u8; SUBSTRUCTURE_LEN] {
    let mut attacks = [0u8; SUBSTRUCTURE_LEN];
    let (move_ids, remaining_pp) = attacks.split_at_mut(ATTACK_PP_OFFSET);
    for ((move_id, pp), slot) in move_ids
        .chunks_exact_mut(MOVE_ID_WIDTH)
        .zip(remaining_pp)
        .zip(mon.moves())
    {
        move_id.copy_from_slice(&slot.move_id.0.to_le_bytes());
        *pp = slot.pp;
    }
    attacks
}

/// The party record's unencrypted tail -- level, current HP, and the derived
/// stat block -- from `stats`, either [`BattlePokemon::stats`]'s live `0`-EV
/// cache or [`compute_levelled_up_stats`]'s EV-aware recompute. The plain
/// `current_hp` this writes into `record.hp` is never the last word in
/// either caller: both re-overlay [`overlay_current_hp_with_hidden_points`]
/// on top, so the load-clamp translation lands against whichever maximum was
/// just filed here.
fn overlay_battle_stats(record: &mut Pokemon, mon: &BattlePokemon, stats: battle::Stats) {
    record.level = mon.level();
    record.hp = clamp_u16(mon.current_hp());
    record.max_hp = clamp_u16(stats.max_hp);
    record.attack = clamp_u16(stats.attack);
    record.defense = clamp_u16(stats.defense);
    record.speed = clamp_u16(stats.speed);
    record.special_attack = clamp_u16(stats.sp_attack);
    record.special_defense = clamp_u16(stats.sp_defense);
}

/// Restores HP hidden by the zero-EV load clamp without reviving a fainted
/// mon: the points of a stored `hp` above the model's maximum were hidden
/// from the session, so they are added back onto the live number, capped at
/// `record.max_hp`, floored at `1` for a live battler. Used for both a
/// *retained* stat block and a freshly *recomputed* one -- the
/// caller always writes `record.max_hp` first, so the cap is whichever block
/// is actually being filed.
///
/// The offset is signed (negative after a level-up whose EV gap shrank),
/// measured once per session -- at load ([`hp_hidden_by_load`]) or fresh by
/// [`to_save_pokemon`] -- and carried, never re-derived here: saving twice
/// must file the same bytes.
fn overlay_current_hp_with_hidden_points(
    record: &mut Pokemon,
    mon: &BattlePokemon,
    hp_hidden_by_load: i32,
) {
    let live_hp = clamp_u16(mon.current_hp());
    record.hp = if live_hp == 0 {
        0
    } else {
        let translated_hp = i64::from(live_hp) + i64::from(hp_hidden_by_load);
        u16::try_from(translated_hp.max(1).min(i64::from(record.max_hp))).unwrap_or(record.max_hp)
    };
}

/// Enforces Shedinja's invariant maximum without refreshing its retained stats.
///
/// `CalculateMonStats` pins Shedinja's maximum HP to one
/// (`pokeemerald/src/pokemon.c:2845-2848`), but save/load does not recalculate
/// the other five cached stats.
fn normalize_retained_shedinja_max_hp(
    record: &mut Pokemon,
    mon: &BattlePokemon,
    hp_hidden_by_load: &mut i32,
) {
    if mon.species() != battle::SPECIES_SHEDINJA {
        return;
    }
    let invariant_max_hp = mon.stats().max_hp;
    let removed_points = clamp_i32(u32::from(record.max_hp).saturating_sub(invariant_max_hp));
    record.max_hp = clamp_u16(invariant_max_hp);
    *hp_hidden_by_load = hp_hidden_by_load.saturating_sub(removed_points);
}

/// Measures current-HP points hidden when a saved mon entered the zero-EV model.
///
/// The floor uses the record's stored level because experience reconciliation
/// may already have changed the battler's level.
pub(crate) fn hp_hidden_by_load(dex: &Dex, stored: &Pokemon, lead: &BattlePokemon) -> i32 {
    let stored_level_floor = zero_ev_max_hp(dex, lead.species().0, stored.level, lead);
    i32::from(stored.hp.saturating_sub(clamp_u16(stored_level_floor)))
}

/// Overlays a battler onto the save record from which it was loaded.
///
/// Writes only the fields the battler owns; every retained field stays as
/// the save wrote it, except EVs, which the merge writes back from
/// [`battle::BattlePokemon::evs`].
///
/// The cached stat block is kept when species and level are unchanged and
/// recomputed EV-aware otherwise ([`compute_levelled_up_stats`]); current
/// HP is translated across the load clamp against whichever block is
/// filed, with the session offset rebased on the recompute branch
/// ([`zero_ev_max_hp`]). A record that cannot safely retain bytes falls
/// back to [`to_save_pokemon`] ([`backing_substructures`]), seeding the
/// offset from the fresh gap it opens.
pub(crate) fn merge_into_save_pokemon(
    dex: &Dex,
    mon: &BattlePokemon,
    base: &Pokemon,
    hp_hidden_by_load: &mut i32,
) -> Pokemon {
    let mut substructures = match backing_substructures(mon, base) {
        Ok(substructures) => substructures,
        Err(reason) => {
            eprintln!(
                "save: party slot 0 {reason} -- writing a record built from the battler \
                 alone: nickname, language, met level/game/ball, and friendship take \
                 CreateMon's own default, but OT name, met location, and OT gender stay \
                 clear -- this path has no save identity or map to draw them from"
            );
            let record = to_save_pokemon(dex, mon);
            // Carried forward, not zeroed (this function's own doc comment
            // covers why). Read back off `record` rather than recomputed,
            // matching exactly what `to_save_pokemon` just filed.
            *hp_hidden_by_load =
                clamp_i32(u32::from(record.max_hp).saturating_sub(mon.stats().max_hp));
            return record;
        }
    };

    let stored_species = read_u16(&substructures.growth, GROWTH_SPECIES);
    let retained_stat_block_is_current =
        stored_species == mon.species().0 && base.level == mon.level();

    substructures.growth[GROWTH_SPECIES].copy_from_slice(&mon.species().0.to_le_bytes());
    substructures.growth[GROWTH_EXPERIENCE].copy_from_slice(&mon.experience().to_le_bytes());
    substructures.growth[GROWTH_PP_BONUSES] = mon.pp_bonuses().bits();
    substructures.attacks = encode_attacks(mon);

    let retained_egg_flag = read_u32(&substructures.misc, MISC_IV_WORD) & IS_EGG_BIT;
    let merged_iv_word = pack_ivs(mon.ivs())
        | retained_egg_flag
        | (u32::from(mon.ability_slot()) << ABILITY_SLOT_SHIFT);
    substructures.misc[MISC_IV_WORD].copy_from_slice(&merged_iv_word.to_le_bytes());

    // `PokemonSubstruct2`'s first six bytes -- the battler's own live EVs,
    // written unconditionally regardless of the stat-block branch below (see
    // `compute_levelled_up_stats`'s own doc for why the two differ). Contest
    // condition, the substructure's other half, has no home in
    // `battle::BattlePokemon` and stays exactly as `base` held it.
    substructures.evs_and_condition[0..6].copy_from_slice(&evs_to_substruct2(mon.evs()));

    let mut merged = *base;
    merged.status = status1_to_word(mon.status1());
    merged.box_data.set_substructures(&substructures);
    // Re-derives `hasSpecies` alongside the growth species this branch
    // re-files every save (`pokeemerald/src/pokemon.c:4220-4227`).
    merged
        .box_data
        .set_has_species(mon.species().0 != SPECIES_NONE);

    if retained_stat_block_is_current {
        normalize_retained_shedinja_max_hp(&mut merged, mon, hp_hidden_by_load);
    } else {
        let old_zero_ev_max_hp = zero_ev_max_hp(dex, stored_species, base.level, mon);
        let old_ev_bonus = clamp_i32(u32::from(base.max_hp).saturating_sub(old_zero_ev_max_hp));

        let recomputed_stats = compute_levelled_up_stats(dex, mon, mon.evs_at_last_level_up());
        overlay_battle_stats(&mut merged, mon, recomputed_stats);
        let new_ev_bonus = clamp_i32(u32::from(merged.max_hp).saturating_sub(mon.stats().max_hp));

        *hp_hidden_by_load = hp_hidden_by_load
            .saturating_add(new_ev_bonus)
            .saturating_sub(old_ev_bonus);
    }

    overlay_current_hp_with_hidden_points(&mut merged, mon, *hp_hidden_by_load);
    merged
}

fn backing_substructures(
    mon: &BattlePokemon,
    base: &Pokemon,
) -> Result<PokemonSubstructures, NotTheBattlersRecord> {
    if base.box_data.personality() != mon.personality()
        || base.box_data.ot_id() != mon.original_trainer_id()
    {
        return Err(NotTheBattlersRecord::DifferentPokemon);
    }
    let substructures = base
        .box_data
        .substructures()
        .map_err(|_| NotTheBattlersRecord::ChecksumFailed)?;
    if read_u16(&substructures.growth, GROWTH_SPECIES) == SPECIES_NONE {
        return Err(NotTheBattlersRecord::Empty);
    }
    Ok(substructures)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotTheBattlersRecord {
    DifferentPokemon,
    ChecksumFailed,
    Empty,
}

impl core::fmt::Display for NotTheBattlersRecord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::DifferentPokemon => "holds a different Pokémon (personality or OT id)",
            Self::ChecksumFailed => "failed its own checksum",
            Self::Empty => "is empty",
        })
    }
}

/// Converts a validated save record into a battle-ready party member --
/// `LoadPlayerParty`'s per-mon half (`src/load_save.c:170-178`).
///
/// Stats are recomputed from species/level/nature/IVs, at `0` EVs, by
/// [`battle::BattlePokemon::new`]; the record's own EVs
/// ([`evs_from_substruct2`]) are adopted right after, through
/// [`battle::BattlePokemon::with_evs`], which leaves that `0`-EV stat block
/// alone except for Deoxys, whose five Speed Forme stats take the adopted
/// EVs as `GetDeoxysStat` does (see [`battle::pokemon::evs`] for why the
/// rest stay `0`-EV). Accumulated experience
/// is wound back to the saved value through
/// [`battle::BattlePokemon::reconcile_saved_experience`];
/// current HP and each move slot's PP are then wound back to the saved
/// values through the remaining mutations that preserve that type's
/// invariants ([`battle::BattlePokemon::apply_damage`] /
/// [`battle::BattlePokemon::deduct_pp`]) -- never by reaching past them.
///
/// # Errors
///
/// Returns [`PartyError`] when the secure checksum or battler fields are
/// invalid, or a known move follows an empty move slot.
pub(crate) fn from_save_pokemon(dex: &Dex, saved: &Pokemon) -> Result<BattlePokemon, PartyError> {
    let substructures = saved.box_data.substructures()?;
    let status1 = status1_from_word(saved.status)?;
    let species = assets::SpeciesId(read_u16(&substructures.growth, GROWTH_SPECIES));
    let iv_word = read_u32(&substructures.misc, MISC_IV_WORD);
    let ability_slot = u8::from(iv_word >> ABILITY_SLOT_SHIFT != 0);

    let (move_ids, remaining_pp) = substructures.attacks.split_at(ATTACK_PP_OFFSET);
    let slot_moves: Vec<assets::MoveId> = move_ids
        .chunks_exact(MOVE_ID_WIDTH)
        .map(|bytes| assets::MoveId(u16::from_le_bytes([bytes[0], bytes[1]])))
        .collect();
    let known_moves = slot_moves
        .iter()
        .take_while(|move_id| **move_id != battle::MOVE_NONE)
        .count();
    if slot_moves[known_moves..]
        .iter()
        .any(|move_id| *move_id != battle::MOVE_NONE)
    {
        return Err(PartyError::InteriorEmptyMove);
    }
    let move_ids = slot_moves[..known_moves].to_vec();

    // Adopted before any PP is wound back: it is what each slot's *capacity*
    // is, so the spend below counts down from the PP-Up-adjusted maximum
    // rather than from base PP. Every byte is legal (two-bit fields cannot
    // overflow), so there is nothing to screen -- `battle::PpBonuses`' own
    // docs.
    let pp_bonuses = battle::PpBonuses::from_bits(substructures.growth[GROWTH_PP_BONUSES]);

    // `PokemonSubstruct2`'s first six bytes -- this record's own EVs,
    // adopted through `with_evs` below so the battler carries them
    // for the merge's own save-time recompute to read back later. Adopting
    // them touches only Deoxys's Speed Forme stats, which the level-up
    // recompute in `reconcile_saved_experience` below re-derives from the
    // same EVs, so the order of the two calls does not matter.
    let evs = evs_from_substruct2(&substructures.evs_and_condition);

    // Carried as stored, even an id outside the item table: the battle crate
    // decides whether a battle can play it (`ensure_held_item_playable`), so a
    // loaded party never drops an item here.
    let held_item = assets::items::ItemId(read_u16(&substructures.growth, GROWTH_HELD_ITEM));

    let mut mon = BattlePokemon::new(
        dex,
        species,
        saved.level,
        unpack_ivs(iv_word),
        saved.box_data.personality(),
        move_ids,
    )?
    .with_original_trainer_id(saved.box_data.ot_id())
    .with_ability_slot(ability_slot)
    .with_pp_bonuses(dex, pp_bonuses)?
    .with_evs(evs)
    .with_held_item(held_item);

    let saved_experience = read_u32(&substructures.growth, GROWTH_EXPERIENCE);
    // Match `CalculateMonStats`: derive level from experience without invoking
    // the separate move-learning path (`pokeemerald/src/pokemon.c:2823-2843`).
    mon.reconcile_saved_experience(saved_experience);

    let saved_damage = mon.stats().max_hp.saturating_sub(u32::from(saved.hp));
    mon.apply_damage(saved_damage);
    mon.set_status1(status1);

    for (index, saved_pp) in remaining_pp.iter().copied().take(known_moves).enumerate() {
        let pp_spent = mon.moves()[index].pp.saturating_sub(saved_pp);
        for _ in 0..pp_spent {
            mon.deduct_pp(index)?;
        }
    }

    Ok(mon)
}

/// Whether `record` is an egg or Bad Egg: the unencrypted header's
/// `isBadEgg` bit, or the secure-region IV word's egg flag. Checked ahead
/// of decode since [`from_save_pokemon`] itself does not reject either, and
/// the header bit survives a valid secure checksum.
fn record_is_egg_or_bad_egg(record: &Pokemon) -> bool {
    record.box_data.is_bad_egg()
        || record.box_data.substructures().is_ok_and(|substructures| {
            read_u32(&substructures.misc, MISC_IV_WORD) & IS_EGG_BIT != 0
        })
}

/// Species and personality of party slot 0 when it is a non-egg holder of
/// `ability`: `CreateWildMon` and `PickWildMonNature` check `gPlayerParty[0]` --
/// not an egg by the unencrypted header's `isEgg` bit (`MON_DATA_SANITY_IS_EGG`,
/// `pokeemerald/src/pokemon.c:3805-3806`), ability `ability`
/// (`pokeemerald/src/wild_encounter.c:367-372,395-398`) -- whether or not that slot
/// is fainted or is the battle lead. The ability is the stored slot's entry in
/// the species table, as `GetAbilityBySpecies` reads it (`abilities[abilityNum]`,
/// `pokeemerald/src/pokemon.c:4546-4553`), so a slot-1 record of a single-ability
/// species has no ability rather than falling back to the primary. `None` for an
/// empty party, an egg or Bad Egg, an undecodable record, or any other ability.
fn slot_zero_ability_holder(
    dex: &Dex,
    party: &[Pokemon],
    ability: assets::AbilityId,
) -> Option<(assets::SpeciesId, u32)> {
    let slot_zero = party.first()?;
    if slot_zero.box_data.is_egg_header_flag() || slot_zero.box_data.is_bad_egg() {
        return None;
    }
    // Only egg status, ability and personality matter upstream, so read just those
    // fields: a full battler decode would also reject a malformed moveset, silently
    // dropping a lead whose moves `from_save_pokemon` refuses.
    let substructures = slot_zero.box_data.substructures().ok()?;
    let species = assets::SpeciesId(read_u16(&substructures.growth, GROWTH_SPECIES));
    let iv_word = read_u32(&substructures.misc, MISC_IV_WORD);
    let ability_slot = usize::from(iv_word >> ABILITY_SLOT_SHIFT != 0);
    (dex.species(species).ok()?.abilities[ability_slot] == ability)
        .then(|| (species, slot_zero.box_data.personality()))
}

/// The nature party slot 0 hands a wild Pokémon through Synchronize
/// (`PickWildMonNature`); see [`slot_zero_ability_holder`] for which slot-0
/// records count.
pub(crate) fn slot_zero_synchronize_nature(dex: &Dex, party: &[Pokemon]) -> Option<battle::Nature> {
    let (species, personality) =
        slot_zero_ability_holder(dex, party, assets::AbilityId::SYNCHRONIZE)?;
    // The probe's level, IVs and move do not affect its nature.
    BattlePokemon::new(
        dex,
        species,
        1,
        Ivs::default(),
        personality,
        vec![assets::MoveId::POUND],
    )
    .ok()
    .map(|probe| probe.nature())
}

/// The gender party slot 0 hands a wild Pokémon through Cute Charm
/// (`CreateWildMon`): the opposite of its own, which its species' ratio and
/// personality fix; see [`slot_zero_ability_holder`] for which slot-0 records
/// count.
pub(crate) fn slot_zero_cute_charm_wild_gender(
    dex: &Dex,
    party: &[Pokemon],
) -> Option<battle::Gender> {
    let (species, personality) =
        slot_zero_ability_holder(dex, party, assets::AbilityId::CUTE_CHARM)?;
    let ratio = dex.species(species).ok()?.gender_ratio;
    Some(battle::Gender::of_personality(ratio, personality).cute_charm_wild_gender())
}

/// [`slot_zero_cute_charm_wild_gender`] for an unsaved party whose whole
/// membership is the live battler `lead`.
pub(crate) fn live_lead_cute_charm_wild_gender(
    dex: &Dex,
    lead: &BattlePokemon,
) -> Option<battle::Gender> {
    if lead.ability() != assets::AbilityId::CUTE_CHARM {
        return None;
    }
    let ratio = dex.species(lead.species()).ok()?.gender_ratio;
    Some(battle::Gender::of_personality(ratio, lead.personality()).cute_charm_wild_gender())
}

/// Selects which saved slot is the active battler on continue: the first
/// non-egg, non-Bad-Egg, non-fainted slot in `party` -- `SetBattlePartyIds`'s
/// player-side scan (`pokeemerald/src/battle_controllers.c:585-606`,
/// called by `InitBattleControllers` at `:97`). Falls back to slot 0's own
/// decode when nothing qualifies, fainted or not -- but never to an egg
/// (`PartyError::Egg`): an egg is not a battler upstream either, so an
/// egg-only party fails closed the same way an all-fainted one does.
///
/// # Errors
///
/// Returns slot 0's [`PartyError`] when nothing qualifies and slot 0 is
/// itself unusable, whether that is a decode failure or an egg.
///
/// # Panics
///
/// Panics if `party` is empty; callers only pass a nonzero-count slice.
pub(crate) fn select_active_battler(
    dex: &Dex,
    party: &[Pokemon],
) -> Result<(usize, BattlePokemon), PartyError> {
    for (slot, record) in party.iter().enumerate() {
        if record_is_egg_or_bad_egg(record) {
            continue;
        }
        match from_save_pokemon(dex, record) {
            Ok(mon) if !mon.is_fainted() => return Ok((slot, mon)),
            Ok(_) => {}
            // Logged, not just discarded: a later slot winning the scan
            // must not hide that an earlier one's bytes would not decode.
            Err(err) => eprintln!("continue: slot {slot}'s record {err} -- skipped"),
        }
    }
    if record_is_egg_or_bad_egg(&party[0]) {
        return Err(PartyError::Egg);
    }
    from_save_pokemon(dex, &party[0]).map(|mon| (0, mon))
}

/// The loaded lead: the one owner of the selected party slot, its live
/// [`BattlePokemon`], the backing save record, and the signed count of
/// current-HP points the zero-EV load hid.
///
/// The three cannot drift apart: [`Self::load`] picks the record through
/// [`select_active_battler`] and measures the offset against that same
/// record, and [`Self::merge_and_save`] replaces the owned record and rebases
/// the owned offset in one step -- the single reconciliation boundary
/// `Cmd_getexp` keeps (`pokeemerald/src/battle_script_commands.c:3465-3478`).
#[derive(Debug)]
pub(crate) struct LoadedLead {
    slot: usize,
    /// `None` only while [`Self::take_battler`] has lent it to a battle.
    battler: Option<BattlePokemon>,
    record: Pokemon,
    hidden_hp_offset: i32,
}

impl LoadedLead {
    /// Selects the active member of `party` and measures what its load hid.
    ///
    /// # Errors
    ///
    /// Returns [`select_active_battler`]'s [`PartyError`].
    ///
    /// # Panics
    ///
    /// Panics if `party` is empty, as [`select_active_battler`] does.
    pub(crate) fn load(dex: &Dex, party: &[Pokemon]) -> Result<Self, PartyError> {
        let (slot, battler) = select_active_battler(dex, party)?;
        let record = party[slot];
        let hidden_hp_offset = hp_hidden_by_load(dex, &record, &battler);
        Ok(Self {
            slot,
            battler: Some(battler),
            record,
            hidden_hp_offset,
        })
    }

    /// Adopts a member just created into slot 0: its battler and the record
    /// encoded from it, with the hidden-HP offset measured against that record.
    pub(crate) fn created(dex: &Dex, battler: BattlePokemon, record: Pokemon) -> Self {
        let hidden_hp_offset = hp_hidden_by_load(dex, &record, &battler);
        Self {
            slot: 0,
            battler: Some(battler),
            record,
            hidden_hp_offset,
        }
    }

    /// Adopts `battler` as the member whose saved record is `record` in `slot`,
    /// measuring the offset against that same pair.
    #[cfg(test)]
    pub(crate) fn backed_for_test(
        dex: &Dex,
        slot: usize,
        battler: BattlePokemon,
        record: Pokemon,
    ) -> Self {
        let hidden_hp_offset = hp_hidden_by_load(dex, &record, &battler);
        Self {
            slot,
            battler: Some(battler),
            record,
            hidden_hp_offset,
        }
    }

    /// The selected party slot.
    pub(crate) fn slot(&self) -> usize {
        self.slot
    }

    /// The backing save record as last merged.
    pub(crate) fn record(&self) -> &Pokemon {
        &self.record
    }

    /// The signed hidden-HP offset carried into the next merge.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the overworld never reads the offset")
    )]
    pub(crate) fn hidden_hp_offset(&self) -> i32 {
        self.hidden_hp_offset
    }

    /// Whether [`Self::take_battler`] has lent the battler out and it has not
    /// come back through [`Self::restore_battler`].
    pub(crate) fn is_lent(&self) -> bool {
        self.battler.is_none()
    }

    /// # Panics
    ///
    /// Panics if the battler is lent out through [`Self::take_battler`].
    pub(crate) fn battler(&self) -> &BattlePokemon {
        self.battler.as_ref().expect(LENT_OUT)
    }

    /// # Panics
    ///
    /// Panics if the battler is lent out through [`Self::take_battler`].
    pub(crate) fn battler_mut(&mut self) -> &mut BattlePokemon {
        self.battler.as_mut().expect(LENT_OUT)
    }

    /// Lends the battler to a battle; hand it back with
    /// [`Self::restore_battler`].
    ///
    /// # Panics
    ///
    /// Panics if the battler is already lent out.
    pub(crate) fn take_battler(&mut self) -> BattlePokemon {
        self.battler.take().expect(LENT_OUT)
    }

    /// Returns the lent battler.
    ///
    /// # Panics
    ///
    /// Panics if the battler was not lent out, or is not the same mon as the
    /// owned record (personality and original trainer id).
    pub(crate) fn restore_battler(&mut self, battler: BattlePokemon) {
        assert!(
            self.battler.is_none(),
            "the loaded lead's battler was never lent out"
        );
        assert!(
            battler.personality() == self.record.box_data.personality()
                && battler.original_trainer_id() == self.record.box_data.ot_id(),
            "a different party member cannot take the loaded lead's place"
        );
        self.battler = Some(battler);
    }

    /// Merges the battler onto the owned record ([`merge_into_save_pokemon`]),
    /// keeps the result as the owned record, rebases the owned offset, and
    /// returns a copy for filing into the save.
    ///
    /// # Panics
    ///
    /// Panics if the battler is lent out through [`Self::take_battler`].
    pub(crate) fn merge_and_save(&mut self, dex: &Dex) -> Pokemon {
        let battler = self.battler.as_ref().expect(LENT_OUT);
        self.record =
            merge_into_save_pokemon(dex, battler, &self.record, &mut self.hidden_hp_offset);
        self.record
    }

    /// Heals the whole lead (`HealPlayerParty`): clears status and restores
    /// HP and PP, re-measures the offset against the healed record, and
    /// merges. When the PP heal fails the record keeps its cleared status
    /// and full HP but is not merged, and the error is returned.
    ///
    /// `player_party_count` is the save's stored count. A lead at or beyond a
    /// nonzero count is not an occupied slot, which `HealPlayerParty` skips
    /// (`pokeemerald/src/script_pokemon_util.c:30-58`): it is not healed, only
    /// re-measured and merged so the re-scan keeps its session state. A zero
    /// count still heals, so an unsaved new game keeps slot 0 (#800).
    ///
    /// # Errors
    ///
    /// Returns the battler's PP-restore error.
    ///
    /// # Panics
    ///
    /// Panics if the battler is lent out through [`Self::take_battler`].
    pub(crate) fn heal_whole_lead(
        &mut self,
        dex: &Dex,
        player_party_count: u8,
    ) -> Result<(), battle::BattleError> {
        let stored_count = usize::from(player_party_count);
        if stored_count != 0 && self.slot >= stored_count {
            let battler = self.battler.as_ref().expect(LENT_OUT);
            self.hidden_hp_offset = hp_hidden_by_load(dex, &self.record, battler);
            self.merge_and_save(dex);
            return Ok(());
        }
        self.record.status = 0;
        self.record.hp = self.record.max_hp;
        let battler = self.battler.as_mut().expect(LENT_OUT);
        battler.heal(dex)?;
        self.hidden_hp_offset = hp_hidden_by_load(dex, &self.record, battler);
        self.merge_and_save(dex);
        Ok(())
    }
}

const LENT_OUT: &str = "the loaded lead's battler is lent out to a battle";

fn read_u16(bytes: &[u8], range: Range<usize>) -> u16 {
    let field = bytes[range]
        .try_into()
        .expect("a u16 save field has exactly two bytes");
    u16::from_le_bytes(field)
}

fn read_u32(bytes: &[u8], range: Range<usize>) -> u32 {
    let field = bytes[range]
        .try_into()
        .expect("a u32 save field has exactly four bytes");
    u32::from_le_bytes(field)
}

fn clamp_u16(value: u32) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

fn clamp_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests;
