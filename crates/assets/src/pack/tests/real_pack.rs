use super::super::AssetPack;
use crate::audio::{SampleId, SongEvent, VoiceGroupId};

const EXPECTED_PALETTE_COLOR_COUNT: u16 = 16;
const EXPECTED_PALETTE_COLOR_COUNT_USIZE: usize = 16;
const SPACE_GLYPH_INDEX: u16 = 0;
const EXPECTED_WINDOW_FRAME_COUNT: u8 = 20;
const FIRST_EXTRA_PALETTE_NUMBER: u8 = 1;
const LAST_EXTRA_PALETTE_NUMBER: u8 = 4;
const EXPECTED_DIRECT_SOUND_SAMPLE_COUNT: usize = 41;
const EXPECTED_PROGRAMMABLE_WAVE_COUNT: usize = 5;
const EXPECTED_MUS_TITLE_SAMPLE_COUNT: usize = 46;
const INTERPOLATION_GUARD_SAMPLE_COUNT: usize = 1;
const FLUTE_AGBP_BASE_FREQUENCY: u32 = 3_425_024;
const FLUTE_SMPL_LOOP_START: u32 = 1312;
const FLUTE_AGBL_LOGICAL_SAMPLE_COUNT: u32 = 1874;
const FLUTE_PCM_BUFFER_LENGTH: usize = 1875;
const FLUTE_INTERPOLATION_GUARD_INDEX: usize = 1874;
const FLUTE_INTERPOLATION_GUARD_VALUE: i8 = -8;
const FLUTE_LOOP_START_INDEX: usize = 1312;
const PROGRAMMABLE_WAVE_01_PCM_BYTES: [u8; 16] = [
    0x01, 0x25, 0x8a, 0xde, 0xfe, 0xc9, 0x63, 0x10, 0x01, 0x25, 0x8a, 0xde, 0xfe, 0xc9, 0x63, 0x10,
];
const MUS_TITLE_PSEUDO_ECHO_VOLUMES: [u8; 3] = [10, 10, 16];

