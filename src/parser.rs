//! Recursive-descent parser producing structural AST. Domain/satisfiability
//! checks that need full-expression scope live in [`crate::validate`].

use crate::ast::*;
use crate::civil::{
    days_from_civil, days_in_month, Naive, MS_PER_DAY, MS_PER_HOUR, MS_PER_MIN, MS_PER_SEC,
};
use crate::error::{ParseError, Warning};
use crate::validate;

/// The largest integer the grammar admits, however many leading zeros it
/// carries (spec §8).
const INTEGER_LIMIT: i64 = 2_147_483_647;

/// Parse a whole DTRExp into its `|`-separated branches, each statically
/// validated as soon as it is complete ([`crate::validate`]), and their
/// warnings.
pub fn parse_branches(input: &str) -> Result<(Vec<Expr>, Vec<Warning>), ParseError> {
    let mut p = Parser {
        b: input.as_bytes(),
        i: 0,
        fault: None,
    };
    let mut branches = Vec::new();
    let mut warnings = Vec::new();
    loop {
        let expr = p.parse_expression().map_err(|e| p.earliest(e))?;
        // The static rules run on the completed branch; of everything it
        // broke, the earliest position wins.
        let checked = validate::check(&expr);
        if let Some(fault) = p.fault.take() {
            return Err(earliest(fault, checked.err()));
        }
        warnings.extend(checked?);
        branches.push(expr);
        p.skip_spaces();
        if p.peek() == Some(b'|') {
            p.bump();
            continue;
        }
        break;
    }
    // `parse_expression` only yields when the cursor is at `|` or end-of-input,
    // so consuming the `|`s above leaves nothing but EOF to fall out here.
    Ok((branches, warnings))
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
    /// The earliest rule the branch broke so far. A rule failure does not stop
    /// the parse: the branch is read on, so that a rule broken earlier in the
    /// text, and found later, is the one reported (spec §8, *Error
    /// positions*). Only a token the grammar cannot accept stops it.
    fault: Option<ParseError>,
}

struct TimeVal {
    ms: i64,
    /// Length of the implied unit interval for a single value (hour/min/sec/ms).
    unit: i64,
    /// True for the exact `2400` end token (maps to end-of-day).
    is_2400: bool,
}

struct TimePart {
    h: i64,
    mi: i64,
    s: Option<i64>,
}

struct DateLit {
    y: i64,
    m: i64,
    d: i64,
    time: Option<TimePart>,
}

impl DateLit {
    fn days(&self) -> i64 {
        days_from_civil(self.y, self.m, self.d)
    }
    fn span_start(&self) -> i64 {
        let base = self.days() * MS_PER_DAY;
        match &self.time {
            None => base,
            Some(tp) => {
                base + tp.h * MS_PER_HOUR + tp.mi * MS_PER_MIN + tp.s.unwrap_or(0) * MS_PER_SEC
            }
        }
    }
    fn span_end(&self) -> i64 {
        match &self.time {
            None => (self.days() + 1) * MS_PER_DAY,
            Some(tp) => {
                let start = self.span_start();
                match tp.s {
                    None => start + MS_PER_MIN,
                    Some(_) => start + MS_PER_SEC,
                }
            }
        }
    }
    fn anchor(&self) -> Naive {
        let (h, mi, s) = match &self.time {
            None => (0, 0, 0),
            Some(tp) => (tp.h, tp.mi, tp.s.unwrap_or(0)),
        };
        Naive::new(self.y, self.m, self.d, h, mi, s, 0)
    }
}

/// The earlier of a rule failure found while parsing and one found after;
/// on a tie, the one found first.
fn earliest(fault: ParseError, other: Option<ParseError>) -> ParseError {
    match other {
        Some(e) if e.pos < fault.pos => e,
        _ => fault,
    }
}

impl<'a> Parser<'a> {
    /// Record a broken rule and read on; the earliest one is kept.
    fn fault(&mut self, pos: usize, message: &str) {
        if self.fault.as_ref().is_none_or(|f| pos < f.pos) {
            self.fault = Some(ParseError::new(pos, message));
        }
    }

