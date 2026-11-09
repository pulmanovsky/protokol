//! Lines of the transcript as the secretary works on them: the time of each
//! word (to hear a line from a word and follow it while playing), a line cut
//! in two at a word (another person spoke the start) or two lines joined,
//! and lines marked «перевірено». Cuts, joins and checks are kept by time,
//! so a new speaker recognition (which writes all lines again) repeats them.

use anyhow::{anyhow, Result};
use serde::Serialize;
use sqlx::SqlitePool;

/// One word of a line with its moment in the recording
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WordTime {
    pub word: String,
    /// When it sounds in the recording
    pub start: f64,
    pub end: f64,
    /// How sure recognition was (1 when unknown)
    pub prob: f32,
    /// Its moment on the lines' own timeline (where the line's stored times are)
    #[serde(skip)]
    pub at: f64,
}

/// The words of a line with times: from the word times of recognition when
/// they cover the line word for word, else spread over the line by length
/// (`exact` says which)
#[derive(Debug, Clone, Serialize)]
pub struct LineWords {
    pub id: String,
    pub exact: bool,
    pub words: Vec<WordTime>,
}

/// Times of the words of a line [start, end) from the recognised pieces
pub(crate) fn words_of_line(pieces: &[super::words::PieceWords], text: &str, start: f64, end: f64) -> (bool, Vec<WordTime>) {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return (false, Vec::new());
    }
    let norm = |w: &str| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    // Recognised words heard around the line (with some slack: a cut or an
    // edit may have moved its edges a little)
    let mut heard: Vec<(String, f64, f32)> = Vec::new();
    for p in pieces {
        // Found by the line's own timeline (words aligned to the sound later
        // may be seconds away from it)
        let times: Vec<f64> = if p.heard.is_empty() { retimed(p) } else { p.words.iter().map(|w| p.line_time(w.1)).collect() };
        for (i, (w, _)) in p.words.iter().enumerate() {
            let t = times[i];
            if t >= start - 0.6 && t < end + 0.3 {
                heard.push((norm(w), t, p.probs.get(i).copied().unwrap_or(1.0)));
            }
        }
    }
    heard.sort_by(|a, b| a.1.total_cmp(&b.1));
    // The line's words matched to the heard ones in order (longest common
    // run): the same word, or one corrected to something that sounds alike
    let line: Vec<String> = words.iter().map(|w| norm(w)).collect();
    let alike = |a: &str, b: &str| a == b || (!a.is_empty() && !b.is_empty() && crate::ukrainian::doubt::sound_similarity(a, b) >= 0.7);
    let (n, m) = (line.len(), heard.len());
    let mut dp = vec![vec![0u16; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if alike(&line[i], &heard[j].0) { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
        }
    }
    let mut matched: Vec<Option<(f64, f32)>> = vec![None; n];
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if alike(&line[i], &heard[j].0) && dp[i][j] == dp[i + 1][j + 1] + 1 {
            matched[i] = Some((heard[j].1.clamp(start - 0.6, end), heard[j].2));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let found = matched.iter().filter(|x| x.is_some()).count();
    // Words not heard as such (typed in by hand): between their neighbours, by length
    let mut starts: Vec<(f64, f32)> = vec![(start, 1.0); n];
    let mut k = 0;
    while k < n {
        if let Some(x) = matched[k] {
            starts[k] = x;
            k += 1;
            continue;
        }
        let gap_end = (k..n).find(|&q| matched[q].is_some()).unwrap_or(n);
        let from = if k == 0 { start } else { starts[k - 1].0 + 0.05 };
        let to = if gap_end < n { matched[gap_end].unwrap().0 } else { end };
        let lens: Vec<usize> = (k..gap_end).map(|q| words[q].chars().count() + 1).collect();
        let total: usize = lens.iter().sum::<usize>().max(1);
        let mut at = 0usize;
        for (q, len) in (k..gap_end).zip(lens) {
            starts[q] = (from + (to - from).max(0.0) * at as f64 / total as f64, 1.0);
            at += len;
        }
        k = gap_end;
    }
    // …and given where they sound
    let sound = super::words::to_sound(pieces);
    let in_sound = |t: f64| sound.as_deref().map(|m| super::words::interpolate(m, t)).unwrap_or(t);
    let out = words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let end_at = starts.get(i + 1).map(|s| s.0).unwrap_or(end).max(starts[i].0);
            WordTime { word: w.to_string(), start: in_sound(starts[i].0), end: in_sound(end_at).max(in_sound(starts[i].0)), prob: starts[i].1, at: starts[i].0 }
        })
        .collect();
    (found == n, out)
}

/// Syllables of a word (its vowels), at least one
fn syllables(word: &str) -> f64 {
    word.chars().filter(|c| "аеєиіїоуюяaeiouyАЕЄИІЇОУЮЯAEIOUY".contains(*c)).count().max(1) as f64
}

/// Whisper's word times inside a piece, made usable for following the
/// sound: words it gives (almost) the same moment are spread by their
/// syllables up to the next word it placed apart, and words it pushes to
/// the very end of the piece are placed at the speaker's own pace
pub(crate) fn retimed(p: &super::words::PieceWords) -> Vec<f64> {
    let n = p.words.len();
    let mut t: Vec<f64> = p.words.iter().map(|w| w.1).collect();
    // Aligned to the sound already: as they are
    if n < 2 || p.aligned {
        return t;
    }
    let syl: Vec<f64> = p.words.iter().map(|w| syllables(&w.0)).collect();
    // Words an earlier correction pushed outside the piece (older versions
    // spread a corrected line evenly over its whole span): put back inside
    if let Some(first_out) = (0..n).find(|&i| t[i] > p.end + 0.3 || t[i] < p.start - 0.3) {
        let from = if first_out == 0 { p.start } else { t[first_out - 1].clamp(p.start, p.end) };
        let total: f64 = syl[first_out..].iter().sum::<f64>().max(1.0);
        let mut acc = 0.0;
        for i in first_out..n {
            t[i] = from + (p.end - from).max(0.0) * acc / total;
            acc += syl[i];
        }
    }
    // The pace where Whisper's times look sound: seconds per syllable
    let mut pace = Vec::new();
    for i in 0..n - 1 {
        let gap = t[i + 1] - t[i];
        if gap > 0.08 && gap < 1.5 {
            pace.push(gap / syl[i]);
        }
    }
    pace.sort_by(|a, b| a.total_cmp(b));
    let per = pace.get(pace.len() / 2).copied().unwrap_or(0.18).clamp(0.08, 0.4);
    // The tail pressed against the end of the piece
    let mut tail = n;
    while tail > 1 && p.end - t[tail - 1] < 0.12 {
        tail -= 1;
    }
    if tail < n {
        for i in tail..n {
            t[i] = t[i - 1] + syl[i - 1] * per;
        }
        // …but still inside the piece
        let last = t[n - 1];
        if last > p.end {
            let from = t[tail - 1];
            let k = (p.end - 0.05 - from).max(0.0) / (last - from).max(0.001);
            for v in t.iter_mut().skip(tail) {
                *v = from + (*v - from) * k;
            }
        }
    }
    // Runs of (almost) the same moment, spread to the next distinct one
    let mut i = 0;
    while i < n {
        let mut j = i + 1;
        while j < n && t[j] - t[i] < 0.05 {
            j += 1;
        }
        if j - i > 1 {
            let to = if j < n { t[j] } else { t[i] + syl[i..j].iter().sum::<f64>() * per };
            let total: f64 = syl[i..j].iter().sum();
            let mut acc = 0.0;
            for k in i..j {
                t[k] = t[i] + (to - t[i]).max(0.0) * acc / total;
                acc += syl[k];
            }
        }
        i = j;
    }
    t
}

type Row = (String, String, f64, f64, Option<String>);

async fn rows_of(pool: &SqlitePool, meeting_id: &str) -> Result<Vec<Row>> {
    let rows: Vec<(String, String, Option<f64>, Option<f64>, Option<String>)> =
        sqlx::query_as("SELECT id, transcript, audio_start_time, audio_end_time, speaker FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time")
            .bind(meeting_id)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|(id, t, a, b, s)| (id, t, a.unwrap_or(0.0), b.or(a).unwrap_or(0.0), s)).collect())
}

