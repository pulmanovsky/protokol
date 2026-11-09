//! Ukrainian language support shared by transcription and protocol generation.
//!
//! - A user glossary (names, positions, abbreviations, domain terms) that biases
//!   Whisper towards the right spelling and is passed to the LLM.
//! - The Whisper initial prompt for Ukrainian speech mixed with surzhyk,
//!   russisms and anglicisms.

pub mod commands;
pub mod docx_export;
pub mod corrections;
pub mod doubt;
pub mod hallucination;
pub mod learning;
pub mod orthography;
pub mod pdf_export;

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use once_cell::sync::Lazy;
use tracing::{info, warn};

/// Default language for this build: transcripts and protocols are Ukrainian.
pub const DEFAULT_LANGUAGE: &str = "uk";

/// Transcript markers written when the microphone is muted in the app.
/// They stay in the live transcript but not in documents.
pub const MIC_MUTED_MARKER: &str = "— Мікрофон вимкнено —";
pub const MIC_UNMUTED_MARKER: &str = "— Мікрофон увімкнено —";

/// Removes the microphone markers from transcript text: whole "[MM:SS] marker"
/// lines and markers inside a line.
pub fn strip_mic_markers(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if !line.contains(MIC_MUTED_MARKER) && !line.contains(MIC_UNMUTED_MARKER) {
            out.push(line.to_string());
            continue;
        }
        let cleaned = line.replace(MIC_MUTED_MARKER, " ").replace(MIC_UNMUTED_MARKER, " ");
        let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
        // A line that only had a time label and the marker is dropped
        let rest = cleaned.strip_prefix('[').and_then(|r| r.split_once(']')).map(|(_, r)| r.trim()).unwrap_or(&cleaned);
        if !rest.is_empty() {
            out.push(cleaned);
        }
    }
    let mut joined = out.join("\n");
    if text.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

const GLOSSARY_FILE: &str = "glossary.txt";

/// Whisper only looks at roughly the last 224 tokens of the prompt, and
/// Cyrillic text uses several tokens per word, so keep it short.
const MAX_WHISPER_PROMPT_CHARS: usize = 400;

/// Hard limit in Whisper tokens (the model keeps n_text_ctx / 2 = 224;
/// a margin is left for the special tokens).
pub const MAX_WHISPER_PROMPT_TOKENS: usize = 200;

/// The formal and anglicism part of the prompt. Whisper sometimes echoes it
/// on silence, so the hallucination filter looks for these words.
pub(crate) const PROMPT_ECHO_WORDS: &str = "Нарада. Порядок денний. Слухали, виступили, вирішили. \
Доручення, виконавець, строк. Дедлайн, фідбек, апдейт, мітинг.";

/// Seed text for Whisper, written the way people actually speak in Ukrainian
/// meetings: official words, surzhyk and Russian words spelled in the
/// Ukrainian alphabet («шо», «всьо», «канєшно», «получається») and anglicisms
/// in Cyrillic («дедлайн», «фідбек»). Whisper continues in the style of its
/// prompt, so this keeps it in Ukrainian spelling instead of drifting to
/// Russian on surzhyk.
///
/// The prompt must end with the formal words. On a real 37-minute meeting,
/// prompts that ended with conversational phrases or with dictionary terms
/// made Whisper skip the beginning of some fragments (a few lost up to their
/// whole text). So dictionary terms go before this text, and the surzhyk
/// words sit in the middle.
const BASE_WHISPER_PROMPT_UK: &str = "Нарада. Порядок денний. Ну шо, всьо, канєшно, получається. \
Слухали, виступили, вирішили. Доручення, виконавець, строк. Дедлайн, фідбек, апдейт, мітинг.";

static GLOSSARY: Lazy<RwLock<Vec<String>>> = Lazy::new(|| RwLock::new(Vec::new()));
static GLOSSARY_PATH: Lazy<RwLock<Option<PathBuf>>> = Lazy::new(|| RwLock::new(None));

/// Parses glossary text: one term per line, `#` starts a comment,
/// blank lines and duplicates are ignored.
pub fn parse_glossary(text: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for line in text.lines() {
        let term = line.split('#').next().unwrap_or("").trim();
        if term.is_empty() {
            continue;
        }
        if !terms.iter().any(|existing| existing == term) {
            terms.push(term.to_string());
        }
    }
    terms
}

