//! Satsuma desktop backend.

pub mod commands;
pub mod db;
pub mod grouping;
pub mod scanner;
pub mod settings;
pub mod tags;

use tauri::Manager;

use commands::AppState;

/// Puts back the library folders the settings file remembers but the
/// database does not, which is what happens when an unusable cache was
/// recreated: the folders are the user's choice, not derived data.
fn restore_folders(db: &db::Db, settings_path: &std::path::Path) {
    let remembered = settings::read(settings_path).folders;
    if remembered.is_empty() {
        return;
    }
    let known: Vec<String> = match db.list_folders() {
        Ok(folders) => folders.into_iter().map(|folder| folder.path).collect(),
        Err(err) => {
            log::warn!("cannot list the library folders ({err})");
            return;
        }
    };
    for path in remembered {
        if !known.contains(&path) {
            match db.add_folder(&path) {
                Ok(_) => log::info!("restored the library folder {path}"),
                Err(err) => log::warn!("cannot restore the library folder {path} ({err})"),
            }
        }
    }
}

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
            let settings_path = data_dir.join(settings::FILE_NAME);
            restore_folders(&db, &settings_path);
            app.manage(AppState::new(db, settings_path));
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
