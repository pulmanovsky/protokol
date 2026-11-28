//! Google Meet's own captions as a second text of the call.
//!
//! In Protokol's Meet window capture.js turns Meet's captions on by itself,
//! in Ukrainian and leaves them on the screen (unless the person turns them
//! off themselves), reads them off the page (who, what, when first and last seen)
//! and they come with its reports. Meet keeps rewriting a caption while the person speaks, so only
//! the last version of each is kept. They are Google's recognition, made
//! during the call; Protokol only compares its own transcript with them:
//! where both heard the same words the text is likely right, where they
//! differ a word is worth a look, and where Meet wrote words while Whisper
//! wrote nothing a remark may be missing.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::quality::score::{align, norm};
use crate::speakers::words::PieceWords;

/// One caption block, as Meet left it
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Caption {
    /// The page it came from and its number there («<page start>:<id>»)
    pub key: String,
    /// The speaker's name as Meet shows it
    pub who: String,
    pub text: String,
    /// First and last seen (ms since 1970)
    pub t0: f64,
    pub t1: f64,
    /// How the text grew: (words, when it first had that many, ms since
    /// 1970), from the versions the reports brought. Each word's time comes
    /// from here; captions kept before this have none and their words are
    /// spread evenly over the block
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grew: Vec<(u32, f64)>,
}

/// Adds the captions of one report, a newer version replacing an older one
pub fn add_report(captions: &mut Vec<Caption>, r: &Value) {
    let Some(list) = r.get("captions").and_then(|v| v.as_array()) else { return };
    let page = r.get("started").and_then(|v| v.as_f64()).unwrap_or(0.0);
    for c in list {
        let (Some(id), Some(text)) = (c.get("id").and_then(|v| v.as_u64()), c.get("text").and_then(|v| v.as_str())) else { continue };
        let key = format!("{page:.0}:{id}");
        let who = c.get("who").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let t0 = c.get("t0").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let t1 = c.get("t1").and_then(|v| v.as_f64()).unwrap_or(t0);
        let n = text.split_whitespace().count() as u32;
        match captions.iter_mut().find(|x| x.key == key) {
            Some(x) if t1 >= x.t1 => {
                x.text = text.to_string();
                x.who = who;
                x.t1 = t1;
                grow(&mut x.grew, n, t1);
            }
            Some(_) => {}
            None => {
                let mut c = Caption { key, who, text: text.to_string(), t0, t1, grew: Vec::new() };
                grow(&mut c.grew, n, t1);
                captions.push(c);
            }
        }
    }
}

/// Notes when a caption first had `n` words (Meet also rewrites a little,
/// so only a count above the highest so far counts)
fn grow(grew: &mut Vec<(u32, f64)>, n: u32, t: f64) {
    if n > 0 && grew.last().map_or(true, |g| n > g.0) {
        grew.push((n, t));
    }
}

/// How long after the words were said Meet shows them (s): about a second
/// on a good connection; the rest of the delay, which can be longer, is
/// found from the words themselves ([`fit_clock`])
const SHOWN_LATE_S: f64 = 0.8;
/// Caption words this far before or after a piece are compared with it (s)
const NEAR_S: f64 = 4.0;
/// A word spread evenly over a caption kept without its growth may be off
/// by this share of the caption's length (s), up to `MAX_SLACK_S`
const SPREAD_SLACK: f64 = 0.25;
const MAX_SLACK_S: f64 = 30.0;
/// Caption words with no Whisper words within this distance (s), this many
/// in a row, are a remark Whisper may have missed
const GAP_S: f64 = 1.5;
const MISSED_WORDS: usize = 3;

/// One caption word on the recording's clock
#[derive(Debug, Clone, PartialEq)]
pub struct CapWord {
    /// As compared ([`norm`])
    pub word: String,
    /// Seconds from the recording's start
    pub t: f64,
    /// The speaker's name as Meet shows it
    pub who: String,
    /// How far off `t` may be (s): 0 when known from the caption's growth
    pub slack: f64,
    /// The caption it is from (its place in the list): a caption's words
    /// move together and keep their order
    pub block: usize,
}

