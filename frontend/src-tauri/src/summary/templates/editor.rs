//! Document templates the user edits in Settings: a simple form (name, the
//! document's blocks in order, how each speaker is written up, length,
//! wishes; parts and hints under «Додатково») saved as the user's own
//! template in `<app data>/meeting-types/`. Built-in templates are never
//! changed: a change to one is saved as a copy.

use serde::{Deserialize, Serialize};

use super::loader::{custom_dir, get_template, list_template_ids, validate_and_parse_template};
use super::types::{MeetingPart, MeetingSpec, Template, TemplateSection, OVERALL_BLOCK};

/// The blocks a document can have, in the usual order
pub const BLOCKS: &[&str] = &["Контроль", "Хід наради", "Рішення", "Завдання", "Відкриті питання", OVERALL_BLOCK, "Не доповідали", "Стенограма"];

/// How each speaker is written up: short points, or the template's fields
pub const STYLE_POINTS: &str = "points";
pub const STYLE_FIELDS: &str = "fields";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MeetingTypeForm {
    /// "custom:<stem>" for the user's own type; a built-in id or None for a new one
    pub id: Option<String>,
    pub name: String,
    pub description: String,
    pub report_fields: Vec<String>,
    pub parts: Vec<MeetingPart>,
    pub blocks: Vec<String>,
    pub title_hints: Vec<String>,
    pub hours: Vec<u32>,
    pub controls: Option<String>,
    /// «Групувати за спікером»
    #[serde(default)]
    pub group_by_speaker: bool,
    /// `STYLE_POINTS` («Короткими пунктами») or `STYLE_FIELDS` («За полями»)
    #[serde(default)]
    pub style: String,
    /// «Докладніше»
    #[serde(default)]
    pub detailed: bool,
    /// «Побажання до тексту»
    #[serde(default)]
    pub wishes: String,
    /// Built-in types are read-only
    pub built_in: bool,
}

pub fn form_of(id: &str) -> Result<MeetingTypeForm, String> {
    let t = get_template(id)?;
    let spec = t.meeting.clone().unwrap_or_default();
    let group_by_speaker = spec.groups_by_speaker();
    // What the document really has: types saved before 0.3.8 write the
    // speakers' blocks and the overall summary whatever their blocks say
    let blocks = if spec.classic { spec.blocks.clone() } else { spec.speaker_document_blocks() };
    Ok(MeetingTypeForm {
        id: Some(id.to_string()),
        name: t.name,
        description: t.description,
        report_fields: spec.report_fields,
        parts: spec.parts,
        blocks,
        title_hints: spec.title_hints,
        hours: spec.hours,
        group_by_speaker,
        controls: spec.controls,
        style: if spec.classic { STYLE_FIELDS } else { STYLE_POINTS }.to_string(),
        detailed: spec.detailed,
        wishes: spec.wishes.unwrap_or_default(),
        built_in: !id.starts_with("custom:"),
    })
}

fn clean(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in list.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if !out.iter().any(|o| o.to_lowercase() == s.to_lowercase()) {
            out.push(s.to_string());
        }
    }
    out
}

