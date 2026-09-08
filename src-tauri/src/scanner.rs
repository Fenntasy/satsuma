//! Incremental folder scanning: walks the library folders, reads tags for
//! new or changed files, and prunes files that disappeared.
//!
//! The scanner only holds the database lock while it writes a batch, so the
//! rest of the application stays responsive while a large library is being
//! scanned. Walking the filesystem and reading tags happen without the lock.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::db::{Db, DbError, FileStamp, Folder, TrackRecord};
use crate::tags::{is_audio_file, read_tags};

/// How many tracks are written per transaction.
const BATCH_SIZE: usize = 200;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanReport {
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
    /// Audio files whose tags could not be read.
    pub failed: u64,
    /// Folders that could not be read. Their tracks are left alone instead
    /// of being pruned.
    pub unreachable: u64,
    /// Folders that hold no audio file although the cache has tracks for
    /// them, which usually means a drive or share is not mounted. Their
    /// tracks are kept.
    pub emptied: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanProgress {
    pub scanned: u64,
    pub total: u64,
    pub path: String,
}

/// Scans every library folder, calling `on_progress` after each file is
/// examined.
///
/// The folder list is read from the database when the scan starts. Folders
/// whose root is not reachable are skipped entirely, so an unplugged drive
/// never empties the cache.
///
/// # Errors
///
/// Returns an error when the database cannot be read or written. Unreadable
/// audio files are counted in [`ScanReport::failed`] instead.
pub fn scan(
    db: &Mutex<Db>,
    mut on_progress: impl FnMut(ScanProgress),
) -> Result<ScanReport, DbError> {
    let folders = with_db(db, Db::list_folders)?;
    let mut report = ScanReport::default();
    let found = collect_files(&folders, &mut report);
    let total = found.files.len() as u64;
    let mut known = load_known(db, &found, &mut report)?;
    let mut pending: Vec<TrackRecord> = Vec::new();
    for (index, (folder_id, stamp)) in found.files.into_iter().enumerate() {
        let path = stamp.path.clone();
        match known.remove(&stamp.path) {
            Some((_, previous)) if previous == stamp => {}
            previous => match read_tags(Path::new(&stamp.path)) {
                Ok(tags) => {
                    pending.push(TrackRecord {
                        folder_id,
                        stamp,
                        tags,
                    });
                    if previous.is_some() {
                        report.updated += 1;
                    } else {
                        report.added += 1;
                    }
                }
                Err(err) => {
                    log::warn!("skipping unreadable file: {err}");
                    report.failed += 1;
                }
            },
        }
        if pending.len() >= BATCH_SIZE {
            with_db(db, |db| db.upsert_tracks(&pending))?;
            pending.clear();
        }
        on_progress(ScanProgress {
            scanned: index as u64 + 1,
            total,
            path,
        });
    }
    if !pending.is_empty() {
        with_db(db, |db| db.upsert_tracks(&pending))?;
    }

    let gone: Vec<String> = known
        .into_iter()
        .filter(|(path, (folder_id, _))| {
            found.prunable.contains(folder_id) && !found.protected.contains(path)
        })
        .map(|(path, _)| path)
        .collect();
    if !gone.is_empty() {
        report.removed = gone.len() as u64;
        with_db(db, |db| db.remove_tracks(&gone))?;
    }
    Ok(report)
}

/// What walking the library folders found.
struct Found<'a> {
    files: Vec<(i64, FileStamp)>,
    /// Folders that were read completely and are therefore safe to prune.
    prunable: HashSet<i64>,
    /// Folders that exist but hold no audio file. Whether they are safe to
    /// prune depends on what the cache holds for them.
    empty: HashSet<i64>,
    /// Paths that exist but could not be read, which must survive pruning.
    protected: HashSet<String>,
    scanned: Vec<&'a Folder>,
}

