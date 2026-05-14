// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Drive-letter newtype — Phase 4 sub-phase 5b.
//!
//! A [`DriveLetter`] is a validated Windows drive letter: an uppercase
//! ASCII byte in `A..=Z`.  The [`DriveLetter::parse`] constructor
//! canonicalises lowercase ASCII to uppercase so both `'c'` and `'C'`
//! produce [`DriveLetter::C`].
//!
//! # Why a newtype?
//!
//! Pre-5b, every drive-letter parameter in the workspace was a raw
//! `char` — 117 function signatures across 50+ files all took `char`
//! and immediately called `to_ascii_uppercase()` or formatted into a
//! path.  An invalid `'9'` or `'é'` getting through was the failure
//! mode that the type system did nothing about.
//!
//! With `DriveLetter`:
//!
//! - Every drive-letter parameter is statically validated at the construction
//!   site (CLI argument parsing, OS API result wrapping, test fixture).
//! - No `.to_ascii_uppercase()` ceremony at every consumer — `parse` already
//!   canonicalised.
//! - Constants ([`DriveLetter::A`] … [`DriveLetter::Z`]) provide a
//!   compile-time-validated form for code-internal use (tests, sentinel
//!   values).
//!
//! # Layer
//!
//! `DriveLetter` lives in `uffs-mft::platform` because `uffs-mft` is
//! the producer crate (the Windows API wrappers that returned `char`
//! before this newtype existed all live here).  Every downstream crate
//! (`uffs-core`, `uffs-daemon`, `uffs-cli`, `uffs-broker`) already
//! depends on `uffs-mft` transitively, so no new dependency edge is
//! introduced.

use core::fmt;

/// A validated Windows drive letter — uppercase ASCII `A..=Z`.
///
/// `DriveLetter` is `#[repr(transparent)]` over `u8` so its in-memory
/// layout is identical to the underlying ASCII byte; no conversion
/// cost is paid at FFI / NTFS-parse boundaries.
///
/// # Invariant
///
/// The wrapped byte is ALWAYS in `b'A'..=b'Z'`.  This is upheld by:
///
/// - [`DriveLetter::parse`] — accepts both ASCII case forms and canonicalises
///   to uppercase.
/// - The 26 [`DriveLetter::A`] … [`DriveLetter::Z`] constants — compile-time
///   validated.
///
/// There is no `pub fn new(u8)` because every external entry point
/// must either parse (for unvalidated input) or use a constant (for
/// code-internal use).  The crate-internal `from_byte_unchecked`
/// helper is `pub(crate)` and used only inside Windows API wrappers
/// where the byte was just emitted by `b'A' + i` from a validated `i`
/// in `0..26`.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct DriveLetter(u8);

/// Error returned by [`DriveLetter::parse`] when the input is not an
/// ASCII letter.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
#[non_exhaustive]
pub struct DriveLetterError {
    /// The original character that failed to parse.
    pub raw: char,
}

impl fmt::Display for DriveLetterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "drive letter must be ASCII A..=Z (case insensitive); got '{}'",
            self.raw.escape_default()
        )
    }
}

impl core::error::Error for DriveLetterError {}

