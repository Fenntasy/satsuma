//! Tauri commands exposed to the frontend.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::db::{Db, Folder, LibraryRow, LibraryStats};
use crate::player::{self, Handle};
use crate::queue::Repeat;
use crate::scanner::{self, ScanProgress, ScanReport};
use crate::settings;

pub const SCAN_PROGRESS_EVENT: &str = "library://scan-progress";
pub const SCAN_FINISHED_EVENT: &str = "library://scan-finished";
pub const PLAYER_STATE_EVENT: &str = "player://state";
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Whether a scan is running, and whether another one was asked for while
/// it was.
#[derive(Debug, Default)]
struct ScanStatus {
    running: bool,
    rescan_requested: bool,
}

impl ScanStatus {
    /// Claims the scan slot. Returns `true` when the caller has to run the
    /// scan, `false` when one is already running and another pass has been
    /// queued instead.
    fn claim(&mut self) -> bool {
        if self.running {
            self.rescan_requested = true;
            false
        } else {
            self.running = true;
            true
        }
    }

    /// Ends a pass. Returns `true` when another pass was requested while it
    /// was running, in which case the slot stays claimed.
    fn finish_pass(&mut self) -> bool {
        if self.rescan_requested {
            self.rescan_requested = false;
            true
        } else {
            self.running = false;
            false
        }
    }

    /// Releases the slot after a scan that ended unexpectedly.
    fn abort(&mut self) {
        self.running = false;
        self.rescan_requested = false;
    }
}

/// Locks the scan status, recovering from a poisoned lock: the status is two
/// booleans with no invariant a panic could break, and refusing to touch it
/// would leave scanning disabled for the rest of the session.
fn lock_status(status: &Mutex<ScanStatus>) -> MutexGuard<'_, ScanStatus> {
    status.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Shared application state managed by Tauri.
pub struct AppState {
    pub db: Mutex<Db>,
    scan: Arc<Mutex<ScanStatus>>,
    settings_path: PathBuf,
    /// Held across reading, changing and writing the settings file: the
    /// commands run on a thread pool and would otherwise lose each other's
    /// changes.
    settings_lock: Mutex<()>,
    player: Handle,
}

impl AppState {
    #[must_use]
    pub fn new(db: Db, settings_path: PathBuf, player: Handle) -> Self {
        AppState {
            db: Mutex::new(db),
            scan: Arc::new(Mutex::new(ScanStatus::default())),
            settings_path,
            settings_lock: Mutex::new(()),
            player,
        }
    }

    /// Mirrors the library folders to the settings file, so they survive a
    /// cache that has to be recreated.
    fn remember_folders(&self) {
        let folders = match self.with_db(Db::list_folders) {
            Ok(folders) => folders,
            Err(err) => {
                log::warn!("cannot list the library folders ({err})");
                return;
            }
        };
        if let Err(err) = self.update_settings(|settings| {
            settings.folders = folders.into_iter().map(|folder| folder.path).collect();
        }) {
            log::warn!("cannot save the library folders ({err})");
        }
    }

    /// What the settings file holds today.
    fn settings(&self) -> settings::Settings {
        settings::read(&self.settings_path)
    }

    /// Changes the settings file, keeping everything `change` does not
    /// touch: writing a whole `Settings` built from one part would drop the
    /// others.
    fn update_settings(&self, change: impl FnOnce(&mut settings::Settings)) -> Result<(), String> {
        let _guard = self
            .settings_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut settings = self.settings();
        change(&mut settings);
        settings::write(&self.settings_path, &settings).map_err(|err| err.to_string())
    }

    /// Runs `f` against the library cache. A poisoned lock is recovered: a
    /// panic under the lock leaves the connection usable (rusqlite rolls a
    /// transaction back when it is dropped), and refusing to touch it would
    /// disable the library for the rest of the session.
    fn with_db<T>(&self, f: impl FnOnce(&Db) -> crate::db::Result<T>) -> Result<T, String> {
        let db = self.db.lock().unwrap_or_else(PoisonError::into_inner);
        f(&db).map_err(|err| err.to_string())
    }
}

