//! Static validation: hard domain/stride errors (returned as [`ParseError`]),
//! plus the §9.1 satisfiability warnings (the required minimum).

use crate::ast::*;
use crate::civil::{days_in_month, days_in_quarter, days_in_year, weeks_in_iso_year};
use crate::error::{ParseError, Warning};
use crate::eval::atom_match;

#[derive(Clone, Copy)]
enum Scope {
    Month,
    Quarter,
    Year,
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

/// (is_zero_based, min_value, max_value, domain_size).
fn domain_info(desig: Desig, scope: Scope) -> (bool, i64, i64, i64) {
    match desig {
        Desig::Quarter => (false, 1, 4, 4),
        Desig::Month => (false, 1, 12, 12),
        Desig::Week => (false, 1, 53, 53),
        Desig::Weekday => (false, 1, 7, 7),
        Desig::Hour => (true, 0, 23, 24),
        Desig::Minute => (true, 0, 59, 60),
        Desig::Second => (true, 0, 59, 60),
        Desig::Day => match scope {
            Scope::Month => (false, 1, 31, 31),
            Scope::Quarter => (false, 1, 92, 92),
            Scope::Year => (false, 1, 366, 366),
        },
        Desig::Year => (false, 1, 9999, 9999),
    }
}

/// Check a value of a selector other than `D` as soon as it is read, so that
/// it competes by position with a grammar error later in the branch (spec §8,
/// *Error positions*). A `D` value passes here: its domain depends on the
/// scope selectors, which may come later, so [`check_days`] decides it.
pub fn check_value(desig: Desig, v: i64, pos: usize) -> Result<(), ParseError> {
    match desig {
        Desig::Day => Ok(()),
        Desig::Year => year_value(pos, v),
        // Only `D` reads the scope.
        _ => value_in_domain(desig, Scope::Month, v, pos),
    }
}

/// Check a stride interval against its parent domain as soon as it is read.
/// `D`'s domain waits for its scope ([`check_days`]), and `Y` strides have
/// no ceiling: the integer limit bounds them (spec §8).
pub fn check_interval(desig: Desig, interval: i64, pos: usize) -> Result<(), ParseError> {
    match desig {
        Desig::Day | Desig::Year => Ok(()),
        _ => interval_in_domain(desig, Scope::Month, interval, pos),
    }
}

/// Check the `D` domain against the completed branch's scope. It reads
/// values only, so it is safe on a branch that broke another rule, whose
/// error it then competes with by position.
pub fn check_days(expr: &Expr) -> Result<(), ParseError> {
    let scope = scope_of(expr);
    for sel in expr.selectors.iter().filter(|s| s.desig == Desig::Day) {
        check_day(sel, scope)?;
    }
    Ok(())
}

/// The warnings of a branch that broke no rule. They evaluate its selectors
/// (a stride's interval divides, a year span is enumerated), so a branch
/// holding a value its rules reject never reaches here.
pub fn warnings(expr: &Expr) -> Vec<Warning> {
    let scope = scope_of(expr);
    let mut ws = Vec::new();
    let month_set = expr
        .selectors
        .iter()
        .find(|s| s.desig == Desig::Month)
        .map(|s| matched_values(s, 1, 12));
    let quarter_set = expr
        .selectors
        .iter()
        .find(|s| s.desig == Desig::Quarter)
        .map(|s| matched_values(s, 1, 4));

    for sel in &expr.selectors {
        match sel.desig {
            Desig::Weekday if sel.ordinal.is_some() => {}
            Desig::Day => {
                if let Some(w) = day_unsat(
                    sel,
                    expr,
                    scope,
                    month_set.as_deref(),
                    quarter_set.as_deref(),
                ) {
                    ws.push(w);
                }
            }
            Desig::Week => {
                if let Some(w) = week_unsat(sel, expr) {
                    ws.push(w);
                }
            }
            _ => {
                if let Some(w) = fixed_unsat(sel) {
                    ws.push(w);
                }
            }
        }
    }

    // M ∩ Q disjointness.
    if let (Some(ms), Some(qs)) = (&month_set, &quarter_set) {
        let disjoint = !ms.is_empty()
            && !qs.is_empty()
            && !ms.iter().any(|&m| qs.contains(&((m - 1) / 3 + 1)));
        if disjoint {
            // A non-empty `month_set` means a Month selector is present.
            let pos = expr
                .selectors
                .iter()
                .find(|s| s.desig == Desig::Month)
                .map(|s| s.pos)
                .unwrap();
            ws.push(Warning::new(
                pos,
                "unsatisfiable — M and Q select disjoint months",
            ));
        }
    }

    ws
}

fn year_value(pos: usize, v: i64) -> Result<(), ParseError> {
    if v < 0 {
        Err(ParseError::new(
            pos,
            "negative value on Y — no edge to count back from",
        ))
    } else if !(1..=9999).contains(&v) {
        // Y takes 4-digit ISO years (spec §2): 1-9999.
        Err(ParseError::new(pos, "year out of domain (1-9999)"))
    } else {
        Ok(())
    }
}

fn value_in_domain(desig: Desig, scope: Scope, v: i64, at: usize) -> Result<(), ParseError> {
    let (zero_based, _minv, maxv, size) = domain_info(desig, scope);
    if v >= 0 {
        if !zero_based && v == 0 {
            return Err(ParseError::new(at, "zero value"));
        }
        if v > maxv {
            return Err(ParseError::new(at, "value out of domain"));
        }
    } else if v < -size {
        return Err(ParseError::new(
            at,
            "negative value out of domain (symmetric parse-time check)",
        ));
    }
    Ok(())
}

fn interval_in_domain(
    desig: Desig,
    scope: Scope,
    interval: i64,
    at: usize,
) -> Result<(), ParseError> {
    let (_, _, _, size) = domain_info(desig, scope);
    if interval > size {
        return Err(ParseError::new(
            at,
            "stride interval exceeds parent domain — use a cadence",
        ));
    }
    Ok(())
}

/// A `D` selector's values and stride interval, against its scope.
fn check_day(sel: &Selector, scope: Scope) -> Result<(), ParseError> {
    let check_val = |v: i64, at: usize| value_in_domain(Desig::Day, scope, v, at);
    for (atom, &(start_at, end_at)) in sel.atoms.iter().zip(&sel.at) {
        match *atom {
            Atom::All => {}
            Atom::Single(v) => check_val(v, start_at)?,
            Atom::Range { start, end, .. } => {
                if let Endpoint::Value(v) = start {
                    check_val(v, start_at)?;
                }
                if let Endpoint::Value(v) = end {
                    check_val(v, end_at)?;
                }
            }
            Atom::Stride {
                start,
                end,
                interval,
                ..
            } => {
                check_val(start, start_at)?;
                if let Endpoint::Value(v) = end {
                    check_val(v, end_at)?;
                }
                interval_in_domain(Desig::Day, scope, interval, sel.interval_at)?;
            }
        }
    }
    Ok(())
}

// ---- satisfiability warnings --------------------------------------------

/// Which values in [minv, maxv] a (non-Year) selector matches.
fn matched_values(sel: &Selector, minv: i64, maxv: i64) -> Vec<i64> {
    (minv..=maxv)
        .filter(|&v| {
            let hit = sel.atoms.iter().any(|a| atom_match(a, minv, maxv, v));
            if sel.exclude {
                !hit
            } else {
                hit
            }
        })
        .collect()
}

/// The Y selector's values as a closed, enumerable set (≤1000-year span), else
/// `None` for open/unbounded/non-enumerable spans. A stride is sugar over its
/// range and enumerates to the years it selects (spec §9.1).
fn enumerate_years(expr: &Expr) -> Option<Vec<i64>> {
    let sel = expr.selectors.iter().find(|s| s.desig == Desig::Year)?;
    if sel.exclude {
        return None;
    }
    let mut years = Vec::new();
    for atom in &sel.atoms {
        // Validated years are 1..=9999 and never wrap, so `a <= b` here.
        let (a, b) = match *atom {
            Atom::Single(v) => (v, v),
            Atom::Range {
                start: Endpoint::Value(a),
                end: Endpoint::Value(b),
                ..
            } => (a, b),
            Atom::Stride {
                start,
                end: Endpoint::Value(b),
                ..
            } => (start, b),
            _ => return None, // `*` (All, or an open end) → open span
        };
        if b - a > 1000 {
            return None;
        }
        years.extend((a..=b).filter(|&y| atom_match(atom, 1, 9999, y)));
    }
    // Every atom selects at least its own start, so a completed loop leaves
    // `years` non-empty.
    let (lo, hi) = (*years.iter().min().unwrap(), *years.iter().max().unwrap());
    if hi - lo > 1000 {
        return None;
    }
    Some(years)
}

/// Fixed-domain selectors (Y/Q/M/E/H/m/s): empty match set ⇒ unsatisfiable.
/// Only an exclusion can empty `Y` (`Y!1:9999`); its domain is years 1-9999.
fn fixed_unsat(sel: &Selector) -> Option<Warning> {
    let (_, minv, maxv, _) = domain_info(sel.desig, Scope::Month);
    if matched_values(sel, minv, maxv).is_empty() {
        Some(Warning::new(
            sel.pos,
            "unsatisfiable — selector matches nothing in its domain",
        ))
    } else {
        None
    }
}

/// Day selector: unsatisfiable iff empty against every possible domain size.
fn day_unsat(
    sel: &Selector,
    expr: &Expr,
    scope: Scope,
    month_set: Option<&[i64]>,
    quarter_set: Option<&[i64]>,
) -> Option<Warning> {
    let years = if expr.has_week {
        None // W present ⇒ Y is the week-year; day-of-year is cross-selector → quiet.
    } else {
        enumerate_years(expr)
    };
    let sizes = day_domain_sizes(scope, month_set, quarter_set, years.as_deref());
    let satisfiable = sizes.iter().any(|&z| !matched_values(sel, 1, z).is_empty());
    if satisfiable {
        None
    } else {
        Some(Warning::new(
            sel.pos,
            "unsatisfiable — day never exists in the covered instances",
        ))
    }
}

fn day_domain_sizes(
    scope: Scope,
    month_set: Option<&[i64]>,
    quarter_set: Option<&[i64]>,
    years: Option<&[i64]>,
) -> Vec<i64> {
    let mut sizes = Vec::new();
    match scope {
        Scope::Month => {
            let months: Vec<i64> = month_set
                .map(|m| m.to_vec())
                .unwrap_or_else(|| (1..=12).collect());
            for m in months {
                match years {
                    Some(ys) => {
                        for &y in ys {
                            sizes.push(days_in_month(y, m));
                        }
                    }
                    None => {
                        sizes.push(days_in_month(2001, m)); // common year
                        sizes.push(days_in_month(2000, m)); // leap year
                    }
                }
            }
        }
        Scope::Quarter => {
            // A quarter scope is only chosen when a Q selector is present.
            let quarters: Vec<i64> = quarter_set
                .map(|q| q.to_vec())
                .expect("quarter scope implies a Q selector");
            for q in quarters {
                match years {
                    Some(ys) => {
                        for &y in ys {
                            sizes.push(days_in_quarter(y, q));
                        }
                    }
                    None => {
                        sizes.push(days_in_quarter(2001, q));
                        sizes.push(days_in_quarter(2000, q));
                    }
                }
            }
        }
        Scope::Year => match years {
            Some(ys) => {
                for &y in ys {
                    sizes.push(days_in_year(y));
                }
            }
            None => {
                sizes.push(365);
                sizes.push(366);
            }
        },
    }
    sizes
}

/// Week selector: unsatisfiable iff empty against every possible week count.
fn week_unsat(sel: &Selector, expr: &Expr) -> Option<Warning> {
    // When W is present, Y (if any) is the ISO week-year.
    let week_counts: Vec<i64> = match enumerate_years(expr) {
        Some(ys) => ys.iter().map(|&y| weeks_in_iso_year(y)).collect(),
        None => vec![52, 53],
    };
    let satisfiable = week_counts
        .iter()
        .any(|&z| !matched_values(sel, 1, z).is_empty());
    if satisfiable {
        None
    } else {
        Some(Warning::new(
            sel.pos,
            "unsatisfiable — week never exists in the covered week-years",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn year_domain_is_years_one_through_9999() {
        // The Year row of the domain table: 1-based, edges 1 and 9999 (spec §3.1).
        assert_eq!(
            domain_info(Desig::Year, Scope::Month),
            (false, 1, 9999, 9999)
        );
    }

    #[test]
    fn a_year_value_is_checked_as_it_is_read() {
        assert!(check_value(Desig::Year, 2020, 1).is_ok());
        assert_eq!(check_value(Desig::Year, 0, 1).unwrap_err().pos, 1);
        assert_eq!(check_value(Desig::Year, -1, 2).unwrap_err().pos, 2);
    }
}
