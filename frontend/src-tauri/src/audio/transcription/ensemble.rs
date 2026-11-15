//! Two recognisers on the same piece of a recording. Where Whisper and
//! Parakeet hear the same word, the word can be trusted; where they hear
//! different words, or one of them hears a word the other does not, the
//! place is worth checking.
//!
//! Words are compared by how they sound rather than how they are spelt:
//! Parakeet sometimes writes a surzhyk word in Russian letters, so «и/і/ы»,
//! «е/є/э/ё» and «г/ґ» are taken as the same letter and the soft sign and
//! the apostrophe are dropped.
//!
//! Where they differ, a third model can say which version fits the sound
//! better: the character model of the word aligner scores both texts over
//! the same frames ([`judge`]).
//!
//! For now this is only measured (`eval_pipeline`, stages `replay` and
//! `reference` with `PROTOKOL_EVAL_PARAKEET=1`); the transcript screen does
//! not use it yet.

use serde::{Deserialize, Serialize};

use crate::quality::score::norm;

/// A word as it sounds: [`norm`], then letters that sound alike merged
pub fn sound_key(word: &str) -> String {
    let mut out = String::new();
    for c in norm(word).chars() {
        let c = match c {
            'і' | 'ї' | 'ы' => 'и',
            'є' | 'э' | 'ё' => 'е',
            'ґ' => 'г',
            '\'' | 'ь' | 'ъ' | '-' => continue,
            c => c,
        };
        // A doubled letter sounds like one («учасник/участник» aside)
        if out.ends_with(c) {
            continue;
        }
        out.push(c);
    }
    out
}

/// One step of lining up two lists of words
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The same word in both
    Same(usize, usize),
    /// Different words at the same place
    Differ(usize, usize),
    /// A word only the first list has
    OnlyFirst(usize),
    /// A word only the second list has
    OnlySecond(usize),
}

/// Lines up two lists of keys with the fewest edits; among equally short ways
/// the one with the most words in common is taken
pub fn line_up(a: &[String], b: &[String]) -> Vec<Step> {
    let (n, m) = (a.len(), b.len());
    // (edits, −words in common): smaller is better
    let mut d = vec![vec![(0u32, 0i32); m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = (i as u32, 0);
    }
    for j in 0..=m {
        d[0][j] = (j as u32, 0);
    }
    let diag = |d: &Vec<Vec<(u32, i32)>>, i: usize, j: usize| {
        let (e, r) = d[i - 1][j - 1];
        if a[i - 1] == b[j - 1] { (e, r - 1) } else { (e + 1, r) }
    };
    for i in 1..=n {
        for j in 1..=m {
            let up = (d[i - 1][j].0 + 1, d[i - 1][j].1);
            let left = (d[i][j - 1].0 + 1, d[i][j - 1].1);
            d[i][j] = diag(&d, i, j).min(up).min(left);
        }
    }
    let mut steps = Vec::with_capacity(n.max(m));
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && d[i][j] == diag(&d, i, j) {
            steps.push(if a[i - 1] == b[j - 1] { Step::Same(i - 1, j - 1) } else { Step::Differ(i - 1, j - 1) });
            i -= 1;
            j -= 1;
        } else if i > 0 && d[i][j] == (d[i - 1][j].0 + 1, d[i - 1][j].1) {
            steps.push(Step::OnlyFirst(i - 1));
            i -= 1;
        } else {
            steps.push(Step::OnlySecond(j - 1));
            j -= 1;
        }
    }
    steps.reverse();
    steps
}

/// What the second recogniser heard at each word of the first
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Compared {
    /// For each word of the first: the second's word at its place, if any
    pub other: Vec<Option<String>>,
    /// For each word of the first: the second heard the same word
    pub same: Vec<bool>,
    /// For each word of the first: how sure the second was of its word there
    /// (0 when it has none)
    pub other_prob: Vec<f32>,
    /// Words only the second heard, by the place among the first's words
    /// they belong before (0..=len), with how sure it was of each
    pub only_other: Vec<(usize, String, f32)>,
}

impl Compared {
    /// Share of the first's words the second heard the same
    pub fn agreement(&self) -> f64 {
        self.same.iter().filter(|s| **s).count() as f64 / self.same.len().max(1) as f64
    }
}

