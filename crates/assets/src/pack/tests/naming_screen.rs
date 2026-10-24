use super::super::{AssetPack, NamingMenuPalette, PackError};
use super::shared::{
    image_meta, pack_bytes, write_pack, Entry, IMAGE_KIND_TAG, INDEXED_8_BIT, PALETTE_KIND_TAG,
    RAW_KIND_TAG, TEXT_WINDOW_BIT_DEPTH,
};

const TILEMAP_BYTES: usize = 1280;
const WALK_SHAPE: (u32, u32) = (144, 32);

fn image(id: &'static str, side: (u32, u32), depth: u8, pixel: u8) -> Entry {
    Entry {
        id,
        kind_tag: IMAGE_KIND_TAG,
        meta: image_meta(side.0, side.1, depth),
        payload: vec![pixel; (side.0 * side.1) as usize],
    }
}

fn palette(id: &'static str, colors: u16, first: u16) -> Entry {
    let mut payload = vec![0u8; usize::from(colors) * 2];
    payload[..2].copy_from_slice(&first.to_le_bytes());
    Entry {
        id,
        kind_tag: PALETTE_KIND_TAG,
        meta: colors.to_le_bytes().to_vec(),
        payload,
    }
}

fn raw(id: &'static str, len: usize, fill: u8) -> Entry {
    Entry {
        id,
        kind_tag: RAW_KIND_TAG,
        meta: vec![],
        payload: vec![fill; len],
    }
}

/// Every id the handle reads, with the shape a correct pack gives it.
fn naming_entries() -> Vec<Entry> {
    let images = [
        ("naming-screen/image/menu", (24, 128), 1u8),
        ("naming-screen/image/page_swap_frame", (40, 32), 2),
        ("naming-screen/image/page_swap_button", (32, 16), 3),
        ("naming-screen/image/page_swap_upper", (40, 8), 4),
        ("naming-screen/image/page_swap_lower", (40, 8), 5),
        ("naming-screen/image/page_swap_others", (40, 8), 6),
        ("naming-screen/image/back_button", (40, 24), 7),
        ("naming-screen/image/ok_button", (40, 24), 8),
        ("naming-screen/image/cursor", (8, 40), 9),
        ("naming-screen/image/cursor_squished", (8, 40), 10),
        ("naming-screen/image/cursor_filled", (8, 40), 11),
        ("naming-screen/image/input_arrow", (8, 8), 12),
        ("naming-screen/image/underscore", (8, 8), 13),
        ("naming-screen/image/pc_icon_off", (16, 24), 14),
        ("naming-screen/image/pc_icon_on", (16, 24), 15),
        ("sprite/brendan/walking", WALK_SHAPE, 2),
        ("sprite/may/walking", WALK_SHAPE, 3),
    ];
    let palettes = [
        ("naming-screen/palette/menu", 0x1111u16),
        ("naming-screen/palette/page_swap_upper", 0x2222),
        ("naming-screen/palette/page_swap_lower", 0x3333),
        ("naming-screen/palette/page_swap_others", 0x4444),
        ("naming-screen/palette/buttons", 0x5555),
        ("naming-screen/palette/cursor", 0x6666),
        ("naming-screen/palette/keyboard", 0x7777),
        ("sprite/palette/brendan", 0x8888),
        ("sprite/palette/may", 0x9999),
        ("text-window/palette/text_pal2", 0xAAAA),
    ];
    let tilemaps = [
        ("naming-screen/raw/background", 0xB1u8),
        ("naming-screen/raw/keyboard_upper", 0xB2),
        ("naming-screen/raw/keyboard_lower", 0xB3),
        ("naming-screen/raw/keyboard_symbols", 0xB4),
    ];
    images
        .into_iter()
        .map(|(id, side, pixel)| image(id, side, TEXT_WINDOW_BIT_DEPTH, pixel))
        .chain(
            palettes
                .into_iter()
                .map(|(id, first)| palette(id, 16, first)),
        )
        .chain(
            tilemaps
                .into_iter()
                .map(|(id, fill)| raw(id, TILEMAP_BYTES, fill)),
        )
        .collect()
}

