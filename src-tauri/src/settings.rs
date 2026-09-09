//! The settings file: what the user chose, as opposed to what the scanner
//! derived. It lives next to the library cache but survives it, so
//! recreating a corrupted cache does not lose the library folders.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The name of the settings file inside the application data directory.
pub const FILE_NAME: &str = "settings.json";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// The library folders, in the form they are stored in the database.
    #[serde(default)]
    pub folders: Vec<String>,
    /// The playlists the user built. Kept here rather than in the library
    /// cache because nothing can rebuild them from the music files.
    #[serde(default)]
    pub playlists: Vec<Playlist>,
}

/// A playlist, holding the paths of its tracks rather than their ids: an
/// id belongs to the cache and does not survive it being rebuilt.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub tracks: Vec<String>,
}

/// Reads the settings file, returning the defaults when it does not exist or
/// cannot be parsed: settings are a convenience, never a reason to fail.
#[must_use]
pub fn read(path: &Path) -> Settings {
    match std::fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|err| {
            log::warn!("ignoring unreadable settings ({err})");
            Settings::default()
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Settings::default(),
        Err(err) => {
            log::warn!("cannot read the settings ({err})");
            Settings::default()
        }
    }
}

/// Writes the settings, replacing the file atomically so an interrupted
/// write cannot leave a truncated one behind.
///
/// # Errors
///
/// Returns an error when the file cannot be written.
pub fn write(path: &Path, settings: &Settings) -> std::io::Result<()> {
    let contents = serde_json::to_string_pretty(settings)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let temporary = temporary_path(path);
    std::fs::write(&temporary, contents)?;
    std::fs::rename(&temporary, path)
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".new");
    name.into()
}

#[cfg(test)]
mod tests {
    use super::{read, write, Settings};

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(read(&dir.path().join("settings.json")), Settings::default());
    }

    #[test]
    fn settings_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let settings = Settings {
            folders: vec!["/music".to_owned(), "/more music".to_owned()],
            playlists: vec![super::Playlist {
                id: 1,
                name: "Favourites".to_owned(),
                tracks: vec!["/music/a.mp3".to_owned()],
            }],
        };
        write(&path, &settings).expect("write");
        assert_eq!(read(&path), settings);
    }

    #[test]
    fn settings_written_before_playlists_existed_still_load() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        std::fs::write(&path, br#"{"folders":["/music"]}"#).expect("write");
        let settings = read(&path);
        assert_eq!(settings.folders, ["/music"]);
        assert!(settings.playlists.is_empty());
    }

    #[test]
    fn a_broken_file_gives_the_defaults_instead_of_failing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"{ not json").expect("write");
        assert_eq!(read(&path), Settings::default());
    }

    #[test]
    fn writing_leaves_no_temporary_file_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        write(&path, &Settings::default()).expect("write");
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, ["settings.json"]);
    }
}
