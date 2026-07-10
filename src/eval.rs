//! Coverage evaluation: `covers(instant, tz)`.
//!
//! One calendar-field extraction in the evaluation zone, then per-component
//! integer tests (spec §9). No date-object iteration.

use crate::ast::*;
use crate::civil::{
    civil_from_days, day_of_quarter, day_of_year, days_from_civil, days_in_month, days_in_quarter,
    days_in_year, iso_week, iso_weekday, quarter_of_month, weeks_in_iso_year, Naive, MS_PER_DAY,
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
    // Year is domain-unbounded and handled specially.
    if sel.desig == Desig::Year {
        let field = if has_week { f.week_year } else { f.year_cal };
        return set_covers_year(sel, field);
    }
    let (minv, maxv, field) = domain_and_field(sel.desig, f, scope);
    let hit = sel.atoms.iter().any(|a| atom_match(a, minv, maxv, field));
    if sel.exclude {
        !hit
    } else {
        hit
    }
}

fn domain_and_field(desig: Desig, f: &Fields, scope: Scope) -> (i64, i64, i64) {
    match desig {
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
        Desig::Year => unreachable!("year handled separately"),
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

fn set_covers_year(sel: &Selector, field: i64) -> bool {
    let hit = sel.atoms.iter().any(|a| match *a {
        Atom::All => true,
        Atom::Single(v) => field == v,
        Atom::Range { start, end, .. } => {
            let lo = match start {
                Endpoint::Star => i64::MIN,
                Endpoint::Value(v) => v,
            };
            let hi = match end {
                Endpoint::Star => i64::MAX,
                Endpoint::Value(v) => v,
            };
            lo <= field && field <= hi
        }
        Atom::Stride {
            start,
            end,
            interval,
            duration,
        } => {
            let hi = match end {
                Endpoint::Star => i64::MAX,
                Endpoint::Value(v) => v,
            };
            if field < start || field > hi {
                false
            } else {
                (field - start).rem_euclid(interval) < duration
            }
        }
    });
    if sel.exclude {
        !hit
    } else {
        hit
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
    let anchor_instant = tz.local_to_instant_ms(c.anchor.ordinal_ms());
    let elapsed = instant_ms - anchor_instant;
    if elapsed < 0 {
        return false;
    }
    let period_ms = c.period_n * unit_ms(c.period_unit);
    let duration_ms = c.duration_n * unit_ms(c.duration_unit);
    elapsed.rem_euclid(period_ms) < duration_ms
}

fn unit_ms(u: CadUnit) -> i64 {
    match u {
        CadUnit::Hour => MS_PER_HOUR,
        CadUnit::Minute => MS_PER_MIN,
        // D/W durations inside an absolute H/m grid are fixed lengths — an
        // absolute grid has no calendar anchor to constrain against (spec §9.3).
        CadUnit::Day => 24 * MS_PER_HOUR,
        CadUnit::Week => 7 * 24 * MS_PER_HOUR,
        // M/Y durations require an M/Y period (spec §5.2) — rejected at parse.
        _ => unreachable!("M/Y duration units never reach an absolute grid"),
    }
}

/// Calendar-period (Y/M/W/D) cadence: naive local wall-clock windows.
fn cadence_calendar(c: &Cadence, local_ms: i64) -> bool {
    let t = Naive::from_ordinal_ms(local_ms);
    let anchor = c.anchor;
    let k_est = match c.period_unit {
        CadUnit::Day => (days_from_civil(t.y, t.mo, t.d)
            - days_from_civil(anchor.y, anchor.mo, anchor.d))
        .div_euclid(c.period_n),
        CadUnit::Week => (days_from_civil(t.y, t.mo, t.d)
            - days_from_civil(anchor.y, anchor.mo, anchor.d))
        .div_euclid(c.period_n * 7),
        CadUnit::Month => ((t.y * 12 + t.mo) - (anchor.y * 12 + anchor.mo)).div_euclid(c.period_n),
        CadUnit::Year => (t.y - anchor.y).div_euclid(c.period_n),
        _ => unreachable!(),
    };
    for k in (k_est - 2).max(0)..=(k_est + 2).max(0) {
        if k < 0 {
            continue;
        }
        let start = start_of_occurrence(anchor, c, k);
        let end = end_of_occurrence(start, c);
        let ws = start.ordinal_ms();
        if local_ms >= ws && local_ms < end {
            return true;
        }
    }
    false
}

fn start_of_occurrence(anchor: Naive, c: &Cadence, k: i64) -> Naive {
    let step = k * c.period_n;
    match c.period_unit {
        CadUnit::Day => anchor.add_days(step),
        CadUnit::Week => anchor.add_days(step * 7),
        CadUnit::Month => anchor.add_months_constrain(step),
        CadUnit::Year => anchor.add_years_constrain(step),
        _ => unreachable!(),
    }
}

fn end_of_occurrence(start: Naive, c: &Cadence) -> i64 {
    match c.duration_unit {
        CadUnit::Year => start.add_years_constrain(c.duration_n).ordinal_ms(),
        CadUnit::Month => start.add_months_constrain(c.duration_n).ordinal_ms(),
        CadUnit::Week => start.add_days(c.duration_n * 7).ordinal_ms(),
        CadUnit::Day => start.add_days(c.duration_n).ordinal_ms(),
        CadUnit::Hour => start.ordinal_ms() + c.duration_n * MS_PER_HOUR,
        CadUnit::Minute => start.ordinal_ms() + c.duration_n * MS_PER_MIN,
    }
}
