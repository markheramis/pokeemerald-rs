//! Assets subsystem (S-4): extraction pipeline + typed access.
//!
//! Exposes extracted upstream game data as idiomatic, owned Rust types
//! `(oop-boundaries)`. Extracted so far: the battle type-effectiveness chart
//! (see [`type_chart`]), the per-move battle-data table (see [`battle_moves`]),
//! the species base-stats table (see [`species`]), the item table (see
//! [`items`]), the per-species evolution table (see [`evolution`]), the
//! per-species TM/HM learnsets (see [`tmhm_learnsets`]), the per-species
//! level-up learnsets (see [`level_up_learnsets`]), the per-species
//! egg-move lists (see [`egg_moves`]), the species/move/ability display text
//! (see [`species_names`], [`move_names`], [`abilities`]), the experience
//! growth curves (see [`experience`]), the per-map wild encounter tables (see
//! [`wild_encounters`]), the trainer roster and parties (see [`trainers`]),
//! the map layout geometry (see [`map_layouts`]), the map headers, groups,
//! and connections (see [`map_headers`]), the per-map object/warp/coord/
//! bg events (see [`map_events`]), and the fresh-save object-hide flag set
//! `EventScript_ResetAllMapFlags` applies (see [`new_game_flags`]); more
//! data follows as the crate fills out.
//!
//! The backend-neutral song/voicegroup/sample pack schemas both
//! audio-extraction backends emit into (S-4) live in [`audio`].
//!
//! Binary graphics (tileset tile graphics, palettes, player/NPC sprite
//! sheets, font glyph sheets, text-window border frames) and map layout
//! grid/border bytes are not compiled into this crate as Rust source,
//! unlike the tables above — per Discussion #71 policy A they live in a
//! local, gitignored asset pack that `cargo xtask extract` produces;
//! [`pack`] is this crate's typed loader over that pack.
//! [`map_layouts`], [`metatile_attributes`], and [`fonts`] provide the
//! decode layer those bytes feed once read out of the pack:
//! [`map_layouts::LayoutGrid`]/[`map_layouts::BorderGrid`] for a layout's
//! metatile grid, [`metatile_attributes::MetatileAttributeTable`] for a
//! tileset's per-metatile behavior/layer-type attributes, and
//! [`fonts::FontGlyphSheet`] for a Latin font's per-glyph bitmap (its
//! per-glyph advance widths, unlike the bitmaps, are ordinary Rust data —
//! see that module's docs).

pub mod abilities;
pub mod audio;
pub mod battle_anim;
pub mod battle_moves;
pub mod bundled_overworld;
pub mod egg_moves;
pub mod error;
pub mod event_scripts;
pub mod evolution;
pub mod experience;
pub mod fonts;
pub mod hidden_item_flags;
pub mod items;
pub mod level_up_learnsets;
pub mod map_events;
pub mod map_headers;
pub mod map_layouts;
pub mod metatile_attributes;
pub mod move_names;
pub mod new_game_flags;
pub mod object_event_flags;
pub mod pack;
pub mod pokedex;
pub mod species;
pub mod species_names;
pub mod tmhm_learnsets;
pub mod trainers;
pub mod type_chart;
pub mod wild_encounters;

