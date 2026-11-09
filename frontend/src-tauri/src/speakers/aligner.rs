//! Where each word really starts: the recognised words of a piece are
//! aligned to its sound by a character model (wav2vec2 XLS-R for Ukrainian,
//! Yehor/w2v-xls-r-uk, Apache-2.0; int8 ONNX, about 355 MB) and a Viterbi
//! path through its per-frame character probabilities (CTC forced
//! alignment). Whisper's own word times drift by seconds inside long
//! pieces; aligned ones are within a frame or two (20 ms) of the sound.
//!
//! Optional: without the model (or on any error) the word times stay as
//! they were.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use ndarray::Array2;
use ort::inputs;
use ort::session::Session;
use ort::value::TensorRef;

pub const MODEL_FILE: &str = "w2v-uk-int8.onnx";
pub const VOCAB_FILE: &str = "vocab.json";
const RATE: f64 = 16_000.0;
/// Sound taken around a piece: Whisper's edges are loose, and the app's
/// decoder may be up to ~0.2 s off the player's timeline
const PAD: f64 = 0.6;
/// The model hears at most this much at once (plus context on each side)
const CHUNK_S: f64 = 4.0;
const CONTEXT_S: f64 = 1.0;

pub struct Aligner {
    session: Session,
    vocab: HashMap<char, usize>,
    blank: usize,
    sep: usize,
}

/// Where the model is: next to the other models in the app data folder, or bundled with the app
pub fn model_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Option<PathBuf> {
    use tauri::Manager;
    let candidates = [
        app.path().app_data_dir().ok().map(|d| d.join("models").join("aligner")),
        app.path().resource_dir().ok().map(|d| d.join("aligner-model")),
    ];
    let found = candidates.into_iter().flatten().find(|d| d.join(MODEL_FILE).exists() && d.join(VOCAB_FILE).exists());
    if let (Some(d), Ok(mut known)) = (&found, KNOWN_DIR.lock()) {
        *known = Some(d.clone());
    }
    found
}

static KNOWN_DIR: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// The model folder last found (or the one next to the given models folder)
pub fn known_dir(models: &Path) -> Option<PathBuf> {
    let own = models.join("aligner");
    if own.join(MODEL_FILE).exists() && own.join(VOCAB_FILE).exists() {
        return Some(own);
    }
    KNOWN_DIR.lock().ok().and_then(|d| d.clone())
}

impl Aligner {
    pub fn open(dir: &Path) -> Result<Self> {
        Self::open_with(dir, 2)
    }

    /// The model on `threads` threads: two in the background (never the
    /// whole machine), more right after a recording, when nobody is listening
    pub fn open_with(dir: &Path, threads: usize) -> Result<Self> {
        // …and memory given back after each piece (no arena kept at its peak)
        let session = ort::session::Session::builder()?
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level1)?
            .with_intra_threads(threads.max(1))?
            .with_memory_pattern(false)?
            .with_prepacking(false)?
            .with_execution_providers([ort::execution_providers::CPUExecutionProvider::default().with_arena_allocator(false).build()])?
            .commit_from_file(dir.join(MODEL_FILE))
            .map_err(|e| anyhow!("Could not open the aligner model: {e}"))?;
        let raw: HashMap<String, usize> = serde_json::from_slice(&std::fs::read(dir.join(VOCAB_FILE))?)?;
        let blank = *raw.get("[PAD]").or(raw.get("<pad>")).ok_or_else(|| anyhow!("no blank in the vocabulary"))?;
        let sep = *raw.get("|").ok_or_else(|| anyhow!("no word separator in the vocabulary"))?;
        let vocab = raw.iter().filter(|(k, _)| k.chars().count() == 1 && k.as_str() != "|").map(|(k, v)| (k.chars().next().unwrap(), *v)).collect();
        Ok(Self { session, vocab, blank, sep })
    }

    /// Per-frame log-probabilities of the characters for 16 kHz audio
    /// [`Self::emissions`] of a long stretch, worked out 4 s at a time (with
    /// 1 s on each side for context) and joined: the model's memory grows with
    /// the length (~35 MB a second), so a minute at once would take gigabytes
    fn emissions_chunked(&mut self, samples: &[f32]) -> Result<Array2<f32>> {
        let (chunk, ctx) = ((CHUNK_S * RATE) as usize, (CONTEXT_S * RATE) as usize);
        if samples.len() <= chunk + ctx {
            return self.emissions(samples);
        }
        let mut rows: Vec<f32> = Vec::new();
        let mut classes = 0;
        let mut kept_frames = 0usize;
        let mut from = 0usize;
        while from < samples.len() {
            let to = (from + chunk).min(samples.len());
            let (a, b) = (from.saturating_sub(ctx), (to + ctx).min(samples.len()));
            let em = self.emissions(&samples[a..b])?;
            classes = em.ncols();
            let per_sample = em.nrows() as f64 / (b - a) as f64;
            // The frames of this chunk's own part (not of its context)
            let f0 = ((from - a) as f64 * per_sample).round() as usize;
            let f1 = (((to - a) as f64 * per_sample).round() as usize).min(em.nrows());
            for f in f0..f1 {
                rows.extend(em.row(f).iter());
                kept_frames += 1;
            }
            from = to;
        }
        Ok(Array2::from_shape_vec((kept_frames, classes), rows)?)
    }

    fn emissions(&mut self, samples: &[f32]) -> Result<Array2<f32>> {
        let n = samples.len().max(1) as f32;
        let mean = samples.iter().sum::<f32>() / n;
        let var = samples.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / n;
        let std = (var + 1e-7).sqrt();
        let input: Vec<f32> = samples.iter().map(|x| (x - mean) / std).collect();
        let input = Array2::from_shape_vec((1, input.len()), input)?;
        let outputs = self.session.run(inputs!["input_values" => TensorRef::from_array_view(input.view())?])?;
        let (shape, data) = outputs["logits"].try_extract_tensor::<f32>()?;
        let (t, c) = (shape[1] as usize, shape[2] as usize);
        let mut out = Array2::<f32>::zeros((t, c));
        for i in 0..t {
            let row = &data[i * c..(i + 1) * c];
            let max = row.iter().cloned().fold(f32::MIN, f32::max);
            let lse = max + row.iter().map(|v| (v - max).exp()).sum::<f32>().ln();
            for j in 0..c {
                out[[i, j]] = row[j] - lse;
            }
        }
        Ok(out)
    }

    /// The characters of a word the model knows (lower case, apostrophes as one)
    fn chars_of(&self, word: &str) -> Vec<usize> {
        word.to_lowercase()
            .chars()
            .map(|c| if matches!(c, '’' | 'ʼ' | '`' | '\u{2018}') { '\'' } else { c })
            .filter_map(|c| self.vocab.get(&c).copied())
            .filter(|&id| id != self.blank)
            .collect()
    }

    /// Start of each word (seconds, on the timeline of `from`) in a stretch of
    /// audio starting at `from`; None for words without known characters
    pub fn align(&mut self, samples: &[f32], from: f64, words: &[String]) -> Result<Vec<Option<f64>>> {
        if samples.len() < 1600 || words.is_empty() {
            return Ok(vec![None; words.len()]);
        }
        let em = self.emissions_chunked(samples)?;
        let frames = em.nrows();
        let frame_s = samples.len() as f64 / RATE / frames.max(1) as f64;
        let mut tokens: Vec<usize> = Vec::new();
        let mut owner: Vec<Option<usize>> = Vec::new();
        for (k, w) in words.iter().enumerate() {
            let cs = self.chars_of(w);
            if cs.is_empty() {
                continue;
            }
            if !tokens.is_empty() {
                tokens.push(self.sep);
                owner.push(None);
            }
            for c in cs {
                tokens.push(c);
                owner.push(Some(k));
            }
        }
        if tokens.is_empty() || tokens.len() > frames {
            return Ok(vec![None; words.len()]);
        }
        let first = viterbi(&em, &tokens, self.blank);
        let mut out = vec![None; words.len()];
        for (i, k) in owner.iter().enumerate() {
            if let (Some(k), Some(f)) = (k, first[i]) {
                if out[*k].is_none() {
                    out[*k] = Some(from + f as f64 * frame_s);
                }
            }
        }
        Ok(out)
    }
}

