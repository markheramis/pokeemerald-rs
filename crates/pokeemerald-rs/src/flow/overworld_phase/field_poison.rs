//! Field poison: the four-step step counter, the whole-party HP drain, and
//! the white-out its faint can reach, split out of
//! [`super::step`] and [`super::white_out`] (a submodule of
//! [`crate::flow::overworld_phase`]).
//!
//! [`OverworldPhase::update_poison_step_counter`] is `UpdatePoisonStepCounter`
//! (`pokeemerald/src/field_control_avatar.c:637-659`),
//! [`OverworldPhase::do_poison_field_effect`] is `DoPoisonFieldEffect`
//! (`pokeemerald/src/field_poison.c:120-154`), and
//! [`OverworldPhase::try_field_poison_white_out`] is `TryFieldPoisonWhiteOut`
//! (`field_poison.c:61-117`), which ends in the standard
//! [`OverworldPhase::white_out`].

use super::OverworldPhase;

/// `VAR_POISON_STEP_COUNTER` (`pokeemerald/include/constants/vars.h:61`).
const VAR_POISON_STEP_COUNTER: u16 = 0x402B;
/// The counter's modulus: poison ticks on every fourth step
/// (`field_control_avatar.c:644`).
const POISON_STEP_PERIOD: u16 = 4;
/// `STATUS1_PSN_ANY`: ordinary or bad poison (`constants/battle.h:124`).
/// `GetAilmentFromStatus` tests it first (`party_menu.c:1909-1913`), so the
/// field effect hurts a toxic or mixed-status mon too.
const STATUS1_PSN_ANY: u32 = 0x88;

/// `DoPoisonFieldEffect`'s result (`constants/field_poison.h:4-10`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldPoison {
    None,
    Poisoned,
    Fainted,
}

/// `PokemonSubstruct0::species` (growth bytes `0..2`) and
/// `PokemonSubstruct3`'s `isEgg` bit (the IV word's bit 30), the two fields
/// `MON_DATA_SPECIES_OR_EGG` reads (`pokeemerald/src/pokemon.c:3992-3996`).
const GROWTH_SPECIES: std::ops::Range<usize> = 0..2;
const MISC_IV_WORD: std::ops::Range<usize> = 4..8;
const IS_EGG_BIT: u32 = 1 << 30;
/// `SPECIES_EGG` (`pokeemerald/include/constants/species.h`), the id
/// `MON_DATA_SPECIES_OR_EGG` reports for an egg and which a record may also
/// carry as its growth species.
const SPECIES_EGG: u16 = 412;

/// `IsMonValidSpecies` (`pokeemerald/src/field_poison.c:20-27`): a species
/// that is neither `SPECIES_NONE` nor `SPECIES_EGG`. A record whose secure
/// checksum fails is a Bad Egg, so it is not valid either.
fn is_valid_species(record: &engine::save::Pokemon) -> bool {
    if record.box_data.is_bad_egg() {
        return false;
    }
    let Ok(substructures) = record.box_data.substructures() else {
        return false;
    };
    let species = u16::from_le_bytes([
        substructures.growth[GROWTH_SPECIES.start],
        substructures.growth[GROWTH_SPECIES.start + 1],
    ]);
    let iv_word = u32::from_le_bytes(
        substructures.misc[MISC_IV_WORD]
            .try_into()
            .expect("a u32 field has four bytes"),
    );
    species != 0 && species != SPECIES_EGG && iv_word & IS_EGG_BIT == 0
}

impl OverworldPhase {
    /// `TryFieldPoisonWhiteOut` (`pokeemerald/src/field_poison.c:61-117`),
    /// run after `OverworldPhase::do_poison_field_effect` reports a
    /// faint. Every valid member at zero HP that is poisoned
    /// (`MonFaintedFromPoison`, `:52-59`) has its status cleared
    /// (`FaintFromFieldPoison`, `:42-50`); then, if no valid member has HP
    /// left (`AllMonsFainted`, `:29-40`), the standard
    /// [`Self::white_out`] runs. Returns whether it did.
    ///
    /// Not modelled: the per-mon "fainted" message, `AdjustFriendship`'s
    /// field-poison faint penalty, and the battle-facility result
    /// (`FLDPSN_FRONTIER_WHITEOUT`, `:96-100`), none of which this port's
    /// facilities can reach.
    pub(super) fn try_field_poison_white_out(&mut self) -> bool {
        for record in &mut self.save1.player_party {
            if is_valid_species(record) && record.hp == 0 && record.status & STATUS1_PSN_ANY != 0 {
                record.status = 0;
            }
        }
        let all_fainted = self
            .save1
            .player_party
            .iter()
            .all(|record| !is_valid_species(record) || record.hp == 0);
        if all_fainted {
            self.white_out();
        }
        all_fainted
    }

