//! Minimal, dependency-free IANA time-zone support.
//!
//! Reads the system TZif database (`/var/db/timezone/zoneinfo` on macOS, or
//! `/usr/share/zoneinfo`) and answers the two questions DTRExp evaluation needs:
//!
//! 1. the UTC offset in effect at a given instant (used to derive local calendar
//!    fields — the workhorse), and
//! 2. the instant a local wall-clock time resolves to under Temporal's
//!    `compatible` disambiguation (used only for `H`/`m`-period cadence anchors,
//!    §9.3).
//!
//! A zone file's transition table answers up to its last transition; the TZ
//! string footer of a v2+ file answers after it ([`footer`]), so slim and fat
//! files read alike.
//!
//! A zone identifier is a tz database zone or link name, matched without
//! regard to case and read in the database's own spelling (API.md): it is
//! looked up in the folder's name list, never handed to the file system as
//! typed, so the answer does not depend on whether that file system ignores
//! case. `UTC` is handled without touching the filesystem so evaluation
//! always works for the default zone.

mod footer;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::civil::MS_PER_SEC;
use crate::error::UnknownTimeZone;
use footer::Footer;

/// A parsed time zone: a sorted list of UTC transition instants, each paired
/// with the offset (seconds east of UTC) that takes effect at that instant,
/// and the footer rule that takes over after the last one.
///
/// With the `host-tz` feature, a zone can instead be backed by a host offset
/// callback ([`Tz::from_offset_fn`]) for environments without a zoneinfo
/// filesystem (WASM).
#[derive(Clone, Debug)]
pub struct Tz {
    name: String,
    /// (utc_transition_seconds, offset_seconds_after_transition), sorted.
    transitions: Vec<(i64, i32)>,
    /// Offset in effect before the first transition.
    first_offset: i32,
    /// The TZ string rule in effect from the last transition on (or always,
    /// with no transitions); without one, the last transition's offset holds.
    footer: Option<Footer>,
    #[cfg(feature = "host-tz")]
    host: Option<HostFn>,
}

/// A host-supplied offset lookup: UTC instant (ms since epoch) → offset
/// (seconds east of UTC). `Rc` keeps [`Tz`] cloneable; a host-backed zone is
/// consequently not `Send` — irrelevant on WASM, the feature's audience.
#[cfg(feature = "host-tz")]
#[derive(Clone)]
struct HostFn(std::rc::Rc<dyn Fn(i64) -> i32>);

#[cfg(feature = "host-tz")]
impl std::fmt::Debug for HostFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostFn(..)")
    }
}

const ZONEINFO_DIRS: [&str; 2] = ["/var/db/timezone/zoneinfo", "/usr/share/zoneinfo"];

impl Tz {
    /// A table-backed zone (the UTC and TZif construction paths).
    fn table(
        name: String,
        transitions: Vec<(i64, i32)>,
        first_offset: i32,
        footer: Option<Footer>,
    ) -> Tz {
        Tz {
            name,
            transitions,
            first_offset,
            footer,
            #[cfg(feature = "host-tz")]
            host: None,
        }
    }

    /// The UTC time zone: fixed zero offset, no transitions.
    pub fn utc() -> Tz {
        Tz::utc_named("UTC")
    }

    fn utc_named(name: &str) -> Tz {
        Tz::table(name.to_string(), Vec::new(), 0, None)
    }

    /// A zone backed by a host offset callback: `f` maps a UTC instant (ms
    /// since the Unix epoch) to the offset in effect (seconds east of UTC).
    /// For hosts without a zoneinfo filesystem (WASM), where the embedder owns
    /// zone data — e.g. via `Intl`. Wall-clock resolution follows Temporal
    /// `compatible` semantics, derived from `f` alone.
    #[cfg(feature = "host-tz")]
    pub fn from_offset_fn(id: &str, f: impl Fn(i64) -> i32 + 'static) -> Tz {
        Tz {
            name: id.to_string(),
            transitions: Vec::new(),
            first_offset: 0,
            footer: None,
            host: Some(HostFn(std::rc::Rc::new(f))),
        }
    }

