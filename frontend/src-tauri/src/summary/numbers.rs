//! Numbers of the document checked against the transcript: the model may
//! change them («4027» → «40.27»). Each number of a bullet is looked for in
//! the bullet's source lines, then anywhere in the transcript; one not found
//! is shown to the secretary, with the likely original when there is one.
//! Nothing is changed on its own.

use std::collections::HashSet;

/// A number in a text: where it is (in chars), how it is written, and the
/// forms it may have been said in
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub start: usize,
    pub end: usize,
    pub text: String,
    /// Its digits alone («40.27» → «4027»)
    pub digits: String,
    /// Normalised forms it matches («4027»; «40.27»; «t8:30»; «7.11»)
    pub keys: Vec<String>,
}

/// A number of the document not found in the transcript
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Issue {
    /// Where in the bullet's text (chars)
    pub start: usize,
    pub end: usize,
    pub text: String,
    /// The likely original from the transcript, when there is one
    pub fix: Option<String>,
    /// The transcript itself has it both ways («40.27» and «4027» nearby)
    pub mixed: bool,
}

fn strip_zeros(d: &str) -> String {
    let t = d.trim_start_matches('0');
    if t.is_empty() { "0".to_string() } else { t.to_string() }
}

const SPACES: [char; 3] = [' ', '\u{00A0}', '\u{202F}'];

/// Numbers written with digits
fn digit_numbers(chars: &[char]) -> Vec<Found> {
    let mut out = Vec::new();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if !chars[i].is_ascii_digit() || (i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_')) {
            i += 1;
            continue;
        }
        let start = i;
        let run = |from: usize| -> usize {
            let mut j = from;
            while j < n && chars[j].is_ascii_digit() {
                j += 1;
            }
            j
        };
        let mut j = run(i);
        let first_len = j - i;
        // Groups of three after a space: «4 027», «1 250 000»
        if first_len <= 3 {
            let mut k = j;
            while k + 3 < n + 1 && k < n && SPACES.contains(&chars[k]) && run(k + 1) - (k + 1) == 3 && (run(k + 1) >= n || !chars[run(k + 1)].is_ascii_digit()) {
                k = run(k + 1);
            }
            if k > j {
                j = k;
            }
        }
        let mut kind = "int";
        // 40.27 / 40,27 / 07.11.2026 / 8:30
        if j + 1 < n && (chars[j] == '.' || chars[j] == ',') && chars[j + 1].is_ascii_digit() {
            let k = run(j + 1);
            kind = if chars[j] == '.' && k + 1 < n && chars[k] == '.' && chars[k + 1].is_ascii_digit() { "date3" } else { "dec" };
            j = if kind == "date3" { run(k + 1) } else { k };
        } else if j + 2 < n && chars[j] == ':' && chars[j + 1].is_ascii_digit() && chars[j + 2].is_ascii_digit() && first_len <= 2 {
            kind = "time";
            j = run(j + 1);
        }
        let text: String = chars[start..j].iter().collect();
        let digits: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
        let mut keys = Vec::new();
        match kind {
            "int" => keys.push(strip_zeros(&digits)),
            "dec" => {
                let t = text.replace(',', ".");
                let (a, b) = t.split_once('.').unwrap_or((&t, ""));
                keys.push(format!("{}.{}", strip_zeros(a), b));
                // A date written «07.11» (day.month) is the same day as «7.11»
                if let (true, Ok(d), Ok(m)) = (text.contains('.'), a.parse::<u32>(), b.parse::<u32>()) {
                    if (1..=31).contains(&d) && (1..=12).contains(&m) {
                        keys.push(format!("date:{d}.{m}"));
                    }
                }
            }
            "date3" => {
                let parts: Vec<&str> = text.split('.').collect();
                if let [d, m, y] = parts[..] {
                    keys.push(format!("date:{}.{}", strip_zeros(d), strip_zeros(m)));
                    keys.push(strip_zeros(y));
                }
            }
            "time" => {
                let (h, m) = text.split_once(':').unwrap_or((&text, "00"));
                keys.push(format!("t{}:{m}", strip_zeros(h)));
            }
            _ => {}
        }
        out.push(Found { start, end: j, text, digits, keys });
        i = j;
    }
    out
}

