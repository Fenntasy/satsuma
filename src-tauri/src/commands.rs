//! Tauri commands exposed to the frontend.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::cover;
use crate::db::{Db, Folder, LibraryRow, LibraryStats, TrackDetails};
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
#[derive(Debug)]
pub struct AppState {
    /// Not `pub`: the lock is the scanner's business, which needs to let go
    /// of it between batches. Everything else goes through [`Self::with_db`].
    pub(crate) db: Mutex<Db>,
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
    fn settings(&self) -> Result<settings::Settings, String> {
        settings::load(&self.settings_path)
    }

    /// Changes the settings file, keeping everything `change` does not
    /// touch: writing a whole `Settings` built from one part would drop the
    /// others.
    fn update_settings(&self, change: impl FnOnce(&mut settings::Settings)) -> Result<(), String> {
        let _guard = self
            .settings_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Read first: writing on top of a file that cannot be parsed would
        // throw away the playlists it holds.
        let mut settings = self.settings()?;
        change(&mut settings);
        settings::write(&self.settings_path, &settings).map_err(|err| err.to_string())
    }

    /// Runs `f` against the library cache. A poisoned lock is recovered: a
    /// panic under the lock leaves the connection usable (rusqlite rolls a
    /// transaction back when it is dropped), and refusing to touch it would
    /// disable the library for the rest of the session.
    fn with_db<T>(&self, f: impl FnOnce(&Db) -> crate::db::Result<T>) -> Result<T, String> {
        self.with_library(|db| f(db).map_err(|err| err.to_string()))
    }

    /// Runs `f` against the library cache, for a caller that answers for
    /// itself rather than returning an error to the frontend. The cover
    /// protocol is one: it has a response to send either way.
    pub(crate) fn with_library<T>(&self, f: impl FnOnce(&Db) -> T) -> T {
        let db = self.db.lock().unwrap_or_else(PoisonError::into_inner);
        f(&db)
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
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn list_folders(state: State<'_, AppState>) -> Result<Vec<Folder>, String> {
    state.with_db(Db::list_folders)
}

/// # Errors
///
/// Returns a message when the folder is already in the library or the
/// database cannot be written.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn add_folder(state: State<'_, AppState>, path: String) -> Result<Folder, String> {
    let folder = state.with_db(|db| db.add_folder(&path))?;
    state.remember_folders();
    Ok(folder)
}

/// # Errors
///
/// Returns a message when the database cannot be written.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn remove_folder(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state.with_db(|db| db.remove_folder(id))?;
    state.remember_folders();
    Ok(())
}

/// # Errors
///
/// Returns a message when the database cannot be read.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
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
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
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

/// Turns "nothing matched that id" into an error: a change that quietly
/// did nothing looks exactly like one that worked.
fn missing_unless(found: bool) -> Result<(), String> {
    if found {
        Ok(())
    } else {
        Err("that playlist is not there any more".to_owned())
    }
}

/// A playlist name with the spaces around it removed.
///
/// # Errors
///
/// Returns a message when nothing is left of it: a tab with no label
/// cannot be clicked, and only deleting it would get rid of it.
fn check_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a playlist needs a name".to_owned());
    }
    Ok(name.to_owned())
}

/// An id no playlist holds. Taken past the highest rather than from the
/// count, so deleting one and adding another cannot collide.
fn next_playlist_id(playlists: &[settings::Playlist]) -> u32 {
    playlists
        .iter()
        .map(|playlist| playlist.id)
        .max()
        .unwrap_or(0)
        + 1
}

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
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn list_playlists(state: State<'_, AppState>) -> Result<Vec<PlaylistView>, String> {
    let saved = state.settings()?.playlists;
    let rows = state.with_db(Db::library_rows)?;
    Ok(resolve_playlists(saved, &rows))
}

/// Looks the stored paths up in the library, keeping the order the playlist
/// has. A path the library does not know is dropped: the file left, and the
/// playlist is not the place to say so.
fn resolve_playlists(saved: Vec<settings::Playlist>, rows: &[LibraryRow]) -> Vec<PlaylistView> {
    let by_path: HashMap<&str, &LibraryRow> =
        rows.iter().map(|row| (row.path.as_str(), row)).collect();
    saved
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
        .collect()
}

/// Adds a playlist and answers with its id.
///
/// # Errors
///
/// Returns a message when the name is empty, or when the settings cannot
/// be written.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn create_playlist(state: State<'_, AppState>, name: String) -> Result<u32, String> {
    create(&state, &name)
}