/// The frame where each token is first emitted on the best path that
/// emits all tokens in order (blanks between, CTC)
pub(crate) fn viterbi(em: &Array2<f32>, tokens: &[usize], blank: usize) -> Vec<Option<usize>> {
    let (t_n, s_n) = (em.nrows(), tokens.len());
    let mut trellis = vec![f32::NEG_INFINITY; (t_n + 1) * (s_n + 1)];
    let at = |t: usize, s: usize| t * (s_n + 1) + s;
    trellis[at(0, 0)] = 0.0;
    for t in 0..t_n {
        trellis[at(t + 1, 0)] = trellis[at(t, 0)] + em[[t, blank]];
        for s in 1..=s_n {
            let stay = trellis[at(t, s)] + em[[t, blank]];
            let mv = trellis[at(t, s - 1)] + em[[t, tokens[s - 1]]];
            trellis[at(t + 1, s)] = stay.max(mv);
        }
    }
    let mut first = vec![None; s_n];
    let (mut t, mut s) = (t_n, s_n);
    while s > 0 && t > 0 {
        let stay = trellis[at(t - 1, s)] + em[[t - 1, blank]];
        let mv = trellis[at(t - 1, s - 1)] + em[[t - 1, tokens[s - 1]]];
        if mv >= stay {
            first[s - 1] = Some(t - 1);
            s -= 1;
        }
        t -= 1;
    }
    first
}

/// Word starts for every word: aligned ones as they are, the others (digits,
/// Latin) between their aligned neighbours by length; None if nothing aligned
pub(crate) fn fill_gaps(aligned: &[Option<f64>], words: &[String], start: f64, end: f64) -> Option<Vec<f64>> {
    let n = words.len();
    let known = aligned.iter().filter(|x| x.is_some()).count();
    if known == 0 || known * 2 < n {
        return None;
    }
    let len = |w: &str| w.chars().count().max(1) as f64;
    let mut out = vec![0.0; n];
    let mut k = 0;
    while k < n {
        if let Some(t) = aligned[k] {
            out[k] = t;
            k += 1;
            continue;
        }
        let gap_end = (k..n).find(|&q| aligned[q].is_some()).unwrap_or(n);
        let from = if k == 0 { start } else { out[k - 1] };
        let to = if gap_end < n { aligned[gap_end].unwrap() } else { end };
        let weights: Vec<f64> = (k..gap_end).map(|q| len(&words[q])).collect();
        let prev = if k == 0 { 0.0 } else { len(&words[k - 1]) };
        let total = prev + weights.iter().sum::<f64>();
        let mut acc = prev;
        for (q, w) in (k..gap_end).zip(weights) {
            out[q] = from + (to - from).max(0.0) * acc / total.max(1.0);
            acc += w;
        }
        k = gap_end;
    }
    // Starts never go back
    for i in 1..n {
        if out[i] < out[i - 1] {
            out[i] = out[i - 1];
        }
    }
    Some(out)
}