impl DriveLetter {
    /// Parse a Windows drive letter, accepting both ASCII case forms.
    ///
    /// Lowercase input is canonicalised to uppercase, so
    /// `DriveLetter::parse('c')` and `DriveLetter::parse('C')` produce
    /// the same value.
    ///
    /// # Errors
    ///
    /// Returns [`DriveLetterError`] if `letter` is not ASCII A..=Z
    /// (a..=z).
    ///
    /// # Examples
    ///
    /// ```
    /// # use uffs_mft::platform::DriveLetter;
    /// assert_eq!(DriveLetter::parse('c').unwrap(), DriveLetter::C);
    /// assert_eq!(DriveLetter::parse('C').unwrap(), DriveLetter::C);
    /// assert!(DriveLetter::parse('1').is_err());
    /// assert!(DriveLetter::parse('é').is_err());
    /// ```
    pub const fn parse(letter: char) -> Result<Self, DriveLetterError> {
        let upper = letter.to_ascii_uppercase();
        if upper.is_ascii_uppercase() {
            // `upper` is provably in `'A'..='Z'`, so its 32-bit code
            // point is in `0x41..=0x5A` and the `as u8` cast is exact.
            // Clippy narrows `cast_possible_truncation` here using the
            // `is_ascii_uppercase` guard, which is why we don't carry
            // an `#[expect(clippy::cast_possible_truncation)]` attribute.
            Ok(Self(upper as u8))
        } else {
            Err(DriveLetterError { raw: letter })
        }
    }

    /// Wrap a byte that the caller has already verified to be in
    /// `b'A'..=b'Z'`.  Crate-internal only — public surfaces must use
    /// [`DriveLetter::parse`] so the invariant check is in the type
    /// system, not in caller discipline.
    ///
    /// Production callers all live behind `#[cfg(windows)]`
    /// (`platform::system::detect_ntfs_drives`), so the helper is
    /// itself `cfg(any(windows, test))` — the `test` arm keeps the
    /// invariant-pinning unit test compiled on every target.
    #[cfg(any(windows, test))]
    #[inline]
    #[must_use]
    pub(crate) const fn from_byte_unchecked(byte: u8) -> Self {
        debug_assert!(byte.is_ascii_uppercase(), "DriveLetter invariant: A..=Z");
        Self(byte)
    }

    /// Returns the uppercase ASCII char (e.g. `'C'`).
    ///
    /// Used at FFI / Display / path-format boundaries where the API
    /// demands `char`.  Pure-Rust code paths inside the workspace
    /// should prefer methods on `DriveLetter` over `.as_char()` +
    /// free-function dispatch.
    #[inline]
    #[must_use]
    pub const fn as_char(self) -> char {
        self.0 as char
    }

    /// Returns the uppercase ASCII byte (e.g. `b'C'`).
    ///
    /// Used when building a `[u8; N]` path prefix or volume root for
    /// a Win32 API call.
    #[inline]
    #[must_use]
    pub const fn as_byte(self) -> u8 {
        self.0
    }

    /// Returns the zero-based alphabet index (A=0, B=1, …, Z=25).
    ///
    /// Used to index a 26-entry `Vec<T>` of per-drive state without a
    /// `HashMap` allocation.
    #[inline]
    #[must_use]
    pub const fn alphabet_index(self) -> usize {
        (self.0 - b'A') as usize
    }

    /// Case-insensitive ASCII comparison against a raw `char`.
    ///
    /// `DriveLetter` is uppercase-canonical by construction, so this is
    /// equivalent to `self.as_char() == other.to_ascii_uppercase()`
    /// when `other` is an ASCII letter, and always `false` otherwise.
    ///
    /// Kept as an inherent method so legacy `char`-flavoured call
    /// sites (CLI filters, JSON wire formats) can compare without an
    /// intermediate `DriveLetter::parse` allocation/branch.
    #[inline]
    #[must_use]
    pub fn eq_ignore_ascii_case(self, other: char) -> bool {
        // Non-ASCII inputs are by definition not an ASCII letter and
        // so can never match a `DriveLetter` (A..=Z by construction).
        // `u8::try_from` returns `Err` for any code point above 0xFF,
        // and the explicit `is_ascii()` guard rejects 0x80..=0xFF.
        if !other.is_ascii() {
            return false;
        }
        let Ok(byte) = u8::try_from(u32::from(other)) else {
            return false;
        };
        // Clear the ASCII case bit (0x20) to fold `'a'..='z'` to
        // their uppercase counterparts; uppercase letters and
        // non-letter ASCII bytes are left alone.  The trailing
        // `is_ascii_uppercase` guard rejects everything that didn't
        // end up in `b'A'..=b'Z'` (digits, symbols, control bytes).
        let folded = byte & !0x20;
        self.0 == folded && folded.is_ascii_uppercase()
    }
}