/// Loads the glossary from `<app_data_dir>/glossary.txt` into memory.
/// Called once at startup; a missing file means an empty glossary.
pub fn init_glossary(app_data_dir: &Path) {
    let path = app_data_dir.join(GLOSSARY_FILE);
    let terms = match std::fs::read_to_string(&path) {
        Ok(text) => parse_glossary(&text),
        Err(_) => Vec::new(),
    };
    info!("Loaded {} glossary terms from {:?}", terms.len(), path);
    if let Ok(mut slot) = GLOSSARY_PATH.write() {
        *slot = Some(path);
    }
    if let Ok(mut glossary) = GLOSSARY.write() {
        *glossary = terms;
    }
}

/// Folder that holds the glossary and the related correction files
pub(crate) fn glossary_dir() -> Option<PathBuf> {
    GLOSSARY_PATH.read().ok().and_then(|p| p.clone()).and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

/// The term part of a glossary line («Гончаренко — відповідає за постачання» → «Гончаренко»)
pub fn term_of(line: &str) -> &str {
    line.split(" — ").next().unwrap_or(line).split(" - ").next().unwrap_or(line).trim()
}

/// Glossary terms without their context notes, most used first (for
/// Whisper's prompt, which only fits the first few)
pub fn glossary_terms() -> Vec<String> {
    let mut terms: Vec<String> = glossary().iter().map(|l| term_of(l).to_string()).filter(|t| !t.is_empty()).collect();
    let counts = learning::usage_counts();
    terms.sort_by_key(|t| std::cmp::Reverse(counts.get(t).copied().unwrap_or(0)));
    // Names, nicknames and roles confirmed in «Спікери» come first: they are
    // said at every meeting and are the words recognised worst
    let mut out: Vec<String> = Vec::new();
    for t in speaker_terms().into_iter().chain(terms) {
        if !out.iter().any(|o| o.to_lowercase() == t.to_lowercase()) {
            out.push(t);
        }
    }
    out
}

static SPEAKER_TERMS: Lazy<RwLock<Vec<String>>> = Lazy::new(|| RwLock::new(Vec::new()));

/// Names and roles of the voices the user confirmed (from voice profiles)
pub fn set_speaker_terms(terms: Vec<String>) {
    let mut clean: Vec<String> = Vec::new();
    for t in terms.iter().map(|t| t.trim()).filter(|t| t.chars().count() >= 2 && t.chars().count() <= 40) {
        if !clean.iter().any(|c: &String| c.to_lowercase() == t.to_lowercase()) {
            clean.push(t.to_string());
        }
    }
    clean.truncate(30);
    if let Ok(mut slot) = SPEAKER_TERMS.write() {
        *slot = clean;
    }
}

pub fn speaker_terms() -> Vec<String> {
    SPEAKER_TERMS.read().map(|t| t.clone()).unwrap_or_default()
}

/// Current glossary lines, with their context notes.
pub fn glossary() -> Vec<String> {
    GLOSSARY.read().map(|g| g.clone()).unwrap_or_default()
}

/// Replaces the glossary and persists it to disk.
pub fn set_glossary(terms: Vec<String>) -> Result<Vec<String>, String> {
    let cleaned = parse_glossary(&terms.join("\n"));
    let path = GLOSSARY_PATH
        .read()
        .ok()
        .and_then(|p| p.clone())
        .ok_or_else(|| "Glossary storage is not initialised".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create data folder: {e}"))?;
    }
    std::fs::write(&path, cleaned.join("\n") + "\n")
        .map_err(|e| format!("Failed to save glossary: {e}"))?;
    if let Ok(mut glossary) = GLOSSARY.write() {
        *glossary = cleaned.clone();
    } else {
        warn!("Glossary lock poisoned; saved to disk only");
    }
    Ok(cleaned)
}

/// Builds the Whisper initial prompt for a transcription language.
/// Returns None for languages other than Ukrainian (upstream behaviour).
pub fn whisper_initial_prompt(language: Option<&str>, glossary: &[String]) -> Option<String> {
    whisper_initial_prompt_fitting(language, glossary, |_| true)
}

/// Same as [`whisper_initial_prompt`], adding dictionary terms (most used
/// first) only while `fits` accepts the prompt, e.g. a Whisper token count.
pub fn whisper_initial_prompt_fitting(
    language: Option<&str>,
    glossary: &[String],
    fits: impl Fn(&str) -> bool,
) -> Option<String> {
    // With auto-detection the Ukrainian prompt and glossary are still used:
    // they keep names right and nudge detection towards Ukrainian
    if language.is_some_and(|l| l != DEFAULT_LANGUAGE) {
        return None;
    }
    #[cfg(test)]
    let base = std::env::var("PROTOKOL_WHISPER_PROMPT").unwrap_or_else(|_| BASE_WHISPER_PROMPT_UK.to_string());
    #[cfg(not(test))]
    let base = BASE_WHISPER_PROMPT_UK.to_string();
    #[cfg(test)]
    let glossary = if std::env::var("PROTOKOL_NO_GLOSSARY").is_ok() { &[][..] } else { glossary };
    // Dictionary terms first, the base text last (see BASE_WHISPER_PROMPT_UK)
    let mut terms = String::new();
    for term in glossary {
        let candidate_terms = format!("{terms}{term}. ");
        let candidate = format!("{candidate_terms}{base}");
        if candidate.chars().count() > MAX_WHISPER_PROMPT_CHARS || !fits(&candidate) {
            break;
        }
        terms = candidate_terms;
    }
    Some(format!("{terms}{base}"))
}

/// Maps the UI language setting to the language Whisper should use.
/// Ukrainian is the default (see `DEFAULT_LANGUAGE` and the UI default);
/// "auto", if chosen in Settings, lets Whisper detect the spoken language
/// and transcribe in it. Nothing is ever translated: "auto-translate" from
/// upstream behaves like "auto".
pub fn resolve_transcription_language(setting: Option<&str>) -> (Option<String>, bool) {
    match setting {
        None | Some("") | Some("auto") | Some("auto-translate") => (None, false),
        Some(code) => (Some(code.to_string()), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mic_markers_are_removed_from_documents() {
        let text = "[00:03] Добрий день.\n[00:10] — Мікрофон вимкнено —\n[00:12] Так. — Мікрофон увімкнено — Почнемо.\n";
        assert_eq!(strip_mic_markers(text), "[00:03] Добрий день.\n[00:12] Так. Почнемо.\n");
        assert_eq!(strip_mic_markers("Без міток"), "Без міток");
    }

    #[test]
    fn parse_glossary_skips_comments_blanks_and_duplicates() {
        let text = "Петренко О. М.\n\n# коментар\nНБУ  # абревіатура\nНБУ\n  СЕД ";
        assert_eq!(parse_glossary(text), vec!["Петренко О. М.", "НБУ", "СЕД"]);
    }

    #[test]
    fn whisper_prompt_only_for_ukrainian() {
        assert!(whisper_initial_prompt(Some("en"), &[]).is_none());
        assert!(whisper_initial_prompt(None, &[]).is_some());
        let prompt = whisper_initial_prompt(Some("uk"), &["ОДА".to_string(), "НБУ".to_string()]).unwrap();
        assert!(prompt.starts_with("ОДА. НБУ. Нарада."));
        assert!(prompt.ends_with("мітинг."));
    }

    #[test]
    fn whisper_prompt_is_capped() {
        let long: Vec<String> = (0..200).map(|i| format!("Термін{i}")).collect();
        let prompt = whisper_initial_prompt(Some("uk"), &long).unwrap();
        assert!(prompt.chars().count() <= MAX_WHISPER_PROMPT_CHARS);
    }

    #[test]
    fn whisper_prompt_respects_the_token_check() {
        let terms: Vec<String> = vec!["Альфа".into(), "Бета".into(), "Гамма".into()];
        let base = whisper_initial_prompt(Some("uk"), &[]).unwrap();
        let prompt = whisper_initial_prompt_fitting(Some("uk"), &terms, |p| p.chars().count() <= base.chars().count() + 14).unwrap();
        assert!(prompt.starts_with("Альфа. Бета. Нарада."));
    }

    #[test]
    fn whisper_prompt_is_in_ukrainian_spelling() {
        let prompt = whisper_initial_prompt(Some("uk"), &[]).unwrap();
        assert_eq!(orthography::russian_letter_count(&prompt), 0);
        assert!(prompt.contains("Дедлайн") && prompt.contains("всьо"));
    }

    #[test]
    fn ukrainian_by_default_auto_detects_only_when_chosen() {
        assert_eq!(resolve_transcription_language(Some("auto")), (None, false));
        assert_eq!(resolve_transcription_language(Some("auto-translate")), (None, false));
        assert_eq!(resolve_transcription_language(None), (None, false));
        assert_eq!(resolve_transcription_language(Some("uk")), (Some("uk".into()), false));
        assert_eq!(resolve_transcription_language(Some("en")), (Some("en".into()), false));
    }
}