    /// Load an IANA zone by identifier (e.g. `"Europe/Berlin"`): a tz
    /// database zone or link name, matched without regard to case; the zone
    /// is named in the database's spelling (`"europe/berlin"` loads as
    /// `"Europe/Berlin"`). `UTC` and `Etc/UTC` are synthesised, and an empty
    /// identifier means UTC; everything else is read from the system TZif
    /// database, whose name list decides what is a zone. Anything else is an
    /// [`UnknownTimeZone`]: a fixed offset (`+02:00`), `Z`, a file that names
    /// no zone (`posixrules`, `Factory`, `localtime`) or a path.
    pub fn load(id: &str) -> Result<Tz, UnknownTimeZone> {
        if id.is_empty() {
            return Ok(Tz::utc());
        }
        for utc in ["UTC", "Etc/UTC"] {
            if id.eq_ignore_ascii_case(utc) {
                return Ok(Tz::utc_named(utc));
            }
        }
        // Guard against path traversal in the identifier.
        if id.starts_with('/') || id.contains("..") {
            return Err(UnknownTimeZone::new(id, "invalid identifier"));
        }
        // An IANA name is a normalized path; `./localtime` or `a//b` is not one.
        if id.split('/').any(|seg| seg.is_empty() || seg == ".") {
            return Err(UnknownTimeZone::new(id, "invalid identifier"));
        }
        // Some systems link `localtime` to the host's zone; it is no IANA name.
        if id.rsplit('/').next() == Some("localtime") {
            return Err(UnknownTimeZone::new(
                id,
                "the host's zone is not an IANA identifier",
            ));
        }
        Names::system().load(id)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// UTC offset (seconds) in effect at the given UTC instant (ms since epoch).
    pub fn offset_at_ms(&self, instant_ms: i64) -> i32 {
        #[cfg(feature = "host-tz")]
        if let Some(host) = &self.host {
            return (host.0)(instant_ms);
        }
        let secs = instant_ms.div_euclid(MS_PER_SEC);
        // Largest transition with time <= secs.
        let idx = self.transitions.partition_point(|&(t, _)| t <= secs);
        match &self.footer {
            Some(footer) if idx == self.transitions.len() => footer.offset_at(secs),
            _ if idx == 0 => self.first_offset,
            _ => self.transitions[idx - 1].1,
        }
    }

    /// Resolve a naive local wall-clock time (as a naive ordinal in ms) to a UTC
    /// instant (ms), using Temporal `compatible` disambiguation: for a repeated
    /// local time pick the earlier instant; for a gap resolve forward past it.
    pub fn local_to_instant_ms(&self, local_ms: i64) -> i64 {
        #[cfg(feature = "host-tz")]
        if self.host.is_some() {
            return self.local_to_instant_host(local_ms);
        }
        // Distinct offsets that appear in this zone.
        let mut offsets: Vec<i32> = vec![self.first_offset];
        let table = self.transitions.iter().map(|&(_, off)| off);
        for off in table.chain(self.footer.iter().flat_map(Footer::offsets)) {
            if !offsets.contains(&off) {
                offsets.push(off);
            }
        }
        let mut valid: Vec<i64> = Vec::new();
        for &o in &offsets {
            let cand = local_ms - i64::from(o) * MS_PER_SEC;
            if self.offset_at_ms(cand) == o {
                valid.push(cand);
            }
        }
        if let Some(&earliest) = valid.iter().min() {
            // Unique → that instant; overlap → the earlier (smaller) instant.
            return earliest;
        }
        // No valid offset means `local_ms` sits in a spring-forward gap: find the
        // transition whose skipped local interval contains it. Temporal
        // `compatible` reads the wall-clock time with the offset in effect BEFORE
        // the transition, moving it forward past the gap (Berlin 02:30 → 03:30
        // CEST). An empty `valid` set always corresponds to exactly one such gap:
        // a listed transition's, or else one the footer makes, where the offset
        // before is the lower of its two; so the search always succeeds.
        (0..self.transitions.len())
            .find_map(|i| {
                let (tt, off_after) = self.transitions[i];
                let off_before = if i == 0 {
                    self.first_offset
                } else {
                    self.transitions[i - 1].1
                };
                let gap_start = tt * MS_PER_SEC + i64::from(off_before) * MS_PER_SEC;
                let gap_end = tt * MS_PER_SEC + i64::from(off_after) * MS_PER_SEC;
                (off_after > off_before && local_ms >= gap_start && local_ms < gap_end)
                    .then_some(local_ms - i64::from(off_before) * MS_PER_SEC)
            })
            .or_else(|| {
                let footer = self.footer.as_ref();
                footer.map(|f| local_ms - i64::from(f.gap_offset()) * MS_PER_SEC)
            })
            .unwrap()
    }

    /// The host-backed variant of [`Tz::local_to_instant_ms`]: with only an
    /// offset lookup available (no transition table), the two candidate
    /// instants come from the offsets in effect a day either side of the
    /// target. Probing the target itself would be wrong: it is a local
    /// pseudo-epoch, not an instant, so which candidate it lands on flips with
    /// the sign of the zone's offset. Mirrors the reference implementation's
    /// `epochFromLocal`.
    #[cfg(feature = "host-tz")]
    fn local_to_instant_host(&self, local_ms: i64) -> i64 {
        const DAY_MS: i64 = 86_400_000;
        let off_ms = |t: i64| i64::from(self.offset_at_ms(t)) * MS_PER_SEC;
        let before = local_ms - off_ms(local_ms - DAY_MS);
        let after = local_ms - off_ms(local_ms + DAY_MS);
        let earlier = before.min(after);
        // A unique local time resolves through its own offset; a repeated one
        // (fall-back overlap) picks the earlier instant. Otherwise `local_ms`
        // sits in a spring-forward gap, and `compatible` resolves forward,
        // which is the later candidate.
        if earlier + off_ms(earlier) == local_ms {
            earlier
        } else {
            before.max(after)
        }
    }
}

/// The zone and link names a zoneinfo folder holds, in the database's own
/// spelling: from its `tzdata.zi` where it has one (the `Z` and `L` lines),
/// else from a walk of the folder for TZif files.
#[derive(Debug, Default)]
struct Names {
    dir: PathBuf,
    names: Vec<String>,
}

impl Names {
    /// The system database's names, read once: the first folder of
    /// [`ZONEINFO_DIRS`] that holds any. Without one, no name is a zone.
    fn system() -> &'static Names {
        static SYSTEM: OnceLock<Names> = OnceLock::new();
        SYSTEM.get_or_init(|| {
            ZONEINFO_DIRS
                .iter()
                .find_map(|dir| Names::open(Path::new(dir)))
                .unwrap_or_default()
        })
    }

    /// The names in `dir`, or `None` when it holds none.
    fn open(dir: &Path) -> Option<Names> {
        let mut names = Vec::new();
        match fs::read_to_string(dir.join("tzdata.zi")) {
            Ok(text) => names.extend(names_in_zi(&text)),
            Err(_) => walk(dir, "", &mut names),
        }
        names.retain(|name| is_zone_name(name));
        (!names.is_empty()).then(|| Names {
            dir: dir.to_path_buf(),
            names,
        })
    }

