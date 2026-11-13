//! «Матеріали наради»: what a 12B model does reliably, instead of a written
//! protocol.
//!
//! The model never rewrites what was said. It only
//! - marks where each report starts and who speaks (when that is said), and
//! - lists decisions, tasks and open questions, each pointing to a moment in
//!   the recording.
//! The transcript under each report is the verbatim recognised text with
//! rule-based cleanup (fillers, immediate repeats). Model output is checked
//! against the transcript: boundaries must exist and run forward, and items
//! without a matching moment are dropped. The secretary writes the rest.

use std::collections::HashSet;

use super::templates::{MeetingPart, MeetingSpec, OVERALL_BLOCK};

/// One transcript line "[MM:SS] text"
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Line {
    pub seconds: u32,
    pub text: String,
}

/// A report: verbatim lines from `start` (index into lines) up to the next one
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Report {
    pub start: usize,
    pub speaker: String,
}

/// A decision, task or open question with the moment it was said
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Item {
    pub fields: Vec<String>,
    pub seconds: u32,
}

/// Fallback block length when the model's segmentation is unusable
const FALLBACK_BLOCK_S: u32 = 180;
/// An item's timestamp must be this close to a transcript line
const MATCH_WINDOW_S: u32 = 90;

pub(crate) fn mmss(seconds: u32) -> String {
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

/// The mark of where a point of the document comes from: ⟨02:14⟩ (several
/// moments ⟨02:14 · 12:06⟩), ⟨?⟩ when no line of the speaker says it. Shown
/// in the app (a click opens the source), left out of DOCX and PDF.
pub(crate) const NO_SOURCE: &str = "⟨?⟩";

pub(crate) fn source_mark(times: &[u32]) -> String {
    if times.is_empty() {
        return NO_SOURCE.to_string();
    }
    format!("⟨{}⟩", times.iter().map(|t| mmss(*t)).collect::<Vec<_>>().join(" · "))
}

/// The text without source marks
pub(crate) fn without_source_marks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for c in text.chars() {
        match c {
            '⟨' => inside = true,
            '⟩' if inside => inside = false,
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    // A space left before punctuation or at the end
    out.replace(" ;", ";").replace(" .", ".").replace(" ,", ",").lines().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n")
}

/// The source marks of a point: its moments, and whether it has none (⟨?⟩)
pub(crate) fn source_times(text: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find('⟨') {
        let Some(j) = rest[i..].find('⟩') else { break };
        for t in rest[i + '⟨'.len_utf8()..i + j].split('·') {
            let p: Vec<u32> = t.trim().split(':').filter_map(|x| x.parse().ok()).collect();
            if let [m, s] = p.as_slice() {
                out.push(m * 60 + s);
            }
        }
        rest = &rest[i + j..];
    }
    out
}

/// The line of a turn that says a point: most shared word stems (numbers
/// must match whole); none when too little is shared
pub(crate) fn source_of(point: &str, turn: &[&Line]) -> Option<u32> {
    let (stems, numbers) = point_words(point);
    if stems.is_empty() && numbers.is_empty() {
        return None;
    }
    let mut best: Option<(f64, u32)> = None;
    for line in turn {
        let (ls, ln) = point_words(&line.text);
        let shared = stems.intersection(&ls).count() + numbers.intersection(&ln).count();
        let score = shared as f64 / (stems.len() + numbers.len()) as f64;
        if shared >= 2.min(stems.len() + numbers.len()) && score >= 0.3 && best.map_or(true, |(b, _)| score > b) {
            best = Some((score, line.seconds));
        }
    }
    best.map(|(_, t)| t)
}

/// Each point of every turn's write-up gets the moment it was said:
/// «Зроблено: a; b» → «Зроблено: a ⟨02:14⟩; b ⟨02:40⟩»; free sentences too.
/// «Не враховано» quotes are the speaker's own words and need none.
pub(crate) fn add_sources(lines: &[Line], reports: &[Report], fields: &mut std::collections::HashMap<usize, Vec<(String, String)>>, summaries: &mut std::collections::HashMap<usize, String>) {
    let turn_of = |i: usize| -> Vec<&Line> {
        let start = reports[i].start;
        let end = reports.get(i + 1).map(|r| r.start).unwrap_or(lines.len());
        lines[start..end.max(start)].iter().collect()
    };
    let mark = |point: &str, turn: &[&Line]| -> String {
        let p = point.trim();
        if p.is_empty() {
            return String::new();
        }
        format!("{} {}", p, source_mark(&source_of(p, turn).into_iter().collect::<Vec<_>>()))
    };
    for (i, values) in fields.iter_mut() {
        let turn = turn_of(*i);
        for (field, value) in values.iter_mut() {
            if field == NOT_REFLECTED_FIELD {
                continue;
            }
            *value = value.split(';').map(|p| mark(p, &turn)).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("; ");
        }
    }
    for (i, text) in summaries.iter_mut() {
        let turn = turn_of(*i);
        *text = text.split_inclusive(['.', '!', '?']).map(|s| mark(s, &turn)).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
    }
}

pub(crate) const NOT_REFLECTED_FIELD: &str = "Не враховано";

fn parse_mmss(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches('[').trim_end_matches(']');
    let parts: Vec<&str> = s.split(':').collect();
    match parts.as_slice() {
        [m, sec] => Some(m.trim().parse::<u32>().ok()? * 60 + sec.trim().parse::<u32>().ok()?),
        [h, m, sec] => Some(h.trim().parse::<u32>().ok()? * 3600 + m.trim().parse::<u32>().ok()? * 60 + sec.trim().parse::<u32>().ok()?),
        _ => None,
    }
}

/// Parses "[MM:SS] text" lines (lines without a time join the previous one)
pub(crate) fn parse_transcript(text: &str) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    for raw in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let parsed = raw
            .strip_prefix('[')
            .and_then(|r| r.split_once(']'))
            .and_then(|(t, rest)| parse_mmss(t).map(|s| (s, rest.trim().to_string())));
        match (parsed, lines.last_mut()) {
            (Some((seconds, text)), _) => lines.push(Line { seconds, text }),
            (None, Some(last)) => {
                last.text.push(' ');
                last.text.push_str(raw);
            }
            (None, None) => lines.push(Line { seconds: 0, text: raw.to_string() }),
        }
    }
    lines
}

/// Rule-based cleanup of spoken text: fillers and immediate word repeats.
/// Nothing is reworded.
pub(crate) fn clean_spoken(text: &str) -> String {
    const FILLERS: &[&str] = &["е-е", "е-е-е", "ееее", "еее", "ее", "ем", "ммм", "мм", "ааа", "типу", "ну"];
    let norm = |w: &str| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-').to_lowercase();
    let mut out: Vec<&str> = Vec::new();
    for word in text.split_whitespace() {
        let n = norm(word);
        if FILLERS.contains(&n.as_str()) && word.chars().all(|c| c.is_alphabetic() || c == '-' || c == ',') {
            continue;
        }
        if let Some(prev) = out.last() {
            if norm(prev) == n && !n.is_empty() {
                continue;
            }
        }
        out.push(word);
    }
    let joined = out.join(" ");
    // Capitalise the first letter if a filler was removed from the start
    let mut chars = joined.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => joined,
    }
}

// ---------------------------------------------------------------------------
// Speakers recognised by voice
// ---------------------------------------------------------------------------

/// The speaker part of a «Спікер 2: text» line, if it looks like one
fn speaker_prefix(text: &str) -> Option<(&str, &str)> {
    let (who, rest) = text.split_once(": ")?;
    let who = who.trim();
    // A voice nobody has named yet
    if who == "?" {
        return Some((who, rest.trim()));
    }
    // «Ім'я (?)» and «Ім'я?» are suggested names, not confirmed yet
    let core = who.trim_end_matches("(?)").trim().trim_end_matches('?').trim();
    let ok = !core.is_empty()
        && who.chars().count() <= 40
        && core.split_whitespace().count() <= 4
        && core.chars().next().is_some_and(|c| c.is_uppercase())
        && !core.contains(['.', ',', '!', '?']);
    ok.then_some((who, rest.trim()))
}

/// When the transcript lines carry speakers («[MM:SS] Спікер 2: text», from
/// speaker recognition by voice), strips them from the lines and returns one
/// report per change of speaker. None when most lines have no speaker.
/// Phrases that end one's report
const REPORT_CLOSINGS: &[&str] = &[
    "в мене все", "у мене все", "в мене наразі все", "у мене наразі все", "наразі все", "доповідь закінчив", "доповідь закінчила",
    "доповідь закінчено", "доповідь завершив", "доповідь завершила", "доповідь закінчую", "дякую, все", "на цьому все", "в мене поки все",
];
/// Phrases that open a report
const REPORT_OPENINGS: &[&str] = &["доброго ранку", "добрий ранок", "добрий день", "доброго дня", "добрий вечір", "вітаю", "шановні колеги", "шановний колего"];

/// One voice, two reports: a line that closes a report («…в мене все»)
/// followed by one that opens another («Доброго ранку…»). Two people whose
/// voices sound alike, or one person speaking twice: either way two reports.
pub(crate) fn opens_new_report(previous: &str, line: &str) -> bool {
    let tail: String = previous.split_whitespace().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" ").to_lowercase();
    let head: String = line.split_whitespace().take(5).collect::<Vec<_>>().join(" ").to_lowercase();
    REPORT_CLOSINGS.iter().any(|c| tail.contains(c)) && (REPORT_OPENINGS.iter().any(|o| head.contains(o)) || super::pack::current().report_openings.iter().any(|o| head.contains(o.as_str())))
}

pub(crate) fn speaker_reports(lines: &mut [Line]) -> Option<Vec<Report>> {
    let with_speaker = lines.iter().filter(|l| speaker_prefix(&l.text).is_some()).count();
    if lines.is_empty() || with_speaker * 5 < lines.len() * 4 {
        return None;
    }
    let mut reports: Vec<Report> = Vec::new();
    let mut previous_text = String::new();
    for (i, line) in lines.iter_mut().enumerate() {
        let Some((who, rest)) = speaker_prefix(&line.text).map(|(w, r)| (w.to_string(), r.to_string())) else { continue };
        line.text = rest;
        // Short remarks from the room do not interrupt a report
        if who == "Репліки" && !reports.is_empty() {
            continue;
        }
        let same_voice_new_report = i > 0
            && reports.last().is_some_and(|r| r.speaker == who)
            && opens_new_report(&previous_text, &line.text);
        if reports.last().map(|r| r.speaker != who).unwrap_or(true) || same_voice_new_report {
            reports.push(Report { start: i, speaker: who });
        }
        previous_text = line.text.clone();
    }
    if reports.first().map(|r| r.start) != Some(0) {
        reports.insert(0, Report { start: 0, speaker: "Доповідач".into() });
    }
    Some(reports)
}

/// Asks who the unnamed voices («Спікер N») are, from how people address and
/// introduce each other. The answer is «Спікер N | ім'я» lines.
pub(crate) fn speaker_names_user_prompt(transcript: &str) -> String {
    format!(
        r#"Нижче стенограма наради. Мовців розпізнано за голосом і позначено «Спікер N», «Я» — це той, хто записував нараду.
<transcript>
{transcript}
</transcript>

Для кожного «Спікер N» визнач ім'я, псевдонім або посаду, лише якщо це однозначно видно зі стенограми: людина сама назвалася («це Олена», «доповідає зв'язок»), або до неї звернулися безпосередньо перед її виступом («Олено, тобі слово», «слово має Петренко»). Якщо сумнів — не пиши нічого для цього спікера. Нічого не вигадуй.
Виведи лише рядки у форматі:
Спікер N | ім'я"#
    )
}

/// Parses «Спікер N | ім'я» answers, keeping only names that appear in the
/// transcript right before or inside one of that speaker's reports.
pub(crate) fn parse_speaker_names(output: &str, lines: &[Line], reports: &[Report]) -> Vec<(String, String)> {
    let mut names: Vec<(String, String)> = Vec::new();
    for row in output.lines() {
        let Some((label, name)) = row.split_once('|') else { continue };
        let label = label.trim().trim_matches('*').trim().to_string();
        let Some(name) = clean_speaker(name) else { continue };
        if !label.starts_with("Спікер ") || names.iter().any(|(l, _)| *l == label) {
            continue;
        }
        let stem: String = name.split_whitespace().next().unwrap_or("").chars().take(4).collect::<String>().to_lowercase();
        if stem.chars().count() < 3 {
            continue;
        }
        // The name must be said as an introduction or an address: in the
        // first words of one of their reports («Доброго ранку, Склад…»), or in
        // the last words right before it («…Олено, тобі слово»). A name said
        // later in someone's report is about another person.
        let has = |words: &[&str]| words.iter().any(|w| w.to_lowercase().contains(&stem));
        let said = reports.iter().filter(|r| r.speaker == label).any(|r| {
            let own: Vec<&str> = lines[r.start].text.split_whitespace().take(8).collect();
            let before: Vec<&str> = if r.start > 0 {
                let w: Vec<&str> = lines[r.start - 1].text.split_whitespace().collect();
                w[w.len().saturating_sub(12)..].to_vec()
            } else {
                Vec::new()
            };
            has(&own) || has(&before)
        });
        if said {
            names.push((label, name));
        }
    }
    names
}

/// Replaces «Спікер N» by the names found (one name per voice)
pub(crate) fn apply_speaker_names(reports: &mut [Report], names: &[(String, String)]) {
    for r in reports.iter_mut() {
        if let Some((_, name)) = names.iter().find(|(l, _)| *l == r.speaker) {
            r.speaker = name.clone();
        }
    }
}

/// Reports at least this long get a few sentences in «Хід наради»
pub(crate) const MIN_SUMMARISED_REPORT_S: u32 = 10;

/// Reports long enough for «Хід наради», with their text
pub(crate) fn reports_to_summarise(lines: &[Line], reports: &[Report]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (i, r) in reports.iter().enumerate() {
        let end = reports.get(i + 1).map(|n| n.start).unwrap_or(lines.len());
        if r.start >= end {
            continue;
        }
        let until = lines.get(end).map(|l| l.seconds).unwrap_or_else(|| lines[end - 1].seconds + 15);
        if until.saturating_sub(lines[r.start].seconds) < MIN_SUMMARISED_REPORT_S {
            continue;
        }
        let text = lines[r.start..end].iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" ");
        // A roll call or a word or two («Дякую», «Плюс») is not a report
        if is_roll_call(&text) || is_interjection(&text) {
            continue;
        }
        out.push((i, clean_spoken(&text)));
    }
    out
}

/// A roll call («С1. На зв'язку. С2. Присутній…»): nothing to summarise
pub(crate) fn is_roll_call(text: &str) -> bool {
    let lower = text.to_lowercase();
    let marks = ["на зв'язку", "на звʼязку", "присутній", "присутня", "на місці", "є."].iter().map(|m| lower.matches(m).count()).sum::<usize>();
    let words = lower.split_whitespace().count().max(1);
    marks >= 4 && marks * 12 >= words
}

/// One report in 2–4 sentences, only what was said
pub(crate) fn report_summary_user_prompt(speaker: &str, text: &str) -> String {
    format!(
        r#"Нижче виступ одного учасника наради ({speaker}).
<report>
{text}
</report>

Напиши рівно так:
Коротко: 1–2 речення (довгий виступ — до 3): головне, що сказав, важливі цифри, що просить або пропонує. Не починай з імені чи «Він/Вона». Числа, дати й одиниці переписуй дослівно, як у виступі (не став крапок чи ком між цифрами, не округлюй).
Завдання:
- <що зробити: 3–10 слів, з дієслова в неозначеній формі> | <кому, якщо доручено іншому; інакше —> | <до коли, якщо сказано; інакше —>

У «Завдання» — лише те, що мовець пообіцяв або планує зробити («зробимо», «сьогодні будемо», «підготую»), і те, що він доручив іншим. Не включай уже зроблене і загальні слова без конкретної дії. Якщо завдань немає — напиши «Завдання: немає». Нічого не вигадуй."#
    )
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// A summary that starts with the speaker's own label repeats the heading:
/// «Спікер 2 повідомив про…» → «Повідомив про…»
pub(crate) fn without_leading_speaker(summary: &str, speaker: &str) -> String {
    match summary.strip_prefix(speaker) {
        Some(rest) if rest.starts_with(' ') => capitalised(rest.trim_start()),
        _ => summary.to_string(),
    }
}

/// The summary and the speaker's own tasks from a per-report answer. Tasks
/// without an owner are left with «—» (the speaker is filled in later).
pub(crate) fn parse_report_summary(answer: &str, seconds: u32) -> (String, Vec<Item>) {
    let (head, tail) = match answer.find("Завдання") {
        Some(i) => (&answer[..i], &answer[i..]),
        None => (answer, ""),
    };
    let summary = head.trim().trim_start_matches("Коротко:").trim().to_string();
    let mut tasks = Vec::new();
    for line in tail.lines().skip(1).chain(tail.lines().next().and_then(|l| l.split_once(':')).map(|(_, r)| r).into_iter()) {
        let line = line.trim().trim_start_matches(['-', '*', '•']).trim();
        if line.is_empty() || line.to_lowercase().starts_with("немає") {
            continue;
        }
        let mut fields: Vec<String> = line.split('|').map(|f| f.trim().trim_end_matches('.').to_string()).collect();
        if fields.first().map(|w| w.split_whitespace().count() < 3).unwrap_or(true) {
            continue;
        }
        while fields.len() < 3 {
            fields.push("—".into());
        }
        fields.truncate(3);
        fields[0] = capitalised(&fields[0]);
        tasks.push(Item { fields, seconds });
    }
    (summary, tasks)
}

// ---------------------------------------------------------------------------
// Prompts
// ---------------------------------------------------------------------------

pub(crate) fn segmentation_user_prompt(transcript: &str) -> String {
    format!(
        r#"Нижче стенограма наради з мітками часу.
<transcript>
{transcript}
</transcript>

Визнач, де починається кожна нова доповідь або виступ (зазвичай після «доповідь закінчив», «дякую», «далі», «слово має…», «доповідає…»). Для кожної виведи один рядок:
ММ:СС | хто

«ММ:СС» — мітка часу рядка, з якого починається доповідь, точно як у стенограмі.
«хто» — ім'я, псевдонім, відділ або посада, лише якщо їх явно названо (наприклад «слово має Петренко», «доповідає зв'язок»). Якщо не названо — «?». Нічого не вигадуй.
Виведи лише ці рядки, у порядку часу, без пояснень."#
    )
}

