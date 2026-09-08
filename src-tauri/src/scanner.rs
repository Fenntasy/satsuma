//! Incremental folder scanning: walks the library folders, reads tags for
//! new or changed files, and prunes files that disappeared.
//!
//! The scanner only holds the database lock while it writes a batch, so the
//! rest of the application stays responsive while a large library is being
//! scanned. Walking the filesystem and reading tags happen without the lock.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

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
    pub failed: u64,
    /// Folders whose root could not be read, and which were therefore left
    /// untouched instead of having their tracks pruned.
    pub unreachable: u64,
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

    let mut files: Vec<(i64, FileStamp)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut scanned_folders: Vec<&Folder> = Vec::new();
    // Only folders we could read completely may have their tracks pruned:
    // a permission error or an I/O hiccup must not empty the cache.
    let mut prunable: HashSet<i64> = HashSet::new();
    for folder in &folders {
        let root = Path::new(&folder.path);
        if !root.is_dir() {
            log::warn!("skipping unreachable folder: {}", folder.path);
            report.unreachable += 1;
            continue;
        }
        scanned_folders.push(folder);
        let walk = audio_files(root);
        if walk.errors == 0 {
            prunable.insert(folder.id);
        } else {
            log::warn!(
                "{} paths under {} could not be read; keeping their cached tracks",
                walk.errors,
                folder.path
            );
            report.failed += walk.errors;
        }
        for file in walk.files {
            // A file can be reached through two folders when one is nested
            // inside the other, or through a symlink. Keep the first.
            if seen.insert(file.path.clone()) {
                files.push((folder.id, file));
            }
        }
    }
    let total = files.len() as u64;

    let mut known: HashMap<String, (i64, FileStamp)> = HashMap::new();
    with_db(db, |db| {
        for folder in &scanned_folders {
            for stamp in db.file_stamps(folder.id)? {
                known.insert(stamp.path.clone(), (folder.id, stamp));
            }
        }
        Ok(())
    })?;

    let mut pending: Vec<TrackRecord> = Vec::new();
    for (index, (folder_id, stamp)) in files.into_iter().enumerate() {
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
        .filter(|(_, (folder_id, _))| prunable.contains(folder_id))
        .map(|(path, _)| path)
        .collect();
    if !gone.is_empty() {
        report.removed = gone.len() as u64;
        with_db(db, |db| db.remove_tracks(&gone))?;
    }
    Ok(report)
}

fn with_db<T>(db: &Mutex<Db>, f: impl FnOnce(&Db) -> Result<T, DbError>) -> Result<T, DbError> {
    let db = db.lock().map_err(|_| DbError::Poisoned)?;
    f(&db)
}

/// The audio files found under a folder, and how many paths could not be
/// read while walking it.
struct FolderWalk {
    files: Vec<FileStamp>,
    errors: u64,
}

fn audio_files(root: &Path) -> FolderWalk {
    let mut walk = FolderWalk {
        files: Vec::new(),
        errors: 0,
    };
    for entry in WalkDir::new(root).follow_links(true) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                log::warn!("cannot read directory entry: {err}");
                walk.errors += 1;
                continue;
            }
        };
        if !entry.file_type().is_file() || !is_audio_file(entry.path()) {
            continue;
        }
        if let Some(stamp) = stamp_of(&entry) {
            walk.files.push(stamp);
        } else {
            log::warn!("cannot stat {}", entry.path().display());
            walk.errors += 1;
        }
    }
    walk
}

fn stamp_of(entry: &walkdir::DirEntry) -> Option<FileStamp> {
    let metadata = entry.metadata().ok()?;
    let mtime = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(FileStamp {
        path: entry.path().to_string_lossy().into_owned(),
        mtime: i64::try_from(mtime.as_secs()).unwrap_or(i64::MAX),
        size: i64::try_from(metadata.len()).unwrap_or(i64::MAX),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex;

    use lofty::prelude::Accessor;

    use super::{scan, ScanProgress, ScanReport, BATCH_SIZE};
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
        let (report, _) = scan_all(&db);
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).expect("restore");

        assert_eq!(
            report.removed, 0,
            "an unreadable subtree must not be pruned"
        );
        assert!(report.failed >= 1);
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
