//! Typed naming-screen graphics (upstream `src/naming_screen.c`).
//!
//! Pack ids stay private so scene code composes no strings. Every lookup
//! re-validates shapes, since a hand-built or corrupt pack can bypass extraction.
//! Sheets are 4-bit: a pixel indexes one 16-colour bank, and index 0 is
//! transparent wherever a sheet is drawn as a sprite.

use super::{AssetPack, ImageRef, PackError, PaletteRef, PalettedImage};

const BIT_DEPTH: u8 = 4;
const BANK_COLORS: u16 = 16;
/// `GetTextWindowPalette(2)`, loaded at background palette 11 for the banner.
const TEXT_WINDOW_PALETTE: u8 = 2;
/// A 32x20 screen of two-byte tilemap entries.
const TILEMAP_BYTES: usize = 32 * 20 * 2;

const MENU_IMAGE: &str = "naming-screen/image/menu";
const MENU_SHAPE: (u32, u32) = (24, 128);
/// `gNamingScreenMenu_Pal`, in the order `LoadPalette` copies the banks.
const MENU_PALETTES: [&str; 6] = [
    "naming-screen/palette/menu",
    "naming-screen/palette/page_swap_upper",
    "naming-screen/palette/page_swap_lower",
    "naming-screen/palette/page_swap_others",
    "naming-screen/palette/buttons",
    "naming-screen/palette/cursor",
];
const KEYBOARD_PALETTE: &str = "naming-screen/palette/keyboard";

const BACKGROUND: &str = "naming-screen/raw/background";
const KEYBOARD_UPPER: &str = "naming-screen/raw/keyboard_upper";
const KEYBOARD_LOWER: &str = "naming-screen/raw/keyboard_lower";
const KEYBOARD_SYMBOLS: &str = "naming-screen/raw/keyboard_symbols";

const PAGE_SWAP_FRAME: &str = "naming-screen/image/page_swap_frame";
const PAGE_SWAP_BUTTON: &str = "naming-screen/image/page_swap_button";
const PAGE_SWAP_UPPER: &str = "naming-screen/image/page_swap_upper";
const PAGE_SWAP_LOWER: &str = "naming-screen/image/page_swap_lower";
const PAGE_SWAP_OTHERS: &str = "naming-screen/image/page_swap_others";
const BACK_BUTTON: &str = "naming-screen/image/back_button";
const OK_BUTTON: &str = "naming-screen/image/ok_button";
const CURSOR: &str = "naming-screen/image/cursor";
const CURSOR_SQUISHED: &str = "naming-screen/image/cursor_squished";
const CURSOR_FILLED: &str = "naming-screen/image/cursor_filled";
const INPUT_ARROW: &str = "naming-screen/image/input_arrow";
const UNDERSCORE: &str = "naming-screen/image/underscore";
const PC_ICON_OFF: &str = "naming-screen/image/pc_icon_off";
const PC_ICON_ON: &str = "naming-screen/image/pc_icon_on";

const BRENDAN_SHEET: &str = "sprite/brendan/walking";
const MAY_SHEET: &str = "sprite/may/walking";
const BRENDAN_PALETTE: &str = "sprite/palette/brendan";
const MAY_PALETTE: &str = "sprite/palette/may";

const FRAME_SHAPE: (u32, u32) = (40, 32);
const BUTTON_SHAPE: (u32, u32) = (40, 24);
const PAGE_LABEL_SHAPE: (u32, u32) = (40, 8);
const PAGE_BUTTON_SHAPE: (u32, u32) = (32, 16);
/// The cursor sheets are published as the five tiles upstream's build keeps.
const CURSOR_SHAPE: (u32, u32) = (8, 40);
const MARK_SHAPE: (u32, u32) = (8, 8);
const PC_ICON_SHAPE: (u32, u32) = (16, 24);
/// Nine 16x32 walk frames (`overworld_frame` of the player object event).
const PLAYER_SHAPE: (u32, u32) = (144, 32);

/// One bank of `gNamingScreenMenu_Pal`.
///
/// [`NamingScreenHandle::menu_palette`] returns that bank. Sprite templates
/// select these same banks by palette tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamingMenuPalette {
    /// `menu.pal`. Also the PC icon's sprite palette.
    Menu,
    /// `page_swap_upper.pal`. One bank the page-swap button can show.
    PageSwapUpper,
    /// `page_swap_lower.pal`. One bank the page-swap button can show.
    PageSwapLower,
    /// `page_swap_others.pal`. One bank the page-swap button can show, and the
    /// input arrow and underscore palette.
    PageSwapOthers,
    /// `buttons.pal`. The page-swap frame and its lettering, and Back and OK.
    Buttons,
    /// `cursor.pal`.
    Cursor,
}

