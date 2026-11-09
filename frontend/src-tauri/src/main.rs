#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use log;

fn main() {
    std::env::set_var("RUST_LOG", "info");
    // The terminal and ~/Library/Logs/ua.protokol.app/protokol.log
    app_lib::logfile::init();
    // Data of an earlier build (identifier com.meetily.ai), before any window opens
    app_lib::identity::migrate_from_old_identifier();

    // Async logger will be initialized lazily when first needed (after Tauri runtime starts)
    log::info!("Starting application...");
    app_lib::run();
}