/// One pass for decisions, open questions and tasks. Tasks are short
/// instructions in the imperative, never sentences copied from the transcript.
/// With `with_tasks` false (tasks come from each turn) only decisions and
/// open questions are asked: a much shorter answer.
pub(crate) fn items_user_prompt(transcript: &str, with_speakers: bool, with_tasks: bool) -> String {
    if !with_tasks {
        return format!(
            r#"Нижче стенограма наради з мітками часу.
<transcript>
{transcript}
</transcript>

Випиши з усієї наради два розділи. Кожен пункт закінчуй міткою часу рядка, де це прозвучало, у квадратних дужках, точно як у стенограмі.

## Рішення
- <що саме вирішили, погодили, призначили або затвердили> [ММ:СС]

## Відкриті питання
- <питання, яке лишилося невирішеним або чекає відповіді> [ММ:СС]

Коротко, по одному рядку на пункт. Нічого не вигадуй. Якщо в розділі нічого немає, залиш його порожнім. Виведи лише ці два розділи."#
        );
    }
    let owner = if with_speakers {
        "кому доручили (ім'я, псевдонім чи відділ); якщо мовець сам бере на себе («зробимо», «я підготую») — той, хто говорить, як позначено на початку рядка; інакше —"
    } else {
        "кому доручили (ім'я, псевдонім чи відділ), лише якщо названо; інакше —"
    };
    format!(
        r#"Нижче стенограма наради з мітками часу.
<transcript>
{transcript}
</transcript>

Випиши з усієї наради три розділи. Кожен пункт закінчуй міткою часу рядка, де це прозвучало, у квадратних дужках, точно як у стенограмі.

## Рішення
- <що саме вирішили, погодили, призначили або затвердили> [ММ:СС]

## Відкриті питання
- <питання, яке лишилося невирішеним або чекає відповіді> [ММ:СС]

## Завдання
- <що зробити> | <хто> | <до коли, дослівно як сказали, інакше —> [ММ:СС]

Завдання — це все, що має бути зроблено після наради: доручення («треба», «прошу», «надайте», «до кінця дня») і те, що мовець сам обіцяє або планує зробити («зроблю», «сьогодні будемо», «плануємо», «підготуємо»). Пройди всю стенограму від початку до кінця і випиши кожне таке завдання окремим рядком; зазвичай їх багато — по одному-три на кожного, хто доповідає. «Що зробити» — коротко, 3–10 слів, з дієслова в неозначеній формі («Надати графік чергувань»), а не речення зі стенограми. «Хто» — {owner}. Те, що вже зроблено (минулий час), не включай.
Нічого не вигадуй. Якщо в розділі нічого немає, залиш його порожнім. Виведи лише ці три розділи."#
    )
}

/// The speakers' own plans («План на сьогодні: …» in «Хід наради») as tasks
pub(crate) fn tasks_from_plans(
    lines: &[Line],
    reports: &[Report],
    fields: &std::collections::HashMap<usize, Vec<(String, String)>>,
) -> Vec<Item> {
    let mut out = Vec::new();
    for (i, report) in reports.iter().enumerate() {
        let Some(values) = fields.get(&i) else { continue };
        let Some((_, plan)) = values.iter().find(|(f, _)| f.to_lowercase().starts_with("план")) else { continue };
        let separators: &[char] = if plan.contains(';') { &['.', ';'] } else { &['.'] };
        for part in plan.split(separators).flat_map(|p| if plan.contains(';') { vec![p] } else { p.split(", ").collect() }).map(str::trim) {
            let words = part.split_whitespace().count();
            if (2..=16).contains(&words) {
                let mut what = part.trim_end_matches(['.', ',']).to_string();
                if let Some(first) = what.chars().next() {
                    what = first.to_uppercase().chain(what.chars().skip(1)).collect();
                }
                // No deadline unless one is said («до вечора», «до п'ятниці»)
                let when = deadline_in(&what).unwrap_or("—").to_string();
                out.push(Item { fields: vec![what, report.speaker.clone(), when], seconds: lines[report.start].seconds });
            }
        }
    }
    out
}

/// A task without an owner belongs to whoever was speaking when it was said
/// (a report: «сьогодні підготуємо…»)
pub(crate) fn fill_owners(tasks: &mut [Item], lines: &[Line], reports: &[Report]) {
    for t in tasks.iter_mut() {
        let empty = t.fields.get(1).map(|w| matches!(w.trim(), "" | "—" | "-" | "?")).unwrap_or(true);
        if !empty {
            continue;
        }
        let Some(r) = reports.iter().rev().find(|r| lines.get(r.start).map(|l| l.seconds <= t.seconds).unwrap_or(false)) else { continue };
        if r.speaker == "Доповідач" {
            continue;
        }
        while t.fields.len() < 2 {
            t.fields.push("—".into());
        }
        t.fields[1] = r.speaker.clone();
    }
}

/// A deadline said in a task («до вечора», «до кінця дня», «до п'ятниці», «завтра»)
fn deadline_in(what: &str) -> Option<&'static str> {
    let l = what.to_lowercase();
    [
        ("до кінця дня", "до кінця дня"), ("до вечора", "до вечора"), ("до обіду", "до обіду"), ("до ранку", "до ранку"),
        ("до кінця тижня", "до кінця тижня"), ("завтра", "завтра"), ("сьогодні", "сьогодні"), ("до понеділка", "до понеділка"),
        ("до вівторка", "до вівторка"), ("до середи", "до середи"), ("до четверга", "до четверга"), ("до п'ятниці", "до п'ятниці"),
    ]
    .iter()
    .find(|(k, _)| l.contains(k))
    .map(|(_, v)| *v)
}

/// Tasks that read like a copied sentence (long) are not kept
pub(crate) fn short_tasks_only(tasks: Vec<Item>) -> Vec<Item> {
    tasks.into_iter().filter(|t| t.fields.first().map(|w| w.split_whitespace().count() <= 16).unwrap_or(false)).collect()
}

// ---------------------------------------------------------------------------
// Parsing and validation of model output
// ---------------------------------------------------------------------------

/// Index of the transcript line that best matches a timestamp
fn line_at(lines: &[Line], seconds: u32) -> Option<usize> {
    lines
        .iter()
        .enumerate()
        .min_by_key(|(_, l)| l.seconds.abs_diff(seconds))
        .filter(|(_, l)| l.seconds.abs_diff(seconds) <= MATCH_WINDOW_S)
        .map(|(i, _)| i)
}

/// The transcript line sharing the most content words with `text`
/// (at least two), for items the model gave without a time
/// How many word stems of `text` a transcript line shares
fn shared_stems(line: &str, text: &str) -> usize {
    let stem = |w: &str| -> Option<String> {
        let w: String = w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
        (w.chars().count() >= 4).then(|| w.chars().take(5).collect())
    };
    let wanted: HashSet<String> = text.split_whitespace().filter_map(stem).collect();
    line.split_whitespace().filter_map(stem).collect::<HashSet<_>>().intersection(&wanted).count()
}

fn line_by_words(lines: &[Line], text: &str) -> Option<usize> {
    let stem = |w: &str| -> Option<String> {
        let w: String = w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
        (w.chars().count() >= 4).then(|| w.chars().take(5).collect())
    };
    let wanted: HashSet<String> = text.split_whitespace().filter_map(stem).collect();
    if wanted.len() < 2 {
        return None;
    }
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| (i, l.text.split_whitespace().filter_map(stem).collect::<HashSet<_>>().intersection(&wanted).count()))
        .filter(|(_, n)| *n >= 2)
        .max_by_key(|(i, n)| (*n, std::cmp::Reverse(*i)))
        .map(|(i, _)| i)
}

fn clean_speaker(raw: &str) -> Option<String> {
    let s = raw
        .replace("(не названо)", "")
        .replace("(ім'я не названо)", "")
        .trim()
        .trim_matches(|c: char| c == '«' || c == '»' || c == '"' || c == '*')
        .trim()
        .to_string();
    let lower = s.to_lowercase();
    let unknown = s.is_empty()
        || s == "?"
        || lower.starts_with("доповідач")
        || lower.starts_with("не назван")
        || lower == "невідомо"
        || s.chars().count() > 40
        || s.split_whitespace().count() > 3
        || s.chars().next().is_some_and(|c| c.is_lowercase());
    (!unknown).then_some(s)
}

const END_CUES: &[&str] = &["дякую", "у мене все", "в мене все", "доповідь закінчив", "доповідь закінчила", "закінчив", "закінчила", "все.", "на цьому все", "наразі все"];
const START_CUES: &[&str] = &[
    "добрий день", "доброго дня", "доброго ранку", "добрий ранок", "доброго вечора", "вітаю", "доповідаю", "доповідає",
    "слово має", "слово надається", "далі", "наступн", 
];

/// Where a report really starts near the model's boundary `start`: right
/// after a line where the previous speaker finishes, or at a line where the
/// next one opens, within one line either way. None if there is no handover.
fn snap_to_handover(lines: &[Line], start: usize) -> Option<usize> {
    let ends = |i: usize| {
        let t = lines[i].text.to_lowercase();
        let tail: String = t.chars().rev().take(90).collect::<Vec<_>>().into_iter().rev().collect();
        END_CUES.iter().any(|c| tail.contains(c))
    };
    let opens = |i: usize| {
        let head: String = lines[i].text.to_lowercase().chars().take(60).collect();
        (START_CUES.iter().any(|c| head.contains(c)) || super::pack::current().handover_cues.iter().any(|c| head.contains(c.as_str())))
    };
    let lo = start.saturating_sub(1);
    let hi = (start + 1).min(lines.len().saturating_sub(1));
    // Prefer the model's own line, then its neighbours
    for j in [start, lo, hi] {
        if j < lines.len() && opens(j) {
            return Some(j);
        }
        if j >= 1 && j <= lines.len() && ends(j - 1) {
            return Some(j);
        }
    }
    None
}

/// Reports from the model's "MM:SS | who" lines; None when unusable
pub(crate) fn parse_segmentation(output: &str, lines: &[Line]) -> Option<Vec<Report>> {
    let mut reports: Vec<Report> = Vec::new();
    let mut unnamed = 0;
    for raw in output.lines() {
        let raw = raw.trim().trim_start_matches(['-', '*', ' ']);
        let Some((time, who)) = raw.split_once('|') else { continue };
        let Some(seconds) = parse_mmss(time) else { continue };
        let Some(start) = line_at(lines, seconds) else { continue };
        if reports.last().is_some_and(|r| start <= r.start) {
            continue; // keep strictly forward order
        }
        let speaker = clean_speaker(who).unwrap_or_else(|| {
            unnamed += 1;
            format!("Доповідач {unnamed}")
        });
        reports.push(Report { start, speaker });
    }
    if reports.is_empty() {
        return None;
    }
    // The first report always starts at the beginning of the meeting
    reports[0].start = 0;
    // A new report starts at a handover: the previous speaker finishes
    // («дякую», «у мене все», «доповідь закінчив») or the next one opens
    // («доброго ранку», «доповідаю», «слово…»). Other boundaries are merged.
    let mut kept: Vec<Report> = Vec::new();
    for (i, r) in reports.iter().enumerate() {
        let start = if i == 0 { Some(0) } else { snap_to_handover(lines, r.start) };
        if let Some(start) = start {
            if kept.last().map_or(true, |k| start > k.start) {
                kept.push(Report { start, speaker: r.speaker.clone() });
            }
        }
    }
    // Renumber «Доповідач N» after merging
    let mut n = 0;
    let kept = kept
        .into_iter()
        .map(|mut r| {
            if r.speaker.starts_with("Доповідач ") {
                n += 1;
                r.speaker = format!("Доповідач {n}");
            }
            r
        })
        .collect();
    Some(kept)
}

/// Fixed-length blocks when the model's segmentation cannot be used
pub(crate) fn fallback_blocks(lines: &[Line]) -> Vec<Report> {
    let mut reports = Vec::new();
    let mut next_at = 0;
    for (i, line) in lines.iter().enumerate() {
        if i == 0 || line.seconds >= next_at {
            reports.push(Report { start: i, speaker: format!("Фрагмент {}", reports.len() + 1) });
            next_at = line.seconds + FALLBACK_BLOCK_S;
        }
    }
    reports
}

/// Items of one section ("## Рішення" etc.), each with a valid moment
pub(crate) fn parse_items(output: &str, section: &str, lines: &[Line]) -> Vec<Item> {
    let mut items = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut inside = false;
    for raw in output.lines() {
        let trimmed = raw.trim();
        if trimmed.starts_with('#') {
            inside = trimmed.trim_start_matches('#').trim().eq_ignore_ascii_case(section);
            continue;
        }
        if !inside || !trimmed.starts_with(['-', '*', '•']) {
            continue;
        }
        let body = trimmed.trim_start_matches(['-', '*', '•', ' ']);
        // The moment: the last [MM:SS] in the line, or else the transcript line
        // the item's words come from; items with neither are dropped
        let stamped = body.rfind('[').and_then(|open| {
            let close = body[open..].find(']').map(|i| open + i)?;
            let seconds = parse_mmss(&body[open + 1..close])?;
            Some((open, seconds))
        });
        let (text, index) = match stamped {
            Some((open, seconds)) => {
                let text = body[..open].trim();
                // The model's time mark is often the start of the meeting or of
                // a long report; the line that holds the item's words is better
                let by_time = line_at(lines, seconds).filter(|&i| shared_stems(&lines[i].text, text) >= 2);
                match by_time.or_else(|| line_by_words(lines, text)).or_else(|| line_at(lines, seconds)) {
                    Some(i) => (text, i),
                    None => continue,
                }
            }
            None => match line_by_words(lines, body) {
                Some(i) => (body, i),
                None => continue,
            },
        };
        let text = text.trim().trim_end_matches(['.', ';', ',', '|']).trim().to_string();
        if text.is_empty() || text.starts_with('<') {
            continue;
        }
        let fields: Vec<String> = text.split('|').map(|f| f.trim().to_string()).collect();
        if !seen.insert(text.to_lowercase()) {
            continue;
        }
        items.push(Item { fields, seconds: lines[index].seconds });
    }
    items
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

/// Words that mark an instruction for the future in spoken Ukrainian
const OBLIGATION_CUES: &[&str] = &[
    "треба", "потрібно", "необхідно", "прошу", "просимо", "маєте", "мають", "має", "повинні", "повинен",
    "доручаю", "доручити", "до кінця", "терміново", "нехай",
];

/// Keeps tasks that are instructions for the future: a real deadline was
/// named, or the transcript line says so («треба», «прошу», an imperative
/// like «надайте»). Reported facts go, even when the model gave them an
/// assignee.
const DECISION_CUES: &[&str] = &[
    "вирішили", "вирішено", "вирішуємо", "погодили", "погоджено", "погоджуємо", "затверджую", "затверджено",
    "затвердили", "призначаю", "призначено", "призначили", "домовились", "домовилися", "ухвалили", "ухвалено",
    "рішення", "буде займатися", "буде відповідати", "відповідальний", "відповідає за", "приймаємо",
];

/// Keeps decisions that were said as decisions («вирішили», «призначаю»,
/// «відповідальний…»), not reports of work or plans
pub(crate) fn keep_real_decisions(decisions: Vec<Item>, lines: &[Line]) -> Vec<Item> {
    decisions
        .into_iter()
        .filter(|d| {
            let Some(line) = lines.iter().find(|l| l.seconds == d.seconds) else { return false };
            let what = d.fields.join(" ");
            let around = sentences_around(&line.text, &what).to_lowercase();
            (DECISION_CUES.iter().any(|c| around.contains(c)) || super::pack::current().decision_cues.iter().any(|c| around.contains(c.as_str()))) || cue_sentence_about(&line.text, &what)
        })
        .collect()
}

/// Drops repeated tasks: the same action in other words (most stems shared)
/// A long meeting is read in overlapping windows (see `overlapping_windows`),
/// so an item said in an overlap comes back twice, worded a little
/// differently. Two items are one when two thirds of the shorter one's word
/// stems are in the other (at least two) and they were said within ten
/// minutes of each other. The earliest moment is kept, and an owner or a
/// deadline («—» or empty in one) is taken from the other.
pub(crate) fn merge_window_items(items: Vec<Item>) -> Vec<Item> {
    const NEAR_S: u32 = 600;
    let stems = |s: &str| -> HashSet<String> {
        s.split_whitespace()
            .map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase())
            .filter(|w| w.chars().count() >= 4)
            .map(|w| w.chars().take(5).collect())
            .collect()
    };
    let blank = |f: &str| {
        let f = f.trim();
        f.is_empty() || f == "—" || f == "-"
    };
    let mut kept: Vec<(HashSet<String>, Item)> = Vec::new();
    for item in items {
        let s = stems(item.fields.first().map(String::as_str).unwrap_or(""));
        let same = kept.iter().position(|(k, old)| {
            let common = k.intersection(&s).count();
            let smaller = k.len().min(s.len()).max(1);
            common * 3 >= smaller * 2 && common >= 2 && old.seconds.abs_diff(item.seconds) <= NEAR_S
        });
        match same {
            Some(i) => {
                let old = &mut kept[i].1;
                old.seconds = old.seconds.min(item.seconds);
                for (j, f) in item.fields.iter().enumerate().skip(1) {
                    match old.fields.get_mut(j) {
                        Some(o) if blank(o) && !blank(f) => *o = f.clone(),
                        None => old.fields.push(f.clone()),
                        _ => {}
                    }
                }
            }
            None => kept.push((s, item)),
        }
    }
    kept.into_iter().map(|(_, t)| t).collect()
}