/// The reply returned by [`ping`], used by the frontend to confirm the
/// backend is reachable.
#[must_use]
pub fn ping_reply() -> String {
    format!("satsuma {}", env!("CARGO_PKG_VERSION"))
}

/// Health check command: returns the backend name and version.
#[tauri::command]
#[must_use]
pub fn ping() -> String {
    ping_reply()
}

/// # Errors
///
/// Returns a message when the database cannot be read.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn list_folders(state: State<'_, AppState>) -> Result<Vec<Folder>, String> {
    state.with_db(Db::list_folders)
}

/// # Errors
///
/// Returns a message when the folder is already in the library or the
/// database cannot be written.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn add_folder(state: State<'_, AppState>, path: String) -> Result<Folder, String> {
    let folder = state.with_db(|db| db.add_folder(&path))?;
    state.remember_folders();
    Ok(folder)
}

/// # Errors
///
/// Returns a message when the database cannot be written.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn remove_folder(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state.with_db(|db| db.remove_folder(id))?;
    state.remember_folders();
    Ok(())
}

/// # Errors
///
/// Returns a message when the database cannot be read.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn library_stats(state: State<'_, AppState>) -> Result<LibraryStats, String> {
    state.with_db(Db::stats)
}

/// Opens the native folder picker and returns the chosen path, if any.
///
/// # Errors
///
/// Returns a message when the dialog thread cannot be joined.
#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    let picked =
        tauri::async_runtime::spawn_blocking(move || app.dialog().file().blocking_pick_folder())
            .await
            .map_err(|err| err.to_string())?;
    Ok(picked.map(|path| path.to_string()))
}

/// Starts a scan of every library folder in the background. Progress is
/// reported through [`SCAN_PROGRESS_EVENT`] and the final report through
/// [`SCAN_FINISHED_EVENT`].
///
/// When a scan is already running, another pass is queued instead: folders
/// added meanwhile are picked up as soon as the current pass ends.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn start_scan(app: AppHandle, state: State<'_, AppState>) {
    let scan = Arc::clone(&state.scan);
    if !lock_status(&scan).claim() {
        return;
    }
    // Built before spawning: if the task is never run, dropping it still
    // releases the slot and tells the frontend.
    let guard = ScanGuard::new(app.clone(), scan);
    tauri::async_runtime::spawn_blocking(move || run_scan(&app, guard));
}

/// Releases the scan slot and tells the frontend if the scan thread dies
/// without reporting a result of its own.
struct ScanGuard {
    app: AppHandle,
    scan: Arc<Mutex<ScanStatus>>,
    handed_over: bool,
}

impl ScanGuard {
    fn new(app: AppHandle, scan: Arc<Mutex<ScanStatus>>) -> Self {
        ScanGuard {
            app,
            scan,
            handed_over: false,
        }
    }
}

impl Drop for ScanGuard {
    fn drop(&mut self) {
        if !self.handed_over {
            lock_status(&self.scan).abort();
            emit(
                &self.app,
                SCAN_FINISHED_EVENT,
                &ScanOutcome::Failed {
                    message: "the scan stopped unexpectedly".to_owned(),
                },
            );
        }
    }
}

fn run_scan(app: &AppHandle, mut guard: ScanGuard) {
    let state = app.state::<AppState>();
    loop {
        let outcome = scan_once(app, &state);
        // Decide whether to run again while still holding the lock, so a
        // request that arrives now is either seen here or starts its own
        // scan afterwards, never dropped in between.
        let mut status = lock_status(&guard.scan);
        if status.finish_pass() {
            continue;
        }
        // Only the last pass is reported, and the slot stays held until the
        // report is out: telling the frontend a scan finished while another
        // one is starting would make the panel flip between the two.
        emit(app, SCAN_FINISHED_EVENT, &outcome);
        guard.handed_over = true;
        return;
    }
}

