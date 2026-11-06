//! Converts decoded text tokens into frame-driven glyph and control events.
//!
//! [`Printer`] owns its glyph source and render state. The caller supplies
//! [`PrinterInput`] and calls [`Printer::tick`] once per frame.
//!
//! A template line spacing is set with [`Printer::with_line_spacing`] and is
//! zero by default.
//!
//! # Deliberately out of scope
//!
//! Formatting, audio, and window-buffer controls are consumed without applying
//! effects, except the ones named here. `CLEAR`, `CLEAR_TO`, and `FILL_WINDOW`
//! move the cursor: `CLEAR`/`CLEAR_TO`'s geometry is reported as a
//! [`ClearedSpan`] and `FILL_WINDOW`'s whole-window equivalent through
//! [`Printer::cleared_window`], both for the caller to erase. Which colour
//! that erase uses stays out of scope with the rest of formatting.
//! `MIN_LETTER_SPACING` widens a following glyph's advance and, when the
//! glyph is narrower than that minimum, reports the leftover gap as a
//! [`ClearedSpan`] on the same tick as the glyph. `FONT` selects another
//! supplied glyph sheet ([`Printer::with_font_sheet`]) and refreshes the
//! newline and clear-span metrics from that font. Placeholders, dynamic
//! tokens, and keypad icons are skipped because this renderer has no
//! expansion context or corresponding glyphs.

use assets::fonts::{FontId, Glyph, GlyphSource};

use super::Token;

mod ext_ctrl {
    pub const ESCAPE: u8 = super::super::EXT_CTRL_CODE_ESCAPE;
    pub const SHIFT_RIGHT: u8 = super::super::EXT_CTRL_CODE_SHIFT_RIGHT;
    pub const SHIFT_DOWN: u8 = super::super::EXT_CTRL_CODE_SHIFT_DOWN;
    pub const SKIP: u8 = super::super::EXT_CTRL_CODE_SKIP;
    pub const CLEAR: u8 = super::super::EXT_CTRL_CODE_CLEAR;
    pub const CLEAR_TO: u8 = super::super::EXT_CTRL_CODE_CLEAR_TO;
    pub const FILL_WINDOW: u8 = super::super::EXT_CTRL_CODE_FILL_WINDOW;
    pub const FONT: u8 = super::super::EXT_CTRL_CODE_FONT;
    pub const MIN_LETTER_SPACING: u8 = super::super::EXT_CTRL_CODE_MIN_LETTER_SPACING;
    pub const PAUSE: u8 = super::super::EXT_CTRL_CODE_PAUSE;
    pub const PAUSE_UNTIL_PRESS: u8 = super::super::EXT_CTRL_CODE_PAUSE_UNTIL_PRESS;
}

const EXTRA_SYMBOL_PAGE_START: u16 = 0x100;

/// Upstream `FONT_*` ids this renderer can select (`include/text.h`).
///
/// `FONT_SHORT_COPY_1` through `_3` share [`FontId::Short`]'s sheet, matching
/// upstream's width-table aliases. `FONT_BRAILLE` and `FONT_BOLD` have no
/// sheet here, so they are not selectable.
fn modelled_font(upstream_id: u8) -> Option<FontId> {
    match upstream_id {
        0 => Some(FontId::Small),
        1 => Some(FontId::Normal),
        2..=5 => Some(FontId::Short),
        7 => Some(FontId::Narrow),
        8 => Some(FontId::SmallNarrow),
        _ => None,
    }
}

/// Returns the vertical cursor advance for a newline in `font`.
#[must_use]
pub const fn max_letter_height(font: FontId) -> i32 {
    match font {
        FontId::Small => 12,
        FontId::Normal | FontId::Narrow => 16,
        FontId::Short => 14,
        FontId::SmallNarrow => 8,
    }
}

/// Returns the number of rows a `CLEAR` or `CLEAR_TO` control erases in
/// `font`.
///
/// `ClearTextSpan` fills `gCurGlyph.height` rows (`pokeemerald/src/text.c`
/// `:649-674`), which is the constant each `DecompressGlyph_*` stores for its
/// Latin glyphs -- Small 13, Narrow 15, `SmallNarrow` 12, Short 14, Normal 15
/// (`text.c:1713`, `:1755`, `:1797`, `:1841`, `:1883`). It is a different
/// metric from [`max_letter_height`], the `\n` advance, so the two tables
/// disagree on every font but Short.
const fn clear_span_height(font: FontId) -> i32 {
    match font {
        FontId::Small => 13,
        FontId::Normal | FontId::Narrow => 15,
        FontId::Short => 14,
        FontId::SmallNarrow => 12,
    }
}

/// The pixels one `CLEAR` or `CLEAR_TO` control erased.
///
/// Upstream's `ClearTextSpan` paints this rectangle into the window's tile
/// buffer (`pokeemerald/src/text.c:649-674`). [`Printer`] owns no such buffer,
/// so it reports the geometry instead and the caller erases; see
/// [`Printer::cleared_span`]. This is a run within one text line, distinct
/// from a `FILL_WINDOW` clear ([`Printer::cleared_window`]) and from the
/// whole-page [`TickEvent::Cleared`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClearedSpan {
    /// Window-local left edge.
    pub x: i32,
    /// Window-local top edge.
    pub y: i32,
    /// Width in pixels; always positive.
    pub width: i32,
    /// Height in pixels: the font's Latin glyph height.
    pub height: i32,
}

/// Text reveal cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextSpeed {
    /// Eight frames per glyph.
    Slow,
    /// Four frames per glyph.
    Mid,
    /// One glyph per frame.
    Fast,
    /// No reveal delay.
    Instant,
}

impl TextSpeed {
    /// Returns the configured frame interval between glyph reveals.
    #[must_use]
    pub const fn frames_per_char(self) -> u8 {
        match self {
            Self::Slow => 8,
            Self::Mid => 4,
            Self::Fast => 1,
            Self::Instant => 0,
        }
    }

    const fn wait_frames(self) -> u8 {
        self.frames_per_char().saturating_sub(1)
    }

    fn scroll_px_per_frame(self, pixels_remaining: i32) -> i32 {
        match self {
            Self::Slow => pixels_remaining.min(1),
            Self::Mid => pixels_remaining.min(2),
            Self::Fast => pixels_remaining.min(4),
            Self::Instant => pixels_remaining,
        }
    }

    /// `GetPlayerTextSpeedDelay`'s validation of the saved
    /// `optionsTextSpeed` option (`OPTIONS_TEXT_SPEED_[SLOW/MID/FAST]` =
    /// `0`/`1`/`2`, `pokeemerald/include/constants/global.h:127-129`;
    /// `pokeemerald/src/menu.c:481-487`): `raw` selects directly at `0` or
    /// `2`, and anything else -- `1` (`MID` itself) or an out-of-range
    /// 3-bit value above `2` -- falls back to [`Self::Mid`], the same
    /// fallback upstream's own clamp lands on.
    #[must_use]
    pub const fn from_raw_option(raw: u8) -> Self {
        match raw {
            0 => Self::Slow,
            2 => Self::Fast,
            _ => Self::Mid,
        }
    }
}

/// A window-local pixel position.
///
/// Origins are unrestricted, so every cursor calculation saturates at the
/// `i32` bounds instead of wrapping or panicking.
pub type PixelPos = (i32, i32);

/// A and B button state for one printer frame.
#[expect(
    clippy::struct_excessive_bools,
    reason = "A and B each expose independent pressed and held states"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PrinterInput {
    /// Whether A was newly pressed.
    pub a_pressed: bool,
    /// Whether B was newly pressed.
    pub b_pressed: bool,
    /// Whether A is held.
    pub a_held: bool,
    /// Whether B is held.
    pub b_held: bool,
}

impl PrinterInput {
    /// Returns input with neither button pressed or held.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            a_pressed: false,
            b_pressed: false,
            a_held: false,
            b_held: false,
        }
    }

    /// Returns whether either button was newly pressed.
    #[must_use]
    pub const fn confirm_pressed(self) -> bool {
        self.a_pressed || self.b_pressed
    }

    /// Returns whether either button is held.
    #[must_use]
    pub const fn confirm_held(self) -> bool {
        self.a_held || self.b_held
    }
}

/// A glyph revealed by [`Printer::tick`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevealedGlyph {
    /// Window-local left edge.
    pub x: i32,
    /// Window-local top edge.
    pub y: i32,
    /// The glyph's bitmap and advance width.
    pub glyph: Glyph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrinterState {
    HandleChar,
    AwaitingScroll,
    Scrolling { pixels_remaining: i32 },
    AwaitingClear,
    AwaitingPress,
    Pause { frames_remaining: u8 },
    Finished,
}

