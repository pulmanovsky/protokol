//! Whether a newly trained model replaces the one in use.
//!
//! Both models hear the same held-out meetings (checked by the user, never
//! trained on) with the same prompt and no learned corrections. The new
//! model is taken only when, over all those meetings, it makes at least
//! [`MIN_GAIN`] fewer mistakes per 100 words, no single meeting gets more
//! than [`MAX_LOSS`] worse, and there are enough words to tell.
//!
//! Lower training loss alone is not enough: in the pilot a model with lower
//! loss made more mistakes (v1, 14.11).

use serde::{Deserialize, Serialize};

/// Points (mistakes per 100 words) the new model must gain overall
pub const MIN_GAIN: f64 = 0.5;
/// Points any one held-out meeting may lose
pub const MAX_LOSS: f64 = 1.0;
/// Fewer words than this say too little (about 10 minutes of speech)
pub const MIN_WORDS: usize = 1_000;

/// Mistakes on one held-out meeting
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Score {
    /// Words of the checked text
    pub words: usize,
    /// Wrong, extra and missing words
    pub mistakes: usize,
}

impl Score {
    pub fn wer(&self) -> f64 {
        100.0 * self.mistakes as f64 / self.words.max(1) as f64
    }
}

fn pooled(scores: &[Score]) -> f64 {
    let words: usize = scores.iter().map(|s| s.words).sum();
    let mistakes: usize = scores.iter().map(|s| s.mistakes).sum();
    100.0 * mistakes as f64 / words.max(1) as f64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Better,
    NotBetter,
    OneMeetingWorse,
    TooLittleToTell,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    pub accept: bool,
    pub reason: Reason,
    /// Mistakes per 100 words over all held-out meetings
    pub before: f64,
    pub after: f64,
    /// Per meeting (before, after), in the order given
    pub per_meeting: Vec<(f64, f64)>,
}

/// `current` and `candidate`: the same meetings in the same order
pub fn decide(current: &[Score], candidate: &[Score]) -> Verdict {
    let (before, after) = (pooled(current), pooled(candidate));
    let per_meeting: Vec<(f64, f64)> = current.iter().zip(candidate).map(|(c, n)| (c.wer(), n.wer())).collect();
    let words: usize = current.iter().map(|s| s.words).sum();
    let same_meetings = current.len() == candidate.len() && current.iter().zip(candidate).all(|(c, n)| c.words == n.words);
    let reason = if !same_meetings || current.is_empty() || words < MIN_WORDS {
        Reason::TooLittleToTell
    } else if before - after < MIN_GAIN - 1e-9 {
        Reason::NotBetter
    } else if per_meeting.iter().any(|(b, a)| a - b > MAX_LOSS + 1e-9) {
        Reason::OneMeetingWorse
    } else {
        Reason::Better
    };
    Verdict { accept: reason == Reason::Better, reason, before, after, per_meeting }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(words: usize, mistakes: usize) -> Score {
        Score { words, mistakes }
    }

    #[test]
    fn the_pilot_result_is_accepted() {
        // v5 on the three meetings of 14.11 (words per meeting approximate)
        let v = decide(&[s(1661, 269), s(1661, 292), s(1662, 305)], &[s(1661, 236), s(1661, 269), s(1662, 279)]);
        assert!(v.accept, "{v:?}");
        assert!((v.before - 17.33).abs() < 0.1 && (v.after - 15.67).abs() < 0.1, "{v:?}");
    }

    #[test]
    fn a_small_gain_is_not_enough() {
        let v = decide(&[s(5000, 875)], &[s(5000, 860)]);
        assert_eq!(v.reason, Reason::NotBetter);
        assert!(!v.accept);
    }

    #[test]
    fn one_meeting_much_worse_blocks_it() {
        let v = decide(&[s(3000, 600), s(1000, 150)], &[s(3000, 480), s(1000, 175)]);
        assert_eq!(v.reason, Reason::OneMeetingWorse, "{v:?}");
    }

    #[test]
    fn too_few_words_or_different_meetings_tell_nothing() {
        assert_eq!(decide(&[s(400, 80)], &[s(400, 40)]).reason, Reason::TooLittleToTell);
        assert_eq!(decide(&[s(3000, 600)], &[s(2900, 400)]).reason, Reason::TooLittleToTell);
        assert_eq!(decide(&[], &[]).reason, Reason::TooLittleToTell);
    }
}