async fn folder_of(pool: &SqlitePool, meeting_id: &str) -> Option<std::path::PathBuf> {
    sqlx::query_scalar::<_, Option<String>>("SELECT folder_path FROM meetings WHERE id = ?")
        .bind(meeting_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten()
        .map(std::path::PathBuf::from)
}

/// The words of every line of a meeting with their times
pub async fn line_words(pool: &SqlitePool, meeting_id: &str) -> Result<Vec<LineWords>> {
    let pieces = folder_of(pool, meeting_id).await.and_then(|f| super::words::load(&f)).unwrap_or_default();
    Ok(rows_of(pool, meeting_id)
        .await?
        .into_iter()
        .map(|(id, text, a, b, _)| {
            let (exact, words) = words_of_line(&pieces, &text, a, b);
            LineWords { id, exact, words }
        })
        .collect())
}

/// Cuts a line before word `at` (1..words): the start stays in the line,
/// the rest becomes a new line with the same speaker. Returns the new id.
pub async fn split(pool: &SqlitePool, meeting_id: &str, segment_id: &str, at: usize) -> Result<String> {
    let rows = rows_of(pool, meeting_id).await?;
    let (id, text, a, b, _) = rows.iter().find(|r| r.0 == segment_id).cloned().ok_or_else(|| anyhow!("Рядок не знайдено"))?;
    let pieces = folder_of(pool, meeting_id).await.and_then(|f| super::words::load(&f)).unwrap_or_default();
    let (_, words) = words_of_line(&pieces, &text, a, b);
    if at == 0 || at >= words.len() {
        return Err(anyhow!("Розрізати можна лише між словами"));
    }
    let t = words[at].at;
    let new_id = split_row(pool, &id, &words, at, t, b).await?;
    record(pool, meeting_id, "split", t).await?;
    Ok(new_id)
}

async fn split_row(pool: &SqlitePool, id: &str, words: &[WordTime], at: usize, t: f64, end: f64) -> Result<String> {
    let first: String = words[..at].iter().map(|w| w.word.as_str()).collect::<Vec<_>>().join(" ");
    let rest: String = words[at..].iter().map(|w| w.word.as_str()).collect::<Vec<_>>().join(" ");
    let new_id = format!("transcript-{}", uuid::Uuid::new_v4());
    let mut tx = pool.begin().await?;
    // The copy keeps everything of the line (speaker, «змінено вручну»…)
    sqlx::query(
        "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, summary, action_items, key_points, audio_start_time, audio_end_time, duration, speaker, speaker_manual, reviewed_at)
         SELECT ?, meeting_id, ?, timestamp, summary, action_items, key_points, ?, ?, ?, speaker, speaker_manual, reviewed_at FROM transcripts WHERE id = ?",
    )
    .bind(&new_id)
    .bind(&rest)
    .bind(t)
    .bind(end)
    .bind(end - t)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    // A corrected line's recognised text no longer fits a half: the
    // corrections stay, «повернути розпізнане» goes
    sqlx::query("UPDATE transcripts SET transcript = ?, audio_end_time = ?, duration = ? - COALESCE(audio_start_time, 0), original_transcript = NULL WHERE id = ?")
        .bind(&first)
        .bind(t)
        .bind(t)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(new_id)
}

/// Joins a line with the one after it (the first one's speaker stays)
pub async fn merge(pool: &SqlitePool, meeting_id: &str, segment_id: &str) -> Result<()> {
    let rows = rows_of(pool, meeting_id).await?;
    let i = rows.iter().position(|r| r.0 == segment_id).ok_or_else(|| anyhow!("Рядок не знайдено"))?;
    let next = rows.get(i + 1).ok_or_else(|| anyhow!("Це останній рядок"))?;
    merge_rows(pool, &rows[i], next).await?;
    // A cut made here earlier is undone; otherwise the join is remembered
    let removed = sqlx::query("DELETE FROM segment_edits WHERE meeting_id = ? AND kind = 'split' AND ABS(at - ?) < 0.3")
        .bind(meeting_id)
        .bind(next.2)
        .execute(pool)
        .await?
        .rows_affected();
    if removed == 0 {
        record(pool, meeting_id, "merge", next.2).await?;
    }
    Ok(())
}

async fn merge_rows(pool: &SqlitePool, first: &Row, second: &Row) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE transcripts SET transcript = ?, audio_end_time = ?, duration = ? - COALESCE(audio_start_time, 0), original_transcript = NULL,
         reviewed_at = CASE WHEN reviewed_at IS NOT NULL AND (SELECT reviewed_at FROM transcripts WHERE id = ?) IS NOT NULL THEN reviewed_at END WHERE id = ?",
    )
    .bind(format!("{} {}", first.1.trim(), second.1.trim()))
    .bind(second.3)
    .bind(second.3)
    .bind(&second.0)
    .bind(&first.0)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM transcripts WHERE id = ?").bind(&second.0).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

async fn record(pool: &SqlitePool, meeting_id: &str, kind: &str, at: f64) -> Result<()> {
    if kind == "split" {
        // Joining here earlier is undone by the cut
        let removed = sqlx::query("DELETE FROM segment_edits WHERE meeting_id = ? AND kind = 'merge' AND ABS(at - ?) < 0.3")
            .bind(meeting_id)
            .bind(at)
            .execute(pool)
            .await?
            .rows_affected();
        if removed > 0 {
            return Ok(());
        }
    }
    sqlx::query("INSERT INTO segment_edits (meeting_id, kind, at, created_at) VALUES (?, ?, ?, ?)")
        .bind(meeting_id)
        .bind(kind)
        .bind(at)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(pool)
        .await?;
    Ok(())
}

/// After a new speaker recognition: the cuts and joins made by hand again
/// (a cut at the word nearest its moment; a join where a line starts
/// within 0.3 s of it), then the checked lines marked again
pub async fn apply(pool: &SqlitePool, meeting_id: &str) -> Result<(usize, usize)> {
    let edits: Vec<(String, f64)> = sqlx::query_as("SELECT kind, at FROM segment_edits WHERE meeting_id = ? ORDER BY id").bind(meeting_id).fetch_all(pool).await?;
    let pieces = folder_of(pool, meeting_id).await.and_then(|f| super::words::load(&f)).unwrap_or_default();
    let mut done = 0;
    for (kind, at) in &edits {
        let rows = rows_of(pool, meeting_id).await?;
        if kind == "split" {
            let Some(row) = rows.iter().find(|r| r.2 + 0.3 < *at && *at < r.3 - 0.3) else { continue };
            let (_, words) = words_of_line(&pieces, &row.1, row.2, row.3);
            let Some(k) = (1..words.len()).min_by(|&x, &y| (words[x].at - at).abs().total_cmp(&(words[y].at - at).abs())) else { continue };
            split_row(pool, &row.0, &words, k, words[k].at, row.3).await?;
            done += 1;
        } else if let Some(i) = rows.iter().position(|r| (r.2 - at).abs() < 0.3).filter(|&i| i > 0) {
            merge_rows(pool, &rows[i - 1], &rows[i]).await?;
            done += 1;
        }
    }
    let reviewed = mark_reviewed_from_spans(pool, meeting_id).await?;
    Ok((done, reviewed))
}

/// Marks lines «перевірено» (or not): kept by time for a new recognition
pub async fn set_reviewed(pool: &SqlitePool, meeting_id: &str, segment_ids: &[String], reviewed: bool) -> Result<()> {
    let rows = rows_of(pool, meeting_id).await?;
    let now = chrono::Utc::now().to_rfc3339();
    for id in segment_ids {
        let Some(r) = rows.iter().find(|r| &r.0 == id) else { continue };
        sqlx::query("DELETE FROM segment_reviews WHERE meeting_id = ? AND start < ? AND end > ?").bind(meeting_id).bind(r.3.max(r.2 + 0.01)).bind(r.2).execute(pool).await?;
        if reviewed {
            sqlx::query("INSERT INTO segment_reviews (meeting_id, start, end, created_at) VALUES (?, ?, ?, ?)").bind(meeting_id).bind(r.2).bind(r.3).bind(&now).execute(pool).await?;
        }
        sqlx::query("UPDATE transcripts SET reviewed_at = ? WHERE id = ?").bind(reviewed.then_some(&now)).bind(id).execute(pool).await?;
    }
    Ok(())
}

