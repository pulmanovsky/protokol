//! Speakers by Google Meet participant.
//!
//! When the call ran in Protokol's Meet window, its reports say every 100 ms
//! which participant's tile was lit as speaking and how loud the call was
//! (`crate::meet::frames`). After a recording, those frames are put on the
//! call recording's own timeline (by matching the call's loudness in both)
//! and the voices found by diarization are relabelled by participant:
//!
//! - a moment when exactly one participant is lit belongs to that participant;
//! - a voice cluster that is almost all one participant is that participant
//!   everywhere, also where the Meet window was not open yet;
//! - several voices from one account (a shared device) are told apart only
//!   when each of them is a confidently recognised saved voice;
//! - what Meet does not settle stays with the voice found by diarization.
//!
//! The result is a [`Diarization`] like the one it came from, so naming,
//! the transcript and the voices work as before.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use log::info;
use serde::{Deserialize, Serialize};

use super::{Diarization, Turn};
use crate::audio::channel_track::ChannelLevels;
use crate::meet::frames::MeetCall;

/// The frame step of the reports and of the call's loudness (s)
const STEP_S: f64 = 0.1;
/// How far the clocks may be apart at first (frames: ±60 s)
const SEARCH: i64 = 600;
/// Windows that follow the clocks along the recording (frames)
const WINDOW: usize = 600;
const WINDOW_STEP: usize = 300;
/// How far a window may move from the whole-call offset (frames: ±3 s)
const FINE: i64 = 30;
/// Loudness correlation needed for the whole call and for one window
const MIN_CORR: f32 = 0.35;
const MIN_WINDOW_CORR: f32 = 0.5;
/// Overlap needed for a correlation to count (frames)
const MIN_PAIRS: usize = 300;
/// The tile lights up about one frame after its sound
const GLOW_LAG: i64 = 1;
/// Share of the call's speech that must be labelled by Meet
const MIN_LABELLED: f64 = 0.3;
/// A voice this much one participant is that participant everywhere
const OWNER_SHARE: f64 = 0.85;
const OWNER_MIN_FRAMES: usize = 30;
/// Several voices from one account: each this share and length
const SHARED_SHARE: f64 = 0.25;
const SHARED_MIN_FRAMES: usize = 200;
/// Lit runs shorter than this are not trusted (frames)
const MIN_RUN: usize = 4;
/// Unlit frames take a participant lit this close in the same turn (frames)
const FILL: usize = 10;
/// The Meet window counts as opened late after this much recording (s)
const LATE_S: f64 = 60.0;
/// Microphone level that counts as the user speaking (as in `timeline`)
const MIC_SPEECH_LEVEL: u16 = 150;

/// What Meet gave a recording, shown at the top of «Спікери»
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MeetStatus {
    /// The voices were split by participant
    pub used: bool,
    /// Participants who spoke (accounts)
    pub accounts: usize,
    /// The Meet window was opened this far into the recording (s)
    pub opened_at_s: Option<f64>,
    /// There were Meet frames, but they did not fit the recording's sound
    pub mismatch: bool,
}

pub const STATUS_FILE: &str = "meet-speakers.json";

pub fn save_status(folder: &Path, status: &MeetStatus) {
    if let Ok(text) = serde_json::to_string(status) {
        let _ = std::fs::write(folder.join(STATUS_FILE), text);
    }
}

pub fn load_status(folder: &Path) -> Option<MeetStatus> {
    serde_json::from_slice(&std::fs::read(folder.join(STATUS_FILE)).ok()?).ok()
}

/// One speaker of the relabelled call
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    /// A participant
    Part(usize),
    /// One of several voices of a shared account
    PartVoice(usize, usize),
    /// A voice Meet did not settle
    Voice(usize),
}

