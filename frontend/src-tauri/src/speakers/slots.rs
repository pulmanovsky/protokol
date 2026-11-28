//! Meet's separate audio streams as the base of a call's transcript.
//!
//! Meet does not send one stream per participant: it sends a few streams,
//! each carrying one of the loudest speakers of the moment, so a stream goes
//! from person to person many times a minute. When Protokol's Meet window
//! kept each stream's own sound beside its reports (`<report>.slots/`), a
//! recording made at that time is recognised stream by stream: people who
//! talk at once are heard apart, and every piece of speech has one speaker:
//!
//! - the participant Meet's own packets name as the stream's source at that
//!   moment (`streams[].src` of the reports: RTP contributing sources), put
//!   to a name by the tiles lit while that source sounded over the call;
//! - without that, the participant whose tile was lit while the stream
//!   sounded (a participant sounds on one stream at a time).
//!
//! The streams are on the Meet page's wall clock. They are put on the call
//! recording's timeline by their loudness and then by the sound itself, so
//! the pieces, the player and the rest of the meeting share one timeline.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::time_map::TimeMap;
use crate::audio::channel_track::ChannelLevels;
use crate::audio::pauses::Pauses;
use crate::meet::frames::MeetCall;

const RATE: usize = 16_000;
/// Samples in one 100 ms frame
const FRAME: usize = 1_600;
const FRAME_MS: f64 = 100.0;

/// The recording folder's own copy of its streams
pub const FOLDER_FILE: &str = "meet-streams.json";
const FOLDER_DIR: &str = "meet-streams";

/// The streams of one recording on one clock
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Streams {
    /// Wall clock of sample 0 (ms since 1970)
    pub t0: f64,
    /// Each stream's sound (16 kHz mono), all of one length
    pub sound: Vec<Vec<i16>>,
    /// Per stream: the sources Meet's packets named in a 100 ms frame
    /// (frames with none are left out), by frame
    pub src: Vec<Vec<(usize, Vec<u32>)>>,
}

impl Streams {
    pub fn frames(&self) -> usize {
        self.sound.iter().map(|s| s.len().div_ceil(FRAME)).max().unwrap_or(0)
    }
}

// ── Writing what the Meet window sends ──────────────────────────────────────

/// A recording's streams as the Meet window sent them, in the recording's
/// own folder until «Спікери» has read them (then only [`FOLDER_DIR`], the
/// stretches with sound, is kept)
const RAW_DIR: &str = "meet.slots";
/// The page learns of a stop with its next report, so its last blocks of a
/// recording come a little after the stop
const AFTER_STOP: Duration = Duration::from_secs(6);

/// The recording the Meet window's streams belong to now
#[derive(Debug, Clone, PartialEq)]
struct Running {
    folder: PathBuf,
    /// When it started (wall clock, ms since 1970)
    started_ms: f64,
    /// When it stopped: for how long the page's last blocks are still taken,
    /// and the wall clock no block is kept after
    stopped: Option<(Instant, f64)>,
}

static RECORDING: Mutex<Option<Running>> = Mutex::new(None);

fn now_ms() -> f64 {
    chrono::Utc::now().timestamp_millis() as f64
}

/// A recording starts: what the Meet window sends of its streams goes to
/// the recording's folder
pub fn recording_started(folder: &Path) {
    *RECORDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Running { folder: folder.to_path_buf(), started_ms: now_ms(), stopped: None });
}

/// The recording stops: the page's last blocks are still taken for a moment
pub fn recording_stopped(folder: &Path) {
    if let Some(r) = RECORDING.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        if r.folder == folder && r.stopped.is_none() {
            r.stopped = Some((Instant::now(), now_ms()));
        }
    }
}

/// Where the streams sent now are written: the folder of the recording that
/// runs (or stopped a moment ago). None with no recording, and once the
/// recording has its own copy of its streams: nothing is kept outside a
/// recording.
fn raw_dir_at(recording: Option<&Running>, now: Instant) -> Option<PathBuf> {
    let r = recording?;
    if r.stopped.is_some_and(|(t, _)| now.saturating_duration_since(t) > AFTER_STOP) || r.folder.join(FOLDER_FILE).exists() {
        return None;
    }
    Some(r.folder.join(RAW_DIR))
}

/// Did a block of `samples` that started at `t` (wall clock, ms) sound while
/// the recording recorded? Judged by its middle: not before the start, not
/// during a pause, not after the stop.
fn recorded(r: &Running, pauses: &Pauses, t: f64, samples: usize) -> bool {
    let middle = t + samples as f64 * 500.0 / RATE as f64;
    middle >= r.started_ms && r.stopped.map_or(true, |(_, stop)| middle <= stop) && !pauses.within(middle)
}

/// What the Meet window sends of its streams with a report (`slots`) is
/// kept for the recording that runs: only the blocks that sounded while it
/// recorded. The page sends a little from before it learned of the
/// recording and goes on through a pause and until it learns of the stop;
/// none of that is written.
pub fn take(slots: &Value) {
    let Some(r) = RECORDING.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
    let Some(dir) = raw_dir_at(Some(&r), Instant::now()) else { return };
    let pauses = crate::audio::pauses::of(&r.folder);
    write_blocks(&dir, slots, &|t, samples| recorded(&r, &pauses, t, samples));
}

/// Appends the blocks of one report (`slots` of the page's report: per
/// stream its index `i`, the blocks' wall clocks `bt` and sample counts
/// `bn`, and their 16 kHz s16le sound `pcm` in base64) that `keep` takes
/// (by a block's wall clock and sample count) to `dir`: `slot-<i>.pcm` and
/// `slot-<i>.idx` (one line per block: wall-clock ms of its start and its
/// sample count)
pub fn write_blocks(dir: &Path, slots: &Value, keep: &dyn Fn(f64, usize) -> bool) {
    use base64::Engine as _;
    let Some(slots) = slots.as_array().filter(|a| !a.is_empty()) else { return };
    let mut made = false;
    for s in slots {
        let (Some(i), Some(pcm)) = (s.get("i").and_then(|v| v.as_u64()), s.get("pcm").and_then(|v| v.as_str())) else { continue };
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(pcm) else { continue };
        let bt = s.get("bt").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let bn = s.get("bn").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        // The index must tell exactly what the sound file holds
        let samples: u64 = bn.iter().map(|n| n.as_u64().unwrap_or(0)).sum();
        if bt.len() != bn.len() || samples * 2 != bytes.len() as u64 {
            continue;
        }
        let (mut sound, mut lines) = (Vec::with_capacity(bytes.len()), String::new());
        let mut at = 0usize;
        for (t, n) in bt.iter().zip(&bn) {
            let (t, n) = (t.as_f64().unwrap_or(0.0), n.as_u64().unwrap_or(0) as usize);
            if keep(t, n) {
                sound.extend_from_slice(&bytes[at * 2..(at + n) * 2]);
                lines.push_str(&format!("{} {n}\n", t.round() as u64));
            }
            at += n;
        }
        if lines.is_empty() {
            continue;
        }
        if !made {
            if fs::create_dir_all(dir).is_err() {
                return;
            }
            made = true;
        }
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("slot-{i}.pcm"))) {
            let _ = f.write_all(&sound);
        }
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("slot-{i}.idx"))) {
            let _ = f.write_all(lines.as_bytes());
        }
    }
}

// ── Reading what the Meet window wrote ──────────────────────────────────────

/// A block that starts this long after the last one's end is a real gap (samples)
const GAP: i64 = 3_200;
/// A block with no sample this loud is silence
const QUIET: u16 = 48;

/// Blocks (wall clock of the start in ms, samples) put on a timeline that
/// starts at `t0`. A block's clock is only as exact as the page's timer, so
/// speech goes on sample after sample; the timeline is put back on the wall
/// clock in silence and after a real gap.
fn place(blocks: &[(f64, usize)], pcm: &[i16], t0: f64, len: usize) -> Vec<i16> {
    let mut out = vec![0i16; len];
    let mut pos: Option<i64> = None;
    let mut read = 0usize;
    for &(t, n) in blocks {
        let Some(block) = pcm.get(read..read + n) else { break };
        read += n;
        let expected = ((t - t0) * RATE as f64 / 1000.0).round() as i64;
        let quiet = block.iter().all(|s| s.unsigned_abs() < QUIET);
        let at = match pos {
            None => expected,
            Some(p) if quiet => {
                pos = Some(p.max(expected + n as i64));
                continue;
            }
            Some(p) if expected - p > GAP => expected,
            Some(p) => p,
        };
        pos = Some(at + n as i64);
        if quiet {
            continue;
        }
        for (k, s) in block.iter().enumerate() {
            let i = at + k as i64;
            if i >= 0 && (i as usize) < len {
                out[i as usize] = *s;
            }
        }
    }
    out
}

/// One stream's blocks within [from, to] and their samples. The blocks are
/// written by the wall clock; [from, to] and the times given back are on the
/// recording's clock (ms, the wall clock with its `pauses` taken out), and a
/// block that sounded during a pause is left out.
fn read_slot(dir: &Path, i: u64, from: f64, to: f64, pauses: &Pauses) -> Option<(Vec<(f64, usize)>, Vec<i16>)> {
    let idx = fs::read_to_string(dir.join(format!("slot-{i}.idx"))).ok()?;
    let mut blocks: Vec<(f64, usize)> = Vec::new();
    // Where each of them is in the sound file (samples)
    let mut places: Vec<u64> = Vec::new();
    let mut at = 0u64;
    for line in idx.lines() {
        let mut it = line.split_whitespace();
        let (Some(t), Some(n)) = (it.next().and_then(|x| x.parse::<f64>().ok()), it.next().and_then(|x| x.parse::<usize>().ok())) else { break };
        let place = at;
        at += n as u64;
        let len_ms = n as f64 * 1000.0 / RATE as f64;
        let Some(t) = pauses.span_on_recording(t, len_ms) else { continue };
        if t > to {
            break;
        }
        if blocks.is_empty() && t + len_ms < from {
            continue;
        }
        blocks.push((t, n));
        places.push(place);
    }
    let (first, last) = (*places.first()?, *places.last()? + blocks.last()?.1 as u64);
    let mut f = fs::File::open(dir.join(format!("slot-{i}.pcm"))).ok()?;
    f.seek(SeekFrom::Start(first * 2)).ok()?;
    let mut bytes = Vec::with_capacity((last - first) as usize * 2);
    f.take((last - first) * 2).read_to_end(&mut bytes).ok()?;
    let all: Vec<i16> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    let samples: usize = blocks.iter().map(|b| b.1).sum();
    if samples as u64 == last - first {
        return Some((blocks, all));
    }
    // Blocks left out in between: only the sound of those taken
    let mut pcm: Vec<i16> = Vec::with_capacity(samples);
    for (place, (_, n)) in places.iter().zip(&blocks) {
        let a = (place - first) as usize;
        match all.get(a..a + n) {
            Some(part) => pcm.extend_from_slice(part),
            // The sound file is shorter than its index says
            None => break,
        }
    }
    Some((blocks, pcm))
}

