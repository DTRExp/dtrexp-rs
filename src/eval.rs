//! Coverage evaluation: `covers(instant, tz)`.
//!
//! One calendar-field extraction in the evaluation zone, then per-component
//! integer tests (spec §9). No date-object iteration.

use crate::ast::*;
use crate::civil::{
    civil_from_days, day_of_quarter, day_of_year, days_in_month, days_in_quarter, days_in_year,
    iso_week, iso_weekday, quarter_of_month, weeks_in_iso_year, Naive, MS_PER_DAY,
    MS_PER_HOUR, MS_PER_MIN, MS_PER_SEC,
};
use crate::tz::Tz;

#[derive(Clone, Copy)]
enum Scope {
    Month,
    Quarter,
    Year,
}

struct Fields {
    local_ms: i64,
    year_cal: i64,
    week_year: i64,
    week: i64,
    quarter: i64,
    month: i64,
    dom: i64,
    doy: i64,
    doq: i64,
    weekday: i64,
    tod_ms: i64,
    hour: i64,
    minute: i64,
    second: i64,
}

fn fields_of(instant_ms: i64, tz: &Tz) -> Fields {
    let offset = i64::from(tz.offset_at_ms(instant_ms));
    let local_ms = instant_ms + offset * MS_PER_SEC;
    let days = local_ms.div_euclid(MS_PER_DAY);
    let tod = local_ms.rem_euclid(MS_PER_DAY);
    let (y, m, d) = civil_from_days(days);
    let (wy, wk) = iso_week(y, m, d);
    Fields {
        local_ms,
        year_cal: y,
        week_year: wy,
        week: wk,
        quarter: quarter_of_month(m),
        month: m,
        dom: d,
        doy: day_of_year(y, m, d),
        doq: day_of_quarter(y, m, d),
        weekday: iso_weekday(days),
        tod_ms: tod,
        hour: tod / MS_PER_HOUR,
        minute: (tod % MS_PER_HOUR) / MS_PER_MIN,
        second: (tod % MS_PER_MIN) / MS_PER_SEC,
    }
}

fn scope_of(expr: &Expr) -> Scope {
    let has = |d: Desig| expr.selectors.iter().any(|s| s.desig == d);
    if has(Desig::Month) {
        Scope::Month
    } else if has(Desig::Quarter) {
        Scope::Quarter
    } else if has(Desig::Year) {
        Scope::Year
    } else {
        Scope::Month
    }
}

/// Does an expression branch cover `instant_ms`?
pub fn expr_covers(expr: &Expr, instant_ms: i64, tz: &Tz) -> bool {
    let f = fields_of(instant_ms, tz);
    let scope = scope_of(expr);
    for sel in &expr.selectors {
        if !selector_covers(sel, &f, scope, expr.has_week) {
            return false;
        }
    }
    if let Some(t) = &expr.time {
        if !time_covers(t, f.tod_ms) {
            return false;
        }
    }
    if let Some(c) = &expr.cadence {
        if !cadence_covers(c, instant_ms, f.local_ms, tz) {
            return false;
        }
    }
    if let Some(b) = &expr.bounds {
        if !bounds_covers(b, f.local_ms) {
            return false;
        }
    }
    true
}

fn selector_covers(sel: &Selector, f: &Fields, scope: Scope, has_week: bool) -> bool {
    // Weekday ordinal is its own thing.
    if let Some((wd, ord)) = sel.ordinal {
        return ordinal_covers(wd, ord, f, scope);
    }
    let (minv, maxv, field) = domain_and_field(sel.desig, f, scope, has_week);
    let hit = sel.atoms.iter().any(|a| atom_match(a, minv, maxv, field));
    if sel.exclude {
        !hit
    } else {
        hit
    }
}

