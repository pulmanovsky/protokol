//! Ukrainian spelling for what Whisper writes down.
//!
//! People in meetings speak Ukrainian mixed with surzhyk, Russian words and
//! anglicisms. The transcript keeps the words as they were said, but always
//! in the Ukrainian alphabet:
//!
//! - letters that Ukrainian does not have are replaced deterministically:
//!   ы → и, э → е, ъ → ', ё → йо / ьо / о depending on the letter before it
//!   («всё» → «всьо», «моё» → «мойо», «ещё» → «ещо»). Other letters are
//!   left alone, so the word keeps the sound that was said;
//! - common anglicisms that Whisper wrote in Latin letters are written in
//!   Cyrillic the way they are said («deadline» → «дедлайн»);
//! - words from the user's dictionary are never changed, so brand and tech
//!   names kept in Latin there stay in Latin.

use std::collections::HashSet;

/// Anglicisms and everyday tech names as they are said in Ukrainian meetings.
/// Only these Latin words are rewritten; any other Latin word is kept.
const ANGLICISMS: &[(&str, &str)] = &[
    ("deadline", "дедлайн"),
    ("deadlines", "дедлайни"),
    ("feedback", "фідбек"),
    ("update", "апдейт"),
    ("updates", "апдейти"),
    ("meeting", "мітинг"),
    ("online", "онлайн"),
    ("offline", "офлайн"),
    ("okay", "окей"),
    ("ok", "окей"),
    ("chat", "чат"),
    ("report", "репорт"),
    ("briefing", "брифінг"),
    ("task", "таск"),
    ("tasks", "таски"),
    ("zoom", "зум"),
    ("google", "гугл"),
    ("excel", "ексель"),
    ("telegram", "телеграм"),
    ("viber", "вайбер"),
    ("whatsapp", "вотсап"),
    ("starlink", "старлінк"),
];

fn is_vowel(c: char) -> bool {
    matches!(c.to_lowercase().next(), Some('а' | 'е' | 'є' | 'и' | 'і' | 'ї' | 'о' | 'у' | 'ю' | 'я' | 'ы' | 'э' | 'ё'))
}

fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '’' | 'ʼ')
}

fn is_word_char(c: char) -> bool {
    c.is_alphabetic() || is_apostrophe(c)
}

fn is_latin(c: char) -> bool {
    c.is_ascii_alphabetic()
}

fn has_russian_letters(word: &str) -> bool {
    word.chars().any(|c| matches!(c, 'ы' | 'Ы' | 'э' | 'Э' | 'ъ' | 'Ъ' | 'ё' | 'Ё'))
}

/// «ПРИВЕТ» is all caps, «Привет» and «О» are not
fn is_all_caps(word: &str) -> bool {
    let letters: Vec<char> = word.chars().filter(|c| c.is_alphabetic()).collect();
    letters.len() > 1 && letters.iter().all(|c| c.is_uppercase())
}

/// Applies the casing of `source` (a single letter) to `replacement`
fn with_case(replacement: &str, upper: bool, all_caps: bool) -> String {
    if all_caps {
        replacement.to_uppercase()
    } else if upper {
        let mut chars = replacement.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().chain(chars).collect(),
            None => String::new(),
        }
    } else {
        replacement.to_string()
    }
}

/// Rewrites the Russian-only letters of one Cyrillic word
fn ukrainize_word(word: &str) -> String {
    let all_caps = is_all_caps(word);
    let chars: Vec<char> = word.chars().collect();
    let mut out = String::with_capacity(word.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        let upper = c.is_uppercase();
        let lower = c.to_lowercase().next().unwrap_or(c);
        let prev = if i > 0 { chars[i - 1].to_lowercase().next() } else { None };
        match lower {
            'ы' => out.push_str(&with_case("и", upper, all_caps)),
            'э' => out.push_str(&with_case("е", upper, all_caps)),
            'ъ' => out.push('\''),
            'ё' => {
                let replacement = match prev {
                    // Start of word, after a vowel, a soft sign or an apostrophe: «йо»
                    None => "йо",
                    Some(p) if is_vowel(p) || p == 'ь' || p == 'ъ' || is_apostrophe(p) => "йо",
                    // After hushing consonants Ukrainian writes plain «о» («шо», «чорт»)
                    Some('ж' | 'ч' | 'ш' | 'щ') => "о",
                    // After any other consonant: «ьо» («всьо», «льон»)
                    Some(_) => "ьо",
                };
                out.push_str(&with_case(replacement, upper, all_caps));
            }
            _ => out.push(c),
        }
    }
    out
}

