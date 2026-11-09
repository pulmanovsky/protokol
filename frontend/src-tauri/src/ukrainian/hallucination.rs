//! Filter for text Whisper invents in silence or noise.
//!
//! Whisper was trained largely on web subtitles. On quiet stretches and noise
//! it tends to return memorised boilerplate (subtitle credits, "music",
//! "thanks for watching"), Russian text in capitals, or an echo of its own
//! initial prompt. None of that was said in the meeting, and it ends up in
//! the protocol if it is kept.

use super::PROMPT_ECHO_WORDS;

/// Fragments of subtitle boilerplate and sound captions (lowercase).
/// Any segment that contains one of these is dropped.
const BOILERPLATE: &[&str] = &[
    "субтитр",
    "subtitles",
    "amara.org",
    "dimatorzok",
    "продолжение следует",
    "спасибо за просмотр",
    "дякую за перегляд",
    "дякуємо за перегляд",
    "подписывайтесь",
    "підписуйтесь",
    "підписуйтеся",
    "ставьте лайк",
    "ставте лайк",
    "thanks for watching",
    "thank you for watching",
];

/// Sound captions: dropped when the segment is short (a real sentence may
/// mention music or applause).
const SOUND_CAPTIONS: &[&str] = &[
    "музыка", "музика", "аплодисменты", "оплески", "смех", "сміх", "music", "applause", "laughter",
];

const SHORT_SEGMENT_WORDS: usize = 4;

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’')
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

/// The whole segment is a caption in brackets, e.g. "[музика]" or "(сміх)".
fn is_bracketed_caption(text: &str) -> bool {
    let t = text.trim();
    let pairs = [('[', ']'), ('(', ')'), ('*', '*'), ('♪', '♪')];
    t.chars().count() > 2
        && pairs.iter().any(|(open, close)| t.starts_with(*open) && t.ends_with(*close))
}

/// Short text in capitals with letters that Ukrainian does not have,
/// e.g. "ТИХАЯ МУЗЫКА". Real speech comes back in sentence case.
fn is_russian_caps(text: &str) -> bool {
    let letters: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() < 4 || words(text).len() > 6 {
        return false;
    }
    let upper = letters.iter().filter(|c| c.is_uppercase()).count();
    let russian_only = letters
        .iter()
        .any(|c| matches!(c.to_lowercase().next(), Some('ы' | 'э' | 'ъ' | 'ё')));
    russian_only && upper * 5 >= letters.len() * 4
}

/// The segment repeats Whisper's own initial prompt ("Дедлайн, фідбек, апдейт").
/// Words are compared by their first four letters, as the echo is often garbled.
fn is_prompt_echo(text: &str) -> bool {
    let segment = words(text);
    if segment.len() < 2 || segment.len() > 6 {
        return false;
    }
    let stem = |w: &str| w.chars().take(4).collect::<String>();
    let prompt: Vec<String> = words(PROMPT_ECHO_WORDS).iter().map(|w| stem(w)).collect();
    let echoed = segment.iter().filter(|w| prompt.contains(&stem(w))).count();
    echoed * 3 >= segment.len() * 2
}

/// Why a segment should be dropped, or `None` to keep it.
pub fn hallucination_reason(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase();
    if BOILERPLATE.iter().any(|p| lower.contains(p)) {
        return Some("subtitle boilerplate");
    }
    if is_bracketed_caption(text) {
        return Some("caption in brackets");
    }
    let segment_words = words(text);
    if segment_words.len() <= SHORT_SEGMENT_WORDS
        && segment_words.iter().any(|w| SOUND_CAPTIONS.contains(&w.as_str()))
    {
        return Some("sound caption");
    }
    if is_russian_caps(text) {
        return Some("non-Ukrainian text in capitals");
    }
    if is_prompt_echo(text) {
        return Some("echo of the Whisper prompt");
    }
    None
}

/// A short segment identical to the previous one is a Whisper loop over
/// silence, not a second utterance.
pub fn is_repeated_segment(previous: Option<&str>, text: &str) -> bool {
    let current = words(text);
    !current.is_empty()
        && current.len() <= 8
        && previous.map(words).as_deref() == Some(current.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_known_hallucinations() {
        assert!(hallucination_reason("ТИХАЯ МУЗЫКА").is_some());
        assert!(hallucination_reason("Субтитры сделал DimaTorzok").is_some());
        assert!(hallucination_reason("[музика]").is_some());
        assert!(hallucination_reason("Дякую за перегляд!").is_some());
        assert!(hallucination_reason("Дедла, фідбек, апарею.").is_some());
        assert!(hallucination_reason("Музика.").is_some());
    }

    #[test]
    fn keeps_real_speech() {
        assert!(hallucination_reason("Дякую, переходимо до наступного.").is_none());
        assert!(hallucination_reason("Строк до п'ятниці, виконавець Петренко.").is_none());
        assert!(hallucination_reason("НБУ, ЦНАП і МСЕК").is_none());
        assert!(hallucination_reason("Ну шо, Петро Іванович, добавиш?").is_none());
        // Surzhyk from the prompt is ordinary speech, not an echo
        assert!(hallucination_reason("Ну шо, всьо понятно?").is_none());
        assert!(hallucination_reason("Канєшно, щас скину.").is_none());
        assert!(hallucination_reason("Після наради ввімкнемо музику для відео про відділ").is_none());
    }

    #[test]
    fn repeated_short_segments() {
        assert!(is_repeated_segment(Some("Дякую."), "дякую"));
        assert!(!is_repeated_segment(Some("Дякую."), "Дякую, давайте."));
        assert!(!is_repeated_segment(None, "Дякую."));
    }
}
