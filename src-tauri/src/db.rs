//! The `SQLite` cache of the library. Music files stay the source of truth;
//! everything here can be rebuilt by rescanning.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::tags::TrackTags;

const SCHEMA_VERSION: i64 = 1;

/// Writes one track, ignoring tracks whose folder was removed since the scan
/// collected them.
const UPSERT_TRACK: &str = "INSERT INTO tracks (
        folder_id, path, mtime, size, title, artist, album, album_artist,
        track_number, disc_number, genre, year, duration_ms, rating,
        grouping_raw, grouping_kind, grouping_volume, grouping_vibe,
        has_embedded_cover
    )
    SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
           ?15, ?16, ?17, ?18, ?19
    WHERE EXISTS (SELECT 1 FROM folders WHERE id = ?1)
    ON CONFLICT(path) DO UPDATE SET
        folder_id = excluded.folder_id,
        mtime = excluded.mtime,
        size = excluded.size,
        title = excluded.title,
        artist = excluded.artist,
        album = excluded.album,
        album_artist = excluded.album_artist,
        track_number = excluded.track_number,
        disc_number = excluded.disc_number,
        genre = excluded.genre,
        year = excluded.year,
        duration_ms = excluded.duration_ms,
        rating = excluded.rating,
        grouping_raw = excluded.grouping_raw,
        grouping_kind = excluded.grouping_kind,
        grouping_volume = excluded.grouping_volume,
        grouping_vibe = excluded.grouping_vibe,
        has_embedded_cover = excluded.has_embedded_cover";

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("folder already in the library: {0}")]
    DuplicateFolder(String),
    #[error("the library was created by a newer version of Satsuma (schema {0})")]
    SchemaTooNew(i64),
    #[error("the unusable library cache could not be moved aside: {0}")]
    CannotSetAside(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, DbError>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: i64,
    pub path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryStats {
    pub track_count: u64,
    pub total_duration_ms: u64,
}

/// A track to write to the cache, as produced by a scan.
#[derive(Clone, Debug)]
pub struct TrackRecord {
    pub folder_id: i64,
    pub stamp: FileStamp,
    pub tags: TrackTags,
}

/// A track as the player needs it: enough to play it and to show what is
/// playing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: i64,
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: u64,
}

/// A track as the library tree needs it: what it is grouped by, and what
/// is shown once a branch is open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryRow {
    pub id: i64,
    pub genre: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub title: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<u32>,
    pub duration_ms: u64,
}