    /// `UpdatePoisonStepCounter` (`field_control_avatar.c:637-659`) for one
    /// completed step the caller has already screened: outside a Secret Base
    /// ([`assets::MapType::SecretBase`]) it advances
    /// [`VAR_POISON_STEP_COUNTER`] modulo four, and on the wrap runs
    /// [`Self::do_poison_field_effect`]. Returns whether the frame is claimed,
    /// which only a faint is (`:648-655`): an ordinary poison tick leaves the
    /// rest of the frame to the encounter roll, arrows, and interaction.
    ///
    /// The counter is the saved event var, so Save and Continue keep it
    /// mid-cycle. The wrap is `u16`'s, like the C `(*ptr)++`.
    pub(super) fn update_poison_step_counter(&mut self, map_type: assets::MapType) -> bool {
        // `InUnionRoom` skips the whole step-count script
        // (`field_control_avatar.c:543-546`, `union_room.c:4307-4311`).
        if map_type == assets::MapType::SecretBase || self.map_id == assets::MapId("MAP_UNION_ROOM")
        {
            return false;
        }
        let counter = self
            .save1
            .event_data
            .var_get(VAR_POISON_STEP_COUNTER)
            .unwrap_or(0)
            .wrapping_add(1)
            % POISON_STEP_PERIOD;
        let _ = self
            .save1
            .event_data
            .var_set(VAR_POISON_STEP_COUNTER, counter);
        counter == 0 && self.do_poison_field_effect() == FieldPoison::Fainted
    }

    /// `ClearPoisonStepCounter` (`field_control_avatar.c:632-635`): a battle
    /// starting restarts the four-step cycle (`battle_setup.c:365-372,930-943`).
    pub(super) fn clear_poison_step_counter(&mut self) {
        let _ = self.save1.event_data.var_set(VAR_POISON_STEP_COUNTER, 0);
    }

    /// `DoPoisonFieldEffect` (`field_poison.c:120-154`): every party slot
    /// that has a species and either poison flag loses one HP, never below
    /// zero; one that reaches or already sits at zero counts as fainted. All
    /// six slots are visited regardless of the stored count, as the C loop
    /// does. The screen flash (`FldEffPoison_Start`) is not modelled, and no
    /// random number is drawn.
    ///
    /// A faint is followed through [`Self::try_field_poison_white_out`]; the
    /// lead is filed into the save first and reloaded from it after, so the
    /// live battler and the saved HP stay one fact.
    fn do_poison_field_effect(&mut self) -> FieldPoison {
        // The live battler is authoritative for the lead's status: a cure in
        // battle reaches the saved record only when it is filed, so file
        // whenever either side still says poisoned.
        let lead_slot = self
            .lead
            .as_ref()
            .filter(|lead| !lead.is_lent())
            .map(crate::party::LoadedLead::slot);
        let lead_poisoned = lead_slot.is_some_and(|slot| {
            self.lead_battler()
                .is_some_and(|lead| lead.status1().is_psn_any())
                || self.save1.player_party[slot].status & STATUS1_PSN_ANY != 0
        });
        if lead_poisoned {
            self.flush_lead_to_save();
        }

        let mut poisoned = 0u32;
        let mut fainted = 0u32;
        for record in &mut self.save1.player_party {
            if !record.box_data.has_species() || record.status & STATUS1_PSN_ANY == 0 {
                continue;
            }
            // `hp == 0 || --hp == 0`: an already-fainted poisoned mon counts
            // again but stays at zero.
            record.hp = record.hp.saturating_sub(1);
            if record.hp == 0 {
                fainted += 1;
            }
            poisoned += 1;
        }

        if fainted != 0 {
            if !self.try_field_poison_white_out() && lead_poisoned {
                self.load_lead_from_save();
            }
            FieldPoison::Fainted
        } else {
            if lead_poisoned {
                self.load_lead_from_save();
            }
            if poisoned != 0 {
                FieldPoison::Poisoned
            } else {
                FieldPoison::None
            }
        }
    }
}