fn scan_once(app: &AppHandle, state: &State<'_, AppState>) -> ScanOutcome {
    let mut last_emit: Option<Instant> = None;
    let result = scanner::scan(&state.db, |progress: ScanProgress| {
        let done = progress.scanned == progress.total;
        if done || last_emit.is_none_or(|at| at.elapsed() >= PROGRESS_INTERVAL) {
            last_emit = Some(Instant::now());
            emit(app, SCAN_PROGRESS_EVENT, &progress);
        }
    });
    match result {
        Ok(report) => ScanOutcome::Finished(report),
        Err(err) => ScanOutcome::Failed {
            message: err.to_string(),
        },
    }
}

// PLAYER

/// A playlist as the frontend shows it: its tracks resolved against the
/// library, so a file that left it simply disappears from the list.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PlaylistView {
    pub id: u32,
    pub name: String,
    pub tracks: Vec<LibraryRow>,
}

/// The playlists, with their tracks looked up in the library.
///
/// # Errors
///
/// Returns a message when the library cannot be read.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn list_playlists(state: State<'_, AppState>) -> Result<Vec<PlaylistView>, String> {
    let saved = state.settings().playlists;
    let rows = state.with_db(Db::library_rows)?;
    let by_path: HashMap<&str, &LibraryRow> =
        rows.iter().map(|row| (row.path.as_str(), row)).collect();
    Ok(saved
        .into_iter()
        .map(|playlist| PlaylistView {
            id: playlist.id,
            name: playlist.name,
            tracks: playlist
                .tracks
                .iter()
                .filter_map(|path| by_path.get(path.as_str()).map(|row| (*row).clone()))
                .collect(),
        })
        .collect())
}

/// Adds a playlist and answers with its id.
///
/// # Errors
///
/// Returns a message when the settings cannot be written.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn create_playlist(state: State<'_, AppState>, name: String) -> Result<u32, String> {
    let mut id = 0;
    state.update_settings(|settings| {
        id = settings
            .playlists
            .iter()
            .map(|playlist| playlist.id)
            .max()
            .unwrap_or(0)
            + 1;
        settings.playlists.push(settings::Playlist {
            id,
            name: name.trim().to_owned(),
            tracks: Vec::new(),
        });
    })?;
    Ok(id)
}

/// # Errors
///
/// Returns a message when the name is empty.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn rename_playlist(state: State<'_, AppState>, id: u32, name: String) -> Result<(), String> {
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err("a playlist needs a name".to_owned());
    }
    state.update_settings(|settings| {
        if let Some(playlist) = settings.playlists.iter_mut().find(|p| p.id == id) {
            playlist.name = name;
        }
    })
}

/// # Errors
///
/// Returns a message when the settings cannot be written.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn delete_playlist(state: State<'_, AppState>, id: u32) -> Result<(), String> {
    state.update_settings(|settings| {
        settings.playlists.retain(|playlist| playlist.id != id);
    })
}

/// Adds tracks at the end of a playlist.
///
/// # Errors
///
/// Returns a message when the library cannot be read.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn add_to_playlist(state: State<'_, AppState>, id: u32, ids: Vec<i64>) -> Result<(), String> {
    let tracks = state.with_db(|db| db.tracks_by_ids(&ids))?;
    state.update_settings(|settings| {
        if let Some(playlist) = settings.playlists.iter_mut().find(|p| p.id == id) {
            playlist
                .tracks
                .extend(tracks.iter().map(|track| track.path.clone()));
        }
    })
}

