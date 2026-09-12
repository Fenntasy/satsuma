//! Reading metadata out of audio files with `lofty`.

use std::fmt::Write as _;
use std::path::Path;

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::{Accessor, ItemKey};
use lofty::tag::items::popularimeter::{Popularimeter, StarRating};
use lofty::tag::items::Timestamp;
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

/// What an edit does to one field.
///
/// Three states rather than two, and the third is the point. One edit is
/// applied to many tracks at once, and the fields the user did not touch
/// have to survive it even where the tracks disagree about them. An
/// `Option` cannot say this: `None` would have to mean both "leave it
/// alone" and "empty it", and a batch edit that confuses those flattens
/// every track to whatever the form happened to show.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase", tag = "action", content = "value")]
pub enum Change<T> {
    /// The file keeps what it has.
    #[default]
    Keep,
    /// Write this value.
    Set(T),
    /// Take the field off the file.
    Clear,
}

impl<T> Change<T> {
    fn is_keep(&self) -> bool {
        matches!(self, Change::Keep)
    }
}

/// An edit to the tags of a file. Every field defaults to [`Change::Keep`],
/// so a change names only what it touches.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct TagEdit {
    pub title: Change<String>,
    pub artist: Change<String>,
    pub album: Change<String>,
    pub album_artist: Change<String>,
    pub track_number: Change<u32>,
    pub disc_number: Change<u32>,
    pub genre: Change<String>,
    pub year: Change<u32>,
    pub grouping: Change<Grouping>,
}

impl TagEdit {
    /// Whether this edit asks for nothing.
    ///
    /// An edit that asks for nothing writes nothing: opening the file to
    /// save tags it already has would change its modification time, and
    /// the next scan would read every one of them again for no reason.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.title.is_keep()
            && self.artist.is_keep()
            && self.album.is_keep()
            && self.album_artist.is_keep()
            && self.track_number.is_keep()
            && self.disc_number.is_keep()
            && self.genre.is_keep()
            && self.year.is_keep()
            && self.grouping.is_keep()
    }
}

/// Writes a star rating into the file, so the rating lives with the music
/// rather than only in the cache.
///
/// `stars` is 1 to 5, or `None` to remove the rating. In an `ID3v2` tag it
/// is a `POPM` frame in the Windows Media Player spelling, which is what
/// Strawberry and most taggers read; other tag formats write it their own
/// way. A tag that cannot hold a rating at all is refused rather than
/// silently left unrated.
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

    let stars = match stars {
        Some(stars) => Some(star_rating(stars).ok_or_else(|| TagError::Write {
            path: path.display().to_string(),
            message: format!("a rating is one to five stars, not {stars}"),
        })?),
        None => None,
    };
    match stars {
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

/// Applies an edit to the tags of a file.
///
/// The fields the edit does not name are left exactly as the file has
/// them, which is what lets one edit cover many tracks. An edit that names
/// nothing does not touch the file at all.
///
/// # Errors
///
/// Returns an error when the file cannot be read or written, or when its
/// kind of tag cannot hold one of the fields asked for. A field that
/// cannot be written is refused rather than skipped: reporting success
/// having written nothing is how a cache and a file come to disagree.
pub fn write_tags(path: &Path, edit: &TagEdit) -> Result<(), TagError> {
    if edit.is_empty() {
        return Ok(());
    }
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

    apply(tag, edit).map_err(|field| TagError::Write {
        path: path.display().to_string(),
        message: format!("{tag_type:?} tags cannot hold {field}"),
    })?;

    tagged
        .save_to_path(path, WriteOptions::default())
        .map_err(|err| TagError::Write {
            path: path.display().to_string(),
            message: causes(&err),
        })
}

/// Carries out the edit on a tag in memory. The error is the name of the
/// field this kind of tag has no room for.
fn apply(tag: &mut Tag, edit: &TagEdit) -> Result<(), &'static str> {
    match &edit.title {
        Change::Keep => {}
        Change::Set(value) => tag.set_title(value.clone()),
        Change::Clear => tag.remove_title(),
    }
    match &edit.artist {
        Change::Keep => {}
        Change::Set(value) => tag.set_artist(value.clone()),
        Change::Clear => tag.remove_artist(),
    }
    match &edit.album {
        Change::Keep => {}
        Change::Set(value) => tag.set_album(value.clone()),
        Change::Clear => tag.remove_album(),
    }
    match &edit.genre {
        Change::Keep => {}
        Change::Set(value) => tag.set_genre(value.clone()),
        Change::Clear => tag.remove_genre(),
    }
    match &edit.track_number {
        Change::Keep => {}
        Change::Set(value) => tag.set_track(*value),
        Change::Clear => tag.remove_track(),
    }
    match &edit.disc_number {
        Change::Keep => {}
        Change::Set(value) => tag.set_disk(*value),
        Change::Clear => tag.remove_disk(),
    }
    match &edit.year {
        Change::Keep => {}
        Change::Set(value) => {
            // A date is more than a year, and the library only ever shows
            // the year. Whatever month and day the file carries are kept,
            // so editing a year does not quietly throw the rest away.
            // The conversion says nothing worth keeping: a year that
            // does not fit in the field has only one thing wrong with it.
            let Ok(year) = u16::try_from(*value) else {
                return Err("a year that far ahead");
            };
            let date = Timestamp {
                year,
                ..tag.date().unwrap_or_default()
            };
            tag.set_date(date);
        }
        Change::Clear => tag.remove_date(),
    }
    // These two have no accessor of their own and go in by key, which
    // answers whether this kind of tag has anywhere to put them.
    text(tag, ItemKey::AlbumArtist, &edit.album_artist).map_err(|()| "an album artist")?;
    let grouping = match &edit.grouping {
        Change::Keep => Change::Keep,
        // An empty grouping is no grouping: writing `/ /` would leave a
        // tag that says nothing and that the parser reads back as empty.
        Change::Set(grouping) if grouping.is_empty() => Change::Clear,
        Change::Set(grouping) => Change::Set(grouping.to_string()),
        Change::Clear => Change::Clear,
    };
    text(tag, ItemKey::ContentGroup, &grouping).map_err(|()| "a grouping")
}

