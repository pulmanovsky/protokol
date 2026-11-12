//! The call's own recording (system.m4a) and the meeting's mix (audio.mp4,
//! what the player plays) are written by two clocks that drift apart (on a
//! 33-minute meeting about 0.08 s a minute, 2.3 s by the end). Everything
//! recognised from the call is put on the mix's timeline with this map, so
//! the lit words, the time marks and the bars meet the sound.

use anyhow::Result;
use realfft::RealFftPlanner;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::Path;

const RATE: usize = 16_000;
/// A point every minute; each compares 10 s with ±3 s around it
const STEP_S: f64 = 60.0;
const WINDOW_S: f64 = 10.0;
const SEARCH_S: f64 = 3.0;
const MIN_CORR: f32 = 0.5;
pub const MAP_FILE: &str = "time_map.json";

/// Moments of the call recording and the same moments in the mix
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TimeMap {
    /// (call time, mix time), increasing in both
    pub points: Vec<(f64, f64)>,
}

impl TimeMap {
    pub fn is_identity(&self) -> bool {
        self.points.iter().all(|(a, b)| (a - b).abs() < 0.02)
    }

    /// A call-recording moment on the mix's timeline
    pub fn map(&self, t: f64) -> f64 {
        match self.points.len() {
            0 => t,
            1 => t + (self.points[0].1 - self.points[0].0),
            _ => {
                let i = self.points.partition_point(|p| p.0 <= t);
                // Before the first / after the last point: that end's offset
                if i == 0 {
                    return t + (self.points[0].1 - self.points[0].0);
                }
                if i == self.points.len() {
                    let (a, b) = self.points[i - 1];
                    return t + (b - a);
                }
                let ((a0, b0), (a1, b1)) = (self.points[i - 1], self.points[i]);
                let k = if a1 > a0 { (t - a0) / (a1 - a0) } else { 0.0 };
                b0 + k * (b1 - b0)
            }
        }
    }

    /// From measured (mix time, offset, correlation) points: reliable ones,
    /// outliers against their neighbours left out, kept monotonic
    pub fn from_measures(measures: &[(f64, f64, f32)]) -> TimeMap {
        let good: Vec<(f64, f64)> = measures.iter().filter(|m| m.2 >= MIN_CORR).map(|m| (m.0, m.1)).collect();
        // Running median of the offsets (5 around): a point far from it is a mismatch
        let mut kept: Vec<(f64, f64)> = Vec::new();
        for (i, (t, d)) in good.iter().enumerate() {
            let lo = i.saturating_sub(2);
            let hi = (i + 3).min(good.len());
            let mut around: Vec<f64> = good[lo..hi].iter().map(|x| x.1).collect();
            around.sort_by(|a, b| a.total_cmp(b));
            let med = around[around.len() / 2];
            if (d - med).abs() <= 0.3 {
                kept.push((*t, *d));
            }
        }
        if kept.is_empty() {
            log::info!("Time map: no reliable point; the call recording is taken as is");
            return TimeMap::default();
        }
        if kept.len() < 3 {
            let (t, d) = kept[0];
            log::info!("Time map: {} reliable point(s); one offset {:+.3} s", kept.len(), d);
            return TimeMap { points: vec![(t + d, t)] };
        }
        // (call time, mix time), both increasing
        let mut points: Vec<(f64, f64)> = Vec::new();
        for (t, d) in kept {
            let p = (t + d, t);
            if points.last().map_or(true, |l| p.0 > l.0 && p.1 > l.1) {
                points.push(p);
            }
        }
        log::info!("Time map: {} point(s), offset {:+.3} … {:+.3} s", points.len(), points[0].0 - points[0].1, points[points.len() - 1].0 - points[points.len() - 1].1);
        TimeMap { points }
    }
}