fn collect_files<'a>(folders: &'a [Folder], report: &mut ScanReport) -> Found<'a> {
    let mut found = Found {
        files: Vec::new(),
        prunable: HashSet::new(),
        empty: HashSet::new(),
        protected: HashSet::new(),
        scanned: Vec::new(),
    };
    let mut seen: HashSet<String> = HashSet::new();
    for folder in folders {
        let root = Path::new(&folder.path);
        if !root.is_dir() {
            log::warn!("skipping unreachable folder: {}", folder.path);
            report.unreachable += 1;
            continue;
        }
        found.scanned.push(folder);
        let walk = audio_files(root);
        if walk.unreadable_dirs > 0 {
            log::warn!(
                "{} directories under {} could not be read; keeping their cached tracks",
                walk.unreadable_dirs,
                folder.path
            );
            report.unreachable += 1;
        } else if walk.files.is_empty() {
            // A mountpoint whose share is not mounted is an existing, empty
            // directory, and emptying the cache for it would look exactly
            // like a genuine deletion.
            found.empty.insert(folder.id);
        } else {
            found.prunable.insert(folder.id);
        }
        found.protected.extend(walk.unreadable_files);
        for file in walk.files {
            // A file can be reached through two folders when one is nested
            // inside the other, or through a symlink. Keep the first.
            if seen.insert(file.key) {
                found.files.push((folder.id, file.stamp));
            }
        }
    }
    found
}

/// Loads what the cache holds for the folders that were walked, and decides
/// whether an empty folder is empty on purpose or waiting for its drive.
fn load_known(
    db: &Mutex<Db>,
    found: &Found<'_>,
    report: &mut ScanReport,
) -> Result<HashMap<String, (i64, FileStamp)>, DbError> {
    let mut known = HashMap::new();
    with_db(db, |db| {
        for folder in &found.scanned {
            let stamps = db.file_stamps(folder.id)?;
            // An empty folder with an empty cache has nothing to prune, so
            // only the case where tracks are cached needs reporting.
            if found.empty.contains(&folder.id) && !stamps.is_empty() {
                log::warn!(
                    "{} holds no audio file but {} are cached; keeping them",
                    folder.path,
                    stamps.len()
                );
                report.emptied += 1;
            }
            for stamp in stamps {
                known.insert(stamp.path.clone(), (folder.id, stamp));
            }
        }
        Ok(())
    })?;
    Ok(known)
}

/// Runs `f` against the library cache, recovering a poisoned lock the same
/// way [`crate::commands::AppState`] does.
fn with_db<T>(db: &Mutex<Db>, f: impl FnOnce(&Db) -> Result<T, DbError>) -> Result<T, DbError> {
    let db = db.lock().unwrap_or_else(PoisonError::into_inner);
    f(&db)
}

/// A file found by the walk, with the key used to recognise it when it is
/// reachable through more than one library folder.
struct FoundFile {
    stamp: FileStamp,
    /// The canonical path, so the same file behind a symlink is not counted
    /// twice.
    key: String,
}

/// The audio files found under a folder.
struct FolderWalk {
    files: Vec<FoundFile>,
    /// Directories that could not be listed, which makes the whole folder
    /// unsafe to prune.
    unreadable_dirs: u64,
    /// Files that exist but could not be read right now. Their cached rows
    /// are kept: unlike a deleted file, this may be a passing failure.
    unreadable_files: Vec<String>,
}

/// What looking at one file yielded.
enum Stamped {
    Found(Box<FoundFile>),
    /// The file is gone, so its cached row may be pruned.
    Gone,
    /// The file is there but could not be read.
    Unreadable,
}

