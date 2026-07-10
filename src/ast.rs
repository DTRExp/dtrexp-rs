//! The parsed abstract syntax of a DTRExp expression.

use crate::civil::Naive;

/// The nine field designators (`T` is modelled separately as [`TimeSel`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desig {
    Year,
    Quarter,
    Month,
    Week,
    Day,
    Weekday,
    Hour,
    Minute,
    Second,
}

/// One endpoint of a range: a literal value (possibly negative) or `*` (edge).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint {
    Value(i64),
    Star,
}

/// A single item within a selector's value list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Atom {
    /// The whole domain (`M*`).
    All,
    /// A single value; negative counts back from the parent instance's edge.
    Single(i64),
    /// An inclusive range. `wrap` is decided syntactically at parse time (only
    /// when both endpoints are literal non-negative and start > end).
    Range {
        start: Endpoint,
        end: Endpoint,
        wrap: bool,
    },
    /// A stride: from `start`, every `interval`-th unit, covering `duration`,
    /// stopping at `end`.
    Stride {
        start: i64,
        end: Endpoint,
        interval: i64,
        duration: i64,
    },
}

/// A field selector such as `M3:7`, `D-7:*`, `E7#-1`, `H0/4`.
#[derive(Clone, Debug)]
pub struct Selector {
    pub desig: Desig,
    /// The value list (empty when `ordinal` is set).
    pub atoms: Vec<Atom>,
    /// `!` exclusion: the covered set is the domain minus `atoms`.
    pub exclude: bool,
    /// `E`-only weekday ordinal: `(weekday 1-7, ord in -5..-1|1..5)`.
    pub ordinal: Option<(i64, i64)>,
    /// Byte offset of this component (for diagnostics).
    pub pos: usize,
}

/// A half-open time-of-day interval in milliseconds since local midnight.
/// `end` may equal [`MS_PER_DAY`](crate::civil::MS_PER_DAY) (the `2400` token).
#[derive(Clone, Copy, Debug)]
pub struct TimeInterval {
    pub start: i64,
    pub end: i64,
    pub wrap: bool,
}

/// The `T` time-of-day component.
#[derive(Clone, Debug)]
pub struct TimeSel {
    pub intervals: Vec<TimeInterval>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CadUnit {
    Year,
    Month,
    Week,
    Day,
    Hour,
    Minute,
}

impl CadUnit {
    pub fn is_month_or_year(self) -> bool {
        matches!(self, CadUnit::Year | CadUnit::Month)
    }
    pub fn is_calendar(self) -> bool {
        matches!(
            self,
            CadUnit::Year | CadUnit::Month | CadUnit::Week | CadUnit::Day
        )
    }
}

/// An anchor-based cadence such as `20200106/10D/3D`.
#[derive(Clone, Copy, Debug)]
pub struct Cadence {
    pub anchor: Naive,
    pub period_n: i64,
    pub period_unit: CadUnit,
    pub duration_n: i64,
    pub duration_unit: CadUnit,
}

/// Absolute bounds: a naive local wall-clock window `[start, end)` in ordinal
/// milliseconds. `None` means unbounded on that side.
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub start: Option<i64>,
    pub end: Option<i64>,
}

/// One `|`-separated expression (a set of intersected components).
#[derive(Clone, Debug, Default)]
pub struct Expr {
    pub selectors: Vec<Selector>,
    pub time: Option<TimeSel>,
    pub cadence: Option<Cadence>,
    pub bounds: Option<Bounds>,
    /// True when a `W` selector is present (then `Y` tests the ISO week-year).
    pub has_week: bool,
}
