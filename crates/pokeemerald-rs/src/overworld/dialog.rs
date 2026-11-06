//! Field message boxes drawn over the composed overworld frame.
//!
//! A dialog advances its text printer once per frame. Field-script dialogs
//! preserve their final text until a fresh A or B press closes the box.
//!
//! # Held-A/B print speed-up
//!
//! Field messages enable delay skipping when they create their text printer
//! (`pokeemerald/src/field_message_box.c:62-69,109-129`). [`NpcDialog::new`]
//! therefore accelerates printing while either confirm button is held.
//!
//! # Script-level `waitbuttonpress`
//!
//! The field script waits for a fresh A or B press only after printing finishes
//! (`pokeemerald/data/scripts/std_msgbox.inc:1-22` and
//! `pokeemerald/src/scrcmd.c:1323-1336`). [`NpcDialog::with_waitbuttonpress`]
//! models that separate wait without clearing the final printed frame.

use assets::fonts::{FontId, OwnedFontGlyphSheet};
use assets::pack::{AssetPack, PackError};
use engine::text::render::{Printer, PrinterInput, TextSpeed, TickEvent};
use engine::text::window::MessageBoxLayout;
use engine::text::Token;
use platform::{ButtonState, Buttons};
use rendering::Framebuffer;

use crate::textbox::{self, FrameAssets, WindowOp};

/// Maps A/B button edges and holds to the shared printer input shape.
///
/// Fresh edges advance prompts and close waits. Held states accelerate printing.
/// Keeping the mapping here prevents field and save dialogs from drifting.
pub(crate) fn confirm_printer_input(buttons: ButtonState) -> PrinterInput {
    PrinterInput {
        a_pressed: buttons.is_newly_pressed(Buttons::A),
        b_pressed: buttons.is_newly_pressed(Buttons::B),
        a_held: buttons.is_held(Buttons::A),
        b_held: buttons.is_held(Buttons::B),
    }
}

/// An error while loading an [`NpcDialog`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NpcDialogError {
    /// The asset pack could not provide the dialog resources.
    Pack(PackError),
    /// The font glyph sheet could not be decoded.
    Font(assets::AssetError),
}

impl std::fmt::Display for NpcDialogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pack(err) => write!(f, "npc dialog: {err}"),
            Self::Font(err) => write!(f, "npc dialog: {err}"),
        }
    }
}

impl std::error::Error for NpcDialogError {}

impl From<PackError> for NpcDialogError {
    fn from(err: PackError) -> Self {
        Self::Pack(err)
    }
}

impl From<assets::AssetError> for NpcDialogError {
    fn from(err: assets::AssetError) -> Self {
        Self::Font(err)
    }
}

/// Whether an [`NpcDialog`] still owns the current frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DialogOutcome {
    /// Keep routing frames to the dialog.
    Continue,
    /// Drop the dialog and resume its caller.
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DialogState {
    Printing,
    AwaitingButtonPress,
    Closed,
}

/// An open field message box.
#[derive(Debug)]
pub(crate) struct NpcDialog {
    frame: FrameAssets,
    printer: Printer<OwnedFontGlyphSheet>,
    ops: Vec<WindowOp>,
    state: DialogState,
    wait_for_button_press: bool,
}

impl NpcDialog {
    /// Creates the standard field message box from decoded assets.
    ///
    /// NPC and save messages share this type because both use the standard field
    /// message resources. Holding A or B accelerates printing.
    pub(crate) fn new(
        sheet: OwnedFontGlyphSheet,
        frame: FrameAssets,
        tokens: Vec<Token>,
        text_speed: TextSpeed,
    ) -> Self {
        let printer = Printer::new(tokens, sheet, text_speed, textbox::STANDARD_PRINTER_ORIGIN)
            .with_ab_speed_up_print();
        Self {
            frame,
            printer,
            ops: Vec::new(),
            state: DialogState::Printing,
            wait_for_button_press: false,
        }
    }

    /// Keeps the final printed frame visible until a fresh A or B press closes it.
    #[must_use]
    pub(crate) const fn with_waitbuttonpress(mut self) -> Self {
        self.wait_for_button_press = true;
        self
    }