/// Caption words on the recording's clock (s from its start): each word at
/// the moment its caption first had it, or, for a caption kept without its
/// growth, spread evenly between when it was first and last seen
pub fn words(captions: &[Caption], start_ms: f64) -> Vec<CapWord> {
    let mut out = Vec::new();
    for (block, c) in captions.iter().enumerate() {
        let raw: Vec<&str> = c.text.split_whitespace().collect();
        if raw.is_empty() {
            continue;
        }
        let at = |ms: f64| (ms - start_ms) / 1000.0 - SHOWN_LATE_S;
        let a = at(c.t0);
        let b = at(c.t1).max(a);
        let n = raw.len();
        let spread_slack = (SPREAD_SLACK * (b - a)).min(MAX_SLACK_S);
        for (i, w) in raw.iter().enumerate() {
            let word = norm(w);
            if word.is_empty() {
                continue;
            }
            let (t, slack) = match c.grew.iter().find(|g| g.0 as usize > i) {
                Some(g) => (at(g.1).max(a), 0.0),
                None if !c.grew.is_empty() => (b, 0.0),
                None => (if n > 1 { a + (b - a) * i as f64 / (n - 1) as f64 } else { a }, spread_slack),
            };
            out.push(CapWord { word, t, who: c.who.clone(), slack, block });
        }
    }
    out.sort_by(|x, y| x.t.total_cmp(&y.t));
    out
}

/// The caption clock is fitted window by window (s of captions)
const CLOCK_WINDOW_S: f64 = 120.0;
/// How far the captions may be from the recording (s): their delay, a late
/// start of the recording, or a pause in it
const CLOCK_MAX_S: f64 = 900.0;
const CLOCK_BIN_S: f64 = 2.0;
/// Words this long or longer say where the captions are (short ones are everywhere)
const CLOCK_MIN_LETTERS: usize = 5;
/// A window's shift needs this much agreement, and this many times the
/// background of chance matches
const CLOCK_MIN_VOTES: f64 = 5.0;
const CLOCK_ABOVE_NOISE: f64 = 4.0;