impl fmt::Debug for DriveLetter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Matches the existing `tracing` log convention (`letter='C'`
        // / `drive='C'`) used across the daemon and cache modules.
        write!(f, "DriveLetter({})", self.0 as char)
    }
}

impl fmt::Display for DriveLetter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&(self.0 as char), f)
    }
}

impl TryFrom<char> for DriveLetter {
    type Error = DriveLetterError;

    /// Delegates to [`DriveLetter::parse`].  Provided so generic code
    /// (e.g. `iter.map(TryInto::try_into)`) works without referring
    /// to the inherent method by name.
    #[inline]
    fn try_from(letter: char) -> Result<Self, Self::Error> {
        Self::parse(letter)
    }
}

impl TryFrom<u8> for DriveLetter {
    type Error = DriveLetterError;

    /// Wire-format conversion: accepts an ASCII byte and validates it
    /// is in `b'A'..=b'Z'` (canonicalising `b'a'..=b'z'` to uppercase
    /// for symmetry with [`DriveLetter::parse`]).  Used by the
    /// shared-memory record format (`uffs_client::shmem::ShmemRecord::drive`)
    /// which stores the letter as a single byte for compact layout.
    ///
    /// # Errors
    ///
    /// Returns [`DriveLetterError`] when the byte is not an ASCII
    /// letter.  The `raw` field is set to the byte's char form for
    /// consistent error messages with [`DriveLetter::parse`].
    #[inline]
    fn try_from(byte: u8) -> Result<Self, Self::Error> {
        // ASCII bytes round-trip losslessly via `char::from(u8)`.
        Self::parse(char::from(byte))
    }
}

impl core::str::FromStr for DriveLetter {
    type Err = DriveLetterError;

    /// Parse a single-character string into a [`DriveLetter`].
    ///
    /// Provided so `clap`-derive structs can use `DriveLetter`
    /// directly as a typed field (clap dispatches to `FromStr`).
    /// Accepts both ASCII case forms; rejects empty strings,
    /// multi-character strings, and any input that
    /// [`DriveLetter::parse`] would reject.
    ///
    /// # Errors
    ///
    /// Returns [`DriveLetterError`] if the string is empty, longer
    /// than one character, or contains a character that is not
    /// ASCII A..=Z (a..=z).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut chars = s.chars();
        match (chars.next(), chars.next()) {
            (Some(ch), None) => Self::parse(ch),
            // Empty or multi-char: surface the offending payload as
            // the *first* character (or U+FFFD for the empty case)
            // so the error message points at the user's input rather
            // than a synthetic sentinel.
            (None, _) => Err(DriveLetterError { raw: '\u{FFFD}' }),
            (Some(ch), Some(_)) => Err(DriveLetterError { raw: ch }),
        }
    }
}

/// JSON / IPC wire format: a single-character ASCII uppercase string
/// (`"C"`).  Stays byte-compatible with the historical
/// `drive: char` serialisation that the daemon's event stream and the
/// MCP server already emit, so this migration is invisible on the
/// wire.
impl serde::Serialize for DriveLetter {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_char(self.as_char())
    }
}

impl<'de> serde::Deserialize<'de> for DriveLetter {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DriveLetterVisitor;

        impl serde::de::Visitor<'_> for DriveLetterVisitor {
            type Value = DriveLetter;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an ASCII letter A..=Z (case-insensitive) as a char or string")
            }

            fn visit_char<E: serde::de::Error>(self, v: char) -> Result<Self::Value, E> {
                DriveLetter::parse(v).map_err(serde::de::Error::custom)
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                v.parse::<DriveLetter>().map_err(serde::de::Error::custom)
            }
        }

        // `deserialize_char` accepts both `char` and `str` payloads
        // from JSON via the visitor above, so a remote
        // `"C"` (string) and `'C'` (char in MessagePack etc.) both
        // decode identically.
        deserializer.deserialize_char(DriveLetterVisitor)
    }
}

