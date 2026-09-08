//! Incremental folder scanning: walks the library folders, reads tags for
//! new or changed files, and prunes files that disappeared.

use std::collections::HashMap;
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::db::{Db, DbError, FileStamp, Folder};
use crate::tags::{is_audio_file, read_tags};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanReport {
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
    pub failed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanProgress {
    pub scanned: u64,
    pub total: u64,
    pub path: String,
}

/// Scans every folder, calling `on_progress` after each file is examined.
///
/// # Errors
///
/// Returns an error when the database cannot be read or written. Unreadable
/// audio files are counted in [`ScanReport::failed`] instead.
pub fn scan(
    db: &Db,
    folders: &[Folder],
    mut on_progress: impl FnMut(ScanProgress),
) -> Result<ScanReport, DbError> {
    let mut report = ScanReport::default();
    let mut files: Vec<(i64, FileStamp)> = Vec::new();
    for folder in folders {
        for file in audio_files(Path::new(&folder.path)) {
            files.push((folder.id, file));
        }
    }
    let total = files.len() as u64;

    let mut known: HashMap<String, FileStamp> = HashMap::new();
    for folder in folders {
        for stamp in db.file_stamps(folder.id)? {
            known.insert(stamp.path.clone(), stamp);
        }
    }

    for (index, (folder_id, stamp)) in files.iter().enumerate() {
        match known.remove(&stamp.path) {
            Some(previous) if previous == *stamp => {}
            previous => match read_tags(Path::new(&stamp.path)) {
                Ok(tags) => {
                    db.upsert_track(*folder_id, stamp, &tags)?;
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
        on_progress(ScanProgress {
            scanned: index as u64 + 1,
            total,
            path: stamp.path.clone(),
        });
    }

    for path in known.into_keys() {
        db.remove_track(&path)?;
        report.removed += 1;
    }
    Ok(report)
}

fn audio_files(root: &Path) -> impl Iterator<Item = FileStamp> {
    WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry),
            Err(err) => {
                log::warn!("cannot read directory entry: {err}");
                None
            }
        })
        .filter(|entry| entry.file_type().is_file() && is_audio_file(entry.path()))
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            let mtime = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
            Some(FileStamp {
                path: entry.path().to_string_lossy().into_owned(),
                mtime: i64::try_from(mtime.as_secs()).unwrap_or(i64::MAX),
                size: i64::try_from(metadata.len()).unwrap_or(i64::MAX),
            })
        })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use lofty::prelude::Accessor;

    use super::{scan, ScanProgress, ScanReport};
    use crate::db::Db;
    use crate::tags::tests::{sample_tag, tagged_copy, FIXTURE};

    fn setup() -> (tempfile::TempDir, Db, Vec<crate::db::Folder>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open_in_memory().expect("db");
        let folder = db
            .add_folder(&dir.path().to_string_lossy())
            .expect("add folder");
        (dir, db, vec![folder])
    }

    fn scan_all(db: &Db, folders: &[crate::db::Folder]) -> (ScanReport, Vec<ScanProgress>) {
        let mut progress = Vec::new();
        let report = scan(db, folders, |p| progress.push(p)).expect("scan");
        (report, progress)
    }

    #[test]
    fn adds_updates_and_removes_files_across_scans() {
        let (dir, db, folders) = setup();
        fs::create_dir(dir.path().join("sub")).expect("mkdir");
        tagged_copy(dir.path(), "one.mp3", &sample_tag());
        let two = tagged_copy(&dir.path().join("sub"), "two.mp3", &sample_tag());
        fs::write(dir.path().join("cover.jpg"), b"not audio").expect("write");

        let (report, progress) = scan_all(&db, &folders);
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
        assert_eq!(db.stats().expect("stats").track_count, 2);

        // Nothing changed: nothing is re-read.
        let (report, _) = scan_all(&db, &folders);
        assert_eq!(report, ScanReport::default());

        // Change one file (size changes because the title is longer).
        let mut tag = sample_tag();
        tag.set_title("A much longer title than before".into());
        tagged_copy(dir.path(), "one.mp3", &tag);
        fs::remove_file(&two).expect("remove");
        let (report, _) = scan_all(&db, &folders);
        assert_eq!(
            report,
            ScanReport {
                updated: 1,
                removed: 1,
                ..ScanReport::default()
            }
        );
        assert_eq!(db.stats().expect("stats").track_count, 1);
    }

    #[test]
    fn unreadable_audio_files_are_counted_not_fatal() {
        let (dir, db, folders) = setup();
        fs::write(dir.path().join("broken.mp3"), b"garbage").expect("write");
        fs::copy(FIXTURE, dir.path().join("fine.mp3")).expect("copy");
        let (report, _) = scan_all(&db, &folders);
        assert_eq!(report.added, 1);
        assert_eq!(report.failed, 1);
    }

    #[test]
    fn missing_folder_yields_no_files() {
        let db = Db::open_in_memory().expect("db");
        let folder = db.add_folder("/definitely/missing").expect("add");
        let (report, progress) = scan_all(&db, &[folder]);
        assert_eq!(report, ScanReport::default());
        assert!(progress.is_empty());
    }
}