/// Compares the words of the first recogniser (Whisper) with those of the
/// second (Parakeet: word, time, how sure)
pub fn compare(first: &[String], second: &[(String, f64, f32)]) -> Compared {
    // Words with nothing left to compare (a lone dash) are skipped
    let keep_a: Vec<usize> = (0..first.len()).filter(|&i| !sound_key(&first[i]).is_empty()).collect();
    let keep_b: Vec<usize> = (0..second.len()).filter(|&j| !sound_key(&second[j].0).is_empty()).collect();
    let a: Vec<String> = keep_a.iter().map(|&i| sound_key(&first[i])).collect();
    let b: Vec<String> = keep_b.iter().map(|&j| sound_key(&second[j].0)).collect();
    let mut c = Compared { other: vec![None; first.len()], same: vec![false; first.len()], other_prob: vec![0.0; first.len()], only_other: Vec::new() };
    // A skipped word of the first counts as heard the same (nothing to check)
    for i in 0..first.len() {
        if !keep_a.contains(&i) {
            c.same[i] = true;
            c.other_prob[i] = 1.0;
        }
    }
    let mut next_a = 0; // place among the first's words for a word only the second has
    for step in line_up(&a, &b) {
        match step {
            Step::Same(x, y) | Step::Differ(x, y) => {
                let (i, j) = (keep_a[x], keep_b[y]);
                c.other[i] = Some(second[j].0.clone());
                c.same[i] = matches!(step, Step::Same(..));
                c.other_prob[i] = second[j].2;
                next_a = i + 1;
            }
            Step::OnlyFirst(x) => next_a = keep_a[x] + 1,
            Step::OnlySecond(y) => c.only_other.push((next_a, second[keep_b[y]].0.clone(), second[keep_b[y]].2)),
        }
    }
    c
}

/// A place where the two recognisers heard different things
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dispute {
    /// The first's words there (indices; empty when only the second heard
    /// something)
    pub first: Vec<usize>,
    /// Where the second's words go among the first's: the index of the
    /// first's word that follows them (its first word there, if any)
    pub at: usize,
    /// The second's words there (empty when only the first heard something)
    pub second: Vec<String>,
    /// How much better the second's version fits the sound than the first's
    /// (log-likelihood, nats; above 0: the second's); None until judged
    #[serde(default)]
    pub gain: Option<f32>,
}

/// The places where the two differ: runs of words not heard the same
pub fn disputes(first: &[String], second: &[(String, f64, f32)]) -> Vec<Dispute> {
    let keep_a: Vec<usize> = (0..first.len()).filter(|&i| !sound_key(&first[i]).is_empty()).collect();
    let keep_b: Vec<usize> = (0..second.len()).filter(|&j| !sound_key(&second[j].0).is_empty()).collect();
    let a: Vec<String> = keep_a.iter().map(|&i| sound_key(&first[i])).collect();
    let b: Vec<String> = keep_b.iter().map(|&j| sound_key(&second[j].0)).collect();
    let mut out: Vec<Dispute> = Vec::new();
    let mut open: Option<Dispute> = None;
    let mut next_a = 0;
    for step in line_up(&a, &b) {
        if let Step::Same(x, _) = step {
            out.extend(open.take());
            next_a = keep_a[x] + 1;
            continue;
        }
        let d = open.get_or_insert_with(|| Dispute { first: Vec::new(), at: next_a, second: Vec::new(), gain: None });
        match step {
            Step::Differ(x, y) => {
                d.first.push(keep_a[x]);
                d.second.push(second[keep_b[y]].0.clone());
                next_a = keep_a[x] + 1;
            }
            Step::OnlyFirst(x) => {
                d.first.push(keep_a[x]);
                next_a = keep_a[x] + 1;
            }
            Step::OnlySecond(y) => d.second.push(second[keep_b[y]].0.clone()),
            Step::Same(..) => {}
        }
        if let Some(&f) = d.first.first() {
            d.at = f;
        }
    }
    out.extend(open);
    out
}

/// The first's words with one place taken as the second heard it
pub fn alternative(first: &[String], d: &Dispute) -> Vec<String> {
    let mut alt = Vec::with_capacity(first.len() + d.second.len());
    for i in 0..=first.len() {
        if i == d.at {
            alt.extend(d.second.iter().cloned());
        }
        if i < first.len() && !d.first.contains(&i) {
            alt.push(first[i].clone());
        }
    }
    alt
}