fn domain_and_field(desig: Desig, f: &Fields, scope: Scope, has_week: bool) -> (i64, i64, i64) {
    match desig {
        // Year is domain-unbounded. Its atoms are validated to be positive
        // (1..9999) with no wrap, so the generic [`atom_match`] over the full
        // i64 span behaves identically to a bespoke path: `resolve` never sees a
        // negative (which would need a finite edge), and `*` endpoints map to the
        // extremes. `has_week` switches the field to the ISO week-year.
        Desig::Year => (i64::MIN, i64::MAX, if has_week { f.week_year } else { f.year_cal }),
        Desig::Quarter => (1, 4, f.quarter),
        Desig::Month => (1, 12, f.month),
        Desig::Week => (1, weeks_in_iso_year(f.week_year), f.week),
        Desig::Weekday => (1, 7, f.weekday),
        Desig::Hour => (0, 23, f.hour),
        Desig::Minute => (0, 59, f.minute),
        Desig::Second => (0, 59, f.second),
        Desig::Day => match scope {
            Scope::Month => (1, days_in_month(f.year_cal, f.month), f.dom),
            Scope::Quarter => (1, days_in_quarter(f.year_cal, f.quarter), f.doq),
            Scope::Year => (1, days_in_year(f.year_cal), f.doy),
        },
    }
}

pub(crate) fn resolve(v: i64, maxv: i64) -> i64 {
    if v < 0 {
        maxv + 1 + v
    } else {
        v
    }
}

pub(crate) fn atom_match(atom: &Atom, minv: i64, maxv: i64, field: i64) -> bool {
    match *atom {
        Atom::All => true,
        Atom::Single(v) => field == resolve(v, maxv),
        Atom::Range { start, end, wrap } => {
            let s = match start {
                Endpoint::Star => minv,
                Endpoint::Value(v) => resolve(v, maxv),
            };
            let e = match end {
                Endpoint::Star => maxv,
                Endpoint::Value(v) => resolve(v, maxv),
            };
            if wrap {
                field >= s || field <= e
            } else {
                s <= field && field <= e
            }
        }
        Atom::Stride {
            start,
            end,
            interval,
            duration,
        } => {
            let e = match end {
                Endpoint::Star => maxv,
                Endpoint::Value(v) => resolve(v, maxv),
            };
            if field < start || field > e {
                false
            } else {
                (field - start).rem_euclid(interval) < duration
            }
        }
    }
}

fn ordinal_covers(wd: i64, ord: i64, f: &Fields, scope: Scope) -> bool {
    if f.weekday != wd {
        return false;
    }
    let (day_index, total) = match scope {
        Scope::Month => (f.dom, days_in_month(f.year_cal, f.month)),
        Scope::Quarter => (f.doq, days_in_quarter(f.year_cal, f.quarter)),
        Scope::Year => (f.doy, days_in_year(f.year_cal)),
    };
    if ord > 0 {
        (day_index - 1) / 7 + 1 == ord
    } else {
        (total - day_index) / 7 + 1 == -ord
    }
}

fn time_covers(t: &TimeSel, tod: i64) -> bool {
    t.intervals.iter().any(|iv| {
        if iv.wrap {
            tod >= iv.start || tod < iv.end
        } else {
            iv.start <= tod && tod < iv.end
        }
    })
}

fn bounds_covers(b: &Bounds, local_ms: i64) -> bool {
    b.start.is_none_or(|s| local_ms >= s) && b.end.is_none_or(|e| local_ms < e)
}

fn cadence_covers(c: &Cadence, instant_ms: i64, local_ms: i64, tz: &Tz) -> bool {
    if c.period_unit.is_calendar() {
        cadence_calendar(c, local_ms)
    } else {
        cadence_absolute(c, instant_ms, tz)
    }
}

/// Absolute (H/m period) cadence: anchor resolves to a single instant.
fn cadence_absolute(c: &Cadence, instant_ms: i64, tz: &Tz) -> bool {
    let anchor_ord = c.anchor.ordinal_ms();
    let anchor_instant = tz.local_to_instant_ms(anchor_ord);
    let elapsed = instant_ms - anchor_instant;
    if elapsed < 0 {
        return false;
    }
    // An absolute grid has no calendar anchor to constrain against (spec §9.3),
    // so period/duration are the fixed naive spans of the H/m/D/W units. M/Y
    // duration units require an M/Y period (rejected at parse) and never arrive.
    let period_ms = add_units(c.anchor, c.period_n, c.period_unit).ordinal_ms() - anchor_ord;
    let duration_ms = add_units(c.anchor, c.duration_n, c.duration_unit).ordinal_ms() - anchor_ord;
    elapsed.rem_euclid(period_ms) < duration_ms
}

