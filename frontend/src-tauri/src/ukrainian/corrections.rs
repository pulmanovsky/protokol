//! Glossary suggestions and spelling corrections.
//!
//! After a meeting, names, nicknames and units that appear in the transcript
//! are collected and grouped when they look like spellings of the same word
//! («Гончаренко» / «Кончаренко»). The secretary confirms the right spelling in
//! «Словник» with one click: it goes into the glossary (which steers Whisper)
//! and the other spellings become corrections, applied to transcripts before
//! a summary is written. Everything stays on this Mac.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::Serialize;

pub(crate) const CORRECTIONS_FILE: &str = "corrections.txt";
const DISMISSED_FILE: &str = "glossary_dismissed.txt";
const MAX_SUGGESTIONS: usize = 40;

/// One suggestion: spellings that probably mean the same name or unit
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Suggestion {
    /// Most frequent spelling first
    pub variants: Vec<String>,
    pub counts: Vec<usize>,
    /// A short piece of transcript around the first occurrence
    pub example: String,
}

pub(crate) fn data_file(name: &str) -> Option<PathBuf> {
    super::glossary_dir().map(|dir| dir.join(name))
}

pub(crate) fn read_lines(name: &str) -> Vec<String> {
    data_file(name)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

pub(crate) fn write_lines(name: &str, lines: &[String]) -> Result<(), String> {
    let path = data_file(name).ok_or("Glossary storage is not initialised")?;
    std::fs::write(path, lines.join("\n") + "\n").map_err(|e| e.to_string())
}

/// A learned correction: what was recognised, what it should be, whether
/// it is applied (switched off in «Словник» it is kept but not used)
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Correction {
    pub wrong: String,
    pub right: String,
    pub enabled: bool,
}

/// All saved corrections («wrong => right»; «!» in front: switched off)
pub fn all_corrections() -> Vec<Correction> {
    read_lines(CORRECTIONS_FILE)
        .into_iter()
        .filter_map(|l| {
            let (off, l) = match l.strip_prefix('!') {
                Some(rest) => (true, rest.to_string()),
                None => (false, l),
            };
            l.split_once("=>").map(|(a, b)| Correction { wrong: a.trim().to_string(), right: b.trim().to_string(), enabled: !off })
        })
        .filter(|c| !c.wrong.is_empty() && !c.right.is_empty() && c.wrong != c.right)
        .collect()
}

/// Corrections in use «wrong => right»
pub fn corrections() -> Vec<(String, String)> {
    all_corrections().into_iter().filter(|c| c.enabled).map(|c| (c.wrong, c.right)).collect()
}

fn save_all(all: &[Correction]) -> Result<(), String> {
    let lines: Vec<String> = all.iter().map(|c| format!("{}{} => {}", if c.enabled { "" } else { "!" }, c.wrong, c.right)).collect();
    write_lines(CORRECTIONS_FILE, &lines)
}

pub(crate) fn add_corrections(pairs: &[(String, String)]) -> Result<(), String> {
    let mut all = all_corrections();
    for (wrong, right) in pairs {
        all.retain(|c| c.wrong.to_lowercase() != wrong.to_lowercase());
        if wrong != right {
            all.push(Correction { wrong: wrong.clone(), right: right.clone(), enabled: true });
        }
    }
    save_all(&all)
}

/// Switches a correction on or off
pub fn set_correction_enabled(wrong: &str, enabled: bool) -> Result<(), String> {
    let mut all = all_corrections();
    for c in all.iter_mut().filter(|c| c.wrong == wrong) {
        c.enabled = enabled;
    }
    save_all(&all)
}

/// Forgets a correction
pub fn remove_correction(wrong: &str) -> Result<(), String> {
    let mut all = all_corrections();
    all.retain(|c| c.wrong != wrong);
    save_all(&all)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '\'' || c == '’' || c == '-'
}

/// Replaces whole words and phrases of up to three words («Кончаренко» →
/// «Гончаренко», «робоч а частина» → «робоча частина»), never inside another
/// word. Matching ignores case; a capitalised word stays capitalised.
pub fn apply_corrections_with(text: &str, pairs: &[(String, String)]) -> String {
    if pairs.is_empty() {
        return text.to_string();
    }
    let map: HashMap<Vec<String>, &str> = pairs
        .iter()
        .map(|(w, r)| (w.split_whitespace().map(|x| x.to_lowercase()).collect::<Vec<_>>(), r.as_str()))
        .filter(|(k, _)| !k.is_empty() && k.len() <= 3)
        .collect();
    let longest = map.keys().map(Vec::len).max().unwrap_or(1);
    // Tokens: words and what is between them
    let mut tokens: Vec<(bool, String)> = Vec::new();
    for c in text.chars() {
        let word = is_word_char(c);
        match tokens.last_mut() {
            Some((w, t)) if *w == word => t.push(c),
            _ => tokens.push((word, c.to_string())),
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < tokens.len() {
        if !tokens[i].0 {
            out.push_str(&tokens[i].1);
            i += 1;
            continue;
        }
        let mut matched = None;
        for n in (1..=longest).rev() {
            // n words at i, separated by plain spaces only
            let mut words = Vec::new();
            let mut j = i;
            while words.len() < n && j < tokens.len() {
                if tokens[j].0 {
                    words.push(tokens[j].1.to_lowercase());
                } else if tokens[j].1.chars().any(|c| !c.is_whitespace()) {
                    break;
                }
                j += 1;
            }
            if words.len() == n {
                if let Some(right) = map.get(&words) {
                    matched = Some((j, *right));
                    break;
                }
            }
        }
        match matched {
            Some((end, right)) => {
                let capital = tokens[i].1.chars().next().is_some_and(char::is_uppercase);
                let mut r = right.to_string();
                if capital && r.chars().next().is_some_and(char::is_lowercase) {
                    let mut c = r.chars();
                    r = c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default();
                }
                out.push_str(&r);
                i = end;
            }
            None => {
                out.push_str(&tokens[i].1);
                i += 1;
            }
        }
    }
    out
}

/// Applies the saved corrections (to new recognised text, and to a transcript
/// before summarising)
pub fn apply_corrections(text: &str) -> String {
    apply_corrections_with(text, &corrections())
}

/// Rules applied while a recording is recognised, waiting for its meeting
/// (the meeting exists only when the recording is saved)
static PENDING: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// As `apply_corrections`, and notes which rules it applied (recognition
/// of a recording or an import; the save takes them with `take_pending`)
pub fn apply_corrections_noting(text: &str) -> String {
    let pairs = corrections();
    let fixed = apply_corrections_with(text, &pairs);
    if fixed != text {
        let found = applied_rules(text, &pairs);
        if let Ok(mut p) = PENDING.lock() {
            p.extend(found);
        }
    }
    fixed
}

/// The rules applied since the last save, for the meeting being saved
pub fn take_pending() -> Vec<(String, String)> {
    PENDING.lock().map(|mut p| std::mem::take(&mut *p)).unwrap_or_default()
}

/// Which of `pairs` match `before` (its words, case aside), once per match
pub fn applied_rules(before: &str, pairs: &[(String, String)]) -> Vec<(String, String)> {
    let words: Vec<String> = before.split(|c: char| !is_word_char(c)).filter(|w| !w.is_empty()).map(|w| w.to_lowercase()).collect();
    let mut out = Vec::new();
    for (wrong, right) in pairs {
        let key: Vec<String> = wrong.split_whitespace().map(|w| w.to_lowercase()).collect();
        if key.is_empty() || key.len() > words.len() {
            continue;
        }
        for i in 0..=words.len() - key.len() {
            if words[i..i + key.len()] == key[..] {
                out.push((wrong.clone(), right.clone()));
            }
        }
    }
    out
}

pub(crate) fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Two spellings of probably the same word
fn similar(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    let len = a.chars().count().max(b.chars().count());
    len >= 4 && levenshtein(&a, &b) <= (len / 3).max(1)
}

/// Capitalised words that are not at the start of a sentence: mostly names,
/// nicknames, units and places. Also abbreviations in capitals.
pub(crate) fn candidates(text: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for line in text.lines() {
        // Skip "[MM:SS]" and "Name:" prefixes
        let line = line.split_once("] ").map(|(_, rest)| rest).unwrap_or(line);
        let words: Vec<&str> = line.split_whitespace().collect();
        for (i, raw) in words.iter().enumerate() {
            let word: String = raw.trim_matches(|c: char| !is_word_char(c)).to_string();
            let letters = word.chars().filter(|c| c.is_alphabetic()).count();
            if letters < 3 {
                continue;
            }
            let first_upper = word.chars().next().is_some_and(|c| c.is_uppercase());
            let all_upper = word.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase());
            let sentence_start = i == 0 || words[i - 1].ends_with(['.', '!', '?', ':', '…']);
            if (first_upper && !sentence_start) || (all_upper && letters <= 6) {
                let from = i.saturating_sub(4);
                let to = (i + 5).min(words.len());
                found.push((word, words[from..to].join(" ")));
            }
        }
    }
    found
}

/// Suggestions from recent transcripts, leaving out what is already in the
/// glossary, corrected or dismissed
pub fn suggestions_from(texts: &[String], glossary: &[String], corrected: &[(String, String)], dismissed: &[String]) -> Vec<Suggestion> {
    let known: HashSet<String> = glossary
        .iter()
        .flat_map(|t| t.split_whitespace().map(|w| w.trim_matches(|c: char| !is_word_char(c)).to_lowercase()))
        .chain(corrected.iter().map(|(w, _)| w.to_lowercase()))
        .chain(dismissed.iter().map(|d| d.to_lowercase()))
        .collect();
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut examples: HashMap<String, String> = HashMap::new();
    for text in texts {
        for (word, context) in candidates(text) {
            if known.contains(&word.to_lowercase()) {
                continue;
            }
            *counts.entry(word.clone()).or_default() += 1;
            examples.entry(word).or_insert(context);
        }
    }
    // Group similar spellings (union-find over the candidate words)
    let words: Vec<String> = counts.keys().cloned().collect();
    let mut parent: Vec<usize> = (0..words.len()).collect();
    fn root(p: &mut Vec<usize>, i: usize) -> usize {
        let mut i = i;
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for i in 0..words.len() {
        for j in i + 1..words.len() {
            if similar(&words[i], &words[j]) {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                parent[a] = b;
            }
        }
    }
    let mut groups: HashMap<usize, Vec<String>> = HashMap::new();
    for i in 0..words.len() {
        let r = root(&mut parent, i);
        groups.entry(r).or_default().push(words[i].clone());
    }
    let mut out: Vec<Suggestion> = groups
        .into_values()
        .filter_map(|mut variants| {
            variants.sort_by_key(|v| std::cmp::Reverse(counts[v]));
            let total: usize = variants.iter().map(|v| counts[v]).sum();
            // A single spelling seen once is noise; variant groups always count
            (variants.len() > 1 || total >= 2).then(|| Suggestion {
                counts: variants.iter().map(|v| counts[v]).collect(),
                example: examples.get(&variants[0]).cloned().unwrap_or_default(),
                variants,
            })
        })
        .collect();
    // Variant groups first (likely recognition errors), then by frequency
    out.sort_by_key(|s| (std::cmp::Reverse(s.variants.len() > 1), std::cmp::Reverse(s.counts.iter().sum::<usize>())));
    out.truncate(MAX_SUGGESTIONS);
    out
}

/// Grammatical forms of one word («Сокіл» / «Соколом»): same beginning,
/// different ending. These are not recognition errors and are never replaced.
pub(crate) fn same_word_forms(a: &str, b: &str) -> bool {
    let a: Vec<char> = a.to_lowercase().chars().collect();
    let b: Vec<char> = b.to_lowercase().chars().collect();
    let shorter = a.len().min(b.len());
    let common = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
    shorter >= 3 && common >= shorter.saturating_sub(2).max(3)
}

/// Confirms `right` as the correct spelling: adds it to the glossary and maps
/// the other spellings to it (not its own grammatical forms)
pub fn accept(right: &str, variants: &[String]) -> Result<(), String> {
    let right = right.trim();
    if right.is_empty() {
        return Err("Порожнє написання".into());
    }
    let mut glossary = super::glossary();
    if !glossary.iter().any(|t| super::term_of(t) == right) {
        glossary.push(right.to_string());
        super::set_glossary(glossary)?;
    }
    let pairs: Vec<(String, String)> =
        variants
            .iter()
            .filter(|v| v.as_str() != right && !same_word_forms(v, right))
            .map(|v| (v.clone(), right.to_string()))
            .collect();
    add_corrections(&pairs)
}

/// Hides a suggestion that is not a name (e.g. an ordinary capitalised word)
pub fn dismiss(variants: &[String]) -> Result<(), String> {
    let mut dismissed = read_lines(DISMISSED_FILE);
    for v in variants {
        if !dismissed.contains(v) {
            dismissed.push(v.clone());
        }
    }
    write_lines(DISMISSED_FILE, &dismissed)
}

pub fn dismissed() -> Vec<String> {
    read_lines(DISMISSED_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_spellings_of_the_same_name() {
        let texts = vec![
            "[00:01] Питання до Ігоря Гончаренка. Гончаренко відповідає. Звертайтесь до Кончаренка.\n[00:05] Ділянка Сокіл готова.".to_string(),
            "[00:01] Доповідає Гончаренко, потім Сокіл.".to_string(),
        ];
        let s = suggestions_from(&texts, &[], &[], &[]);
        let group = s.iter().find(|g| g.variants.contains(&"Гончаренко".to_string())).unwrap();
        assert!(group.variants.contains(&"Кончаренка".to_string()) || group.variants.contains(&"Гончаренка".to_string()));
        assert!(s.iter().any(|g| g.variants == vec!["Сокіл".to_string()]));
    }

    #[test]
    fn known_terms_are_not_suggested() {
        let texts = vec!["[00:01] Доповідає Гончаренко. Ще раз Гончаренко.".to_string()];
        assert!(suggestions_from(&texts, &["Гончаренко".to_string()], &[], &[]).is_empty());
        assert!(suggestions_from(&texts, &[], &[], &["Гончаренко".to_string()]).is_empty());
    }

    #[test]
    fn corrections_replace_whole_words_only() {
        let pairs = vec![("Кончаренко".to_string(), "Гончаренко".to_string())];
        assert_eq!(apply_corrections_with("Кончаренко сказав, Кончаренкова ні.", &pairs), "Гончаренко сказав, Кончаренкова ні.");
        // Phrases, any case, whole words only
        let pairs = vec![("робоч а частина".to_string(), "робоча частина".to_string()), ("прат".to_string(), "ПрАТ".to_string()), ("вузол".to_string(), "Вузол-2".to_string())];
        assert_eq!(apply_corrections_with("Робоч а частина готова, прат теж.", &pairs), "Робоча частина готова, ПрАТ теж.");
        assert_eq!(apply_corrections_with("ПРАТ і вузловий пункт", &pairs), "ПрАТ і вузловий пункт");
        // A phrase across punctuation is not one phrase
        assert_eq!(apply_corrections_with("робоч а. частина", &pairs), "робоч а. частина");
    }

    #[test]
    fn word_forms_are_not_errors() {
        assert!(same_word_forms("Сокіл", "Соколом"));
        assert!(same_word_forms("Гончаренко", "Гончаренка"));
        assert!(!same_word_forms("Кончаренко", "Гончаренко"));
    }

    #[test]
    fn similarity_needs_close_spellings() {
        assert!(similar("Гончаренко", "Кончаренко"));
        assert!(!similar("Сокіл", "Берег"));
        assert!(!similar("АБВ", "ГДЕ"));
    }
}

#[cfg(test)]
mod learning_eval {
    /// His text corrections on a COPY vs what the saved corrections now do by
    /// themselves: PROTOKOL_WORDS_DB, PROTOKOL_SPEAKERS_MEETING, PROTOKOL_CORRECTIONS (a copy
    /// of corrections.txt). Counts and rates only.
    #[tokio::test]
    #[ignore = "needs a database copy"]
    async fn corrections_against_hand_edits() {
        let db = std::env::var("PROTOKOL_WORDS_DB").unwrap();
        assert!(db.contains(&crate::test_copy_root()), "only a copy");
        let meeting = std::env::var("PROTOKOL_SPEAKERS_MEETING").unwrap();
        let pairs: Vec<(String, String)> = std::fs::read_to_string(std::env::var("PROTOKOL_CORRECTIONS").unwrap())
            .unwrap()
            .lines()
            .filter(|l| !l.starts_with('!'))
            .filter_map(|l| l.split_once("=>").map(|(a, b)| (a.trim().to_string(), b.trim().to_string())))
            .filter(|(a, b)| !a.is_empty() && a != b)
            .collect();
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{db}?mode=ro")).await.unwrap();
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT original_transcript, transcript FROM transcripts WHERE meeting_id = ? AND original_transcript IS NOT NULL AND original_transcript <> transcript").bind(&meeting).fetch_all(&pool).await.unwrap();
        let words = |s: &str| s.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()).filter(|w| !w.is_empty()).collect::<Vec<_>>();
        let dist = |a: &[String], b: &[String]| {
            let mut prev: Vec<usize> = (0..=b.len()).collect();
            for i in 1..=a.len() {
                let mut cur = vec![i; b.len() + 1];
                for j in 1..=b.len() {
                    cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
                }
                prev = cur;
            }
            prev[b.len()]
        };
        let (mut ref_words, mut before_err, mut after_err, mut edits, mut auto) = (0, 0, 0, 0, 0);
        for (orig, fixed) in &rows {
            let auto_text = super::apply_corrections_with(orig, &pairs);
            let (o, f, a) = (words(orig), words(fixed), words(&auto_text));
            ref_words += f.len();
            before_err += dist(&o, &f);
            after_err += dist(&a, &f);
            for (w, r) in crate::ukrainian::learning::edit_pairs(orig, fixed) {
                edits += 1;
                if auto_text.contains(&r) && !orig.contains(&r) && !auto_text.contains(&w) {
                    auto += 1;
                }
            }
        }
        println!("WORDS edited lines {}, his word fixes {edits}, now applied by themselves {auto}; WER of edited lines vs his text: as recognised {:.1}%, with saved corrections {:.1}% ({} corrections saved)",
            rows.len(), 100.0 * before_err as f64 / ref_words.max(1) as f64, 100.0 * after_err as f64 / ref_words.max(1) as f64, pairs.len());
    }
}
