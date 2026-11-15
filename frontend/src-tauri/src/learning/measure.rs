//! Mistakes a model makes on the held-out meetings: what [`super::gate`]
//! decides from.
//!
//! Each model is loaded into its own engine from a folder that holds just
//! that model under the original's file name, so the same discovery,
//! prompt and text clean-up as in daily recognition apply to both.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::dataset::PieceFile;
use super::gate::Score;
use crate::ukrainian::doubt::{self, Thresholds};

/// A folder where `filename` is the model at `model` (a link, no copy)
pub fn model_folder(work: &Path, name: &str, filename: &str, model: &Path) -> Result<PathBuf> {
    let dir = work.join(name);
    std::fs::create_dir_all(&dir)?;
    let link = dir.join(filename);
    let _ = std::fs::remove_file(&link);
    #[cfg(unix)]
    std::os::unix::fs::symlink(model, &link).context("link the model")?;
    #[cfg(not(unix))]
    std::fs::hard_link(model, &link).context("link the model")?;
    Ok(dir)
}

/// Kinds of mistakes, by the right word (or what was heard instead)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A number, in digits or in words
    Number,
    /// A word of the dictionary
    Dictionary,
    /// The right word with another ending
    Ending,
    Other,
}

/// Number words that are never anything else
const NUMBER_WORDS: [&str; 12] = ["нуль", "один", "одна", "одне", "два", "дві", "три", "сім", "сто", "сота", "сотні", "сотня"];
/// Stems of number words long enough not to start other words
const NUMBER_STEMS: [&str; 24] = [
    "одинадцят", "дванадцят", "двадцят", "двох", "двом", "трьох", "тридцят", "чотир", "п'ят", "шіст", "шести", "сьом", "семи",
    "вісім", "восьм", "дев'ят", "десят", "сорок", "двіст", "трист", "тисяч", "мільйон", "мільярд", "трет",
];

fn is_number(word: &str) -> bool {
    word.chars().any(|c| c.is_ascii_digit()) || NUMBER_WORDS.contains(&word) || NUMBER_STEMS.iter().any(|s| word.starts_with(s))
}

/// The same word with another ending: at most three letters differ, at the
/// end, and the common start is most of the word
fn same_stem(a: &str, b: &str) -> bool {
    let common = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
    let longer = a.chars().count().max(b.chars().count());
    common >= 3 && longer - common <= 3 && common * 5 > longer * 3
}

/// What kind of mistake: `right` and `heard` compared as [`crate::quality::score::norm`]
/// makes them; `heard` is `None` for a missing word
pub fn kind_of(right: &str, heard: Option<&str>, dictionary: &HashSet<String>) -> Kind {
    if is_number(right) || heard.is_some_and(is_number) {
        Kind::Number
    } else if dictionary.contains(right) {
        Kind::Dictionary
    } else if heard.is_some_and(|h| same_stem(right, h)) {
        Kind::Ending
    } else {
        Kind::Other
    }
}

/// Words of the dictionary as compared (three letters or more)
pub fn dictionary_words(terms: &[String]) -> HashSet<String> {
    terms
        .iter()
        .flat_map(|t| t.split_whitespace())
        .map(crate::quality::score::norm)
        .filter(|w| w.chars().count() >= 3)
        .collect()
}

/// What a model gets wrong on the held-out meetings beyond the count: kinds
/// of mistakes and how the underlines do. Numbers only.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Detail {
    pub seconds: f64,
    pub words: usize,
    pub mistakes: usize,
    /// Mistakes by kind (extra words count as other)
    pub numbers: usize,
    pub dictionary: usize,
    pub endings: usize,
    pub other: usize,
    /// Underlined words, and those of them that were mistakes
    pub underlined: usize,
    pub underlined_wrong: usize,
    /// Mistakes an underline points to
    pub caught: usize,
}

impl Detail {
    /// The share of mistakes an underline points to
    pub fn caught_share(&self) -> f64 {
        if self.mistakes == 0 {
            0.0
        } else {
            self.caught as f64 / self.mistakes as f64
        }
    }

    /// One piece: the right text, the recognised words and which are underlined
    pub fn add(&mut self, seconds: f64, right: &str, recognised: &[String], underlined: &[bool], dictionary: &HashSet<String>) {
        use crate::quality::score::{align, norm, Tally};
        let mut t = Tally::default();
        t.add(seconds, right, recognised, underlined);
        self.seconds += seconds;
        self.words += t.reference_words;
        self.mistakes += t.mistakes;
        self.underlined += t.flagged;
        self.underlined_wrong += t.flagged - t.flagged_right;
        self.caught += t.caught;
        let r: Vec<String> = right.split_whitespace().map(norm).filter(|w| !w.is_empty()).collect();
        let h: Vec<String> = recognised.iter().map(|w| norm(w)).filter(|w| !w.is_empty()).collect();
        let a = align(&r, &h);
        let kinds = a
            .wrong
            .iter()
            .map(|&(i, j)| kind_of(&r[i], Some(&h[j]), dictionary))
            .chain(a.missed.iter().map(|&i| kind_of(&r[i], None, dictionary)))
            .chain(std::iter::repeat(Kind::Other).take(a.inserted));
        for k in kinds {
            match k {
                Kind::Number => self.numbers += 1,
                Kind::Dictionary => self.dictionary += 1,
                Kind::Ending => self.endings += 1,
                Kind::Other => self.other += 1,
            }
        }
    }
}

