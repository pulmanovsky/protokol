//! Name suggestions from what was said, without a language model:
//! - a self-introduction at the start of a turn («Доброго ранку, КЗП…»,
//!   «Це Олена…», «Доповідає зв'язок» is not taken: lowercase);
//! - a direct address right before the turn («слово має Петренко»,
//!   «Тарас, кажіть»).
//! Precision over recall: a name is suggested only from a clear pattern,
//! only once per name, and never when the evidence for a voice disagrees.
//! Suggestions are shown as «Ім'я (?)» until the user confirms them.

use std::collections::HashMap;

const GREETINGS: &[&str] = &[
    "доброго ранку", "добрий ранок", "добрий день", "доброго дня", "добрий вечір", "доброго вечора", "вітаю", "всім вітаю",
];
/// Words after a greeting that are not a name
const NOT_NAMES: &[&str] = &[
    "всім", "всіх", "панове", "пане", "пані", "колеги", "шановні", "друзі", "я", "це", "так", "ну", "добре", "дякую",
    "товариші", "всі", "ще", "раз", "отже", "значить", "без",
];
fn is_not_name(word: &str) -> bool {
    NOT_NAMES.contains(&word) || crate::summary::pack::current().not_names.iter().any(|n| n == word)
}

/// Words that follow a direct address («Тарас, кажіть»)
const ADDRESS_FOLLOWERS: &[&str] = &[
    "кажіть", "кажи", "доповідайте", "доповідай", "давай", "давайте", "прошу", "тобі слово", "вам слово", "слухаємо", "що у вас",
    "що в тебе", "що у тебе", "ваша черга",
];

/// A turn of a speaker with its first and last words
pub struct TurnText<'a> {
    pub speaker: usize,
    pub text: &'a str,
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’' && c != '-').to_string()).filter(|w| !w.is_empty()).collect()
}

/// A token that can be a name or a unit: capitalised, or an abbreviation in
/// capitals («КЗП», «С4»), not a common word
fn name_like(w: &str) -> bool {
    let first = w.chars().next();
    let lower = w.to_lowercase();
    first.is_some_and(|c| c.is_uppercase()) && w.chars().count() >= 2 && !is_not_name(&lower)
}

/// Vocative forms («Петре», «Олено», «Андрію») are not suggested: they are
/// not how a name is written. Where a name is addressed or greeted, «-о»
/// counts too («Олено»); after «слово має», «це» the case is nominative and
/// «Петренко», «Остап» are fine.
fn looks_vocative(w: &str) -> bool {
    vocative_with(w, &["е", "о", "ю", "у"])
}

fn looks_vocative_in_nominative(w: &str) -> bool {
    vocative_with(w, &["е", "ю", "у"])
}

fn vocative_with(w: &str, endings: &[&str]) -> bool {
    let lower = w.to_lowercase();
    !w.chars().all(|c| c.is_uppercase() || c.is_ascii_digit()) && endings.iter().any(|e| lower.ends_with(e))
}

/// Name after a greeting at the start of a turn: «Доброго ранку, КЗП.»
fn self_introduction(text: &str) -> Option<String> {
    let head: String = text.split_whitespace().take(10).collect::<Vec<_>>().join(" ");
    let lower = head.to_lowercase();
    // «Це Олена», «Я Олена»
    let ws = words(&head);
    if ws.len() >= 2 && matches!(ws[0].to_lowercase().as_str(), "це" | "я") && name_like(&ws[1]) && !looks_vocative_in_nominative(&ws[1]) {
        return Some(ws[1].clone());
    }
    let greeting = GREETINGS.iter().filter_map(|g| lower.find(g).map(|i| (i, g))).min_by_key(|(i, _)| *i)?;
    if greeting.0 > 20 {
        return None;
    }
    // The words right after the greeting, up to the first sentence end
    let rest = &head[greeting.0 + greeting.1.len()..];
    let clause = rest.split(['.', '!', '?']).next().unwrap_or("");
    let after: Vec<String> = words(clause);
    // Skip «шановні колеги», «всім» and the like
    let mut i = 0;
    while i < after.len() && is_not_name(&after[i].to_lowercase()) {
        i += 1;
    }
    let candidate = after.get(i)?;
    // Only a single clear token right after the greeting, ending the clause
    // or followed by another capitalised word («Медичний Пункт» is rare)
    if !name_like(candidate) || looks_vocative(candidate) || after.len() > i + 2 {
        return None;
    }
    let mut name = candidate.clone();
    if let Some(next) = after.get(i + 1) {
        if name_like(next) && !looks_vocative(next) {
            name = format!("{name} {next}");
        } else {
            return None;
        }
    }
    Some(name)
}

