//! Google Meet's own captions as a second text of the call.
//!
//! In Protokol's Meet window capture.js turns Meet's captions on by itself,
//! in Ukrainian and out of sight (unless the person turns them on or off
//! themselves), reads them off the page (who, what, when first and last seen)
//! and they come with its reports. Meet keeps rewriting a caption while the person speaks, so only
//! the last version of each is kept. They are Google's recognition, made
//! during the call; Protokol only compares its own transcript with them:
//! where both heard the same words the text is likely right, where they
//! differ a word is worth a look, and where Meet wrote words while Whisper
//! wrote nothing a remark may be missing.

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
        match captions.iter_mut().find(|x| x.key == key) {
            Some(x) if t1 >= x.t1 => {
                x.text = text.to_string();
                x.who = who;
                x.t1 = t1;
            }
            Some(_) => {}
            None => captions.push(Caption { key, who, text: text.to_string(), t0, t1 }),
        }
    }
}

/// How long after the words were said Meet shows them (s): a caption is
/// first seen a little after its first word and last changed a little
/// after its last
const SHOWN_LATE_S: f64 = 0.8;
/// Caption words this far before or after a piece are compared with it (s)
const NEAR_S: f64 = 4.0;
/// Caption words with no Whisper words within this distance (s), this many
/// in a row, are a remark Whisper may have missed
const GAP_S: f64 = 1.5;
const MISSED_WORDS: usize = 3;

/// Caption words on the recording's clock (s from its start): each caption's
/// words spread evenly between when it was first and last seen
pub fn words(captions: &[Caption], start_ms: f64) -> Vec<(String, f64, String)> {
    let mut out = Vec::new();
    for c in captions {
        let ws: Vec<String> = c.text.split_whitespace().map(norm).filter(|w| !w.is_empty()).collect();
        if ws.is_empty() {
            continue;
        }
        let a = (c.t0 - start_ms) / 1000.0 - SHOWN_LATE_S;
        let b = ((c.t1 - start_ms) / 1000.0 - SHOWN_LATE_S).max(a);
        let n = ws.len();
        for (i, w) in ws.into_iter().enumerate() {
            let t = if n > 1 { a + (b - a) * i as f64 / (n - 1) as f64 } else { a };
            out.push((w, t, c.who.clone()));
        }
    }
    out.sort_by(|x, y| x.1.total_cmp(&y.1));
    out
}

/// What the captions say about the transcript
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Comparison {
    /// For each piece, each Whisper word: Some(true) the captions have it,
    /// Some(false) they have another word there, None no captions near
    pub agree: Vec<Vec<Option<bool>>>,
    /// Remarks only in the captions: (from, to in s, who, text)
    pub missed: Vec<(f64, f64, String, String)>,
}