/// Replaces what a playlist holds, which is how a removal or a reorder is
/// saved.
///
/// # Errors
///
/// Returns a message when the library cannot be read.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn set_playlist_tracks(
    state: State<'_, AppState>,
    id: u32,
    ids: Vec<i64>,
) -> Result<(), String> {
    let tracks = state.with_db(|db| db.tracks_by_ids(&ids))?;
    state.update_settings(|settings| {
        if let Some(playlist) = settings.playlists.iter_mut().find(|p| p.id == id) {
            playlist.tracks = tracks.iter().map(|track| track.path.clone()).collect();
        }
    })
}

/// Rates a track, writing the stars into the file first so the music keeps
/// the rating, then into the cache.
///
/// # Errors
///
/// Returns a message when the track is unknown or the file cannot be
/// written.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn set_rating(state: State<'_, AppState>, id: i64, stars: Option<u8>) -> Result<(), String> {
    let track = state
        .with_db(|db| db.tracks_by_ids(&[id]))?
        .into_iter()
        .next()
        .ok_or_else(|| "that track is not in the library any more".to_owned())?;
    crate::tags::write_rating(std::path::Path::new(&track.path), stars)
        .map_err(|err| err.to_string())?;
    state.with_db(|db| db.set_rating(id, stars))
}

/// Everything the library tree groups by, for every track.
///
/// # Errors
///
/// Returns a message when the library cannot be read.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn library_rows(state: State<'_, AppState>) -> Result<Vec<LibraryRow>, String> {
    state.with_db(Db::library_rows)
}

/// Replaces the queue with these tracks, in this order, and plays them.
///
/// `start_id` is the one the user picked; the rest of the queue follows it,
/// so choosing a track from an album plays the album from there. Without
/// one, shuffle decides where to start.
///
/// # Errors
///
/// Returns a message when the library cannot be read or the player stopped.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn play_tracks(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    start_id: Option<i64>,
) -> Result<(), String> {
    let tracks = state.with_db(|db| db.tracks_by_ids(&ids))?;
    if tracks.is_empty() {
        return Err("none of those tracks are in the library any more".to_owned());
    }
    // Found by id rather than taken as an index: a track may have left the
    // library since the tree was built.
    let start = start_id.and_then(|id| tracks.iter().position(|track| track.id == id));
    state.player.send(player::Command::Play { tracks, start })
}

/// Adds these tracks to the end of the queue.
///
/// # Errors
///
/// Returns a message when the library cannot be read or the player stopped.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn enqueue_tracks(state: State<'_, AppState>, ids: Vec<i64>) -> Result<(), String> {
    let tracks = state.with_db(|db| db.tracks_by_ids(&ids))?;
    if tracks.is_empty() {
        return Err("none of those tracks are in the library any more".to_owned());
    }
    state.player.send(player::Command::Enqueue(tracks))
}

/// Queues every track in the library and starts playing.
///
/// # Errors
///
/// Returns a message when the library cannot be read or the player stopped.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn play_library(state: State<'_, AppState>) -> Result<(), String> {
    let tracks = state.with_db(|db| db.list_tracks(None))?;
    if tracks.is_empty() {
        return Err("the library is empty; scan a folder first".to_owned());
    }
    state.player.send(player::Command::Play {
        tracks,
        start: None,
    })
}

/// Asks the player to report where it is, e.g. when the window opens.
///
/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn player_state(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(player::Command::ReportState)
}

/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn player_play_pause(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(player::Command::PlayPause)
}

/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn player_stop(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(player::Command::Stop)
}

/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn player_next(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(player::Command::Next)
}

/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn player_previous(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(player::Command::Previous)
}

/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn player_seek(state: State<'_, AppState>, position_ms: u64) -> Result<(), String> {
    state
        .player
        .send(player::Command::Seek(Duration::from_millis(position_ms)))
}

/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn player_set_volume(state: State<'_, AppState>, volume: f32) -> Result<(), String> {
    state.player.send(player::Command::SetVolume(volume))
}

