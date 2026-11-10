//! Template lookup. There is one built-in format, «Підсумок наради»; any
//! other identifier (saved by older versions: protocol_uk, standard_meeting,
//! custom templates from samples) falls back to it.

use super::defaults;
use super::types::Template;
use log::{info, warn};
use std::path::PathBuf;

pub use defaults::MEETING_SUMMARY_ID as DEFAULT_TEMPLATE_ID;

/// Kept for API compatibility: templates are embedded, the bundled folder is not read.
pub fn set_bundled_templates_dir(path: PathBuf) {
    info!("Templates are built in; ignoring bundled templates directory {:?}", path);
}

static CUSTOM_DIR: once_cell::sync::Lazy<std::sync::RwLock<Option<PathBuf>>> = once_cell::sync::Lazy::new(|| std::sync::RwLock::new(None));

/// Folder with the user's own meeting types (`<app data>/meeting-types/*.json`):
/// a copy of a built-in type with other fields, parts or blocks
pub fn set_custom_templates_dir(path: PathBuf) {
    if let Ok(mut slot) = CUSTOM_DIR.write() {
        *slot = Some(path);
    }
}

pub(crate) fn custom_dir() -> Option<PathBuf> {
    CUSTOM_DIR.read().ok().and_then(|d| d.clone())
}

/// The user's own types: (id = "custom:<file stem>", JSON)
fn custom_templates() -> Vec<(String, String)> {
    let Some(dir) = CUSTOM_DIR.read().ok().and_then(|d| d.clone()) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<(String, String)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .filter_map(|p| {
            let stem = p.file_stem()?.to_str()?.to_string();
            let text = std::fs::read_to_string(&p).ok()?;
            match validate_and_parse_template(&text) {
                Ok(_) => Some((format!("custom:{stem}"), text)),
                Err(e) => {
                    warn!("Meeting type {:?} skipped: {e}", p);
                    None
                }
            }
        })
        .collect();
    out.sort();
    out
}

/// Returns the template for `template_id`, or «Підсумок наради» for unknown ids
pub fn get_template(template_id: &str) -> Result<Template, String> {
    if template_id.starts_with("custom:") {
        if let Some((_, text)) = custom_templates().into_iter().find(|(id, _)| id == template_id) {
            return validate_and_parse_template(&text);
        }
    }
    let id = if defaults::get_builtin_template(template_id).is_some() {
        template_id
    } else {
        if !template_id.is_empty() {
            warn!("Template '{}' no longer exists; using '{}'", template_id, DEFAULT_TEMPLATE_ID);
        }
        DEFAULT_TEMPLATE_ID
    };
    let content = defaults::get_builtin_template(id).ok_or_else(|| format!("Template '{id}' not found"))?;
    validate_and_parse_template(content)
}

/// Validate and parse template JSON
pub fn validate_and_parse_template(json_content: &str) -> Result<Template, String> {
    let template: Template =
        serde_json::from_str(json_content).map_err(|e| format!("Failed to parse template JSON: {}", e))?;
    template.validate()?;
    Ok(template)
}

/// All template identifiers: built-in types, then the user's own
pub fn list_template_ids() -> Vec<String> {
    let mut ids: Vec<String> = defaults::list_builtin_template_ids().into_iter().map(str::to_string).collect();
    ids.extend(custom_templates().into_iter().map(|(id, _)| id));
    ids
}

/// The meeting type that fits a meeting best, by words in its title and the
/// hour it started; None when nothing fits (the app keeps the default)
pub fn suggest_template(title: &str, hour: u32) -> Option<String> {
    let types = offered_template_ids().into_iter().filter_map(|id| get_template(&id).ok().and_then(|t| t.meeting).map(|m| (id, m))).collect::<Vec<_>>();
    best_type(&types, title, hour)
}