/// Sets or removes a field addressed by key. `insert_text` answers whether
/// the tag could hold it, and a `false` that went unchecked would be a
/// write that quietly did nothing.
fn text(tag: &mut Tag, key: ItemKey, change: &Change<String>) -> Result<(), ()> {
    match change {
        Change::Keep => Ok(()),
        Change::Set(value) => {
            if tag.insert_text(key, value.clone()) {
                Ok(())
            } else {
                Err(())
            }
        }
        Change::Clear => {
            tag.remove_key(key);
            Ok(())
        }
    }
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
    use lofty::file::TaggedFileExt;
    use lofty::picture::{MimeType, Picture, PictureType};
    use lofty::prelude::{Accessor, ItemKey, TagExt};
    use lofty::tag::items::popularimeter::{Popularimeter, StarRating};
    use lofty::tag::items::Timestamp;
    use lofty::tag::{Tag, TagType};

    use super::{is_audio_file, read_tags, write_tags, Change, TagEdit, TagError, TrackTags};
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
        let error = read_tags(Path::new("/definitely/missing.mp3")).unwrap_err();
        // The path has to be in the message: "cannot read tags" alone tells
        // nobody which file of a library-wide scan went wrong.
        assert!(
            matches!(&error, TagError::Read { path, .. } if path == "/definitely/missing.mp3"),
            "expected a read error naming the file, got {error:?}"
        );
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
        // format that stores ratings another way.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("silence.flac");
        std::fs::copy(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/silence.flac"),
            &path,
        )
        .expect("copy");

        super::write_rating(&path, Some(3)).expect("write");
        assert_eq!(
            read_tags(&path).expect("read").rating,
            Some(3),
            "the rating must survive in a format that is not ID3"
        );

        super::write_rating(&path, None).expect("write");
        assert_eq!(read_tags(&path).expect("read").rating, None);
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
    fn an_impossible_rating_is_refused_rather_than_clearing_the_one_there() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tagged_copy(dir.path(), "rated.mp3", &sample_tag());
        super::write_rating(&path, Some(3)).expect("write");

        assert!(super::write_rating(&path, Some(9)).is_err());
        assert!(super::write_rating(&path, Some(0)).is_err());
        assert_eq!(
            read_tags(&path).expect("read").rating,
            Some(3),
            "a refused write must leave the rating alone"
        );
    }

    #[test]
    fn writing_to_a_missing_file_is_an_error() {
        // Which error, not merely that there was one: a file that is not
        // there failed to be read, and saying it could not be written
        // would send whoever reads the log looking at permissions.
        let error = super::write_rating(Path::new("/definitely/missing.mp3"), Some(3))
            .expect_err("a file that is not there cannot be rated");
        assert!(
            matches!(&error, TagError::Read { path, .. } if path == "/definitely/missing.mp3"),
            "expected a read error naming the file, got {error:?}"
        );
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

    // WRITING TAGS

    /// The edit that changes one field and names nothing else.
    fn only(edit: TagEdit) -> TagEdit {
        edit
    }

    #[test]
    fn an_edit_that_names_nothing_leaves_the_file_alone() {
        // The rule the whole batch rests on. A form where the user typed
        // in one box sends `Keep` for every other field, across tracks
        // whose values differ; if `Keep` wrote anything at all, one edit
        // would flatten a hundred files to whatever the form showed.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tagged_copy(dir.path(), "untouched.mp3", &sample_tag());
        let before = read_tags(&path).expect("read");
        // Pushed into the past first. The file system's clock is coarse —
        // whole jiffies on the Linux that gates this branch — so a file
        // rewritten immediately after being written keeps the same
        // timestamp, and comparing before and after would prove nothing
        // there even though it does here.
        let written_at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|file| file.set_modified(written_at))
            .expect("set the modification time");

        write_tags(&path, &TagEdit::default()).expect("write");

        assert_eq!(read_tags(&path).expect("read"), before, "no tag may move");
        // The modification time rather than the length: rewriting the same
        // tags produces a file of the same size, so the size cannot tell
        // whether the file was opened at all. The scanner skips a file by
        // its time and size, and a rewrite that changed nothing would
        // still send it through every tag again.
        assert_eq!(
            std::fs::metadata(&path)
                .and_then(|it| it.modified())
                .expect("metadata"),
            written_at,
            "a file nothing was asked of must not be written to"
        );
    }

    #[test]
    fn every_field_can_be_written_and_read_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tagged_copy(dir.path(), "written.mp3", &sample_tag());

        write_tags(
            &path,
            &TagEdit {
                title: Change::Set("New Title".to_owned()),
                artist: Change::Set("New Artist".to_owned()),
                album: Change::Set("New Album".to_owned()),
                album_artist: Change::Set("New Album Artist".to_owned()),
                track_number: Change::Set(7),
                disc_number: Change::Set(2),
                genre: Change::Set("Jazz".to_owned()),
                year: Change::Set(1977),
                grouping: Change::Set(Grouping {
                    kind: Some(Kind::Instru),
                    volume: Some(Volume::Soft),
                    vibe: Some(Vibe::Sad),
                }),
            },
        )
        .expect("write");

        // Read back off the disk, not out of what we sent: the point is
        // that another player can see it.
        let tags = read_tags(&path).expect("read");
        assert_eq!(tags.title.as_deref(), Some("New Title"));
        assert_eq!(tags.artist.as_deref(), Some("New Artist"));
        assert_eq!(tags.album.as_deref(), Some("New Album"));
        assert_eq!(tags.album_artist.as_deref(), Some("New Album Artist"));
        assert_eq!(tags.track_number, Some(7));
        assert_eq!(tags.disc_number, Some(2));
        assert_eq!(tags.genre.as_deref(), Some("Jazz"));
        assert_eq!(tags.year, Some(1977));
        assert_eq!(tags.grouping_raw.as_deref(), Some("Instru / Soft / Sad"));
        assert_eq!(
            tags.grouping,
            Some(Grouping {
                kind: Some(Kind::Instru),
                volume: Some(Volume::Soft),
                vibe: Some(Vibe::Sad),
            })
        );
    }

    #[test]
    fn changing_one_field_leaves_its_neighbours_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tagged_copy(dir.path(), "one.mp3", &sample_tag());
        let before = read_tags(&path).expect("read");

        write_tags(
            &path,
            &only(TagEdit {
                genre: Change::Set("Jazz".to_owned()),
                ..TagEdit::default()
            }),
        )
        .expect("write");

        let after = read_tags(&path).expect("read");
        assert_eq!(after.genre.as_deref(), Some("Jazz"));
        assert_eq!(
            TrackTags {
                genre: before.genre.clone(),
                ..after.clone()
            },
            before,
            "nothing but the genre may differ"
        );
    }

    #[test]
    fn clearing_a_field_takes_it_off_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tagged_copy(dir.path(), "cleared.mp3", &sample_tag());
        assert!(
            read_tags(&path).expect("read").genre.is_some(),
            "the genre has to be there for clearing it to mean anything"
        );

        write_tags(
            &path,
            &only(TagEdit {
                genre: Change::Clear,
                album_artist: Change::Clear,
                year: Change::Clear,
                grouping: Change::Clear,
                ..TagEdit::default()
            }),
        )
        .expect("write");

        let tags = read_tags(&path).expect("read");
        assert_eq!(tags.genre, None);
        assert_eq!(tags.album_artist, None);
        assert_eq!(tags.year, None);
        assert_eq!(tags.grouping_raw, None);
        assert_eq!(
            tags.title.as_deref(),
            Some("Orange Sun"),
            "clearing some fields must not clear the others"
        );
    }

    #[test]
    fn clearing_is_not_the_same_as_leaving_alone() {
        // The distinction the `Change` type exists for, asserted rather
        // than assumed: the same field, the same file, two different
        // outcomes.
        let dir = tempfile::tempdir().expect("tempdir");
        let kept = tagged_copy(dir.path(), "kept.mp3", &sample_tag());
        let cleared = tagged_copy(dir.path(), "cleared.mp3", &sample_tag());

        write_tags(
            &kept,
            &only(TagEdit {
                title: Change::Set("Whatever".to_owned()),
                ..TagEdit::default()
            }),
        )
        .expect("write");
        write_tags(
            &cleared,
            &only(TagEdit {
                title: Change::Set("Whatever".to_owned()),
                genre: Change::Clear,
                ..TagEdit::default()
            }),
        )
        .expect("write");

        assert_eq!(
            read_tags(&kept).expect("read").genre.as_deref(),
            Some("Indie")
        );
        assert_eq!(read_tags(&cleared).expect("read").genre, None);
    }

    #[test]
    fn editing_the_year_keeps_the_rest_of_the_date() {
        // The library shows a year, but the file may hold a whole date.
        // Writing the year must not throw the month and day away.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tag = sample_tag();
        tag.set_date(Timestamp {
            year: 2024,
            month: Some(3),
            day: Some(17),
            ..Timestamp::default()
        });
        let path = tagged_copy(dir.path(), "dated.mp3", &tag);

        write_tags(
            &path,
            &only(TagEdit {
                year: Change::Set(1999),
                ..TagEdit::default()
            }),
        )
        .expect("write");

        let written = lofty::read_from_path(&path).expect("read");
        let date = TaggedFileExt::primary_tag(&written)
            .expect("a tag")
            .date()
            .expect("a date");
        assert_eq!(date.year, 1999);
        assert_eq!((date.month, date.day), (Some(3), Some(17)));
    }

    #[test]
    fn an_empty_grouping_takes_the_tag_off_rather_than_writing_separators() {
        // `/ /` is not a grouping, it is the shape of one with nothing in
        // it, and it would sit in the file looking like data.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tagged_copy(dir.path(), "grouping.mp3", &sample_tag());

        write_tags(
            &path,
            &only(TagEdit {
                grouping: Change::Set(Grouping::default()),
                ..TagEdit::default()
            }),
        )
        .expect("write");

        assert_eq!(read_tags(&path).expect("read").grouping_raw, None);
    }

    #[test]
    fn writing_tags_to_a_missing_file_says_which_file() {
        let error = write_tags(
            Path::new("/definitely/missing.mp3"),
            &only(TagEdit {
                title: Change::Set("x".to_owned()),
                ..TagEdit::default()
            }),
        )
        .expect_err("a file that is not there cannot be tagged");
        assert!(
            matches!(&error, TagError::Read { path, .. } if path == "/definitely/missing.mp3"),
            "expected a read error naming the file, got {error:?}"
        );
    }
}
