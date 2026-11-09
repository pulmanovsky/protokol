//! Updates: the app asks one address (Settings; at first the release feed in
//! tauri.conf.json, looked at once a day until turned off) whether a newer
//! version is out — a single GET of a small manifest, with nothing about the
//! user or the Mac in it — and installs it on the user's word. Every update is
//! checked against the signing key built into the app, so it does not matter
//! where it came from (a website, the organisation's server, a USB stick).
//! For closed networks an update can be installed from a folder: the app
//! serves that folder to itself on 127.0.0.1 for as long as it takes.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_updater::UpdaterExt;

use crate::database::repositories::setting::SettingsRepository;

const URL_FLAG: &str = "update_url";
const AUTO_FLAG: &str = "update_auto";
const LAST_FLAG: &str = "update_last_check";
/// A new install looks once a day until the user turns it off
const DEFAULT_AUTO: bool = true;

/// The address an install starts with: the release feed in tauri.conf.json
/// (plugins.updater.endpoints); an organisation can put its own in Settings
fn default_url<R: Runtime>(app: &AppHandle<R>) -> String {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("endpoints"))
        .and_then(|e| e.get(0))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Settings as stored, or the defaults where the user never chose; an
/// address the user cleared stays empty
fn resolve(url: Option<String>, auto: Option<String>, default_url: &str) -> (String, bool) {
    (url.unwrap_or_else(|| default_url.to_string()), auto.map_or(DEFAULT_AUTO, |a| a == "1"))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSettings {
    /// The manifest's address (latest.json); empty: not set
    pub url: String,
    /// Look once a day
    pub auto: bool,
    /// When it was last looked (RFC 3339)
    pub last_check: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Available {
    pub version: String,
    pub current: String,
    pub notes: Option<String>,
    pub date: Option<String>,
}

/// The update found last, kept to install it
static FOUND: Mutex<Option<tauri_plugin_updater::Update>> = Mutex::new(None);
/// The local server of an offline update (stops when the app quits)
static LOCAL: Mutex<Option<(PathBuf, u16)>> = Mutex::new(None);

#[tauri::command]
pub async fn api_update_settings<R: Runtime>(app: AppHandle<R>) -> Result<UpdateSettings, String> {
    let pool = crate::speakers::commands::pool(&app).map_err(|e| e.to_string())?;
    let get = |k: &'static str| {
        let pool = pool.clone();
        async move { SettingsRepository::get_flag(&pool, k).await.ok().flatten() }
    };
    let (url, auto) = resolve(get(URL_FLAG).await, get(AUTO_FLAG).await, &default_url(&app));
    Ok(UpdateSettings { url, auto, last_check: get(LAST_FLAG).await })
}

#[tauri::command]
pub async fn api_set_update_settings<R: Runtime>(app: AppHandle<R>, url: String, auto: bool) -> Result<(), String> {
    let pool = crate::speakers::commands::pool(&app).map_err(|e| e.to_string())?;
    let url = url.trim().to_string();
    if !url.is_empty() {
        let parsed = url::Url::parse(&url).map_err(|_| "Адреса має бути на кшталт https://…/latest.json".to_string())?;
        if !matches!(parsed.scheme(), "https" | "http") {
            return Err("Адреса має починатися з https:// або http://".into());
        }
    }
    SettingsRepository::set_flag(&pool, URL_FLAG, &url).await.map_err(|e| e.to_string())?;
    SettingsRepository::set_flag(&pool, AUTO_FLAG, if auto { "1" } else { "0" }).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Is a newer version at `url` (or the address in Settings)? None: up to date
#[tauri::command]
pub async fn api_update_check<R: Runtime>(app: AppHandle<R>, url: Option<String>) -> Result<Option<Available>, String> {
    let pool = crate::speakers::commands::pool(&app).map_err(|e| e.to_string())?;
    let url = match url.filter(|u| !u.trim().is_empty()) {
        Some(u) => u,
        None => resolve(SettingsRepository::get_flag(&pool, URL_FLAG).await.ok().flatten(), None, &default_url(&app)).0,
    };
    if url.trim().is_empty() {
        return Err("Адресу оновлень не вказано".into());
    }
    let found = check_at(&app, &url).await?;
    let _ = SettingsRepository::set_flag(&pool, LAST_FLAG, &chrono::Utc::now().to_rfc3339()).await;
    Ok(found)
}

async fn check_at<R: Runtime>(app: &AppHandle<R>, url: &str) -> Result<Option<Available>, String> {
    let endpoint = url::Url::parse(url.trim()).map_err(|_| "Неправильна адреса оновлень".to_string())?;
    let updater = app
        .updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|e| e.to_string())?
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    match updater.check().await {
        Ok(Some(update)) => {
            let found = Available {
                version: update.version.clone(),
                current: update.current_version.clone(),
                notes: update.body.clone(),
                date: update.date.map(|d| d.to_string()),
            };
            *FOUND.lock().map_err(|e| e.to_string())? = Some(update);
            Ok(Some(found))
        }
        Ok(None) => Ok(None),
        Err(e) => {
            log::warn!("Update check failed: {e}");
            Err(format!("Не вдалося перевірити оновлення: {e}"))
        }
    }
}

/// The update found last (by the daily check or «Перевірити зараз») and not
/// installed yet, so Settings shows it without asking again
#[tauri::command]
pub fn api_update_found() -> Option<Available> {
    FOUND.lock().ok()?.as_ref().map(|u| Available {
        version: u.version.clone(),
        current: u.current_version.clone(),
        notes: u.body.clone(),
        date: u.date.map(|d| d.to_string()),
    })
}

#[derive(Clone, Serialize)]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
}