/// The relabelled call, before the voices of the participants are measured
#[derive(Debug, Clone)]
pub struct Plan {
    pub turns: Vec<Turn>,
    pub speech_s: Vec<f64>,
    /// The participant of each speaker, if any
    pub meet_name: Vec<Option<String>>,
    /// The participant's account carried several voices
    pub shared: Vec<bool>,
    /// The diarization voice of each speaker when its own is not measured
    /// (a voice Meet did not settle, one voice of a shared account), or the
    /// voice it mostly came from (fallback)
    pub voice: Vec<usize>,
    /// The speaker's voice is measured from its own turns
    pub measure: Vec<bool>,
    pub status: MeetStatus,
    /// The user's own tile, not counted
    pub self_name: Option<String>,
}

/// Loudness of the call recording every 100 ms (log scale)
pub fn envelope(samples: &[f32]) -> Vec<f32> {
    samples
        .chunks(1600)
        .map(|c| {
            let ms = c.iter().map(|x| x * x).sum::<f32>() / c.len().max(1) as f32;
            (ms.sqrt() * 10_000.0 + 1.0).ln()
        })
        .collect()
}

/// The Meet frames on a regular 100 ms grid of the wall clock
struct Grid {
    t0: f64,
    level: Vec<Option<f32>>,
    lit: Vec<Option<Vec<usize>>>,
}

fn grid(call: &MeetCall) -> Option<Grid> {
    let t0 = call.t.iter().copied().fold(f64::INFINITY, f64::min);
    let t1 = call.t.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !t0.is_finite() || !t1.is_finite() {
        return None;
    }
    let n = ((t1 - t0) / 100.0).round() as usize + 1;
    let mut g = Grid { t0, level: vec![None; n], lit: vec![None; n] };
    for i in 0..call.t.len() {
        let j = ((call.t[i] - t0) / 100.0).round() as usize;
        if j < n {
            g.level[j] = Some((call.level.get(i).copied().unwrap_or(0) as f32 + 1.0).ln());
            g.lit[j] = Some(call.lit.get(i).cloned().unwrap_or_default());
        }
    }
    Some(g)
}

/// Correlation of the recording's loudness `a[i]` with Meet's `b[i + off]`
/// over `range`, with the number of pairs
fn correlation(a: &[f32], b: &[Option<f32>], off: i64, range: std::ops::Range<usize>) -> (f32, usize) {
    let (mut n, mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0usize, 0f64, 0f64, 0f64, 0f64, 0f64);
    for i in range {
        let j = i as i64 + off;
        if j < 0 {
            continue;
        }
        let Some(Some(y)) = b.get(j as usize) else { continue };
        let (x, y) = (a[i] as f64, *y as f64);
        n += 1;
        sx += x;
        sy += y;
        sxx += x * x;
        syy += y * y;
        sxy += x * y;
    }
    if n < 2 {
        return (0.0, n);
    }
    let nf = n as f64;
    let cov = sxy - sx * sy / nf;
    let vx = sxx - sx * sx / nf;
    let vy = syy - sy * sy / nf;
    if vx <= 1e-9 || vy <= 1e-9 {
        return (0.0, n);
    }
    ((cov / (vx * vy).sqrt()) as f32, n)
}

/// Best offset in `[from, to]` and its correlation
fn best_offset(a: &[f32], b: &[Option<f32>], from: i64, to: i64, range: std::ops::Range<usize>) -> Option<(i64, f32)> {
    let mut best: Option<(i64, f32)> = None;
    for off in from..=to {
        let (c, n) = correlation(a, b, off, range.clone());
        if n >= MIN_PAIRS.min(range.len() / 2).max(20) && best.map_or(true, |(_, bc)| c > bc) {
            best = Some((off, c));
        }
    }
    best
}

/// Where each frame of the recording is in the Meet grid: `i + a + b·i`
#[derive(Debug, Clone, Copy)]
struct Offset {
    a: f64,
    b: f64,
}

impl Offset {
    fn at(&self, i: usize) -> i64 {
        (i as f64 + self.a + self.b * i as f64).round() as i64
    }
}

