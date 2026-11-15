//! Dictionary terms for one piece of a recording. Whisper's prompt holds only
//! a few dozen terms, so the most used ones always win and the rest of a
//! long dictionary never reaches the decoder. Here the terms are picked by
//! what was heard in the piece on a first pass (by Whisper and Parakeet):
//! a term that sounds like a heard word goes first in the prompt for a
//! second pass.

use crate::audio::transcription::ensemble::sound_key;

/// How alike two sound keys are (1: the same)
fn alike(a: &str, b: &str) -> f32 {
    let (la, lb) = (a.chars().count(), b.chars().count());
    if la == 0 || lb == 0 {
        return 0.0;
    }
    let d = super::corrections::levenshtein(a, b);
    1.0 - d as f32 / la.max(lb) as f32
}

/// The same word in another form: a long enough common beginning
/// («Гончаренко/Гончаренка», «вузла/вузол»)
fn same_stem(a: &str, b: &str) -> bool {
    let common = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
    let shorter = a.chars().count().min(b.chars().count());
    shorter >= 4 && common >= 4 && common * 10 >= shorter * 6
}

/// Terms of the dictionary that sound like a word heard in the piece, the
/// closest first (at most `limit`). Short words (under 4 letters) are not
/// matched: too many terms sound like them.
pub fn terms_heard(terms: &[String], heard: &[String], limit: usize) -> Vec<String> {
    let keys: Vec<String> = heard.iter().map(|w| sound_key(w)).filter(|k| k.chars().count() >= 4).collect();
    if keys.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(f32, usize)> = Vec::new();
    for (n, term) in terms.iter().enumerate() {
        let best = term
            .split_whitespace()
            .map(sound_key)
            .filter(|k| k.chars().count() >= 4)
            .flat_map(|tk| keys.iter().map(move |h| if same_stem(&tk, h) { 1.0 } else { alike(&tk, h) }))
            .fold(0.0f32, f32::max);
        if best >= 0.75 {
            scored.push((best, n));
        }
    }
    // Closest first; among equals the dictionary's own order (most used first)
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, n)| terms[n].clone()).collect()
}

/// Terms Whisper did not already write as they are go first: those are the
/// ones the second pass can put right (a term heard right needs no help)
pub fn missing_first(picked: Vec<String>, whisper: &[String]) -> Vec<String> {
    let have: std::collections::HashSet<String> = whisper.iter().map(|w| sound_key(w)).collect();
    let written = |t: &String| t.split_whitespace().all(|w| have.contains(&sound_key(w)));
    let (done, missing): (Vec<String>, Vec<String>) = picked.into_iter().partition(written);
    missing.into_iter().chain(done).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_term_heard_slightly_wrong_or_in_another_form_is_picked() {
        let terms = s(&["Мельник", "Гончаренко", "Вузол-2", "Північний сектор", "логістика"]);
        let heard = s(&["доповідав", "Кончаренко,", "про", "стан", "північного"]);
        let got = terms_heard(&terms, &heard, 10);
        assert!(got.contains(&"Гончаренко".to_string()), "{got:?}");
        assert!(got.contains(&"Північний сектор".to_string()), "{got:?}");
        assert!(!got.contains(&"Мельник".to_string()), "{got:?}");
        assert!(!got.contains(&"логістика".to_string()), "{got:?}");
    }

    #[test]
    fn terms_whisper_already_wrote_go_last() {
        let got = missing_first(s(&["Гончаренко", "Мельник"]), &s(&["Мельник", "доповів"]));
        assert_eq!(got, s(&["Гончаренко", "Мельник"]));
        let got = missing_first(s(&["Мельник", "Гончаренко"]), &s(&["Мельник,"]));
        assert_eq!(got, s(&["Гончаренко", "Мельник"]));
    }

    #[test]
    fn short_words_pick_nothing() {
        assert!(terms_heard(&s(&["Буг"]), &s(&["бук", "і", "та"]), 10).is_empty());
    }
}
