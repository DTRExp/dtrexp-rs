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
        Desig::Year => (false, 1, i64::MAX, i64::MAX),
    }
}

/// Run hard validation (errors) and collect warnings for one branch.
pub fn check(expr: &Expr) -> Result<Vec<Warning>, ParseError> {
    let scope = scope_of(expr);
    for sel in &expr.selectors {
        if sel.desig == Desig::Year {
            check_year(sel)?;
        } else {
            check_selector(sel, scope)?;
        }
    }
    Ok(warnings(expr, scope))
}

fn check_year(sel: &Selector) -> Result<(), ParseError> {
    if let Some((wd, _)) = sel.ordinal {
        // Ordinals are E-only; this is unreachable via the parser, but be safe.
        return Err(ParseError::new(
            sel.pos,
            format!("ordinal not valid here ({wd})"),
        ));
    }
    for atom in &sel.atoms {
        match *atom {
            Atom::All => {}
            Atom::Single(v) => year_value(sel.pos, v)?,
            Atom::Range { start, end, wrap } => {
                if wrap {
                    return Err(ParseError::new(
                        sel.pos,
                        "Y range cannot wrap — no edge to wrap around",
                    ));
                }
                if let Endpoint::Value(v) = start {
                    year_value(sel.pos, v)?;
                }
                if let Endpoint::Value(v) = end {
                    year_value(sel.pos, v)?;
                }
            }
            Atom::Stride {
                start,
                end,
                interval,
                duration,
            } => {
                year_value(sel.pos, start)?;
                if let Endpoint::Value(v) = end {
                    year_value(sel.pos, v)?;
                }
                if interval < 2 {
                    return Err(ParseError::new(sel.pos, "stride interval must be >= 2"));
                }
                if duration < 1 || duration >= interval {
                    return Err(ParseError::new(
                        sel.pos,
                        "stride duration must be in 1..interval",
                    ));
                }
            }
        }
    }
    Ok(())
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

fn check_selector(sel: &Selector, scope: Scope) -> Result<(), ParseError> {
    let (zero_based, _minv, maxv, size) = domain_info(sel.desig, scope);
    if let Some((wd, _)) = sel.ordinal {
        if !(1..=7).contains(&wd) {
            return Err(ParseError::new(sel.pos, "weekday out of domain (1-7)"));
        }
        return Ok(());
    }
    let check_val = |v: i64| -> Result<(), ParseError> {
        if v >= 0 {
            if !zero_based && v == 0 {
                return Err(ParseError::new(sel.pos, "zero value"));
            }
            if v > maxv {
                return Err(ParseError::new(sel.pos, "value out of domain"));
            }
        } else if v < -size {
            return Err(ParseError::new(
                sel.pos,
                "negative value out of domain (symmetric parse-time check)",
            ));
        }
        Ok(())
    };
    for atom in &sel.atoms {
        match *atom {
            Atom::All => {}
            Atom::Single(v) => check_val(v)?,
            Atom::Range { start, end, .. } => {
                if let Endpoint::Value(v) = start {
                    check_val(v)?;
                }
                if let Endpoint::Value(v) = end {
                    check_val(v)?;
                }
            }
            Atom::Stride {
                start,
                end,
                interval,
                duration,
            } => {
                check_val(start)?;
                if let Endpoint::Value(v) = end {
                    check_val(v)?;
                }
                if interval < 2 {
                    return Err(ParseError::new(sel.pos, "stride interval must be >= 2"));
                }
                if interval > size {
                    return Err(ParseError::new(
                        sel.pos,
                        "stride interval exceeds parent domain — use a cadence",
                    ));
                }
                if duration < 1 || duration >= interval {
                    return Err(ParseError::new(
                        sel.pos,
                        "stride duration must be in 1..interval",
                    ));
                }
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
/// `None` for open/unbounded/non-enumerable spans.
fn enumerate_years(expr: &Expr) -> Option<Vec<i64>> {
    let sel = expr.selectors.iter().find(|s| s.desig == Desig::Year)?;
    if sel.exclude {
        return None;
    }
    let mut years = Vec::new();
    for atom in &sel.atoms {
        match *atom {
            Atom::Single(v) if v > 0 => years.push(v),
            Atom::Range {
                start: Endpoint::Value(a),
                end: Endpoint::Value(b),
                wrap: false,
            } if a > 0 && b >= a => {
                if b - a > 1000 {
                    return None;
                }
                years.extend(a..=b);
            }
            _ => return None, // Star, All, stride, negative → open/unknown
        }
    }
    if years.is_empty() {
        return None;
    }
    let (lo, hi) = (*years.iter().min().unwrap(), *years.iter().max().unwrap());
    if hi - lo > 1000 {
        return None;
    }
    Some(years)
}

fn warnings(expr: &Expr, scope: Scope) -> Vec<Warning> {
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
            Desig::Year => {}
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
            let pos = expr
                .selectors
                .iter()
                .find(|s| s.desig == Desig::Month)
                .map(|s| s.pos)
                .unwrap_or(0);
            ws.push(Warning::new(
                pos,
                "unsatisfiable — M and Q select disjoint months",
            ));
        }
    }

    ws
}

/// Fixed-domain selectors (Q/M/E/H/m/s): empty match set ⇒ unsatisfiable.
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
            let quarters: Vec<i64> = quarter_set
                .map(|q| q.to_vec())
                .unwrap_or_else(|| (1..=4).collect());
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