/// The words of every piece aligned to the mix (what the player plays):
/// each piece in a window around it (wider by the call/mix offset); pieces
/// that cannot be aligned keep their times. Returns how many were aligned.
pub fn align_pieces(aligner: &mut Aligner, mix: &[i16], pieces: &mut [super::words::PieceWords], map: Option<&super::time_map::TimeMap>, progress: &dyn Fn(usize, usize), stop: &dyn Fn() -> bool) -> usize {
    let mut done = 0;
    let total = pieces.len();
    for (i, p) in pieces.iter_mut().enumerate() {
        if stop() {
            break;
        }
        progress(i, total);
        if p.words.is_empty() || p.aligned {
            continue;
        }
        let (ms, me) = map.map(|m| (m.map(p.start), m.map(p.end))).unwrap_or((p.start, p.end));
        let a = (p.start.min(ms) - PAD).max(0.0);
        let b = (p.end.max(me) + PAD).min(mix.len() as f64 / RATE);
        let (i0, i1) = ((a * RATE) as usize, ((b * RATE) as usize).min(mix.len()));
        if i1 <= i0 + 1600 || b - a > 60.0 {
            continue;
        }
        let words: Vec<String> = p.words.iter().map(|w| w.0.clone()).collect();
        // Where the lines' own times put each word, before it moves
        let before = if p.heard.is_empty() { Some(super::lines::retimed(p)) } else { None };
        let window: Vec<f32> = mix[i0..i1].iter().map(|&x| x as f32 / 32768.0).collect();
        match aligner.align(&window, a, &words) {
            Ok(starts) => {
                if let Some(times) = fill_gaps(&starts, &words, a, b) {
                    if let Some(before) = before {
                        let mut heard: Vec<(f64, f64)> = times.iter().copied().zip(before).collect();
                        heard.sort_by(|x, y| x.0.total_cmp(&y.0));
                        p.heard = heard;
                    }
                    for (w, t) in p.words.iter_mut().zip(&times) {
                        w.1 = *t;
                    }
                    p.start = p.start.min(times[0]);
                    p.end = p.end.max(*times.last().unwrap());
                    p.aligned = true;
                    done += 1;
                }
            }
            Err(e) => log::warn!("Word alignment failed for a piece at {:.1}s: {e}", p.start),
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_best_path_finds_where_tokens_start() {
        // 3 symbols (0 = blank, 1, 2); token 1 heard at frame 2, token 2 at 6
        let mut em = Array2::<f32>::from_elem((9, 3), -5.0);
        for t in 0..9 {
            em[[t, 0]] = -0.1;
        }
        em[[2, 1]] = -0.05;
        em[[2, 0]] = -4.0;
        em[[6, 2]] = -0.05;
        em[[6, 0]] = -4.0;
        assert_eq!(viterbi(&em, &[1, 2], 0), vec![Some(2), Some(6)]);
    }

    /// The one-off pass on a COPY of the database and meeting folder:
    /// PROTOKOL_ALIGN_DB, PROTOKOL_ALIGN_MEETING. Runs twice (the second is a no-op).
    #[tokio::test]
    #[ignore = "needs a database copy and a copied meeting folder"]
    async fn align_pass_on_a_database_copy() {
        let db = std::env::var("PROTOKOL_ALIGN_DB").unwrap();
        assert!(db.contains(&crate::test_copy_root()), "only a copy");
        let meeting = std::env::var("PROTOKOL_ALIGN_MEETING").unwrap();
        let manager = crate::database::manager::DatabaseManager::new(&db, "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        let folder: String = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?").bind(&meeting).fetch_one(&pool).await.unwrap();
        assert!(folder.contains(&crate::test_copy_root()), "only a copied folder");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("aligner-model");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let t = std::time::Instant::now();
        let first = super::align_meeting_words(&pool, &meeting, &dir, move |p| s2.lock().unwrap().push(p), || {}).await;
        println!("first pass: {first} piece(s) in {:.1}s; progress {:?}", t.elapsed().as_secs_f64(), seen.lock().unwrap());
        let t = std::time::Instant::now();
        let second = super::align_meeting_words(&pool, &meeting, &dir, |_| {}, || {}).await;
        println!("second pass: {second} piece(s) in {:.3}s", t.elapsed().as_secs_f64());
        assert_eq!(second, 0);
        let flag = crate::database::repositories::setting::SettingsRepository::get_flag(&pool, &format!("words_aligned:{meeting}")).await.unwrap();
        println!("flag {flag:?}");
    }

    /// What «Перевірте» and the highlight get on a COPY: PROTOKOL_ALIGN_DB,
    /// PROTOKOL_ALIGN_MEETING, PROTOKOL_SPEAKERS_MODELS, PROTOKOL_LOOK_AT (seconds)
    #[tokio::test]
    #[ignore = "needs a database copy and a copied meeting folder"]
    async fn queue_on_a_database_copy() {
        let db = std::env::var("PROTOKOL_ALIGN_DB").unwrap();
        assert!(db.contains(&crate::test_copy_root()), "only a copy");
        let meeting = std::env::var("PROTOKOL_ALIGN_MEETING").unwrap();
        let manager = crate::database::manager::DatabaseManager::new(&db, "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        let folder: String = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?").bind(&meeting).fetch_one(&pool).await.unwrap();
        assert!(folder.contains(&crate::test_copy_root()), "only a copied folder");
        let pieces = crate::speakers::words::load(std::path::Path::new(&folder)).unwrap();
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT id, transcript FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time").bind(&meeting).fetch_all(&pool).await.unwrap();
        let doubts = crate::speakers::words::doubts(&pieces, &rows, &crate::ukrainian::glossary_terms());
        let auto = doubts.iter().filter(|d| d.heard.is_some()).count();
        println!("doubtful words {} (auto-replaced {auto}) on {} line(s)", doubts.len(), doubts.iter().map(|d| &d.segment_id).collect::<std::collections::HashSet<_>>().len());
        let voices = crate::speakers::lines::voice_changes(&pool, std::path::Path::new(&std::env::var("PROTOKOL_SPEAKERS_MODELS").unwrap()), &meeting).await.unwrap();
        println!("voice changes {}", voices.len());
        for v in &voices { println!("voice {} {:.1} {}..{}", v.segment_id, v.seconds, v.from, v.to); }
        let lines = crate::speakers::lines::line_words(&pool, &meeting).await.unwrap();
        let exact = lines.iter().filter(|l| l.exact).count();
        println!("lines {} exact {}", lines.len(), exact);
        let look: f64 = std::env::var("PROTOKOL_LOOK_AT").ok().and_then(|v| v.parse().ok()).unwrap_or(160.0);
        let stored: Vec<(String, f64)> = sqlx::query_as("SELECT id, audio_start_time FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time").bind(&meeting).fetch_all(&pool).await.unwrap();
        for (id, a) in stored.iter().filter(|(_, a)| (a - look).abs() < 30.0) {
            let l = lines.iter().find(|l| &l.id == id).unwrap();
            println!("stored {a:.2}: first words {:?}", l.words.iter().take(3).map(|w| (w.start * 100.0).round() / 100.0).collect::<Vec<_>>());
        }
    }

    #[test]
    fn words_are_followed_by_default_only_with_enough_memory() {
        let gb = 1024u64 * 1024 * 1024;
        assert!(!super::default_on(8 * gb));
        assert!(super::default_on(16 * gb));
        assert!(super::default_on(32 * gb));
    }

    #[tokio::test]
    async fn turned_off_nothing_is_aligned() {
        let dir = tempfile::tempdir().unwrap();
        let manager = crate::database::manager::DatabaseManager::new(dir.path().join("t.sqlite").to_str().unwrap(), "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        crate::database::repositories::setting::SettingsRepository::set_flag(&pool, "word_highlight", "0").await.unwrap();
        super::WORDS_ON.store(0, std::sync::atomic::Ordering::SeqCst);
        assert!(!super::words_on(&pool).await);
        // The pass does nothing (before even looking for the meeting)
        assert_eq!(super::align_meeting_words(&pool, "m1", std::path::Path::new("/nonexistent"), |_| {}, || {}).await, 0);
        assert!(crate::database::repositories::setting::SettingsRepository::get_flag(&pool, "words_aligned:m1").await.unwrap().is_none());
        // Processing a new recording does not align either, even with the model there
        let models = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(models.path().join("aligner")).unwrap();
        std::fs::write(models.path().join("aligner").join(super::MODEL_FILE), b"x").unwrap();
        std::fs::write(models.path().join("aligner").join(super::VOCAB_FILE), b"{}").unwrap();
        assert!(super::for_processing(&pool, models.path()).await.is_none());
        super::WORDS_ON.store(1, std::sync::atomic::Ordering::SeqCst);
        assert!(super::for_processing(&pool, models.path()).await.is_some());
        super::WORDS_ON.store(0, std::sync::atomic::Ordering::SeqCst);
    }

    #[test]
    fn the_part_being_heard_goes_first() {
        use crate::speakers::words::PieceWords;
        let piece = |a: f64, b: f64| PieceWords { start: a, end: b, text: String::new(), words: vec![("x".into(), a)], probs: vec![], auto: vec![], aligned: false, heard: vec![] };
        let pieces = vec![piece(0.0, 10.0), piece(10.0, 20.0), piece(20.0, 30.0), piece(30.0, 40.0)];
        let mut left = vec![true; 4];
        // Nothing heard: in order
        assert_eq!(super::next_piece(&pieces, &left, None), Some(0));
        // Heard at 25 s: that piece, then the next, then from the start
        assert_eq!(super::next_piece(&pieces, &left, Some(25.0)), Some(2));
        left[2] = false;
        assert_eq!(super::next_piece(&pieces, &left, Some(25.0)), Some(3));
        left[3] = false;
        assert_eq!(super::next_piece(&pieces, &left, Some(25.0)), Some(0));
        left = vec![false; 4];
        assert_eq!(super::next_piece(&pieces, &left, Some(25.0)), None);
    }

    #[test]
    fn corrected_pieces_keep_their_words() {
        use crate::speakers::words::PieceWords;
        let piece = |w: &[(&str, f64)], aligned: bool| PieceWords { start: 0.0, end: 3.0, text: String::new(), words: w.iter().map(|(a, b)| (a.to_string(), *b)).collect(), probs: vec![], auto: vec![], aligned, heard: vec![] };
        let mut now = vec![piece(&[("так", 0.0), ("ні", 1.0)], false), piece(&[("нове", 0.0)], false)];
        let aligned = vec![piece(&[("так", 0.4), ("ні", 1.2)], true), piece(&[("старе", 0.7)], true)];
        assert_eq!(super::take_aligned(&mut now, &aligned), 1);
        assert_eq!(now[0].words[1].1, 1.2);
        assert!(now[0].aligned);
        assert_eq!(now[1].words[0], ("нове".to_string(), 0.0));
        assert!(!now[1].aligned);
    }

    fn peak_mb() -> u64 {
        #[repr(C)]
        struct Rusage {
            utime: [i64; 2],
            stime: [i64; 2],
            maxrss: i64,
            rest: [i64; 13],
        }
        extern "C" {
            fn getrusage(who: i32, usage: *mut Rusage) -> i32;
        }
        let mut u = Rusage { utime: [0; 2], stime: [0; 2], maxrss: 0, rest: [0; 13] };
        unsafe { getrusage(0, &mut u) };
        (u.maxrss as u64) / 1024 / 1024
    }

    /// Memory of the model on one 13 s chunk, then ten more (PROTOKOL_ALIGN_PCM: 16 kHz i16)
    #[test]
    #[ignore = "needs the model and a copied recording"]
    fn memory_of_one_chunk() {
        let pcm = std::fs::read(std::env::var("PROTOKOL_ALIGN_PCM").unwrap()).unwrap();
        let samples: Vec<f32> = pcm.chunks_exact(2).take(16000 * std::env::var("PROTOKOL_SECONDS").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(13)).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("aligner-model");
        println!("start {} MB", peak_mb());
        let mut al = super::Aligner::open(&dir).unwrap();
        println!("model {} MB", peak_mb());
        al.emissions(&samples).unwrap();
        println!("one chunk {} MB", peak_mb());
        for _ in 0..10 {
            al.emissions(&samples).unwrap();
        }
        println!("eleven chunks {} MB", peak_mb());
    }

    /// The app's decoding of a copied recording, as raw f32 (to compare with other decoders)
    #[test]
    #[ignore = "needs a copied recording"]
    fn dump_a_decoded_copy() {
        let path = std::env::var("PROTOKOL_DECODE_IN").unwrap();
        assert!(path.contains(&crate::test_copy_root()), "only a copy");
        let mix = crate::speakers::commands::decode_16k(std::path::Path::new(&path)).unwrap();
        let bytes: Vec<u8> = mix.iter().flat_map(|x| x.to_le_bytes()).collect();
        std::fs::write(std::env::var("PROTOKOL_DECODE_OUT").unwrap(), bytes).unwrap();
    }

    /// On a COPIED meeting folder (read only): PROTOKOL_ALIGN_FOLDER, PROTOKOL_ALIGN_OUT
    #[test]
    #[ignore = "needs the model and a copied meeting folder"]
    fn align_a_meeting_copy() {
        let folder = std::path::PathBuf::from(std::env::var("PROTOKOL_ALIGN_FOLDER").unwrap());
        assert!(folder.to_string_lossy().contains(&crate::test_copy_root()), "only a copy");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("aligner-model");
        let mut pieces = crate::speakers::words::load(&folder).unwrap();
        let before: Vec<Vec<f64>> = pieces.iter().map(|p| p.words.iter().map(|w| w.1).collect()).collect();
        let spans: Vec<(f64, f64)> = pieces.iter().map(|p| (p.start, p.end)).collect();
        let t = std::time::Instant::now();
        let mix = super::player_mix(&crate::audio::retranscription::find_audio_file(&folder).unwrap()).unwrap();
        let decoded = t.elapsed().as_secs_f64();
        println!("peak after decode {} MB", peak_mb());
        let t = std::time::Instant::now();
        let threads = std::env::var("PROTOKOL_ALIGN_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(2);
        let mut al = super::Aligner::open_with(&dir, threads).unwrap();
        let loaded = t.elapsed().as_secs_f64();
        println!("threads {threads}");
        println!("peak after model {} MB", peak_mb());
        let longest = pieces.iter().map(|p| p.end - p.start).fold(0.0, f64::max);
        println!("longest piece {longest:.1}s");
        let t = std::time::Instant::now();
        let n = super::align_pieces(&mut al, &mix, &mut pieces, None, &|_, _| {}, &|| false);
        let took = t.elapsed().as_secs_f64();
        println!("peak after align {} MB", peak_mb());
        println!("audio {:.1} min; decode {decoded:.1}s, model load {loaded:.1}s, align {took:.1}s; aligned {n} of {} pieces", mix.len() as f64 / RATE / 60.0, pieces.len());
        let out: Vec<serde_json::Value> = pieces
            .iter()
            .zip(&before)
            .zip(&spans)
            .map(|((p, b), s)| serde_json::json!({"start": s.0, "end": s.1, "aligned": p.aligned, "words": p.words.iter().map(|w| &w.0).collect::<Vec<_>>(), "before": b, "after": p.words.iter().map(|w| w.1).collect::<Vec<_>>()}))
            .collect();
        std::fs::write(std::env::var("PROTOKOL_ALIGN_OUT").unwrap(), serde_json::to_string(&out).unwrap()).unwrap();
    }

    #[test]
    fn a_wav_file_gives_its_samples() {
        let mut b = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0".to_vec();
        b.extend([0u8; 16]);
        b.extend(b"data\x04\0\0\0");
        b.extend([0x00, 0x40, 0x00, 0xc0]);
        assert_eq!(wav_samples(&b).unwrap(), vec![16384, -16384]);
        assert!(wav_samples(b"nope").is_err());
    }

    #[test]
    fn words_the_model_cannot_spell_go_between_their_neighbours() {
        let words: Vec<String> = ["Отримали", "4027", "генераторів", "BMS"].iter().map(|s| s.to_string()).collect();
        let t = fill_gaps(&[Some(1.0), None, Some(2.0), None], &words, 0.5, 3.0).unwrap();
        assert_eq!(t[0], 1.0);
        assert!(t[1] > 1.0 && t[1] < 2.0);
        assert_eq!(t[2], 2.0);
        assert!(t[3] > 2.0 && t[3] <= 3.0);
        // Mostly unaligned: not trusted
        assert!(fill_gaps(&[None, None, Some(1.0), None], &words, 0.0, 3.0).is_none());
    }
}

/// The recording as the player hears it. The app's own decoder drifts from
/// the player (on a 33-min meeting by up to ~0.18 s, growing evenly); on macOS
/// the system decoder (the one the player uses) gives the player's timeline.
/// Any failure falls back to the app's decoder.
pub fn player_mix(path: &Path) -> Result<Vec<i16>> {
    #[cfg(target_os = "macos")]
    {
        match afconvert_16k(path) {
            Ok(m) if !m.is_empty() => return Ok(m),
            Ok(_) => log::warn!("System decoder gave no sound; the app's decoder is used"),
            Err(e) => log::warn!("System decoder failed ({e}); the app's decoder is used"),
        }
    }
    // 16-bit, as the system decoder gives it: half the memory of 32-bit
    Ok(super::commands::decode_16k(path)?.iter().map(|&x| (x.clamp(-1.0, 1.0) * 32767.0) as i16).collect())
}

#[cfg(target_os = "macos")]
fn afconvert_16k(path: &Path) -> Result<Vec<i16>> {
    let out = std::env::temp_dir().join(format!("protokol-align-{}-{}.wav", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
    let status = std::process::Command::new("/usr/bin/afconvert")
        .arg("-f")
        .arg("WAVE")
        .arg("-d")
        .arg("LEI16@16000")
        .arg("-c")
        .arg("1")
        .arg(path)
        .arg(&out)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let bytes = std::fs::read(&out);
    let _ = std::fs::remove_file(&out);
    if !status?.success() {
        return Err(anyhow!("afconvert failed"));
    }
    wav_samples(&bytes?)
}

/// 16-bit PCM samples of a WAV file's data chunk
fn wav_samples(b: &[u8]) -> Result<Vec<i16>> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err(anyhow!("not a WAV file"));
    }
    let mut i = 12;
    while i + 8 <= b.len() {
        let size = u32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]) as usize;
        if &b[i..i + 4] == b"data" {
            let data = &b[i + 8..(i + 8 + size).min(b.len())];
            return Ok(data.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect());
        }
        i += 8 + size + (size & 1);
    }
    Err(anyhow!("no sound in the WAV file"))
}

/// The last meeting's sound, kept for the corrections that follow one another
static MIX: once_cell::sync::Lazy<std::sync::Mutex<Option<(PathBuf, std::sync::Arc<Vec<i16>>)>>> = once_cell::sync::Lazy::new(|| std::sync::Mutex::new(None));

fn cached_mix(path: &Path) -> Result<std::sync::Arc<Vec<i16>>> {
    let mut slot = MIX.lock().map_err(|_| anyhow!("sound cache poisoned"))?;
    if let Some((p, m)) = slot.as_ref() {
        if p == path {
            return Ok(m.clone());
        }
    }
    let m = std::sync::Arc::new(player_mix(path)?);
    *slot = Some((path.to_path_buf(), m.clone()));
    Ok(m)
}

/// One loaded model for the app (it is large); None when it is not installed
static LOADED: once_cell::sync::Lazy<std::sync::Mutex<Option<(PathBuf, Aligner)>>> = once_cell::sync::Lazy::new(|| std::sync::Mutex::new(None));

/// Runs `f` with the model, loading it once; None without the model
pub fn with_aligner<T>(dir: &Path, f: impl FnOnce(&mut Aligner) -> T) -> Option<T> {
    let mut slot = LOADED.lock().ok()?;
    if slot.as_ref().map(|(d, _)| d != dir).unwrap_or(true) {
        match Aligner::open(dir) {
            Ok(a) => *slot = Some((dir.to_path_buf(), a)),
            Err(e) => {
                log::warn!("Word aligner not available: {e}");
                return None;
            }
        }
    }
    slot.as_mut().map(|(_, a)| f(a))
}

#[derive(Clone, serde::Serialize)]
struct AlignProgress {
    meeting_id: String,
    progress_percentage: u32,
}

/// A meeting's word times aligned to the sound, once (in the background,
/// with progress events «words-align-progress»; never while recording).
/// Returns how many pieces were aligned (0 when nothing to do).
#[tauri::command]
pub async fn api_align_meeting_words<R: tauri::Runtime>(app: tauri::AppHandle<R>, meeting_id: String) -> Result<usize, String> {
    use tauri::Emitter;
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    let Some(dir) = model_dir(&app) else { return Ok(0) };
    let id = meeting_id.clone();
    let id2 = meeting_id.clone();
    let app2 = app.clone();
    let app3 = app.clone();
    let done = align_meeting_words(
        &pool,
        &meeting_id,
        &dir,
        move |pct| {
            let _ = app2.emit("words-align-progress", AlignProgress { meeting_id: id.clone(), progress_percentage: pct });
        },
        move || {
            let _ = app3.emit("words-aligned", id2.clone());
        },
    )
    .await;
    let _ = app.emit("words-align-progress", AlignProgress { meeting_id, progress_percentage: 100 });
    Ok(done)
}

/// «Підсвічувати кожне слово»: words placed by the sound (on), or no
/// alignment at all and the whole line lit (off). 0 = not read yet.
static WORDS_ON: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
const WORDS_FLAG: &str = "word_highlight";

/// On by default with 16 GB of memory or more
pub(crate) fn default_on(memory_bytes: u64) -> bool {
    memory_bytes >= 15 * 1024 * 1024 * 1024
}

fn memory_bytes() -> u64 {
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("/usr/sbin/sysctl").arg("-n").arg("hw.memsize").output() {
            if let Ok(n) = String::from_utf8_lossy(&out.stdout).trim().parse::<u64>() {
                return n;
            }
        }
    }
    16 * 1024 * 1024 * 1024
}

/// Whether words are placed by the sound (the user's choice, else by the Mac's memory)
pub async fn words_on(pool: &sqlx::SqlitePool) -> bool {
    match WORDS_ON.load(std::sync::atomic::Ordering::SeqCst) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    let on = match crate::database::repositories::setting::SettingsRepository::get_flag(pool, WORDS_FLAG).await.ok().flatten().as_deref() {
        Some("1") => true,
        Some("0") => false,
        _ => default_on(memory_bytes()),
    };
    WORDS_ON.store(if on { 1 } else { 2 }, std::sync::atomic::Ordering::SeqCst);
    on
}

/// The model for aligning a new recognition, unless «Підсвічувати кожне
/// слово» is off (the Mac is spared) or the model is not there
pub async fn for_processing(pool: &sqlx::SqlitePool, models: &Path) -> Option<PathBuf> {
    if !words_on(pool).await {
        return None;
    }
    known_dir(models)
}

/// Turned off meanwhile (a pass in progress stops at once)
fn words_off_now() -> bool {
    WORDS_ON.load(std::sync::atomic::Ordering::SeqCst) == 2
}

#[tauri::command]
pub async fn api_get_word_highlight<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<bool, String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    Ok(words_on(&pool).await)
}