#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end walk over every typed accessor; a split scatters one round-trip claim across ignored tests"
)]
#[test]
#[ignore = "needs a local pack: run `cargo xtask extract` first"]
fn real_pack_loads_and_every_typed_accessor_works() {
    use crate::fonts::FontId;
    use crate::map_layouts::{BorderGrid, LayoutId, LayoutTable};

    let pack = AssetPack::load_repo().expect("run `cargo xtask extract` first");

    let general = pack
        .tileset("general")
        .expect("general tileset should be in the pack");
    assert!(general.tiles.width > 0 && general.tiles.height > 0);
    assert_eq!(
        general.tiles.pixels.len(),
        general.tiles.width as usize * general.tiles.height as usize
    );
    for palette in &general.palettes {
        assert_eq!(palette.color_count, EXPECTED_PALETTE_COLOR_COUNT);
        assert_eq!(palette.colors().count(), EXPECTED_PALETTE_COLOR_COUNT_USIZE);
    }
    assert!(!general.metatiles.is_empty());
    assert!(!general.metatile_attributes.is_empty());

    for name in [
        "general",
        "building",
        "petalburg",
        "brendans_mays_house",
        "lab",
        "generic_building",
        "pokemon_center",
        "shop",
        "inside_of_truck",
    ] {
        pack.tileset(name)
            .unwrap_or_else(|e| panic!("tileset `{name}` should load: {e}"));
    }

    let walking = pack
        .sprite("brendan/walking")
        .expect("brendan/walking sprite");
    assert!(walking.width > 0 && walking.height > 0);
    let brendan_palette = pack.sprite_palette("brendan").expect("brendan palette");
    assert_eq!(brendan_palette.color_count, EXPECTED_PALETTE_COLOR_COUNT);
    pack.sprite_palette("may").expect("may palette");
    for sheet in [
        "berry_trees/cheri",
        "berry_trees/dirt_pile",
        "berry_trees/leppa",
        "berry_trees/sprout",
        "dolls/pichu_doll",
        "dolls/swablu_doll",
        "misc/birchs_bag",
        "misc/cuttable_tree",
        "misc/item_ball",
        "misc/moving_box",
        "misc/truck",
        "pokemon/enemy_zigzagoon",
        "pokemon/vigoroth",
    ] {
        let image = pack
            .sprite(sheet)
            .unwrap_or_else(|e| panic!("sprite `{sheet}` should be in the pack: {e}"));
        assert!(image.width > 0 && image.height > 0, "{sheet}");
    }
    for palette in ["enemy_zigzagoon", "moving_box", "truck", "vigoroth"] {
        let bank = pack
            .sprite_palette(palette)
            .unwrap_or_else(|e| panic!("sprite palette `{palette}` should be in the pack: {e}"));
        assert_eq!(bank.color_count, EXPECTED_PALETTE_COLOR_COUNT, "{palette}");
    }

    let logo = pack
        .image("title/image/pokemon_logo")
        .expect("title logo image");
    assert!(logo.width > 0 && logo.height > 0);
    pack.palette("title/palette/pokemon_logo")
        .expect("title logo palette");
    pack.raw("title/raw/pokemon_logo")
        .expect("title logo raw tilemap");

    let attr_table = general.metatile_attribute_table();
    assert!(!attr_table.is_empty());
    for attr in attr_table.attributes() {
        attr.unwrap_or_else(|e| panic!("metatile attribute decode failed: {e}"));
    }

    let table = LayoutTable::new();
    for (layout_id, pack_name) in [
        ("LAYOUT_LITTLEROOT_TOWN", "littleroot_town"),
        (
            "LAYOUT_LITTLEROOT_TOWN_BRENDANS_HOUSE_1F",
            "littleroot_town_brendans_house_1f",
        ),
        (
            "LAYOUT_LITTLEROOT_TOWN_BRENDANS_HOUSE_2F",
            "littleroot_town_brendans_house_2f",
        ),
        (
            "LAYOUT_LITTLEROOT_TOWN_MAYS_HOUSE_1F",
            "littleroot_town_mays_house_1f",
        ),
        (
            "LAYOUT_LITTLEROOT_TOWN_MAYS_HOUSE_2F",
            "littleroot_town_mays_house_2f",
        ),
        (
            "LAYOUT_LITTLEROOT_TOWN_PROFESSOR_BIRCHS_LAB",
            "littleroot_town_professor_birchs_lab",
        ),
        (
            "LAYOUT_LITTLEROOT_TOWN_PROFESSOR_BIRCHS_LAB_WITH_TABLE",
            "littleroot_town_professor_birchs_lab_with_table",
        ),
        ("LAYOUT_ROUTE101", "route101"),
        ("LAYOUT_OLDALE_TOWN", "oldale_town"),
        ("LAYOUT_ROUTE103", "route103"),
        ("LAYOUT_INSIDE_OF_TRUCK", "inside_of_truck"),
        ("LAYOUT_HOUSE1", "house1"),
        ("LAYOUT_HOUSE2", "house2"),
        ("LAYOUT_MART", "mart"),
        ("LAYOUT_POKEMON_CENTER_1F", "pokemon_center_1f"),
        ("LAYOUT_POKEMON_CENTER_2F", "pokemon_center_2f"),
    ] {
        let layout = table
            .layout(LayoutId(layout_id))
            .unwrap_or_else(|e| panic!("{layout_id} should be in LayoutTable: {e}"));
        let map_bytes = pack
            .layout_map(pack_name)
            .unwrap_or_else(|e| panic!("layout/{pack_name}/map should be in the pack: {e}"));
        let grid = layout
            .grid(map_bytes)
            .unwrap_or_else(|e| panic!("{layout_id}'s map.bin should decode: {e}"));
        assert_eq!(grid.cell_count(), layout.cell_count());

        let border_bytes = pack
            .layout_border(pack_name)
            .unwrap_or_else(|e| panic!("layout/{pack_name}/border should be in the pack: {e}"));
        let border = BorderGrid::new(border_bytes)
            .unwrap_or_else(|e| panic!("{layout_id}'s border.bin should decode: {e}"));
        assert_eq!(border.cells().count(), crate::map_layouts::BORDER_CELLS);
    }

    for font in FontId::ALL {
        let image = pack
            .font(font)
            .unwrap_or_else(|e| panic!("font `{}` should be in the pack: {e}", font.pack_name()));
        assert_eq!(image.image().width, crate::fonts::SHEET_WIDTH);
        assert_eq!(image.image().height, crate::fonts::SHEET_HEIGHT);
        let sheet = crate::fonts::FontGlyphSheet::new(image)
            .unwrap_or_else(|e| panic!("font `{}` sheet shape: {e}", font.pack_name()));
        let glyph = sheet.glyph(SPACE_GLYPH_INDEX).unwrap_or_else(|| {
            panic!(
                "font `{}` glyph {SPACE_GLYPH_INDEX} should decode",
                font.pack_name()
            )
        });
        assert_eq!(
            glyph.advance_width,
            font.glyph_width(SPACE_GLYPH_INDEX).unwrap()
        );
    }

    for frame_id in 0..EXPECTED_WINDOW_FRAME_COUNT {
        let frame = pack.text_window_frame(frame_id).unwrap_or_else(|e| {
            panic!("text-window frame id {frame_id} should be in the pack: {e}")
        });
        assert!(frame.tiles.width > 0 && frame.tiles.height > 0);
        assert_eq!(frame.palette.color_count, EXPECTED_PALETTE_COLOR_COUNT);
        assert_eq!(
            frame.palette.colors().count(),
            EXPECTED_PALETTE_COLOR_COUNT_USIZE
        );
    }
    let message_box = pack
        .message_box()
        .expect("message-box frame should be in the pack");
    assert!(message_box.tiles.width > 0 && message_box.tiles.height > 0);
    assert_eq!(
        message_box.palette.color_count,
        EXPECTED_PALETTE_COLOR_COUNT
    );
    for n in FIRST_EXTRA_PALETTE_NUMBER..=LAST_EXTRA_PALETTE_NUMBER {
        let palette = pack
            .text_window_extra_palette(n)
            .unwrap_or_else(|e| panic!("text-window extra palette {n} should be in the pack: {e}"));
        assert_eq!(palette.color_count, EXPECTED_PALETTE_COLOR_COUNT);
    }

    let naming = pack
        .naming_screen()
        .expect("naming-screen graphics should be in the pack");
    assert_eq!((naming.menu.width, naming.menu.height), (24, 128));
    assert_eq!(naming.background.len(), 32 * 20 * 2);
    assert_eq!(naming.keyboard_upper.len(), naming.background.len());
    assert_eq!(naming.brendan.image.width, 144);
    assert_eq!(naming.may.image.height, 32);
    assert_eq!(
        naming.text_palette.color_count,
        EXPECTED_PALETTE_COLOR_COUNT
    );
}