/// Every line not yet checked counts as checked
pub async fn mark_all_reviewed(pool: &SqlitePool, meeting_id: &str) -> Result<()> {
    let open: Vec<String> = sqlx::query_scalar("SELECT id FROM transcripts WHERE meeting_id = ? AND reviewed_at IS NULL").bind(meeting_id).fetch_all(pool).await?;
    set_reviewed(pool, meeting_id, &open, true).await
}

async fn mark_reviewed_from_spans(pool: &SqlitePool, meeting_id: &str) -> Result<usize> {
    let spans: Vec<(f64, f64, String)> = sqlx::query_as("SELECT start, end, created_at FROM segment_reviews WHERE meeting_id = ?").bind(meeting_id).fetch_all(pool).await?;
    if spans.is_empty() {
        return Ok(0);
    }
    let mut n = 0;
    for (id, _, a, b, _) in rows_of(pool, meeting_id).await? {
        let length = (b - a).max(0.01);
        let covered: f64 = spans.iter().map(|(s, e, _)| (b.min(*e) - a.max(*s)).max(0.0)).sum();
        if covered >= length / 2.0 {
            let at = spans.iter().find(|(s, e, _)| *s < b && *e > a).map(|x| x.2.clone());
            sqlx::query("UPDATE transcripts SET reviewed_at = ? WHERE id = ?").bind(at).bind(&id).execute(pool).await?;
            n += 1;
        }
    }
    Ok(n)
}

/// How much of a meeting is checked: lines and share of the speaking time
#[derive(Debug, Serialize, PartialEq)]
pub struct ReviewProgress {
    pub reviewed: usize,
    pub total: usize,
    pub seconds_reviewed: f64,
    pub seconds_total: f64,
}

pub async fn progress(pool: &SqlitePool, meeting_id: &str) -> Result<ReviewProgress> {
    let rows: Vec<(Option<f64>, Option<f64>, Option<String>)> =
        sqlx::query_as("SELECT audio_start_time, audio_end_time, reviewed_at FROM transcripts WHERE meeting_id = ?").bind(meeting_id).fetch_all(pool).await?;
    let mut p = ReviewProgress { reviewed: 0, total: rows.len(), seconds_reviewed: 0.0, seconds_total: 0.0 };
    for (a, b, r) in rows {
        let d = (b.unwrap_or(0.0) - a.unwrap_or(0.0)).max(0.0);
        p.seconds_total += d;
        if r.is_some() {
            p.reviewed += 1;
            p.seconds_reviewed += d;
        }
    }
    Ok(p)
}

/// Words of a line that may be another voice («Так, доброго ранку…» said by
/// someone else at the start of a line)
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct VoiceDoubt {
    pub segment_id: String,
    /// Words `from..to` of the line
    pub from: usize,
    pub to: usize,
    pub seconds: f64,
    /// Whose voice it sounds like
    pub likely: String,
    /// How alike to that voice (0..1)
    pub similarity: f32,
    /// The whole line sounds like that person (not a stretch inside it)
    #[serde(default)]
    pub whole: bool,
    /// How much more like that person than like the one it is given to
    #[serde(default)]
    pub margin: f32,
}

/// Shortest stretch whose voice is compared (shorter ones are not reliable)
const VOICE_CHUNK_MIN_S: f64 = 1.5;
/// How much closer to another voice a stretch must be to be shown
const VOICE_MARGIN: f32 = 0.08;
/// …or this much less like the line's own voice than its other sentences
const VOICE_DROP: f32 = 0.2;

/// Sentences of a line as word ranges, short ones joined to the next
pub(crate) fn sentence_chunks(words: &[WordTime]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut from = 0;
    for (i, w) in words.iter().enumerate() {
        let last = i + 1 == words.len();
        if w.word.ends_with(['.', '!', '?', '…']) || last {
            if words[i].end - words[from].start >= VOICE_CHUNK_MIN_S || last {
                out.push((from, i + 1));
                from = i + 1;
            }
        }
    }
    out
}

/// Stretches inside lines that sound like another voice of the meeting
/// (each sentence of a line compared with the voices heard in it)
/// Places to check who spoke: stretches inside a line that sound like
/// someone else, and whole lines that sound like another named person
pub async fn voice_changes(pool: &SqlitePool, models: &std::path::Path, meeting_id: &str) -> Result<Vec<VoiceDoubt>> {
    let mut found = stretch_changes(pool, models, meeting_id).await?;
    match whole_line_changes(pool, models, meeting_id).await {
        Ok(lines) => {
            // A line suggested whole is not also suggested in parts
            let whole: std::collections::HashSet<String> = lines.iter().map(|l| l.segment_id.clone()).collect();
            found.retain(|d| !whole.contains(&d.segment_id));
            found.extend(lines);
        }
        Err(e) => log::warn!("Whole lines not checked by voice: {e}"),
    }
    Ok(found)
}

