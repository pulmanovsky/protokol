//! Tauri commands: recognise speakers in a saved meeting, rename a speaker
//! (which also saves the voice profile), list a meeting's speakers.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use log::{info, warn};
use serde::Serialize;
use sqlx::{Row, SqlitePool};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use super::profiles::{from_blob, match_profiles, to_blob, updated_embedding, VoiceProfile};
use super::timeline::{me_turns, overlay, ME};
use super::{speaker_label, Turn};
use crate::state::AppState;

pub const SEGMENTATION_MODEL: &str = "segmentation-3.0.onnx";
pub const EMBEDDING_MODEL: &str = "voice-embedding.onnx";
/// Label of the person at this Mac's microphone
pub const ME_LABEL: &str = "Я";

#[derive(Debug, Clone, Serialize, Default)]
pub struct MeetingSpeaker {
    pub label: String,
    pub name: Option<String>,
    /// Optional role («керівник», «керівник групи»)
    pub role: Option<String>,
    /// Name suggested from what was said, not yet confirmed
    pub suggested: Option<String>,
    pub speech_seconds: f64,
    /// When this voice is first heard (seconds)
    pub first_seconds: Option<f64>,
    /// The first few things this voice said
    pub samples: Vec<String>,
    /// A short stretch of this voice alone, for listening (seconds)
    pub clip_start: Option<f64>,
    pub clip_end: Option<f64>,
}

/// How the lines of short, unlisted voices are shown
pub const REMARKS: &str = "Репліки";
/// A voice heard for less than this (seconds) is a remark, not a speaker
const SHORT_VOICE_S: f64 = 10.0;
/// Voices this similar may be one person: offered for merging
const LIKELY_SAME: std::ops::Range<f32> = 0.55..0.65;

/// Voices with little speech and no known name
pub(crate) fn short_voices(order: &[usize], speech_s: &[f64], known: &dyn Fn(usize) -> bool) -> std::collections::HashSet<usize> {
    order.iter().copied().filter(|&k| speech_s.get(k).copied().unwrap_or(0.0) < SHORT_VOICE_S && !known(k)).collect()
}

/// Two voices of a meeting that may be one person
#[derive(Debug, Clone, Serialize)]
pub struct LikelySame {
    pub keep: String,
    pub merge: String,
    pub similarity: f32,
}

/// Pairs of voices whose centroids are close (but not close enough to be
/// joined automatically), most similar first; two different confirmed names
/// are never offered
pub async fn likely_same_of(pool: &SqlitePool, meeting_id: &str) -> Result<Vec<LikelySame>> {
    let rows = sqlx::query("SELECT label, name, centroid, speech_seconds FROM meeting_speakers WHERE meeting_id = ?").bind(meeting_id).fetch_all(pool).await?;
    let voices: Vec<(String, Option<String>, Vec<f32>, f64)> = rows
        .iter()
        .map(|r| (r.get("label"), r.get("name"), from_blob(r.get::<Vec<u8>, _>("centroid").as_slice()), r.get::<Option<f64>, _>("speech_seconds").unwrap_or(0.0)))
        .collect();
    let mut out = Vec::new();
    for a in 0..voices.len() {
        for b in a + 1..voices.len() {
            let (x, y) = (&voices[a], &voices[b]);
            if x.2.is_empty() || y.2.is_empty() || (x.1.is_some() && y.1.is_some()) {
                continue;
            }
            let similarity = super::embedding::cosine(&x.2, &y.2);
            if LIKELY_SAME.contains(&similarity) {
                // The voice with more speech (or a name) stays
                let (keep, merge) = if (x.1.is_some(), x.3) >= (y.1.is_some(), y.3) { (x, y) } else { (y, x) };
                let shown = |v: &(String, Option<String>, Vec<f32>, f64)| v.1.clone().unwrap_or_else(|| v.0.clone());
                out.push(LikelySame { keep: shown(keep), merge: shown(merge), similarity });
            }
        }
    }
    out.sort_by(|a, b| b.similarity.total_cmp(&a.similarity));
    out.truncate(5);
    Ok(out)
}

/// Longest clip offered for listening
const CLIP_S: f64 = 8.0;

#[derive(Debug, Clone, Serialize)]
pub struct SpeakersResult {
    pub speakers: Vec<MeetingSpeaker>,
    pub turns: usize,
    pub seconds: f64,
}

#[derive(Debug, Clone, Serialize)]
struct SpeakersProgress {
    meeting_id: String,
    stage: String,
    progress_percentage: u32,
}

fn emit<R: Runtime>(app: &AppHandle<R>, meeting_id: &str, stage: &str, pct: u32) {
    let _ = app.emit("speakers-progress", SpeakersProgress { meeting_id: meeting_id.into(), stage: stage.into(), progress_percentage: pct });
}

/// Folder with the two speaker models: bundled with the app, or put next to
/// the other models in the app data folder
pub fn models_dir<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    let candidates = [
        app.path().resource_dir().ok().map(|d| d.join("speaker-models")),
        app.path().app_data_dir().ok().map(|d| d.join("models").join("speakers")),
    ];
    candidates.into_iter().flatten().find(|d| d.join(SEGMENTATION_MODEL).exists() && d.join(EMBEDDING_MODEL).exists())
}

pub(crate) fn pool<R: Runtime>(app: &AppHandle<R>) -> Result<SqlitePool> {
    let state = app.try_state::<AppState>().ok_or_else(|| anyhow!("App state not available"))?;
    Ok(state.db_manager.pool().clone())
}