/// The value of a Ukrainian cardinal word (with its common case forms);
/// `mult` for тисяча / мільйон
fn word_value(w: &str) -> Option<(u64, bool)> {
    let v: u64 = match w {
        "нуль" => 0,
        "один" | "одна" | "одне" | "одного" | "одному" | "одній" | "одним" | "одну" | "одною" => 1,
        "два" | "дві" | "двох" | "двом" | "двома" => 2,
        "три" | "трьох" | "трьом" | "трьома" => 3,
        "чотири" | "чотирьох" | "чотирьом" | "чотирма" => 4,
        "п'ять" | "п'яти" | "п'ятьох" | "п'ятьма" | "пʼять" => 5,
        "шість" | "шести" | "шістьох" | "шістьма" => 6,
        "сім" | "семи" | "сімох" | "сьома" => 7,
        "вісім" | "восьми" | "вісьмох" | "вісьма" => 8,
        "дев'ять" | "дев'яти" | "дев'ятьох" => 9,
        "десять" | "десяти" | "десятьох" => 10,
        "одинадцять" | "одинадцяти" => 11,
        "дванадцять" | "дванадцяти" => 12,
        "тринадцять" | "тринадцяти" => 13,
        "чотирнадцять" | "чотирнадцяти" => 14,
        "п'ятнадцять" | "п'ятнадцяти" => 15,
        "шістнадцять" | "шістнадцяти" => 16,
        "сімнадцять" | "сімнадцяти" => 17,
        "вісімнадцять" | "вісімнадцяти" => 18,
        "дев'ятнадцять" | "дев'ятнадцяти" => 19,
        "двадцять" | "двадцяти" => 20,
        "тридцять" | "тридцяти" => 30,
        "сорок" | "сорока" => 40,
        "п'ятдесят" | "п'ятдесяти" => 50,
        "шістдесят" | "шістдесяти" => 60,
        "сімдесят" | "сімдесяти" => 70,
        "вісімдесят" | "вісімдесяти" => 80,
        "дев'яносто" | "дев'яноста" => 90,
        "сто" | "ста" => 100,
        "двісті" | "двохсот" => 200,
        "триста" | "трьохсот" => 300,
        "чотириста" | "чотирьохсот" => 400,
        "п'ятсот" | "п'ятисот" => 500,
        "шістсот" | "шестисот" => 600,
        "сімсот" | "семисот" => 700,
        "вісімсот" | "восьмисот" => 800,
        "дев'ятсот" | "дев'ятисот" => 900,
        "тисяча" | "тисячі" | "тисяч" | "тисячу" | "тисячею" | "тисячам" => return Some((1000, true)),
        "мільйон" | "мільйони" | "мільйонів" | "мільйона" => return Some((1_000_000, true)),
        _ => return None,
    };
    Some((v, false))
}

/// Numbers said in words («чотири тисячі двадцять сім» → 4027)
fn word_numbers(chars: &[char]) -> Vec<Found> {
    let norm = |c: char| match c {
        '’' | 'ʼ' | '`' | '\u{2019}' => '\'',
        c => c.to_lowercase().next().unwrap_or(c),
    };
    let is_word = |c: char| c.is_alphabetic() || matches!(c, '\'' | '’' | 'ʼ' | '`');
    // Words with their places
    let mut words: Vec<(usize, usize, String)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if is_word(chars[i]) {
            let s = i;
            while i < chars.len() && is_word(chars[i]) {
                i += 1;
            }
            words.push((s, i, chars[s..i].iter().map(|&c| norm(c)).collect()));
        } else {
            i += 1;
        }
    }
    let mut out = Vec::new();
    let mut k = 0;
    while k < words.len() {
        let Some(_) = word_value(&words[k].2) else {
            k += 1;
            continue;
        };
        let (mut total, mut current) = (0u64, 0u64);
        let s = k;
        while k < words.len() {
            let Some((v, mult)) = word_value(&words[k].2) else { break };
            // Words of one number follow each other with only spaces between
            if k > s && !chars[words[k - 1].1..words[k].0].iter().all(|c| c.is_whitespace()) {
                break;
            }
            if mult {
                total += current.max(1) * v;
                current = 0;
            } else {
                current += v;
            }
            k += 1;
        }
        let value = total + current;
        let (start, end) = (words[s].0, words[k - 1].1);
        // «один» alone is mostly not a number («один з них»): two words or more, or a value over 10
        if k - s >= 2 || value > 10 {
            let text: String = chars[start..end].iter().collect();
            out.push(Found { start, end, text, digits: value.to_string(), keys: vec![value.to_string()] });
        }
    }
    out
}