/// Lines (3 s or longer) that sound more like another named person of the
/// meeting than like the one they are given to — the mistake fixed by hand
/// most often: a whole turn put on the wrong voice. Measured on 17 fixes made
/// by hand: every whole-turn fix found, with the right person, about 20
/// suggestions in a 37-minute meeting.
pub async fn whole_line_changes(pool: &SqlitePool, models: &std::path::Path, meeting_id: &str) -> Result<Vec<VoiceDoubt>> {
    use super::embedding::cosine;
    use super::profiles::from_blob;
    let model = super::commands::model_id(models);
    // The meeting's named voices, made firmer with what was learned of them before
    let named: Vec<(String, Vec<u8>)> = sqlx::query_as("SELECT name, centroid FROM meeting_speakers WHERE meeting_id = ? AND name IS NOT NULL AND name <> ''").bind(meeting_id).fetch_all(pool).await?;
    let profiles: Vec<(String, Vec<u8>)> = sqlx::query_as("SELECT name, embedding FROM voice_profiles WHERE model = ?").bind(&model).fetch_all(pool).await?;
    let unit = |v: Vec<f32>| {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
        v.into_iter().map(|x| x / n).collect::<Vec<f32>>()
    };
    let mut people: Vec<(String, Vec<f32>)> = Vec::new();
    for (name, c) in named {
        let name = name.trim_end_matches('?').trim().to_string();
        if name.is_empty() || name.starts_with("Спікер") || c.is_empty() || people.iter().any(|p| p.0 == name) {
            continue;
        }
        let mut v = unit(from_blob(&c));
        if let Some((_, p)) = profiles.iter().find(|p| p.0 == name) {
            let p = unit(from_blob(p));
            if p.len() == v.len() {
                v = unit(v.iter().zip(&p).map(|(a, b)| a + b).collect());
            }
        }
        people.push((name, v));
    }
    if people.len() < 2 {
        return Ok(Vec::new());
    }
    let folder = folder_of(pool, meeting_id).await.ok_or_else(|| anyhow!("Для цього запису немає папки зі звуком"))?;
    // Lines the user already decided on are left alone
    let todo: Vec<(String, String, f64, f64, usize)> = sqlx::query_as::<_, (String, String, Option<f64>, Option<f64>, Option<String>)>(
        "SELECT id, transcript, audio_start_time, audio_end_time, speaker FROM transcripts WHERE meeting_id = ? AND speaker_manual = 0 AND reviewed_at IS NULL ORDER BY audio_start_time",
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .filter_map(|(id, text, a, b, sp)| {
        let (a, b) = (a?, b?);
        let sp = sp?.trim_end_matches('?').trim().to_string();
        (b - a >= WHOLE_LINE_MIN_S && people.iter().any(|p| p.0 == sp)).then(|| (id, sp, a, b, text.split_whitespace().count()))
    })
    .collect();
    if todo.is_empty() {
        return Ok(Vec::new());
    }
    // Kept with the meeting like the stretch checks: a line is listened to once
    let key_of = |id: &str, sp: &str, a: f64, b: f64| {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (id, sp, a.to_bits(), b.to_bits()).hash(&mut h);
        for p in &people {
            p.0.hash(&mut h);
            p.1.iter().for_each(|x| x.to_bits().hash(&mut h));
        }
        format!("{:016x}", h.finish())
    };
    let mut kept = load_checks(&folder, WHOLE_LINE_CHECKS);
    let missing: Vec<&(String, String, f64, f64, usize)> = todo.iter().filter(|t| !kept.contains_key(&key_of(&t.0, &t.1, t.2, t.3))).collect();
    if !missing.is_empty() {
        let audio = crate::audio::retranscription::find_audio_file(&folder)?;
        let emb_model = models.join(super::commands::EMBEDDING_MODEL);
        let work: Vec<(String, String, f64, f64, usize)> = missing.into_iter().cloned().collect();
        let people2 = people.clone();
        let answers = tokio::task::spawn_blocking(move || -> Result<Vec<(String, String, f64, f64, Option<VoiceDoubt>)>> {
            super::aligner::lower_priority();
            let pool = rayon::ThreadPoolBuilder::new().num_threads(2).start_handler(|_| super::aligner::lower_priority()).build()?;
            let samples = pool.install(|| super::commands::decode_16k(&audio))?;
            let mut embedder = super::embedding::Embedder::open_light(&emb_model)?;
            let mut out = Vec::new();
            for (id, sp, a, b, n) in work {
                let (i, j) = ((a * 16000.0) as usize, (((a + (b - a).min(20.0)) * 16000.0) as usize).min(samples.len()));
                let found = if j > i {
                    embedder.embed(&samples[i..j])?.and_then(|e| {
                        let own = people2.iter().find(|p| p.0 == sp).map(|p| cosine(&e, &p.1))?;
                        let (who, best) = people2.iter().filter(|p| p.0 != sp).map(|p| (p.0.clone(), cosine(&e, &p.1))).max_by(|x, y| x.1.total_cmp(&y.1))?;
                        (best - own >= WHOLE_LINE_MARGIN).then(|| VoiceDoubt { segment_id: id.clone(), from: 0, to: n, seconds: a, likely: who, similarity: best, whole: true, margin: best - own })
                    })
                } else {
                    None
                };
                out.push((id, sp, a, b, found));
            }
            Ok(out)
        })
        .await??;
        for (id, sp, a, b, found) in answers {
            kept.insert(key_of(&id, &sp, a, b), found.into_iter().collect());
        }
        let current: std::collections::HashMap<String, Vec<VoiceDoubt>> = todo.iter().filter_map(|t| {
            let k = key_of(&t.0, &t.1, t.2, t.3);
            kept.get(&k).map(|v| (k, v.clone()))
        }).collect();
        save_checks(&folder, WHOLE_LINE_CHECKS, &current);
    }
    Ok(todo.iter().flat_map(|t| kept.get(&key_of(&t.0, &t.1, t.2, t.3)).cloned().unwrap_or_default()).collect())
}

/// Whole lines shorter than this are not judged by voice (too little sound)
const WHOLE_LINE_MIN_S: f64 = 3.0;
/// How much more like another person a line must sound
const WHOLE_LINE_MARGIN: f32 = 0.0;
const WHOLE_LINE_CHECKS: &str = "whole_line_checks.json";

async fn stretch_changes(pool: &SqlitePool, models: &std::path::Path, meeting_id: &str) -> Result<Vec<VoiceDoubt>> {
    use super::embedding::cosine;
    use super::profiles::from_blob;
    let voices: Vec<(String, Option<String>, Vec<u8>)> = sqlx::query_as("SELECT label, name, centroid FROM meeting_speakers WHERE meeting_id = ?").bind(meeting_id).fetch_all(pool).await?;
    let voices: Vec<(String, Vec<f32>)> = voices
        .into_iter()
        .filter(|v| !v.2.is_empty())
        .map(|(label, name, c)| (name.filter(|n| !n.trim().is_empty()).unwrap_or(label), from_blob(&c)))
        .collect();
    if voices.len() < 2 {
        return Ok(Vec::new());
    }
    let folder = folder_of(pool, meeting_id).await.ok_or_else(|| anyhow!("Для цього запису немає папки зі звуком"))?;
    let pieces = super::words::load(&folder).unwrap_or_default();
    let mut todo: Vec<(String, usize, usize, f64, f64, usize)> = Vec::new(); // id, from, to, start, end, own voice
    // Lines the user already decided on (speaker set by hand, checked) are left alone
    let settled: std::collections::HashSet<String> =
        sqlx::query_scalar("SELECT id FROM transcripts WHERE meeting_id = ? AND (speaker_manual = 1 OR reviewed_at IS NOT NULL)").bind(meeting_id).fetch_all(pool).await?.into_iter().collect();
    for (id, text, a, b, speaker) in rows_of(pool, meeting_id).await? {
        if settled.contains(&id) {
            continue;
        }
        let Some(own) = speaker.as_deref().and_then(|s| voices.iter().position(|v| v.0 == s.trim_end_matches("(?)").trim())) else { continue };
        let (exact, words) = words_of_line(&pieces, &text, a, b);
        if !exact {
            continue;
        }
        let chunks = sentence_chunks(&words);
        if chunks.len() < 2 {
            continue;
        }
        for (f, t) in chunks {
            todo.push((id.clone(), f, t, words[f].start, words[t - 1].end, own));
        }
    }
    if todo.is_empty() {
        return Ok(Vec::new());
    }
    // Each line's answer is kept with the meeting (by what was checked: the
    // line, its stretches and the voices): a line is listened to again only
    // when it changed (a speaker edit, a correction), never on opening
    let voices_key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for v in &voices {
            v.0.hash(&mut h);
            v.1.iter().for_each(|x| x.to_bits().hash(&mut h));
        }
        h.finish()
    };
    let mut line_keys: Vec<(String, String)> = Vec::new(); // line id, its key
    for (id, ..) in &todo {
        if line_keys.last().map(|l| &l.0) != Some(id) {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            voices_key.hash(&mut h);
            for (i2, f, t, a, b, own) in todo.iter().filter(|x| &x.0 == id) {
                (i2, f, t, a.to_bits(), b.to_bits(), own).hash(&mut h);
            }
            line_keys.push((id.clone(), format!("{:016x}", h.finish())));
        }
    }
    let kept = load_voice_checks(&folder);
    let missing: std::collections::HashSet<String> = line_keys.iter().filter(|(_, k)| !kept.contains_key(k)).map(|(id, _)| id.clone()).collect();
    let mut answers: std::collections::HashMap<String, Vec<VoiceDoubt>> = line_keys.iter().filter_map(|(_, k)| kept.get(k).map(|v| (k.clone(), v.clone()))).collect();
    if missing.is_empty() {
        return Ok(line_keys.iter().flat_map(|(_, k)| answers.get(k).cloned().unwrap_or_default()).collect());
    }
    todo.retain(|x| missing.contains(&x.0));
    let audio = crate::audio::retranscription::find_audio_file(&folder)?;
    let emb_model = models.join(super::commands::EMBEDDING_MODEL);
    let found = tokio::task::spawn_blocking(move || -> Result<Vec<VoiceDoubt>> {
        // In the background: two threads at a low priority
        super::aligner::lower_priority();
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).start_handler(|_| super::aligner::lower_priority()).build()?;
        let samples = pool.install(|| super::commands::decode_16k(&audio))?;
        let mut embedder = super::embedding::Embedder::open_light(&emb_model)?;
        // Each stretch: how close to the line's own voice, and to the nearest other
        let mut scored: Vec<(String, usize, usize, f64, f32, f32, usize, f64)> = Vec::new();
        for (id, f, t, a, b, own) in todo {
            let (i, j) = ((a * 16000.0) as usize, ((b * 16000.0) as usize).min(samples.len()));
            if b - a < VOICE_CHUNK_MIN_S || j <= i {
                continue;
            }
            let Some(e) = embedder.embed(&samples[i..j])? else { continue };
            let own_sim = cosine(&e, &voices[own].1);
            let Some((k, best)) = voices.iter().enumerate().filter(|(k, _)| *k != own).map(|(k, v)| (k, cosine(&e, &v.1))).max_by(|x, y| x.1.total_cmp(&y.1)) else { continue };
            scored.push((id, f, t, a, own_sim, best, k, b - a));
        }
        // Shown: closer to another voice, or much less like the line's own
        // voice than the rest of the line is
        let mut out = Vec::new();
        let rule = *VOICE_RULE.lock().map(|r| r).unwrap_or_else(|e| e.into_inner());
        let mut dump = Vec::new();
        for (n, (id, f, t, a, own_sim, best, k, len)) in scored.iter().enumerate() {
            let rest = scored.iter().enumerate().filter(|(m, x)| *m != n && &x.0 == id).map(|(_, x)| x.4).fold(f32::MIN, f32::max);
            let rest = if rest > f32::MIN { Some(rest) } else { None };
            dump.push(VoiceScore { segment_id: id.clone(), start: *a, end: *a + *len, own: *own_sim, best: *best, rest });
            if voice_shown(rule, *own_sim, *best, rest, *len) {
                out.push(VoiceDoubt { segment_id: id.clone(), from: *f, to: *t, seconds: *a, likely: voices[*k].0.clone(), similarity: *best, whole: false, margin: *best - *own_sim });
            }
        }
        if let Ok(mut d) = LAST_VOICE_SCORES.lock() {
            *d = dump;
        }
        Ok(out)
    })
    .await??;
    // Kept: the answer of every line checked now (none found is an answer too)
    for (id, k) in &line_keys {
        if missing.contains(id) {
            answers.insert(k.clone(), found.iter().filter(|d| &d.segment_id == id).cloned().collect());
        }
    }
    let current: std::collections::HashMap<String, Vec<VoiceDoubt>> = line_keys.iter().filter_map(|(_, k)| answers.get(k).map(|v| (k.clone(), v.clone()))).collect();
    save_voice_checks(&folder, &current);
    Ok(line_keys.iter().flat_map(|(_, k)| current.get(k).cloned().unwrap_or_default()).collect())
}

