//! The integer limit (spec §8): every integer is at most 2147483647, leading
//! zeros unlimited, and a larger one is a positioned syntax error at that
//! integer. Every count up to the limit is legal where its rules allow it, so
//! evaluation meets the largest of them and must neither overflow nor panic.

use dtrexp::{parse, Tz};

/// UTC instant (ms since the Unix epoch) for a civil date-time.
fn utc_ms(y: i64, mo: i64, d: i64, h: i64) -> i64 {
    let yy = y - i64::from(mo <= 2);
    let era = if yy >= 0 { yy } else { yy - 399 } / 400;
    let yoe = yy - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) * 86_400_000 + h * 3_600_000
}

fn err_at(expr: &str, pos: usize) {
    match parse(expr) {
        Ok(_) => panic!("`{expr}` should be rejected"),
        Err(e) => {
            assert_eq!(
                e.message, "integer above the limit of 2147483647",
                "`{expr}`"
            );
            assert_eq!(e.pos, pos, "`{expr}` rejected at the wrong position");
        }
    }
}

#[test]
fn leading_zeros_are_unlimited() {
    let three = parse("M0000000000000000000003").unwrap();
    assert!(three.covers_in(utc_ms(2026, 3, 1, 0), &Tz::utc()));
    assert!(parse("Y1:*/000000002147483647").is_ok());
}

#[test]
fn a_value_over_the_limit_is_an_error_at_its_sign() {
    err_at("M2147483648", 1);
    err_at("M-2147483648", 1);
    err_at("M1:99999999999", 3);
    err_at("D1,-99999999999", 3);
}

#[test]
fn a_count_over_the_limit_is_an_error_at_the_count() {
    err_at("E7#-2147483648", 3);
    err_at("M1/2/2147483648", 5);
    err_at("20200101/1D/2147483648H", 12);
}

#[test]
fn year_strides_reach_the_limit() {
    let expr = parse("Y1:*/2147483647").unwrap();
    let utc = Tz::utc();
    assert!(expr.covers_in(utc_ms(1, 6, 1, 0), &utc));
    assert!(!expr.covers_in(utc_ms(2, 6, 1, 0), &utc));
    assert!(!expr.covers_in(utc_ms(9999, 12, 31, 23), &utc));
    let long = parse("Y5:*/2147483647/2147483646").unwrap();
    assert!(!long.covers_in(utc_ms(4, 12, 31, 23), &utc));
    assert!(long.covers_in(utc_ms(9999, 12, 31, 23), &utc));
}

#[test]
fn calendar_cadences_at_the_limit_evaluate_without_overflow() {
    let utc = Tz::utc();
    for (expr, inside) in [
        ("20200101/2147483647Y", false),
        ("20200101/2147483647Y/2147483646Y", true),
        ("20200101/2147483647Y/2147483647M", true),
        ("20200101/2147483647M/2147483646M", true),
        ("20200101/2147483647M/2147483647D", true),
        ("20200101/2147483647W/2147483646W", true),
        ("20200101/2147483647D/2147483646D", true),
        ("20200101/2147483647D/2147483647H", true),
    ] {
        let dtr = parse(expr).unwrap();
        assert!(
            !dtr.covers_in(utc_ms(2019, 12, 31, 23), &utc),
            "`{expr}` before the anchor"
        );
        assert!(
            dtr.covers_in(utc_ms(2020, 1, 1, 0), &utc),
            "`{expr}` at the anchor"
        );
        assert_eq!(
            dtr.covers_in(utc_ms(9999, 12, 31, 23), &utc),
            inside,
            "`{expr}` at the end of 9999"
        );
    }
    // A one-unit window inside a period at the limit: covered once, then never.
    let once = parse("20200101/2147483647Y").unwrap();
    assert!(once.covers_in(utc_ms(2020, 12, 31, 23), &utc));
    assert!(!once.covers_in(utc_ms(2021, 1, 1, 0), &utc));
    assert!(!once.covers_in(utc_ms(9999, 12, 31, 23), &utc));
}

#[test]
fn absolute_cadences_at_the_limit_evaluate_without_overflow() {
    let utc = Tz::utc();
    for expr in [
        "20200101/2147483647H/2147483646H",
        "20200101/2147483647H/89478485D",
        "20200101/2147483647m/2147483646m",
    ] {
        let dtr = parse(expr).unwrap();
        assert!(!dtr.covers_in(utc_ms(2019, 12, 31, 23), &utc), "`{expr}`");
        assert!(dtr.covers_in(utc_ms(2020, 1, 1, 0), &utc), "`{expr}`");
        assert!(dtr.covers_in(utc_ms(2020, 6, 1, 0), &utc), "`{expr}`");
    }
}