/// Writes a Latin anglicism in Cyrillic, or returns None to keep it as is.
/// Abbreviations in capitals («OK», «CRM») are kept.
fn anglicism(word: &str) -> Option<String> {
    if is_all_caps(word) {
        return None;
    }
    let lower = word.to_lowercase();
    let (_, cyrillic) = ANGLICISMS.iter().find(|(latin, _)| *latin == lower)?;
    let upper = word.chars().next().is_some_and(char::is_uppercase);
    Some(with_case(cyrillic, upper, false))
}

/// Words of the dictionary that must stay exactly as written there
pub fn protected_words(terms: &[String]) -> HashSet<String> {
    let mut words = HashSet::new();
    for term in terms {
        let mut current = String::new();
        for c in term.chars().chain(std::iter::once(' ')) {
            if is_word_char(c) {
                current.push(c);
            } else if !current.is_empty() {
                words.insert(current.to_lowercase());
                current.clear();
            }
        }
    }
    words
}

/// Rewrites a transcript in the Ukrainian alphabet, keeping dictionary words.
pub fn ukrainize_with(text: &str, protected: &HashSet<String>) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.is_empty() {
            return;
        }
        let keep = protected.contains(&word.to_lowercase());
        if keep {
            out.push_str(word);
        } else if word.chars().any(is_latin) {
            match anglicism(word) {
                Some(cyrillic) => out.push_str(&cyrillic),
                None => out.push_str(word),
            }
        } else if has_russian_letters(word) {
            out.push_str(&ukrainize_word(word));
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in text.chars() {
        if is_word_char(c) {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// Number of Russian-only letters (ы э ъ ё) in a text
pub fn russian_letter_count(text: &str) -> usize {
    text.chars().filter(|c| matches!(c, 'ы' | 'Ы' | 'э' | 'Э' | 'ъ' | 'Ъ' | 'ё' | 'Ё')).count()
}

/// Whether Whisper output in this language gets the Ukrainian spelling:
/// Ukrainian (the default) and auto-detection, never an explicitly chosen
/// other language such as Russian or English.
pub fn applies_to(language: Option<&str>) -> bool {
    language.is_none() || language == Some(super::DEFAULT_LANGUAGE)
}

/// Ukrainian spelling for a transcription segment, with the current dictionary
pub fn ukrainize(text: &str) -> String {
    ukrainize_with(text, &protected_words(&super::glossary_terms()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uk(text: &str) -> String {
        ukrainize_with(text, &HashSet::new())
    }

    #[test]
    fn russian_only_letters() {
        assert_eq!(uk("Ты что, вообще?"), "Ти что, вообще?");
        assert_eq!(uk("это экран"), "ето екран");
        assert_eq!(uk("Это"), "Ето");
        assert_eq!(uk("объект, подъезд"), "об'ект, под'езд");
        assert_eq!(uk("ВЫШЕ"), "ВИШЕ");
    }

    #[test]
    fn yo_by_position() {
        assert_eq!(uk("всё, ещё, щёлк, жёлтый, чёрт"), "всьо, ещо, щолк, жолтий, чорт");
        assert_eq!(uk("ёлка, моё, бельё, объём"), "йолка, мойо, бельйо, об'йом");
        assert_eq!(uk("Ёлка ЁЖ ВСЁ"), "Йолка ЙОЖ ВСЬО");
    }

    #[test]
    fn ukrainian_text_is_unchanged() {
        let text = "Строк до п'ятниці, виконавець Петренко. Дедлайн — завтра; НБУ, ЦНАП.";
        assert_eq!(uk(text), text);
        assert_eq!(russian_letter_count(&uk("Ты всё сделаешь? Эй, объём!")), 0);
    }

    #[test]
    fn anglicisms_in_latin_become_cyrillic() {
        assert_eq!(uk("Deadline у п'ятницю, дай feedback, Zoom о 10."), "Дедлайн у п'ятницю, дай фідбек, Зум о 10.");
        // Unknown Latin words and abbreviations stay as they are
        assert_eq!(uk("Smart Ops, CRM, OK"), "Smart Ops, CRM, OK");
    }

    #[test]
    fn dictionary_words_are_kept() {
        let protected = protected_words(&["Zoom".to_string(), "Объект «Ёлка»".to_string()]);
        assert_eq!(ukrainize_with("Zoom і zoom, объект Ёлка, всё", &protected), "Zoom і zoom, объект Ёлка, всьо");
    }

    #[test]
    fn language_filter() {
        assert!(applies_to(None));
        assert!(applies_to(Some("uk")));
        assert!(!applies_to(Some("ru")));
        assert!(!applies_to(Some("en")));
    }
}
