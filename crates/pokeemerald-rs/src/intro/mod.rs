//! Birch's new-game introduction.
//!
//! # Reduced cinematic
//!
//! [`IntroScene`] renders the dialogue and standard message box. The scene
//! omits the character animation, gender and naming screens, confirmation
//! menu, and audio. Questions held on screen by those omitted transitions
//! wait for A or B before the scene continues.
//!
//! # Advance
//!
//! A and B advance printer waits, and holding either button accelerates text.
//!
//! # Traversal pacing
//!
//! [`TRAVERSAL_RUNS`] records the printer-derived timing used by scripted runs.

mod speech;

use assets::fonts::{FontId, OwnedFontGlyphSheet};
use assets::pack::{AssetPack, PackError};
use engine::text::render::{Printer, PrinterInput, TextSpeed, TickEvent};
use engine::text::window::MessageBoxLayout;
use engine::text::Token;
use rendering::{Framebuffer, Rgb888};

use crate::new_game::{NewGameIdentity, NewGameOptions};
use crate::textbox::{self, FrameAssets, WindowOp};

pub use speech::NUM_PAGES;

/// A consecutive span of frames without input while reading Birch's speech.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraversalRun {
    /// Frames elapsed before the printer waits or continues automatically.
    pub frames: u32,
    /// Whether the next frame must contain an A or B press.
    pub confirm_after: bool,
}

/// Printer-derived timing for a full read at [`TextSpeed::Mid`] without held input.
pub const TRAVERSAL_RUNS: &[TraversalRun] = &[
    waits_for_confirmation(121),
    waits_for_confirmation(132),
    waits_for_confirmation(72),
    waits_for_confirmation(179),
    advances_automatically(4),
    waits_for_confirmation(230),
    advances_automatically(4),
    waits_for_confirmation(244),
    waits_for_confirmation(279),
    advances_automatically(9),
    waits_for_confirmation(140),
    waits_for_confirmation(235),
    waits_for_confirmation(267),
    waits_for_confirmation(235),
    waits_for_confirmation(247),
    advances_automatically(9),
    waits_for_confirmation(72),
    advances_automatically(4),
    waits_for_confirmation(49),
    advances_automatically(4),
    waits_for_confirmation(112),
    advances_automatically(4),
    waits_for_confirmation(49),
    advances_automatically(4),
    waits_for_confirmation(37),
    waits_for_confirmation(215),
    advances_automatically(9),
    waits_for_confirmation(56),
    advances_automatically(4),
    waits_for_confirmation(101),
    waits_for_confirmation(175),
    waits_for_confirmation(251),
    advances_automatically(9),
    waits_for_confirmation(136),
    waits_for_confirmation(263),
    advances_automatically(4),
];

const fn waits_for_confirmation(frames: u32) -> TraversalRun {
    TraversalRun {
        frames,
        confirm_after: true,
    }
}

const fn advances_automatically(frames: u32) -> TraversalRun {
    TraversalRun {
        frames,
        confirm_after: false,
    }
}

/// Total frames for [`TRAVERSAL_RUNS`], including required confirmation frames.
pub const TRAVERSAL_FRAMES: usize = traversal_frames(TRAVERSAL_RUNS);

const fn traversal_frames(runs: &[TraversalRun]) -> usize {
    let mut total = 0;
    let mut i = 0;
    while i < runs.len() {
        total += runs[i].frames as usize;
        if runs[i].confirm_after {
            total += 1;
        }
        i += 1;
    }
    total
}

/// An error while building an [`IntroScene`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntroSceneError {
    /// The asset pack could not be loaded or read.
    Pack(PackError),
    /// The font glyph sheet could not be decoded.
    Font(assets::AssetError),
}

impl std::fmt::Display for IntroSceneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pack(err) => write!(f, "intro: {err}"),
            Self::Font(err) => write!(f, "intro: {err}"),
        }
    }
}

impl std::error::Error for IntroSceneError {}

impl From<PackError> for IntroSceneError {
    fn from(err: PackError) -> Self {
        Self::Pack(err)
    }
}