/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn player_set_shuffle(state: State<'_, AppState>, shuffle: bool) -> Result<(), String> {
    state.player.send(player::Command::SetShuffle(shuffle))
}

/// # Errors
///
/// Returns a message when the repeat mode is unknown or the player stopped.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn player_set_repeat(state: State<'_, AppState>, repeat: String) -> Result<(), String> {
    let repeat = match repeat.as_str() {
        "off" => Repeat::Off,
        "track" => Repeat::Track,
        "queue" => Repeat::Queue,
        other => return Err(format!("unknown repeat mode: {other}")),
    };
    state.player.send(player::Command::SetRepeat(repeat))
}

/// Queues a track of the current queue to play right after the current one.
///
/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn player_play_next(state: State<'_, AppState>, index: usize) -> Result<(), String> {
    state.player.send(player::Command::PlayNext(index))
}

/// Plays the track at `index` in the current queue now.
///
/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn player_jump_to(state: State<'_, AppState>, index: usize) -> Result<(), String> {
    state.player.send(player::Command::JumpTo(index))
}

/// Adds every track of the library to the end of the queue.
///
/// # Errors
///
/// Returns a message when the library cannot be read or the player stopped.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn enqueue_library(state: State<'_, AppState>) -> Result<(), String> {
    let tracks = state.with_db(|db| db.list_tracks(None))?;
    if tracks.is_empty() {
        return Err("the library is empty; scan a folder first".to_owned());
    }
    state.player.send(player::Command::Enqueue(tracks))
}

/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn player_set_stop_after_current(state: State<'_, AppState>, stop: bool) -> Result<(), String> {
    state
        .player
        .send(player::Command::SetStopAfterCurrent(stop))
}

#[derive(Clone, serde::Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum ScanOutcome {
    Finished(ScanReport),
    Failed { message: String },
}

fn emit<T: serde::Serialize + Clone>(app: &AppHandle, event: &str, payload: &T) {
    if let Err(err) = app.emit(event, payload) {
        log::warn!("cannot emit {event}: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::{ping_reply, ScanOutcome, ScanStatus};
    use crate::scanner::ScanReport;

    #[test]
    fn the_first_caller_runs_the_scan() {
        let mut status = ScanStatus::default();
        assert!(status.claim());
        assert!(!status.finish_pass());
        assert!(!status.running);
    }

    #[test]
    fn a_scan_asked_for_during_a_scan_runs_one_more_pass() {
        let mut status = ScanStatus::default();
        assert!(status.claim());
        // Two requests while the scan runs collapse into a single extra pass.
        assert!(!status.claim());
        assert!(!status.claim());
        assert!(status.finish_pass(), "the queued pass must run");
        assert!(status.running, "the slot stays claimed between passes");
        assert!(!status.finish_pass());
        assert!(!status.running);
    }

    #[test]
    fn aborting_releases_the_slot_and_forgets_queued_passes() {
        let mut status = ScanStatus::default();
        assert!(status.claim());
        assert!(!status.claim());
        status.abort();
        assert!(!status.running);
        assert!(status.claim(), "a later scan can claim the slot again");
        assert!(!status.finish_pass(), "the queued pass was forgotten");
    }

    #[test]
    fn ping_reply_contains_name_and_version() {
        let reply = ping_reply();
        assert!(reply.starts_with("satsuma "));
        assert!(reply.ends_with(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn scan_outcome_is_tagged_by_status() {
        let finished = serde_json::to_value(ScanOutcome::Finished(ScanReport {
            added: 1,
            ..ScanReport::default()
        }))
        .expect("serialize");
        assert_eq!(finished["status"], "finished");
        assert_eq!(finished["added"], 1);

        let failed = serde_json::to_value(ScanOutcome::Failed {
            message: "boom".to_owned(),
        })
        .expect("serialize");
        assert_eq!(failed["status"], "failed");
        assert_eq!(failed["message"], "boom");
    }
}