    /// A grammar error ends the parse; a rule broken before it still wins.
    fn earliest(&mut self, grammar: ParseError) -> ParseError {
        match self.fault.take() {
            Some(fault) => earliest(fault, Some(grammar)),
            None => grammar,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
    fn peek_at(&self, k: usize) -> Option<u8> {
        self.b.get(self.i + k).copied()
    }
    fn bump(&mut self) -> u8 {
        let c = self.b[self.i];
        self.i += 1;
        c
    }
    fn skip_spaces(&mut self) {
        while self.peek() == Some(b' ') {
            self.i += 1;
        }
    }

    fn parse_expression(&mut self) -> Result<Expr, ParseError> {
        let mut expr = Expr::default();
        let mut n = 0;
        loop {
            self.skip_spaces();
            match self.peek() {
                None | Some(b'|') => break,
                _ => {}
            }
            self.parse_component(&mut expr)?;
            n += 1;
        }
        if n == 0 {
            return Err(ParseError::new(self.i, "empty expression"));
        }
        expr.has_week = expr.selectors.iter().any(|s| s.desig == Desig::Week);
        Ok(expr)
    }

    fn parse_component(&mut self, expr: &mut Expr) -> Result<(), ParseError> {
        let c = self.peek().unwrap();
        if c.is_ascii_digit() {
            self.parse_date_component(expr)
        } else if c == b'*' {
            self.parse_bounds_star(expr)
        } else if c == b'T' {
            self.parse_time(expr)
        } else if let Some(d) = designator(c) {
            self.parse_selector(expr, d)
        } else {
            Err(ParseError::new(self.i, "unknown designator"))
        }
    }

    // ---- selectors -------------------------------------------------------

    fn parse_selector(&mut self, expr: &mut Expr, desig: Desig) -> Result<(), ParseError> {
        let pos = self.i;
        self.bump(); // designator letter

        // Value part must start with a digit, '-', '*' or '!'.
        match self.peek() {
            Some(c) if c.is_ascii_digit() || c == b'-' || c == b'*' || c == b'!' => {}
            _ => return Err(ParseError::new(self.i, "designator requires a value")),
        }

        let mut exclude = false;
        let mut ordinal = None;
        let mut interval_at = 0;
        let (atoms, at);

        if self.peek() == Some(b'!') {
            self.bump();
            exclude = true;
            (atoms, at) = build_atoms(self.parse_items()?, true)?;
            if self.peek() == Some(b'/') {
                return Err(ParseError::new(
                    self.i,
                    "a component is either an exclusion or a stride, never both",
                ));
            }
        } else if self.peek() == Some(b'*') && self.peek_at(1) != Some(b':') {
            at = vec![(self.i, self.i)];
            self.bump();
            atoms = vec![Atom::All];
        } else {
            let first = self.parse_item()?;
            match self.peek() {
                Some(b'/') => {
                    interval_at = self.i + 1;
                    at = vec![first.at];
                    atoms = vec![self.build_stride(desig, first)?];
                }
                Some(b'#') => {
                    if desig != Desig::Weekday {
                        return Err(ParseError::new(self.i, "ordinal '#' is valid only on E"));
                    }
                    let wd = match first.raw {
                        // a negative base resolves against E's fixed domain
                        // first: E-1#2 ≡ E7#2 (spec §3)
                        Raw::Value(v) if (-7..=-1).contains(&v) => 8 + v,
                        Raw::Value(v) => v,
                        _ => {
                            return Err(ParseError::new(self.i, "ordinal weekday must be a value"))
                        }
                    };
                    self.bump(); // '#'
                    let ord_pos = self.i;
                    let neg = self.peek() == Some(b'-');
                    if neg {
                        self.bump();
                    }
                    let n = self.read_uint(ord_pos)?;
                    if n == 0 || n > 5 {
                        self.fault(ord_pos, "ordinal out of range (-5..-1, 1..5)");
                    }
                    ordinal = Some((wd, if neg { -n } else { n }));
                    at = vec![first.at];
                    atoms = Vec::new();
                }
                Some(b',') => {
                    let mut items = vec![first];
                    while self.peek() == Some(b',') {
                        self.bump();
                        items.push(self.parse_item()?);
                    }
                    if self.peek() == Some(b'/') {
                        return Err(ParseError::new(
                            self.i,
                            "a stride attaches to a single value or range, not a list",
                        ));
                    }
                    (atoms, at) = build_atoms(items, false)?;
                }
                _ => {
                    // A single item never trips the bare-`*`-in-a-list guard (a
                    // lone `*` is handled above), so this is infallible.
                    (atoms, at) = build_atoms(vec![first], false).unwrap();
                }
            }
        }

        // Checked once the selector is read: a malformed repeat fails on its
        // own syntax first (spec §8, *Error positions*).
        if expr.selectors.iter().any(|s| s.desig == desig) {
            self.fault(pos, "duplicate designator in one expression");
        }
        expr.selectors.push(Selector {
            desig,
            atoms,
            exclude,
            ordinal,
            pos,
            at,
            interval_at,
        });
        Ok(())
    }

    /// A value list: one or more items separated by `,`.
    fn parse_items(&mut self) -> Result<Vec<Item>, ParseError> {
        let mut items = vec![self.parse_item()?];
        while self.peek() == Some(b',') {
            self.bump();
            items.push(self.parse_item()?);
        }
        Ok(items)
    }

    fn build_stride(&mut self, desig: Desig, first: Item) -> Result<Atom, ParseError> {
        let stride_pos = self.i;
        let (start, end) = match first.raw {
            Raw::Value(v) => (v, Endpoint::Star),
            Raw::Range { start, end } => {
                let s = match start {
                    Endpoint::Value(v) => v,
                    Endpoint::Star => {
                        self.fault(
                            stride_pos,
                            "stride start must be an explicit non-negative value",
                        );
                        1 // in every domain, so it breaks no rule of its own
                    }
                };
                // A wrap range takes no stride; on Y the range itself is wrong,
                // since years cannot wrap.
                if let Endpoint::Value(e) = end {
                    if s >= 0 && e >= 0 && s > e {
                        if desig == Desig::Year {
                            self.fault(first.at.1, "Y range cannot wrap — no edge to wrap around");
                        } else {
                            self.fault(stride_pos, "wrap ranges take no stride");
                        }
                    }
                }
                (s, end)
            }
            Raw::Star => {
                return Err(ParseError::new(
                    stride_pos,
                    "anchorless stride — an explicit start is required",
                ))
            }
        };
        if start < 0 {
            self.fault(
                stride_pos,
                "stride start must be non-negative (end-relative anchors shift per parent instance)",
            );
        }
        self.bump(); // '/'
        let interval_pos = self.i;
        let interval = self.read_uint(interval_pos)?;
        if interval < 2 {
            self.fault(interval_pos, "stride interval must be >= 2");
        }
        let duration = if self.peek() == Some(b'/') {
            self.bump();
            let duration_pos = self.i;
            let d = self.read_uint(duration_pos)?;
            if d < 1 || d >= interval {
                self.fault(duration_pos, "stride duration must be in 1..interval");
            }
            d
        } else {
            1
        };
        Ok(Atom::Stride {
            start,
            end,
            interval,
            duration,
        })
    }

    /// Parse a single value-list item: value, `*`, or a range.
    fn parse_item(&mut self) -> Result<Item, ParseError> {
        let start_pos = self.i;
        let start = self.parse_endpoint()?;
        if self.peek() == Some(b':') {
            self.bump();
            let end_pos = self.i;
            let end = self.parse_endpoint()?;
            return Ok(Item {
                raw: Raw::Range { start, end },
                at: (start_pos, end_pos),
            });
        }
        let raw = match start {
            Endpoint::Star => Raw::Star,
            Endpoint::Value(v) => Raw::Value(v),
        };
        Ok(Item {
            raw,
            at: (start_pos, start_pos),
        })
    }

    fn parse_endpoint(&mut self) -> Result<Endpoint, ParseError> {
        if self.peek() == Some(b'*') {
            self.bump();
            Ok(Endpoint::Star)
        } else {
            Ok(Endpoint::Value(self.read_int_signed()?))
        }
    }

    // ---- time (T) --------------------------------------------------------

    fn parse_time(&mut self, expr: &mut Expr) -> Result<(), ParseError> {
        let pos = self.i;
        self.bump(); // 'T'
        if expr.time.is_some() {
            self.fault(pos, "duplicate T component");
        }
        match self.peek() {
            Some(b'!') => {
                return Err(ParseError::new(
                    self.i,
                    "T takes no exclusion — write the complement explicitly",
                ))
            }
            Some(b'*') => return Err(ParseError::new(self.i, "T takes no '*'")),
            None => return Err(ParseError::new(self.i, "T requires a value")),
            _ => {}
        }
        let mut intervals = Vec::new();
        loop {
            intervals.push(self.parse_time_item()?);
            if self.peek() == Some(b',') {
                self.bump();
                continue;
            }
            break;
        }
        if self.peek() == Some(b'/') {
            return Err(ParseError::new(self.i, "T takes no stride"));
        }
        expr.time = Some(TimeSel { intervals });
        Ok(())
    }

    fn parse_time_item(&mut self) -> Result<TimeInterval, ParseError> {
        let start = self.parse_timeval(false)?;
        if self.peek() == Some(b':') {
            self.bump();
            if self.peek() == Some(b'*') {
                return Err(ParseError::new(self.i, "T takes no '*'"));
            }
            let end_pos = self.i;
            let end = self.parse_timeval(true)?;
            let end_ms = if end.is_2400 { MS_PER_DAY } else { end.ms };
            if start.ms == end_ms {
                self.fault(end_pos, "T range with equal endpoints covers nothing");
            }
            Ok(TimeInterval {
                start: start.ms,
                end: end_ms,
                wrap: start.ms > end_ms,
            })
        } else {
            Ok(TimeInterval {
                start: start.ms,
                end: start.ms + start.unit,
                wrap: false,
            })
        }
    }

    fn parse_timeval(&mut self, allow_2400: bool) -> Result<TimeVal, ParseError> {
        let pos = self.i;
        let mut digits = Vec::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                digits.push(c - b'0');
                self.bump();
            } else {
                break;
            }
        }
        let n = digits.len();
        let two = |a: usize, d: &[u8]| i64::from(d[a]) * 10 + i64::from(d[a + 1]);
        match n {
            2 => {
                let hh = two(0, &digits);
                if hh > 23 {
                    self.fault(pos, "hour out of range in time value");
                }
                Ok(TimeVal {
                    ms: hh * MS_PER_HOUR,
                    unit: MS_PER_HOUR,
                    is_2400: false,
                })
            }
            4 => {
                let hh = two(0, &digits);
                let mm = two(2, &digits);
                if hh == 24 && mm == 0 {
                    if !allow_2400 {
                        self.fault(pos, "'2400' is valid only in range-end position");
                    }
                    return Ok(TimeVal {
                        ms: MS_PER_DAY,
                        unit: 0,
                        is_2400: true,
                    });
                }
                if hh > 23 {
                    self.fault(
                        pos,
                        "hour out of range — 24 exists only as the exact token '2400'",
                    );
                }
                if mm > 59 {
                    self.fault(pos, "minute out of range in time value");
                }
                Ok(TimeVal {
                    ms: hh * MS_PER_HOUR + mm * MS_PER_MIN,
                    unit: MS_PER_MIN,
                    is_2400: false,
                })
            }
            6 => {
                let hh = two(0, &digits);
                let mm = two(2, &digits);
                let ss = two(4, &digits);
                if hh > 23 {
                    self.fault(
                        pos,
                        "hour out of range — 24 exists only as the exact token '2400'",
                    );
                }
                if mm > 59 {
                    self.fault(pos, "minute out of range in time value");
                }
                if ss > 59 {
                    self.fault(
                        pos,
                        "second out of range (leap seconds are not representable)",
                    );
                }
                let mut ms = hh * MS_PER_HOUR + mm * MS_PER_MIN + ss * MS_PER_SEC;
                let mut unit = MS_PER_SEC;
                if self.peek() == Some(b'.') {
                    // The fraction is a token of its own, from its '.' (spec §8).
                    let frac_pos = self.i;
                    self.bump();
                    let mut frac = Vec::new();
                    while let Some(c) = self.peek() {
                        if c.is_ascii_digit() {
                            frac.push(c - b'0');
                            self.bump();
                        } else {
                            break;
                        }
                    }
                    if frac.len() != 3 {
                        return Err(ParseError::new(
                            frac_pos,
                            "millisecond precision requires exactly 3 digits",
                        ));
                    }
                    ms += i64::from(frac[0]) * 100 + i64::from(frac[1]) * 10 + i64::from(frac[2]);
                    unit = 1;
                }
                Ok(TimeVal {
                    ms,
                    unit,
                    is_2400: false,
                })
            }
            _ => Err(ParseError::new(pos, "malformed time value")),
        }
    }