/// Consecutive windows of whole lines, each about `window_tokens` long; each
/// window after the first starts with the last `overlap_tokens` of lines of
/// the one before, so an item said across a boundary is read whole once.
pub(crate) fn overlapping_windows(text: &str, window_tokens: usize, overlap_tokens: usize) -> Vec<String> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tokens: Vec<usize> = lines.iter().map(|l| super::processor::rough_token_count(l) + 1).collect();
    let mut windows = Vec::new();
    let mut start = 0;
    while start < lines.len() {
        let mut end = start;
        let mut size = 0;
        while end < lines.len() && (end == start || size + tokens[end] <= window_tokens) {
            size += tokens[end];
            end += 1;
        }
        windows.push(lines[start..end].join("\n") + "\n");
        if end >= lines.len() {
            break;
        }
        // Back off by about `overlap_tokens`, but always move forward
        let mut back = end;
        let mut overlap = 0;
        while back > start + 1 && overlap + tokens[back - 1] <= overlap_tokens {
            back -= 1;
            overlap += tokens[back];
        }
        start = back;
    }
    windows
}

pub(crate) fn dedup_tasks(tasks: Vec<Item>) -> Vec<Item> {
    let stems = |s: &str| -> HashSet<String> {
        s.split_whitespace()
            .map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase())
            .filter(|w| w.chars().count() >= 4)
            .map(|w| w.chars().take(5).collect())
            .collect()
    };
    let mut kept: Vec<(HashSet<String>, Item)> = Vec::new();
    for t in tasks {
        let s = stems(t.fields.first().map(String::as_str).unwrap_or(""));
        let duplicate = kept.iter().any(|(k, _)| {
            let common = k.intersection(&s).count();
            let smaller = k.len().min(s.len()).max(1);
            common * 3 >= smaller * 2 && common >= 2
        });
        if !duplicate {
            kept.push((s, t));
        }
    }
    kept.into_iter().map(|(_, t)| t).collect()
}

/// The sentence of a transcript line that best matches a task's wording
fn matching_sentence<'a>(line: &'a str, what: &str) -> &'a str {
    let stem = |w: &str| w.trim_matches(|c: char| !c.is_alphabetic()).to_lowercase().chars().take(5).collect::<String>();
    let wanted: HashSet<String> = what.split_whitespace().map(stem).filter(|s| s.chars().count() >= 4).collect();
    line.split(['.', '!', '?'])
        .max_by_key(|s| s.split_whitespace().map(stem).filter(|w| wanted.contains(w)).count())
        .unwrap_or(line)
}

/// A sentence of the line that says a decision («…буде займатися Тарас»)
/// about the same thing: at least two of its words
fn cue_sentence_about(line: &str, what: &str) -> bool {
    let stem = |w: &str| w.trim_matches(|c: char| !c.is_alphabetic()).to_lowercase().chars().take(5).collect::<String>();
    let wanted: HashSet<String> = what.split_whitespace().map(stem).filter(|s| s.chars().count() >= 4).collect();
    line.split(['.', '!', '?']).any(|sentence| {
        let lower = sentence.to_lowercase();
        (DECISION_CUES.iter().any(|c| lower.contains(c)) || super::pack::current().decision_cues.iter().any(|c| lower.contains(c.as_str()))) && sentence.split_whitespace().map(stem).filter(|w| wanted.contains(w)).count() >= 2
    })
}

/// The best-matching sentence with its neighbours (a decision is often
/// said over two sentences: «…напрямок комплектації. Цим буде займатися …»)
fn sentences_around(line: &str, what: &str) -> String {
    let sentences: Vec<&str> = line.split(['.', '!', '?']).collect();
    let best = matching_sentence(line, what);
    let i = sentences.iter().position(|s| *s == best).unwrap_or(0);
    sentences[i.saturating_sub(1)..(i + 2).min(sentences.len())].join(".")
}

/// Said about the past: a report of work already done («вчора закрили»,
/// «було надіслано»), not an instruction. Future and obligation words win.
pub(crate) fn reports_done_work(sentence: &str) -> bool {
    let s = sentence.to_lowercase();
    let words: Vec<&str> = s.split(|c: char| !c.is_alphabetic() && c != '\'' && c != '’').filter(|w| !w.is_empty()).collect();
    let future = OBLIGATION_CUES.iter().any(|c| s.contains(c))
        || words.iter().any(|w| {
            ["буде", "будемо", "будуть", "завтра", "сьогодні", "плануємо", "планується"].contains(w)
                || (w.chars().count() >= 6 && (w.ends_with("тиму") || w.ends_with("тимемо") || w.ends_with("тимуть")))
        });
    let past = words.iter().any(|w| {
        ["вчора", "вже", "було", "були", "був"].contains(w)
            || (w.chars().count() >= 6 && ["или", "ала", "ало", "ено", "ано", "ито", "ото"].iter().any(|e| w.ends_with(e)))
    });
    past && !future
}

pub(crate) fn keep_real_tasks(tasks: Vec<Item>, lines: &[Line]) -> Vec<Item> {
    let is_named = |f: Option<&String>| f.is_some_and(|v| { let v = v.trim(); !v.is_empty() && v != "—" && v != "-" });
    tasks
        .into_iter()
        .filter(|t| {
            // Work already done is reported, not assigned
            if let (Some(line), Some(what)) = (lines.iter().find(|l| l.seconds == t.seconds), t.fields.first()) {
                if reports_done_work(matching_sentence(&line.text, what)) {
                    return false;
                }
            }
            // A real deadline («до п'ятниці», «до кінця дня», a date) is enough on its own
            let deadline = t.fields.get(2).map(|d| d.trim().to_lowercase()).unwrap_or_default();
            if is_named(t.fields.get(2)) && (deadline.starts_with("до ") || deadline.chars().any(|c| c.is_ascii_digit())) {
                return true;
            }
            let Some(i) = lines.iter().position(|l| l.seconds == t.seconds) else { return false };
            let around = lines[i].text.to_lowercase();
            let imperative = around.split_whitespace().any(|w| {
                let w = w.trim_matches(|c: char| !c.is_alphabetic());
                w.chars().count() >= 5 && (w.ends_with("іть") || w.ends_with("йте") || w.ends_with("ьте"))
            });
            imperative || OBLIGATION_CUES.iter().any(|c| around.contains(c))
        })
        .collect()
}

/// Builds the markdown document from validated pieces
/// Everything a meeting document is built from
pub(crate) struct DocInput<'a> {
    pub title: &'a str,
    pub date: Option<&'a str>,
    pub lines: &'a [Line],
    pub reports: &'a [Report],
    pub spec: &'a MeetingSpec,
    /// Free summary per report index («Хід наради» without fields)
    pub summaries: &'a std::collections::HashMap<usize, String>,
    /// Field values per report index («Зроблено: …»)
    pub fields: &'a std::collections::HashMap<usize, Vec<(String, String)>>,
    /// Report index where each part starts, with its title
    pub part_starts: &'a [(usize, String)],
    /// Regular participants who did not speak
    pub absent: &'a [String],
    /// Status of the earlier meeting's tasks: (task, status, seconds or None)
    pub control: &'a [(String, String, Option<u32>)],
    pub decisions: &'a [Item],
    pub tasks: &'a [Item],
    pub questions: &'a [Item],
    /// One block per person: «Хід наради» shows these when there are any
    pub speaker_blocks: &'a [SpeakerBlock],
    /// «Загальний підсумок»: «- …» points
    pub overall: &'a str,
    /// Shown under the header (e.g. whose words stayed as said, and why)
    pub note: Option<&'a str>,
}

/// «Загальна зустріч» with the default blocks (kept for tests and callers
/// that have no meeting type)
pub(crate) fn build_document(
    date: Option<&str>,
    lines: &[Line],
    reports: &[Report],
    summaries: &std::collections::HashMap<usize, String>,
    decisions: &[Item],
    tasks: &[Item],
    questions: &[Item],
) -> String {
    let spec = MeetingSpec::default();
    build_meeting_document(&DocInput {
        title: "Матеріали наради",
        date,
        lines,
        reports,
        spec: &spec,
        summaries,
        fields: &Default::default(),
        part_starts: &[],
        absent: &[],
        control: &[],
        decisions,
        tasks,
        questions,
        speaker_blocks: &[],
        overall: "",
        note: None,
    })
}

fn participants_named<'a>(reports: &'a [Report]) -> Vec<&'a str> {
    let mut participants: Vec<&str> = Vec::new();
    let glossary = crate::ukrainian::glossary_terms();
    for r in reports {
        // Only clear personal names («Ім'я Прізвище») or names from the glossary
        let words: Vec<&str> = r.speaker.split_whitespace().collect();
        let person = words.len() == 2
            && words.iter().all(|w| w.chars().next().is_some_and(char::is_uppercase) && w.chars().skip(1).all(|c| c.is_lowercase() || c == '\'' || c == '’' || c == '-'));
        let known = glossary.iter().any(|g| g == &r.speaker);
        if (person || known) && !participants.contains(&r.speaker.as_str()) {
            participants.push(&r.speaker);
        }
    }
    participants
}

/// The document of a meeting type: blocks in the order the type lists them
/// Everything one person said in the meeting, for the document made of
/// speakers' blocks
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SpeakerBlock {
    pub speaker: String,
    /// When this person first spoke
    pub seconds: u32,
    /// Their lines, in order, with the moment of each
    pub lines: Vec<(u32, String)>,
    /// About how long they spoke
    pub spoken: u32,
    /// The written block: «- …» points
    pub text: String,
}

/// About how long a line took: the time to the next line, at most what its words need
fn line_length(words: usize, gap: u32) -> u32 {
    gap.min((words as f64 * 0.7).ceil() as u32 + 1)
}

/// One block per person («Спікер N» and «?» too), in the order they first
/// spoke; a person's lines in the order they were said
pub(crate) fn speaker_blocks(lines: &[Line], reports: &[Report]) -> Vec<SpeakerBlock> {
    let mut out: Vec<SpeakerBlock> = Vec::new();
    for (i, r) in reports.iter().enumerate() {
        let end = reports.get(i + 1).map(|n| n.start).unwrap_or(lines.len()).min(lines.len());
        for k in r.start.min(end)..end {
            let text = lines[k].text.trim();
            if text.is_empty() {
                continue;
            }
            let words = text.split_whitespace().count();
            let gap = lines.get(k + 1).map(|n| n.seconds.saturating_sub(lines[k].seconds)).unwrap_or(u32::MAX);
            let spoken = line_length(words, gap);
            let at = out.iter().position(|b| b.speaker == r.speaker).unwrap_or_else(|| {
                out.push(SpeakerBlock { speaker: r.speaker.clone(), seconds: lines[k].seconds, lines: Vec::new(), spoken: 0, text: String::new() });
                out.len() - 1
            });
            out[at].lines.push((lines[k].seconds, text.to_string()));
            out[at].spoken += spoken;
        }
    }
    out
}

/// A person's lines for the model, in pieces within `max_tokens`
pub(crate) fn lines_for_model(lines: &[(u32, String)], max_tokens: usize) -> Vec<String> {
    let mut parts = vec![String::new()];
    for (at, text) in lines {
        let line = format!("[{}] {}\n", mmss(*at), text);
        let last = parts.last().unwrap();
        if !last.is_empty() && super::processor::rough_token_count(last) + super::processor::rough_token_count(&line) > max_tokens {
            parts.push(String::new());
        }
        parts.last_mut().unwrap().push_str(&line);
    }
    parts.retain(|p| !p.trim().is_empty());
    parts
}

/// How many points the overall summary has: (at least, at most)
pub(crate) fn overall_points(detailed: bool) -> (usize, usize) {
    if detailed { (8, 15) } else { (5, 10) }
}

/// «Докладніше»: half as many points again (twelve at most)
pub(crate) fn points_for_detailed(words: usize) -> usize {
    (points_for(words) * 3).div_ceil(2).min(12)
}

/// How many points a person's block may have: one for a short remark, a
/// short speech told more fully, one more per minute or so of a long report
/// (eight at most)
pub(crate) fn points_for(words: usize) -> usize {
    if words < 40 {
        1
    } else if words < SHORT_SPEECH_WORDS {
        words.div_ceil(18).clamp(2, 6)
    } else {
        words.div_ceil(60).clamp(2, 8)
    }
}

const SHORT_SPEECH_WORDS: usize = 150;
/// A lost number comes back: at most this many in one point, this many new points in a block
const NUMBERS_PER_POINT: usize = 2;
const NEW_NUMBER_POINTS: usize = 2;
/// Words a number brings back when it joins a point made from its line
const APPENDED_WORDS: usize = 9;

/// A greeting, thanks, a roll call, a remark about sound: nothing for the minutes
fn is_service_remark(sentence: &str) -> bool {
    let lower = sentence.trim().to_lowercase();
    let t = lower.trim_matches(|c: char| !c.is_alphanumeric());
    let words = t.split_whitespace().count();
    let digits = t.chars().any(|c| c.is_ascii_digit());
    const OPENINGS: &[&str] = &[
        "доброго ранку", "добрий ранок", "доброго дня", "добрий день", "дякую", "доповідь закінч", "у мене все", "в мене все", "у мене наразі все", "наразі в мене все", "на цьому все",
        "слово надається", "хто наступний", "всім гарного дня", "вибачаюсь", "перепрошую", "мене чути", "не чути", "тебе не чути", "вас не чути",
    ];
    if !digits && words <= 10 && OPENINGS.iter().any(|o| t.starts_with(o)) {
        return true;
    }
    if t.contains("якісь питання") || t.contains("є питання") || ((t.starts_with("пане ") || t.starts_with("панове")) && words <= 3) {
        return true;
    }
    // A word or two alone: only an answer like «Так.» or «Зрозумів.» is
    // nothing for the minutes; «Втрат немає.» or «Орієнтир Дотик.» is the report
    const REPLIES: &[&str] = &[
        "так", "ні", "добре", "гаразд", "зрозумів", "зрозуміла", "зрозуміло", "прийняв", "прийняла", "прийнято", "є", "слухаю", "ок", "окей", "ага", "угу", "ясно", "все", "дякую",
        "дуже", "вас", "тебе", "мене", "чути", "чую", "колеги", "панове", "пане", "вітаю", "привіт", "отже", "значить", "наступний", "далі",
    ];
    words <= 2 && !digits && t.split_whitespace().all(|w| REPLIES.contains(&w.trim_matches(|c: char| !c.is_alphanumeric())))
}

/// Swearing: words that begin with a rude stem (not a stem inside a word:
/// «кораблями» and «рублями» are fine)
fn is_rude(text: &str) -> bool {
    const STEMS: &[&str] = &["їба", "їбе", "їбу", "проїб", "заїб", "виїб", "наїб", "бля", "хуй", "хує", "хуї", "пизд", "нахер", "нахуй", "сука", "суки"];
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’')
        .any(|w| STEMS.iter().any(|x| w.starts_with(x)) || w.contains("їб"))
}