/// Time marks of the document (⟨41:02⟩, [12:30], ⟨?⟩): not numbers that were said
fn marks(chars: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (open, close) in [('⟨', '⟩'), ('[', ']')] {
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == open {
                if let Some(j) = chars[i..].iter().position(|&c| c == close) {
                    let inner = &chars[i + 1..i + j];
                    if inner.iter().all(|c| c.is_ascii_digit() || *c == ':' || *c == '?' || *c == ' ' || *c == '–' || *c == '-') {
                        out.push((i, i + j + 1));
                        i += j + 1;
                        continue;
                    }
                }
            }
            i += 1;
        }
    }
    out
}

/// Every number of a text (digits and words), time marks left out
pub fn extract(text: &str) -> Vec<Found> {
    let chars: Vec<char> = text.chars().collect();
    let skip = marks(&chars);
    let mut all: Vec<Found> = digit_numbers(&chars).into_iter().chain(word_numbers(&chars)).filter(|f| !skip.iter().any(|(a, b)| f.start >= *a && f.end <= *b)).collect();
    all.sort_by_key(|f| f.start);
    all
}

/// What was said: every form of every number in a text
pub struct Said {
    keys: HashSet<String>,
    /// Digits, how they were written, and what they mean
    forms: Vec<(String, String, Vec<String>)>,
}

impl Said {
    pub fn of(text: &str) -> Self {
        let found = extract(text);
        let keys = found.iter().flat_map(|f| f.keys.iter().cloned()).collect();
        let forms = found.into_iter().map(|f| (f.digits, f.text, f.keys)).collect();
        Self { keys, forms }
    }

    fn has(&self, f: &Found) -> bool {
        f.keys.iter().any(|k| self.keys.contains(k))
    }

    /// The one form with exactly the same digits, written otherwise
    fn same_digits(&self, f: &Found) -> Option<String> {
        let mut seen: Vec<&String> = Vec::new();
        for (digits, text, keys) in &self.forms {
            // Another value with the same digits («4027» and «40.27»), not the same value written otherwise («4 027»)
            let same_value = keys.iter().any(|k| f.keys.contains(k));
            if *digits == f.digits && !same_value && text != &f.text && !seen.contains(&text) {
                seen.push(text);
            }
        }
        if seen.len() == 1 { Some(seen[0].clone()) } else { None }
    }

