//! Tauri commands exposed to the frontend.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::db::{Db, Folder, LibraryStats};
use crate::scanner::{self, ScanProgress, ScanReport};

pub const SCAN_PROGRESS_EVENT: &str = "library://scan-progress";
pub const SCAN_FINISHED_EVENT: &str = "library://scan-finished";
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
}

impl AppState {
    #[must_use]
    pub fn new(db: Db) -> Self {
        AppState {
            db: Mutex::new(db),
            scan: Arc::new(Mutex::new(ScanStatus::default())),
        }
    }

    fn with_db<T>(&self, f: impl FnOnce(&Db) -> crate::db::Result<T>) -> Result<T, String> {
        let db = self.db.lock().map_err(|_| "database lock poisoned")?;
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
    state.with_db(|db| db.add_folder(&path))
}

/// # Errors
///
/// Returns a message when the database cannot be written.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn remove_folder(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state.with_db(|db| db.remove_folder(id))
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
    tauri::async_runtime::spawn_blocking(move || {
        let guard = ScanGuard::new(app.clone(), scan);
        run_scan(&app, guard);
    });
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
        scan_once(app, &state);
        // Decide whether to run again while still holding the lock, so a
        // request that arrives now is either seen here or starts its own
        // scan afterwards, never dropped in between.
        if !lock_status(&guard.scan).finish_pass() {
            guard.handed_over = true;
            return;
        }
    }
}

fn scan_once(app: &AppHandle, state: &State<'_, AppState>) {
    let mut last_emit: Option<Instant> = None;
    let result = scanner::scan(&state.db, |progress: ScanProgress| {
        let done = progress.scanned == progress.total;
        if done || last_emit.is_none_or(|at| at.elapsed() >= PROGRESS_INTERVAL) {
            last_emit = Some(Instant::now());
            emit(app, SCAN_PROGRESS_EVENT, &progress);
        }
    })
    .map_err(|err| err.to_string());
    match result {
        Ok(report) => emit(app, SCAN_FINISHED_EVENT, &ScanOutcome::Finished(report)),
        Err(message) => emit(app, SCAN_FINISHED_EVENT, &ScanOutcome::Failed { message }),
    }
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
