//! Reading metadata out of audio files with `lofty`.

use std::fmt::Write as _;
use std::path::Path;

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::{Accessor, ItemKey};
use lofty::tag::items::popularimeter::{Popularimeter, StarRating};
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

/// Writes a star rating into the file, so the rating lives with the music
/// rather than only in the cache.
///
/// `stars` is 1 to 5, or `None` to remove the rating. The Windows Media
/// Player spelling is used, which is what Strawberry and most taggers read.
///
/// # Errors
///
/// Returns an error when the file cannot be read or written.
pub fn write_rating(path: &Path, stars: Option<u8>) -> Result<(), TagError> {
    let mut tagged = lofty::read_from_path(path).map_err(|source| TagError::Read {
        path: path.display().to_string(),
        source,
    })?;
    let tag_type = tagged.primary_tag_type();
    if tagged.primary_tag_mut().is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }
    let tag = tagged.primary_tag_mut().ok_or_else(|| TagError::Write {
        path: path.display().to_string(),
        message: "the file holds no tag that could be written".to_owned(),
    })?;

    match stars.and_then(star_rating) {
        Some(rating) => {
            let popularimeter = Popularimeter::windows_media_player(rating, 0);
            // The answer says whether this kind of tag can hold a rating at
            // all; ignoring it would report success having written nothing.
            if !tag.insert_text(ItemKey::Popularimeter, popularimeter.to_string()) {
                return Err(TagError::Write {
                    path: path.display().to_string(),
                    message: format!("{tag_type:?} tags cannot hold a rating"),
                });
            }
        }
        None => {
            tag.remove_key(ItemKey::Popularimeter);
        }
    }

    tagged
        .save_to_path(path, WriteOptions::default())
        .map_err(|err| TagError::Write {
            path: path.display().to_string(),
            message: causes(&err),
        })
}

fn star_rating(stars: u8) -> Option<StarRating> {
    match stars {
        1 => Some(StarRating::One),
        2 => Some(StarRating::Two),
        3 => Some(StarRating::Three),
        4 => Some(StarRating::Four),
        5 => Some(StarRating::Five),
        _ => None,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TagError {
    #[error("cannot read {path}: {}", causes(.source))]
    Read {
        path: String,
        #[source]
        source: lofty::error::FileParseError,
    },
    #[error("cannot write {path}: {message}")]
    Write { path: String, message: String },
}

/// The whole cause chain of an error, so the log says what actually went
/// wrong: lofty's own message is only "failed to parse Mpeg file".
fn causes(err: &dyn std::error::Error) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        write!(message, ": {cause}").expect("writing to a String cannot fail");
        source = cause.source();
    }
    message
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

    #[test]
    fn a_rating_is_written_into_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tagged_copy(dir.path(), "rated.mp3", &sample_tag());

        super::write_rating(&path, Some(2)).expect("write");
        assert_eq!(read_tags(&path).expect("read").rating, Some(2));

        super::write_rating(&path, Some(5)).expect("write");
        assert_eq!(read_tags(&path).expect("read").rating, Some(5));
    }

    #[test]
    fn a_rating_round_trips_through_a_flac_too() {
        // Every other rating test uses an MP3, so nothing would notice a
        // format whose tags cannot hold one.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("silence.flac");
        std::fs::copy(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/silence.flac"),
            &path,
        )
        .expect("copy");

        match super::write_rating(&path, Some(3)) {
            Ok(()) => assert_eq!(
                read_tags(&path).expect("read").rating,
                Some(3),
                "a rating reported as written must be readable again"
            ),
            Err(err) => {
                let message = err.to_string();
                assert!(
                    message.contains("cannot hold a rating"),
                    "a format that cannot hold a rating must say so: {message}"
                );
            }
        }
    }

    #[test]
    fn a_rating_can_be_taken_off_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tagged_copy(dir.path(), "rated.mp3", &sample_tag());
        super::write_rating(&path, Some(3)).expect("write");
        super::write_rating(&path, None).expect("write");
        assert_eq!(read_tags(&path).expect("read").rating, None);
    }

    #[test]
    fn a_rating_can_be_written_to_a_file_with_no_tag_yet() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bare.mp3");
        std::fs::copy(FIXTURE, &path).expect("copy");
        super::write_rating(&path, Some(4)).expect("write");
        assert_eq!(read_tags(&path).expect("read").rating, Some(4));
    }

    #[test]
    fn writing_a_rating_leaves_the_other_tags_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tagged_copy(dir.path(), "rated.mp3", &sample_tag());
        super::write_rating(&path, Some(1)).expect("write");
        let tags = read_tags(&path).expect("read");
        assert_eq!(tags.title.as_deref(), Some("Orange Sun"));
        assert_eq!(tags.grouping_raw.as_deref(), Some("Chant / Loud / Happy"));
    }

    #[test]
    fn an_impossible_rating_is_treated_as_no_rating() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tagged_copy(dir.path(), "rated.mp3", &sample_tag());
        super::write_rating(&path, Some(9)).expect("write");
        assert_eq!(read_tags(&path).expect("read").rating, None);
    }

    #[test]
    fn writing_to_a_missing_file_is_an_error() {
        assert!(super::write_rating(Path::new("/definitely/missing.mp3"), Some(3)).is_err());
    }

    #[test]
    fn the_error_message_says_what_went_wrong() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("broken.mp3");
        std::fs::write(&path, b"not an mp3 at all").expect("write");
        let message = read_tags(&path).expect_err("must fail").to_string();
        assert!(message.contains("broken.mp3"), "{message}");
        assert!(
            message.matches(':').count() >= 2,
            "the underlying cause must be included: {message}"
        );
    }
}