/// Puts the Meet grid on the recording: a whole-call offset near the one the
/// clocks give, then followed window by window (a straight line through them)
fn align(env: &[f32], g: &Grid, start_ms: f64) -> Option<(Offset, f32)> {
    let base = ((g.t0 - start_ms) / 100.0).round() as i64;
    // Grid index j = i - base + lag
    let all = 0..env.len();
    let (lag, corr) = best_offset(env, &g.level, -base - SEARCH, -base + SEARCH, all)?;
    if corr < MIN_CORR {
        info!("Meet: the call's loudness does not match the recording (best {corr:.2})");
        return None;
    }
    let mut points: Vec<(f64, f64)> = Vec::new();
    let mut w = 0;
    while w + WINDOW / 2 < env.len() {
        let range = w..(w + WINDOW).min(env.len());
        if let Some((l, c)) = best_offset(env, &g.level, lag - FINE, lag + FINE, range.clone()) {
            if c >= MIN_WINDOW_CORR {
                points.push(((range.start + range.end) as f64 / 2.0, l as f64));
            }
        }
        w += WINDOW_STEP;
    }
    let mut lags: Vec<f64> = points.iter().map(|p| p.1).collect();
    lags.sort_by(|a, b| a.total_cmp(b));
    let median = lags.get(lags.len() / 2).copied().unwrap_or(lag as f64);
    points.retain(|p| (p.1 - median).abs() <= 5.0);
    let spread = points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max) - points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let offset = if points.len() >= 3 && spread >= 1200.0 {
        let n = points.len() as f64;
        let (mx, my) = (points.iter().map(|p| p.0).sum::<f64>() / n, points.iter().map(|p| p.1).sum::<f64>() / n);
        let sxx: f64 = points.iter().map(|p| (p.0 - mx).powi(2)).sum();
        let sxy: f64 = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
        let b = if sxx > 0.0 { sxy / sxx } else { 0.0 };
        Offset { a: my - b * mx, b }
    } else if !points.is_empty() {
        Offset { a: median, b: 0.0 }
    } else {
        Offset { a: lag as f64, b: 0.0 }
    };
    info!("Meet: call put on the recording ({} window(s), loudness match {corr:.2})", points.len());
    Some((offset, corr))
}

/// The participant who is the user: lit while this Mac's microphone speaks
fn find_self(lit: &[Option<Vec<usize>>], levels: Option<&ChannelLevels>, people: usize) -> Option<usize> {
    let levels = levels?;
    let mut lit_n = vec![0usize; people];
    let mut me_n = vec![0usize; people];
    for (i, l) in lit.iter().enumerate() {
        let Some(l) = l else { continue };
        let (Some(&m), Some(&s)) = (levels.mic.get(i), levels.system.get(i)) else { continue };
        let me = m >= MIC_SPEECH_LEVEL && (m as u64).pow(2) > 4 * (s as u64).pow(2);
        for &p in l {
            if p < people {
                lit_n[p] += 1;
                if me {
                    me_n[p] += 1;
                }
            }
        }
    }
    (0..people).filter(|&p| lit_n[p] >= 20).map(|p| (p, me_n[p] as f64 / lit_n[p] as f64)).filter(|(_, s)| *s >= 0.5).max_by(|a, b| a.1.total_cmp(&b.1)).map(|(p, _)| p)
}

