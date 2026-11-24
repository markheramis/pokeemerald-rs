//! The Game Corner slot machine's three reel strips and the positions each
//! reel starts a game from.
//!
//! Source: `src/slot_machine.c` (`sReelSymbols`, `sInitialReelPositions`, and
//! the `SYMBOL_*` and reel enums; the strip length is `SYMBOLS_PER_REEL`).
//!
//! Reels are ordered left, middle, right. A strip is read from position `0`
//! in upstream order, and a starting position is an index into that strip.
//! Every lookup refuses a reel, symbol or position that is out of range
//! instead of clamping it. The Reel Time number strip is a separate table
//! (see [`crate::slot_machine_reel_time`]). The odds, the stop logic, the
//! payouts and the coin changes are not ported here.

use std::error::Error;
use std::fmt;

/// The number of reels (`NUM_REELS`).
pub const REEL_COUNT: usize = 3;

/// The number of symbols on each reel strip (`SYMBOLS_PER_REEL`).
pub const SYMBOLS_PER_REEL: usize = 21;

/// The number of distinct reel symbols.
pub const SYMBOL_COUNT: usize = 7;

/// A failed lookup in the slot-machine reel data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotMachineError {
    /// The reel is not below [`REEL_COUNT`].
    UnknownReel(usize),
    /// The symbol is not below [`SYMBOL_COUNT`].
    UnknownSymbol(u8),
    /// The position is not below [`SYMBOLS_PER_REEL`].
    InvalidPosition(usize),
}

impl fmt::Display for SlotMachineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownReel(reel) => {
                write!(
                    f,
                    "unknown slot-machine reel `{reel}` (there are {REEL_COUNT})"
                )
            }
            Self::UnknownSymbol(symbol) => {
                write!(
                    f,
                    "unknown slot-machine symbol `{symbol}` (there are {SYMBOL_COUNT})"
                )
            }
            Self::InvalidPosition(position) => write!(
                f,
                "slot-machine reel position `{position}` is out of range (a strip holds \
                 {SYMBOLS_PER_REEL})"
            ),
        }
    }
}

impl Error for SlotMachineError {}

/// One of the three reels, in upstream order (`LEFT_REEL`, `MIDDLE_REEL`,
/// `RIGHT_REEL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reel {
    Left,
    Middle,
    Right,
}

impl Reel {
    /// Every reel, left to right.
    pub const ALL: [Self; REEL_COUNT] = [Self::Left, Self::Middle, Self::Right];

    /// Validates a raw reel index.
    ///
    /// # Errors
    ///
    /// Returns [`SlotMachineError::UnknownReel`] when `index` is not below
    /// [`REEL_COUNT`].
    pub const fn new(index: usize) -> Result<Self, SlotMachineError> {
        match index {
            0 => Ok(Self::Left),
            1 => Ok(Self::Middle),
            2 => Ok(Self::Right),
            _ => Err(SlotMachineError::UnknownReel(index)),
        }
    }

    /// Returns the raw upstream index.
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// A reel symbol. The discriminants are the upstream `SYMBOL_*` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum ReelSymbol {
    RedSeven,
    BlueSeven,
    Azurill,
    Lotad,
    Cherry,
    Power,
    Replay,
}

impl ReelSymbol {
    /// Every symbol, in upstream order.
    pub const ALL: [Self; SYMBOL_COUNT] = [
        Self::RedSeven,
        Self::BlueSeven,
        Self::Azurill,
        Self::Lotad,
        Self::Cherry,
        Self::Power,
        Self::Replay,
    ];

    /// Validates a raw `SYMBOL_*` value.
    ///
    /// # Errors
    ///
    /// Returns [`SlotMachineError::UnknownSymbol`] when `value` is not below
    /// [`SYMBOL_COUNT`].
    pub const fn new(value: u8) -> Result<Self, SlotMachineError> {
        if (value as usize) < SYMBOL_COUNT {
            Ok(Self::ALL[value as usize])
        } else {
            Err(SlotMachineError::UnknownSymbol(value))
        }
    }

    /// Returns the raw upstream value.
    #[must_use]
    pub const fn index(self) -> u8 {
        self as u8
    }
}

/// Which of the two starting-position sets a game uses
/// (`sInitialReelPositions` columns 0 and 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StartKind {
    /// An ordinary game.
    Normal,
    /// A lucky game.
    Lucky,
}