fn audio_files(root: &Path) -> FolderWalk {
    let mut walk = FolderWalk {
        files: Vec::new(),
        unreadable_dirs: 0,
        unreadable_files: Vec::new(),
    };
    // Sorted so that, when the same file is reachable through two paths, the
    // same one wins on every scan.
    for entry in WalkDir::new(root).follow_links(true).sort_by_file_name() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                if is_harmless_walk_error(&err) {
                    // A dangling symlink or a symlink loop says nothing
                    // about whether the folder was read completely.
                    log::debug!("skipping entry: {err}");
                } else {
                    log::warn!("cannot read directory entry: {err}");
                    walk.unreadable_dirs += 1;
                }
                continue;
            }
        };
        if !entry.file_type().is_file() || !is_audio_file(entry.path()) {
            continue;
        }
        match stamp_of(&entry) {
            Stamped::Found(file) => walk.files.push(*file),
            Stamped::Gone => log::debug!("{} went away during the scan", entry.path().display()),
            Stamped::Unreadable => {
                log::warn!("cannot stat {}", entry.path().display());
                walk.unreadable_files
                    .push(entry.path().to_string_lossy().into_owned());
            }
        }
    }
    walk
}

/// Whether a walk error means an entry is unusable rather than a directory
/// being unreadable: a dangling symlink, a symlink loop, or an entry that
/// disappeared while the walk was running.
fn is_harmless_walk_error(err: &walkdir::Error) -> bool {
    err.loop_ancestor().is_some()
        || err
            .io_error()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
}

fn stamp_of(entry: &walkdir::DirEntry) -> Stamped {
    let metadata = match entry.metadata() {
        Ok(metadata) => metadata,
        Err(err) if err.io_error().is_some_and(is_not_found) => return Stamped::Gone,
        Err(_) => return Stamped::Unreadable,
    };
    let Ok(modified) = metadata.modified() else {
        return Stamped::Unreadable;
    };
    let path = entry.path().to_string_lossy().into_owned();
    let key = std::fs::canonicalize(entry.path())
        .map_or_else(|_| path.clone(), |real| real.to_string_lossy().into_owned());
    Stamped::Found(Box::new(FoundFile {
        stamp: FileStamp {
            path,
            mtime: seconds_since_epoch(modified),
            size: i64::try_from(metadata.len()).unwrap_or(i64::MAX),
        },
        key,
    }))
}

fn is_not_found(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::NotFound
}