/// The moment of a point: the first time in its last «[…]» mark
fn point_time(point: &str) -> Option<u32> {
    let open = point.rfind('[')?;
    let inner = &point[open + 1..];
    let inner = &inner[..inner.find(']')?];
    parse_mmss(inner.split(',').next()?)
}

/// A person's points in the order things were said: the model or a number
/// brought back may put a point from the middle of a report at its end. A
/// point without a time stays after the one before it.
pub(crate) fn in_said_order(points: &str) -> String {
    let mut keyed: Vec<(u32, usize, &str)> = Vec::new();
    let mut last = 0u32;
    for (i, line) in points.lines().filter(|l| !l.trim().is_empty()).enumerate() {
        if let Some(t) = point_time(line) {
            last = t;
        }
        keyed.push((last, i, line));
    }
    keyed.sort_by_key(|(t, i, _)| (*t, *i));
    keyed.into_iter().map(|k| k.2).collect::<Vec<_>>().join("\n")
}

/// A person's lines without the service remarks
pub(crate) fn working_lines(lines: &[(u32, String)]) -> Vec<(u32, String)> {
    lines
        .iter()
        .filter_map(|(at, line)| {
            let kept = line.split_inclusive(['.', '!', '?', '…']).filter(|s| !is_service_remark(s)).collect::<String>();
            (!kept.trim().is_empty()).then(|| (*at, kept.trim().to_string()))
        })
        .collect()
}

/// Points that say something: no service remarks, nothing rude, no repeats
pub(crate) fn clean_points(points: &str) -> String {
    let stems = |t: &str| t.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 4).map(|w| w.chars().take(5).collect::<String>()).collect::<HashSet<_>>();
    let mut kept: Vec<(&str, HashSet<String>)> = Vec::new();
    for line in points.lines().filter(|l| !l.trim().is_empty()) {
        let text = line.trim_start_matches(['-', ' ']);
        if is_rude(text) || is_service_remark(text.split(" [").next().unwrap_or(text)) {
            continue;
        }
        let s = stems(line);
        if !kept.iter().any(|(_, o)| !s.is_empty() && s.intersection(o).count() * 10 >= s.union(o).count() * 7) {
            kept.push((line, s));
        }
    }
    kept.into_iter().map(|k| k.0).collect::<Vec<_>>().join("\n")
}

fn fragment_around(sentence: &str, number: &str, most: usize) -> String {
    let words: Vec<&str> = sentence.split_whitespace().collect();
    let at = words.iter().position(|w| w.contains(number)).unwrap_or(0);
    let from = at.saturating_sub(most / 2);
    let to = (from + most).min(words.len());
    words[from..to].join(" ").trim_end_matches([',', ';', ':', '.']).to_string()
}

/// Words that open a clause which cannot stand alone («, який переводиться
/// з філії 4512»): it hangs on the clause before
const DEPENDENT_OPENERS: &[&str] = &[
    "який", "яка", "яке", "які", "якого", "якої", "якому", "якій", "яким", "якими", "яких", "котрий", "котра", "котре", "котрі",
    "чий", "чия", "чиє", "чиї", "де", "куди", "звідки", "що", "щоб", "бо", "оскільки",
];

fn first_word(text: &str) -> String {
    text.split_whitespace().next().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()).unwrap_or_default()
}

fn hangs_on_previous(clause: &str) -> bool {
    DEPENDENT_OPENERS.contains(&first_word(clause).as_str())
}

/// «Також …», «А …» at the start of a point read as a cut from the middle
fn without_leading_link(text: &str) -> &str {
    let mut t = text.trim_start();
    for _ in 0..2 {
        let w = first_word(t);
        if ["також", "і", "й", "та", "а"].contains(&w.as_str()) && t.split_whitespace().count() > 3 {
            t = t[t.find(char::is_whitespace).unwrap_or(t.len())..].trim_start();
        } else {
            break;
        }
    }
    t
}

/// The part of a said sentence that carries `number`, readable on its own:
/// whole clauses (between commas), never a cut in the middle of one. Up to
/// `most` words; a clause longer than that is cut around the number. A
/// clause that hangs on the one before («який …», «що …») always takes it,
/// even past `most`.
fn clause_around(sentence: &str, number: &str, most: usize) -> String {
    let clean = clean_spoken(sentence.trim());
    // «, » and «; » only: «4,5» is one number
    let clauses: Vec<&str> = clean.split(", ").flat_map(|c| c.split("; ")).map(|c| c.trim().trim_end_matches([',', ';'])).filter(|c| !c.is_empty()).collect();
    let count = |c: &str| c.split_whitespace().count();
    let Some(at) = clauses.iter().position(|c| c.contains(number)) else { return fragment_around(&clean, number, most) };
    if count(clauses[at]) > most {
        return fragment_around(clauses[at], number, most);
    }
    let (mut from, mut to) = (at, at);
    while from > 0 && hangs_on_previous(clauses[from]) {
        from -= 1;
    }
    let mut words: usize = clauses[from..=to].iter().map(|c| count(c)).sum();
    // The clause before first (it usually says what the number is about), then the one after
    loop {
        let mut grew = false;
        if from > 0 && words + count(clauses[from - 1]) <= most {
            from -= 1;
            words += count(clauses[from]);
            grew = true;
        }
        if to + 1 < clauses.len() && words + count(clauses[to + 1]) <= most {
            to += 1;
            words += count(clauses[to]);
            grew = true;
        }
        if !grew {
            break;
        }
    }
    let text = clauses[from..=to].join(", ");
    let text = without_leading_link(text.trim_end_matches(['.', '!', '?', '…', ':']));
    if from == 0 {
        capitalised(text)
    } else {
        text.to_string()
    }
}

/// What became of the numbers a block had lost
#[derive(Debug, Default, PartialEq)]
pub(crate) struct NumbersKept {
    pub appended: usize,
    pub added: usize,
    pub left: usize,
}

/// Numbers said but missing from the points come back: into the point made
/// from the same line, else as a short point of their own (a few at most)
pub(crate) fn keep_numbers(points: &str, lines: &[(u32, String)]) -> (String, NumbersKept) {
    use super::numbers::extract;
    let mut out: Vec<String> = points.lines().map(str::to_string).collect();
    let mut grown = vec![0usize; out.len()];
    let mut kept = NumbersKept::default();
    let written: HashSet<String> = extract(points).into_iter().flat_map(|f| f.keys).collect();
    let mut seen: Vec<String> = Vec::new();
    for (at, line) in lines {
        let mark = format!(" [{}]", mmss(*at));
        for sentence in line.split_inclusive(['.', '!', '?', '…']) {
            for f in extract(sentence) {
                // Numbers in digits: a word like «три» reads badly when stitched on
                if !f.text.chars().any(|c| c.is_ascii_digit()) || is_rude(sentence) || f.keys.iter().any(|k| written.contains(k)) || seen.contains(&f.text) {
                    continue;
                }
                seen.push(f.text.clone());
                let place = out.iter().enumerate().position(|(i, p)| p.contains(&mark) && grown.get(i).copied().unwrap_or(NUMBERS_PER_POINT) < NUMBERS_PER_POINT);
                let phrase = clause_around(sentence, &f.text, APPENDED_WORDS);
                // A whole thought too long to tack on is a point of its own
                let own = phrase.split_whitespace().count() > APPENDED_WORDS && kept.added < NEW_NUMBER_POINTS;
                if let (Some(i), false) = (place, own) {
                    // Already there word for word (another number of the same clause)
                    if out[i].contains(&phrase) {
                        kept.appended += 1;
                        continue;
                    }
                    let cut = out[i].rfind(&mark).unwrap_or(out[i].len());
                    let head = out[i][..cut].trim_end_matches(['.', ' ']).to_string();
                    out[i] = format!("{head}; {phrase}{}", &out[i][cut..]);
                    grown[i] += 1;
                    kept.appended += 1;
                } else if kept.added < NEW_NUMBER_POINTS {
                    out.push(format!("- {}{mark}", capitalised(&clause_around(sentence, &f.text, 18))));
                    grown.push(0);
                    kept.added += 1;
                } else {
                    kept.left += 1;
                }
            }
        }
    }
    (out.join("\n"), kept)
}

pub(crate) fn speaker_block_prompt(speaker: &str, said: &str, points: usize, detailed: bool) -> String {
    let (kind, words) = if detailed { ("докладний", 25) } else { ("стислий", 15) };
    format!(
        r#"Нижче все, що сказав на нараді один учасник ({speaker}): рядки стенограми усного мовлення з мітками часу.
<said>
{said}
</said>

Склади {kind} виклад сказаного: головне з доповіді, проблеми, пропозиції, завдання, які дано або взято (кому і який термін, лише якщо це названо). Дрібне об'єднуй, повторів не пиши.
Не більше {points} пунктів, кожен не довший за {words} слів, у форматі «- текст [ММ:СС]», де мітка часу — того рядка, де це прозвучало. Пункти в тому самому порядку, у якому це прозвучало: не переставляй сказане із середини в кінець.
Кожен пункт — завершена думка, а не обірваний шматок речення. Пиши словами доповідача: не замінюй і не пропускай назв, псевдонімів, орієнтирів, населених пунктів, номерів підрозділів і техніки; числа пиши цифрами, дати й імена дослівно. Нічого не додавай від себе й не роби висновків. Не бери привітань, подяк, перекличок і реплік про звук. Не пиши вступу, заголовків чи імені доповідача. Виведи лише пункти українською."#
    )
}

pub(crate) fn overall_summary_prompt(blocks: &str, detailed: bool) -> String {
    let (least, most) = overall_points(detailed);
    format!(
        r#"Нижче виступи учасників наради, по порядку.
<meeting>
{blocks}
</meeting>

Напиши загальний підсумок наради: від {least} до {most} коротких пунктів, кожен з нового рядка, починаючи з «- ». Спочатку головні теми, потім рішення, далі завдання (хто відповідальний і термін, лише якщо це названо).
Лише те, що є в тексті: нічого не вигадуй. Числа, дати й імена дослівно. Виведи лише пункти українською."#
    )
}

/// The model's answer as «- …» points (plain lines become points too)
pub(crate) fn as_points(answer: &str, most: usize) -> String {
    answer
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| format!("- {}", l.trim_start_matches(['-', '*', '•', ' '])))
        .take(most)
        .collect::<Vec<_>>()
        .join("\n")
}