/// One stretch's scores (for measuring the rule)
#[derive(Debug, Clone)]
pub(crate) struct VoiceScore {
    pub segment_id: String,
    pub start: f64,
    pub end: f64,
    pub own: f32,
    pub best: f32,
    pub rest: Option<f32>,
}

pub(crate) static LAST_VOICE_SCORES: std::sync::Mutex<Vec<VoiceScore>> = std::sync::Mutex::new(Vec::new());
/// Which rule shows a stretch (see [`voice_shown`]); tests try others.
/// Rule 2: measured on a meeting with 17 speaker fixes made by hand, the old
/// rule 1 showed 40 stretches and caught 1 fix; rule 2 shows 9 (none of them
/// a fix either: those were whole turns, not stretches inside a line).
pub(crate) static VOICE_RULE: std::sync::Mutex<u8> = std::sync::Mutex::new(2);

/// Whether a stretch is shown as maybe another voice. Rules:
/// 1 — closer to another voice by a margin, or much less like the line's own
///     voice than the rest of the line;
/// 2 — clearly like another voice (≥0.50) and closer to it by the margin;
/// 3 — as 2 with ≥0.45;
/// 4 — as 2, or the drop rule for stretches of 3 s and more
pub(crate) fn voice_shown(rule: u8, own: f32, best: f32, rest: Option<f32>, len: f64) -> bool {
    let closer = best - own >= VOICE_MARGIN;
    let drop = rest.is_some_and(|r| r - own >= VOICE_DROP);
    match rule {
        2 => best >= 0.50 && closer,
        3 => best >= 0.45 && closer,
        4 => (best >= 0.50 && closer) || (drop && len >= 3.0),
        _ => closer || drop,
    }
}

const VOICE_CHECKS: &str = "voice_checks_v2.json";

fn load_voice_checks(folder: &std::path::Path) -> std::collections::HashMap<String, Vec<VoiceDoubt>> {
    load_checks(folder, VOICE_CHECKS)
}

fn save_voice_checks(folder: &std::path::Path, checks: &std::collections::HashMap<String, Vec<VoiceDoubt>>) {
    save_checks(folder, VOICE_CHECKS, checks)
}

fn load_checks(folder: &std::path::Path, file: &str) -> std::collections::HashMap<String, Vec<VoiceDoubt>> {
    std::fs::read(folder.join(file)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_checks(folder: &std::path::Path, file: &str, checks: &std::collections::HashMap<String, Vec<VoiceDoubt>>) {
    if let Ok(b) = serde_json::to_vec(checks) {
        let tmp = folder.join(format!("{file}.tmp"));
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(&tmp, folder.join(file));
        }
    }
}

/// Looks for words in lines that sound like another person (a few seconds)
#[tauri::command]
pub async fn find_voice_changes<R: tauri::Runtime>(app: tauri::AppHandle<R>, meeting_id: String) -> Result<Vec<VoiceDoubt>, String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    let Some(models) = super::commands::models_dir(&app) else { return Ok(Vec::new()) };
    voice_changes(&pool, &models, &meeting_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_line_words<R: tauri::Runtime>(app: tauri::AppHandle<R>, meeting_id: String) -> Result<Vec<LineWords>, String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    line_words(&pool, &meeting_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn split_segment<R: tauri::Runtime>(app: tauri::AppHandle<R>, meeting_id: String, segment_id: String, word_index: usize) -> Result<String, String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    crate::speakers::journal::journaled(&pool, &meeting_id, "розрізання рядка", || async { split(&pool, &meeting_id, &segment_id, word_index).await.map_err(|e| e.to_string()) }).await
}

#[tauri::command]
pub async fn merge_segments<R: tauri::Runtime>(app: tauri::AppHandle<R>, meeting_id: String, segment_id: String) -> Result<(), String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    crate::speakers::journal::journaled(&pool, &meeting_id, "об'єднання рядків", || async { merge(&pool, &meeting_id, &segment_id).await.map_err(|e| e.to_string()) }).await
}

/// «Це сказав…» for words `from..to` of a line: the line is cut around them
/// and they are given to `speaker`. Returns the id of the line they form.
#[tauri::command]
pub async fn assign_words_speaker<R: tauri::Runtime>(app: tauri::AppHandle<R>, meeting_id: String, segment_id: String, from: usize, to: usize, speaker: String) -> Result<String, String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    let label = format!("слова → {}", speaker.trim());
    crate::speakers::journal::journaled(&pool, &meeting_id, &label, || async {
        let id = cut_out(&pool, &meeting_id, &segment_id, from, to).await.map_err(|e| e.to_string())?;
        super::overrides::reassign_and_learn(&app, &pool, &meeting_id, &id, &speaker, super::overrides::Scope::Line).await?;
        Ok(id)
    })
    .await
}

/// Cuts words `from..to` of a line out into a line of their own; its id
pub async fn cut_out(pool: &SqlitePool, meeting_id: &str, segment_id: &str, from: usize, to: usize) -> Result<String> {
    let n = rows_of(pool, meeting_id).await?.into_iter().find(|r| r.0 == segment_id).map(|r| r.1.split_whitespace().count()).ok_or_else(|| anyhow!("Рядок не знайдено"))?;
    let to = to.min(n);
    if from >= to {
        return Err(anyhow!("Виділіть слова"));
    }
    if to < n {
        split(pool, meeting_id, segment_id, to).await?;
    }
    if from > 0 {
        return split(pool, meeting_id, segment_id, from).await;
    }
    Ok(segment_id.to_string())
}

#[tauri::command]
pub async fn set_segments_reviewed<R: tauri::Runtime>(app: tauri::AppHandle<R>, meeting_id: String, segment_ids: Vec<String>, reviewed: bool, quiet: Option<bool>) -> Result<ReviewProgress, String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    // Marked on its own (a line listened through or corrected): not a step to undo
    if quiet.unwrap_or(false) {
        set_reviewed(&pool, &meeting_id, &segment_ids, reviewed).await.map_err(|e| e.to_string())?;
        return progress(&pool, &meeting_id).await.map_err(|e| e.to_string());
    }
    let label = if reviewed { "позначка «перевірено»" } else { "зняття позначки «перевірено»" };
    crate::speakers::journal::journaled(&pool, &meeting_id, label, || async { set_reviewed(&pool, &meeting_id, &segment_ids, reviewed).await.map_err(|e| e.to_string()) }).await?;
    progress(&pool, &meeting_id).await.map_err(|e| e.to_string())
}

/// «Позначити все перевіреним»: one step of the history
#[tauri::command]
pub async fn api_mark_all_reviewed<R: tauri::Runtime>(app: tauri::AppHandle<R>, meeting_id: String) -> Result<ReviewProgress, String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    crate::speakers::journal::journaled(&pool, &meeting_id, "усе позначено перевіреним", || async { mark_all_reviewed(&pool, &meeting_id).await.map_err(|e| e.to_string()) }).await?;
    progress(&pool, &meeting_id).await.map_err(|e| e.to_string())
}