/// Compares the transcript's pieces with the caption words (from `words`)
pub fn compare(pieces: &[PieceWords], caps: &[(String, f64, String)]) -> Comparison {
    let mut out = Comparison::default();
    let mut used = vec![false; caps.len()];
    for p in pieces {
        let heard: Vec<String> = p.words.iter().map(|(w, _)| norm(w)).collect();
        let near: Vec<usize> = (0..caps.len()).filter(|&i| caps[i].1 >= p.start - NEAR_S && caps[i].1 <= p.end + NEAR_S).collect();
        if near.is_empty() || heard.is_empty() {
            out.agree.push(vec![None; heard.len()]);
            continue;
        }
        let reference: Vec<String> = near.iter().map(|&i| caps[i].0.clone()).collect();
        // A number is one token on both sides, however written («8000» and «вісім тисяч»)
        let (heard_t, heard_spans) = numbers_as_one(&heard);
        let (ref_t, ref_spans) = numbers_as_one(&reference);
        let a = align(&ref_t, &heard_t);
        // Caption words the piece accounts for (matched or paired as wrong)
        let mut matched = vec![false; ref_t.len()];
        for (ri, _) in &a.wrong {
            matched[*ri] = true;
        }
        for ri in matched_pairs(&ref_t, &heard_t, &a.correct) {
            matched[ri] = true;
        }
        let mut row: Vec<Option<bool>> = vec![None; heard.len()];
        for (k, &ok) in a.correct.iter().enumerate() {
            for j in heard_spans[k].clone() {
                row[j] = Some(ok);
            }
        }
        for (k, span) in ref_spans.iter().enumerate() {
            if matched[k] {
                for idx in span.clone() {
                    used[near[idx]] = true;
                }
            }
        }
        out.agree.push(row);
    }
    // Runs of caption words no piece took, with no Whisper words nearby
    let whisper_times: Vec<f64> = pieces.iter().flat_map(|p| p.words.iter().map(|w| w.1)).collect();
    let quiet = |t: f64| !whisper_times.iter().any(|&w| (w - t).abs() <= GAP_S);
    let mut i = 0;
    while i < caps.len() {
        if used[i] || !quiet(caps[i].1) {
            i += 1;
            continue;
        }
        let start = i;
        while i < caps.len() && !used[i] && quiet(caps[i].1) && caps[i].2 == caps[start].2 {
            i += 1;
        }
        if i - start >= MISSED_WORDS {
            let text = caps[start..i].iter().map(|c| c.0.as_str()).collect::<Vec<_>>().join(" ");
            out.missed.push((caps[start].1, caps[i - 1].1, caps[start].2.clone(), text));
        }
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

    #[test]
    fn the_last_version_of_each_caption_is_kept() {
        let mut caps = Vec::new();
        add_report(&mut caps, &json!({"started": 5, "captions": [
            {"id": 0, "who": "Олена", "text": "Доброго", "t0": 1000, "t1": 1000},
            {"id": 0, "who": "Олена", "text": "Доброго ранку", "t0": 1000, "t1": 1500}]}));
        add_report(&mut caps, &json!({"started": 5, "captions": [
            {"id": 0, "who": "Олена", "text": "Доброго ранку, колеги.", "t0": 1000, "t1": 4000},
            {"id": 1, "who": "Іван", "text": "Дякую", "t0": 4200, "t1": 4300}]}));
        // Another page of the same window numbers its captions from 0 again
        add_report(&mut caps, &json!({"started": 9, "captions": [{"id": 0, "who": "Іван", "text": "Ще раз", "t0": 9000, "t1": 9100}]}));
        assert_eq!(caps.len(), 3);
        assert_eq!(caps[0].text, "Доброго ранку, колеги.");
        assert_eq!((caps[0].t0, caps[0].t1), (1000.0, 4000.0));
    }

    #[test]
    fn caption_words_go_on_the_recording_clock() {
        let caps = vec![Caption { key: "a".into(), who: "Олена".into(), text: "Доброго ранку, колеги.".into(), t0: 11_800.0, t1: 13_800.0 }];
        let w = words(&caps, 10_000.0);
        assert_eq!(w.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["доброго", "ранку", "колеги"]);
        assert!((w[0].1 - 1.0).abs() < 1e-9 && (w[2].1 - 3.0).abs() < 1e-9);
    }

    #[test]
    fn differing_words_and_a_missed_remark_are_found() {
        let caps = vec![
            Caption { key: "a".into(), who: "Олена".into(), text: "Склад заповнено на вісімдесят відсотків".into(), t0: 1_800.0, t1: 5_800.0 },
            Caption { key: "b".into(), who: "Іван".into(), text: "Зрозуміло, дякую за доповідь".into(), t0: 20_800.0, t1: 22_800.0 },
        ];
        let w = words(&caps, 0.0);
        // Whisper heard the first remark with one word wrong and wrote nothing for the second
        let pieces = vec![piece(1.0, &[("Склад", 1.0), ("заповнено", 2.0), ("на", 3.0), ("вісімнадцять", 3.5), ("відсотків", 4.5)])];
        let c = compare(&pieces, &w);
        assert_eq!(c.agree[0], vec![Some(true), Some(true), Some(true), Some(false), Some(true)]);
        assert_eq!(c.missed.len(), 1);
        assert_eq!(c.missed[0].2, "Іван");
        assert_eq!(c.missed[0].3, "зрозуміло дякую за доповідь");
    }

    #[test]
    fn a_number_in_digits_and_in_words_is_the_same() {
        let caps = vec![Caption { key: "a".into(), who: "Олена".into(), text: "Зібрали 8000 грн на ремонт".into(), t0: 1_800.0, t1: 4_800.0 }];
        let w = words(&caps, 0.0);
        let pieces = vec![piece(1.0, &[("Зібрали", 1.0), ("вісім", 1.6), ("тисяч", 2.0), ("гривень", 2.6), ("на", 3.2), ("ремонт", 3.6)])];
        let c = compare(&pieces, &w);
        // «грн» and «гривень» stay different words; the number agrees
        assert_eq!(c.agree[0], vec![Some(true), Some(true), Some(true), Some(false), Some(true), Some(true)]);
        let (t, spans) = numbers_as_one(&["вісім".into(), "тисяч".into(), "гривень".into()]);
        assert_eq!(t, ["#8000", "гривень"]);
        assert_eq!(spans, [0..2, 2..3]);
    }

    #[test]
    fn without_captions_nothing_is_said() {
        let pieces = vec![piece(0.0, &[("так", 0.5)])];
        let c = compare(&pieces, &[]);
        assert_eq!(c.agree, vec![vec![None]]);
        assert!(c.missed.is_empty());
    }
}