    /// Builds a confirm-to-close dialog from an already-loaded asset pack, at
    /// a caller-chosen `text_speed`.
    ///
    /// # Errors
    ///
    /// Returns [`NpcDialogError::Pack`] for missing or malformed pack entries,
    /// and [`NpcDialogError::Font`] when the font sheet cannot be decoded.
    pub(crate) fn from_pack_at_speed(
        pack: &AssetPack,
        tokens: Vec<Token>,
        text_speed: TextSpeed,
    ) -> Result<Self, NpcDialogError> {
        let sheet = OwnedFontGlyphSheet::new(pack.font(FontId::Normal)?)?;
        let frame = FrameAssets::from_handle(pack.message_box()?);
        Ok(Self::new(sheet, frame, tokens, text_speed).with_waitbuttonpress())
    }

    /// Loads an asset pack and opens a confirm-to-close dialog, at a
    /// caller-chosen `text_speed` -- see [`Self::from_pack_at_speed`].
    ///
    /// # Errors
    ///
    /// Returns [`NpcDialogError::Pack`] when the pack cannot be loaded or read,
    /// and [`NpcDialogError::Font`] when the font sheet cannot be decoded.
    pub(crate) fn open_at_speed(
        source: &crate::pack_source::PackSource,
        tokens: Vec<Token>,
        text_speed: TextSpeed,
    ) -> Result<Self, NpcDialogError> {
        let pack = source.load()?;
        Self::from_pack_at_speed(&pack, tokens, text_speed)
    }

    /// Returns the number of glyphs currently retained (revealed, and not
    /// since dropped by a page or window clear).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn revealed_glyph_count(&self) -> usize {
        self.ops
            .iter()
            .filter(|op| matches!(op, WindowOp::Glyph(_)))
            .count()
    }

    /// Advances the dialog by one frame and reports whether it remains open.
    ///
    /// Fresh A/B edges advance printer prompts and the final script wait. Held A/B
    /// states accelerate printing without satisfying that final wait.
    pub(crate) fn tick(&mut self, input: PrinterInput) -> DialogOutcome {
        match self.state {
            DialogState::Closed => return DialogOutcome::Closed,
            DialogState::AwaitingButtonPress => {
                if input.confirm_pressed() {
                    self.state = DialogState::Closed;
                    return DialogOutcome::Closed;
                }
                return DialogOutcome::Continue;
            }
            DialogState::Printing => {}
        }

        let event = self.printer.tick(input);
        // Checked ahead of `event` so a same-tick `FILL_WINDOW` glyph
        // survives the clear it follows; see `Printer::cleared_window`.
        if self.printer.cleared_window() {
            self.ops.clear();
        }
        // Polled right after `tick`. A positive `CLEAR`/`CLEAR_TO` ends the
        // frame as `TickEvent::Idle`. A `MIN_LETTER_SPACING` gap is the
        // exception: it is cleared on the same tick as the glyph it follows,
        // and the span starts at the glyph's far edge so the two do not
        // overlap.
        if let Some(span) = self.printer.cleared_span() {
            self.ops.push(WindowOp::ClearSpan(span));
        }
        match event {
            TickEvent::Glyph(g) => self.ops.push(WindowOp::Glyph(g)),
            TickEvent::Scrolling { dy } => {
                for op in &mut self.ops {
                    op.shift_y(-dy);
                }
            }
            TickEvent::Finished => {
                if self.wait_for_button_press {
                    self.state = DialogState::AwaitingButtonPress;
                } else {
                    self.state = DialogState::Closed;
                }
            }
            TickEvent::Idle
            | TickEvent::Cleared
            | TickEvent::AwaitingScroll
            | TickEvent::ScrollStarted
            | TickEvent::ScrollFinished
            | TickEvent::AwaitingClear
            | TickEvent::AwaitingPress
            | TickEvent::PressAccepted
            | TickEvent::Paused
            | TickEvent::PauseFinished => {}
        }

        if self.state == DialogState::Closed {
            DialogOutcome::Closed
        } else {
            DialogOutcome::Continue
        }
    }

    /// Draws the message box and visible glyphs over `base`.
    #[must_use]
    pub(crate) fn compose_over(&self, mut base: Framebuffer) -> Framebuffer {
        let tiles = MessageBoxLayout::STANDARD.frame_tiles();
        textbox::blit_frame_tiles(&mut base, &tiles, self.frame.image(), &self.frame.palette);
        textbox::compose_window_ops(
            &mut base,
            &self.ops,
            textbox::STANDARD_BOX_SCREEN_ORIGIN,
            textbox::STANDARD_BOX_CONTENT_SIZE_PX,
            &self.frame.glyph_colors(),
        );
        base
    }
}

/// Creates an asset-independent open dialog for crate tests.
#[cfg(test)]
pub(crate) fn synthetic_dialog(tokens: Vec<Token>) -> NpcDialog {
    synthetic_dialog_at_speed(tokens, TextSpeed::Mid)
}

