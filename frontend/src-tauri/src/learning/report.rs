//! The report on learning that a user may choose to send to the developers,
//! to compare how fine-tuning works in different organisations.
//!
//! Numbers only. Settings shows every line before anything leaves, and any
//! line can be taken out. Nothing is sent from here: the user opens a
//! prefilled GitHub issue or saves a file. Never in it: audio, texts of
//! transcripts and documents, single words, the dictionary and rules, names,
//! titles and dates of meetings, the model itself.
//!
//! What was sent is kept on this Mac as it left ([`keep_sent`]), so the
//! user can always see what the developers have.

use std::path::Path;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use super::measure::Detail;
use super::versions::{Outcome, Run};

/// One line of the report
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Line {
    /// Stable name of the line, for the user's choice of what to send
    pub key: &'static str,
    pub label: String,
    pub value: String,
}

/// What the report is made of
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub app_version: String,
    /// «Mac mini (Apple M4), 24 ГБ»
    pub mac: String,
    pub checked_hours: f64,
    /// Runs, newest first
    pub history: Vec<Run>,
    /// Words fixed by learned rules by themselves, and rules turned into hints after an undo
    pub auto_fixes: Option<(usize, usize)>,
    /// Corrections by kind over the newest checked meetings: (meetings, words, kinds)
    pub edits: Option<(usize, usize, super::corrected::Edits)>,
    /// Meet's captions against the corrections, over the newest checked meetings that have them
    pub captions: Option<super::captions_check::CaptionsCheck>,
}

/// A number the Ukrainian way: decimal comma
fn num(x: f64, digits: usize) -> String {
    format!("{x:.digits$}").replace('.', ",")
}

/// «1 запис», «3 записи», «5 записів»
fn recordings(n: usize) -> String {
    let word = match (n % 10, n % 100) {
        (1, r) if r != 11 => "запис",
        (2..=4, r) if !(12..=14).contains(&r) => "записи",
        _ => "записів",
    };
    format!("{n} {word}")
}

/// Change from `before` to `after` in percent, with a real minus sign
fn change(before: usize, after: usize) -> String {
    let p = (100.0 * (after as f64 - before as f64) / before as f64).round() as i64;
    match p {
        0 => "0%".into(),
        p if p > 0 => format!("+{p}%"),
        p => format!("−{}%", -p),
    }
}

