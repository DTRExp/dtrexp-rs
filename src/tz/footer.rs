//! The POSIX TZ string a TZif v2+ file carries as its footer (RFC 8536 §3.3).
//!
//! It answers for every instant after the file's last transition, and for
//! all instants when the file lists none. "Slim" TZif files (the zic default
//! since 2020) stop listing transitions once the rules settle, so the footer
//! is what carries every current year.
//!
//! Grammar: `std offset [dst [offset] ,rule,rule]`, where a name is three or
//! more letters or `<...>` quoted (letters, digits, `+`, `-`), an offset is
//! `[+-]hh[:mm[:ss]]` west of UTC (the opposite sign of a TZif offset), a dst
//! offset left out means one hour ahead of standard time, and a rule is `Jn`
//! (1..=365, February 29 never counted), `n` (0..=365, counted) or `Mm.w.d`
//! (month, week 1..=5 where 5 is the last, weekday 0 = Sunday), each with an
//! optional `/time` of local wall-clock time (default 02:00, and from -167 to
//! 167 hours under the version-3 extension).

use crate::civil::{civil_from_days, days_from_civil, days_in_month, is_leap, iso_weekday};

const SECS_PER_DAY: i64 = 86_400;

/// A parsed footer: the standard offset, and the DST half when there is one.
#[derive(Clone, Debug)]
pub(super) struct Footer {
    /// Standard offset, seconds east of UTC.
    std: i32,
    dst: Option<Dst>,
}

#[derive(Clone, Debug)]
struct Dst {
    /// DST offset, seconds east of UTC.
    offset: i32,
    start: Rule,
    end: Rule,
}

/// A transition rule: a day of the year and a local wall-clock time on it.
#[derive(Clone, Debug)]
struct Rule {
    date: Date,
    /// Seconds after local midnight, read in the offset in effect before.
    time: i64,
}

#[derive(Clone, Debug)]
enum Date {
    /// `Jn`: day 1..=365, February 29 never counted.
    Julian(i64),
    /// `n`: zero-based day 0..=365, February 29 counted.
    Zero(i64),
    /// `Mm.w.d`: weekday `d` of week `w` of month `m`.
    Month(i64, i64, i64),
}

/// Read the bytes after a v2+ data block: `\n`, the TZ string, `\n`. An empty
/// TZ string is no footer; a missing or unparseable one is an error.
pub(super) fn read(rest: &[u8]) -> Result<Option<Footer>, String> {
    let malformed = || "malformed TZ string footer".to_string();
    let inner = rest
        .strip_prefix(b"\n")
        .and_then(|r| r.strip_suffix(b"\n"))
        .ok_or_else(malformed)?;
    if inner.is_empty() {
        return Ok(None);
    }
    parse(inner).map(Some).ok_or_else(malformed)
}

fn parse(s: &[u8]) -> Option<Footer> {
    let mut c = Cursor { s, p: 0 };
    c.name()?;
    let std = -c.hms(24)?;
    if c.done() {
        return Some(Footer {
            std: std as i32,
            dst: None,
        });
    }
    c.name()?;
    let offset = if c.peek() == Some(b',') {
        std + 3600
    } else {
        -c.hms(24)?
    };
    c.eat(b',').then_some(())?;
    let start = c.rule()?;
    c.eat(b',').then_some(())?;
    let end = c.rule()?;
    c.done().then_some(Footer {
        std: std as i32,
        dst: Some(Dst {
            offset: offset as i32,
            start,
            end,
        }),
    })
}

