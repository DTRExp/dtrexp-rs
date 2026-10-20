//! DTRExp — Date-Time Range & Recurrence Expression (draft 2.9).
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
//! zone is a parameter; the default is the zone bound with
//! [`Dtrexp::with_zone`], else [`Tz::utc`].

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
    zone: Option<Bound>,
}

/// The evaluation zone bound by [`Dtrexp::with_zone`]: the name as given, and
/// the zone it loaded to at bind time. Under `host-tz` a [`Tz`] is not `Send`,
/// so the binding keeps only the name and loads it per call, as
/// [`Dtrexp::covers`] does for a name passed to it; `Dtrexp` stays `Send + Sync`.
#[derive(Clone, Debug)]
struct Bound {
    name: String,
    #[cfg(not(feature = "host-tz"))]
    tz: Tz,
}

impl Bound {
    fn new(name: &str, tz: Tz) -> Bound {
        #[cfg(feature = "host-tz")]
        drop(tz);
        Bound {
            name: name.to_string(),
            #[cfg(not(feature = "host-tz"))]
            tz,
        }
    }

    #[cfg(not(feature = "host-tz"))]
    fn covers(&self, dtr: &Dtrexp, instant_ms: i64) -> Result<bool, UnknownTimeZone> {
        Ok(dtr.covers_in(instant_ms, &self.tz))
    }

    #[cfg(feature = "host-tz")]
    fn covers(&self, dtr: &Dtrexp, instant_ms: i64) -> Result<bool, UnknownTimeZone> {
        Ok(dtr.covers_in(instant_ms, &Tz::load(&self.name)?))
    }
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
    Ok(Dtrexp {
        branches,
        warnings,
        zone: None,
    })
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

    /// A copy of this expression whose evaluation defaults to the IANA zone
    /// `tz_id`; `self` is left as it is. The zone is loaded once, here, so an
    /// unknown identifier fails at once with [`UnknownTimeZone`], and so does
    /// an empty one: binding is explicit and never quietly means UTC. The
    /// copy's warnings are the original's, since the zone belongs to
    /// evaluation, never to the expression (spec §1). A zone passed to
    /// [`Dtrexp::covers`] or [`Dtrexp::covers_in`] still wins.
    ///
    /// ```
    /// let hours = dtrexp::parse("T0900:1800 E1:5")?.with_zone("Europe/Berlin")?;
    /// assert_eq!(hours.zone(), Some("Europe/Berlin"));
    /// // 2026-07-07 (a Tuesday) 07:30:00Z is 09:30 in Berlin:
    /// assert!(hours.covers(1_783_409_400_000, "")?);
    /// assert!(!hours.covers(1_783_409_400_000, "UTC")?);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_zone(&self, tz_id: &str) -> Result<Dtrexp, UnknownTimeZone> {
        if tz_id.is_empty() {
            return Err(UnknownTimeZone::new(
                tz_id,
                "a time zone is required to bind",
            ));
        }
        let tz = Tz::load(tz_id)?;
        Ok(Dtrexp {
            zone: Some(Bound::new(tz_id, tz)),
            ..self.clone()
        })
    }

    /// The zone bound with [`Dtrexp::with_zone`], as it was given, or `None`
    /// when none is bound.
    pub fn zone(&self) -> Option<&str> {
        self.zone.as_ref().map(|b| b.name.as_str())
    }

    /// Does this expression cover `instant_ms` (milliseconds since the Unix
    /// epoch, UTC) when evaluated in the IANA zone `tz_id`? An empty
    /// identifier means no zone was passed: the bound zone
    /// ([`Dtrexp::with_zone`]), else UTC (the spec's default — §1). Any other
    /// identifier, `"UTC"` included, wins over the bound zone. Errors with
    /// [`UnknownTimeZone`] when the identifier does not resolve.
    pub fn covers(&self, instant_ms: i64, tz_id: &str) -> Result<bool, UnknownTimeZone> {
        if tz_id.is_empty() {
            if let Some(bound) = &self.zone {
                return bound.covers(self, instant_ms);
            }
        }
        let tz = Tz::load(tz_id)?;
        Ok(self.covers_in(instant_ms, &tz))
    }

    /// The preloaded-zone variant of [`Dtrexp::covers`] (API.md `covers in`):
    /// evaluate against an already-loaded [`Tz`], infallibly. The zone passed
    /// here always wins over a bound one.
    pub fn covers_in(&self, instant_ms: i64, tz: &Tz) -> bool {
        self.branches
            .iter()
            .any(|b| eval::expr_covers(b, instant_ms, tz))
    }
}