/// Keep expected sample ids independent of the extractor's manifest.
const REAL_PACK_DIRECT_SOUND_SAMPLES: [&str; EXPECTED_DIRECT_SOUND_SAMPLE_COUNT] = [
    "classical_choir_voice_ahhs",
    "ethnic_flavours_hyoushigi",
    "ethnic_flavours_ohtsuzumi",
    "sc88pro_flute",
    "sc88pro_french_horn_60",
    "sc88pro_french_horn_72",
    "sc88pro_glockenspiel",
    "sc88pro_harp",
    "sc88pro_mute_high_conga",
    "sc88pro_open_low_conga",
    "sc88pro_orchestra_cymbal_crash",
    "sc88pro_orchestra_snare",
    "sc88pro_piano1_48",
    "sc88pro_piano1_60",
    "sc88pro_piano1_72",
    "sc88pro_piano1_84",
    "sc88pro_rnd_kick",
    "sc88pro_rnd_snare",
    "sc88pro_string_ensemble_60",
    "sc88pro_string_ensemble_72",
    "sc88pro_string_ensemble_84",
    "sc88pro_tambourine",
    "sc88pro_timpani",
    "sc88pro_tr909_hand_clap",
    "sc88pro_trumpet_60",
    "sc88pro_trumpet_72",
    "sc88pro_trumpet_84",
    "sc88pro_tuba_39",
    "sc88pro_tuba_51",
    "sc88pro_tubular_bell",
    "sc88pro_xylophone",
    "sd90_classical_oboe",
    "trinity_cymbal_crash",
    "unknown_bell",
    "unknown_close_hihat",
    "unknown_open_hihat",
    "unknown_snare",
    "unused_guitar_separates_power_chord",
    "unused_sc55_tom",
    "unused_sc88pro_unison_slap",
    "unused_sd90_oboe",
];