struct Cursor<'a> {
    s: &'a [u8],
    p: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.p).copied()
    }

    fn done(&self) -> bool {
        self.p == self.s.len()
    }

    fn eat(&mut self, b: u8) -> bool {
        let hit = self.peek() == Some(b);
        self.p += usize::from(hit);
        hit
    }

    /// Advance past the bytes `ok` accepts; how many it took.
    fn take(&mut self, ok: impl Fn(u8) -> bool) -> usize {
        let from = self.p;
        while self.peek().is_some_and(&ok) {
            self.p += 1;
        }
        self.p - from
    }

    /// A zone abbreviation: three or more letters, or `<...>` quoted.
    fn name(&mut self) -> Option<()> {
        let quoted = self.eat(b'<');
        let n = if quoted {
            self.take(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-')
        } else {
            self.take(|b| b.is_ascii_alphabetic())
        };
        (n >= 3 && (!quoted || self.eat(b'>'))).then_some(())
    }

    /// An unsigned decimal of at least one digit, at most `max`.
    fn number(&mut self, max: i64) -> Option<i64> {
        let digits = self.take(|b| b.is_ascii_digit());
        let v = self.s[self.p - digits..self.p].iter().fold(0i64, |v, &b| {
            v.saturating_mul(10).saturating_add(i64::from(b - b'0'))
        });
        (digits > 0 && v <= max).then_some(v)
    }

    /// `[+-]hh[:mm[:ss]]` with `hh <= max_hours`, as signed seconds.
    fn hms(&mut self, max_hours: i64) -> Option<i64> {
        let negative = self.eat(b'-');
        if !negative {
            self.eat(b'+');
        }
        let mut secs = self.number(max_hours)? * 3600;
        if self.eat(b':') {
            secs += self.number(59)? * 60;
            if self.eat(b':') {
                secs += self.number(59)?;
            }
        }
        Some(if negative { -secs } else { secs })
    }

    /// `Jn`, `n` or `Mm.w.d`, then an optional `/time`.
    fn rule(&mut self) -> Option<Rule> {
        let date = if self.eat(b'J') {
            Date::Julian(self.number(365).filter(|&n| n >= 1)?)
        } else if self.eat(b'M') {
            let m = self.number(12).filter(|&m| m >= 1)?;
            self.eat(b'.').then_some(())?;
            let w = self.number(5).filter(|&w| w >= 1)?;
            self.eat(b'.').then_some(())?;
            Date::Month(m, w, self.number(6)?)
        } else {
            Date::Zero(self.number(365)?)
        };
        let time = if self.eat(b'/') { self.hms(167)? } else { 7200 };
        Some(Rule { date, time })
    }
}

impl Rule {
    /// The UTC instant (seconds) this rule names in `year`, its wall-clock
    /// time read in `offset` (seconds east), the offset in effect before.
    fn utc(&self, year: i64, offset: i32) -> i64 {
        self.day(year) * SECS_PER_DAY + self.time - i64::from(offset)
    }

    /// The rule's day in `year`, as days since 1970-01-01.
    fn day(&self, year: i64) -> i64 {
        let jan1 = days_from_civil(year, 1, 1);
        match self.date {
            Date::Julian(n) => jan1 + n - 1 + i64::from(is_leap(year) && n >= 60),
            Date::Zero(n) => jan1 + n,
            Date::Month(m, w, d) => {
                let first = days_from_civil(year, m, 1);
                // ISO Sunday is 7, the same as POSIX's 0 modulo 7.
                let day = first + (d - iso_weekday(first)).rem_euclid(7) + (w - 1) * 7;
                // Week 5 means the last: step back when it runs past the month.
                if day >= first + days_in_month(year, m) {
                    day - 7
                } else {
                    day
                }
            }
        }
    }
}

impl Footer {
    /// The offset (seconds east) in effect at the UTC instant `t` (seconds).
    pub(super) fn offset_at(&self, t: i64) -> i32 {
        let Some(dst) = &self.dst else {
            return self.std;
        };
        // A DST period runs from a start to the next end: the same rule
        // year's, or the next one's when the end comes first in the year (the
        // southern hemisphere). The neighboring rule years cover periods that
        // reach into this one, and times that spill over a year's edge.
        let in_period = |k: i64| {
            let start = dst.start.utc(k, self.std);
            let same_year = dst.end.utc(k, dst.offset);
            let end = if same_year < start {
                dst.end.utc(k + 1, dst.offset)
            } else {
                same_year
            };
            start <= t && t < end
        };
        let year = civil_from_days(t.div_euclid(SECS_PER_DAY)).0;
        if (year - 1..=year + 1).any(in_period) {
            dst.offset
        } else {
            self.std
        }
    }

    /// The offsets the footer can put in effect.
    pub(super) fn offsets(&self) -> [i32; 2] {
        [self.std, self.dst.as_ref().map_or(self.std, |d| d.offset)]
    }