/// The template saved for a form
pub fn template_of(form: &MeetingTypeForm) -> Result<Template, String> {
    let name = form.name.trim();
    if name.is_empty() {
        return Err("Вкажіть назву типу".into());
    }
    // In the form's order; unknown and repeated blocks dropped
    let mut blocks: Vec<String> = Vec::new();
    for b in &form.blocks {
        if BLOCKS.contains(&b.as_str()) && !blocks.contains(b) {
            blocks.push(b.clone());
        }
    }
    if blocks.is_empty() {
        return Err("Виберіть хоча б один розділ документа".into());
    }
    let by_fields = form.style == STYLE_FIELDS;
    let report_fields = if by_fields { clean(&form.report_fields) } else { Vec::new() };
    if by_fields && blocks.iter().any(|b| b == "Хід наради") && report_fields.is_empty() {
        return Err("Додайте хоча б одне поле або виберіть «Короткими пунктами»".into());
    }
    let parts: Vec<MeetingPart> = form
        .parts
        .iter()
        .filter(|p| !p.title.trim().is_empty())
        .map(|p| MeetingPart {
            title: p.title.trim().to_string(),
            cues: clean(&p.cues).into_iter().map(|c| c.to_lowercase()).collect(),
            role: p.role.as_ref().map(|r| r.trim().to_lowercase()).filter(|r| !r.is_empty()),
        })
        .collect();
    let sections = blocks
        .iter()
        .map(|b| TemplateSection {
            title: if b == "Стенограма" { "Стенограма по доповідачах".into() } else { b.clone() },
            instruction: b.clone(),
            format: "list".into(),
            item_format: None,
            example_item_format: None,
        })
        .collect();
    let description = if form.description.trim().is_empty() { describe(&blocks, &report_fields) } else { form.description.trim().to_string() };
    Ok(Template {
        name: name.to_string(),
        description,
        sections,
        language: Some("uk".into()),
        examples: Vec::new(),
        style_notes: Some("Пиши ясно і стисло, звичайною діловою мовою. Кожен пункт — одне коротке речення.".into()),
        meeting: Some(MeetingSpec {
            report_fields,
            parts,
            blocks,
            title_hints: clean(&form.title_hints).into_iter().map(|h| h.to_lowercase()).collect(),
            hours: form.hours.iter().copied().filter(|h| *h < 24).collect(),
            controls: form.controls.clone().filter(|c| list_template_ids().contains(c)),
            group_by_speaker: Some(form.group_by_speaker),
            classic: by_fields,
            ordered: true,
            detailed: form.detailed,
            wishes: Some(form.wishes.trim().to_string()).filter(|w| !w.is_empty()),
        }),
    })
}

/// What the document has, in words: «Підсумок кожного доповідача, завдання.»
fn describe(blocks: &[String], report_fields: &[String]) -> String {
    let words: Vec<String> = blocks
        .iter()
        .map(|b| match b.as_str() {
            "Хід наради" if !report_fields.is_empty() => format!("кожен доповідач за полями ({})", report_fields.join(", ").to_lowercase()),
            "Хід наради" => "підсумок кожного доповідача".into(),
            "Контроль" => "контроль попередніх завдань".into(),
            "Не доповідали" => "хто не доповідав".into(),
            "Стенограма" => "повна стенограма".into(),
            other => other.to_lowercase(),
        })
        .collect();
    let text = words.join(", ");
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

/// The template as a file to keep or pass on (what «Імпорт із файлу» reads)
pub fn export(id: &str) -> Result<String, String> {
    let template = get_template(id)?;
    serde_json::to_string_pretty(&template).map_err(|e| e.to_string())
}

/// Saves the form as the user's own type; returns its id. A built-in type
/// (or a new one) gets a new file; the user's own type is overwritten.
pub fn save(form: &MeetingTypeForm) -> Result<String, String> {
    let template = template_of(form)?;
    let dir = custom_dir().ok_or("Папку типів нарад не знайдено")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stem = match form.id.as_deref().and_then(|id| id.strip_prefix("custom:")) {
        Some(stem) => stem.to_string(),
        None => {
            let base = chrono::Local::now().format("type-%Y%m%d-%H%M%S").to_string();
            let mut stem = base.clone();
            let mut n = 2;
            while dir.join(format!("{stem}.json")).exists() {
                stem = format!("{base}-{n}");
                n += 1;
            }
            stem
        }
    };
    let json = serde_json::to_string_pretty(&template).map_err(|e| e.to_string())?;
    validate_and_parse_template(&json)?;
    let tmp = dir.join(format!(".{stem}.json.tmp"));
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join(format!("{stem}.json"))).map_err(|e| e.to_string())?;
    Ok(format!("custom:{stem}"))
}