    /// Load the zone `id` names, matched without regard to case.
    fn load(&self, id: &str) -> Result<Tz, UnknownTimeZone> {
        let name = self
            .names
            .iter()
            .find(|name| name.eq_ignore_ascii_case(id))
            .ok_or_else(|| {
                UnknownTimeZone::new(id, "not a zone or link name in the tz database")
            })?;
        let bytes = fs::read(self.dir.join(name))
            .map_err(|_| UnknownTimeZone::new(id, "no TZif entry in the system database"))?;
        parse_tzif(&bytes, name).map_err(|message| UnknownTimeZone::new(id, message))
    }
}

/// The zone (`Z name …`) and link (`L target name`) names of a `tzdata.zi`.
fn names_in_zi(text: &str) -> impl Iterator<Item = String> + '_ {
    text.lines().filter_map(|line| {
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("Z") => fields.next(),
            Some("L") => fields.nth(1),
            _ => None,
        }
        .map(str::to_string)
    })
}

/// Collect the TZif files under `dir` as names relative to the folder root,
/// skipping what [`is_zone_name`] rules out without reading it.
fn walk(dir: &Path, prefix: &str, names: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.file_name();
        let name = if prefix.is_empty() {
            file.to_string_lossy().into_owned()
        } else {
            format!("{prefix}/{}", file.to_string_lossy())
        };
        let path = entry.path();
        if !is_zone_name(&name) {
            continue;
        }
        if path.is_dir() {
            walk(&path, &name, names);
        } else if is_tzif(&path) {
            names.push(name);
        }
    }
}

fn is_tzif(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok_and(|()| &magic == b"TZif")
}

/// Whether a name in a zoneinfo folder can name a zone: not a file that names
/// none (`posixrules`, `Factory`, `localtime`), and not under the folders that
/// copy the database for other uses (`posix/`, `right/`) or hold the retired
/// `SystemV/` zones.
fn is_zone_name(name: &str) -> bool {
    let top = name.split('/').next().unwrap_or(name);
    !matches!(
        top,
        "posixrules" | "Factory" | "localtime" | "posix" | "right" | "SystemV"
    )
}

fn be_i32(d: &[u8], p: usize) -> i32 {
    i32::from_be_bytes([d[p], d[p + 1], d[p + 2], d[p + 3]])
}

fn be_u32(d: &[u8], p: usize) -> usize {
    u32::from_be_bytes([d[p], d[p + 1], d[p + 2], d[p + 3]]) as usize
}

fn be_i64(d: &[u8], p: usize) -> i64 {
    i64::from_be_bytes([
        d[p],
        d[p + 1],
        d[p + 2],
        d[p + 3],
        d[p + 4],
        d[p + 5],
        d[p + 6],
        d[p + 7],
    ])
}

struct Counts {
    isutcnt: usize,
    isstdcnt: usize,
    leapcnt: usize,
    timecnt: usize,
    typecnt: usize,
    charcnt: usize,
}

fn read_counts(d: &[u8], pos: usize) -> Result<Counts, String> {
    if d.len() < pos + 44 || &d[pos..pos + 4] != b"TZif" {
        return Err("not a TZif file".to_string());
    }
    let base = pos + 20; // skip magic(4) + version(1) + reserved(15)
    Ok(Counts {
        isutcnt: be_u32(d, base),
        isstdcnt: be_u32(d, base + 4),
        leapcnt: be_u32(d, base + 8),
        timecnt: be_u32(d, base + 12),
        typecnt: be_u32(d, base + 16),
        charcnt: be_u32(d, base + 20),
    })
}

/// Size in bytes of a TZif data block for the given time width (4 or 8).
fn block_size(c: &Counts, time_size: usize, leap_time_size: usize) -> usize {
    c.timecnt * time_size
        + c.timecnt
        + c.typecnt * 6
        + c.charcnt
        + c.leapcnt * (leap_time_size + 4)
        + c.isstdcnt
        + c.isutcnt
}

fn parse_body(
    d: &[u8],
    start: usize,
    time_size: usize,
    c: &Counts,
) -> Result<(Vec<(i64, i32)>, i32), String> {
    if d.len() < start + block_size(c, time_size, time_size) {
        return Err("truncated TZif file".to_string());
    }
    let mut p = start;
    let mut times: Vec<i64> = Vec::with_capacity(c.timecnt);
    for _ in 0..c.timecnt {
        let t = if time_size == 8 {
            be_i64(d, p)
        } else {
            i64::from(be_i32(d, p))
        };
        times.push(t);
        p += time_size;
    }
    let type_idx = &d[p..p + c.timecnt];
    p += c.timecnt;
    let mut utoff: Vec<i32> = Vec::with_capacity(c.typecnt);
    let mut isdst: Vec<bool> = Vec::with_capacity(c.typecnt);
    for _ in 0..c.typecnt {
        utoff.push(be_i32(d, p));
        isdst.push(d[p + 4] != 0);
        p += 6;
    }
    if utoff.is_empty() {
        return Err("TZif file has no local time types".to_string());
    }
    let mut transitions: Vec<(i64, i32)> = Vec::with_capacity(c.timecnt);
    for i in 0..c.timecnt {
        let ti = type_idx[i] as usize;
        transitions.push((times[i], utoff[ti]));
    }
    // Offset before the first transition: first standard-time type, else type 0.
    let first_offset = (0..c.typecnt)
        .find(|&i| !isdst[i])
        .map(|i| utoff[i])
        .unwrap_or(utoff[0]);
    Ok((transitions, first_offset))
}