/// `streams[].src` of the reports within [from, to] (the recording's clock:
/// the wall clock with its `pauses` taken out): per stream index, the
/// sources named in each frame (frames are 100 ms steps from `from`; frames
/// of a pause are left out)
fn read_sources(report: &Path, from: f64, to: f64, pauses: &Pauses) -> HashMap<u64, Vec<(usize, Vec<u32>)>> {
    let mut out: HashMap<u64, Vec<(usize, Vec<u32>)>> = HashMap::new();
    let Ok(file) = fs::File::open(report) else { return out };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if !line.contains("\"src\"") {
            continue;
        }
        let Ok(r) = serde_json::from_str::<Value>(&line) else { continue };
        let ft: Vec<f64> = r.get("ft").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
        let Some(streams) = r.get("streams").and_then(|v| v.as_array()) else { continue };
        for s in streams {
            let (Some(i), Some(src)) = (s.get("i").and_then(|v| v.as_u64()), s.get("src").and_then(|v| v.as_array())) else { continue };
            // A stream that joined during the report has fewer, the last frames'
            let skip = ft.len().saturating_sub(src.len());
            for (j, x) in src.iter().enumerate() {
                let Some(t) = ft.get(skip + j).copied().and_then(|t| pauses.on_recording(t)) else { continue };
                if t < from || t > to {
                    continue;
                }
                let ids: Vec<u32> = x.as_array().map(|a| a.iter().filter_map(|v| v.as_u64()).map(|v| v as u32).collect()).unwrap_or_default();
                if !ids.is_empty() {
                    out.entry(i).or_default().push((((t - from) / FRAME_MS).round() as usize, ids));
                }
            }
        }
    }
    for v in out.values_mut() {
        v.sort_by_key(|x| x.0);
        v.dedup_by_key(|x| x.0);
    }
    out
}

/// The streams the Meet window wrote within [from, to], from every report
/// folder; None when there is no sound in them. [from, to] is on the
/// recording's clock (ms): the wall clock with its `pauses` taken out, so
/// the streams go on from where the recording was paused, as its sound does.
pub fn from_sessions(dirs: &[PathBuf], from: f64, to: f64, pauses: &Pauses) -> Option<Streams> {
    let len = (((to - from) / 1000.0) * RATE as f64).max(0.0) as usize;
    let mut found: Vec<(String, u64, Vec<i16>, Vec<(usize, Vec<u32>)>)> = Vec::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if !path.is_dir() || path.extension().and_then(|x| x.to_str()) != Some("slots") {
                continue;
            }
            let mut indices: Vec<u64> = fs::read_dir(&path)
                .map(|d| {
                    d.flatten()
                        .filter_map(|f| f.file_name().to_str().and_then(|n| n.strip_prefix("slot-")).and_then(|n| n.strip_suffix(".idx")).and_then(|n| n.parse().ok()))
                        .collect()
                })
                .unwrap_or_default();
            indices.sort_unstable();
            let mut sources: Option<HashMap<u64, Vec<(usize, Vec<u32>)>>> = None;
            for i in indices {
                let Some((blocks, pcm)) = read_slot(&path, i, from, to, pauses) else { continue };
                let sound = place(&blocks, &pcm, from, len);
                if sound.iter().all(|s| s.unsigned_abs() < QUIET) {
                    continue;
                }
                let src = sources.get_or_insert_with(|| read_sources(&path.with_extension("jsonl"), from, to, pauses)).remove(&i).unwrap_or_default();
                found.push((path.to_string_lossy().into_owned(), i, sound, src));
            }
        }
    }
    if found.is_empty() {
        return None;
    }
    found.sort_by(|a, b| (a.0.as_str(), a.1).cmp(&(b.0.as_str(), b.1)));
    info!("Meet streams: {} stream(s) with sound for this recording", found.len());
    let mut out = Streams { t0: from, ..Default::default() };
    for (_, _, sound, src) in found {
        out.sound.push(sound);
        out.src.push(src);
    }
    Some(out)
}

#[derive(Serialize, Deserialize)]
struct Kept {
    v: u32,
    t0: f64,
    len: usize,
    streams: Vec<KeptStream>,
}

#[derive(Serialize, Deserialize)]
struct KeptStream {
    file: String,
    /// (first sample, samples) of each stretch with sound; the file holds them one after another
    runs: Vec<(usize, usize)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    src: Vec<(usize, Vec<u32>)>,
}

/// Stretches with sound (a little silence kept around them)
fn sounding(sound: &[i16]) -> Vec<(usize, usize)> {
    const AROUND: usize = 2;
    let loud: Vec<bool> = sound.chunks(FRAME).map(|c| c.iter().any(|s| s.unsigned_abs() >= QUIET)).collect();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (f, l) in loud.iter().enumerate() {
        if !l {
            continue;
        }
        let (a, b) = (f.saturating_sub(AROUND) * FRAME, ((f + 1 + AROUND) * FRAME).min(sound.len()));
        match out.last_mut() {
            Some(last) if a <= last.0 + last.1 => last.1 = b - last.0,
            _ => out.push((a, b - a)),
        }
    }
    out
}

/// Keeps the streams with the recording (only their stretches with sound),
/// so recognising it again does not depend on the Meet window's own files
pub fn save_folder(folder: &Path, streams: &Streams) -> std::io::Result<()> {
    let dir = folder.join(FOLDER_DIR);
    fs::create_dir_all(&dir)?;
    let mut kept = Kept { v: 1, t0: streams.t0, len: streams.sound.first().map_or(0, |s| s.len()), streams: Vec::new() };
    for (i, sound) in streams.sound.iter().enumerate() {
        let runs = sounding(sound);
        let mut bytes: Vec<u8> = Vec::with_capacity(runs.iter().map(|r| r.1 * 2).sum());
        for (a, n) in &runs {
            for s in &sound[*a..*a + *n] {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
        }
        let file = format!("{i}.pcm");
        fs::write(dir.join(&file), bytes)?;
        kept.streams.push(KeptStream { file, runs, src: streams.src.get(i).cloned().unwrap_or_default() });
    }
    let tmp = folder.join(".meet-streams.json.tmp");
    fs::write(&tmp, serde_json::to_vec(&kept).unwrap_or_default())?;
    fs::rename(tmp, folder.join(FOLDER_FILE))
}

pub fn load_folder(folder: &Path) -> Option<Streams> {
    let kept: Kept = serde_json::from_slice(&fs::read(folder.join(FOLDER_FILE)).ok()?).ok()?;
    let mut out = Streams { t0: kept.t0, ..Default::default() };
    for s in kept.streams {
        let bytes = fs::read(folder.join(FOLDER_DIR).join(&s.file)).ok()?;
        let mut sound = vec![0i16; kept.len];
        let mut read = 0usize;
        for (a, n) in &s.runs {
            let part = bytes.get(read * 2..(read + n) * 2)?;
            read += n;
            for (k, b) in part.chunks_exact(2).enumerate() {
                if let Some(x) = sound.get_mut(a + k) {
                    *x = i16::from_le_bytes([b[0], b[1]]);
                }
            }
        }
        out.sound.push(sound);
        out.src.push(s.src);
    }
    (!out.sound.is_empty()).then_some(out)
}

/// The streams that went with a recording: its own copy, or what the Meet
/// window sent while it was recorded (in its folder; a test build's window
/// keeps files of its own). What was sent is then kept as the recording's
/// own copy, the stretches with sound only, and the rest is deleted.
pub fn for_recording(folder: &Path, duration_s: f64) -> Option<Streams> {
    if let Some(streams) = load_folder(folder) {
        return Some(streams);
    }
    let start = crate::meet::frames::recording_start_ms(folder)?;
    wait_for_last_blocks(folder);
    let mut dirs = vec![folder.to_path_buf()];
    dirs.extend(crate::meet::frames::sessions_dirs());
    let pauses = crate::audio::pauses::of(folder);
    if !pauses.is_empty() {
        info!("Meet streams: the recording was paused {} time(s), {:.1} s in all; the streams go on from where it was paused", pauses.count(), pauses.total() / 1000.0);
    }
    // Two minutes on both sides: the clocks are lined up by the sound later
    let streams = from_sessions(&dirs, start - 120_000.0, start + duration_s * 1000.0 + 120_000.0, &pauses);
    // Nothing is settled while the recording still runs
    if state_of(folder) == Some(None) {
        return streams;
    }
    let raw = folder.join(RAW_DIR);
    match &streams {
        Some(streams) => match save_folder(folder, streams) {
            Ok(()) => drop_raw(&raw),
            Err(e) => warn!("Meet streams: could not keep them with the recording: {e}"),
        },
        // Nothing but silence was sent
        None => drop_raw(&raw),
    }
    streams
}

/// What the Meet window sent for a recording that is not heard stream by
/// stream (no call sound of its own, or the setting is off) is still put in
/// order: its stretches with sound kept, the rest deleted
pub fn settle(folder: &Path, duration_s: f64) {
    if folder.join(RAW_DIR).is_dir() {
        let _ = for_recording(folder, duration_s);
    }
}

/// Some(None): this folder's recording runs now; Some(Some(t)): it stopped at `t`
fn state_of(folder: &Path) -> Option<Option<Instant>> {
    RECORDING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|r| r.folder == folder).map(|r| r.stopped.map(|(at, _)| at))
}

/// A recording read right after its stop: the page's last blocks are waited for
fn wait_for_last_blocks(folder: &Path) {
    if let Some(Some(stopped)) = state_of(folder) {
        if let Some(left) = AFTER_STOP.checked_sub(stopped.elapsed()) {
            std::thread::sleep(left);
        }
    }
}

fn drop_raw(raw: &Path) {
    if raw.is_dir() {
        if let Err(e) = fs::remove_dir_all(raw) {
            warn!("Meet streams: what the window sent could not be deleted: {e}");
        }
    }
}

/// What the window sent for recordings that were never opened afterwards
/// does not stay for ever: older than this, it is deleted
const RAW_KEPT: Duration = Duration::from_secs(7 * 24 * 3600);

/// Deletes what the Meet window sent long ago for recordings under `root`
/// that «Спікери» never read (the recordings themselves are not touched)
pub fn sweep(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else { return };
    for e in entries.flatten() {
        let raw = e.path().join(RAW_DIR);
        let old = fs::metadata(&raw).and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age > RAW_KEPT);
        if old && state_of(&e.path()).is_none() {
            info!("Meet streams: unread streams of an old recording deleted");
            drop_raw(&raw);
        }
    }
}

// ── The streams' clock on the call recording ────────────────────────────────