fn load(name: &str, entries: Vec<Entry>) -> (AssetPack, std::path::PathBuf) {
    let path = write_pack(name, &pack_bytes(entries));
    (AssetPack::load(&path).unwrap(), path)
}

fn without(id: &str) -> Vec<Entry> {
    naming_entries()
        .into_iter()
        .filter(|e| e.id != id)
        .collect()
}

fn replaced(id: &str, entry: Entry) -> Vec<Entry> {
    let mut entries = without(id);
    entries.push(entry);
    entries
}

#[test]
fn naming_screen_bundles_every_graphic_the_scene_draws() {
    let (pack, path) = load("naming-screen", naming_entries());
    let screen = pack.naming_screen().unwrap();

    assert_eq!((screen.menu.width, screen.menu.height), (24, 128));
    assert_eq!(screen.menu.bit_depth, 4);
    assert_eq!(screen.menu.pixels[0], 1);
    assert_eq!(
        screen.menu_palette(NamingMenuPalette::Menu).color(0),
        Some(0x1111)
    );
    assert_eq!(
        screen.menu_palette(NamingMenuPalette::Buttons).color(0),
        Some(0x5555)
    );
    assert_eq!(
        screen.menu_palette(NamingMenuPalette::Cursor).color(0),
        Some(0x6666)
    );
    assert_eq!(screen.keyboard_palette.color(0), Some(0x7777));
    assert_eq!(screen.text_palette.color(0), Some(0xAAAA));
    assert_eq!(screen.background, vec![0xB1; TILEMAP_BYTES].as_slice());
    assert_eq!(screen.keyboard_upper, vec![0xB2; TILEMAP_BYTES].as_slice());
    assert_eq!(screen.keyboard_lower, vec![0xB3; TILEMAP_BYTES].as_slice());
    assert_eq!(
        screen.keyboard_symbols,
        vec![0xB4; TILEMAP_BYTES].as_slice()
    );
    assert_eq!(screen.page_swap_frame.image.pixels[0], 2);
    assert_eq!(screen.page_swap_frame.palette.color(0), Some(0x5555));
    assert_eq!(screen.page_swap_button.pixels[0], 3);
    assert_eq!(screen.page_swap_upper.image.pixels[0], 4);
    assert_eq!(screen.back_button.image.pixels[0], 7);
    assert_eq!(screen.ok_button.palette.color(0), Some(0x5555));
    assert_eq!(
        (screen.cursor.image.width, screen.cursor.image.height),
        (8, 40)
    );
    assert_eq!(screen.cursor_squished.image.pixels[0], 10);
    assert_eq!(screen.cursor_filled.palette.color(0), Some(0x6666));
    assert_eq!(screen.input_arrow.palette.color(0), Some(0x4444));
    assert_eq!(screen.underscore.image.pixels[0], 13);
    assert_eq!(screen.pc_icon_off.palette.color(0), Some(0x1111));
    assert_eq!(screen.pc_icon_on.image.pixels[0], 15);
    assert_eq!(screen.brendan.image.width, 144);
    assert_eq!(screen.brendan.palette.color(0), Some(0x8888));
    assert_eq!(screen.may.image.height, 32);
    assert_eq!(screen.may.palette.color(0), Some(0x9999));

    let _ = std::fs::remove_file(path);
}