fn create(state: &AppState, name: &str) -> Result<u32, String> {
    let name = check_name(name)?;
    let mut id = 0;
    state.update_settings(|settings| {
        id = next_playlist_id(&settings.playlists);
        settings.playlists.push(settings::Playlist {
            id,
            name: name.clone(),
            tracks: Vec::new(),
        });
    })?;
    Ok(id)
}

/// # Errors
///
/// Returns a message when the name is empty, when no playlist has that id,
/// or when the settings cannot be written.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn rename_playlist(state: State<'_, AppState>, id: u32, name: String) -> Result<(), String> {
    rename(&state, id, &name)
}

fn rename(state: &AppState, id: u32, name: &str) -> Result<(), String> {
    let name = check_name(name)?;
    let mut found = false;
    state.update_settings(|settings| {
        if let Some(playlist) = settings.playlists.iter_mut().find(|p| p.id == id) {
            playlist.name = name;
            found = true;
        }
    })?;
    missing_unless(found)
}

/// # Errors
///
/// Returns a message when no playlist has that id, or when the settings
/// cannot be written.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn delete_playlist(state: State<'_, AppState>, id: u32) -> Result<(), String> {
    delete(&state, id)
}

fn delete(state: &AppState, id: u32) -> Result<(), String> {
    let mut found = false;
    state.update_settings(|settings| {
        found = settings.playlists.iter().any(|playlist| playlist.id == id);
        settings.playlists.retain(|playlist| playlist.id != id);
    })?;
    missing_unless(found)
}

/// Adds tracks at the end of a playlist.
///
/// # Errors
///
/// Returns a message when the library cannot be read, when no playlist has
/// that id, or when the settings cannot be written.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn add_to_playlist(state: State<'_, AppState>, id: u32, ids: Vec<i64>) -> Result<(), String> {
    add_tracks(&state, id, &ids)
}

fn add_tracks(state: &AppState, id: u32, ids: &[i64]) -> Result<(), String> {
    let tracks = state.with_db(|db| db.tracks_by_ids(ids))?;
    let mut found = false;
    state.update_settings(|settings| {
        if let Some(playlist) = settings.playlists.iter_mut().find(|p| p.id == id) {
            playlist
                .tracks
                .extend(tracks.iter().map(|track| track.path.clone()));
            found = true;
        }
    })?;
    missing_unless(found)
}

/// Rates a track, writing the stars into the file first so the music keeps
/// the rating, then into the cache.
///
/// # Errors
///
/// Returns a message when the track is unknown or the file cannot be
/// written.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn set_rating(state: State<'_, AppState>, id: i64, stars: Option<u8>) -> Result<(), String> {
    // Checked here rather than trusted: the file would quietly lose its
    // rating while the cache kept the impossible number.
    if let Some(stars) = stars {
        if !(1..=5).contains(&stars) {
            return Err(format!("a rating is one to five stars, not {stars}"));
        }
    }
    let track = state
        .with_db(|db| db.tracks_by_ids(&[id]))?
        .into_iter()
        .next()
        .ok_or_else(|| "that track is not in the library any more".to_owned())?;
    crate::tags::write_rating(std::path::Path::new(&track.path), stars)
        .map_err(|err| err.to_string())?;
    state.with_db(|db| db.set_rating(id, stars))
}

/// A track as the now-playing panel shows it: its tags, and the key its
/// cover is served under.
#[derive(Clone, Debug, serde::Serialize)]
pub struct NowPlaying {
    #[serde(flatten)]
    pub track: TrackDetails,
    /// What to ask the cover protocol for. Worked out here rather than in
    /// the frontend, which would have to know how an album becomes a key
    /// and get the same answer.
    pub cover_key: String,
}

/// Everything the panel shows about the track with this id.
///
/// Answers `None` rather than an error for a track that has left the
/// library: the panel is showing what is playing, and a file that was
/// deleted mid-play is not a failure to report.
///
/// # Errors
///
/// Returns a message when the library cannot be read.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn track_details(state: State<'_, AppState>, id: i64) -> Result<Option<NowPlaying>, String> {
    Ok(state.with_db(|db| db.track_details(id))?.map(now_playing))
}

fn now_playing(track: TrackDetails) -> NowPlaying {
    let key = cover::Key::of(
        track.album_artist.as_deref(),
        track.artist.as_deref(),
        track.album.as_deref(),
        &track.path,
    );
    NowPlaying {
        cover_key: key.encode(),
        track,
    }
}

