<p align="center"><img src="./.github/logo.svg" width="200" alt="dtrexp-rs" /></p>

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

Rust 2021 edition. No dependencies, including the zone handling; IANA zones are read straight from the system zoneinfo folder (`/var/db/timezone/zoneinfo` on macOS, else `/usr/share/zoneinfo`), and slim and fat zone files read alike. That folder is also where the zone rules come from: the answers follow the tz database release the system has installed, and pinning a release means pinning the system's tz data (spec §9).

## Usage

```rust
use dtrexp::{parse, Tz};

let dtr = parse("T0900:1800 E1:5").unwrap();   // business hours, Mon–Fri

// 2026-07-07 (a Tuesday) 10:00:00Z, in ms since the Unix epoch:
let t: i64 = 1_783_418_400_000;

let ok = dtr.covers(t, "Europe/Berlin").unwrap();
// —> true; a weekday, 09:00–18:00 Berlin local time.
// The zone is an evaluation parameter, never part of the expression;
// an empty identifier means the bound zone (below), else UTC.

// Preloaded zone; cannot fail:
let berlin = Tz::load("Europe/Berlin").unwrap();
let ok = dtr.covers_in(t, &berlin);

// Bound zone; the no-zone call evaluates in it:
let hours = dtr.with_zone("Europe/Berlin").unwrap();
let ok = hours.covers(t, "").unwrap();       // —> true; Berlin
let ok = hours.covers(t, "UTC").unwrap();    // a zone passed to the call wins
```

Instants are milliseconds since the Unix epoch (UTC); the time zone is passed at evaluation, and the default is the bound zone, else `Tz::utc()`. Note that you parse **once** (at write/config time) and evaluate **many**; a `Dtrexp` value is immutable after `parse`. `covers` is a single calendar-field extraction followed by integer comparisons; no occurrence iteration.

**Which to use:** bind the zone with `with_zone()` when it belongs to what the expression describes (a store's hours, a venue's schedule), and pass it per call when it belongs to whoever is asking (a rule shown to each user in their own zone). A bound copy loads its zone once, at bind (under the `host-tz` feature it keeps only the name and loads it per call); a cache of parsed expressions keys on the source string and the zone together.

## Host-Supplied Zones

Where there is no zoneinfo folder to read, as on WebAssembly, the `host-tz` feature lets the embedder supply the zone instead:

```toml
[dependencies]
dtrexp = { version = "1", features = ["host-tz"] }
```

`Tz::from_offset_fn(id, f)` builds a zone from a callback; `f` maps a UTC instant (ms since the Unix epoch) to the offset in effect there, in seconds east of UTC. In a JS host that is usually answered through `Intl`. Evaluate with `covers_in`:

```rust
use dtrexp::{parse, Tz};

let tz = Tz::from_offset_fn("Host/Plus2", |_ms| 7_200);   // a fixed +02:00 zone
let dtr = parse("T0900:1800 E1:5").unwrap();

let ok = dtr.covers_in(1_783_411_200_000, &tz);
// —> true; 2026-07-07 08:00Z is 10:00 on a Tuesday there.
```

Wall-clock times resolve as they do for a loaded zone; a skipped time moves forward past the gap, and a repeated one takes the earlier instant. Both come from `f` alone. A host-backed `Tz` is not `Send`; `Dtrexp` stays `Send` and `Sync`, because under this feature a zone bound with `with_zone` keeps only its name and is loaded per call. [dtrexp-wasm][wasm] is built on this feature.

## Errors and Warnings

Both a `ParseError` and a `Warning` carry a **position**: the 0-based offset of the first character of the token at fault, as spec §8 defines it. It counts code points and bytes alike, since every character before it is ASCII. When an expression breaks more than one rule, the error is the one with the earliest position, a later grammar error included (`M13/1` and `M13 X` both name the `13`); only a `D` value's domain waits for the branch to end, since it depends on scope selectors that may follow it (`D40 X` names the `X`).

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
- `with_zone(tz_id)` returns a copy whose evaluation defaults to `tz_id`, readable as `zone()` (`None` when unbound); the original is left as it is, and the copy's warnings are the same, since the zone belongs to evaluation, never to the expression. The zone is loaded once, at bind, so an unknown identifier fails there (under `host-tz` it is checked at bind and loaded again per call, from the zoneinfo filesystem; where there is none, as on WASM, evaluate a host-backed `Tz` with `covers_in` instead) with `UnknownTimeZone`; so does an empty one, because binding is explicit and never quietly means UTC.
- `covers` / `covers_in` take an IANA zone; on `covers`, an empty identifier means the bound zone, else UTC, and any other identifier (`"UTC"` included) wins over the bound zone. `covers_in`'s zone always wins. An identifier that does not resolve is the one runtime failure, `UnknownTimeZone { id, message }`; `covers_in` takes an already-loaded `Tz` and cannot fail.
- An integer in an expression is at most 2,147,483,647, however many leading zeros it carries (`M0003` is `M3`); a larger one is a parse error at that integer. Every count up to the limit evaluates, so `Y1:*/2147483647` and `20200101/2147483647Y` are legal expressions.
- Warnings are the spec's [§9.1](https://github.com/DTRExp/dtrexp/blob/main/spec.md#91-the-existence-rule) unsatisfiability lint: expressions that parse but can never match.

## Zone Identifiers

A zone identifier is a tz database zone or link name (`Europe/Berlin`, `US/Eastern`, `UTC`, `Etc/GMT+5`), matched without regard to case; the loaded `Tz`, and the zone a `with_zone` copy reports, carry the database's spelling (`"europe/berlin"` loads as `Europe/Berlin`). The names come from the zoneinfo folder's own list, its `tzdata.zi`, or a walk of the folder for zone files where it has none; so a name loads the same on a file system that ignores case and on one that does not. Anything else is an `UnknownTimeZone`: a fixed offset (`+02:00`, `UTC+2`), `Z`, a file in the folder that names no zone (`posixrules`, `Factory`, `localtime`, `zone.tab`), the `posix/`, `right/` and `SystemV/` trees, or a path. On `covers`, an empty identifier is no name; it means no zone was passed.

## Conformance & Quality

- Implements DTRExp **draft 3**. The test suite is driven by the shared [`vectors.json`][vectors] from the spec repo (the draft 3 file), vendored at `tests/vectors.json`: every coverage, rejection, warning and quiet vector, including the calendar traps (Feb 29 across 2000/2024/**2100**, `W53` existence, DST gap/overlap in `Europe/Berlin`), and every rejection at the position the vector gives. Run `cargo test`. See [VECTORS.md][vectors-md] for how the suite works.
- **Hand-rolled TZif reader.** `Tz::load` reads the system zoneinfo database directly; there is no time-zone crate underneath. A zone file's transitions answer up to the last one it lists, and its POSIX TZ string footer answers after that, so a slim file (the zic default since 2020) and a fat one give the same offsets, and dates past 2037 follow the zone's current rules.
- Zero dependencies.

## Related Projects

- [**dtrexp** (spec)][spec] — the DTRExp specification (grammar, semantics, conformance vectors) this package implements.
- [**dtrexp-js**][js] — the reference implementation; adds `intersect`, `next`, `describe`, `toRRule` and canonicalization.
- [**dtrexp-py**][py] · [**dtrexp-go**][go] · [**dtrexp-swift**][swift] · [**dtrexp-java**][java] — the other ports; same core interface.
- [**dtrexp-wasm**][wasm] — this crate compiled to WebAssembly for JS hosts.

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
[wasm]: https://github.com/DTRExp/dtrexp-wasm
