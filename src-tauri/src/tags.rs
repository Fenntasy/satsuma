//! Reading metadata out of audio files with `lofty`.

use std::path::Path;

use lofty::file::TaggedFileExt;
use lofty::prelude::{Accessor, AudioFile, ItemKey};
use lofty::tag::Tag;
use serde::{Deserialize, Serialize};

use crate::grouping::Grouping;

/// File extensions the scanner considers audio files.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "opus", "m4a", "aac", "wav", "aiff", "aif", "wv", "ape",
];

/// Metadata read from a single audio file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub genre: Option<String>,
    pub year: Option<u32>,
    pub duration_ms: u64,
    /// Star rating from 1 to 5.
    pub rating: Option<u8>,
    /// The raw grouping tag as stored in the file.
    pub grouping_raw: Option<String>,
    /// The grouping tag parsed with the `Type / Volume / Vibe` rules, when
    /// it follows them.
    pub grouping: Option<Grouping>,
    pub has_embedded_cover: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum TagError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: lofty::error::FileParseError,
    },
}

/// Returns true when the path has an audio file extension.
#[must_use]
pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            let ext = ext.to_ascii_lowercase();
            AUDIO_EXTENSIONS.contains(&ext.as_str())
        })
}

/// Reads the tags and audio properties of a file.
///
/// # Errors
///
/// Returns an error when the file cannot be opened or parsed.
pub fn read_tags(path: &Path) -> Result<TrackTags, TagError> {
    let tagged = lofty::read_from_path(path).map_err(|source| TagError::Read {
        path: path.display().to_string(),
        source,
    })?;
    let duration_ms = u64::try_from(tagged.properties().duration().as_millis()).unwrap_or(u64::MAX);
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return Ok(TrackTags {
            duration_ms,
            ..TrackTags::default()
        });
    };
    Ok(from_tag(tag, duration_ms))
}

fn from_tag(tag: &Tag, duration_ms: u64) -> TrackTags {
    let grouping_raw = non_empty(tag.get_string(ItemKey::ContentGroup));
    TrackTags {
        title: non_empty(tag.title().as_deref()),
        artist: non_empty(tag.artist().as_deref()),
        album: non_empty(tag.album().as_deref()),
        album_artist: non_empty(tag.get_string(ItemKey::AlbumArtist)),
        track_number: tag.track(),
        disc_number: tag.disk(),
        genre: non_empty(tag.genre().as_deref()),
        year: tag.date().map(|date| u32::from(date.year)),
        duration_ms,
        rating: tag.ratings().next().map(|popm| popm.rating() as u8),
        grouping: grouping_raw.as_deref().and_then(Grouping::parse),
        grouping_raw,
        has_embedded_cover: !tag.pictures().is_empty(),
    }
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::{Path, PathBuf};

    use lofty::config::WriteOptions;
    use lofty::picture::{MimeType, Picture, PictureType};
    use lofty::prelude::{Accessor, ItemKey, TagExt};
    use lofty::tag::items::popularimeter::{Popularimeter, StarRating};
    use lofty::tag::items::Timestamp;
    use lofty::tag::{Tag, TagType};

    use super::{is_audio_file, read_tags};
    use crate::grouping::{Grouping, Kind, Vibe, Volume};

    pub(crate) const FIXTURE: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/silence.mp3");

    /// Copies the silent fixture into `dir` under `name` and writes the given
    /// tag into it.
    pub(crate) fn tagged_copy(dir: &Path, name: &str, tag: &Tag) -> PathBuf {
        let path = dir.join(name);
        std::fs::copy(FIXTURE, &path).expect("copy fixture");
        tag.save_to_path(&path, WriteOptions::default())
            .expect("write tags");
        path
    }

    pub(crate) fn sample_tag() -> Tag {
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_title("Orange Sun".into());
        tag.set_artist("The Satsumas".into());
        tag.set_album("Citrus".into());
        tag.insert_text(ItemKey::AlbumArtist, "Various Citrus".into());
        tag.set_track(3);
        tag.set_disk(1);
        tag.set_genre("Indie".into());
        tag.set_date(Timestamp {
            year: 2024,
            ..Timestamp::default()
        });
        tag.insert_text(ItemKey::ContentGroup, "Chant / Loud / Happy".into());
        tag.insert_text(
            ItemKey::Popularimeter,
            Popularimeter::windows_media_player(StarRating::Four, 0).to_string(),
        );
        tag
    }

    #[test]
    fn detects_audio_extensions_case_insensitively() {
        assert!(is_audio_file(Path::new("a/b/song.mp3")));
        assert!(is_audio_file(Path::new("a/b/song.FLAC")));
        assert!(!is_audio_file(Path::new("a/b/cover.jpg")));
        assert!(!is_audio_file(Path::new("a/b/noext")));
    }

    #[test]
    fn reads_every_field_from_a_tagged_mp3() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tagged_copy(dir.path(), "tagged.mp3", &sample_tag());
        let tags = read_tags(&path).expect("read tags");
        assert_eq!(tags.title.as_deref(), Some("Orange Sun"));
        assert_eq!(tags.artist.as_deref(), Some("The Satsumas"));
        assert_eq!(tags.album.as_deref(), Some("Citrus"));
        assert_eq!(tags.album_artist.as_deref(), Some("Various Citrus"));
        assert_eq!(tags.track_number, Some(3));
        assert_eq!(tags.disc_number, Some(1));
        assert_eq!(tags.genre.as_deref(), Some("Indie"));
        assert_eq!(tags.year, Some(2024));
        assert!(
            (900..=1200).contains(&tags.duration_ms),
            "{}",
            tags.duration_ms
        );
        assert_eq!(tags.rating, Some(4));
        assert_eq!(tags.grouping_raw.as_deref(), Some("Chant / Loud / Happy"));
        assert_eq!(
            tags.grouping,
            Some(Grouping {
                kind: Some(Kind::Chant),
                volume: Some(Volume::Loud),
                vibe: Some(Vibe::Happy),
            })
        );
        assert!(!tags.has_embedded_cover);
    }

    #[test]
    fn untagged_file_still_has_a_duration() {
        let tags = read_tags(Path::new(FIXTURE)).expect("read tags");
        assert_eq!(tags.title, None);
        assert_eq!(tags.rating, None);
        assert!(tags.duration_ms > 0);
    }

    #[test]
    fn keeps_raw_grouping_when_it_does_not_follow_the_rules() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tag = Tag::new(TagType::Id3v2);
        tag.insert_text(ItemKey::ContentGroup, "Workout".into());
        let path = tagged_copy(dir.path(), "odd.mp3", &tag);
        let tags = read_tags(&path).expect("read tags");
        assert_eq!(tags.grouping_raw.as_deref(), Some("Workout"));
        assert_eq!(tags.grouping, None);
    }

    #[test]
    fn detects_embedded_cover() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tag = Tag::new(TagType::Id3v2);
        tag.push_picture(
            Picture::unchecked(vec![0x89, b'P', b'N', b'G'])
                .pic_type(PictureType::CoverFront)
                .mime_type(MimeType::Png)
                .build(),
        );
        let path = tagged_copy(dir.path(), "cover.mp3", &tag);
        assert!(read_tags(&path).expect("read tags").has_embedded_cover);
    }

    #[test]
    fn unreadable_path_is_an_error() {
        assert!(read_tags(Path::new("/definitely/missing.mp3")).is_err());
    }
}