    /// The one likely original of a number not found: the same digits written
    /// otherwise, or (3 digits or more) one digit added, lost or changed
    fn original_of(&self, f: &Found) -> Option<String> {
        let mut seen: Vec<String> = Vec::new();
        for (digits, text, _) in &self.forms {
            let close = *digits == f.digits || (f.digits.len() >= 3 && digits.len() >= 3 && edit_distance(digits, &f.digits) <= 1);
            if close && text != &f.text && !seen.contains(text) {
                seen.push(text.clone());
            }
        }
        if seen.len() == 1 { seen.pop() } else { None }
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The numbers of one bullet not found in its source lines nor anywhere in
/// the transcript; a fix is offered from the source lines first
pub fn check(bullet: &str, source: &Said, transcript: &Said) -> Vec<Issue> {
    extract(bullet)
        .into_iter()
        .filter_map(|f| {
            // The same digits written otherwise in its own source («4027» for
            // «40.27»): changed, or the transcript itself has it both ways
            if let Some(same) = source.same_digits(&f) {
                let mixed = source.has(&f);
                return Some(Issue { start: f.start, end: f.end, text: f.text, fix: Some(same), mixed });
            }
            if source.has(&f) || transcript.has(&f) {
                return None;
            }
            let fix = source.original_of(&f).or_else(|| transcript.original_of(&f));
            Some(Issue { start: f.start, end: f.end, text: f.text, fix, mixed: false })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Vec<Vec<String>> {
        extract(text).into_iter().map(|f| f.keys).collect()
    }

    /// The check on a COPY of a meeting's document: PROTOKOL_NUMBERS_DB, PROTOKOL_NUMBERS_MEETING.
    /// Prints counts by kind only (no content).
    #[tokio::test]
    #[ignore = "needs a database copy"]
    async fn numbers_of_a_document_copy() {
        let db = std::env::var("PROTOKOL_NUMBERS_DB").unwrap();
        assert!(db.contains(&crate::test_copy_root()), "only a copy");
        let meeting = std::env::var("PROTOKOL_NUMBERS_MEETING").unwrap();
        let manager = crate::database::manager::DatabaseManager::new(&db, "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        let result: String = sqlx::query_scalar("SELECT result FROM summary_processes WHERE meeting_id = ?").bind(&meeting).fetch_one(&pool).await.unwrap();
        let markdown = serde_json::from_str::<serde_json::Value>(&result).unwrap()["markdown"].as_str().unwrap().to_string();
        let rows: Vec<(String, Option<f64>, Option<String>)> =
            sqlx::query_as("SELECT transcript, audio_start_time, speaker FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time").bind(&meeting).fetch_all(&pool).await.unwrap();
        // Bullets as the document view sees them, with their moments
        let mut bullets = Vec::new();
        let mut block_time: Option<f64> = None;
        let mmss = |s: &str, open: char, close: char| -> Option<f64> {
            let a = s.find(open)?;
            let b = s[a..].find(close)? + a;
            let (m, sec) = s[a + open.len_utf8()..b].split_once(':')?;
            Some(m.trim().parse::<f64>().ok()? * 60.0 + sec.trim().parse::<f64>().ok()?)
        };
        for line in markdown.lines() {
            if line.contains("ВИСТУПИЛИ") {
                block_time = mmss(line, '[', ']');
            }
            let t = line.trim_start();
            if let Some(text) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
                bullets.push(Bullet { text: text.to_string(), seconds: mmss(text, '⟨', '⟩').or(block_time) });
            }
        }
        let all = Said::of(&rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>().join("\n"));
        let kind = |f: &Found| -> &'static str {
            if f.text.chars().any(|c| c.is_alphabetic()) {
                "words"
            } else if f.text.contains(':') {
                "time"
            } else if f.keys.iter().any(|k| k.starts_with("date:")) {
                "date"
            } else if f.text.contains('.') || f.text.contains(',') {
                "decimal"
            } else if f.text.chars().any(|c| SPACES.contains(&c)) {
                "spaced"
            } else {
                "integer"
            }
        };
        let mut stats: std::collections::BTreeMap<&str, [usize; 6]> = Default::default(); // checked, in source, elsewhere, flagged, fix, mixed
        let issues = check_bullets(&rows, &bullets);
        for (b, iss) in bullets.iter().zip(&issues) {
            let source = Said::of(&source_text(&rows, b.seconds));
            for f in extract(&b.text) {
                let s = stats.entry(kind(&f)).or_default();
                s[0] += 1;
                let issue = iss.iter().find(|i| i.start == f.start);
                if let Some(i) = issue {
                    s[3] += 1;
                    if i.fix.is_some() {
                        s[4] += 1;
                    }
                    if i.mixed {
                        s[5] += 1;
                    }
                } else if source.has(&f) {
                    s[1] += 1;
                } else {
                    s[2] += 1;
                }
            }
        }
        println!("bullets {}, with a moment {}", bullets.len(), bullets.iter().filter(|b| b.seconds.is_some()).count());
        println!("kind      checked  in-source  elsewhere  flagged  fix-offered  both-ways-in-transcript");
        for (k, s) in &stats {
            println!("{k:<9} {:>7} {:>10} {:>10} {:>8} {:>12} {:>12}", s[0], s[1], s[2], s[3], s[4], s[5]);
        }
    }

    #[test]
    fn numbers_are_found_in_all_their_forms() {
        assert_eq!(keys("переведено 4027 осіб"), vec![vec!["4027".to_string()]]);
        assert_eq!(keys("переведено 4 027 осіб"), vec![vec!["4027".to_string()]]);
        assert_eq!(keys("ріст на 12,5%"), vec![vec!["12.5".to_string()]]);
        assert_eq!(keys("о 8:30"), vec![vec!["t8:30".to_string()]]);
        assert_eq!(keys("до 07.11.2026"), vec![vec!["date:7.11".to_string(), "2026".to_string()]]);
        assert_eq!(keys("10 підарів на 7 олієнтирах"), vec![vec!["10".to_string()], vec!["7".to_string()]]);
    }

    #[test]
    fn ukrainian_number_words_have_their_values() {
        assert_eq!(keys("переведено чотири тисячі двадцять сім осіб"), vec![vec!["4027".to_string()]]);
        assert_eq!(keys("двісті п'ятдесят метрів"), vec![vec!["250".to_string()]]);
        assert_eq!(keys("тисяча двісті"), vec![vec!["1200".to_string()]]);
        assert_eq!(keys("двадцять чотири метри"), vec![vec!["24".to_string()]]);
        // «один з них» is not a number
        assert!(keys("один з них прийшов").is_empty());
    }

    #[test]
    fn time_marks_are_not_numbers_that_were_said() {
        assert!(keys("Звернутися до Масика ⟨02:18⟩").is_empty());
        assert!(keys("ВИСТУПИЛИ: Доповідач [03:56]").is_empty());
        assert!(keys("пункт ⟨?⟩").is_empty());
    }

    #[test]
    fn a_changed_number_is_shown_with_its_original() {
        let source = Said::of("стосовно підйомних для контрактників, які перевелися з 4027");
        let transcript = Said::of("на початку 101 доставка");
        let issues = check("Уточнити деталі для контрактників з 40.27", &source, &transcript);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].text, "40.27");
        assert_eq!(issues[0].fix.as_deref(), Some("4027"));
        // A number that was said is fine, in words or in digits
        let said = Said::of("перевелися чотири тисячі двадцять сім");
        assert!(check("з 4027", &said, &transcript).is_empty());
        assert!(check("з 4 027", &said, &transcript).is_empty());
        // «40.27» said elsewhere does not hide the change of this bullet's «4027»
        let elsewhere = Said::of("з 4027\nділянка 40.27");
        let issues = check("контрактників з 40.27", &source, &elsewhere);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].fix.as_deref(), Some("4027"));
        assert!(!issues[0].mixed);
        // The transcript has it both ways near the point: shown as such
        let both = Said::of("перевелися з 4027\nз 40.27");
        let issues = check("контрактників з 40.27", &both, &both);
        assert_eq!((issues.len(), issues[0].fix.as_deref(), issues[0].mixed), (1, Some("4027"), true));
    }

    #[test]
    fn a_number_said_elsewhere_is_fine_and_one_never_said_has_no_guess() {
        let source = Said::of("зафіксовано 101 доставку");
        let transcript = Said::of("вітер 24 метри за секунду; 10 підарів");
        assert!(check("вітер 24 м/с", &source, &transcript).is_empty());
        let issues = check("знищено 55 цілей", &source, &transcript);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].fix, None);
        // One digit off (three or more digits): offered; two different candidates: none
        assert_eq!(check("доставок 102", &source, &transcript)[0].fix.as_deref(), Some("101"));
        let two = Said::of("101 і 103");
        assert_eq!(check("доставок 102", &two, &Said::of(""))[0].fix, None);
    }
}