impl From<assets::AssetError> for IntroSceneError {
    fn from(err: assets::AssetError) -> Self {
        Self::Font(err)
    }
}

impl IntroSceneError {
    /// Returns whether the asset pack was not found.
    #[must_use]
    pub const fn is_pack_missing(&self) -> bool {
        matches!(self, Self::Pack(PackError::NotFound(_)))
    }
}

/// The introduction's progress towards the overworld handoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntroStatus {
    /// The current page or a later page still needs processing.
    Continue,
    /// Every page has finished.
    Finished,
}

/// `gSaveBlock2Ptr->optionsTextSpeed` values above this are invalid;
/// upstream's `GetPlayerTextSpeedDelay` treats them exactly like
/// [`NORMALIZED_TEXT_SPEED`] (`pokeemerald/include/constants/global.h:127-129`).
/// Mirrors `start_menu.rs`'s own identically-valued, separately-scoped
/// constant of the same name (issue #927's live-block repair) -- this one
/// repairs the byte a NEW GAME carries forward instead.
const MAX_VALID_TEXT_SPEED: u8 = 2;

/// The value `GetPlayerTextSpeedDelay` repairs an out-of-range
/// `optionsTextSpeed` to (`pokeemerald/src/menu.c:483-484`).
const NORMALIZED_TEXT_SPEED: u8 = 1;

/// [`Self::from_pack_with_options`]'s own repair, split out so it is
/// unit-testable without a pack: `raw` unchanged when valid, otherwise
/// [`NORMALIZED_TEXT_SPEED`].
const fn normalized_text_speed(raw: u8) -> u8 {
    if raw > MAX_VALID_TEXT_SPEED {
        NORMALIZED_TEXT_SPEED
    } else {
        raw
    }
}

/// Birch's introduction speech, rendered one page at a time.
#[derive(Debug)]
pub struct IntroScene {
    frame: FrameAssets,
    pages: [Vec<Token>; NUM_PAGES],
    page_index: usize,
    printer: Printer<OwnedFontGlyphSheet>,
    ops: Vec<WindowOp>,
    finished: bool,
    /// The boot-recovered options (issue #1125) this NEW GAME will hand to
    /// [`crate::new_game::init_save_blocks_with_options`] once the intro
    /// finishes -- see [`NewGameOptions`]'s own doc comment for which boot
    /// verdicts carry their recovered `SaveBlock2` bytes here instead of
    /// [`NewGameOptions::DEFAULT`].
    new_game_options: NewGameOptions,
    /// The gender and name the new game commits at the handoff; the
    /// unselected default until the chooser and naming screens supply one
    /// ([`Self::with_identity`]).
    new_game_identity: NewGameIdentity,
}

impl IntroScene {
    /// Creates an introduction from decoded rendering assets at `speed`,
    /// carrying `new_game_options` onward to the eventual new-game handoff
    /// (issue #1125) -- see [`NewGameOptions`]'s own doc comment. This
    /// module's own tests, which have no boot-recovered save to carry
    /// options from, pass [`NewGameOptions::DEFAULT`].
    #[must_use]
    pub(crate) fn new(
        sheet: OwnedFontGlyphSheet,
        frame: FrameAssets,
        speed: TextSpeed,
        new_game_options: NewGameOptions,
    ) -> Self {
        let pages = speech::pages();
        // The upstream speech printer enables held-A/B acceleration (src/main_menu.c:1339).
        let printer = Printer::new(
            pages[0].clone(),
            sheet,
            speed,
            textbox::STANDARD_PRINTER_ORIGIN,
        )
        .with_ab_speed_up_print();
        Self {
            frame,
            pages,
            page_index: 0,
            printer,
            ops: Vec::new(),
            finished: false,
            new_game_options,
            new_game_identity: NewGameIdentity::DEFAULT,
        }
    }

