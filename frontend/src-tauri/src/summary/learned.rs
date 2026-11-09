//! What the app learns from the secretary's approved protocols.
//!
//! When a protocol is approved, the document the app wrote (the draft) is
//! compared with the approved one: tasks the secretary removed (the app was
//! wrong), added (the app missed them) or reworded, reports rewritten. Each
//! approved item is tied to the turn it came from, and «this turn → this is
//! how it was written up» is kept as an example for the meeting type.
//! When the next meeting of that type is written up, each turn gets the two
//! or three most similar examples in its prompt («як секретар оформив схожі
//! виступи раніше»). Nothing retrains the model; everything stays local.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

/// A protocol as written: reports, tasks, decisions with their moments
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Doc {
    pub reports: Vec<DocReport>,
    pub tasks: Vec<DocTask>,
    pub decisions: Vec<(String, Option<u32>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DocReport {
    pub speaker: String,
    pub time: Option<u32>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocTask {
    pub who: String,
    pub what: String,
    pub when: String,
    pub time: Option<u32>,
}

fn time_of(s: &str) -> Option<u32> {
    let i = s.find('[')?;
    let j = s[i..].find(']')? + i;
    let p: Vec<u32> = s[i + 1..j].split(';').next().unwrap_or("").split(':').map(|x| x.trim().parse().ok()).collect::<Option<Vec<_>>>()?;
    match p.as_slice() {
        [m, s] => Some(m * 60 + s),
        [h, m, s] => Some(h * 3600 + m * 60 + s),
        _ => None,
    }
}

fn strip_time(s: &str) -> String {
    match (s.find('['), s.rfind(']')) {
        (Some(i), Some(j)) if j > i && time_of(&s[i..=j]).is_some() => format!("{}{}", &s[..i], &s[j + 1..]).trim().trim_end_matches('—').trim().to_string(),
        _ => s.trim().to_string(),
    }
}

/// Reads a protocol in the app's format (also after the secretary's edits)
pub fn parse_doc(md: &str) -> Doc {
    let md = &super::materials::without_source_marks(md);
    let mut doc = Doc::default();
    let mut section = String::new();
    for line in md.lines() {
        let l = line.trim();
        if let Some(h) = l.strip_prefix("## ") {
            section = h.trim().to_string();
            continue;
        }
        if let Some(rest) = l.strip_prefix("**ВИСТУПИЛИ:**") {
            let rest = rest.trim();
            let time = time_of(rest);
            let (head, body) = match rest.find(" — ") {
                Some(i) => (&rest[..i], rest[i + " — ".len()..].trim().to_string()),
                None => (rest, String::new()),
            };
            let speaker = head.split('[').next().unwrap_or(head).trim().to_string();
            doc.reports.push(DocReport { speaker, time, body });
            continue;
        }
        if section == "Хід наради" && l.starts_with("- ") {
            if let Some(last) = doc.reports.last_mut() {
                if !last.body.is_empty() {
                    last.body.push('\n');
                }
                last.body.push_str(&l[2..]);
            }
            continue;
        }
        if section == "Завдання" && l.starts_with('|') && !l.starts_with("| Хто") && !l.starts_with("| ---") {
            let cells: Vec<String> = l.trim_matches('|').split('|').map(|c| c.trim().to_string()).collect();
            if cells.len() >= 2 && !cells[1].is_empty() {
                let time = cells.get(3).and_then(|c| time_of(c));
                doc.tasks.push(DocTask { who: cells[0].clone(), what: cells[1].clone(), when: cells.get(2).cloned().unwrap_or_default(), time });
            }
            continue;
        }
        if section == "Рішення" && l.starts_with("- ") {
            doc.decisions.push((strip_time(&l[2..]), time_of(l)));
        }
    }
    doc
}

fn stems(s: &str) -> HashSet<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(|w| w.to_lowercase().chars().take(5).collect())
        .collect()
}

/// How alike two texts are (shared word stems)
pub fn likeness(a: &str, b: &str) -> f64 {
    let (x, y) = (stems(a), stems(b));
    if x.is_empty() || y.is_empty() {
        return 0.0;
    }
    x.intersection(&y).count() as f64 / x.len().min(y.len()) as f64
}

/// What the secretary changed
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edit {
    TaskAdded(DocTask),
    TaskRemoved(DocTask),
    TaskReworded { before: DocTask, after: DocTask },
    ReportReworded { speaker: String, time: Option<u32>, before: String, after: String },
}

/// Draft vs approved protocol
pub fn diff(draft: &Doc, approved: &Doc) -> Vec<Edit> {
    let mut edits = Vec::new();
    let mut used = vec![false; draft.tasks.len()];
    for t in &approved.tasks {
        let best = draft
            .tasks
            .iter()
            .enumerate()
            .filter(|(i, _)| !used[*i])
            .map(|(i, d)| (i, likeness(&d.what, &t.what)))
            .filter(|(_, s)| *s >= 0.5)
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((i, _)) => {
                used[i] = true;
                let d = &draft.tasks[i];
                if d.what != t.what || d.who != t.who || d.when != t.when {
                    edits.push(Edit::TaskReworded { before: d.clone(), after: t.clone() });
                }
            }
            None => edits.push(Edit::TaskAdded(t.clone())),
        }
    }
    for (i, d) in draft.tasks.iter().enumerate() {
        if !used[i] {
            edits.push(Edit::TaskRemoved(d.clone()));
        }
    }
    for a in &approved.reports {
        if let Some(d) = draft.reports.iter().find(|d| d.time == a.time && d.time.is_some()) {
            if d.body.trim() != a.body.trim() && !a.body.trim().is_empty() {
                edits.push(Edit::ReportReworded { speaker: a.speaker.clone(), time: a.time, before: d.body.clone(), after: a.body.clone() });
            }
        }
    }
    edits
}

/// A turn of the transcript: who spoke, from when, what was said
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub speaker: String,
    pub start: u32,
    pub text: String,
}

