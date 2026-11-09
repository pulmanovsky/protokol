//! The dictionary learns as the app is used. Everything stays on this Mac.
//!
//! - From the secretary's edits: when a saved document differs from the one
//!   the app wrote by a word or a name («Кончаренко» → «Гончаренко», «Доповідач 3» →
//!   «Олег»), the right spelling joins the glossary and the wrong one
//!   becomes a correction applied before the next summary.
//! - From repetition: a name, nickname or abbreviation written the same way
//!   in several meetings is added on its own, marked «вивчено».
//! Each learned entry keeps where it came from, how often it was seen, when,
//! and a short example. Entries can be removed in «Словник».

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::corrections::{self, add_corrections, levenshtein, read_lines, write_lines};

const META_FILE: &str = "glossary_meta.json";
/// A term is learned when seen in this many meetings…
const AUTO_LEARN_MEETINGS: usize = 2;
/// …or this many times in all
const AUTO_LEARN_OCCURRENCES: usize = 3;
/// Common words that say nothing about a term's role
const CONTEXT_STOPWORDS: &[&str] = &[
    "і", "й", "та", "а", "але", "що", "як", "це", "по", "на", "в", "у", "з", "із", "до", "від", "для", "про", "за", "не",
    "так", "там", "тут", "ми", "ви", "він", "вона", "вони", "я", "є", "був", "була", "буде", "все", "всі", "дякую",
    "добре", "також", "теж", "ще", "вже", "щодо", "який", "яка", "які", "цей", "ця", "ці", "той", "мене", "нас", "вас",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TermMeta {
    /// "edit" (from the secretary's corrections) or "auto" (repeated in meetings)
    pub source: String,
    /// Meetings it was seen in
    #[serde(default)]
    pub meetings: usize,
    pub count: usize,
    pub last_seen: String,
    pub example: String,
    /// Spellings corrected to this term
    #[serde(default)]
    pub wrong: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LearnedTerm {
    pub term: String,
    pub context: String,
    #[serde(flatten)]
    pub meta: TermMeta,
}

fn load_meta() -> HashMap<String, TermMeta> {
    corrections::data_file(META_FILE)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_meta(meta: &HashMap<String, TermMeta>) -> Result<(), String> {
    let path = corrections::data_file(META_FILE).ok_or("Glossary storage is not initialised")?;
    std::fs::write(path, serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// How often each glossary term was seen or used (for ranking)
pub fn usage_counts() -> HashMap<String, usize> {
    load_meta().into_iter().map(|(k, v)| (k, v.count)).collect()
}

fn today() -> String {
    chrono::Local::now().format("%d.%m.%Y").to_string()
}

/// Adds `term` to the glossary (if missing) and records where it came from
fn learn(term: &str, source: &str, example: &str, wrong: Option<&str>) -> Result<(), String> {
    let term = term.trim();
    if term.is_empty() {
        return Ok(());
    }
    let mut glossary = super::glossary();
    if !glossary.iter().any(|l| super::term_of(l) == term) {
        glossary.push(term.to_string());
        super::set_glossary(glossary)?;
    }
    let mut meta = load_meta();
    let entry = meta.entry(term.to_string()).or_insert_with(|| TermMeta { source: source.to_string(), ..Default::default() });
    if source == "edit" {
        entry.source = "edit".into(); // a confirmed correction outranks auto
    }
    entry.count += 1;
    entry.last_seen = today();
    if !example.is_empty() {
        entry.example = example.chars().take(120).collect();
    }
    if let Some(w) = wrong.filter(|w| !w.is_empty() && *w != term) {
        if !entry.wrong.iter().any(|x| x == w) {
            entry.wrong.push(w.to_string());
        }
    }
    save_meta(&meta)
}

fn words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

fn bare(w: &str) -> &str {
    w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’' && c != '-')
}

/// Word substitutions between the app's text and the edited one:
/// replaced spans of one to three words where the new text is a name
/// (capitalised) or a close respelling. Paragraphs are compared one to one.
pub fn substitutions(before: &str, after: &str) -> Vec<(String, String)> {
    let old_paras: Vec<&str> = before.lines().filter(|l| !l.trim().is_empty()).collect();
    let new_paras: Vec<&str> = after.lines().filter(|l| !l.trim().is_empty()).collect();
    if old_paras.len() != new_paras.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (a, b) in old_paras.iter().zip(new_paras.iter()) {
        if a == b {
            continue;
        }
        let (aw, bw) = (words(a), words(b));
        if aw.len() > 600 || bw.len() > 600 {
            continue;
        }
        // Longest common subsequence over bare words
        let (n, m) = (aw.len(), bw.len());
        let mut dp = vec![vec![0u16; m + 1]; n + 1];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                dp[i][j] = if bare(aw[i]) == bare(bw[j]) { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        let (mut del, mut ins): (Vec<&str>, Vec<&str>) = (Vec::new(), Vec::new());
        let mut flush = |del: &mut Vec<&str>, ins: &mut Vec<&str>, out: &mut Vec<(String, String)>| {
            if (1..=3).contains(&del.len()) && (1..=3).contains(&ins.len()) {
                let wrong = del.iter().map(|w| bare(w)).collect::<Vec<_>>().join(" ");
                let right = ins.iter().map(|w| bare(w)).collect::<Vec<_>>().join(" ");
                let name = right.chars().next().is_some_and(char::is_uppercase);
                let close = levenshtein(&wrong.to_lowercase(), &right.to_lowercase()) <= right.chars().count().max(2) / 2;
                if !wrong.is_empty() && !right.is_empty() && wrong != right && (name || close) {
                    out.push((wrong, right));
                }
            }
            del.clear();
            ins.clear();
        };
        while i < n || j < m {
            if i < n && j < m && bare(aw[i]) == bare(bw[j]) {
                flush(&mut del, &mut ins, &mut out);
                i += 1;
                j += 1;
            } else if j < m && (i == n || dp[i][j + 1] >= dp[i + 1][j]) {
                ins.push(bw[j]);
                j += 1;
            } else {
                del.push(aw[i]);
                i += 1;
            }
        }
        flush(&mut del, &mut ins, &mut out);
    }
    out
}

/// Words too common to ever be corrected automatically («так» → «Так»
/// would change every sentence)
const COMMON: &[&str] = &[
    "і", "й", "та", "а", "але", "що", "як", "це", "по", "на", "в", "у", "з", "із", "до", "від", "для", "про", "за", "не",
    "так", "ні", "там", "тут", "ми", "ви", "він", "вона", "вони", "я", "є", "був", "була", "буде", "все", "всі", "дякую",
    "добре", "ну", "от", "то", "ще", "вже", "його", "її", "їх", "мене", "нас", "вас", "цей", "ця", "ці", "той",
];

/// Replacement pairs from a corrected transcript line (one to three words
/// each). Case-only changes count when the right form has capitals (an
/// abbreviation or a name: «прат» → «ПрАТ»); very common words never do.
pub fn edit_pairs(before: &str, after: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (wrong, right) in substitutions_any(before, after) {
        let common = wrong.split_whitespace().all(|w| COMMON.contains(&w.to_lowercase().as_str()));
        let case_only = wrong.to_lowercase() == right.to_lowercase();
        // A capital only on the first letter at the start of a sentence is grammar, not a name
        let inner_capital = right.chars().skip(1).any(char::is_uppercase);
        let starts_sentence = after.find(right.as_str()).is_some_and(|at| {
            after[..at].trim_end().chars().last().map_or(true, |c| matches!(c, '.' | '!' | '?' | '…'))
        });
        let grammar_only = case_only && !inner_capital && (starts_sentence || !right.chars().any(char::is_uppercase));
        if common || wrong.chars().count() < 2 || grammar_only {
            continue;
        }
        if !out.iter().any(|(w, _)| *w == wrong) {
            out.push((wrong, right));
        }
    }
    out
}

/// Like [`substitutions`], but any respelling counts (the user typed it on
/// purpose), not only names and close ones
fn substitutions_any(before: &str, after: &str) -> Vec<(String, String)> {
    let (aw, bw) = (words(before), words(after));
    if aw.len() > 600 || bw.len() > 600 {
        return Vec::new();
    }
    let (n, m) = (aw.len(), bw.len());
    let mut dp = vec![vec![0u16; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if bare(aw[i]) == bare(bw[j]) { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    let (mut del, mut ins): (Vec<&str>, Vec<&str>) = (Vec::new(), Vec::new());
    // Groups of words split after punctuation («Вузол два, | прат»)
    fn groups<'a>(ws: &[&'a str]) -> Vec<Vec<&'a str>> {
        let mut out: Vec<Vec<&str>> = vec![Vec::new()];
        for w in ws {
            out.last_mut().unwrap().push(*w);
            if w.ends_with([',', '.', ';', ':', '!', '?']) {
                out.push(Vec::new());
            }
        }
        out.retain(|g| !g.is_empty());
        out
    }
    let flush = |del: &mut Vec<&str>, ins: &mut Vec<&str>, out: &mut Vec<(String, String)>| {
        let (gd, gi) = (groups(del), groups(ins));
        let parts: Vec<(Vec<&str>, Vec<&str>)> = if gd.len() == gi.len() && gd.len() > 1 { gd.into_iter().zip(gi).collect() } else { vec![(del.clone(), ins.clone())] };
        for (d, n) in parts {
            if (1..=3).contains(&d.len()) && (1..=3).contains(&n.len()) {
                let wrong = d.iter().map(|w| bare(w)).collect::<Vec<_>>().join(" ");
                let right = n.iter().map(|w| bare(w)).collect::<Vec<_>>().join(" ");
                if !wrong.is_empty() && !right.is_empty() && wrong != right {
                    out.push((wrong, right));
                }
            }
        }
        del.clear();
        ins.clear();
    };
    while i < n || j < m {
        if i < n && j < m && bare(aw[i]) == bare(bw[j]) {
            flush(&mut del, &mut ins, &mut out);
            i += 1;
            j += 1;
        } else if j < m && (i == n || dp[i][j + 1] >= dp[i + 1][j]) {
            ins.push(bw[j]);
            j += 1;
        } else {
            del.push(aw[i]);
            i += 1;
        }
    }
    flush(&mut del, &mut ins, &mut out);
    out
}

const SEEN_FILE: &str = "corrections_seen.json";

/// Counts how often each «wrong → right» was corrected by hand. Returns, for
/// each pair, whether it has now been made at least twice (then it is
/// remembered without asking).
pub fn note_seen(pairs: &[(String, String)]) -> Vec<bool> {
    let path = corrections::data_file(SEEN_FILE);
    let mut seen: HashMap<String, usize> = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let out = pairs
        .iter()
        .map(|(w, r)| {
            let n = seen.entry(format!("{} => {}", w.to_lowercase(), r)).or_default();
            *n += 1;
            *n >= 2
        })
        .collect();
    if let Some(p) = path {
        let _ = std::fs::write(p, serde_json::to_string(&seen).unwrap_or_default());
    }
    out
}

fn seen_map() -> HashMap<String, usize> {
    corrections::data_file(SEEN_FILE).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn refused_key(wrong: &str, right: &str) -> String {
    format!("! {} => {}", wrong.to_lowercase(), right)
}

/// «Скасувати» on a rule learned from a fix in the text: the same fix does not teach it again
pub fn refuse(wrong: &str, right: &str) {
    let mut seen = seen_map();
    seen.remove(&format!("{} => {}", wrong.to_lowercase(), right));
    seen.insert(refused_key(wrong, right), 1);
    if let Some(p) = corrections::data_file(SEEN_FILE) {
        let _ = std::fs::write(p, serde_json::to_string(&seen).unwrap_or_default());
    }
}

fn added_key(term: &str) -> String {
    format!("+ {term}")
}

/// A rule from a fix in the text put this term in the dictionary (it was not there)
pub fn mark_added_by_rule(term: &str) {
    let mut seen = seen_map();
    seen.insert(added_key(term), 1);
    if let Some(p) = corrections::data_file(SEEN_FILE) {
        let _ = std::fs::write(p, serde_json::to_string(&seen).unwrap_or_default());
    }
}

pub fn added_by_rule(term: &str) -> bool {
    seen_map().contains_key(&added_key(term))
}

pub fn clear_added_by_rule(term: &str) {
    let mut seen = seen_map();
    if seen.remove(&added_key(term)).is_some() {
        if let Some(p) = corrections::data_file(SEEN_FILE) {
            let _ = std::fs::write(p, serde_json::to_string(&seen).unwrap_or_default());
        }
    }
}

pub fn refused(wrong: &str, right: &str) -> bool {
    seen_map().contains_key(&refused_key(wrong, right))
}

/// Remembers corrections the user confirmed: the right spelling joins the
/// dictionary (and so Whisper's prompt), the wrong one is replaced from now on
pub fn remember(pairs: &[(String, String)], example: &str) -> Result<usize, String> {
    let mut n = 0;
    for (wrong, right) in pairs {
        learn(right, "edit", example, Some(wrong))?;
        n += 1;
    }
    add_corrections(pairs)?;
    Ok(n)
}

/// Learns from a document the secretary edited: corrections for what the
/// app got wrong, and the right spellings in the glossary.
pub fn learn_from_edit(before: &str, after: &str) -> usize {
    let pairs: Vec<(String, String)> = substitutions(before, after)
        .into_iter()
        // «Доповідач 3» → «Олег» names a speaker; it is not a misspelling
        .filter(|(wrong, _)| !wrong.starts_with("Доповідач") && !wrong.starts_with("Фрагмент"))
        .collect();
    let mut learned = 0;
    for (wrong, right) in &pairs {
        let example = after.lines().find(|l| l.contains(right.as_str())).unwrap_or("").trim().to_string();
        if learn(right, "edit", &example, Some(wrong)).is_ok() {
            learned += 1;
        }
    }
    let corrections: Vec<(String, String)> =
        pairs.into_iter().filter(|(w, r)| !corrections::same_word_forms(w, r)).collect();
    if !corrections.is_empty() && add_corrections(&corrections).is_ok() {
        for (wrong, right) in &corrections {
            crate::learned_events::note("word", wrong, right, 1, None);
        }
    }
    // Speaker names given by hand (instead of «Доповідач N») are names too
    for (wrong, right) in substitutions(before, after) {
        if wrong.starts_with("Доповідач") && right.chars().next().is_some_and(char::is_uppercase) {
            let _ = learn(&right, "edit", "", None);
            learned += 1;
        }
    }
    learned
}

/// Words that most often stand right after a term: a hint at its role
/// («Вузол-2» → «зв'язок»)
fn auto_context(contexts: &[String], term: &str) -> String {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for ctx in contexts {
        let ws: Vec<&str> = ctx.split_whitespace().collect();
        if let Some(pos) = ws.iter().position(|w| bare(w) == term) {
            for w in ws.iter().skip(pos + 1).take(3) {
                let w = bare(w).to_lowercase();
                if w.chars().count() >= 4 && !CONTEXT_STOPWORDS.contains(&w.as_str()) && w.chars().all(|c| c.is_alphabetic() || c == '\'' || c == '’') {
                    *counts.entry(w).or_default() += 1;
                }
            }
        }
    }
    let mut best: Vec<(String, usize)> = counts.into_iter().filter(|(_, n)| *n >= 2).collect();
    best.sort_by_key(|(w, n)| (std::cmp::Reverse(*n), w.clone()));
    best.into_iter().take(2).map(|(w, _)| w).collect::<Vec<_>>().join(", ")
}

/// Scans meeting transcripts and fills the dictionary on its own: names,
/// nicknames, units, abbreviations and places seen in several meetings (or
/// often), with a context hint. Where one name is spelled in different ways,
/// the most frequent spelling becomes the default and the others are
/// corrected; the secretary can still pick another one in «Словник».
/// Entries the secretary corrected are never overridden.
pub fn auto_learn(meetings: &[String]) -> Vec<String> {
    let dismissed: HashSet<String> = corrections::dismissed().into_iter().map(|d| d.to_lowercase()).collect();
    let mut meta = load_meta();
    let mut in_meetings: HashMap<String, HashSet<usize>> = HashMap::new();
    let mut occurrences: HashMap<String, usize> = HashMap::new();
    let mut contexts: HashMap<String, Vec<String>> = HashMap::new();
    for (index, text) in meetings.iter().enumerate() {
        for (word, context) in corrections::candidates(text) {
            in_meetings.entry(word.clone()).or_default().insert(index);
            *occurrences.entry(word.clone()).or_default() += 1;
            let list = contexts.entry(word).or_default();
            if list.len() < 30 {
                list.push(context);
            }
        }
    }
    let frequent: Vec<String> = in_meetings
        .keys()
        .filter(|w| {
            !dismissed.contains(&w.to_lowercase())
                && (in_meetings[*w].len() >= AUTO_LEARN_MEETINGS || occurrences[*w] >= AUTO_LEARN_OCCURRENCES)
        })
        .cloned()
        .collect();

    // Spellings of what looks like one name: the most frequent one wins
    let mut corrections_to_add: Vec<(String, String)> = Vec::new();
    let mut learned = Vec::new();
    let glossary_lines = super::glossary();
    let edited: HashSet<String> = meta.iter().filter(|(_, m)| m.source == "edit").map(|(k, _)| k.clone()).collect();
    for word in &frequent {
        let lower = word.to_lowercase();
        let rivals: Vec<&String> = in_meetings
            .keys()
            .filter(|other| {
                *other != word && !corrections::same_word_forms(other, word) && {
                    let len = other.chars().count().max(word.chars().count());
                    len >= 4 && levenshtein(&other.to_lowercase(), &lower) <= len / 3
                }
            })
            .collect();
        let my_count = occurrences[word];
        if rivals.iter().any(|r| occurrences[*r] > my_count || (occurrences[*r] == my_count && r.as_str() < word.as_str())) {
            continue; // a more frequent spelling wins
        }
        if rivals.iter().any(|r| edited.contains(r.as_str())) {
            continue; // the secretary chose another spelling
        }
        for rival in &rivals {
            if !edited.contains(rival.as_str()) {
                corrections_to_add.push((rival.to_string(), word.clone()));
            }
        }
        let context = auto_context(contexts.get(word).map(Vec::as_slice).unwrap_or(&[]), word);
        let already = glossary_lines.iter().any(|l| super::term_of(l) == word);
        let entry = meta.entry(word.clone()).or_insert_with(|| TermMeta { source: "auto".into(), ..Default::default() });
        entry.count = entry.count.max(my_count);
        entry.meetings = in_meetings[word].len();
        entry.last_seen = today();
        if entry.example.is_empty() {
            entry.example = contexts.get(word).and_then(|c| c.first()).cloned().unwrap_or_default().chars().take(120).collect();
        }
        if !already {
            learned.push(if context.is_empty() { word.clone() } else { format!("{word} — {context}") });
        }
    }
    if !learned.is_empty() {
        let mut glossary = super::glossary();
        glossary.extend(learned.iter().cloned());
        let _ = super::set_glossary(glossary);
    }
    let _ = save_meta(&meta);
    if !corrections_to_add.is_empty() {
        let existing: HashSet<String> = corrections::corrections().into_iter().map(|(w, _)| w).collect();
        let fresh: Vec<(String, String)> = corrections_to_add.into_iter().filter(|(w, _)| !existing.contains(w)).collect();
        let _ = add_corrections(&fresh);
    }
    learned
}

/// Learned entries for «Словник»
pub fn learned_terms() -> Vec<LearnedTerm> {
    let meta = load_meta();
    let mut out: Vec<LearnedTerm> = super::glossary()
        .iter()
        .filter_map(|line| {
            let term = super::term_of(line).to_string();
            let m = meta.get(&term)?.clone();
            let context = line.splitn(2, " — ").nth(1).unwrap_or("").trim().to_string();
            Some(LearnedTerm { term, context, meta: m })
        })
        .collect();
    out.sort_by_key(|t| std::cmp::Reverse(t.meta.count));
    out
}

/// Removes a learned entry: from the glossary, its corrections and its record
pub fn forget(term: &str) -> Result<(), String> {
    let glossary: Vec<String> = super::glossary().into_iter().filter(|l| super::term_of(l) != term).collect();
    super::set_glossary(glossary)?;
    let kept: Vec<String> = read_lines(corrections::CORRECTIONS_FILE)
        .into_iter()
        .filter(|l| l.split_once("=>").map(|(_, r)| r.trim() != term).unwrap_or(true))
        .collect();
    write_lines(corrections::CORRECTIONS_FILE, &kept)?;
    let mut meta = load_meta();
    meta.remove(term);
    save_meta(&meta)
}

#[cfg(test)]
mod tests {
    #[test]
    fn pairs_from_a_corrected_line() {
        let p = super::edit_pairs("Доповідає Вузол два, прат на місці, так.", "Доповідає Вузол-2, ПрАТ на місці. Так.");
        assert_eq!(p, vec![("Вузол два".to_string(), "Вузол-2".to_string()), ("прат".to_string(), "ПрАТ".to_string())]);
        // A common word or a plain case change is not learned
        assert!(super::edit_pairs("так, зрозумів", "Так, зрозумів").is_empty());
        assert!(super::edit_pairs("отримали пальне", "Отримали пальне").is_empty());
        // A misheard surname is
        assert_eq!(super::edit_pairs("слово має Кончаренко", "слово має Гончаренко"), vec![("Кончаренко".to_string(), "Гончаренко".to_string())]);
    }

    use super::*;

    #[test]
    fn edits_give_name_substitutions() {
        let before = "**Доповідач 3** (05:50):\nДоповідає Кончаренко про зв'язок.";
        let after = "**Олег** (05:50):\nДоповідає Гончаренко про зв'язок.";
        let subs = substitutions(before, after);
        assert!(subs.contains(&("Кончаренко".to_string(), "Гончаренко".to_string())));
        assert!(subs.contains(&("Доповідач 3".to_string(), "Олег".to_string())));
    }

    #[test]
    fn rewording_is_not_learned() {
        // A long rewrite or a lowercase rewording is not a name correction
        let subs = substitutions("Ми зробили це вчора ввечері швидко.", "Це виконано.");
        assert!(subs.is_empty());
    }

    #[test]
    fn context_comes_from_neighbouring_words() {
        let ctx = vec!["доповідає Вузол зв'язок працює".to_string(), "далі Вузол зв'язок ретранслятори".to_string()];
        assert_eq!(auto_context(&ctx, "Вузол"), "зв'язок");
    }

    #[test]
    fn unequal_paragraphs_are_skipped() {
        assert!(substitutions("a\nb", "a").is_empty());
    }
}