/// Downloads the update found, checks its signature and puts it in place of
/// the app; the app is restarted after that (by the user)
#[tauri::command]
pub async fn api_update_install<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    if crate::audio::recording_commands::recording_now() {
        return Err("Іде запис — оновлення встановиться після нього".into());
    }
    let update = FOUND.lock().map_err(|e| e.to_string())?.take().ok_or("Спершу перевірте оновлення")?;
    let mut downloaded = 0u64;
    let app2 = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                let _ = app2.emit("update-progress", Progress { downloaded, total });
            },
            || {},
        )
        .await
        .map_err(|e| {
            log::warn!("Update not installed: {e}");
            format!("Оновлення не встановлено: {e}")
        })?;
    log::info!("Update installed; restart to use it");
    Ok(())
}

/// An update from a folder (a USB stick, a shared drive): the folder must hold
/// latest.json and the archive it names; checked like any other
#[tauri::command]
pub async fn api_update_from_folder<R: Runtime>(app: AppHandle<R>, folder: String) -> Result<Option<Available>, String> {
    let folder = PathBuf::from(folder);
    if !folder.join("latest.json").is_file() {
        return Err("У папці немає latest.json".into());
    }
    let port = serve_folder(&folder).map_err(|e| format!("Не вдалося відкрити папку: {e}"))?;
    check_at(&app, &format!("http://127.0.0.1:{port}/latest.json")).await
}

/// Serves `folder` on 127.0.0.1 (a free port) to this app only; latest.json is
/// given with its archive addresses pointing here
fn serve_folder(folder: &Path) -> std::io::Result<u16> {
    let mut local = LOCAL.lock().map_err(|_| std::io::Error::other("lock"))?;
    if let Some((f, port)) = local.as_ref() {
        if f == folder {
            return Ok(*port);
        }
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let root = folder.to_path_buf();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let root = root.clone();
            std::thread::spawn(move || {
                let _ = answer(stream, &root, port);
            });
        }
    });
    *local = Some((folder.to_path_buf(), port));
    Ok(port)
}

fn answer(mut stream: std::net::TcpStream, root: &Path, port: u16) -> std::io::Result<()> {
    // The whole request head: it may come in several pieces, and answering
    // a piece would serve the wrong file (or reset the connection)
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf);
    let path = head.split_whitespace().nth(1).unwrap_or("/");
    let name = path.trim_start_matches('/').split('?').next().unwrap_or("");
    // Only a plain file of that folder
    let ok_name = !name.is_empty() && !name.contains('/') && !name.contains("..") && !name.starts_with('.');
    let file = root.join(name);
    if !ok_name || !file.is_file() {
        stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
        return Ok(());
    }
    let mut body = std::fs::read(&file)?;
    if name == "latest.json" {
        body = local_manifest(&body, port).unwrap_or(body);
    }
    let kind = if name.ends_with(".json") { "application/json" } else { "application/octet-stream" };
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())?;
    stream.write_all(&body)
}

