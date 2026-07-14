# Testing

The crate is tested at three levels: the conformance vectors (`tests/vectors.json`, the behavioral contract), behavioural tests for everything the vectors don't reach (`tests/coverage.rs`, `tests/boundaries.rs`, `tests/tz_error.rs`, and per-module `#[cfg(test)]` white-box units), and mutation testing with [cargo-mutants](https://github.com/sourcefrog/cargo-mutants).

The crate itself is zero-dependency. `cargo-llvm-cov` and `cargo-mutants` are developer tools, installed on the machine, never in `Cargo.toml`.

## Commands

```sh
cargo test                                    # vectors + behavioural + white-box
cargo install cargo-llvm-cov cargo-mutants    # one-time (dev tools, not deps)
cargo llvm-cov --text                         # coverage, merged per line (see below)
cargo mutants --no-shuffle -j 2               # mutation testing
```

## Coverage: 100% of Lines and Regions, Measured Honestly

`cargo llvm-cov --text` shows **zero uncovered lines and zero uncovered regions** across `src/`, driven by real behavioural assertions, never line-touching.

The `--summary-only` table prints a slightly lower region figure (~99%). That residual is a measurement artifact, not a gap: the crate is instrumented twice — once with `cfg(test)` for the in-source white-box tests and once plain for the integration tests — and each compilation's unexercised copies of the *other* profile's records count against the merged summary. The `--text` report merges instantiations by line and is the source of truth; every region it marks is covered.

A few spots that would otherwise be unreachable defensive code were removed rather than excluded, because reachable code is the only code worth covering:

- The Year selector shares the generic selector path (its validated 1..9999 domain makes the generic integer match behave identically), which deleted a bespoke `set_covers_year` and an `unreachable!` arm.
- All cadence occurrence arithmetic funnels through one total `add_units` / `elapsed_units` pair over `CadUnit`, so the period/duration math carries no dead match arms.
- The `parse_expression` contract (it only yields at `|` or end-of-input) makes the top-level "trailing input" guard unreachable; it was removed. Likewise the re-validation of a cadence anchor that `parse_date` already validated.
- Three genuinely-total helpers keep their guarantee expressed as `.unwrap()`/`.expect()` on an infallible result (a single-item value list, a quarter-scoped day domain, a spring-forward gap that provably exists), so the impossible branch lives in `std`, not in this crate.

## Mutation Testing

Full run: **1215 mutants: 1198 caught, 17 equivalent (justified below), 35 unviable, 0 unjustified survivors.**

Eight of the caught are timeout kills: non-termination mutants (`skip_spaces` looping forever, parser stubs that never consume input, a deleted match arm that sends a domain scan to `i64::MAX`). A suite that can never pass has detected the mutant; timeouts count as kills, as cargo-mutants and Stryker both classify them.

Unviable mutants are compile errors (`Default::default()` stubs for types without `Default`): nothing behavioural to test.

Every other mutant changes observable behaviour and is killed by a test. The survivors below are behaviourally equivalent to the original; each carries its proof, because "no test could distinguish it" is a claim that has to be earned per mutant.

### Justified Equivalent Survivors (17)

cargo-mutants has no inline suppression mechanism, so equivalents are documented here.

| Mutant | Site | Why equivalent |
| --- | --- | --- |
| `eval.rs:195` | `ord > 0` → `>=` | Ordinal `0` is a parse error ("ordinal out of range"); the operators differ only at 0. |
| `eval.rs:261` | scan low bound `k_est - 2` → `k_est / 2` | The occurrence estimate satisfies `k ≤ k_est ≤ k + 1` (exact units: equality; calendar units: `constrain` clamps the day, never the month count, and a duration `< period` spans at most one extra boundary). The bounds differ observably only where a true occurrence would need estimate error ≥ 2, impossible. Extra scanned windows sit more than a period away from the instant and cannot contain it. |
| `eval.rs:261` | scan high bound `k_est + 2` → `k_est * 2` | Same bound: a shrunken high end (only at `k_est < 2`) would need a true `k > k_est`, impossible by `k ≤ k_est`. |
| `parser.rs:120` | `n += 1` → `-=` | `n`'s only read is `n == 0` (empty-expression check); zero iterations leave 0 either way, one or more leave nonzero either way. |
| `parser.rs:416` | T-range `wrap: start > end` → `>=` | Equal endpoints are rejected two lines earlier ("covers nothing"); the operators agree on everything else. |
| `tz.rs:118` | gap `off_after > off_before` → `>=` | The extra matches are no-op transitions, whose gap window `[x, x)` contains no instant. |
| `tz.rs:118` | gap `local < gap_end` → `<=` | At `local == gap_end` the candidate `local − off_after` is exactly the transition instant, whose offset lookup (inclusive) validates it; such an input resolves on the valid-offset path and never reaches the gap search. |
| `tz.rs:118` | first `&&` → `\|\|` | The gap search runs only when no offset validates the input. Divergence needs `local` below a spring-forward window at some index while inside a later gap; validity-emptiness forces `local` below the previous transition's window too, and the descent bottoms out at index 0, where "below the window" means the (unbounded-below) first-offset strip validates `local`, contradicting that the search ran. |
| `tz.rs:118` | second `&&` → `\|\|` | The mutant would return at the first index with `local < gap_end` alone; minimality of that index plus validity-emptiness squeezes `local` into the very offset strip between the previous window's end and this window, making it valid; same contradiction. |
| `validate.rs:219` | `Single(v) if v > 0` → `if true` | `check_year` rejects `v ≤ 0` before warnings run; the guard is already always true on reachable input. |
| `validate.rs:219` | `v > 0` → `>=` | Differs only at `v == 0`, rejected by the year-domain check ("year out of domain") first. |
| `validate.rs:224` | range guard → `true` | Non-positive endpoints and descending (wrap) ranges are rejected by `check_year` before warnings; every reaching range already has `a > 0 ∧ b ≥ a`. |
| `validate.rs:224` | `a > 0 && b >= a` → `\|\|` | Both conjuncts are true on reachable input (previous row); `∧`/`∨` agree on (true, true). |
| `validate.rs:224` | `a > 0` → `>=` | Differs only at `a == 0`, unreachable (same rejection). |
| `validate.rs:225` | span `> 1000` → `==` | At span exactly 1000: any 1001 consecutive years contain leap, common, 52- and 53-week years, so the enumerated domain sizes equal the fallback sets and warnings are identical (warnings are the only consumer). Above 1000: the `hi − lo > 1000` backstop below returns `None` regardless. |
| `validate.rs:225` | span `> 1000` → `>=` | Differs only at span exactly 1000; identical warnings by the 1001-consecutive-years argument. |
| `validate.rs:225` | `b - a` → `b / a` | `b/a > 1000` with `b − a ≤ 1000` forces `a = 1, b = 1001` (span exactly 1000: previous row); the reverse direction is caught by the `hi − lo` backstop. Enumeration stays bounded by the 9999-year domain either way. |