/// Moves the caption words onto the recording's own clock. The folder's
/// start is not exactly when the sound starts, Meet shows its captions
/// late (longer on a weak connection), and a pause leaves the recording
/// behind the call: the shift is found window by window from the same
/// longer words in the captions and in what Whisper heard (word, s), a
/// window without enough of them taking its neighbour's. Returns the
/// shifts found (s), oldest first; empty when nothing fits (nothing moved).
pub fn fit_clock(caps: &mut [CapWord], heard: &[(String, f64)]) -> Vec<f64> {
    let mut index: HashMap<&str, Vec<f64>> = HashMap::new();
    for (i, (w, t)) in heard.iter().enumerate() {
        // Words stacked on one moment (the end of a piece whose words could
        // not be timed) say nothing of when they were said
        let stacked = (i > 0 && heard[i - 1].1 == *t) || heard.get(i + 1).is_some_and(|n| n.1 == *t);
        if w.chars().count() >= CLOCK_MIN_LETTERS && !stacked {
            index.entry(w.as_str()).or_default().push(*t);
        }
    }
    if caps.is_empty() || index.is_empty() {
        return Vec::new();
    }
    caps.sort_by(|x, y| x.t.total_cmp(&y.t));
    let first = caps.iter().map(|c| c.t).fold(f64::INFINITY, f64::min);
    let last = caps.iter().map(|c| c.t).fold(f64::NEG_INFINITY, f64::max);
    let windows = ((last - first) / CLOCK_WINDOW_S).floor() as usize + 1;
    let window = |t: f64| (((t - first) / CLOCK_WINDOW_S).floor().max(0.0) as usize).min(windows - 1);
    // Per window: votes per shift bin (a caption word's vote split over its
    // matches, so a word said everywhere counts little) and the shifts
    let mut votes: Vec<HashMap<i64, f64>> = vec![HashMap::new(); windows];
    let mut shifts: Vec<Vec<(f64, f64)>> = vec![Vec::new(); windows];
    for c in caps.iter().filter(|c| c.word.chars().count() >= CLOCK_MIN_LETTERS) {
        let Some(ts) = index.get(c.word.as_str()) else { continue };
        let ds: Vec<f64> = ts.iter().map(|t| t - c.t).filter(|d| d.abs() <= CLOCK_MAX_S).collect();
        if ds.is_empty() {
            continue;
        }
        let k = window(c.t);
        let weight = 1.0 / ds.len() as f64;
        for d in ds {
            *votes[k].entry((d / CLOCK_BIN_S).round() as i64).or_default() += weight;
            shifts[k].push((d, weight));
        }
    }
    let bins = 2.0 * CLOCK_MAX_S / CLOCK_BIN_S + 1.0;
    let found: Vec<Option<f64>> = (0..windows)
        .map(|k| {
            let v = &votes[k];
            let total: f64 = v.values().sum();
            let around = |b: i64| v.get(&(b - 1)).unwrap_or(&0.0) + v.get(&b).unwrap_or(&0.0) + v.get(&(b + 1)).unwrap_or(&0.0);
            // The best bin with its neighbours; on a tie the smaller shift
            let (b, n) = v.keys().map(|&b| (b, around(b))).max_by(|x, y| x.1.total_cmp(&y.1).then(y.0.abs().cmp(&x.0.abs())))?;
            if n < CLOCK_MIN_VOTES || n < CLOCK_ABOVE_NOISE * 3.0 * total / bins {
                return None;
            }
            // The weighted middle of the shifts around it
            let centre = b as f64 * CLOCK_BIN_S;
            let mut near: Vec<(f64, f64)> = shifts[k].iter().copied().filter(|(d, _)| (d - centre).abs() <= 1.5 * CLOCK_BIN_S).collect();
            near.sort_by(|x, y| x.0.total_cmp(&y.0));
            let half = near.iter().map(|x| x.1).sum::<f64>() / 2.0;
            let mut acc = 0.0;
            near.iter().find(|(_, w)| {
                acc += w;
                acc >= half
            }).map(|x| x.0)
        })
        .collect();
    let known: Vec<(usize, f64)> = found.iter().enumerate().filter_map(|(k, s)| s.map(|s| (k, s))).collect();
    if known.is_empty() {
        return Vec::new();
    }
    let shift_of = |k: usize| found[k].unwrap_or_else(|| known.iter().min_by_key(|(j, _)| (*j as i64 - k as i64).abs()).map(|x| x.1).unwrap_or(0.0));
    // Caption by caption, the shift of its window or a neighbouring one that
    // puts most of its words where Whisper heard them (so a pause is
    // followed where it falls, not where a window ends). A caption's words
    // move together: moved one by one, neighbours would swap places.
    let mut by_block: HashMap<usize, Vec<f64>> = HashMap::new();
    for c in caps.iter() {
        if c.word.chars().count() < CLOCK_MIN_LETTERS {
            continue;
        }
        let Some(ts) = index.get(c.word.as_str()) else { continue };
        let k = window(c.t);
        let mut options = vec![shift_of(k), shift_of(k.saturating_sub(1)), shift_of((k + 1).min(windows - 1))];
        options.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        let best = options
            .into_iter()
            .filter_map(|s| {
                let off = ts.iter().map(|t| (t - c.t - s).abs()).fold(f64::INFINITY, f64::min);
                (off <= CLOCK_BIN_S).then_some((off, s))
            })
            .min_by(|x, y| x.0.total_cmp(&y.0));
        if let Some((_, s)) = best {
            by_block.entry(c.block).or_default().push(s);
        }
    }
    let block_shift: HashMap<usize, f64> = by_block
        .into_iter()
        .map(|(b, mut v)| {
            v.sort_by(|x, y| x.total_cmp(y));
            (b, v[v.len() / 2])
        })
        .collect();
    // A caption with none of its words to go by: its first word's window
    let mut first_t: HashMap<usize, f64> = HashMap::new();
    for c in caps.iter() {
        first_t.entry(c.block).or_insert(c.t);
    }
    for c in caps.iter_mut() {
        c.t += block_shift.get(&c.block).copied().unwrap_or_else(|| shift_of(window(first_t[&c.block])));
    }
    caps.sort_by(|x, y| x.t.total_cmp(&y.t));
    known.into_iter().map(|x| x.1).collect()
}

/// What the captions have where Whisper heard a word
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Said {
    /// The same word
    Same,
    /// Nearly the same: another ending, «у» for «в», «із» for «з»
    Alike,
    /// Another word
    Other,
    /// Nothing there
    Nothing,
}

/// What the captions say about the transcript
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Comparison {
    /// For each piece, each Whisper word: what the captions have there; None
    /// when there are no captions near (or the word is no word)
    pub said: Vec<Vec<Option<Said>>>,
    /// Remarks only in the captions: (from, to in s, who, text)
    pub missed: Vec<(f64, f64, String, String)>,
}

/// Compares the transcript's pieces with the caption words (from `words`,
/// moved by `fit_clock`; in time order, as both leave them): each piece with the caption words around it, its
/// words fitted to the stretch of them that matches best
pub fn compare(pieces: &[PieceWords], caps: &[CapWord]) -> Comparison {
    compare_with(pieces, caps, true)
}

/// The comparison as made until 0.4.7: a fixed window of `NEAR_S` around
/// each piece, both ends of it compared; for the report, to show what the
/// clock and the fitting changed
pub fn compare_as_before(pieces: &[PieceWords], caps: &[CapWord]) -> Comparison {
    compare_with(pieces, caps, false)
}

