//! The `SQLite` cache of the library. Music files stay the source of truth;
//! everything here can be rebuilt by rescanning.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::tags::TrackTags;

const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("folder already in the library: {0}")]
    DuplicateFolder(String),
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
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Db { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version < 1 {
            self.conn.execute_batch(
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
        self.conn
            .pragma_update(None, "user_version", SCHEMA_VERSION)?;
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
        match self
            .conn
            .execute("INSERT INTO folders (path) VALUES (?1)", [path])
        {
            Ok(_) => Ok(Folder {
                id: self.conn.last_insert_rowid(),
                path: path.to_owned(),
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

    /// Inserts or replaces the track at `stamp.path`.
    ///
    /// # Errors
    ///
    /// Returns an error on query failure.
    pub fn upsert_track(&self, folder_id: i64, stamp: &FileStamp, tags: &TrackTags) -> Result<()> {
        let grouping = tags.grouping.unwrap_or_default();
        self.conn.execute(
            "INSERT INTO tracks (
                folder_id, path, mtime, size, title, artist, album, album_artist,
                track_number, disc_number, genre, year, duration_ms, rating,
                grouping_raw, grouping_kind, grouping_volume, grouping_vibe,
                has_embedded_cover
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                ?15, ?16, ?17, ?18, ?19
            )
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
                has_embedded_cover = excluded.has_embedded_cover",
            params![
                folder_id,
                stamp.path,
                stamp.mtime,
                stamp.size,
                tags.title,
                tags.artist,
                tags.album,
                tags.album_artist,
                tags.track_number,
                tags.disc_number,
                tags.genre,
                tags.year,
                i64::try_from(tags.duration_ms).unwrap_or(i64::MAX),
                tags.rating,
                tags.grouping_raw,
                grouping.kind.map(|kind| serde_variant(&kind)),
                grouping.volume.map(|volume| serde_variant(&volume)),
                grouping.vibe.map(|vibe| serde_variant(&vibe)),
                tags.has_embedded_cover,
            ],
        )?;
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error on query failure.
    pub fn remove_track(&self, path: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM tracks WHERE path = ?1", [path])?;
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error on query failure.
    pub fn stats(&self) -> Result<LibraryStats> {
        let stats = self
            .conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(duration_ms), 0) FROM tracks",
                [],
                |row| {
                    Ok(LibraryStats {
                        track_count: row.get::<_, i64>(0)?.try_into().unwrap_or(0),
                        total_duration_ms: row.get::<_, i64>(1)?.try_into().unwrap_or(0),
                    })
                },
            )
            .optional()?
            .unwrap_or_default();
        Ok(stats)
    }

    #[cfg(test)]
    fn grouping_of(&self, path: &str) -> Result<Option<crate::grouping::Grouping>> {
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
            .optional()?;
        Ok(row.map(|(kind, volume, vibe)| crate::grouping::Grouping {
            kind: kind.and_then(|value| serde_json::from_value(value.into()).ok()),
            volume: volume.and_then(|value| serde_json::from_value(value.into()).ok()),
            vibe: vibe.and_then(|value| serde_json::from_value(value.into()).ok()),
        }))
    }
}

/// The lowercase serde name of a unit enum variant, as stored in the DB.
fn serde_variant<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{Db, DbError, FileStamp};
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
        db.upsert_track(folder.id, &stamp("/music/a.mp3"), &tags("A", 1000))
            .expect("upsert");
        db.upsert_track(folder.id, &stamp("/music/b.mp3"), &tags("B", 2000))
            .expect("upsert");
        db.upsert_track(folder.id, &stamp("/music/a.mp3"), &tags("A2", 3000))
            .expect("upsert");
        let stats = db.stats().expect("stats");
        assert_eq!(stats.track_count, 2);
        assert_eq!(stats.total_duration_ms, 5000);
    }

    #[test]
    fn file_stamps_and_removal() {
        let db = Db::open_in_memory().expect("db");
        let folder = db.add_folder("/music").expect("add");
        db.upsert_track(folder.id, &stamp("/music/a.mp3"), &tags("A", 1))
            .expect("upsert");
        assert_eq!(
            db.file_stamps(folder.id).expect("stamps"),
            vec![stamp("/music/a.mp3")]
        );
        db.remove_track("/music/a.mp3").expect("remove");
        assert!(db.file_stamps(folder.id).expect("stamps").is_empty());
    }

    #[test]
    fn removing_a_folder_removes_its_tracks() {
        let db = Db::open_in_memory().expect("db");
        let folder = db.add_folder("/music").expect("add");
        db.upsert_track(folder.id, &stamp("/music/a.mp3"), &tags("A", 1))
            .expect("upsert");
        db.remove_folder(folder.id).expect("remove");
        assert_eq!(db.stats().expect("stats").track_count, 0);
        assert!(db.list_folders().expect("list").is_empty());
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
        db.upsert_track(folder.id, &stamp("/music/a.mp3"), &track)
            .expect("upsert");
        assert_eq!(
            db.grouping_of("/music/a.mp3").expect("query"),
            Some(grouping)
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
