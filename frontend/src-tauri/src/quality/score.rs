//! What recognition got wrong in a piece, against the secretary's text, and
//! how well the underlined words point to those mistakes.
//!
//! Words are compared without case and the punctuation around them. A
//! mistake is a wrong word, an extra word or a missing one. A missing word
//! counts as pointed to when a word next to its place is underlined.
//!
//! The four numbers of the roadmap:
//! - WER: mistakes per word of the right text;
//! - recall: share of mistakes the underlines point to;
//! - mistakes in the trusted text: wrong words among those not underlined
//!   (plus missing words with no underline next to them), per such word;
//! - underlines per hour of recording.

use serde::{Deserialize, Serialize};

/// A word as compared: lower case, no punctuation around it, one apostrophe
pub fn norm(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’' && c != 'ʼ' && c != '-')
        .trim_matches(|c: char| c == '-')
        .replace(['’', 'ʼ', '`'], "'")
        .to_lowercase()
}

/// Short function words: an underline on them is not put in the «Перевірте»
/// queue unless recognition was really unsure (as `needsReview` in the
/// transcript screen)
const FUNCTION_WORDS: [&str; 17] = ["й", "і", "у", "в", "з", "а", "та", "чи", "же", "ж", "би", "б", "не", "ні", "що", "як", "це"];
const VERY_UNSURE: f32 = 0.2;

/// An underlined word the secretary is asked to check
pub fn queued(word: &str, probability: f32) -> bool {
    !FUNCTION_WORDS.contains(&norm(word).as_str()) || probability < VERY_UNSURE
}

/// How recognised words line up with the right ones
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Alignment {
    /// Words of the right text
    pub reference: usize,
    pub substituted: usize,
    pub deleted: usize,
    pub inserted: usize,
    /// For each recognised word: whether it is right
    pub correct: Vec<bool>,
    /// Missing words, by the place among the recognised words they belong
    /// before (0..=recognised)
    pub missing_at: Vec<usize>,
    /// Wrong words: (place in the right text, place among the recognised words)
    pub wrong: Vec<(usize, usize)>,
    /// Missing words by their place in the right text
    pub missed: Vec<usize>,
}

/// Letters two words share at the start
fn shared_start(a: &str, b: &str) -> i32 {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count() as i32
}

/// Fewest edits from `hyp` to `reference` (Levenshtein), with what each
/// recognised word turned out to be. Among equally short ways the one that
/// keeps the most words right is taken, so a missing word is not counted as
/// a run of wrong ones; then the one that pairs similar words as wrong ones
/// (`доповів` heard as `доповідав`, not as the next word).
pub fn align(reference: &[String], hyp: &[String]) -> Alignment {
    let (n, m) = (reference.len(), hyp.len());
    // (edits, −right words, −letters shared by wrong pairs): smaller is better
    let mut d = vec![vec![(0u32, 0i32, 0i32); m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = (i as u32, 0, 0);
    }
    for j in 0..=m {
        d[0][j] = (j as u32, 0, 0);
    }
    let diag = |d: &Vec<Vec<(u32, i32, i32)>>, i: usize, j: usize| {
        let same = reference[i - 1] == hyp[j - 1];
        let (e, r, s) = d[i - 1][j - 1];
        if same { (e, r - 1, s) } else { (e + 1, r, s - shared_start(&reference[i - 1], &hyp[j - 1])) }
    };
    let step = |c: (u32, i32, i32)| (c.0 + 1, c.1, c.2);
    for i in 1..=n {
        for j in 1..=m {
            let up = step(d[i - 1][j]);
            let left = step(d[i][j - 1]);
            d[i][j] = diag(&d, i, j).min(up).min(left);
        }
    }
    let mut a = Alignment { reference: n, correct: vec![false; m], ..Default::default() };
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && d[i][j] == diag(&d, i, j) {
            if reference[i - 1] == hyp[j - 1] {
                a.correct[j - 1] = true;
            } else {
                a.substituted += 1;
                a.wrong.push((i - 1, j - 1));
            }
            i -= 1;
            j -= 1;
        } else if j > 0 && d[i][j] == step(d[i][j - 1]) {
            a.inserted += 1;
            j -= 1;
        } else {
            a.deleted += 1;
            a.missing_at.push(j);
            a.missed.push(i - 1);
            i -= 1;
        }
    }
    a.missing_at.reverse();
    a.wrong.reverse();
    a.missed.reverse();
    a
}

/// Counts over any number of pieces
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tally {
    pub pieces: usize,
    pub seconds: f64,
    pub reference_words: usize,
    pub recognised_words: usize,
    pub substituted: usize,
    pub deleted: usize,
    pub inserted: usize,
    /// Mistakes (wrong, extra, missing words) and how many are underlined
    pub mistakes: usize,
    pub caught: usize,
    /// Underlined words, and those of them that were right after all
    pub flagged: usize,
    pub flagged_right: usize,
    /// Words not underlined, and the mistakes among them (missing words with
    /// no underline next to them included)
    pub trusted: usize,
    pub trusted_mistakes: usize,
}