/// Relabels a call's voices by Meet participant. `start_ms`: when the
/// recording started (wall clock); `system`: the call recording (16 kHz);
/// `voice_names`: the saved person each diarization voice confidently or
/// doubtfully matches («Name?»). Err: why Meet was not used.
pub fn plan(call: &MeetCall, start_ms: f64, system: &[f32], levels: Option<&ChannelLevels>, d: &Diarization, voice_names: &[Option<String>]) -> Result<Plan, MeetStatus> {
    let mismatch = MeetStatus { mismatch: true, ..Default::default() };
    let g = grid(call).ok_or_else(MeetStatus::default)?;
    let env = envelope(system);
    let (offset, _) = align(&env, &g, start_ms).ok_or_else(|| mismatch.clone())?;
    let people = call.names.len();

    // What Meet shows for each frame of the recording
    let raw: Vec<Option<Vec<usize>>> = (0..env.len())
        .map(|i| {
            let j = offset.at(i) + GLOW_LAG;
            (j >= 0).then(|| g.lit.get(j as usize).cloned().flatten()).flatten()
        })
        .collect();
    let me = find_self(&raw, levels, people);
    let self_name = me.map(|p| call.names[p].clone());
    let one = |i: usize| -> Option<Option<usize>> {
        let l = raw.get(i)?.as_ref()?;
        let l: Vec<usize> = l.iter().copied().filter(|&p| Some(p) != me).collect();
        Some(if l.len() == 1 { Some(l[0]) } else { None })
    };
    let opened_at = raw.iter().position(|l| l.is_some()).map(|i| i as f64 * STEP_S);

    // Frames of each turn
    let frames_of = |t: &Turn| -> std::ops::Range<usize> { ((t.start / STEP_S).floor() as usize)..((t.end / STEP_S).ceil() as usize) };
    let voices = d.centroids.len().max(d.turns.iter().map(|t| t.speaker + 1).max().unwrap_or(0));
    let mut by_voice = vec![vec![0usize; people]; voices];
    let (mut covered, mut labelled) = (0usize, 0usize);
    for t in &d.turns {
        for i in frames_of(t) {
            if let Some(l) = one(i) {
                covered += 1;
                if let Some(p) = l {
                    labelled += 1;
                    by_voice[t.speaker][p] += 1;
                }
            }
        }
    }
    if covered == 0 || (labelled as f64) < MIN_LABELLED * covered as f64 {
        info!("Meet: too little of the call was lit as speaking ({labelled} of {covered} frames)");
        return Err(mismatch);
    }
    // A voice that is almost all one participant
    let owner: Vec<Option<usize>> = by_voice
        .iter()
        .map(|counts| {
            let total: usize = counts.iter().sum();
            let (p, n) = counts.iter().copied().enumerate().max_by_key(|(_, n)| *n)?;
            (total >= OWNER_MIN_FRAMES && n as f64 >= OWNER_SHARE * total as f64).then_some(p)
        })
        .collect();
    // Accounts several voices spoke from; split when each is a sure saved voice
    let mut shared = vec![false; people];
    let mut split: Vec<Vec<usize>> = vec![Vec::new(); people];
    let mut major: Vec<Option<usize>> = vec![None; people];
    for p in 0..people {
        let total: usize = (0..voices).map(|k| by_voice[k][p]).sum();
        if total == 0 {
            continue;
        }
        major[p] = (0..voices).max_by_key(|&k| by_voice[k][p]);
        let big: Vec<usize> = (0..voices).filter(|&k| by_voice[k][p] >= SHARED_MIN_FRAMES && by_voice[k][p] as f64 >= SHARED_SHARE * total as f64).collect();
        if big.len() < 2 {
            continue;
        }
        shared[p] = true;
        let names: Vec<Option<&String>> = big.iter().map(|&k| voice_names.get(k).and_then(|n| n.as_ref()).filter(|n| !n.ends_with('?'))).collect();
        let distinct = names.iter().flatten().collect::<std::collections::HashSet<_>>().len();
        if names.iter().all(|n| n.is_some()) && distinct == big.len() {
            split[p] = big;
        }
    }
    let key_of = |p: usize, k: usize| -> Key {
        if split[p].is_empty() {
            Key::Part(p)
        } else if split[p].contains(&k) {
            Key::PartVoice(p, k)
        } else {
            // A small third voice goes with the account's main one
            Key::PartVoice(p, major[p].filter(|m| split[p].contains(m)).unwrap_or(split[p][0]))
        }
    };

    // Each turn frame by frame
    let mut keys: Vec<Key> = Vec::new();
    let mut index: HashMap<Key, usize> = HashMap::new();
    let mut turns: Vec<Turn> = Vec::new();
    for t in &d.turns {
        let frames: Vec<usize> = frames_of(t).collect();
        if frames.is_empty() {
            continue;
        }
        let mut lab: Vec<Option<usize>> = frames.iter().map(|&i| one(i).flatten()).collect();
        // Short lit runs between other participants are not trusted
        let mut s = 0;
        while s < lab.len() {
            let mut e = s;
            while e < lab.len() && lab[e] == lab[s] {
                e += 1;
            }
            if lab[s].is_some() && e - s < MIN_RUN && (s > 0 || e < lab.len()) {
                let before = lab[..s].iter().rev().flatten().next().copied();
                let after = lab[e..].iter().flatten().next().copied();
                if before.is_some_and(|b| Some(b) != lab[s]) || after.is_some_and(|a| Some(a) != lab[s]) {
                    lab[s..e].iter_mut().for_each(|x| *x = None);
                }
            }
            s = e;
        }
        // Unlit frames: the participant lit nearby in this turn, or the voice's owner
        let known: Vec<(usize, usize)> = lab.iter().enumerate().filter_map(|(x, p)| p.map(|p| (x, p))).collect();
        let filled: Vec<Key> = (0..lab.len())
            .map(|x| {
                if let Some(p) = lab[x] {
                    return key_of(p, t.speaker);
                }
                let near = known.iter().filter(|(y, _)| y.abs_diff(x) <= FILL).min_by_key(|(y, _)| (y.abs_diff(x), *y > x)).map(|(_, p)| *p);
                match near.or(owner.get(t.speaker).copied().flatten()) {
                    Some(p) => key_of(p, t.speaker),
                    None => Key::Voice(t.speaker),
                }
            })
            .collect();
        for (x, key) in filled.iter().enumerate() {
            let i = frames[x];
            let a = if x == 0 { t.start } else { i as f64 * STEP_S };
            let b = if x + 1 == frames.len() { t.end } else { (i + 1) as f64 * STEP_S };
            if b <= a {
                continue;
            }
            let id = *index.entry(key.clone()).or_insert_with(|| {
                keys.push(key.clone());
                keys.len() - 1
            });
            match turns.last_mut() {
                Some(last) if last.speaker == id && (a - last.end).abs() < 1e-6 => last.end = b,
                _ => turns.push(Turn { start: a, end: b, speaker: id }),
            }
        }
    }

    let n = keys.len();
    let mut speech_s = vec![0.0; n];
    let mut from_voice = vec![vec![0.0f64; voices]; n];
    for (t, src) in turns.iter().zip(turns_sources(&turns, &d.turns)) {
        speech_s[t.speaker] += t.end - t.start;
        if let Some(k) = src {
            from_voice[t.speaker][k] += t.end - t.start;
        }
    }
    let main_voice = |s: usize| (0..voices).max_by(|&a, &b| from_voice[s][a].total_cmp(&from_voice[s][b])).unwrap_or(0);
    let meet_name: Vec<Option<String>> = keys
        .iter()
        .map(|k| match k {
            Key::Part(p) | Key::PartVoice(p, _) => Some(call.names[*p].clone()),
            Key::Voice(_) => None,
        })
        .collect();
    let shared_of: Vec<bool> = keys.iter().map(|k| matches!(k, Key::Part(p) | Key::PartVoice(p, _) if shared[*p])).collect();
    let voice: Vec<usize> = keys
        .iter()
        .enumerate()
        .map(|(s, k)| match k {
            Key::Voice(v) | Key::PartVoice(_, v) => *v,
            Key::Part(_) => main_voice(s),
        })
        .collect();
    let measure: Vec<bool> = keys.iter().zip(&shared_of).map(|(k, sh)| matches!(k, Key::Part(_)) && !sh).collect();
    let accounts = keys.iter().filter_map(|k| match k {
        Key::Part(p) | Key::PartVoice(p, _) => Some(*p),
        Key::Voice(_) => None,
    });
    let accounts = accounts.collect::<std::collections::HashSet<_>>().len();
    let status = MeetStatus { used: true, accounts, opened_at_s: opened_at.filter(|&s| s > LATE_S), mismatch: false };
    info!("Meet: {} of {} frames lit as one participant; {accounts} participant(s), {} speaker(s)", labelled, covered, n);
    Ok(Plan { turns, speech_s, meet_name, shared: shared_of, voice, measure, status, self_name })
}