/// A number the transcript does not back (in digits or in words) gets the
/// amber mark for the secretary. Returns the text and how many points were marked.
pub(crate) fn mark_unbacked_numbers(points: &str, speaker: &super::numbers::Said, all: &super::numbers::Said) -> (String, usize) {
    let mut marked = 0;
    let out = points
        .lines()
        .map(|l| {
            if super::numbers::check(l, speaker, all).is_empty() {
                l.to_string()
            } else {
                marked += 1;
                format!("{l} {NO_SOURCE}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    (out, marked)
}

/// A short remark, written as it was said
pub(crate) fn verbatim_points(lines: &[(u32, String)]) -> String {
    let at = lines.first().map(|l| l.0).unwrap_or(0);
    format!("- {} [{}]", clean_spoken(&lines.iter().map(|l| l.1.as_str()).collect::<Vec<_>>().join(" ")), mmss(at))
}

fn minutes(seconds: u32) -> String {
    format!("{} хв", ((seconds + 30) / 60).max(1))
}

/// The document made of speakers' blocks: header, one block per person, the
/// overall summary. Blocks read as «Хід наради» so that «Затвердити» learns from them.
pub(crate) fn speaker_blocks_markdown(title: &str, date: Option<&str>, lines: &[Line], reports: &[Report], blocks: &[SpeakerBlock], overall: &str, note: Option<&str>) -> String {
    let spec = MeetingSpec::default();
    let spec = MeetingSpec { blocks: spec.speaker_document_blocks(), ..spec };
    build_meeting_document(&DocInput {
        title,
        date,
        lines,
        reports,
        spec: &spec,
        summaries: &Default::default(),
        fields: &Default::default(),
        part_starts: &[],
        absent: &[],
        control: &[],
        decisions: &[],
        tasks: &[],
        questions: &[],
        speaker_blocks: blocks,
        overall,
        note,
    })
}

/// A capital after a full stop inside recognised text, except after an
/// abbreviation or a single letter («м. Київ», «ім. Шевченка»)
pub(crate) fn capitalize_sentences(text: &str) -> String {
    const ABBREVIATIONS: &[&str] = &["р", "рр", "т.д", "т.п", "ім", "вул", "м", "грн", "тис", "млн", "ст", "п", "с"];
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut capitalize = false;
    while let Some(c) = chars.next() {
        if capitalize && c.is_lowercase() {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        capitalize = false;
        if c == '.' && chars.peek() == Some(&' ') {
            let token = out[..out.len() - 1].rsplit(char::is_whitespace).next().unwrap_or("").trim_start_matches(|x: char| !x.is_alphanumeric()).to_lowercase();
            let single = token.chars().count() <= 1;
            if !single && !ABBREVIATIONS.contains(&token.as_str()) {
                out.push(chars.next().unwrap());
                capitalize = true;
            }
        }
    }
    out
}

/// Speech joined into a paragraph: sentences closed and opened properly where
/// recognition cut them, nothing left hanging at the end
pub(crate) fn join_speech(parts: &[String]) -> String {
    let mut out = String::new();
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if out.is_empty() {
            out.push_str(part);
            continue;
        }
        let closed = out.ends_with(['.', '!', '?', '…']);
        let opens_big = part.chars().next().is_some_and(char::is_uppercase);
        if !closed && opens_big {
            out.truncate(out.trim_end_matches([',', ';', ':', ' ']).len());
            out.push('.');
        }
        out.push(' ');
        if closed && !opens_big {
            let mut c = part.chars();
            let first = c.next().unwrap();
            out.extend(first.to_uppercase());
            out.push_str(c.as_str());
        } else {
            out.push_str(part);
        }
    }
    let mut out = capitalize_sentences(&clean_spoken(&out));
    let end = out.trim_end_matches([',', ';', ':', ' ', '-', '—']).len();
    out.truncate(end);
    if !out.is_empty() && !out.ends_with(['.', '!', '?', '…']) {
        out.push('.');
    }
    if let Some(first) = out.chars().next() {
        let big: String = first.to_uppercase().collect();
        out.replace_range(..first.len_utf8(), &big);
    }
    out
}

/// One line of a meeting for the stenogram
pub(crate) struct StenoLine {
    pub speaker: String,
    pub text: String,
    pub start: f64,
    pub end: f64,
}

/// The stenogram: no model, the words as they stand. Header, then turns:
/// «Ім'я · 00:03» and a paragraph; lines of one speaker in a row are one turn
pub(crate) fn stenogram_markdown(title: &str, date: Option<&str>, lines: &[StenoLine]) -> String {
    let mut md = format!("# {title}\n\n");
    let mut header = Vec::new();
    if let Some(d) = date {
        header.push(format!("**Дата:** {d}"));
    }
    let end = lines.iter().map(|l| l.end).fold(0.0, f64::max);
    if end > 0.0 {
        header.push(format!("**Тривалість:** {} хв", (end / 60.0).ceil() as u32));
    }
    let mut people: Vec<&str> = Vec::new();
    for l in lines {
        if !l.speaker.is_empty() && l.speaker != "?" && !people.contains(&l.speaker.as_str()) {
            people.push(&l.speaker);
        }
    }
    if !people.is_empty() {
        header.push(format!("**Учасники:** {}", people.join(", ")));
    }
    if !header.is_empty() {
        md.push_str(&header.join("  \n"));
        md.push_str("\n\n");
    }
    for (who, start, text) in stenogram_turns(lines) {
        md.push_str(&format!("**{} · {}**\n\n{}\n\n", who, mmss(start as u32), text));
    }
    md
}

/// The speakers' turns, in order: (who, start, text); the same for every copy of the stenogram
pub(crate) fn stenogram_turns(lines: &[StenoLine]) -> Vec<(String, f64, String)> {
    let spoken: Vec<&StenoLine> = lines.iter().filter(|l| !is_mic_note(&l.text)).collect();
    let mut turns = Vec::new();
    let mut i = 0;
    while i < spoken.len() {
        let mut j = i;
        while j < spoken.len() && spoken[j].speaker == spoken[i].speaker {
            j += 1;
        }
        let parts: Vec<String> = spoken[i..j].iter().map(|l| l.text.clone()).collect();
        let text = join_speech(&parts);
        if !text.is_empty() {
            let who = if spoken[i].speaker.is_empty() { "?" } else { spoken[i].speaker.as_str() };
            turns.push((who.to_string(), spoken[i].start, text));
        }
        i = j;
    }
    turns
}

fn is_mic_note(text: &str) -> bool {
    let t = text.trim();
    t.starts_with('—') && t.contains("Мікрофон") && t.ends_with('—')
}

/// The stenogram as plain text to paste elsewhere: (text, number of turns)
pub(crate) fn stenogram_plain(heading: &str, lines: &[StenoLine]) -> (String, usize) {
    let end = lines.iter().map(|l| l.end).fold(0.0, f64::max);
    let mut out = if end > 0.0 { format!("{heading} · {} хв\n\n", (end / 60.0).ceil() as u32) } else { format!("{heading}\n\n") };
    let turns = stenogram_turns(lines);
    for (who, start, text) in &turns {
        out.push_str(&format!("{who} · {}\n{text}\n\n", mmss(*start as u32)));
    }
    (out.trim_end().to_string(), turns.len())
}

pub(crate) fn build_meeting_document(d: &DocInput) -> String {
    let duration_min = d.lines.last().map(|l| l.seconds.div_ceil(60)).unwrap_or(0);
    let participants = participants_named(d.reports);
    let mut md = format!("# {}\n\n", d.title);
    let mut header = Vec::new();
    if let Some(date) = d.date {
        header.push(format!("**Дата:** {date}"));
    }
    if duration_min > 0 {
        header.push(format!("**Тривалість:** {duration_min} хв"));
    }
    if !participants.is_empty() {
        header.push(format!("**Учасники (названі в записі):** {}", participants.join(", ")));
    }
    if !header.is_empty() {
        md.push_str(&header.join("  \n"));
        md.push_str("\n\n");
    }
    if let Some(note) = d.note {
        md.push_str(note);
        md.push_str("\n\n");
    }
    for block in &d.spec.blocks {
        match block.as_str() {
            "Хід наради" if !d.speaker_blocks.is_empty() => {
                md.push_str("## Хід наради\n\n");
                for b in d.speaker_blocks {
                    md.push_str(&format!("**ВИСТУПИЛИ:** {} [{}] · {}\n\n{}\n\n", b.speaker, mmss(b.seconds), minutes(b.spoken), b.text.trim()));
                }
            }
            "Хід наради" => course_block(&mut md, d),
            OVERALL_BLOCK if !d.overall.trim().is_empty() => {
                md.push_str(&format!("## {OVERALL_BLOCK}\n\n"));
                md.push_str(d.overall.trim());
                md.push_str("\n\n");
            }
            "Рішення" => items_block(&mut md, "Рішення", d.decisions),
            "Відкриті питання" => items_block(&mut md, "Відкриті питання", d.questions),
            "Завдання" => tasks_block(&mut md, d.tasks),
            "Не доповідали" if !d.absent.is_empty() => {
                md.push_str("## Не доповідали\n\n");
                md.push_str(&d.absent.join(", "));
                md.push_str("\n\n");
            }
            "Контроль" if !d.control.is_empty() => {
                md.push_str("## Контроль виконання\n\n| Завдання | Стан | Час у записі |\n| --- | --- | --- |\n");
                for (task, status, at) in d.control {
                    let at = at.map(|s| format!("[{}]", mmss(s))).unwrap_or_else(|| "—".into());
                    md.push_str(&format!("| {} | {} | {} |\n", task.replace('|', "/"), status, at));
                }
                md.push('\n');
            }
            "Стенограма" => verbatim_block(&mut md, d.lines, d.reports),
            _ => {}
        }
    }
    md
}

fn course_block(md: &mut String, d: &DocInput) {
    if d.summaries.is_empty() && d.fields.is_empty() {
        return;
    }
    md.push_str("## Хід наради\n\n");
    if d.spec.groups_by_speaker() {
        return grouped_course(md, d);
    }
    for (i, report) in d.reports.iter().enumerate() {
        if let Some((_, part)) = d.part_starts.iter().find(|(at, _)| *at == i) {
            md.push_str(&format!("### {part}\n\n"));
        }
        let at = mmss(d.lines[report.start].seconds);
        if let Some(values) = d.fields.get(&i) {
            md.push_str(&format!("**ВИСТУПИЛИ:** {} [{at}]\n", report.speaker));
            // The plan is listed under «Завдання» with this speaker as owner
            let plans_are_tasks = d.spec.has("Завдання");
            for (field, value) in values {
                if plans_are_tasks && field.to_lowercase().starts_with("план") {
                    continue;
                }
                md.push_str(&format!("- {field}: {value}\n"));
            }
            md.push('\n');
        } else if let Some(summary) = d.summaries.get(&i) {
            md.push_str(&format!("**ВИСТУПИЛИ:** {} [{at}] — {}\n\n", report.speaker, summary.trim()));
        }
    }
}

/// Words that carry the meaning of a point: stems of words of 4+ letters,
/// numbers whole
fn point_words(s: &str) -> (HashSet<String>, HashSet<String>) {
    let mut stems = HashSet::new();
    let mut numbers = HashSet::new();
    for w in s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
        if w.chars().any(|c| c.is_ascii_digit()) {
            numbers.insert(w.to_string());
        } else if w.chars().count() >= 4 {
            stems.insert(w.to_lowercase().chars().take(5).collect());
        }
    }
    (stems, numbers)
}

/// Two points say the same: one holds the other, or most of the shorter
/// one's words are in the longer one and none of its numbers differ
fn same_point(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    let (na, nb) = (norm(a), norm(b));
    if na.is_empty() || nb.is_empty() {
        return na == nb;
    }
    let ((sa, numa), (sb, numb)) = (point_words(a), point_words(b));
    let (short_n, long_n) = if sa.len() + numa.len() <= sb.len() + numb.len() { (&numa, &numb) } else { (&numb, &numa) };
    if !short_n.is_subset(long_n) {
        return false;
    }
    if na.contains(&nb) || nb.contains(&na) {
        return true;
    }
    let shorter = sa.len().min(sb.len());
    shorter >= 2 && sa.intersection(&sb).count() as f64 >= shorter as f64 * 0.6
}

/// Points without repeats; of two that say the same the fuller one stays
/// (in the place of the first)
pub(crate) fn dedup_points(points: Vec<String>) -> Vec<String> {
    // (text without marks, its moments, whether a point had no source)
    let mut kept: Vec<(String, Vec<u32>, bool)> = Vec::new();
    let marked = points.iter().any(|p| p.contains('⟨'));
    for p in points.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        let times = source_times(p);
        let unknown = p.contains(NO_SOURCE);
        let text = without_source_marks(p).trim().trim_end_matches(['.', ';']).trim().to_string();
        if text.is_empty() {
            continue;
        }
        match kept.iter_mut().find(|k| same_point(&k.0, &text)) {
            Some(k) => {
                if text.chars().count() > k.0.chars().count() {
                    k.0 = text;
                }
                for t in times {
                    if !k.1.contains(&t) {
                        k.1.push(t);
                    }
                }
                k.2 &= unknown;
            }
            None => kept.push((text, times, unknown)),
        }
    }
    kept.into_iter()
        .map(|(text, mut times, _)| {
            if !marked {
                return text;
            }
            times.sort();
            format!("{text} {}", source_mark(&times))
        })
        .collect()
}

/// The quotes of «Не враховано» («…» «…»)
fn quotes_of(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(i) = rest.find('«') {
        let after = &rest[i + '«'.len_utf8()..];
        // Quotes may hold quotes: the matching closing one
        let mut depth = 1;
        let mut end = None;
        for (j, c) in after.char_indices() {
            match c {
                '«' => depth += 1,
                '»' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(j);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(j) = end else { break };
        out.push(after[..j].trim().to_string());
        rest = &after[j + '»'.len_utf8()..];
    }
    out
}

const NOT_REFLECTED: &str = NOT_REFLECTED_FIELD;

/// Everything one person said in the meeting under that person, once: the
/// fields of all their turns merged, repeats left out, the moments listed
pub(crate) fn merged_fields(turns: &[&Vec<(String, String)>], order: &[String]) -> Vec<(String, String)> {
    let mut names: Vec<String> = order.iter().filter(|f| f.as_str() != NOT_REFLECTED).cloned().collect();
    for t in turns {
        for (f, _) in t.iter() {
            if f != NOT_REFLECTED && !names.contains(f) {
                names.push(f.clone());
            }
        }
    }
    let mut out: Vec<(String, String)> = Vec::new();
    for f in &names {
        let points: Vec<String> = turns.iter().flat_map(|t| t.iter().filter(|(x, _)| x == f)).flat_map(|(_, v)| v.split(';').map(str::to_string).collect::<Vec<_>>()).collect();
        let points = dedup_points(points);
        if !points.is_empty() {
            out.push((f.clone(), points.join("; ")));
        }
    }
    // «Не враховано»: what the merged write-up now says is left out
    let written: HashSet<String> = out.iter().flat_map(|(_, v)| point_words(&without_source_marks(v)).0).collect();
    let quotes: Vec<String> = turns.iter().flat_map(|t| t.iter().filter(|(x, _)| x == NOT_REFLECTED)).flat_map(|(_, v)| quotes_of(v)).collect();
    let quotes: Vec<String> = dedup_points(quotes)
        .into_iter()
        .filter(|q| {
            let own = point_words(q).0;
            let shared = own.iter().filter(|w| written.contains(*w)).count();
            shared < 2 && (shared as f64) < own.len() as f64 * 0.25
        })
        .collect();
    if !quotes.is_empty() {
        out.push((NOT_REFLECTED.to_string(), quotes.iter().map(|q| format!("«{q}»")).collect::<Vec<_>>().join(" ")));
    }
    out
}

/// «Хід наради» grouped by speaker: one «ВИСТУПИЛИ» per person (per part),
/// in the order they first spoke
fn grouped_course(md: &mut String, d: &DocInput) {
    let plans_are_tasks = d.spec.has("Завдання");
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    let flush = |md: &mut String, groups: &mut Vec<(String, Vec<usize>)>| {
        for (speaker, turns) in groups.drain(..) {
            let times: Vec<String> = turns.iter().map(|&i| mmss(d.lines[d.reports[i].start].seconds)).collect();
            let at = times.join("; ");
            let with_fields: Vec<&Vec<(String, String)>> = turns.iter().filter_map(|i| d.fields.get(i)).collect();
            let sentences: Vec<String> = turns
                .iter()
                .filter(|i| !d.fields.contains_key(i))
                .filter_map(|i| d.summaries.get(i))
                .flat_map(|s| if s.contains('⟨') { s.split_inclusive('⟩').map(str::to_string).collect::<Vec<_>>() } else { s.split_inclusive(['.', '!', '?']).map(str::to_string).collect::<Vec<_>>() })
                .collect();
            let sentences = dedup_points(sentences);
            if with_fields.is_empty() {
                let text: Vec<String> = sentences.iter().map(|s| if s.ends_with(['!', '?', '»', ')', '⟩']) { s.clone() } else { format!("{s}.") }).collect();
                md.push_str(&format!("**ВИСТУПИЛИ:** {speaker} [{at}] — {}\n\n", text.join(" ")));
                continue;
            }
            md.push_str(&format!("**ВИСТУПИЛИ:** {speaker} [{at}]\n"));
            for (field, value) in merged_fields(&with_fields, &d.spec.report_fields) {
                if plans_are_tasks && field.to_lowercase().starts_with("план") {
                    continue;
                }
                md.push_str(&format!("- {field}: {value}\n"));
            }
            for s in sentences {
                md.push_str(&format!("- {s}\n"));
            }
            md.push('\n');
        }
    };
    for (i, report) in d.reports.iter().enumerate() {
        if let Some((_, part)) = d.part_starts.iter().find(|(at, _)| *at == i) {
            flush(md, &mut groups);
            md.push_str(&format!("### {part}\n\n"));
        }
        if !d.fields.contains_key(&i) && !d.summaries.contains_key(&i) {
            continue;
        }
        match groups.iter_mut().find(|(s, _)| *s == report.speaker) {
            Some((_, turns)) => turns.push(i),
            None => groups.push((report.speaker.clone(), vec![i])),
        }
    }
    flush(md, &mut groups);
}

fn items_block(md: &mut String, title: &str, items: &[Item]) {
    // An empty section is not shown: the protocol stays short
    if items.is_empty() {
        return;
    }
    md.push_str(&format!("## {title}\n\n"));
    for item in items {
        md.push_str(&format!("- {} [{}]\n", item.fields.join(" "), mmss(item.seconds)));
    }
    md.push('\n');
}

fn tasks_block(md: &mut String, tasks: &[Item]) {
    if tasks.is_empty() {
        return;
    }
    md.push_str("## Завдання\n\n| Хто | Що | До коли | Час у записі |\n| --- | --- | --- | --- |\n");
    for t in tasks {
        let (what, who, when) = match t.fields.as_slice() {
            [what, who, when, ..] => (what.as_str(), who.as_str(), when.as_str()),
            [what, who] => (what.as_str(), who.as_str(), "—"),
            [what] => (what.as_str(), "—", "—"),
            [] => continue,
        };
        let cell = |s: &str| if s.trim().is_empty() { "—".to_string() } else { s.replace('|', "/") };
        md.push_str(&format!("| {} | {} | {} | [{}] |\n", cell(who), cell(what), cell(when), mmss(t.seconds)));
    }
    md.push('\n');
}

fn verbatim_block(md: &mut String, lines: &[Line], reports: &[Report]) {
    md.push_str("## Стенограма по доповідачах\n\n");
    for (i, report) in reports.iter().enumerate() {
        let end = reports.get(i + 1).map(|r| r.start).unwrap_or(lines.len());
        if report.start >= end {
            continue;
        }
        let text = lines[report.start..end].iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" ");
        let text = clean_spoken(&text);
        if text.trim().is_empty() {
            continue;
        }
        // A short remark between reports («Дякую, далі») stays inside the
        // current report instead of opening a new one
        if is_interjection(&text) && i > 0 {
            md.push_str(&format!("_{}: {}_\n\n", report.speaker, text.trim()));
            continue;
        }
        md.push_str(&format!("### [{}] {}\n\n{}\n\n", mmss(lines[report.start].seconds), report.speaker, text));
    }
}

/// Where each part of the meeting starts: the first report (after the
/// previous part) whose first words, or the words just before it, contain
/// one of the part's cues, or whose speaker has the part's role. A part
/// without cues or role starts at the first report.
pub(crate) fn assign_parts(parts: &[MeetingPart], lines: &[Line], reports: &[Report], roles: &std::collections::HashMap<String, String>) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut from = 0usize;
    for part in parts {
        let cues: Vec<String> = part.cues.iter().map(|c| c.to_lowercase()).collect();
        if cues.is_empty() && part.role.is_none() {
            if out.is_empty() {
                out.push((0, part.title.clone()));
            }
            continue;
        }
        let found = (from..reports.len()).find(|&i| {
            let r = &reports[i];
            let own: String = lines[r.start].text.split_whitespace().take(20).collect::<Vec<_>>().join(" ").to_lowercase();
            let before: String = if r.start > 0 {
                let w: Vec<&str> = lines[r.start - 1].text.split_whitespace().collect();
                w[w.len().saturating_sub(20)..].join(" ").to_lowercase()
            } else {
                String::new()
            };
            let cue = cues.iter().any(|c| own.contains(c.as_str()) || before.contains(c.as_str()));
            // A part may list several roles: «S1, S2, S3»
            let role = part.role.as_ref().is_some_and(|want| {
                roles.get(&r.speaker).is_some_and(|has| {
                    let has = has.to_lowercase();
                    want.split(',').map(|w| w.trim().to_lowercase()).filter(|w| !w.is_empty()).any(|w| has == w || has.contains(&w))
                })
            });
            cue || role
        });
        if let Some(i) = found {
            if out.last().map(|(at, _)| *at < i).unwrap_or(true) || out.last().map(|(at, _)| *at == i).unwrap_or(false) {
                out.retain(|(at, _)| *at != i);
                out.push((i, part.title.clone()));
                from = i + 1;
            }
        }
    }
    out
}

/// Tasks of an earlier meeting, from its document's «Завдання» table:
/// "що | хто | до коли" per row
pub(crate) fn tasks_from_document(markdown: &str) -> Vec<String> {
    let mut inside = false;
    let mut out = Vec::new();
    for line in markdown.lines() {
        let t = line.trim();
        if t.starts_with("## ") {
            inside = t == "## Завдання";
            continue;
        }
        if !inside || !t.starts_with('|') || t.starts_with("| Хто") || t.starts_with("| ---") {
            continue;
        }
        let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
        if let [who, what, when, ..] = cells.as_slice() {
            if !what.is_empty() && *what != "—" {
                out.push(format!("{what} | {who} | {when}"));
            }
        }
    }
    out
}

/// Earlier tasks passed in the meeting facts («Завдання попередньої наради:» then «- …» lines)
pub(crate) fn earlier_tasks_from_facts(custom_prompt: &str) -> Vec<String> {
    let Some(rest) = custom_prompt.split("Завдання попередньої наради:").nth(1) else { return Vec::new() };
    rest.lines().map(str::trim).take_while(|l| l.is_empty() || l.starts_with("- ")).filter_map(|l| l.strip_prefix("- ")).map(str::to_string).collect()
}

/// Transcript lines most related to a task (by shared word stems), in time order
pub(crate) fn lines_about(lines: &[Line], task: &str, max: usize) -> Vec<usize> {
    let what = task.split('|').next().unwrap_or(task);
    let mut scored: Vec<(usize, usize)> = lines.iter().enumerate().map(|(i, l)| (shared_stems(&l.text, what), i)).filter(|(n, _)| *n >= 2).collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut picked: Vec<usize> = scored.into_iter().take(max).map(|(_, i)| i).collect();
    picked.sort();
    picked
}

/// Asks whether one earlier task was reported on, from a few related lines
pub(crate) fn control_user_prompt(task: &str, excerpt: &str) -> String {
    format!(
        r#"Завдання з попередньої наради: {task}

Уривки з поточної наради, де про це може йтися:
<transcript>
{excerpt}
</transcript>

Чи доповіли про це завдання? Відповідай одним рядком у форматі:
<стан> | <мітка часу рядка [ММ:СС] або —>
де <стан> — одне з: Виконано, Не виконано, Перенесено, В процесі, Не згадувалось. Вибирай «Не згадувалось», якщо в уривках про це завдання прямо не сказано. Нічого не вигадуй."#
    )
}

/// The answer to [`control_user_prompt`]: status and moment
pub(crate) fn parse_control_answer(output: &str, lines: &[Line]) -> (String, Option<u32>) {
    const STATES: &[&str] = &["Виконано", "Не виконано", "Перенесено", "В процесі", "Не згадувалось"];
    let line = output.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let lower = line.to_lowercase();
    // Longest match first: «Не виконано» before «Виконано»
    let mut states: Vec<&&str> = STATES.iter().collect();
    states.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let status = states.into_iter().find(|s| lower.contains(&s.to_lowercase())).map(|s| s.to_string()).unwrap_or_else(|| "Не згадувалось".into());
    let at = line
        .rfind('[')
        .and_then(|o| line[o + 1..].split(']').next().and_then(parse_mmss))
        .and_then(|s| line_at(lines, s).map(|i| lines[i].seconds));
    let at = if status == "Не згадувалось" { None } else { at };
    (status, at)
}

/// Prompt for the fields of one report («Зроблено», «План»…)
pub(crate) fn report_fields_user_prompt(speaker: &str, text: &str, fields: &[String]) -> String {
    let form: String = fields
        .iter()
        .map(|f| if f.to_lowercase().starts_with("план") { format!("{f}: <дії через «;», кожна з дієслова: «Підготувати…; Перевірити…»>\n") } else { format!("{f}: …\n") })
        .collect();
    format!(
        r#"Нижче доповідь одного учасника наради ({speaker}).
<report>
{text}
</report>

Заповни коротко за пунктами — лише те, що доповідач справді сказав по суті, без вигадок. Кожен пункт — одне коротке речення або перелік. Перекличку («на зв'язку», «присутній»), привітання і фрази на кшталт «доповідь закінчив», «в мене все» не записуй. Якщо про пункт нічого по суті не сказано, напиши «—».
Не губи: звернення й прохання до всіх, доручення («підготувати», «надати людей», «визначити»), подяки й похвалу, призначені наради — це в пункт «Оголошення / доручення». «Потреби» — лише те, що потрібно самому доповідачеві від інших. Зберігай, хто що зробив: що передали доповідачеві, а що зробив він сам.
{form}
Виведи лише ці рядки."#
    )
}

/// The type's fields with «Оголошення / доручення» added when it has no
/// such field (custom types made before it existed)
pub(crate) fn fields_with_announcements(fields: &[String]) -> Vec<String> {
    let mut out = fields.to_vec();
    if !out.is_empty() && !out.iter().any(|f| {
        let l = f.to_lowercase();
        l.contains("доруч") || l.contains("оголош")
    }) {
        out.push("Оголошення / доручення".to_string());
    }
    out
}

/// Words that make a sentence matter for the protocol: a request or an
/// order, a deadline, thanks
const MATTERS: &[&str] = &[
    "прош", "прохан", "треба", "потрібн", "необхідн", "підготу", "надат", "надайт", "визнач", "передай", "передайт", "зробіт", "перевірт",
    "доповіст", "зверн", "звертай", "сьогодні", "завтра", "обід", "вечор", "наступн", "до кінц", "дяку", "молодц", "хвал", "нарад",
];

/// Sentences of a turn that carry a request, a number, a deadline or
/// thanks but are not in any bullet of the write-up: given to the secretary
/// as «Не враховано» instead of being lost (at most three)
pub(crate) fn unreflected(turn: &str, written: &[String]) -> Vec<String> {
    let stems = |s: &str| -> HashSet<String> {
        s.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 4).map(|w| w.to_lowercase().chars().take(5).collect()).collect()
    };
    let have: HashSet<String> = written.iter().flat_map(|w| stems(w)).collect();
    let mut out = Vec::new();
    for sentence in turn.split_inclusive(['.', '!', '?']).map(str::trim) {
        let lower = sentence.to_lowercase();
        let words = sentence.split_whitespace().count();
        if words < 5 || words > 40 || REPORT_CLOSINGS.iter().any(|c| lower.contains(c)) || REPORT_OPENINGS.iter().any(|o| lower.starts_with(o)) {
            continue;
        }
        let matters = sentence.chars().any(|c| c.is_ascii_digit()) || MATTERS.iter().any(|m| lower.contains(m));
        let own = stems(sentence);
        if !matters || own.len() < 3 {
            continue;
        }
        // Said in other words is fine: two shared stems or a quarter of them
        let shared = own.iter().filter(|w| have.contains(*w)).count();
        if shared < 2 && (shared as f64) < own.len() as f64 * 0.25 {
            out.push(sentence.to_string());
            if out.len() == 2 {
                break;
            }
        }
    }
    out
}

/// Field values from the model's answer; empty ones («—») are left out
pub(crate) fn parse_report_fields(output: &str, fields: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for field in fields {
        let found = output.lines().find_map(|l| {
            let l = l.trim().trim_start_matches(['-', '*', '•', ' ']).replace("**", "");
            let (name, value) = l.split_once(':')?;
            (name.trim().eq_ignore_ascii_case(field)).then(|| value.trim().to_string())
        });
        if let Some(v) = found {
            let lower = v.to_lowercase();
            // Filler: nothing reported, roll call, the end of a report
            let filler = ["доповідь закінч", "в мене все", "у мене все", "на зв'язку", "на звʼязку", "присутн", "нічого не сказано"]
                .iter()
                .any(|f| lower.contains(f))
                && lower.split_whitespace().count() <= 12;
            // The model sometimes echoes the form («…», «<дії через «;»…>»)
            let placeholder = v.trim_matches(|c: char| c == '.' || c == '…' || c.is_whitespace()).is_empty() || v.trim_start().starts_with('<');
            let empty = v.is_empty() || v == "—" || v == "-" || lower.starts_with("не сказано") || lower == "немає" || filler || placeholder;
            if !empty {
                out.push((field.clone(), v));
            }
        }
    }
    out
}

/// Regular participants («Постійні учасники: А, Б, В.» in the meeting facts)
/// who did not speak in this meeting
pub(crate) fn absent_regulars(custom_prompt: &str, reports: &[Report]) -> Vec<String> {
    let Some(list) = custom_prompt.split("Постійні учасники: ").nth(1).map(|r| r.split('\n').next().unwrap_or("")) else { return Vec::new() };
    list.trim_end_matches('.')
        .split(", ")
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .filter(|n| !reports.iter().any(|r| r.speaker.trim_end_matches('?') == *n))
        .map(str::to_string)
        .collect()
}

/// Speaker roles («Ролі: А — керівник; Б — керівник групи.»)
pub(crate) fn roles_from_facts(custom_prompt: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    if let Some(list) = custom_prompt.split("Ролі: ").nth(1).map(|r| r.split('\n').next().unwrap_or("")) {
        for pair in list.trim_end_matches('.').split("; ") {
            if let Some((name, role)) = pair.split_once(" — ") {
                out.insert(name.trim().to_string(), role.trim().to_string());
            }
        }
    }
    out
}

/// A remark too short to be a report of its own («Плюс. Далі.», «Так, кажіть»)
pub(crate) fn is_interjection(text: &str) -> bool {
    text.split_whitespace().count() < 6
}

/// «Стенограма»: a header line, then each report in order — a speaker line
/// and the cleaned verbatim text. `polished` (by report index) replaces the
/// rule-based cleanup where a guarded polish was accepted.
pub(crate) fn build_transcript_document(
    date: Option<&str>,
    lines: &[Line],
    reports: &[Report],
    polished: &std::collections::HashMap<usize, String>,
) -> String {
    let duration_min = lines.last().map(|l| l.seconds.div_ceil(60)).unwrap_or(0);
    let mut md = match date {
        Some(date) => format!("Нарада {date}, {duration_min} хв\n\n"),
        None => format!("Нарада, {duration_min} хв\n\n"),
    };
    for (i, report) in reports.iter().enumerate() {
        let text = polished.get(&i).cloned().unwrap_or_else(|| report_text(lines, reports, i));
        if text.trim().is_empty() {
            continue;
        }
        // Short remarks between reports go in italics, not as a new report
        if is_interjection(&text) && i > 0 && !polished.contains_key(&i) {
            md.push_str(&format!("_{}: {}_\n\n", report.speaker, text.trim()));
            continue;
        }
        md.push_str(&format!("**{}** ({}):\n{}\n\n", report.speaker, mmss(lines[report.start].seconds), text.trim()));
    }
    md
}

/// Rule-based cleaned text of report `i`
pub(crate) fn report_text(lines: &[Line], reports: &[Report], i: usize) -> String {
    let end = reports.get(i + 1).map(|r| r.start).unwrap_or(lines.len());
    let start = reports[i].start.min(end);
    clean_spoken(&lines[start..end].iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" "))
}

pub(crate) fn polish_user_prompt(text: &str) -> String {
    format!(
        r#"Нижче фрагмент стенограми наради. Виправ лише:
- розділові знаки і поділ на речення;
- очевидні граматичні помилки розпізнавання (закінчення, узгодження);
- повтори і слова-паразити.
НЕ переказуй, НЕ скорочуй, НЕ додавай слів і фактів, НЕ змінюй імена, цифри й терміни. Збережи всі думки в тому ж порядку. Виведи лише виправлений текст.

<text>
{text}
</text>"#
    )
}

fn content_stems(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase())
        .filter(|w| w.chars().count() >= 4 || w.chars().any(|c| c.is_ascii_digit()))
        .map(|w| w.chars().take(5).collect())
        .collect()
}

