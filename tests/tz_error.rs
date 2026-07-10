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