#[tauri::command]
pub async fn api_set_word_highlight<R: tauri::Runtime>(app: tauri::AppHandle<R>, on: bool) -> Result<(), String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    crate::database::repositories::setting::SettingsRepository::set_flag(&pool, WORDS_FLAG, if on { "1" } else { "0" }).await.map_err(|e| e.to_string())?;
    WORDS_ON.store(if on { 1 } else { 2 }, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// The page's own measurements, for test runs (PROTOKOL_PERF=1 at launch)
#[tauri::command]
pub fn api_perf_enabled() -> bool {
    std::env::var("PROTOKOL_PERF").map(|v| v == "1").unwrap_or(false)
}

#[tauri::command]
pub fn api_perf_log(text: String) {
    if api_perf_enabled() {
        log::info!("perf: {text}");
    }
}

/// Threads for aligning right after a recording: half the Mac's cores, 2 to 4
pub fn processing_threads() -> usize {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    (cores / 2).clamp(2, 4)
}

/// Where the user is listening in a meeting being aligned: that part goes first
static FOCUS: std::sync::Mutex<Option<(String, f64, u64)>> = std::sync::Mutex::new(None);
static FOCUS_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The player is at `seconds` (on play and seek, and now and then while playing)
#[tauri::command]
pub fn api_align_focus(meeting_id: String, seconds: f64) {
    let n = FOCUS_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    if let Ok(mut f) = FOCUS.lock() {
        *f = Some((meeting_id, seconds, n));
    }
}

fn focus_of(meeting_id: &str) -> Option<(f64, u64)> {
    FOCUS.lock().ok()?.as_ref().filter(|f| f.0 == meeting_id).map(|f| (f.1, f.2))
}

/// The next piece to align: the one being heard (or the next one after it),
/// else the first one left
pub(crate) fn next_piece(pieces: &[super::words::PieceWords], left: &[bool], at: Option<f64>) -> Option<usize> {
    if let Some(t) = at {
        let ahead = (0..pieces.len()).filter(|&i| left[i] && pieces[i].end >= t - 0.5).min_by(|&a, &b| pieces[a].start.total_cmp(&pieces[b].start));
        if ahead.is_some() {
            return ahead;
        }
    }
    (0..pieces.len()).find(|&i| left[i])
}

/// Background work at a low priority (macOS: «utility»), so the Mac stays cool
/// and the player and the window never wait for it. Threads started from here
/// (the model's own) inherit it.
pub(crate) fn lower_priority() {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
        }
        const QOS_CLASS_UTILITY: u32 = 0x11;
        unsafe {
            pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0);
        }
    }
}

