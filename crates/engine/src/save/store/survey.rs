//! Per-slot survey and cross-slot verdict for [`super::SaveStore::load`]:
//! what each slot's 14 physical sectors say, and which generation wins.

use super::super::sector::{Sector, SECTOR_SIGNATURE};
use super::{
    sector_payload_len, SaveStatus, NUM_SAVE_SLOTS, NUM_SAVE_SLOTS_U32, NUM_SECTORS_PER_SLOT,
    NUM_SECTORS_PER_SLOT_U16, PKMN_STORAGE_CHUNKS, SECTOR_ID_PKMN_STORAGE_START,
};

/// The rotation of the same-slot predecessor a rotation-zero write lays
/// over: two saves, so two rotations, behind.
const TORN_WRITE_PREDECESSOR_ROTATION: usize = NUM_SECTORS_PER_SLOT - NUM_SAVE_SLOTS;
const LEGACY_ERA_IDS_MASK: u32 = (1 << SECTOR_ID_PKMN_STORAGE_START) - 1;
/// The nine opaque `PokemonStorage` sector ids (5-13) as a bitmask.
const PKMN_STORAGE_IDS_MASK: u32 =
    ((1u32 << PKMN_STORAGE_CHUNKS) - 1) << SECTOR_ID_PKMN_STORAGE_START;

/// Ids 0-4: the sectors `GetSaveValidStatus` requires of a generation.
const SAVE_BLOCK_IDS: u32 = LEGACY_ERA_IDS_MASK;
const PKMN_STORAGE_FIRST_ID: usize = SECTOR_ID_PKMN_STORAGE_START as usize;

/// Compares counters from adjacent save generations, including the sole
/// `u32::MAX` to zero wrap.
#[must_use]
pub(super) fn second_counter_is_newer(first: u32, second: u32) -> bool {
    match (first, second) {
        (u32::MAX, 0) => true,
        (0, u32::MAX) => false,
        _ => first < second,
    }
}

/// Whether `older` precedes `newer` as a wrapping serial number (RFC 1982),
/// for any gap; [`second_counter_is_newer`] assumes adjacent generations.
#[must_use]
pub(super) fn older_generation_precedes(older: u32, newer: u32) -> bool {
    let delta = newer.wrapping_sub(older);
    delta != 0 && delta < (1 << 31)
}