impl Tally {
    /// One piece: the right text, the recognised words and which of them are underlined
    pub fn add(&mut self, seconds: f64, reference: &str, recognised: &[String], flagged: &[bool]) {
        // Words with nothing left after normalising (a lone dash) are not compared
        let r: Vec<String> = reference.split_whitespace().map(norm).filter(|w| !w.is_empty()).collect();
        let kept: Vec<usize> = (0..recognised.len()).filter(|&i| !norm(&recognised[i]).is_empty()).collect();
        let h: Vec<String> = kept.iter().map(|&i| norm(&recognised[i])).collect();
        let f: Vec<bool> = kept.iter().map(|&i| flagged.get(i).copied().unwrap_or(false)).collect();
        let a = align(&r, &h);
        self.pieces += 1;
        self.seconds += seconds;
        self.reference_words += a.reference;
        self.recognised_words += h.len();
        self.substituted += a.substituted;
        self.deleted += a.deleted;
        self.inserted += a.inserted;
        for (k, right) in a.correct.iter().enumerate() {
            if f[k] {
                self.flagged += 1;
                self.flagged_right += *right as usize;
            } else {
                self.trusted += 1;
            }
            if !right {
                self.mistakes += 1;
                if f[k] {
                    self.caught += 1;
                } else {
                    self.trusted_mistakes += 1;
                }
            }
        }
        for &at in &a.missing_at {
            self.mistakes += 1;
            let near = (at > 0 && f[at - 1]) || f.get(at).copied().unwrap_or(false);
            if near {
                self.caught += 1;
            } else {
                self.trusted_mistakes += 1;
            }
        }
    }

    pub fn merge(&mut self, o: &Tally) {
        self.pieces += o.pieces;
        self.seconds += o.seconds;
        self.reference_words += o.reference_words;
        self.recognised_words += o.recognised_words;
        self.substituted += o.substituted;
        self.deleted += o.deleted;
        self.inserted += o.inserted;
        self.mistakes += o.mistakes;
        self.caught += o.caught;
        self.flagged += o.flagged;
        self.flagged_right += o.flagged_right;
        self.trusted += o.trusted;
        self.trusted_mistakes += o.trusted_mistakes;
    }

    pub fn wer(&self) -> f64 {
        (self.substituted + self.deleted + self.inserted) as f64 / self.reference_words.max(1) as f64
    }

    pub fn recall(&self) -> f64 {
        self.caught as f64 / self.mistakes.max(1) as f64
    }

    pub fn trusted_error(&self) -> f64 {
        self.trusted_mistakes as f64 / self.trusted.max(1) as f64
    }

    pub fn flags_per_hour(&self) -> f64 {
        self.flagged as f64 / (self.seconds / 3600.0).max(1e-9)
    }

    /// One line for a report
    pub fn summary(&self) -> String {
        format!(
            "{:.2} h, {} pieces, {} words · WER {:.1}% (wrong {}, missing {}, extra {}) · underlines catch {:.0}% of {} mistakes · mistakes in trusted text {:.2}% · underlines {:.0}/h ({:.0}% of them were right)",
            self.seconds / 3600.0,
            self.pieces,
            self.reference_words,
            100.0 * self.wer(),
            self.substituted,
            self.deleted,
            self.inserted,
            100.0 * self.recall(),
            self.mistakes,
            100.0 * self.trusted_error(),
            self.flags_per_hour(),
            100.0 * self.flagged_right as f64 / self.flagged.max(1) as f64,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn norm_ignores_case_punctuation_and_apostrophe_forms() {
        assert_eq!(norm("«Пам’ять,»"), "пам'ять");
        assert_eq!(norm("—"), "");
        assert_eq!(norm("Вузол-2."), "вузол-2");
    }

    #[test]
    fn align_counts_each_kind_of_mistake() {
        let r: Vec<String> = words("доповідає керівник першого відділу").iter().map(|w| norm(w)).collect();
        let h: Vec<String> = words("доповідав керівник відділу зараз").iter().map(|w| norm(w)).collect();
        let a = align(&r, &h);
        assert_eq!(a.substituted + a.deleted + a.inserted, 3);
        assert_eq!(a.correct, vec![false, true, true, false]);
        assert_eq!(a.deleted, 1);
        assert_eq!(a.missing_at, vec![2]);
        assert_eq!(a.missed, vec![2], "«першого» is missing");
        assert_eq!(a.wrong, vec![(0, 0)], "«доповідає» heard as «доповідав»");
    }

    #[test]
    fn tally_measures_underlines_against_mistakes() {
        let mut t = Tally::default();
        // «Гончаренко» heard wrong and underlined; «доповів» wrong and not underlined
        t.add(10.0, "Гончаренко доповів про стан", &words("Кончаренко доповідав про стан"), &[true, false, false, false]);
        assert_eq!(t.mistakes, 2);
        assert_eq!(t.caught, 1);
        assert_eq!(t.flagged, 1);
        assert_eq!(t.flagged_right, 0);
        assert_eq!(t.trusted, 3);
        assert_eq!(t.trusted_mistakes, 1);
        assert!((t.wer() - 0.5).abs() < 1e-9);
        assert!((t.recall() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn a_missing_word_next_to_an_underline_counts_as_caught() {
        let mut t = Tally::default();
        t.add(5.0, "три тисячі вісімнадцять", &words("три вісімнадцять"), &[true, false]);
        assert_eq!(t.deleted, 1);
        assert_eq!(t.caught, 1);
        assert_eq!(t.trusted_mistakes, 0);
    }
}