/// The turn an item belongs to: by its moment, otherwise by its words
pub fn link(turns: &[Turn], time: Option<u32>, text: &str) -> Option<usize> {
    if let Some(t) = time {
        if let Some(i) = turns.iter().rposition(|x| x.start <= t) {
            return Some(i);
        }
    }
    turns
        .iter()
        .enumerate()
        .map(|(i, x)| (i, likeness(text, &x.text)))
        .filter(|(_, s)| *s >= 0.3)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// One learned example: a turn and how the secretary wrote it up
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Example {
    pub meeting_id: String,
    pub meeting_type: String,
    pub speaker: String,
    #[serde(default)]
    pub role: Option<String>,
    pub turn_text: String,
    /// The approved write-up: the report and its tasks, as in the protocol
    pub approved: String,
    /// What the secretary changed for this turn (added/removed/reworded)
    pub edits: Vec<String>,
}

/// The longest piece of a turn kept in an example
const EXAMPLE_TURN_CHARS: usize = 700;

fn shorten(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let cut: String = s.chars().take(n).collect();
    format!("{}…", cut.rsplit_once(' ').map(|(a, _)| a).unwrap_or(&cut))
}

/// Examples from an approved protocol: every turn that has an approved
/// report or task, with what the secretary changed there
pub fn examples_from(meeting_id: &str, meeting_type: &str, turns: &[Turn], roles: &HashMap<String, String>, draft: &Doc, approved: &Doc) -> Vec<Example> {
    let edits = diff(draft, approved);
    let mut per_turn: HashMap<usize, (Vec<String>, Vec<String>)> = HashMap::new();
    for r in &approved.reports {
        if let Some(i) = link(turns, r.time, &r.body) {
            if !r.body.trim().is_empty() {
                per_turn.entry(i).or_default().0.push(r.body.clone());
            }
        }
    }
    for t in &approved.tasks {
        if let Some(i) = link(turns, t.time, &t.what) {
            per_turn.entry(i).or_default().0.push(format!("Завдання: {} | {} | {}", t.what, t.who, t.when));
        }
    }
    for e in &edits {
        let (time, text, label) = match e {
            Edit::TaskAdded(t) => (t.time, t.what.clone(), format!("секретар додав завдання «{}»", t.what)),
            Edit::TaskRemoved(t) => (t.time, t.what.clone(), format!("секретар прибрав зайве завдання «{}»", t.what)),
            Edit::TaskReworded { before, after } => {
                let mut changes = Vec::new();
                if before.what != after.what {
                    changes.push(format!("«{}» → «{}»", before.what, after.what));
                }
                if before.who != after.who {
                    changes.push(format!("виконавець «{}» → «{}»", before.who, after.who));
                }
                if before.when != after.when {
                    changes.push(format!("строк «{}» → «{}»", before.when, after.when));
                }
                (after.time, after.what.clone(), format!("секретар виправив завдання: {}", changes.join(", ")))
            }
            Edit::ReportReworded { time, after, .. } => (*time, after.clone(), "секретар переписав підсумок".to_string()),
        };
        if let Some(i) = link(turns, time, &text) {
            per_turn.entry(i).or_default().1.push(label);
        }
    }
    let mut out: Vec<(usize, Example)> = per_turn
        .into_iter()
        .filter(|(_, (items, _))| !items.is_empty())
        .map(|(i, (items, edits))| {
            let t = &turns[i];
            (
                i,
                Example {
                    meeting_id: meeting_id.to_string(),
                    meeting_type: meeting_type.to_string(),
                    speaker: t.speaker.clone(),
                    role: roles.get(&t.speaker).cloned(),
                    turn_text: shorten(&t.text, EXAMPLE_TURN_CHARS),
                    approved: items.join("\n"),
                    edits,
                },
            )
        })
        .collect();
    out.sort_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, e)| e).collect()
}