impl NamingMenuPalette {
    const fn index(self) -> usize {
        match self {
            Self::Menu => 0,
            Self::PageSwapUpper => 1,
            Self::PageSwapLower => 2,
            Self::PageSwapOthers => 3,
            Self::Buttons => 4,
            Self::Cursor => 5,
        }
    }
}

/// Every graphic the naming screen draws, borrowing the pack for `'a`.
///
/// [`AssetPack::naming_screen`] returns it with every entry validated.
/// The banner's text colour is [`text_palette`](Self::text_palette), which is
/// `GetTextWindowPalette(2)` and not a naming-screen file.
#[derive(Debug, Clone, Copy)]
pub struct NamingScreenHandle<'a> {
    /// The 32x20 background tilemap (`background.bin`).
    pub background: &'a [u8],
    /// The 32x20 upper-letters keyboard tilemap.
    pub keyboard_upper: &'a [u8],
    /// The 32x20 lower-letters keyboard tilemap.
    pub keyboard_lower: &'a [u8],
    /// The 32x20 symbols keyboard tilemap.
    pub keyboard_symbols: &'a [u8],
    /// The 24x128 menu tile sheet. A pixel indexes the bank
    /// [`menu_palette`](Self::menu_palette) the background tilemap selects.
    pub menu: ImageRef<'a>,
    /// `sKeyboard_Pal`, loaded at background palette 10.
    pub keyboard_palette: PaletteRef<'a>,
    /// `GetTextWindowPalette(2)`, loaded at background palette 11.
    pub text_palette: PaletteRef<'a>,
    /// Page-swap frame, drawn with the buttons bank.
    pub page_swap_frame: PalettedImage<'a>,
    /// Page-swap button. `SetPageSwapButtonGfx` recolours it with the upper,
    /// lower, or others bank, all of which cover its pixels.
    pub page_swap_button: ImageRef<'a>,
    /// "UPPER" lettering, drawn with the buttons bank.
    pub page_swap_upper: PalettedImage<'a>,
    /// "LOWER" lettering, drawn with the buttons bank.
    pub page_swap_lower: PalettedImage<'a>,
    /// "OTHERS" lettering, drawn with the buttons bank.
    pub page_swap_others: PalettedImage<'a>,
    /// Back button, drawn with the buttons bank.
    pub back_button: PalettedImage<'a>,
    /// OK button, drawn with the buttons bank.
    pub ok_button: PalettedImage<'a>,
    /// Cursor, and the squished and filled frames the cursor anim swaps in.
    pub cursor: PalettedImage<'a>,
    /// Squished cursor frame, sharing [`cursor`](Self::cursor)'s palette.
    pub cursor_squished: PalettedImage<'a>,
    /// Filled cursor frame, sharing [`cursor`](Self::cursor)'s palette.
    pub cursor_filled: PalettedImage<'a>,
    /// Input arrow, drawn with the page-swap others bank.
    pub input_arrow: PalettedImage<'a>,
    /// Underscore, drawn with the page-swap others bank.
    pub underscore: PalettedImage<'a>,
    /// PC icon, off, drawn with the menu bank.
    pub pc_icon_off: PalettedImage<'a>,
    /// PC icon, on, drawn with the menu bank.
    pub pc_icon_on: PalettedImage<'a>,
    /// Brendan's walking object-event sheet, the male player icon.
    pub brendan: PalettedImage<'a>,
    /// May's walking object-event sheet, the female player icon.
    pub may: PalettedImage<'a>,
    menu_palettes: [PaletteRef<'a>; 6],
}

impl<'a> NamingScreenHandle<'a> {
    /// The named bank of `gNamingScreenMenu_Pal`.
    #[must_use]
    pub const fn menu_palette(self, bank: NamingMenuPalette) -> PaletteRef<'a> {
        self.menu_palettes[bank.index()]
    }
}

