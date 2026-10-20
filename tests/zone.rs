//! `with_zone`: a copy of the expression whose evaluation defaults to a bound
//! zone. Tuesday 2026-07-07 07:30:00Z is 09:30 in Berlin (CEST): inside
//! 09:00–18:00 there, outside it in UTC.

use dtrexp::{parse, Tz, UnknownTimeZone};

const HOURS: &str = "T0900:1800 E1:5";
/// 2026-07-07T07:30:00Z, in ms since the Unix epoch.
const T: i64 = 1_783_409_400_000;
/// 2026-07-07T09:30:00Z: 11:30 in Berlin, 18:30 in Tokyo.
const T2: i64 = 1_783_416_600_000;

#[test]
fn returns_a_copy_and_leaves_the_original_unbound() {
    let dtr = parse(HOURS).unwrap();
    let berlin = dtr.with_zone("Europe/Berlin").unwrap();
    assert_eq!(berlin.zone(), Some("Europe/Berlin"));
    assert_eq!(dtr.zone(), None);
    assert!(!dtr.covers(T, "").unwrap());
}

#[test]
fn the_no_zone_call_evaluates_in_the_bound_zone() {
    let berlin = parse(HOURS).unwrap().with_zone("Europe/Berlin").unwrap();
    assert!(berlin.covers(T, "").unwrap());
    assert!(!berlin.covers(T - 3_600_000, "").unwrap()); // 08:30 Berlin
}

#[test]
fn a_zone_passed_to_a_call_wins_over_the_bound_zone() {
    let dtr = parse(HOURS).unwrap();
    let berlin = dtr.with_zone("Europe/Berlin").unwrap();
    assert!(!berlin.covers(T, "UTC").unwrap());
    assert!(!berlin.covers_in(T, &Tz::utc()));
    let utc = dtr.with_zone("UTC").unwrap();
    assert!(!utc.covers(T, "").unwrap());
    assert!(utc.covers(T, "Europe/Berlin").unwrap());
    assert!(utc.covers_in(T, &Tz::load("Europe/Berlin").unwrap()));
}

#[test]
fn a_call_on_a_bound_copy_still_rejects_an_unknown_zone() {
    let berlin = parse(HOURS).unwrap().with_zone("Europe/Berlin").unwrap();
    let err = berlin.covers(T, "Mars/Olympus").unwrap_err();
    assert_eq!(
        err,
        UnknownTimeZone::new("Mars/Olympus", "no TZif entry in the system database")
    );
}

#[test]
fn rebinds_a_bound_copy() {
    let berlin = parse(HOURS).unwrap().with_zone("Europe/Berlin").unwrap();
    let tokyo = berlin.with_zone("Asia/Tokyo").unwrap();
    assert_eq!(tokyo.zone(), Some("Asia/Tokyo"));
    assert!(!tokyo.covers(T2, "").unwrap());
    assert!(berlin.covers(T2, "").unwrap());
    assert_eq!(berlin.zone(), Some("Europe/Berlin"));
}

#[test]
fn keeps_the_zone_name_as_given() {
    let dtr = parse(HOURS).unwrap();
    assert_eq!(dtr.with_zone("Etc/UTC").unwrap().zone(), Some("Etc/UTC"));
}

#[test]
fn fails_at_once_on_an_unknown_zone() {
    let err = parse(HOURS).unwrap().with_zone("Mars/Olympus").unwrap_err();
    assert_eq!(
        err,
        UnknownTimeZone::new("Mars/Olympus", "no TZif entry in the system database")
    );
}

#[test]
fn refuses_a_missing_zone_instead_of_binding_utc_or_the_host_zone() {
    let dtr = parse(HOURS).unwrap();
    let err = dtr.with_zone("").unwrap_err();
    assert_eq!(
        err,
        UnknownTimeZone::new("", "a time zone is required to bind")
    );
    for host in ["Local", "localtime"] {
        assert!(
            dtr.with_zone(host).is_err(),
            "`{host}` should not bind the host zone"
        );
    }
}

#[test]
fn leaves_the_warnings_unchanged() {
    let lint = parse("D30 M2").unwrap();
    let bound = lint.with_zone("Europe/Berlin").unwrap();
    assert!(!lint.warnings().is_empty());
    assert_eq!(bound.warnings(), lint.warnings());
}

#[test]
fn refuses_localtime_in_a_call_too() {
    // Debian and Ubuntu link zoneinfo's `localtime` to the host's zone.
    let dtr = parse("T0900:1800").unwrap();
    let err = dtr.covers(0, "localtime").unwrap_err();
    assert_eq!(
        err,
        UnknownTimeZone::new("localtime", "the host's zone is not an IANA identifier")
    );
}

#[test]
fn refuses_a_path_that_reaches_localtime() {
    let dtr = parse("T0900:1800").unwrap();
    for id in [
        "./localtime",
        "Europe/./Berlin",
        "Europe//Berlin",
        "Europe/",
    ] {
        assert_eq!(
            dtr.with_zone(id).unwrap_err(),
            UnknownTimeZone::new(id, "invalid identifier")
        );
    }
    assert_eq!(
        dtr.with_zone("posix/localtime").unwrap_err(),
        UnknownTimeZone::new(
            "posix/localtime",
            "the host's zone is not an IANA identifier"
        )
    );
}

#[test]
fn stays_send_and_sync() {
    fn shareable<T: Send + Sync>() {}
    shareable::<dtrexp::Dtrexp>();
}