/// Where 10 s of the mix at `c` are found in the call recording: (offset in
/// seconds, call minus mix; normalised correlation)
fn offset_at(mix: &[f32], call: &[f32], c: usize) -> Option<(f64, f32)> {
    let w = (WINDOW_S * RATE as f64) as usize;
    let l = (SEARCH_S * RATE as f64) as usize;
    if c + w > mix.len() {
        return None;
    }
    let a = &mix[c..c + w];
    let from = c.saturating_sub(l);
    let to = (c + w + l).min(call.len());
    if to <= from + w {
        return None;
    }
    let seg = &call[from..to];
    let n = (seg.len() + w).next_power_of_two();
    let mut planner = RealFftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut x = vec![0f32; n];
    x[..seg.len()].copy_from_slice(seg);
    let mut y = vec![0f32; n];
    y[..w].copy_from_slice(a);
    let mut xs = fwd.make_output_vec();
    let mut ys = fwd.make_output_vec();
    fwd.process(&mut x, &mut xs).ok()?;
    fwd.process(&mut y, &mut ys).ok()?;
    for (p, q) in xs.iter_mut().zip(&ys) {
        *p *= q.conj();
    }
    let mut out = inv.make_output_vec();
    inv.process(&mut xs, &mut out).ok()?;
    // Normalised at each lag (the window's energy in the call recording)
    let mut cum = vec![0f64; seg.len() + 1];
    for (i, v) in seg.iter().enumerate() {
        cum[i + 1] = cum[i] + (*v as f64) * (*v as f64);
    }
    let na = a.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>().sqrt();
    let mut best: Option<(usize, f32)> = None;
    for k in 0..=(seg.len() - w) {
        let e = (cum[k + w] - cum[k]).sqrt() * na;
        if e <= 1e-9 {
            continue;
        }
        let r = (out[k] as f64 / n as f64 / e) as f32;
        if best.map_or(true, |b| r > b.1) {
            best = Some((k, r));
        }
    }
    let (k, r) = best?;
    Some(((from + k) as f64 / RATE as f64 - c as f64 / RATE as f64, r))
}

/// Measures the drift between the mix and the call recording (16 kHz mono)
pub fn fit(mix: &[f32], call: &[f32]) -> TimeMap {
    let step = (STEP_S * RATE as f64) as usize;
    let mut measures = Vec::new();
    let mut c = (5 * RATE).min(mix.len());
    while c + (WINDOW_S as usize) * RATE < mix.len().min(call.len() + 3 * RATE) {
        if let Some((d, r)) = offset_at(mix, call, c) {
            measures.push((c as f64 / RATE as f64, d, r));
        }
        c += step;
    }
    TimeMap::from_measures(&measures)
}

/// Where speech starts after at least 0.4 s of quiet (10 ms frames)
pub(crate) fn speech_onsets(samples: &[f32]) -> Vec<f64> {
    let frame = RATE / 100;
    let rms: Vec<f32> = samples.chunks(frame).map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len().max(1) as f32).sqrt()).collect();
    if rms.len() < 100 {
        return Vec::new();
    }
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let (floor, speech) = (sorted[sorted.len() / 10], sorted[sorted.len() * 7 / 10]);
    let thr = floor + (speech - floor) * 0.3;
    let mut out = Vec::new();
    let mut quiet = 0;
    for (i, r) in rms.iter().enumerate() {
        if *r < thr {
            quiet += 1;
        } else {
            if quiet >= 40 {
                out.push(i as f64 / 100.0);
            }
            quiet = 0;
        }
    }
    out
}

/// How far word starts are from the speech onsets of the mix (median, signed)
pub(crate) fn onset_lag(onsets: &[f64], words: &[f64]) -> Option<f64> {
    let mut sorted = words.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mut d: Vec<f64> = onsets
        .iter()
        .filter_map(|o| {
            let i = sorted.partition_point(|w| w < o);
            [i.checked_sub(1), Some(i)].into_iter().flatten().filter_map(|k| sorted.get(k)).map(|w| w - o).filter(|x| x.abs() < 1.5).min_by(|a, b| a.abs().total_cmp(&b.abs()))
        })
        .collect();
    if d.len() < 10 {
        return None;
    }
    d.sort_by(|a, b| a.total_cmp(b));
    Some(d[d.len() / 2])
}