#[test]
fn each_naming_screen_entry_is_required_and_kind_checked() {
    let ids: Vec<&str> = naming_entries().iter().map(|e| e.id).collect();
    for id in ids {
        let (pack, path) = load(
            &format!("naming-missing-{}", id.replace('/', "-")),
            without(id),
        );
        assert!(
            matches!(pack.naming_screen().unwrap_err(), PackError::UnknownAsset(missing) if missing == id),
            "missing `{id}` should be unknown"
        );
        let _ = std::fs::remove_file(path);

        let entry = naming_entries().into_iter().find(|e| e.id == id).unwrap();
        let wrong = match entry.kind_tag {
            IMAGE_KIND_TAG | PALETTE_KIND_TAG => raw(id, 4, 0),
            RAW_KIND_TAG => image(id, (2, 2), INDEXED_8_BIT, 0),
            other => panic!("unexpected kind {other}"),
        };
        let (pack, path) = load(
            &format!("naming-kind-{}", id.replace('/', "-")),
            replaced(id, wrong),
        );
        let err = pack.naming_screen().unwrap_err();
        assert!(
            matches!(err, PackError::WrongKind { id: ref bad, .. } if bad == id),
            "wrong kind for `{id}` reported {err}"
        );
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn each_naming_screen_entry_rejects_the_wrong_shape() {
    let images = [
        ("naming-screen/image/menu", (24, 128)),
        ("naming-screen/image/page_swap_frame", (40, 32)),
        ("naming-screen/image/page_swap_button", (32, 16)),
        ("naming-screen/image/page_swap_upper", (40, 8)),
        ("naming-screen/image/page_swap_lower", (40, 8)),
        ("naming-screen/image/page_swap_others", (40, 8)),
        ("naming-screen/image/back_button", (40, 24)),
        ("naming-screen/image/ok_button", (40, 24)),
        ("naming-screen/image/cursor", (8, 40)),
        ("naming-screen/image/cursor_squished", (8, 40)),
        ("naming-screen/image/cursor_filled", (8, 40)),
        ("naming-screen/image/input_arrow", (8, 8)),
        ("naming-screen/image/underscore", (8, 8)),
        ("naming-screen/image/pc_icon_off", (16, 24)),
        ("naming-screen/image/pc_icon_on", (16, 24)),
        ("sprite/brendan/walking", WALK_SHAPE),
        ("sprite/may/walking", WALK_SHAPE),
    ];
    let palettes = [
        "naming-screen/palette/menu",
        "naming-screen/palette/page_swap_upper",
        "naming-screen/palette/page_swap_lower",
        "naming-screen/palette/page_swap_others",
        "naming-screen/palette/buttons",
        "naming-screen/palette/cursor",
        "naming-screen/palette/keyboard",
        "sprite/palette/brendan",
        "sprite/palette/may",
    ];
    let tilemaps = [
        "naming-screen/raw/background",
        "naming-screen/raw/keyboard_upper",
        "naming-screen/raw/keyboard_lower",
        "naming-screen/raw/keyboard_symbols",
    ];
    let mut cases = Vec::new();
    for (id, side) in images {
        let wrong = if side == (8, 8) { (16, 16) } else { (8, 8) };
        cases.push(image(id, wrong, TEXT_WINDOW_BIT_DEPTH, 1));
    }
    cases.push(image(
        "naming-screen/image/menu",
        (24, 128),
        INDEXED_8_BIT,
        1,
    ));
    cases.push(image(
        "naming-screen/image/menu",
        (24, 128),
        TEXT_WINDOW_BIT_DEPTH,
        16,
    ));
    for id in palettes {
        cases.push(palette(id, 8, 0));
    }
    for id in tilemaps {
        cases.push(raw(id, TILEMAP_BYTES - 2, 0));
    }
    for (n, entry) in cases.into_iter().enumerate() {
        let id = entry.id;
        let (pack, path) = load(&format!("naming-shape-{n}"), replaced(id, entry));
        let err = pack.naming_screen().unwrap_err();
        assert!(
            matches!(err, PackError::MalformedNamingScreenAsset { id: ref bad, .. } if bad == id),
            "wrong shape for `{id}` reported {err}"
        );
        let _ = std::fs::remove_file(path);
    }

    let (pack, path) = load(
        "naming-text-palette-short",
        replaced(
            "text-window/palette/text_pal2",
            palette("text-window/palette/text_pal2", 2, 0),
        ),
    );
    assert!(matches!(
        pack.naming_screen().unwrap_err(),
        PackError::MalformedTextWindowPalette { id, .. } if id == "text-window/palette/text_pal2"
    ));
    let _ = std::fs::remove_file(path);
}
