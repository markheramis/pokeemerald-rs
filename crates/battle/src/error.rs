//! Failures reported by battle data, construction, and turn operations.

use assets::items::ItemId;
use assets::trainers::{AiFlags, TrainerId};
use assets::{AbilityId, MoveId, SpeciesId};
use std::error::Error;
use std::fmt;

/// A failure reported by the `battle` crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BattleError {
    /// A stat-stage offset was outside `-6..=6`.
    StatStageOutOfRange(i8),

    /// A raw nature ID was outside the range recognized by
    /// [`Nature`](crate::nature::Nature).
    UnknownNature(u8),

    /// A [`SpeciesId`] was outside the extracted species table.
    UnknownSpecies(SpeciesId),

    /// A [`MoveId`] was outside the extracted move table.
    UnknownMove(MoveId),

    /// A move had no combat [`assets::Type`].
    UnsupportedMoveType(MoveId),

    /// A move slot index was outside the battler's move list.
    InvalidMoveSlot(usize),

    /// A moveset was empty or exceeded [`crate::pokemon::MAX_MON_MOVES`].
    InvalidMoveCount(usize),

    /// A moveset contained the empty-slot placeholder
    /// [`crate::pokemon::MOVE_NONE`].
    PlaceholderMove(usize),

    /// A Pokémon level was outside
    /// [`crate::pokemon::MIN_LEVEL`]`..=`[`crate::pokemon::MAX_LEVEL`].
    InvalidLevel(u8),

    /// A Pokémon individual value exceeded [`crate::pokemon::MAX_IV`].
    InvalidIv(u8),

    /// A caller tried to spend PP from a move slot with none remaining.
    /// [`crate::battle::Battle::take_turn`] rejects a player's spent slot
    /// before the turn mutates state or consumes randomness; an opponent's
    /// spent slot instead produces [`crate::battle::BattleEvent::FailedNoPp`].
    NoPpRemaining(usize),

    /// A damaging-move pipeline received a move with zero power.
    NonDamagingMove(MoveId),

    /// A move's battle effect was not supported by the selected execution
    /// pipeline.
    UnsupportedMoveEffect(MoveId),

    /// A move produced a secondary effect that the post-damage hook could
    /// not apply. The hook reports this only after the effect-chance draw and
    /// preceding damage have occurred.
    UnportedSecondaryEffect(MoveId),

    /// A battler's ability rewrites the outcome in a way no pipeline models,
    /// reported by the pre-turn and constructor screens before any draw.
    UnportedAbilityInteraction(AbilityId),

    /// A participant holds an item whose in-battle effect no pipeline models,
    /// reported by the pre-turn and constructor screens before any draw.
    UnsupportedHeldItem(ItemId),

    /// A trainer's bag holds an item its AI would use mid-battle
    /// (`ShouldUseItem`, `battle_ai_switch_items.c`). Trainer action
    /// selection has no item use, so the battle is refused before any draw
    /// instead of silently playing out without the item.
    UnsupportedTrainerBagItem(ItemId),

    /// A species was an empty or compatibility placeholder:
    /// [`crate::pokemon::SPECIES_NONE`] or
    /// [`crate::pokemon::SPECIES_OLD_UNOWN_B`]`..=`
    /// [`crate::pokemon::SPECIES_OLD_UNOWN_Z`].
    PlaceholderSpecies,

    /// A battle was constructed with an already-fainted participant.
    /// The payload is `true` for the player's battler.
    FaintedBattler(bool),

    /// A turn was requested after the battle reached a terminal outcome.
    BattleAlreadyOver,

    /// A caller tried to run from the first battle, or a wild battle whose
    /// opponent's Shadow Tag or grounded Arena Trap forbids escape. Rejected
    /// before turn state or randomness changes
    /// (`pokeemerald/src/battle_main.c:4038`-`:4062`, `:4078`-`:4082`, `:4339`-`:4344`).
    RunForbidden,

    /// A caller tried to run from a trainer battle. The selection is rejected
    /// before turn state or randomness changes.
    NoRunningFromTrainer,

    /// A trainer ID was outside the extracted trainer table.
    UnknownTrainer(TrainerId),

    /// A trainer switch named a party slot that is not a healthy non-active
    /// member: the active slot, a slot past the party, or a fainted member.
    InvalidTrainerSwitch(usize),

    /// A trainer knew a move that the turn engine could execute but the
    /// trainer AI could not score. Trainer-battle construction rejects it
    /// before consuming randomness.
    UnscoreableMoveEffect(MoveId),

    /// A trainer selected unsupported AI scripts. The payload contains only
    /// the unsupported flag bits.
    UnsupportedAiFlags(AiFlags),

    /// A trainer AI scoring target's species has two ability slots and its
    /// selected ability is not Shadow Tag, Magnet Pull, or Arena Trap (which
    /// upstream reads directly). The engine models no battle ability history,
    /// so it cannot reproduce upstream's `get_ability AI_TARGET` read-or-guess
    /// (`pokeemerald/src/battle_ai_script_commands.c:1350`-`:1396`) and
    /// refuses the lead before consuming randomness.
    AmbiguousPlayerAbility(SpeciesId),

    /// Natural Cure's else-if would read `gBattleMoves[MOVE_UNAVAILABLE]`.
    /// That index is not in the move table, so the switch is refused after
    /// the coin that reaches the read and before any lookup.
    UndefinedNaturalCureMove,

    /// A turn was requested while a level-up move awaited a learn decision.
    /// The pending move ID is returned without changing battle state.
    MoveLearnPending(MoveId),

    /// A move-learn decision was submitted without a pending prompt.
    NoMoveLearnPending,

    /// A move-learn decision tried to replace an HM move. The pending prompt
    /// remains unchanged so the caller can choose another slot or decline.
    HmMoveCantBeForgotten(MoveId),

    /// A trainer battle was constructed with an empty opponent party.
    EmptyTrainerParty(TrainerId),
}

