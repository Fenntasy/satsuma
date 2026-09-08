//! Satsuma desktop backend.

pub mod commands;
pub mod db;
pub mod grouping;
pub mod scanner;
pub mod tags;

use std::path::Path;

use tauri::Manager;

use commands::AppState;

/// Opens the library cache, recreating it when the existing file cannot be
/// used (corrupted, or written by a newer version of Satsuma).
///
/// # Errors
///
/// Returns an error only when a fresh database cannot be created either.
fn open_library(path: &Path) -> Result<db::Db, db::DbError> {
    match db::Db::open(path) {
        Ok(db) => Ok(db),
        Err(err) => {
            log::warn!("cannot open the library cache ({err}); starting a new one");
            let backup = path.with_extension("sqlite.unusable");
            for suffix in ["", "-wal", "-shm"] {
                let from = with_suffix(path, suffix);
                if from.exists() {
                    let _ = std::fs::rename(&from, with_suffix(&backup, suffix));
                }
            }
            db::Db::open(path)
        }
    }
}

fn with_suffix(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

/// Builds and runs the Tauri application.
///
/// # Panics
///
/// Panics if the Tauri runtime fails to start.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db = open_library(&data_dir.join("library.sqlite"))?;
            app.manage(AppState::new(db));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::list_folders,
            commands::add_folder,
            commands::remove_folder,
            commands::library_stats,
            commands::pick_folder,
            commands::start_scan,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Satsuma");
}