/// Accepts a polished paragraph only if it kept the content: at least 90 % of
/// the source's content words survive, and it adds almost no new ones
pub(crate) fn polish_is_faithful(source: &str, polished: &str) -> bool {
    let src = content_stems(source);
    let out = content_stems(polished);
    if src.is_empty() {
        return false;
    }
    let out_set: std::collections::HashSet<&String> = out.iter().collect();
    let src_set: std::collections::HashSet<&String> = src.iter().collect();
    let kept = src.iter().filter(|w| out_set.contains(w)).count();
    let added = out.iter().filter(|w| !src_set.contains(w)).count();
    kept * 10 >= src.len() * 9 && added * 20 <= out.len().max(1)
}

/// "Відомі факти: нараду записано 04.11.2026 о 16:59." → "04.11.2026"
pub(crate) fn date_from_facts(custom_prompt: &str) -> Option<String> {
    let after = custom_prompt.split("нараду записано ").nth(1)?;
    let date: String = after.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    (date.len() == 10).then_some(date)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_block_per_person_in_the_order_they_first_spoke() {
        let line = |s: u32, t: &str| Line { seconds: s, text: t.to_string() };
        let lines = vec![line(10, "Доброго ранку."), line(20, "Склад готовий."), line(30, "Дякую."), line(40, "Ще одне: звіт у п'ятницю."), line(50, "Зрозуміло.")];
        let rep = |start: usize, s: &str| Report { start, speaker: s.to_string() };
        let reports = vec![rep(0, "Сокіл"), rep(1, "Спікер 2"), rep(2, "Сокіл"), rep(4, "Спікер 2")];
        let mut b = speaker_blocks(&lines, &reports);
        assert_eq!(b.len(), 2);
        assert_eq!((b[0].speaker.as_str(), b[0].seconds, b[0].lines.len()), ("Сокіл", 10, 3));
        assert_eq!((b[1].speaker.as_str(), b[1].seconds, b[1].lines.len()), ("Спікер 2", 20, 2));
        assert!(b[0].spoken > 0);
        b[0].text = "- Привітання [00:10]".into();
        b[1].text = "- Склад готовий [00:20]".into();
        let md = speaker_blocks_markdown("Нарада", Some("08.11.2026"), &lines, &reports, &b, "- Підсумок", None);
        assert!(md.find("**ВИСТУПИЛИ:** Сокіл [00:10]").unwrap() < md.find("**ВИСТУПИЛИ:** Спікер 2 [00:20]").unwrap());
        assert!(md.find("Спікер 2").unwrap() < md.find("## Загальний підсумок").unwrap());
        // «Затвердити» reads the same document
        let doc = super::super::learned::parse_doc(&md);
        assert_eq!(doc.reports.len(), 2);
        assert_eq!((doc.reports[0].speaker.as_str(), doc.reports[0].time), ("Сокіл", Some(10)));
        assert!(doc.reports[0].body.contains("Привітання"));
        assert_eq!(lines_for_model(&b[0].lines, 12).len(), 3);
        assert_eq!(as_points("Перший\n- Другий\n\n* Третій", 8), "- Перший\n- Другий\n- Третій");
        assert_eq!(as_points("- А\n- Б\n- В", 2), "- А\n- Б");
        assert_eq!((points_for(30), points_for(40), points_for(72), points_for(149), points_for(150), points_for(300), points_for(5000)), (1, 3, 4, 6, 3, 5, 8));
        // Service remarks go, the rest of the line stays
        let lines = vec![(5, "Доброго ранку, панове. Отримали 5 генераторів. Дякую.".to_string()), (9, "Так.".to_string()), (12, "У когось якісь питання?".to_string())];
        assert_eq!(working_lines(&lines), vec![(5, "Отримали 5 генераторів.".to_string())]);
        assert_eq!(clean_points("- Всім гарного дня [00:10]\n- Отримано 5 генераторів [00:12]\n- Отримано 5 генераторів [00:13]\n- Це нікого не їбе [00:13]\n- Пане Тест [00:14]"), "- Отримано 5 генераторів [00:12]");
        // A short report line is kept; an answer alone is not; «кораблями» is not swearing
        let lines = vec![(5, "Втрат немає. Так. Орієнтир Дотик.".to_string()), (9, "Зрозумів.".to_string())];
        assert_eq!(working_lines(&lines), vec![(5, "Втрат немає. Орієнтир Дотик.".to_string())]);
        assert_eq!(clean_points("- Втрат немає [00:05]\n- Удари по кораблями і рублями [00:06]"), "- Втрат немає [00:05]\n- Удари по кораблями і рублями [00:06]");
        // Points in the order they were said: one from the middle stays in the middle
        assert_eq!(
            in_said_order("- А [00:10]\n- В [01:30]\n- Б [00:40, 02:00]\n- без часу\n- Г [01:50] ⟨?⟩"),
            "- А [00:10]\n- Б [00:40, 02:00]\n- без часу\n- В [01:30]\n- Г [01:50] ⟨?⟩"
        );
        // A lost number goes into the point from its line; else a short point; never more than allowed
        let said = vec![(10, "Передали склад і 150 одиниць майна до офісу.".to_string()), (20, "На ділянці 7 машин, далі 8 ящиків, потім 9 бочок, ще 11 мішків.".to_string())];
        let (text, kept) = keep_numbers("- Передали склад до офісу [00:10]", &said);
        // The numbers of one sentence come back in one point, made of whole clauses
        assert_eq!(kept, NumbersKept { appended: 4, added: 1, left: 0 });
        assert!(text.lines().next().unwrap().contains("150") && text.lines().next().unwrap().ends_with("[00:10]"));
        assert_eq!(text.lines().count(), 2);
        assert_eq!(text.lines().nth(1), Some("- На ділянці 7 машин, далі 8 ящиків, потім 9 бочок, ще 11 мішків [00:20]"));
        let said = vec![(86, "Після завтрашнього дня, тобто з 14 листопада, у нас запускається по плану новий процес обліку складу, тобто ми ведемо і в системі, і продовжуємо робити це через групу, як ми це робили попередні 4,5 роки.".to_string())];
        let (text, _) = keep_numbers("- Інше [00:10]", &said);
        assert_eq!(
            text.lines().nth(1),
            Some("- Після завтрашнього дня, тобто з 14 листопада, у нас запускається по плану новий процес обліку складу; як ми це робили попередні 4,5 роки [01:26]")
        );
        assert!(text.lines().all(|l| !l.contains("дослівно")));
        // A clause that hangs on the one before («який …») never comes back
        // alone: here the whole thought is a point of its own
        let said = vec![(236, "Також підготовка попередніх даних для ремонту обладнання, яке надійшло з філії 4512, для оформлення кошторису на ремонт.".to_string())];
        let (text, kept) = keep_numbers("- Зустрічі з новими працівниками відділу [03:56]", &said);
        assert_eq!(kept, NumbersKept { appended: 0, added: 1, left: 0 });
        assert_eq!(
            text,
            "- Зустрічі з новими працівниками відділу [03:56]\n- Підготовка попередніх даних для ремонту обладнання, яке надійшло з філії 4512, для оформлення кошторису на ремонт [03:56]"
        );
        assert_eq!(clause_around("Техніка, яка прибула 12 числа, на складі.", "12", 9), "Техніка, яка прибула 12 числа, на складі");
        // A number said in words backs the digits; one nobody said is marked
        let said = super::super::numbers::Said::of("отримали двадцять генераторів");
        let (text, n) = mark_unbacked_numbers("- Отримали 20 генераторів [00:10]\n- Отримали 35 генераторів [00:20]", &said, &said);
        assert_eq!(n, 1);
        assert!(text.lines().next().unwrap().ends_with("[00:10]"));
        assert!(text.lines().nth(1).unwrap().ends_with(NO_SOURCE));
        // A suggested name and an unnamed voice are speakers too
        assert_eq!(speaker_prefix("Сокіл?: Доброго ранку"), Some(("Сокіл?", "Доброго ранку")));
        assert_eq!(speaker_prefix("?: Так"), Some(("?", "Так")));
    }

    #[test]
    fn speech_is_joined_cleanly() {
        let j = |p: &[&str]| join_speech(&p.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(j(&["доброго ранку,", "Склад готовий"]), "Доброго ранку. Склад готовий.");
        assert_eq!(j(&["Це перше.", "а це друге"]), "Це перше. А це друге.");
        assert_eq!(j(&["ми поїхали", "на склад і"]), "Ми поїхали на склад і.");
        assert_eq!(j(&["Так,"]), "Так.");
        assert_eq!(j(&[" ", ""]), "");
        assert_eq!(capitalize_sentences("Це перше. а це друге. і третє"), "Це перше. А це друге. І третє");
        assert_eq!(capitalize_sentences("у м. київ, вул. шевченка, ім. франка"), "у м. київ, вул. шевченка, ім. франка");
        assert_eq!(capitalize_sentences("2026 р. далі, 5 тис. грн. усього, т.д. інше, а. б. Коваль"), "2026 р. далі, 5 тис. грн. усього, т.д. інше, а. б. Коваль");
        assert_eq!(capitalize_sentences("Без крапки в кінці."), "Без крапки в кінці.");
    }

    #[test]
    fn stenogram_turns_and_header() {
        let l = |s: &str, t: &str, a: f64, b: f64| StenoLine { speaker: s.into(), text: t.into(), start: a, end: b };
        let plain_lines = [
            StenoLine { speaker: "Сокіл".into(), text: "Доброго ранку.".into(), start: 3.0, end: 5.0 },
            StenoLine { speaker: "Сокіл".into(), text: "Почнемо.".into(), start: 5.0, end: 7.0 },
            StenoLine { speaker: "Сокіл".into(), text: "— Мікрофон вимкнено —".into(), start: 7.0, end: 7.5 },
            StenoLine { speaker: "Сокіл".into(), text: "Ще одне.".into(), start: 8.0, end: 10.0 },
            StenoLine { speaker: "Спікер 3".into(), text: "Прийнято.".into(), start: 61.0, end: 90.0 },
            StenoLine { speaker: "Сокіл?".into(), text: "Дякую.".into(), start: 91.0, end: 121.0 },
        ];
        assert_eq!(
            stenogram_plain("Нарада 01.02.2026 10:00", &plain_lines),
            ("Нарада 01.02.2026 10:00 · 3 хв\n\nСокіл · 00:03\nДоброго ранку. Почнемо. Ще одне.\n\nСпікер 3 · 01:01\nПрийнято.\n\nСокіл? · 01:31\nДякую.".to_string(), 3)
        );
        let md = stenogram_markdown("Нарада", Some("08.11.2026"), &[l("Сокіл", "доброго ранку", 3.0, 5.0), l("Сокіл", "починаємо", 5.0, 7.0), l("Вузол", "Склад готовий.", 8.0, 70.0), l("?", "так", 71.0, 72.0), l("Сокіл", "Дякую.", 73.0, 75.0)]);
        assert!(md.contains("**Учасники:** Сокіл, Вузол"));
        assert!(md.contains("**Тривалість:** 2 хв"));
        assert!(md.contains("**Сокіл · 00:03**\n\nДоброго ранку починаємо."));
        assert!(md.contains("**Вузол · 00:08**"));
        assert_eq!(md.matches("**Сокіл ·").count(), 2);
    }

    #[test]
    fn overlapping_windows_cover_every_line_with_overlap() {
        let text: String = (0..300).map(|i| format!("{:02}:{:02} Рядок номер {i} про щось важливе на нараді\n", i / 60, i % 60)).collect();
        let w = overlapping_windows(&text, 1000, 100);
        assert!(w.len() > 2);
        // Every line is in some window; consecutive windows share lines
        for i in 0..300 {
            assert!(w.iter().any(|x| x.contains(&format!("номер {i} "))), "line {i} lost");
        }
        for pair in w.windows(2) {
            let last = pair[0].lines().last().unwrap();
            assert!(pair[1].contains(last), "no overlap");
            assert!(super::super::processor::rough_token_count(&pair[0]) <= 1000 + 30);
        }
        // Short text: one window, unchanged lines
        assert_eq!(overlapping_windows("00:01 Так\n00:02 Ні\n", 1000, 100), vec!["00:01 Так\n00:02 Ні\n".to_string()]);
    }

    #[test]
    fn window_items_merged_by_stems_and_time() {
        let it = |f: &[&str], s: u32| Item { fields: f.iter().map(|x| x.to_string()).collect(), seconds: s };
        let merged = merge_window_items(vec![
            it(&["Підготувати звіт про ремонт техніки", "—", "—"], 400),
            it(&["Затвердити графік чергування"], 500),
            // The same task from the next window, with its owner and deadline
            it(&["Підготувати звіт щодо ремонту техніки", "Петренко", "до п'ятниці"], 380),
            // Same words an hour later: another item
            it(&["Затвердити графік чергування"], 4100),
        ]);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].seconds, 380);
        assert_eq!(merged[0].fields, vec!["Підготувати звіт про ремонт техніки", "Петренко", "до п'ятниці"]);
        assert_eq!(merged[2].seconds, 4100);
    }

    fn lines() -> Vec<Line> {
        parse_transcript("[00:00] Добрий день, починаємо.\n[00:30] Ну доповідає склад, е-е пального на три дні.\n[01:10] Дякую. Далі зв'язок.\n[01:20] Зв'язок працює, треба треба замінити антену до п'ятниці.\n[02:00] Дякую всім.")
    }

    #[test]
    fn decisions_need_decision_words_and_tasks_are_not_repeated() {
        let l = parse_transcript("[00:00] Вирішили: за постачання відповідає склад.\n[00:30] Вчора закрили звіт по складу.\n[01:00] Треба підготувати звіт до п'ятниці.");
        let d = vec![
            Item { fields: vec!["За постачання відповідає склад".into()], seconds: 0 },
            Item { fields: vec!["Закрили звіт по складу".into()], seconds: 30 },
        ];
        assert_eq!(keep_real_decisions(d, &l).len(), 1);
        // The decision word may be in the next sentence
        let l2 = parse_transcript("[00:00] Перше, весь напрямок постачання. Цим буде займатися склад.");
        let d2 = vec![Item { fields: vec!["Склад відповідає за напрямок постачання".into()], seconds: 0 }];
        assert_eq!(keep_real_decisions(d2, &l2).len(), 1);
        let t = vec![
            Item { fields: vec!["Підготувати звіт до п'ятниці".into(), "склад".into()], seconds: 60 },
            Item { fields: vec!["Підготувати звіт".into(), "—".into()], seconds: 60 },
            Item { fields: vec!["Замовити пальне".into()], seconds: 60 },
        ];
        assert_eq!(dedup_tasks(t).len(), 2);
        // A wrong [00:00] from the model moves to the line with the item's words
        let items = parse_items("## Завдання\n- Підготувати звіт до п'ятниці | склад | — [00:00]", "Завдання", &l);
        assert_eq!(items[0].seconds, 60);
        assert!(is_interjection("Плюс. Далі."));
    }

    #[test]
    fn meeting_type_parts_fields_and_absent() {
        let mut l = parse_transcript("[00:00] Я: Оголошення: завтра навчання.\n[00:20] Спікер 2: Доповідаю по складу.\n[00:40] Я: Переходимо до керівників.\n[00:45] Спікер 3: Група готова.\n[01:00] Спікер 4: Команда готова.");
        let reports = speaker_reports(&mut l).unwrap();
        let parts = vec![
            MeetingPart { title: "Оголошення".into(), ..Default::default() },
            MeetingPart { title: "Доповіді керівників".into(), cues: vec!["переходимо до керівників".into()], role: None },
        ];
        let starts = assign_parts(&parts, &l, &reports, &Default::default());
        // The part starts with the chair's transition («Переходимо до керівників»)
        assert_eq!(starts, vec![(0, "Оголошення".to_string()), (2, "Доповіді керівників".to_string())]);
        // By role, when roles are known
        let mut roles = std::collections::HashMap::new();
        roles.insert("Спікер 4".to_string(), "Керівник групи".to_string());
        let by_role = vec![MeetingPart { title: "Керівники".into(), cues: vec![], role: Some("керівник".into()) }];
        // Several roles for one part
        let several = vec![MeetingPart { title: "Секції".into(), cues: vec![], role: Some("S1, S2".into()) }];
        let mut r2 = std::collections::HashMap::new();
        r2.insert("Спікер 3".to_string(), "S2".to_string());
        assert_eq!(assign_parts(&several, &l, &reports, &r2), vec![(3, "Секції".to_string())]);
        assert_eq!(assign_parts(&by_role, &l, &reports, &roles), vec![(4, "Керівники".to_string())]);
        let fields = vec!["Зроблено".to_string(), "Проблеми".to_string()];
        assert_eq!(parse_report_fields("Зроблено: облаштували склад\n**Проблеми:** —", &fields), vec![("Зроблено".to_string(), "облаштували склад".to_string())]);
        assert_eq!(absent_regulars("Відомі факти: …\nПостійні учасники: Олена, Спікер 3, Богдан.", &reports), vec!["Олена".to_string(), "Богдан".to_string()]);
        assert_eq!(roles_from_facts("Ролі: Олена — керівник; Богдан — керівник групи.").get("Богдан").map(String::as_str), Some("керівник групи"));
    }

    #[test]
    fn control_of_earlier_tasks() {
        let doc = "# Щоденна нарада\n\n## Завдання\n\n| Хто | Що | До коли | Час у записі |\n| --- | --- | --- | --- |\n| склад | Підготувати звіт про пальне | до вечора | [01:00] |\n| — | — | — | [02:00] |\n\n## Відкриті питання\n";
        let tasks = tasks_from_document(doc);
        assert_eq!(tasks, vec!["Підготувати звіт про пальне | склад | до вечора".to_string()]);
        let facts = format!("Відомі факти: …\nЗавдання попередньої наради:\n- {}\n", tasks[0]);
        assert_eq!(earlier_tasks_from_facts(&facts), tasks);
        let l = parse_transcript("[00:00] Добрий вечір.\n[00:30] Звіт про пальне підготували, передали.\n[01:00] Інше питання.");
        assert_eq!(lines_about(&l, &tasks[0], 3), vec![1]);
        assert_eq!(parse_control_answer("Виконано | [00:30]", &l), ("Виконано".to_string(), Some(30)));
        assert_eq!(parse_control_answer("Не виконано | —", &l).0, "Не виконано");
        assert_eq!(parse_control_answer("Не згадувалось | [00:30]", &l), ("Не згадувалось".to_string(), None));
    }

    #[test]
    fn a_decision_said_a_few_sentences_later_is_kept() {
        let l = parse_transcript("[00:00] Перше, весь напрямок постачання відділу. Це закупівлі. Це склад. Цим всім буде займатися Тарас Коваль. Він усіх знає.");
        let d = vec![Item { fields: vec!["Тарас Коваль відповідає за напрямок постачання відділу".into()], seconds: 0 }];
        assert_eq!(keep_real_decisions(d, &l).len(), 1);
        let not = vec![Item { fields: vec!["Закупити нові столи".into()], seconds: 0 }];
        assert!(keep_real_decisions(not, &l).is_empty());
    }

    #[test]
    fn a_roll_call_is_not_a_report() {
        assert!(is_roll_call("Доброго ранку. С1. На зв'язку. С2. На місці. С3. На зв'язку. Склад. Присутній. Зв'язок. На зв'язку."));
        assert!(!is_roll_call("Доповідаю: на зв'язку з постачальником, склад готовий, пальне отримали, завтра перевіримо генератор і доповімо."));
    }

    #[test]
    fn report_answers_give_a_summary_and_tasks() {
        let answer = "Коротко: Склад готовий, бракує двох генераторів.\nЗавдання:\n- Підготувати заявку на генератори | — | до вечора\n- Перевірити | — | —\n- Передати ключі черговому | Черговий | —";
        let (summary, tasks) = parse_report_summary(answer, 75);
        assert_eq!(summary, "Склад готовий, бракує двох генераторів.");
        // «Перевірити» alone is too vague
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].fields, vec!["Підготувати заявку на генератори", "—", "до вечора"]);
        assert_eq!(tasks[1].fields[1], "Черговий");
        assert_eq!(parse_report_summary("Завдання:\n- продовжити навчання складу | — | —", 0).1[0].fields[0], "Продовжити навчання складу");
        assert_eq!(without_leading_speaker("Спікер 2 повідомив про доставки.", "Спікер 2"), "Повідомив про доставки.");
        assert_eq!(without_leading_speaker("Спікер 21 теж.", "Спікер 2"), "Спікер 21 теж.");
        assert_eq!(tasks[1].seconds, 75);
        let (s2, t2) = parse_report_summary("Коротко: Без змін.\nЗавдання: немає", 0);
        assert_eq!(s2, "Без змін.");
        assert!(t2.is_empty());
        // An answer without the labels is all summary
        assert_eq!(parse_report_summary("Все добре.", 0).0, "Все добре.");
    }

    #[test]
    fn what_was_asked_is_not_lost() {
        let turn = "Доброго ранку. Склад отримав пальне. Прошу всіх керівників до обіду надати списки водіїв. Передайте розрахунку, що вони молодці. Вчора все пройшло спокійно і без пригод.";
        let written = vec!["Отримано пальне на склад".to_string()];
        let missed = unreflected(turn, &written);
        assert_eq!(missed.len(), 2);
        assert!(missed[0].starts_with("Прошу всіх керівників"));
        assert!(missed[1].contains("молодці"));
        // Said in other words: not flagged
        let reworded = vec!["Пояснити співвідношення службової ієрархії".to_string()];
        assert!(unreflected("Величезне прохання до всіх зрозуміти, як працюють вертикалі і горизонталі співвідношення службової ієрархії.", &reworded).is_empty());
        // The form echoed back is empty
        assert!(parse_report_fields("Зроблено: …\nПлан на сьогодні: <дії через «;»>\nПроблеми: Бракує пального.", &["Зроблено".into(), "План на сьогодні".into(), "Проблеми".into()]).len() == 1);
        // Once written up, nothing is flagged
        let written = vec!["Отримано пальне".to_string(), "Керівникам до обіду надати списки водіїв".to_string(), "Передати подяку розрахунку, молодці".to_string()];
        assert!(unreflected(turn, &written).is_empty());
        assert_eq!(fields_with_announcements(&["Зроблено".into(), "Потреби".into()]).last().unwrap(), "Оголошення / доручення");
        assert_eq!(fields_with_announcements(&["Зроблено".into(), "Доручення".into()]).len(), 2);
        assert!(fields_with_announcements(&[]).is_empty());
    }

    #[test]
    fn one_voice_two_reports() {
        let mut l = parse_transcript("[00:00] Я: Почнемо.\n[00:10] Спікер 2: Доброго ранку, шановні колеги. Склад без змін. Дякую, в мене все.\n[00:40] Спікер 2: Доброго ранку. Зв'язок працює, антену замінили.\n[01:10] Спікер 2: Завтра перевіримо генератор.\n[01:30] Я: Дякую.");
        let r = speaker_reports(&mut l).unwrap();
        assert_eq!(r.iter().map(|x| (x.start, x.speaker.as_str())).collect::<Vec<_>>(), vec![(0, "Я"), (1, "Спікер 2"), (2, "Спікер 2"), (4, "Я")]);
        // A greeting in the middle of a report is not a new one
        assert!(!opens_new_report("Склад без змін, генератор працює.", "Доброго ранку всім, хто приєднався."));
        assert!(opens_new_report("Наразі у мене все, доповідь закінчив.", "Добрий день, шановний колего."));
    }

    #[test]
    fn remarks_do_not_split_a_report() {
        let mut l = parse_transcript("[00:00] Я: Почнемо.\n[00:10] Спікер 2: Доповідаю, все добре.\n[00:20] Репліки: Так.\n[00:22] Спікер 2: Далі по складу.\n[01:00] Я: Дякую.");
        let r = speaker_reports(&mut l).unwrap();
        assert_eq!(r.iter().map(|x| x.speaker.as_str()).collect::<Vec<_>>(), vec!["Я", "Спікер 2", "Я"]);
    }

    #[test]
    fn tasks_from_plans_and_short_tasks() {
        let mut l = parse_transcript("[00:00] Я: Почнемо.\n[00:10] Спікер 2: Доповідаю, сьогодні підготуємо звіт.\n[01:00] Я: Дякую.");
        let reports = speaker_reports(&mut l).unwrap();
        let mut fields = std::collections::HashMap::new();
        fields.insert(1, vec![("План на сьогодні".to_string(), "підготувати звіт про пальне, перевірити генератор".to_string())]);
        let t = tasks_from_plans(&l, &reports, &fields);
        assert_eq!(t.iter().map(|i| i.fields[0].as_str()).collect::<Vec<_>>(), vec!["Підготувати звіт про пальне", "Перевірити генератор"]);
        assert_eq!(t[0].fields[1], "Спікер 2");
        // No deadline unless one is said
        assert_eq!(t[0].fields[2], "—");
        fields.insert(1, vec![("План на сьогодні".to_string(), "Підготувати звіт про пальне, мастила й фільтри до вечора; Перевірити генератор".to_string())]);
        let t2 = tasks_from_plans(&l, &reports, &fields);
        assert_eq!(t2.len(), 2);
        assert_eq!(t2[0].fields[2], "до вечора");
        let parsed = parse_report_fields("Зроблено: На зв'язку.\nПроблеми: Не вистачає пального.\nПотреби: —", &["Зроблено".into(), "Проблеми".into(), "Потреби".into()]);
        assert_eq!(parsed, vec![("Проблеми".to_string(), "Не вистачає пального.".to_string())]);
        let long = Item { fields: vec!["Отже всі хто ще не встиг подивитися новий графік чергувань будь ласка подивіться його сьогодні ввечері і скажіть що думаєте".into()], seconds: 0 };
        assert!(short_tasks_only(vec![long]).is_empty());
        assert!(items_user_prompt("x", true, true).contains("## Завдання"));
        assert!(!items_user_prompt("x", true, false).contains("## Завдання"));
        let mut open = vec![Item { fields: vec!["Перевірити генератор".into(), "—".into()], seconds: 20 }];
        fill_owners(&mut open, &l, &reports);
        assert_eq!(open[0].fields[1], "Спікер 2");
    }

    #[test]
    fn done_work_is_not_a_task() {
        assert!(reports_done_work("Вчора ввечері закрили питання з документами"));
        assert!(reports_done_work("Відповідно до плану було надіслано звіт"));
        assert!(!reports_done_work("Сьогодні будемо займатися ремонтом"));
        assert!(!reports_done_work("Треба підготувати документи до п'ятниці"));
        assert!(!reports_done_work("Надайте інформацію до кінця дня"));
        assert_eq!(matching_sentence("Вчора закрили звіт. Сьогодні треба зустрітися з бухгалтером.", "зустрітися з бухгалтером"), " Сьогодні треба зустрітися з бухгалтером");
    }

    #[test]
    fn long_reports_are_summarised() {
        let mut l = parse_transcript("[00:00] Я: Почнемо.\n[00:05] Спікер 2: Доповідаю довго про склад і пальне.\n[00:20] Спікер 2: Ще про генератор і зв'язок.\n[00:50] Я: Дякую.");
        let reports = speaker_reports(&mut l).unwrap();
        let picked = reports_to_summarise(&l, &reports);
        assert_eq!(picked.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![1]);
        let mut summaries = std::collections::HashMap::new();
        summaries.insert(1, "Доповів, що все за планом.".to_string());
        let md = build_document(None, &l, &reports, &summaries, &[], &[], &[]);
        // The hint about unnamed voices is shown in the app, not in the document
        assert!(!md.contains("без імені"));
        assert!(md.contains("**ВИСТУПИЛИ:** Спікер 2 [00:05] — Доповів, що все за планом."));
    }

    #[test]
    fn reports_follow_recognised_speakers() {
        let mut l = parse_transcript("[00:00] Я: Добрий день. Богдане, тобі слово.\n[00:10] Спікер 2: Доповідаю, все за планом.\n[00:30] Спікер 2: Дякую.\n[00:40] Я: Дякую, Богдане.\n[00:50] Спікер 3: Зв'язок: працює.");
        let mut reports = speaker_reports(&mut l).unwrap();
        assert_eq!(reports.iter().map(|r| (r.start, r.speaker.as_str())).collect::<Vec<_>>(), vec![(0, "Я"), (1, "Спікер 2"), (3, "Я"), (4, "Спікер 3")]);
        assert_eq!(l[1].text, "Доповідаю, все за планом.");
        assert_eq!(l[4].text, "Зв'язок: працює.");
        assert_eq!(speaker_prefix("Олег (?): Доповідаю."), Some(("Олег (?)", "Доповідаю.")));
        // Names only when said next to that voice
        let names = parse_speaker_names("Спікер 2 | Богдан\nСпікер 3 | Петренко", &l, &reports);
        assert_eq!(names, vec![("Спікер 2".to_string(), "Богдан".to_string())]);
        apply_speaker_names(&mut reports, &names);
        assert_eq!(reports[1].speaker, "Богдан");
        // A name said in the middle of a report is about someone else
        let mut l2 = parse_transcript("[00:00] Спікер 1: Перше. Цим усім напрямком тепер буде займатися наш колега Богдан, звертайтеся.\n[00:20] Спікер 2: Доповідаю.");
        let r2 = speaker_reports(&mut l2).unwrap();
        assert!(parse_speaker_names("Спікер 1 | Богдан", &l2, &r2).is_empty());
        // Ordinary transcripts are left to the model
        let mut plain = lines();
        assert!(speaker_reports(&mut plain).is_none());
    }

    #[test]
    fn transcript_lines_are_parsed() {
        let l = lines();
        assert_eq!(l.len(), 5);
        assert_eq!(l[3].seconds, 80);
    }

    #[test]
    fn cleanup_removes_fillers_and_repeats_only() {
        assert_eq!(clean_spoken("Ну доповідає склад, е-е пального на три дні."), "Доповідає склад, пального на три дні.");
        assert_eq!(clean_spoken("треба треба замінити антену"), "Треба замінити антену");
        assert_eq!(clean_spoken("Тут ніяких змін."), "Тут ніяких змін.");
    }

    #[test]
    fn segmentation_is_validated() {
        let l = lines();
        let out = "00:30 | склад\n01:20 | Зв'язок\n00:50 | назад у часі\n99:00 | немає такого\n";
        // 01:20 follows «Дякую. Далі зв'язок.»: a handover
        let r = parse_segmentation(out, &l).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].start, 0);
        assert_eq!(r[1].speaker, "Зв'язок");
        assert!(parse_segmentation("нісенітниця", &l).is_none());
    }

    #[test]
    fn unnamed_speakers_are_numbered() {
        let l = lines();
        let r = parse_segmentation("00:00 | ?\n01:20 | Доповідач (не названо)\n", &l).unwrap();
        assert_eq!(r[0].speaker, "Доповідач 1");
        assert_eq!(r[1].speaker, "Доповідач 2");
    }

    #[test]
    fn items_need_a_real_moment() {
        let l = lines();
        let out = "## Рішення\n- Замінити антену [01:20]\n- Вигадане рішення без мітки\n- Інше [55:00]\n## Завдання\n- замінити антену | Зв'язок | до п'ятниці [01:20]\n";
        let d = parse_items(out, "Рішення", &l);
        // "Замінити антену" is found by its words; the made-up items are dropped
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].seconds, 80);
        let t = parse_items(out, "Завдання", &l);
        assert_eq!(t[0].fields, vec!["замінити антену", "Зв'язок", "до п'ятниці"]);
        // Without a time, the line is found by the task's words
        let t2 = parse_items("## Завдання\n- замінити антену зв'язок | — | до п'ятниці\n", "Завдання", &l);
        assert_eq!(t2.len(), 1);
        assert_eq!(t2[0].seconds, 80);
    }

    #[test]
    fn document_has_all_sections_and_verbatim_text() {
        let l = lines();
        let reports = vec![Report { start: 0, speaker: "Олена Петренко".into() }, Report { start: 3, speaker: "Зв'язок".into() }];
        let tasks = vec![Item { fields: vec!["замінити антену".into(), "Зв'язок".into(), "до п'ятниці".into()], seconds: 80 }];
        let md = build_document(Some("04.11.2026"), &l, &reports, &Default::default(), &[], &tasks, &[]);
        assert!(md.contains("**Дата:** 04.11.2026"));
        assert!(md.contains("**Учасники (названі в записі):** Олена Петренко"));
        assert!(md.contains("| Зв'язок | замінити антену | до п'ятниці | [01:20] |"));
        assert!(md.contains("### [01:20] Зв'язок\n\nЗв'язок працює, треба замінити антену до п'ятниці."));
        // No decisions: the section is not shown
        assert!(!md.contains("## Рішення"));
        assert!(md.find("## Завдання").unwrap() < md.find("## Стенограма по доповідачах").unwrap());
    }

    #[test]
    fn reported_facts_are_not_tasks() {
        let l = parse_transcript("[00:00] Ділянку облаштовано, доставки виконано.\n[00:30] Надайте звіт до кінця дня.\n[01:00] Новий генератор працює.");
        let tasks = vec![
            Item { fields: vec!["облаштувати ділянку".into(), "—".into(), "—".into()], seconds: 0 },
            Item { fields: vec!["надати звіт".into(), "—".into(), "до кінця дня".into()], seconds: 30 },
            Item { fields: vec!["перевірити генератор".into(), "Склад".into(), "—".into()], seconds: 60 },
            Item { fields: vec!["здати звіт".into(), "—".into(), "до п'ятниці".into()], seconds: 60 },
        ];
        let kept = keep_real_tasks(tasks, &l);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].fields[0], "надати звіт");
        assert_eq!(kept[1].fields[0], "здати звіт");
    }

    #[test]
    fn transcript_document_is_plain_text_by_speaker() {
        let l = lines();
        let reports = vec![Report { start: 0, speaker: "Доповідач 1".into() }, Report { start: 3, speaker: "Зв'язок".into() }];
        let md = build_transcript_document(Some("04.11.2026"), &l, &reports, &Default::default());
        assert!(md.starts_with("Нарада 04.11.2026, 2 хв"));
        assert!(md.contains("**Зв'язок** (01:20):\nЗв'язок працює, треба замінити антену до п'ятниці."));
        assert!(!md.contains('|') && !md.contains("##"));
    }

    #[test]
    fn polish_guard_rejects_dropped_or_added_content() {
        let src = "доповідаю що пального на складі на три дні заявку подали вчора ввечері";
        assert!(polish_is_faithful(src, "Доповідаю, що пального на складі на три дні. Заявку подали вчора ввечері."));
        assert!(!polish_is_faithful(src, "Пального на три дні."));
        assert!(!polish_is_faithful(src, "Доповідаю, що пального на складі на три дні, заявку подали вчора ввечері, керівник схвалив закупівлю генераторів."));
    }

    #[test]
    fn date_comes_from_meeting_facts() {
        assert_eq!(date_from_facts("Відомі факти: нараду записано 04.11.2026 о 16:59.").as_deref(), Some("04.11.2026"));
        assert_eq!(date_from_facts("нічого"), None);
    }

    #[test]
    fn repeated_points_are_written_once() {
        let points = vec![
            "Отримали 12 генераторів".to_string(),
            "отримали 12 генераторів на склад".to_string(),
            "Отримали 15 генераторів".to_string(),
            "Перевірили зв'язок".to_string(),
        ];
        // The fuller one stays; another number is another fact
        assert_eq!(dedup_points(points), vec!["отримали 12 генераторів на склад", "Отримали 15 генераторів", "Перевірили зв'язок"]);
    }

    #[test]
    fn a_speaker_who_spoke_three_times_gets_one_block() {
        let lines: Vec<Line> = [(5, "Остап"), (60, "Тарас"), (120, "Остап"), (200, "Тарас"), (260, "Остап")]
            .iter()
            .map(|(s, _)| Line { seconds: *s, text: "текст".into() })
            .collect();
        let reports: Vec<Report> = [(0, "Остап Гнатюк"), (1, "Тарас Мельник"), (2, "Остап Гнатюк"), (3, "Тарас Мельник"), (4, "Остап Гнатюк")]
            .iter()
            .map(|(i, s)| Report { start: *i, speaker: s.to_string() })
            .collect();
        let f = |v: &[(&str, &str)]| v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>();
        let mut fields = std::collections::HashMap::new();
        fields.insert(0, f(&[("Зроблено", "Отримали 12 генераторів; перевірили зв'язок"), ("Не враховано", "«Склад на 40 місць заповнено.»")]));
        fields.insert(1, f(&[("Зроблено", "Провели навчання")]));
        fields.insert(2, f(&[("Зроблено", "отримали 12 генераторів на склад"), ("Проблеми", "Бракує пального")]));
        fields.insert(3, f(&[("Зроблено", "Провели навчання з водіями")]));
        fields.insert(4, f(&[("Проблеми", "бракує пального"), ("Зроблено", "Склад на 40 місць заповнено")]));
        let spec = MeetingSpec { report_fields: vec!["Зроблено".into(), "Проблеми".into()], blocks: vec!["Хід наради".into()], ..Default::default() };
        assert!(spec.groups_by_speaker());
        let md = build_meeting_document(&DocInput {
            title: "Нарада",
            date: None,
            lines: &lines,
            reports: &reports,
            spec: &spec,
            summaries: &Default::default(),
            fields: &fields,
            part_starts: &[],
            absent: &[],
            control: &[],
            decisions: &[],
            tasks: &[],
            questions: &[],
            speaker_blocks: &[],
            overall: "",
            note: None,
        });
        assert_eq!(md.matches("**ВИСТУПИЛИ:**").count(), 2, "{md}");
        assert!(md.contains("**ВИСТУПИЛИ:** Остап Гнатюк [00:05; 02:00; 04:20]\n- Зроблено: отримали 12 генераторів на склад; перевірили зв'язок; Склад на 40 місць заповнено\n- Проблеми: Бракує пального\n"), "{md}");
        // Said later by the same person: no longer «not reflected»
        assert!(!md.contains("Не враховано"), "{md}");
        assert!(md.contains("**ВИСТУПИЛИ:** Тарас Мельник [01:00; 03:20]\n- Зроблено: Провели навчання з водіями\n"), "{md}");
        // Off: in time order, as before
        let spec = MeetingSpec { group_by_speaker: Some(false), ..spec };
        let md = build_meeting_document(&DocInput { title: "Нарада", date: None, lines: &lines, reports: &reports, spec: &spec, summaries: &Default::default(), fields: &fields, part_starts: &[], absent: &[], control: &[], decisions: &[], tasks: &[], questions: &[], speaker_blocks: &[], overall: "", note: None });
        assert_eq!(md.matches("**ВИСТУПИЛИ:**").count(), 5);
    }

    #[test]
    fn every_point_knows_where_it_was_said() {
        let lines: Vec<Line> = [(60, "Доброго ранку. Отримали дванадцять генераторів на склад."), (75, "Також перевірили зв'язок з усіма вузлами."), (90, "У мене все.")]
            .iter()
            .map(|(s, t)| Line { seconds: *s, text: t.to_string() })
            .collect();
        let reports = vec![Report { start: 0, speaker: "Остап Гнатюк".into() }];
        let mut fields = std::collections::HashMap::new();
        fields.insert(0, vec![
            ("Зроблено".to_string(), "Отримали генератори на склад; перевірили зв'язок з вузлами; провели навчання з водіями".to_string()),
            ("Не враховано".to_string(), "«Доброго ранку.»".to_string()),
        ]);
        let mut summaries = std::collections::HashMap::new();
        add_sources(&lines, &reports, &mut fields, &mut summaries);
        assert_eq!(fields[&0][0].1, "Отримали генератори на склад ⟨01:00⟩; перевірили зв'язок з вузлами ⟨01:15⟩; провели навчання з водіями ⟨?⟩");
        assert_eq!(fields[&0][1].1, "«Доброго ранку.»");
        // Out of DOCX/PDF
        assert_eq!(without_source_marks(&fields[&0][0].1), "Отримали генератори на склад; перевірили зв'язок з вузлами; провели навчання з водіями");
        // The same point said twice keeps both moments
        let merged = dedup_points(vec!["Отримали генератори ⟨01:00⟩".into(), "отримали генератори на склад ⟨12:06⟩".into(), "Провели навчання ⟨?⟩".into()]);
        assert_eq!(merged, vec!["отримали генератори на склад ⟨01:00 · 12:06⟩", "Провели навчання ⟨?⟩"]);
        assert_eq!(source_times("a ⟨01:00 · 12:06⟩"), vec![60, 726]);
    }
}
