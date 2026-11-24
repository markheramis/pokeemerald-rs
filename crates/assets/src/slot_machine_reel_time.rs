//! The Game Corner slot machine's Reel Time number strip.
//!
//! Source: `src/slot_machine.c` (`sReelTimeSymbols` and `REELTIME_SYMBOLS`).
//! `GetReelTimeSymbol` reads one position of this strip.
//!
//! Positions are indexed from `0` in upstream order. A lookup refuses a
//! position that is out of range instead of wrapping it. A Reel Time number
//! is the digit stored in the strip, not one of the seven ordinary reel
//! symbols. Spinning, position selection, odds, animation and awards are not
//! ported here.

use std::error::Error;
use std::fmt;

/// The number of positions on the Reel Time strip (`REELTIME_SYMBOLS`).
pub const REEL_TIME_LENGTH: usize = 6;

/// A failed lookup in the Reel Time number strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReelTimeError {
    /// The number is not one of the six digits the strip stores.
    UnknownNumber(u8),
    /// The position is not below [`REEL_TIME_LENGTH`].
    InvalidPosition(usize),
}

impl fmt::Display for ReelTimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownNumber(number) => write!(
                f,
                "unknown Reel Time number `{number}` (the strip stores 0 through 5)"
            ),
            Self::InvalidPosition(position) => write!(
                f,
                "Reel Time position `{position}` is out of range (the strip holds \
                 {REEL_TIME_LENGTH})"
            ),
        }
    }
}

impl Error for ReelTimeError {}

/// One digit of `sReelTimeSymbols`. The discriminant is the stored value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum ReelTimeNumber {
    /// Digit `0`.
    Zero = 0,
    /// Digit `1`.
    One = 1,
    /// Digit `2`.
    Two = 2,
    /// Digit `3`.
    Three = 3,
    /// Digit `4`.
    Four = 4,
    /// Digit `5`.
    Five = 5,
}

impl ReelTimeNumber {
    /// Every digit the strip can store, from `0` through `5`.
    pub const ALL: [Self; REEL_TIME_LENGTH] = [
        Self::Zero,
        Self::One,
        Self::Two,
        Self::Three,
        Self::Four,
        Self::Five,
    ];

    /// Validates a raw strip value.
    ///
    /// # Errors
    ///
    /// Returns [`ReelTimeError::UnknownNumber`] when `value` is not one of
    /// the six digits.
    pub const fn new(value: u8) -> Result<Self, ReelTimeError> {
        match value {
            0 => Ok(Self::Zero),
            1 => Ok(Self::One),
            2 => Ok(Self::Two),
            3 => Ok(Self::Three),
            4 => Ok(Self::Four),
            5 => Ok(Self::Five),
            _ => Err(ReelTimeError::UnknownNumber(value)),
        }
    }

    /// Returns the raw stored digit.
    #[must_use]
    pub const fn value(self) -> u8 {
        self as u8
    }
}

/// `sReelTimeSymbols`, in upstream order.
const REEL_TIME_STRIP: [ReelTimeNumber; REEL_TIME_LENGTH] = [
    ReelTimeNumber::One,
    ReelTimeNumber::Zero,
    ReelTimeNumber::Five,
    ReelTimeNumber::Four,
    ReelTimeNumber::Three,
    ReelTimeNumber::Two,
];

/// Returns the Reel Time strip, from position `0`, in upstream order.
#[must_use]
pub const fn reel_time_strip() -> &'static [ReelTimeNumber; REEL_TIME_LENGTH] {
    &REEL_TIME_STRIP
}

/// Returns the digit at `position` on the Reel Time strip.
///
/// # Errors
///
/// Returns [`ReelTimeError::InvalidPosition`] when `position` is not below
/// [`REEL_TIME_LENGTH`].
pub fn reel_time_number_at(position: usize) -> Result<ReelTimeNumber, ReelTimeError> {
    reel_time_strip()
        .get(position)
        .copied()
        .ok_or(ReelTimeError::InvalidPosition(position))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The strip as raw stored digits, read independently of
    /// `REEL_TIME_STRIP` from `sReelTimeSymbols` in `src/slot_machine.c`.
    const EXPECTED: [u8; REEL_TIME_LENGTH] = [1, 0, 5, 4, 3, 2];

    #[test]
    fn every_position_is_pinned_in_upstream_order() {
        let raw = reel_time_strip().map(ReelTimeNumber::value);
        assert_eq!(raw, EXPECTED);
        for (position, &value) in EXPECTED.iter().enumerate() {
            assert_eq!(
                reel_time_number_at(position),
                ReelTimeNumber::new(value),
                "position {position}"
            );
        }
        assert_eq!(REEL_TIME_LENGTH, 6);
    }

    #[test]
    fn the_strip_is_not_sorted_or_compacted() {
        assert_ne!(EXPECTED, [0, 1, 2, 3, 4, 5]);
        assert_eq!(EXPECTED.len(), REEL_TIME_LENGTH);
    }

    #[test]
    fn number_values_round_trip() {
        for (value, number) in (0..).zip(ReelTimeNumber::ALL) {
            assert_eq!(number.value(), value);
            assert_eq!(ReelTimeNumber::new(value), Ok(number));
        }
    }

    #[test]
    fn an_out_of_range_position_or_number_is_refused() {
        assert_eq!(
            reel_time_number_at(REEL_TIME_LENGTH),
            Err(ReelTimeError::InvalidPosition(REEL_TIME_LENGTH))
        );
        assert_eq!(
            reel_time_number_at(usize::MAX),
            Err(ReelTimeError::InvalidPosition(usize::MAX))
        );
        assert!(reel_time_number_at(REEL_TIME_LENGTH - 1).is_ok());
        assert_eq!(ReelTimeNumber::new(6), Err(ReelTimeError::UnknownNumber(6)));
        assert_eq!(
            ReelTimeNumber::new(u8::MAX),
            Err(ReelTimeError::UnknownNumber(u8::MAX))
        );
    }

    #[test]
    fn errors_name_the_offending_value() {
        assert!(ReelTimeError::UnknownNumber(9).to_string().contains('9'));
        assert!(ReelTimeError::InvalidPosition(8).to_string().contains('8'));
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

    #[test]
    #[ignore = "reads the pinned upstream checkout; run ./init.sh first"]
    fn upstream_source_matches_the_strip() {
        let source = upstream_file("src/slot_machine.c");
        let start = "static const u8 sReelTimeSymbols[] = {";
        let from = source
            .find(start)
            .unwrap_or_else(|| panic!("`{start}` not found"))
            + start.len();
        let body = source[from..]
            .split('}')
            .next()
            .unwrap_or_else(|| panic!("sReelTimeSymbols initializer does not close"));
        let parsed: Vec<u8> = body
            .split(',')
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(|token| {
                token
                    .parse()
                    .unwrap_or_else(|_| panic!("non-numeric Reel Time entry `{token}`"))
            })
            .collect();
        assert_eq!(parsed, reel_time_strip().map(ReelTimeNumber::value));
    }
}