    // ---- date-started components: cadence or bounds ----------------------

    fn parse_date_component(&mut self, expr: &mut Expr) -> Result<(), ParseError> {
        let pos = self.i;
        let date = self.parse_date()?;
        if self.peek() == Some(b'/') {
            self.parse_cadence(expr, date, pos)
        } else {
            self.finish_bounds(expr, Some(date), pos)
        }
    }

    fn parse_bounds_star(&mut self, expr: &mut Expr) -> Result<(), ParseError> {
        let pos = self.i;
        self.bump(); // '*'
        if self.peek() != Some(b':') {
            return Err(ParseError::new(
                self.i,
                "'*' is only valid as a bounds start",
            ));
        }
        self.bump(); // ':'
        if self.peek() == Some(b'*') {
            return Err(ParseError::new(
                self.i,
                "bounds require at least one date-literal endpoint",
            ));
        }
        let end = self.parse_date()?;
        if expr.bounds.is_some() {
            self.fault(pos, "more than one bounds component per expression");
        }
        expr.bounds = Some(Bounds {
            start: None,
            end: Some(end.span_end()),
        });
        Ok(())
    }

    fn finish_bounds(
        &mut self,
        expr: &mut Expr,
        start: Option<DateLit>,
        pos: usize,
    ) -> Result<(), ParseError> {
        let start = start.unwrap();
        let end_pos = self.i + 1;
        let (lo, hi) = if self.peek() == Some(b':') {
            self.bump();
            if self.peek() == Some(b'*') {
                self.bump();
                (Some(start.span_start()), None)
            } else {
                let end = self.parse_date()?;
                (Some(start.span_start()), Some(end.span_end()))
            }
        } else {
            (Some(start.span_start()), Some(start.span_end()))
        };
        if let (Some(l), Some(h)) = (lo, hi) {
            // A range end that cannot follow its start: the error names the end
            // literal (spec §8, *Error positions*). A single literal's span is
            // never empty, so only a range reaches here.
            if l >= h {
                self.fault(end_pos, "backwards bounds range");
            }
        }
        if expr.bounds.is_some() {
            self.fault(pos, "more than one bounds component per expression");
        }
        expr.bounds = Some(Bounds { start: lo, end: hi });
        Ok(())
    }

