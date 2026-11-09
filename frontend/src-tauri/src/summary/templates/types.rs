use serde::{Deserialize, Serialize};

/// Represents a single section in a meeting template
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateSection {
    /// Section title (e.g., "Summary", "Action Items")
    pub title: String,

    /// Instruction for the LLM on what to extract/include
    pub instruction: String,

    /// Format type: "paragraph", "list", or "string"
    pub format: String,

    /// Optional markdown formatting hint for list items (e.g., table structure)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_format: Option<String>,

    /// Alternative formatting hint
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example_item_format: Option<String>,
}

/// A part of a meeting (e.g. «Доповіді керівників»): starts at the first
/// report whose opening words, or the words just before it, contain a cue,
/// or whose speaker has the role.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MeetingPart {
    pub title: String,
    /// Phrases that open the part («переходимо до керівників»)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cues: Vec<String>,
    /// Speaker role that marks the part (e.g. «керівник»), set in «Спікери»
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// How a meeting of this type is written up: one engine, different recipes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MeetingSpec {
    /// What to note for each report («Зроблено», «План», «Проблеми»…);
    /// empty means 2–4 free sentences
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub report_fields: Vec<String>,
    /// Optional parts in order
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<MeetingPart>,
    /// Blocks of the document, in order: «Хід наради», «Рішення», «Завдання»,
    /// «Відкриті питання», «Не доповідали», «Контроль», «Стенограма»
    #[serde(default = "MeetingSpec::default_blocks")]
    pub blocks: Vec<String>,
    /// Words in a meeting title that suggest this type («ранкова», «щоденна»)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub title_hints: Vec<String>,
    /// Hours of the day (local) when such meetings start, for the suggestion
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hours: Vec<u32>,
    /// Type of the earlier meeting whose tasks this one checks («Контроль»)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controls: Option<String>,
    /// «Групувати за спікером»: everything one person said in the meeting
    /// is written once, under that person; unset = on when reports have
    /// fields («Зроблено», «План»…)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by_speaker: Option<bool>,
    /// The older document with fixed fields per report, instead of one block
    /// per speaker and an overall summary
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub classic: bool,
}

impl MeetingSpec {
    pub fn default_blocks() -> Vec<String> {
        ["Хід наради", "Рішення", "Завдання", "Відкриті питання", "Стенограма"].iter().map(|s| s.to_string()).collect()
    }
    pub fn has(&self, block: &str) -> bool {
        self.blocks.iter().any(|b| b == block)
    }
    pub fn groups_by_speaker(&self) -> bool {
        self.group_by_speaker.unwrap_or(!self.report_fields.is_empty())
    }
}

impl Default for MeetingSpec {
    fn default() -> Self {
        Self { report_fields: Vec::new(), parts: Vec::new(), blocks: Self::default_blocks(), title_hints: Vec::new(), hours: Vec::new(), controls: None, group_by_speaker: None, classic: false }
    }
}

/// Represents a complete meeting template
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    /// Template display name
    pub name: String,

    /// Brief description of the template's purpose
    pub description: String,

    /// List of sections in the template
    pub sections: Vec<TemplateSection>,

    /// Output language code for the generated document (e.g. "uk").
    /// When set, the document is written natively in this language instead of
    /// being drafted in English and translated afterwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// Sample documents (e.g. real protocols) provided by the user.
    /// The LLM follows their structure, wording and formatting, but never
    /// copies facts from them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,

    /// Free-form style rules (e.g. "Рішення формулювати як «Доручити …»").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_notes: Option<String>,

    /// Meeting type: written up report by report with the shared engine
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting: Option<MeetingSpec>,
}

impl Template {
    /// Validates the template structure
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("Template name cannot be empty".to_string());
        }

        if self.description.is_empty() {
            return Err("Template description cannot be empty".to_string());
        }

        if self.sections.is_empty() {
            return Err("Template must have at least one section".to_string());
        }

        for (i, section) in self.sections.iter().enumerate() {
            if section.title.is_empty() {
                return Err(format!("Section {} has empty title", i));
            }

            if section.instruction.is_empty() {
                return Err(format!("Section '{}' has empty instruction", section.title));
            }

            match section.format.as_str() {
                "paragraph" | "list" | "string" => {},
                other => return Err(format!(
                    "Section '{}' has invalid format '{}'. Must be 'paragraph', 'list', or 'string'",
                    section.title, other
                )),
            }
        }

        Ok(())
    }

    /// Generates a clean markdown template structure
    pub fn to_markdown_structure(&self) -> String {
        let mut markdown = String::from("# <Add Title here>\n\n");

        for section in &self.sections {
            markdown.push_str(&format!("**{}**\n\n", section.title));
        }

        markdown
    }

    /// Generates section-specific instructions for the LLM
    pub fn to_section_instructions(&self) -> String {
        let mut instructions = String::from(
            "- **For the main title (`# [AI-Generated Title]`):** Analyze the entire transcript and create a concise, descriptive title for the meeting.\n"
        );

        for section in &self.sections {
            instructions.push_str(&format!(
                "- **For the '{}' section:** {}.\n",
                section.title, section.instruction
            ));

            // Add item format instructions if present
            let item_format = section.item_format.as_ref()
                .or(section.example_item_format.as_ref());

            if let Some(format) = item_format {
                instructions.push_str(&format!(
                    "  - Items in this section should follow the format: `{}`.\n",
                    format
                ));
            }
        }

        instructions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_valid_template() {
        let template = Template {
            name: "Test Template".to_string(),
            description: "A test template".to_string(),
            sections: vec![
                TemplateSection {
                    title: "Summary".to_string(),
                    instruction: "Provide a summary".to_string(),
                    format: "paragraph".to_string(),
                    item_format: None,
                    example_item_format: None,
                },
            ],
            language: None,
            examples: vec![],
            style_notes: None,
            meeting: None,
        };

        assert!(template.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_name() {
        let template = Template {
            name: "".to_string(),
            description: "A test template".to_string(),
            sections: vec![],
            language: None,
            examples: vec![],
            style_notes: None,
            meeting: None,
        };

        assert!(template.validate().is_err());
    }

    #[test]
    fn test_validate_invalid_format() {
        let template = Template {
            name: "Test".to_string(),
            description: "Test".to_string(),
            sections: vec![
                TemplateSection {
                    title: "Test".to_string(),
                    instruction: "Test".to_string(),
                    format: "invalid".to_string(),
                    item_format: None,
                    example_item_format: None,
                },
            ],
            language: None,
            examples: vec![],
            style_notes: None,
            meeting: None,
        };

        assert!(template.validate().is_err());
    }
}