/// The diarization voice under each new turn (the one it was cut from)
fn turns_sources(new: &[Turn], old: &[Turn]) -> Vec<Option<usize>> {
    let mut j = 0;
    new.iter()
        .map(|t| {
            let mid = (t.start + t.end) / 2.0;
            while j < old.len() && old[j].end < mid {
                j += 1;
            }
            // Turns are in time order; look around in case of overlaps
            old[j.saturating_sub(2)..(j + 3).min(old.len())].iter().find(|o| o.start <= mid && mid <= o.end).map(|o| o.speaker)
        })
        .collect()
}

/// Longest stretch measured for a voice (s), and how many are measured
const MEASURE_PIECE_S: f64 = 8.0;
const MEASURE_PIECES: usize = 15;
const MEASURE_MIN_S: f64 = 1.0;

/// The finished relabelled call: every speaker's voice measured from its own
/// turns (a participant), or taken from diarization (a voice Meet did not
/// settle, one voice of a shared account, a participant too short to measure)
pub fn with_voices(plan: &Plan, system: &[f32], d: &Diarization, embed: &mut dyn FnMut(&[f32]) -> Option<Vec<f32>>) -> Diarization {
    let n = plan.speech_s.len();
    let mut centroids: Vec<Vec<f32>> = Vec::with_capacity(n);
    for s in 0..n {
        let fallback = d.centroids.get(plan.voice[s]).cloned().unwrap_or_default();
        if !plan.measure[s] {
            centroids.push(fallback);
            continue;
        }
        let mut spans: Vec<(f64, f64)> = plan.turns.iter().filter(|t| t.speaker == s && t.end - t.start >= MEASURE_MIN_S).map(|t| (t.start, t.end)).collect();
        spans.sort_by(|a, b| (b.1 - b.0).total_cmp(&(a.1 - a.0)));
        let mut found: Vec<Vec<f32>> = Vec::new();
        for (a, b) in spans.into_iter().take(MEASURE_PIECES) {
            let b = b.min(a + MEASURE_PIECE_S);
            let (from, to) = ((a * 16_000.0) as usize, ((b * 16_000.0) as usize).min(system.len()));
            if to > from {
                if let Some(e) = embed(&system[from..to]) {
                    found.push(e);
                }
            }
        }
        centroids.push(super::embedding::centroid(found.iter()).unwrap_or(fallback));
    }
    Diarization { turns: plan.turns.clone(), centroids, speech_s: plan.speech_s.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A call of `secs` seconds: who speaks in each second (None = silence)
    fn script(secs: usize, who: impl Fn(usize) -> Option<usize>) -> Vec<Option<usize>> {
        (0..secs).map(who).collect()
    }

    /// The call recording: loud in speech, louder or quieter second by second
    fn sound(s: &[Option<usize>]) -> Vec<f32> {
        let mut out = Vec::new();
        for (x, w) in s.iter().enumerate() {
            let amp = match w {
                Some(_) => 0.05 + 0.04 * (((x as u64 * 2_654_435_761) >> 9) % 5) as f32,
                None => 0.0005,
            };
            out.extend((0..16_000).map(|i| amp * ((i as f32) * 0.05).sin()));
        }
        out
    }

    /// The Meet reports of that call, `delay_ms` after the recording's clock
    fn meet(s: &[Option<usize>], names: &[&str], start_ms: f64, delay_ms: f64, from_s: usize) -> MeetCall {
        let mut call = MeetCall { v: 2, names: names.iter().map(|n| n.to_string()).collect(), ..Default::default() };
        let env = envelope(&sound(s));
        for i in (from_s * 10)..env.len() {
            call.t.push(start_ms + delay_ms + i as f64 * 100.0);
            call.level.push(((env[i].exp() - 1.0).max(0.0)) as u32);
            // Lit one frame after the sound
            let lit_sec = i.saturating_sub(1) / 10;
            call.lit.push(s.get(lit_sec).copied().flatten().map(|p| vec![p]).unwrap_or_default());
        }
        call
    }

    /// Diarization that merged participants 0 and 1 into one voice, and
    /// found participant 2 as its own
    fn diarized(s: &[Option<usize>]) -> Diarization {
        let mut turns: Vec<Turn> = Vec::new();
        for (x, w) in s.iter().enumerate() {
            let Some(p) = w else { continue };
            let k = if *p == 2 { 1 } else { 0 };
            match turns.last_mut() {
                Some(t) if t.speaker == k && (t.end - x as f64).abs() < 1e-9 => t.end = x as f64 + 1.0,
                _ => turns.push(Turn { start: x as f64, end: x as f64 + 1.0, speaker: k }),
            }
        }
        Diarization { turns, centroids: vec![vec![1.0, 0.0], vec![0.0, 1.0]], speech_s: vec![0.0, 0.0] }
    }

    fn who(x: usize) -> Option<usize> {
        // 10 s blocks: A, B, silence, C, A, B, ...
        match (x / 10) % 4 {
            0 => Some(0),
            1 => Some(1),
            2 => None,
            _ => Some(2),
        }
    }

    #[test]
    fn voices_split_by_participant_despite_clock_offset() {
        let s = script(400, who);
        let start = 1_794_500_000_000.0;
        let call = meet(&s, &["Анна", "Борис", "Віра"], start, 7_300.0, 0);
        let d = diarized(&s);
        let p = plan(&call, start, &sound(&s), None, &d, &[None, None]).expect("Meet used");
        assert!(p.status.used && !p.status.mismatch);
        assert_eq!(p.status.accounts, 3);
        assert_eq!(p.meet_name.iter().flatten().count(), 3);
        // Anna's and Boris's time are apart again, each about 100 s
        let time = |name: &str| -> f64 {
            let s = p.meet_name.iter().position(|n| n.as_deref() == Some(name)).unwrap();
            p.speech_s[s]
        };
        assert!((time("Анна") - 100.0).abs() < 3.0, "{}", time("Анна"));
        assert!((time("Борис") - 100.0).abs() < 3.0, "{}", time("Борис"));
        assert!((time("Віра") - 100.0).abs() < 3.0, "{}", time("Віра"));
        // A turn at 15 s is Boris
        let at = |sec: f64| p.turns.iter().find(|t| t.start <= sec && sec < t.end).map(|t| p.meet_name[t.speaker].clone().unwrap_or_default());
        assert_eq!(at(15.0).as_deref(), Some("Борис"));
        assert_eq!(at(5.0).as_deref(), Some("Анна"));
        // Participants' voices are measured; Vira's own cluster too (it is all hers)
        assert!(p.measure.iter().all(|m| *m));
    }

    #[test]
    fn window_opened_late_still_names_known_voices() {
        let s = script(400, who);
        let start = 1_794_500_000_000.0;
        // The Meet window opened 120 s into the recording
        let call = meet(&s, &["Анна", "Борис", "Віра"], start, 0.0, 120);
        let p = plan(&call, start, &sound(&s), None, &diarized(&s), &[None, None]).expect("Meet used");
        assert!(p.status.opened_at_s.unwrap() > 100.0);
        // Vira's voice (its own cluster) is hers from the very start
        let first = p.turns.iter().find(|t| t.start <= 35.0 && 35.0 < t.end).unwrap();
        assert_eq!(p.meet_name[first.speaker].as_deref(), Some("Віра"));
        // The merged voice before the window opened stays a voice
        let early = p.turns.iter().find(|t| t.start <= 5.0 && 5.0 < t.end).unwrap();
        assert_eq!(p.meet_name[early.speaker], None);
    }

    #[test]
    fn unrelated_reports_are_not_used() {
        let s = script(300, who);
        let other = script(300, |x| if x % 3 == 0 { Some(0) } else { None });
        let start = 1_794_500_000_000.0;
        let call = meet(&other, &["Анна"], start, 0.0, 0);
        let err = plan(&call, start, &sound(&s), None, &diarized(&s), &[None, None]).unwrap_err();
        assert!(err.mismatch && !err.used);
    }

    #[test]
    fn shared_account_split_only_by_sure_saved_voices() {
        // One account («Штаб»), two voices of diarization
        let s = script(400, |x| if (x / 20) % 2 == 0 { Some(0) } else { None });
        let mut d = Diarization { turns: Vec::new(), centroids: vec![vec![1.0, 0.0], vec![0.0, 1.0]], speech_s: vec![0.0, 0.0] };
        for b in (0..400).step_by(40) {
            d.turns.push(Turn { start: b as f64, end: b as f64 + 10.0, speaker: 0 });
            d.turns.push(Turn { start: b as f64 + 10.0, end: b as f64 + 20.0, speaker: 1 });
        }
        let start = 1_794_500_000_000.0;
        let call = meet(&s, &["Штаб"], start, 0.0, 0);
        let sure = [Some("Грім".to_string()), Some("Тайфун".to_string())];
        let p = plan(&call, start, &sound(&s), None, &d, &sure).unwrap();
        assert_eq!(p.meet_name.iter().filter(|n| n.as_deref() == Some("Штаб")).count(), 2);
        assert!(p.shared.iter().all(|x| *x));
        assert!(p.measure.iter().all(|m| !m));
        // A new voice: the account stays one speaker
        let unsure = [Some("Грім".to_string()), Some("Тайфун?".to_string())];
        let p = plan(&call, start, &sound(&s), None, &d, &unsure).unwrap();
        assert_eq!(p.meet_name.iter().filter(|n| n.as_deref() == Some("Штаб")).count(), 1);
        assert!(p.shared[0]);
    }

    #[test]
    fn own_tile_lit_with_the_microphone_is_left_out() {
        let s = script(300, who);
        let start = 1_794_500_000_000.0;
        let mut call = meet(&s, &["Анна", "Борис", "Віра", "Я сам"], start, 0.0, 0);
        // The user's tile is lit in the silent blocks, when the microphone speaks
        let mut levels = ChannelLevels { step_ms: 100, mic: vec![0; 3000], system: vec![0; 3000] };
        for i in 0..call.t.len() {
            if who(i / 10).is_none() {
                call.lit[i] = vec![3];
                levels.mic[i] = 900;
            }
        }
        let p = plan(&call, start, &sound(&s), Some(&levels), &diarized(&s), &[None, None]).unwrap();
        assert_eq!(p.self_name.as_deref(), Some("Я сам"));
        assert!(!p.meet_name.iter().flatten().any(|n| n == "Я сам"));
    }

    #[test]
    fn participant_voice_measured_from_its_turns() {
        let s = script(200, who);
        let start = 1_794_500_000_000.0;
        let call = meet(&s, &["Анна", "Борис", "Віра"], start, 0.0, 0);
        let d = diarized(&s);
        let p = plan(&call, start, &sound(&s), None, &d, &[None, None]).unwrap();
        let mut calls = 0;
        let out = with_voices(&p, &sound(&s), &d, &mut |samples| {
            calls += 1;
            Some(vec![samples.len() as f32, 1.0])
        });
        assert!(calls > 0);
        assert_eq!(out.centroids.len(), p.speech_s.len());
        assert!(out.centroids.iter().all(|c| c.len() == 2));
    }
}
