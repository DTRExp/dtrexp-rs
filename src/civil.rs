//! Proleptic Gregorian calendar arithmetic, std-only.
//!
//! Days are counted from the Unix epoch (1970-01-01 = day 0). Milliseconds
//! likewise count from the epoch. All functions here are pure integer math —
//! no floating point, no external date library.

pub const MS_PER_SEC: i64 = 1_000;
pub const MS_PER_MIN: i64 = 60_000;
pub const MS_PER_HOUR: i64 = 3_600_000;
pub const MS_PER_DAY: i64 = 86_400_000;

/// Is `y` a leap year in the proleptic Gregorian calendar?
pub fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Number of days in month `m` (1–12) of year `y`.
pub fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

pub fn days_in_year(y: i64) -> i64 {
    if is_leap(y) {
        366
    } else {
        365
    }
}

/// Days since 1970-01-01 for the given civil date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// Civil date (y, m, d) from days since 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (y + i64::from(m <= 2), m, d)
}

/// ISO weekday: Monday = 1 … Sunday = 7.
pub fn iso_weekday(days: i64) -> i64 {
    // 1970-01-01 (day 0) is a Thursday (ISO 4).
    (days + 3).rem_euclid(7) + 1
}

/// Day of year, 1-based.
pub fn day_of_year(y: i64, m: i64, d: i64) -> i64 {
    days_from_civil(y, m, d) - days_from_civil(y, 1, 1) + 1
}

pub fn quarter_of_month(m: i64) -> i64 {
    (m - 1) / 3 + 1
}

pub fn quarter_start_month(q: i64) -> i64 {
    (q - 1) * 3 + 1
}

pub fn days_in_quarter(y: i64, q: i64) -> i64 {
    let sm = quarter_start_month(q);
    (sm..sm + 3).map(|m| days_in_month(y, m)).sum()
}

/// Day within the quarter, 1-based.
pub fn day_of_quarter(y: i64, m: i64, d: i64) -> i64 {
    let q = quarter_of_month(m);
    day_of_year(y, m, d) - day_of_year(y, quarter_start_month(q), 1) + 1
}

/// Number of ISO weeks (52 or 53) in ISO week-year `y`.
pub fn weeks_in_iso_year(y: i64) -> i64 {
    fn p(y: i64) -> i64 {
        (y + y.div_euclid(4) - y.div_euclid(100) + y.div_euclid(400)).rem_euclid(7)
    }
    if p(y) == 4 || p(y - 1) == 3 {
        53
    } else {
        52
    }
}

/// ISO week-date: returns (week_year, week_number).
pub fn iso_week(y: i64, m: i64, d: i64) -> (i64, i64) {
    let days = days_from_civil(y, m, d);
    let wd = iso_weekday(days);
    let doy = day_of_year(y, m, d);
    let week = (doy - wd + 10) / 7;
    if week < 1 {
        (y - 1, weeks_in_iso_year(y - 1))
    } else if week > weeks_in_iso_year(y) {
        (y + 1, 1)
    } else {
        (y, week)
    }
}

/// A naive (zoneless) local wall-clock date-time.
///
/// Used for cadence occurrence windows and bounds spans, which the spec (§9.3)
/// defines as local wall-clock intervals that are never resolved to an instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Naive {
    pub y: i64,
    pub mo: i64,
    pub d: i64,
    pub h: i64,
    pub mi: i64,
    pub s: i64,
    pub ms: i64,
}

impl Naive {
    pub fn new(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64, ms: i64) -> Self {
        Naive {
            y,
            mo,
            d,
            h,
            mi,
            s,
            ms,
        }
    }

    /// Naive ordinal in milliseconds (civil date treated as if UTC). This is a
    /// monotonic key for wall-clock comparison; DST is deliberately ignored.
    pub fn ordinal_ms(&self) -> i64 {
        days_from_civil(self.y, self.mo, self.d) * MS_PER_DAY
            + self.h * MS_PER_HOUR
            + self.mi * MS_PER_MIN
            + self.s * MS_PER_SEC
            + self.ms
    }

    /// Decompose a naive ordinal-ms back into fields.
    pub fn from_ordinal_ms(v: i64) -> Naive {
        let days = v.div_euclid(MS_PER_DAY);
        let rem = v.rem_euclid(MS_PER_DAY);
        let (y, mo, d) = civil_from_days(days);
        Naive {
            y,
            mo,
            d,
            h: rem / MS_PER_HOUR,
            mi: (rem % MS_PER_HOUR) / MS_PER_MIN,
            s: (rem % MS_PER_MIN) / MS_PER_SEC,
            ms: rem % MS_PER_SEC,
        }
    }

    /// Add days, keeping the time of day (naive).
    pub fn add_days(&self, n: i64) -> Naive {
        let (y, mo, d) = civil_from_days(days_from_civil(self.y, self.mo, self.d) + n);
        Naive { y, mo, d, ..*self }
    }

    /// Add `n` months with `constrain` overflow (clamp day to month length).
    pub fn add_months_constrain(&self, n: i64) -> Naive {
        let total = self.y * 12 + (self.mo - 1) + n;
        let y = total.div_euclid(12);
        let mo = total.rem_euclid(12) + 1;
        let d = self.d.min(days_in_month(y, mo));
        Naive { y, mo, d, ..*self }
    }

    /// Add `n` years with `constrain` overflow (Feb 29 → Feb 28 in common years).
    pub fn add_years_constrain(&self, n: i64) -> Naive {
        let y = self.y + n;
        let d = self.d.min(days_in_month(y, self.mo));
        Naive { y, d, ..*self }
    }
}