/// Deletes the user's own type (built-in ones cannot be deleted)
pub fn delete(id: &str) -> Result<(), String> {
    let stem = id.strip_prefix("custom:").ok_or("Вбудований тип не можна видалити")?;
    if stem.contains('/') || stem.contains("..") {
        return Err("Неправильний тип".into());
    }
    let dir = custom_dir().ok_or("Папку типів нарад не знайдено")?;
    std::fs::remove_file(dir.join(format!("{stem}.json"))).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> MeetingTypeForm {
        MeetingTypeForm {
            id: None,
            name: "  Планування тижня ".into(),
            description: "".into(),
            report_fields: vec!["Зроблено".into(), " ".into(), "зроблено".into(), "Ризики".into()],
            parts: vec![
                MeetingPart { title: "Огляд".into(), cues: vec![" Почнемо з огляду ".into()], role: Some(" ".into()) },
                MeetingPart { title: " ".into(), cues: vec![], role: None },
            ],
            blocks: vec!["Завдання".into(), "Хід наради".into(), "Невідомий".into()],
            title_hints: vec!["Планування".into()],
            hours: vec![9, 30],
            controls: Some("немає такого".into()),
            group_by_speaker: true,
            style: STYLE_FIELDS.into(),
            detailed: true,
            wishes: "  Посади писати повністю ".into(),
            built_in: false,
        }
    }

    #[test]
    fn a_form_becomes_a_valid_type() {
        let t = template_of(&form()).unwrap();
        assert_eq!(t.name, "Планування тижня");
        let spec = t.meeting.unwrap();
        // Blocks in the form's order, unknown ones dropped
        assert_eq!(spec.blocks, vec!["Завдання", "Хід наради"]);
        assert_eq!(spec.report_fields, vec!["Зроблено", "Ризики"]);
        assert!(spec.classic && spec.ordered && spec.detailed);
        assert_eq!(spec.wishes.as_deref(), Some("Посади писати повністю"));
        assert_eq!(t.description, "Завдання, кожен доповідач за полями (зроблено, ризики).");
        assert_eq!(spec.parts.len(), 1);
        assert_eq!(spec.parts[0].cues, vec!["почнемо з огляду"]);
        assert_eq!(spec.parts[0].role, None);
        assert_eq!(spec.hours, vec![9]);
        assert_eq!(spec.controls, None);
        assert_eq!(spec.title_hints, vec!["планування"]);
    }

    #[test]
    fn a_form_needs_a_name_and_a_block() {
        let mut f = form();
        f.name = " ".into();
        assert!(template_of(&f).is_err());
        let mut f = form();
        f.blocks.clear();
        assert!(template_of(&f).is_err());
    }

    #[test]
    fn short_points_need_no_fields_and_keep_the_order() {
        let mut f = form();
        f.style = STYLE_POINTS.into();
        f.blocks = vec![OVERALL_BLOCK.into(), "Хід наради".into(), "Стенограма".into(), OVERALL_BLOCK.into()];
        let t = template_of(&f).unwrap();
        let spec = t.meeting.unwrap();
        assert!(!spec.classic && spec.ordered);
        assert!(spec.report_fields.is_empty());
        assert_eq!(spec.speaker_document_blocks(), vec![OVERALL_BLOCK, "Хід наради", "Стенограма"]);
        assert_eq!(t.description, "Загальний підсумок, підсумок кожного доповідача, повна стенограма.");
        // «За полями» with no field is refused
        let mut f = form();
        f.report_fields = vec![" ".into()];
        assert!(template_of(&f).is_err());
    }

    #[test]
    fn a_type_saved_before_shows_what_its_document_has() {
        // An old type: blocks listed, but the document is speakers' blocks and the overall summary
        let old = MeetingSpec { blocks: vec!["Хід наради".into(), "Рішення".into()], ..Default::default() };
        assert_eq!(old.speaker_document_blocks(), vec!["Хід наради", OVERALL_BLOCK]);
        let f = form_of("meeting_materials_uk").unwrap();
        assert_eq!(f.style, STYLE_POINTS);
        assert_eq!(f.blocks, vec!["Хід наради", OVERALL_BLOCK]);
        let f = form_of("daily_reports_fields_uk").unwrap();
        assert_eq!(f.style, STYLE_FIELDS);
        assert!(export("decisions_tasks_uk").unwrap().contains("Рішення та завдання"));
    }

    #[test]
    fn built_in_types_open_read_only_and_round_trip() {
        let f = form_of("meeting_materials_uk").unwrap();
        assert!(f.built_in);
        let t = template_of(&f).unwrap();
        assert_eq!(t.meeting.unwrap().blocks, f.blocks);
    }

    #[test]
    fn built_in_types_cannot_be_deleted() {
        assert!(delete("meeting_materials_uk").is_err());
        assert!(delete("custom:../x").is_err());
    }
}