/// Add `n` of `unit` to a naive wall-clock time. Calendar units (Y/M) use
/// `constrain` overflow; W/D/H/m are exact spans. Total over [`CadUnit`] so the
/// occurrence math carries no dead match arms.
fn add_units(base: Naive, n: i64, unit: CadUnit) -> Naive {
    match unit {
        CadUnit::Year => base.add_years_constrain(n),
        CadUnit::Month => base.add_months_constrain(n),
        CadUnit::Week => base.add_days(n * 7),
        CadUnit::Day => base.add_days(n),
        CadUnit::Hour => Naive::from_ordinal_ms(base.ordinal_ms() + n * MS_PER_HOUR),
        CadUnit::Minute => Naive::from_ordinal_ms(base.ordinal_ms() + n * MS_PER_MIN),
    }
}

/// Calendar-period (Y/M/W/D) cadence: naive local wall-clock windows.
fn cadence_calendar(c: &Cadence, local_ms: i64) -> bool {
    let t = Naive::from_ordinal_ms(local_ms);
    let anchor = c.anchor;
    // Only an estimate — `constrain` overflow makes occurrence starts slightly
    // non-linear, so we scan a ±2 window of occurrence indices around it.
    let k_est = elapsed_units(c.period_unit, anchor, t).div_euclid(c.period_n);
    for k in (k_est - 2).max(0)..=(k_est + 2).max(0) {
        let start = start_of_occurrence(anchor, c, k);
        let end = end_of_occurrence(start, c);
        let ws = start.ordinal_ms();
        if local_ms >= ws && local_ms < end {
            return true;
        }
    }
    false
}

/// Whole `unit`s of naive wall-clock time elapsed from `anchor` to `t` (may be
/// negative). Fixed-length units divide the ordinal-ms span; calendar units
/// (M/Y) count calendar steps. Total over [`CadUnit`] — the inverse shape of
/// [`add_units`], seeding the occurrence scan without a dead match arm.
fn elapsed_units(unit: CadUnit, anchor: Naive, t: Naive) -> i64 {
    let span = t.ordinal_ms() - anchor.ordinal_ms();
    match unit {
        CadUnit::Year => t.y - anchor.y,
        CadUnit::Month => (t.y * 12 + t.mo) - (anchor.y * 12 + anchor.mo),
        CadUnit::Week => span.div_euclid(7 * MS_PER_DAY),
        CadUnit::Day => span.div_euclid(MS_PER_DAY),
        CadUnit::Hour => span.div_euclid(MS_PER_HOUR),
        CadUnit::Minute => span.div_euclid(MS_PER_MIN),
    }
}

fn start_of_occurrence(anchor: Naive, c: &Cadence, k: i64) -> Naive {
    add_units(anchor, k * c.period_n, c.period_unit)
}

fn end_of_occurrence(start: Naive, c: &Cadence) -> i64 {
    add_units(start, c.duration_n, c.duration_unit).ordinal_ms()
}

#[cfg(test)]
mod tests {
    use super::*;

    // `elapsed_units` is total over `CadUnit` for uniformity with `add_units`;
    // the calendar-cadence evaluator only ever feeds it Y/M/W/D period units, so
    // its sub-day arms are pinned here directly against their naive definition.
    #[test]
    fn elapsed_units_counts_whole_sub_day_units() {
        let anchor = Naive::new(2020, 1, 1, 0, 0, 0, 0);
        let t = Naive::new(2020, 1, 1, 5, 30, 0, 0); // 5h30m later
        assert_eq!(elapsed_units(CadUnit::Hour, anchor, t), 5);
        assert_eq!(elapsed_units(CadUnit::Minute, anchor, t), 330);
    }

    #[test]
    fn elapsed_units_floors_toward_negative_infinity() {
        let anchor = Naive::new(2020, 1, 1, 6, 0, 0, 0);
        let t = Naive::new(2020, 1, 1, 5, 0, 0, 0); // one hour before the anchor
        assert_eq!(elapsed_units(CadUnit::Hour, anchor, t), -1);
        assert_eq!(elapsed_units(CadUnit::Minute, anchor, t), -60);
    }
}
