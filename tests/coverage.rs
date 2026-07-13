//! Behavioural coverage for regions the vendored conformance vectors don't
//! reach: specific parse/validation error paths, satisfiability warnings, and
//! evaluation corners (negative weekday ordinals, weekly cadences). Every case
//! asserts the spec-level outcome, not merely that a line executed.

use dtrexp::{parse, validate, Tz};

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

// ---- parser error paths --------------------------------------------------

#[test]
fn selector_syntax_errors() {
    err("M!3/2"); // a component is either an exclusion or a stride, never both
    err("E1:3#2"); // ordinal '#' needs a bare weekday value, not a range
    err("M3,*"); // bare '*' inside a list
    err("M*:5/2"); // stride start must be an explicit value, not '*'
    err("M5:3/2"); // a wrap range takes no stride
    err("M10000001"); // integer too large
    err("M-"); // sign with no number
    err("M13"); // value out of domain
}

#[test]
fn time_component_errors() {
    err("T0900 T1000"); // duplicate T
    err("T*"); // T takes no '*'
    err("T"); // T requires a value
    err("T1"); // malformed (1 digit)
    err("T006000"); // minute 60 in a 6-digit value
    err("T000060"); // second 60 in a 6-digit value
    err("T120000.5"); // millisecond precision needs exactly 3 digits
}

#[test]
fn bounds_and_cadence_errors() {
    err("*"); // '*' only valid as a bounds start
    err("*5"); // '*' must be followed by ':'
    err("*:*"); // bounds need at least one date endpoint
    err("202001"); // date literal needs 8 digits
    err("20200101T000099"); // seconds out of range in date-literal time
    err("20200101/2X"); // unknown cadence unit
    err("20200101/2D/0D"); // explicit duration must be >= 1
}

#[test]
fn year_selector_errors() {
    const WRAP: &str = "Y range cannot wrap — no edge to wrap around";
    const DOMAIN: &str = "year out of domain (1-9999)";
    err_msg("Y2020:2010", WRAP); // Y range cannot wrap — no edge
    err_msg("Y2020:10000", DOMAIN); // year range end out of domain
    err_msg("Y0:2020", DOMAIN); // year range start out of domain
    // A descending range is caught as a wrap before either endpoint is
    // domain-checked, so it does not exercise the start-endpoint check.
    err_msg("Y10000:2020", WRAP);
    err("Y2000/1"); // stride interval must be >= 2
    err("Y2000/4/4"); // stride duration must be in 1..interval
    err("Y10000/4/2"); // stride start out of domain
    err("Y2000:10000/4/2"); // stride end out of domain
    err("E8#2"); // ordinal weekday out of domain (1-7)
}

#[test]
fn domain_bounds_on_ranges_and_strides() {
    // Valid endpoints keep both range and stride checks on their happy path.
    quiet("M3:7"); // range: both endpoints in domain
    quiet("H0:20/4"); // stride over a range: both endpoints in domain
    quiet("Y2000:3000/4/2"); // year stride: both endpoints in domain
    // Out-of-domain endpoints trip the per-endpoint checks.
    err("M0:7"); // range start is the forbidden zero
    err("M3:99"); // range end out of domain
    err("M0/4"); // stride start is the forbidden zero
    err("M3:99/4"); // stride end out of domain
}

#[test]
fn day_without_month_scope_uses_the_full_month_set() {
    // No Y/Q/M selector → default month scope with every month considered.
    quiet("D31 E1"); // the 31st exists in some month, so it is satisfiable
}

// ---- validation warnings -------------------------------------------------

#[test]
fn month_quarter_disjoint_warns() {
    // February is in Q1; asking for Q3 as well can never be satisfied.
    warns("M2 Q3");
}

#[test]
fn day_never_exists_warns() {
    warns("Y2020 M2 D30"); // Feb 2020 has 29 days
    warns("Y2020 Q1 D92"); // Q1 2020 has 91 days
    warns("Y2021 D366"); // 2021 is a common year (365 days)
}

#[test]
fn open_year_spans_stay_quiet() {
    quiet("M!3,5"); // exclusion value-list parses cleanly
    quiet("Y!2020 D31"); // excluded year → open set, day is satisfiable
    quiet("Y1:2000 D31"); // >1000-year range → not enumerated
    quiet("Y1,2000 D31"); // >1000-year span → not enumerated
    quiet("Y*:2020"); // open-start year range (a '*' endpoint, not a literal)
    quiet("M2 Q1"); // February is inside Q1 → satisfiable, no disjoint warning
}