impl AssetPack {
    /// Bundle every naming-screen graphic, including the player-icon sheets.
    ///
    /// # Errors
    ///
    /// [`PackError::UnknownAsset`] or [`PackError::WrongKind`] for a missing or
    /// wrong-kind entry; [`PackError::MalformedNamingScreenAsset`] when
    /// dimensions, bit depth (4 for every sheet), palette size, pixel indices,
    /// or tilemap length differ from upstream. The banner palette reports the
    /// text-window errors of [`text_window_extra_palette`](Self::text_window_extra_palette).
    pub fn naming_screen(&self) -> Result<NamingScreenHandle<'_>, PackError> {
        let menu_palettes = [
            self.bank(MENU_PALETTES[0])?,
            self.bank(MENU_PALETTES[1])?,
            self.bank(MENU_PALETTES[2])?,
            self.bank(MENU_PALETTES[3])?,
            self.bank(MENU_PALETTES[4])?,
            self.bank(MENU_PALETTES[5])?,
        ];
        let menu = self.sheet(
            MENU_IMAGE,
            MENU_PALETTES[NamingMenuPalette::Menu.index()],
            MENU_SHAPE,
        )?;
        let buttons = MENU_PALETTES[NamingMenuPalette::Buttons.index()];
        let others = MENU_PALETTES[NamingMenuPalette::PageSwapOthers.index()];
        let cursor_palette = MENU_PALETTES[NamingMenuPalette::Cursor.index()];
        let menu_palette = MENU_PALETTES[NamingMenuPalette::Menu.index()];
        // The button is recoloured with each page's bank, so each must cover it.
        let page_swap_button = self.sheet(PAGE_SWAP_BUTTON, MENU_PALETTES[1], PAGE_BUTTON_SHAPE)?;
        self.sheet(PAGE_SWAP_BUTTON, MENU_PALETTES[2], PAGE_BUTTON_SHAPE)?;
        self.sheet(PAGE_SWAP_BUTTON, MENU_PALETTES[3], PAGE_BUTTON_SHAPE)?;
        Ok(NamingScreenHandle {
            background: self.tilemap(BACKGROUND)?,
            keyboard_upper: self.tilemap(KEYBOARD_UPPER)?,
            keyboard_lower: self.tilemap(KEYBOARD_LOWER)?,
            keyboard_symbols: self.tilemap(KEYBOARD_SYMBOLS)?,
            menu: menu.image,
            keyboard_palette: self.bank(KEYBOARD_PALETTE)?,
            text_palette: self.text_window_extra_palette(TEXT_WINDOW_PALETTE)?,
            page_swap_frame: self.sheet(PAGE_SWAP_FRAME, buttons, FRAME_SHAPE)?,
            page_swap_button: page_swap_button.image,
            page_swap_upper: self.sheet(PAGE_SWAP_UPPER, buttons, PAGE_LABEL_SHAPE)?,
            page_swap_lower: self.sheet(PAGE_SWAP_LOWER, buttons, PAGE_LABEL_SHAPE)?,
            page_swap_others: self.sheet(PAGE_SWAP_OTHERS, buttons, PAGE_LABEL_SHAPE)?,
            back_button: self.sheet(BACK_BUTTON, buttons, BUTTON_SHAPE)?,
            ok_button: self.sheet(OK_BUTTON, buttons, BUTTON_SHAPE)?,
            cursor: self.sheet(CURSOR, cursor_palette, CURSOR_SHAPE)?,
            cursor_squished: self.sheet(CURSOR_SQUISHED, cursor_palette, CURSOR_SHAPE)?,
            cursor_filled: self.sheet(CURSOR_FILLED, cursor_palette, CURSOR_SHAPE)?,
            input_arrow: self.sheet(INPUT_ARROW, others, MARK_SHAPE)?,
            underscore: self.sheet(UNDERSCORE, others, MARK_SHAPE)?,
            pc_icon_off: self.sheet(PC_ICON_OFF, menu_palette, PC_ICON_SHAPE)?,
            pc_icon_on: self.sheet(PC_ICON_ON, menu_palette, PC_ICON_SHAPE)?,
            brendan: self.sheet(BRENDAN_SHEET, BRENDAN_PALETTE, PLAYER_SHAPE)?,
            may: self.sheet(MAY_SHEET, MAY_PALETTE, PLAYER_SHAPE)?,
            menu_palettes,
        })
    }

    fn sheet(
        &self,
        image_id: &str,
        palette_id: &str,
        shape: (u32, u32),
    ) -> Result<PalettedImage<'_>, PackError> {
        self.paired_image(
            image_id,
            palette_id,
            shape,
            BIT_DEPTH,
            BANK_COLORS,
            |id, detail| PackError::MalformedNamingScreenAsset { id, detail },
        )
    }

    fn bank(&self, id: &str) -> Result<PaletteRef<'_>, PackError> {
        let palette = self.palette(id)?;
        if palette.color_count != BANK_COLORS || palette.raw.len() != usize::from(BANK_COLORS) * 2 {
            return Err(PackError::MalformedNamingScreenAsset {
                id: id.to_owned(),
                detail: format!(
                    "declares {} colours in {} bytes: expected {BANK_COLORS} colours",
                    palette.color_count,
                    palette.raw.len()
                ),
            });
        }
        Ok(palette)
    }

    fn tilemap(&self, id: &str) -> Result<&[u8], PackError> {
        let bytes = self.raw(id)?;
        if bytes.len() != TILEMAP_BYTES {
            return Err(PackError::MalformedNamingScreenAsset {
                id: id.to_owned(),
                detail: format!("is {} bytes: expected {TILEMAP_BYTES}", bytes.len()),
            });
        }
        Ok(bytes)
    }
}
