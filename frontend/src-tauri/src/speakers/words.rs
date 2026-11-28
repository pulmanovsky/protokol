//! Speakers attached to the text of the first transcription.
//!
//! Import keeps a time for every word (`words.json` in the meeting folder,
//! from Whisper's token timestamps of the same decoding). Speaker
//! recognition then only has to say who spoke when: each word goes to the
//! voice heard at its time, and the text is cut where the voice changes.
//! The recording is not transcribed a second time and no text changes.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::Turn;

pub const WORDS_FILE: &str = "words.json";

/// One transcribed piece of the recording with its words' times (seconds
/// from the start of the recording)
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PieceWords {
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub words: Vec<(String, f64)>,
    /// How sure Whisper was of each word (0..1); empty in older files
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probs: Vec<f32>,
    /// Words replaced on their own by a dictionary term: (index, what was heard)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto: Vec<(usize, String)>,
    /// Word times aligned to the sound (not Whisper's own)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub aligned: bool,
    /// For a meeting aligned after its lines were made: (time in the sound,
    /// time on the lines' own timeline) per word as aligned, to find a line's
    /// words by the line's stored times. Empty when the two are the same.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub heard: Vec<(f64, f64)>,
    /// The underline bars of the fine-tuned model that heard this piece;
    /// none for the original model (`ukrainian::doubt::Thresholds`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bars: Option<crate::ukrainian::doubt::Thresholds>,
    /// The speaker of the whole piece when it was heard on a Meet stream of
    /// its own (`speakers::slots`): the participant's name, or «#n» for a
    /// voice Meet did not name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
}

impl PieceWords {
    /// A moment of this piece's sound on the lines' own timeline
    pub fn line_time(&self, t: f64) -> f64 {
        interpolate(&self.heard, t)
    }
}

/// Piecewise-linear through (x, y) points sorted by x; outside them the
/// nearest point's offset; identity without points
pub fn interpolate(points: &[(f64, f64)], x: f64) -> f64 {
    let n = points.len();
    if n == 0 {
        return x;
    }
    if x <= points[0].0 {
        return x + points[0].1 - points[0].0;
    }
    if x >= points[n - 1].0 {
        return x + points[n - 1].1 - points[n - 1].0;
    }
    let i = points.partition_point(|p| p.0 <= x);
    let (a, b) = (points[i - 1], points[i]);
    if b.0 - a.0 < 1e-9 {
        return a.1;
    }
    a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0)
}

/// From the lines' own timeline to the sound, over a whole meeting (None when
/// they are the same: nothing was aligned after the lines were made)
pub fn to_sound(pieces: &[PieceWords]) -> Option<Vec<(f64, f64)>> {
    if pieces.iter().all(|p| p.heard.is_empty()) {
        return None;
    }
    let mut pts: Vec<(f64, f64)> = Vec::new();
    for p in pieces {
        for (_, t) in &p.words {
            pts.push((p.line_time(*t), *t));
        }
    }
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Kept in order both ways
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(pts.len());
    for (x, y) in pts {
        match out.last() {
            Some(&(lx, ly)) if x <= lx || y <= ly => {}
            _ => out.push((x, y)),
        }
    }
    Some(out)
}

/// Re-decode a piece more carefully when at least this many words are below
/// REDECODE_P (truly unsure: a few pieces in a meeting)
const REDECODE_MIN_DOUBTFUL: usize = 2;
const REDECODE_P: f32 = 0.6;
const REDECODE_BEAM: i32 = 8;

fn mean(v: &[f32]) -> f32 {
    if v.is_empty() { 1.0 } else { v.iter().sum::<f32>() / v.len() as f32 }
}

/// Doubtful words of a piece (see `ukrainian::doubt`)
pub fn doubtful_in(piece: &PieceWords, terms: &[String]) -> Vec<bool> {
    let words: Vec<String> = piece.words.iter().map(|(w, _)| w.clone()).collect();
    let known = |w: &str| terms.iter().any(|t| t.eq_ignore_ascii_case(w) || t.to_lowercase() == w.to_lowercase());
    crate::ukrainian::doubt::doubtful_with(&words, &piece.probs, &known, piece.bars.unwrap_or(crate::ukrainian::doubt::Thresholds::DEFAULT))
}

/// Doubtful words that sound like a dictionary term become that term; the
/// heard word is kept in `auto`
pub fn snap_doubtful(piece: &mut PieceWords, terms: &[String]) {
    let flags = doubtful_in(piece, terms);
    for (i, doubtful) in flags.into_iter().enumerate() {
        if !doubtful {
            continue;
        }
        let heard = piece.words[i].0.clone();
        if let Some(term) = crate::ukrainian::doubt::snap(&heard, terms) {
            piece.words[i].0 = crate::ukrainian::doubt::replace_keeping_punctuation(&heard, term);
            piece.auto.push((i, heard));
        }
    }
    piece.text = piece.words.iter().map(|(w, _)| w.as_str()).collect::<Vec<_>>().join(" ");
}

/// One speech piece transcribed with word times and confidence, as on
/// import: a second, more careful pass when several words are doubtful
/// (kept only if Whisper is surer overall), then doubtful words that sound
/// like a dictionary term are put right. None when nothing was heard.
pub async fn transcribe_piece(
    engine: &crate::whisper_engine::WhisperEngine,
    samples: &[f32],
    start_s: f64,
    end_s: f64,
    language: Option<String>,
) -> anyhow::Result<Option<PieceWords>> {
    let terms = crate::ukrainian::glossary_terms();
    let bars = engine.bars().await;
    let bars = (bars != crate::ukrainian::doubt::Thresholds::DEFAULT).then_some(bars);
    let make = |text: String, words: Vec<(String, f64)>, probs: Vec<f32>| PieceWords {
        text: text.trim().to_string(),
        start: start_s,
        end: end_s,
        words: words.into_iter().map(|(w, t)| (w, start_s + t)).collect(),
        probs,
        auto: Vec::new(), aligned: false, heard: vec![], bars, who: None,
    };
    let (text, words, probs) = engine.transcribe_audio_with_word_probs(samples.to_vec(), language.clone(), None).await?;
    let mut piece = make(text, words, probs);
    if piece.text.is_empty() {
        return Ok(None);
    }
    if piece.probs.iter().filter(|p| **p < REDECODE_P).count() >= REDECODE_MIN_DOUBTFUL {
        let (text, words, probs) = engine.transcribe_audio_with_word_probs(samples.to_vec(), language, Some(REDECODE_BEAM)).await?;
        let second = make(text, words, probs);
        if !second.text.is_empty() && mean(&second.probs) > mean(&piece.probs) {
            log::debug!("Doubtful piece decoded again: mean confidence {:.2} → {:.2}", mean(&piece.probs), mean(&second.probs));
            piece = second;
        }
    }
    snap_doubtful(&mut piece, &terms);
    Ok(Some(piece))
}