/// The best-scoring type by title words (2 each) and start hour (1); on a
/// tie the user's own type wins over a built-in one
fn best_type(types: &[(String, super::types::MeetingSpec)], title: &str, hour: u32) -> Option<String> {
    let title = title.to_lowercase();
    let mut best: Option<(u32, &String)> = None;
    for (id, spec) in types {
        let score = 2 * spec.title_hints.iter().filter(|h| title.contains(h.as_str())).count() as u32 + u32::from(spec.hours.contains(&hour));
        let own = id.starts_with("custom:");
        if score > 0 && best.map(|(s, _)| score > s || (own && score == s)).unwrap_or(true) {
            best = Some((score, id));
        }
    }
    best.map(|(_, id)| id.clone())
}

/// The templates offered to choose from: all but `NOT_OFFERED`
pub fn offered_template_ids() -> Vec<String> {
    list_template_ids().into_iter().filter(|id| !defaults::NOT_OFFERED.contains(&id.as_str())).collect()
}

/// (id, name, description) of every template offered
pub fn list_templates() -> Vec<(String, String, String)> {
    offered_template_ids()
        .into_iter()
        .filter_map(|id| get_template(&id).ok().map(|t| (id, t.name, t.description)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_template_loads() {
        let template = get_template(DEFAULT_TEMPLATE_ID).unwrap();
        assert_eq!(template.name, "Стенограма");
        let materials = get_template("meeting_materials_uk").unwrap();
        assert_eq!(materials.name, "Підсумки доповідачів");
        let spec = materials.meeting.unwrap();
        assert!(spec.ordered);
        assert_eq!(spec.blocks, ["Хід наради", "Загальний підсумок"]);
        let decisions = get_template("decisions_tasks_uk").unwrap().meeting.unwrap();
        assert_eq!(decisions.speaker_document_blocks(), ["Рішення", "Завдання"]);
    }

    #[test]
    fn the_users_own_type_wins_a_tie() {
        let spec = get_template("daily_reports_uk").unwrap().meeting.unwrap();
        let types = vec![("daily_reports_uk".to_string(), spec.clone()), ("custom:ranok".to_string(), spec.clone())];
        assert_eq!(best_type(&types, "Ранкова нарада", 8).as_deref(), Some("custom:ranok"));
        assert_eq!(best_type(&types[..1], "Ранкова нарада", 8).as_deref(), Some("daily_reports_uk"));
        assert_eq!(best_type(&types, "Зустріч з постачальником", 23), None);
    }

    #[test]
    fn old_template_ids_fall_back_to_the_summary() {
        for id in ["protocol_uk", "standard_meeting", "daily_standup", "custom_shtab", ""] {
            assert_eq!(get_template(id).unwrap().name, "Стенограма");
        }
    }

    #[test]
    fn the_main_template_is_offered_first_and_the_old_daily_one_not_at_all() {
        assert_eq!(
            offered_template_ids(),
            vec!["meeting_materials_uk".to_string(), "decisions_tasks_uk".into(), "daily_reports_fields_uk".into(), DEFAULT_TEMPLATE_ID.into(), "control_meeting_uk".into()]
        );
        assert!(list_templates().iter().all(|(id, _, _)| id != "daily_reports_uk"));
        // Still opens: documents made with it, «Контрольна нарада» checks it
        assert!(list_template_ids().contains(&"daily_reports_uk".to_string()));
        assert!(get_template("daily_reports_uk").unwrap().meeting.is_some());
    }

    #[test]
    fn meeting_types_and_suggestion() {
        let ids = list_template_ids();
        for id in ["meeting_summary_uk", "daily_reports_uk", "control_meeting_uk", "meeting_materials_uk"] {
            assert!(ids.contains(&id.to_string()), "{id}");
        }
        assert!(get_template("daily_reports_uk").unwrap().meeting.unwrap().report_fields.len() == 5);
        // The old daily type is no longer suggested
        assert_eq!(suggest_template("Ранкова нарада", 8), None);
        assert_eq!(suggest_template("Вечірня нарада", 18).as_deref(), Some("control_meeting_uk"));
        // A default title («Запис …») at an ordinary hour keeps «Стенограма»
        assert_eq!(suggest_template("Запис 06.11.2026 13:00", 13), None);
        assert_eq!(suggest_template("Зустріч з партнерами", 13).as_deref(), Some("meeting_materials_uk"));
    }

    #[test]
    fn invalid_json_is_rejected() {
        assert!(validate_and_parse_template("not json").is_err());
    }
}