/// Everything the library tree groups by, for every track.
///
/// # Errors
///
/// Returns a message when the library cannot be read.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
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
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
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
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
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
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
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
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn player_state(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(player::Command::ReportState)
}

/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn player_play_pause(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(player::Command::PlayPause)
}

/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn player_stop(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(player::Command::Stop)
}

/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn player_next(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(player::Command::Next)
}

/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn player_previous(state: State<'_, AppState>) -> Result<(), String> {
    state.player.send(player::Command::Previous)
}

/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn player_seek(state: State<'_, AppState>, position_ms: u64) -> Result<(), String> {
    state
        .player
        .send(player::Command::Seek(Duration::from_millis(position_ms)))
}

/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn player_set_volume(state: State<'_, AppState>, volume: f32) -> Result<(), String> {
    state.player.send(player::Command::SetVolume(volume))
}

/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn player_set_shuffle(state: State<'_, AppState>, shuffle: bool) -> Result<(), String> {
    state.player.send(player::Command::SetShuffle(shuffle))
}

/// # Errors
///
/// Returns a message when the repeat mode is unknown or the player stopped.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
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
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn player_play_next(state: State<'_, AppState>, index: usize) -> Result<(), String> {
    state.player.send(player::Command::PlayNext(index))
}

/// Plays the track at `index` in the current queue now.
///
/// # Errors
///
/// Returns a message when the player stopped.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
pub fn player_jump_to(state: State<'_, AppState>, index: usize) -> Result<(), String> {
    state.player.send(player::Command::JumpTo(index))
}