    /// Replaces the identity the handoff will commit with a selected one.
    #[must_use]
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the chooser UI (#1649) is the production caller")
    )]
    pub(crate) const fn with_identity(mut self, identity: NewGameIdentity) -> Self {
        self.new_game_identity = identity;
        self
    }

    /// The identity [`crate::new_game::init_save_blocks_with_identity`]
    /// commits once the intro finishes.
    #[must_use]
    pub(crate) const fn new_game_identity(&self) -> NewGameIdentity {
        self.new_game_identity
    }

    /// Loads owned rendering assets from `pack` at the default text speed.
    ///
    /// # Errors
    ///
    /// Returns [`IntroSceneError::Pack`] if an asset is missing or malformed,
    /// or [`IntroSceneError::Font`] if the font sheet cannot be decoded.
    pub fn from_pack(pack: &AssetPack) -> Result<Self, IntroSceneError> {
        Self::from_pack_with_options(pack, NewGameOptions::DEFAULT)
    }

    /// [`Self::from_pack`], reading the printer's text speed from `options`
    /// (`TextSpeed::from_raw_option`) and retaining `options`' two raw bytes
    /// for the eventual [`crate::new_game::init_save_blocks_with_options`]
    /// handoff (issue #1125) -- the pack's fixed message-box asset is
    /// unaffected by `options.window_frame_type`, which upstream's own
    /// `optionsWindowFrameType` never applies to Birch's speech box either.
    ///
    /// `options.text_speed` is repaired to `OPTIONS_TEXT_SPEED_MID` first if
    /// out of range, exactly like the retained byte
    /// [`NewGameOptions::text_speed`]'s own doc comment describes: Birch's
    /// speech is the very first message a NEW GAME ever prints, so this is
    /// the earliest point `GetPlayerTextSpeedDelay`'s own write-back
    /// (`pokeemerald/src/menu.c:481-487`) could run upstream, and an
    /// unrepaired out-of-range byte must not survive past it into the fresh
    /// save `NewGameInitData` never re-validates afterward.
    /// `options.window_frame_type` is not repaired here: no upstream call
    /// site write-back-repairs it the way `optionsTextSpeed` is (see that
    /// field's own doc comment).
    ///
    /// # Errors
    ///
    /// See [`Self::from_pack`].
    pub(crate) fn from_pack_with_options(
        pack: &AssetPack,
        options: NewGameOptions,
    ) -> Result<Self, IntroSceneError> {
        let sheet = OwnedFontGlyphSheet::new(pack.font(FontId::Normal)?)?;
        let frame = FrameAssets::from_handle(pack.message_box()?);
        let speed = TextSpeed::from_raw_option(options.text_speed);
        let options = NewGameOptions {
            text_speed: normalized_text_speed(options.text_speed),
            ..options
        };
        Ok(Self::new(sheet, frame, speed, options))
    }

    /// The boot-recovered options this intro will hand to
    /// [`crate::new_game::init_save_blocks_with_options`] once it finishes
    /// (issue #1125).
    #[must_use]
    pub(crate) const fn new_game_options(&self) -> NewGameOptions {
        self.new_game_options
    }

    /// Returns the current page index in `0..NUM_PAGES`.
    #[must_use]
    pub const fn page_index(&self) -> usize {
        self.page_index
    }

    /// Returns whether every page has finished.
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        self.finished
    }

    /// Returns the number of glyphs currently retained (revealed, and not
    /// since dropped by a page or window clear).
    #[must_use]
    pub fn revealed_glyph_count(&self) -> usize {
        self.ops
            .iter()
            .filter(|op| matches!(op, WindowOp::Glyph(_)))
            .count()
    }

    /// Advances the introduction by one frame.
    ///
    /// Once this returns [`IntroStatus::Finished`], later calls also return it.
    pub fn tick(&mut self, input: PrinterInput) -> IntroStatus {
        if self.finished {
            return IntroStatus::Finished;
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
            TickEvent::Finished => self.advance_page(),
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

        if self.finished {
            IntroStatus::Finished
        } else {
            IntroStatus::Continue
        }
    }

    fn advance_page(&mut self) {
        if self.page_index + 1 < NUM_PAGES {
            self.page_index += 1;
            self.ops.clear();
            self.printer.restart(self.pages[self.page_index].clone());
        } else {
            self.finished = true;
        }
    }

    /// Renders the dialogue box and visible glyphs into a new framebuffer.
    #[must_use]
    pub fn compose(&self) -> Framebuffer {
        let mut fb = Framebuffer::new();
        fb.fill(Rgb888::BLACK);

        let tiles = MessageBoxLayout::STANDARD.frame_tiles();
        textbox::blit_frame_tiles(&mut fb, &tiles, self.frame.image(), &self.frame.palette);
        textbox::compose_window_ops(
            &mut fb,
            &self.ops,
            textbox::STANDARD_BOX_SCREEN_ORIGIN,
            textbox::STANDARD_BOX_CONTENT_SIZE_PX,
            &self.frame.glyph_colors(),
        );

        fb
    }

    /// Renders a presentation-ready frame.
    #[must_use]
    pub fn compose_frame(&self) -> Box<platform::Frame> {
        crate::frame::to_platform_frame(&self.compose())
    }
}

