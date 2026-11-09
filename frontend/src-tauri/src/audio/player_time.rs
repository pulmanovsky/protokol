//! Meetings recorded in 30-second chunks and joined without re-encoding have
//! a timing table that loses a little at every join (about 37 ms in the
//! file's table, none in its packets). Decoding reads the packets one after
//! another, so word times, the waveform and everything else follow the
//! packets; the player follows the table and runs ahead of them (2 s on a
//! 30-minute meeting). These points turn one clock into the other.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Below this the two clocks count as the same
const SAME_S: f64 = 0.05;
/// A `moov` larger than this is not one of ours
const MAX_MOOV: u64 = 64 * 1024 * 1024;

/// (count of packets, duration of each)
type Entry = (u32, u32);

fn children(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 8 <= data.len() {
        let mut size = u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize;
        let kind = [data[at + 4], data[at + 5], data[at + 6], data[at + 7]];
        let mut head = 8;
        if size == 1 {
            if at + 16 > data.len() {
                break;
            }
            size = u64::from_be_bytes(data[at + 8..at + 16].try_into().unwrap_or([0; 8])) as usize;
            head = 16;
        } else if size == 0 {
            size = data.len() - at;
        }
        if size < head || at + size > data.len() {
            break;
        }
        out.push((kind, &data[at + head..at + size]));
        at += size;
    }
    out
}

fn child<'a>(data: &'a [u8], path: &[&[u8; 4]]) -> Option<&'a [u8]> {
    let (first, rest) = path.split_first()?;
    let found = children(data).into_iter().find(|(k, _)| &k == first)?.1;
    if rest.is_empty() {
        Some(found)
    } else {
        child(found, rest)
    }
}

fn read_moov(path: &Path) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut at = 0u64;
    while at + 8 <= len {
        file.seek(SeekFrom::Start(at)).ok()?;
        let mut head = [0u8; 16];
        file.read_exact(&mut head[..8]).ok()?;
        let mut size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64;
        let kind = [head[4], head[5], head[6], head[7]];
        let mut header = 8;
        if size == 1 {
            file.read_exact(&mut head[8..16]).ok()?;
            size = u64::from_be_bytes(head[8..16].try_into().ok()?);
            header = 16;
        } else if size == 0 {
            size = len - at;
        }
        if size < header {
            return None;
        }
        if &kind == b"moov" {
            if size > MAX_MOOV {
                return None;
            }
            let mut body = vec![0u8; (size - header) as usize];
            file.read_exact(&mut body).ok()?;
            return Some(body);
        }
        at += size;
    }
    None
}

/// The sound track's timescale and table
fn sound_table(moov: &[u8]) -> Option<(u32, Vec<Entry>)> {
    for (kind, track) in children(moov) {
        if &kind != b"trak" {
            continue;
        }
        let handler = child(track, &[b"mdia", b"hdlr"])?;
        if handler.get(8..12) != Some(b"soun") {
            continue;
        }
        let mdhd = child(track, &[b"mdia", b"mdhd"])?;
        let at = if *mdhd.first()? == 1 { 20 } else { 12 };
        let timescale = u32::from_be_bytes(mdhd.get(at..at + 4)?.try_into().ok()?);
        let stts = child(track, &[b"mdia", b"minf", b"stbl", b"stts"])?;
        let count = u32::from_be_bytes(stts.get(4..8)?.try_into().ok()?) as usize;
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let at = 8 + i * 8;
            entries.push((u32::from_be_bytes(stts.get(at..at + 4)?.try_into().ok()?), u32::from_be_bytes(stts.get(at + 4..at + 8)?.try_into().ok()?)));
        }
        return Some((timescale, entries));
    }
    None
}

/// Seconds by the table and seconds by the packets
pub(crate) fn durations(timescale: u32, entries: &[Entry]) -> (f64, f64) {
    let Some(frame) = entries.iter().max_by_key(|e| e.0).map(|e| e.1) else { return (0.0, 0.0) };
    let scale = timescale.max(1) as f64;
    let table: u64 = entries.iter().map(|e| e.0 as u64 * e.1 as u64).sum();
    let packets: u64 = entries.iter().map(|e| e.0 as u64 * frame as u64).sum();
    (table as f64 / scale, packets as f64 / scale)
}

/// (packets' time, table's time) at the ends of every stretch that is off;
/// nothing when the two clocks agree
pub(crate) fn points(timescale: u32, entries: &[Entry]) -> Vec<(f64, f64)> {
    let (table, packets) = durations(timescale, entries);
    if (table - packets).abs() < SAME_S {
        return Vec::new();
    }
    let frame = entries.iter().max_by_key(|e| e.0).map(|e| e.1).unwrap_or(1) as u64;
    let scale = timescale.max(1) as f64;
    let (mut p, mut t) = (0u64, 0u64);
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut push = |p: u64, t: u64| {
        let point = (p as f64 / scale, t as f64 / scale);
        if out.last() != Some(&point) {
            out.push(point);
        }
    };
    for &(count, duration) in entries {
        let off = duration as u64 != frame;
        if off {
            push(p, t);
        }
        p += count as u64 * frame;
        t += count as u64 * duration as u64;
        if off {
            push(p, t);
        }
    }
    out
}

/// The clocks of a recording (nothing for a file that needs none)
pub fn points_of(file: &Path) -> Vec<(f64, f64)> {
    read_moov(file).and_then(|m| sound_table(&m)).map(|(scale, entries)| points(scale, &entries)).unwrap_or_default()
}

/// Both durations of a file, for the check after joining chunks
pub fn durations_of(file: &Path) -> Option<(f64, f64)> {
    let (scale, entries) = sound_table(&read_moov(file)?)?;
    Some(durations(scale, &entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_joined_recording_gets_a_point_at_every_join() {
        let joined = [(1407, 1024), (1, 1), (1, 255), (1406, 1024), (1, 1), (1, 255), (1407, 1024)];
        let (table, packets) = durations(48_000, &joined);
        assert!((packets - table - 2.0 * 1792.0 / 48_000.0).abs() < 1e-9);
        // Too small to matter on its own
        assert!(points(48_000, &joined[..4]).is_empty());
        let long: Vec<Entry> = joined.iter().cycle().take(7 * 10).copied().collect();
        let p = points(48_000, &long);
        assert!(!p.is_empty());
        let last = p.last().unwrap();
        assert!(last.0 > last.1);
        assert!(p.windows(2).all(|w| w[0].0 <= w[1].0 && w[0].1 <= w[1].1));
    }

    #[test]
    fn an_even_table_needs_no_points() {
        assert!(points(48_000, &[(81_834, 1024)]).is_empty());
        assert!(points(48_000, &[(3003, 1024), (1, 1023), (1, 1025), (2624, 1024)]).is_empty());
    }

    #[test]
    fn a_file_that_is_not_an_mp4_has_none() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.wav");
        std::fs::write(&file, b"RIFF....WAVEfmt ").unwrap();
        assert!(points_of(&file).is_empty());
        assert!(points_of(&dir.path().join("none.mp4")).is_empty());
    }
}