/// Scores each place: the whole piece's text as the first heard it and with
/// that place as the second heard it, both over the same sound
pub fn judge(aligner: &mut crate::speakers::aligner::Aligner, samples: &[f32], first: &[String], disputes: &mut [Dispute]) -> anyhow::Result<()> {
    use crate::speakers::aligner::ctc_log_likelihood;
    if disputes.is_empty() {
        return Ok(());
    }
    let frames = aligner.frames(samples)?;
    let blank = aligner.blank();
    let base = ctc_log_likelihood(&frames, &aligner.label(first), blank);
    for d in disputes.iter_mut() {
        // A place with a number or a Latin word is left unjudged: the
        // model cannot spell either, so one version would only look shorter
        let words = d.first.iter().map(|&i| first[i].as_str()).chain(d.second.iter().map(String::as_str));
        if words.into_iter().any(|w| w.chars().any(char::is_alphanumeric) && !aligner.knows(w)) {
            continue;
        }
        let other = ctc_log_likelihood(&frames, &aligner.label(&alternative(first, d)), blank);
        d.gain = match (base.is_finite(), other.is_finite()) {
            (true, true) => Some(other - base),
            (true, false) => Some(-1000.0),
            (false, true) => Some(1000.0),
            (false, false) => None,
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(s: &str) -> Vec<(String, f64, f32)> {
        s.split_whitespace().enumerate().map(|(i, w)| (w.to_string(), i as f64 * 0.4, 0.9)).collect()
    }

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn sound_key_merges_letters_that_sound_alike() {
        assert_eq!(sound_key("Сьогодні,"), sound_key("сьогодни"));
        assert_eq!(sound_key("підрозділ"), sound_key("пидроздил"));
        assert_eq!(sound_key("пам’ять"), sound_key("памят"));
        assert_eq!(sound_key("ґанок"), sound_key("ганок"));
        assert_eq!(sound_key("група"), sound_key("группа"));
        assert_ne!(sound_key("рота"), sound_key("робота"));
    }

    #[test]
    fn compare_marks_same_different_and_missing_words() {
        let c = compare(&words("Доповідає керівник відділу зараз"), &heard("доповідає керівник першого відділу тепер"));
        assert_eq!(c.same, vec![true, true, true, false]);
        assert_eq!(c.other[3].as_deref(), Some("тепер"));
        assert_eq!(c.only_other, vec![(2, "першого".to_string(), 0.9)]);
        assert!((c.agreement() - 0.75).abs() < 1e-9);
    }

    #[test]
    fn disputes_group_places_and_alternatives_swap_them_in() {
        let first = words("Доповідає керівник відділу зараз");
        let d = disputes(&first, &heard("доповідає керівник першого відділу тепер вже"));
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].first.clone(), d[0].at, d[0].second.clone()), (vec![], 2, vec!["першого".to_string()]));
        assert_eq!((d[1].first.clone(), d[1].at), (vec![3], 3));
        assert_eq!(d[1].second, vec!["тепер".to_string(), "вже".to_string()]);
        assert_eq!(alternative(&first, &d[0]).join(" "), "Доповідає керівник першого відділу зараз");
        assert_eq!(alternative(&first, &d[1]).join(" "), "Доповідає керівник відділу тепер вже");
        // A word only the first heard, at the end
        let d = disputes(&words("так ні зараз"), &heard("так ні"));
        assert_eq!((d[0].first.clone(), d[0].at, d[0].second.len()), (vec![2], 2, 0));
        assert_eq!(alternative(&words("так ні зараз"), &d[0]).join(" "), "так ні");
    }

    #[test]
    fn a_lone_dash_is_not_a_disagreement() {
        let c = compare(&words("так — ні"), &heard("так ні"));
        assert_eq!(c.same, vec![true, true, true]);
        assert!(c.only_other.is_empty());
    }

    #[test]
    fn nothing_from_the_second_leaves_every_word_unconfirmed() {
        let c = compare(&words("три тисячі"), &[]);
        assert_eq!(c.same, vec![false, false]);
        assert_eq!(c.other, vec![None, None]);
    }
}