/// How far the clocks may be apart at first (frames: ±60 s)
const SEARCH: i64 = 600;
/// Loudness correlation needed to trust the match
const MIN_CORR: f32 = 0.35;

/// Where the streams are on the call recording
#[derive(Debug, Clone, Default)]
pub struct Clock {
    /// The streams' frame that sounds in the recording's frame 0
    pub shift: i64,
    /// From there to the recording exactly, by the sound itself
    pub fine: TimeMap,
    pub corr: f32,
}

impl Clock {
    /// A moment of the streams (seconds from their sample 0) on the call recording
    pub fn to_recording(&self, t: f64) -> f64 {
        self.fine.map(t - self.shift as f64 * 0.1)
    }
}

/// All the streams together (as the call recording hears them)
fn sum(streams: &Streams) -> Vec<f32> {
    let len = streams.sound.iter().map(|s| s.len()).max().unwrap_or(0);
    let mut out = vec![0f32; len];
    for s in &streams.sound {
        for (o, x) in out.iter_mut().zip(s) {
            *o += *x as f32 / 32_768.0;
        }
    }
    out
}

/// Puts the streams on the call recording (`system`, 16 kHz) that started at
/// `start_ms` of the wall clock; None when their sound does not match it
pub fn clock(streams: &Streams, system: &[f32], start_ms: f64) -> Option<Clock> {
    let all = sum(streams);
    let env = super::meet::envelope(system);
    let theirs: Vec<Option<f32>> = super::meet::envelope(&all).into_iter().map(Some).collect();
    let base = ((start_ms - streams.t0) / FRAME_MS).round() as i64;
    let (shift, corr) = super::meet::best_offset(&env, &theirs, base - SEARCH, base + SEARCH, 0..env.len())?;
    if corr < MIN_CORR {
        info!("Meet streams: their loudness does not match the recording (best {corr:.2})");
        return None;
    }
    // The same sound sample by sample: the streams as the recording's clock
    // would have them, then where 10 s windows of the recording are in them
    let moved: Vec<f32> = (0..system.len())
        .map(|n| {
            let i = n as i64 + shift * FRAME as i64;
            if i >= 0 { all.get(i as usize).copied().unwrap_or(0.0) } else { 0.0 }
        })
        .collect();
    let fine = super::time_map::fit(system, &moved);
    info!("Meet streams: put on the recording ({:+.1} s from the clocks, loudness match {corr:.2}, {} exact point(s))", (shift - base) as f64 * 0.1, fine.points.len());
    Some(Clock { shift, fine, corr })
}

// ── Whose each stretch of a stream is ───────────────────────────────────────

/// Loudness (RMS of 16-bit samples) that counts as sound on a stream
const ACTIVE: f32 = 200.0;
/// Quiet frames inside one stretch of sound
const BRIDGE: usize = 3;
/// Loud frames a stretch needs
const MIN_ACTIVE: usize = 2;
/// A source this many frames a participant: that participant
const NAME_VOTES: f32 = 5.0;
/// What a frame says for the source Meet's packets name / for a lit tile
const PACKET_WEIGHT: f32 = 3.0;
/// A speaker already sounding on another stream then
const BUSY: f32 = 1.5;
/// A tile stays lit this long after its sound (frames)
const LINGER: usize = 5;
/// What a frame is worth with nobody named
const NOBODY: f32 = 0.15;
/// What changing the speaker inside a stretch costs
const SWITCH: f32 = 4.0;
/// A tile lit this soon after a short stretch still names it (frames)
const LATE_TILE: usize = 12;
const LATE_TILE_VOTES: f32 = 2.0;
/// A stretch nobody named takes its neighbour on the stream this close (frames)
const NEIGHBOUR: usize = 10;

/// One stretch of one stream (frames) and its speaker, if known
#[derive(Debug, Clone, PartialEq)]
pub struct Stretch {
    pub stream: usize,
    pub from: usize,
    pub to: usize,
    pub speaker: Option<usize>,
}

/// The streams' stretches by speaker
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Owned {
    pub stretches: Vec<Stretch>,
    /// Each speaker's participant name; none for a source no tile named
    pub names: Vec<Option<String>>,
    /// Stretches named by Meet's packets (the rest by lit tiles)
    pub by_packets: usize,
}

/// Loudness of each 100 ms frame of a stream
fn loudness(sound: &[i16]) -> Vec<f32> {
    sound.chunks(FRAME).map(|c| (c.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>() / c.len().max(1) as f64).sqrt() as f32).collect()
}

/// Stretches of sound: [from, to) in frames
fn runs(level: &[f32]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize, usize)> = Vec::new();
    for (f, l) in level.iter().enumerate() {
        if *l < ACTIVE {
            continue;
        }
        match out.last_mut() {
            Some(r) if f <= r.1 + BRIDGE => {
                r.1 = f + 1;
                r.2 += 1;
            }
            _ => out.push((f, f + 1, 1)),
        }
    }
    out.into_iter().filter(|r| r.2 >= MIN_ACTIVE).map(|r| (r.0, r.1)).collect()
}

/// How many frames after the sound its marks come: the delay within `range`
/// at which most frames with sound have a mark
fn delay(sound: &[Vec<bool>], marked: &dyn Fn(usize, usize) -> bool, range: std::ops::RangeInclusive<i64>) -> i64 {
    let mut best: (i64, usize) = (0, 0);
    for d in range {
        let mut n = 0usize;
        for (s, frames) in sound.iter().enumerate() {
            for (f, on) in frames.iter().enumerate() {
                let j = f as i64 + d;
                if *on && j >= 0 && marked(s, j as usize) {
                    n += 1;
                }
            }
        }
        if n > best.1 || (n == best.1 && n > 0 && d.abs() < best.0.abs()) {
            best = (d, n);
        }
    }
    best.0
}