pub(crate) async fn profiles(pool: &SqlitePool, model: &str) -> Vec<VoiceProfile> {
    // A voice switched off in «Усі збережені» names no one
    sqlx::query("SELECT name, embedding, meetings, role FROM voice_profiles WHERE model = ? AND recognize = 1")
        .bind(model)
        .fetch_all(pool)
        .await
        .map(|rows| {
            rows.iter()
                .map(|r| VoiceProfile {
                    name: r.get("name"),
                    embedding: from_blob(r.get::<Vec<u8>, _>("embedding").as_slice()),
                    meetings: r.get("meetings"),
                    role: r.get("role"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Identifies the embedding model so profiles from another model are not mixed in
pub fn model_id(dir: &Path) -> String {
    std::fs::metadata(dir.join(EMBEDDING_MODEL)).map(|m| format!("emb-{}", m.len())).unwrap_or_else(|_| "emb".into())
}

pub(crate) fn decode_16k(path: &Path) -> Result<Vec<f32>> {
    let decoded = crate::audio::decoder::decode_audio_file(path)?;
    Ok(decoded.to_whisper_format())
}

/// Recognises who spoke in a saved meeting and rewrites its transcript by
/// speaker turns.
pub async fn identify<R: Runtime>(app: &AppHandle<R>, meeting_id: &str) -> Result<SpeakersResult> {
    let pool = pool(app)?;
    let models = models_dir(app).ok_or_else(|| anyhow!("Моделі розпізнавання спікерів не знайдено"))?;
    let engine = crate::audio::retranscription::get_or_init_whisper(app, None).await?;
    let language = crate::get_language_preference_internal();
    let id = meeting_id.to_string();
    let stop = move || is_stopping(&id);
    identify_with_stop(&pool, &models, &engine, language, meeting_id, |stage, pct| emit(app, meeting_id, stage, pct), stop).await
}

/// The work of [`identify`], without the app handle (also used by tests)
pub async fn identify_with(
    pool: &SqlitePool,
    models: &Path,
    engine: &crate::whisper_engine::WhisperEngine,
    language: Option<String>,
    meeting_id: &str,
    progress: impl Fn(&str, u32),
) -> Result<SpeakersResult> {
    identify_with_stop(pool, models, engine, language, meeting_id, progress, || false).await
}

/// [`identify_with`] that the user can stop: checked between stages, in the
/// voice stage and before every piece of text; nothing is saved when stopped
/// (the transcript and the voices are written together at the very end)
pub async fn identify_with_stop(
    pool: &SqlitePool,
    models: &Path,
    engine: &crate::whisper_engine::WhisperEngine,
    language: Option<String>,
    meeting_id: &str,
    progress: impl Fn(&str, u32),
    stop: impl Fn() -> bool + Send + Sync + Clone + 'static,
) -> Result<SpeakersResult> {
    let started = std::time::Instant::now();
    let pool = pool.clone();
    let models = models.to_path_buf();
    let folder: Option<String> = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?").bind(meeting_id).fetch_optional(&pool).await?.flatten();
    let folder = PathBuf::from(folder.ok_or_else(|| anyhow!("Для цього запису немає папки зі звуком"))?);
    let audio_path = crate::audio::retranscription::find_audio_file(&folder)?;
    // Times stored before the call/mix map existed are moved first
    if let Err(e) = super::time_map::migrate(&pool, meeting_id, &folder, |p| decode_16k(p)).await {
        warn!("Could not move the meeting to the mix's timeline: {e}");
    }

    progress("decoding", 5);
    let (mixed, system, levels) = {
        let folder = folder.clone();
        let audio_path = audio_path.clone();
        tokio::task::spawn_blocking(move || -> Result<_> {
            let mixed = decode_16k(&audio_path)?;
            let levels = crate::audio::channel_track::load_levels(&folder);
            let system_path = folder.join(crate::audio::channel_track::SYSTEM_AUDIO_FILE);
            let system = if levels.is_some() && system_path.exists() { decode_16k(&system_path).ok() } else { None };
            Ok((mixed, system, levels))
        })
        .await??
    };

    // With the channels kept, the call is diarized alone and the microphone is «Я»
    progress("diarizing", 15);
    let seg_model = models.join(super::commands::SEGMENTATION_MODEL);
    let emb_model = models.join(EMBEDDING_MODEL);
    let call_audio = system.clone().unwrap_or_else(|| mixed.clone());
    // Voices: the slow part before the text; its progress goes 15 → 30 %
    let (tx, rx) = std::sync::mpsc::channel::<f32>();
    if stop() {
        return Err(anyhow!(super::diarize::STOPPED));
    }
    let stop_voices = stop.clone();
    let mut job = tokio::task::spawn_blocking(move || {
        super::diarize::diarize_cancellable(
            &call_audio,
            &seg_model,
            &emb_model,
            super::diarize::DEFAULT_CLUSTER_THRESHOLD,
            &|f| {
                let _ = tx.send(f);
            },
            &stop_voices,
        )
    });
    let mut last = 15;
    let diarization = loop {
        tokio::select! {
            done = &mut job => break done??,
            _ = tokio::time::sleep(std::time::Duration::from_millis(700)) => {
                let mut f = None;
                while let Ok(v) = rx.try_recv() {
                    f = Some(v);
                }
                if let Some(f) = f {
                    let pct = 15 + (f.clamp(0.0, 1.0) * 15.0) as u32;
                    if pct > last {
                        last = pct;
                        progress("diarizing", pct);
                    }
                }
            }
        }
    };
    // The call recording's clock drifts from the mix the player plays: what
    // is found in it is put on the mix's timeline
    let time_map = match &system {
        Some(call) => {
            let (m, c) = (mixed.clone(), call.clone());
            let map = tokio::task::spawn_blocking(move || super::time_map::fit(&m, &c)).await?;
            super::time_map::save(&folder, &map);
            Some(map).filter(|m| !m.is_identity())
        }
        None => None,
    };
    let mut turns: Vec<Turn> = match (&levels, &system) {
        (Some(levels), Some(_)) => overlay(&diarization.turns, &me_turns(levels)),
        _ => diarization.turns.clone(),
    };

    // Labels: «Спікер N» numbered by first appearance; known voices get names
    let model = model_id(&models);
    // Settings → Пам'ять → «Упізнавати збережені голоси» off: no saved names
    let known_profiles = if voice_recognition_on(&pool).await { profiles(&pool, &model).await } else { Vec::new() };
    let known = match_profiles(&diarization.centroids, &known_profiles);
    let role_of = |k: usize| -> Option<String> {
        let name = known.get(k).cloned().flatten()?;
        let name = name.trim_end_matches('?');
        known_profiles.iter().find(|p| p.name == name).and_then(|p| p.role.clone())
    };
    let mut order: Vec<usize> = Vec::new();
    for t in &turns {
        if t.speaker != ME && !order.contains(&t.speaker) {
            order.push(t.speaker);
        }
    }
    // Voices heard for a few seconds only (a remark from the room) are not
    // listed one by one: their lines are shown as «Репліки»
    let short: std::collections::HashSet<usize> =
        short_voices(&order, &diarization.speech_s, &|k| known.get(k).cloned().flatten().is_some());
    order.retain(|k| !short.contains(k));
    let label_of = |k: usize| -> String {
        if k == ME {
            return ME_LABEL.to_string();
        }
        let n = order.iter().position(|&o| o == k).unwrap_or(k);
        speaker_label(n)
    };
    let display_of = |k: usize| -> String {
        if k == ME {
            return ME_LABEL.to_string();
        }
        if short.contains(&k) {
            return REMARKS.to_string();
        }
        known.get(k).cloned().flatten().unwrap_or_else(|| label_of(k))
    };

    refresh_speaker_terms(&pool).await;
    // The text of the import with its word times: each word goes to the
    // voice heard then, nothing is transcribed again
    let current: Vec<String> =
        sqlx::query_scalar("SELECT transcript FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time").bind(meeting_id).fetch_all(&pool).await?;
    let mut kept_words = super::words::load(&folder).filter(|w| super::words::matches_transcript(w, &current));
    // An import: its words placed by the sound now, on more threads than the
    // background pass would (nobody is listening yet); any failure keeps them as they are
    if let (Some(words), Some(dir)) = (kept_words.as_mut(), super::aligner::for_processing(&pool, &models).await) {
        if words.iter().any(|p| !p.aligned && !p.words.is_empty()) && !crate::audio::recording_commands::recording_now() {
            progress("aligning", 95);
            let (path, map, stop2, mut taken) = (audio_path.clone(), time_map.clone(), stop.clone(), words.clone());
            let joined = tokio::task::spawn_blocking(move || {
                let Ok(mix) = super::aligner::player_mix(&path) else { return (taken, 0) };
                let n = match super::aligner::Aligner::open_with(&dir, super::aligner::processing_threads()) {
                    Ok(mut a) => super::aligner::align_pieces(&mut a, &mix, &mut taken, map.as_ref(), &|_, _| {}, &|| stop2() || crate::audio::recording_commands::recording_now()),
                    Err(e) => {
                        warn!("Word aligner not available: {e}");
                        0
                    }
                };
                (taken, n)
            })
            .await;
            if let Ok((aligned, n)) = joined {
                if n > 0 {
                    info!("Imported words aligned to the sound in {n} piece(s)");
                    if let Err(e) = super::words::save(&folder, &aligned) {
                        warn!("Could not save word times: {e}");
                    }
                    *words = aligned;
                }
            }
        }
    }
    let word_rows = kept_words.as_ref().map(|w| super::words::assign(w, &turns).into_iter().map(|(t, a, b, k)| (t, a * 1000.0, b * 1000.0, k)).collect::<Vec<_>>());
    // Once the lines are made again from these word times, they share one timeline
    let one_timeline = kept_words.filter(|w| w.iter().any(|p| !p.heard.is_empty()));
    if word_rows.is_some() {
        info!("Speakers attached to the existing text by word times");
    }
    // Otherwise (a live recording, an edited or re-transcribed text): one
    // pass of Whisper over the recording with word times, as on import,
    // kept for the next time
    let rows: Vec<(String, f64, f64, usize)> = match word_rows {
        Some(rows) => rows,
        None => {
            progress("transcribing", 30);
            // The call and the user's own speech transcribed apart (recordings)
            let segments = super::words::channel_pieces_tagged(&mixed, system.as_deref(), levels.as_ref())?;
            let mut pieces: Vec<super::words::PieceWords> = Vec::new();
            let mut from_call: Vec<bool> = Vec::new();
            let mut previous: Option<String> = None;
            for (i, (seg, call)) in segments.iter().enumerate() {
                if stop() {
                    return Err(anyhow!(super::diarize::STOPPED));
                }
                if i % 5 == 0 {
                    progress("transcribing", 30 + (65 * i / segments.len().max(1)) as u32);
                }
                let Some(piece) =
                    super::words::transcribe_piece(engine, &seg.samples, seg.start_timestamp_ms / 1000.0, seg.end_timestamp_ms / 1000.0, language.clone()).await?
                else {
                    continue;
                };
                if crate::ukrainian::hallucination::is_repeated_segment(previous.as_deref(), &piece.text) {
                    continue;
                }
                previous = Some(piece.text.clone());
                pieces.push(piece);
                from_call.push(*call);
            }
            // Each word placed where it sounds in the recording (a speech
            // model's forced alignment); any failure keeps Whisper's times
            let mut aligned = 0usize;
            let whisper_times: Vec<Vec<f64>> = pieces.iter().map(|p| p.words.iter().map(|w| w.1).collect()).collect();
            if let Some(dir) = super::aligner::for_processing(&pool, &models).await {
                if !crate::audio::recording_commands::recording_now() {
                    progress("aligning", 95);
                    let (path, map, stop2) = (audio_path.clone(), time_map.clone(), stop.clone());
                    let mut taken = pieces.clone();
                    let joined = tokio::task::spawn_blocking(move || {
                        // The player's timeline (the app's decoder drifts from it)
                        let Ok(mix) = super::aligner::player_mix(&path) else { return (taken, 0) };
                        // Nobody is listening yet: more threads than the background pass
                        let n = match super::aligner::Aligner::open_with(&dir, super::aligner::processing_threads()) {
                            Ok(mut a) => Some(super::aligner::align_pieces(&mut a, &mix, &mut taken, map.as_ref(), &|_, _| {}, &|| stop2() || crate::audio::recording_commands::recording_now())),
                            Err(e) => {
                                warn!("Word aligner not available: {e}");
                                None
                            }
                        };
                        (taken, n.unwrap_or(0))
                    })
                    .await;
                    match joined {
                        Ok((back, n)) => {
                            pieces = back;
                            aligned = n;
                        }
                        Err(e) => {
                            // Only a panic gets here: Whisper's times are kept
                            warn!("Word alignment stopped: {e}");
                        }
                    }
                    info!("Words aligned to the sound in {aligned} of {} piece(s)", pieces.len());
                    // The lines are made from these times: one timeline
                    pieces.iter_mut().for_each(|p| p.heard.clear());
                }
            }
            // The call's clock drifts from the mix: its words and voices go on
            // the mix's timeline, but only if that clearly meets the mix's
            // speech better (decided on what was heard, not assumed)
            if let Some(map) = &time_map {
                let mix = mixed.clone();
                let onsets = tokio::task::spawn_blocking(move || super::time_map::speech_onsets(&mix)).await?;
                let better = if aligned * 2 > pieces.len() {
                    // The aligned words show where the call's speech is on the
                    // mix: the map is taken if it brings Whisper's call times closer
                    let (mut raw, mut mapped) = (Vec::new(), Vec::new());
                    for ((p, c), was) in pieces.iter().zip(&from_call).zip(&whisper_times) {
                        if *c && p.aligned {
                            for (w, t) in p.words.iter().zip(was) {
                                raw.push((w.1 - t).abs());
                                mapped.push((w.1 - map.map(*t)).abs());
                            }
                        }
                    }
                    let median = |mut v: Vec<f64>| -> f64 {
                        v.sort_by(|a, b| a.total_cmp(b));
                        v.get(v.len() / 2).copied().unwrap_or(0.0)
                    };
                    let (r, m) = (median(raw.clone()), median(mapped));
                    info!("Call words vs the sound: {r:.2}s as heard, {m:.2}s mapped ({} words)", raw.len());
                    raw.len() >= 20 && m + 0.15 <= r
                } else {
                    let before: Vec<f64> = pieces.iter().flat_map(|p| p.words.iter().map(|w| w.1)).collect();
                    let after: Vec<f64> = pieces.iter().zip(&from_call).flat_map(|(p, c)| p.words.iter().map(move |w| if *c && !p.aligned { map.map(w.1) } else { w.1 })).collect();
                    super::time_map::map_is_better(&onsets, &before, &after)
                };
                let _ = crate::database::repositories::setting::SettingsRepository::set_flag(&pool, &format!("time_map_applied:{meeting_id}"), if better { "new-mapped" } else { "new-mix" }).await;
                if better {
                    for (p, c) in pieces.iter_mut().zip(&from_call) {
                        if *c && !p.aligned {
                            p.start = map.map(p.start);
                            p.end = map.map(p.end);
                            p.words.iter_mut().for_each(|w| w.1 = map.map(w.1));
                        }
                    }
                    let mapped: Vec<Turn> = diarization.turns.iter().map(|t| Turn { start: map.map(t.start), end: map.map(t.end), speaker: t.speaker }).collect();
                    turns = match (&levels, &system) {
                        (Some(levels), Some(_)) => overlay(&mapped, &me_turns(levels)),
                        _ => mapped,
                    };
                }
            }
            if let Err(e) = super::words::save(&folder, &pieces) {
                warn!("Could not save word times: {e}");
            }
            super::words::assign(&pieces, &turns).into_iter().map(|(t, a, b, k)| (t, a * 1000.0, b * 1000.0, k)).collect()
        }
    };
    if rows.is_empty() {
        return Err(anyhow!("Не вдалося розпізнати мовлення"));
    }

    // Names from what was said (introductions, addresses), as suggestions
    // for voices that have no known name
    let mut turn_texts: Vec<(usize, String)> = Vec::new();
    for (text, _, _, k) in &rows {
        match turn_texts.last_mut() {
            Some((last, t)) if last == k => {
                t.push(' ');
                t.push_str(text);
            }
            _ => turn_texts.push((*k, text.clone())),
        }
    }
    let turns_for_names: Vec<super::names::TurnText> = turn_texts.iter().map(|(k, t)| super::names::TurnText { speaker: *k, text: t }).collect();
    let suggested = super::names::suggest(&turns_for_names, &|k| k == ME || short.contains(&k) || known.get(k).cloned().flatten().is_some());
    info!("Speaker names suggested from the transcript: {}", suggested.len());
    let shown_of = |k: usize| -> String {
        match suggested.get(&k) {
            Some(name) => super::names::unconfirmed(name),
            None => display_of(k),
        }
    };

    // Save: transcript rows with speakers, the meeting's voices
    if stop() {
        return Err(anyhow!(super::diarize::STOPPED));
    }
    progress("saving", 95);
    let segments = crate::audio::common::create_transcript_segments(&rows.iter().map(|(t, s, e, _)| (t.clone(), *s, *e)).collect::<Vec<_>>());
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?").bind(meeting_id).execute(&mut *tx).await?;
    for (seg, (_, _, _, k)) in segments.iter().zip(&rows) {
        let speaker = shown_of(*k);
        sqlx::query(
            "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&seg.id)
        .bind(meeting_id)
        .bind(&seg.text)
        .bind(&seg.timestamp)
        .bind(seg.audio_start_time)
        .bind(seg.audio_end_time)
        .bind(seg.duration)
        .bind(&speaker)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("DELETE FROM meeting_speakers WHERE meeting_id = ?").bind(meeting_id).execute(&mut *tx).await?;
    let mut speakers: Vec<MeetingSpeaker> = Vec::new();
    for &k in &order {
        let name = known.get(k).cloned().flatten();
        let speech = diarization.speech_s.get(k).copied().unwrap_or(0.0);
        sqlx::query("INSERT INTO meeting_speakers (meeting_id, label, name, centroid, speech_seconds, role, suggested) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(meeting_id)
            .bind(label_of(k))
            .bind(&name)
            .bind(to_blob(&diarization.centroids[k]))
            .bind(speech)
            .bind(role_of(k))
            .bind(suggested.get(&k))
            .execute(&mut *tx)
            .await?;
        speakers.push(MeetingSpeaker { label: label_of(k), name, role: role_of(k), suggested: suggested.get(&k).cloned(), speech_seconds: speech, ..Default::default() });
    }
    tx.commit().await?;
    if let Some(mut w) = one_timeline {
        w.iter_mut().for_each(|p| p.heard.clear());
        if let Err(e) = super::words::save(&folder, &w) {
            warn!("Could not save word times: {e}");
        }
    }
    // Voices inside lines checked now, while processing (kept with the meeting,
    // so opening it later costs nothing)
    match super::lines::voice_changes(&pool, &models, meeting_id).await {
        Ok(found) => info!("Voice changes inside lines: {}", found.len()),
        Err(e) => warn!("Voice changes not checked: {e}"),
    }
    // A new recognition wrote every line: undo history starts over
    if let Err(e) = super::journal::clear(&pool, meeting_id).await {
        warn!("Could not clear the undo history: {e}");
    }
    // Lines cut or joined by hand, and checked ones, as before
    match super::lines::apply(&pool, meeting_id).await {
        Ok((edits, reviewed)) if edits + reviewed > 0 => info!("Line cuts/joins applied again: {edits}; checked lines: {reviewed}"),
        Ok(_) => {}
        Err(e) => warn!("Could not apply line cuts and checks: {e}"),
    }
    // Lines the user gave to another speaker by hand keep that speaker
    match super::overrides::apply(&pool, meeting_id).await {
        Ok(n) if n > 0 => info!("Speakers set by hand applied again: {n} line(s)"),
        Ok(_) => {}
        Err(e) => warn!("Could not apply speakers set by hand: {e}"),
    }
    if let Err(e) = crate::audio::common::write_transcripts_json(&folder, &segments) {
        warn!("Could not update transcripts.json: {e}");
    }
    let seconds = started.elapsed().as_secs_f64();
    info!("Speakers recognised: {} voice(s), {} piece(s), {:.1}s", speakers.len(), rows.len(), seconds);
    progress("complete", 100);
    // The timeline's waveform, built now in the background (not while recording)
    crate::waveform::build_later(folder.to_path_buf());
    Ok(SpeakersResult { speakers, turns: turns.len(), seconds })
}

/// Settings → Пам'ять → «Упізнавати збережені голоси» (on unless switched off)
pub(crate) async fn voice_recognition_on(pool: &SqlitePool) -> bool {
    crate::database::repositories::setting::SettingsRepository::get_flag(pool, "voice_recognition").await.ok().flatten().as_deref() != Some("off")
}

#[tauri::command]
pub async fn api_voice_recognition_get<R: Runtime>(app: AppHandle<R>) -> Result<bool, String> {
    let pool = pool(&app).map_err(|e| e.to_string())?;
    Ok(voice_recognition_on(&pool).await)
}

#[tauri::command]
pub async fn api_voice_recognition_set<R: Runtime>(app: AppHandle<R>, on: bool) -> Result<(), String> {
    let pool = pool(&app).map_err(|e| e.to_string())?;
    crate::database::repositories::setting::SettingsRepository::set_flag(&pool, "voice_recognition", if on { "on" } else { "off" })
        .await
        .map_err(|e| e.to_string())
}

/// Gives a speaker of a meeting a name: the transcript is relabelled and the
/// voice is remembered for later meetings (except for «Я» and «?» names).
pub async fn rename<R: Runtime>(app: &AppHandle<R>, meeting_id: &str, current: &str, name: &str) -> Result<()> {
    let pool = pool(app)?;
    let model = models_dir(app).map(|d| model_id(&d)).unwrap_or_else(|| "emb".into());
    rename_with(&pool, &model, meeting_id, current, name).await
}

/// The work of [`rename`]; `model` identifies the embedding model of profiles
pub async fn rename_with(pool: &SqlitePool, model: &str, meeting_id: &str, current: &str, name: &str) -> Result<()> {
    let name = name.trim().trim_end_matches("(?)").trim_end_matches('?').trim();
    if name.is_empty() || name == current {
        return Ok(());
    }
    let pool = pool.clone();
    // A name given by hand is offered again, even one set aside before
    super::saved::unhide(&pool, name).await?;
    // «Ім'я (?)» is a suggestion of a voice: find it by its suggested name
    let lookup = current.trim_end_matches("(?)").trim();
    sqlx::query("UPDATE transcripts SET speaker = ? WHERE meeting_id = ? AND speaker = ?").bind(name).bind(meeting_id).bind(current).execute(&pool).await?;
    // Which voice is it: by its label or its current name
    let row = sqlx::query("SELECT label, centroid FROM meeting_speakers WHERE meeting_id = ? AND (label = ? OR name = ? OR suggested = ?)")
        .bind(meeting_id)
        .bind(current)
        .bind(current)
        .bind(lookup)
        .fetch_optional(&pool)
        .await?;
    let Some(row) = row else { return Ok(()) };
    let label: String = row.get("label");
    let centroid = from_blob(row.get::<Vec<u8>, _>("centroid").as_slice());
    sqlx::query("UPDATE meeting_speakers SET name = ?, suggested = NULL WHERE meeting_id = ? AND label = ?").bind(name).bind(meeting_id).bind(&label).execute(&pool).await?;
    if label == ME_LABEL || centroid.is_empty() || !voice_recognition_on(&pool).await {
        return Ok(());
    }
    let existing = sqlx::query("SELECT embedding, meetings FROM voice_profiles WHERE name = ? AND model = ?").bind(name).bind(model).fetch_optional(&pool).await?;
    let (embedding, meetings) = match existing {
        Some(r) => {
            let old = from_blob(r.get::<Vec<u8>, _>("embedding").as_slice());
            let n: i64 = r.get("meetings");
            (updated_embedding(&old, n, &centroid), n + 1)
        }
        None => (centroid, 1),
    };
    sqlx::query(
        "INSERT INTO voice_profiles (name, embedding, model, meetings, updated_at) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(name) DO UPDATE SET embedding = excluded.embedding, model = excluded.model, meetings = excluded.meetings, updated_at = excluded.updated_at",
    )
    .bind(name)
    .bind(to_blob(&embedding))
    .bind(model)
    .bind(meetings)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&pool)
    .await?;
    info!("Voice profile saved ({} meeting(s))", meetings);
    Ok(())
}

static STOPPING: once_cell::sync::Lazy<std::sync::Mutex<std::collections::HashSet<String>>> =
    once_cell::sync::Lazy::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

fn is_stopping(meeting_id: &str) -> bool {
    STOPPING.lock().map(|s| s.contains(meeting_id)).unwrap_or(false)
}

/// Stops speaker recognition of a meeting; automatic recognition will not
/// start it again (the user can start it from «Спікери»)
#[tauri::command]
pub async fn stop_speaker_recognition<R: Runtime>(app: AppHandle<R>, meeting_id: String) -> Result<(), String> {
    if let Ok(mut s) = STOPPING.lock() {
        s.insert(meeting_id.clone());
    }
    let pool = pool(&app).map_err(|e| e.to_string())?;
    crate::database::repositories::setting::SettingsRepository::set_flag(&pool, &format!("speakers_auto_stopped:{meeting_id}"), "1")
        .await
        .map_err(|e| e.to_string())
}

static RUNNING: once_cell::sync::Lazy<std::sync::Mutex<std::collections::HashSet<String>>> =
    once_cell::sync::Lazy::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

#[derive(Debug, Clone, Serialize)]
struct SpeakersDone {
    meeting_id: String,
    ok: bool,
    error: Option<String>,
    speakers: usize,
    suggested: usize,
}

/// Starts speaker recognition in the background; progress comes as
/// «speakers-progress» events and the end as «speakers-done»
#[tauri::command]
pub async fn start_speaker_recognition<R: Runtime>(app: AppHandle<R>, meeting_id: String) -> Result<(), String> {
    {
        let mut running = RUNNING.lock().map_err(|e| e.to_string())?;
        if !running.insert(meeting_id.clone()) {
            return Ok(());
        }
    }
    // A new start (by hand or automatic) clears an earlier «Зупинити»
    if let Ok(mut s) = STOPPING.lock() {
        s.remove(&meeting_id);
    }
    if let Ok(pool) = pool(&app) {
        let _ = sqlx::query("DELETE FROM app_flags WHERE key = ?").bind(format!("speakers_auto_stopped:{meeting_id}")).execute(&pool).await;
    }
    tauri::async_runtime::spawn(async move {
        let result = identify(&app, &meeting_id).await;
        // Marked done only when it finished: a run cut short (the app was
        // closed) starts again next time the meeting is opened
        if result.is_ok() {
            if let Ok(pool) = pool(&app) {
                let _ = crate::database::repositories::setting::SettingsRepository::set_flag(&pool, &format!("speakers_auto_done:{meeting_id}"), "1").await;
            }
        }
        if let Ok(mut running) = RUNNING.lock() {
            running.remove(&meeting_id);
        }
        if let Ok(mut s) = STOPPING.lock() {
            s.remove(&meeting_id);
        }
        let done = match result {
            Ok(r) => SpeakersDone {
                meeting_id: meeting_id.clone(),
                ok: true,
                error: None,
                speakers: r.speakers.len(),
                suggested: r.speakers.iter().filter(|s| s.suggested.is_some()).count(),
            },
            Err(e) => {
                warn!("Speaker recognition failed: {e}");
                SpeakersDone { meeting_id: meeting_id.clone(), ok: false, error: Some(e.to_string()), speakers: 0, suggested: 0 }
            }
        };
        let ready = done.ok.then(|| done.suggested);
        let _ = app.emit("speakers-done", done);
        if let Some(unsure) = ready {
            if let Ok(pool) = pool(&app) {
                let title: Option<String> = sqlx::query_scalar("SELECT title FROM meetings WHERE id = ?")
                    .bind(&meeting_id)
                    .fetch_optional(&pool)
                    .await
                    .ok()
                    .flatten();
                if let Some(title) = title {
                    crate::notifications::commands::notify_when_behind(
                        &app,
                        crate::notifications::types::Notification::transcript_ready(&title, unsure),
                    )
                    .await;
                }
            }
        }
    });
    Ok(())
}

/// Setting: write the meeting's document on its own after speaker
/// recognition (off by default)
#[tauri::command]
pub async fn get_auto_document<R: Runtime>(app: AppHandle<R>) -> bool {
    let Ok(pool) = pool(&app) else { return false };
    crate::database::repositories::setting::SettingsRepository::get_flag(&pool, "auto_document_after_speakers").await.ok().flatten().as_deref() == Some("1")
}

#[tauri::command]
pub async fn set_auto_document<R: Runtime>(app: AppHandle<R>, enabled: bool) -> Result<(), String> {
    let pool = pool(&app).map_err(|e| e.to_string())?;
    crate::database::repositories::setting::SettingsRepository::set_flag(&pool, "auto_document_after_speakers", if enabled { "1" } else { "0" })
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn speaker_recognition_running(meeting_id: String) -> bool {
    RUNNING.lock().map(|r| r.contains(&meeting_id)).unwrap_or(false)
}

/// Whether to recognise speakers on its own for this meeting: "run",
/// "needs_models" (ask once to download), or "skip" (done before, already
/// has speakers, or a meeting from before automatic recognition)
#[tauri::command]
pub async fn speaker_auto_status<R: Runtime>(app: AppHandle<R>, meeting_id: String) -> Result<String, String> {
    use crate::database::repositories::setting::SettingsRepository;
    let pool = pool(&app).map_err(|e| e.to_string())?;
    // Automatic recognition starts with this version: older meetings are left alone
    let since = match SettingsRepository::get_flag(&pool, "speakers_auto_since").await.map_err(|e| e.to_string())? {
        Some(v) => v,
        None => {
            let now = chrono::Utc::now().to_rfc3339();
            SettingsRepository::set_flag(&pool, "speakers_auto_since", &now).await.map_err(|e| e.to_string())?;
            now
        }
    };
    let created: Option<String> = sqlx::query_scalar("SELECT created_at FROM meetings WHERE id = ?").bind(&meeting_id).fetch_optional(&pool).await.map_err(|e| e.to_string())?;
    let Some(created) = created else { return Ok("skip".into()) };
    let newer = match (chrono::DateTime::parse_from_rfc3339(&created), chrono::DateTime::parse_from_rfc3339(&since)) {
        // Meetings from the day before the update too (e.g. an import just before)
        (Ok(c), Ok(s)) => c >= s - chrono::Duration::days(1),
        _ => false,
    };
    let done = SettingsRepository::get_flag(&pool, &format!("speakers_auto_done:{meeting_id}")).await.map_err(|e| e.to_string())?.is_some();
    // The user stopped it: do not start it again on its own
    if SettingsRepository::get_flag(&pool, &format!("speakers_auto_stopped:{meeting_id}")).await.map_err(|e| e.to_string())?.is_some() {
        return Ok("skip".into());
    }
    let has: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meeting_speakers WHERE meeting_id = ?").bind(&meeting_id).fetch_one(&pool).await.map_err(|e| e.to_string())?;
    // «Done» without any voices saved means a run was cut short (older
    // versions marked it at the start): run again
    let finished = done && has > 0;
    if !newer || finished || has > 0 {
        return Ok("skip".into());
    }
    Ok(if models_dir(&app).is_some() { "run".into() } else { "needs_models".into() })
}

#[tauri::command]
pub async fn identify_speakers<R: Runtime>(app: AppHandle<R>, meeting_id: String) -> Result<SpeakersResult, String> {
    identify(&app, &meeting_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rename_speaker<R: Runtime>(app: AppHandle<R>, meeting_id: String, current: String, name: String) -> Result<(), String> {
    let p = pool(&app).map_err(|e| e.to_string())?;
    let label = format!("ім'я: {}", name.trim());
    let result = crate::speakers::journal::journaled(&p, &meeting_id, &label, || async { rename(&app, &meeting_id, &current, &name).await.map_err(|e| e.to_string()) }).await;
    refresh_speaker_terms(&p).await;
    result
}

#[tauri::command]
pub async fn speaker_models_available<R: Runtime>(app: AppHandle<R>) -> bool {
    models_dir(&app).is_some()
}

/// A meeting's voices with what helps to tell them apart: talk time, first
/// appearance, first lines and a clip, most talkative first
pub async fn speakers_of(pool: &SqlitePool, meeting_id: &str) -> Result<Vec<MeetingSpeaker>> {
    let rows = sqlx::query("SELECT label, name, role, suggested FROM meeting_speakers WHERE meeting_id = ?").bind(meeting_id).fetch_all(pool).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let label: String = r.get("label");
        let name: Option<String> = r.get("name");
        let role: Option<String> = r.get("role");
        let suggested: Option<String> = r.get("suggested");
        let shown = name.clone().or_else(|| suggested.as_deref().map(super::names::unconfirmed)).unwrap_or_else(|| label.clone());
        let lines = sqlx::query("SELECT transcript, audio_start_time, audio_end_time FROM transcripts WHERE meeting_id = ? AND speaker = ? ORDER BY audio_start_time")
            .bind(meeting_id)
            .bind(&shown)
            .fetch_all(pool)
            .await?;
        let mut sp = MeetingSpeaker { label, name, role, suggested, ..Default::default() };
        let mut longest = (0.0f64, 0.0f64, 0.0f64);
        for l in &lines {
            let (a, b): (Option<f64>, Option<f64>) = (l.get("audio_start_time"), l.get("audio_end_time"));
            let (Some(a), Some(b)) = (a, b) else { continue };
            sp.speech_seconds += (b - a).max(0.0);
            sp.first_seconds.get_or_insert(a);
            if b - a > longest.0 {
                longest = (b - a, a, b);
            }
            if sp.samples.len() < 3 {
                let text: String = l.get("transcript");
                let short: String = text.chars().take(160).collect();
                sp.samples.push(if text.chars().count() > 160 { format!("{short}…") } else { short });
            }
        }
        if longest.0 > 0.0 {
            // The middle of the longest stretch: the voice is settled there
            let start = longest.1 + ((longest.0 - CLIP_S) / 2.0).max(0.0);
            sp.clip_start = Some(start);
            sp.clip_end = Some((start + CLIP_S).min(longest.2));
        }
        // A voice with no recognised words is nothing to name
        if !sp.samples.is_empty() {
            out.push(sp);
        }
    }
    out.sort_by(|a, b| b.speech_seconds.total_cmp(&a.speech_seconds));
    Ok(out)
}

/// Joins two voices of a meeting that are one person: `merge` becomes `keep`
pub async fn merge_with(pool: &SqlitePool, meeting_id: &str, keep: &str, merge: &str) -> Result<()> {
    if keep == merge {
        return Ok(());
    }
    let row = |who: &str| {
        sqlx::query("SELECT label, name, centroid, speech_seconds FROM meeting_speakers WHERE meeting_id = ? AND (label = ? OR name = ?)")
            .bind(meeting_id.to_string())
            .bind(who.to_string())
            .bind(who.to_string())
    };
    let a = row(keep).fetch_optional(pool).await?.ok_or_else(|| anyhow!("Спікера {keep} не знайдено"))?;
    let b = row(merge).fetch_optional(pool).await?.ok_or_else(|| anyhow!("Спікера {merge} не знайдено"))?;
    let (la, lb): (String, String) = (a.get("label"), b.get("label"));
    let shown_a: String = a.get::<Option<String>, _>("name").unwrap_or_else(|| la.clone());
    let shown_b: String = b.get::<Option<String>, _>("name").unwrap_or_else(|| lb.clone());
    let (ca, cb) = (from_blob(a.get::<Vec<u8>, _>("centroid").as_slice()), from_blob(b.get::<Vec<u8>, _>("centroid").as_slice()));
    let (sa, sb): (f64, f64) = (a.get("speech_seconds"), b.get("speech_seconds"));
    let centroid = if ca.len() == cb.len() && !ca.is_empty() {
        let (wa, wb) = (sa.max(1.0) as f32, sb.max(1.0) as f32);
        let mut v: Vec<f32> = ca.iter().zip(&cb).map(|(x, y)| (x * wa + y * wb) / (wa + wb)).collect();
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if n > 0.0 {
            v.iter_mut().for_each(|x| *x /= n);
        }
        v
    } else {
        ca
    };
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE transcripts SET speaker = ? WHERE meeting_id = ? AND speaker = ?").bind(&shown_a).bind(meeting_id).bind(&shown_b).execute(&mut *tx).await?;
    sqlx::query("UPDATE meeting_speakers SET centroid = ?, speech_seconds = ? WHERE meeting_id = ? AND label = ?")
        .bind(to_blob(&centroid))
        .bind(sa + sb)
        .bind(meeting_id)
        .bind(&la)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM meeting_speakers WHERE meeting_id = ? AND label = ?").bind(meeting_id).bind(&lb).execute(&mut *tx).await?;
    tx.commit().await?;
    info!("Merged two voices of a meeting");
    Ok(())
}

/// Sets the role of a meeting's voice; a named voice keeps it in its profile
pub async fn set_role_with(pool: &SqlitePool, meeting_id: &str, current: &str, role: &str) -> Result<()> {
    let role = role.trim();
    let role = (!role.is_empty()).then_some(role);
    sqlx::query("UPDATE meeting_speakers SET role = ? WHERE meeting_id = ? AND (label = ? OR name = ?)")
        .bind(role)
        .bind(meeting_id)
        .bind(current)
        .bind(current)
        .execute(pool)
        .await?;
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM meeting_speakers WHERE meeting_id = ? AND (label = ? OR name = ?)")
        .bind(meeting_id)
        .bind(current)
        .bind(current)
        .fetch_optional(pool)
        .await?
        .flatten();
    if let Some(name) = name {
        sqlx::query("UPDATE voice_profiles SET role = ? WHERE name = ?").bind(role).bind(name).execute(pool).await?;
    }
    Ok(())
}

/// Facts for the meeting document: regular participants (named voices heard
/// in at least two meetings) and roles of this meeting's voices
pub async fn meeting_facts(pool: &SqlitePool, meeting_id: &str) -> String {
    let regulars: Vec<String> = sqlx::query_scalar("SELECT name FROM voice_profiles WHERE meetings >= 2 AND name NOT IN (SELECT name FROM hidden_speaker_names) ORDER BY name")
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    let roles: Vec<(Option<String>, String, Option<String>)> = sqlx::query_as("SELECT name, label, role FROM meeting_speakers WHERE meeting_id = ? AND role IS NOT NULL AND role != ''")
        .bind(meeting_id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    let mut facts = String::new();
    if !regulars.is_empty() {
        facts.push_str(&format!("\nПостійні учасники: {}.", regulars.join(", ")));
    }
    let roles: Vec<String> = roles
        .into_iter()
        .filter_map(|(name, label, role)| Some(format!("{} — {}", name.unwrap_or(label), role?)))
        .collect();
    if !roles.is_empty() {
        facts.push_str(&format!("\nРолі: {}.", roles.join("; ")));
    }
    facts
}

/// A suggested name was wrong: the voice goes back to «Спікер N»
pub async fn reject_suggestion_with(pool: &SqlitePool, meeting_id: &str, current: &str) -> Result<()> {
    let lookup = current.trim_end_matches("(?)").trim();
    let label: Option<String> = sqlx::query_scalar("SELECT label FROM meeting_speakers WHERE meeting_id = ? AND suggested = ?")
        .bind(meeting_id)
        .bind(lookup)
        .fetch_optional(pool)
        .await?;
    let Some(label) = label else { return Ok(()) };
    sqlx::query("UPDATE transcripts SET speaker = ? WHERE meeting_id = ? AND speaker = ?").bind(&label).bind(meeting_id).bind(current).execute(pool).await?;
    sqlx::query("UPDATE meeting_speakers SET suggested = NULL WHERE meeting_id = ? AND label = ?").bind(meeting_id).bind(&label).execute(pool).await?;
    Ok(())
}

#[tauri::command]
pub async fn reject_speaker_suggestion<R: Runtime>(app: AppHandle<R>, meeting_id: String, current: String) -> Result<(), String> {
    let pool = pool(&app).map_err(|e| e.to_string())?;
    reject_suggestion_with(&pool, &meeting_id, &current).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_speaker_role<R: Runtime>(app: AppHandle<R>, meeting_id: String, current: String, role: String) -> Result<(), String> {
    let pool = pool(&app).map_err(|e| e.to_string())?;
    let result = crate::speakers::journal::journaled(&pool, &meeting_id, "роль", || async { set_role_with(&pool, &meeting_id, &current, &role).await.map_err(|e| e.to_string()) }).await;
    refresh_speaker_terms(&pool).await;
    result
}

#[tauri::command]
pub async fn get_meeting_speakers<R: Runtime>(app: AppHandle<R>, meeting_id: String) -> Result<Vec<MeetingSpeaker>, String> {
    let pool = pool(&app).map_err(|e| e.to_string())?;
    speakers_of(&pool, &meeting_id).await.map_err(|e| e.to_string())
}

/// Confirmed names and roles go into Whisper's prompt (see
/// `ukrainian::glossary_terms`): nicknames are recognised better next time
pub async fn refresh_speaker_terms(pool: &SqlitePool) {
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT name, role FROM voice_profiles WHERE name NOT IN (SELECT name FROM hidden_speaker_names) ORDER BY meetings DESC, updated_at DESC").fetch_all(pool).await.unwrap_or_default();
    let mut terms: Vec<String> = Vec::new();
    for (name, role) in rows {
        terms.push(name);
        if let Some(role) = role {
            terms.push(role);
        }
    }
    crate::ukrainian::set_speaker_terms(terms);
}

/// A place where one voice's lines probably hold another person: a report
/// closed («…в мене все») and a new one opened («Доброго ранку…») by the same
/// voice
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OtherVoice {
    pub speaker: String,
    pub from_seconds: f64,
    pub to_seconds: f64,
    pub preview: String,
}

fn other_voices_in(rows: &[(String, Option<String>, f64, f64)]) -> Vec<OtherVoice> {
    let mut out = Vec::new();
    for i in 1..rows.len() {
        let (text, speaker, start, _) = &rows[i];
        let Some(who) = speaker.as_ref() else { continue };
        if rows[i - 1].1.as_ref() != Some(who) || !crate::summary::materials::opens_new_report(&rows[i - 1].0, text) {
            continue;
        }
        let end = rows[i..].iter().take_while(|r| r.1.as_ref() == Some(who)).last().map(|r| r.3).unwrap_or(*start);
        out.push(OtherVoice { speaker: who.clone(), from_seconds: *start, to_seconds: end, preview: text.chars().take(120).collect() });
    }
    out
}

pub async fn other_voices_of(pool: &SqlitePool, meeting_id: &str) -> Result<Vec<OtherVoice>> {
    let rows: Vec<(String, Option<String>, Option<f64>, Option<f64>)> =
        sqlx::query_as("SELECT transcript, speaker, audio_start_time, audio_end_time FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time")
            .bind(meeting_id)
            .fetch_all(pool)
            .await?;
    let rows: Vec<(String, Option<String>, f64, f64)> = rows.into_iter().map(|(t, s, a, b)| (t, s, a.unwrap_or(0.0), b.or(a).unwrap_or(0.0))).collect();
    Ok(other_voices_in(&rows))
}

/// Gives the lines of `speaker` from `from_seconds` (until another voice
/// speaks) a new voice label («Спікер N»), to be named in «Спікери»
pub async fn split_off_voice(pool: &SqlitePool, meeting_id: &str, speaker: &str, from_seconds: f64) -> Result<String> {
    let rows: Vec<(String, Option<String>, Option<f64>, Option<f64>)> =
        sqlx::query_as("SELECT id, speaker, audio_start_time, audio_end_time FROM transcripts WHERE meeting_id = ? AND audio_start_time >= ? ORDER BY audio_start_time")
            .bind(meeting_id)
            .bind(from_seconds - 0.01)
            .fetch_all(pool)
            .await?;
    let ids: Vec<(String, f64)> = rows
        .into_iter()
        .take_while(|(_, s, _, _)| s.as_deref() == Some(speaker))
        .map(|(id, _, a, b)| (id, (b.unwrap_or(0.0) - a.unwrap_or(0.0)).max(0.0)))
        .collect();
    if ids.is_empty() {
        return Err(anyhow!("Рядків цього голосу з {from_seconds:.0} с не знайдено"));
    }
    let labels: Vec<String> = sqlx::query_scalar("SELECT label FROM meeting_speakers WHERE meeting_id = ?").bind(meeting_id).fetch_all(pool).await?;
    let next = labels.iter().filter_map(|l| l.strip_prefix("Спікер ").and_then(|n| n.parse::<usize>().ok())).max().unwrap_or(0) + 1;
    let label = format!("Спікер {next}");
    let mut tx = pool.begin().await?;
    for (id, _) in &ids {
        sqlx::query("UPDATE transcripts SET speaker = ? WHERE id = ?").bind(&label).bind(id).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO meeting_speakers (meeting_id, label, name, centroid, speech_seconds) VALUES (?, ?, NULL, ?, ?)")
        .bind(meeting_id)
        .bind(&label)
        .bind(Vec::<u8>::new())
        .bind(ids.iter().map(|(_, d)| d).sum::<f64>())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(label)
}

#[tauri::command]
pub async fn get_other_voices<R: Runtime>(app: AppHandle<R>, meeting_id: String) -> Result<Vec<OtherVoice>, String> {
    let pool = pool(&app).map_err(|e| e.to_string())?;
    other_voices_of(&pool, &meeting_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn split_speaker_voice<R: Runtime>(app: AppHandle<R>, meeting_id: String, speaker: String, from_seconds: f64) -> Result<String, String> {
    let pool = pool(&app).map_err(|e| e.to_string())?;
    crate::speakers::journal::journaled(&pool, &meeting_id, "розділення голосу", || async { split_off_voice(&pool, &meeting_id, &speaker, from_seconds).await.map_err(|e| e.to_string()) }).await
}

/// Roles already given to voices («S1», «керівник»…), for the meeting-type
/// editor: parts can start at a speaker with one of these roles
#[tauri::command]
pub async fn get_speaker_roles<R: Runtime>(app: AppHandle<R>) -> Result<Vec<String>, String> {
    let pool = pool(&app).map_err(|e| e.to_string())?;
    let roles: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT role FROM (SELECT role FROM voice_profiles UNION SELECT role FROM meeting_speakers) WHERE role IS NOT NULL AND TRIM(role) <> '' ORDER BY role",
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(roles)
}

/// Voices of a meeting that may be one person, for «Об'єднати / Ні»
#[tauri::command]
pub async fn get_likely_same_speakers<R: Runtime>(app: AppHandle<R>, meeting_id: String) -> Result<Vec<LikelySame>, String> {
    let pool = pool(&app).map_err(|e| e.to_string())?;
    likely_same_of(&pool, &meeting_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn merge_speakers<R: Runtime>(app: AppHandle<R>, meeting_id: String, keep: String, merge: String) -> Result<(), String> {
    let pool = pool(&app).map_err(|e| e.to_string())?;
    crate::speakers::journal::journaled(&pool, &meeting_id, "об'єднання голосів", || async { merge_with(&pool, &meeting_id, &keep, &merge).await.map_err(|e| e.to_string()) }).await
}

#[cfg(test)]
mod identify_only {
    /// Speaker recognition alone on a COPY of the app database (no renames
    /// or merges): PROTOKOL_SPEAKERS_DB, PROTOKOL_SPEAKERS_MODELS, PROTOKOL_SPEAKERS_MEETING
    #[tokio::test]
    #[ignore = "needs a database copy, a recording and the models"]
    async fn identify_only_on_a_database_copy() {
        let _ = env_logger::builder().is_test(true).filter_level(log::LevelFilter::Info).try_init();
        let db = std::env::var("PROTOKOL_SPEAKERS_DB").unwrap();
        let models = std::path::PathBuf::from(std::env::var("PROTOKOL_SPEAKERS_MODELS").unwrap());
        let meeting = std::env::var("PROTOKOL_SPEAKERS_MEETING").unwrap();
        let manager = crate::database::manager::DatabaseManager::new(&db, "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        let data_dir = dirs::data_dir().unwrap().join(crate::identity::IDENTIFIER);
        crate::ukrainian::init_glossary(&data_dir);
        let engine = crate::whisper_engine::WhisperEngine::new_with_models_dir(Some(data_dir.join("models"))).unwrap();
        engine.discover_models().await.unwrap();
        engine.load_model(crate::config::DEFAULT_WHISPER_MODEL).await.unwrap();
        let r = super::identify_with(&pool, &models, &engine, Some("uk".into()), &meeting, |_, _| {}).await.unwrap();
        let others = super::other_voices_of(&pool, &meeting).await.unwrap();
        println!("IDENTIFY {} speakers, {} turns, {:.1}s; possible other voices: {}", r.speakers.len(), r.turns, r.seconds, others.len());
    }
}

#[cfg(test)]
mod other_voice_tests {
    #[tokio::test]
    async fn a_second_report_in_one_voice_is_found_and_split_off() {
        let rows = vec![
            ("Доброго ранку. Склад без змін. Дякую, в мене все.".to_string(), Some("Спікер 2".to_string()), 10.0, 30.0),
            ("Доброго ранку, шановні колеги. Зв'язок працює.".to_string(), Some("Спікер 2".to_string()), 31.0, 50.0),
            ("Завтра перевіримо генератор.".to_string(), Some("Спікер 2".to_string()), 51.0, 60.0),
            ("Дякую.".to_string(), Some("Я".to_string()), 61.0, 62.0),
        ];
        let found = super::other_voices_in(&rows);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].from_seconds, found[0].to_seconds), (31.0, 60.0));

        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.sqlite");
        let manager = crate::database::manager::DatabaseManager::new(db.to_str().unwrap(), "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('m1', 'Тест', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO meeting_speakers (meeting_id, label, centroid, speech_seconds) VALUES ('m1', 'Спікер 2', x'', 50)").execute(&pool).await.unwrap();
        for (i, (text, who, a, b)) in rows.iter().enumerate() {
            sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, speaker) VALUES (?, 'm1', ?, 'x', ?, ?, ?)")
                .bind(format!("t{i}"))
                .bind(text)
                .bind(a)
                .bind(b)
                .bind(who)
                .execute(&pool)
                .await
                .unwrap();
        }
        let label = super::split_off_voice(&pool, "m1", "Спікер 2", 31.0).await.unwrap();
        assert_eq!(label, "Спікер 3");
        let moved: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transcripts WHERE speaker = 'Спікер 3'").fetch_one(&pool).await.unwrap();
        assert_eq!(moved, 2);
        assert!(super::other_voices_of(&pool, "m1").await.unwrap().is_empty());
    }
}

#[cfg(test)]
mod short_voice_tests {
    #[test]
    fn short_unknown_voices_are_remarks() {
        let speech = [120.0, 4.0, 9.5, 30.0, 2.0];
        let short = super::short_voices(&[0, 1, 2, 3, 4], &speech, &|k| k == 4);
        let mut v: Vec<_> = short.into_iter().collect();
        v.sort();
        // Voice 4 is short but known by its profile: listed
        assert_eq!(v, vec![1, 2]);
    }
}

#[cfg(test)]
mod tests {
    /// Runs speaker recognition, a rename and the profile match on a COPY of
    /// the app database. PROTOKOL_SPEAKERS_DB = copied meeting_minutes.sqlite,
    /// PROTOKOL_SPEAKERS_MODELS = folder with the two models,
    /// PROTOKOL_SPEAKERS_MEETING = meeting id.
    #[tokio::test]
    #[ignore = "needs a database copy, a recording and the models"]
    async fn identify_on_a_database_copy() {
        let _ = env_logger::builder().is_test(true).filter_level(log::LevelFilter::Info).try_init();
        let db = std::env::var("PROTOKOL_SPEAKERS_DB").unwrap();
        let models = std::path::PathBuf::from(std::env::var("PROTOKOL_SPEAKERS_MODELS").unwrap());
        let meeting = std::env::var("PROTOKOL_SPEAKERS_MEETING").unwrap();
        let manager = crate::database::manager::DatabaseManager::new(&db, "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        let data_dir = dirs::data_dir().unwrap().join(crate::identity::IDENTIFIER);
        let engine = crate::whisper_engine::WhisperEngine::new_with_models_dir(Some(data_dir.join("models"))).unwrap();
        engine.discover_models().await.unwrap();
        engine.load_model(crate::config::DEFAULT_WHISPER_MODEL).await.unwrap();
        let r = super::identify_with(&pool, &models, &engine, Some("uk".into()), &meeting, |s, p| println!("progress {s} {p}%")).await.unwrap();
        println!("RESULT {} speakers, {} turns, {:.1}s", r.speakers.len(), r.turns, r.seconds);
        for s in &r.speakers {
            println!("  {} {:?} {:.0}s", s.label, s.name, s.speech_seconds);
        }
        let model = super::model_id(&models);
        // Name the second voice; the transcript and a profile follow
        let second = r.speakers.get(1).map(|s| s.label.clone()).unwrap();
        super::rename_with(&pool, &model, &meeting, &second, "Тестове Ім'я").await.unwrap();
        let renamed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transcripts WHERE meeting_id = ? AND speaker = ?").bind(&meeting).bind("Тестове Ім'я").fetch_one(&pool).await.unwrap();
        let profiles: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM voice_profiles").fetch_one(&pool).await.unwrap();
        println!("RENAMED lines {renamed}, profiles {profiles}");
        // The same meeting again: the named voice is recognised from its profile
        let again = super::identify_with(&pool, &models, &engine, Some("uk".into()), &meeting, |_, _| {}).await.unwrap();
        println!("AGAIN names: {:?}", again.speakers.iter().map(|s| (s.label.clone(), s.name.clone())).collect::<Vec<_>>());
        println!("AGAIN took {:.1}s (word times kept from the first run)", again.seconds);
        let words_kept = super::super::words::load(&std::path::PathBuf::from(sqlx::query_scalar::<_, String>("SELECT folder_path FROM meetings WHERE id = ?").bind(&meeting).fetch_one(&pool).await.unwrap())).is_some();
        assert!(words_kept);
        for p in super::likely_same_of(&pool, &meeting).await.unwrap() {
            println!("LIKELY SAME {} ~ {} {:.2}", p.keep, p.merge, p.similarity);
        }
        assert!(again.speakers.iter().any(|s| s.name.as_deref() == Some("Тестове Ім'я")));
        // What the dialog shows, then a merge of the two smallest voices
        let list = super::speakers_of(&pool, &meeting).await.unwrap();
        for s in &list {
            println!("LIST {} {:?} {:.0}s first {:?} clip {:?}-{:?} samples {}", s.label, s.name, s.speech_seconds, s.first_seconds, s.clip_start, s.clip_end, s.samples.len());
        }
        let n = list.len();
        let (keep, merge) = (list[n - 2].label.clone(), list[n - 1].label.clone());
        super::merge_with(&pool, &meeting, &keep, &merge).await.unwrap();
        let after = super::speakers_of(&pool, &meeting).await.unwrap();
        println!("MERGED {} -> {} voices", n, after.len());
        assert_eq!(after.len(), n - 1);
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transcripts WHERE meeting_id = ? AND speaker = ?").bind(&meeting).bind(&merge).fetch_one(&pool).await.unwrap();
        assert_eq!(left, 0);
    }
}


#[cfg(test)]
mod stopping {
    /// «Зупинити» early, in the voice stage and near the end: the run ends
    /// with «Зупинено» and the meeting's transcript is left as it was.
    /// Same environment as identify_on_a_database_copy.
    #[tokio::test]
    #[ignore = "needs a database copy, a recording and the models"]
    async fn stop_at_three_points() {
        let _ = env_logger::builder().is_test(true).filter_level(log::LevelFilter::Warn).try_init();
        let db = std::env::var("PROTOKOL_SPEAKERS_DB").unwrap();
        let models = std::path::PathBuf::from(std::env::var("PROTOKOL_SPEAKERS_MODELS").unwrap());
        let meeting = std::env::var("PROTOKOL_SPEAKERS_MEETING").unwrap();
        let manager = crate::database::manager::DatabaseManager::new(&db, "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        let data_dir = dirs::data_dir().unwrap().join(crate::identity::IDENTIFIER);
        let engine = crate::whisper_engine::WhisperEngine::new_with_models_dir(Some(data_dir.join("models"))).unwrap();
        engine.discover_models().await.unwrap();
        engine.load_model(crate::config::DEFAULT_WHISPER_MODEL).await.unwrap();
        let snapshot = || async {
            let rows: Vec<(String, Option<String>)> = sqlx::query_as("SELECT transcript, speaker FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time")
                .bind(&meeting)
                .fetch_all(&pool)
                .await
                .unwrap();
            rows
        };
        let before = snapshot().await;
        // Stop when this stage has reached this percentage
        for (stage, at) in [("decoding", 5u32), ("diarizing", 22), ("transcribing", 85)] {
            let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let f = flag.clone();
            let started = std::time::Instant::now();
            let result = super::identify_with_stop(
                &pool,
                &models,
                &engine,
                Some("uk".into()),
                &meeting,
                |s, p| {
                    if s == stage && p >= at {
                        flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                },
                move || f.load(std::sync::atomic::Ordering::SeqCst),
            )
            .await;
            let err = result.err().map(|e| e.to_string());
            let unchanged = snapshot().await == before;
            println!("STOP at {stage} {at}%: {:?} after {:.0}s, transcript unchanged: {unchanged}", err, started.elapsed().as_secs_f64());
            assert_eq!(err.as_deref(), Some(super::super::diarize::STOPPED));
            assert!(unchanged);
        }
    }
}