/// The work of [`api_align_meeting_words`] (also used by tests). Never fails:
/// anything wrong leaves the word times as they were. The part being listened
/// to goes first; what is done is saved as it goes (`saved` is told), so the
/// highlight is right there at once.
pub async fn align_meeting_words(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
    dir: &Path,
    progress: impl Fn(u32) + Send + Sync + 'static,
    saved: impl Fn() + Send + Sync + 'static,
) -> usize {
    use crate::database::repositories::setting::SettingsRepository;
    let flag = format!("words_aligned:{meeting_id}");
    if !words_on(pool).await || SettingsRepository::get_flag(pool, &flag).await.ok().flatten().is_some() || crate::audio::recording_commands::recording_now() {
        return 0;
    }
    let folder: Option<String> = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?").bind(meeting_id).fetch_optional(pool).await.ok().flatten().flatten();
    let Some(folder) = folder.map(PathBuf::from) else { return 0 };
    let Some(pieces) = super::words::load(&folder) else { return 0 };
    let Ok(audio) = crate::audio::retranscription::find_audio_file(&folder) else { return 0 };
    let map = super::time_map::load(&folder).filter(|m| !m.is_identity());
    let dir = dir.to_path_buf();
    let id = meeting_id.to_string();
    let started = std::time::Instant::now();
    let result = tokio::task::spawn_blocking(move || -> Result<(usize, Option<&'static str>)> {
        lower_priority();
        let mix = player_mix(&audio)?;
        let mut pieces = pieces;
        let n = pieces.len();
        let mut left: Vec<bool> = pieces.iter().map(|p| !p.aligned && !p.words.is_empty()).collect();
        let total = left.iter().filter(|x| **x).count().max(1);
        let (mut done, mut tried) = (0usize, 0usize);
        let mut unsaved = 0usize;
        let mut last_save = std::time::Instant::now();
        let mut last_progress = std::time::Instant::now();
        let mut seen_focus = 0u64;
        let mut urgent = 0usize;
        // Saved as it goes: lines corrected meanwhile keep their words
        let save = |aligned: &[super::words::PieceWords]| -> bool {
            let Some(mut now) = super::words::load(&folder) else { return false };
            if take_aligned(&mut now, aligned) == 0 {
                return false;
            }
            super::words::save(&folder, &now).is_ok()
        };
        let mut stopped: Option<&'static str> = None;
        while let Some(i) = next_piece(&pieces, &left, focus_of(&id).map(|f| f.0)) {
            // A recording starting meanwhile has the machine first; turned off: stop
            if crate::audio::recording_commands::recording_now() {
                stopped = Some("a recording started");
                break;
            }
            if words_off_now() {
                stopped = Some("«Підсвічувати кожне слово» was turned off");
                break;
            }
            if let Some((_, seq)) = focus_of(&id) {
                if seq != seen_focus {
                    seen_focus = seq;
                    // The piece heard now and the next two: shown as soon as each is done
                    urgent = 3;
                }
            }
            left[i] = false;
            tried += 1;
            let ok = with_aligner(&dir, |a| align_pieces(a, &mix, &mut pieces[i..=i], map.as_ref(), &|_, _| {}, &|| false)).unwrap_or(0);
            if ok > 0 {
                done += 1;
                unsaved += 1;
            }
            if unsaved > 0 && (urgent > 0 || last_save.elapsed().as_secs_f64() > 15.0) {
                if save(&pieces) {
                    saved();
                }
                unsaved = 0;
                last_save = std::time::Instant::now();
            }
            urgent = urgent.saturating_sub(1);
            if last_progress.elapsed().as_millis() >= 500 {
                progress((100 * tried / total).min(99) as u32);
                last_progress = std::time::Instant::now();
            }
            if tried >= n {
                break;
            }
        }
        if unsaved > 0 && save(&pieces) {
            saved();
        }
        Ok((done, stopped))
    })
    .await;
    match result {
        Ok(Ok((done, stopped))) => {
            match stopped {
                Some(why) => log::info!("Word times aligned to the sound: {done} piece(s) in {:.1}s, then stopped: {why}; the rest follows later", started.elapsed().as_secs_f64()),
                None => log::info!("Word times aligned to the sound: {done} piece(s) in {:.1}s", started.elapsed().as_secs_f64()),
            }
            // Done for good only when not cut short
            if stopped.is_none() && !crate::audio::recording_commands::recording_now() && !words_off_now() {
                let _ = SettingsRepository::set_flag(pool, &flag, if done > 0 { "1" } else { "none" }).await;
            }
            done
        }
        Ok(Err(e)) => {
            log::warn!("Word alignment skipped: {e}");
            0
        }
        Err(e) => {
            log::warn!("Word alignment stopped: {e}");
            0
        }
    }
}