/// One held-out piece as a model heard it
#[derive(Debug, Clone, Default)]
pub struct Heard {
    pub seconds: f64,
    /// The checked text
    pub right: String,
    pub words: Vec<String>,
    pub probs: Vec<f32>,
    /// Held to the higher bar: numbers, names outside the dictionary
    pub special: Vec<bool>,
    /// Underlined whatever the bars: a learned correction offers to change it
    pub hinted: Vec<bool>,
}

impl Heard {
    /// Which words are underlined under the bars `t`, as in the transcript
    pub fn underlined(&self, t: Thresholds) -> Vec<bool> {
        self.words
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let p = self.probs.get(i).copied().unwrap_or(1.0);
                self.hinted.get(i).copied().unwrap_or(false) || doubt::below(w, p, self.special.get(i).copied().unwrap_or(false), t)
            })
            .collect()
    }
}

/// Each held-out meeting (in the order given) as the model in `models_dir`
/// hears it, and the underline bars of that model. While `recording()` is
/// true it waits: recording has the Mac to itself.
pub async fn hear(
    models_dir: PathBuf,
    model_name: &str,
    meetings: &[Vec<PieceFile>],
    recording: &(impl Fn() -> bool + Sync),
) -> Result<(Vec<Vec<Heard>>, Thresholds)> {
    let engine = crate::whisper_engine::WhisperEngine::new_with_models_dir(Some(models_dir))?;
    engine.discover_models().await?;
    engine.load_model(model_name).await?;
    let bars = engine.bars().await;
    let terms = crate::ukrainian::glossary_terms();
    let known = |w: &str| terms.iter().any(|t| t.eq_ignore_ascii_case(w) || t.to_lowercase() == w.to_lowercase());
    let hints = crate::ukrainian::corrections::hint_words();
    let mut out = Vec::new();
    for pieces in meetings {
        let mut meeting = Vec::new();
        for p in pieces {
            while recording() {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            let samples = crate::quality::wav::read(&p.file)?;
            let seconds = samples.len() as f64 / 16_000.0;
            let piece = crate::speakers::words::transcribe_piece(&engine, &samples, 0.0, seconds, Some("uk".into())).await?;
            let (words, probs): (Vec<String>, Vec<f32>) = match piece {
                Some(h) => {
                    let probs = (0..h.words.len()).map(|i| h.probs.get(i).copied().unwrap_or(1.0)).collect();
                    (h.words.into_iter().map(|w| w.0).collect(), probs)
                }
                None => (Vec::new(), Vec::new()),
            };
            let special = doubt::special_words(&words, &known);
            let hinted = words.iter().zip(&probs).map(|(w, p)| doubt::hinted(w, *p, &hints)).collect();
            meeting.push(Heard { seconds, right: p.text.clone(), words, probs, special, hinted });
        }
        out.push(meeting);
    }
    engine.unload_model().await;
    Ok((out, bars))
}

/// Mistakes per meeting
pub fn scores(heard: &[Vec<Heard>]) -> Vec<Score> {
    heard
        .iter()
        .map(|pieces| {
            let mut tally = crate::quality::score::Tally::default();
            for h in pieces {
                tally.add(h.seconds, &h.right, &h.words, &[]);
            }
            Score { words: tally.reference_words, mistakes: tally.substituted + tally.deleted + tally.inserted }
        })
        .collect()
}

/// Kinds of mistakes and how the underlines do under the bars `t`, over all meetings
pub fn detail(heard: &[Vec<Heard>], t: Thresholds, dictionary: &HashSet<String>) -> Detail {
    let mut d = Detail::default();
    for h in heard.iter().flatten() {
        d.add(h.seconds, &h.right, &h.words, &h.underlined(t), dictionary);
    }
    d
}

/// Fewer mistakes than this on the held-out meetings say too little to choose bars by
const MIN_MISTAKES_TO_TUNE: usize = 50;

/// Bars for a new model: the lowest at which its underlines still point to
/// `share` of its mistakes (the share the model in use reaches), so a model
/// that is right more often, or sure of its words in another way, asks the
/// secretary to look at no more places than needed. The defaults when there
/// is too little to go by.
pub fn tune(heard: &[Vec<Heard>], share: f64) -> Thresholds {
    if !(share > 0.0) {
        return Thresholds::DEFAULT;
    }
    let caught_at = |t: Thresholds| {
        let mut tally = crate::quality::score::Tally::default();
        for h in heard.iter().flatten() {
            tally.add(h.seconds, &h.right, &h.words, &h.underlined(t));
        }
        (tally.caught, tally.mistakes)
    };
    let (_, mistakes) = caught_at(Thresholds::DEFAULT);
    if mistakes < MIN_MISTAKES_TO_TUNE {
        return Thresholds::DEFAULT;
    }
    // From 0.30 up to 0.97 by 0.01: more words go under the bar at each step
    for step in 30..=97u32 {
        let t = Thresholds::from_normal(step as f32 / 100.0);
        let (caught, mistakes) = caught_at(t);
        if caught as f64 >= share * mistakes as f64 {
            return t;
        }
    }
    Thresholds::from_normal(0.97)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mistakes_are_sorted_by_kind() {
        let dictionary = dictionary_words(&["Гончаренко".to_string(), "ЗСУ".to_string(), "Збройні Сили".to_string()]);
        assert_eq!(kind_of("3018", Some("3008"), &dictionary), Kind::Number);
        assert_eq!(kind_of("тисячі", None, &dictionary), Kind::Number);
        assert_eq!(kind_of("п'ятнадцять", Some("п'ятдесят"), &dictionary), Kind::Number);
        assert_eq!(kind_of("гончаренко", Some("кончаренко"), &dictionary), Kind::Dictionary);
        assert_eq!(kind_of("зсу", None, &dictionary), Kind::Dictionary);
        assert_eq!(kind_of("доповідає", Some("доповідав"), &dictionary), Kind::Ending);
        assert_eq!(kind_of("роти", Some("роті"), &dictionary), Kind::Ending);
        assert_eq!(kind_of("стоїть", Some("стояв"), &dictionary), Kind::Other, "«сто» only as a whole word");
        assert_eq!(kind_of("сім'я", Some("сімка"), &dictionary), Kind::Other);
        assert_eq!(kind_of("план", Some("клан"), &dictionary), Kind::Other);
    }

    #[test]
    fn the_detail_counts_kinds_and_underlines() {
        let dictionary = dictionary_words(&["Гончаренко".to_string()]);
        let words = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        let mut d = Detail::default();
        // «Гончаренко» wrong and underlined, «доповів» wrong ending, «три» missing, «ну» extra
        d.add(10.0, "Гончаренко доповів про три пункти", &words("Кончаренко доповідав ну про пункти"), &[true, false, false, false, true], &dictionary);
        assert_eq!(d.words, 5);
        assert_eq!(d.mistakes, 4);
        assert_eq!((d.numbers, d.dictionary, d.endings, d.other), (1, 1, 1, 1));
        assert_eq!(d.underlined, 2);
        assert_eq!(d.underlined_wrong, 1);
    }

    fn heard(right: &str, words: &[&str], probs: &[f32]) -> Heard {
        let words: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        Heard { seconds: 5.0, right: right.into(), special: vec![false; words.len()], hinted: vec![false; words.len()], words, probs: probs.to_vec() }
    }

    #[test]
    fn underlines_follow_the_bars_names_and_hints() {
        let mut h = heard("Петренко доповів план", &["Петренко", "доповів", "план"], &[0.55, 0.7, 0.99]);
        h.special[0] = true;
        assert_eq!(h.underlined(Thresholds::DEFAULT), vec![true, true, false]);
        // Lower bars (0.5, names 0.6): the name still, the plain word no longer
        assert_eq!(h.underlined(Thresholds::from_normal(0.5)), vec![true, false, false]);
        h.hinted[2] = true;
        assert_eq!(h.underlined(Thresholds::from_normal(0.3)), vec![false, false, true]);
    }

    #[test]
    fn a_new_model_gets_the_lowest_bars_that_catch_as_much() {
        // 100 pieces, one wrong word in each, heard with sureness from 0.30 to 0.894
        let meeting: Vec<Heard> = (0..100)
            .map(|k| heard("план виконано вчасно", &["план", "виконали", "вчасно"], &[0.98, 0.3 + 0.006 * k as f32, 0.97]))
            .collect();
        let meetings = vec![meeting];
        assert_eq!(scores(&meetings), vec![Score { words: 300, mistakes: 100 }]);
        // Half the mistakes: the 50 least sure ones lie below 0.6
        assert_eq!(tune(&meetings, 0.5), Thresholds::from_normal(0.6));
        assert_eq!(tune(&meetings, 1.0), Thresholds::from_normal(0.9));
        let d = detail(&meetings, Thresholds::from_normal(0.6), &HashSet::new());
        assert_eq!((d.mistakes, d.caught, d.underlined, d.underlined_wrong), (100, 50, 50, 50));
        assert!((d.caught_share() - 0.5).abs() < 1e-9);
        // Nothing to go by: the defaults
        assert_eq!(tune(&meetings, 0.0), Thresholds::DEFAULT);
        assert_eq!(tune(&[meetings[0][..10].to_vec()], 0.5), Thresholds::DEFAULT);
    }
}