pub fn save(folder: &Path, pieces: &[PieceWords]) -> std::io::Result<()> {
    let tmp = folder.join(".words.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(pieces).unwrap_or_default())?;
    std::fs::rename(tmp, folder.join(WORDS_FILE))
}

pub fn load(folder: &Path) -> Option<Vec<PieceWords>> {
    serde_json::from_slice(&std::fs::read(folder.join(WORDS_FILE)).ok()?).ok()
}

/// Speech pieces of the recording as on import (up to 25 s each)
pub fn speech_pieces(audio: &[f32]) -> anyhow::Result<Vec<crate::audio::vad::SpeechSegment>> {
    const REDEMPTION_MS: u32 = 2000;
    const MAX_PIECE_SAMPLES: usize = 25 * 16_000;
    let speech = crate::audio::vad::get_speech_chunks(audio, REDEMPTION_MS)?;
    let mut segments = Vec::new();
    for seg in &speech {
        segments.extend(crate::audio::common::split_segment_at_silence(seg, MAX_PIECE_SAMPLES));
    }
    Ok(segments.into_iter().filter(|s| s.samples.len() >= 1600).collect())
}

/// A line of the transcript was corrected by hand: its words in the saved
/// word times are replaced too (times spread over the old span), so the
/// next speaker recognition keeps the correction instead of transcribing
/// again. Returns false when the old words are not found.
fn times_of(olds: &[(usize, f64)], j: Option<usize>) -> f64 {
    j.map(|j| olds[j].1).unwrap_or(0.0)
}

pub fn replace_line(pieces: &mut [PieceWords], old_text: &str, new_text: &str, near: f64) -> bool {
    let old: Vec<&str> = old_text.split_whitespace().collect();
    let new: Vec<String> = new_text.split_whitespace().map(str::to_string).collect();
    if old.is_empty() {
        return false;
    }
    let flat: Vec<(usize, usize)> = pieces.iter().enumerate().flat_map(|(p, x)| (0..x.words.len()).map(move |w| (p, w))).collect();
    let word = |k: usize| &pieces[flat[k].0].words[flat[k].1];
    let start = (0..flat.len().saturating_sub(old.len() - 1))
        .filter(|&k| (0..old.len()).all(|j| word(k + j).0 == old[j]))
        .min_by(|&a, &b| (word(a).1 - near).abs().total_cmp(&(word(b).1 - near).abs()));
    let Some(k) = start else { return false };
    let (t0, t1) = (word(k).1, word(k + old.len() - 1).1);
    let (first_piece, _first_word) = flat[k];
    // Per-word confidence and «heard as» move with the words
    let mut extra: Vec<Vec<(f32, Option<String>)>> = pieces
        .iter()
        .map(|p| {
            (0..p.words.len())
                .map(|i| (p.probs.get(i).copied().unwrap_or(1.0), p.auto.iter().find(|(j, _)| *j == i).map(|(_, h)| h.clone())))
                .collect()
        })
        .collect();
    // The new words keep the moments of the old words they match (the same
    // word, or one corrected to something that sounds alike); words typed in
    // go between their neighbours by syllables. Each goes back to the piece
    // its moment belongs to, so no word ever lands outside its piece.
    let olds: Vec<(usize, f64)> = (k..k + old.len()).map(|j| (flat[j].0, word(j).1)).collect();
    let norm = |w: &str| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    let alike = |a: &str, b: &str| a == b || (!a.is_empty() && !b.is_empty() && crate::ukrainian::doubt::sound_similarity(a, b) >= 0.7);
    let (on, nn) = (old.len(), new.len());
    let mut dp = vec![vec![0u16; on + 1]; nn + 1];
    for i in (0..nn).rev() {
        for j in (0..on).rev() {
            dp[i][j] = if alike(&norm(&new[i]), &norm(old[j])) { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
        }
    }
    let mut matched: Vec<Option<usize>> = vec![None; nn];
    let (mut i, mut j) = (0, 0);
    while i < nn && j < on {
        if alike(&norm(&new[i]), &norm(old[j])) && dp[i][j] == dp[i + 1][j + 1] + 1 {
            matched[i] = Some(j);
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let syl = |w: &str| w.chars().filter(|c| "аеєиіїоуюяaeiouyАЕЄИІЇОУЮЯAEIOUY".contains(*c)).count().max(1) as f64;
    let mut times = vec![t0; nn];
    let mut owner = vec![first_piece; nn];
    let mut q = 0;
    while q < nn {
        if let Some(j) = matched[q] {
            times[q] = olds[j].1;
            owner[q] = olds[j].0;
            q += 1;
            continue;
        }
        let gap_end = (q..nn).find(|&x| matched[x].is_some()).unwrap_or(nn);
        let from = if q == 0 { t0 } else { times[q - 1] };
        let to = if gap_end < nn { times_of(&olds, matched[gap_end]) } else { t1.max(from) };
        let total: f64 = (q..gap_end).map(|x| syl(&new[x])).sum::<f64>() + if q == 0 { 0.0 } else { syl(&new[q - 1]) };
        let mut acc = if q == 0 { 0.0 } else { syl(&new[q - 1]) };
        let piece_of_prev = if q == 0 { first_piece } else { owner[q - 1] };
        for x in q..gap_end {
            times[x] = if q == 0 && x == 0 { from } else { from + (to - from).max(0.0) * acc / total.max(1.0) };
            acc += syl(&new[x]);
            owner[x] = piece_of_prev;
        }
        q = gap_end;
    }
    // Pieces given words of their own are no longer aligned to the sound
    // (until aligned again): their new words are only spread by syllables
    for x in 0..nn {
        if matched[x].is_none() {
            pieces[owner[x]].aligned = false;
        }
    }
    // Remove the old words (back to front), remembering where each piece lost its first one
    let mut place: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for j in (k..k + old.len()).rev() {
        let (p, w) = flat[j];
        pieces[p].words.remove(w);
        extra[p].remove(w);
        place.insert(p, w);
    }
    // …and put the new ones in, piece by piece, inside each piece's time
    let mut by_piece: Vec<(usize, Vec<(String, f64)>)> = Vec::new();
    for (x, w) in new.into_iter().enumerate() {
        let p = owner[x];
        let t = times[x].clamp(pieces[p].start, pieces[p].end.max(pieces[p].start));
        match by_piece.last_mut() {
            Some((lp, v)) if *lp == p => v.push((w, t)),
            _ => by_piece.push((p, vec![(w, t)])),
        }
    }
    for (p, words) in by_piece.into_iter().rev() {
        let at = place.get(&p).copied().unwrap_or(pieces[p].words.len()).min(pieces[p].words.len());
        // What the user wrote is sure
        extra[p].splice(at..at, words.iter().map(|_| (1.0f32, None)));
        pieces[p].words.splice(at..at, words);
    }
    for (p, e) in pieces.iter_mut().zip(extra) {
        p.text = p.words.iter().map(|(w, _)| w.as_str()).collect::<Vec<_>>().join(" ");
        let had_probs = !p.probs.is_empty();
        p.probs = if had_probs || e.iter().any(|(x, _)| *x < 1.0) { e.iter().map(|(x, _)| *x).collect() } else { Vec::new() };
        p.auto = e.into_iter().enumerate().filter_map(|(i, (_, h))| h.map(|h| (i, h))).collect();
    }
    true
}

/// A word to check in «Перевірте»
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Doubt {
    /// Transcript line and the word's place in it
    pub segment_id: String,
    pub word_index: usize,
    pub word: String,
    /// When it was said (seconds)
    pub seconds: f64,
    pub probability: f32,
    /// Replaced on its own by a dictionary term: what was heard
    pub heard: Option<String>,
}

/// A word as compared between a line and the word times: case and the
/// punctuation around it aside
fn same_form(w: &str) -> String {
    w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’' && c != '-').to_lowercase()
}

/// Longest common subsequence of `a` and `b`: the pairs (index in a, index in b)
fn common_words(a: &[String], b: &[String]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut t = vec![vec![0u16; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if !a[i].is_empty() && a[i] == b[j] { t[i + 1][j + 1] + 1 } else { t[i + 1][j].max(t[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n && j < m {
        if !a[i].is_empty() && a[i] == b[j] {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if t[i + 1][j] >= t[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// Doubtful and auto-replaced words of a meeting, mapped onto its transcript
/// lines (`rows`: id and text, in order). A line need not match the word
/// times word for word: a dictionary rule, an edit or a cut changes some of
/// its words, and the words left as heard keep their doubt (a line skipped
/// whole left most of a meeting's doubtful words unmarked).
pub fn doubts(pieces: &[PieceWords], rows: &[(String, String)], terms: &[String]) -> Vec<Doubt> {
    // Every word with its time, confidence, doubt and «heard as»
    let mut flat: Vec<(String, f64, f32, bool, Option<String>)> = Vec::new();
    // Words a learned correction only offers to change are checked too
    let hints = crate::ukrainian::corrections::hint_words();
    for p in pieces {
        let flags = doubtful_in(p, terms);
        for (i, (w, t)) in p.words.iter().enumerate() {
            let heard = p.auto.iter().find(|(j, _)| *j == i).map(|(_, h)| h.clone());
            let prob = p.probs.get(i).copied().unwrap_or(1.0);
            let doubtful = flags.get(i).copied().unwrap_or(false) || crate::ukrainian::doubt::hinted(w, prob, &hints);
            flat.push((w.clone(), *t, prob, doubtful, heard));
        }
    }
    let forms: Vec<String> = flat.iter().map(|f| same_form(&f.0)).collect();
    // How far before the previous line's end a line may start: a little
    // (a cut, an edit); more in a call heard stream by stream, where a line
    // of somebody who spoke over a long remark comes before that remark's end
    let back = if pieces.iter().any(|p| p.who.is_some()) { 240 } else { 60 };
    let mut out = Vec::new();
    let mut k = 0usize;
    for (id, text) in rows {
        let words: Vec<String> = text.split_whitespace().map(same_form).collect();
        if words.is_empty() {
            continue;
        }
        let n = words.len();
        // Where the line may start: a place where one of its first words is
        // heard, near where the previous line ended (a little before it too)
        let from = k.saturating_sub(back);
        let to = (k + 600).min(flat.len());
        let mut starts: Vec<usize> = Vec::new();
        for s in from..to {
            for (j, w) in words.iter().take(4).enumerate() {
                if !w.is_empty() && forms[s] == *w && s >= j {
                    starts.push(s - j);
                }
            }
        }
        starts.sort_unstable_by_key(|s| (s.abs_diff(k), *s));
        starts.dedup();
        starts.truncate(24);
        let slack = 3 + n / 4;
        let mut best: Option<(usize, Vec<(usize, usize)>)> = None;
        for s in starts {
            let end = (s + n + slack).min(flat.len());
            let pairs = common_words(&words, &forms[s..end]);
            if best.as_ref().map_or(true, |(_, b)| pairs.len() > b.len()) {
                best = Some((s, pairs));
            }
        }
        let Some((s, pairs)) = best else { continue };
        // Most of the line must be the heard words (else it is another place)
        if pairs.len() * 2 < n || pairs.is_empty() {
            continue;
        }
        for &(j, f) in &pairs {
            let (w, t, p, doubtful, heard) = &flat[s + f];
            if *doubtful || heard.is_some() {
                out.push(Doubt { segment_id: id.clone(), word_index: j, word: w.clone(), seconds: *t, probability: *p, heard: heard.clone() });
            }
        }
        k = s + pairs.last().map(|x| x.1 + 1).unwrap_or(0);
    }
    out
}

/// A doubtful word the user checked: it is sure now (and no longer «auto»)
pub fn resolve(pieces: &mut [PieceWords], seconds: f64, word: &str) -> bool {
    for p in pieces.iter_mut() {
        if let Some(i) = p.words.iter().position(|(w, t)| w == word && (t - seconds).abs() < 0.01) {
            if p.probs.len() < p.words.len() {
                p.probs.resize(p.words.len(), 1.0);
            }
            p.probs[i] = 1.0;
            p.auto.retain(|(j, _)| *j != i);
            return true;
        }
    }
    false
}

/// How sure the app is of one word, for undoing «Залишити як є»
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WordMark {
    pub seconds: f64,
    pub word: String,
    pub prob: f32,
    /// What was heard, when the dictionary replaced the word
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto: Option<String>,
}

fn word_at(pieces: &[PieceWords], seconds: f64, word: &str) -> Option<(usize, usize)> {
    pieces.iter().enumerate().find_map(|(p, x)| x.words.iter().position(|(w, t)| w == word && (t - seconds).abs() < 0.01).map(|i| (p, i)))
}

pub fn mark_of(pieces: &[PieceWords], seconds: f64, word: &str) -> Option<WordMark> {
    let (p, i) = word_at(pieces, seconds, word)?;
    let x = &pieces[p];
    Some(WordMark { seconds, word: word.to_string(), prob: x.probs.get(i).copied().unwrap_or(1.0), auto: x.auto.iter().find(|(j, _)| *j == i).map(|(_, h)| h.clone()) })
}

pub fn set_mark(pieces: &mut [PieceWords], m: &WordMark) -> bool {
    let Some((p, i)) = word_at(pieces, m.seconds, &m.word) else { return false };
    let x = &mut pieces[p];
    if x.probs.len() < x.words.len() {
        x.probs.resize(x.words.len(), 1.0);
    }
    x.probs[i] = m.prob;
    x.auto.retain(|(j, _)| *j != i);
    if let Some(h) = &m.auto {
        x.auto.push((i, h.clone()));
        x.auto.sort_by_key(|(j, _)| *j);
    }
    true
}

/// Speech pieces of a recording, channel by channel: the call (system
/// audio alone, no room noise or own voice mixed in) and the user's own
/// speech («Я», where the microphone was louder), cut from the mix. Imports
/// and one-channel recordings use the mix.
pub fn channel_pieces(
    mixed: &[f32],
    system: Option<&[f32]>,
    levels: Option<&crate::audio::channel_track::ChannelLevels>,
) -> anyhow::Result<Vec<crate::audio::vad::SpeechSegment>> {
    channel_pieces_mapped(mixed, system, levels, None)
}

/// [`channel_pieces`], each marked whether it comes from the call recording
pub fn channel_pieces_tagged(
    mixed: &[f32],
    system: Option<&[f32]>,
    levels: Option<&crate::audio::channel_track::ChannelLevels>,
) -> anyhow::Result<Vec<(crate::audio::vad::SpeechSegment, bool)>> {
    let (Some(system), Some(levels)) = (system, levels) else { return Ok(speech_pieces(mixed)?.into_iter().map(|s| (s, false)).collect()) };
    let mut pieces: Vec<(crate::audio::vad::SpeechSegment, bool)> = speech_pieces(system)?.into_iter().map(|s| (s, true)).collect();
    pieces.extend(own_speech(mixed, &super::timeline::me_turns(levels)).into_iter().map(|s| (s, false)));
    pieces.sort_by(|a, b| a.0.start_timestamp_ms.total_cmp(&b.0.start_timestamp_ms));
    Ok(pieces)
}

/// [`channel_pieces`] with the call's pieces put on the mix's timeline
pub fn channel_pieces_mapped(
    mixed: &[f32],
    system: Option<&[f32]>,
    levels: Option<&crate::audio::channel_track::ChannelLevels>,
    map: Option<&super::time_map::TimeMap>,
) -> anyhow::Result<Vec<crate::audio::vad::SpeechSegment>> {
    let (Some(system), Some(levels)) = (system, levels) else { return speech_pieces(mixed) };
    let mut pieces = speech_pieces(system)?;
    if let Some(map) = map {
        for p in pieces.iter_mut() {
            p.start_timestamp_ms = map.map(p.start_timestamp_ms / 1000.0) * 1000.0;
            p.end_timestamp_ms = map.map(p.end_timestamp_ms / 1000.0) * 1000.0;
        }
    }
    pieces.extend(own_speech(mixed, &super::timeline::me_turns(levels)));
    pieces.sort_by(|a, b| a.start_timestamp_ms.total_cmp(&b.start_timestamp_ms));
    Ok(pieces)
}

/// The user's own turns cut from the mix (a little padding, at most 25 s each)
pub(crate) fn own_speech(mixed: &[f32], turns: &[Turn]) -> Vec<crate::audio::vad::SpeechSegment> {
    const RATE: f64 = 16_000.0;
    const PAD_S: f64 = 0.2;
    const MAX_S: f64 = 25.0;
    let mut out = Vec::new();
    for t in turns.iter().filter(|t| t.end - t.start >= 0.3) {
        let mut a = (t.start - PAD_S).max(0.0);
        let end = (t.end + PAD_S).min(mixed.len() as f64 / RATE);
        while a < end {
            let b = (a + MAX_S).min(end);
            let (i, j) = ((a * RATE) as usize, ((b * RATE) as usize).min(mixed.len()));
            if j > i + 1600 {
                out.push(crate::audio::vad::SpeechSegment { samples: mixed[i..j].to_vec(), start_timestamp_ms: a * 1000.0, end_timestamp_ms: b * 1000.0, confidence: 1.0 });
            }
            a = b;
        }
    }
    out
}

/// Stretches where a voice spoke for 2 s or more and no word was heard
/// (Whisper drops a quiet or foreign-sounding voice in a long piece): turns
/// of one voice closer than half a second joined, cut at most 25 s long
pub fn silent_turns(pieces: &[PieceWords], turns: &[Turn]) -> Vec<Turn> {
    const MIN_S: f64 = 2.0;
    const JOIN_S: f64 = 0.5;
    const MAX_S: f64 = 25.0;
    let mut sorted = turns.to_vec();
    sorted.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut joined: Vec<Turn> = Vec::new();
    for t in sorted {
        match joined.last_mut() {
            Some(last) if last.speaker == t.speaker && t.start - last.end < JOIN_S => last.end = last.end.max(t.end),
            _ => joined.push(t),
        }
    }
    let mut times: Vec<f64> = pieces.iter().flat_map(|p| p.words.iter().map(|w| w.1)).collect();
    times.sort_by(|a, b| a.total_cmp(b));
    let heard = |a: f64, b: f64| {
        let i = times.partition_point(|t| *t < a);
        times.get(i).is_some_and(|t| *t <= b)
    };
    let mut out = Vec::new();
    for t in joined.into_iter().filter(|t| t.end - t.start >= MIN_S && !heard(t.start, t.end)) {
        let mut a = t.start;
        while t.end - a >= MIN_S {
            let b = (a + MAX_S).min(t.end);
            out.push(Turn { start: a, end: b, speaker: t.speaker });
            a = b;
        }
    }
    out
}

/// Most of a new piece's words are already in the text within 5 s of it
fn already_nearby(pieces: &[PieceWords], new: &PieceWords) -> bool {
    const NEAR_S: f64 = 5.0;
    let near: std::collections::HashSet<String> = pieces
        .iter()
        .flat_map(|p| p.words.iter())
        .filter(|w| w.1 >= new.start - NEAR_S && w.1 <= new.end + NEAR_S)
        .map(|w| same_form(&w.0))
        .collect();
    let found = new.words.iter().filter(|w| near.contains(&same_form(&w.0))).count();
    found * 2 > new.words.len()
}

/// Each silent turn (`silent_turns`) transcribed on its own, cut from the
/// 16 kHz mix; what Whisper makes up on noise is left out. The new pieces
/// go in among the others by time. Returns how many were added.
pub async fn fill_silent_turns(
    engine: &crate::whisper_engine::WhisperEngine,
    mixed: &[f32],
    pieces: &mut Vec<PieceWords>,
    turns: &[Turn],
    language: Option<String>,
    stop: &(dyn Fn() -> bool + Send + Sync),
) -> anyhow::Result<usize> {
    const RATE: f64 = 16_000.0;
    const PAD_S: f64 = 0.2;
    const MIN_SURE: f32 = 0.35;
    const MIN_WORDS_PER_S: f64 = 0.5;
    // Bounds the extra time on a meeting where nothing at all was heard
    const MAX_TURNS: usize = 300;
    let silent = silent_turns(pieces, turns);
    if silent.is_empty() {
        return Ok(0);
    }
    log::info!("{} stretch(es) of speech without words: transcribed on their own", silent.len());
    let mut added = Vec::new();
    for t in silent.iter().take(MAX_TURNS) {
        if stop() {
            break;
        }
        let a = (t.start - PAD_S).max(0.0);
        let b = (t.end + PAD_S).min(mixed.len() as f64 / RATE);
        let (i, j) = ((a * RATE) as usize, ((b * RATE) as usize).min(mixed.len()));
        if j <= i + 1600 {
            continue;
        }
        let Some(piece) = transcribe_piece(engine, &mixed[i..j], a, b, language.clone()).await? else { continue };
        let sure = piece.probs.is_empty() || mean(&piece.probs) >= MIN_SURE;
        // Speech has a word every second or two; a few words over a long
        // stretch, or no Ukrainian letters at all, are made up on noise
        let dense = piece.words.len() as f64 >= (t.end - t.start) * MIN_WORDS_PER_S;
        let cyrillic = piece.text.chars().any(|c| matches!(c, 'а'..='я' | 'А'..='Я' | 'і' | 'ї' | 'є' | 'ґ' | 'І' | 'Ї' | 'Є' | 'Ґ'));
        if piece.words.is_empty() || !sure || !dense || !cyrillic || crate::ukrainian::hallucination::hallucination_reason(&piece.text).is_some() {
            log::debug!("Stretch {:.1}–{:.1}s left out: {} word(s), sure {sure}, Ukrainian letters {cyrillic}", t.start, t.end, piece.words.len());
            continue;
        }
        // The same words as the piece before: Whisper repeating itself
        let before = pieces.iter().rev().find(|p| p.start <= t.start).map(|p| p.text.as_str());
        if crate::ukrainian::hallucination::is_repeated_segment(before, &piece.text) {
            continue;
        }
        // Words already written nearby with times a little off: not new speech
        if already_nearby(pieces, &piece) {
            log::debug!("Stretch {:.1}–{:.1}s: its words are already nearby", t.start, t.end);
            continue;
        }
        added.push(piece);
    }
    let n = added.len();
    pieces.extend(added);
    pieces.sort_by(|a, b| a.start.total_cmp(&b.start));
    log::info!("{n} of {} stretch(es) without words now have text", silent.len());
    Ok(n)
}

/// Forget the word times (the transcript was made again another way)
pub fn remove(folder: &Path) {
    let _ = std::fs::remove_file(folder.join(WORDS_FILE));
}

fn normalised(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The word times still describe this transcript (it was not edited or
/// transcribed again since import)
pub fn matches_transcript(pieces: &[PieceWords], transcript: &[String]) -> bool {
    let a = normalised(&pieces.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join(" "));
    let b = normalised(&transcript.join(" "));
    if a.is_empty() || !pieces.iter().all(|p| p.words.len() == p.text.split_whitespace().count()) {
        return false;
    }
    if a == b {
        return true;
    }
    // A call heard stream by stream: people talk at once, so the lines (by
    // the moment each starts) and the pieces need not come in one order;
    // the same words, whatever the order
    if pieces.iter().any(|p| p.who.is_some()) {
        let sorted = |t: &str| {
            let mut w: Vec<&str> = t.split(' ').collect();
            w.sort_unstable();
            w.join(" ")
        };
        return sorted(&a) == sorted(&b);
    }
    false
}

/// The voice heard at `t`: the turn around it, else the closest turn
fn speaker_at(turns: &[Turn], t: f64) -> Option<usize> {
    let i = turns.partition_point(|x| x.start <= t);
    let candidates = [i.checked_sub(1), Some(i)];
    candidates
        .iter()
        .flatten()
        .filter_map(|&k| turns.get(k))
        .map(|x| (if t < x.start { x.start - t } else if t > x.end { t - x.end } else { 0.0 }, x.speaker))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, s)| s)
}

/// Rows of (text, start, end, speaker) in seconds: words in order, a new
/// row where the voice changes. A voice for one or two words between the
/// same voice is taken as a diarization slip. Long rows are cut at sentence
/// ends so that a row stays readable (about 25 s).
pub fn assign(pieces: &[PieceWords], turns: &[Turn]) -> Vec<(String, f64, f64, usize)> {
    let mut turns = turns.to_vec();
    turns.sort_by(|a, b| a.start.total_cmp(&b.start));
    // (word, time, speaker, piece end, starts a piece)
    let mut words: Vec<(String, f64, usize, f64, bool)> = Vec::new();
    for p in pieces {
        for (k, (w, t)) in p.words.iter().enumerate() {
            let Some(s) = speaker_at(&turns, *t) else { continue };
            // A piece opens with silence or another voice the words did not catch
            let at = if k == 0 { p.start.max(*t - WORD_REACH_S).min(*t) } else { *t };
            words.push((w.clone(), at, s, p.end, k == 0));
        }
    }
    // Short slips: a run of at most 2 words between runs of one voice
    let mut runs: Vec<(usize, usize)> = Vec::new(); // (first word, len)
    for i in 0..words.len() {
        match runs.last_mut() {
            Some((start, len)) if words[*start].2 == words[i].2 => *len += 1,
            _ => runs.push((i, 1)),
        }
    }
    for r in 1..runs.len().saturating_sub(1) {
        let (start, len) = runs[r];
        let (before, after) = (words[runs[r - 1].0].2, words[runs[r + 1].0].2);
        if len <= 2 && before == after {
            for w in &mut words[start..start + len] {
                w.2 = before;
            }
        }
    }
    snap_to_sentences(&mut words);
    const MAX_ROW_S: f64 = 25.0;
    let mut rows: Vec<(String, f64, f64, usize)> = Vec::new();
    for (i, (w, t, s, piece_end, piece_start)) in words.iter().enumerate() {
        let next_start = words.get(i + 1).map(|n| if n.4 { *piece_end } else { n.1 }).unwrap_or(*piece_end);
        // A word lasts until the next one, but not over a long silence or
        // another voice: no further than its own voice's turn goes on
        let own_end = turns.iter().find(|x| x.speaker == *s && x.start <= *t && *t <= x.end).map(|x| x.end).unwrap_or(f64::INFINITY);
        let end = next_start.min(*piece_end).min(*t + WORD_REACH_S).min(own_end).max(*t);
        let new_row = match rows.last() {
            None => true,
            Some(last) => {
                last.3 != *s
                    || *piece_start && *t - last.2 > 2.0
                    || (*t - last.1 > MAX_ROW_S && last.0.ends_with(['.', '!', '?']))
                    || other_voice_between(&turns, last.2, *t, *s) && !stray_tail(&words, i)
            }
        };
        if new_row {
            rows.push((w.clone(), *t, end, *s));
        } else if let Some(last) = rows.last_mut() {
            last.0.push(' ');
            last.0.push_str(w);
            last.2 = end;
        }
    }
    rows
}

/// Rows when every piece has one speaker of its own (a call heard stream by
/// stream): pieces in the order they start, one speaker's pieces in a row
/// made into rows as [`assign`] does. People who talk at once get rows of
/// their own, each whole; nothing is split by the moment a word sounds.
pub fn rows_by_piece(pieces: &[PieceWords], speaker_of: &dyn Fn(&PieceWords) -> usize) -> Vec<(String, f64, f64, usize)> {
    let mut order: Vec<&PieceWords> = pieces.iter().filter(|p| !p.words.is_empty()).collect();
    order.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut rows: Vec<(String, f64, f64, usize)> = Vec::new();
    let mut i = 0;
    while i < order.len() {
        let speaker = speaker_of(order[i]);
        let mut group: Vec<PieceWords> = Vec::new();
        while i < order.len() && speaker_of(order[i]) == speaker {
            group.push(order[i].clone());
            i += 1;
        }
        rows.extend(assign(&group, &[Turn { start: f64::MIN, end: f64::MAX, speaker }]));
    }
    rows
}

/// One or two words of a voice after another voice spoke, then a change
/// again: the end of the earlier row whose times came out late, not a row
fn stray_tail(words: &[(String, f64, usize, f64, bool)], i: usize) -> bool {
    let s = words[i].2;
    let run = words[i..].iter().take_while(|w| w.2 == s).count();
    run <= 2 && i + run < words.len()
}

/// How far a word reaches from its time when nothing follows it soon
const WORD_REACH_S: f64 = 1.5;

/// Another voice spoke for a second or more between `from` and `to`: the
/// row of `speaker` does not go on over it
fn other_voice_between(turns: &[Turn], from: f64, to: f64, speaker: usize) -> bool {
    const MIN_S: f64 = 1.0;
    turns.iter().any(|x| x.speaker != speaker && x.end.min(to) - x.start.max(from) >= MIN_S)
}

/// Phrases that close one's own turn («…в мене все», «доповідь закінчив»):
/// said by the previous speaker even when the word times put them later
const CLOSING: &[&str] = &[
    "в мене все", "у мене все", "наразі все", "на цьому все", "доповідь закінчив", "доповідь закінчила", "доповідь закінчено",
    "все, дякую", "це все", "в мене поки все", "у мене поки все",
];

fn ends_sentence(w: &str) -> bool {
    w.ends_with(['.', '!', '?', '…'])
}

/// Word times from Whisper are off by a second or so, which cuts turns in
/// the middle of a sentence. A change of speaker inside a sentence moves to
/// the nearer end of that sentence (at most 12 words away); a short closing
/// phrase right after a change goes back to the one who said it.
fn snap_to_sentences(words: &mut [(String, f64, usize, f64, bool)]) {
    const MAX_SHIFT: usize = 12;
    let mut i = 1;
    while i < words.len() {
        if words[i].2 == words[i - 1].2 {
            i += 1;
            continue;
        }
        let (before, after) = (words[i - 1].2, words[i].2);
        let mut boundary = i;
        if !ends_sentence(&words[i - 1].0) {
            // Words to the end of this sentence, and back to its start
            let forward = (i..words.len()).take(MAX_SHIFT).find(|&k| ends_sentence(&words[k].0)).map(|k| k + 1 - i);
            let back = (0..i).rev().take(MAX_SHIFT).find(|&k| ends_sentence(&words[k].0)).map(|k| i - 1 - k).or((i <= MAX_SHIFT).then_some(i));
            let lower = words[i].0.chars().next().is_some_and(|c| c.is_lowercase());
            // A sentence that opens with a greeting belongs to whoever starts speaking
            let greets = back.is_some_and(|b| {
                let start = words[i - b..].iter().take(3).map(|w| w.0.to_lowercase()).collect::<Vec<_>>().join(" ");
                ["доброго", "добрий", "вітаю", "слава україні"].iter().any(|g| start.starts_with(g))
            });
            boundary = match (forward, back) {
                (_, Some(b)) if greets => i - b,
                (Some(f), Some(b)) if lower || f <= b => i + f,
                (Some(f), None) => i + f,
                (_, Some(b)) => i - b,
                (None, None) => i,
            };
            // Never across another change of speaker
            let limit_fwd = (i..words.len()).find(|&k| words[k].2 != after).unwrap_or(words.len());
            let limit_back = (0..i).rev().find(|&k| words[k].2 != before).map(|k| k + 1).unwrap_or(0);
            boundary = boundary.clamp(limit_back, limit_fwd);
        }
        // A closing phrase of the previous speaker right after the change
        let first_end = (boundary..words.len()).take(8).find(|&k| ends_sentence(&words[k].0));
        if let Some(k) = first_end {
            let sentence = words[boundary..=k].iter().map(|w| w.0.to_lowercase()).collect::<Vec<_>>().join(" ");
            if words[boundary..=k].iter().all(|w| w.2 == after) && CLOSING.iter().any(|c| sentence.contains(c)) {
                boundary = k + 1;
            }
        }
        for w in &mut words[i.min(boundary)..i.max(boundary)] {
            w.2 = if boundary > i { before } else { after };
        }
        i = boundary.max(i) + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(start: f64, end: f64, speaker: usize) -> Turn {
        Turn { start, end, speaker }
    }

    fn piece(start: f64, end: f64, words: &[(&str, f64)]) -> PieceWords {
        PieceWords {
            text: words.iter().map(|(w, _)| *w).collect::<Vec<_>>().join(" "),
            start,
            end,
            words: words.iter().map(|(w, t)| (w.to_string(), *t)).collect(),
            probs: Vec::new(),
            auto: Vec::new(), aligned: false, heard: vec![], bars: None, who: None,
        }
    }

    #[test]
    fn text_is_cut_where_the_voice_changes() {
        let pieces = vec![piece(0.0, 6.0, &[("Доброго", 0.3), ("ранку.", 0.9), ("Доповідаю,", 3.2), ("все", 3.8), ("добре.", 4.5)])];
        let turns = vec![turn(0.0, 1.5, 0), turn(2.8, 6.0, 1)];
        let rows = assign(&pieces, &turns);
        assert_eq!(rows.iter().map(|r| (r.0.as_str(), r.3)).collect::<Vec<_>>(), vec![("Доброго ранку.", 0), ("Доповідаю, все добре.", 1)]);
        // No word is lost or changed
        let joined = rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>().join(" ");
        assert_eq!(joined, pieces[0].text);
        assert_eq!(rows[1].1, 3.2);
    }

    #[test]
    fn a_one_word_slip_stays_with_the_speaker() {
        let pieces = vec![piece(0.0, 4.0, &[("раз", 0.5), ("два", 1.0), ("три", 1.5), ("чотири", 2.0), ("п'ять", 2.5)])];
        let turns = vec![turn(0.0, 1.2, 0), turn(1.2, 1.7, 1), turn(1.7, 4.0, 0)];
        let rows = assign(&pieces, &turns);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].3, 0);
    }

    #[test]
    fn a_row_does_not_stretch_over_another_voice() {
        // Speaker 1 talks 3–9 s, but Whisper heard no words of it
        let pieces = vec![piece(0.0, 12.0, &[("Перше", 0.5), ("питання.", 1.2), ("Друге", 9.5), ("питання.", 10.2)])];
        let turns = vec![turn(0.0, 2.5, 0), turn(3.0, 9.0, 1), turn(9.2, 12.0, 0)];
        let rows = assign(&pieces, &turns);
        assert_eq!(rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), vec!["Перше питання.", "Друге питання."]);
        assert!(rows[0].2 <= 2.7, "first row ends at {}", rows[0].2);
        assert_eq!(rows[1].1, 9.5);
    }

    #[test]
    fn a_late_last_word_stays_in_its_row() {
        // «готово.» timed after voice 1 started: still the end of voice 0's row
        let pieces = vec![piece(0.0, 12.0, &[("Все", 0.5), ("готово.", 4.6), ("Дякую,", 6.0), ("прийнято.", 6.8)])];
        let turns = vec![turn(0.0, 3.0, 0), turn(3.2, 4.5, 1), turn(4.5, 5.0, 0), turn(5.0, 12.0, 1)];
        let rows = assign(&pieces, &turns);
        assert_eq!(rows.iter().map(|r| (r.0.as_str(), r.3)).collect::<Vec<_>>(), vec![("Все готово.", 0), ("Дякую, прийнято.", 1)]);
    }

    #[test]
    fn a_piece_does_not_start_long_before_its_first_word() {
        let pieces = vec![piece(10.0, 20.0, &[("Так.", 16.0), ("Дякую.", 17.0)])];
        let turns = vec![turn(10.0, 15.0, 1), turn(15.5, 20.0, 0)];
        let rows = assign(&pieces, &turns);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, 14.5);
        assert_eq!(rows[0].3, 0);
    }

    #[test]
    fn speech_without_words_is_found() {
        let pieces = vec![piece(0.0, 12.0, &[("Перше", 0.5), ("питання.", 1.2), ("Друге", 9.5)])];
        // Voice 1 in two parts with a short pause, voice 2 too short, voice 0 has words
        let turns = vec![turn(0.0, 2.5, 0), turn(3.0, 5.0, 1), turn(5.3, 9.0, 1), turn(9.0, 9.4, 2), turn(9.4, 12.0, 0), turn(40.0, 70.0, 2)];
        let silent = silent_turns(&pieces, &turns);
        assert_eq!(silent, vec![turn(3.0, 9.0, 1), turn(40.0, 65.0, 2), turn(65.0, 70.0, 2)]);
        // Words of the same speech, only timed a little off, are not added twice
        assert!(already_nearby(&pieces, &piece(9.0, 11.0, &[("друге,", 9.1)])));
        assert!(!already_nearby(&pieces, &piece(3.0, 9.0, &[("Склад", 3.2), ("готовий", 3.8), ("питання.", 4.0)])));
    }

    fn w(text: &str, speaker: usize) -> Vec<(String, f64, usize, f64, bool)> {
        text.split(' ').map(|x| (x.to_string(), 0.0, speaker, 0.0, false)).collect()
    }

    fn speakers_of(words: &[(String, f64, usize, f64, bool)]) -> String {
        words.iter().map(|x| x.2.to_string()).collect()
    }

    #[test]
    fn a_change_inside_a_sentence_moves_to_its_end() {
        // «…які можуть до них | приводити. Наразі в мене все. | Стан ринку…»
        let mut words = w("Повідомляйте про події які можуть до них", 0);
        words.extend(w("приводити. Наразі в мене все. Стан ринку без змін.", 1));
        snap_to_sentences(&mut words);
        // Up to «все.» is the first speaker's
        assert_eq!(speakers_of(&words), "0000000000001111");
    }

    #[test]
    fn a_change_late_in_a_sentence_moves_back_to_its_start() {
        let mut words = w("Все виконано. Доброго ранку, С2", 0);
        words.extend(w("доповідає. Без змін.", 1));
        snap_to_sentences(&mut words);
        // «Доброго ранку, С2 доповідає.» opens the next speaker's turn
        assert_eq!(speakers_of(&words), "00111111");
        let mut words = w("Все виконано. Доброго ранку,", 0);
        words.extend(w("С2 доповідає щось довге і без крапки ще кілька слів тут далі.", 1));
        snap_to_sentences(&mut words);
        assert_eq!(speakers_of(&words), "0011111111111111");
    }

    #[test]
    fn changes_at_sentence_ends_stay() {
        let mut words = w("Все виконано.", 0);
        words.extend(w("Дякую. Далі.", 1));
        snap_to_sentences(&mut words);
        assert_eq!(speakers_of(&words), "0011");
    }

    #[test]
    fn words_between_turns_go_to_the_closest() {
        let turns = vec![turn(0.0, 1.0, 0), turn(5.0, 6.0, 1)];
        assert_eq!(speaker_at(&turns, 1.5), Some(0));
        assert_eq!(speaker_at(&turns, 4.0), Some(1));
        assert_eq!(speaker_at(&[], 1.0), None);
    }

    #[test]
    fn a_corrected_line_updates_the_word_times() {
        let mut pieces = vec![
            piece(0.0, 4.0, &[("Доповідає", 0.5), ("Вузол", 1.0), ("два,", 1.5)]),
            piece(4.0, 8.0, &[("прат", 4.5), ("на", 5.0), ("місці.", 5.5)]),
        ];
        assert!(replace_line(&mut pieces, "Вузол два, прат на", "Вузол-2, ПрАТ на", 1.0));
        // Each word stays in its piece with its moment; a changed word takes the old one's
        assert_eq!(pieces[0].text, "Доповідає Вузол-2,");
        assert_eq!(pieces[1].text, "ПрАТ на місці.");
        assert_eq!(pieces[1].words[0], ("ПрАТ".to_string(), 4.5));
        assert!(pieces.iter().all(|p| p.words.iter().all(|w| w.1 >= p.start && w.1 <= p.end)));
        // The transcript with the correction still matches the word times
        assert!(matches_transcript(&pieces, &["Доповідає Вузол-2, ПрАТ на місці.".to_string()]));
        assert!(!replace_line(&mut pieces, "немає такого", "x", 0.0));
    }

    #[test]
    fn doubtful_words_snap_and_keep_what_was_heard() {
        let mut p = piece(0.0, 2.0, &[("Доповідає", 0.1), ("Кончаренко,", 0.5), ("все", 0.9), ("добре.", 1.2)]);
        p.probs = vec![0.95, 0.5, 0.9, 0.97];
        let terms = vec!["Гончаренко".to_string()];
        snap_doubtful(&mut p, &terms);
        assert_eq!(p.text, "Доповідає Гончаренко, все добре.");
        assert_eq!(p.auto, vec![(1, "Кончаренко,".to_string())]);
        // Old files without confidence: nothing is doubtful
        let mut old = piece(0.0, 1.0, &[("Кончаренко", 0.1)]);
        snap_doubtful(&mut old, &terms);
        assert_eq!(old.text, "Кончаренко");
    }

    #[test]
    fn own_speech_is_cut_from_the_mix() {
        let mixed = vec![0.0f32; 16_000 * 40];
        let turns = vec![turn(2.0, 4.0, super::super::timeline::ME), turn(5.0, 35.0, super::super::timeline::ME), turn(39.0, 39.05, super::super::timeline::ME)];
        let pieces = own_speech(&mixed, &turns);
        // 1.8–4.2, then 4.8–29.8 and 29.8–35.2; the 0.05 s turn is too short
        assert_eq!(pieces.len(), 3);
        assert!((pieces[0].start_timestamp_ms - 1800.0).abs() < 1.0);
        assert!((pieces[1].end_timestamp_ms - 29_800.0).abs() < 1.0);
        assert_eq!(pieces[1].samples.len(), 16_000 * 25);
    }

    #[test]
    fn doubts_map_to_lines_and_can_be_resolved() {
        let mut a = piece(0.0, 3.0, &[("Доповідає", 0.1), ("Гончаренко,", 0.5), ("отримали", 1.0), ("12", 1.5)]);
        a.probs = vec![0.95, 0.9, 0.3, 0.99];
        a.auto = vec![(1, "Кончаренко,".to_string())];
        let mut b = piece(3.0, 5.0, &[("Все", 3.2), ("добре.", 3.6)]);
        b.probs = vec![0.9, 0.9];
        let mut pieces = vec![a, b];
        let rows = vec![("r1".to_string(), "Доповідає Гончаренко,".to_string()), ("r2".to_string(), "отримали 12 Все добре.".to_string())];
        let d = doubts(&pieces, &rows, &["Гончаренко".to_string()]);
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].segment_id.as_str(), d[0].word_index, d[0].heard.as_deref()), ("r1", 1, Some("Кончаренко,")));
        assert_eq!((d[1].segment_id.as_str(), d[1].word_index, d[1].word.as_str()), ("r2", 0, "отримали"));
        assert!(resolve(&mut pieces, 1.0, "отримали"));
        assert!(resolve(&mut pieces, 0.5, "Гончаренко,"));
        assert!(doubts(&pieces, &rows, &["Гончаренко".to_string()]).is_empty());
        // A line changed in places (a dictionary rule, a typed fix, punctuation)
        // keeps the doubts of the words left as heard
        let mut e = piece(10.0, 16.0, &[("Ми", 10.1), ("подивимося,", 10.5), ("що", 11.0), ("по", 11.2), ("шинам", 11.5), ("Ковальчук", 12.0), ("скаже", 12.5)]);
        e.probs = vec![0.99, 0.3, 0.99, 0.4, 0.99, 0.2, 0.99];
        let rows = vec![("r3".to_string(), "Ми подивимося що по шинах Ковальчуку скаже.".to_string())];
        let d = doubts(&[e], &rows, &[]);
        assert_eq!(d.iter().map(|x| x.word_index).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(d[0].word, "подивимося,");
        // An edit keeps confidence aligned with the words
        let mut c = piece(0.0, 2.0, &[("раз", 0.1), ("двa", 0.5), ("три", 0.9)]);
        c.probs = vec![0.2, 0.9, 0.3];
        let mut v = vec![c];
        assert!(replace_line(&mut v, "раз двa", "один два", 0.1));
        assert_eq!(v[0].probs, vec![1.0, 1.0, 0.3]);
    }

    #[test]
    fn edited_transcripts_do_not_match() {
        let pieces = vec![piece(0.0, 2.0, &[("Так", 0.5), ("точно.", 1.0)])];
        assert!(matches_transcript(&pieces, &["Так  точно.".to_string()]));
        assert!(!matches_transcript(&pieces, &["Так, точно.".to_string()]));
        // Heard stream by stream: the lines of people who talk at once need
        // not come in the pieces' order, the words are the same
        let piece = |who: &str, text: &str| PieceWords {
            text: text.into(),
            start: 0.0,
            end: 3.0,
            words: text.split_whitespace().map(|w| (w.to_string(), 0.0)).collect(),
            probs: Vec::new(),
            auto: Vec::new(), aligned: false, heard: vec![], bars: None, who: Some(who.into()),
        };
        let pieces = vec![piece("А", "План готовий. Далі склад."), piece("Б", "Так, згоден.")];
        assert!(matches_transcript(&pieces, &["План готовий.".to_string(), "Так, згоден.".to_string(), "Далі склад.".to_string()]));
        assert!(!matches_transcript(&pieces, &["План готовий.".to_string(), "Так, згодна.".to_string(), "Далі склад.".to_string()]));
    }
}