fn compare_with(pieces: &[PieceWords], caps: &[CapWord], fitted: bool) -> Comparison {
    let mut out = Comparison::default();
    let mut used = vec![false; caps.len()];
    for p in pieces {
        let heard: Vec<String> = p.words.iter().map(|(w, _)| norm(w)).collect();
        // Words that count, by their place in the piece
        let keep: Vec<usize> = (0..heard.len()).filter(|&j| !heard[j].is_empty()).collect();
        // Caption words are in time order: look only around the piece
        let reach = NEAR_S + if fitted { MAX_SLACK_S } else { 0.0 };
        let (from, to) = (caps.partition_point(|c| c.t < p.start - reach), caps.partition_point(|c| c.t <= p.end + reach));
        let near: Vec<usize> = (from..to.max(from))
            .filter(|&i| {
                let m = NEAR_S + if fitted { caps[i].slack } else { 0.0 };
                caps[i].t >= p.start - m && caps[i].t <= p.end + m
            })
            .collect();
        let mut row: Vec<Option<Said>> = vec![None; heard.len()];
        if near.is_empty() || keep.is_empty() {
            out.said.push(row);
            continue;
        }
        let heard_w: Vec<String> = keep.iter().map(|&j| heard[j].clone()).collect();
        let reference: Vec<String> = near.iter().map(|&i| caps[i].word.clone()).collect();
        // A number is one token on both sides, however written («8000» and «вісім тисяч»)
        let (heard_t, heard_spans) = numbers_as_one(&heard_w);
        let (ref_t, ref_spans) = numbers_as_one(&reference);
        let pairs = if fitted { fit(&ref_t, &heard_t) } else { pairs_of(&ref_t, &heard_t) };
        for (k, pair) in pairs.iter().enumerate() {
            let said = match pair {
                Some(r) if ref_t[*r] == heard_t[k] => Said::Same,
                Some(r) if alike(&ref_t[*r], &heard_t[k]) => Said::Alike,
                Some(_) => Said::Other,
                None => Said::Nothing,
            };
            for j in heard_spans[k].clone() {
                row[keep[j]] = Some(said);
            }
            if let Some(r) = pair {
                for idx in ref_spans[*r].clone() {
                    used[near[idx]] = true;
                }
            }
        }
        out.said.push(row);
    }
    if !fitted {
        return out;
    }
    // Runs of caption words no piece took, with no Whisper words nearby
    let whisper_times: Vec<f64> = pieces.iter().flat_map(|p| p.words.iter().map(|w| w.1)).collect();
    let quiet = |c: &CapWord| !whisper_times.iter().any(|&w| (w - c.t).abs() <= GAP_S + c.slack);
    let mut i = 0;
    while i < caps.len() {
        if used[i] || !quiet(&caps[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < caps.len() && !used[i] && quiet(&caps[i]) && caps[i].who == caps[start].who {
            i += 1;
        }
        if i - start >= MISSED_WORDS {
            let text = caps[start..i].iter().map(|c| c.word.as_str()).collect::<Vec<_>>().join(" ");
            out.missed.push((caps[start].t, caps[i - 1].t, caps[start].who.clone(), text));
        }
    }
    out
}

/// Nearly the same word: another ending, or the sounds Ukrainian swaps for
/// ease of saying (у/в, і/й at the start; з, із, зі, зо). Numbers are not.
fn alike(a: &str, b: &str) -> bool {
    if a.starts_with('#') || b.starts_with('#') {
        return false;
    }
    fn soft(w: &str) -> String {
        match w {
            "із" | "зі" | "зо" => "з".to_string(),
            "іще" => "ще".to_string(),
            _ => {
                let mut c = w.chars();
                match c.next() {
                    Some('у') => format!("в{}", c.as_str()),
                    Some('і') => format!("й{}", c.as_str()),
                    _ => w.to_string(),
                }
            }
        }
    }
    soft(a) == soft(b) || crate::learning::measure::same_stem(a, b)
}

/// Where the recognised words best fit among the caption words: as
/// [`align`], but caption words before and after the stretch that fits
/// cost nothing (the window around a piece holds more than the piece).
/// For each recognised word, the caption word it is paired with (the same
/// or another), None for a word the captions do not have there.
fn fit(reference: &[String], heard: &[String]) -> Vec<Option<usize>> {
    let (n, m) = (reference.len(), heard.len());
    // (edits, −right words, −letters shared by wrong pairs): smaller is better
    let mut d = vec![vec![(0u32, 0i32, 0i32); m + 1]; n + 1];
    for j in 0..=m {
        d[0][j] = (j as u32, 0, 0);
    }
    let shared = |a: &str, b: &str| a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count() as i32;
    let diag = |d: &Vec<Vec<(u32, i32, i32)>>, i: usize, j: usize| {
        let (e, r, s) = d[i - 1][j - 1];
        if reference[i - 1] == heard[j - 1] {
            (e, r - 1, s)
        } else {
            (e + 1, r, s - shared(&reference[i - 1], &heard[j - 1]))
        }
    };
    let step = |c: (u32, i32, i32)| (c.0 + 1, c.1, c.2);
    for i in 1..=n {
        for j in 1..=m {
            d[i][j] = diag(&d, i, j).min(step(d[i - 1][j])).min(step(d[i][j - 1]));
        }
    }
    let mut out = vec![None; m];
    // The best end among the caption words; the rest after it is free
    let Some(mut i) = (0..=n).min_by_key(|&i| d[i][m]) else { return out };
    let mut j = m;
    while j > 0 {
        if i > 0 && d[i][j] == diag(&d, i, j) {
            out[j - 1] = Some(i - 1);
            i -= 1;
            j -= 1;
        } else if d[i][j] == step(d[i][j - 1]) {
            j -= 1;
        } else {
            i -= 1;
        }
    }
    out
}

/// The pairs of [`align`] over the whole window, as until 0.4.7
fn pairs_of(reference: &[String], heard: &[String]) -> Vec<Option<usize>> {
    let a = align(reference, heard);
    let mut out = vec![None; heard.len()];
    for &(ri, j) in &a.wrong {
        out[j] = Some(ri);
    }
    let right: Vec<usize> = (0..heard.len()).filter(|&j| a.correct[j]).collect();
    for (j, ri) in right.into_iter().zip(matched_pairs(reference, heard, &a.correct)) {
        out[j] = Some(ri);
    }
    out
}

/// Words with each number as one token («#8000»), and the words each token
/// stands for
fn numbers_as_one(words: &[String]) -> (Vec<String>, Vec<std::ops::Range<usize>>) {
    // Where each word starts and ends in the words joined by spaces (chars)
    let mut places = Vec::with_capacity(words.len());
    let mut at = 0;
    for w in words {
        let n = w.chars().count();
        places.push((at, at + n));
        at += n + 1;
    }
    let numbers = crate::summary::numbers::extract(&words.join(" "));
    let mut tokens = Vec::new();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let (s, e) = places[i];
        let number = numbers.iter().find(|f| f.start < e && f.end > s).and_then(|f| f.keys.first().map(|k| (f, k)));
        match number {
            Some((f, key)) => {
                let mut j = i + 1;
                while j < words.len() && places[j].0 < f.end {
                    j += 1;
                }
                tokens.push(format!("#{key}"));
                spans.push(i..j);
                i = j;
            }
            None => {
                tokens.push(words[i].clone());
                spans.push(i..i + 1);
                i += 1;
            }
        }
    }
    (tokens, spans)
}

/// Places in the caption words of the recognised words that match exactly,
/// in order (a greedy pass along the alignment's right words)
fn matched_pairs(reference: &[String], heard: &[String], correct: &[bool]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut r = 0;
    for (j, ok) in correct.iter().enumerate() {
        if !ok {
            continue;
        }
        while r < reference.len() && reference[r] != heard[j] {
            r += 1;
        }
        if r < reference.len() {
            out.push(r);
            r += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn piece(start: f64, words: &[(&str, f64)]) -> PieceWords {
        PieceWords {
            text: words.iter().map(|w| w.0).collect::<Vec<_>>().join(" "),
            start,
            end: words.last().map(|w| w.1 + 0.4).unwrap_or(start),
            words: words.iter().map(|(w, t)| (w.to_string(), *t)).collect(),
            ..Default::default()
        }
    }

    fn cap(key: &str, who: &str, text: &str, t0: f64, t1: f64) -> Caption {
        Caption { key: key.into(), who: who.into(), text: text.into(), t0, t1, grew: Vec::new() }
    }

    #[test]
    fn the_last_version_of_each_caption_is_kept_with_how_it_grew() {
        let mut caps = Vec::new();
        add_report(&mut caps, &json!({"started": 5, "captions": [
            {"id": 0, "who": "Олена", "text": "Доброго", "t0": 1000, "t1": 1000},
            {"id": 0, "who": "Олена", "text": "Доброго ранку", "t0": 1000, "t1": 1500}]}));
        add_report(&mut caps, &json!({"started": 5, "captions": [
            // Meet rewrites a word: the count stays, nothing new is noted
            {"id": 0, "who": "Олена", "text": "Добрий ранок", "t0": 1000, "t1": 2000},
            {"id": 0, "who": "Олена", "text": "Доброго ранку, колеги.", "t0": 1000, "t1": 4000},
            {"id": 1, "who": "Іван", "text": "Дякую", "t0": 4200, "t1": 4300}]}));
        // Another page of the same window numbers its captions from 0 again
        add_report(&mut caps, &json!({"started": 9, "captions": [{"id": 0, "who": "Іван", "text": "Ще раз", "t0": 9000, "t1": 9100}]}));
        assert_eq!(caps.len(), 3);
        assert_eq!(caps[0].text, "Доброго ранку, колеги.");
        assert_eq!((caps[0].t0, caps[0].t1), (1000.0, 4000.0));
        assert_eq!(caps[0].grew, vec![(1, 1000.0), (2, 1500.0), (3, 4000.0)]);
        // Kept with the recording and read back; older copies without it still read
        let back: Caption = serde_json::from_str(&serde_json::to_string(&caps[0]).unwrap()).unwrap();
        assert_eq!(back, caps[0]);
        let old: Caption = serde_json::from_str(r#"{"key":"a","who":"Іван","text":"Так","t0":1,"t1":2}"#).unwrap();
        assert!(old.grew.is_empty());
    }

    #[test]
    fn caption_words_go_on_the_recording_clock() {
        // Kept without its growth: spread evenly over the caption, with room to be off
        let caps = vec![cap("a", "Олена", "Доброго ранку, колеги.", 11_800.0, 13_800.0)];
        let w = words(&caps, 10_000.0);
        assert_eq!(w.iter().map(|x| x.word.as_str()).collect::<Vec<_>>(), ["доброго", "ранку", "колеги"]);
        assert!((w[0].t - 1.0).abs() < 1e-9 && (w[2].t - 3.0).abs() < 1e-9);
        assert!((w[1].slack - 0.5).abs() < 1e-9);
        // With its growth: each word when it first showed, however long the caption
        let mut c = cap("b", "Іван", "Склад заповнено на вісімдесят відсотків", 20_800.0, 80_800.0);
        c.grew = vec![(2, 21_800.0), (3, 22_300.0), (5, 79_800.0)];
        let w = words(&[c], 10_000.0);
        let t: Vec<f64> = w.iter().map(|x| (x.t * 10.0).round() / 10.0).collect();
        assert_eq!(t, [11.0, 11.0, 11.5, 69.0, 69.0]);
        assert!(w.iter().all(|x| x.slack == 0.0));
    }

    #[test]
    fn differing_words_and_a_missed_remark_are_found() {
        let caps = vec![
            cap("a", "Олена", "Склад заповнено на вісімдесят відсотків", 1_800.0, 5_800.0),
            cap("b", "Іван", "Зрозуміло, дякую за доповідь", 20_800.0, 22_800.0),
        ];
        let w = words(&caps, 0.0);
        // Whisper heard the first remark with one word wrong and wrote nothing for the second
        let pieces = vec![piece(1.0, &[("Склад", 1.0), ("заповнено", 2.0), ("на", 3.0), ("вісімнадцять", 3.5), ("відсотків", 4.5)])];
        let c = compare(&pieces, &w);
        use Said::*;
        assert_eq!(c.said[0], vec![Some(Same), Some(Same), Some(Same), Some(Other), Some(Same)]);
        assert_eq!(c.missed.len(), 1);
        assert_eq!(c.missed[0].2, "Іван");
        assert_eq!(c.missed[0].3, "зрозуміло дякую за доповідь");
    }

    #[test]
    fn a_number_in_digits_and_in_words_is_the_same() {
        let caps = vec![cap("a", "Олена", "Зібрали 8000 грн на ремонт", 1_800.0, 4_800.0)];
        let w = words(&caps, 0.0);
        let pieces = vec![piece(1.0, &[("Зібрали", 1.0), ("вісім", 1.6), ("тисяч", 2.0), ("гривень", 2.6), ("на", 3.2), ("ремонт", 3.6)])];
        let c = compare(&pieces, &w);
        use Said::*;
        // «грн» and «гривень» stay different words; the number agrees
        assert_eq!(c.said[0], vec![Some(Same), Some(Same), Some(Same), Some(Other), Some(Same), Some(Same)]);
        let (t, spans) = numbers_as_one(&["вісім".into(), "тисяч".into(), "гривень".into()]);
        assert_eq!(t, ["#8000", "гривень"]);
        assert_eq!(spans, [0..2, 2..3]);
    }

    #[test]
    fn nearly_the_same_words_are_told_from_other_words() {
        assert!(alike("у", "в") && alike("вже", "уже") && alike("зі", "з") && alike("йти", "іти"));
        assert!(alike("доповідав", "доповідала") && alike("склат", "склад"));
        assert!(!alike("вісімдесят", "вісімнадцять") && !alike("грн", "гривень") && !alike("#800", "#8000"));
        let caps = vec![cap("a", "Олена", "Доповідала у відділі", 1_000.0, 3_000.0)];
        let c = compare(&[piece(0.5, &[("доповідав", 0.5), ("в", 1.2), ("відділі", 1.8)])], &words(&caps, 0.0));
        assert_eq!(c.said[0], vec![Some(Said::Alike), Some(Said::Alike), Some(Said::Same)]);
    }

    /// A long remark: what was said, word by word every 0.4 s from `from`
    fn speech(n: usize, from: f64) -> Vec<(String, f64)> {
        // Made-up words, each said once, and a word said every seventh time
        let syl = ["ба", "ве", "гі", "до", "жу", "за", "ки", "ло", "ми", "но", "пу", "ру"];
        (0..n)
            .map(|i| {
                let w = if i % 7 == 3 { "склад".to_string() } else { format!("{}{}{}к", syl[i % 12], syl[i / 12 % 12], syl[i / 144 % 12]) };
                (w, from + 0.4 * i as f64)
            })
            .collect()
    }

    #[test]
    fn the_clock_follows_the_delay_and_a_pause() {
        // 300 words; the captions show them 6 s late; after the 150th the
        // recording was paused for 40 s, so the call is 40 s ahead from there
        let said = speech(300, 10.0);
        let mut caps: Vec<CapWord> = said
            .iter()
            .enumerate()
            .map(|(i, (w, t))| CapWord { word: w.clone(), t: t + 6.0 + if i >= 150 { 40.0 } else { 0.0 }, who: "Олена".into(), slack: 0.0, block: i / 10 })
            .collect();
        let shifts = fit_clock(&mut caps, &said);
        assert!(!shifts.is_empty());
        assert!(shifts.iter().any(|s| (s + 6.0).abs() < 1.0) && shifts.iter().any(|s| (s + 46.0).abs() < 1.0), "{shifts:?}");
        let off: Vec<f64> = caps.iter().zip(&said).map(|(c, s)| (c.t - s.1).abs()).collect();
        assert!(off.iter().filter(|d| **d < 1.0).count() >= 280, "most words back in place");
        // Nothing to go by: nothing moves
        let mut few = vec![CapWord { word: "так".into(), t: 5.0, who: "Іван".into(), slack: 0.0, block: 0 }];
        assert!(fit_clock(&mut few, &[("ні".into(), 1.0)]).is_empty());
        assert_eq!(few[0].t, 5.0);
    }

    #[test]
    fn captions_spread_evenly_keep_their_order_when_moved() {
        // Twenty remarks of 30 words, 0.4 s apart, 2 s between remarks; Meet
        // first shows each 1.1 s after it starts and last changes it 1 to 5 s
        // after it ends, so evenly spread words come out late, by more or less
        let said = speech(600, 0.0);
        let mut timed = Vec::new();
        let mut caps = Vec::new();
        for (r, chunk) in said.chunks(30).enumerate() {
            let start = r as f64 * 14.0;
            let ws: Vec<(String, f64)> = chunk.iter().enumerate().map(|(i, (w, _))| (w.clone(), start + 0.4 * i as f64)).collect();
            let end = ws.last().unwrap().1;
            let text = ws.iter().map(|x| x.0.as_str()).collect::<Vec<_>>().join(" ");
            caps.push(cap(&format!("{r}"), if r % 2 == 0 { "Олена" } else { "Іван" }, &text, (start + 1.1 + SHOWN_LATE_S) * 1000.0, (end + 1.0 + (r * 7 % 5) as f64 + SHOWN_LATE_S) * 1000.0));
            timed.extend(ws);
        }
        let spread = words(&caps, 0.0);
        let mut w = spread.clone();
        let shifts = fit_clock(&mut w, &timed);
        assert!(!shifts.is_empty());
        // Each caption's words stay in their order
        for b in 0..caps.len() {
            let mine = |w: &[CapWord]| w.iter().filter(|c| c.block == b).map(|c| c.word.clone()).collect::<Vec<_>>();
            assert_eq!(mine(&w), mine(&spread), "caption {b}");
        }
        // In time order they read at least as close to what was said
        let said_words: Vec<String> = timed.iter().map(|x| x.0.clone()).collect();
        let errors = |w: &[CapWord]| {
            let a = align(&said_words, &w.iter().map(|c| c.word.clone()).collect::<Vec<_>>());
            a.substituted + a.deleted + a.inserted
        };
        assert!(errors(&w) <= errors(&spread), "{} vs {}", errors(&w), errors(&spread));
    }

    #[test]
    fn a_long_caption_spread_evenly_is_still_matched() {
        // A 60 s remark kept without its growth; the speaker paused in the
        // middle, so the even spread puts the second half's words too early
        let mut said = speech(60, 0.0);
        said.extend(speech(60, 36.0).into_iter().map(|(w, t)| (format!("{w}х"), t)));
        let text = said.iter().map(|x| x.0.as_str()).collect::<Vec<_>>().join(" ");
        let caps = words(&[cap("a", "Олена", &text, 800.0, 60_400.0)], 0.0);
        // Whisper's pieces of 10 words, heard right
        let pieces: Vec<PieceWords> = said.chunks(10).map(|c| piece(c[0].1, &c.iter().map(|(w, t)| (w.as_str(), *t)).collect::<Vec<_>>())).collect();
        let same = |c: &Comparison| c.said.iter().flatten().filter(|s| **s == Some(Said::Same)).count();
        let before = compare_as_before(&pieces, &caps);
        let now = compare(&pieces, &caps);
        assert!(same(&now) >= 115, "fitted: {} of 120 the same", same(&now));
        assert!(same(&before) < same(&now), "as before: {}", same(&before));
    }

    /// A recorded call to look into by hand: CAPTIONS_DEBUG = a JSON file
    /// {"created_at": ms, "captions": [[key, who, text, t0, t1], …],
    /// "pieces": [[start, end, [[word, t], …]], …]}
    #[test]
    #[ignore]
    fn debug_a_recorded_call() {
        let Ok(path) = std::env::var("CAPTIONS_DEBUG") else { return };
        let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let start = v["created_at"].as_f64().unwrap();
        let caps: Vec<Caption> = v["captions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| Caption {
                key: c[0].as_str().unwrap_or("").into(),
                who: c[1].as_str().unwrap_or("").into(),
                text: c[2].as_str().unwrap_or("").into(),
                t0: c[3].as_f64().unwrap(),
                t1: c[4].as_f64().unwrap(),
                grew: c.get(5).and_then(|g| serde_json::from_value(g.clone()).ok()).unwrap_or_default(),
            })
            .collect();
        let pieces: Vec<PieceWords> = v["pieces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| PieceWords {
                start: p[0].as_f64().unwrap(),
                end: p[1].as_f64().unwrap(),
                words: p[2].as_array().unwrap().iter().map(|w| (w[0].as_str().unwrap().to_string(), w[1].as_f64().unwrap())).collect(),
                ..Default::default()
            })
            .collect();
        let raw = words(&caps, start);
        let heard: Vec<(String, f64)> = pieces.iter().flat_map(|p| p.words.iter().map(|(w, t)| (norm(w), *t))).collect();
        // Unique longer words in both: how far apart
        let count = |w: &str, list: &mut dyn Iterator<Item = String>| list.filter(|x| x == w).count();
        let mut ds: Vec<f64> = raw
            .iter()
            .filter(|c| c.word.chars().count() >= CLOCK_MIN_LETTERS)
            .filter(|c| count(&c.word, &mut raw.iter().map(|x| x.word.clone())) == 1 && count(&c.word, &mut heard.iter().map(|x| x.0.clone())) == 1)
            .filter_map(|c| heard.iter().find(|h| h.0 == c.word).map(|h| h.1 - c.t))
            .collect();
        ds.sort_by(|a, b| a.total_cmp(b));
        println!("unique pairs {}: median {:.2}, quartiles {:.2} {:.2}", ds.len(), ds.get(ds.len() / 2).copied().unwrap_or(0.0), ds.get(ds.len() / 4).copied().unwrap_or(0.0), ds.get(3 * ds.len() / 4).copied().unwrap_or(0.0));
        let mut fitted = raw.clone();
        let shifts = fit_clock(&mut fitted, &heard);
        println!("shifts {shifts:?}");
        for (name, caps) in [("as kept", &raw), ("moved", &fitted)] {
            for (how, c) in [("before", compare_as_before(&pieces, caps)), ("fitted", compare(&pieces, caps))] {
                let all: Vec<Said> = c.said.iter().flatten().flatten().copied().collect();
                let n = |s: Said| all.iter().filter(|x| **x == s).count();
                println!("{name} / {how}: covered {} same {} alike {} other {} nothing {}", all.len(), n(Said::Same), n(Said::Alike), n(Said::Other), n(Said::Nothing));
            }
        }
    }

    #[test]
    fn without_captions_nothing_is_said() {
        let pieces = vec![piece(0.0, &[("так", 0.5)])];
        let c = compare(&pieces, &[]);
        assert_eq!(c.said, vec![vec![None]]);
        assert!(c.missed.is_empty());
    }
}