/// Whose each stretch of sound is. `lit`: the participants lit in each frame
/// of the streams' clock (indices into `people`); `me`: the user's own tile.
pub fn owners(level: &[Vec<f32>], src: &[Vec<(usize, Vec<u32>)>], lit: &[Vec<usize>], people: &[String], me: Option<usize>) -> Owned {
    let streams = level.len();
    let frames = level.iter().map(|l| l.len()).max().unwrap_or(0);
    let active: Vec<Vec<bool>> = level.iter().map(|l| l.iter().map(|x| *x >= ACTIVE).collect()).collect();
    let any_lit = |_: usize, f: usize| lit.get(f).is_some_and(|l| l.iter().any(|p| Some(*p) != me));
    let tile_delay = delay(&active, &any_lit, 0..=6);
    let lit_at = |f: usize| -> Vec<usize> {
        let j = f as i64 + tile_delay;
        if j < 0 {
            return Vec::new();
        }
        lit.get(j as usize).map(|l| l.iter().copied().filter(|p| Some(*p) != me && *p < people.len()).collect()).unwrap_or_default()
    };

    // The sources Meet's packets name. One that is loud on several streams
    // at once is not a participant (Meet marks every stream with it).
    let named: Vec<HashMap<usize, &Vec<u32>>> = (0..streams).map(|s| src.get(s).map(|v| v.iter().map(|(f, ids)| (*f, ids)).collect()).unwrap_or_default()).collect();
    let mut seen: HashMap<u32, (usize, usize)> = HashMap::new();
    for s in 0..streams {
        for (f, ids) in &named[s] {
            for id in ids.iter() {
                let elsewhere = (0..streams).any(|o| o != s && named[o].get(f).is_some_and(|x| x.contains(id)));
                let e = seen.entry(*id).or_default();
                e.0 += 1;
                e.1 += elsewhere as usize;
            }
        }
    }
    let ignored = |id: u32| id < 256 || seen.get(&id).is_some_and(|(n, twice)| *n >= 10 && twice * 2 >= *n);
    let mut source: Vec<HashMap<usize, u32>> = vec![HashMap::new(); streams];
    for s in 0..streams {
        let mut fs: Vec<usize> = named[s].keys().copied().collect();
        fs.sort_unstable();
        let mut last: Option<(usize, u32)> = None;
        for f in fs {
            let ids: Vec<u32> = named[s][&f].iter().copied().filter(|id| !ignored(*id)).collect();
            let pick = match last {
                Some((lf, id)) if lf + 1 == f && ids.contains(&id) => Some(id),
                _ => ids.first().copied(),
            };
            if let Some(id) = pick {
                source[s].insert(f, id);
                last = Some((f, id));
            }
        }
    }
    let has_source = |s: usize, f: usize| source[s].contains_key(&f);
    let packet_delay = delay(&active, &has_source, -8..=8);
    let source_at = |s: usize, f: usize| -> Option<u32> {
        let j = f as i64 + packet_delay;
        if j < 0 { None } else { source[s].get(&(j as usize)).copied() }
    };

    // Each source's participant: the tiles lit while it sounded. First by the
    // frames where it sounded alone; a source that only ever spoke over
    // others gets the tile the others' names leave.
    let mut heard: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut at_frame: HashMap<usize, Vec<u32>> = HashMap::new();
    for s in 0..streams {
        for f in 0..level[s].len() {
            if !active[s][f] {
                continue;
            }
            if let Some(id) = source_at(s, f) {
                heard.entry(id).or_default().push(f);
                at_frame.entry(f).or_default().push(id);
            }
        }
    }
    let mut ids: Vec<u32> = heard.keys().copied().collect();
    ids.sort_unstable();
    let mut part_of: HashMap<u32, usize> = HashMap::new();
    for round in 0..3 {
        for id in &ids {
            if part_of.contains_key(id) {
                continue;
            }
            let mut v = vec![0f32; people.len()];
            for f in &heard[id] {
                let others: Vec<u32> = at_frame[f].iter().copied().filter(|x| x != id).collect();
                if round == 0 && !others.is_empty() {
                    continue;
                }
                let taken: Vec<usize> = others.iter().filter_map(|x| part_of.get(x).copied()).collect();
                let l: Vec<usize> = lit_at(*f).into_iter().filter(|p| !taken.contains(p)).collect();
                for p in &l {
                    v[*p] += 1.0 / l.len() as f32;
                }
            }
            // A participant has one source: later rounds choose among those left
            let mut order: Vec<usize> = (0..v.len()).filter(|p| round == 0 || !part_of.values().any(|x| x == p)).collect();
            order.sort_by(|a, b| v[*b].total_cmp(&v[*a]));
            let top = order.first().map(|p| v[*p]).unwrap_or(0.0);
            let next = order.get(1).map(|p| v[*p]).unwrap_or(0.0);
            if top >= NAME_VOTES && top >= 2.0 * next {
                part_of.insert(*id, order[0]);
            }
        }
    }
    // Speakers: participants first come first, then sources no tile named
    let mut names: Vec<Option<String>> = Vec::new();
    let mut of_part: HashMap<usize, usize> = HashMap::new();
    let mut of_source: HashMap<u32, usize> = HashMap::new();
    for id in ids {
        let k = match part_of.get(&id) {
            Some(p) => *of_part.entry(*p).or_insert_with(|| {
                names.push(Some(people[*p].clone()));
                names.len() - 1
            }),
            None => {
                names.push(None);
                names.len() - 1
            }
        };
        of_source.insert(id, k);
    }
    let mut speaker_of_part = |p: usize, names: &mut Vec<Option<String>>| -> usize {
        *of_part.entry(p).or_insert_with(|| {
            names.push(Some(people[p].clone()));
            names.len() - 1
        })
    };

    // What each frame of each stretch says, then the likeliest speakers of
    // the stretch: long stretches first, each speaker on one stream at a time
    const NO_SOUND: i32 = -2;
    const UNKNOWN: i32 = -1;
    let mut all_runs: Vec<(usize, usize, usize)> = Vec::new();
    for s in 0..streams {
        all_runs.extend(runs(&level[s]).into_iter().map(|(a, b)| (s, a, b)));
    }
    all_runs.sort_by(|a, b| (b.2 - b.1).cmp(&(a.2 - a.1)).then(a.cmp(b)));
    let mut said: Vec<Vec<Vec<(usize, f32)>>> = Vec::with_capacity(all_runs.len());
    let mut packets: Vec<bool> = Vec::with_capacity(all_runs.len());
    for &(s, a, b) in &all_runs {
        let by_packets = (a..b).any(|f| source_at(s, f).is_some());
        packets.push(by_packets);
        said.push(
            (a..b)
                .map(|f| {
                    if by_packets {
                        source_at(s, f).and_then(|id| of_source.get(&id).copied()).map(|k| vec![(k, PACKET_WEIGHT)]).unwrap_or_default()
                    } else {
                        let l = lit_at(f);
                        l.iter().map(|p| (speaker_of_part(*p, &mut names), 1.0 / l.len() as f32)).collect()
                    }
                })
                .collect(),
        );
    }
    let mut owner: Vec<Vec<i32>> = (0..streams).map(|s| vec![NO_SOUND; level[s].len().max(frames)]).collect();
    let busy = |owner: &Vec<Vec<i32>>, k: usize, f: usize, s: usize| (0..streams).any(|o| o != s && owner[o].get(f).copied() == Some(k as i32));
    let lingers = |owner: &Vec<Vec<i32>>, k: usize, f: usize, s: usize| (f.saturating_sub(LINGER)..=f).any(|g| busy(owner, k, g, s));
    for _pass in 0..2 {
        for (r, &(s, a, b)) in all_runs.iter().enumerate() {
            let mut who: Vec<usize> = said[r].iter().flatten().map(|x| x.0).collect();
            who.sort_unstable();
            who.dedup();
            let n = who.len() + 1; // the last state: nobody named
            let t = b - a;
            let mut score = vec![vec![0f32; n]; t];
            let mut from = vec![vec![0usize; n]; t];
            for i in 0..t {
                let f = a + i;
                for (j, &k) in who.iter().enumerate() {
                    let mut e = said[r][i].iter().find(|x| x.0 == k).map(|x| x.1).unwrap_or(0.0);
                    // A tile still lit for what its participant says (or just said) on another stream
                    if !packets[r] && e > 0.0 && lingers(&owner, k, f, s) {
                        e = 0.0;
                    }
                    if busy(&owner, k, f, s) {
                        e -= BUSY;
                    }
                    score[i][j] = e;
                }
                score[i][n - 1] = NOBODY;
                if i > 0 {
                    let (best, best_j) = (0..n).map(|j| (score[i - 1][j], j)).fold((f32::NEG_INFINITY, 0), |m, x| if x.0 > m.0 { x } else { m });
                    for j in 0..n {
                        let (stay, change) = (score[i - 1][j], best - SWITCH);
                        if stay >= change {
                            score[i][j] += stay;
                            from[i][j] = j;
                        } else {
                            score[i][j] += change;
                            from[i][j] = best_j;
                        }
                    }
                }
            }
            let mut j = (0..n).map(|j| (score[t - 1][j], j)).fold((f32::NEG_INFINITY, 0), |m, x| if x.0 > m.0 { x } else { m }).1;
            for i in (0..t).rev() {
                owner[s][a + i] = if j == n - 1 { UNKNOWN } else { who[j] as i32 };
                j = from[i][j];
            }
        }
    }

    // Stretches nobody named: a tile lit a little late (a short remark is
    // over before its tile lights up), else the neighbour on the stream
    for s in 0..streams {
        let mut f = 0;
        while f < owner[s].len() {
            if owner[s][f] != UNKNOWN {
                f += 1;
                continue;
            }
            let a = f;
            while f < owner[s].len() && owner[s][f] == UNKNOWN {
                f += 1;
            }
            let b = f;
            let mut w: HashMap<usize, f32> = HashMap::new();
            for g in a..(b + LATE_TILE).min(frames) {
                let l = lit_at(g);
                for p in &l {
                    let k = speaker_of_part(*p, &mut names);
                    if !lingers(&owner, k, g, s) && !(a..b).any(|x| busy(&owner, k, x, s)) {
                        *w.entry(k).or_default() += 1.0 / l.len() as f32;
                    }
                }
            }
            let mut order: Vec<(usize, f32)> = w.into_iter().collect();
            order.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
            let mut pick = order.first().filter(|x| x.1 >= LATE_TILE_VOTES && order.get(1).map_or(true, |n| x.1 >= 1.5 * n.1)).map(|x| x.0);
            if pick.is_none() {
                let before = (a.saturating_sub(NEIGHBOUR)..a).rev().find_map(|g| (owner[s][g] >= 0).then_some(owner[s][g] as usize));
                let after = (b..(b + NEIGHBOUR).min(owner[s].len())).find_map(|g| (owner[s][g] >= 0).then_some(owner[s][g] as usize));
                pick = [before, after].into_iter().flatten().find(|k| !(a..b).any(|x| busy(&owner, *k, x, s)));
            }
            if let Some(k) = pick {
                owner[s][a..b].iter_mut().for_each(|x| *x = k as i32);
            }
        }
    }

    // A change of speaker inside a stretch is cut at the quietest frame around it
    for s in 0..streams {
        for f in 1..owner[s].len() {
            let (was, now) = (owner[s][f - 1], owner[s][f]);
            if was == now || was == NO_SOUND || now == NO_SOUND {
                continue;
            }
            let (lo, hi) = (f.saturating_sub(2), (f + 2).min(owner[s].len() - 1));
            let loud = |g: usize| level[s].get(g).copied().unwrap_or(0.0);
            let around = (lo..=hi).filter(|g| owner[s][*g] == was || owner[s][*g] == now);
            let Some(q) = around.min_by(|x, y| loud(*x).total_cmp(&loud(*y))) else { continue };
            // Only to a pause: a frame clearly quieter than the speech on both sides
            if loud(q) > 0.5 * loud(f - 1).min(loud(f)) {
                continue;
            }
            for g in q..f {
                if owner[s][g] == was {
                    owner[s][g] = now;
                }
            }
            for g in f..q {
                if owner[s][g] == now {
                    owner[s][g] = was;
                }
            }
        }
    }

    let mut out = Owned { names, ..Default::default() };
    for s in 0..streams {
        let mut f = 0;
        while f < owner[s].len() {
            let k = owner[s][f];
            if k == NO_SOUND {
                f += 1;
                continue;
            }
            let a = f;
            while f < owner[s].len() && owner[s][f] == k {
                f += 1;
            }
            out.stretches.push(Stretch { stream: s, from: a, to: f, speaker: (k >= 0).then_some(k as usize) });
        }
    }
    out.by_packets = all_runs.iter().zip(&packets).filter(|x| *x.1).count();
    info!(
        "Meet streams: {} stretch(es) of sound, {} speaker(s) ({} by name), {} stretch(es) named by Meet's packets, {} with nobody named; tiles light {tile_delay} frame(s) after the sound",
        out.stretches.len(),
        out.names.len(),
        out.names.iter().filter(|n| n.is_some()).count(),
        out.by_packets,
        out.stretches.iter().filter(|x| x.speaker.is_none()).count(),
    );
    out
}

// ── The pieces Whisper hears ────────────────────────────────────────────────

/// The same speaker goes on over a pause this long (frames)
const PAUSE: usize = 20;
/// Sound kept before and after a piece (samples)
const LEAD: usize = 3_200;
const TAIL: usize = 4_800;
/// Speech a piece needs (samples)
const MIN_SPEECH: usize = 4_000;
/// The longest piece sent to Whisper at once (as on import)
const MAX_PIECE: usize = 25 * RATE;
/// Share of the recording's sound no stream has, above which the streams are not used
const MAX_UNCOVERED: f64 = 0.15;

/// One piece of one stream (samples) with its speaker
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    pub stream: usize,
    pub from: usize,
    pub to: usize,
    pub speaker: usize,
}

/// A recording's call by its streams: who speaks in which piece
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub clock: Clock,
    /// Each speaker's participant name; none for a voice Meet did not name
    pub names: Vec<Option<String>>,
    /// By start
    pub pieces: Vec<Piece>,
    /// Stretches named by Meet's packets
    pub by_packets: usize,
    /// Share of the recording's sound that is on no stream
    pub uncovered: f64,
    /// The user's own tile
    pub self_name: Option<String>,
    /// Pieces given to another speaker by their voice: (piece, whose it was by the tiles)
    pub by_voice: Vec<(usize, String)>,
}

impl Plan {
    /// What a piece's words keep of its speaker: the participant's name, or
    /// «#n» for the n-th speaker when Meet did not name it
    pub fn key(&self, speaker: usize) -> String {
        match self.names.get(speaker).cloned().flatten() {
            Some(name) => name,
            None => format!("#{speaker}"),
        }
    }

    pub fn speaker_of(&self, key: &str) -> Option<usize> {
        (0..self.names.len()).find(|k| self.key(*k) == key)
    }

    /// A piece on the call recording's timeline (seconds)
    pub fn span(&self, p: &Piece) -> (f64, f64) {
        (self.clock.to_recording(p.from as f64 / RATE as f64).max(0.0), self.clock.to_recording(p.to as f64 / RATE as f64).max(0.0))
    }

    /// Speakers left without a piece are dropped and the rest renumbered;
    /// returns which of the earlier speakers stay, in their new order
    fn compact(&mut self) -> Vec<usize> {
        let kept: Vec<usize> = (0..self.names.len()).filter(|k| self.pieces.iter().any(|p| p.speaker == *k)).collect();
        if kept.len() < self.names.len() {
            for p in &mut self.pieces {
                p.speaker = kept.iter().position(|k| *k == p.speaker).unwrap_or(0);
            }
            self.names = kept.iter().map(|k| self.names[*k].clone()).collect();
        }
        kept
    }

