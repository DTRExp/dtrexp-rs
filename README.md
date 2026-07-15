<p align="center"><picture><source media="(prefers-color-scheme: dark)" srcset="./.github/logo-dark.svg"><img src="./.github/logo.svg" width="200" alt="dtrexp-rs" /></picture></p>

# dtrexp-rs

Rust implementation of **[DTRExp](https://github.com/DTRExp/dtrexp)** (read: "**DTR Expression**") — a compact string expression for date-time ranges and recurrence, evaluated by **coverage** rather than enumeration.

```
T0900:1800 E1:5          Mon–Fri, 09:00–18:00
E7#-1 M4                 last Sunday of April, every year
20200106/10D             every 10 days from 2020-01-06 (cron can't say this)
M!7                      every month except July
```

Scope: **parsing, validation and coverage evaluation** (the spec's core interface). Rendering, description and RRULE export are out of scope; the [reference implementation][js] has them.

## Install

```sh
cargo add dtrexp
```

Rust 2021 edition. No dependencies, including the zone handling; IANA zones are read straight from the system TZif database.

## Usage

```rust
use dtrexp::{parse, Tz};

let dtr = parse("T0900:1800 E1:5").unwrap();   // business hours, Mon–Fri

// 2026-07-07 (a Tuesday) 10:00:00Z, in ms since the Unix epoch:
let t: i64 = 1_783_418_400_000;

let ok = dtr.covers(t, "Europe/Berlin").unwrap();
// —> true; a weekday, 09:00–18:00 Berlin local time.
// The zone is an evaluation parameter, never part of the expression;
// an empty identifier or "UTC" means UTC.

// Preloaded zone; cannot fail:
let berlin = Tz::load("Europe/Berlin").unwrap();
let ok = dtr.covers_in(t, &berlin);
```

Instants are milliseconds since the Unix epoch (UTC); the time zone is passed at evaluation, and the default is `Tz::utc()`. Note that you parse **once** (at write/config time) and evaluate **many**; a `Dtrexp` value is immutable after `parse`. `covers` is a single calendar-field extraction followed by integer comparisons; no occurrence iteration.

## Errors and Warnings

Both a `ParseError` and a `Warning` carry a **position**; the byte offset into the source where the problem was detected.

```rust
let err = dtrexp::parse("T2500").unwrap_err();      // hour out of range
err.pos;        // 1 — points at the offending token
err.message;    // "hour out of range — 24 exists only as the exact token '2400'"

let warnings = dtrexp::validate("D30 M2").unwrap();  // parses cleanly
warnings[0].pos;      // 0
warnings[0].message;  // "unsatisfiable — day never exists …"  (no February has 30 days)
```

- `parse(s)` returns the expression or a `ParseError { pos, message }`. Warnings from a clean parse are available via `Dtrexp::warnings()`.
- `validate(s)` returns `Result<Vec<Warning>, ParseError>`: the warnings on success, or the same `ParseError` when the input does not parse. It carries the same warnings as `Dtrexp::warnings()`.
- `covers` / `covers_in` take an IANA zone. An identifier that does not resolve is the one runtime failure, `UnknownTimeZone { id, message }`; `covers_in` takes an already-loaded `Tz` and cannot fail.
- Warnings are the spec's [§9.1](https://github.com/DTRExp/dtrexp/blob/main/spec.md#91-the-existence-rule) unsatisfiability lint: expressions that parse but can never match.

## Conformance & Quality

- The test suite is driven by the shared [`vectors.json`][vectors] from the spec repo (draft 2.8), vendored at `tests/vectors.json`: every coverage, rejection, warning and quiet vector, including the calendar traps (Feb 29 across 2000/2024/**2100**, `W53` existence, DST gap/overlap in `Europe/Berlin`). Run `cargo test`. See [VECTORS.md][vectors-md] for how the suite works.
- **Hand-rolled TZif reader.** `Tz::load` reads the system zoneinfo database directly; there is no time-zone crate underneath.
- Zero dependencies.

## Related Projects

- [**dtrexp** (spec)][spec] — the DTRExp specification (grammar, semantics, conformance vectors) this package implements.
- [**dtrexp-js**][js] — the reference implementation; adds `intersect`, `next`, `describe`, `toRRule` and canonicalization.
- [**dtrexp-py**][py] · [**dtrexp-go**][go] · [**dtrexp-swift**][swift] · [**dtrexp-java**][java] — the other ports; same core interface.

## License

© 2026, Onur Yıldırım. [**MIT**](LICENSE) License.

[spec]: https://github.com/DTRExp/dtrexp
[js]: https://github.com/DTRExp/dtrexp-js
[py]: https://github.com/DTRExp/dtrexp-py
[go]: https://github.com/DTRExp/dtrexp-go
[swift]: https://github.com/DTRExp/dtrexp-swift
[java]: https://github.com/DTRExp/dtrexp-java
[vectors]: https://github.com/DTRExp/dtrexp/blob/main/vectors.json
[vectors-md]: https://github.com/DTRExp/dtrexp/blob/main/VECTORS.md