/// The 2–3 most similar examples of the same meeting type for a turn
/// (shared words; the same role or speaker counts extra), never from the
/// meeting being written up
pub fn similar<'a>(examples: &'a [Example], meeting_type: &str, exclude_meeting: Option<&str>, speaker: &str, role: Option<&str>, turn: &str, k: usize) -> Vec<&'a Example> {
    let mut scored: Vec<(f64, &Example)> = examples
        .iter()
        .filter(|e| e.meeting_type == meeting_type && Some(e.meeting_id.as_str()) != exclude_meeting)
        .map(|e| {
            let mut s = likeness(turn, &e.turn_text);
            if role.is_some() && e.role.as_deref() == role {
                s += 0.2;
            } else if e.speaker == speaker {
                s += 0.1;
            }
            (s, e)
        })
        .filter(|(s, _)| *s >= 0.15)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.into_iter().take(k).map(|(_, e)| e).collect()
}

/// The examples as a prompt block, at most `max_chars` long
pub fn prompt_block(examples: &[&Example], max_chars: usize) -> String {
    if examples.is_empty() {
        return String::new();
    }
    let mut out = String::from("\nПриклади, як секретар оформив схожі виступи раніше (дотримуйся такого ж стилю й повноти, але бери факти лише з поточного виступу):\n");
    for e in examples {
        let mut block = format!("---\nВиступ: {}\nОформлено:\n{}\n", shorten(&e.turn_text, 400), e.approved);
        if !e.edits.is_empty() {
            block.push_str(&format!("Що виправив секретар: {}\n", e.edits.join("; ")));
        }
        if out.chars().count() + block.chars().count() > max_chars {
            break;
        }
        out.push_str(&block);
    }
    if out.ends_with(":\n") {
        return String::new();
    }
    out.push_str("---\n");
    out
}

static CURRENT: once_cell::sync::Lazy<RwLock<(Vec<Example>, Option<String>)>> = once_cell::sync::Lazy::new(|| RwLock::new((Vec::new(), None)));

/// The examples for the document being written now, and its meeting (left out)
pub fn set_current(examples: Vec<Example>, meeting_id: Option<String>) {
    if let Ok(mut slot) = CURRENT.write() {
        *slot = (examples, meeting_id);
    }
}

