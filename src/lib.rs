//! DTRExp — Date-Time Range & Recurrence Expression (draft 2.8).
//!
//! A compact string expression denoting a possibly-infinite set of time
//! intervals, evaluated for **coverage** ("is this instant inside the set?").
//!
//! ```
//! use dtrexp::{parse, Tz};
//!
//! let expr = parse("T0900:1800 E1:5").unwrap();      // business hours
//! assert!(expr.warnings().is_empty());
//! // 2026-07-07 (a Tuesday) 10:00:00Z, in ms since the Unix epoch:
//! let t = 1_783_591_200_000;
//! assert!(expr.covers(t, "UTC").unwrap());
//! // or with a preloaded zone (infallible):
//! assert!(expr.covers_in(t, &Tz::utc()));
//! ```
//!
//! Instants are milliseconds since the Unix epoch (UTC). The evaluation time
//! zone is a parameter; the default is [`Tz::utc`].

mod ast;
mod civil;
mod error;
mod eval;
mod parser;
mod tz;
mod validate;

pub use error::{ParseError, UnknownTimeZone, Warning};
pub use tz::Tz;

/// A parsed, validated DTRExp.
#[derive(Clone, Debug)]
pub struct Dtrexp {
    branches: Vec<ast::Expr>,
    warnings: Vec<Warning>,
}

/// Parse and statically validate a DTRExp string.
///
/// Returns a positioned [`ParseError`] for syntactically or statically invalid
/// input. On success, any non-fatal diagnostics are available via
/// [`Dtrexp::warnings`].
pub fn parse(input: &str) -> Result<Dtrexp, ParseError> {
    let branches = parser::parse_branches(input)?;
    let mut warnings = Vec::new();
    for b in &branches {
        warnings.extend(validate::check(b)?);
    }
    Ok(Dtrexp { branches, warnings })
}

/// Parse and return only the warnings (errors are still returned as `Err`).
pub fn validate(input: &str) -> Result<Vec<Warning>, ParseError> {
    Ok(parse(input)?.warnings)
}

impl Dtrexp {
    /// Static-validation warnings (e.g. unsatisfiable selectors). Empty for a
    /// clean expression.
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// Does this expression cover `instant_ms` (milliseconds since the Unix
    /// epoch, UTC) when evaluated in the IANA zone `tz_id`? An empty
    /// identifier or `"UTC"` selects UTC (the spec's default — §1). Errors
    /// with [`UnknownTimeZone`] when the identifier does not resolve.
    pub fn covers(&self, instant_ms: i64, tz_id: &str) -> Result<bool, UnknownTimeZone> {
        let tz = Tz::load(tz_id)?;
        Ok(self.covers_in(instant_ms, &tz))
    }

    /// The preloaded-zone variant of [`Dtrexp::covers`] (API.md `covers in`):
    /// evaluate against an already-loaded [`Tz`], infallibly.
    pub fn covers_in(&self, instant_ms: i64, tz: &Tz) -> bool {
        self.branches
            .iter()
            .any(|b| eval::expr_covers(b, instant_ms, tz))
    }
}
