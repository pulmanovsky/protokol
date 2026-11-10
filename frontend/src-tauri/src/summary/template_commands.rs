use crate::summary::templates;
use serde::{Deserialize, Serialize};
use tauri::Runtime;
use tracing::{info, warn};

/// Template metadata for UI display
#[derive(Debug, Serialize, Deserialize)]
pub struct TemplateInfo {
    /// Template identifier (e.g., "daily_standup", "standard_meeting")
    pub id: String,

    /// Display name for the template
    pub name: String,

    /// Brief description of the template's purpose
    pub description: String,

    /// What the document has, in order (as `MeetingSpec::blocks`); a
    /// template without a meeting spec is the transcript only («Стенограма»)
    #[serde(default)]
    pub blocks: Vec<String>,

    /// Each speaker by the template's fields («Зроблено», «План»…)
    #[serde(default, rename = "byFields")]
    pub by_fields: bool,

    /// Built in (read-only; a change is saved as a copy)
    #[serde(default, rename = "builtIn")]
    pub built_in: bool,
}

/// Detailed template structure for preview/debugging
#[derive(Debug, Serialize, Deserialize)]
pub struct TemplateDetails {
    /// Template identifier
    pub id: String,

    /// Display name
    pub name: String,

    /// Description
    pub description: String,

    /// List of section titles in order
    pub sections: Vec<String>,
}

/// Lists all available templates
///
/// Returns templates from both built-in (embedded) and custom (user data directory) sources.
/// Templates are automatically discovered - no code changes needed to add new templates.
///
/// # Returns
/// Vector of TemplateInfo with id, name, and description for each template
#[tauri::command]
pub async fn api_list_templates<R: Runtime>(
    _app: tauri::AppHandle<R>,
) -> Result<Vec<TemplateInfo>, String> {
    info!("api_list_templates called");

    let templates = templates::list_templates();

    let template_infos: Vec<TemplateInfo> = templates
        .into_iter()
        .map(|(id, name, description)| {
            let spec = templates::get_template(&id).ok().and_then(|t| t.meeting);
            let (blocks, by_fields) = match spec {
                Some(spec) if spec.classic => (spec.blocks.clone(), true),
                Some(spec) => (spec.speaker_document_blocks(), false),
                None => (vec!["Стенограма".to_string()], false),
            };
            TemplateInfo { built_in: !id.starts_with("custom:"), id, name, description, blocks, by_fields }
        })
        .collect();

    info!("Found {} available templates", template_infos.len());

    Ok(template_infos)
}

/// The step the document generator is on, for a progress line
#[tauri::command]
pub async fn api_summary_stage() -> String {
    crate::summary::processor::current_stage()
}

/// The meeting type that fits a meeting by its title and start hour, if any
#[tauri::command]
pub async fn api_suggest_template(title: String, hour: u32) -> Option<String> {
    templates::suggest_template(&title, hour)
}

/// A meeting type as a form for the editor in Settings
#[tauri::command]
pub async fn api_get_meeting_type(id: String) -> Result<templates::editor::MeetingTypeForm, String> {
    templates::editor::form_of(&id)
}

/// Saves a meeting type from the editor; a built-in one is saved as a copy.
/// Returns the id of the saved type.
#[tauri::command]
pub async fn api_save_meeting_type(form: templates::editor::MeetingTypeForm) -> Result<String, String> {
    templates::editor::save(&form)
}

/// Deletes one of the user's own meeting types
#[tauri::command]
pub async fn api_delete_meeting_type(id: String) -> Result<(), String> {
    templates::editor::delete(&id)
}

/// Gets detailed information about a specific template
///
/// # Arguments
/// * `template_id` - Template identifier (e.g., "daily_standup")
///
/// # Returns
/// TemplateDetails with full template structure
#[tauri::command]
pub async fn api_get_template_details<R: Runtime>(
    _app: tauri::AppHandle<R>,
    template_id: String,
) -> Result<TemplateDetails, String> {
    info!("api_get_template_details called for template_id: {}", template_id);

    let template = templates::get_template(&template_id)?;

    let section_titles: Vec<String> = template
        .sections
        .iter()
        .map(|section| section.title.clone())
        .collect();

    let details = TemplateDetails {
        id: template_id,
        name: template.name,
        description: template.description,
        sections: section_titles,
    };

    info!("Retrieved template details for '{}'", details.name);

    Ok(details)
}