/// The map is applied only where it brings the words closer to the mix's
/// speech: a meeting recognised from the mix is already on its timeline
pub(crate) fn words_need_map(onsets: &[f64], words: &[f64], map: &TimeMap) -> bool {
    let mapped: Vec<f64> = words.iter().map(|t| map.map(*t)).collect();
    map_is_better(onsets, words, &mapped)
}

/// Whether the mapped word starts meet the mix's speech clearly better
pub(crate) fn map_is_better(onsets: &[f64], words: &[f64], mapped: &[f64]) -> bool {
    match (onset_lag(onsets, words), onset_lag(onsets, mapped)) {
        (Some(now), Some(then)) => {
            log::info!("Word starts vs the mix's speech onsets: {:+.3} s as stored, {:+.3} s mapped", now, then);
            then.abs() + 0.15 <= now.abs() && then.abs() < 0.3
        }
        _ => false,
    }
}

pub fn load(folder: &Path) -> Option<TimeMap> {
    serde_json::from_slice(&std::fs::read(folder.join(MAP_FILE)).ok()?).ok()
}

pub fn save(folder: &Path, map: &TimeMap) {
    if let Ok(json) = serde_json::to_vec(map) {
        let _ = std::fs::write(folder.join(MAP_FILE), json);
    }
}

/// A meeting recognised from the call recording before the map existed:
/// its stored moments are put on the mix's timeline once (the transcript,
/// the word times, what the secretary set by hand and the undo history).
/// Returns whether anything moved.
pub async fn migrate(pool: &SqlitePool, meeting_id: &str, folder: &Path, decode: impl Fn(&Path) -> Result<Vec<f32>>) -> Result<bool> {
    let flag = format!("time_map_applied:{meeting_id}");
    if crate::database::repositories::setting::SettingsRepository::get_flag(pool, &flag).await?.is_some() {
        return Ok(false);
    }
    let system = folder.join(crate::audio::channel_track::SYSTEM_AUDIO_FILE);
    let words = folder.join(super::words::WORDS_FILE);
    if !system.exists() || !words.exists() {
        crate::database::repositories::setting::SettingsRepository::set_flag(pool, &flag, "none").await?;
        return Ok(false);
    }
    let mix_path = crate::audio::retranscription::find_audio_file(folder)?;
    let (mix, call) = (decode(&mix_path)?, decode(&system)?);
    let (map, onsets) = tokio::task::spawn_blocking(move || (fit(&mix, &call), speech_onsets(&mix))).await?;
    save(folder, &map);
    if map.is_identity() {
        crate::database::repositories::setting::SettingsRepository::set_flag(pool, &flag, "identity").await?;
        return Ok(false);
    }
    // Recognised from the mix already (older versions): nothing to move
    let starts: Vec<f64> = super::words::load(folder).map(|p| p.iter().flat_map(|x| x.words.iter().map(|w| w.1)).collect()).unwrap_or_default();
    if !words_need_map(&onsets, &starts, &map) {
        crate::database::repositories::setting::SettingsRepository::set_flag(pool, &flag, "mix").await?;
        return Ok(false);
    }
    apply_to_meeting(pool, meeting_id, folder, &map).await?;
    crate::database::repositories::setting::SettingsRepository::set_flag(pool, &flag, "mapped").await?;
    Ok(true)
}

