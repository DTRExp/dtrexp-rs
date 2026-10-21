//! The typed runtime error: `UnknownTimeZone` from `Tz::load` and `covers`.

use dtrexp::{parse, Tz, UnknownTimeZone};

#[test]
fn load_unknown_zone_yields_typed_error() {
    let err = Tz::load("No/Such_Zone").unwrap_err();
    assert_eq!(err.id, "No/Such_Zone");
    assert_eq!(err.message, "not a zone or link name in the tz database");
    assert_eq!(
        err.to_string(),
        "unknown time zone No/Such_Zone: not a zone or link name in the tz database"
    );
}

#[test]
fn load_rejects_path_traversal_as_invalid_identifier() {
    for id in ["/etc/passwd", "../secrets", "Europe/../etc"] {
        let err = Tz::load(id).unwrap_err();
        assert_eq!(err.id, id);
        assert_eq!(err.message, "invalid identifier");
    }
}

/// The zone database holds plain-text files (`+VERSION`, `zone.tab`, …) beside
/// the binary zones. None of them is a zone name, so naming one is an unknown
/// zone, never a parse of the file.
#[test]
fn a_plain_text_entry_is_no_zone_name() {
    let entry = ["+VERSION", "zone.tab", "iso3166.tab", "tzdata.zi"]
        .into_iter()
        .find(|id| {
            ["/var/db/timezone/zoneinfo", "/usr/share/zoneinfo"]
                .iter()
                .any(|dir| std::path::Path::new(dir).join(id).is_file())
        })
        .expect("the system zone database should hold at least one plain-text entry");

    let err = Tz::load(entry).unwrap_err();
    assert_eq!(err.id, entry);
    assert_eq!(err.message, "not a zone or link name in the tz database");
}

#[test]
fn covers_propagates_the_typed_error() {
    let expr = parse("M3").unwrap();
    let err = expr.covers(0, "Not/A_Zone").unwrap_err();
    assert_eq!(
        err,
        UnknownTimeZone::new("Not/A_Zone", "not a zone or link name in the tz database")
    );
}

#[test]
fn unknown_time_zone_is_a_std_error() {
    let err: Box<dyn std::error::Error> = Box::new(Tz::load("No/Such_Zone").unwrap_err());
    assert!(err.to_string().contains("No/Such_Zone"));
}