/// `sReelSymbols`, indexed by reel then position.
#[rustfmt::skip]
const REEL_STRIPS: [[ReelSymbol; SYMBOLS_PER_REEL]; REEL_COUNT] = {
    use ReelSymbol::{Azurill, BlueSeven, Cherry, Lotad, Power, RedSeven, Replay};
    [
        [
            RedSeven, Cherry, Azurill, Replay, Power, Lotad, BlueSeven,
            Lotad, Cherry, Power, Replay, Azurill, RedSeven, Power, Lotad,
            Replay, Azurill, BlueSeven, Power, Lotad, Replay,
        ],
        [
            RedSeven, Cherry, Replay, Lotad, Azurill, Cherry, Replay,
            Power, Power, Lotad, BlueSeven, Lotad, Replay, Cherry, Azurill,
            Lotad, Replay, Cherry, Lotad, Replay, Cherry,
        ],
        [
            RedSeven, Power, BlueSeven, Replay, Lotad, Azurill, Replay,
            Lotad, Power, Azurill, Replay, Lotad, Azurill, Power, Replay,
            Lotad, Azurill, Power, Replay, Lotad, Cherry,
        ],
    ]
};

/// `sInitialReelPositions`, indexed by reel then [`StartKind`] (normal, lucky).
const INITIAL_POSITIONS: [[usize; 2]; REEL_COUNT] = [[0, 6], [0, 10], [0, 2]];

/// Returns the symbols of one reel, from position `0`, in upstream order.
#[must_use]
pub const fn reel_strip(reel: Reel) -> &'static [ReelSymbol; SYMBOLS_PER_REEL] {
    &REEL_STRIPS[reel.index()]
}

/// Returns the symbol at `position` on `reel`.
///
/// # Errors
///
/// Returns [`SlotMachineError::InvalidPosition`] when `position` is not below
/// [`SYMBOLS_PER_REEL`].
pub fn symbol_at(reel: Reel, position: usize) -> Result<ReelSymbol, SlotMachineError> {
    reel_strip(reel)
        .get(position)
        .copied()
        .ok_or(SlotMachineError::InvalidPosition(position))
}

