//! The learning engine (`scripts/learning-engine`): installed once, only
//! when the user asks, from the app's public releases or, for a closed
//! network, from a file copied by hand. Either way the archive must carry a
//! valid signature by the key of the app's own updates (checked as the
//! updater checks it); nothing unsigned is unpacked.
//!
//! Installed, it is `learning/engine/` in the app's data folder, with a
//! `manifest.json` naming the program to run.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};

use anyhow::{anyhow, Context, Result};
use base64::Engine as _;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;

use super::runner::Trainer;

/// The engine archive and its signature (`<archive>.sig`, as `tauri signer sign` writes it)
pub const ARCHIVE: &str = "learning-engine-macos-arm64.tar.gz";
/// The public release that holds them
const RELEASE: &str = "https://github.com/pulmanovsky/protokol/releases/download/learning-engine-1";
/// About how much is downloaded (MB), for the question before it
pub const DOWNLOAD_MB: u32 = 70;

/// Percent of the download on now, -1 when none is
static DOWNLOADING: AtomicI32 = AtomicI32::new(-1);

/// How far the download on now is (percent), for Settings opened meanwhile
pub fn downloading() -> Option<u32> {
    u32::try_from(DOWNLOADING.load(Ordering::SeqCst)).ok()
}

/// Marks the download over however it ends
struct Downloading;

impl Drop for Downloading {
    fn drop(&mut self) {
        DOWNLOADING.store(-1, Ordering::SeqCst);
    }
}

/// `manifest.json`: the program path is relative to the engine folder;
/// `{engine}` in an argument stands for that folder
#[derive(Debug, Deserialize)]
struct Manifest {
    program: PathBuf,
    #[serde(default)]
    args: Vec<String>,
}

/// The trainer in an engine folder, when it is complete
pub fn trainer_in(dir: &Path) -> Option<Trainer> {
    let m: Manifest = serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).ok()?).ok()?;
    let program = dir.join(&m.program);
    if !program.is_file() {
        return None;
    }
    let here = dir.to_string_lossy().into_owned();
    let args = m.args.iter().map(|a| a.replace("{engine}", &here)).collect();
    Some(Trainer { program, args })
}

/// The installed trainer
pub fn trainer(learning: &Path) -> Option<Trainer> {
    trainer_in(&learning.join("engine"))
}

fn base64_text(s: &str) -> Result<String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(s.trim()).context("not base64")?;
    String::from_utf8(bytes).context("not text")
}

/// Whether `data` is signed with the key whose public half is `pubkey`
/// (both base64, as in the app's config and in `.sig` files)
pub fn verify(data: &[u8], signature: &str, pubkey: &str) -> Result<()> {
    let key = minisign_verify::PublicKey::decode(&base64_text(pubkey)?).map_err(|e| anyhow!("public key: {e}"))?;
    let sig = minisign_verify::Signature::decode(&base64_text(signature)?).map_err(|e| anyhow!("signature: {e}"))?;
    key.verify(data, &sig, true).map_err(|_| anyhow!("the signature does not match"))
}

/// The public key of the app's updates
fn update_key(app: &AppHandle) -> Result<String> {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("pubkey"))
        .and_then(|k| k.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("the app has no update key"))
}