    fn parse_cadence(
        &mut self,
        expr: &mut Expr,
        date: DateLit,
        pos: usize,
    ) -> Result<(), ParseError> {
        self.bump(); // '/'
        let period_pos = self.i;
        let (period_n, period_unit) = self.read_cadence_term()?;
        if period_n < 1 {
            self.fault(period_pos, "cadence period must be >= 1");
        }
        // An implied duration has no token of its own, so its period answers
        // for it (spec §8, *Error positions*).
        let (duration_n, duration_unit, duration_pos) = if self.peek() == Some(b'/') {
            self.bump();
            let at = self.i;
            let (n, u) = self.read_cadence_term()?;
            if n < 1 {
                self.fault(at, "cadence duration must be >= 1");
            }
            (n, u, at)
        } else {
            (1, period_unit, period_pos)
        };
        // Month/year duration units require a month/year period.
        if duration_unit.is_month_or_year() && !period_unit.is_month_or_year() {
            self.fault(
                duration_pos,
                "month/year duration unit requires a month/year period",
            );
        } else if !duration_lt_period(duration_n, duration_unit, period_n, period_unit) {
            // duration < period, conservatively.
            self.fault(duration_pos, "cadence duration must be < period");
        }
        if expr.cadence.is_some() {
            self.fault(pos, "more than one cadence per expression");
        }
        expr.cadence = Some(Cadence {
            anchor: date.anchor(),
            period_n,
            period_unit,
            duration_n,
            duration_unit,
        });
        Ok(())
    }

