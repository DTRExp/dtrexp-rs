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
//! `UTC` is handled without touching the filesystem so evaluation always works
//! for the default zone.

use std::fs;
use std::path::PathBuf;

use crate::civil::MS_PER_SEC;
use crate::error::UnknownTimeZone;

/// A parsed time zone: a sorted list of UTC transition instants, each paired
/// with the offset (seconds east of UTC) that takes effect at that instant.
#[derive(Clone, Debug)]
pub struct Tz {
    name: String,
    /// (utc_transition_seconds, offset_seconds_after_transition), sorted.
    transitions: Vec<(i64, i32)>,
    /// Offset in effect before the first transition.
    first_offset: i32,
}

const ZONEINFO_DIRS: [&str; 2] = ["/var/db/timezone/zoneinfo", "/usr/share/zoneinfo"];

impl Tz {
    /// The UTC time zone: fixed zero offset, no transitions.
    pub fn utc() -> Tz {
        Tz {
            name: "UTC".to_string(),
            transitions: Vec::new(),
            first_offset: 0,
        }
    }

    /// Load an IANA zone by identifier (e.g. `"Europe/Berlin"`). `UTC` is
    /// synthesised; everything else is read from the system TZif database.
    pub fn load(id: &str) -> Result<Tz, UnknownTimeZone> {
        if id == "UTC" || id == "Etc/UTC" || id.is_empty() {
            return Ok(Tz::utc());
        }
        // Guard against path traversal in the identifier.
        if id.starts_with('/') || id.contains("..") {
            return Err(UnknownTimeZone::new(id, "invalid identifier"));
        }
        for dir in ZONEINFO_DIRS {
            let mut path = PathBuf::from(dir);
            path.push(id);
            if let Ok(bytes) = fs::read(&path) {
                return parse_tzif(&bytes, id).map_err(|message| UnknownTimeZone::new(id, message));
            }
        }
        Err(UnknownTimeZone::new(id, "no TZif entry in the system database"))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// UTC offset (seconds) in effect at the given UTC instant (ms since epoch).
    pub fn offset_at_ms(&self, instant_ms: i64) -> i32 {
        let secs = instant_ms.div_euclid(MS_PER_SEC);
        // Largest transition with time <= secs.
        let idx = self.transitions.partition_point(|&(t, _)| t <= secs);
        if idx == 0 {
            self.first_offset
        } else {
            self.transitions[idx - 1].1
        }
    }

    /// Resolve a naive local wall-clock time (as a naive ordinal in ms) to a UTC
    /// instant (ms), using Temporal `compatible` disambiguation: for a repeated
    /// local time pick the earlier instant; for a gap resolve forward past it.
    pub fn local_to_instant_ms(&self, local_ms: i64) -> i64 {
        // Distinct offsets that appear in this zone.
        let mut offsets: Vec<i32> = vec![self.first_offset];
        for &(_, off) in &self.transitions {
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
        if !valid.is_empty() {
            // Unique → that instant; overlap → the earlier (smaller) instant.
            return *valid.iter().min().unwrap();
        }
        // Gap: find the spring-forward transition whose skipped local interval
        // contains `local_ms`. Temporal `compatible` interprets the wall-clock
        // time with the offset in effect BEFORE the transition, which moves it
        // forward past the gap by the gap's length (Berlin 02:30 -> 03:30 CEST).
        for i in 0..self.transitions.len() {
            let (tt, off_after) = self.transitions[i];
            let off_before = if i == 0 {
                self.first_offset
            } else {
                self.transitions[i - 1].1
            };
            if off_after > off_before {
                let gap_start = tt * MS_PER_SEC + i64::from(off_before) * MS_PER_SEC;
                let gap_end = tt * MS_PER_SEC + i64::from(off_after) * MS_PER_SEC;
                if local_ms >= gap_start && local_ms < gap_end {
                    return local_ms - i64::from(off_before) * MS_PER_SEC;
                }
            }
        }
        // Fallback (should not happen for well-formed zones).
        local_ms - i64::from(self.first_offset) * MS_PER_SEC
    }
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
    let (transitions, first_offset) = if version == b'2' || version == b'3' {
        // Skip the 32-bit v1 block, then parse the 64-bit v2/v3 block.
        let v1_body = 44 + block_size(&c1, 4, 4);
        let c2 = read_counts(d, v1_body)?;
        let v2_body_start = v1_body + 44;
        parse_body(d, v2_body_start, 8, &c2)?
    } else {
        parse_body(d, 44, 4, &c1)?
    };
    Ok(Tz {
        name: name.to_string(),
        transitions,
        first_offset,
    })
}