const REAL_PACK_PROGRAMMABLE_WAVES: [u32; EXPECTED_PROGRAMMABLE_WAVE_COUNT] = [1, 2, 3, 5, 6];

// wav2agb `convert`/`convert_uncompressed_bin` (tools/wav2agb/converter.cpp): header
// overrides do not shorten the emitted PCM, so the pack keeps the sample past the logical count.
#[test]
#[ignore = "needs a local pack: run `cargo xtask extract` first"]
fn real_pack_audio_samples_decode_through_the_sample_schema() {
    use crate::audio::Sample;

    let pack = AssetPack::load_repo().expect("run `cargo xtask extract` first");

    let mut decoded = 0usize;
    for name in REAL_PACK_DIRECT_SOUND_SAMPLES {
        let id = format!("audio/sample/direct-sound/{name}");
        let bytes = pack
            .raw(&id)
            .unwrap_or_else(|e| panic!("`{id}` should be in the pack: {e}"));
        let Sample::DirectSound(sample) =
            Sample::decode(bytes).unwrap_or_else(|e| panic!("`{id}` should decode: {e}"))
        else {
            panic!("`{id}` should decode as a DirectSound sample, not a wave table");
        };
        assert!(!sample.data().is_empty(), "`{id}` should carry PCM");
        assert_ne!(sample.base_frequency, 0, "`{id}` should carry a pitch word");
        if let Some(start) = sample.loop_start() {
            let start = usize::try_from(start).expect("a real loop start fits a usize");
            let count = usize::try_from(sample.sample_count()).expect("a real count fits a usize");
            assert!(
                start < count,
                "`{id}`'s loop start {start} is past its {count} logical samples"
            );
        }
        assert_eq!(
            sample.data().len(),
            usize::try_from(sample.sample_count()).expect("a real count fits a usize")
                + INTERPOLATION_GUARD_SAMPLE_COUNT,
            "`{id}` should retain exactly one interpolation-guard sample"
        );
        decoded += 1;
    }

    for n in REAL_PACK_PROGRAMMABLE_WAVES {
        let id = format!("audio/sample/programmable-wave/{n:02}");
        let bytes = pack
            .raw(&id)
            .unwrap_or_else(|e| panic!("`{id}` should be in the pack: {e}"));
        let Sample::ProgrammableWave(_) =
            Sample::decode(bytes).unwrap_or_else(|e| panic!("`{id}` should decode: {e}"))
        else {
            panic!("`{id}` should decode as a programmable-wave table, not PCM");
        };
        decoded += 1;
    }
    assert_eq!(
        decoded, EXPECTED_MUS_TITLE_SAMPLE_COUNT,
        "the expected-id lists must cover every mus_title sample"
    );

    let flute_bytes = pack
        .raw("audio/sample/direct-sound/sc88pro_flute")
        .expect("the flute sample should be in the pack");
    let Sample::DirectSound(flute) =
        Sample::decode(flute_bytes).expect("the flute sample should decode")
    else {
        panic!("the flute sample should decode as a DirectSound sample");
    };
    assert_eq!(flute.base_frequency, FLUTE_AGBP_BASE_FREQUENCY);
    assert_eq!(flute.loop_start(), Some(FLUTE_SMPL_LOOP_START));
    assert_eq!(flute.sample_count(), FLUTE_AGBL_LOGICAL_SAMPLE_COUNT);
    assert_eq!(flute.data().len(), FLUTE_PCM_BUFFER_LENGTH);
    assert_eq!(
        flute.data()[FLUTE_INTERPOLATION_GUARD_INDEX],
        FLUTE_INTERPOLATION_GUARD_VALUE
    );
    assert_eq!(
        flute.data()[FLUTE_INTERPOLATION_GUARD_INDEX],
        flute.data()[FLUTE_LOOP_START_INDEX]
    );

    let wave_bytes = pack
        .raw("audio/sample/programmable-wave/01")
        .expect("programmable-wave 01 should be in the pack");
    let Sample::ProgrammableWave(wave) =
        Sample::decode(wave_bytes).expect("programmable-wave 01 should decode")
    else {
        panic!("programmable-wave 01 should decode as a wave table");
    };
    assert_eq!(wave.table, PROGRAMMABLE_WAVE_01_PCM_BYTES);
}