    fn read_cadence_term(&mut self) -> Result<(i64, CadUnit), ParseError> {
        let n = self.read_uint(self.i)?;
        let upos = self.i;
        let unit = match self.peek() {
            Some(b'Y') => CadUnit::Year,
            Some(b'M') => CadUnit::Month,
            Some(b'W') => CadUnit::Week,
            Some(b'D') => CadUnit::Day,
            Some(b'H') => CadUnit::Hour,
            Some(b'm') => CadUnit::Minute,
            _ => return Err(ParseError::new(upos, "unknown cadence unit")),
        };
        self.bump();
        Ok((n, unit))
    }

    fn parse_date(&mut self) -> Result<DateLit, ParseError> {
        let pos = self.i;
        let mut digs = [0i64; 8];
        for slot in &mut digs {
            match self.peek() {
                Some(c) if c.is_ascii_digit() => {
                    *slot = i64::from(c - b'0');
                    self.bump();
                }
                _ => return Err(ParseError::new(pos, "date literal requires 8 digits")),
            }
        }
        // Exactly 8 digits: a ninth is no second literal (spec §8).
        if self.peek().is_some_and(|c| c.is_ascii_digit()) {
            return Err(ParseError::new(
                pos,
                "date literal is exactly 8 digits, never followed by another digit",
            ));
        }
        let y = digs[0] * 1000 + digs[1] * 100 + digs[2] * 10 + digs[3];
        let m = digs[4] * 10 + digs[5];
        let d = digs[6] * 10 + digs[7];
        if !valid_date(y, m, d) {
            self.fault(pos, "date literal is not a real calendar date");
        }
        let time = if self.peek() == Some(b'T') {
            self.bump();
            Some(self.parse_date_time(pos)?)
        } else {
            None
        };
        Ok(DateLit { y, m, d, time })
    }