/// Returns the strip position `reel` starts from in a game of `kind`.
#[must_use]
pub const fn initial_position(reel: Reel, kind: StartKind) -> usize {
    INITIAL_POSITIONS[reel.index()][kind as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The strips as raw `SYMBOL_*` values, read independently of
    /// `REEL_STRIPS` from the enum in `src/slot_machine.c`: `SYMBOL_7_RED` 0,
    /// `SYMBOL_7_BLUE` 1, `SYMBOL_AZURILL` 2, `SYMBOL_LOTAD` 3, `SYMBOL_CHERRY` 4,
    /// `SYMBOL_POWER` 5, `SYMBOL_REPLAY` 6.
    const EXPECTED_STRIPS: [[u8; SYMBOLS_PER_REEL]; REEL_COUNT] = [
        [
            0, 4, 2, 6, 5, 3, 1, 3, 4, 5, 6, 2, 0, 5, 3, 6, 2, 1, 5, 3, 6,
        ],
        [
            0, 4, 6, 3, 2, 4, 6, 5, 5, 3, 1, 3, 6, 4, 2, 3, 6, 4, 3, 6, 4,
        ],
        [
            0, 5, 1, 6, 3, 2, 6, 3, 5, 2, 6, 3, 2, 5, 6, 3, 2, 5, 6, 3, 4,
        ],
    ];

    /// Starting positions as (normal, lucky) per reel.
    const EXPECTED_STARTS: [(usize, usize); REEL_COUNT] = [(0, 6), (0, 10), (0, 2)];

    #[test]
    fn every_symbol_of_every_strip_is_pinned() {
        for reel in Reel::ALL {
            let raw = reel_strip(reel).map(ReelSymbol::index);
            assert_eq!(raw, EXPECTED_STRIPS[reel.index()], "{reel:?}");
            for (position, &value) in EXPECTED_STRIPS[reel.index()].iter().enumerate() {
                assert_eq!(
                    symbol_at(reel, position),
                    ReelSymbol::new(value),
                    "{reel:?} position {position}"
                );
            }
        }
        assert_eq!(REEL_COUNT * SYMBOLS_PER_REEL, 63);
    }

    #[test]
    fn every_starting_position_is_pinned() {
        for reel in Reel::ALL {
            let (normal, lucky) = EXPECTED_STARTS[reel.index()];
            assert_eq!(
                initial_position(reel, StartKind::Normal),
                normal,
                "{reel:?}"
            );
            assert_eq!(initial_position(reel, StartKind::Lucky), lucky, "{reel:?}");
        }
    }

    #[test]
    fn starting_positions_lie_on_their_strips() {
        for reel in Reel::ALL {
            for kind in [StartKind::Normal, StartKind::Lucky] {
                assert!(symbol_at(reel, initial_position(reel, kind)).is_ok());
            }
        }
    }

    #[test]
    fn a_lucky_game_starts_each_reel_on_a_named_symbol() {
        let lucky = |reel| symbol_at(reel, initial_position(reel, StartKind::Lucky));
        assert_eq!(lucky(Reel::Left), Ok(ReelSymbol::BlueSeven));
        assert_eq!(lucky(Reel::Middle), Ok(ReelSymbol::BlueSeven));
        assert_eq!(lucky(Reel::Right), Ok(ReelSymbol::BlueSeven));
    }

    #[test]
    fn symbol_values_round_trip() {
        for (value, symbol) in (0..).zip(ReelSymbol::ALL) {
            assert_eq!(symbol.index(), value);
            assert_eq!(ReelSymbol::new(value), Ok(symbol));
        }
    }

    #[test]
    fn reel_values_round_trip() {
        for (index, reel) in Reel::ALL.into_iter().enumerate() {
            assert_eq!(reel.index(), index);
            assert_eq!(Reel::new(index), Ok(reel));
        }
    }

    #[test]
    fn an_invalid_reel_symbol_or_position_is_refused() {
        assert_eq!(Reel::new(3), Err(SlotMachineError::UnknownReel(3)));
        assert_eq!(
            Reel::new(usize::MAX),
            Err(SlotMachineError::UnknownReel(usize::MAX))
        );
        assert_eq!(ReelSymbol::new(7), Err(SlotMachineError::UnknownSymbol(7)));
        assert_eq!(
            ReelSymbol::new(u8::MAX),
            Err(SlotMachineError::UnknownSymbol(u8::MAX))
        );
        for reel in Reel::ALL {
            assert_eq!(
                symbol_at(reel, SYMBOLS_PER_REEL),
                Err(SlotMachineError::InvalidPosition(SYMBOLS_PER_REEL))
            );
            assert!(symbol_at(reel, SYMBOLS_PER_REEL - 1).is_ok());
        }
    }

    #[test]
    fn errors_name_the_offending_value() {
        assert!(SlotMachineError::UnknownReel(5).to_string().contains('5'));
        assert!(SlotMachineError::UnknownSymbol(9).to_string().contains('9'));
        assert!(SlotMachineError::InvalidPosition(30)
            .to_string()
            .contains("30"));
    }

    /// Reads a file of the pinned upstream checkout.
    fn upstream_file(relative: &str) -> String {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("crate sits two levels below the workspace root")
            .join("pokeemerald");
        std::fs::read_to_string(root.join(relative)).unwrap_or_else(|error| {
            panic!("cannot read pokeemerald/{relative}: {error}; run ./init.sh")
        })
    }

    /// The text between `start` and the next `end` after it.
    fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        let from = source
            .find(start)
            .unwrap_or_else(|| panic!("`{start}` not found"))
            + start.len();
        let length = source[from..]
            .find(end)
            .unwrap_or_else(|| panic!("`{end}` not found after `{start}`"));
        &source[from..from + length]
    }

    #[test]
    #[ignore = "reads the pinned upstream checkout; run ./init.sh first"]
    fn upstream_source_matches_the_tables() {
        let source = upstream_file("src/slot_machine.c");

        let mut symbols = vec!["SYMBOL_7_RED"];
        symbols.extend(
            between(&source, "enum {\n    SYMBOL_7_RED,", "};")
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty()),
        );
        assert_eq!(symbols.len(), SYMBOL_COUNT);
        let symbol_value = |name: &str| -> u8 {
            let position = symbols
                .iter()
                .position(|candidate| *candidate == name)
                .unwrap_or_else(|| panic!("unknown symbol `{name}`"));
            u8::try_from(position).unwrap()
        };

        let strips = between(
            &source,
            "static const u8 sReelSymbols[NUM_REELS][SYMBOLS_PER_REEL] =\n{",
            "\n};",
        );
        let parsed: Vec<Vec<u8>> = strips
            .split("_REEL] = {")
            .skip(1)
            .map(|reel| {
                reel.split('}')
                    .next()
                    .unwrap()
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(symbol_value)
                    .collect()
            })
            .collect();
        assert_eq!(parsed.len(), REEL_COUNT);
        for reel in Reel::ALL {
            assert_eq!(
                parsed[reel.index()],
                reel_strip(reel).map(ReelSymbol::index),
                "{reel:?}"
            );
        }

        let starts = between(
            &source,
            "static const s16 sInitialReelPositions[NUM_REELS][2] = {",
            "};",
        );
        let parsed: Vec<Vec<usize>> = starts
            .lines()
            .filter_map(|line| {
                let pair = line.split('{').nth(1)?.split('}').next()?;
                Some(pair.split(',').map(|n| n.trim().parse().unwrap()).collect())
            })
            .collect();
        assert_eq!(parsed.len(), REEL_COUNT);
        for reel in Reel::ALL {
            assert_eq!(
                parsed[reel.index()],
                [
                    initial_position(reel, StartKind::Normal),
                    initial_position(reel, StartKind::Lucky)
                ],
                "{reel:?}"
            );
        }
    }
}
