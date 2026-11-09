//! Downloads the two speaker models once (≈ 33 MB) into the app data folder.
//! After that, speaker recognition works offline. Files are checked against
//! fixed SHA-256 sums; licences and authors are in THIRD_PARTY_NOTICES.md.

use std::path::Path;

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use log::info;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::io::AsyncWriteExt;

use super::commands::{EMBEDDING_MODEL, SEGMENTATION_MODEL};

struct ModelFile {
    file: &'static str,
    url: &'static str,
    sha256: &'static str,
    size: u64,
}

/// pyannote segmentation-3.0 (MIT, CNRS) and WeSpeaker ResNet34-LM (CC BY 4.0),
/// ONNX exports published by k2-fsa
const FILES: &[ModelFile] = &[
    ModelFile {
        file: SEGMENTATION_MODEL,
        url: "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/main/model.onnx",
        sha256: "220ad67ca923bef2fa91f2390c786097bf305bceb5e261d4af67b38e938e1079",
        size: 5_992_913,
    },
    ModelFile {
        file: EMBEDDING_MODEL,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_resnet34_LM.onnx",
        sha256: "e9848563da86f263117134dfd7ad63c92355b37de492b55e325400c9d9c39012",
        size: 26_530_550,
    },
];

#[derive(Debug, Clone, Serialize)]
struct DownloadProgress {
    progress_percentage: u32,
    status: String,
}

fn sha256_of(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(format!("{:x}", Sha256::digest(&bytes)))
}

/// Whether a model file is present and intact
fn is_ready(dir: &Path, f: &ModelFile) -> bool {
    let path = dir.join(f.file);
    std::fs::metadata(&path).map(|m| m.len() == f.size).unwrap_or(false) && sha256_of(&path).as_deref() == Some(f.sha256)
}

pub async fn download<R: Runtime>(app: &AppHandle<R>) -> Result<()> {
    let dir = app.path().app_data_dir()?.join("models").join("speakers");
    tokio::fs::create_dir_all(&dir).await?;
    let total: u64 = FILES.iter().map(|f| f.size).sum();
    let mut done: u64 = 0;
    let client = reqwest::Client::new();
    for f in FILES {
        if is_ready(&dir, f) {
            done += f.size;
            continue;
        }
        info!("Downloading speaker model {}", f.file);
        let tmp = dir.join(format!("{}.part", f.file));
        let response = client.get(f.url).send().await?.error_for_status()?;
        let mut out = tokio::fs::File::create(&tmp).await?;
        let mut hasher = Sha256::new();
        let mut stream = response.bytes_stream();
        let mut last_pct = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            hasher.update(&chunk);
            out.write_all(&chunk).await?;
            done += chunk.len() as u64;
            let pct = (done * 100 / total.max(1)).min(99) as u32;
            if pct != last_pct {
                last_pct = pct;
                let _ = app.emit("speaker-models-progress", DownloadProgress { progress_percentage: pct, status: "downloading".into() });
            }
        }
        out.flush().await?;
        drop(out);
        let sum = format!("{:x}", hasher.finalize());
        if sum != f.sha256 {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(anyhow!("Модель {} пошкоджена під час завантаження", f.file));
        }
        tokio::fs::rename(&tmp, dir.join(f.file)).await?;
    }
    let _ = app.emit("speaker-models-progress", DownloadProgress { progress_percentage: 100, status: "completed".into() });
    Ok(())
}

#[tauri::command]
pub async fn download_speaker_models<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    download(&app).await.map_err(|e| e.to_string())
}
