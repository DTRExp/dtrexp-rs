//! `host-tz` feature: end-to-end evaluation through a host-supplied zone.

#![cfg(feature = "host-tz")]

use dtrexp::{parse, Tz};

#[test]
fn covers_evaluates_in_a_host_supplied_zone() {
    // A fixed +2h zone supplied by the host.
    let tz = Tz::from_offset_fn("Host/Plus2", |_| 7_200);
    let expr = parse("T0900:1800 E1:5").unwrap();
    // 2026-07-07 (Tue) 08:00:00Z = 10:00 local → covered.
    assert!(expr.covers_in(1_783_584_000_000, &tz));
    // 2026-07-07 17:00:00Z = 19:00 local → not covered.
    assert!(!expr.covers_in(1_783_616_400_000, &tz));
}

#[test]
fn cadence_anchors_resolve_through_the_host_zone() {
    // An H-period cadence anchors at local midnight, which §9.3 resolves via
    // the zone's wall-clock mapping — the host path's other operation.
    let tz = Tz::from_offset_fn("Host/Plus2", |_| 7_200);
    let expr = parse("20260707/2H/1H").unwrap();
    // Local 2026-07-07T00:00 (+2h) = 2026-07-06T22:00:00Z: the window opens.
    assert!(expr.covers_in(1_783_548_000_000, &tz));
    // One hour later the 1H duration has lapsed.
    assert!(!expr.covers_in(1_783_551_600_000, &tz));
}