#[test]
#[ignore = "needs a local pack: run `cargo xtask extract` first"]
fn real_pack_audio_song_decodes_through_the_song_schema() {
    use crate::audio::{Song, SongEvent, VoiceGroupId};

    let pack = AssetPack::load_repo().expect("run `cargo xtask extract` first");
    let bytes = pack
        .raw("audio/song/mus_title")
        .expect("`audio/song/mus_title` should be in the pack");
    let song = Song::decode(bytes).expect("`audio/song/mus_title` should decode");

    assert_eq!(
        song.voicegroup(),
        &VoiceGroupId("audio/voicegroup/title".to_owned())
    );
    assert_eq!(song.priority(), 0);
    assert_eq!(song.reverb(), Some(50));
    assert_eq!(song.tracks().len(), 10);

    let track0 = &song.tracks()[0];
    assert_eq!(
        &track0[..6],
        [
            SongEvent::KeyShift(0),
            SongEvent::Tempo(144),
            SongEvent::Voice(14),
            SongEvent::Pan(40),
            SongEvent::LfoSpeed(44),
            SongEvent::Volume(86),
        ]
    );
    let last = track0.len();
    assert_eq!(
        &track0[last - 5..],
        [
            SongEvent::Volume(9),
            SongEvent::Wait(4),
            SongEvent::Volume(8),
            SongEvent::Wait(4),
            SongEvent::Fine,
        ]
    );

    let goto_count = song
        .tracks()
        .iter()
        .flatten()
        .filter(|e| matches!(e, SongEvent::Goto(_)))
        .count();
    assert_eq!(goto_count, 0);

    let volumes: Vec<u8> = song
        .tracks()
        .iter()
        .flatten()
        .filter_map(|e| match e {
            SongEvent::PseudoEchoVolume(v) => Some(*v),
            _ => None,
        })
        .collect();
    assert_eq!(volumes, MUS_TITLE_PSEUDO_ECHO_VOLUMES);
}