/// Loads an introduction from the default asset pack, at
/// [`NewGameOptions::DEFAULT`].
///
/// # Errors
///
/// Returns an error if the pack cannot be loaded or its rendering assets
/// cannot be decoded.
pub fn load_default() -> Result<IntroScene, IntroSceneError> {
    load(
        &crate::pack_source::PackSource::Runtime,
        NewGameOptions::DEFAULT,
    )
}

/// Loads an introduction from `source`, carrying `options` onward to the
/// eventual new-game handoff (issue #1125) -- [`crate::flow::advance_scene`]'s
/// `MainMenu` -> `Intro` arm derives `options` from the boot-recovered save
/// before this call.
///
/// # Errors
///
/// Returns an error if the pack cannot be loaded or its rendering assets
/// cannot be decoded.
pub(crate) fn load(
    source: &crate::pack_source::PackSource,
    options: NewGameOptions,
) -> Result<IntroScene, IntroSceneError> {
    let pack = source.load()?;
    IntroScene::from_pack_with_options(&pack, options)
}

/// Returns a synthetic, finished introduction for flow tests.
#[cfg(test)]
pub(crate) fn synthetic_finished_scene() -> IntroScene {
    use assets::fonts::FontImageRef;
    use assets::pack::ImageRef;

    const SHEET_WIDTH: u32 = 256;
    const SHEET_HEIGHT: u32 = 512;
    const FRAME_WIDTH: u32 = 56;
    const FRAME_HEIGHT: u32 = 16;
    const MAX_TICKS_TO_FINISH: usize = 5_000;
    let pixels = vec![0u8; (SHEET_WIDTH * SHEET_HEIGHT) as usize];
    let image = ImageRef {
        width: SHEET_WIDTH,
        height: SHEET_HEIGHT,
        bit_depth: 2,
        pixels: &pixels,
    };
    let sheet = OwnedFontGlyphSheet::new(FontImageRef::new_for_tests(FontId::Normal, image))
        .expect("this is the exact real glyph-sheet shape");
    let frame = FrameAssets {
        pixels: vec![0u8; (FRAME_WIDTH * FRAME_HEIGHT) as usize],
        width: FRAME_WIDTH,
        height: FRAME_HEIGHT,
        palette: vec![Rgb888::BLACK; 16],
    };
    let mut scene = IntroScene::new(sheet, frame, TextSpeed::Instant, NewGameOptions::DEFAULT);
    let confirm_a = PrinterInput {
        a_pressed: true,
        b_pressed: false,
        a_held: true,
        b_held: false,
    };
    let mut status = IntroStatus::Continue;
    for _ in 0..MAX_TICKS_TO_FINISH {
        status = scene.tick(confirm_a);
        if status == IntroStatus::Finished {
            break;
        }
    }
    debug_assert_eq!(status, IntroStatus::Finished);
    scene
}

#[cfg(test)]
mod tests;
