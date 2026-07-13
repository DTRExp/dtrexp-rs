//! Boundary and invariant pins driven by the mutation pass: each test kills at
//! least one surviving mutant by asserting the exact spec-level boundary the
//! mutant crossed (window edges, digit weights, domain limits, warning
//! positions). Helpers are duplicated from `coverage.rs` — integration tests
//! are separate crates, and a shared module would churn the verified coverage
//! suite for four small functions.

use dtrexp::{parse, Tz};

/// UTC instant (ms since the Unix epoch) for a civil date-time.
fn utc_ms(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> i64 {
    // Howard Hinnant's days-from-civil.
    let yy = y - i64::from(mo <= 2);
    let era = if yy >= 0 { yy } else { yy - 399 } / 400;
    let yoe = yy - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    days * 86_400_000 + h * 3_600_000 + mi * 60_000 + s * 1_000
}

fn err(expr: &str) {
    assert!(parse(expr).is_err(), "`{expr}` should be rejected");
}

/// Rejection *for a named reason* — an expression can be invalid on several
/// counts at once, and the earliest check wins.
fn err_msg(expr: &str, message: &str) {
    match parse(expr) {
        Ok(_) => panic!("`{expr}` should be rejected"),
        Err(e) => assert_eq!(e.message, message, "`{expr}` rejected for the wrong reason"),
    }
}

fn quiet(expr: &str) {
    match parse(expr) {
        Ok(p) => assert!(
            p.warnings().is_empty(),
            "`{expr}` should be quiet, warned: {:?}",
            p.warnings()
        ),
        Err(e) => panic!("`{expr}` should parse, errored: {e}"),
    }
}

fn warns(expr: &str) {
    match parse(expr) {
        Ok(p) => assert!(!p.warnings().is_empty(), "`{expr}` should warn"),
        Err(e) => panic!("`{expr}` should parse-with-warning, errored: {e}"),
    }
}

// ---- parser: guard messages come from the guard, not a downstream failure --

#[test]
fn guard_rejections_carry_their_own_message() {
    // Each input would also fail a later check; the named message pins that
    // the dedicated guard fires first (deleting it changes the reason).
    err_msg("M:5", "designator requires a value");
    err_msg("T!0900", "T takes no exclusion — write the complement explicitly");
    err_msg("T*", "T takes no '*'");
    err_msg("T", "T requires a value");
}

// ---- parser: time-value digit boundaries and weights -----------------------

#[test]
fn time_value_upper_bounds_are_inclusive() {
    quiet("T23"); // hour 23 is the last valid 2-digit value
    quiet("T2359"); // 23:59 is the last valid 4-digit value
    quiet("T235959"); // 23:59:59 is the last valid 6-digit value
    err("T2360"); // minute 60 in a 4-digit value
}

#[test]
fn four_digit_time_value_weights_its_minutes() {
    let expr = parse("T0130").unwrap();
    let utc = Tz::utc();
    assert!(expr.covers_in(utc_ms(2020, 1, 1, 1, 30, 30), &utc));
    assert!(!expr.covers_in(utc_ms(2020, 1, 1, 1, 0, 0), &utc));
}

#[test]
fn millisecond_time_value_weights_its_digits() {
    // A bare millisecond value covers exactly its 1 ms interval.
    let expr = parse("T120000.015").unwrap();
    let utc = Tz::utc();
    let noon = utc_ms(2020, 1, 1, 12, 0, 0);
    assert!(expr.covers_in(noon + 15, &utc));
    assert!(!expr.covers_in(noon + 5, &utc));
}

// ---- parser: date-literal digit weights and validity ------------------------

#[test]
fn date_literal_requires_eight_digits_not_eight_bytes() {
    err("2020010A"); // a non-digit inside the 8-digit window
}

#[test]
fn date_literal_year_digit_weights() {
    let utc = Tz::utc();
    // Nonzero hundreds digit: 1990 must not collapse toward year 190/1900.
    let expr = parse("19900101:*").unwrap();
    assert!(!expr.covers_in(utc_ms(1989, 6, 1, 0, 0, 0), &utc));
    assert!(expr.covers_in(utc_ms(1991, 6, 1, 0, 0, 0), &utc));
    // Nonzero tens digit: 2020 must not collapse toward year 2000.
    let expr = parse("20200101:*").unwrap();
    assert!(!expr.covers_in(utc_ms(2010, 6, 1, 0, 0, 0), &utc));
}

#[test]
fn date_literal_validity_checks_each_field() {
    err("20200230"); // day beyond the month length
    err("20201301"); // month out of domain
    err("20200100"); // day zero
}

#[test]
fn date_literal_time_part_boundaries() {
    quiet("20200101T2300:*"); // hour 23 is valid in a date-literal time-part
    err("20200101T2430:*"); // hour 24 is not (no '2400' form here)
    quiet("20200101T2359:*"); // minute 59 is valid
    err("20200101T1360:*"); // minute 60 is not
}

#[test]
fn date_literal_seconds_weight() {
    // A dated bound with seconds covers exactly its 1 s span.
    let expr = parse("20200101T120059").unwrap();
    let utc = Tz::utc();
    assert!(expr.covers_in(utc_ms(2020, 1, 1, 12, 0, 59), &utc));
    assert!(!expr.covers_in(utc_ms(2020, 1, 1, 12, 0, 50), &utc));
}

#[test]
fn bounds_start_honours_seconds() {
    let expr = parse("20200101T120030:*").unwrap();
    let utc = Tz::utc();
    assert!(!expr.covers_in(utc_ms(2020, 1, 1, 12, 0, 15), &utc));
    assert!(expr.covers_in(utc_ms(2020, 1, 1, 12, 0, 30), &utc)); // closed start
    assert!(expr.covers_in(utc_ms(2020, 1, 1, 12, 0, 45), &utc));
}

// ---- parser: stride and integer limits --------------------------------------

#[test]
fn a_degenerate_range_takes_a_stride() {
    // 3:3 is equal-endpoint, not a wrap — the wrap check is strictly `start > end`.
    quiet("M3:3/2");
}

#[test]
fn read_uint_accepts_exactly_ten_million() {
    // The overflow guard is strictly `> 10_000_000`.
    quiet("20200101/10000000m");
}

// ---- parser: cadence length table -------------------------------------------

#[test]
fn cadence_length_comparison_uses_exact_unit_lengths() {
    // Each rejection/acceptance flips if a `cad_lengths` constant degrades.
    err("20200101/25M/2Y"); // 2 × 366 d > 25 × 28 d — year duration too long
    err("20200101/1Y/12M"); // 12 × 31 d > 365 d — month duration too long
    err("20200101/1M/4W"); // 4 × 7 d = 28 × 1 d — week duration not shorter
    err("20200101/1M/28D"); // exactly equal is still rejected: strictly '<'
    quiet("20200101/1M/27D"); // one day under the conservative bound passes
}

// ---- eval: weekday ordinals and cadence window edges -------------------------

#[test]
fn the_seventh_can_be_the_first_weekday_of_its_kind() {
    // 2020-09-07 is a Monday and the 7th — the last day still in ordinal 1.
    let expr = parse("E1#1").unwrap();
    let utc = Tz::utc();
    assert!(expr.covers_in(utc_ms(2020, 9, 7, 12, 0, 0), &utc));
    assert!(!expr.covers_in(utc_ms(2020, 9, 14, 12, 0, 0), &utc)); // second Monday
}

#[test]
fn absolute_cadence_window_edges() {
    let expr = parse("20200601T0000/2H/1H").unwrap();
    let utc = Tz::utc();
    let anchor = utc_ms(2020, 6, 1, 0, 0, 0);
    assert!(expr.covers_in(anchor, &utc)); // the anchor instant itself is covered
    assert!(!expr.covers_in(anchor + 3_600_000, &utc)); // window end is exclusive
    assert!(!expr.covers_in(anchor - 7_200_000, &utc)); // a whole period early: still before the anchor
    assert!(expr.covers_in(anchor + 7_200_000, &utc)); // second occurrence
}

#[test]
fn monthly_cadence_with_a_mid_year_anchor() {
    // A June anchor keeps the month-difference arithmetic honest: any sign
    // slip in the anchor term shifts the occurrence estimate off the scan.
    let expr = parse("20200615/2M").unwrap();
    let utc = Tz::utc();
    assert!(expr.covers_in(utc_ms(2020, 8, 20, 12, 0, 0), &utc)); // second occurrence
    assert!(!expr.covers_in(utc_ms(2020, 7, 20, 12, 0, 0), &utc)); // off-month
}

// ---- validate: stride domain edges -------------------------------------------

#[test]
fn stride_interval_lower_and_upper_edges_are_legal() {
    quiet("Y2000:3000/2"); // interval 2 is the smallest legal stride
    quiet("M1/12"); // interval equal to the domain size is legal
}

// ---- validate: year enumeration and its 1000-year cutoff ----------------------

#[test]
fn enumerated_year_ranges_detect_unsatisfiable_days() {
    // Three common years: Feb 29 never exists — needs the range enumerated.
    warns("Y2001:2003 M2 D29");
}

#[test]
fn the_year_span_cutoff_is_strictly_over_1000() {
    // Span exactly 1000 (both common years): still enumerated → warns.
    warns("Y1001,2001 M2 D29");
    // Span 1001: not enumerated → the leap-year fallback stays satisfiable.
    quiet("Y1001,2002 M2 D29");
}

// ---- validate: warning selection and position ----------------------------------

#[test]
fn ordinal_weekday_selectors_never_warn() {
    // An ordinal E carries no atoms; running the empty-match check on it
    // would flag every `E1#2` as unsatisfiable.
    quiet("E1#2");
}

#[test]
fn exclude_everything_on_weekday_warns() {
    warns("E!*");
}

#[test]
fn quarter_mapping_uses_month_minus_one() {
    quiet("M3 Q1"); // March is the edge month of Q1: (3-1)/3+1 == 1
}

#[test]
fn disjoint_warning_points_at_the_month_selector() {
    let p = parse("Q3 M2").unwrap();
    let w = p.warnings();
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].pos, 3, "the warning should anchor to M, not Q");
}
