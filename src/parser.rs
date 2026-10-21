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
/// validated ([`crate::validate`]): a selector as soon as it is read, the `D`
/// domain and the warnings once the branch is complete.
pub fn parse_branches(input: &str) -> Result<(Vec<Expr>, Vec<Warning>), ParseError> {
    let mut p = Parser {
        b: input.as_bytes(),
        i: 0,
        fault: None,
        seen: 0,
    };
    let mut branches = Vec::new();
    let mut warnings = Vec::new();
    loop {
        let expr = p.parse_expression().map_err(|e| p.earliest(e))?;
        // The `D` domain waits for the completed branch's scope; of everything
        // the branch broke, the earliest position wins. The warnings read a
        // branch that broke no rule, never one holding a value its rules reject.
        let days = validate::check_days(&expr);
        if let Some(fault) = p.fault.take() {
            return Err(earliest(fault, days.err()));
        }
        days?;
        warnings.extend(validate::warnings(&expr));
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
    /// The designators the branch has read so far, one bit each, so that a
    /// repeat is found without rescanning the branch.
    seen: u16,
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
        self.seen = 0;
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
        expr.has_week = self.seen & desig_bit(Desig::Week) != 0;
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
            (atoms, at) = self.read_list(desig, true, None)?;
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
            let first = self.parse_item(desig)?;
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
                _ => {
                    // The first item is never a bare `*` (a lone `*` is
                    // handled above), so it always has an atom.
                    let atom = atom_of(first.raw).unwrap();
                    (atoms, at) = self.read_list(desig, false, Some((atom, first.at)))?;
                    if self.peek() == Some(b'/') {
                        return Err(ParseError::new(
                            self.i,
                            "a stride attaches to a single value or range, not a list",
                        ));
                    }
                }
            }
        }

        let sel = Selector {
            desig,
            atoms,
            exclude,
            ordinal,
            pos,
            at,
            interval_at,
        };
        // A repeat is a rule between two designators, decided once the second
        // is read, so it fails on its own syntax first (spec §8).
        if self.seen & desig_bit(desig) != 0 {
            self.fault(pos, "duplicate designator in one expression");
        }
        self.seen |= desig_bit(desig);
        expr.selectors.push(sel);
        Ok(())
    }

    /// A value list: one or more items separated by `,`, after `first` when
    /// it has been read already.
    fn read_list(
        &mut self,
        desig: Desig,
        exclude: bool,
        first: Option<(Atom, (usize, usize))>,
    ) -> Result<Atoms, ParseError> {
        let (mut atoms, mut at) = (Vec::new(), Vec::new());
        let mut item = match first {
            Some(item) => item,
            None => self.list_item(desig, exclude)?,
        };
        loop {
            atoms.push(item.0);
            at.push(item.1);
            if self.peek() != Some(b',') {
                return Ok((atoms, at));
            }
            self.bump();
            item = self.list_item(desig, exclude)?;
        }
    }

    /// A list item, with where its values begin. A bare `*` is never a list
    /// item (spec §3): a list holding it is the whole domain, and an exclusion
    /// takes a value list (`M!*` is an error, `M!*:*` a range). It breaks a
    /// rule, decided at the token after it once that is not the `:` of a
    /// range (spec §8): the error names the `*`, and the list reads on.
    fn list_item(
        &mut self,
        desig: Desig,
        exclude: bool,
    ) -> Result<(Atom, (usize, usize)), ParseError> {
        let item = self.parse_item(desig)?;
        let atom = atom_of(item.raw).unwrap_or_else(|| {
            self.fault(
                item.at.0,
                if exclude {
                    "an exclusion takes a value list, not a bare '*' — '!*:*' excludes the whole domain"
                } else {
                    "bare '*' in a list — the list is already the whole domain"
                },
            );
            Atom::All
        });
        Ok((atom, item.at))
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
                // A wrap range takes no stride. On Y the range itself is
                // wrong, which its end, before the '/', already recorded.
                if let Endpoint::Value(e) = end {
                    if s >= 0 && e >= 0 && s > e {
                        self.fault(stride_pos, "wrap ranges take no stride");
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
        if let Err(e) = validate::check_interval(desig, interval, interval_pos) {
            self.fault(e.pos, &e.message);
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

    /// Parse a single value-list item: value, `*`, or a range. Each value is
    /// checked against `desig`'s domain as it is read (spec §8).
    fn parse_item(&mut self, desig: Desig) -> Result<Item, ParseError> {
        let start_pos = self.i;
        let start = self.parse_endpoint(desig)?;
        if self.peek() == Some(b':') {
            self.bump();
            let end_pos = self.i;
            let end = self.parse_endpoint(desig)?;
            // Years have no edge to wrap around: a range end before its start
            // is wrong at the end.
            if let (Desig::Year, Endpoint::Value(s), Endpoint::Value(e)) = (desig, start, end) {
                if s > e {
                    self.fault(end_pos, "Y range cannot wrap — no edge to wrap around");
                }
            }
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

    fn parse_endpoint(&mut self, desig: Desig) -> Result<Endpoint, ParseError> {
        if self.peek() == Some(b'*') {
            self.bump();
            return Ok(Endpoint::Star);
        }
        let pos = self.i;
        let v = self.read_int_signed()?;
        if let Err(e) = validate::check_value(desig, v, pos) {
            self.fault(e.pos, &e.message);
        }
        Ok(Endpoint::Value(v))
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

    /// A time value and its fraction. A value the rules reject (a malformed
    /// one included) is recorded at its token and read past, its fraction
    /// whole, so that the branch reads on (spec §8, *Error positions*); only a
    /// missing value is a grammar error.
    fn parse_timeval(&mut self, allow_2400: bool) -> Result<TimeVal, ParseError> {
        let pos = self.i;
        let digits = self.digit_run();
        if digits.is_empty() {
            return Err(ParseError::new(pos, "malformed time value"));
        }
        // The fraction is a token of its own, from its '.' (spec §8): exactly
        // 3 digits, after the 6 of hhmmss.
        let dotted = self.peek() == Some(b'.');
        let frac = if dotted {
            let frac_pos = self.i;
            self.bump();
            let frac = self.digit_run();
            if digits.len() != 6 || frac.len() != 3 {
                self.fault(
                    frac_pos,
                    "a fraction is '.' and 3 digits, after hhmmss only",
                );
                None
            } else {
                Some(self.number(frac))
            }
        } else {
            None
        };
        let two = |p: &Self, k: usize| p.number(digits.start + k..digits.start + k + 2);
        let (hh, mm, ss, unit) = match digits.len() {
            2 => (two(self, 0), 0, 0, MS_PER_HOUR),
            4 => (two(self, 0), two(self, 2), 0, MS_PER_MIN),
            6 => (two(self, 0), two(self, 2), two(self, 4), MS_PER_SEC),
            _ => {
                self.fault(pos, "malformed time value");
                (0, 0, 0, MS_PER_SEC)
            }
        };
        // `2400` is the exact token, never `24` or a `2400` with a fraction.
        if digits.len() == 4 && hh == 24 && mm == 0 && !dotted {
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
        if ss > 59 {
            self.fault(
                pos,
                "second out of range (leap seconds are not representable)",
            );
        }
        let ms = hh * MS_PER_HOUR + mm * MS_PER_MIN + ss * MS_PER_SEC;
        Ok(match frac {
            Some(f) => TimeVal {
                ms: ms + f,
                unit: 1,
                is_2400: false,
            },
            None => TimeVal {
                ms,
                unit,
                is_2400: false,
            },
        })
    }

    // ---- date-started components: cadence or bounds ----------------------

    fn parse_date_component(&mut self, expr: &mut Expr) -> Result<(), ParseError> {
        let pos = self.i;
        let date = self.scan_date();
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
        let end = self.expect_date()?;
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
                let end = self.expect_date()?;
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

    /// A date literal where one is required: a missing one is a grammar
    /// error at the token in its place.
    fn expect_date(&mut self) -> Result<DateLit, ParseError> {
        if !self.peek().is_some_and(|c| c.is_ascii_digit()) {
            return Err(ParseError::new(self.i, "date literal requires 8 digits"));
        }
        Ok(self.scan_date())
    }

    /// A date literal at the cursor, which is a digit. One the rules reject,
    /// its shape included, is recorded at its first character and read past,
    /// its digit runs and `T` part whole, so that the branch reads on (spec
    /// §8, *Error positions*); it then stands for 0001-01-01, which breaks no
    /// rule of its own.
    fn scan_date(&mut self) -> DateLit {
        const NO_DATE: DateLit = DateLit {
            y: 1,
            m: 1,
            d: 1,
            time: None,
        };
        let pos = self.i;
        let digits = self.digit_run();
        // The T-glue is unconditional: a `T` after the digits is the literal's.
        let time = if self.peek() == Some(b'T') {
            self.bump();
            Some(self.digit_run())
        } else {
            None
        };
        // Exactly 8 digits (a ninth is no second literal), and a time part of
        // hhmm or hhmmss (spec §8).
        if digits.len() != 8 || time.as_ref().is_some_and(|t| t.len() != 4 && t.len() != 6) {
            self.fault(
                pos,
                "malformed date literal — exactly 8 digits, its time part hhmm or hhmmss",
            );
            return NO_DATE;
        }
        let at = |p: &Self, k: usize, n: usize| p.number(pos + k..pos + k + n);
        let (y, m, d) = (at(self, 0, 4), at(self, 4, 2), at(self, 6, 2));
        if !valid_date(y, m, d) {
            self.fault(pos, "date literal is not a real calendar date");
            return NO_DATE;
        }
        let time = time.map(|t| TimePart {
            h: self.number(t.start..t.start + 2),
            mi: self.number(t.start + 2..t.start + 4),
            s: (t.len() == 6).then(|| self.number(t.start + 4..t.end)),
        });
        if let Some(tp) = &time {
            if tp.h > 23 || tp.mi > 59 || tp.s.is_some_and(|s| s > 59) {
                self.fault(pos, "time-part out of range in date literal");
                return NO_DATE;
            }
        }
        DateLit { y, m, d, time }
    }

    // ---- primitive readers ----------------------------------------------

    /// The run of ASCII digits at the cursor, read past.
    fn digit_run(&mut self) -> std::ops::Range<usize> {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        start..self.i
    }

    /// The decimal value of a short run of digits.
    fn number(&self, digits: std::ops::Range<usize>) -> i64 {
        self.b[digits]
            .iter()
            .fold(0, |v, &c| v * 10 + i64::from(c - b'0'))
    }

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

/// The atom a list item stands for; `None` for a bare `*`, which stands for
/// none (see [`Parser::list_item`]).
fn atom_of(raw: Raw) -> Option<Atom> {
    match raw {
        Raw::Star => None,
        Raw::Value(v) => Some(Atom::Single(v)),
        Raw::Range { start, end } => {
            let wrap = matches!((start, end), (Endpoint::Value(s), Endpoint::Value(e)) if s >= 0 && e >= 0 && s > e);
            Some(Atom::Range { start, end, wrap })
        }
    }
}

/// A designator's bit in [`Parser::seen`].
fn desig_bit(d: Desig) -> u16 {
    1 << d as u16
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
            seen: 0,
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