/// Unpacks a checked archive as the engine: into `engine.part` first, then
/// in place of the old one only if it holds a complete engine
pub fn unpack(archive: &Path, learning: &Path) -> Result<()> {
    let part = learning.join("engine.part");
    let _ = std::fs::remove_dir_all(&part);
    std::fs::create_dir_all(&part)?;
    let status = std::process::Command::new("/usr/bin/tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(&part)
        .status()
        .context("tar did not start")?;
    if !status.success() || trainer_in(&part).is_none() {
        let _ = std::fs::remove_dir_all(&part);
        return Err(anyhow!("the archive holds no learning engine"));
    }
    let engine = learning.join("engine");
    let old = learning.join("engine.old");
    let _ = std::fs::remove_dir_all(&old);
    if engine.exists() {
        std::fs::rename(&engine, &old)?;
    }
    std::fs::rename(&part, &engine)?;
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// Checks an archive against its signature text and installs it
fn install_checked(archive: &Path, signature: &str, pubkey: &str, learning: &Path) -> Result<()> {
    let data = std::fs::read(archive).context("read the archive")?;
    verify(&data, signature, pubkey)?;
    drop(data);
    unpack(archive, learning)
}

#[derive(Debug, Clone, Serialize)]
struct Progress {
    percent: u32,
}

async fn download(app: &AppHandle, learning: &Path) -> Result<()> {
    std::fs::create_dir_all(learning)?;
    let key = update_key(app)?;
    let client = reqwest::Client::new();
    let signature = client.get(format!("{RELEASE}/{ARCHIVE}.sig")).send().await?.error_for_status()?.text().await?;
    let response = client.get(format!("{RELEASE}/{ARCHIVE}")).send().await?.error_for_status()?;
    let total = response.content_length().unwrap_or(DOWNLOAD_MB as u64 * 1024 * 1024);
    let tmp = learning.join(format!("{ARCHIVE}.part"));
    let mut out = tokio::fs::File::create(&tmp).await?;
    let mut stream = response.bytes_stream();
    let (mut done, mut last) = (0u64, 0u32);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        out.write_all(&chunk).await?;
        done += chunk.len() as u64;
        let percent = (done * 100 / total.max(1)).min(99) as u32;
        if percent != last {
            last = percent;
            DOWNLOADING.store(percent as i32, Ordering::SeqCst);
            let _ = app.emit("learning-engine-progress", Progress { percent });
        }
    }
    out.flush().await?;
    drop(out);
    let learning2 = learning.to_path_buf();
    let tmp2 = tmp.clone();
    let result = tokio::task::spawn_blocking(move || install_checked(&tmp2, &signature, &key, &learning2)).await?;
    let _ = std::fs::remove_file(&tmp);
    result?;
    let _ = app.emit("learning-engine-progress", Progress { percent: 100 });
    Ok(())
}

fn learning_dir(app: &AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;
    Ok(app.path().app_data_dir().map_err(|e| e.to_string())?.join("learning"))
}

/// Downloads and installs the engine from the public release (the user agreed to the download)
#[tauri::command]
pub async fn api_learning_engine_download(app: AppHandle) -> Result<(), String> {
    let learning = learning_dir(&app)?;
    if DOWNLOADING.compare_exchange(-1, 0, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return Err("the engine is being downloaded already".into());
    }
    let _over = Downloading;
    log::info!("learning: downloading the engine");
    download(&app, &learning).await.map_err(|e| {
        log::warn!("learning: the engine was not installed: {e}");
        e.to_string()
    })
}

/// Installs the engine from an archive copied by hand (a closed network);
/// its signature `<archive>.sig` must lie next to it
#[tauri::command]
pub async fn api_learning_engine_install_file(app: AppHandle, path: String) -> Result<(), String> {
    let learning = learning_dir(&app)?;
    let key = update_key(&app).map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || {
        let archive = PathBuf::from(path);
        let sig_path = PathBuf::from(format!("{}.sig", archive.display()));
        let signature = std::fs::read_to_string(&sig_path).map_err(|_| anyhow!("the signature file {ARCHIVE}.sig is not next to the archive"))?;
        std::fs::create_dir_all(&learning)?;
        install_checked(&archive, &signature, &key, &learning)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// «З файлу…»: the user picks the archive copied by hand; `false` when
/// nothing was picked
#[tauri::command]
pub async fn api_learning_engine_choose_file(app: AppHandle) -> Result<bool, String> {
    use tauri_plugin_dialog::DialogExt;
    let picker = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        picker.dialog().file().set_title("Модуль навчання").add_filter("Модуль навчання", &["gz"]).blocking_pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(false) };
    api_learning_engine_install_file(app, path.to_string_lossy().into_owned()).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test key (not the app's) and its signature of `DATA`, made as `tauri signer sign` makes them
    const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDA4MDcwNjA1MDQwMzAyMDEKUldRQkFnTUVCUVlIQ0FPaEI3L3p6aEMrSFhEZEdPZEx3SmxuNU5Zd202VU5YeDNjaG1RU1ZURzQK";
    const SIGNATURE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRQkFnTUVCUVlIQ0ZIU1hFek9McnRwNDZyMkloNkl0K2NiaEhGbit2R3o2WkoybktyTktVYnMvRHNKMEdMNTVCUkpjNnhGa3NtVHpKeFl1aUhwbjB4d2FPaUMvSTV4dFFvPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzYzMjAwMDAwCWZpbGU6bGVhcm5pbmctZW5naW5lLW1hY29zLWFybTY0LnRhci5negloYXNoZWQKWVFFSHdaWkx2bzQxdk5yalNHVDdNVGNYYmZpbm5sdjU1Ujd3a1hoZ3pTMU5qTG5ENk1DTWZqM2QzNEtlbkN4Wm9Qb3QwdW8zM3VMdUFUSWUwVzNPQmc9PQo=";
    const DATA: &[u8] = b"protokol learning engine test archive";

    #[test]
    fn only_a_signed_archive_passes() {
        assert!(verify(DATA, SIGNATURE, PUBKEY).is_ok());
        assert!(verify(b"protokol learning engine test archivE", SIGNATURE, PUBKEY).is_err());
        assert!(verify(DATA, "not a signature", PUBKEY).is_err());
    }

    #[test]
    fn the_app_update_key_is_a_minisign_key() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let key = conf["plugins"]["updater"]["pubkey"].as_str().unwrap();
        assert!(minisign_verify::PublicKey::decode(&base64_text(key).unwrap()).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn an_archive_is_unpacked_only_when_it_holds_an_engine() {
        let root = std::env::temp_dir().join(format!("protokol-engine-unpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("src");
        std::fs::create_dir_all(src.join("python/bin")).unwrap();
        std::fs::write(src.join("python/bin/python3"), "").unwrap();
        std::fs::write(src.join("protokol_train.py"), "").unwrap();
        let learning = root.join("learning");
        std::fs::create_dir_all(&learning).unwrap();
        let tar = |name: &str| {
            let a = root.join(name);
            assert!(std::process::Command::new("/usr/bin/tar").arg("-czf").arg(&a).arg("-C").arg(&src).arg(".").status().unwrap().success());
            a
        };
        // No manifest: refused, nothing installed
        let bad = tar("bad.tar.gz");
        assert!(unpack(&bad, &learning).is_err());
        assert!(trainer(&learning).is_none() && !learning.join("engine.part").exists());
        std::fs::write(src.join("manifest.json"), r#"{"program": "python/bin/python3", "args": ["-I", "{engine}/protokol_train.py"]}"#).unwrap();
        let good = tar("good.tar.gz");
        unpack(&good, &learning).unwrap();
        let t = trainer(&learning).unwrap();
        assert_eq!(t.program, learning.join("engine/python/bin/python3"));
        assert_eq!(t.args[1], format!("{}/protokol_train.py", learning.join("engine").display()));
        // A newer one replaces it
        unpack(&good, &learning).unwrap();
        assert!(trainer(&learning).is_some() && !learning.join("engine.old").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
