//! Words Whisper was not sure of, and what can be done about them on the
//! Mac without guessing:
//! - a word is *doubtful* when its least sure token was below a threshold;
//!   numbers and capitalised words in the middle of a sentence (names, call
//!   signs) that are not in the dictionary are held to a higher bar;
//! - a doubtful word that sounds like a dictionary term (a confirmed name, a
//!   role, a glossary entry) is replaced by it, keeping the original (`auto`);
//!   declined forms of the same word are left alone («Гончаренка» is not an
//!   error for «Гончаренко»).
//! The user checks what is left in «Перевірте».

/// Below this a word is doubtful (tuned on a 37-minute meeting: Whisper is
/// sure of most words, median 1.0; below 0.85 are about 3.5% of words)
pub const DOUBTFUL_P: f32 = 0.85;
/// Numbers and names outside the dictionary are doubtful below this
pub const DOUBTFUL_P_SPECIAL: f32 = 0.95;
/// How alike two spellings must sound to replace one with the other
pub const SNAP_SIMILARITY: f64 = 0.8;

fn bare(w: &str) -> &str {
    w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’' && c != '-')
}

fn has_digit(w: &str) -> bool {
    w.chars().any(|c| c.is_ascii_digit())
}

/// Which words of a piece are doubtful. `known` says whether a word is in
/// the dictionary (or a confirmed name/role).
pub fn doubtful(words: &[String], probs: &[f32], known: &dyn Fn(&str) -> bool) -> Vec<bool> {
    let mut out = Vec::with_capacity(words.len());
    for (i, w) in words.iter().enumerate() {
        let p = probs.get(i).copied().unwrap_or(1.0);
        let word = bare(w);
        if word.is_empty() {
            out.push(false);
            continue;
        }
        let sentence_start = i == 0 || words[i - 1].ends_with(['.', '!', '?', '…']);
        let name_like = !sentence_start && word.chars().next().is_some_and(char::is_uppercase) && !known(word);
        let special = has_digit(word) || name_like;
        out.push(p < DOUBTFUL_P || (special && p < DOUBTFUL_P_SPECIAL));
    }
    out
}

/// A rough sound of a word: lower case, Latin letters read as Ukrainian,
/// letters that are easily misheard merged, soft signs, apostrophes and
/// doubled letters dropped
pub fn sound_of(word: &str) -> String {
    let lower = bare(word).to_lowercase();
    let mut s = String::new();
    let chars: Vec<char> = lower.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let mapped: &str = match (c, next) {
            ('s', Some('h')) => {
                i += 1;
                "ш"
            }
            ('c', Some('h')) | ('t', Some('s')) if c == 't' => {
                i += 1;
                "ц"
            }
            ('c', Some('h')) => {
                i += 1;
                "ч"
            }
            ('z', Some('h')) => {
                i += 1;
                "ж"
            }
            ('k', Some('h')) => {
                i += 1;
                "г"
            }
            _ => match c {
                'a' => "а",
                'b' => "б",
                'v' | 'w' => "в",
                'g' | 'h' | 'ґ' | 'х' => "г",
                'd' => "д",
                'e' | 'є' | 'э' => "е",
                'z' => "з",
                'i' | 'y' | 'и' | 'ї' | 'й' | 'ы' => "і",
                'k' | 'q' => "к",
                'l' => "л",
                'm' => "м",
                'n' => "н",
                'o' => "о",
                'p' => "п",
                'r' => "р",
                's' => "с",
                't' => "т",
                'u' => "у",
                'f' => "ф",
                'c' => "к",
                'j' => "й",
                'x' => "кс",
                'ь' | '\'' | '’' | 'ъ' | '-' => "",
                'ю' => "у",
                'я' => "а",
                'щ' => "ш",
                _ => {
                    s.push(c);
                    ""
                }
            },
        };
        s.push_str(mapped);
        i += 1;
    }
    // Doubled letters sound like one
    let mut out = String::new();
    for c in s.chars() {
        if !out.ends_with(c) {
            out.push(c);
        }
    }
    out
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// How alike two words sound, 0..1
pub fn sound_similarity(a: &str, b: &str) -> f64 {
    let (x, y): (Vec<char>, Vec<char>) = (sound_of(a).chars().collect(), sound_of(b).chars().collect());
    let longest = x.len().max(y.len());
    if longest == 0 {
        return 0.0;
    }
    1.0 - levenshtein(&x, &y) as f64 / longest as f64
}

/// The same word in another case or number («Гончаренка» / «Гончаренко»):
/// all but the last two letters agree
fn same_stem(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (bare(a).to_lowercase().chars().collect(), bare(b).to_lowercase().chars().collect());
    let n = a.len().min(b.len());
    n >= 4 && a[..n - 2] == b[..n - 2]
}

/// The dictionary term a doubtful word was probably meant to be, if one
/// sounds alike enough (and is not just another form of the same word)
pub fn snap<'a>(word: &str, terms: &'a [String]) -> Option<&'a str> {
    let w = bare(word);
    if w.chars().count() < 4 || has_digit(w) {
        return None;
    }
    let mut best: Option<(&str, f64)> = None;
    for t in terms {
        if t.split_whitespace().count() != 1 || t.chars().count() < 4 {
            continue;
        }
        if t.to_lowercase() == w.to_lowercase() {
            return None;
        }
        // Spelled differently but sounds the same: the dictionary spelling
        if sound_of(w) == sound_of(t) {
            return Some(t.as_str());
        }
        if same_stem(w, t) {
            return None;
        }
        let sim = sound_similarity(w, t);
        if sim >= SNAP_SIMILARITY && best.map_or(true, |(_, b)| sim > b) {
            best = Some((t.as_str(), sim));
        }
    }
    best.map(|(t, _)| t)
}