/// A file as recorded by the last scan, used to skip unchanged files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileStamp {
    pub path: String,
    pub mtime: i64,
    pub size: i64,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    /// Opens (and migrates) the database file at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be opened or migrated.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    /// Opens the library cache at `path`, moving the existing file aside and
    /// starting a new one when it cannot be used at all: the cache is
    /// rebuildable, so a corrupted file or one written by a newer version of
    /// Satsuma must not prevent the app from starting. Failures that may be
    /// temporary (a locked file, an I/O error) are returned as-is so a
    /// healthy cache is never thrown away.
    ///
    /// # Errors
    ///
    /// Returns an error when the database cannot be opened and the failure
    /// does not mean the file is unusable, or when the replacement cannot be
    /// created either.
    pub fn open_or_recreate(path: &Path) -> Result<Self> {
        match Self::open(path) {
            Ok(db) => Ok(db),
            Err(err) if is_unusable(&err) => {
                log::warn!("the library cache is unusable ({err}); starting a new one");
                set_aside(path)?;
                Self::open(path)
            }
            Err(err) => Err(err),
        }
    }

    /// Opens an in-memory database, used by tests.
    ///
    /// # Errors
    ///
    /// Returns an error when the schema cannot be created.
    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Db { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(DbError::SchemaTooNew(version));
        }
        if version == SCHEMA_VERSION {
            return Ok(());
        }
        let transaction = self.conn.unchecked_transaction()?;
        if version < 1 {
            transaction.execute_batch(
                "CREATE TABLE folders (
                    id INTEGER PRIMARY KEY,
                    path TEXT NOT NULL UNIQUE
                );
                CREATE TABLE tracks (
                    id INTEGER PRIMARY KEY,
                    folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                    path TEXT NOT NULL UNIQUE,
                    mtime INTEGER NOT NULL,
                    size INTEGER NOT NULL,
                    title TEXT,
                    artist TEXT,
                    album TEXT,
                    album_artist TEXT,
                    track_number INTEGER,
                    disc_number INTEGER,
                    genre TEXT,
                    year INTEGER,
                    duration_ms INTEGER NOT NULL,
                    rating INTEGER,
                    grouping_raw TEXT,
                    grouping_kind TEXT,
                    grouping_volume TEXT,
                    grouping_vibe TEXT,
                    has_embedded_cover INTEGER NOT NULL DEFAULT 0,
                    added_at INTEGER NOT NULL DEFAULT (unixepoch())
                );
                CREATE INDEX tracks_folder ON tracks(folder_id);
                CREATE INDEX tracks_genre_artist_album ON tracks(genre, artist, album);",
            )?;
        }
        // The schema and the version it is stamped with must land together,
        // otherwise a crash in between leaves an unopenable database.
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        transaction.commit()?;
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error on query failure.
    pub fn list_folders(&self) -> Result<Vec<Folder>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path FROM folders ORDER BY path")?;
        let folders = stmt
            .query_map([], |row| {
                Ok(Folder {
                    id: row.get(0)?,
                    path: row.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(folders)
    }

    /// # Errors
    ///
    /// Returns [`DbError::DuplicateFolder`] when the path is already tracked.
    pub fn add_folder(&self, path: &str) -> Result<Folder> {
        let path = &normalize_folder(path);
        match self
            .conn
            .execute("INSERT INTO folders (path) VALUES (?1)", [path])
        {
            Ok(_) => Ok(Folder {
                id: self.conn.last_insert_rowid(),
                path: path.clone(),
            }),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(DbError::DuplicateFolder(path.to_owned()))
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Removes a folder and every track under it.
    ///
    /// # Errors
    ///
    /// Returns an error on query failure.
    pub fn remove_folder(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM folders WHERE id = ?1", [id])?;
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error on query failure.
    pub fn file_stamps(&self, folder_id: i64) -> Result<Vec<FileStamp>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, mtime, size FROM tracks WHERE folder_id = ?1")?;
        let stamps = stmt
            .query_map([folder_id], |row| {
                Ok(FileStamp {
                    path: row.get(0)?,
                    mtime: row.get(1)?,
                    size: row.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(stamps)
    }

    /// Inserts or replaces a batch of tracks in a single transaction.
    ///
    /// Tracks whose folder disappeared since the scan started are skipped
    /// rather than aborting the batch.
    ///
    /// # Errors
    ///
    /// Returns an error on query failure.
    pub fn upsert_tracks(&self, tracks: &[TrackRecord]) -> Result<()> {
        let transaction = self.conn.unchecked_transaction()?;
        {
            let mut stmt = transaction.prepare_cached(UPSERT_TRACK)?;
            for track in tracks {
                let (kind, volume, vibe) = track.tags.grouping.unwrap_or_default().stored_names();
                stmt.execute(params![
                    track.folder_id,
                    track.stamp.path,
                    track.stamp.mtime,
                    track.stamp.size,
                    track.tags.title,
                    track.tags.artist,
                    track.tags.album,
                    track.tags.album_artist,
                    track.tags.track_number,
                    track.tags.disc_number,
                    track.tags.genre,
                    track.tags.year,
                    i64::try_from(track.tags.duration_ms).unwrap_or(i64::MAX),
                    track.tags.rating,
                    track.tags.grouping_raw,
                    kind,
                    volume,
                    vibe,
                    track.tags.has_embedded_cover,
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// Removes a batch of tracks by path, in a single transaction.
    ///
    /// # Errors
    ///
    /// Returns an error on query failure.
    pub fn remove_tracks(&self, paths: &[String]) -> Result<()> {
        let transaction = self.conn.unchecked_transaction()?;
        {
            let mut stmt = transaction.prepare_cached("DELETE FROM tracks WHERE path = ?1")?;
            for path in paths {
                stmt.execute([path])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// Lists tracks in the order a library is usually played: by artist,
    /// then album, then disc and track number.
    ///
    /// # Errors
    ///
    /// Returns an error on query failure.
    pub fn list_tracks(&self, limit: Option<u32>) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, title, artist, album, duration_ms
             FROM tracks
             ORDER BY artist IS NULL, artist, album IS NULL, album,
                      disc_number IS NULL, disc_number,
                      track_number IS NULL, track_number, title
             LIMIT ?1",
        )?;
        let limit = limit.map_or(-1, i64::from);
        let tracks = stmt
            .query_map([limit], |row| {
                Ok(Track {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    title: row.get(2)?,
                    artist: row.get(3)?,
                    album: row.get(4)?,
                    duration_ms: row.get::<_, i64>(5)?.try_into().unwrap_or(0),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(tracks)
    }

    /// Everything the library tree groups by, for every track.
    ///
    /// # Errors
    ///
    /// Returns an error on query failure.
    pub fn library_rows(&self) -> Result<Vec<LibraryRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, genre, artist, album, title, track_number, disc_number,
                    year, duration_ms
             FROM tracks
             ORDER BY genre IS NULL, genre, artist IS NULL, artist,
                      album IS NULL, album,
                      disc_number IS NULL, disc_number,
                      track_number IS NULL, track_number, title",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(LibraryRow {
                    id: row.get(0)?,
                    genre: row.get(1)?,
                    artist: row.get(2)?,
                    album: row.get(3)?,
                    title: row.get(4)?,
                    track_number: row.get(5)?,
                    disc_number: row.get(6)?,
                    year: row.get(7)?,
                    duration_ms: row.get::<_, i64>(8)?.try_into().unwrap_or(0),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The tracks with these ids, in the order they were asked for. Ids
    /// that are not in the library are skipped.
    ///
    /// # Errors
    ///
    /// Returns an error on query failure.
    pub fn tracks_by_ids(&self, ids: &[i64]) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, path, title, artist, album, duration_ms
             FROM tracks WHERE id = ?1",
        )?;
        let mut tracks = Vec::with_capacity(ids.len());
        for id in ids {
            let track = stmt
                .query_row([id], |row| {
                    Ok(Track {
                        id: row.get(0)?,
                        path: row.get(1)?,
                        title: row.get(2)?,
                        artist: row.get(3)?,
                        album: row.get(4)?,
                        duration_ms: row.get::<_, i64>(5)?.try_into().unwrap_or(0),
                    })
                })
                .optional()?;
            if let Some(track) = track {
                tracks.push(track);
            }
        }
        Ok(tracks)
    }

    /// # Errors
    ///
    /// Returns an error on query failure.
    pub fn stats(&self) -> Result<LibraryStats> {
        let stats = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(duration_ms), 0) FROM tracks",
            [],
            |row| {
                Ok(LibraryStats {
                    track_count: row.get::<_, i64>(0)?.try_into().unwrap_or(0),
                    total_duration_ms: row.get::<_, i64>(1)?.try_into().unwrap_or(0),
                })
            },
        )?;
        Ok(stats)
    }

    #[cfg(test)]
    fn grouping_of(&self, path: &str) -> Option<crate::grouping::Grouping> {
        let row = self
            .conn
            .query_row(
                "SELECT grouping_kind, grouping_volume, grouping_vibe FROM tracks WHERE path = ?1",
                [path],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .ok();
        row.map(|(kind, volume, vibe)| crate::grouping::Grouping {
            kind: kind.and_then(|value| serde_json::from_value(value.into()).ok()),
            volume: volume.and_then(|value| serde_json::from_value(value.into()).ok()),
            vibe: vibe.and_then(|value| serde_json::from_value(value.into()).ok()),
        })
    }
}

/// The form a folder path is stored in, so the same directory reached by two
/// spellings (a trailing separator, a symlink) is not added twice.
fn normalize_folder(path: &str) -> String {
    std::fs::canonicalize(path).map_or_else(
        |_| path.trim_end_matches(['/', '\\']).to_owned(),
        |real| strip_verbatim_prefix(&real.to_string_lossy()),
    )
}

/// Removes the `\\?\` prefix Windows canonicalisation adds. Keeping it would
/// show `\\?\C:\Music` in the folder list and make every stored path differ
/// from the one the user knows.
fn strip_verbatim_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        // `\\?\UNC\server\share` is the share `\\server\share`.
        return format!(r"\\{rest}");
    }
    path.strip_prefix(r"\\?\").unwrap_or(path).to_owned()
}

/// Moves an unusable cache out of the way, keeping it for inspection under a
/// name that does not overwrite an older one.
fn set_aside(path: &Path) -> Result<()> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    // A crash loop can reach this twice within the same second, and renaming
    // over the previous copy would destroy what it is kept for.
    let mut backup = None;
    for attempt in 0..1000 {
        let candidate = if attempt == 0 {
            with_suffix(path, &format!(".unusable-{stamp}"))
        } else {
            with_suffix(path, &format!(".unusable-{stamp}-{attempt}"))
        };
        if !candidate.exists() {
            backup = Some(candidate);
            break;
        }
    }
    let Some(backup) = backup else {
        log::error!("no free name to set {} aside", path.display());
        return Err(DbError::CannotSetAside(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "too many unusable library caches are already kept",
        )));
    };
    for suffix in ["", "-wal", "-shm"] {
        let from = with_suffix(path, suffix);
        if from.exists() {
            std::fs::rename(&from, with_suffix(&backup, suffix)).inspect_err(|err| {
                log::error!("cannot move {} aside: {err}", from.display());
            })?;
        }
    }
    Ok(())
}

/// Whether the file behind this error can never be opened, however many
/// times we try.
fn is_unusable(err: &DbError) -> bool {
    match err {
        DbError::SchemaTooNew(_) => true,
        DbError::Sqlite(rusqlite::Error::SqliteFailure(failure, _)) => matches!(
            failure.code,
            rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt
        ),
        _ => false,
    }
}

fn with_suffix(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

#[cfg(test)]
mod tests {
    use super::{Db, DbError, FileStamp, TrackRecord};
    use crate::grouping::{Grouping, Kind, Vibe};
    use crate::tags::TrackTags;

    fn stamp(path: &str) -> FileStamp {
        FileStamp {
            path: path.to_owned(),
            mtime: 10,
            size: 20,
        }
    }

    fn tags(title: &str, duration_ms: u64) -> TrackTags {
        TrackTags {
            title: Some(title.to_owned()),
            duration_ms,
            ..TrackTags::default()
        }
    }

    fn record(folder_id: i64, path: &str, tags: TrackTags) -> TrackRecord {
        TrackRecord {
            folder_id,
            stamp: stamp(path),
            tags,
        }
    }

    #[test]
    fn windows_verbatim_prefixes_are_stripped() {
        use super::strip_verbatim_prefix;

        assert_eq!(strip_verbatim_prefix(r"\\?\C:\Music"), r"C:\Music");
        assert_eq!(strip_verbatim_prefix(r"\\?\UNC\nas\music"), r"\\nas\music");
        assert_eq!(strip_verbatim_prefix("/Users/me/Music"), "/Users/me/Music");
        assert_eq!(strip_verbatim_prefix(r"C:\Music"), r"C:\Music");
    }

    #[test]
    fn the_same_folder_cannot_be_added_twice_under_two_spellings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open_in_memory().expect("db");
        let path = dir.path().to_string_lossy().into_owned();
        db.add_folder(&path).expect("add");
        assert!(matches!(
            db.add_folder(&format!("{path}/")),
            Err(DbError::DuplicateFolder(_))
        ));
        assert_eq!(db.list_folders().expect("list").len(), 1);
    }

    #[test]
    fn folders_are_listed_sorted_and_unique() {
        let db = Db::open_in_memory().expect("db");
        db.add_folder("/music/b").expect("add");
        db.add_folder("/music/a").expect("add");
        let paths: Vec<String> = db
            .list_folders()
            .expect("list")
            .into_iter()
            .map(|folder| folder.path)
            .collect();
        assert_eq!(paths, ["/music/a", "/music/b"]);
        assert!(matches!(
            db.add_folder("/music/a"),
            Err(DbError::DuplicateFolder(path)) if path == "/music/a"
        ));
    }

    #[test]
    fn upsert_replaces_and_stats_sum_durations() {
        let db = Db::open_in_memory().expect("db");
        let folder = db.add_folder("/music").expect("add");
        db.upsert_tracks(&[
            record(folder.id, "/music/a.mp3", tags("A", 1000)),
            record(folder.id, "/music/b.mp3", tags("B", 2000)),
        ])
        .expect("upsert");
        db.upsert_tracks(&[record(folder.id, "/music/a.mp3", tags("A2", 3000))])
            .expect("upsert");
        let stats = db.stats().expect("stats");
        assert_eq!(stats.track_count, 2);
        assert_eq!(stats.total_duration_ms, 5000);
    }

    #[test]
    fn file_stamps_and_removal() {
        let db = Db::open_in_memory().expect("db");
        let folder = db.add_folder("/music").expect("add");
        db.upsert_tracks(&[record(folder.id, "/music/a.mp3", tags("A", 1))])
            .expect("upsert");
        assert_eq!(
            db.file_stamps(folder.id).expect("stamps"),
            vec![stamp("/music/a.mp3")]
        );
        db.remove_tracks(&["/music/a.mp3".to_owned()])
            .expect("remove");
        assert!(db.file_stamps(folder.id).expect("stamps").is_empty());
    }

    #[test]
    fn removing_a_folder_removes_its_tracks() {
        let db = Db::open_in_memory().expect("db");
        let folder = db.add_folder("/music").expect("add");
        db.upsert_tracks(&[record(folder.id, "/music/a.mp3", tags("A", 1))])
            .expect("upsert");
        db.remove_folder(folder.id).expect("remove");
        assert_eq!(db.stats().expect("stats").track_count, 0);
        assert!(db.list_folders().expect("list").is_empty());
    }

    #[test]
    fn tracks_are_listed_by_artist_then_album_then_track() {
        let db = Db::open_in_memory().expect("db");
        let folder = db.add_folder("/music").expect("add");
        let entry = |path: &str, artist: &str, album: &str, number: u32, title: &str| TrackRecord {
            folder_id: folder.id,
            stamp: stamp(path),
            tags: TrackTags {
                title: Some(title.to_owned()),
                artist: Some(artist.to_owned()),
                album: Some(album.to_owned()),
                track_number: Some(number),
                duration_ms: 1000,
                ..TrackTags::default()
            },
        };
        let entry_without_number =
            |path: &str, artist: &str, album: &str, title: &str| TrackRecord {
                folder_id: folder.id,
                stamp: stamp(path),
                tags: TrackTags {
                    title: Some(title.to_owned()),
                    artist: Some(artist.to_owned()),
                    album: Some(album.to_owned()),
                    duration_ms: 1000,
                    ..TrackTags::default()
                },
            };
        db.upsert_tracks(&[
            entry("/music/c.mp3", "Beta", "Second", 1, "C"),
            entry("/music/b.mp3", "Alpha", "First", 2, "B"),
            entry("/music/a.mp3", "Alpha", "First", 1, "A"),
        ])
        .expect("upsert");

        let titles: Vec<String> = db
            .list_tracks(None)
            .expect("list")
            .into_iter()
            .filter_map(|track| track.title)
            .collect();
        assert_eq!(titles, ["A", "B", "C"]);
        assert_eq!(db.list_tracks(Some(2)).expect("list").len(), 2);

        // Within an album, an untagged track number sorts after the
        // numbered ones rather than ahead of track 1.
        db.upsert_tracks(&[entry_without_number("/music/z.mp3", "Alpha", "First", "Z")])
            .expect("upsert");
        let titles: Vec<String> = db
            .list_tracks(None)
            .expect("list")
            .into_iter()
            .filter_map(|track| track.title)
            .collect();
        assert_eq!(titles, ["A", "B", "Z", "C"]);

        // A track with no artist sorts after the ones that have any.
        db.upsert_tracks(&[TrackRecord {
            folder_id: folder.id,
            stamp: stamp("/music/d.mp3"),
            tags: TrackTags {
                title: Some("D".to_owned()),
                duration_ms: 1000,
                ..TrackTags::default()
            },
        }])
        .expect("upsert");
        let titles: Vec<String> = db
            .list_tracks(None)
            .expect("list")
            .into_iter()
            .filter_map(|track| track.title)
            .collect();
        assert_eq!(titles, ["A", "B", "Z", "C", "D"]);
    }

    #[test]
    fn library_rows_carry_what_the_tree_groups_by() {
        let db = Db::open_in_memory().expect("db");
        let folder = db.add_folder("/music").expect("add");
        db.upsert_tracks(&[TrackRecord {
            folder_id: folder.id,
            stamp: stamp("/music/a.mp3"),
            tags: TrackTags {
                title: Some("A".to_owned()),
                artist: Some("Artist".to_owned()),
                album: Some("Album".to_owned()),
                genre: Some("Indie".to_owned()),
                track_number: Some(3),
                disc_number: Some(1),
                year: Some(2024),
                duration_ms: 1000,
                ..TrackTags::default()
            },
        }])
        .expect("upsert");

        let rows = db.library_rows().expect("rows");
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.genre.as_deref(), Some("Indie"));
        assert_eq!(row.artist.as_deref(), Some("Artist"));
        assert_eq!(row.album.as_deref(), Some("Album"));
        assert_eq!(row.title.as_deref(), Some("A"));
        assert_eq!(row.track_number, Some(3));
        assert_eq!(row.year, Some(2024));
        assert_eq!(row.duration_ms, 1000);
    }

    #[test]
    fn tracks_are_fetched_in_the_order_they_were_asked_for() {
        let db = Db::open_in_memory().expect("db");
        let folder = db.add_folder("/music").expect("add");
        let entry = |path: &str, title: &str| TrackRecord {
            folder_id: folder.id,
            stamp: stamp(path),
            tags: TrackTags {
                title: Some(title.to_owned()),
                duration_ms: 1000,
                ..TrackTags::default()
            },
        };
        db.upsert_tracks(&[
            entry("/music/a.mp3", "A"),
            entry("/music/b.mp3", "B"),
            entry("/music/c.mp3", "C"),
        ])
        .expect("upsert");
        let ids: Vec<i64> = db
            .list_tracks(None)
            .expect("list")
            .into_iter()
            .map(|track| track.id)
            .collect();

        let asked = vec![ids[2], ids[0], 9999];
        let titles: Vec<String> = db
            .tracks_by_ids(&asked)
            .expect("fetch")
            .into_iter()
            .filter_map(|track| track.title)
            .collect();
        assert_eq!(titles, ["C", "A"], "an unknown id is skipped, order kept");
        assert!(db.tracks_by_ids(&[]).expect("fetch").is_empty());
    }

    #[test]
    fn grouping_parts_are_stored_separately() {
        let db = Db::open_in_memory().expect("db");
        let folder = db.add_folder("/music").expect("add");
        let grouping = Grouping {
            kind: Some(Kind::Instru),
            volume: None,
            vibe: Some(Vibe::Dark),
        };
        let track = TrackTags {
            grouping: Some(grouping),
            grouping_raw: Some("Instru / / Dark".to_owned()),
            ..TrackTags::default()
        };
        db.upsert_tracks(&[record(folder.id, "/music/a.mp3", track)])
            .expect("upsert");
        assert_eq!(db.grouping_of("/music/a.mp3"), Some(grouping));
    }

    #[test]
    fn tracks_of_a_removed_folder_are_skipped_not_fatal() {
        let db = Db::open_in_memory().expect("db");
        let folder = db.add_folder("/music").expect("add");
        db.remove_folder(folder.id).expect("remove");
        db.upsert_tracks(&[record(folder.id, "/music/a.mp3", tags("A", 1))])
            .expect("upsert must not fail on a vanished folder");
        assert_eq!(db.stats().expect("stats").track_count, 0);
    }

    #[test]
    fn refuses_a_database_from_a_newer_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("library.sqlite");
        {
            let conn = rusqlite::Connection::open(&path).expect("open");
            conn.pragma_update(None, "user_version", 99).expect("stamp");
        }
        assert!(matches!(Db::open(&path), Err(DbError::SchemaTooNew(99))));
    }

    #[test]
    fn an_unusable_cache_is_moved_aside_and_recreated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("library.sqlite");
        std::fs::write(&path, b"this is not a database").expect("write garbage");

        let db = Db::open_or_recreate(&path).expect("recreate");
        assert!(db.list_folders().expect("list").is_empty());
        assert_eq!(
            std::fs::read(set_aside_file(dir.path())).expect("backup"),
            b"this is not a database",
            "the unusable file must be kept for inspection"
        );
    }

    /// The single `library.sqlite.unusable-<stamp>` file in `dir`.
    fn set_aside_file(dir: &std::path::Path) -> std::path::PathBuf {
        let mut found: Vec<_> = std::fs::read_dir(dir)
            .expect("read dir")
            .filter_map(|entry| {
                let path = entry.expect("entry").path();
                let name = path.file_name()?.to_string_lossy().into_owned();
                name.starts_with("library.sqlite.unusable-").then_some(path)
            })
            .collect();
        assert_eq!(found.len(), 1, "exactly one cache must have been set aside");
        found.pop().expect("one file")
    }

    #[test]
    fn a_cache_from_a_newer_version_is_moved_aside() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("library.sqlite");
        {
            let conn = rusqlite::Connection::open(&path).expect("open");
            conn.pragma_update(None, "user_version", 99).expect("stamp");
        }
        Db::open_or_recreate(&path).expect("recreate");
        assert!(set_aside_file(dir.path()).exists());
    }

    #[test]
    fn a_healthy_cache_is_never_moved_aside() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("library.sqlite");
        Db::open_or_recreate(&path)
            .expect("create")
            .add_folder("/music")
            .expect("add");
        let db = Db::open_or_recreate(&path).expect("reopen");
        assert_eq!(db.list_folders().expect("list").len(), 1);
        assert!(
            std::fs::read_dir(dir.path())
                .expect("read dir")
                .all(|entry| !entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .contains("unusable")),
            "a healthy cache must not be moved aside"
        );
    }

    #[test]
    fn reopening_keeps_schema_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("library.sqlite");
        Db::open(&path).expect("open");
        let db = Db::open(&path).expect("reopen");
        assert!(db.list_folders().expect("list").is_empty());
    }
}