/// The value all but at most one of `values` share, held by a strict
/// majority of them.
pub(super) fn one_outlier_consensus<T: Copy + Eq>(values: &[T]) -> Option<T> {
    values.iter().copied().find(|&candidate| {
        let agreeing = values.iter().filter(|&&v| v == candidate).count();
        agreeing + 1 >= values.len() && 2 * agreeing > values.len()
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SlotIntegrity {
    Empty,
    /// All 14 sectors validate, or the slot holds a five-sector generation
    /// (see [`super::SaveStore::scan_slot`]).
    Ok,
    Error,
}

pub(super) struct SlotScan {
    pub(super) integrity: SlotIntegrity,
    pub(super) counter: u32,
    /// Whether an `Ok` integrity came from the legacy five-sector fallback
    /// rather than all 14 sectors validating. See [`resolve`].
    pub(super) legacy: bool,
    /// Generation of a complete storage set (ids 5-13) in this slot,
    /// whatever the slot's own integrity (`SlotSurvey::storage_generation`).
    pub(super) storage_counter: Option<u32>,
    /// The footer counter a strict majority of the slot's checksum-valid
    /// sectors carry. Unlike `counter`, which is the last valid sector's (as
    /// upstream reads it), a minority of damaged footers cannot move it.
    pub(super) majority_counter: Option<u32>,
}

/// The physical slots [`super::SaveStore::load`] copies each half of its result
/// from.
pub(super) struct Resolution {
    pub(super) status: SaveStatus,
    /// The adopted generation number, which picks the next save's slot and
    /// the slot fields copy from.
    pub(super) counter: u32,
    /// Scanned slot index donating `PokemonStorage` when the adopted
    /// generation carries none ([`storage_donor`]).
    pub(super) storage_from_slot: Option<usize>,
    /// Whether `counter`'s slot is a five-sector head, so its positions 5-13
    /// are never read as progress.
    pub(super) legacy: bool,
}

/// The per-position observations [`super::SaveStore::scan_slot`] accumulates over a
/// slot's 14 physical sectors, and the verdict it draws from them.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent tally over one scan loop, not caller configuration; \
              grouping them into sub-structs would only rename the tallies"
)]
pub(super) struct SlotSurvey {
    signature_valid: bool,
    valid_ids: u32,
    counter: u32,
    head_valid_ids: u32,
    head_is_identity: bool,
    tail_signature_seen: bool,
    tail_unusable_count: usize,
    tail_unusable_at_first_tail_position: bool,
    /// The footer counter of each checksum-valid tail sector, in position
    /// order; only the first `tail_valid_count` entries are meaningful.
    tail_counters: [u32; PKMN_STORAGE_CHUNKS],
    tail_valid_count: usize,
    /// The rotation each checksum-valid tail sector's id/position pairing
    /// implies, parallel to `tail_counters`.
    tail_rotations: [usize; PKMN_STORAGE_CHUNKS],
    storage_valid_ids: u32,
    /// The footer counter of each checksum-valid storage id (5-13), indexed
    /// by chunk; meaningful only for the ids set in `storage_valid_ids`.
    storage_counters: [u32; PKMN_STORAGE_CHUNKS],
    storage_ids_unique: bool,
    /// The rotation every checksum-valid storage id's position implies,
    /// while they all imply the same one.
    storage_rotation: Option<usize>,
    storage_rotation_coherent: bool,
    /// Footer counter and id of every valid save-block sector (ids 0-4) in
    /// position order; `save_block_count` entries are meaningful.
    save_block_counters: [u32; NUM_SECTORS_PER_SLOT],
    save_block_ids: [u16; NUM_SECTORS_PER_SLOT],
    save_block_count: usize,
    legacy_counter: Option<u32>,
    legacy_consistent: bool,
}

impl SlotSurvey {
    pub(super) fn new() -> Self {
        Self {
            signature_valid: false,
            valid_ids: 0,
            counter: 0,
            head_valid_ids: 0,
            head_is_identity: true,
            tail_signature_seen: false,
            tail_unusable_count: 0,
            tail_unusable_at_first_tail_position: false,
            tail_counters: [0; PKMN_STORAGE_CHUNKS],
            tail_valid_count: 0,
            tail_rotations: [0; PKMN_STORAGE_CHUNKS],
            storage_valid_ids: 0,
            storage_counters: [0; PKMN_STORAGE_CHUNKS],
            storage_ids_unique: true,
            storage_rotation: None,
            storage_rotation_coherent: true,
            save_block_counters: [0; NUM_SECTORS_PER_SLOT],
            save_block_ids: [0; NUM_SECTORS_PER_SLOT],
            save_block_count: 0,
            legacy_counter: None,
            legacy_consistent: true,
        }
    }

    /// Folds the sector at physical position `i` into the survey.
    pub(super) fn observe(&mut self, i: usize, sector: &Sector) {
        let in_tail = i >= usize::from(SECTOR_ID_PKMN_STORAGE_START);
        // An unsigned tail sector was erased or never finished. Upstream's
        // `GetSaveValidStatus` never treats it as a present id
        // (`pokeemerald/src/save.c`), so it spends the same unusable-tail
        // budget as a checksum failure. Returning before that bookkeeping
        // let a sixth-sector tear, stopped after the erase, read as a stale
        // legacy tail.
        if sector.signature() != SECTOR_SIGNATURE {
            if in_tail {
                self.note_unusable_tail(i);
            }
            return;
        }
        if in_tail {
            self.tail_signature_seen = true;
        }
        self.signature_valid = true;
        let id = sector.id();
        let is_valid_sector = sector_payload_len(id).is_some_and(|len| sector.is_valid(len));
        if in_tail && !is_valid_sector {
            self.note_unusable_tail(i);
        }
        if !is_valid_sector {
            return;
        }
        let counter = sector.counter();
        self.counter = counter;
        self.valid_ids |= 1 << id;
        let rotation = (i + NUM_SECTORS_PER_SLOT - usize::from(id) % NUM_SECTORS_PER_SLOT)
            % NUM_SECTORS_PER_SLOT;
        if id < SECTOR_ID_PKMN_STORAGE_START {
            self.save_block_counters[self.save_block_count] = counter;
            self.save_block_ids[self.save_block_count] = id;
            self.save_block_count += 1;
        }
        // Storage ids are tracked at every physical position.
        if id >= SECTOR_ID_PKMN_STORAGE_START {
            // A generation writes each id once; a repeat is not a donor set.
            if self.storage_valid_ids & (1 << id) != 0 {
                self.storage_ids_unique = false;
            }
            self.storage_valid_ids |= 1 << id;
            self.storage_counters[usize::from(id - SECTOR_ID_PKMN_STORAGE_START)] = counter;
            match self.storage_rotation {
                None => self.storage_rotation = Some(rotation),
                Some(r) if r == rotation => {}
                Some(_) => self.storage_rotation_coherent = false,
            }
        }
        if in_tail {
            self.tail_counters[self.tail_valid_count] = counter;
            self.tail_rotations[self.tail_valid_count] = rotation;
            self.tail_valid_count += 1;
        } else if id < SECTOR_ID_PKMN_STORAGE_START {
            self.head_valid_ids |= 1 << id;
            if usize::from(id) != i {
                self.head_is_identity = false;
            }
            match self.legacy_counter {
                None => self.legacy_counter = Some(counter),
                Some(c) if c == counter => {}
                Some(_) => self.legacy_consistent = false,
            }
        }
    }

    /// Counts a tail sector that cannot contribute an id.
    ///
    /// The first-tail flag records whether *this* position is the slot's
    /// first tail sector. A later unusable sector replaces it, matching one
    /// pass in physical order.
    fn note_unusable_tail(&mut self, i: usize) {
        self.tail_unusable_count += 1;
        self.tail_unusable_at_first_tail_position = i == usize::from(SECTOR_ID_PKMN_STORAGE_START);
    }

    /// Counter of the one complete storage set here, unless an outlier is a
    /// partial save-block generation or the slot's next write.
    fn storage_generation(&self) -> Option<u32> {
        if !self.storage_rotation_coherent {
            return None;
        }
        let generation = self.storage_counters.iter().copied().find(|&candidate| {
            self.storage_counters
                .iter()
                .filter(|&&counter| counter == candidate)
                .count()
                >= PKMN_STORAGE_CHUNKS - 1
        })?;
        let outlier_is_accepted = self.storage_counters.iter().all(|&counter| {
            counter == generation
                || match self.save_block_ids_carrying(counter) {
                    0 => !self.is_next_write_into_this_slot(generation, counter),
                    ids => ids & SAVE_BLOCK_IDS == SAVE_BLOCK_IDS,
                }
        });
        outlier_is_accepted.then_some(generation)
    }

    /// The counter shared by more than half of the checksum-valid sectors.
    fn majority_counter(&self) -> Option<u32> {
        let storage = (0..PKMN_STORAGE_CHUNKS)
            .filter(|&chunk| self.storage_valid_ids & (1 << (chunk + PKMN_STORAGE_FIRST_ID)) != 0)
            .map(|chunk| self.storage_counters[chunk]);
        let counters = self.save_block_counters[..self.save_block_count]
            .iter()
            .copied()
            .chain(storage);
        let total = counters.clone().count();
        counters
            .clone()
            .find(|&candidate| 2 * counters.clone().filter(|&c| c == candidate).count() > total)
    }

    /// Whether `counter` follows one this slot carries by the two-slot stride
    /// of upstream's `WriteSaveSectorOrSlot` (`pokeemerald/src/save.c:138-173`).
    fn is_next_write_into_this_slot(&self, generation: u32, counter: u32) -> bool {
        let successor_of = |c: u32| c.wrapping_add(NUM_SAVE_SLOTS_U32) == counter;
        successor_of(generation)
            || self.save_block_counters[..self.save_block_count]
                .iter()
                .any(|&c| successor_of(c))
    }

    /// The ids 0-4 present among the save-block sectors carrying `counter`.
    fn save_block_ids_carrying(&self, counter: u32) -> u32 {
        self.save_block_counters[..self.save_block_count]
            .iter()
            .zip(&self.save_block_ids[..self.save_block_count])
            .filter(|&(&c, _)| c == counter)
            .fold(0u32, |ids, (_, &id)| ids | 1 << id)
    }

    /// The rotation and counter of the stale tail behind a legacy head, if one
    /// is identifiable; upstream has no such tail (`pokeemerald/src/save.c:514-585`).
    fn tail_generation(&self) -> Option<(usize, u32)> {
        if self.tail_unusable_count > 1 {
            return None;
        }
        // One unusable sector spends the whole outlier budget.
        if self.tail_unusable_count == 1 && self.tail_valid_count != PKMN_STORAGE_CHUNKS - 1 {
            return None;
        }
        let counters = &self.tail_counters[..self.tail_valid_count];
        let rotations = &self.tail_rotations[..self.tail_valid_count];
        let rotation = one_outlier_consensus(rotations)?;
        let counter = one_outlier_consensus(counters)?;
        // A write torn past five sectors always damages the position after
        // the head's, over a predecessor it rotated away from. A rotated tail
        // behind that damage is refused by layout; a rotation-0 tail there
        // was never that write's predecessor and can only be a legacy head's.
        if self.tail_unusable_count == 1
            && self.tail_unusable_at_first_tail_position
            && rotation != 0
        {
            return None;
        }
        let mut outliers = rotations
            .iter()
            .zip(counters)
            .filter(|&(&r, &c)| r != rotation || c != counter);
        let outlier = outliers.next();
        if outliers.next().is_some() || (outlier.is_some() && self.tail_unusable_count == 1) {
            return None;
        }
        let rotation_outlier_is_the_heads_generation =
            outlier.is_some_and(|(&r, &c)| r != rotation && self.legacy_counter == Some(c));
        (!rotation_outlier_is_the_heads_generation).then_some((rotation, counter))
    }

    pub(super) fn verdict(&self) -> SlotScan {
        let tail = self.tail_generation();
        let tail_counter = tail.map(|(_, counter)| counter);
        // An identity head over a rotation-12 tail is the shape a full
        // write torn after five sectors leaves; upstream reads it Error.
        let ambiguous_with_a_torn_full_write = self.head_is_identity
            && tail.is_some_and(|(rotation, _)| rotation == TORN_WRITE_PREDECESSOR_ROTATION);

        let stale_tail_is_donor_only = self.tail_signature_seen
            && tail_counter.is_some()
            && self
                .legacy_counter
                .zip(tail_counter)
                .is_some_and(|(legacy, tail)| older_generation_precedes(tail, legacy))
            && !ambiguous_with_a_torn_full_write;

        // A rotated head is no torn full write unless a tail sector shares its
        // counter; that is a full generation missing an id, Error upstream.
        let tail_shares_head_generation = self.legacy_counter.is_some_and(|legacy| {
            self.tail_counters[..self.tail_valid_count].contains(&legacy)
                && !tail_counter.is_some_and(|tail| older_generation_precedes(tail, legacy))
        });
        let head_cannot_be_a_torn_full_write =
            !self.head_is_identity && !tail_shares_head_generation;
        let all_valid_mask = (1u32 << u32::from(NUM_SECTORS_PER_SLOT_U16)) - 1;
        let legacy_intact = self.legacy_consistent
            && self.head_valid_ids == LEGACY_ERA_IDS_MASK
            && (!self.tail_signature_seen
                || stale_tail_is_donor_only
                || head_cannot_be_a_torn_full_write);
        let integrity = if !self.signature_valid {
            SlotIntegrity::Empty
        } else if self.valid_ids == all_valid_mask || legacy_intact {
            SlotIntegrity::Ok
        } else {
            SlotIntegrity::Error
        };
        // A legacy slot reports its head's counter, not the tail's.
        let counter = if legacy_intact {
            self.legacy_counter
                .expect("legacy_intact requires a legacy head counter")
        } else {
            self.counter
        };
        // Only a complete set holding each id once may donate.
        let storage_counter =
            if self.storage_valid_ids == PKMN_STORAGE_IDS_MASK && self.storage_ids_unique {
                self.storage_generation()
            } else {
                None
            };
        SlotScan {
            integrity,
            counter,
            legacy: legacy_intact,
            storage_counter,
            majority_counter: self.majority_counter(),
        }
    }
}

/// Picks the generation to load: the newer intact slot, with a five-sector
/// head taking `PokemonStorage` from the newest verified set in either slot.
pub(super) fn resolve(slot0: &SlotScan, slot1: &SlotScan) -> Resolution {
    use SlotIntegrity::{Empty, Error, Ok};
    let (status, counter, storage_from_slot, legacy) = match (slot0.integrity, slot1.integrity) {
        (Ok, Ok) => resolve_both_ok(slot0, slot1),
        (Ok, Error) => (
            SaveStatus::Error,
            slot0.counter,
            storage_donor(slot0.legacy, slot0, slot1),
            slot0.legacy,
        ),
        (Ok, Empty) => (
            SaveStatus::Ok,
            slot0.counter,
            storage_donor(slot0.legacy, slot0, slot1),
            slot0.legacy,
        ),
        (Error, Ok) => (
            SaveStatus::Error,
            slot1.counter,
            storage_donor(slot1.legacy, slot0, slot1),
            slot1.legacy,
        ),
        (Empty, Ok) => (
            SaveStatus::Ok,
            slot1.counter,
            storage_donor(slot1.legacy, slot0, slot1),
            slot1.legacy,
        ),
        (Empty, Empty) => (SaveStatus::Empty, 0, None, false),
        (Error | Empty, Error) | (Error, Empty) => (SaveStatus::Corrupt, 0, None, false),
    };
    Resolution {
        status,
        counter,
        storage_from_slot,
        legacy,
    }
}

/// The `(Ok, Ok)` half of [`resolve`]: the newer counter wins.
fn resolve_both_ok(slot0: &SlotScan, slot1: &SlotScan) -> (SaveStatus, u32, Option<usize>, bool) {
    if slot0.legacy == slot1.legacy {
        let counter = if second_counter_is_newer(slot0.counter, slot1.counter) {
            slot1.counter
        } else {
            slot0.counter
        };
        return (
            SaveStatus::Ok,
            counter,
            storage_donor(slot0.legacy, slot0, slot1),
            slot0.legacy,
        );
    }
    let (legacy, full) = if slot0.legacy {
        (slot0, slot1)
    } else {
        (slot1, slot0)
    };
    if second_counter_is_newer(full.counter, legacy.counter) {
        // The newer legacy head borrows the newest storage set. An `Ok` full
        // slot's storage belongs to its generation even when its footer
        // counters do not agree: upstream's `GetSaveValidStatus` judges
        // sectors by checksum alone (`pokeemerald/src/save.c:512-570`). That
        // generation is the majority footer counter, not `full.counter`,
        // which a damaged final sector would supply.
        let full_storage = full
            .storage_counter
            .or(full.majority_counter)
            .or(Some(full.counter));
        let (left, right) = if slot0.legacy {
            (slot0.storage_counter, full_storage)
        } else {
            (full_storage, slot1.storage_counter)
        };
        (
            SaveStatus::Ok,
            legacy.counter,
            newest_storage_set(left, right),
            true,
        )
    } else {
        (SaveStatus::Ok, full.counter, None, false)
    }
}

/// The slot holding the newest complete verified storage set, even one
/// too damaged to be `Ok` itself.
fn storage_donor(adopted_is_legacy: bool, slot0: &SlotScan, slot1: &SlotScan) -> Option<usize> {
    if !adopted_is_legacy {
        return None;
    }
    newest_storage_set(slot0.storage_counter, slot1.storage_counter)
}

/// The slot whose storage generation is newer, given each slot's own.
fn newest_storage_set(slot0: Option<u32>, slot1: Option<u32>) -> Option<usize> {
    match (slot0, slot1) {
        (Some(left), Some(right)) => Some(usize::from(older_generation_precedes(left, right))),
        (Some(_), None) => Some(0),
        (None, Some(_)) => Some(1),
        (None, None) => None,
    }
}