    /// Seconds of speech of each speaker
    pub fn speech_s(&self) -> Vec<f64> {
        let mut out = vec![0.0; self.names.len()];
        for p in &self.pieces {
            if let Some(x) = out.get_mut(p.speaker) {
                *x += (p.to - p.from) as f64 / RATE as f64;
            }
        }
        out
    }
}

/// A piece's sound for Whisper
pub fn sound_of(streams: &Streams, p: &Piece) -> Vec<f32> {
    streams.sound.get(p.stream).and_then(|s| s.get(p.from..p.to.min(s.len()))).map(|s| s.iter().map(|x| *x as f32 / 32_768.0).collect()).unwrap_or_default()
}

/// Where a stream has speech (samples), by the same detector as on import
fn speech(sound: &[i16]) -> Vec<(usize, usize)> {
    let samples: Vec<f32> = sound.iter().map(|s| *s as f32 / 32_768.0).collect();
    match crate::audio::vad::get_speech_chunks(&samples, 2000) {
        Ok(chunks) => chunks.iter().map(|c| ((c.start_timestamp_ms * 16.0).max(0.0) as usize, ((c.end_timestamp_ms * 16.0) as usize).min(sound.len()))).filter(|(a, b)| b > a).collect(),
        Err(e) => {
            warn!("Meet streams: speech not found in a stream ({e}); every stretch of sound is taken");
            vec![(0, sound.len())]
        }
    }
}

/// The quietest 100 ms within [from, to): where a long piece is cut
fn quietest(sound: &[i16], from: usize, to: usize) -> usize {
    let mut best = (from, f64::INFINITY);
    let mut a = from;
    while a + FRAME <= to.min(sound.len()) {
        let e: f64 = sound[a..a + FRAME].iter().map(|s| (*s as f64) * (*s as f64)).sum();
        if e < best.1 {
            best = (a + FRAME / 2, e);
        }
        a += FRAME / 2;
    }
    best.0
}

/// Pieces from the stretches: one speaker's stretches on a stream joined
/// over short pauses, kept where the stream has speech, at most 25 s each.
/// `spoken`: per stream, where speech is (samples); `unknown`: the speaker
/// of stretches nobody named.
fn pieces(owned: &Owned, sound: &[Vec<i16>], spoken: &[Vec<(usize, usize)>], unknown: usize) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    for (s, sound) in sound.iter().enumerate() {
        let mut list: Vec<&Stretch> = owned.stretches.iter().filter(|x| x.stream == s).collect();
        list.sort_by_key(|x| x.from);
        // (speaker, from, to) in frames
        let mut spans: Vec<(usize, usize, usize)> = Vec::new();
        for x in list {
            let k = x.speaker.unwrap_or(unknown);
            match spans.last_mut() {
                Some(last) if last.0 == k && x.from <= last.2 + PAUSE => last.2 = x.to,
                _ => spans.push((k, x.from, x.to)),
            }
        }
        for j in 0..spans.len() {
            let (k, a, b) = spans[j];
            // Not into the neighbours: half of the pause each, a change of speaker exactly
            let lo = if j > 0 { (spans[j - 1].2 + a) * FRAME / 2 } else { 0 };
            let hi = if j + 1 < spans.len() { (b + spans[j + 1].1) * FRAME / 2 } else { sound.len() };
            let from = (a * FRAME).saturating_sub(LEAD).max(lo);
            let to = (b * FRAME + TAIL).min(hi).min(sound.len());
            if to <= from {
                continue;
            }
            let inside: Vec<(usize, usize)> = spoken.get(s).map(|v| v.iter().filter(|r| r.1 > from && r.0 < to).map(|r| (r.0.max(from), r.1.min(to))).collect()).unwrap_or_default();
            if inside.iter().map(|r| r.1 - r.0).sum::<usize>() < MIN_SPEECH {
                continue;
            }
            let (mut from, to) = (inside[0].0, inside[inside.len() - 1].1);
            while to - from > MAX_PIECE {
                let cut = quietest(sound, from + MAX_PIECE - 5 * RATE, from + MAX_PIECE);
                out.push(Piece { stream: s, from, to: cut, speaker: k });
                from = cut;
            }
            out.push(Piece { stream: s, from, to, speaker: k });
        }
    }
    out.sort_by(|a, b| a.from.cmp(&b.from).then(a.stream.cmp(&b.stream)));
    out
}

/// The call of a recording by its streams. `system`: the call recording
/// (16 kHz) that started at `start_ms`; `levels`: the microphone and call
/// loudness, to know the user's own tile. None: the streams do not fit the
/// recording or cover too little of it (then the recording is recognised as
/// before).
pub fn plan(streams: &Streams, call: &MeetCall, system: &[f32], levels: Option<&ChannelLevels>, start_ms: f64) -> Option<Plan> {
    plan_with(streams, call, system, levels, start_ms, &speech)
}

fn plan_with(streams: &Streams, call: &MeetCall, system: &[f32], levels: Option<&ChannelLevels>, start_ms: f64, find_speech: &dyn Fn(&[i16]) -> Vec<(usize, usize)>) -> Option<Plan> {
    let clock = clock(streams, system, start_ms)?;
    let level: Vec<Vec<f32>> = streams.sound.iter().map(|s| loudness(s)).collect();
    let frames = streams.frames();
    // The lit tiles on the streams' clock (both are the page's wall clock)
    let mut lit: Vec<Vec<usize>> = vec![Vec::new(); frames];
    for (i, t) in call.t.iter().enumerate() {
        let f = ((t - streams.t0) / FRAME_MS).round();
        if f >= 0.0 && (f as usize) < frames {
            lit[f as usize] = call.lit.get(i).cloned().unwrap_or_default();
        }
    }
    // The user's own tile: lit while this Mac's microphone speaks
    let on_recording: Vec<Option<Vec<usize>>> = (0..system.len() / FRAME)
        .map(|i| {
            let f = i as i64 + clock.shift + 1;
            (f >= 0).then(|| lit.get(f as usize).cloned()).flatten()
        })
        .collect();
    let me = super::meet::find_self(&on_recording, levels, call.names.len());
    let owned = owners(&level, &streams.src, &lit, &call.names, me);
    if owned.stretches.is_empty() {
        info!("Meet streams: no sound in them for this recording");
        return None;
    }

    // How much of the recording's sound is on no stream (the Meet window
    // opened late, another call, a video played on this Mac)
    let rms: Vec<f32> = system.chunks(FRAME).map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len().max(1) as f32).sqrt()).collect();
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let loud = (sorted.get(sorted.len() * 95 / 100).copied().unwrap_or(0.0) * 0.1).max(0.005);
    let sounding = |f: i64| f >= 0 && level.iter().any(|l| l.get(f as usize).is_some_and(|x| *x >= ACTIVE));
    let (mut heard, mut alone) = (0usize, 0usize);
    for (i, r) in rms.iter().enumerate() {
        if *r >= loud {
            heard += 1;
            let f = i as i64 + clock.shift;
            if !(-3..=3).any(|d| sounding(f + d)) {
                alone += 1;
            }
        }
    }
    let uncovered = if heard > 0 { alone as f64 / heard as f64 } else { 0.0 };
    if uncovered > MAX_UNCOVERED {
        info!("Meet streams: {:.0}% of the recording's sound is on no stream; not used", uncovered * 100.0);
        return None;
    }

    let spoken: Vec<Vec<(usize, usize)>> = streams.sound.iter().map(|s| find_speech(s)).collect();
    let mut names = owned.names.clone();
    let unknown = names.len();
    // Only what sounds inside the recording: the streams are read with two
    // minutes on both sides of it, and people talk before «Почати запис»
    // and after the recording is stopped
    let lo = clock.shift.max(0) as usize * FRAME;
    let hi = (clock.shift + (system.len() / FRAME) as i64).max(0) as usize * FRAME;
    let pieces: Vec<Piece> = pieces(&owned, &streams.sound, &spoken, unknown)
        .into_iter()
        .filter_map(|mut p| {
            p.from = p.from.max(lo);
            p.to = p.to.min(hi);
            (p.to > p.from && p.to - p.from >= MIN_SPEECH).then_some(p)
        })
        .collect();
    if pieces.iter().any(|p| p.speaker == unknown) {
        names.push(None);
    }
    if pieces.is_empty() {
        info!("Meet streams: no speech found in them");
        return None;
    }
    info!("Meet streams: {} piece(s) of speech, {:.0}% of the recording's sound on no stream", pieces.len(), uncovered * 100.0);
    let mut plan = Plan { clock, names, pieces, by_packets: owned.by_packets, uncovered, self_name: me.and_then(|p| call.names.get(p).cloned()), by_voice: Vec::new() };
    // Somebody whose every stretch was too short for a piece is not a speaker
    plan.compact();
    Some(plan)
}

// ── The speakers' voices ────────────────────────────────────────────────────

/// How a speaker's voice is measured: its longest pieces, a part of each
const MEASURE_PIECES: usize = 15;
const MEASURE_MIN: usize = RATE;
const MEASURE_MAX: usize = 8 * RATE;
/// A piece nobody named sounds this much like a named speaker, and this much more than like the next: that speaker's
const SAME_VOICE: f32 = 0.55;
const SAME_VOICE_MARGIN: f32 = 0.08;
/// A piece the tiles gave to one speaker sounds this much more like another: the other's
const OTHER_VOICE_MARGIN: f32 = 0.2;