/// The result of one [`Printer::tick`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickEvent {
    /// The frame revealed no glyph: either the reveal delay consumed it, or a
    /// `CLEAR`/`CLEAR_TO` control erased a span. Upstream ends the frame for
    /// both (`RENDER_UPDATE` and `RENDER_PRINT`, `pokeemerald/src/text.c`
    /// `:352-360`). [`Printer::cleared_span`] distinguishes them. At Slow/Mid
    /// speed a `FILL_WINDOW` control also produces this variant, the same as
    /// any other `RENDER_REPEAT`-class token: its own reveal-delay reload is
    /// spent before the next token starts (`text.c:941-962`); only at
    /// Fast/Instant does its glyph, if any, reach the caller within the same
    /// call. [`Printer::cleared_window`] distinguishes an idle `FILL_WINDOW`
    /// tick from an ordinary delay.
    Idle,
    /// A glyph became visible.
    Glyph(Box<RevealedGlyph>),
    /// A scroll prompt is waiting for a new A or B press.
    AwaitingScroll,
    /// A scroll was confirmed and will move on the next frame.
    ScrollStarted,
    /// The window contents moved upward.
    Scrolling {
        /// Pixels moved this frame.
        dy: i32,
    },
    /// The scroll completed; token handling resumes next frame.
    ScrollFinished,
    /// A page-clear prompt is waiting for a new A or B press.
    AwaitingClear,
    /// The page was confirmed and the cursor returned to its origin.
    /// [`Printer::cleared_window`] is also set on this tick.
    Cleared,
    /// `PAUSE_UNTIL_PRESS` is waiting for a new A or B press.
    AwaitingPress,
    /// The wait was satisfied; token handling resumes next frame.
    PressAccepted,
    /// A pause consumed one frame.
    Paused,
    /// The pause completed; token handling resumes next frame.
    PauseFinished,
    /// The token stream ended.
    Finished,
}

/// What handling one extended control code leaves for the frame to do.
///
/// These are upstream `RenderText`'s own return values for the control-code
/// cases (`pokeemerald/src/text.c:960-1135`), minus the ones no control in this
/// renderer produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlOutcome {
    /// `RENDER_REPEAT`: the control had no glyph of its own, so token handling
    /// continues within the same frame.
    Repeat,
    /// The escape control resolved to an extra-symbol glyph to reveal.
    Glyph(u16),
    /// `RENDER_PRINT`: a span was erased and the frame ends.
    ClearedSpan,
    /// `FILL_WINDOW`'s `RENDER_REPEAT`: the whole window was cleared and the
    /// cursor reset to the origin, but -- unlike [`Self::ClearedSpan`] --
    /// token handling continues within the same frame
    /// (`pokeemerald/src/text.c:1052-1056`).
    ClearedWindow,
}

#[must_use]
fn glyph_id_for_token(tok: &Token) -> Option<u16> {
    match *tok {
        Token::Char(c) => super::char_to_byte(c).map(u16::from),
        Token::Symbol(s) => Some(u16::from(super::byte_from_symbol(s))),
        Token::ExtraSymbol(idx) => Some(EXTRA_SYMBOL_PAGE_START | u16::from(idx)),
        Token::BardWordDelimit => Some(u16::from(super::CHAR_BARD_WORD_DELIMIT)),
        _ => None,
    }
}

/// A frame-driven printer over decoded text tokens.
///
/// The printer owns its glyph source and emits blit and layout events rather
/// than writing a pixel buffer.
#[derive(Debug)]
pub struct Printer<S> {
    tokens: Vec<Token>,
    next_token_index: usize,
    sheets: Vec<S>,
    active_sheet: usize,
    speed: TextSpeed,
    reveal_delay_frames_remaining: u8,
    origin: PixelPos,
    cursor: PixelPos,
    /// Template line spacing added to the active font's maximum letter height.
    line_spacing: u8,
    line_height: i32,
    clear_height: i32,
    /// `0` leaves the advance at the glyph's own width (`text.c` treats a
    /// zero `minLetterSpacing` as unset).
    min_letter_spacing: u8,
    cleared_span: Option<ClearedSpan>,
    cleared_window: bool,
    state: PrinterState,
    allows_ab_speed_up: bool,
    ab_speed_up_latched: bool,
}

impl<S: GlyphSource> Printer<S> {
    /// Creates a printer at the window-local `origin`.
    ///
    /// A/B speed-up is disabled until [`Self::with_ab_speed_up_print`] is used.
    #[must_use]
    pub fn new(tokens: Vec<Token>, glyphs: S, speed: TextSpeed, origin: PixelPos) -> Self {
        let font = glyphs.font();
        Self {
            tokens,
            next_token_index: 0,
            line_spacing: 0,
            line_height: max_letter_height(font),
            clear_height: clear_span_height(font),
            min_letter_spacing: 0,
            cleared_span: None,
            cleared_window: false,
            sheets: vec![glyphs],
            active_sheet: 0,
            speed,
            reveal_delay_frames_remaining: 0,
            origin,
            cursor: origin,
            state: PrinterState::HandleChar,
            allows_ab_speed_up: false,
            ab_speed_up_latched: false,
        }
    }

    /// Registers another glyph sheet a mid-string `FONT` control can select.
    ///
    /// The sheet passed to [`Self::new`] stays the initial font. A `FONT`
    /// control whose upstream id maps to a registered sheet switches to it
    /// and refreshes newline and clear-span metrics; an id with no sheet
    /// (including braille and the Japanese bold face) leaves the current
    /// sheet in place.
    #[must_use]
    pub fn with_font_sheet(mut self, sheet: S) -> Self {
        self.sheets.push(sheet);
        self
    }

    fn glyphs(&self) -> &S {
        &self.sheets[self.active_sheet]
    }

    /// Sets the template line spacing in pixels (`TextPrinterTemplate.lineSpacing`).
    ///
    /// A newline advances the cursor by the active font's maximum letter height
    /// plus this spacing (`pokeemerald/src/text.c:970`), and a scroll prompt
    /// scrolls the same distance (`text.c:1184`). A later `FONT` control keeps
    /// this spacing and swaps only the font's own height. Printers default to
    /// zero spacing.
    #[must_use]
    pub fn with_line_spacing(mut self, line_spacing: u8) -> Self {
        self.line_spacing = line_spacing;
        self.line_height = max_letter_height(self.glyphs().font()) + i32::from(line_spacing);
        self
    }

    /// Enables A/B speed-up for this printer.
    #[must_use]
    pub const fn with_ab_speed_up_print(mut self) -> Self {
        self.allows_ab_speed_up = true;
        self
    }

    /// The cursor's current window-local pixel position.
    #[must_use]
    pub const fn cursor(&self) -> PixelPos {
        self.cursor
    }

    /// The span the most recent [`Self::tick`] erased, if that tick handled a
    /// forward `CLEAR` or `CLEAR_TO`.
    ///
    /// Upstream erases the pixels itself; this printer emits events instead of
    /// owning a window buffer, so a caller that retains revealed glyphs uses
    /// this rectangle to erase them. At most one span exists per tick because
    /// upstream's `RENDER_PRINT` ends the frame (`pokeemerald/src/text.c`
    /// `:352-360`).
    #[must_use]
    pub const fn cleared_span(&self) -> Option<ClearedSpan> {
        self.cleared_span
    }

    /// Whether the most recent [`Self::tick`] cleared the complete window and
    /// reset the cursor to the origin: set by a forward `FILL_WINDOW`
    /// (`pokeemerald/src/text.c:1052-1056`) or by the page-clear prompt's
    /// confirmation ([`TickEvent::Cleared`]). Like [`Self::cleared_span`], the
    /// caller applies the erase itself; unlike it, `FILL_WINDOW` never ends
    /// upstream's frame (it is unconditionally `RENDER_REPEAT`), so at
    /// Fast/Instant speed a glyph decoded later in the same [`Self::tick`]
    /// call can still be its returned event -- at Slow/Mid that call instead
    /// returns [`TickEvent::Idle`], the same as any other `RENDER_REPEAT`
    /// token's own reveal-delay reload.
    #[must_use]
    pub const fn cleared_window(&self) -> bool {
        self.cleared_window
    }