/// Seconds since the Unix epoch, negative for the files dated before it that
/// bad archive extraction and legacy imports leave behind.
fn seconds_since_epoch(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
        Err(before) => {
            i64::try_from(before.duration().as_secs()).map_or(i64::MIN, |seconds| -seconds)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex;

    use lofty::prelude::Accessor;

    use super::{scan, seconds_since_epoch, ScanProgress, ScanReport, BATCH_SIZE};
    use crate::db::Db;
    use crate::tags::tests::{sample_tag, tagged_copy, FIXTURE};

    fn setup() -> (tempfile::TempDir, Mutex<Db>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open_in_memory().expect("db");
        db.add_folder(&dir.path().to_string_lossy())
            .expect("add folder");
        (dir, Mutex::new(db))
    }

    fn scan_all(db: &Mutex<Db>) -> (ScanReport, Vec<ScanProgress>) {
        let mut progress = Vec::new();
        let report = scan(db, |p| progress.push(p)).expect("scan");
        (report, progress)
    }

    fn track_count(db: &Mutex<Db>) -> u64 {
        db.lock().expect("lock").stats().expect("stats").track_count
    }

    #[test]
    fn adds_updates_and_removes_files_across_scans() {
        let (dir, db) = setup();
        fs::create_dir(dir.path().join("sub")).expect("mkdir");
        tagged_copy(dir.path(), "one.mp3", &sample_tag());
        let two = tagged_copy(&dir.path().join("sub"), "two.mp3", &sample_tag());
        fs::write(dir.path().join("cover.jpg"), b"not audio").expect("write");

        let (report, progress) = scan_all(&db);
        assert_eq!(
            report,
            ScanReport {
                added: 2,
                ..ScanReport::default()
            }
        );
        assert_eq!(progress.len(), 2);
        assert_eq!(progress[1].scanned, 2);
        assert_eq!(progress[1].total, 2);
        assert_eq!(track_count(&db), 2);

        // Nothing changed: nothing is re-read.
        let (report, _) = scan_all(&db);
        assert_eq!(report, ScanReport::default());

        // Change one file (size changes because the title is longer).
        let mut tag = sample_tag();
        tag.set_title("A much longer title than before".into());
        tagged_copy(dir.path(), "one.mp3", &tag);
        fs::remove_file(&two).expect("remove");
        let (report, _) = scan_all(&db);
        assert_eq!(
            report,
            ScanReport {
                updated: 1,
                removed: 1,
                ..ScanReport::default()
            }
        );
        assert_eq!(track_count(&db), 1);
    }

    #[test]
    fn unreadable_audio_files_are_counted_not_fatal() {
        let (dir, db) = setup();
        fs::write(dir.path().join("broken.mp3"), b"garbage").expect("write");
        fs::copy(FIXTURE, dir.path().join("fine.mp3")).expect("copy");
        let (report, _) = scan_all(&db);
        assert_eq!(report.added, 1);
        assert_eq!(report.failed, 1);
    }

    #[test]
    fn an_unreachable_folder_keeps_its_tracks() {
        let (dir, db) = setup();
        fs::copy(FIXTURE, dir.path().join("song.mp3")).expect("copy");
        let (report, _) = scan_all(&db);
        assert_eq!(report.added, 1);

        // The drive goes away: the folder root can no longer be read.
        fs::remove_dir_all(dir.path()).expect("remove root");
        let (report, progress) = scan_all(&db);
        assert_eq!(
            report,
            ScanReport {
                unreachable: 1,
                ..ScanReport::default()
            }
        );
        assert!(progress.is_empty());
        assert_eq!(track_count(&db), 1, "the cached track must survive");
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_subdirectory_keeps_its_tracks() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, db) = setup();
        let sub = dir.path().join("locked");
        fs::create_dir(&sub).expect("mkdir");
        fs::copy(FIXTURE, sub.join("song.mp3")).expect("copy");
        fs::copy(FIXTURE, dir.path().join("reachable.mp3")).expect("copy");
        let (report, _) = scan_all(&db);
        assert_eq!(report.added, 2);

        // The subdirectory becomes unreadable (permissions, I/O error).
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o000)).expect("chmod");
        // Permissions do not apply to root, where this cannot be simulated.
        if fs::read_dir(&sub).is_ok() {
            fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).expect("restore");
            return;
        }
        let (report, _) = scan_all(&db);
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).expect("restore");

        assert_eq!(
            report.removed, 0,
            "an unreadable subtree must not be pruned"
        );
        assert_eq!(report.unreachable, 1, "the folder must be flagged");
        assert_eq!(report.failed, 0, "no audio file was unreadable");
        assert_eq!(track_count(&db), 2, "cached tracks must survive");
    }

    #[test]
    fn a_file_reachable_through_two_folders_is_scanned_once() {
        let (dir, db) = setup();
        let nested = dir.path().join("nested");
        fs::create_dir(&nested).expect("mkdir");
        fs::copy(FIXTURE, nested.join("song.mp3")).expect("copy");
        db.lock()
            .expect("lock")
            .add_folder(&nested.to_string_lossy())
            .expect("add nested folder");

        let (report, progress) = scan_all(&db);
        assert_eq!(report.added, 1);
        assert_eq!(progress.len(), 1);
        assert_eq!(track_count(&db), 1);

        // A second scan must see the file as unchanged, not as new again.
        let (report, _) = scan_all(&db);
        assert_eq!(report, ScanReport::default());
    }

    #[cfg(unix)]
    #[test]
    fn a_file_reached_through_a_symlinked_folder_is_scanned_once() {
        let (dir, db) = setup();
        let real = dir.path().join("real");
        fs::create_dir(&real).expect("mkdir");
        fs::copy(FIXTURE, real.join("song.mp3")).expect("copy");
        std::os::unix::fs::symlink(&real, dir.path().join("link")).expect("symlink");

        let (report, _) = scan_all(&db);
        assert_eq!(report.added, 1, "the same file must not be added twice");
        assert_eq!(track_count(&db), 1);

        let (report, _) = scan_all(&db);
        assert_eq!(report, ScanReport::default());
    }

    #[test]
    fn an_empty_folder_that_used_to_hold_tracks_keeps_them() {
        let (dir, db) = setup();
        fs::copy(FIXTURE, dir.path().join("song.mp3")).expect("copy");
        let (report, _) = scan_all(&db);
        assert_eq!(report.added, 1);

        // The mountpoint is still there, but the share is not mounted.
        fs::remove_file(dir.path().join("song.mp3")).expect("remove");
        let (report, _) = scan_all(&db);
        assert_eq!(
            report,
            ScanReport {
                emptied: 1,
                ..ScanReport::default()
            }
        );
        assert_eq!(track_count(&db), 1, "cached tracks must survive");
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_symlink_does_not_disable_pruning() {
        let (dir, db) = setup();
        fs::copy(FIXTURE, dir.path().join("kept.mp3")).expect("copy");
        fs::copy(FIXTURE, dir.path().join("deleted.mp3")).expect("copy");
        let (report, _) = scan_all(&db);
        assert_eq!(report.added, 2);

        // A link to a file that no longer exists, next to a deleted track.
        std::os::unix::fs::symlink(dir.path().join("nowhere.mp3"), dir.path().join("link.mp3"))
            .expect("symlink");
        fs::remove_file(dir.path().join("deleted.mp3")).expect("remove");
        let (report, _) = scan_all(&db);
        assert_eq!(
            report,
            ScanReport {
                removed: 1,
                ..ScanReport::default()
            },
            "a dangling symlink must not stop the scan from pruning"
        );
        assert_eq!(track_count(&db), 1);
    }

    #[test]
    fn a_date_before_the_epoch_becomes_a_negative_timestamp() {
        use std::time::{Duration, UNIX_EPOCH};

        assert_eq!(seconds_since_epoch(UNIX_EPOCH), 0);
        assert_eq!(
            seconds_since_epoch(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            1_700_000_000
        );
        assert_eq!(
            seconds_since_epoch(UNIX_EPOCH - Duration::from_hours(24)),
            -86_400,
            "a file older than 1970 must still be scannable"
        );
    }

    #[test]
    fn an_empty_folder_with_an_empty_cache_is_not_reported() {
        let (_dir, db) = setup();
        let (report, _) = scan_all(&db);
        assert_eq!(report, ScanReport::default());
    }

    #[test]
    fn a_cached_track_that_becomes_unreadable_is_kept() {
        let (dir, db) = setup();
        let path = dir.path().join("song.mp3");
        fs::copy(FIXTURE, &path).expect("copy");
        fs::copy(FIXTURE, dir.path().join("other.mp3")).expect("copy");
        let (report, _) = scan_all(&db);
        assert_eq!(report.added, 2);

        // The file is still there but its content is now garbage.
        fs::write(&path, b"corrupted beyond repair").expect("corrupt");
        let (report, _) = scan_all(&db);
        assert_eq!(report.failed, 1);
        assert_eq!(report.removed, 0, "an unreadable track must not be pruned");
        assert_eq!(track_count(&db), 2, "its cached row must survive");
    }

    #[test]
    fn writes_more_files_than_fit_in_one_batch() {
        let (dir, db) = setup();
        for index in 0..=BATCH_SIZE {
            fs::copy(FIXTURE, dir.path().join(format!("song{index}.mp3"))).expect("copy");
        }
        let (report, _) = scan_all(&db);
        let expected = u64::try_from(BATCH_SIZE + 1).expect("fits");
        assert_eq!(report.added, expected);
        assert_eq!(track_count(&db), expected);
    }

    #[test]
    fn missing_folder_yields_no_files() {
        let db = Db::open_in_memory().expect("db");
        db.add_folder("/definitely/missing").expect("add");
        let (report, progress) = scan_all(&Mutex::new(db));
        assert_eq!(
            report,
            ScanReport {
                unreachable: 1,
                ..ScanReport::default()
            }
        );
        assert!(progress.is_empty());
    }
}