/// Aligned times put into the current pieces, for each piece whose words are
/// still the same (a piece corrected meanwhile keeps its own). How many taken.
pub(crate) fn take_aligned(now: &mut [super::words::PieceWords], aligned: &[super::words::PieceWords]) -> usize {
    let mut taken = 0;
    for (n, a) in now.iter_mut().zip(aligned) {
        let same = a.aligned && !n.aligned && n.words.len() == a.words.len() && n.words.iter().zip(&a.words).all(|(x, y)| x.0 == y.0) && (n.start - a.start).abs() < 5.0;
        if same {
            for (x, y) in n.words.iter_mut().zip(&a.words) {
                x.1 = y.1;
            }
            n.start = a.start;
            n.end = a.end;
            n.aligned = true;
            n.heard = a.heard.clone();
            taken += 1;
        }
    }
    taken
}

/// After a line was corrected: the pieces it touches aligned again (in the background)
#[tauri::command]
pub async fn api_realign_line<R: tauri::Runtime>(app: tauri::AppHandle<R>, meeting_id: String, segment_id: String) -> Result<bool, String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    if crate::audio::recording_commands::is_recording().await || !words_on(&pool).await {
        return Ok(false);
    }
    let Some(dir) = model_dir(&app) else { return Ok(false) };
    let row: Option<(Option<f64>, Option<f64>, Option<String>)> =
        sqlx::query_as("SELECT t.audio_start_time, t.audio_end_time, m.folder_path FROM transcripts t JOIN meetings m ON m.id = t.meeting_id WHERE t.id = ? AND t.meeting_id = ?")
            .bind(&segment_id)
            .bind(&meeting_id)
            .fetch_optional(&pool)
            .await
            .map_err(|e| e.to_string())?;
    let Some((Some(a), b, Some(folder))) = row else { return Ok(false) };
    let b = b.unwrap_or(a + 5.0);
    let folder = PathBuf::from(folder);
    let Some(mut pieces) = super::words::load(&folder) else { return Ok(false) };
    let Ok(audio) = crate::audio::retranscription::find_audio_file(&folder) else { return Ok(false) };
    let map = super::time_map::load(&folder).filter(|m| !m.is_identity());
    let res = tokio::task::spawn_blocking(move || -> Result<Vec<super::words::PieceWords>> {
        lower_priority();
        // Only the pieces of this line
        let touched: Vec<usize> = pieces.iter().enumerate().filter(|(_, p)| p.start < b + 0.5 && p.end > a - 0.5).map(|(i, _)| i).collect();
        if touched.is_empty() {
            return Ok(pieces);
        }
        let mix = cached_mix(&audio)?;
        with_aligner(&dir, |al| {
            for &i in &touched {
                pieces[i].aligned = false;
                align_pieces(al, &mix, &mut pieces[i..=i], map.as_ref(), &|_, _| {}, &|| false);
            }
        });
        Ok(pieces)
    })
    .await
    .map_err(|e| e.to_string())?;
    match res {
        Ok(pieces) => {
            let Some(mut now) = super::words::load(&folder) else { return Ok(false) };
            if take_aligned(&mut now, &pieces) == 0 {
                return Ok(false);
            }
            super::words::save(&folder, &now).map_err(|e| e.to_string())?;
            Ok(true)
        }
        Err(e) => {
            log::warn!("Line not aligned again: {e}");
            Ok(false)
        }
    }
}