#[test]
fn exclude_everything_is_unsatisfiable() {
    warns("M!*"); // excluding the whole domain leaves nothing
}

#[test]
fn duplicate_bounds_are_rejected() {
    err("*:20200301 *:20200401"); // two open-start bounds in one expression
}

// ---- evaluation ----------------------------------------------------------

#[test]
fn last_weekday_ordinal() {
    let expr = parse("E1#-1").unwrap(); // last Monday of the month
    let utc = Tz::utc();
    // July 2026 Mondays: 6, 13, 20, 27 → last is the 27th.
    assert!(expr.covers_in(utc_ms(2026, 7, 27, 12, 0, 0), &utc));
    assert!(!expr.covers_in(utc_ms(2026, 7, 20, 12, 0, 0), &utc));
    // A Tuesday is never a Monday-ordinal match.
    assert!(!expr.covers_in(utc_ms(2026, 7, 28, 12, 0, 0), &utc));
}

#[test]
fn year_scoped_last_weekday_ordinal() {
    // With a Year selector present the ordinal scope is the whole year.
    let expr = parse("Y2026 E1#-1").unwrap(); // last Monday of 2026
    let utc = Tz::utc();
    assert!(expr.covers_in(utc_ms(2026, 12, 28, 12, 0, 0), &utc)); // last Monday
    assert!(!expr.covers_in(utc_ms(2026, 12, 21, 12, 0, 0), &utc)); // Monday, not last
}

#[test]
fn quarter_scoped_last_weekday_ordinal() {
    // A Quarter selector scopes the ordinal to the quarter.
    let expr = parse("Q3 E1#-1").unwrap(); // last Monday of Q3
    let utc = Tz::utc();
    assert!(expr.covers_in(utc_ms(2026, 9, 28, 12, 0, 0), &utc)); // last Monday of Q3 2026
    assert!(!expr.covers_in(utc_ms(2026, 9, 21, 12, 0, 0), &utc)); // Monday, not last
}

#[test]
fn weekly_cadence_windows() {
    // Anchor Monday 2020-01-06, every 2 weeks, a 3-day window.
    let expr = parse("20200106/2W/3D").unwrap();
    let utc = Tz::utc();
    assert!(expr.covers_in(utc_ms(2020, 1, 6, 0, 0, 0), &utc)); // window start
    assert!(expr.covers_in(utc_ms(2020, 1, 8, 23, 0, 0), &utc)); // inside first window
    assert!(!expr.covers_in(utc_ms(2020, 1, 9, 0, 0, 0), &utc)); // window closed
    assert!(!expr.covers_in(utc_ms(2020, 1, 13, 0, 0, 0), &utc)); // off-week
    assert!(expr.covers_in(utc_ms(2020, 1, 20, 12, 0, 0), &utc)); // second window
    assert!(!expr.covers_in(utc_ms(2020, 1, 5, 0, 0, 0), &utc)); // before the anchor
}

#[test]
fn mixed_unit_cadences_parse() {
    // Duration unit differs from period unit → the length comparison runs.
    quiet("20200106/3W/10D"); // 10 days < 3 weeks
    quiet("20200101T0000/2H/30m"); // 30 minutes < 2 hours
}

#[test]
fn month_year_duration_requires_a_month_year_period() {
    // Period 100 days is *longer* than a 1-month duration, so only the
    // unit-kind rule (not the length rule) can reject this — pinning that a
    // month/year duration demands a month/year period.
    err("20200101/100D/1M");
    err("20200101/100D/1Y");
    // A month/year period accepts a month/year duration.
    quiet("20200101/2Y/1M"); // 1 month < 2 years
    quiet("20200101/3M/1M"); // 1 month < 3 months
}

// ---- public surface ------------------------------------------------------

#[test]
fn validate_helper_returns_warnings_and_errors() {
    assert!(!validate("M2 Q3").unwrap().is_empty());
    assert!(validate("M13").is_err());
}

#[test]
fn covers_resolves_named_zone() {
    let expr = parse("M7").unwrap();
    // 2026-07-15 12:00Z is in July.
    assert!(expr.covers(utc_ms(2026, 7, 15, 12, 0, 0), "UTC").unwrap());
    assert!(!expr.covers(utc_ms(2026, 3, 15, 12, 0, 0), "UTC").unwrap());
}
