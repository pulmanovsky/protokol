/// The single built-in format, embedded at compile time.
///
/// Протокол writes one kind of document: a short Ukrainian meeting summary.
/// The earlier protocol, standup and other templates were removed.

/// Identifier of «Підсумок наради»
pub const MEETING_SUMMARY_ID: &str = "meeting_summary_uk";

/// «Стенограма» (default)
pub const MEETING_SUMMARY_UK: &str = include_str!("../../../templates/meeting_summary_uk.json");

/// Identifier of «Матеріали наради» (decisions, tasks, open questions + transcript)
pub const MEETING_MATERIALS_ID: &str = "meeting_materials_uk";

/// «Матеріали наради»
pub const MEETING_MATERIALS_UK: &str = include_str!("../../../templates/meeting_materials_uk.json");

/// «Щоденна нарада (доповіді)»
pub const DAILY_REPORTS_ID: &str = "daily_reports_uk";
pub const DAILY_REPORTS_UK: &str = include_str!("../../../templates/daily_reports_uk.json");

/// The older document with fixed fields, kept as a meeting type
pub const DAILY_REPORTS_FIELDS_ID: &str = "daily_reports_fields_uk";
pub const DAILY_REPORTS_FIELDS_UK: &str = include_str!("../../../templates/daily_reports_fields_uk.json");

/// «Контрольна нарада»
pub const CONTROL_MEETING_ID: &str = "control_meeting_uk";
pub const CONTROL_MEETING_UK: &str = include_str!("../../../templates/control_meeting_uk.json");

/// Registry of all built-in templates (meeting types), in the order offered
pub fn get_builtin_templates() -> Vec<(&'static str, &'static str)> {
    vec![
        (MEETING_SUMMARY_ID, MEETING_SUMMARY_UK),
        (DAILY_REPORTS_ID, DAILY_REPORTS_UK),
        (DAILY_REPORTS_FIELDS_ID, DAILY_REPORTS_FIELDS_UK),
        (CONTROL_MEETING_ID, CONTROL_MEETING_UK),
        (MEETING_MATERIALS_ID, MEETING_MATERIALS_UK),
    ]
}

/// Get a built-in template by identifier
pub fn get_builtin_template(id: &str) -> Option<&'static str> {
    get_builtin_templates().into_iter().find(|(i, _)| *i == id).map(|(_, t)| t)
}

/// List all built-in template identifiers
pub fn list_builtin_template_ids() -> Vec<&'static str> {
    get_builtin_templates().into_iter().map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_template_is_valid_json() {
        for (id, content) in get_builtin_templates() {
            assert!(serde_json::from_str::<serde_json::Value>(content).is_ok(), "{id} is not valid JSON");
        }
    }
}