/// Name addressed in the last words of the previous turn:
/// «…слово має Петренко», «…Тарас, кажіть»
fn address_before(previous: &str) -> Option<String> {
    let tail: Vec<&str> = previous.split_whitespace().collect();
    let tail = tail[tail.len().saturating_sub(10)..].join(" ");
    let lower = tail.to_lowercase();
    for cue in ["слово має", "слово надається", "доповідає"] {
        if let Some(i) = lower.rfind(cue) {
            let after = words(&tail[i + cue.len()..]);
            let candidate = after.first()?;
            return (name_like(candidate) && !looks_vocative_in_nominative(candidate) && after.len() <= 2).then(|| candidate.clone());
        }
    }
    // «Тарас, кажіть.» — the address and its follower end the turn
    let sentences: Vec<&str> = tail.split(['.', '!', '?']).map(str::trim).filter(|s| !s.is_empty()).collect();
    let last = sentences.last()?;
    let (name, follower) = last.split_once(',')?;
    let follower = follower.trim().to_lowercase();
    let name_words = words(name);
    if name_words.len() != 1 || !ADDRESS_FOLLOWERS.iter().any(|f| follower.starts_with(f)) {
        return None;
    }
    let candidate = &name_words[0];
    (name_like(candidate) && !looks_vocative(candidate)).then(|| candidate.clone())
}

/// Suggested names per speaker. A speaker gets a name only when all its
/// evidence agrees and no other speaker has the same suggestion.
pub fn suggest(turns: &[TurnText], skip: &dyn Fn(usize) -> bool) -> HashMap<usize, String> {
    let mut evidence: HashMap<usize, Vec<String>> = HashMap::new();
    for (i, t) in turns.iter().enumerate() {
        if skip(t.speaker) {
            continue;
        }
        if let Some(n) = self_introduction(t.text) {
            evidence.entry(t.speaker).or_default().push(n);
        }
        if i > 0 && turns[i - 1].speaker != t.speaker {
            if let Some(n) = address_before(turns[i - 1].text) {
                evidence.entry(t.speaker).or_default().push(n);
            }
        }
    }
    let mut chosen: HashMap<usize, String> = HashMap::new();
    for (speaker, names) in &evidence {
        let first = names[0].to_lowercase();
        if names.iter().all(|n| n.to_lowercase() == first) {
            chosen.insert(*speaker, names[0].clone());
        }
    }
    // The same name for two voices: none of them gets it
    let mut count: HashMap<String, usize> = HashMap::new();
    for n in chosen.values() {
        *count.entry(n.to_lowercase()).or_default() += 1;
    }
    chosen.retain(|_, n| count.get(&n.to_lowercase()) == Some(&1));
    chosen
}

/// How a suggested, not yet confirmed name is shown
pub fn unconfirmed(name: &str) -> String {
    format!("{name} (?)")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(speaker: usize, text: &str) -> TurnText<'_> {
        TurnText { speaker, text }
    }

    #[test]
    fn introductions_and_addresses() {
        assert_eq!(self_introduction("Доброго ранку, КЗП. По складу без змін."), Some("КЗП".into()));
        assert_eq!(self_introduction("Доброго ранку, шановні колеги, С4. Було надіслано звіт."), Some("С4".into()));
        assert_eq!(self_introduction("Це Олена, доповідаю по складу."), Some("Олена".into()));
        // Not names: a greeting to everyone, a long sentence, a vocative
        assert_eq!(self_introduction("Доброго ранку всім. Доповідаю."), None);
        assert_eq!(self_introduction("Доброго ранку, Петре."), None);
        assert_eq!(self_introduction("Доброго ранку, ми вчора закінчили роботи на складі."), None);
        assert_eq!(address_before("Дякую, давайте далі. Тарас, кажіть."), Some("Тарас".into()));
        assert_eq!(address_before("Так, слово має Петренко."), Some("Петренко".into()));
        assert_eq!(address_before("Андрію, тобі слово."), None);
        assert_eq!(address_before("Дякую, доповідь закінчив."), None);
    }

    #[test]
    fn suggestions_need_agreement_and_unique_names() {
        let turns = vec![
            t(0, "Почнемо. Тарас, кажіть."),
            t(1, "Доброго ранку, КЗП. Все без змін."),
            t(0, "Дякую. Слово має Петренко."),
            t(2, "Доброго ранку. Доповідаю."),
            t(0, "Далі. Олена, доповідайте."),
            t(3, "Доброго ранку, Склад. Все добре."),
        ];
        let s = suggest(&turns, &|_| false);
        // Speaker 1: «Тарас» from the address but «КЗП» from the greeting: no name
        assert!(!s.contains_key(&1));
        assert_eq!(s.get(&2).map(String::as_str), Some("Петренко"));
        // Speaker 3: «Олена» and «Склад» disagree
        assert!(!s.contains_key(&3));
        // A voice that already has a name is skipped
        let s2 = suggest(&turns, &|k| k == 2);
        assert!(!s2.contains_key(&2));
    }
}
