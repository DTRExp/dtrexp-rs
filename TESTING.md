# Testing

The crate is tested at three levels: the conformance vectors (`tests/vectors.json`, the behavioral contract), behavioural tests for everything the vectors don't reach (`tests/coverage.rs`, `tests/boundaries.rs`, `tests/limits.rs`, `tests/positions.rs`, `tests/tz_error.rs`, `tests/zone.rs`, and per-module `#[cfg(test)]` white-box units), and mutation testing with [cargo-mutants](https://github.com/sourcefrog/cargo-mutants).

The crate itself is zero-dependency. `cargo-llvm-cov` and `cargo-mutants` are developer tools, installed on the machine, never in `Cargo.toml`.

## Commands

```sh
cargo test                                    # vectors + behavioural + white-box
cargo test --features host-tz                 # + the host-supplied-zone backend (WASM seam)
cargo install cargo-llvm-cov cargo-mutants    # one-time (dev tools, not deps)
cargo llvm-cov --text                         # coverage, merged per line (see below)
cargo mutants --no-shuffle -j 2               # mutation testing
```

## Zone Fixtures

The zone tests never read the host's zoneinfo, which may be slim or fat. They read TZif files built with `zic` into `tests/fixtures/`: Berlin both slim and fat, slim Sydney, Santiago, Nuuk (a version-3 file with a negative rule time) and Riyadh (a footer with no DST), all from the 2026c `tzdata.zi` (`zic -b slim -d out tzdata.zi`, and `-b fat` for the fat Berlin), plus `berlin-rules-only.tzif` from `berlin-rules-only.zi` beside it. Two small zoneinfo folders test the name list that decides what is a zone: `zoneinfo-zi/` has a `tzdata.zi` naming zones, a link, `Factory`, a zone whose file is text and one with no file; `zoneinfo-walk/` has none, so it is walked, and holds the entries a walk must skip (`posixrules`, `Factory`, `localtime`, `posix/`, `right/`, `SystemV/`, plain-text files). Their zone files are copies of the slim Berlin. Footer rules no real zone uses (`Jn`, plain `n`, spills over a year's edge, zero-length and permanent DST) are tested on synthetic TZ strings in `src/tz/footer.rs`.

## Coverage: 100% of Lines and Regions, Measured Honestly

`cargo llvm-cov --text` shows **zero uncovered lines and zero uncovered regions** across `src/`, driven by real behavioural assertions, never line-touching.

The `--summary-only` table prints a slightly lower region figure (~99%). That residual is a measurement artifact, not a gap: the crate is instrumented twice — once with `cfg(test)` for the in-source white-box tests and once plain for the integration tests — and each compilation's unexercised copies of the *other* profile's records count against the merged summary. The `--text` report merges instantiations by line and is the source of truth; every region it marks is covered.

A few spots that would otherwise be unreachable defensive code were removed rather than excluded, because reachable code is the only code worth covering:

- The Year selector shares the generic selector path (its validated 1..9999 domain makes the generic integer match behave identically), which deleted a bespoke `set_covers_year` and an `unreachable!` arm.
- All cadence occurrence arithmetic funnels through one total `add_units` / `elapsed_units` pair over `CadUnit`, so the period/duration math carries no dead match arms.
- The `parse_expression` contract (it only yields at `|` or end-of-input) makes the top-level "trailing input" guard unreachable; it was removed. Likewise the re-validation of a cadence anchor that the date literal's own reading already validated, and a missing-literal error where the literal is known to start with a digit (`scan_date` reads it; `expect_date` is the fallible one, for a bounds end).
- Three genuinely-total helpers keep their guarantee expressed as `.unwrap()`/`.expect()` on an infallible result (a value list's first item, never a bare `*`; a quarter-scoped day domain; a spring-forward gap that provably exists), so the impossible branch lives in `std`, not in this crate.

## Mutation Testing

Full run: **1359 mutants: 1305 caught, 15 equivalent, 39 unviable, 0 unjustified survivors**, as of the run after `with_zone`; the TZ string footer was run on its own after that (`cargo mutants -f src/tz.rs -f src/tz/footer.rs`, 348 of the 1359, in both builds since cargo-mutants mutates `cfg`-gated code whatever the features; each build misses only the other build's code).

Draft 3 (2026-10-21) was run on the lines it changed, in both builds: `git diff -- src > src.diff`, then `cargo mutants --no-shuffle -j 2 --in-diff src.diff -f src/parser.rs -f src/validate.rs -f src/eval.rs -f src/tz.rs -f src/lib.rs -f src/ast.rs`, and again with `--features host-tz`. Each build: **260 mutants: 235 caught, 3 timeouts, 21 unviable, 1 equivalent** (the `eval.rs:278` row below). A whole-file run of the six changed files is 956 mutants, about 80 minutes per build at 10 s a mutant, so the changed lines are the scope. The draft 3 rewrite of the year enumeration removed seven equivalents of the old table (the `validate.rs` guards it no longer has, and the 1000-year cutoff, which a strided span now makes observable), and stopping the occurrence scan at its estimate removed the high-bound one; the table holds 7.

Checking a selector's values as it is read, so that they compete with a later grammar error (spec §8), was run the same way on its own lines: **12 mutants: 9 caught, 3 unviable** in each build, no survivors.

Deciding every rule but `D`'s at its token (a value, a stride interval, a bare `*` in a list, a malformed date literal, time value or fraction), keeping the warnings off a branch that broke a rule, the designator set that replaced the rescan for repeats, the TZif checks (type indices, ascending times, 64-bit block sizes, saturated gap arithmetic) and the timeline guard on `covers` were run on the functions they touched, in both builds. With no git history to diff against, the diff names each changed function whole (its lines as added), so the run reaches past the edited lines: **370 mutants: 341 caught, 4 timeouts, 21 unviable, 4 missed** in each build. Three of the missed are the `parser.rs:172` and two `tz.rs:216` rows below; the fourth, a one-year `Y` range read as a wrap, is killed by a test added after the run (`Y2020:2020`, rechecked in both builds). The four timeouts are a repeat that goes unseen (the 200,000-selector test then reaches the warnings) and a digit reader that never advances.

Nine of the caught are timeout kills: non-termination mutants (`skip_spaces` looping forever, parser stubs that never consume input, a deleted match arm that sends a domain scan to `i64::MAX`, a footer cursor whose position never advances). A suite that can never pass has detected the mutant; timeouts count as kills, as cargo-mutants and Stryker both classify them.

Unviable mutants are compile errors (`Default::default()` stubs for types without `Default`): nothing behavioural to test.

Every other mutant changes observable behaviour and is killed by a test. The survivors below are behaviourally equivalent to the original; each carries its proof, because "no test could distinguish it" is a claim that has to be earned per mutant.

### Justified Equivalent Survivors (7)

cargo-mutants has no inline suppression mechanism, so equivalents are documented here.

| Mutant | Site | Why equivalent |
| --- | --- | --- |
| `eval.rs:209` | `ord > 0` → `>=` | Ordinal `0` is a parse error ("ordinal out of range"); the operators differ only at 0. |
| `eval.rs:278` | scan low bound `k_est - 2` → `k_est / 2` | The occurrence holding the instant starts at or before it, so its index `k` is at most the estimate; and `k_est ≤ k + 1` (exact units: equality; calendar units: `constrain` clamps the day, never the month count, and a duration `< period` spans at most one extra boundary). So `k_est - 1 ≤ k ≤ k_est`, and `k_est / 2 ≤ k_est - 1` for every `k_est ≥ 2`; below 2 both bounds are 0 after `.max(0)`. Extra scanned windows sit more than a period away from the instant and cannot contain it. |
| `parser.rs:172` | `n += 1` → `-=` | `n`'s only read is `n == 0` (empty-expression check); zero iterations leave 0 either way, one or more leave nonzero either way. |
| `parser.rs:505` | T-range `wrap: start > end` → `>=` | Equal endpoints break a rule two lines earlier ("covers nothing"), so such a parse never succeeds; the operators agree on everything else. |
| `tz.rs:216` | gap `off_after > off_before` → `>=` | The extra matches are no-op transitions, whose gap window `[x, x)` contains no instant. |
| `tz.rs:216` | gap `local < gap_end` → `<=` | At `local == gap_end` the candidate `local − off_after` is exactly the transition instant, whose offset lookup (inclusive) validates it; such an input resolves on the valid-offset path and never reaches the gap search. For the last transition the lookup at that instant is the footer's, which RFC 8536 §3.3 requires to agree with the last transition's type, so the argument holds for every file that standard accepts. |
