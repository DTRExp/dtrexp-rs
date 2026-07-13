//! The typed runtime error: `UnknownTimeZone` from `Tz::load` and `covers`.

use dtrexp::{parse, Tz, UnknownTimeZone};

#[test]
fn load_unknown_zone_yields_typed_error() {
    let err = Tz::load("No/Such_Zone").unwrap_err();
    assert_eq!(err.id, "No/Such_Zone");
    assert_eq!(err.message, "no TZif entry in the system database");
    assert_eq!(
        err.to_string(),
        "unknown time zone No/Such_Zone: no TZif entry in the system database"
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
/// the binary zones. Naming one resolves to a readable file that is not TZif —
/// the parse failure has to surface as the typed error, not a panic.
#[test]
fn a_readable_non_tzif_entry_yields_a_parse_error() {
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
    assert_eq!(err.message, "not a TZif file");
}

#[test]
fn covers_propagates_the_typed_error() {
    let expr = parse("M3").unwrap();
    let err = expr.covers(0, "Not/A_Zone").unwrap_err();
    assert_eq!(err, UnknownTimeZone::new("Not/A_Zone", "no TZif entry in the system database"));
}

#[test]
fn unknown_time_zone_is_a_std_error() {
    let err: Box<dyn std::error::Error> = Box::new(Tz::load("No/Such_Zone").unwrap_err());
    assert!(err.to_string().contains("No/Such_Zone"));
}