#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end walk from song to every voicegroup and sample; a split scatters one chain claim across ignored tests"
)]
#[test]
#[ignore = "needs a local pack: run `cargo xtask extract` first"]
fn real_pack_mus_title_data_chain_round_trips_through_the_typed_accessors() {
    use std::collections::HashSet;

    use crate::audio::VoiceEntry;

    let pack = AssetPack::load_repo().expect("run `cargo xtask extract` first");

    let song = pack
        .song("mus_title")
        .expect("`audio/song/mus_title` should load through `AssetPack::song`");

    let mut selected_voices: Vec<u8> = song
        .tracks()
        .iter()
        .flatten()
        .filter_map(|event| match event {
            SongEvent::Voice(index) => Some(*index),
            _ => None,
        })
        .collect();
    selected_voices.sort_unstable();
    selected_voices.dedup();
    assert!(
        !selected_voices.is_empty(),
        "mus_title should select at least one VOICE"
    );

    let mut visited_groups: HashSet<VoiceGroupId> = HashSet::new();
    let mut pending_groups = vec![song.voicegroup().clone()];
    let mut visited_samples: HashSet<SampleId> = HashSet::new();
    let mut leaf_voice_count = 0usize;

    while let Some(group_id) = pending_groups.pop() {
        if !visited_groups.insert(group_id.clone()) {
            continue;
        }
        let group = pack
            .voicegroup(&group_id)
            .unwrap_or_else(|e| panic!("`{}` should resolve: {e}", group_id.0));
        assert_eq!(
            group.slots().len(),
            crate::audio::VOICE_SLOT_COUNT,
            "`{}` should carry every voicegroup slot",
            group_id.0
        );
        for slot in group.slots() {
            match slot {
                VoiceEntry::DirectSound(voice) => {
                    if visited_samples.insert(voice.sample.clone()) {
                        pack.sample(&voice.sample)
                            .unwrap_or_else(|e| panic!("`{}` should decode: {e}", voice.sample.0));
                    }
                    leaf_voice_count += 1;
                }
                VoiceEntry::ProgrammableWave(voice) => {
                    if visited_samples.insert(voice.wave.clone()) {
                        pack.sample(&voice.wave)
                            .unwrap_or_else(|e| panic!("`{}` should decode: {e}", voice.wave.0));
                    }
                    leaf_voice_count += 1;
                }
                VoiceEntry::Square1(_) | VoiceEntry::Square2(_) | VoiceEntry::Noise(_) => {
                    leaf_voice_count += 1;
                }
                VoiceEntry::KeySplit(voice) => pending_groups.push(voice.children.clone()),
                VoiceEntry::Rhythm(voice) => pending_groups.push(voice.children.clone()),
                VoiceEntry::Empty => {}
            }
        }
    }

    let mut visited_labels: Vec<&str> = visited_groups
        .iter()
        .map(|id| {
            id.0.strip_prefix("audio/voicegroup/")
                .unwrap_or_else(|| panic!("`{}` should carry the audio/voicegroup/ prefix", id.0))
        })
        .collect();
    visited_labels.sort_unstable();
    let mut expected_labels = [
        "french_horn_keysplit",
        "piano_keysplit",
        "rs_drumset",
        "strings_keysplit",
        "title",
        "trumpet_keysplit",
        "tuba_keysplit",
    ];
    expected_labels.sort_unstable();
    assert_eq!(
        visited_labels, expected_labels,
        "the voicegroup closure reached from mus_title's own song entry should be exactly \
         the seven expected voicegroups"
    );
    assert!(
        leaf_voice_count > 0,
        "the walk should have resolved at least one concrete (non-indirection) voice"
    );
    assert_eq!(
        visited_samples.len(),
        EXPECTED_MUS_TITLE_SAMPLE_COUNT,
        "the voicegroup closure should reference every mus_title sample"
    );

    // `ply_voice` (src/m4a_1.s) indexes tone records without a declared-slot bound, so
    // undeclared title voices must resolve to real entries.
    let title = pack
        .voicegroup(song.voicegroup())
        .expect("the top-level `title` group should already be in `visited_groups`");
    for index in selected_voices {
        let entry = title
            .slot(usize::from(index))
            .unwrap_or_else(|| panic!("VOICE {index} has no slot in `title`"));
        assert!(
            !matches!(entry, VoiceEntry::Empty),
            "VOICE {index}, selected by mus_title's own event stream, resolves to an \
             empty placeholder in `title`"
        );
    }
}