/// latest.json with each platform's archive address pointing at the local
/// server (the archive's file name kept)
pub(crate) fn local_manifest(body: &[u8], port: u16) -> Option<Vec<u8>> {
    let mut v: serde_json::Value = serde_json::from_slice(body).ok()?;
    for (_, p) in v.get_mut("platforms")?.as_object_mut()? {
        let url = p.get("url")?.as_str()?.to_string();
        let file = url.rsplit('/').next()?.split('?').next()?.to_string();
        p["url"] = serde_json::Value::String(format!("http://127.0.0.1:{port}/{file}"));
    }
    serde_json::to_vec(&v).ok()
}

/// Once a day, when allowed, in the background: tells the window if a newer
/// version is out (it is installed only on the user's word)
pub fn check_daily<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        loop {
            if let Ok(s) = api_update_settings(app.clone()).await {
                let due = s.last_check.as_deref().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|t| chrono::Utc::now().signed_duration_since(t) > chrono::Duration::hours(23)).unwrap_or(true);
                if s.auto && !s.url.is_empty() && due && !crate::audio::recording_commands::recording_now() {
                    if let Ok(Some(found)) = api_update_check(app.clone(), None).await {
                        let _ = app.emit("update-available", found);
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_new_install_uses_the_release_feed_and_looks_daily() {
        let feed = "https://example.org/releases/latest/download/latest.json";
        assert_eq!(super::resolve(None, None, feed), (feed.to_string(), true));
        // What the user chose stays, a cleared address too
        assert_eq!(super::resolve(Some(String::new()), Some("0".into()), feed), (String::new(), false));
        assert_eq!(super::resolve(Some("https://org.example/latest.json".into()), Some("1".into()), feed), ("https://org.example/latest.json".to_string(), true));
    }

    #[test]
    fn an_offline_manifest_points_at_the_local_server() {
        let body = br#"{"version":"0.2.31","notes":"x","platforms":{"darwin-aarch64":{"signature":"sig","url":"https://example.org/r/Protokol_0.2.31_aarch64.app.tar.gz"}}}"#;
        let out = super::local_manifest(body, 4321).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["platforms"]["darwin-aarch64"]["url"], "http://127.0.0.1:4321/Protokol_0.2.31_aarch64.app.tar.gz");
        assert_eq!(v["platforms"]["darwin-aarch64"]["signature"], "sig");
        assert_eq!(v["version"], "0.2.31");
    }

    #[test]
    fn the_local_server_gives_only_files_of_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("latest.json"), br#"{"version":"1.0.0","platforms":{"darwin-aarch64":{"signature":"s","url":"https://h/a.tar.gz"}}}"#).unwrap();
        std::fs::write(dir.path().join("a.tar.gz"), b"archive").unwrap();
        let port = super::serve_folder(dir.path()).unwrap();
        let get = |p: &str| -> (u16, Vec<u8>) {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            use std::io::{Read, Write};
            write!(s, "GET {p} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
            let mut r = Vec::new();
            s.read_to_end(&mut r).unwrap();
            let text = String::from_utf8_lossy(&r).to_string();
            let code = text.split_whitespace().nth(1).unwrap().parse().unwrap();
            let body = text.split("\r\n\r\n").nth(1).unwrap_or("").as_bytes().to_vec();
            (code, body)
        };
        assert_eq!(get("/a.tar.gz"), (200, b"archive".to_vec()));
        assert!(String::from_utf8(get("/latest.json").1).unwrap().contains(&format!("http://127.0.0.1:{port}/a.tar.gz")));
        assert_eq!(get("/../etc/passwd").0, 404);
        assert_eq!(get("/nothing").0, 404);
    }
}
