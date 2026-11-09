//! Meeting summary format. One built-in template, «Підсумок наради»
//! (`templates/meeting_summary_uk.json`); unknown ids fall back to it.

mod defaults;
pub mod editor;
mod loader;
mod types;

// Re-export public API
pub use loader::{
    get_template, list_template_ids, list_templates, set_bundled_templates_dir, set_custom_templates_dir, suggest_template,
    validate_and_parse_template,
    DEFAULT_TEMPLATE_ID,
};
pub use types::{MeetingPart, MeetingSpec, Template, TemplateSection};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_integration() {
        // Test that we can load all built-in templates
        let ids = list_template_ids();
        assert!(!ids.is_empty());

        for id in ids {
            let result = get_template(&id);
            assert!(
                result.is_ok(),
                "Failed to load template '{}': {:?}",
                id,
                result.err()
            );
        }
    }

    #[test]
    fn test_template_metadata() {
        let templates = list_templates();
        assert!(!templates.is_empty());

        for (id, name, description) in templates {
            assert!(!id.is_empty());
            assert!(!name.is_empty());
            assert!(!description.is_empty());
        }
    }
}