fn parse_tzif(d: &[u8], name: &str) -> Result<Tz, String> {
    let c1 = read_counts(d, 0)?;
    let version = d[4];
    if version == b'2' || version == b'3' {
        // Skip the 32-bit v1 block, then parse the 64-bit v2/v3 block and the
        // TZ string footer after it.
        let v1_body = 44 + block_size(&c1, 4, 4);
        let c2 = read_counts(d, v1_body)?;
        let v2_body_start = v1_body + 44;
        let (transitions, first_offset) = parse_body(d, v2_body_start, 8, &c2)?;
        let footer = footer::read(&d[v2_body_start + block_size(&c2, 8, 8)..])?;
        Ok(Tz::table(
            name.to_string(),
            transitions,
            first_offset,
            footer,
        ))
    } else {
        let (transitions, first_offset) = parse_body(d, 44, 4, &c1)?;
        Ok(Tz::table(name.to_string(), transitions, first_offset, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 44-byte TZif header with the given `version` byte and counts.
    fn header(version: u8, timecnt: u32, typecnt: u32) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(b"TZif");
        d.push(version);
        d.extend_from_slice(&[0u8; 15]); // reserved
        for c in [0u32, 0, 0, timecnt, typecnt, 0] {
            d.extend_from_slice(&c.to_be_bytes()); // isut, isstd, leap, time, type, char
        }
        d
    }

    /// A minimal TZif data block (32-bit times) with one STD→DST transition at
    /// the epoch and the two given offsets.
    fn block_32(std_off: i32, dst_off: i32) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&0i32.to_be_bytes()); // transition time: epoch
        d.push(1); // → type index 1 (DST) after the transition
        d.extend_from_slice(&std_off.to_be_bytes()); // type 0: STD
        d.extend_from_slice(&[0, 0]); // isdst=0, abbrind=0
        d.extend_from_slice(&dst_off.to_be_bytes()); // type 1: DST
        d.extend_from_slice(&[1, 0]); // isdst=1, abbrind=0
        d
    }

    /// A complete legacy (version-1) TZif buffer.
    fn v1_tzif(std_off: i32, dst_off: i32) -> Vec<u8> {
        let mut d = header(0, 1, 2);
        d.extend_from_slice(&block_32(std_off, dst_off));
        d
    }

    /// Europe/Berlin built with `zic -b slim`: transitions stop in 1996 and
    /// the footer `CET-1CEST,M3.5.0,M10.5.0/3` carries every later year.
    const BERLIN_SLIM: &[u8] = include_bytes!("../tests/fixtures/berlin-slim.tzif");

    #[test]
    fn a_slim_zone_reads_current_years_from_its_footer() {
        let tz = parse_tzif(BERLIN_SLIM, "Europe/Berlin").unwrap();
        // 2026-01-15T12:00:00Z: CET.
        assert_eq!(tz.offset_at_ms(1_768_478_400_000), 3600);
        // 2026-07-07T07:30:00Z: CEST, so 09:30 local.
        assert_eq!(tz.offset_at_ms(1_783_409_400_000), 7200);
    }

    /// The same zone built with `zic -b fat`: transitions listed to 2037.
    const BERLIN_FAT: &[u8] = include_bytes!("../tests/fixtures/berlin-fat.tzif");
    /// `tests/fixtures/berlin-rules-only.zi` built with `zic -b slim`: one
    /// transition (1981, to CEST), then the footer for everything after.
    const BERLIN_RULES: &[u8] = include_bytes!("../tests/fixtures/berlin-rules-only.tzif");
    const SYDNEY_SLIM: &[u8] = include_bytes!("../tests/fixtures/sydney-slim.tzif");
    const SANTIAGO_SLIM: &[u8] = include_bytes!("../tests/fixtures/santiago-slim.tzif");
    const NUUK_SLIM: &[u8] = include_bytes!("../tests/fixtures/nuuk-slim.tzif");
    const RIYADH_SLIM: &[u8] = include_bytes!("../tests/fixtures/riyadh-slim.tzif");

    /// A v2 file with no transitions, one UTC type, and the given footer.
    fn v2_footer_only(footer: &[u8]) -> Vec<u8> {
        let mut d = header(b'2', 0, 0); // the v1 block, skipped
        d.extend_from_slice(&header(b'2', 0, 1));
        d.extend_from_slice(&0i32.to_be_bytes());
        d.extend_from_slice(&[0, 0]); // type 0: UTC, standard
        d.extend_from_slice(footer);
        d
    }

    /// Assert the offset flips from `before` to `after` exactly at `t_ms`.
    fn flips(tz: &Tz, t_ms: i64, before: i32, after: i32) {
        assert_eq!(tz.offset_at_ms(t_ms - 1), before, "just before {t_ms}");
        assert_eq!(tz.offset_at_ms(t_ms), after, "at {t_ms}");
    }

    #[test]
    fn a_slim_zone_agrees_with_its_fat_build() {
        let slim = parse_tzif(BERLIN_SLIM, "Europe/Berlin").unwrap();
        let fat = parse_tzif(BERLIN_FAT, "Europe/Berlin").unwrap();
        for tz in [&slim, &fat] {
            // 1975 kept no summer time; the footer alone would say CEST.
            assert_eq!(tz.offset_at_ms(173_448_000_000), 3600);
            // 1945-05-24 02:30 never occurs (CEST → CEMT at 00:00Z): forward
            // through CEST to 00:30Z, not through the footer's CET.
            assert_eq!(tz.local_to_instant_ms(-776_554_200_000), -776_561_400_000);
        }
        // Every 7h13m from 1970 through 2037, where the fat file lists all.
        let step = (7 * 60 + 13) * 60_000;
        let mut t: i64 = 0;
        while t < 2_145_916_800_000 {
            assert_eq!(slim.offset_at_ms(t), fat.offset_at_ms(t), "offset at {t}");
            assert_eq!(
                slim.local_to_instant_ms(t),
                fat.local_to_instant_ms(t),
                "wall clock {t}"
            );
            t += step;
        }
    }

    #[test]
    fn a_slim_zone_resolves_the_gap_and_the_overlap_through_its_footer() {
        let tz = parse_tzif(BERLIN_SLIM, "Europe/Berlin").unwrap();
        flips(&tz, 1_774_746_000_000, 3600, 7200); // 2026-03-29T01:00Z
        flips(&tz, 1_792_890_000_000, 7200, 3600); // 2026-10-25T01:00Z
                                                   // 2026-03-29 02:30 never occurs: forward to 03:30 CEST, 01:30Z.
        assert_eq!(tz.local_to_instant_ms(1_774_751_400_000), 1_774_747_800_000);
        // 2026-10-25 02:30 occurs twice: the earlier, CEST, 00:30Z.
        assert_eq!(tz.local_to_instant_ms(1_792_895_400_000), 1_792_888_200_000);
    }

    #[test]
    fn a_rules_only_zone_takes_both_offsets_from_its_footer() {
        // The table lists only CEST; winter wall-clock times resolve through
        // the footer's CET.
        let tz = parse_tzif(BERLIN_RULES, "Test/Berlin").unwrap();
        assert_eq!(tz.offset_at_ms(1_768_478_400_000), 3600); // 2026-01-15T12:00Z
        assert_eq!(tz.offset_at_ms(1_783_409_400_000), 7200); // 2026-07-07T07:30Z
                                                              // 2026-01-15 12:00 local is 11:00Z.
        assert_eq!(tz.local_to_instant_ms(1_768_478_400_000), 1_768_474_800_000);
        assert_eq!(tz.local_to_instant_ms(1_774_751_400_000), 1_774_747_800_000);
        assert_eq!(tz.local_to_instant_ms(1_792_895_400_000), 1_792_888_200_000);
    }

    #[test]
    fn a_southern_slim_zone_keeps_dst_over_the_year_end() {
        let tz = parse_tzif(SYDNEY_SLIM, "Australia/Sydney").unwrap();
        assert_eq!(tz.offset_at_ms(1_768_435_200_000), 39_600); // 2026-01-15: AEDT
        assert_eq!(tz.offset_at_ms(1_784_073_600_000), 36_000); // 2026-07-15: AEST
        flips(&tz, 1_775_318_400_000, 39_600, 36_000); // 2026-04-04T16:00Z
        flips(&tz, 1_791_043_200_000, 36_000, 39_600); // 2026-10-03T16:00Z
                                                       // 2026-10-04 02:30 never occurs: forward to 03:30 AEDT, 10-03T16:30Z.
        assert_eq!(tz.local_to_instant_ms(1_791_081_000_000), 1_791_045_000_000);
        // 2026-04-05 02:30 occurs twice: the earlier, AEDT, 04-04T15:30Z.
        assert_eq!(tz.local_to_instant_ms(1_775_356_200_000), 1_775_316_600_000);
    }

    #[test]
    fn slim_zones_with_24_hour_and_negative_rule_times() {
        // Santiago: `<-04>4<-03>,M9.1.6/24,M4.1.6/24`.
        let tz = parse_tzif(SANTIAGO_SLIM, "America/Santiago").unwrap();
        assert_eq!(tz.offset_at_ms(1_768_435_200_000), -10_800);
        assert_eq!(tz.offset_at_ms(1_784_073_600_000), -14_400);
        flips(&tz, 1_775_358_000_000, -10_800, -14_400); // 2026-04-05T03:00Z
        flips(&tz, 1_788_667_200_000, -14_400, -10_800); // 2026-09-06T04:00Z
                                                         // Nuuk: `<-02>2<-01>,M3.5.0/-1,M10.5.0/0`, a version-3 file.
        let tz = parse_tzif(NUUK_SLIM, "America/Nuuk").unwrap();
        flips(&tz, 1_774_746_000_000, -7200, -3600); // 2026-03-29T01:00Z
        flips(&tz, 1_792_890_000_000, -3600, -7200); // 2026-10-25T01:00Z
    }

    #[test]
    fn a_std_only_footer_holds_after_the_last_transition() {
        // Riyadh: one transition (1947, to +03), then `<+03>-3`.
        let tz = parse_tzif(RIYADH_SLIM, "Asia/Riyadh").unwrap();
        assert_eq!(tz.offset_at_ms(1_783_409_400_000), 10_800);
        assert_eq!(tz.local_to_instant_ms(1_783_409_400_000), 1_783_398_600_000);
    }

    #[test]
    fn a_fat_zone_follows_its_footer_past_2037() {
        let tz = parse_tzif(BERLIN_FAT, "Europe/Berlin").unwrap();
        assert_eq!(tz.offset_at_ms(4_103_697_600_000), 3600); // 2100-01-15T12:00Z
        assert_eq!(tz.offset_at_ms(4_118_628_600_000), 7200); // 2100-07-07T07:30Z
                                                              // 2100-03-28 02:30 never occurs: 01:30Z; 2100-10-31 02:30 repeats: 00:30Z.
        assert_eq!(tz.local_to_instant_ms(4_109_884_200_000), 4_109_880_600_000);
        assert_eq!(tz.local_to_instant_ms(4_128_633_000_000), 4_128_625_800_000);
    }

    #[test]
    fn a_file_with_no_transitions_answers_from_its_footer() {
        // On at J60 (March 1) 02:00 at +1, off at day 300 (October 28) 02:00 at +2.
        let tz = parse_tzif(&v2_footer_only(b"\n<+01>-1<+02>,J60,300\n"), "x").unwrap();
        flips(&tz, 1_772_326_800_000, 3600, 7200); // 2026-03-01T01:00Z
        flips(&tz, 1_793_145_600_000, 7200, 3600); // 2026-10-28T00:00Z
                                                   // The gap: 2026-03-01 02:30 local is 01:30Z.
        assert_eq!(tz.local_to_instant_ms(1_772_332_200_000), 1_772_328_600_000);
    }

    #[test]
    fn a_negative_dst_footer_resolves_its_gap_from_the_lower_offset() {
        // Dublin's legal form: IST +1 is standard, GMT the winter "dst".
        let tz = parse_tzif(&v2_footer_only(b"\nIST-1GMT0,M10.5.0,M3.5.0/1\n"), "x").unwrap();
        // 2026-03-29 01:30 local never occurs: forward to 02:30 IST, 01:30Z.
        assert_eq!(tz.local_to_instant_ms(1_774_747_800_000), 1_774_747_800_000);
        // 2026-10-25 01:30 local repeats: the earlier, IST, 00:30Z.
        assert_eq!(tz.local_to_instant_ms(1_792_891_800_000), 1_792_888_200_000);
    }

    #[test]
    fn an_empty_footer_keeps_the_last_transition() {
        let tz = parse_tzif(&v2_footer_only(b"\n\n"), "x").unwrap();
        assert_eq!(tz.offset_at_ms(1_783_409_400_000), 0);
    }

    #[test]
    fn a_malformed_footer_is_an_error() {
        for footer in [
            &b""[..],
            b"\n",
            b"\nCET-1CEST,M3.5.0,M10.5.0/3",
            b"CET-1CEST,M3.5.0,M10.5.0/3\n",
            b"\nCET-1CEST,M3.5.0\n",
            b"\nCET-1CEST,M3.5.0,M10.5.0/3\n\n",
            b"\n\xff\xfe\n",
        ] {
            let err = parse_tzif(&v2_footer_only(footer), "x").unwrap_err();
            assert_eq!(err, "malformed TZ string footer");
        }
    }

    #[test]
    fn rejects_a_truncated_body() {
        let mut d = v1_tzif(0, 3600);
        d.pop();
        assert_eq!(parse_tzif(&d, "x").unwrap_err(), "truncated TZif file");
        let mut d = BERLIN_SLIM.to_vec();
        d.truncate(d.len() - 40);
        assert_eq!(parse_tzif(&d, "x").unwrap_err(), "truncated TZif file");
    }

    #[test]
    fn utc_reports_its_name() {
        assert_eq!(Tz::utc().name(), "UTC");
    }

    #[test]
    fn utc_aliases_load_without_the_filesystem() {
        // Matched without regard to case, named in the database's spelling.
        assert_eq!(Tz::load("Etc/UTC").unwrap().name(), "Etc/UTC");
        assert_eq!(Tz::load("etc/utc").unwrap().name(), "Etc/UTC");
        assert_eq!(Tz::load("utc").unwrap().name(), "UTC");
        assert_eq!(Tz::load("").unwrap().name(), "UTC");
    }

    // ---- the name list ---------------------------------------------------

    fn fixture(dir: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(dir)
    }

    #[test]
    fn a_tzdata_zi_lists_its_zones_and_links() {
        let names = Names::open(&fixture("zoneinfo-zi")).unwrap();
        assert_eq!(
            names.names,
            [
                "Europe/Berlin",
                "Broken/Text",
                "Gone/Zone",
                "Europe/Busingen"
            ]
        );
    }

    #[test]
    fn a_listed_name_loads_in_the_database_spelling_whatever_its_case() {
        let names = Names::open(&fixture("zoneinfo-zi")).unwrap();
        for id in ["Europe/Berlin", "europe/berlin", "EUROPE/BERLIN"] {
            assert_eq!(names.load(id).unwrap().name(), "Europe/Berlin");
        }
        assert_eq!(
            names.load("europe/busingen").unwrap().name(),
            "Europe/Busingen"
        );
    }

    #[test]
    fn a_name_the_list_lacks_is_unknown_even_with_a_file() {
        let names = Names::open(&fixture("zoneinfo-zi")).unwrap();
        for id in ["Factory", "Europe/Paris", "+02:00"] {
            assert_eq!(
                names.load(id).unwrap_err(),
                UnknownTimeZone::new(id, "not a zone or link name in the tz database")
            );
        }
    }

    #[test]
    fn a_listed_name_without_a_readable_tzif_file_is_unknown() {
        let names = Names::open(&fixture("zoneinfo-zi")).unwrap();
        assert_eq!(
            names.load("gone/zone").unwrap_err(),
            UnknownTimeZone::new("gone/zone", "no TZif entry in the system database")
        );
        assert_eq!(
            names.load("Broken/Text").unwrap_err(),
            UnknownTimeZone::new("Broken/Text", "not a TZif file")
        );
    }

    #[test]
    fn without_a_tzdata_zi_the_folder_is_walked_for_zones() {
        let mut names = Names::open(&fixture("zoneinfo-walk")).unwrap();
        names.names.sort();
        // `posixrules`, `Factory`, `localtime`, `posix/`, `right/` and
        // `SystemV/` are skipped, and so are the files that are not TZif.
        assert_eq!(names.names, ["America/Sao_Paulo", "Europe/Berlin"]);
        assert_eq!(names.load("europe/berlin").unwrap().name(), "Europe/Berlin");
    }

    #[test]
    fn a_folder_without_names_holds_no_database() {
        assert!(Names::open(&fixture("no-such-folder")).is_none());
        assert!(Names::open(&fixture("zoneinfo-walk/Text")).is_none());
    }

    #[test]
    fn the_system_names_are_read_once() {
        assert!(std::ptr::eq(Names::system(), Names::system()));
        assert!(Names::system().names.iter().any(|n| n == "Europe/Berlin"));
    }

    #[test]
    fn parses_a_legacy_v1_body() {
        let tz = parse_tzif(&v1_tzif(0, 3600), "Test/V1").unwrap();
        assert_eq!(tz.name(), "Test/V1");
        // Before the epoch → the STD (first, non-DST) offset.
        assert_eq!(tz.offset_at_ms(-1_000), 0);
        // After the epoch transition → the DST offset.
        assert_eq!(tz.offset_at_ms(1_000), 3600);
    }

    #[test]
    fn resolves_a_wall_clock_time_inside_a_spring_forward_gap() {
        // v1_tzif springs forward 0→+3600 at the epoch, so local wall-clock
        // 00:00..01:00 (ms [0, 3_600_000)) never occurs; `compatible` pushes it
        // forward using the pre-transition offset (0), i.e. maps it unchanged.
        let tz = parse_tzif(&v1_tzif(0, 3600), "Test/Gap").unwrap();
        assert_eq!(tz.local_to_instant_ms(1_800_000), 1_800_000);
        // A time outside the gap resolves through a valid offset.
        assert_eq!(tz.local_to_instant_ms(-3_600_000), -3_600_000);
    }

    /// A minimal TZif data block (64-bit times) with one STD→DST transition at
    /// the epoch and the two given offsets.
    fn block_64(std_off: i32, dst_off: i32) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&0i64.to_be_bytes()); // transition time: epoch
        d.push(1); // → type index 1 (DST) after the transition
        d.extend_from_slice(&std_off.to_be_bytes());
        d.extend_from_slice(&[0, 0]);
        d.extend_from_slice(&dst_off.to_be_bytes());
        d.extend_from_slice(&[1, 0]);
        d
    }

    #[test]
    fn big_endian_readers_use_each_byte_position_once() {
        // Distinct byte values at a nonzero offset: any repeated, shifted, or
        // dropped index changes the value (or reads out of bounds).
        let d = [0x99u8, 0x98, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        assert_eq!(be_i32(&d, 2), 0x0102_0304);
        assert_eq!(be_u32(&d, 2), 0x0102_0304);
        assert_eq!(be_i64(&d, 2), 0x0102_0304_0506_0708);
    }

    #[test]
    fn rejects_a_non_tzif_buffer() {
        assert!(parse_tzif(b"not a tzif file", "x").is_err());
    }

    #[test]
    fn rejects_a_truncated_buffer_with_a_valid_magic() {
        // Shorter than one header, but the magic matches: the length check must
        // reject it on its own, not rely on the magic comparison.
        assert!(parse_tzif(b"TZif2 truncated", "x").is_err());
    }

    #[test]
    fn v2_skip_honours_isstdcnt_and_leapcnt() {
        // A v1 block carrying one leap-second record (8 bytes) and one
        // standard/wall flag (1 byte): the v2 header is found only if
        // `block_size` counts both.
        let mut d = Vec::new();
        d.extend_from_slice(b"TZif2");
        d.extend_from_slice(&[0u8; 15]); // reserved
        for c in [0u32, 1, 1, 1, 2, 0] {
            d.extend_from_slice(&c.to_be_bytes()); // isut 0, isstd 1, leap 1, time 1, type 2, char 0
        }
        d.extend_from_slice(&block_32(0, 3600));
        d.extend_from_slice(&[0u8; 8]); // leap record: occurrence(4) + correction(4)
        d.push(0); // standard/wall flag
        d.extend_from_slice(&header(b'2', 1, 2)); // second (64-bit) header
        d.extend_from_slice(&block_64(0, 3600));
        d.extend_from_slice(b"\n\n"); // empty footer
        let tz = parse_tzif(&d, "x").unwrap();
        assert_eq!(tz.offset_at_ms(1_000), 3600);
    }

    #[test]
    fn first_offset_prefers_the_first_standard_type() {
        // Type 0 is DST, type 1 is STD → the pre-transition offset must come
        // from type 1 (the first non-DST type), not type 0.
        let mut d = header(0, 1, 2);
        d.extend_from_slice(&0i32.to_be_bytes()); // transition at the epoch
        d.push(0); // → type 0 (DST)
        d.extend_from_slice(&3600i32.to_be_bytes());
        d.extend_from_slice(&[1, 0]); // type 0: DST +3600
        d.extend_from_slice(&0i32.to_be_bytes());
        d.extend_from_slice(&[0, 0]); // type 1: STD 0
        let tz = parse_tzif(&d, "x").unwrap();
        assert_eq!(tz.offset_at_ms(-1_000), 0); // before the transition: first STD type
        assert_eq!(tz.offset_at_ms(1_000), 3600);
    }

    #[test]
    fn gap_resolution_with_negative_offsets_and_a_pre_epoch_transition() {
        // Spring-forward -18000 → -14400 at t = -86400 s; the skipped local
        // interval is [-104400, -100800) s. Negative transition time and
        // negative offsets pin every sign and scale in the gap-window
        // arithmetic — any slip moves the window and strands the lookup.
        let mut d = header(0, 1, 2);
        d.extend_from_slice(&(-86_400i32).to_be_bytes());
        d.push(1);
        d.extend_from_slice(&(-18_000i32).to_be_bytes());
        d.extend_from_slice(&[0, 0]);
        d.extend_from_slice(&(-14_400i32).to_be_bytes());
        d.extend_from_slice(&[1, 0]);
        let tz = parse_tzif(&d, "x").unwrap();
        // Exactly at the gap start; `compatible` maps it forward through the
        // pre-transition offset: -104400 s + 18000 s = -86400 s.
        assert_eq!(tz.local_to_instant_ms(-104_400_000), -86_400_000);
    }

    #[test]
    fn gap_resolution_reads_the_offset_before_a_later_transition() {
        // Two transitions; the gap sits at index 1, so `off_before` must come
        // from `transitions[0]` (an off-by-one reads past the end).
        let mut d = header(0, 2, 2);
        d.extend_from_slice(&(-200_000i32).to_be_bytes());
        d.extend_from_slice(&(-86_400i32).to_be_bytes());
        d.push(0); // t = -200000 → type 0 (a no-op: same offset as first_offset)
        d.push(1); // t = -86400 → type 1 (the spring-forward)
        d.extend_from_slice(&(-18_000i32).to_be_bytes());
        d.extend_from_slice(&[0, 0]);
        d.extend_from_slice(&(-14_400i32).to_be_bytes());
        d.extend_from_slice(&[1, 0]);
        let tz = parse_tzif(&d, "x").unwrap();
        assert_eq!(tz.local_to_instant_ms(-104_400_000), -86_400_000);
    }

    #[test]
    fn rejects_a_buffer_with_no_local_time_types() {
        let err = parse_tzif(&header(0, 0, 0), "x").unwrap_err();
        assert!(err.contains("no local time types"));
    }

    #[test]
    fn rejects_a_v2_file_with_a_corrupt_second_header() {
        // version '2' → the parser skips the v1 block and re-reads a header; a
        // non-TZif second header fails there.
        let mut d = header(b'2', 1, 2);
        d.extend_from_slice(&block_32(0, 3600));
        d.extend_from_slice(&[0u8; 44]); // bogus second header
        assert!(parse_tzif(&d, "x").is_err());
    }

    #[test]
    fn rejects_a_v2_file_whose_second_block_has_no_types() {
        // Valid second header but typecnt = 0 → the 64-bit body parse fails.
        let mut d = header(b'2', 1, 2);
        d.extend_from_slice(&block_32(0, 3600));
        d.extend_from_slice(&header(b'2', 0, 0)); // empty second block
        let err = parse_tzif(&d, "x").unwrap_err();
        assert!(err.contains("no local time types"));
    }
}