/// Puts a term in place of a word, keeping the punctuation around it
pub fn replace_keeping_punctuation(word: &str, term: &str) -> String {
    let core = bare(word);
    match word.find(core) {
        Some(at) => format!("{}{}{}", &word[..at], term, &word[at + core.len()..]),
        None => term.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn doubtful_words() {
        let words = s(&["Доповідає", "Кончаренко,", "отримали", "12", "генераторів."]);
        let probs = [0.99, 0.6, 0.3, 0.65, 0.97];
        let known = |w: &str| w == "Гончаренко";
        // A name outside the dictionary, an unsure plain word, an unsure number
        assert_eq!(doubtful(&words, &probs, &known), vec![false, true, true, true, false]);
        // A known name at 0.9 is fine; an unknown one is held to a higher bar
        let words = s(&["слово", "має", "Гончаренко."]);
        assert_eq!(doubtful(&words, &[0.99, 0.99, 0.9], &known), vec![false, false, false]);
        let words = s(&["слово", "має", "Остапенко."]);
        assert_eq!(doubtful(&words, &[0.99, 0.99, 0.9], &known), vec![false, false, true]);
    }

    #[test]
    fn misheard_names_snap_to_the_dictionary() {
        let terms = s(&["Гончаренко", "Вузол-2", "Сокіл", "ПрАТ"]);
        assert_eq!(snap("Кончаренко,", &terms), Some("Гончаренко"));
        assert_eq!(snap("Сокил", &terms), Some("Сокіл"));
        // Latin spelling of the same sound
        assert_eq!(snap("Honcharenko", &terms), Some("Гончаренко"));
        // Another case of the same name is not an error
        assert_eq!(snap("Гончаренка", &terms), None);
        // Nothing alike
        assert_eq!(snap("генератор", &terms), None);
        assert_eq!(replace_keeping_punctuation("«Кончаренко»,", "Гончаренко"), "«Гончаренко»,");
    }

    #[test]
    fn sounds() {
        assert_eq!(sound_of("Сокіл"), sound_of("Сокил"));
        assert_eq!(sound_of("Ґрім"), sound_of("Грім"));
        assert!(sound_similarity("Кончаренко", "Гончаренко") >= 0.8);
        assert!(sound_similarity("склад", "Сокіл") < 0.8);
    }
}

/// A word the secretary may mean instead of the one written
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Alternative {
    pub text: String,
    /// «почуто» (what recognition heard), «виправляли раніше», «словник»
    pub why: String,
}

/// Options for a word in «Текст»: what was heard, what the user put
/// instead before, and dictionary terms that sound alike (at most 5)
pub fn alternatives(word: &str, heard: Option<&str>, terms: &[String], corrections: &[(String, String)]) -> Vec<Alternative> {
    let w = bare(word);
    let mut out: Vec<Alternative> = Vec::new();
    let mut push = |text: &str, why: &str, out: &mut Vec<Alternative>| {
        let t = text.trim();
        if t.is_empty() || t.eq_ignore_ascii_case(w) || t.to_lowercase() == w.to_lowercase() || out.iter().any(|a| a.text.to_lowercase() == t.to_lowercase()) {
            return;
        }
        out.push(Alternative { text: t.to_string(), why: why.to_string() });
    };
    if let Some(h) = heard {
        push(bare(h), "почуто", &mut out);
    }
    for (wrong, right) in corrections {
        if wrong.to_lowercase() == w.to_lowercase() {
            push(right, "виправляли раніше", &mut out);
        }
    }
    let mut alike: Vec<(&String, f64)> = terms
        .iter()
        .filter(|t| t.split_whitespace().count() == 1)
        .map(|t| (t, sound_similarity(w, t)))
        .filter(|(_, s)| *s >= 0.6)
        .collect();
    alike.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (t, _) in alike.into_iter().take(3) {
        push(t, "словник", &mut out);
    }
    out.truncate(5);
    out
}

#[cfg(test)]
mod alternative_tests {
    use super::*;

    #[test]
    fn options_for_a_word() {
        let terms: Vec<String> = ["Гончаренко", "Сокіл", "генератор"].iter().map(|s| s.to_string()).collect();
        let corrections = vec![("кончаренко".to_string(), "Гончаренко".to_string())];
        let alts = alternatives("Кончаренко,", Some("Кончеренко"), &terms, &corrections);
        assert_eq!(alts[0], Alternative { text: "Кончеренко".into(), why: "почуто".into() });
        assert_eq!(alts[1], Alternative { text: "Гончаренко".into(), why: "виправляли раніше".into() });
        // The dictionary term is not repeated
        assert_eq!(alts.len(), 2);
        assert!(alternatives("Сокил", None, &terms, &[]).iter().any(|a| a.text == "Сокіл" && a.why == "словник"));
        assert!(alternatives("склад", None, &terms, &[]).is_empty());
    }
}
