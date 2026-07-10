//! Positioned parse errors and validation warnings.

use std::fmt;

/// A parse/validation failure, positioned at a byte offset into the input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// Byte offset into the source expression where the problem was detected.
    pub pos: usize,
    /// Human-readable description.
    pub message: String,
}

impl ParseError {
    pub fn new(pos: usize, message: impl Into<String>) -> ParseError {
        ParseError {
            pos,
            message: message.into(),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "parse error at {}: {}", self.pos, self.message)
    }
}

impl std::error::Error for ParseError {}

/// A non-fatal diagnostic: the expression parsed, but is (e.g.) statically
/// unsatisfiable. Carries the byte offset of the offending component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    pub pos: usize,
    pub message: String,
}

impl Warning {
    pub fn new(pos: usize, message: impl Into<String>) -> Warning {
        Warning {
            pos,
            message: message.into(),
        }
    }
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "warning at {}: {}", self.pos, self.message)
    }
}

/// The one runtime failure: an IANA time-zone identifier that does not
/// resolve to a usable zone — unknown, invalid, or an unreadable TZif entry.
/// Parse errors aside, nothing else in the crate fails.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownTimeZone {
    /// The identifier that failed to resolve.
    pub id: String,
    /// What went wrong.
    pub message: String,
}

impl UnknownTimeZone {
    pub fn new(id: impl Into<String>, message: impl Into<String>) -> UnknownTimeZone {
        UnknownTimeZone {
            id: id.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for UnknownTimeZone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown time zone {}: {}", self.id, self.message)
    }
}

impl std::error::Error for UnknownTimeZone {}