/// Validates a custom template JSON string
///
/// Useful for template editor UI or validation before saving custom templates
///
/// # Arguments
/// * `template_json` - Raw JSON string of the template
///
/// # Returns
/// Ok(template_name) if valid, Err(error_message) if invalid
#[tauri::command]
pub async fn api_validate_template<R: Runtime>(
    _app: tauri::AppHandle<R>,
    template_json: String,
) -> Result<String, String> {
    info!("api_validate_template called");

    match templates::validate_and_parse_template(&template_json) {
        Ok(template) => {
            info!("Template '{}' validated successfully", template.name);
            Ok(template.name)
        }
        Err(e) => {
            warn!("Template validation failed: {}", e);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_list_templates() {
        // This test requires the templates to be embedded/available
        // In a real test environment, you might want to mock the templates module

        // For now, just verify the function compiles and runs
        // You can expand this with more specific assertions
    }

    #[tokio::test]
    async fn test_validate_template_valid() {
        let valid_json = r#"
        {
            "name": "Test Template",
            "description": "A test template",
            "sections": [
                {
                    "title": "Summary",
                    "instruction": "Provide a summary",
                    "format": "paragraph"
                }
            ]
        }"#;

        // Mock app handle would be needed for actual testing
        // For now, test the validation logic directly
        let result = templates::validate_and_parse_template(valid_json);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_template_invalid() {
        let invalid_json = "invalid json";

        let result = templates::validate_and_parse_template(invalid_json);
        assert!(result.is_err());
    }
}

/// Approved meetings the app learned from (for «Словник»)
#[tauri::command]
pub async fn api_learned_meetings(state: tauri::State<'_, crate::state::AppState>) -> Result<Vec<crate::summary::learning_store::LearnedMeeting>, String> {
    crate::summary::learning_store::learned_meetings(state.db_manager.pool()).await.map_err(|e| e.to_string())
}

/// Forgets what was learned from one approved meeting
#[tauri::command]
pub async fn api_forget_learned_meeting(state: tauri::State<'_, crate::state::AppState>, meeting_id: String) -> Result<(), String> {
    crate::summary::learning_store::forget_meeting(state.db_manager.pool(), &meeting_id).await.map_err(|e| e.to_string())
}

/// How many voices the app knows by name
#[tauri::command]
pub async fn api_voice_profile_count(state: tauri::State<'_, crate::state::AppState>) -> Result<i64, String> {
    sqlx::query_scalar("SELECT COUNT(*) FROM voice_profiles").fetch_one(state.db_manager.pool()).await.map_err(|e| e.to_string())
}

/// «Зберегти у файл»: the template as a .json file that «Імпорт із файлу»
/// reads on another Mac. Returns the path, None when cancelled.
#[tauri::command]
pub async fn api_export_meeting_type<R: Runtime>(app: tauri::AppHandle<R>, id: String) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let template = templates::get_template(&id)?;
    let json = templates::editor::export(&id)?;
    // The same shape as an organisation's pack: one type, nothing else
    let stem: String = template.name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
    let pack = format!("{{\"meeting_types\":[[{},{}]]}}", serde_json::to_string(stem.trim()).map_err(|e| e.to_string())?, json);
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().add_filter("Шаблон", &["json"]).set_file_name(format!("{}.json", stem.trim())).blocking_save_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    std::fs::write(&path, pack).map_err(|e| format!("Файл не збережено: {e}"))?;
    Ok(Some(path.to_string_lossy().to_string()))
}

/// The documents kept for a meeting, one per template it was made with
#[tauri::command]
pub async fn api_document_variants(
    state: tauri::State<'_, crate::state::AppState>,
    meeting_id: String,
) -> Result<Vec<crate::database::repositories::document_variant::DocumentVariant>, String> {
    crate::database::repositories::document_variant::DocumentVariantsRepository::list(state.db_manager.pool(), &meeting_id)
        .await
        .map_err(|e| e.to_string())
}

/// A document made before 0.3.8: kept under `template_id` before another template replaces it
#[tauri::command]
pub async fn api_keep_shown_document(
    state: tauri::State<'_, crate::state::AppState>,
    meeting_id: String,
    template_id: String,
) -> Result<bool, String> {
    log::info!("api_keep_shown_document: meeting {}, template {}", meeting_id, template_id);
    crate::database::repositories::document_variant::DocumentVariantsRepository::keep_shown(state.db_manager.pool(), &meeting_id, &template_id)
        .await
        .map_err(|e| e.to_string())
}

/// Shows the document kept under `template_id`; false when there is none
#[tauri::command]
pub async fn api_show_document_variant(
    state: tauri::State<'_, crate::state::AppState>,
    meeting_id: String,
    template_id: String,
) -> Result<bool, String> {
    log::info!("api_show_document_variant: meeting {}, template {}", meeting_id, template_id);
    crate::database::repositories::document_variant::DocumentVariantsRepository::show(state.db_manager.pool(), &meeting_id, &template_id)
        .await
        .map_err(|e| e.to_string())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownDocument {
    /// The template of the document shown (None for documents made before 0.3.8)
    pub template_id: Option<String>,
    /// The transcript changed since the document was made
    pub transcript_changed: bool,
}

/// Which template the document shown was made with, and whether the
/// transcript (`text`, as it would be sent now) changed since
#[tauri::command]
pub async fn api_shown_document(
    state: tauri::State<'_, crate::state::AppState>,
    meeting_id: String,
    text: Option<String>,
) -> Result<ShownDocument, String> {
    let (template_id, transcript_changed) =
        crate::database::repositories::document_variant::DocumentVariantsRepository::shown(state.db_manager.pool(), &meeting_id, text.as_deref())
            .await
            .map_err(|e| e.to_string())?;
    log::info!(
        "api_shown_document: meeting {}, template {}, transcript changed: {}",
        meeting_id,
        template_id.as_deref().unwrap_or("-"),
        transcript_changed
    );
    Ok(ShownDocument { template_id, transcript_changed })
}