/// Internal helper macro: declares the 26 [`DriveLetter`] associated
/// constants (`A`…`Z`) from a list of `letter = byte` pairs.
///
/// Kept as a macro so the alphabet table stays in lock-step with the
/// `as_byte` representation, and so we get one `Self::X = Self(b'X')`
/// line per letter instead of 26 hand-written copies that could
/// silently drift apart.
macro_rules! drive_letter_consts {
    ($( $letter:ident = $byte:literal ),* $(,)?) => {
        impl DriveLetter {
            $(
                #[doc = concat!("Drive letter `", stringify!($letter), "`.")]
                pub const $letter: Self = Self($byte);
            )*
        }
    };
}

drive_letter_consts! {
    A = b'A', B = b'B', C = b'C', D = b'D', E = b'E', F = b'F', G = b'G',
    H = b'H', I = b'I', J = b'J', K = b'K', L = b'L', M = b'M', N = b'N',
    O = b'O', P = b'P', Q = b'Q', R = b'R', S = b'S', T = b'T', U = b'U',
    V = b'V', W = b'W', X = b'X', Y = b'Y', Z = b'Z',
}

#[cfg(test)]
mod tests {
    use super::{DriveLetter, DriveLetterError};

    #[test]
    fn parse_accepts_uppercase() {
        assert_eq!(DriveLetter::parse('C').unwrap(), DriveLetter::C);
        assert_eq!(DriveLetter::parse('A').unwrap(), DriveLetter::A);
        assert_eq!(DriveLetter::parse('Z').unwrap(), DriveLetter::Z);
    }

    #[test]
    fn parse_canonicalises_lowercase() {
        assert_eq!(DriveLetter::parse('c').unwrap(), DriveLetter::C);
        assert_eq!(DriveLetter::parse('a').unwrap(), DriveLetter::A);
        assert_eq!(DriveLetter::parse('z').unwrap(), DriveLetter::Z);
    }

    #[test]
    fn parse_rejects_digits() {
        assert_eq!(DriveLetter::parse('1').unwrap_err(), DriveLetterError {
            raw: '1'
        });
        DriveLetter::parse('0').unwrap_err();
        DriveLetter::parse('9').unwrap_err();
    }

    #[test]
    fn parse_rejects_non_ascii() {
        DriveLetter::parse('é').unwrap_err();
        DriveLetter::parse('☃').unwrap_err();
        DriveLetter::parse('\u{0}').unwrap_err();
    }

    #[test]
    fn parse_rejects_punctuation() {
        DriveLetter::parse(':').unwrap_err();
        DriveLetter::parse('/').unwrap_err();
        DriveLetter::parse(' ').unwrap_err();
    }

    #[test]
    fn as_char_returns_uppercase() {
        assert_eq!(DriveLetter::C.as_char(), 'C');
        assert_eq!(DriveLetter::parse('c').unwrap().as_char(), 'C');
    }

    #[test]
    fn as_byte_returns_ascii_byte() {
        assert_eq!(DriveLetter::C.as_byte(), b'C');
        assert_eq!(DriveLetter::A.as_byte(), b'A');
        assert_eq!(DriveLetter::Z.as_byte(), b'Z');
    }

    #[test]
    fn alphabet_index_runs_a_to_z() {
        assert_eq!(DriveLetter::A.alphabet_index(), 0);
        assert_eq!(DriveLetter::B.alphabet_index(), 1);
        assert_eq!(DriveLetter::M.alphabet_index(), 12);
        assert_eq!(DriveLetter::Z.alphabet_index(), 25);
    }