    /// The offset in effect before any gap the footer makes: of its two
    /// offsets, the lower, since a gap is a step up.
    pub(super) fn gap_offset(&self) -> i32 {
        let [a, b] = self.offsets();
        a.min(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn footer(s: &str) -> Footer {
        parse(s.as_bytes()).unwrap()
    }

    fn rule(s: &str) -> Rule {
        let mut c = Cursor {
            s: s.as_bytes(),
            p: 0,
        };
        let r = c.rule().unwrap();
        assert!(c.done(), "`{s}` should parse whole");
        r
    }

    /// UTC seconds for a civil date and time.
    fn at(y: i64, m: i64, d: i64, h: i64, mi: i64) -> i64 {
        days_from_civil(y, m, d) * SECS_PER_DAY + h * 3600 + mi * 60
    }

    /// Assert the offset flips from `before` to `after` exactly at `t`.
    fn flips(f: &Footer, t: i64, before: i32, after: i32) {
        assert_eq!(f.offset_at(t - 1), before, "one second before {t}");
        assert_eq!(f.offset_at(t), after, "at {t}");
    }

    #[test]
    fn reads_the_framing_around_the_tz_string() {
        assert!(read(b"\n\n").unwrap().is_none());
        let f = read(b"\n<+03>-3\n").unwrap().unwrap();
        assert_eq!(f.offsets(), [10_800, 10_800]);
        for bad in [
            &b""[..],
            b"\n",
            b"<+03>-3\n",
            b"\n<+03>-3",
            b"\n<+03>-3\nx",
            b"\nnot one\n",
        ] {
            assert_eq!(read(bad).unwrap_err(), "malformed TZ string footer");
        }
    }

    #[test]
    fn parses_offsets_west_of_utc_as_seconds_east() {
        assert_eq!(footer("CET-1CEST,M3.5.0,M10.5.0/3").offsets(), [3600, 7200]);
        assert_eq!(
            footer("EST5EDT,M3.2.0,M11.1.0").offsets(),
            [-18_000, -14_400]
        );
        assert_eq!(
            footer("<-04>4<-03>,M9.1.6/24,M4.1.6/24").offsets(),
            [-14_400, -10_800]
        );
        assert_eq!(footer("AAA+3").offsets(), [-10_800, -10_800]);
        assert_eq!(footer("AAA-5:30:15").offsets(), [19_815, 19_815]);
        assert_eq!(footer("AAA-5:30").offsets(), [19_800, 19_800]);
        assert_eq!(footer("AAA24").offsets(), [-86_400, -86_400]);
        // An explicit dst offset wins over the one-hour default.
        assert_eq!(footer("AAA0BBB-2,M3.5.0,M10.5.0").offsets(), [0, 7200]);
        assert_eq!(
            footer("AAA0<+0130>-1:30,M3.5.0,M10.5.0").offsets(),
            [0, 5400]
        );
    }

    #[test]
    fn rejects_malformed_tz_strings() {
        for bad in [
            "",
            "CE",
            "CET",
            "CET-",
            "CET+",
            "CET25",
            "CET-1:60",
            "CET-1:00:60",
            "CET-1:",
            "CET-99999999999999999999999",
            "<+03",
            "<+0>-3",
            "<>-3",
            "<+03>-3 ",
            "CET-1CE",
            "CET-1CEST",
            "CET-1CEST,",
            "CET-1CEST-25,M3.5.0,M10.5.0",
            "CET-1CEST-2",
            "CET-1CEST-2;M3.5.0,M10.5.0",
            "CET-1CEST,M3.5.0",
            "CET-1CEST,M3.5.0,",
            "CET-1CEST,M3.5.0M10.5.0",
            "CET-1CEST,M3.5.0,M10.5.0/",
            "CET-1CEST,M3.5.0,M10.5.0/168",
            "CET-1CEST,M3.5.0,M10.5.0x",
            "CET-1CEST,M0.5.0,M10.5.0",
            "CET-1CEST,M13.5.0,M10.5.0",
            "CET-1CEST,M3.0.0,M10.5.0",
            "CET-1CEST,M3.6.0,M10.5.0",
            "CET-1CEST,M3.5.7,M10.5.0",
            "CET-1CEST,M3.5,M10.5.0",
            "CET-1CEST,M3x5.0,M10.5.0",
            "CET-1CEST,M3.5x0,M10.5.0",
            "CET-1CEST,M.5.0,M10.5.0",
            "CET-1CEST,J0,J300",
            "CET-1CEST,J366,J300",
            "CET-1CEST,366,300",
            "CET-1CEST,X,300",
        ] {
            assert!(
                parse(bad.as_bytes()).is_none(),
                "`{bad}` should be rejected"
            );
        }
    }

    #[test]
    fn reads_rule_times_with_the_version_3_range() {
        assert_eq!(rule("M3.5.0").time, 7200);
        assert_eq!(rule("M3.5.0/3").time, 10_800);
        assert_eq!(rule("M3.5.0/-1").time, -3600);
        assert_eq!(rule("M3.5.0/+1:30:15").time, 5415);
        assert_eq!(rule("M3.5.0/167").time, 601_200);
        assert_eq!(rule("M3.5.0/-167").time, -601_200);
    }

    #[test]
    fn julian_days_never_count_february_29() {
        let d = |s: &str, y: i64| rule(s).day(y);
        assert_eq!(d("J1", 2026), days_from_civil(2026, 1, 1));
        assert_eq!(d("J59", 2028), days_from_civil(2028, 2, 28));
        assert_eq!(d("J60", 2028), days_from_civil(2028, 3, 1));
        assert_eq!(d("J59", 2026), days_from_civil(2026, 2, 28));
        assert_eq!(d("J60", 2026), days_from_civil(2026, 3, 1));
        assert_eq!(d("J365", 2028), days_from_civil(2028, 12, 31));
    }

    #[test]
    fn zero_based_days_count_february_29() {
        let d = |s: &str, y: i64| rule(s).day(y);
        assert_eq!(d("0", 2026), days_from_civil(2026, 1, 1));
        assert_eq!(d("59", 2028), days_from_civil(2028, 2, 29));
        assert_eq!(d("59", 2026), days_from_civil(2026, 3, 1));
        assert_eq!(d("365", 2028), days_from_civil(2028, 12, 31));
    }

    #[test]
    fn month_rules_pick_the_nth_weekday_and_week_5_is_the_last() {
        let d = |s: &str, y: i64| rule(s).day(y);
        assert_eq!(d("M3.5.0", 2026), days_from_civil(2026, 3, 29));
        // March 2029 starts on a Thursday: a fifth Sunday would be April 1.
        assert_eq!(d("M3.5.0", 2029), days_from_civil(2029, 3, 25));
        // March 2026 starts on a Sunday, so that is its first.
        assert_eq!(d("M3.1.0", 2026), days_from_civil(2026, 3, 1));
        assert_eq!(d("M3.2.0", 2026), days_from_civil(2026, 3, 8));
        assert_eq!(d("M11.1.0", 2026), days_from_civil(2026, 11, 1));
        assert_eq!(d("M9.1.6", 2026), days_from_civil(2026, 9, 5));
        assert_eq!(d("M10.4.3", 2026), days_from_civil(2026, 10, 28));
        assert_eq!(d("M12.5.4", 2026), days_from_civil(2026, 12, 31));
        assert_eq!(d("M2.5.0", 2026), days_from_civil(2026, 2, 22));
    }

    #[test]
    fn a_std_only_footer_is_a_fixed_offset() {
        let f = footer("<+03>-3");
        assert_eq!(f.offset_at(at(2026, 7, 7, 0, 0)), 10_800);
        assert_eq!(f.offset_at(at(1900, 1, 1, 0, 0)), 10_800);
        assert_eq!(f.gap_offset(), 10_800);
    }

    #[test]
    fn northern_rules_switch_at_their_utc_instants() {
        // Berlin: on 03-29 at 02:00 CET (01:00Z), off on 10-25 at 03:00 CEST (01:00Z).
        let f = footer("CET-1CEST,M3.5.0,M10.5.0/3");
        flips(&f, at(2026, 3, 29, 1, 0), 3600, 7200);
        flips(&f, at(2026, 10, 25, 1, 0), 7200, 3600);
        flips(&f, at(2029, 3, 25, 1, 0), 3600, 7200);
        assert_eq!(f.offset_at(at(2026, 1, 1, 0, 0)), 3600);
        assert_eq!(f.offset_at(at(2026, 12, 31, 23, 59)), 3600);
        assert_eq!(f.gap_offset(), 3600);
    }

    #[test]
    fn southern_rules_carry_dst_across_the_year_end() {
        // Sydney: off on 04-05 at 03:00 AEDT (04-04 16:00Z), on on 10-04 at
        // 02:00 AEST (10-03 16:00Z).
        let f = footer("AEST-10AEDT,M10.1.0,M4.1.0/3");
        flips(&f, at(2026, 4, 4, 16, 0), 39_600, 36_000);
        flips(&f, at(2026, 10, 3, 16, 0), 36_000, 39_600);
        assert_eq!(f.offset_at(at(2026, 1, 1, 0, 0)), 39_600);
        assert_eq!(f.offset_at(at(2026, 7, 1, 0, 0)), 36_000);
        assert_eq!(f.offset_at(at(2026, 12, 31, 23, 0)), 39_600);
    }

    #[test]
    fn negative_rule_times_land_on_the_previous_day() {
        // Nuuk: on at -01:00 on the last Sunday of March (Saturday 23:00 at
        // -02), off at 00:00 on the last Sunday of October (at -01).
        let f = footer("<-02>2<-01>,M3.5.0/-1,M10.5.0/0");
        flips(&f, at(2026, 3, 29, 1, 0), -7200, -3600);
        flips(&f, at(2026, 10, 25, 1, 0), -3600, -7200);
    }

    #[test]
    fn julian_and_zero_based_rules_switch_on_their_days() {
        // On at J60 (March 1) 02:00 at +1, off at day 300 (October 28) 02:00 at +2.
        let f = footer("<+01>-1<+02>,J60,300");
        flips(&f, at(2026, 3, 1, 1, 0), 3600, 7200);
        flips(&f, at(2026, 10, 28, 0, 0), 7200, 3600);
        // 2028 is a leap year: J60 is still March 1, day 300 one day earlier.
        flips(&f, at(2028, 3, 1, 1, 0), 3600, 7200);
        flips(&f, at(2028, 10, 27, 0, 0), 7200, 3600);
    }

    #[test]
    fn permanent_dst_holds_across_the_year_end() {
        // zic's form for DST all year: on at January 1 00:00, off at December
        // 31 25:00, the same instant as the next year's start.
        let f = footer("EST5EDT,0/0,J365/25");
        for t in [
            at(2026, 1, 1, 4, 59),
            at(2026, 1, 1, 5, 0),
            at(2026, 7, 1, 0, 0),
            at(2026, 12, 31, 23, 59),
            at(2027, 1, 1, 2, 0),
            at(2027, 1, 1, 5, 0),
        ] {
            assert_eq!(f.offset_at(t), -14_400, "at {t}");
        }
    }

    #[test]
    fn a_start_that_spills_into_the_previous_year_counts() {
        // On at January 1 -01:00, which is December 31 23:00 of the year before.
        let f = footer("AAA0BBB,0/-1,M6.1.0");
        flips(&f, at(2026, 12, 31, 23, 0), 0, 3600);
        assert_eq!(f.offset_at(at(2027, 3, 1, 0, 0)), 3600);
        assert_eq!(f.offset_at(at(2027, 7, 1, 0, 0)), 0);
    }

    #[test]
    fn an_end_that_spills_into_the_next_year_counts() {
        // Off on the last Sunday of December at +167:00, a week into January.
        let f = footer("AAA0BBB,M6.1.0,M12.5.0/167");
        let end = at(2026, 12, 27, 0, 0) + 167 * 3600 - 3600;
        flips(&f, end, 3600, 0);
        assert_eq!(f.offset_at(at(2027, 1, 2, 0, 0)), 3600);
    }

    #[test]
    fn a_zero_length_dst_period_is_never_in_effect() {
        // On at 01:00 at +0, off at 02:00 at +1: both 01:00Z, the same instant.
        let f = footer("AAA0BBB,M3.5.0/1,M3.5.0/2");
        for t in [
            at(2026, 3, 29, 0, 0),
            at(2026, 3, 29, 1, 0),
            at(2026, 7, 1, 0, 0),
        ] {
            assert_eq!(f.offset_at(t), 0, "at {t}");
        }
    }

    #[test]
    fn a_negative_dst_gap_steps_up_from_the_dst_offset() {
        // Dublin's legal form: standard IST +1 in summer, GMT as the "dst" of
        // winter, so the spring gap is the step from the dst offset.
        let f = footer("IST-1GMT0,M10.5.0,M3.5.0/1");
        assert_eq!(f.offsets(), [3600, 0]);
        assert_eq!(f.gap_offset(), 0);
        flips(&f, at(2026, 3, 29, 1, 0), 0, 3600);
        flips(&f, at(2026, 10, 25, 1, 0), 3600, 0);
    }
}