/// Adds every track of the library to the end of the queue.
///
/// # Errors
///
/// Returns a message when the library cannot be read or the player stopped.
#[tauri::command(async)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
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
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands a command its State by value"
)]
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
    use std::sync::mpsc;

    use super::{next_playlist_id, ping_reply, AppState, ScanOutcome, ScanStatus};
    use crate::db::{Db, FileStamp, TrackDetails, TrackRecord};
    use crate::player::Handle;
    use crate::scanner::ScanReport;
    use crate::settings;
    use crate::tags::TrackTags;

    fn playlist(id: u32) -> settings::Playlist {
        settings::Playlist {
            id,
            name: format!("Playlist {id}"),
            tracks: Vec::new(),
        }
    }

    /// A state with its own settings file and a player that goes nowhere.
    fn state(dir: &std::path::Path) -> AppState {
        let (sender, _receiver) = mpsc::channel();
        AppState::new(
            Db::open_in_memory().expect("db"),
            dir.join("settings.json"),
            Handle::new(sender),
        )
    }

    fn library_row(id: i64, path: &str) -> crate::db::LibraryRow {
        crate::db::LibraryRow {
            id,
            path: path.to_owned(),
            genre: None,
            artist: None,
            album: None,
            title: None,
            track_number: None,
            disc_number: None,
            rating: None,
            grouping: None,
            duration_ms: 1000,
        }
    }

    #[test]
    fn a_playlist_keeps_its_order_and_loses_only_what_left_the_library() {
        let rows = [
            library_row(1, "/music/a.mp3"),
            library_row(2, "/music/b.mp3"),
        ];
        let saved = vec![settings::Playlist {
            id: 1,
            name: "Mine".to_owned(),
            tracks: vec![
                "/music/b.mp3".to_owned(),
                "/music/gone.mp3".to_owned(),
                "/music/a.mp3".to_owned(),
                "/music/b.mp3".to_owned(),
            ],
        }];

        let resolved = super::resolve_playlists(saved, &rows);
        let ids: Vec<i64> = resolved[0].tracks.iter().map(|track| track.id).collect();
        assert_eq!(
            ids,
            [2, 1, 2],
            "the order stands, a track listed twice stays twice, and the \
             one that is gone simply drops out"
        );
    }

    #[test]
    fn a_playlist_name_cannot_be_empty() {
        assert_eq!(super::check_name("  Loud  "), Ok("Loud".to_owned()));
        assert_eq!(
            super::check_name("   ").unwrap_err(),
            "a playlist needs a name"
        );
    }

    #[test]
    fn a_change_to_a_playlist_that_is_gone_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = state(dir.path());
        let gone = super::create(&state, "Mine").expect("create");
        // A second playlist that stays: without one, "the settings are
        // unchanged" would hold however the refused calls behaved, since
        // none of them can add a playlist.
        let kept = super::create(&state, "Kept").expect("create");

        super::delete(&state, gone).expect("delete");
        // Everything that changes a playlist has to notice it is gone,
        // rather than reporting a change it did not make. The message is
        // checked too: `is_err` passes for the wrong error.
        for (what, outcome) in [
            ("delete", super::delete(&state, gone)),
            ("rename", super::rename(&state, gone, "Other")),
            ("add tracks", super::add_tracks(&state, gone, &[1])),
        ] {
            assert_eq!(
                outcome.unwrap_err(),
                "that playlist is not there any more",
                "{what} on a playlist that is gone"
            );
        }

        let left = state.settings().expect("load").playlists;
        assert_eq!(left.len(), 1, "a refused change must add nothing");
        assert_eq!(left[0].id, kept, "and must not touch the one still there");
        assert_eq!(left[0].name, "Kept", "least of all rename it");
        assert!(left[0].tracks.is_empty(), "nor give it tracks");
    }

    #[test]
    fn a_playlist_is_created_renamed_and_deleted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = state(dir.path());

        let first = super::create(&state, "  Loud  ").expect("create");
        assert_eq!(
            state.settings().expect("load").playlists[0].name,
            "Loud",
            "the name is stored without the spaces around it"
        );
        assert_eq!(
            super::create(&state, "  ").unwrap_err(),
            "a playlist needs a name"
        );

        let second = super::create(&state, "Quiet").expect("create");
        assert_ne!(first, second);

        super::rename(&state, first, "Louder").expect("rename");
        let playlists = state.settings().expect("load").playlists;
        assert_eq!(playlists[0].name, "Louder");
        assert_eq!(playlists[1].name, "Quiet", "only the one asked for");

        super::delete(&state, first).expect("delete");
        let left = state.settings().expect("load").playlists;
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, second);
    }

    #[test]
    fn a_new_playlist_id_never_collides_with_a_live_one() {
        assert_eq!(next_playlist_id(&[]), 1);
        assert_eq!(next_playlist_id(&[playlist(1), playlist(2)]), 3);
        // The highest was deleted: counting would hand out 2 again.
        assert_eq!(next_playlist_id(&[playlist(1), playlist(7)]), 8);
    }

    #[test]
    fn changing_one_part_of_the_settings_keeps_the_others() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = state(dir.path());

        state
            .update_settings(|settings| {
                settings.folders = vec!["/music".to_owned()];
                settings.playlists = vec![playlist(1)];
            })
            .expect("write");

        state
            .update_settings(|settings| settings.playlists.push(playlist(2)))
            .expect("write");
        assert_eq!(
            state.settings().expect("load").folders,
            ["/music"],
            "changing the playlists must not drop the folders"
        );

        state
            .update_settings(|settings| settings.folders.push("/more".to_owned()))
            .expect("write");
        assert_eq!(
            state.settings().expect("load").playlists.len(),
            2,
            "changing the folders must not drop the playlists"
        );
    }

    #[test]
    fn a_settings_file_that_cannot_be_read_is_never_written_over() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = state(dir.path());
        state
            .update_settings(|settings| settings.playlists.push(playlist(1)))
            .expect("write");

        // Something made the file unreadable. Writing on top of it would
        // take the playlist with it.
        std::fs::write(dir.path().join("settings.json"), b"{ not json").expect("write");
        assert!(state.update_settings(|_| {}).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("settings.json")).expect("read"),
            "{ not json",
            "the file must be left as it is, for the user to recover"
        );
    }

    #[test]
    fn a_settings_file_that_cannot_be_written_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = state(dir.path());
        state
            .update_settings(|settings| settings.playlists.push(playlist(1)))
            .expect("write");

        // The settings are written by replacing a temporary file, so a
        // directory in its place fails the write while the read still
        // works: without that, this would fail before reaching the write.
        std::fs::create_dir(dir.path().join("settings.json.new")).expect("mkdir");
        assert!(
            state.settings().is_ok(),
            "the read must still work, or the write is never reached"
        );
        assert!(state.update_settings(|_| {}).is_err());
    }

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

    #[test]
    fn the_panel_is_told_everything_it_shows() {
        let db = Db::open_in_memory().expect("db");
        let folder = db.add_folder("/music").expect("folder");
        db.upsert_tracks(&[TrackRecord {
            folder_id: folder.id,
            stamp: FileStamp {
                path: "/music/1.mp3".to_owned(),
                mtime: 1,
                size: 2,
            },
            tags: TrackTags {
                title: Some("Orange Sun".to_owned()),
                artist: Some("The Satsumas".to_owned()),
                album_artist: Some("Various".to_owned()),
                album: Some("Citrus".to_owned()),
                genre: Some("Indie".to_owned()),
                year: Some(1998),
                track_number: Some(3),
                disc_number: Some(1),
                rating: Some(4),
                grouping_raw: Some("Chant / Loud / Happy".to_owned()),
                duration_ms: 185_000,
                ..TrackTags::default()
            },
        }])
        .expect("upsert");
        let id = db.library_rows().expect("rows")[0].id;

        let details = db.track_details(id).expect("query").expect("a track");
        // The year and the album artist are the reason this is not a
        // library row: nothing else asks for them.
        assert_eq!(details.year, Some(1998));
        assert_eq!(details.album_artist.as_deref(), Some("Various"));
        assert_eq!(details.title.as_deref(), Some("Orange Sun"));
        assert_eq!(details.artist.as_deref(), Some("The Satsumas"));
        assert_eq!(details.album.as_deref(), Some("Citrus"));
        assert_eq!(details.genre.as_deref(), Some("Indie"));
        assert_eq!(details.track_number, Some(3));
        assert_eq!(details.disc_number, Some(1));
        assert_eq!(details.rating, Some(4));
        assert_eq!(details.grouping.as_deref(), Some("Chant / Loud / Happy"));
        assert_eq!(details.duration_ms, 185_000);
    }

    #[test]
    fn a_track_that_has_left_the_library_is_nothing_rather_than_an_error() {
        let db = Db::open_in_memory().expect("db");
        assert_eq!(db.track_details(404).expect("query"), None);
    }

    #[test]
    fn the_cover_key_names_the_album_the_track_belongs_to() {
        let details = |album_artist: Option<&str>, artist, album, path: &str| {
            super::now_playing(TrackDetails {
                id: 1,
                path: path.to_owned(),
                title: None,
                artist: Option::<&str>::map(artist, str::to_owned),
                album_artist: album_artist.map(str::to_owned),
                album: Option::<&str>::map(album, str::to_owned),
                genre: None,
                year: None,
                track_number: None,
                disc_number: None,
                rating: None,
                grouping: None,
                duration_ms: 0,
            })
        };

        let first = details(None, Some("Alpha"), Some("Citrus"), "/music/1.mp3");
        let second = details(None, Some("Alpha"), Some("Citrus"), "/music/2.mp3");
        assert_eq!(
            first.cover_key, second.cover_key,
            "two tracks of one record must ask for the same cover"
        );

        let elsewhere = details(None, Some("Alpha"), Some("Lemon"), "/music/3.mp3");
        assert_ne!(
            first.cover_key, elsewhere.cover_key,
            "another record is another cover"
        );

        // The key is what the protocol will be asked for, so it has to
        // read back as the album it stands for.
        assert_eq!(
            crate::cover::Key::decode(&first.cover_key),
            Some(crate::cover::Key::Album {
                artist: "Alpha".to_owned(),
                album: "Citrus".to_owned()
            })
        );

        // A record whose tracks name different performers is held together
        // by its album artist, which is why that is preferred.
        let one = details(
            Some("Various"),
            Some("Beta"),
            Some("Citrus"),
            "/music/4.mp3",
        );
        let other = details(
            Some("Various"),
            Some("Delta"),
            Some("Citrus"),
            "/music/5.mp3",
        );
        assert_eq!(one.cover_key, other.cover_key);
        assert_ne!(
            first.cover_key, one.cover_key,
            "Alpha's Citrus and Various' Citrus are two records that share a name"
        );
    }

    #[test]
    fn a_record_tagged_inconsistently_is_two_records() {
        // Worth knowing rather than discovering: a record where some
        // tracks carry an album artist and others do not is two covers,
        // because each track is keyed on what it actually says. The tree
        // groups the same way, by the tags a track carries and nothing
        // else, so the fix for both is to fix the tags.
        let track = |album_artist: Option<&str>| {
            super::now_playing(TrackDetails {
                id: 1,
                path: "/music/1.mp3".to_owned(),
                title: None,
                artist: Some("Alpha".to_owned()),
                album_artist: album_artist.map(str::to_owned),
                album: Some("Citrus".to_owned()),
                genre: None,
                year: None,
                track_number: None,
                disc_number: None,
                rating: None,
                grouping: None,
                duration_ms: 0,
            })
            .cover_key
        };
        assert_ne!(track(None), track(Some("The Satsumas")));
        assert_eq!(
            track(None),
            track(Some("   ")),
            "an album artist of only spaces is none at all, not a third record"
        );
    }
}