/// Every speaker's voice measured from its own pieces (clean: one voice per
/// stream at a time). Pieces nobody named go to the named speaker they
/// clearly sound like, and where only the tiles named the speakers a piece
/// that clearly sounds like another speaker goes to that one; a speaker left
/// with no piece is dropped. A speaker too short to measure gets an empty voice.
pub fn voices(streams: &Streams, plan: &mut Plan, embed: &mut dyn FnMut(&[f32]) -> Option<Vec<f32>>) -> Vec<Vec<f32>> {
    let n = plan.names.len();
    fn measure(streams: &Streams, pieces: &[&Piece], embed: &mut dyn FnMut(&[f32]) -> Option<Vec<f32>>) -> Option<Vec<f32>> {
        let mut long: Vec<&&Piece> = pieces.iter().filter(|p| p.to - p.from >= MEASURE_MIN).collect();
        long.sort_by(|a, b| (b.to - b.from).cmp(&(a.to - a.from)));
        let mut found: Vec<Vec<f32>> = Vec::new();
        for p in long.into_iter().take(MEASURE_PIECES) {
            let part = Piece { to: p.to.min(p.from + MEASURE_MAX), ..(**p).clone() };
            if let Some(e) = embed(&sound_of(streams, &part)) {
                found.push(e);
            }
        }
        if found.is_empty() {
            // Only short remarks: all of them together
            let all: Vec<f32> = pieces.iter().flat_map(|p| sound_of(streams, p)).take(MEASURE_MAX).collect();
            if all.len() >= RATE / 2 {
                found.extend(embed(&all));
            }
        }
        super::embedding::centroid(found.iter())
    }
    let mut out: Vec<Option<Vec<f32>>> = (0..n).map(|k| measure(streams, &plan.pieces.iter().filter(|p| p.speaker == k).collect::<Vec<_>>(), embed)).collect();
    // Pieces nobody named, by voice
    let nameless: Vec<usize> = (0..n).filter(|k| plan.names[*k].is_none()).collect();
    let named: Vec<usize> = (0..n).filter(|k| plan.names[*k].is_some() && out[*k].is_some()).collect();
    let mut moved = 0usize;
    if !named.is_empty() {
        for i in 0..plan.pieces.len() {
            let p = plan.pieces[i].clone();
            if !nameless.contains(&p.speaker) || p.to - p.from < MEASURE_MIN {
                continue;
            }
            let part = Piece { to: p.to.min(p.from + MEASURE_MAX), ..p.clone() };
            let Some(e) = embed(&sound_of(streams, &part)) else { continue };
            let mut like: Vec<(f32, usize)> = named.iter().map(|k| (super::embedding::cosine(&e, out[*k].as_ref().unwrap()), *k)).collect();
            like.sort_by(|a, b| b.0.total_cmp(&a.0));
            if like[0].0 >= SAME_VOICE && like.get(1).map_or(true, |x| like[0].0 - x.0 >= SAME_VOICE_MARGIN) {
                plan.pieces[i].speaker = like[0].1;
                moved += 1;
            }
        }
    }
    if moved > 0 {
        info!("Meet streams: {moved} piece(s) nobody named given to a named speaker by voice");
        for k in nameless {
            out[k] = measure(streams, &plan.pieces.iter().filter(|p| p.speaker == k).collect::<Vec<_>>(), embed);
        }
    }
    // A second opinion where only the tiles named the speakers: while two
    // people talk at once the other one's tile may win a stretch. A piece
    // that clearly sounds like another speaker is that speaker's. (Packets
    // name the speaker exactly: nothing is moved then.)
    let known: Vec<usize> = (0..n).filter(|k| out[*k].is_some()).collect();
    if plan.by_packets == 0 && known.len() >= 2 {
        for i in 0..plan.pieces.len() {
            let p = plan.pieces[i].clone();
            let Some(own) = out.get(p.speaker).and_then(|v| v.as_ref()) else { continue };
            if p.to - p.from < MEASURE_MIN {
                continue;
            }
            let part = Piece { to: p.to.min(p.from + MEASURE_MAX), ..p.clone() };
            let Some(e) = embed(&sound_of(streams, &part)) else { continue };
            let own = super::embedding::cosine(&e, own);
            let other = known.iter().filter(|k| **k != p.speaker).map(|k| (super::embedding::cosine(&e, out[*k].as_ref().unwrap()), *k)).max_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((like, k)) = other {
                if like >= SAME_VOICE && like - own >= OTHER_VOICE_MARGIN {
                    plan.by_voice.push((i, plan.key(p.speaker)));
                    plan.pieces[i].speaker = k;
                }
            }
        }
        if !plan.by_voice.is_empty() {
            info!("Meet streams: {} piece(s) given to another speaker by their voice", plan.by_voice.len());
        }
    }
    // A nameless speaker whose pieces all went to named ones is gone
    let kept = plan.compact();
    let size = out.iter().flatten().map(|v| v.len()).next().unwrap_or(0);
    kept.into_iter().map(|k| out[k].take().unwrap_or_else(|| vec![0.0; size])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream: `who[second]` speaks (a tone of its own), silence otherwise
    fn stream(secs: usize, on: impl Fn(usize) -> bool) -> Vec<i16> {
        let mut out = Vec::with_capacity(secs * RATE);
        for x in 0..secs {
            let amp = if on(x) { 3000.0 + 1500.0 * (((x as u64 * 2_654_435_761) >> 9) % 5) as f32 } else { 0.0 };
            out.extend((0..RATE).map(|i| (amp * ((i as f32) * 0.05).sin()) as i16));
        }
        out
    }

    /// Tiles lit `lag` frames after each second's speaker(s), kept `linger` frames
    fn tiles(secs: usize, who: impl Fn(usize) -> Vec<usize>, lag: usize, linger: usize) -> Vec<Vec<usize>> {
        let mut out = vec![Vec::new(); secs * 10 + 20];
        for x in 0..secs {
            for p in who(x) {
                for f in (x * 10 + lag)..(x * 10 + 10 + lag + linger) {
                    if !out[f].contains(&p) {
                        out[f].push(p);
                    }
                }
            }
        }
        out
    }

    fn people(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("Учасник {}", i + 1)).collect()
    }

    fn speaker_at(o: &Owned, stream: usize, frame: usize) -> Option<&str> {
        o.stretches.iter().find(|x| x.stream == stream && x.from <= frame && frame < x.to).and_then(|x| x.speaker).and_then(|k| o.names[k].as_deref())
    }

    #[test]
    fn blocks_follow_the_wall_clock_in_silence_only() {
        // 100-sample blocks, 6.25 ms each; the page's timer is 3 ms late on the second
        let loud = vec![1000i16; 100];
        let quiet = vec![0i16; 100];
        let mut pcm = Vec::new();
        pcm.extend(&loud);
        pcm.extend(&loud);
        pcm.extend(&quiet);
        pcm.extend(&loud);
        let blocks = vec![(1000.0, 100), (1009.25, 100), (1012.5, 100), (1500.0, 100)];
        let out = place(&blocks, &pcm, 1000.0, 16_000);
        // Speech goes on sample after sample although its clock said 48 samples later
        assert!(out[..200].iter().all(|s| *s == 1000) && out[200] == 0);
        // After a real gap the block is where the clock puts it
        assert!(out[7_999] == 0 && out[8_000] == 1000 && out[8_099] == 1000 && out[8_100] == 0);
    }

    #[test]
    fn stretches_of_sound() {
        let mut level = vec![0f32; 100];
        for f in (10..30).chain(32..40).chain(60..61).chain(80..90) {
            level[f] = 1000.0;
        }
        assert_eq!(runs(&level), vec![(10, 40), (80, 90)]);
    }

    #[test]
    fn two_people_take_turns_on_one_stream() {
        // 0–9 s participant 0, 10–19 s participant 1, no pause between them
        let sound = vec![stream(20, |_| true)];
        let level: Vec<Vec<f32>> = sound.iter().map(|s| loudness(s)).collect();
        let lit = tiles(20, |x| vec![x / 10], 2, 4);
        let o = owners(&level, &[Vec::new()], &lit, &people(2), None);
        assert_eq!(speaker_at(&o, 0, 50), Some("Учасник 1"));
        assert_eq!(speaker_at(&o, 0, 150), Some("Учасник 2"));
        // The change is found within half a second of where it is
        let change = o.stretches.iter().find(|x| x.speaker.is_some() && x.from > 0).map(|x| x.from).unwrap();
        assert!((95..=105).contains(&change), "{change}");
    }

    #[test]
    fn a_remark_over_a_long_speech_is_the_other_participant() {
        // Participant 0 speaks 0–30 s on stream 0; participant 1 says something at 12–14 s on stream 1
        let sound = vec![stream(30, |_| true), stream(30, |x| (12..14).contains(&x))];
        let level: Vec<Vec<f32>> = sound.iter().map(|s| loudness(s)).collect();
        let lit = tiles(30, |x| if (12..14).contains(&x) { vec![0, 1] } else { vec![0] }, 2, 4);
        let o = owners(&level, &[Vec::new(), Vec::new()], &lit, &people(2), None);
        assert_eq!(speaker_at(&o, 0, 130), Some("Учасник 1"));
        assert_eq!(speaker_at(&o, 1, 130), Some("Учасник 2"));
        assert_eq!(o.stretches.iter().filter(|x| x.stream == 1).count(), 1);
    }

    #[test]
    fn a_short_remark_takes_the_tile_lit_after_it() {
        // Participant 1's half-second «так» on stream 1; its tile lights up 0.8 s later
        let mut quick = vec![0i16; 30 * RATE];
        for i in (10 * RATE)..(10 * RATE + RATE / 2) {
            quick[i] = (4000.0 * ((i as f32) * 0.05).sin()) as i16;
        }
        let sound = vec![stream(30, |x| x < 8 || x > 20), quick];
        let level: Vec<Vec<f32>> = sound.iter().map(|s| loudness(s)).collect();
        let mut lit = tiles(30, |x| if x < 8 || x > 20 { vec![0] } else { vec![] }, 2, 4);
        for f in 108..116 {
            lit[f] = vec![1];
        }
        let o = owners(&level, &[Vec::new(), Vec::new()], &lit, &people(2), None);
        assert_eq!(speaker_at(&o, 1, 102), Some("Учасник 2"));
    }

    #[test]
    fn packets_name_the_speaker_and_tiles_give_the_name() {
        // Stream 0: one source for 0–9 s, another for 10–19 s; stream 1: a third at 5–7 s, over
        // the first. Source 42 is on both streams whenever anyone speaks. Tiles lag and linger.
        let sound = vec![stream(20, |_| true), stream(20, |x| (5..8).contains(&x))];
        let level: Vec<Vec<f32>> = sound.iter().map(|s| loudness(s)).collect();
        let lit = tiles(20, |x| if (5..8).contains(&x) { vec![x / 10, 2] } else { vec![x / 10] }, 3, 8);
        let src = vec![
            (0..200).map(|f| (f, vec![42, if f < 100 { 1_325_215_850 } else { 2_867_621_577 }])).collect::<Vec<_>>(),
            (50..80).map(|f| (f, vec![25_413_854, 42])).chain((80..200).map(|f| (f, vec![42]))).collect::<Vec<_>>(),
        ];
        let o = owners(&level, &src, &lit, &people(3), None);
        assert_eq!(speaker_at(&o, 0, 50), Some("Учасник 1"));
        assert_eq!(speaker_at(&o, 0, 96), Some("Учасник 1"));
        assert_eq!(speaker_at(&o, 0, 103), Some("Учасник 2"));
        assert_eq!(speaker_at(&o, 1, 60), Some("Учасник 3"));
        assert!(o.by_packets == 2 && o.names.len() == 3, "{o:?}");
    }

    #[test]
    fn pieces_join_one_speaker_over_a_pause_and_stop_at_another() {
        let owned = Owned {
            stretches: vec![
                Stretch { stream: 0, from: 10, to: 50, speaker: Some(0) },
                Stretch { stream: 0, from: 60, to: 100, speaker: Some(0) },
                Stretch { stream: 0, from: 100, to: 140, speaker: Some(1) },
                Stretch { stream: 0, from: 300, to: 320, speaker: None },
            ],
            names: vec![Some("А".into()), Some("Б".into())],
            by_packets: 0,
        };
        let sound = vec![vec![1000i16; 40 * RATE]];
        let got = pieces(&owned, &sound, &[vec![(0, 40 * RATE)]], 2);
        assert_eq!(got.len(), 3);
        assert_eq!((got[0].speaker, got[0].from, got[0].to), (0, 10 * FRAME - LEAD, 100 * FRAME));
        assert_eq!((got[1].speaker, got[1].from, got[1].to), (1, 100 * FRAME, 140 * FRAME + TAIL));
        assert_eq!(got[2].speaker, 2);
        // No speech found in a stretch: no piece
        assert!(pieces(&owned, &sound, &[vec![(0, 20 * RATE)]], 2).iter().all(|p| p.speaker != 2));
    }

    #[test]
    fn streams_are_kept_with_the_recording() {
        let dir = std::env::temp_dir().join(format!("protokol-slots-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let streams = Streams { t0: 1_700_000_000_000.0, sound: vec![stream(6, |x| x == 2 || x == 4), stream(6, |x| x == 3)], src: vec![vec![(20, vec![25_413_854, 42])], Vec::new()] };
        save_folder(&dir, &streams).unwrap();
        let back = load_folder(&dir).unwrap();
        assert_eq!(back, streams);
        // Silence is not written
        let kept = fs::metadata(dir.join(FOLDER_DIR).join("0.pcm")).unwrap().len() as usize;
        assert!(kept < 3 * RATE * 2, "{kept}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn streams_read_from_the_meet_window_files() {
        let dir = std::env::temp_dir().join(format!("protokol-slots-read-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let slots = dir.join("2026-11-27T21-00-00.slots");
        fs::create_dir_all(&slots).unwrap();
        // 20 s of blocks of 1365 samples from wall clock 1 000 000 ms; sound in seconds 5–7
        let (mut idx, mut pcm) = (String::new(), Vec::new());
        let mut t = 1_000_000.0;
        for b in 0..235 {
            let on = (5.0..7.0).contains(&(b as f64 * 1365.0 / 16_000.0));
            idx.push_str(&format!("{} 1365\n", t as u64));
            for i in 0..1365 {
                let v: i16 = if on { (3000.0 * ((i as f32) * 0.05).sin()) as i16 } else { 0 };
                pcm.extend_from_slice(&v.to_le_bytes());
            }
            t += 1365.0 / 16.0;
        }
        fs::write(slots.join("slot-0.idx"), idx).unwrap();
        fs::write(slots.join("slot-0.pcm"), pcm).unwrap();
        let report = serde_json::json!({"ft": [1_005_000.0, 1_005_100.0], "streams": [{"i": 0, "levels": [900, 900], "src": [[42, 25_413_854u32], []]}]});
        fs::write(dir.join("2026-11-27T21-00-00.jsonl"), format!("{report}\n")).unwrap();
        // The recording's span starts 2 s into the files
        let got = from_sessions(&[dir.clone()], 1_002_000.0, 1_012_000.0, &Pauses::default()).unwrap();
        assert_eq!(got.sound.len(), 1);
        assert_eq!(got.sound[0].len(), 10 * RATE);
        let level = loudness(&got.sound[0]);
        assert!(level[25] < 1.0 && level[35] > 1000.0 && level[45] > 1000.0 && level[55] < 1.0, "{:?}", &level[20..60]);
        assert_eq!(got.src[0], vec![(30, vec![42, 25_413_854])]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// What the page sends in reports of 2 s from `from` to `to` (ms): per
    /// stream, blocks of 1365 samples that sound where `on(stream, ms)` says
    fn reports(from: f64, to: f64, streams: u64, on: impl Fn(u64, f64) -> bool) -> Vec<Value> {
        use base64::Engine as _;
        let mut out = Vec::new();
        let mut t = from;
        while t < to {
            let mut slots = Vec::new();
            for i in 0..streams {
                let (mut bt, mut bn, mut pcm) = (Vec::new(), Vec::new(), Vec::new());
                let mut bt_t = t;
                for _ in 0..24 {
                    let sounds = on(i, bt_t);
                    bt.push(bt_t);
                    bn.push(1365);
                    for k in 0..1365 {
                        let v: i16 = if sounds { (3000.0 * ((k as f32) * 0.05).sin()) as i16 } else { 0 };
                        pcm.extend_from_slice(&v.to_le_bytes());
                    }
                    bt_t += 1365.0 / 16.0;
                }
                slots.push(serde_json::json!({ "i": i, "bt": bt, "bn": bn, "pcm": base64::engine::general_purpose::STANDARD.encode(&pcm) }));
            }
            out.push(Value::Array(slots));
            t += 24.0 * 1365.0 / 16.0;
        }
        out
    }

    /// The wall clocks of the blocks written for one stream
    fn written(raw: &Path, i: u64) -> Vec<f64> {
        fs::read_to_string(raw.join(format!("slot-{i}.idx"))).unwrap_or_default().lines().filter_map(|l| l.split_whitespace().next()?.parse().ok()).collect()
    }

    #[test]
    fn what_the_window_sends_goes_to_the_recording_and_only_its_sound_stays() {
        use base64::Engine as _;
        let folder = std::env::temp_dir().join(format!("protokol-slots-rec-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        // A recording that started at wall clock 1 700 000 010 s
        let start = chrono::DateTime::from_timestamp(1_700_000_010, 0).unwrap();
        fs::write(folder.join("metadata.json"), serde_json::json!({ "created_at": start.to_rfc3339() }).to_string()).unwrap();
        let t0 = start.timestamp_millis() as f64;
        // No recording: nothing is written anywhere
        assert_eq!(raw_dir_at(None, Instant::now()), None);
        let running = Running { folder: folder.clone(), started_ms: t0, stopped: None };
        let raw = raw_dir_at(Some(&running), Instant::now()).unwrap();
        assert_eq!(raw, folder.join(RAW_DIR));
        let never = Pauses::default();
        // Blocks from 4 s before the start (what the page held when it
        // learned of the recording): stream 0 sounds in the second before
        // the start and in seconds 3–5 of the recording, stream 1 never
        for mut slots in reports(t0 - 4_000.0, t0 + 10_000.0, 2, |i, t| i == 0 && ((-1_000.0..0.0).contains(&(t - t0)) || (3_000.0..5_000.0).contains(&(t - t0)))) {
            // A block list that does not fit its sound is left out whole
            slots.as_array_mut().unwrap().push(serde_json::json!({ "i": 7, "bt": [t0], "bn": [999], "pcm": base64::engine::general_purpose::STANDARD.encode([0u8; 10]) }));
            write_blocks(&raw, &slots, &|t, n| recorded(&running, &never, t, n));
        }
        assert!(raw.join("slot-0.pcm").exists() && raw.join("slot-1.idx").exists() && !raw.join("slot-7.pcm").exists());
        // Nothing from before the recording started is written
        let first = written(&raw, 0)[0];
        assert!(first > t0 - 50.0 && first < t0 + 90.0, "{}", first - t0);
        assert_eq!(fs::metadata(raw.join("slot-0.pcm")).unwrap().len(), written(&raw, 0).len() as u64 * 1365 * 2);
        // «Спікери» reads them: one stream with sound, on the recording's clock
        let got = for_recording(&folder, 8.0).unwrap();
        assert_eq!(got.sound.len(), 1);
        let level = loudness(&got.sound[0]);
        let at = |sec: f64| level[((120.0 + sec) * 10.0) as usize];
        assert!(at(-0.5) < 1.0 && at(2.5) < 1.0 && at(3.5) > 1000.0 && at(4.5) > 1000.0 && at(5.5) < 1.0);
        // The recording keeps its own copy; what the window sent is gone,
        // and nothing more is taken for this recording
        assert!(folder.join(FOLDER_FILE).exists() && !raw.exists());
        assert_eq!(for_recording(&folder, 8.0).unwrap(), got);
        assert_eq!(raw_dir_at(Some(&running), Instant::now()), None);
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_pause_and_the_time_after_the_stop_are_left_out_of_the_streams() {
        let folder = std::env::temp_dir().join(format!("protokol-slots-pause-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        let start = chrono::DateTime::from_timestamp(1_700_000_010, 0).unwrap();
        fs::write(folder.join("metadata.json"), serde_json::json!({ "created_at": start.to_rfc3339() }).to_string()).unwrap();
        let t0 = start.timestamp_millis() as f64;
        // Recorded for 20 s of the wall clock, paused from 6 s to 10 s: 16 s of recording
        fs::write(folder.join(crate::audio::pauses::FILE), serde_json::json!({ "v": 1, "pauses": [[t0 + 6_000.0, t0 + 10_000.0]] }).to_string()).unwrap();
        let pauses = crate::audio::pauses::of(&folder);
        assert_eq!(pauses.total(), 4_000.0);
        let running = Running { folder: folder.clone(), started_ms: t0, stopped: Some((Instant::now(), t0 + 20_000.0)) };
        // Someone speaks in seconds 3–5, 7–9 (during the pause), 12–14 and
        // 21–23 (after the stop); the page sends all of it
        let speech = |i: u64, t: f64| i == 0 && [(3.0, 5.0), (7.0, 9.0), (12.0, 14.0), (21.0, 23.0)].iter().any(|(a, b)| (a * 1000.0..b * 1000.0).contains(&(t - t0)));
        let sent = reports(t0 - 4_000.0, t0 + 25_000.0, 1, speech);
        let raw = raw_dir_at(Some(&running), Instant::now()).unwrap();
        for slots in &sent {
            write_blocks(&raw, slots, &|t, n| recorded(&running, &pauses, t, n));
        }
        // Written: nothing of the pause and nothing after the stop
        let times = written(&raw, 0);
        let block = 1365.0 / 16.0;
        assert!(times.iter().all(|t| (t + block / 2.0 >= t0 && t + block / 2.0 < t0 + 6_000.0) || (t + block / 2.0 >= t0 + 10_000.0 && t + block / 2.0 <= t0 + 20_000.0)));
        assert!((times.len() as f64 * block - 16_000.0).abs() < 2.0 * block, "{}", times.len());
        // Read: what followed the pause goes on from where the recording was paused
        let got = for_recording(&folder, 16.0).unwrap();
        let level = loudness(&got.sound[0]);
        let at = |sec: f64| level[((120.0 + sec) * 10.0) as usize];
        assert!(at(3.5) > 1000.0 && at(4.5) > 1000.0 && at(5.5) < 1.0 && at(6.5) < 1.0 && at(7.5) < 1.0, "{:?}", &level[1230..1280]);
        assert!(at(8.5) > 1000.0 && at(9.5) > 1000.0 && at(10.5) < 1.0);
        assert!(level[1360..].iter().all(|l| *l < 1.0));
        let _ = fs::remove_dir_all(&folder);

        // A test build's window keeps all it sends beside its reports: the
        // pause is taken out when that is read, and the sources Meet's
        // packets name follow the same clock
        let dir = std::env::temp_dir().join(format!("protokol-slots-pause-all-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let slots = dir.join("2026-11-28T15-00-00.slots");
        for part in &sent {
            write_blocks(&slots, part, &|_, _| true);
        }
        let ft: Vec<f64> = (0..250).map(|f| t0 + f as f64 * 100.0).collect();
        let src: Vec<Vec<u32>> = (0..250).map(|f| if f < 100 { vec![7] } else { vec![9] }).collect();
        let report = serde_json::json!({ "ft": ft, "streams": [{ "i": 0, "src": src }] });
        fs::write(dir.join("2026-11-28T15-00-00.jsonl"), format!("{report}\n")).unwrap();
        let all = from_sessions(&[dir.clone()], t0 - 120_000.0, t0 + 136_000.0, &pauses).unwrap();
        let level = loudness(&all.sound[0]);
        let at = |sec: f64| level[((120.0 + sec) * 10.0) as usize];
        assert!(at(4.5) > 1000.0 && at(6.5) < 1.0 && at(7.5) < 1.0 && at(8.5) > 1000.0 && at(9.5) > 1000.0 && at(10.5) < 1.0);
        // (read by the wall clock alone, the speech after the stop is still there: 21–23 s is 17–19 s)
        assert!(at(17.5) > 1000.0);
        let frame = |sec: f64| ((120.0 + sec) * 10.0).round() as usize;
        let named = |f: usize| all.src[0].iter().find(|x| x.0 == f).map(|x| x.1.clone());
        // Source 7 until the pause; source 9 named from second 10 of the wall clock, second 6 of the recording
        assert_eq!((named(frame(5.9)), named(frame(6.0)), named(frame(15.0))), (Some(vec![7]), Some(vec![9]), Some(vec![9])));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_last_blocks_are_taken_a_moment_after_the_stop_and_silence_is_not_kept() {
        let folder = std::env::temp_dir().join(format!("protokol-slots-stop-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        let stopped = Instant::now();
        let rec = Running { folder: folder.clone(), started_ms: 0.0, stopped: Some((stopped, 60_000.0)) };
        assert!(raw_dir_at(Some(&rec), stopped + Duration::from_secs(3)).is_some());
        assert_eq!(raw_dir_at(Some(&rec), stopped + AFTER_STOP + Duration::from_secs(1)), None);
        // Blocks that come after the stop are taken when they sounded before it
        let never = Pauses::default();
        assert!(recorded(&rec, &never, 59_900.0, 1365) && !recorded(&rec, &never, 60_000.0, 1365) && !recorded(&rec, &never, -100.0, 1365));
        // A recording whose streams were silent keeps nothing of them
        let start = chrono::DateTime::from_timestamp(1_700_000_010, 0).unwrap();
        fs::write(folder.join("metadata.json"), serde_json::json!({ "created_at": start.to_rfc3339() }).to_string()).unwrap();
        let raw = folder.join(RAW_DIR);
        fs::create_dir_all(&raw).unwrap();
        fs::write(raw.join("slot-0.idx"), format!("{} 1600\n", start.timestamp_millis())).unwrap();
        fs::write(raw.join("slot-0.pcm"), vec![0u8; 3200]).unwrap();
        assert_eq!(for_recording(&folder, 5.0), None);
        assert!(!raw.exists() && !folder.join(FOLDER_FILE).exists());
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_call_by_its_streams() {
        // A 60 s recording that started 3.4 s after the streams' files; two participants:
        // 0 speaks 5–25 s and 40–55 s on stream 0, 1 speaks 20–35 s on stream 1 (5 s at once)
        // (and both say something outside the recording: 0 before it started, 1 after it was stopped)
        let (a, b) = (stream(70, |x| x < 2 || (5..25).contains(&x) || (40..55).contains(&x)), stream(70, |x| (20..35).contains(&x) || (65..69).contains(&x)));
        let streams = Streams { t0: 1_000_000.0, sound: vec![a.clone(), b.clone()], src: vec![Vec::new(), Vec::new()] };
        let start_ms = 1_003_400.0;
        let system: Vec<f32> = (0..60 * RATE).map(|n| {
            let i = n + 3 * RATE + 6_400;
            (a.get(i).copied().unwrap_or(0) as f32 + b.get(i).copied().unwrap_or(0) as f32) / 32_768.0
        }).collect();
        let mut call = MeetCall { v: 2, names: people(2), ..Default::default() };
        let lit = tiles(70, |x| {
            let mut v = Vec::new();
            if x < 2 || (5..25).contains(&x) || (40..55).contains(&x) { v.push(0); }
            if (20..35).contains(&x) || (65..69).contains(&x) { v.push(1); }
            v
        }, 2, 4);
        for (f, l) in lit.iter().enumerate().take(700) {
            call.t.push(1_000_000.0 + f as f64 * 100.0);
            call.level.push(0);
            call.lit.push(l.clone());
        }
        let energy = |s: &[i16]| -> Vec<(usize, usize)> {
            let l = loudness(s);
            runs(&l).into_iter().map(|(a, b)| (a * FRAME, b * FRAME)).collect()
        };
        // The clocks said 2 s more than the sound does
        let plan = plan_with(&streams, &call, &system, None, start_ms + 2_000.0, &energy).expect("plan");
        assert_eq!(plan.clock.shift, 34);
        assert_eq!(plan.names, vec![Some("Учасник 1".to_string()), Some("Учасник 2".to_string())]);
        assert_eq!(plan.pieces.len(), 3, "{:?}", plan.pieces);
        let (from, to) = plan.span(&plan.pieces[0]);
        // 5–25 s of the streams, 3.4 s before the recording started
        assert!((from - 1.6).abs() < 0.1 && (to - 21.6).abs() < 0.1, "{from} {to}");
        assert_eq!(plan.pieces.iter().map(|p| (p.stream, p.speaker)).collect::<Vec<_>>(), vec![(0, 0), (1, 1), (0, 0)]);
        assert!(plan.uncovered < 0.01);
        assert_eq!(plan.key(1), "Учасник 2");
        assert_eq!(plan.speaker_of("Учасник 2"), Some(1));
        // Voices: each speaker measured from its own pieces
        let mut plan2 = plan.clone();
        let v = voices(&streams, &mut plan2, &mut |s| Some(vec![s.len() as f32, 1.0]));
        assert_eq!(v.len(), 2);
        assert!(v.iter().all(|x| x.len() == 2));
    }

    #[test]
    fn a_nameless_speaker_whose_pieces_went_to_a_named_one_is_dropped() {
        let streams = Streams { t0: 0.0, sound: vec![stream(8, |x| x < 3), stream(8, |x| (4..6).contains(&x))], src: vec![Vec::new(), Vec::new()] };
        let mut plan = Plan {
            names: vec![Some("А".into()), None],
            pieces: vec![Piece { stream: 0, from: 0, to: 3 * RATE, speaker: 0 }, Piece { stream: 1, from: 4 * RATE, to: 6 * RATE, speaker: 1 }],
            ..Default::default()
        };
        // Every piece sounds the same: the nameless one is the named speaker
        let found = voices(&streams, &mut plan, &mut |_| Some(vec![1.0, 0.0]));
        assert_eq!(plan.names, vec![Some("А".to_string())]);
        assert!(plan.pieces.iter().all(|p| p.speaker == 0));
        assert_eq!(found.len(), 1);
        // A voice of its own stays a speaker of its own
        let mut plan = Plan {
            names: vec![Some("А".into()), None],
            pieces: vec![Piece { stream: 0, from: 0, to: 3 * RATE, speaker: 0 }, Piece { stream: 1, from: 4 * RATE, to: 6 * RATE, speaker: 1 }],
            ..Default::default()
        };
        // (told apart here by the length of the sound)
        let found = voices(&streams, &mut plan, &mut |s| Some(if s.len() > 2 * RATE { vec![1.0, 0.0] } else { vec![0.0, 1.0] }));
        assert_eq!(plan.names.len(), 2);
        assert_eq!(found.len(), 2);
        assert_eq!(plan.key(1), "#1");
    }

    #[test]
    fn a_piece_that_sounds_like_another_speaker_is_theirs_unless_packets_named_it() {
        // Stream 0: А for 6 s, then 2 s that the tiles gave to А but sound like Б; stream 1: Б for 5 s
        let streams = Streams { t0: 0.0, sound: vec![stream(10, |x| x < 6 || x >= 8), stream(10, |x| x < 5)], src: vec![Vec::new(), Vec::new()] };
        let pieces = vec![
            Piece { stream: 0, from: 0, to: 6 * RATE, speaker: 0 },
            Piece { stream: 1, from: 0, to: 5 * RATE, speaker: 1 },
            Piece { stream: 0, from: 8 * RATE, to: 10 * RATE, speaker: 0 },
        ];
        // The voices, told apart here by the length of the sound
        let voice = |s: &[f32]| {
            Some(match s.len() / RATE {
                6 => vec![1.0, 0.0],
                5 => vec![0.0, 1.0],
                _ => vec![0.1, 1.0],
            })
        };
        let mut plan = Plan { names: vec![Some("А".into()), Some("Б".into())], pieces: pieces.clone(), ..Default::default() };
        voices(&streams, &mut plan, &mut |s| voice(s));
        assert_eq!(plan.pieces[2].speaker, 1);
        assert_eq!(plan.by_voice, vec![(2, "А".to_string())]);
        assert_eq!((plan.pieces[0].speaker, plan.pieces[1].speaker), (0, 1));
        // Named by packets: nothing is moved
        let mut plan = Plan { names: vec![Some("А".into()), Some("Б".into())], pieces, by_packets: 3, ..Default::default() };
        voices(&streams, &mut plan, &mut |s| voice(s));
        assert_eq!(plan.pieces[2].speaker, 0);
        assert!(plan.by_voice.is_empty());
    }

    #[test]
    fn streams_that_miss_the_recordings_sound_are_not_used() {
        // The recording has 40 s of sound, the stream only its last 10 s
        let s = stream(50, |x| (30..40).contains(&x));
        let whole = stream(50, |x| x < 40);
        let streams = Streams { t0: 0.0, sound: vec![s], src: vec![Vec::new()] };
        let system: Vec<f32> = whole.iter().map(|x| *x as f32 / 32_768.0).collect();
        let mut call = MeetCall { v: 2, names: people(1), ..Default::default() };
        for f in 0..500 {
            call.t.push(f as f64 * 100.0);
            call.level.push(0);
            call.lit.push(if (300..400).contains(&f) { vec![0] } else { vec![] });
        }
        let energy = |s: &[i16]| -> Vec<(usize, usize)> { runs(&loudness(s)).into_iter().map(|(a, b)| (a * FRAME, b * FRAME)).collect() };
        assert!(plan_with(&streams, &call, &system, None, 0.0, &energy).is_none());
    }
}