fn kinds(before: &Detail, after: &Detail) -> String {
    [("числа", before.numbers, after.numbers), ("словник", before.dictionary, after.dictionary), ("закінчення", before.endings, after.endings), ("інше", before.other, after.other)]
        .iter()
        .filter(|(_, b, _)| *b > 0)
        .map(|(name, b, a)| format!("{name} {}", change(*b, *a)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn underlines(d: &Detail) -> Option<String> {
    if d.seconds <= 0.0 || d.underlined == 0 {
        return None;
    }
    Some(format!(
        "{:.0} на годину, з них помилки {:.0}%, позначають {:.0}% помилок",
        d.underlined as f64 / (d.seconds / 3600.0),
        100.0 * d.underlined_wrong as f64 / d.underlined as f64,
        100.0 * d.caught as f64 / d.mistakes.max(1) as f64,
    ))
}

fn runs(history: &[Run]) -> String {
    let count = |o: Outcome| history.iter().filter(|r| r.outcome == o).count();
    let parts: Vec<String> = [
        ("прийнято", count(Outcome::Accepted)),
        ("не точніше", count(Outcome::Rejected)),
        ("зупинено", count(Outcome::Stopped)),
        ("не завершено", count(Outcome::Failed)),
    ]
    .iter()
    .filter(|(_, n)| *n > 0)
    .map(|(name, n)| format!("{name} {n}"))
    .collect();
    format!("{}: {}", history.len(), parts.join(", "))
}

pub fn lines(f: &Facts) -> Vec<Line> {
    let mut out = Vec::new();
    let mut add = |key: &'static str, label: &str, value: String| out.push(Line { key, label: label.into(), value });
    add("version", "Версія програми", f.app_version.clone());
    if !f.mac.is_empty() {
        add("mac", "Mac", f.mac.clone());
    }
    add("checked", "Перевірене мовлення", format!("{} год", num(f.checked_hours, 1)));
    if !f.history.is_empty() {
        add("runs", "Запуски навчання", runs(&f.history));
    }
    // The newest run that got as far as the check
    if let Some(r) = f.history.iter().find(|r| r.wer_before.is_some() && r.wer_after.is_some()) {
        let accepted = r.outcome == Outcome::Accepted;
        add("before", "Помилок до донавчання", format!("{}%", num(r.wer_before.unwrap_or(0.0), 1)));
        add(
            "after",
            "Помилок після донавчання",
            format!("{}%{}", num(r.wer_after.unwrap_or(0.0), 1), if accepted { "" } else { " (модель не прийнято)" }),
        );
        if let (Some(b), Some(a)) = (&r.before, &r.after) {
            let k = kinds(b, a);
            if !k.is_empty() {
                add("kinds", "Зміна за видами помилок", k);
            }
        }
        let mut training = format!("{} хв", num(r.minutes, 0));
        if let Some(m) = r.peak_memory_gb {
            training.push_str(&format!(", пам'ять до {} ГБ", num(m, 0)));
        }
        add("training", "Навчання", training);
        // How the underlines do with the model in use after the run
        if let Some(u) = (if accepted { r.after.as_ref() } else { r.before.as_ref() }).and_then(underlines) {
            add("underlines", "Підкреслення", u);
        }
    }
    if let Some((meetings, words, e)) = f.edits.filter(|(_, words, _)| *words > 0) {
        let share = |n: usize| 100.0 * n as f64 / words as f64;
        let w = e.words();
        let mut parts: Vec<(&str, usize)> = vec![
            ("назви", e.names),
            ("числа", e.numbers),
            ("закінчення", e.endings),
            ("інше слово", e.replaced),
            ("дописано", e.added),
            ("паразити", e.fillers),
            ("прибрано", e.removed),
        ];
        parts.retain(|(_, n)| *n > 0);
        parts.sort_by(|a, b| b.1.cmp(&a.1));
        let mut value = format!("{}% слів", num(share(w), 1));
        if !parts.is_empty() {
            value.push_str(": ");
            value.push_str(&parts.iter().map(|(k, n)| format!("{k} {}%", num(share(*n), 1))).collect::<Vec<_>>().join(", "));
        }
        value.push_str(&format!("; іншому мовцю {} рядків, лише розділові знаки {}", e.speaker_lines, e.punctuation_lines));
        add("edits", &format!("Правки за видами, останні {meetings} нарад"), value);
        if e.names > 0 {
            add("names", "Правки назв зі словника", format!("{}: з малої літери {}, повтори в тій самій нараді {}", e.names, e.names_lower, e.names_repeats));
        }
        if w > 0 {
            add("repeats", "Повторні правки в тій самій нараді", format!("{} з {} виправлених слів", e.repeats, w));
        }
    }
    if let Some(c) = f.captions.filter(|c| c.any.covered > 0) {
        let pct = |a: usize, b: usize| num(100.0 * a as f64 / b.max(1) as f64, 0);
        // Flagged words, how many of them were corrected, how many corrections they catch
        // per hour of checked speech, as the underlines are counted
        let hours = c.seconds as f64 / 3600.0;
        let per_hour = |n: usize| if hours > 0.0 { format!(" ({} на годину)", (n as f64 / hours).round()) } else { String::new() };
        let how = |x: &super::captions_check::Flags| {
            format!(
                "{}% слів{}, з них виправлено {}%, позначили б {}% виправлених слів",
                pct(x.flagged, x.covered),
                per_hour(x.flagged),
                pct(x.corrected_flagged, x.flagged),
                pct(x.corrected_flagged, x.corrected)
            )
        };
        add(
            "captions",
            &format!("Субтитри Meet, {}", recordings(c.meetings)),
            format!("є для {}% слів; розбіжності у {}; до уточнення розбіжності у {}% слів", pct(c.any.covered, c.words), how(&c.any), pct(c.before.flagged, c.before.covered)),
        );
        add("captions_filters", "Субтитри Meet, суворіші позначки", format!("лише інше слово: {}; лише від двох слів підряд: {}", how(&c.other), how(&c.runs)));
        add(
            "captions_records",
            "Субтитри Meet, записи",
            format!(
                "українською {}, іншою мовою {}, мова невідома {}; зсунуто більш як на 3 с {}, пауза в записі {}, не вдалося вирівняти {}; розбіжності в більшості слів {}",
                c.lang_uk, c.lang_other, c.lang_unknown, c.shifted, c.jumped, c.unfitted, c.mostly_different
            ),
        );
    }
    if let Some((applied, undone)) = f.auto_fixes {
        add("auto_fixes", "Автовиправлення", format!("спрацювало {applied}, скасовано {undone}"));
    }
    out
}

/// A copy of a report as it left, kept on this Mac
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sent {
    /// When, local time: `2026-11-22 09:41`
    pub at: String,
    /// `issue` (a prefilled GitHub issue) or `file` (saved to send by hand)
    pub how: String,
    /// The lines the user left in: label and value
    pub lines: Vec<(String, String)>,
}

/// The folder of copies, under the app's `learning` folder
const SENT_DIR: &str = "sent";
/// More than this is not a report of ours
const MAX_LINES: usize = 60;
const MAX_TEXT: usize = 300;

/// Keeps a copy of a report that was sent (or saved to be sent)
pub fn keep_sent(learning: &Path, now: chrono::DateTime<chrono::Local>, how: &str, lines: Vec<(String, String)>) -> Result<Sent> {
    if how != "issue" && how != "file" {
        return Err(anyhow!("unknown way of sending: {how}"));
    }
    if lines.is_empty() || lines.len() > MAX_LINES || lines.iter().any(|(l, v)| l.chars().count() > MAX_TEXT || v.chars().count() > MAX_TEXT) {
        return Err(anyhow!("not a report"));
    }
    let dir = learning.join(SENT_DIR);
    std::fs::create_dir_all(&dir)?;
    let sent = Sent { at: now.format("%Y-%m-%d %H:%M").to_string(), how: how.to_string(), lines };
    let stem = now.format("%Y-%m-%dT%H-%M-%S").to_string();
    // Numbered within the second, so the names sort by time
    let mut n = 1;
    let mut path = dir.join(format!("{stem}-{n:02}.json"));
    while path.exists() {
        n += 1;
        path = dir.join(format!("{stem}-{n:02}.json"));
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&sent)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(sent)
}

/// Every report sent, newest first
pub fn sent(learning: &Path) -> Vec<Sent> {
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(learning.join(SENT_DIR))
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
        .unwrap_or_default();
    files.sort();
    files.reverse();
    files.iter().filter_map(|p| serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()).collect()
}

/// The report as plain text, as «Зберегти у файл» writes it
pub fn as_text(lines: &[(String, String)]) -> String {
    let mut s = String::from("Звіт про якість розпізнавання\n\n");
    for (label, value) in lines {
        s.push_str(label);
        s.push_str(": ");
        s.push_str(value);
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checked_run() -> Run {
        let detail = |numbers, dictionary, endings, other, underlined, wrong, caught| Detail {
            seconds: 1800.0,
            words: 4000,
            mistakes: numbers + dictionary + endings + other,
            numbers,
            dictionary,
            endings,
            other,
            underlined,
            underlined_wrong: wrong,
            caught,
        };
        Run {
            id: "2026-11-22T01-30".into(),
            outcome: Outcome::Accepted,
            reason: None,
            train_hours: 3.0,
            held_hours: 0.5,
            train_meetings: 10,
            held_meetings: 2,
            checked_hours: 3.5,
            data_rule: crate::learning::dataset::DATA_RULE,
            wer_before: Some(17.5),
            wer_after: Some(15.8),
            per_meeting: vec![(16.0, 14.0), (19.0, 17.6)],
            minutes: 50.4,
            peak_memory_gb: Some(13.6),
            before: Some(detail(60, 140, 200, 300, 130, 80, 400)),
            after: Some(detail(40, 120, 220, 252, 120, 74, 380)),
            thresholds: None,
        }
    }

    #[test]
    fn the_report_holds_numbers_only() {
        let mut stopped = checked_run();
        stopped.id = "2026-11-23T01-30".into();
        stopped.outcome = Outcome::Stopped;
        stopped.wer_before = None;
        stopped.wer_after = None;
        let f = Facts {
            app_version: "0.4.0".into(),
            mac: "Mac mini (Apple M4), 24 ГБ".into(),
            checked_hours: 3.04,
            history: vec![stopped, checked_run()],
            auto_fixes: Some((41, 3)),
            captions: None,
            edits: Some((
                5,
                2000,
                super::super::corrected::Edits { names: 40, numbers: 20, fillers: 30, endings: 10, speaker_lines: 7, punctuation_lines: 12, names_lower: 9, names_repeats: 6, repeats: 15, ..Default::default() },
            )),
        };
        let l = lines(&f);
        let value = |key: &str| l.iter().find(|x| x.key == key).map(|x| x.value.clone()).unwrap_or_default();
        assert_eq!(value("checked"), "3,0 год");
        assert_eq!(value("runs"), "2: прийнято 1, зупинено 1");
        assert_eq!(value("before"), "17,5%");
        assert_eq!(value("after"), "15,8%");
        assert_eq!(value("kinds"), "числа −33%, словник −14%, закінчення +10%, інше −16%");
        assert_eq!(value("training"), "50 хв, пам'ять до 14 ГБ");
        assert_eq!(value("underlines"), "240 на годину, з них помилки 62%, позначають 60% помилок");
        assert_eq!(value("auto_fixes"), "спрацювало 41, скасовано 3");
        assert_eq!(value("edits"), "5,0% слів: назви 2,0%, паразити 1,5%, числа 1,0%, закінчення 0,5%; іншому мовцю 7 рядків, лише розділові знаки 12");
        assert_eq!(value("names"), "40: з малої літери 9, повтори в тій самій нараді 6");
        assert_eq!(value("repeats"), "15 з 100 виправлених слів");
        assert!(l.iter().all(|x| x.key != "captions"), "no captions, no line");
        use super::super::captions_check::{CaptionsCheck, Flags};
        let f = Facts {
            app_version: "0.4.8".into(),
            captions: Some(CaptionsCheck {
                meetings: 3,
                words: 1000,
                seconds: 1800,
                any: Flags { covered: 900, flagged: 90, corrected: 60, corrected_flagged: 39 },
                other: Flags { covered: 900, flagged: 45, corrected: 60, corrected_flagged: 30 },
                runs: Flags { covered: 900, flagged: 20, corrected: 60, corrected_flagged: 15 },
                before: Flags { covered: 880, flagged: 352, corrected: 60, corrected_flagged: 55 },
                lang_uk: 2,
                lang_unknown: 1,
                shifted: 2,
                jumped: 1,
                ..Default::default()
            }),
            ..Default::default()
        };
        let l = lines(&f);
        let value = |key: &str| l.iter().find(|x| x.key == key).map(|x| (x.label.clone(), x.value.clone())).expect(key);
        assert_eq!(
            value("captions"),
            ("Субтитри Meet, 3 записи".into(), "є для 90% слів; розбіжності у 10% слів (180 на годину), з них виправлено 43%, позначили б 65% виправлених слів; до уточнення розбіжності у 40% слів".into())
        );
        assert_eq!(
            value("captions_filters").1,
            "лише інше слово: 5% слів (90 на годину), з них виправлено 67%, позначили б 50% виправлених слів; лише від двох слів підряд: 2% слів (40 на годину), з них виправлено 75%, позначили б 25% виправлених слів"
        );
        assert_eq!(
            value("captions_records").1,
            "українською 2, іншою мовою 0, мова невідома 1; зсунуто більш як на 3 с 2, пауза в записі 1, не вдалося вирівняти 0; розбіжності в більшості слів 0"
        );
        assert_eq!((recordings(1), recordings(11), recordings(22), recordings(5)), ("1 запис".into(), "11 записів".into(), "22 записи".into(), "5 записів".into()));
        // No dates, ids or per-meeting numbers anywhere
        assert!(!l.iter().any(|x| x.value.contains("2026") || x.value.contains("16,0")));
    }

    #[test]
    fn a_rejected_model_is_said_so_and_without_runs_little_is_shown() {
        let mut r = checked_run();
        r.outcome = Outcome::Rejected;
        let f = Facts { app_version: "0.4.0".into(), history: vec![r], ..Default::default() };
        let l = lines(&f);
        assert!(l.iter().any(|x| x.key == "after" && x.value.ends_with("(модель не прийнято)")));
        let f = Facts { app_version: "0.4.0".into(), checked_hours: 1.0, ..Default::default() };
        assert_eq!(lines(&f).iter().map(|x| x.key).collect::<Vec<_>>(), vec!["version", "checked"]);
    }

    #[test]
    fn what_was_sent_is_kept_newest_first() {
        use chrono::TimeZone;
        let dir = std::env::temp_dir().join(format!("protokol-sent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let at = |h, m| chrono::Local.with_ymd_and_hms(2026, 11, 22, h, m, 0).single().unwrap();
        let line = |l: &str, v: &str| (l.to_string(), v.to_string());
        keep_sent(&dir, at(9, 41), "issue", vec![line("Версія застосунку", "0.4.0")]).unwrap();
        keep_sent(&dir, at(9, 41), "file", vec![line("Перевірено", "3,5 год")]).unwrap();
        keep_sent(&dir, at(12, 5), "file", vec![line("Версія застосунку", "0.4.1")]).unwrap();
        let all = sent(&dir);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].at, "2026-11-22 12:05");
        assert_eq!(all[0].lines, vec![line("Версія застосунку", "0.4.1")]);
        assert_eq!((all[1].how.as_str(), all[2].how.as_str()), ("file", "issue"), "two in the same second both kept, in order");
        // Only our two ways, and only something report-sized
        assert!(keep_sent(&dir, at(13, 0), "mail", vec![line("a", "b")]).is_err());
        assert!(keep_sent(&dir, at(13, 0), "file", vec![]).is_err());
        assert!(keep_sent(&dir, at(13, 0), "file", vec![line("a", &"x".repeat(301))]).is_err());
        assert_eq!(sent(&dir).len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