pub fn current() -> (Vec<Example>, Option<String>) {
    CURRENT.read().map(|c| c.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRAFT: &str = "# Нарада\n\n## Хід наради\n\n**ВИСТУПИЛИ:** Спікер 2 [01:00]\n- Зроблено: Склад готовий.\n\n**ВИСТУПИЛИ:** Спікер 3 [03:00] — Доповів про зв'язок.\n\n## Завдання\n\n| Хто | Що | До коли | Час у записі |\n| --- | --- | --- | --- |\n| Спікер 2 | Підготувати заявку на пальне | — | [01:10] |\n| Спікер 3 | Перевірити генератор | — | [03:05] |\n\n## Рішення\n\n- Призначити чергового [03:20]\n";
    const FINAL: &str = "# Нарада\n\n## Хід наради\n\n**ВИСТУПИЛИ:** Спікер 2 [01:00]\n- Зроблено: Склад готовий до прийому.\n\n**ВИСТУПИЛИ:** Спікер 3 [03:00] — Доповів про зв'язок.\n\n## Завдання\n\n| Хто | Що | До коли | Час у записі |\n| --- | --- | --- | --- |\n| Спікер 2 | Подати заявку на пальне | до вечора | [01:10] |\n| Спікер 3 | Замінити антену | завтра | [03:30] |\n";

    fn turns() -> Vec<Turn> {
        vec![
            Turn { speaker: "Я".into(), start: 0, text: "Почнемо нараду.".into() },
            Turn { speaker: "Спікер 2".into(), start: 60, text: "Склад готовий до прийому. Сьогодні подамо заявку на пальне до вечора.".into() },
            Turn { speaker: "Спікер 3".into(), start: 180, text: "Зв'язок працює. Генератор перевірили. Завтра заміню антену.".into() },
        ]
    }

    #[test]
    fn protocols_parse() {
        let d = parse_doc(DRAFT);
        assert_eq!(d.reports.len(), 2);
        assert_eq!(d.reports[0].body, "Зроблено: Склад готовий.");
        assert_eq!(d.reports[1].body, "Доповів про зв'язок.");
        assert_eq!(d.tasks[1], DocTask { who: "Спікер 3".into(), what: "Перевірити генератор".into(), when: "—".into(), time: Some(185) });
        assert_eq!(d.decisions, vec![("Призначити чергового".to_string(), Some(200))]);
    }

    #[test]
    fn the_secretarys_changes_are_found() {
        let e = diff(&parse_doc(DRAFT), &parse_doc(FINAL));
        assert!(e.iter().any(|x| matches!(x, Edit::TaskReworded { after, .. } if after.what == "Подати заявку на пальне")));
        assert!(e.iter().any(|x| matches!(x, Edit::TaskAdded(t) if t.what == "Замінити антену")));
        assert!(e.iter().any(|x| matches!(x, Edit::TaskRemoved(t) if t.what == "Перевірити генератор")));
        assert!(e.iter().any(|x| matches!(x, Edit::ReportReworded { after, .. } if after.contains("до прийому"))));
    }

    #[test]
    fn items_link_to_their_turns() {
        let t = turns();
        assert_eq!(link(&t, Some(70), ""), Some(1));
        assert_eq!(link(&t, None, "Замінити антену"), Some(2));
        assert_eq!(link(&t, None, "щось зовсім інше"), None);
    }

    #[test]
    fn examples_are_kept_per_turn_and_retrieved() {
        let mut roles = HashMap::new();
        roles.insert("Спікер 3".to_string(), "зв'язок".to_string());
        let ex = examples_from("m1", "daily", &turns(), &roles, &parse_doc(DRAFT), &parse_doc(FINAL));
        assert_eq!(ex.len(), 2);
        assert!(ex[1].approved.contains("Завдання: Замінити антену | Спікер 3 | завтра"));
        assert!(ex[1].edits.iter().any(|e| e.contains("додав")));
        assert_eq!(ex[1].role.as_deref(), Some("зв'язок"));
        // A similar turn of the same type finds it; another type or the same meeting does not
        let found = similar(&ex, "daily", Some("m2"), "Спікер 9", Some("зв'язок"), "Зв'язок працює, антену заміню завтра.", 2);
        assert_eq!(found[0].speaker, "Спікер 3");
        assert!(similar(&ex, "general", None, "x", None, "Зв'язок працює", 2).is_empty());
        assert!(similar(&ex, "daily", Some("m1"), "x", None, "Зв'язок працює", 2).is_empty());
    }

    #[test]
    fn the_prompt_block_keeps_to_its_budget() {
        let ex = examples_from("m1", "daily", &turns(), &HashMap::new(), &parse_doc(DRAFT), &parse_doc(FINAL));
        let refs: Vec<&Example> = ex.iter().collect();
        let block = prompt_block(&refs, 2000);
        assert!(block.contains("Приклади, як секретар оформив"));
        assert!(block.contains("Оформлено:"));
        let small = prompt_block(&refs, 300);
        assert!(small.chars().count() <= 300 + 4);
        assert!(prompt_block(&refs, 50).is_empty());
        assert!(prompt_block(&[], 2000).is_empty());
    }
}
