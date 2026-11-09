//! A template pack: meeting types, dictionary terms and extra vocabulary for
//! one organisation, imported from a file and kept with the user's data.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

const FILE: &str = "vocabulary.json";

/// Extra words an organisation uses; everything is optional
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vocabulary {
    /// (as written in a prompt, as this organisation says it)
    pub prompt_wording: Vec<(String, String)>,
    pub report_openings: Vec<String>,
    pub handover_cues: Vec<String>,
    pub decision_cues: Vec<String>,
    pub not_names: Vec<String>,
}

/// A pack file
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Pack {
    /// (file name without extension, the meeting type)
    pub meeting_types: Vec<(String, serde_json::Value)>,
    pub glossary: Vec<String>,
    pub vocabulary: Vocabulary,
}

static VOCABULARY: RwLock<Option<Vocabulary>> = RwLock::new(None);
static DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

pub fn current() -> Vocabulary {
    VOCABULARY.read().ok().and_then(|v| v.clone()).unwrap_or_default()
}

/// Loads what was imported before (at the start of the app)
pub fn load(dir: &Path) {
    if let Ok(mut d) = DIR.write() {
        *d = Some(dir.to_path_buf());
    }
    let found = std::fs::read_to_string(dir.join(FILE)).ok().and_then(|t| serde_json::from_str::<Vocabulary>(&t).ok());
    if let Ok(mut v) = VOCABULARY.write() {
        *v = found;
    }
}

/// A prompt in the organisation's own words
pub fn worded(prompt: &str) -> String {
    worded_with(prompt, &current())
}

pub(crate) fn worded_with(prompt: &str, vocabulary: &Vocabulary) -> String {
    vocabulary.prompt_wording.iter().fold(prompt.to_string(), |text, (from, to)| text.replace(from, to))
}

/// Puts a pack in place: its meeting types, its terms added to the dictionary
/// (existing ones stay) and its vocabulary. Returns (types, new terms).
pub fn import(text: &str, dir: &Path, types_dir: &Path) -> Result<(usize, usize), String> {
    let pack: Pack = serde_json::from_str(text).map_err(|e| format!("Файл шаблону не прочитано: {e}"))?;
    std::fs::create_dir_all(types_dir).map_err(|e| e.to_string())?;
    for (stem, body) in &pack.meeting_types {
        if stem.is_empty() || stem.contains(['/', '\\', '.']) {
            return Err(format!("Неправильна назва типу запису: {stem}"));
        }
        let json = serde_json::to_string_pretty(body).map_err(|e| e.to_string())?;
        super::templates::validate_and_parse_template(&json)?;
        std::fs::write(types_dir.join(format!("{stem}.json")), json).map_err(|e| e.to_string())?;
    }
    let file = dir.join("glossary.txt");
    let existing = std::fs::read_to_string(&file).unwrap_or_default();
    let known: std::collections::HashSet<String> = existing.lines().map(|l| l.trim().to_lowercase()).collect();
    let new: Vec<&String> = pack.glossary.iter().filter(|t| !t.trim().is_empty() && !known.contains(&t.trim().to_lowercase())).collect();
    if !new.is_empty() {
        let mut body = existing;
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        for term in &new {
            body.push_str(term.trim());
            body.push('\n');
        }
        std::fs::write(&file, body).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(&pack.vocabulary).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(FILE), json).map_err(|e| e.to_string())?;
    load(dir);
    Ok((pack.meeting_types.len(), new.len()))
}

/// «Імпорт шаблону»: without a path, a file picker opens. None = cancelled.
#[tauri::command]
pub async fn api_import_pack<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    path: Option<String>,
) -> Result<Option<(usize, usize)>, String> {
    use tauri_plugin_dialog::DialogExt;
    let dir = DIR.read().ok().and_then(|d| d.clone()).ok_or("Папку даних не знайдено")?;
    let path = match path {
        Some(p) => p,
        None => {
            let picked = tauri::async_runtime::spawn_blocking(move || {
                app.dialog().file().add_filter("Шаблон організації", &["json"]).blocking_pick_file()
            })
            .await
            .map_err(|e| e.to_string())?;
            match picked.and_then(|p| p.into_path().ok()) {
                Some(p) => p.to_string_lossy().to_string(),
                None => return Ok(None),
            }
        }
    };
    let text = std::fs::read_to_string(&path).map_err(|e| format!("Файл шаблону не прочитано: {e}"))?;
    import(&text, &dir, &dir.join("meeting-types")).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pack_brings_types_terms_and_wording_and_keeps_what_is_there() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("glossary.txt"), "Наявний\n").unwrap();
        let pack = r#"{"meeting_types":[["своя",{"name":"Своя","description":"x","sections":[{"title":"А","instruction":"Б","format":"paragraph"}]}]],
            "glossary":["наявний","Новий"],"vocabulary":{"prompt_wording":[["ім'я","позначка"]],"not_names":["слово"]}}"#;
        let types = dir.path().join("meeting-types");
        assert_eq!(import(pack, dir.path(), &types).unwrap(), (1, 1));
        assert!(types.join("своя.json").exists());
        assert_eq!(std::fs::read_to_string(dir.path().join("glossary.txt")).unwrap(), "Наявний\nНовий\n");
        let v: Vocabulary = serde_json::from_str(&std::fs::read_to_string(dir.path().join(FILE)).unwrap()).unwrap();
        assert_eq!(worded_with("хто: ім'я", &v), "хто: позначка");
        assert_eq!(v.not_names, vec!["слово"]);
        assert!(import(r#"{"meeting_types":[["../x",{}]]}"#, dir.path(), &types).is_err());
    }

    #[test]
    fn nothing_changes_without_a_pack() {
        assert_eq!(worded_with("текст", &Vocabulary::default()), "текст");
    }
}
