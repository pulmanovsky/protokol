//! A protocol that says only what was said.
//!
//! Each turn is read by the local model with its answer held to a JSON
//! schema (llama-helper turns it into a grammar): what the speaker reported,
//! decisions, tasks and open questions, every item with a verbatim quote
//! from the turn. Code then checks each item against the transcript:
//! - the quote must be (almost) word for word in the turn;
//! - numbers in the item must be in its quote (or, for a report, the turn);
//! - a deadline is kept only when its words were said;
//! - an owner must be a speaker, a role, or named in what was said.
//! Decisions and tasks that pass are asked once more, yes or no: does the
//! quote say this? What fails is dropped (decisions, tasks) or marked
//! «перевірте».

use serde::Deserialize;
use std::collections::HashSet;

/// Share of the quote's words that must be in the turn
pub const QUOTE_OVERLAP: f64 = 0.85;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct RawItem {
    pub kind: String,
    #[serde(default)]
    pub field: Option<String>,
    pub text: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub deadline: Option<String>,
    #[serde(default)]
    pub quote: String,
    #[serde(default)]
    pub time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Answer {
    #[serde(default)]
    items: Vec<RawItem>,
}

/// The JSON schema of a turn's answer. With report fields («Зроблено»,
/// «Проблеми»…) a report item names its field.
pub fn turn_schema(fields: &[String]) -> String {
    let mut item = serde_json::json!({
        "type": "object",
        "properties": {
            "kind": { "enum": ["report", "decision", "task", "question"] },
            "text": { "type": "string" },
            "owner": { "type": "string" },
            "deadline": { "type": "string" },
            "quote": { "type": "string" },
            "time": { "type": "string", "pattern": "^[0-9]{1,2}:[0-9]{2}(:[0-9]{2})?$" }
        },
        "required": ["kind", "text", "time"]
    });
    if !fields.is_empty() {
        item["properties"]["field"] = serde_json::json!({ "enum": fields });
    }
    serde_json::json!({
        "type": "object",
        "properties": { "items": { "type": "array", "items": item, "maxItems": 8 } },
        "required": ["items"]
    })
    .to_string()
}

/// The prompt for one turn; `reply` is what was said right after it (the
/// chair's answer often holds the assignment)
pub fn turn_prompt(speaker: &str, turn: &str, reply: &str, fields: &[String]) -> String {
    let report = if fields.is_empty() {
        "report — що доповів по суті: 1–3 пункти, кожен — одне коротке речення".to_string()
    } else {
        format!("report — що доповів по суті, з полем field — одне з: {}", fields.join(", "))
    };
    let reply = if reply.trim().is_empty() { String::new() } else { format!("\nПісля виступу (для контексту доручень):\n<reply>\n{reply}\n</reply>\n") };
    format!(
        r#"Нижче виступ одного учасника наради ({speaker}) з мітками часу.
<turn>
{turn}
</turn>
{reply}
Випиши пункти у JSON:
- {report};
- decision — що вирішили, погодили, призначили;
- task — що треба зробити після наради (пообіцяв мовець або доручили): text — коротка дія з дієслова в неозначеній формі («Підготувати…»); owner — кому, якщо названо; deadline — строк, лише якщо прозвучав;
- question — питання, що лишилося невирішеним.
Для decision, task і question обов'язково quote — ДОСЛІВНА цитата з виступу (5–15 слів), яка це підтверджує; для report quote не потрібна. time — мітка часу рядка, де це сказано.
Числа, дати, час і одиниці переписуй з виступу дослівно, як вони там записані: не став крапок чи ком між цифрами, не округлюй, не переводи в інші одиниці («4027» лишається «4027», не «40.27»).
Перекличку, привітання і «доповідь закінчив» не записуй. Нічого не вигадуй: чого немає у виступі, того немає у відповіді."#
    )
}

/// Items from the model's answer; None when it is not the expected JSON
pub fn parse_answer(answer: &str) -> Option<Vec<RawItem>> {
    let start = answer.find('{')?;
    let end = answer.rfind('}')?;
    let parsed: Answer = serde_json::from_str(&answer[start..=end]).ok()?;
    Some(parsed.items.into_iter().filter(|i| !i.text.trim().is_empty()).collect())
}

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’')
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

fn numbers(s: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            cur.push(c);
        } else if !cur.is_empty() {
            out.insert(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.insert(cur);
    }
    out
}

/// The text without its [MM:SS] marks (they are not numbers that were said)
fn without_times(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('[') {
        out.push_str(&rest[..i]);
        match rest[i..].find(']') {
            Some(j) if rest[i + 1..i + j].chars().all(|c| c.is_ascii_digit() || c == ':') => rest = &rest[i + j + 1..],
            _ => {
                out.push('[');
                rest = &rest[i + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn stem(w: &str) -> String {
    w.chars().take(5).collect()
}

/// Share of the quote's words found in the source
pub fn quote_overlap(quote: &str, source: &str) -> f64 {
    let q = tokens(quote);
    if q.is_empty() {
        return 0.0;
    }
    let src: HashSet<String> = tokens(source).into_iter().collect();
    q.iter().filter(|w| src.contains(*w)).count() as f64 / q.len() as f64
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Fine as it is (the item may have been trimmed: a deadline or owner removed)
    Keep(RawItem),
    /// Kept with «перевірте»
    Doubtful(RawItem, &'static str),
    /// Not said: left out
    Drop(&'static str),
}

/// Checks one item against the turn (`source` = the turn and the reply).
/// `people` are the meeting's speakers and roles.
pub fn verify(item: &RawItem, source: &str, people: &[String]) -> Verdict {
    let mut item = item.clone();
    let firm = matches!(item.kind.as_str(), "decision" | "task");
    let quote_ok = tokens(&item.quote).len() >= 3 && quote_overlap(&item.quote, source) >= QUOTE_OVERLAP;
    // A report needs no quote: its numbers are checked against the whole turn
    let needs_quote = firm || item.kind == "question" || !item.quote.trim().is_empty();
    if needs_quote && !quote_ok {
        return if firm || item.kind == "question" { Verdict::Drop("quote not in the turn") } else { Verdict::Doubtful(item, "quote not in the turn") };
    }
    // Numbers: a task or decision only with numbers from its quote; a report
    // with numbers that were said in the turn
    let allowed = if firm { numbers(&item.quote) } else { numbers(&without_times(source)) };
    if !numbers(&item.text).is_subset(&allowed) {
        return if firm { Verdict::Drop("numbers not said") } else { Verdict::Doubtful(item, "numbers not said") };
    }
    // A deadline only when its words were said
    if let Some(deadline) = item.deadline.clone() {
        let said = tokens(source).iter().map(|w| stem(w)).collect::<HashSet<_>>();
        let words: Vec<String> = tokens(&deadline).into_iter().filter(|w| w.chars().count() >= 3).collect();
        if deadline.trim().is_empty() || deadline.trim() == "—" || words.is_empty() || !words.iter().all(|w| said.contains(&stem(w)) || numbers(source).contains(w)) {
            item.deadline = None;
        }
    }
    // An owner: a speaker or role of the meeting, or named in what was said
    if let Some(owner) = item.owner.clone() {
        let o = owner.trim();
        if o.is_empty() || o == "—" {
            item.owner = None;
        } else {
            let said: HashSet<String> = tokens(source).iter().map(|w| stem(w)).collect();
            let known = people.iter().any(|p| p.to_lowercase() == o.to_lowercase())
                || tokens(o).iter().filter(|w| w.chars().count() >= 3).all(|w| said.contains(&stem(w)) || people.iter().any(|p| tokens(p).iter().any(|x| stem(x) == stem(w))));
            if !known {
                return Verdict::Doubtful(item, "owner not named");
            }
        }
    }
    Verdict::Keep(item)
}

/// The yes/no check: does the quote say this?
pub const YES_NO_SCHEMA: &str = r#"{"type":"object","properties":{"answer":{"enum":["так","ні"]}},"required":["answer"]}"#;

pub fn yes_no_prompt(item: &RawItem) -> String {
    let what = match item.kind.as_str() {
        "decision" => "рішення",
        "task" => "завдання",
        _ => "пункт",
    };
    format!(
        "Цитата з наради: «{}»\nТвердження ({what}): «{}»\nЧи цитата прямо каже це? Відповідай «так» або «ні».",
        item.quote.trim(),
        item.text.trim()
    )
}

/// «так» unless the answer clearly says «ні»
pub fn parse_yes(answer: &str) -> bool {
    !answer.to_lowercase().contains("\"ні\"") && !answer.trim().to_lowercase().starts_with("ні")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, text: &str, quote: &str) -> RawItem {
        RawItem { kind: kind.into(), field: None, text: text.into(), owner: None, deadline: None, quote: quote.into(), time: Some("01:20".into()) }
    }

    const TURN: &str = "[01:20] Склад отримав 12 генераторів. Завтра до обіду підготуємо заявку на пальне. Олена перевірить документи.";

    #[test]
    fn the_schema_is_a_valid_grammar_source() {
        let schema = turn_schema(&["Зроблено".to_string(), "Проблеми".to_string()]);
        let v: serde_json::Value = serde_json::from_str(&schema).unwrap();
        assert_eq!(v["properties"]["items"]["items"]["properties"]["field"]["enum"][1], "Проблеми");
        assert!(turn_schema(&[]).contains("\"quote\""));
        assert!(serde_json::from_str::<serde_json::Value>(YES_NO_SCHEMA).is_ok());
    }

    #[test]
    fn answers_parse() {
        let a = r#"{"items":[{"kind":"task","text":"Підготувати заявку на пальне","deadline":"завтра до обіду","quote":"Завтра до обіду підготуємо заявку на пальне","time":"01:20"}]}"#;
        let items = parse_answer(a).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].deadline.as_deref(), Some("завтра до обіду"));
        assert!(parse_answer("не JSON").is_none());
    }

    #[test]
    fn a_task_from_the_turn_is_kept_with_its_deadline() {
        let mut t = item("task", "Підготувати заявку на пальне", "Завтра до обіду підготуємо заявку на пальне");
        t.deadline = Some("завтра до обіду".into());
        let Verdict::Keep(k) = verify(&t, TURN, &[]) else { panic!() };
        assert_eq!(k.deadline.as_deref(), Some("завтра до обіду"));
    }

    #[test]
    fn made_up_things_are_dropped_or_marked() {
        // A quote that is not in the turn
        assert!(matches!(verify(&item("task", "Закупити столи", "закупимо нові столи для офісу"), TURN, &[]), Verdict::Drop(_)));
        // A number that was not said
        assert!(matches!(verify(&item("task", "Отримати 15 генераторів", "Склад отримав 12 генераторів"), TURN, &[]), Verdict::Drop(_)));
        // A deadline nobody said is removed, the task stays
        let mut t = item("task", "Підготувати заявку на пальне", "Завтра до обіду підготуємо заявку на пальне");
        t.deadline = Some("до п'ятниці".into());
        let Verdict::Keep(k) = verify(&t, TURN, &[]) else { panic!() };
        assert_eq!(k.deadline, None);
        // An owner named in the turn is fine; an unknown one is marked
        let mut t = item("task", "Перевірити документи", "Олена перевірить документи");
        t.owner = Some("Олена".into());
        assert!(matches!(verify(&t, TURN, &[]), Verdict::Keep(_)));
        t.owner = Some("Тарас".into());
        assert!(matches!(verify(&t, TURN, &[]), Verdict::Doubtful(_, _)));
        assert!(matches!(verify(&t, TURN, &["Тарас".to_string()]), Verdict::Keep(_)));
        // A report without a quote is checked by its numbers against the turn
        assert!(matches!(verify(&item("report", "Отримано 12 генераторів", ""), TURN, &[]), Verdict::Keep(_)));
        assert!(matches!(verify(&item("report", "Отримано 20 генераторів", ""), TURN, &[]), Verdict::Doubtful(_, _)));
        // A report with a number from the turn is fine
        assert!(matches!(verify(&item("report", "Отримано 12 генераторів", "Склад отримав 12 генераторів"), TURN, &[]), Verdict::Keep(_)));
    }

    #[test]
    fn yes_or_no() {
        assert!(parse_yes(r#"{"answer":"так"}"#));
        assert!(!parse_yes(r#"{"answer":"ні"}"#));
        assert!(yes_no_prompt(&item("task", "Підготувати заявку", "підготуємо заявку")).contains("Чи цитата прямо каже це?"));
    }
}
