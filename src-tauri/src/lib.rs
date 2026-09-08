//! Satsuma desktop backend.

pub mod commands;
pub mod db;
pub mod grouping;
pub mod scanner;
pub mod tags;

use tauri::Manager;

use commands::AppState;

/// Builds and runs the Tauri application.
///
/// # Panics
///
/// Panics if the Tauri runtime fails to start.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some("satsuma".to_owned()),
                    }),
                ])
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db = db::Db::open_or_recreate(&data_dir.join("library.sqlite"))?;
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