    fn parse_date_time(&mut self, pos: usize) -> Result<TimePart, ParseError> {
        // The T-glue is unconditional: exactly 4 digits (hhmm), optionally 6 (hhmmss).
        let mut digs = Vec::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                digs.push(i64::from(c - b'0'));
                self.bump();
            } else {
                break;
            }
        }
        if digs.len() != 4 && digs.len() != 6 {
            return Err(ParseError::new(pos, "malformed time-part in date literal"));
        }
        let h = digs[0] * 10 + digs[1];
        let mi = digs[2] * 10 + digs[3];
        if h > 23 || mi > 59 {
            self.fault(pos, "time-part out of range in date literal");
        }
        let s = if digs.len() == 6 {
            let sec = digs[4] * 10 + digs[5];
            if sec > 59 {
                self.fault(pos, "seconds out of range in date literal");
            }
            Some(sec)
        } else {
            None
        };
        Ok(TimePart { h, mi, s })
    }

    // ---- primitive readers ----------------------------------------------

    /// A value: an integer with an optional sign, the sign part of its token.
    fn read_int_signed(&mut self) -> Result<i64, ParseError> {
        let pos = self.i;
        let neg = self.peek() == Some(b'-');
        if neg {
            self.bump();
        }
        let v = self.read_uint(pos)?;
        if neg && v == 0 {
            // the sign requires a nonzero integer: '-0' is not a value (spec §3)
            self.fault(pos, "'-0' is not a value");
        }
        Ok(if neg { -v } else { v })
    }

    /// An unsigned integer at the cursor, whose token begins at `token_pos`
    /// (before its sign, if it has one). Leading zeros are unlimited; a value
    /// above [`INTEGER_LIMIT`] is an error at the token (spec §8).
    fn read_uint(&mut self, token_pos: usize) -> Result<i64, ParseError> {
        let pos = self.i;
        let mut v: i64 = 0;
        while let Some(c) = self.peek() {
            if !c.is_ascii_digit() {
                break;
            }
            // Held one above the limit once past it, so the digits read on.
            v = (v * 10 + i64::from(c - b'0')).min(INTEGER_LIMIT + 1);
            self.bump();
        }
        if v > INTEGER_LIMIT {
            self.fault(token_pos, "integer above the limit of 2147483647");
        }
        if self.i == pos {
            return Err(ParseError::new(pos, "expected a number"));
        }
        Ok(v)
    }
}

enum Raw {
    Star,
    Value(i64),
    Range { start: Endpoint, end: Endpoint },
}

/// A value-list item and where its start and end value tokens begin.
struct Item {
    raw: Raw,
    at: (usize, usize),
}

