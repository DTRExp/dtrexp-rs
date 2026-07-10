//! Conformance suite: runs the vendored `vectors.json` (spec §12).
//!
//! `vectors.json` — not the prose — is the contract. Every `invalid` expression
//! must be rejected at parse, every `warnings` expression accepted with ≥1
//! warning, every `quiet` expression accepted with zero warnings, and every
//! instant in every `coverage` group must return the expected boolean.

use dtrexp::{parse, Tz};

use json::Json;

const VECTORS: &str = include_str!("vectors.json");

// ---- minimal std-only JSON reader ---------------------------------------

mod json {
    use std::collections::BTreeMap;

    #[derive(Debug, Clone)]
    pub enum Json {
        Null,
        Bool(bool),
        // Numbers are parsed but unused by the suite (spec version, etc.).
        Num(#[allow(dead_code)] f64),
        Str(String),
        Arr(Vec<Json>),
        Obj(BTreeMap<String, Json>),
    }

    impl Json {
        pub fn get(&self, key: &str) -> &Json {
            match self {
                Json::Obj(m) => m.get(key).unwrap_or(&Json::Null),
                _ => &Json::Null,
            }
        }
        pub fn as_str(&self) -> &str {
            match self {
                Json::Str(s) => s,
                _ => panic!("expected string, got {self:?}"),
            }
        }
        pub fn as_arr(&self) -> &[Json] {
            match self {
                Json::Arr(a) => a,
                _ => panic!("expected array"),
            }
        }
        pub fn as_bool(&self) -> bool {
            match self {
                Json::Bool(b) => *b,
                _ => panic!("expected bool"),
            }
        }
        pub fn as_obj(&self) -> &BTreeMap<String, Json> {
            match self {
                Json::Obj(m) => m,
                _ => panic!("expected object"),
            }
        }
    }

    pub fn parse(s: &str) -> Json {
        let mut p = P {
            b: s.as_bytes(),
            i: 0,
        };
        p.ws();
        let v = p.value();
        p.ws();
        v
    }

    struct P<'a> {
        b: &'a [u8],
        i: usize,
    }

    impl P<'_> {
        fn ws(&mut self) {
            while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
                self.i += 1;
            }
        }
        fn value(&mut self) -> Json {
            self.ws();
            match self.b[self.i] {
                b'{' => self.object(),
                b'[' => self.array(),
                b'"' => Json::Str(self.string()),
                b't' => {
                    self.i += 4;
                    Json::Bool(true)
                }
                b'f' => {
                    self.i += 5;
                    Json::Bool(false)
                }
                b'n' => {
                    self.i += 4;
                    Json::Null
                }
                _ => self.number(),
            }
        }
        fn object(&mut self) -> Json {
            let mut m = BTreeMap::new();
            self.i += 1; // {
            self.ws();
            if self.b[self.i] == b'}' {
                self.i += 1;
                return Json::Obj(m);
            }
            loop {
                self.ws();
                let k = self.string();
                self.ws();
                self.i += 1; // :
                let v = self.value();
                m.insert(k, v);
                self.ws();
                match self.b[self.i] {
                    b',' => self.i += 1,
                    b'}' => {
                        self.i += 1;
                        break;
                    }
                    c => panic!("bad object char {}", c as char),
                }
            }
            Json::Obj(m)
        }
        fn array(&mut self) -> Json {
            let mut a = Vec::new();
            self.i += 1; // [
            self.ws();
            if self.b[self.i] == b']' {
                self.i += 1;
                return Json::Arr(a);
            }
            loop {
                a.push(self.value());
                self.ws();
                match self.b[self.i] {
                    b',' => self.i += 1,
                    b']' => {
                        self.i += 1;
                        break;
                    }
                    c => panic!("bad array char {}", c as char),
                }
            }
            Json::Arr(a)
        }
        fn string(&mut self) -> String {
            self.i += 1; // opening quote
            let mut s = String::new();
            while self.b[self.i] != b'"' {
                let c = self.b[self.i];
                if c == b'\\' {
                    self.i += 1;
                    let e = self.b[self.i];
                    match e {
                        b'n' => s.push('\n'),
                        b't' => s.push('\t'),
                        b'r' => s.push('\r'),
                        b'"' => s.push('"'),
                        b'\\' => s.push('\\'),
                        b'/' => s.push('/'),
                        b'u' => {
                            let hex = std::str::from_utf8(&self.b[self.i + 1..self.i + 5]).unwrap();
                            let cp = u32::from_str_radix(hex, 16).unwrap();
                            s.push(char::from_u32(cp).unwrap_or('?'));
                            self.i += 4;
                        }
                        _ => s.push(e as char),
                    }
                    self.i += 1;
                } else {
                    // copy a UTF-8 byte run verbatim
                    let start = self.i;
                    while self.b[self.i] != b'"' && self.b[self.i] != b'\\' {
                        self.i += 1;
                    }
                    s.push_str(std::str::from_utf8(&self.b[start..self.i]).unwrap());
                }
            }
            self.i += 1; // closing quote
            s
        }
        fn number(&mut self) -> Json {
            let start = self.i;
            while self.i < self.b.len()
                && matches!(
                    self.b[self.i],
                    b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E'
                )
            {
                self.i += 1;
            }
            let s = std::str::from_utf8(&self.b[start..self.i]).unwrap();
            Json::Num(s.parse().unwrap())
        }
    }
}