#[cfg(test)]
#[cfg(feature = "host-tz")]
mod host_tests {
    use super::*;

    /// The host mirror of `v1_tzif(0, 3600)`: a 0 → +3600 spring-forward at
    /// the epoch.
    fn spring(t: i64) -> i32 {
        if t < 0 {
            0
        } else {
            3600
        }
    }

    /// A +3600 → 0 fall-back at the epoch.
    fn fall(t: i64) -> i32 {
        if t < 0 {
            3600
        } else {
            0
        }
    }

    #[test]
    fn host_zone_reports_its_name_and_dispatches_offsets() {
        let tz = Tz::from_offset_fn("Host/Step", spring);
        assert_eq!(tz.name(), "Host/Step");
        assert_eq!(tz.offset_at_ms(-1_000), 0);
        assert_eq!(tz.offset_at_ms(1_000), 3600);
    }

    #[test]
    fn host_zone_survives_clone_and_names_its_backend_in_debug() {
        let tz = Tz::from_offset_fn("Host/Step", spring).clone();
        assert_eq!(tz.offset_at_ms(1_000), 3600);
        assert!(format!("{tz:?}").contains("HostFn(..)"));
    }

    #[test]
    fn host_zone_resolves_a_unique_wall_clock_time_on_each_side() {
        let tz = Tz::from_offset_fn("Host/Step", spring);
        // Local 22:00 before the epoch: only offset 0 validates it.
        assert_eq!(tz.local_to_instant_ms(-7_200_000), -7_200_000);
        // Local 02:00 after: only offset +3600 validates it.
        assert_eq!(tz.local_to_instant_ms(7_200_000), 3_600_000);
    }

    #[test]
    fn host_zone_resolves_a_gap_forward() {
        let tz = Tz::from_offset_fn("Host/Gap", spring);
        // Local 00:30 never occurs (the spring-forward skips it); `compatible`
        // pushes it forward through the pre-transition offset — the same
        // mapping the table test pins for this zone shape.
        assert_eq!(tz.local_to_instant_ms(1_800_000), 1_800_000);
    }

    #[test]
    fn host_zone_resolves_a_repeated_time_to_the_earlier_instant() {
        let tz = Tz::from_offset_fn("Host/Fall", fall);
        // Local 00:30 occurs twice around the fall-back: as -1_800_000
        // (through +3600) and 1_800_000 (through 0); `compatible` picks the
        // earlier.
        assert_eq!(tz.local_to_instant_ms(1_800_000), -1_800_000);
    }
}
