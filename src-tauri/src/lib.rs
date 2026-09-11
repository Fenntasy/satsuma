//! Satsuma desktop backend.

pub mod commands;
pub mod cover;
pub mod db;
pub mod grouping;
pub mod player;
pub mod queue;
pub mod scanner;
pub mod settings;
pub mod tags;

use std::sync::mpsc;

use tauri::{Emitter, Manager};

use commands::AppState;

/// Puts back the library folders the settings file remembers but the
/// database does not, which is what happens when an unusable cache was
/// recreated: the folders are the user's choice, not derived data.
fn restore_folders(db: &db::Db, settings_path: &std::path::Path) {
    let remembered = match settings::load(settings_path) {
        Ok(settings) => settings.folders,
        Err(err) => {
            log::warn!("cannot restore the library folders ({err})");
            return;
        }
    };
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

/// Starts the thread that owns the audio output and returns the handle used
/// to talk to it. Its state updates are forwarded to the frontend.
fn start_player(app: &tauri::AppHandle) -> player::Handle {
    let (sender, receiver) = mpsc::channel();
    let notify = sender.clone();
    let handle = player::Handle::new(sender);
    let alive = handle.liveness();
    let app = app.clone();
    std::thread::Builder::new()
        .name("satsuma-player".to_owned())
        .spawn(move || {
            player::run(&receiver, &notify, &alive, |state| {
                if let Err(err) = app.emit(commands::PLAYER_STATE_EVENT, &state) {
                    log::warn!("cannot report the player state: {err}");
                }
            });
        })
        .expect("cannot start the player thread");
    handle
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
        // Covers are served rather than sent: the bytes stay out of the
        // IPC messages, and the webview keeps the one it has instead of
        // asking again for every track of the same record.
        .register_asynchronous_uri_scheme_protocol("satsuma-cover", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_owned();
            // Off the main thread: this reads a tag out of a music file,
            // which is not something to do while the window waits.
            std::thread::spawn(move || {
                let state = app.state::<AppState>();
                responder.respond(state.with_library(|db| cover::respond(db, &path)));
            });
        })
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db = db::Db::open_or_recreate(&data_dir.join("library.sqlite"))?;
            let settings_path = data_dir.join(settings::FILE_NAME);
            restore_folders(&db, &settings_path);
            app.manage(AppState::new(db, settings_path, start_player(app.handle())));
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
            commands::library_rows,
            commands::track_details,
            commands::list_playlists,
            commands::create_playlist,
            commands::rename_playlist,
            commands::delete_playlist,
            commands::add_to_playlist,
            commands::set_rating,
            commands::play_tracks,
            commands::enqueue_tracks,
            commands::play_library,
            commands::player_state,
            commands::player_play_pause,
            commands::player_stop,
            commands::player_next,
            commands::player_previous,
            commands::player_seek,
            commands::player_set_volume,
            commands::player_set_shuffle,
            commands::player_set_repeat,
            commands::player_set_stop_after_current,
            commands::player_play_next,
            commands::player_jump_to,
            commands::enqueue_library,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Satsuma");
}