impl fmt::Display for BattleError {
    #[allow(
        clippy::too_many_lines,
        reason = "one arm per variant; splitting the match would hide exhaustiveness"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StatStageOutOfRange(offset) => {
                write!(f, "stat stage offset `{offset}` outside -6..=6")
            }
            Self::UnknownNature(id) => write!(f, "unknown nature id `{id}`"),
            Self::UnknownSpecies(id) => write!(f, "unknown species id `{}`", id.0),
            Self::UnknownMove(id) => write!(f, "unknown move id `{}`", id.0),
            Self::UnsupportedMoveType(id) => {
                write!(f, "move `{}` has no supported combat type", id.0)
            }
            Self::InvalidMoveSlot(index) => write!(f, "invalid move slot index `{index}`"),
            Self::InvalidMoveCount(count) => {
                write!(f, "moveset of `{count}` moves outside 1..=4")
            }
            Self::PlaceholderMove(index) => {
                write!(f, "move slot `{index}` is the MOVE_NONE placeholder")
            }
            Self::InvalidLevel(level) => write!(f, "level `{level}` outside 1..=100"),
            Self::InvalidIv(value) => write!(f, "individual value `{value}` outside 0..=31"),
            Self::NoPpRemaining(index) => write!(f, "move slot `{index}` has no PP remaining"),
            Self::NonDamagingMove(id) => {
                write!(f, "move `{}` has no base damage", id.0)
            }
            Self::UnsupportedMoveEffect(id) => {
                write!(f, "move `{}` has an unsupported battle effect", id.0)
            }
            Self::UnportedSecondaryEffect(id) => write!(
                f,
                "move `{}` produced an unsupported secondary effect",
                id.0
            ),
            Self::UnportedAbilityInteraction(id) => write!(
                f,
                "ability `{}` alters this battle in an unmodelled way",
                id.0
            ),
            Self::UnsupportedHeldItem(id) => write!(
                f,
                "held item `{}` has a battle effect no pipeline models",
                id.0
            ),
            Self::UnsupportedTrainerBagItem(id) => write!(
                f,
                "trainer bag item `{}` would be used by an AI that never uses items here",
                id.0
            ),
            Self::PlaceholderSpecies => {
                write!(
                    f,
                    "species is a reserved placeholder slot (SPECIES_NONE or old-Unown)"
                )
            }
            Self::FaintedBattler(is_player) => {
                let side = if *is_player { "player" } else { "enemy" };
                write!(f, "the {side} battler is already fainted")
            }
            Self::BattleAlreadyOver => write!(f, "the battle has already ended"),
            Self::RunForbidden => {
                write!(f, "running is forbidden by this battle's rules")
            }
            Self::NoRunningFromTrainer => {
                write!(f, "running is forbidden in trainer battles")
            }
            Self::UnknownTrainer(id) => write!(f, "unknown trainer id `{}`", id.0),
            Self::InvalidTrainerSwitch(slot) => {
                write!(f, "party slot `{slot}` is not a healthy non-active member")
            }
            Self::UnscoreableMoveEffect(id) => write!(
                f,
                "move `{}` has a battle effect the trainer AI does not score",
                id.0
            ),
            Self::UnsupportedAiFlags(flags) => write!(
                f,
                "trainer AI flags `{:#x}` include unsupported scripts",
                flags.bits()
            ),
            Self::AmbiguousPlayerAbility(species) => write!(
                f,
                "species `{}` has two abilities the trainer AI cannot tell apart",
                species.0
            ),
            Self::UndefinedNaturalCureMove => write!(
                f,
                "natural cure would read MOVE_UNAVAILABLE, which the move table does not define"
            ),
            Self::MoveLearnPending(id) => write!(
                f,
                "move `{}` is still waiting on a learn/forget decision",
                id.0
            ),
            Self::NoMoveLearnPending => write!(f, "no move-learn decision is pending"),
            Self::HmMoveCantBeForgotten(id) => write!(
                f,
                "HM move `{}` can't be forgotten to make room for a new move",
                id.0
            ),
            Self::EmptyTrainerParty(id) => {
                write!(f, "trainer `{}` has an empty party", id.0)
            }
        }
    }
}

impl Error for BattleError {}