    #[test]
    fn from_byte_unchecked_round_trips_uppercase_range() {
        // The Windows drive-bitmask path in `platform::system` is the
        // sole production caller, but the invariant (`b'A'..=b'Z'` ↦
        // canonical `DriveLetter`) is platform-agnostic.  Pinning it
        // here means the helper stays linked on every target and a
        // future refactor can't silently change the round-trip shape.
        for offset in 0_u8..26 {
            let byte = b'A' + offset;
            let dl = DriveLetter::from_byte_unchecked(byte);
            assert_eq!(dl.as_byte(), byte);
            assert_eq!(dl.as_char(), char::from(byte));
            assert_eq!(usize::from(offset), dl.alphabet_index());
        }
    }

    #[test]
    fn try_from_delegates_to_parse() {
        let from_try: DriveLetter = 'c'.try_into().unwrap();
        let from_parse: DriveLetter = DriveLetter::parse('c').unwrap();
        assert_eq!(from_try, from_parse);
    }

    #[test]
    fn display_is_uppercase_letter() {
        let buf = format!("{}", DriveLetter::C);
        assert_eq!(buf, "C");
    }

    #[test]
    fn debug_includes_letter() {
        let buf = format!("{:?}", DriveLetter::C);
        assert_eq!(buf, "DriveLetter(C)");
    }

    #[test]
    fn ordering_matches_alphabet() {
        assert!(DriveLetter::A < DriveLetter::B);
        assert!(DriveLetter::C < DriveLetter::Z);
        let mut sorted = [DriveLetter::Z, DriveLetter::A, DriveLetter::M];
        sorted.sort_unstable();
        assert_eq!(sorted, [DriveLetter::A, DriveLetter::M, DriveLetter::Z]);
    }

    #[test]
    fn repr_transparent_size_alignment() {
        // `#[repr(transparent)]` over `u8` guarantees identical layout.
        assert_eq!(size_of::<DriveLetter>(), size_of::<u8>());
        assert_eq!(align_of::<DriveLetter>(), align_of::<u8>());
    }

    #[test]
    fn error_display_includes_input_char() {
        let err = DriveLetter::parse('1').unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("'1'"), "got: {msg}");
        assert!(msg.contains("A..=Z"), "got: {msg}");
    }

    #[test]
    fn error_implements_std_error() {
        // Compile-time check: `core::error::Error` requires Debug + Display.
        fn assert_error<E: core::error::Error>() {}
        assert_error::<DriveLetterError>();
    }

    #[test]
    fn parse_is_const_fn() {
        // `DriveLetter::parse` is `const fn`, so this compiles at
        // compile time — pin the const-evaluability invariant.
        const PARSED: Result<DriveLetter, DriveLetterError> = DriveLetter::parse('c');
        assert_eq!(PARSED.unwrap(), DriveLetter::C);
    }

    #[test]
    fn from_str_accepts_single_letter() {
        use core::str::FromStr as _;
        assert_eq!(DriveLetter::from_str("c").unwrap(), DriveLetter::C);
        assert_eq!(DriveLetter::from_str("Z").unwrap(), DriveLetter::Z);
        assert_eq!("A".parse::<DriveLetter>().unwrap(), DriveLetter::A);
    }

    #[test]
    fn from_str_rejects_empty_and_multichar() {
        use core::str::FromStr as _;
        DriveLetter::from_str("").unwrap_err();
        DriveLetter::from_str("CD").unwrap_err();
        DriveLetter::from_str("C:").unwrap_err();
        // Multi-char surfaces the FIRST character in the error so users
        // see what they typed.
        let err = DriveLetter::from_str("XY").unwrap_err();
        assert_eq!(err.raw, 'X');
    }

    #[test]
    fn from_str_rejects_non_letter_single_char() {
        use core::str::FromStr as _;
        DriveLetter::from_str("1").unwrap_err();
        DriveLetter::from_str(":").unwrap_err();
        DriveLetter::from_str("é").unwrap_err();
    }
}