pub use abilities::{Abilities, AbilityData, ABILITIES_COUNT};
// `audio::VoiceGroupId`/`audio::SampleId` are re-exported here alongside the
// three schema types they identify; `audio`'s own internal cross-references
// between them stay reachable through the `audio` module path too.
pub use audio::{
    cry_id, AudioError, CryId, CryMode, CrySettings, CryTable, DirectSoundMode, DirectSoundSample,
    DirectSoundVoice, Envelope, KeySplitVoice, MusicPlayerId, NoiseVoice, ProgrammableWave,
    ProgrammableWaveVoice, RhythmVoice, Sample, SampleId, Song, SongEvent, Square1Voice,
    Square2Voice, VoiceEntry, VoiceGroup, VoiceGroupId, MAX_TRACKS, VOICE_SLOT_COUNT,
};
pub use battle_moves::{
    MoveData, MoveEffect, MoveFlags, MoveId, MoveTable, MoveTarget, MoveType, MOVES_COUNT,
};
pub use egg_moves::{
    EggMoveList, EggMoveTable, EGG_MOVES_SPECIES_OFFSET, EGG_MOVES_TERMINATOR,
    EGG_MOVE_SPECIES_COUNT,
};
pub use error::AssetError;
pub use evolution::{EvoMethod, Evolution, EvolutionTable};
pub use experience::{experience_for_level, MAX_LEVEL};
pub use fonts::{
    FontGlyphSheet, FontId, FontImageRef, Glyph, GlyphSource, OwnedFontGlyphSheet, GLYPH_COLUMNS,
    GLYPH_COUNT, GLYPH_PIXELS, GLYPH_ROWS, GLYPH_SIZE, SHEET_HEIGHT, SHEET_WIDTH,
};
pub use level_up_learnsets::{LevelUpLearnsets, LevelUpMove, SPECIES_COUNT};
pub use map_events::{
    BgEvent, BgEventKind, CoordEvent, CoordEventKind, CoordWeather, FacingDirection, MapEvents,
    MapEventsTable, MovementType, ObjectEvent, SecretBaseId, TrainerType, WarpDestination,
    WarpEvent, WarpId, MAP_EVENTS_COUNT,
};
pub use map_headers::{
    BattleScene, Direction, MapConnection, MapGroup, MapHeader, MapHeaderTable, MapType, MusicId,
    RegionMapSectionId, Weather, MAP_COUNT, MAP_GROUP_COUNT,
};
pub use map_layouts::{
    BorderGrid, LayoutGrid, LayoutId, LayoutTable, MapLayout, MetatileCell, BORDER_CELLS,
    BORDER_HEIGHT, BORDER_WIDTH, LAYOUT_COUNT,
};
pub use metatile_attributes::{MetatileAttribute, MetatileAttributeTable, MetatileLayerType};
pub use new_game_flags::{
    FLAG_HIDE_LITTLEROOT_TOWN_BRENDANS_HOUSE_RIVAL_BEDROOM, RESET_MAP_FLAGS,
    TRUCK_INTRO_FLAGS_FEMALE, TRUCK_INTRO_FLAGS_MALE, TRUCK_INTRO_VARS_FEMALE,
    TRUCK_INTRO_VARS_MALE, WALL_CLOCK_VIGOROTH_HIDE_FLAGS,
};

// `items::ItemId` is the full item-table identifier; it is intentionally *not*
// re-exported at the crate root to avoid clashing with `species::ItemId` (the
// lightweight held-item reference used by the species table). Reach it via the
// `items` module path.
pub use battle_anim::{
    AnimCommand, AnimSymbol, AnimationSelector, AnimationTable, BattleAnimationCatalog,
    BattleAnimationError, BattleAnimationProgram, EffectSpriteId, PanSweep, ProgramId,
};
pub use items::{
    BattleUsage, HoldEffect, ItemData, ItemDisplayData, ItemDisplayTable, ItemTable, ItemType,
    Pocket, ITEMS_COUNT,
};
pub use move_names::MoveNames;
pub use pack::{
    AssetPack, BagAssets, BattlerDirection, BattlerHandle, BirchSpeechHandle, EntryKind, ImageRef,
    NamingMenuPalette, NamingScreenHandle, PackError, PaletteRef, PaletteVariant, PalettedImage,
    PokedexGraphicsHandle, StarterChooserHandle, StarterPreviewHandle, StarterSpecies,
    TilesetHandle, TrainerPicHandle, WallClockHandle,
};
pub use pokedex::{
    NationalDexNo, PokedexData, PokedexEntry, PokedexError, PokedexOrderKind, PokedexOrders,
    NATIONAL_DEX_COUNT, NATIONAL_DEX_LAST, NATIONAL_DEX_OLD_UNOWN_COUNT,
};
pub use species::{
    AbilityId, BaseStats, BodyColor, EggGroup, EvYield, GenderRatio, GrowthRate, ItemId, SpeciesId,
    SpeciesTable,
};
pub use species_names::SpeciesNames;
pub use tmhm_learnsets::{is_hm_move, TmHmLearnsets, TmHmSlot};
pub use trainers::{
    AiFlags, EncounterMusic, TrainerClass, TrainerData, TrainerId, TrainerMonItemCustomMoves,
    TrainerMonItemDefaultMoves, TrainerMonNoItemCustomMoves, TrainerMonNoItemDefaultMoves,
    TrainerParty, TrainerPicId, TrainerTable, MAX_TRAINER_ITEMS, TRAINERS_COUNT,
};
pub use type_chart::{Effectiveness, Type, TypeChart};
pub use wild_encounters::{
    FishingEncounters, FishingRod, LandEncounters, MapId, RockSmashEncounters, WaterEncounters,
    WildEncounterHeader, WildEncounterTable, WildPokemon, FISHING_SLOTS, LAND_SLOTS,
    MAP_HEADER_COUNT, ROCK_SMASH_SLOTS, WATER_SLOTS,
};