/// Atoms with their value positions alongside ([`Selector::at`]).
type Atoms = (Vec<Atom>, Vec<(usize, usize)>);

/// Turn list items into atoms, with their value positions alongside. A bare
/// `*` is never a list item (spec §3): a list holding it is the whole domain,
/// and an exclusion takes a value list (`M!*` is an error, `M!*:*` a range),
/// so the error names the `*`.
fn build_atoms(items: Vec<Item>, exclude: bool) -> Result<Atoms, ParseError> {
    let mut atoms = Vec::with_capacity(items.len());
    let mut at = Vec::with_capacity(items.len());
    for it in items {
        atoms.push(match it.raw {
            Raw::Star if exclude => {
                return Err(ParseError::new(
                    it.at.0,
                    "an exclusion takes a value list, not a bare '*' — '!*:*' excludes the whole domain",
                ))
            }
            Raw::Star => {
                return Err(ParseError::new(
                    it.at.0,
                    "bare '*' in a list — the list is already the whole domain",
                ))
            }
            Raw::Value(v) => Atom::Single(v),
            Raw::Range { start, end } => {
                let wrap = matches!((start, end), (Endpoint::Value(s), Endpoint::Value(e)) if s >= 0 && e >= 0 && s > e);
                Atom::Range { start, end, wrap }
            }
        });
        at.push(it.at);
    }
    Ok((atoms, at))
}

fn designator(c: u8) -> Option<Desig> {
    Some(match c {
        b'Y' => Desig::Year,
        b'Q' => Desig::Quarter,
        b'M' => Desig::Month,
        b'W' => Desig::Week,
        b'D' => Desig::Day,
        b'E' => Desig::Weekday,
        b'H' => Desig::Hour,
        b'm' => Desig::Minute,
        b's' => Desig::Second,
        _ => return None,
    })
}

fn valid_date(y: i64, m: i64, d: i64) -> bool {
    (1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m) && (1..=9999).contains(&y)
}

fn cad_lengths(u: CadUnit) -> (i64, i64) {
    // (max_len_sec, min_len_sec)
    let day = 86_400;
    match u {
        CadUnit::Year => (366 * day, 365 * day),
        CadUnit::Month => (31 * day, 28 * day),
        CadUnit::Week => (7 * day, 7 * day),
        CadUnit::Day => (day, day),
        CadUnit::Hour => (3_600, 3_600),
        CadUnit::Minute => (60, 60),
    }
}

fn duration_lt_period(dn: i64, du: CadUnit, pn: i64, pu: CadUnit) -> bool {
    if du == pu {
        return dn < pn;
    }
    let (dmax, _) = cad_lengths(du);
    let (_, pmin) = cad_lengths(pu);
    (dn as i128) * (dmax as i128) < (pn as i128) * (pmin as i128)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_stride_rejects_a_bare_star_anchor() {
        // In the parser a stride only ever attaches to a value or a range, so
        // `build_stride` never actually sees `Raw::Star`; the guard defends that
        // invariant against future callers rather than any current input.
        let mut p = Parser {
            b: b"",
            i: 0,
            fault: None,
        };
        let star = Item {
            raw: Raw::Star,
            at: (0, 0),
        };
        let err = p.build_stride(Desig::Month, star).unwrap_err();
        assert!(err.message.contains("anchorless stride"));
    }

    // Each of these fails inside a specific sub-parser so the surrounding `?`
    // propagates — pinning the error path of every fallible call in the grammar.
    #[test]
    fn sub_parse_failures_propagate() {
        for bad in [
            "M!",           // exclusion value-list: first item missing
            "M!3,",         // exclusion value-list: item after ',' missing
            "M3,",          // list: item after ',' missing
            "E1#",          // ordinal: number after '#' missing
            "M3/",          // stride: interval missing
            "M3/4/",        // stride: duration missing
            "M*:",          // range: endpoint after '*:' missing
            "M3:",          // range: endpoint after ':' missing
            "*:2020",       // '*'-bounds: end is not an 8-digit date
            "20200101:99",  // bounds: end is not an 8-digit date
            "20200101/x",   // cadence: period term unit missing
            "20200101/2D/", // cadence: duration term missing
        ] {
            assert!(parse_branches(bad).is_err(), "`{bad}` should be rejected");
        }
    }
}