/// [`synthetic_dialog`], but at a caller-chosen [`TextSpeed`].
///
/// Only used within this module's own tests, which need [`TextSpeed::Instant`]
/// to observe same-tick behaviour (e.g. `FILL_WINDOW`'s `RENDER_REPEAT`).
#[cfg(test)]
fn synthetic_dialog_at_speed(tokens: Vec<Token>, speed: TextSpeed) -> NpcDialog {
    use assets::fonts::FontImageRef;
    use assets::pack::ImageRef;
    use rendering::Rgb888;

    const SHEET_WIDTH: u32 = 256;
    const SHEET_HEIGHT: u32 = 512;
    const SHEET_BIT_DEPTH: u8 = 2;
    const FRAME_WIDTH: u32 = 56;
    const FRAME_HEIGHT: u32 = 16;
    const FRAME_PALETTE_SIZE: usize = 16;

    let pixels = vec![0u8; (SHEET_WIDTH * SHEET_HEIGHT) as usize];
    let image = ImageRef {
        width: SHEET_WIDTH,
        height: SHEET_HEIGHT,
        bit_depth: SHEET_BIT_DEPTH,
        pixels: &pixels,
    };
    let sheet = OwnedFontGlyphSheet::new(FontImageRef::new_for_tests(FontId::Normal, image))
        .expect("this is the exact real glyph-sheet shape");
    let frame = FrameAssets {
        pixels: vec![0u8; (FRAME_WIDTH * FRAME_HEIGHT) as usize],
        width: FRAME_WIDTH,
        height: FRAME_HEIGHT,
        palette: vec![Rgb888::BLACK; FRAME_PALETTE_SIZE],
    };
    NpcDialog::new(sheet, frame, tokens, speed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rendering::Rgb888;

    const NO_INPUT: PrinterInput = PrinterInput {
        a_pressed: false,
        b_pressed: false,
        a_held: false,
        b_held: false,
    };

    const A_PRESS: PrinterInput = PrinterInput {
        a_pressed: true,
        b_pressed: false,
        a_held: true,
        b_held: false,
    };

    const A_HELD: PrinterInput = PrinterInput {
        a_pressed: false,
        b_pressed: false,
        a_held: true,
        b_held: false,
    };

    const B_PRESS: PrinterInput = PrinterInput {
        a_pressed: false,
        b_pressed: true,
        a_held: false,
        b_held: true,
    };

    const ONE_GLYPH_PRINT_FRAME_LIMIT: usize = 8;
    const TWO_GLYPH_PRINT_FRAME_LIMIT: usize = 16;
    const MID_SPEED_PROMPT_READY_FRAMES: usize = 8;
    const POST_CLEAR_CLOSE_FRAME_LIMIT: usize = 8;
    const WAIT_STATE_STABILITY_FRAMES: usize = 8;
    const ACCELERATION_TEST_FRAME_LIMIT: usize = 16;
    const OUTSIDE_DIALOG_PIXEL: (usize, usize) = (120, 0);

    fn advance_until_state(dialog: &mut NpcDialog, expected: DialogState, frame_limit: usize) {
        for _ in 0..frame_limit {
            if dialog.state == expected {
                return;
            }
            assert_eq!(dialog.tick(NO_INPUT), DialogOutcome::Continue);
        }
        assert_eq!(dialog.state, expected);
    }

    #[test]
    fn tick_reveals_glyphs_and_stays_open_while_printing() {
        let mut dialog = synthetic_dialog(vec![Token::Char('H'), Token::Char('i'), Token::End]);
        assert_eq!(dialog.tick(NO_INPUT), DialogOutcome::Continue);
        assert_eq!(dialog.ops.len(), 1);
    }

    #[test]
    fn fill_window_drops_stale_glyphs_but_keeps_the_glyph_printed_after_it() {
        // `0x0F` is `EXT_CTRL_CODE_FILL_WINDOW` (`pokeemerald/src/text.c`
        // `:1052-1056`), zero arguments per `charmap.txt:427`.
        let mut dialog = synthetic_dialog_at_speed(
            vec![
                Token::Char('A'),
                Token::Char('B'),
                Token::ExtCtrl {
                    sub: 0x0F,
                    args: vec![],
                },
                Token::Char('C'),
                Token::End,
            ],
            TextSpeed::Instant,
        );

        assert_eq!(dialog.tick(NO_INPUT), DialogOutcome::Continue);
        assert_eq!(dialog.tick(NO_INPUT), DialogOutcome::Continue);
        assert_eq!(
            dialog.revealed_glyph_count(),
            2,
            "both glyphs printed before FILL_WINDOW should be on screen"
        );

        assert_eq!(dialog.tick(NO_INPUT), DialogOutcome::Continue);
        assert_eq!(
            dialog.revealed_glyph_count(),
            1,
            "FILL_WINDOW must drop the stale glyphs, but not the one printed after it \
             in the same tick"
        );
        let WindowOp::Glyph(only_glyph) = &dialog.ops[0] else {
            panic!("expected the surviving op to be a glyph")
        };
        assert_eq!(
            (only_glyph.x, only_glyph.y),
            textbox::STANDARD_PRINTER_ORIGIN,
            "the surviving glyph must be placed at the reset cursor, not the stale one"
        );
    }

    #[test]
    fn a_message_without_a_trailing_prompt_closes_the_instant_printing_finishes() {
        let mut dialog = synthetic_dialog(vec![Token::Char('A'), Token::End]);
        for _ in 0..ONE_GLYPH_PRINT_FRAME_LIMIT {
            if dialog.tick(NO_INPUT) == DialogOutcome::Closed {
                return;
            }
        }
        panic!("a short, un-prompted message must close on its own");
    }

    #[test]
    fn a_trailing_prompt_clear_waits_for_confirm_then_closes_on_the_next_tick() {
        let mut dialog = synthetic_dialog(vec![Token::Char('A'), Token::PromptClear, Token::End]);
        for _ in 0..MID_SPEED_PROMPT_READY_FRAMES {
            assert!(
                dialog.tick(NO_INPUT) != DialogOutcome::Closed,
                "must not close before a confirm press reaches the trailing prompt"
            );
        }
        assert_eq!(
            dialog.tick(A_PRESS),
            DialogOutcome::Continue,
            "Cleared, not yet Closed"
        );
        for _ in 0..POST_CLEAR_CLOSE_FRAME_LIMIT {
            if dialog.tick(NO_INPUT) == DialogOutcome::Closed {
                return;
            }
        }
        panic!("must close once the post-clear reveal delay drains and Token::End is reached");
    }

    #[test]
    fn waitbuttonpress_holds_every_glyph_until_confirm_then_closes_on_that_same_tick() {
        let mut dialog = synthetic_dialog(vec![Token::Char('H'), Token::Char('i'), Token::End])
            .with_waitbuttonpress();

        advance_until_state(
            &mut dialog,
            DialogState::AwaitingButtonPress,
            TWO_GLYPH_PRINT_FRAME_LIMIT,
        );
        assert_eq!(dialog.revealed_glyph_count(), 2);

        for _ in 0..WAIT_STATE_STABILITY_FRAMES {
            assert_eq!(dialog.tick(NO_INPUT), DialogOutcome::Continue);
            assert_eq!(
                dialog.revealed_glyph_count(),
                2,
                "text must stay on screen while awaiting the confirm press -- \
                 no Cleared, ever, on this path"
            );
        }

        assert_eq!(
            dialog.tick(A_PRESS),
            DialogOutcome::Closed,
            "a confirm edge must close the dialog on this exact tick"
        );
        assert_eq!(
            dialog.revealed_glyph_count(),
            2,
            "the box must close with its text intact, never blanked first"
        );
    }

    #[test]
    fn waitbuttonpress_accepts_a_fresh_b_press_too() {
        let mut dialog =
            synthetic_dialog(vec![Token::Char('A'), Token::End]).with_waitbuttonpress();
        advance_until_state(
            &mut dialog,
            DialogState::AwaitingButtonPress,
            ONE_GLYPH_PRINT_FRAME_LIMIT,
        );
        assert_eq!(dialog.revealed_glyph_count(), 1);
        assert_eq!(
            dialog.tick(B_PRESS),
            DialogOutcome::Closed,
            "a fresh B edge must close a waitbuttonpress dialog just as A does"
        );
    }

    #[test]
    fn waitbuttonpress_ignores_a_held_button_with_no_fresh_edge() {
        let mut dialog =
            synthetic_dialog(vec![Token::Char('A'), Token::End]).with_waitbuttonpress();
        advance_until_state(
            &mut dialog,
            DialogState::AwaitingButtonPress,
            ONE_GLYPH_PRINT_FRAME_LIMIT,
        );
        assert_eq!(dialog.revealed_glyph_count(), 1);
        assert_eq!(
            dialog.tick(A_HELD),
            DialogOutcome::Continue,
            "a held-but-not-freshly-pressed button must not close the dialog"
        );
        assert_eq!(
            dialog.tick(A_PRESS),
            DialogOutcome::Closed,
            "a real fresh edge afterward must still close it"
        );
    }

    #[test]
    fn tick_is_idempotent_once_closed() {
        let mut dialog = synthetic_dialog(vec![Token::End]);
        assert_eq!(dialog.tick(NO_INPUT), DialogOutcome::Closed);
        assert_eq!(dialog.tick(NO_INPUT), DialogOutcome::Closed);
        assert_eq!(dialog.tick(A_PRESS), DialogOutcome::Closed);
    }

    #[test]
    fn held_confirm_reaches_the_next_glyph_in_fewer_ticks_than_unheld() {
        let tokens = || {
            vec![
                Token::Char('A'),
                Token::Char('B'),
                Token::Char('C'),
                Token::End,
            ]
        };

        let ticks_to_second_glyph = |inputs: &[PrinterInput]| {
            let mut dialog = synthetic_dialog(tokens());
            for tick in 0..ACCELERATION_TEST_FRAME_LIMIT {
                let input = inputs[tick % inputs.len()];
                dialog.tick(input);
                if dialog.revealed_glyph_count() >= 2 {
                    return tick;
                }
            }
            panic!("the second glyph must print within the frame limit");
        };

        let unheld_ticks = ticks_to_second_glyph(&[NO_INPUT]);
        let held_ticks = ticks_to_second_glyph(&[NO_INPUT, A_PRESS, A_HELD]);

        assert!(
            held_ticks < unheld_ticks,
            "held confirm must reach the second glyph faster ({held_ticks} ticks) than \
             unheld ({unheld_ticks} ticks)"
        );
    }

    /// A dialog whose message-box palette and font sheet are fully controlled:
    /// every glyph pixel is `glyph_index`, and palette slot 2/3 are distinct.
    fn dialog_with_palette(glyph_index: u8, palette: Vec<Rgb888>) -> NpcDialog {
        use assets::fonts::FontImageRef;
        use assets::pack::ImageRef;

        const SHEET_WIDTH: u32 = 256;
        const SHEET_HEIGHT: u32 = 512;
        const FONT_SHEET_BIT_DEPTH: u8 = 2;
        const FRAME_WIDTH: u32 = 56;
        const FRAME_HEIGHT: u32 = 16;

        let pixels = vec![glyph_index; (SHEET_WIDTH * SHEET_HEIGHT) as usize];
        let image = ImageRef {
            width: SHEET_WIDTH,
            height: SHEET_HEIGHT,
            bit_depth: FONT_SHEET_BIT_DEPTH,
            pixels: &pixels,
        };
        let sheet = OwnedFontGlyphSheet::new(FontImageRef::new_for_tests(FontId::Normal, image))
            .expect("this is the exact real glyph-sheet shape");
        let frame = FrameAssets {
            // Every frame pixel is palette index 0, i.e. transparent, so only
            // glyph pixels reach the framebuffer.
            pixels: vec![0u8; (FRAME_WIDTH * FRAME_HEIGHT) as usize],
            width: FRAME_WIDTH,
            height: FRAME_HEIGHT,
            palette,
        };
        NpcDialog::new(
            sheet,
            frame,
            vec![Token::Char('A'), Token::End],
            TextSpeed::Mid,
        )
    }

    /// Upstream's palette-index choice (`pokeemerald/src/menu.c:191-195`);
    /// see [`FrameAssets::glyph_colors`] for the full contract this asserts.
    #[test]
    fn dialog_glyphs_take_their_colors_from_the_loaded_message_box_palette() {
        const FOREGROUND_FONT_INDEX: u8 = 1;
        const SHADOW_FONT_INDEX: u8 = 2;
        const MESSAGE_BOX_FOREGROUND_SLOT: usize = 2;
        const MESSAGE_BOX_SHADOW_SLOT: usize = 3;
        const PALETTE_SIZE: usize = 16;

        let foreground = Rgb888 {
            r: 96,
            g: 96,
            b: 96,
        };
        let shadow = Rgb888 {
            r: 208,
            g: 208,
            b: 200,
        };
        let mut palette = vec![Rgb888::BLACK; PALETTE_SIZE];
        palette[MESSAGE_BOX_FOREGROUND_SLOT] = foreground;
        palette[MESSAGE_BOX_SHADOW_SLOT] = shadow;

        let first_glyph_pixel = (
            usize::try_from(
                textbox::STANDARD_BOX_SCREEN_ORIGIN.0 + textbox::STANDARD_PRINTER_ORIGIN.0,
            )
            .unwrap(),
            usize::try_from(
                textbox::STANDARD_BOX_SCREEN_ORIGIN.1 + textbox::STANDARD_PRINTER_ORIGIN.1,
            )
            .unwrap(),
        );

        for (font_index, expected) in [
            (FOREGROUND_FONT_INDEX, foreground),
            (SHADOW_FONT_INDEX, shadow),
        ] {
            let mut dialog = dialog_with_palette(font_index, palette.clone());
            assert_eq!(dialog.tick(NO_INPUT), DialogOutcome::Continue);
            assert_eq!(dialog.revealed_glyph_count(), 1);

            let composed = dialog.compose_over(Framebuffer::new());

            assert_eq!(
                composed.pixel(first_glyph_pixel.0, first_glyph_pixel.1),
                Some(expected),
                "font index {font_index} must paint the loaded palette's slot color"
            );
        }
    }

    #[test]
    fn compose_over_leaves_the_base_frame_visible_outside_the_box() {
        let dialog = synthetic_dialog(vec![Token::End]);
        let mut base = Framebuffer::new();
        let marker = Rgb888 {
            r: 10,
            g: 20,
            b: 30,
        };
        base.fill(marker);

        let composed = dialog.compose_over(base);

        assert_eq!(
            composed.pixel(OUTSIDE_DIALOG_PIXEL.0, OUTSIDE_DIALOG_PIXEL.1),
            Some(marker)
        );
    }

    const OPAQUE_GLYPH_PALETTE_INDEX: u8 = 1;
    const FRAME_INTERIOR_PALETTE_INDEX: u8 = 1;
    // The palette index upstream selects as the message-box glyph
    // foreground (`TEXT_COLOR_DARK_GRAY`, `pokeemerald/src/menu.c:191-201`);
    // mirrors `textbox::MESSAGE_BOX_FOREGROUND_PALETTE_INDEX`.
    const GLYPH_FOREGROUND_PALETTE_INDEX: u8 = 2;
    const NORMAL_A_ADVANCE_WIDTH: i32 = 6;

    const OPAQUE_GLYPH_COLOR: Rgb888 = Rgb888 {
        r: 24,
        g: 24,
        b: 24,
    };

    const FRAME_INTERIOR_COLOR: Rgb888 = Rgb888 {
        r: 200,
        g: 40,
        b: 40,
    };

    /// A dialog whose glyph sheet and message box are solid, distinguishable
    /// colors, so a cleared span's pixels can be told apart from an unerased
    /// glyph's -- `synthetic_dialog`'s all-transparent fixture cannot.
    fn opaque_synthetic_dialog(tokens: Vec<Token>) -> NpcDialog {
        use assets::fonts::FontImageRef;
        use assets::pack::ImageRef;

        const SHEET_WIDTH: u32 = 256;
        const SHEET_HEIGHT: u32 = 512;
        const SHEET_BIT_DEPTH: u8 = 2;
        const FRAME_WIDTH: u32 = 56;
        const FRAME_HEIGHT: u32 = 16;
        const FRAME_PALETTE_SIZE: usize = 16;

        let pixels = vec![OPAQUE_GLYPH_PALETTE_INDEX; (SHEET_WIDTH * SHEET_HEIGHT) as usize];
        let image = ImageRef {
            width: SHEET_WIDTH,
            height: SHEET_HEIGHT,
            bit_depth: SHEET_BIT_DEPTH,
            pixels: &pixels,
        };
        let sheet = OwnedFontGlyphSheet::new(FontImageRef::new_for_tests(FontId::Normal, image))
            .expect("this is the exact real glyph-sheet shape");

        let mut palette = vec![Rgb888::BLACK; FRAME_PALETTE_SIZE];
        palette[usize::from(FRAME_INTERIOR_PALETTE_INDEX)] = FRAME_INTERIOR_COLOR;
        palette[usize::from(GLYPH_FOREGROUND_PALETTE_INDEX)] = OPAQUE_GLYPH_COLOR;
        let frame = FrameAssets {
            pixels: vec![FRAME_INTERIOR_PALETTE_INDEX; (FRAME_WIDTH * FRAME_HEIGHT) as usize],
            width: FRAME_WIDTH,
            height: FRAME_HEIGHT,
            palette,
        };
        NpcDialog::new(sheet, frame, tokens, TextSpeed::Instant)
    }

    /// Screen pixel for a window-local content coordinate in the standard box.
    fn box_pixel(content: (i32, i32)) -> (usize, usize) {
        let x = textbox::STANDARD_BOX_SCREEN_ORIGIN.0 + content.0;
        let y = textbox::STANDARD_BOX_SCREEN_ORIGIN.1 + content.1;
        (usize::try_from(x).unwrap(), usize::try_from(y).unwrap())
    }

    #[test]
    fn clear_paints_the_erased_span_with_the_windows_background() {
        // `0x0D`/`0x11` are `EXT_CTRL_CODE_SHIFT_RIGHT`/`EXT_CTRL_CODE_CLEAR`
        // (`pokeemerald/src/text.c:1063-1072`), one argument byte each per
        // `charmap.txt`. `SHIFT_RIGHT 0` moves the cursor back over the `A`
        // just printed, so `CLEAR` erases exactly that glyph's width.
        let mut dialog = opaque_synthetic_dialog(vec![
            Token::Char('A'),
            Token::ExtCtrl {
                sub: 0x0D,
                args: vec![0],
            },
            Token::ExtCtrl {
                sub: 0x11,
                args: vec![u8::try_from(NORMAL_A_ADVANCE_WIDTH).unwrap()],
            },
            Token::End,
        ]);

        assert_eq!(
            dialog.tick(NO_INPUT),
            DialogOutcome::Continue,
            "reveals 'A'"
        );
        assert_eq!(
            dialog.tick(NO_INPUT),
            DialogOutcome::Continue,
            "SHIFT_RIGHT is free; CLEAR ends the frame"
        );

        let fb = dialog.compose_over(Framebuffer::new());

        let (cleared_x, cleared_y) = box_pixel((
            textbox::STANDARD_PRINTER_ORIGIN.0 + 2,
            textbox::STANDARD_PRINTER_ORIGIN.1 + 8,
        ));
        assert_eq!(
            fb.pixel(cleared_x, cleared_y),
            Some(FRAME_INTERIOR_COLOR),
            "the erased span must show the window's background, not the glyph"
        );

        let (still_glyph_x, still_glyph_y) = box_pixel((
            textbox::STANDARD_PRINTER_ORIGIN.0 + NORMAL_A_ADVANCE_WIDTH + 4,
            textbox::STANDARD_PRINTER_ORIGIN.1 + 8,
        ));
        assert_eq!(
            fb.pixel(still_glyph_x, still_glyph_y),
            Some(OPAQUE_GLYPH_COLOR),
            "pixels past the cleared width belong to the same glyph and must stay visible"
        );
    }

    #[test]
    fn clear_to_paints_only_up_to_its_target_column() {
        // `0x13` is `EXT_CTRL_CODE_CLEAR_TO` (`pokeemerald/src/text.c`
        // `:1077-1090`); the argument is an origin-relative column, so this
        // erases fewer pixels than the glyph's full advance width.
        const CLEAR_TO_COLUMN: u8 = 4;
        let cleared_width = i32::from(CLEAR_TO_COLUMN);

        let mut dialog = opaque_synthetic_dialog(vec![
            Token::Char('A'),
            Token::ExtCtrl {
                sub: 0x0D,
                args: vec![0],
            },
            Token::ExtCtrl {
                sub: 0x13,
                args: vec![CLEAR_TO_COLUMN],
            },
            Token::End,
        ]);

        assert_eq!(
            dialog.tick(NO_INPUT),
            DialogOutcome::Continue,
            "reveals 'A'"
        );
        assert_eq!(
            dialog.tick(NO_INPUT),
            DialogOutcome::Continue,
            "SHIFT_RIGHT is free; CLEAR_TO ends the frame"
        );

        let fb = dialog.compose_over(Framebuffer::new());

        let (cleared_x, cleared_y) = box_pixel((
            textbox::STANDARD_PRINTER_ORIGIN.0 + 1,
            textbox::STANDARD_PRINTER_ORIGIN.1 + 8,
        ));
        assert_eq!(
            fb.pixel(cleared_x, cleared_y),
            Some(FRAME_INTERIOR_COLOR),
            "the span up to the target column must show the window's background"
        );

        let (still_glyph_x, still_glyph_y) = box_pixel((
            textbox::STANDARD_PRINTER_ORIGIN.0 + cleared_width + 4,
            textbox::STANDARD_PRINTER_ORIGIN.1 + 8,
        ));
        assert_eq!(
            fb.pixel(still_glyph_x, still_glyph_y),
            Some(OPAQUE_GLYPH_COLOR),
            "pixels past the target column belong to the same glyph and must stay visible"
        );
    }

    /// The smallest pack [`NpcDialog::from_pack_at_speed`] accepts: the
    /// standard message box frame and the normal font sheet. Mirrors
    /// `crate::start_menu::pack_fixture::synthetic_start_menu_pack_bytes`'s own
    /// fixture.
    fn synthetic_field_dialog_pack_bytes() -> Vec<u8> {
        use crate::pack_test_support::{image_entry, pack_bytes, palette_entry};

        const MESSAGE_BOX_WIDTH: u32 = 56;
        const MESSAGE_BOX_HEIGHT: u32 = 16;
        const FRAME_BIT_DEPTH: u8 = 4;
        const FONT_BIT_DEPTH: u8 = 2;
        const PALETTE_COLOUR_COUNT: u16 = 16;

        pack_bytes(vec![
            image_entry(
                "text-window/image/message_box",
                MESSAGE_BOX_WIDTH,
                MESSAGE_BOX_HEIGHT,
                FRAME_BIT_DEPTH,
                0,
            ),
            palette_entry("text-window/palette/message_box", PALETTE_COLOUR_COUNT),
            image_entry(
                "font/normal/glyphs",
                assets::fonts::SHEET_WIDTH,
                assets::fonts::SHEET_HEIGHT,
                FONT_BIT_DEPTH,
                0,
            ),
        ])
    }

    /// Ticks `dialog` until `glyph_count` glyphs are revealed, or panics past
    /// `PRINT_FRAME_BUDGET` frames.
    fn frames_to_reveal(dialog: &mut NpcDialog, glyph_count: usize) -> usize {
        const PRINT_FRAME_BUDGET: usize = 64;
        for frame in 1..=PRINT_FRAME_BUDGET {
            dialog.tick(NO_INPUT);
            if dialog.revealed_glyph_count() == glyph_count {
                return frame;
            }
        }
        panic!("a message of {glyph_count} glyphs must print within {PRINT_FRAME_BUDGET} frames");
    }

    fn field_dialog_scratch_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "pokeemerald-rs-field-dialog-text-speed-{}-{:?}.pack",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    /// Writes the synthetic field-dialog pack to its scratch path and
    /// returns that path; the pacing test and its planted-symlink regression
    /// share this one writer.
    fn write_field_dialog_scratch_pack() -> std::path::PathBuf {
        let path = field_dialog_scratch_path();
        crate::overworld::test_support::write_scratch_exclusive(
            &path,
            synthetic_field_dialog_pack_bytes(),
        );
        path
    }

    #[cfg(unix)]
    #[test]
    fn field_dialog_scratch_pack_refuses_a_planted_symlink() {
        crate::overworld::test_support::assert_planted_symlink_untouched(
            &field_dialog_scratch_path(),
            || {
                write_field_dialog_scratch_pack();
            },
        );
    }

    /// A dialog opened at a given speed paces at it: FAST prints the same
    /// message in fewer frames than SLOW (`sTextSpeedFrameDelays`,
    /// `pokeemerald/src/menu.c:77-82`, `:191-196`).
    #[test]
    fn field_dialogs_pace_at_the_speed_they_are_given() {
        const MESSAGE_GLYPHS: usize = 3;
        let tokens = || {
            vec![
                Token::Char('H'),
                Token::Char('i'),
                Token::Char('!'),
                Token::End,
            ]
        };

        let path = write_field_dialog_scratch_pack();
        let pack = AssetPack::load(&path).expect("the synthetic field-dialog pack must load");
        let _ = std::fs::remove_file(&path);

        let mut fast_dialog =
            NpcDialog::from_pack_at_speed(&pack, tokens(), TextSpeed::from_raw_option(2))
                .expect("the synthetic pack carries the dialog's font and message box");
        let mut slow_dialog =
            NpcDialog::from_pack_at_speed(&pack, tokens(), TextSpeed::from_raw_option(0))
                .expect("the synthetic pack carries the dialog's font and message box");

        let fast = frames_to_reveal(&mut fast_dialog, MESSAGE_GLYPHS);
        let slow = frames_to_reveal(&mut slow_dialog, MESSAGE_GLYPHS);

        assert!(
            fast < slow,
            "a field dialog given the FAST saved option must print in fewer frames than one \
             given SLOW, but they took {fast} and {slow} frames"
        );
    }
}