/// A bullet of the document as shown: its text and the moment it points to
/// (its own ⟨mm:ss⟩ mark, or that of its «ВИСТУПИЛИ» block)
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Bullet {
    pub text: String,
    pub seconds: Option<f64>,
}

/// The numbers of the document's bullets not found in the transcript (one
/// list per bullet, in order)
#[tauri::command]
pub async fn api_check_document_numbers<R: tauri::Runtime>(app: tauri::AppHandle<R>, meeting_id: String, bullets: Vec<Bullet>) -> Result<Vec<Vec<Issue>>, String> {
    let pool = crate::speakers::commands::pool(&app).map_err(|e| e.to_string())?;
    let rows: Vec<(String, Option<f64>, Option<String>)> =
        sqlx::query_as("SELECT transcript, audio_start_time, speaker FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time").bind(&meeting_id).fetch_all(&pool).await.map_err(|e| e.to_string())?;
    Ok(check_bullets(&rows, &bullets))
}

/// The lines a bullet comes from: those heard from its moment on for a
/// minute (and the one just before: the document's marks may be a line early)
pub fn source_text(rows: &[(String, Option<f64>, Option<String>)], seconds: Option<f64>) -> String {
    let Some(t) = seconds else { return String::new() };
    let first = rows.iter().rposition(|r| r.1.unwrap_or(0.0) <= t + 0.5).unwrap_or(0);
    rows.iter()
        .enumerate()
        .filter(|(i, r)| *i >= first && r.1.unwrap_or(0.0) <= t + SOURCE_WINDOW_S)
        .map(|(_, r)| r.0.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// How far after its moment a bullet's source may go
const SOURCE_WINDOW_S: f64 = 60.0;

/// [`api_check_document_numbers`] on the lines (text, start, speaker)
pub fn check_bullets(rows: &[(String, Option<f64>, Option<String>)], bullets: &[Bullet]) -> Vec<Vec<Issue>> {
    let all = Said::of(&rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>().join("\n"));
    bullets
        .iter()
        .map(|b| {
            // The source lines: the line at that moment and the same person's next ones
            let source = Said::of(&source_text(rows, b.seconds));
            check(&b.text, &source, &all)
        })
        .collect()
}