// ---- instant parsing (ISO 8601 UTC → ms since epoch) --------------------

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Parse "YYYY-MM-DDTHH:MM:SS[.fff]Z" into ms since the Unix epoch (UTC).
fn parse_instant(s: &str) -> i64 {
    let b = s.as_bytes();
    let num =
        |a: usize, n: usize| -> i64 { std::str::from_utf8(&b[a..a + n]).unwrap().parse().unwrap() };
    let y = num(0, 4);
    let mo = num(5, 2);
    let d = num(8, 2);
    let h = num(11, 2);
    let mi = num(14, 2);
    let sec = num(17, 2);
    let mut ms = 0i64;
    if b.get(19) == Some(&b'.') {
        // fractional seconds, 1..3 digits before 'Z'
        let mut i = 20;
        let mut frac = String::new();
        while i < b.len() && b[i].is_ascii_digit() {
            frac.push(b[i] as char);
            i += 1;
        }
        while frac.len() < 3 {
            frac.push('0');
        }
        ms = frac[..3].parse().unwrap();
    }
    days_from_civil(y, mo, d) * 86_400_000 + h * 3_600_000 + mi * 60_000 + sec * 1_000 + ms
}

// ---- the suite ----------------------------------------------------------

struct Report {
    section: &'static str,
    passed: usize,
    failed: usize,
    failures: Vec<String>,
}

fn run_coverage(root: &Json) -> Report {
    let mut r = Report {
        section: "coverage",
        passed: 0,
        failed: 0,
        failures: Vec::new(),
    };
    for group in root.get("coverage").as_arr() {
        let id = group.get("id").as_str();
        let expr_s = group.get("expression").as_str();
        let tz_id = group.get("tz").as_str();
        let parsed = match parse(expr_s) {
            Ok(p) => p,
            Err(e) => {
                r.failed += 1;
                r.failures
                    .push(format!("[{id}] parse failed for `{expr_s}`: {e}"));
                continue;
            }
        };
        let tz = Tz::load(tz_id).unwrap_or_else(|e| panic!("[{id}] tz `{tz_id}`: {e}"));
        for (instant, expected) in group.get("cases").as_obj() {
            let want = expected.as_bool();
            let got = parsed.covers_in(parse_instant(instant), &tz);
            if got == want {
                r.passed += 1;
            } else {
                r.failed += 1;
                r.failures.push(format!(
                    "[{id}] `{expr_s}` @ {instant} ({tz_id}): expected {want}, got {got}"
                ));
            }
        }
    }
    r
}

fn run_invalid(root: &Json) -> Report {
    let mut r = Report {
        section: "invalid",
        passed: 0,
        failed: 0,
        failures: Vec::new(),
    };
    for case in root.get("invalid").as_arr() {
        let expr_s = case.get("expression").as_str();
        let reason = case.get("reason").as_str();
        match parse(expr_s) {
            Err(_) => r.passed += 1,
            Ok(_) => {
                r.failed += 1;
                r.failures
                    .push(format!("`{expr_s}` should be rejected ({reason})"));
            }
        }
    }
    r
}

fn run_warnings(root: &Json) -> Report {
    let mut r = Report {
        section: "warnings",
        passed: 0,
        failed: 0,
        failures: Vec::new(),
    };
    for case in root.get("warnings").as_arr() {
        let expr_s = case.get("expression").as_str();
        match parse(expr_s) {
            Ok(p) if !p.warnings().is_empty() => r.passed += 1,
            Ok(_) => {
                r.failed += 1;
                r.failures
                    .push(format!("`{expr_s}` should warn but produced none"));
            }
            Err(e) => {
                r.failed += 1;
                r.failures.push(format!(
                    "`{expr_s}` should parse-with-warning, but errored: {e}"
                ));
            }
        }
    }
    r
}

fn run_quiet(root: &Json) -> Report {
    let mut r = Report {
        section: "quiet",
        passed: 0,
        failed: 0,
        failures: Vec::new(),
    };
    for case in root.get("quiet").as_arr() {
        let expr_s = case.get("expression").as_str();
        match parse(expr_s) {
            Ok(p) if p.warnings().is_empty() => r.passed += 1,
            Ok(p) => {
                r.failed += 1;
                r.failures.push(format!(
                    "`{expr_s}` should be quiet but warned: {:?}",
                    p.warnings()
                ));
            }
            Err(e) => {
                r.failed += 1;
                r.failures
                    .push(format!("`{expr_s}` should parse quietly, but errored: {e}"));
            }
        }
    }
    r
}

#[test]
fn conformance() {
    let root: Json = json::parse(VECTORS);
    let reports = [
        run_coverage(&root),
        run_invalid(&root),
        run_warnings(&root),
        run_quiet(&root),
    ];
    let mut total_failed = 0;
    println!("\n=== DTRExp conformance ===");
    for r in &reports {
        println!(
            "  {:9} {:>4} passed, {:>3} failed",
            r.section, r.passed, r.failed
        );
        for f in &r.failures {
            println!("      FAIL {f}");
        }
        total_failed += r.failed;
    }
    println!("==========================\n");
    assert_eq!(total_failed, 0, "{total_failed} vector(s) failed");
}