    /// Returns whether the token stream has ended.
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        matches!(self.state, PrinterState::Finished)
    }

    /// Starts a new token stream with the existing glyph source and settings.
    pub fn restart(&mut self, tokens: Vec<Token>) {
        self.tokens = tokens;
        self.next_token_index = 0;
        self.reveal_delay_frames_remaining = 0;
        self.cursor = self.origin;
        self.cleared_span = None;
        self.cleared_window = false;
        self.state = PrinterState::HandleChar;
        self.ab_speed_up_latched = false;
    }

    /// Advance the printer by exactly one frame.
    ///
    /// New button presses confirm clear, scroll, and press-wait prompts.
    /// Pressing during a reveal delay latches A/B speed-up when enabled;
    /// later held frames skip remaining reveal delays.
    pub fn tick(&mut self, input: PrinterInput) -> TickEvent {
        self.cleared_span = None;
        self.cleared_window = false;
        match self.state {
            PrinterState::Finished => TickEvent::Finished,
            PrinterState::HandleChar => self.tick_handle_char(input),
            PrinterState::AwaitingScroll => {
                if input.confirm_pressed() {
                    self.cursor.0 = self.origin.0;
                    self.state = PrinterState::Scrolling {
                        pixels_remaining: self.line_height,
                    };
                    TickEvent::ScrollStarted
                } else {
                    TickEvent::AwaitingScroll
                }
            }
            PrinterState::Scrolling { pixels_remaining } => {
                if pixels_remaining > 0 {
                    let dy = self.speed.scroll_px_per_frame(pixels_remaining);
                    self.state = PrinterState::Scrolling {
                        pixels_remaining: pixels_remaining - dy,
                    };
                    TickEvent::Scrolling { dy }
                } else {
                    self.state = PrinterState::HandleChar;
                    TickEvent::ScrollFinished
                }
            }
            PrinterState::AwaitingClear => {
                if input.confirm_pressed() {
                    self.cursor = self.origin;
                    self.cleared_window = true;
                    self.state = PrinterState::HandleChar;
                    TickEvent::Cleared
                } else {
                    TickEvent::AwaitingClear
                }
            }
            PrinterState::AwaitingPress => {
                if input.confirm_pressed() {
                    self.state = PrinterState::HandleChar;
                    TickEvent::PressAccepted
                } else {
                    TickEvent::AwaitingPress
                }
            }
            PrinterState::Pause { frames_remaining } => self.tick_pause(frames_remaining),
        }
    }

    fn tick_pause(&mut self, frames_remaining: u8) -> TickEvent {
        if frames_remaining != 0 {
            self.state = PrinterState::Pause {
                frames_remaining: frames_remaining - 1,
            };
            TickEvent::Paused
        } else {
            self.state = PrinterState::HandleChar;
            TickEvent::PauseFinished
        }
    }

    fn tick_handle_char(&mut self, input: PrinterInput) -> TickEvent {
        loop {
            if self.consume_reveal_delay_frame(input) {
                return TickEvent::Idle;
            }
            self.reveal_delay_frames_remaining = self.speed.wait_frames();

            let Some(token) = self.tokens.get(self.next_token_index).cloned() else {
                self.state = PrinterState::Finished;
                return TickEvent::Finished;
            };
            self.next_token_index += 1;

            let glyph_id = match token {
                Token::End => {
                    self.state = PrinterState::Finished;
                    return TickEvent::Finished;
                }
                Token::Newline => {
                    self.cursor.0 = self.origin.0;
                    self.cursor.1 = self.cursor.1.saturating_add(self.line_height);
                    continue;
                }
                Token::PromptScroll => {
                    self.state = PrinterState::AwaitingScroll;
                    return TickEvent::AwaitingScroll;
                }
                Token::PromptClear => {
                    self.state = PrinterState::AwaitingClear;
                    return TickEvent::AwaitingClear;
                }
                Token::ExtCtrl { sub, args } if sub == ext_ctrl::PAUSE => {
                    let frames_remaining = args.first().copied().unwrap_or(0);
                    self.reveal_delay_frames_remaining = 0;
                    self.state = PrinterState::Pause { frames_remaining };
                    return self.tick_pause(frames_remaining);
                }
                // Non-auto-scroll `TextPrinterWait` only (`text.c:884-899`);
                // auto-scroll is unmodelled in this port.
                Token::ExtCtrl { sub, .. } if sub == ext_ctrl::PAUSE_UNTIL_PRESS => {
                    self.state = PrinterState::AwaitingPress;
                    return TickEvent::AwaitingPress;
                }
                Token::ExtCtrl { sub, args } => match self.apply_extended_control(sub, &args) {
                    ControlOutcome::Repeat => None,
                    ControlOutcome::Glyph(id) => Some(id),
                    // Upstream's `RENDER_PRINT` copies the erased window to
                    // VRAM and ends the frame (`text.c:352-360`).
                    ControlOutcome::ClearedSpan => return TickEvent::Idle,
                    // `FILL_WINDOW` is also `RENDER_REPEAT` (`text.c`
                    // `:1052-1056`): the caller reads `cleared_window()`
                    // alongside whichever event this call eventually
                    // returns, so token handling keeps going here.
                    ControlOutcome::ClearedWindow => {
                        self.cleared_window = true;
                        None
                    }
                },
                ref other => glyph_id_for_token(other),
            };

            let Some(glyph_id) = glyph_id else {
                continue;
            };
            let Some(glyph) = self.glyphs().glyph(glyph_id) else {
                continue;
            };

            let advance_width = glyph.advance_width;
            let placement = RevealedGlyph {
                x: self.cursor.0,
                y: self.cursor.1,
                glyph,
            };
            self.advance_past_glyph(advance_width);
            return TickEvent::Glyph(Box::new(placement));
        }
    }

    /// Moves the cursor past a glyph, widening the step to `minLetterSpacing`
    /// and recording the leftover gap when that minimum is larger
    /// (`pokeemerald/src/text.c:1149-1165`).
    ///
    /// The gap is cleared after the glyph's own advance, so it never covers
    /// the glyph. A minimum of zero, or one the glyph already meets, adds
    /// nothing beyond the glyph's advance width.
    fn advance_past_glyph(&mut self, advance_width: u8) {
        let advance = i32::from(advance_width);
        self.cursor.0 = self.cursor.0.saturating_add(advance);
        if self.min_letter_spacing == 0 {
            return;
        }
        let gap = i32::from(self.min_letter_spacing) - advance;
        if gap <= 0 {
            return;
        }
        let representable = gap.min(i32::MAX.saturating_sub(self.cursor.0));
        if representable > 0 {
            self.cleared_span = Some(ClearedSpan {
                x: self.cursor.0,
                y: self.cursor.1,
                width: representable,
                height: self.clear_height,
            });
            self.cursor.0 = self.cursor.0.saturating_add(representable);
        }
    }

    fn consume_reveal_delay_frame(&mut self, input: PrinterInput) -> bool {
        if input.confirm_held() && self.ab_speed_up_latched {
            self.reveal_delay_frames_remaining = 0;
        }
        if self.reveal_delay_frames_remaining == 0 {
            return false;
        }

        self.reveal_delay_frames_remaining -= 1;
        if self.allows_ab_speed_up && input.confirm_pressed() {
            self.ab_speed_up_latched = true;
            self.reveal_delay_frames_remaining = 0;
        }
        true
    }

    fn apply_extended_control(&mut self, subcode: u8, arguments: &[u8]) -> ControlOutcome {
        match subcode {
            ext_ctrl::ESCAPE => {
                if let Some(&index) = arguments.first() {
                    return ControlOutcome::Glyph(EXTRA_SYMBOL_PAGE_START | u16::from(index));
                }
            }
            ext_ctrl::SHIFT_RIGHT | ext_ctrl::SKIP => {
                if let Some(&offset) = arguments.first() {
                    self.cursor.0 = self.origin.0.saturating_add(i32::from(offset));
                }
            }
            ext_ctrl::SHIFT_DOWN => {
                if let Some(&offset) = arguments.first() {
                    self.cursor.1 = self.origin.1.saturating_add(i32::from(offset));
                }
            }
            // `text.c:1063-1072`: the argument is the width to erase from the
            // cursor, which then advances by it.
            ext_ctrl::CLEAR => {
                let width = arguments.first().map_or(0, |&width| i32::from(width));
                return self.clear_span(width);
            }
            // `text.c:1077-1090`: the argument is an origin-relative column;
            // the width erased is the distance to it, so a column at or behind
            // the cursor erases nothing and never moves the cursor back.
            ext_ctrl::CLEAR_TO => {
                let Some(&column) = arguments.first() else {
                    return ControlOutcome::Repeat;
                };
                let target = self.origin.0.saturating_add(i32::from(column));
                return self.clear_span(target.saturating_sub(self.cursor.0));
            }
            // `text.c:1052-1056`: fills the whole window and resets both
            // cursor axes to the printer origin, then repeats within the
            // same frame.
            ext_ctrl::FILL_WINDOW => {
                self.cursor = self.origin;
                return ControlOutcome::ClearedWindow;
            }
            // `text.c:1007-1010`: the argument is an upstream `FONT_*` id.
            // Switching requires a sheet registered with [`Self::with_font_sheet`];
            // braille, the Japanese bold face, and any unregistered id stay on
            // the current sheet so the metrics cannot drift from the glyphs.
            ext_ctrl::FONT => {
                let Some(&id) = arguments.first() else {
                    return ControlOutcome::Repeat;
                };
                let Some(font) = modelled_font(id) else {
                    return ControlOutcome::Repeat;
                };
                let Some(index) = self.sheets.iter().position(|sheet| sheet.font() == font) else {
                    return ControlOutcome::Repeat;
                };
                self.active_sheet = index;
                self.clear_height = clear_span_height(font);
                self.line_height = max_letter_height(font) + i32::from(self.line_spacing);
            }
            // `text.c:1091-1092`: stored and applied after each later glyph.
            ext_ctrl::MIN_LETTER_SPACING => {
                if let Some(&spacing) = arguments.first() {
                    self.min_letter_spacing = spacing;
                }
            }
            _ => {}
        }
        ControlOutcome::Repeat
    }

    /// Records `width` pixels erased at the cursor and advances past them.
    ///
    /// Upstream guards both clear controls with `width > 0`
    /// (`pokeemerald/src/text.c:1066`, `:1084`), so a non-positive width
    /// neither erases nor ends the frame.
    ///
    /// The recorded span stops at `i32::MAX`, where the cursor saturates, so
    /// it never reaches a column the cursor cannot represent; a positive width
    /// still ends the frame even when no column remains.
    fn clear_span(&mut self, width: i32) -> ControlOutcome {
        if width <= 0 {
            return ControlOutcome::Repeat;
        }
        let representable = width.min(i32::MAX.saturating_sub(self.cursor.0));
        if representable > 0 {
            self.cleared_span = Some(ClearedSpan {
                x: self.cursor.0,
                y: self.cursor.1,
                width: representable,
                height: self.clear_height,
            });
            self.cursor.0 = self.cursor.0.saturating_add(representable);
        }
        ControlOutcome::ClearedSpan
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assets::fonts::{FontGlyphSheet, FontId, FontImageRef, SHEET_HEIGHT, SHEET_WIDTH};
    use assets::pack::ImageRef;

    const ENCODED_A: u8 = 0xBB;
    const NORMAL_A_ADVANCE_WIDTH: u8 = 6;
    const BARD_DELIMITER_ADVANCE_WIDTH: u8 = 3;
    const PAUSE_DURATION: u8 = 96;

    fn blank_sheet_pixels() -> Vec<u8> {
        vec![0u8; (SHEET_WIDTH * SHEET_HEIGHT) as usize]
    }

    fn synthetic_sheet(pixels: &[u8], font: FontId) -> FontGlyphSheet<'_> {
        let image = ImageRef {
            width: SHEET_WIDTH,
            height: SHEET_HEIGHT,
            bit_depth: 2,
            pixels,
        };
        FontGlyphSheet::new(FontImageRef::new_for_tests(font, image)).unwrap()
    }

    fn decode_tokens(bytes: &[u8]) -> Vec<Token> {
        super::super::decode(bytes).unwrap()
    }

    const fn press_a() -> PrinterInput {
        PrinterInput {
            a_pressed: true,
            b_pressed: false,
            a_held: false,
            b_held: false,
        }
    }

    #[test]
    fn advance_widths_move_the_cursor_by_the_sheets_glyph_width() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[ENCODED_A, ENCODED_A, super::super::EOS]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (0, 1));

        let TickEvent::Glyph(first) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph")
        };
        assert_eq!((first.x, first.y), (0, 1));
        assert_eq!(first.glyph.advance_width, NORMAL_A_ADVANCE_WIDTH);

        let TickEvent::Glyph(second) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph")
        };
        assert_eq!((second.x, second.y), (i32::from(NORMAL_A_ADVANCE_WIDTH), 1));
        assert_eq!(printer.cursor(), (i32::from(NORMAL_A_ADVANCE_WIDTH) * 2, 1));
    }

    #[test]
    fn bard_word_delimiter_advances_by_the_blank_glyph_width() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            ENCODED_A,
            super::super::CHAR_BARD_WORD_DELIMIT,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (0, 1));

        let TickEvent::Glyph(first) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph")
        };
        assert_eq!((first.x, first.y), (0, 1));

        let TickEvent::Glyph(delim) = printer.tick(PrinterInput::none()) else {
            panic!("expected the delimiter's blank glyph")
        };
        assert_eq!((delim.x, delim.y), (i32::from(NORMAL_A_ADVANCE_WIDTH), 1));
        assert_eq!(delim.glyph.advance_width, BARD_DELIMITER_ADVANCE_WIDTH);

        let TickEvent::Glyph(second) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph")
        };
        assert_eq!(
            (second.x, second.y),
            (
                i32::from(NORMAL_A_ADVANCE_WIDTH + BARD_DELIMITER_ADVANCE_WIDTH),
                1
            ),
            "words must stay separated"
        );
    }

    #[test]
    fn every_font_reports_the_upstream_max_letter_height() {
        assert_eq!(max_letter_height(FontId::Small), 12);
        assert_eq!(max_letter_height(FontId::Normal), 16);
        assert_eq!(max_letter_height(FontId::Short), 14);
        assert_eq!(max_letter_height(FontId::Narrow), 16);
        assert_eq!(max_letter_height(FontId::SmallNarrow), 8);
    }

    #[test]
    fn newline_resets_x_and_advances_y_by_line_height_at_instant_speed() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            ENCODED_A,
            super::super::CHAR_NEWLINE,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (0, 1));

        let TickEvent::Glyph(first) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph")
        };
        assert_eq!((first.x, first.y), (0, 1));

        let TickEvent::Glyph(second) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph after the free newline")
        };
        assert_eq!((second.x, second.y), (0, 1 + 16));
    }

    #[test]
    fn newline_costs_a_reveal_delay_period_at_mid_speed() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            ENCODED_A,
            super::super::CHAR_NEWLINE,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Mid, (0, 0));

        let TickEvent::Glyph(first) = printer.tick(PrinterInput::none()) else {
            panic!("expected the first glyph on frame 0")
        };
        assert_eq!((first.x, first.y), (0, 0));

        for frame in 1..=6 {
            assert_eq!(
                printer.tick(PrinterInput::none()),
                TickEvent::Idle,
                "frame {frame} should be idle"
            );
        }

        let TickEvent::Glyph(second) = printer.tick(PrinterInput::none()) else {
            panic!("expected the second glyph on frame 7")
        };
        assert_eq!((second.x, second.y), (0, 16));
    }

    #[test]
    fn reveal_cadence_matches_frames_per_char_table() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[ENCODED_A, ENCODED_A, super::super::EOS]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Mid, (0, 0));

        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
        for _ in 0..3 {
            assert_eq!(printer.tick(PrinterInput::none()), TickEvent::Idle);
        }
        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
    }

    #[test]
    fn fast_speed_reveals_a_glyph_every_frame() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[ENCODED_A, ENCODED_A, ENCODED_A, super::super::EOS]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Fast, (0, 0));

        for _ in 0..3 {
            assert!(matches!(
                printer.tick(PrinterInput::none()),
                TickEvent::Glyph(_)
            ));
        }
    }

    #[test]
    fn slow_speed_waits_seven_frames_between_glyphs() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[ENCODED_A, ENCODED_A, super::super::EOS]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Slow, (0, 0));

        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
        for _ in 0..7 {
            assert_eq!(printer.tick(PrinterInput::none()), TickEvent::Idle);
        }
        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
    }

    #[test]
    fn line_spacing_extends_the_newline_advance() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            ENCODED_A,
            super::super::CHAR_NEWLINE,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer =
            Printer::new(tokens, sheet, TextSpeed::Instant, (0, 1)).with_line_spacing(1);
        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
        let TickEvent::Glyph(g) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph after the newline")
        };
        assert_eq!((g.x, g.y), (0, 1 + 16 + 1));
    }

    #[test]
    fn line_spacing_extends_the_scroll_distance() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            ENCODED_A,
            super::super::CHAR_PROMPT_SCROLL,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Fast, (0, 1)).with_line_spacing(1);
        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
        assert_eq!(
            printer.tick(PrinterInput::none()),
            TickEvent::AwaitingScroll
        );
        assert_eq!(printer.tick(press_a()), TickEvent::ScrollStarted);
        let mut total = 0;
        let mut frames = 0;
        loop {
            match printer.tick(PrinterInput::none()) {
                TickEvent::Scrolling { dy } => {
                    total += dy;
                    frames += 1;
                }
                TickEvent::ScrollFinished => break,
                other => panic!("unexpected event {other:?}"),
            }
        }
        // 17 px at the fast speed's 4 px/frame: 4, 4, 4, 4, 1.
        assert_eq!((total, frames), (17, 5));
    }

    #[test]
    fn prompt_scroll_waits_for_confirm_then_animates_the_scroll() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            ENCODED_A,
            super::super::CHAR_PROMPT_SCROLL,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Fast, (0, 1));

        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
        assert_eq!(
            printer.tick(PrinterInput::none()),
            TickEvent::AwaitingScroll
        );
        assert_eq!(
            printer.tick(PrinterInput::none()),
            TickEvent::AwaitingScroll
        );
        assert_eq!(printer.tick(press_a()), TickEvent::ScrollStarted);
        for _ in 0..4 {
            assert_eq!(
                printer.tick(PrinterInput::none()),
                TickEvent::Scrolling { dy: 4 }
            );
        }
        assert_eq!(
            printer.tick(PrinterInput::none()),
            TickEvent::ScrollFinished
        );
        let TickEvent::Glyph(g) = printer.tick(PrinterInput::none()) else {
            panic!("expected printing to resume after the scroll")
        };
        assert_eq!((g.x, g.y), (0, 1));
    }

    #[test]
    fn prompt_clear_waits_for_confirm_then_resets_the_cursor_on_page() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            ENCODED_A,
            super::super::CHAR_NEWLINE,
            super::super::CHAR_PROMPT_CLEAR,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Fast, (0, 1));

        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
        assert_eq!(printer.tick(PrinterInput::none()), TickEvent::AwaitingClear);
        assert_eq!(printer.tick(PrinterInput::none()), TickEvent::AwaitingClear);
        assert_eq!(printer.tick(press_a()), TickEvent::Cleared);
        assert_eq!(printer.cursor(), (0, 1));
        assert!(
            printer.cleared_window(),
            "the page-clear confirmation should also report a whole-window clear"
        );
        let TickEvent::Glyph(g) = printer.tick(PrinterInput::none()) else {
            panic!("expected printing to resume on the new page")
        };
        assert_eq!((g.x, g.y), (0, 1));
        assert!(
            !printer.cleared_window(),
            "the flag should not persist into the next tick"
        );
    }

    #[test]
    fn page_clear_resumes_after_a_reveal_delay_at_mid_speed() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            ENCODED_A,
            super::super::CHAR_PROMPT_CLEAR,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Mid, (0, 0));

        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
        for frame in 1..=3 {
            assert_eq!(
                printer.tick(PrinterInput::none()),
                TickEvent::Idle,
                "frame {frame} should be idle"
            );
        }
        assert_eq!(printer.tick(PrinterInput::none()), TickEvent::AwaitingClear);
        assert_eq!(printer.tick(PrinterInput::none()), TickEvent::AwaitingClear);
        assert_eq!(printer.tick(press_a()), TickEvent::Cleared);
        assert_eq!(printer.cursor(), (0, 0));
        for frame in 7..=9 {
            assert_eq!(
                printer.tick(PrinterInput::none()),
                TickEvent::Idle,
                "resume frame {frame} should be idle"
            );
        }
        let TickEvent::Glyph(first_glyph_on_new_page) = printer.tick(PrinterInput::none()) else {
            panic!("expected the new page's glyph on frame 10")
        };
        assert_eq!(
            (first_glyph_on_new_page.x, first_glyph_on_new_page.y),
            (0, 0)
        );
    }

    #[test]
    fn scroll_resumes_after_a_reveal_delay_at_mid_speed() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            ENCODED_A,
            super::super::CHAR_PROMPT_SCROLL,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Mid, (0, 1));

        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
        for _ in 0..3 {
            assert_eq!(printer.tick(PrinterInput::none()), TickEvent::Idle);
        }
        assert_eq!(
            printer.tick(PrinterInput::none()),
            TickEvent::AwaitingScroll
        );
        assert_eq!(
            printer.tick(PrinterInput::none()),
            TickEvent::AwaitingScroll
        );
        assert_eq!(printer.tick(press_a()), TickEvent::ScrollStarted);
        for _ in 0..8 {
            assert_eq!(
                printer.tick(PrinterInput::none()),
                TickEvent::Scrolling { dy: 2 }
            );
        }
        assert_eq!(
            printer.tick(PrinterInput::none()),
            TickEvent::ScrollFinished
        );
        for _ in 0..3 {
            assert_eq!(printer.tick(PrinterInput::none()), TickEvent::Idle);
        }
        let TickEvent::Glyph(g) = printer.tick(PrinterInput::none()) else {
            panic!("expected printing to resume after the scroll's reveal delay")
        };
        assert_eq!((g.x, g.y), (0, 1));
    }

    #[test]
    fn finished_is_terminal_and_idempotent() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[ENCODED_A, super::super::EOS]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (0, 0));

        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
        assert_eq!(printer.tick(PrinterInput::none()), TickEvent::Finished);
        assert!(printer.is_finished());
        assert_eq!(printer.tick(PrinterInput::none()), TickEvent::Finished);
    }

    #[test]
    fn extra_symbol_addresses_the_extended_glyph_page() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[super::super::CHAR_EXTRA_SYMBOL, 0, super::super::EOS]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (0, 0));

        let TickEvent::Glyph(g) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph")
        };
        assert_eq!(
            g.glyph.advance_width,
            FontId::Normal.glyph_width(EXTRA_SYMBOL_PAGE_START).unwrap()
        );
    }

    #[test]
    fn unsupported_tokens_are_skipped_without_costing_a_glyph() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            super::super::PLACEHOLDER_BEGIN,
            super::super::PLACEHOLDER_PLAYER,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (0, 0));

        let TickEvent::Glyph(g) = printer.tick(PrinterInput::none()) else {
            panic!("expected the placeholder to be skipped and 'A' drawn")
        };
        assert_eq!((g.x, g.y), (0, 0));
    }

    #[test]
    fn shift_right_extended_control_moves_the_cursor() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            super::super::EXT_CTRL_CODE_BEGIN,
            ext_ctrl::SHIFT_RIGHT,
            20,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (0, 1));

        let TickEvent::Glyph(g) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph")
        };
        assert_eq!((g.x, g.y), (20, 1));
    }

    #[test]
    fn min_letter_spacing_widens_the_advance_and_clears_the_gap() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            super::super::EXT_CTRL_CODE_BEGIN,
            ext_ctrl::MIN_LETTER_SPACING,
            12,
            ENCODED_A,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (0, 1));

        let TickEvent::Glyph(first) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph")
        };
        assert_eq!((first.x, first.y), (0, 1));
        assert_eq!(
            printer.cleared_span(),
            Some(ClearedSpan {
                x: i32::from(NORMAL_A_ADVANCE_WIDTH),
                y: 1,
                width: 12 - i32::from(NORMAL_A_ADVANCE_WIDTH),
                height: 15,
            })
        );
        assert_eq!(printer.cursor(), (12, 1));

        let TickEvent::Glyph(second) = printer.tick(PrinterInput::none()) else {
            panic!("expected a second glyph")
        };
        assert_eq!((second.x, second.y), (12, 1));
        assert_eq!(printer.cursor(), (24, 1));
    }

    #[test]
    fn min_letter_spacing_does_not_clear_when_the_glyph_is_already_wider() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            super::super::EXT_CTRL_CODE_BEGIN,
            ext_ctrl::MIN_LETTER_SPACING,
            1,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (0, 1));

        let TickEvent::Glyph(glyph) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph")
        };
        assert_eq!((glyph.x, glyph.y), (0, 1));
        assert_eq!(printer.cleared_span(), None);
        assert_eq!(printer.cursor(), (i32::from(NORMAL_A_ADVANCE_WIDTH), 1));
    }

    #[test]
    fn font_control_switches_the_supplied_glyph_source_and_line_metrics() {
        let pixels = blank_sheet_pixels();
        let normal = synthetic_sheet(&pixels, FontId::Normal);
        let small = synthetic_sheet(&pixels, FontId::Small);
        let small_advance = FontId::Small
            .glyph_width(u16::from(ENCODED_A))
            .expect("small 'A' has a width");
        assert_ne!(small_advance, NORMAL_A_ADVANCE_WIDTH);
        let tokens = decode_tokens(&[
            super::super::EXT_CTRL_CODE_BEGIN,
            ext_ctrl::FONT,
            0, // FONT_SMALL
            ENCODED_A,
            super::super::CHAR_NEWLINE,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer =
            Printer::new(tokens, normal, TextSpeed::Instant, (0, 1)).with_font_sheet(small);

        let TickEvent::Glyph(first) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph from the small font")
        };
        assert_eq!((first.x, first.y), (0, 1));
        assert_eq!(first.glyph.advance_width, small_advance);

        let TickEvent::Glyph(second) = printer.tick(PrinterInput::none()) else {
            panic!("expected the glyph after the newline")
        };
        assert_eq!(
            (second.x, second.y),
            (0, 1 + max_letter_height(FontId::Small))
        );
        assert_eq!(second.glyph.advance_width, small_advance);
    }

    #[test]
    fn font_control_without_a_sheet_keeps_the_current_glyphs() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            super::super::EXT_CTRL_CODE_BEGIN,
            ext_ctrl::FONT,
            0, // FONT_SMALL, no sheet registered
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (0, 1));

        let TickEvent::Glyph(glyph) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph")
        };
        assert_eq!(glyph.glyph.advance_width, NORMAL_A_ADVANCE_WIDTH);
    }

    #[test]
    fn clear_erases_its_width_at_the_cursor_and_advances_past_it() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            ENCODED_A,
            super::super::EXT_CTRL_CODE_BEGIN,
            ext_ctrl::CLEAR,
            4,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (3, 1));

        let TickEvent::Glyph(before) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph")
        };
        assert_eq!((before.x, before.y), (3, 1));
        assert_eq!(printer.cleared_span(), None);

        let cursor_before_clear = printer.cursor();
        assert_eq!(
            cursor_before_clear,
            (3 + i32::from(NORMAL_A_ADVANCE_WIDTH), 1)
        );
        assert_eq!(printer.tick(PrinterInput::none()), TickEvent::Idle);
        assert_eq!(
            printer.cleared_span(),
            Some(ClearedSpan {
                x: cursor_before_clear.0,
                y: 1,
                width: 4,
                height: 15,
            })
        );
        assert_eq!(printer.cursor(), (cursor_before_clear.0 + 4, 1));

        let TickEvent::Glyph(after) = printer.tick(PrinterInput::none()) else {
            panic!("expected the glyph after the cleared span")
        };
        assert_eq!((after.x, after.y), (cursor_before_clear.0 + 4, 1));
        assert_eq!(
            printer.cleared_span(),
            None,
            "the span belongs to the tick that erased it"
        );
    }

    fn extreme_printer<'a>(
        pixels: &'a [u8],
        bytes: &[u8],
        origin: PixelPos,
    ) -> Printer<FontGlyphSheet<'a>> {
        let sheet = synthetic_sheet(pixels, FontId::Normal);
        Printer::new(decode_tokens(bytes), sheet, TextSpeed::Instant, origin)
    }

    #[test]
    fn newline_saturates_y_at_the_i32_maximum() {
        let pixels = blank_sheet_pixels();
        let mut printer = extreme_printer(
            &pixels,
            &[super::super::CHAR_NEWLINE, ENCODED_A, super::super::EOS],
            (0, i32::MAX - 1),
        );
        let TickEvent::Glyph(g) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph")
        };
        assert_eq!((g.x, g.y), (0, i32::MAX));
    }

    #[test]
    fn glyph_advance_saturates_x_at_the_i32_maximum() {
        let pixels = blank_sheet_pixels();
        let mut printer = extreme_printer(
            &pixels,
            &[ENCODED_A, ENCODED_A, super::super::EOS],
            (i32::MAX, 0),
        );
        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
        assert_eq!(printer.cursor(), (i32::MAX, 0));
        let TickEvent::Glyph(g) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph")
        };
        assert_eq!(g.x, i32::MAX);
    }

    #[test]
    fn shift_controls_saturate_at_the_i32_maximum() {
        let pixels = blank_sheet_pixels();
        let mut printer = extreme_printer(
            &pixels,
            &[
                super::super::EXT_CTRL_CODE_BEGIN,
                ext_ctrl::SHIFT_RIGHT,
                255,
                super::super::EXT_CTRL_CODE_BEGIN,
                ext_ctrl::SHIFT_DOWN,
                255,
                ENCODED_A,
                super::super::EOS,
            ],
            (i32::MAX - 10, i32::MAX - 10),
        );
        let TickEvent::Glyph(g) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph")
        };
        assert_eq!((g.x, g.y), (i32::MAX, i32::MAX));
    }

    #[test]
    fn clear_saturates_the_cursor_at_the_i32_maximum() {
        let pixels = blank_sheet_pixels();
        let mut printer = extreme_printer(
            &pixels,
            &[
                super::super::EXT_CTRL_CODE_BEGIN,
                ext_ctrl::CLEAR,
                255,
                super::super::EOS,
            ],
            (i32::MAX - 10, 0),
        );
        assert_eq!(printer.tick(PrinterInput::none()), TickEvent::Idle);
        assert_eq!(printer.cursor(), (i32::MAX, 0));
        assert_eq!(
            printer.cleared_span().map(|span| (span.x, span.width)),
            Some((i32::MAX - 10, 10))
        );
    }

    #[test]
    fn clear_at_the_saturated_cursor_still_ends_the_frame() {
        let pixels = blank_sheet_pixels();
        let mut printer = extreme_printer(
            &pixels,
            &[
                super::super::EXT_CTRL_CODE_BEGIN,
                ext_ctrl::CLEAR,
                1,
                ENCODED_A,
                super::super::EOS,
            ],
            (i32::MAX, 0),
        );
        assert_eq!(printer.tick(PrinterInput::none()), TickEvent::Idle);
        assert_eq!(printer.cleared_span(), None);
        assert_eq!(printer.cursor(), (i32::MAX, 0));
        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
    }

    #[test]
    fn clear_to_saturates_its_target_at_the_i32_maximum() {
        let pixels = blank_sheet_pixels();
        let mut printer = extreme_printer(
            &pixels,
            &[
                super::super::EXT_CTRL_CODE_BEGIN,
                ext_ctrl::CLEAR_TO,
                255,
                super::super::EOS,
            ],
            (i32::MAX - 10, 0),
        );
        assert_eq!(printer.tick(PrinterInput::none()), TickEvent::Idle);
        assert_eq!(printer.cursor(), (i32::MAX, 0));
    }

    #[test]
    fn clear_to_erases_up_to_the_origin_relative_column() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            ENCODED_A,
            super::super::EXT_CTRL_CODE_BEGIN,
            ext_ctrl::CLEAR_TO,
            20,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (3, 1));

        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
        let cursor_before_clear = printer.cursor();
        assert_eq!(cursor_before_clear, (9, 1));

        assert_eq!(printer.tick(PrinterInput::none()), TickEvent::Idle);
        // The target column is relative to the origin, so it is x 23, and the
        // erased width is the distance the cursor still had to travel.
        assert_eq!(
            printer.cleared_span(),
            Some(ClearedSpan {
                x: 9,
                y: 1,
                width: 14,
                height: 15,
            })
        );
        assert_eq!(printer.cursor(), (23, 1));

        let TickEvent::Glyph(aligned) = printer.tick(PrinterInput::none()) else {
            panic!("expected the glyph at the cleared column")
        };
        assert_eq!((aligned.x, aligned.y), (23, 1));
    }

    #[test]
    fn a_clear_with_nothing_ahead_erases_nothing_and_costs_no_frame() {
        // Each case leaves the cursor at x 9 after the leading 'A', so none of
        // them has a positive width to erase (`text.c:1066`, `:1084`). The
        // backward CLEAR_TO is what separates it from its sibling SKIP, which
        // would move the cursor to x 5.
        for (subcode, argument) in [
            (ext_ctrl::CLEAR, 0),
            (ext_ctrl::CLEAR_TO, 6),
            (ext_ctrl::CLEAR_TO, 2),
        ] {
            let pixels = blank_sheet_pixels();
            let sheet = synthetic_sheet(&pixels, FontId::Normal);
            let tokens = decode_tokens(&[
                ENCODED_A,
                super::super::EXT_CTRL_CODE_BEGIN,
                subcode,
                argument,
                ENCODED_A,
                super::super::EOS,
            ]);
            let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (3, 1));

            assert!(matches!(
                printer.tick(PrinterInput::none()),
                TickEvent::Glyph(_)
            ));

            let TickEvent::Glyph(after) = printer.tick(PrinterInput::none()) else {
                panic!("expected {subcode:#04X} {argument} to be consumed within the frame")
            };
            assert_eq!((after.x, after.y), (9, 1));
            assert_eq!(printer.cleared_span(), None);
        }
    }

    #[test]
    fn fill_window_clears_stale_glyphs_and_resets_the_cursor_within_the_same_frame() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        // The Bard's song appends `EXT_CTRL_CODE_BEGIN, FILL_WINDOW` between
        // paragraphs (`pokeemerald/src/mauville_old_man.c:224-230`).
        let tokens = decode_tokens(&[
            ENCODED_A,
            super::super::CHAR_NEWLINE,
            ENCODED_A,
            super::super::EXT_CTRL_CODE_BEGIN,
            ext_ctrl::FILL_WINDOW,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (0, 1));

        let TickEvent::Glyph(first) = printer.tick(PrinterInput::none()) else {
            panic!("expected the first glyph")
        };
        assert_eq!((first.x, first.y), (0, 1));
        assert!(!printer.cleared_window());

        let TickEvent::Glyph(second) = printer.tick(PrinterInput::none()) else {
            panic!("expected the second-line glyph")
        };
        assert_eq!((second.x, second.y), (0, 17));

        // `text.c:1052-1056` fills the window, resets both cursor axes to the
        // printer origin, and returns `RENDER_REPEAT`, so the glyph after
        // `FILL_WINDOW` still renders within this same tick call.
        let TickEvent::Glyph(after_fill) = printer.tick(PrinterInput::none()) else {
            panic!("expected the glyph after FILL_WINDOW in the same frame")
        };
        assert_eq!((after_fill.x, after_fill.y), (0, 1));
        assert!(
            printer.cleared_window(),
            "FILL_WINDOW must report a whole-window clear"
        );
        assert_eq!(
            printer.cursor(),
            (i32::from(NORMAL_A_ADVANCE_WIDTH), 1),
            "FILL_WINDOW must reset the cursor to the origin before the next glyph advances it"
        );
    }

    #[test]
    fn restart_discards_the_span_the_previous_stream_cleared() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            super::super::EXT_CTRL_CODE_BEGIN,
            ext_ctrl::CLEAR,
            4,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (0, 1));

        assert_eq!(printer.tick(PrinterInput::none()), TickEvent::Idle);
        assert!(printer.cleared_span().is_some());

        printer.restart(decode_tokens(&[ENCODED_A, super::super::EOS]));
        assert_eq!(printer.cleared_span(), None);

        let TickEvent::Glyph(g) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph")
        };
        assert_eq!((g.x, g.y), (0, 1));
    }

    #[test]
    fn cleared_span_height_is_the_fonts_latin_glyph_height() {
        for (font, height) in [
            (FontId::Small, 13),
            (FontId::Normal, 15),
            (FontId::Narrow, 15),
            (FontId::Short, 14),
            (FontId::SmallNarrow, 12),
        ] {
            assert_eq!(clear_span_height(font), height);
        }
        // `gCurGlyph.height` is not `sFontInfos`' `maxLetterHeight`; only
        // Short's two metrics agree.
        assert_eq!(
            clear_span_height(FontId::Short),
            max_letter_height(FontId::Short)
        );
        for font in [
            FontId::Small,
            FontId::Normal,
            FontId::Narrow,
            FontId::SmallNarrow,
        ] {
            assert_ne!(clear_span_height(font), max_letter_height(font));
        }
    }

    #[test]
    fn restart_reprints_a_new_stream_from_the_top_keeping_the_sheet() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[ENCODED_A, super::super::EOS]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (3, 1));

        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Finished
        ));
        assert!(printer.is_finished());

        printer.restart(decode_tokens(&[ENCODED_A, ENCODED_A, super::super::EOS]));
        assert!(!printer.is_finished(), "a restarted printer prints again");
        assert_eq!(printer.cursor(), (3, 1), "the cursor is back at the origin");

        let TickEvent::Glyph(first) = printer.tick(PrinterInput::none()) else {
            panic!("expected the restarted stream's first glyph")
        };
        assert_eq!((first.x, first.y), (3, 1));
        let TickEvent::Glyph(second) = printer.tick(PrinterInput::none()) else {
            panic!("expected the restarted stream's second glyph")
        };
        assert_eq!(
            (second.x, second.y),
            (3 + i32::from(NORMAL_A_ADVANCE_WIDTH), 1),
            "advanced by the first glyph's width"
        );
        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Finished
        ));
    }

    #[test]
    fn a_printer_over_an_owned_sheet_outlives_the_pack_bytes_it_came_from() {
        let mut printer = {
            let pixels = blank_sheet_pixels();
            let owned = synthetic_sheet(&pixels, FontId::Normal).to_owned_sheet();
            Printer::new(
                decode_tokens(&[ENCODED_A, super::super::EOS]),
                owned,
                TextSpeed::Instant,
                (0, 1),
            )
        };

        let TickEvent::Glyph(g) = printer.tick(PrinterInput::none()) else {
            panic!("expected a glyph from the owned sheet")
        };
        assert_eq!((g.x, g.y), (0, 1));
        assert_eq!(g.glyph.advance_width, NORMAL_A_ADVANCE_WIDTH);
    }

    #[test]
    fn b_alone_advances_a_prompt_clear_exactly_like_a_alone() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            ENCODED_A,
            super::super::CHAR_PROMPT_CLEAR,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Fast, (0, 1));

        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
        assert_eq!(printer.tick(PrinterInput::none()), TickEvent::AwaitingClear);
        let b_only = PrinterInput {
            a_pressed: false,
            b_pressed: true,
            a_held: false,
            b_held: false,
        };
        assert_eq!(printer.tick(b_only), TickEvent::Cleared);
        assert_eq!(printer.cursor(), (0, 1));
        let TickEvent::Glyph(g) = printer.tick(PrinterInput::none()) else {
            panic!("expected printing to resume on the new page")
        };
        assert_eq!((g.x, g.y), (0, 1));
    }

    #[test]
    fn pause_until_press_requires_a_new_press_and_preserves_the_cursor() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            ENCODED_A,
            super::super::EXT_CTRL_CODE_BEGIN,
            super::super::EXT_CTRL_CODE_PAUSE_UNTIL_PRESS,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (0, 1));

        assert!(
            matches!(printer.tick(PrinterInput::none()), TickEvent::Glyph(_)),
            "the first glyph reveals before the wait"
        );
        let cursor_at_wait = printer.cursor();

        assert_eq!(printer.tick(PrinterInput::none()), TickEvent::AwaitingPress);
        let held_a = PrinterInput {
            a_pressed: false,
            b_pressed: false,
            a_held: true,
            b_held: false,
        };
        assert_eq!(
            printer.tick(held_a),
            TickEvent::AwaitingPress,
            "a held button with no fresh edge must not release the wait"
        );
        assert_eq!(
            printer.tick(PrinterInput::none()),
            TickEvent::AwaitingPress,
            "releasing the held button must not release the wait either"
        );
        assert_eq!(
            printer.cursor(),
            cursor_at_wait,
            "waiting must not move the cursor"
        );

        assert_eq!(printer.tick(press_a()), TickEvent::PressAccepted);
        assert_eq!(
            printer.cursor(),
            cursor_at_wait,
            "accepting the press must not move the cursor"
        );

        let TickEvent::Glyph(second) = printer.tick(PrinterInput::none()) else {
            panic!("expected the next glyph the tick after the press was accepted")
        };
        assert_eq!(
            (second.x, second.y),
            (i32::from(NORMAL_A_ADVANCE_WIDTH), 1),
            "advanced past the first glyph"
        );
    }

    #[test]
    fn mid_speed_pause_until_press_preserves_the_pending_reveal_delay() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            ENCODED_A,
            super::super::EXT_CTRL_CODE_BEGIN,
            super::super::EXT_CTRL_CODE_PAUSE_UNTIL_PRESS,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Mid, (0, 1));

        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
        for _ in 0..3 {
            assert_eq!(printer.tick(PrinterInput::none()), TickEvent::Idle);
        }
        assert_eq!(printer.tick(PrinterInput::none()), TickEvent::AwaitingPress);
        assert_eq!(printer.tick(press_a()), TickEvent::PressAccepted);

        // Unlike timed PAUSE, this control does not reset the reveal delay.
        for _ in 0..3 {
            assert_eq!(printer.tick(PrinterInput::none()), TickEvent::Idle);
        }
        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
    }

    #[test]
    fn pause_control_code_blocks_for_exactly_the_argument_frame_count() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            ENCODED_A,
            super::super::EXT_CTRL_CODE_BEGIN,
            super::super::EXT_CTRL_CODE_PAUSE,
            PAUSE_DURATION,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (0, 1));

        assert!(
            matches!(printer.tick(PrinterInput::none()), TickEvent::Glyph(_)),
            "the first glyph reveals before the pause"
        );

        let mut paused_ticks = 0;
        loop {
            match printer.tick(PrinterInput::none()) {
                TickEvent::Paused => paused_ticks += 1,
                TickEvent::PauseFinished => break,
                other => panic!("expected Paused or PauseFinished, got {other:?}"),
            }
        }
        assert_eq!(
            paused_ticks,
            usize::from(PAUSE_DURATION),
            "pause duration must include the control code's frame"
        );

        let TickEvent::Glyph(second) = printer.tick(PrinterInput::none()) else {
            panic!("expected the second glyph the tick after PauseFinished")
        };
        assert_eq!(
            (second.x, second.y),
            (i32::from(NORMAL_A_ADVANCE_WIDTH), 1),
            "advanced past the first glyph"
        );
    }

    #[test]
    fn mid_speed_pause_resumes_without_an_extra_reveal_delay() {
        const TWO_FRAME_PAUSE: u8 = 2;

        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[
            ENCODED_A,
            super::super::EXT_CTRL_CODE_BEGIN,
            super::super::EXT_CTRL_CODE_PAUSE,
            TWO_FRAME_PAUSE,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Mid, (0, 1));

        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
        for _ in 0..3 {
            assert_eq!(printer.tick(PrinterInput::none()), TickEvent::Idle);
        }
        for _ in 0..TWO_FRAME_PAUSE {
            assert_eq!(printer.tick(PrinterInput::none()), TickEvent::Paused);
        }
        assert_eq!(printer.tick(PrinterInput::none()), TickEvent::PauseFinished);
        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
    }

    #[test]
    fn held_ab_speed_up_only_affects_printers_that_opt_in() {
        let pixels = blank_sheet_pixels();

        let opted_out_sheet = synthetic_sheet(&pixels, FontId::Normal);
        let opted_out_tokens = decode_tokens(&[ENCODED_A, ENCODED_A, ENCODED_A, super::super::EOS]);
        let mut opted_out_printer =
            Printer::new(opted_out_tokens, opted_out_sheet, TextSpeed::Mid, (0, 0));

        let held_a = PrinterInput {
            a_pressed: true,
            b_pressed: false,
            a_held: true,
            b_held: false,
        };
        assert!(
            matches!(opted_out_printer.tick(held_a), TickEvent::Glyph(_)),
            "the first glyph reveals on frame 0 regardless"
        );
        for frame in 1..=3 {
            assert_eq!(
                opted_out_printer.tick(held_a),
                TickEvent::Idle,
                "an opted-out printer's frame {frame} must stay un-accelerated"
            );
        }
        assert!(
            matches!(opted_out_printer.tick(held_a), TickEvent::Glyph(_)),
            "the second glyph still waits out the full MID delay"
        );

        let intro_sheet = synthetic_sheet(&pixels, FontId::Normal);
        let intro_tokens = decode_tokens(&[ENCODED_A, ENCODED_A, ENCODED_A, super::super::EOS]);
        let mut intro_printer = Printer::new(intro_tokens, intro_sheet, TextSpeed::Mid, (0, 0))
            .with_ab_speed_up_print();

        assert!(
            matches!(
                intro_printer.tick(PrinterInput::none()),
                TickEvent::Glyph(_)
            ),
            "the first glyph still reveals on frame 0"
        );
        assert_eq!(intro_printer.tick(held_a), TickEvent::Idle);
        assert!(
            matches!(
                intro_printer.tick(PrinterInput::none()),
                TickEvent::Glyph(_)
            ),
            "the second glyph reveals early once the speed-up latches"
        );
        assert!(
            matches!(intro_printer.tick(held_a), TickEvent::Glyph(_)),
            "the third glyph reveals immediately while A remains held"
        );
    }

    #[test]
    fn restart_clears_a_stuck_pause_state() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);

        let tokens = decode_tokens(&[
            super::super::EXT_CTRL_CODE_BEGIN,
            super::super::EXT_CTRL_CODE_PAUSE,
            PAUSE_DURATION,
            ENCODED_A,
            super::super::EOS,
        ]);
        let mut printer = Printer::new(tokens, sheet, TextSpeed::Instant, (0, 0));

        for tick in 1..=5 {
            assert_eq!(
                printer.tick(PrinterInput::none()),
                TickEvent::Paused,
                "tick {tick} should still be mid-pause"
            );
        }

        printer.restart(decode_tokens(&[ENCODED_A, super::super::EOS]));
        assert!(!printer.is_finished());
        assert_eq!(printer.cursor(), (0, 0));

        assert!(
            matches!(printer.tick(PrinterInput::none()), TickEvent::Glyph(_)),
            "restart must not leave the printer stuck mid-pause"
        );
    }

    #[test]
    fn restart_clears_the_latched_speed_up_flag() {
        let pixels = blank_sheet_pixels();
        let sheet = synthetic_sheet(&pixels, FontId::Normal);
        let tokens = decode_tokens(&[ENCODED_A, ENCODED_A, super::super::EOS]);
        let mut printer =
            Printer::new(tokens, sheet, TextSpeed::Mid, (0, 0)).with_ab_speed_up_print();

        let press_and_hold_a = PrinterInput {
            a_pressed: true,
            b_pressed: false,
            a_held: true,
            b_held: false,
        };
        let hold_a = PrinterInput {
            a_pressed: false,
            b_pressed: false,
            a_held: true,
            b_held: false,
        };

        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));
        assert_eq!(printer.tick(press_and_hold_a), TickEvent::Idle);
        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));

        printer.restart(decode_tokens(&[ENCODED_A, ENCODED_A, super::super::EOS]));
        assert!(matches!(
            printer.tick(PrinterInput::none()),
            TickEvent::Glyph(_)
        ));

        for frame in 1..=3 {
            assert_eq!(
                printer.tick(hold_a),
                TickEvent::Idle,
                "restart frame {frame}: the speed-up latch must not survive"
            );
        }
        assert!(matches!(printer.tick(hold_a), TickEvent::Glyph(_)));
    }

    #[test]
    fn text_speed_frame_tables_match_upstream() {
        assert_eq!(TextSpeed::Slow.frames_per_char(), 8);
        assert_eq!(TextSpeed::Mid.frames_per_char(), 4);
        assert_eq!(TextSpeed::Fast.frames_per_char(), 1);
        assert_eq!(TextSpeed::Instant.frames_per_char(), 0);
        assert_eq!(TextSpeed::Slow.scroll_px_per_frame(16), 1);
        assert_eq!(TextSpeed::Mid.scroll_px_per_frame(16), 2);
        assert_eq!(TextSpeed::Fast.scroll_px_per_frame(16), 4);
    }

    /// `GetPlayerTextSpeedDelay` (`pokeemerald/src/menu.c:481-487`):
    /// `OPTIONS_TEXT_SPEED_SLOW`/`_FAST` (`0`/`2`) select directly, `_MID`
    /// (`1`) is already the fallback target, and every out-of-range 3-bit
    /// value (`3..=7`, and `u8::MAX` as a belt-and-suspenders check) clamps
    /// to `MID` exactly as upstream's own `optionsTextSpeed > FAST` guard
    /// does.
    #[test]
    fn text_speed_from_raw_option_matches_upstreams_clamp() {
        assert_eq!(TextSpeed::from_raw_option(0), TextSpeed::Slow);
        assert_eq!(TextSpeed::from_raw_option(1), TextSpeed::Mid);
        assert_eq!(TextSpeed::from_raw_option(2), TextSpeed::Fast);
        for raw in [3, 4, 5, 6, 7, u8::MAX] {
            assert_eq!(
                TextSpeed::from_raw_option(raw),
                TextSpeed::Mid,
                "raw value {raw} is out of OPTIONS_TEXT_SPEED's 0..=2 range and must clamp to Mid"
            );
        }
    }
}