/// Every meeting's checked share of speaking time (0..1), for the list of meetings
#[tauri::command]
pub async fn get_review_overview<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<std::collections::HashMap<String, f64>, String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    let rows: Vec<(String, f64, f64)> = sqlx::query_as(
        "SELECT meeting_id, SUM(CASE WHEN reviewed_at IS NOT NULL THEN MAX(COALESCE(audio_end_time, 0) - COALESCE(audio_start_time, 0), 0) ELSE 0 END),
                SUM(MAX(COALESCE(audio_end_time, 0) - COALESCE(audio_start_time, 0), 0)) FROM transcripts GROUP BY meeting_id",
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(rows.into_iter().filter(|r| r.2 > 0.0).map(|(id, done, all)| (id, done / all)).collect())
}

#[tauri::command]
pub async fn get_review_progress<R: tauri::Runtime>(app: tauri::AppHandle<R>, meeting_id: String) -> Result<ReviewProgress, String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    progress(&pool, &meeting_id).await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speakers::words::PieceWords;

    #[test]
    fn voice_checks_are_kept_with_the_meeting() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_voice_checks(dir.path()).is_empty());
        let mut checks = std::collections::HashMap::new();
        checks.insert("a1".to_string(), vec![VoiceDoubt { segment_id: "s1".into(), from: 0, to: 3, seconds: 12.5, likely: "Спікер 2".into(), similarity: 0.71, whole: false, margin: 0.1 }]);
        checks.insert("b2".to_string(), Vec::new());
        save_voice_checks(dir.path(), &checks);
        assert_eq!(load_voice_checks(dir.path()), checks);
    }

    #[test]
    fn words_aligned_later_are_found_by_the_lines_own_times() {
        // Two lines stored at 10–13 and 13–16; the words sound ~2 s earlier
        let pieces = vec![PieceWords {
            text: "Добрий день. Склад готовий".into(),
            start: 8.0,
            end: 16.0,
            words: vec![("Добрий".into(), 8.1), ("день.".into(), 8.6), ("Склад".into(), 11.2), ("готовий".into(), 11.6)],
            probs: vec![],
            auto: vec![],
            aligned: true,
            heard: vec![(8.1, 10.0), (8.6, 10.6), (11.2, 13.0), (11.6, 13.5)],
        }];
        let (exact, w) = words_of_line(&pieces, "Склад готовий", 13.0, 16.0);
        assert!(exact);
        // Where they sound, and where the line's timeline has them
        assert_eq!((w[0].start, w[1].start), (11.2, 11.6));
        assert!((w[0].at - 13.0).abs() < 1e-9 && (w[1].at - 13.5).abs() < 1e-9);
        let (exact, w) = words_of_line(&pieces, "Добрий день.", 10.0, 13.0);
        assert!(exact);
        assert_eq!(w[0].start, 8.1);
    }

    #[test]
    fn words_get_times_from_recognition_or_by_length() {
        let pieces = vec![PieceWords {
            text: "Добрий день. Склад готовий".into(),
            start: 10.0,
            end: 14.0,
            words: vec![("Добрий".into(), 10.0), ("день.".into(), 10.6), ("Склад".into(), 12.0), ("готовий".into(), 12.5)],
            probs: vec![0.99, 0.9, 0.5, 0.97],
            auto: vec![], aligned: false, heard: vec![],
        }];
        // A line with the last two words: exact times and certainty
        let (exact, w) = words_of_line(&pieces, "Склад готовий", 12.0, 14.0);
        assert!(exact);
        assert_eq!((w[0].start, w[0].end, w[0].prob), (12.0, 12.5, 0.5));
        assert_eq!(w[1].end, 14.0);
        // A word typed in by hand: the others keep their times, it goes between them
        let (exact, w) = words_of_line(&pieces, "Склад повністю готовий", 12.0, 14.0);
        assert!(!exact);
        assert_eq!(w.len(), 3);
        assert_eq!((w[0].start, w[2].start), (12.0, 12.5));
        assert!(w[1].start > 12.0 && w[1].start < 12.5);
        // A word corrected to one that sounds alike keeps its moment
        let (exact, w) = words_of_line(&pieces, "Склад готові", 12.0, 14.0);
        assert!(exact);
        assert_eq!(w[1].start, 12.5);
    }

    async fn meeting() -> (tempfile::TempDir, crate::database::manager::DatabaseManager) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.sqlite");
        let manager = crate::database::manager::DatabaseManager::new(db.to_str().unwrap(), "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('m1', 'Тест', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')").execute(&pool).await.unwrap();
        for (id, text, a, b, s) in [
            ("a", "Добрий ранок, починаємо. Склад отримав генератори і пальне", 0.0, 10.0, "Тарас"),
            ("b", "Дякую, далі зв'язок", 10.0, 14.0, "Остап"),
        ] {
            sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, speaker) VALUES (?, 'm1', ?, 'x', ?, ?, ?)")
                .bind(id)
                .bind(text)
                .bind(a)
                .bind(b)
                .bind(s)
                .execute(&pool)
                .await
                .unwrap();
        }
        (dir, manager)
    }

    async fn lines(pool: &SqlitePool) -> Vec<(String, String, f64, f64, Option<String>)> {
        rows_of(pool, "m1").await.unwrap()
    }

    #[tokio::test]
    async fn the_start_of_a_line_given_to_another_person_and_back() {
        let (_d, m) = meeting().await;
        let pool = m.pool().clone();
        // «Добрий ранок, починаємо.» was said by someone else
        let start = cut_out(&pool, "m1", "a", 0, 3).await.unwrap();
        assert_eq!(start, "a");
        let rows = lines(&pool).await;
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].1, "Добрий ранок, починаємо.");
        assert_eq!(rows[1].1, "Склад отримав генератори і пальне");
        assert!(rows[0].3 == rows[1].2 && rows[1].3 == 10.0);
        assert_eq!(rows[1].4.as_deref(), Some("Тарас"));
        crate::speakers::overrides::reassign(&pool, "m1", "a", "Остап", crate::speakers::overrides::Scope::Line).await.unwrap();
        // A new recognition writes the old two lines again: the cut and the speaker come back
        let cut_at = rows[1].2;
        sqlx::query("DELETE FROM transcripts").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, speaker) VALUES ('n1', 'm1', 'Добрий ранок, починаємо. Склад отримав генератори і пальне', 'x', 0.0, 10.0, 'Спікер 1'), ('n2', 'm1', 'Дякую, далі зв''язок', 'x', 10.0, 14.0, 'Спікер 2')")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(apply(&pool, "m1").await.unwrap().0, 1);
        crate::speakers::overrides::apply(&pool, "m1").await.unwrap();
        let rows = lines(&pool).await;
        assert_eq!(rows.len(), 3);
        assert!((rows[1].2 - cut_at).abs() < 0.01);
        assert_eq!(rows[0].4.as_deref(), Some("Остап"));
        assert_eq!(rows[1].4.as_deref(), Some("Спікер 1"));
        // Joined again: one line, and the cut is forgotten
        merge(&pool, "m1", &rows[0].0).await.unwrap();
        assert_eq!(lines(&pool).await.len(), 2);
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM segment_edits").fetch_one(&pool).await.unwrap();
        assert_eq!(left, 0);
        assert!(split(&pool, "m1", "n2", 0).await.is_err());
    }

    #[test]
    fn bunched_and_tail_words_are_spread() {
        let w = |pairs: &[(&str, f64)]| pairs.iter().map(|(a, b)| (a.to_string(), *b)).collect::<Vec<_>>();
        let p = PieceWords {
            text: String::new(),
            start: 0.0,
            end: 6.0,
            words: w(&[("Доброго", 0.2), ("ранку", 0.9), ("отримали", 1.6), ("дванадцять", 1.6), ("генераторів", 1.62), ("на", 3.4), ("склад", 6.0), ("сьогодні", 6.0)]),
            probs: vec![],
            auto: vec![], aligned: false, heard: vec![],
        };
        let t = retimed(&p);
        // Three words at one moment are spread up to the next one (3.4)
        assert!(t[2] == 1.6 && t[3] > 1.9 && t[4] > t[3] && t[4] < 3.4, "{t:?}");
        // The two words pushed to the end are placed at the speaker's pace, inside the piece
        assert!(t[6] > 3.4 && t[6] < t[7] && t[7] < 6.0, "{t:?}");
        // Sound times are kept
        assert_eq!((t[0], t[1], t[5]), (0.2, 0.9, 3.4));
        // Words an old correction spilled 10–15 s past the piece come back inside it
        let p = PieceWords {
            text: String::new(),
            start: 10.0,
            end: 20.0,
            words: w(&[("перше", 10.2), ("друге", 11.0), ("третє", 24.0), ("четверте", 29.0), ("п'яте", 34.5)]),
            probs: vec![],
            auto: vec![], aligned: false, heard: vec![],
        };
        let t = retimed(&p);
        assert!(t.iter().all(|x| (10.0..=20.0).contains(x)), "{t:?}");
        assert!(t.windows(2).all(|x| x[0] <= x[1]) && t[1] == 11.0, "{t:?}");
    }

    #[test]
    fn sentences_short_ones_joined() {
        let w = |word: &str, start: f64, end: f64| WordTime { word: word.into(), start, end, prob: 1.0, at: start };
        let words = vec![w("Так.", 0.0, 0.4), w("Доброго", 0.4, 0.9), w("ранку.", 0.9, 1.6), w("Значить,", 2.0, 2.6), w("оголошення", 2.6, 3.5), w("такі.", 3.5, 4.0)];
        assert_eq!(sentence_chunks(&words), vec![(0, 3), (3, 6)]);
    }

    /// On a COPY: how many lines get every word's time from recognition
    /// (PROTOKOL_SPEAKERS_DB, PROTOKOL_SPEAKERS_MEETING)
    #[tokio::test]
    #[ignore = "needs a database copy and its recording folder"]
    async fn word_times_on_a_database_copy() {
        let db = std::env::var("PROTOKOL_SPEAKERS_DB").unwrap();
        assert!(!db.contains("Application Support"), "only a copy");
        let manager = crate::database::manager::DatabaseManager::new(&db, "/nonexistent-backend.db").await.unwrap();
        let lines = line_words(manager.pool(), &std::env::var("PROTOKOL_SPEAKERS_MEETING").unwrap()).await.unwrap();
        let exact = lines.iter().filter(|l| l.exact).count();
        println!("{} line(s): {} with every word's time from recognition, {} partly interpolated", lines.len(), exact, lines.len() - exact);
    }

    /// On a COPY: PROTOKOL_SPEAKERS_DB, PROTOKOL_SPEAKERS_MODELS, PROTOKOL_SPEAKERS_MEETING
    #[tokio::test]
    #[ignore = "needs a database copy, a recording and the models"]
    async fn voice_changes_on_a_database_copy() {
        let db = std::env::var("PROTOKOL_SPEAKERS_DB").unwrap();
        assert!(!db.contains("Application Support"), "only a copy");
        let manager = crate::database::manager::DatabaseManager::new(&db, "/nonexistent-backend.db").await.unwrap();
        let started = std::time::Instant::now();
        let found = voice_changes(manager.pool(), std::path::Path::new(&std::env::var("PROTOKOL_SPEAKERS_MODELS").unwrap()), &std::env::var("PROTOKOL_SPEAKERS_MEETING").unwrap()).await.unwrap();
        println!("{} place(s) in {:.1}s", found.len(), started.elapsed().as_secs_f64());
        for d in &found {
            println!("{:>7.1}s words {}..{} → {}", d.seconds, d.from, d.to, d.likely);
        }
    }

    /// Which rule catches the speaker fixes made by hand, on a COPY: the lines
    /// get back the speakers recognition gave them; nothing is excluded.
    /// PROTOKOL_VOICE_DB, PROTOKOL_SPEAKERS_MEETING, PROTOKOL_SPEAKERS_MODELS. Counts only.
    #[tokio::test]
    #[ignore = "needs a database copy, a recording and the models"]
    async fn voice_rules_against_hand_fixes() {
        let db = std::env::var("PROTOKOL_VOICE_DB").unwrap();
        assert!(db.contains(&crate::test_copy_root()), "only a copy");
        let meeting = std::env::var("PROTOKOL_SPEAKERS_MEETING").unwrap();
        let manager = crate::database::manager::DatabaseManager::new(&db, "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        let fixes: Vec<(f64, f64, String)> = sqlx::query_as("SELECT start, end, previous FROM speaker_overrides WHERE meeting_id = ? AND previous IS NOT NULL AND end - start >= 0.3 ORDER BY id DESC").bind(&meeting).fetch_all(&pool).await.unwrap();
        for (a, b, previous) in &fixes {
            sqlx::query("UPDATE transcripts SET speaker = ? WHERE meeting_id = ? AND audio_start_time >= ? - 0.05 AND audio_start_time < ? + 0.05").bind(previous).bind(&meeting).bind(a).bind(b).execute(&pool).await.unwrap();
        }
        sqlx::query("UPDATE transcripts SET speaker_manual = 0, reviewed_at = NULL WHERE meeting_id = ?").bind(&meeting).execute(&pool).await.unwrap();
        sqlx::query("DELETE FROM segment_reviews WHERE meeting_id = ?").bind(&meeting).execute(&pool).await.unwrap();
        let models = std::path::PathBuf::from(std::env::var("PROTOKOL_SPEAKERS_MODELS").unwrap());
        let _ = voice_changes(&pool, &models, &meeting).await.unwrap();
        let scores = LAST_VOICE_SCORES.lock().unwrap().clone();
        let lines: Vec<(String, f64, f64)> = sqlx::query_as("SELECT id, audio_start_time, audio_end_time FROM transcripts WHERE meeting_id = ?").bind(&meeting).fetch_all(&pool).await.unwrap();
        println!("hand fixes {} (inside a checked line: {}), stretches scored {}", fixes.len(), fixes.iter().filter(|f| scores.iter().any(|s| s.start < f.1 && s.end > f.0)).count(), scores.len());
        println!("rule  shown  lines  fixes-caught  fix@134s");
        for rule in 1..=4u8 {
            let shown: Vec<&VoiceScore> = scores.iter().filter(|s| voice_shown(rule, s.own, s.best, s.rest, s.end - s.start)).collect();
            let shown_lines: std::collections::HashSet<&String> = shown.iter().map(|s| &s.segment_id).collect();
            let caught = fixes.iter().filter(|f| shown.iter().any(|s| s.start < f.1 + 0.5 && s.end > f.0 - 0.5)).count();
            let at134 = fixes.iter().filter(|f| (f.0 - 134.0).abs() < 3.0).any(|f| shown.iter().any(|s| s.start < f.1 + 0.5 && s.end > f.0 - 0.5));
            println!("{rule:>4} {:>6} {:>6} {:>13} {:>9}", shown.len(), shown_lines.len(), caught, at134);
        }
        let _ = lines;
        // The scores on the fixed stretches (numbers only)
        for f in &fixes {
            for s in scores.iter().filter(|s| s.start < f.1 + 0.5 && s.end > f.0 - 0.5) {
                println!("fix@{:>6.1}s len {:>5.1}  stretch {:>6.1}-{:>6.1}  own {:.2} best {:.2} rest {:?}", f.0, f.1 - f.0, s.start, s.end, s.own, s.best, s.rest.map(|r| (r * 100.0).round() / 100.0));
            }
        }
        // Everything else, for the base rate
        let mut d: Vec<f32> = scores.iter().map(|s| s.best - s.own).collect();
        d.sort_by(|a, b| a.total_cmp(b));
        println!("best-own over all stretches: p50 {:.2} p90 {:.2} max {:.2}", d[d.len() / 2], d[d.len() * 9 / 10], d[d.len() - 1]);
    }

    /// The whole-line check («Можливо, це X?») against the hand fixes, on a
    /// COPY: each line's voice is compared with every person's voice made from
    /// the other lines (as corrected by hand; the line itself left out).
    /// PROTOKOL_VOICE_DB, PROTOKOL_SPEAKERS_MEETING, PROTOKOL_SPEAKERS_MODELS. Counts only.
    #[tokio::test]
    #[ignore = "needs a database copy, a recording and the models"]
    async fn whole_line_voice_check_against_hand_fixes() {
        use super::super::embedding::cosine;
        let db = std::env::var("PROTOKOL_VOICE_DB").unwrap();
        assert!(db.contains(&crate::test_copy_root()), "only a copy");
        let meeting = std::env::var("PROTOKOL_SPEAKERS_MEETING").unwrap();
        let manager = crate::database::manager::DatabaseManager::new(&db, "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        let fixes: Vec<(f64, f64, String, String)> = sqlx::query_as("SELECT start, end, previous, speaker FROM speaker_overrides WHERE meeting_id = ? AND previous IS NOT NULL AND end - start >= 0.3").bind(&meeting).fetch_all(&pool).await.unwrap();
        let rows: Vec<(String, f64, f64, Option<String>)> = sqlx::query_as("SELECT id, audio_start_time, audio_end_time, speaker FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time").bind(&meeting).fetch_all(&pool).await.unwrap();
        let folder = folder_of(&pool, &meeting).await.unwrap();
        let audio = crate::audio::retranscription::find_audio_file(&folder).unwrap();
        let samples = super::super::commands::decode_16k(&audio).unwrap();
        let models = std::path::PathBuf::from(std::env::var("PROTOKOL_SPEAKERS_MODELS").unwrap());
        let mut emb = super::super::embedding::Embedder::open_light(&models.join(super::super::commands::EMBEDDING_MODEL)).unwrap();
        // Each line: its voice, its speaker now (true), and the one recognition gave (before the fix)
        struct L { now: String, before: String, e: Vec<f32>, fixed: bool, at: f64 }
        let mut lines: Vec<L> = Vec::new();
        let mut short = 0;
        let min_len: f64 = std::env::var("PROTOKOL_MIN_LINE_S").ok().and_then(|v| v.parse().ok()).unwrap_or(1.5);
        for (_, a, b, sp) in &rows {
            let Some(now) = sp.clone() else { continue };
            if b - a < min_len {
                short += 1;
                continue;
            }
            let fix = fixes.iter().find(|f| f.0 < b - 0.2 && f.1 > a + 0.2 && f.3 == now);
            let before = fix.map(|f| f.2.clone()).unwrap_or_else(|| now.clone());
            let (i, j) = ((a * 16000.0) as usize, (((a + (b - a).min(20.0)) * 16000.0) as usize).min(samples.len()));
            let Some(e) = emb.embed(&samples[i..j]).unwrap() else { continue };
            lines.push(L { fixed: before != now, now, before, e, at: *a });
        }
        let positives = lines.iter().filter(|l| l.fixed).count();
        println!("lines with voice {} (short {short}), fixed by hand among them {positives}", lines.len());
        let mean = |v: Vec<&Vec<f32>>| -> Option<Vec<f32>> {
            if v.is_empty() { return None; }
            let mut m = vec![0f32; v[0].len()];
            for e in &v { for (x, y) in m.iter_mut().zip(e.iter()) { *x += y; } }
            let n = m.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
            Some(m.into_iter().map(|x| x / n).collect())
        };
        let people: Vec<String> = { let mut p: Vec<String> = lines.iter().map(|l| l.now.clone()).collect(); p.sort(); p.dedup(); p };
        // For each line: how much more like someone else than like the speaker it was given
        let mut margins: Vec<(f32, bool, bool)> = Vec::new(); // margin, fixed, suggests the right person
        let mut fixed_at: Vec<(f64, f32, bool)> = Vec::new();
        for (k, l) in lines.iter().enumerate() {
            let profile = |who: &str| mean(lines.iter().enumerate().filter(|(m, x)| *m != k && x.now == who).map(|(_, x)| &x.e).collect());
            let Some(own) = profile(&l.before) else { continue };
            let own_sim = cosine(&l.e, &own);
            let best = people.iter().filter(|p| **p != l.before).filter_map(|p| profile(p).map(|pr| (p.clone(), cosine(&l.e, &pr)))).max_by(|a, b| a.1.total_cmp(&b.1));
            let Some((who, sim)) = best else { continue };
            margins.push((sim - own_sim, l.fixed, who == l.now));
            if l.fixed {
                fixed_at.push((l.at, sim - own_sim, who == l.now));
            }
        }
        println!("margin  shown  fixes-caught  right-person");
        for m in [-0.02f32, 0.0, 0.03, 0.05, 0.10, 0.15] {
            let shown = margins.iter().filter(|x| x.0 >= m).count();
            let caught = margins.iter().filter(|x| x.0 >= m && x.1).count();
            let right = margins.iter().filter(|x| x.0 >= m && x.1 && x.2).count();
            println!("{m:>6.2} {shown:>6} {caught:>13} {right:>13}");
        }
        for (at, m, right) in &fixed_at {
            println!("fixed line at {:>6.1}s: margin {m:+.2}, suggests the right person: {right}", at);
        }
    }

    /// Both tiers of «Можливо, X?» on a COPY (PROTOKOL_VOICE_DB, PROTOKOL_SPEAKERS_MEETING,
    /// PROTOKOL_SPEAKERS_MODELS; PROTOKOL_AS_RECOGNISED=1: his fixes undone first). Counts only.
    #[tokio::test]
    #[ignore = "needs a database copy, a recording and the models"]
    async fn whole_line_tiers_on_a_copy() {
        let db = std::env::var("PROTOKOL_VOICE_DB").unwrap();
        assert!(db.contains(&crate::test_copy_root()), "only a copy");
        let meeting = std::env::var("PROTOKOL_SPEAKERS_MEETING").unwrap();
        let manager = crate::database::manager::DatabaseManager::new(&db, "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        let fixes: Vec<(f64, f64, String)> = sqlx::query_as("SELECT start, end, previous FROM speaker_overrides WHERE meeting_id = ? AND previous IS NOT NULL ORDER BY id DESC").bind(&meeting).fetch_all(&pool).await.unwrap();
        if std::env::var("PROTOKOL_AS_RECOGNISED").is_ok() {
            for (a, b, previous) in &fixes {
                sqlx::query("UPDATE transcripts SET speaker = ?, speaker_manual = 0 WHERE meeting_id = ? AND audio_start_time >= ? - 0.05 AND audio_start_time < ? + 0.05").bind(previous).bind(&meeting).bind(a).bind(b).execute(&pool).await.unwrap();
            }
            sqlx::query("UPDATE transcripts SET reviewed_at = NULL WHERE meeting_id = ?").bind(&meeting).execute(&pool).await.unwrap();
        }
        let models = std::path::PathBuf::from(std::env::var("PROTOKOL_SPEAKERS_MODELS").unwrap());
        let found = whole_line_changes(&pool, &models, &meeting).await.unwrap();
        let lines: std::collections::HashSet<&String> = found.iter().map(|d| &d.segment_id).collect();
        let settled: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transcripts WHERE meeting_id = ? AND (speaker_manual = 1 OR reviewed_at IS NOT NULL)").bind(&meeting).fetch_one(&pool).await.unwrap();
        let caught = |m: f32| fixes.iter().filter(|f| found.iter().any(|d| d.margin >= m && (d.seconds - f.0).abs() < 1.0)).count();
        println!("TIERS hints (margin>=0) {} on {} lines (max 1 per line: {}); queue (>=0.10) {}; settled lines skipped {settled}; hand fixes caught: hint {} queue {}",
            found.len(), lines.len(), found.len() == lines.len(), found.iter().filter(|d| d.margin >= 0.10).count(), caught(0.0), caught(0.10));
    }

    #[tokio::test]
    async fn checked_lines_and_progress_survive_a_new_recognition() {
        let (_d, m) = meeting().await;
        let pool = m.pool().clone();
        let p = progress(&pool, "m1").await.unwrap();
        assert_eq!((p.reviewed, p.total), (0, 2));
        set_reviewed(&pool, "m1", &["a".to_string()], true).await.unwrap();
        let p = progress(&pool, "m1").await.unwrap();
        assert_eq!((p.reviewed, p.seconds_reviewed, p.seconds_total), (1, 10.0, 14.0));
        sqlx::query("UPDATE transcripts SET reviewed_at = NULL, id = 'z' WHERE id = 'a'").execute(&pool).await.unwrap();
        apply(&pool, "m1").await.unwrap();
        assert_eq!(progress(&pool, "m1").await.unwrap().reviewed, 1);
        set_reviewed(&pool, "m1", &["z".to_string()], false).await.unwrap();
        assert_eq!(progress(&pool, "m1").await.unwrap().reviewed, 0);
    }
}