/// Every stored moment of a meeting through the map
pub async fn apply_to_meeting(pool: &SqlitePool, meeting_id: &str, folder: &Path, map: &TimeMap) -> Result<()> {
    let mut tx = pool.begin().await?;
    let rows: Vec<(String, Option<f64>, Option<f64>)> = sqlx::query_as("SELECT id, audio_start_time, audio_end_time FROM transcripts WHERE meeting_id = ?").bind(meeting_id).fetch_all(&mut *tx).await?;
    for (id, a, b) in rows {
        let (a, b) = (a.map(|x| map.map(x)), b.map(|x| map.map(x)));
        sqlx::query("UPDATE transcripts SET audio_start_time = ?, audio_end_time = ?, duration = ? WHERE id = ?")
            .bind(a)
            .bind(b)
            .bind(match (a, b) {
                (Some(a), Some(b)) => Some(b - a),
                _ => None,
            })
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    for (table, cols) in [("speaker_overrides", ["start", "end"].as_slice()), ("segment_reviews", &["start", "end"]), ("segment_exclusions", &["start", "end"]), ("segment_edits", &["at"])] {
        let ids: Vec<(i64, f64, Option<f64>)> = if cols.len() == 2 {
            sqlx::query_as(&format!("SELECT id, start, end FROM {table} WHERE meeting_id = ?")).bind(meeting_id).fetch_all(&mut *tx).await?
        } else {
            sqlx::query_as::<_, (i64, f64)>(&format!("SELECT id, at FROM {table} WHERE meeting_id = ?")).bind(meeting_id).fetch_all(&mut *tx).await?.into_iter().map(|(i, a)| (i, a, None)).collect()
        };
        for (id, a, b) in ids {
            if let Some(b) = b {
                sqlx::query(&format!("UPDATE {table} SET start = ?, end = ? WHERE id = ?")).bind(map.map(a)).bind(map.map(b)).bind(id).execute(&mut *tx).await?;
            } else {
                sqlx::query(&format!("UPDATE {table} SET at = ? WHERE id = ?")).bind(map.map(a)).bind(id).execute(&mut *tx).await?;
            }
        }
    }
    // The undo history: moments inside its snapshots too
    let entries: Vec<(i64, String, String)> = sqlx::query_as("SELECT id, before_state, after_state FROM edit_journal WHERE meeting_id = ?").bind(meeting_id).fetch_all(&mut *tx).await?;
    for (id, before, after) in entries {
        sqlx::query("UPDATE edit_journal SET before_state = ?, after_state = ? WHERE id = ?")
            .bind(map_snapshot(&before, map))
            .bind(map_snapshot(&after, map))
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    // The word times
    if let Some(mut pieces) = super::words::load(folder) {
        for p in pieces.iter_mut() {
            p.start = map.map(p.start);
            p.end = map.map(p.end);
            for w in p.words.iter_mut() {
                w.1 = map.map(w.1);
            }
        }
        super::words::save(folder, &pieces)?;
    }
    Ok(())
}

/// A journal snapshot (JSON) with its moments mapped
fn map_snapshot(json: &str, map: &TimeMap) -> String {
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(json) else { return json.to_string() };
    fn walk(v: &mut serde_json::Value, map: &TimeMap) {
        match v {
            serde_json::Value::Object(o) => {
                for (k, x) in o.iter_mut() {
                    if matches!(k.as_str(), "audio_start_time" | "audio_end_time" | "start" | "end" | "at") {
                        if let Some(t) = x.as_f64() {
                            *x = serde_json::Value::from(map.map(t));
                            continue;
                        }
                    }
                    walk(x, map);
                }
                // A line's length follows its ends
                if let (Some(a), Some(b)) = (o.get("audio_start_time").and_then(|x| x.as_f64()), o.get("audio_end_time").and_then(|x| x.as_f64())) {
                    if o.contains_key("duration") {
                        o.insert("duration".into(), serde_json::Value::from(b - a));
                    }
                }
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(|x| walk(x, map)),
            _ => {}
        }
    }
    walk(&mut v, map);
    v.to_string()
}

/// Before the first move of a meeting in this run of the app: a copy of the database
static BACKED_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Opening a meeting: moves it to the mix's timeline if it was recognised
/// before the map existed (once; the database is copied first). True when
/// its times changed (the page reloads the lines).
#[tauri::command]
pub async fn api_align_meeting_timeline<R: tauri::Runtime>(app: tauri::AppHandle<R>, meeting_id: String) -> Result<bool, String> {
    use tauri::Manager;
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    let flag = format!("time_map_applied:{meeting_id}");
    if crate::database::repositories::setting::SettingsRepository::get_flag(&pool, &flag).await.map_err(|e| e.to_string())?.is_some() {
        return Ok(false);
    }
    let folder: Option<String> = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?").bind(&meeting_id).fetch_optional(&pool).await.map_err(|e| e.to_string())?.flatten();
    let Some(folder) = folder.map(std::path::PathBuf::from) else { return Ok(false) };
    if !folder.join(crate::audio::channel_track::SYSTEM_AUDIO_FILE).exists() || !folder.join(super::words::WORDS_FILE).exists() {
        let _ = crate::database::repositories::setting::SettingsRepository::set_flag(&pool, &flag, "none").await;
        return Ok(false);
    }
    if !BACKED_UP.swap(true, std::sync::atomic::Ordering::SeqCst) {
        if let Ok(dir) = app.path().app_data_dir() {
            let dir = dir.join("backups");
            let _ = std::fs::create_dir_all(&dir);
            let file = dir.join(format!("before-time-map-{}.sqlite", chrono::Local::now().format("%Y%m%d-%H%M%S")));
            if let Err(e) = sqlx::query(&format!("VACUUM INTO '{}'", file.display().to_string().replace('\'', "''"))).execute(&pool).await {
                log::warn!("Could not copy the database before moving times: {e}");
            }
        }
    }
    migrate(&pool, &meeting_id, &folder, |p| super::commands::decode_16k(p)).await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drifting_clock_is_mapped_and_outliers_left_out() {
        // The call recording runs 0.075 s a minute ahead; two bad measures
        let mut m: Vec<(f64, f64, f32)> = (0..30).map(|i| (60.0 * i as f64, -0.075 * i as f64 / 1.0, 0.9)).collect();
        m[7].1 = 2.5; // a mismatch
        m[12].2 = 0.2; // not reliable
        let map = TimeMap::from_measures(&m);
        assert_eq!(map.points.len(), 28);
        // A call moment at 20 minutes (mix 1200 s ↔ call 1198.5 s)
        assert!((map.map(1198.5) - 1200.0).abs() < 0.01);
        assert!((map.map(10.0 * 60.0 - 0.75) - 600.0).abs() < 0.01);
        // Increasing
        assert!(map.points.windows(2).all(|w| w[1].0 > w[0].0 && w[1].1 > w[0].1));
        // Too few points: one offset; none: as is
        let one = TimeMap::from_measures(&[(60.0, -0.4, 0.9)]);
        assert!((one.map(100.0) - 100.4).abs() < 1e-9);
        assert!(TimeMap::from_measures(&[(60.0, -0.4, 0.1)]).is_identity());
    }

    #[test]
    fn words_already_on_the_mix_are_not_moved() {
        let onsets: Vec<f64> = (0..40).map(|i| 30.0 + 45.0 * i as f64).collect();
        let map = TimeMap { points: vec![(0.0, 0.0), (1000.0, 1001.0), (2000.0, 2002.0)] };
        // Words that start at the onsets (on the mix): left alone
        let on_mix: Vec<f64> = onsets.iter().map(|o| o + 0.05).collect();
        assert!(!words_need_map(&onsets, &on_mix, &map));
        // Words on the call's clock (earlier by the drift): moved
        let on_call: Vec<f64> = onsets.iter().map(|o| o - (o / 1000.0) + 0.05).collect();
        assert!(words_need_map(&onsets, &on_call, &map));
    }

    #[test]
    fn the_offset_of_a_shifted_signal_is_found() {
        // A noise-like signal, the call copy starting 0.4 s later
        let mut s = 12345u32;
        let mix: Vec<f32> = (0..RATE * 40)
            .map(|_| {
                s = s.wrapping_mul(1103515245).wrapping_add(12345);
                ((s >> 16) as f32 / 65536.0 - 0.5) * 0.5
            })
            .collect();
        let shift = (0.4 * RATE as f64) as usize;
        let mut call = vec![0f32; shift];
        call.extend_from_slice(&mix);
        let (d, r) = offset_at(&mix, &call, RATE * 10).unwrap();
        assert!((d - 0.4).abs() < 0.001 && r > 0.9, "{d} {r}");
    }

    #[tokio::test]
    async fn moving_a_meeting_keeps_what_was_set_by_hand() {
        let dir = tempfile::tempdir().unwrap();
        let manager = crate::database::manager::DatabaseManager::new(dir.path().join("t.sqlite").to_str().unwrap(), "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('m1', 'Тест', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, speaker) VALUES ('a', 'm1', 'Склад готовий', 'x', 600.0, 610.0, 'Спікер 1'), ('b', 'm1', 'Дякую', 'x', 610.0, 612.0, 'Спікер 2')")
            .execute(&pool)
            .await
            .unwrap();
        crate::speakers::overrides::reassign(&pool, "m1", "a", "Остап", crate::speakers::overrides::Scope::Line).await.unwrap();
        // The call recording 0.75 s behind at 10 minutes
        let map = TimeMap { points: vec![(0.0, 0.0), (599.25, 600.0), (1198.5, 1200.0)] };
        apply_to_meeting(&pool, "m1", dir.path(), &map).await.unwrap();
        let a: (f64, f64, String) = sqlx::query_as("SELECT audio_start_time, audio_end_time, speaker FROM transcripts WHERE id = 'a'").fetch_one(&pool).await.unwrap();
        assert!((a.0 - map.map(600.0)).abs() < 1e-9 && a.2 == "Остап");
        // A new recognition (mix timeline) still finds the line given by hand
        sqlx::query("UPDATE transcripts SET speaker = 'Спікер 1', speaker_manual = 0 WHERE id = 'a'").execute(&pool).await.unwrap();
        assert_eq!(crate::speakers::overrides::apply(&pool, "m1").await.unwrap(), 1);
        let who: String = sqlx::query_scalar("SELECT speaker FROM transcripts WHERE id = 'a'").fetch_one(&pool).await.unwrap();
        assert_eq!(who, "Остап");
    }
}

#[cfg(test)]
mod copy_test {
    /// On a COPY (database and meeting folder): PROTOKOL_SPEAKERS_DB, PROTOKOL_SPEAKERS_MEETING
    #[tokio::test]
    #[ignore = "needs a database copy and a copied meeting folder"]
    async fn move_a_meeting_copy() {
        let db = std::env::var("PROTOKOL_SPEAKERS_DB").unwrap();
        assert!(!db.contains("Application Support"), "only a copy");
        let meeting = std::env::var("PROTOKOL_SPEAKERS_MEETING").unwrap();
        let manager = crate::database::manager::DatabaseManager::new(&db, "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        let folder: String = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?").bind(&meeting).fetch_one(&pool).await.unwrap();
        assert!(folder.contains(&crate::test_copy_root()), "only a copied folder");
        let folder = std::path::PathBuf::from(folder);
        let before = crate::speakers::words::load(&folder).unwrap();
        let started = std::time::Instant::now();
        let moved = super::migrate(&pool, &meeting, &folder, |p| crate::speakers::commands::decode_16k(p)).await.unwrap();
        let map = super::load(&folder).unwrap();
        let mix = crate::speakers::commands::decode_16k(&crate::audio::retranscription::find_audio_file(&folder).unwrap()).unwrap();
        let on = super::speech_onsets(&mix);
        let starts: Vec<f64> = before.iter().flat_map(|p| p.words.iter().map(|w| w.1)).collect();
        let mapped: Vec<f64> = starts.iter().map(|t| map.map(*t)).collect();
        println!("onset lag as stored {:?}, mapped {:?}", super::onset_lag(&on, &starts), super::onset_lag(&on, &mapped));
        println!("moved {moved} in {:.1}s; map points {}", started.elapsed().as_secs_f64(), map.points.len());
        let mut d: Vec<f64> = before.iter().flat_map(|p| p.words.iter().map(|w| (map.map(w.1) - w.1).abs())).collect();
        d.sort_by(|a, b| a.total_cmp(b));
        println!("word times moved: median {:.3}s, max {:.3}s over {} words", d[d.len() / 2], d[d.len() - 1], d.len());
        for (a, b) in map.points.iter().step_by(5) {
            println!("  call {a:>8.2} → mix {b:>8.2} ({:+.3})", b - a);
        }
        // Idempotent
        assert!(!super::migrate(&pool, &meeting, &folder, |p| crate::speakers::commands::decode_16k(p)).await.unwrap());
    }
}
