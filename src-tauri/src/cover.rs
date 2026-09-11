//! Cover art for what is playing, found in the music itself.
//!
//! A cover belongs to an album rather than to a track, so every track of a
//! record asks for the same one and the webview fetches it once. The album
//! is named in the key, which is why the key is reversible: nothing stores
//! a mapping, and the protocol handler works out what was asked for from
//! the key alone.
//!
//! Two sources, in this order: a picture inside the file, then an image
//! file sitting beside the music. Finding one online is #31.

use std::path::{Path, PathBuf};

use lofty::file::TaggedFileExt;
use tauri::http::{header, HeaderValue, Response, StatusCode};

use crate::db::{Db, Result as DbResult};

/// The image files a cover is looked for under, in the order they win.
/// Lower case; the comparison folds case, because Windows and Linux
/// disagree about whether `Cover.jpg` and `cover.jpg` are the same file.
const COVER_NAMES: &[&str] = &["cover", "folder", "front", "album", "albumart"];

/// The extensions those files may carry, in the order they win when one
/// folder holds the same name twice. `cover.png` beats `cover.jpg`:
/// nothing in the file system decides which of the two is served, so this
/// has to, or the answer changes with the platform and the listing order.
const COVER_EXTENSIONS: &[&str] = &["png", "webp", "jpg", "jpeg"];

/// Separates the parts inside a key before they are hex encoded. A unit
/// separator cannot appear in a tag or a path, so decoding cannot be
/// fooled by an album called `A/B`.
const SEPARATOR: char = '\u{001F}';

/// A tag that actually says something, or nothing at all. A tag of only
/// spaces is the same as a missing one everywhere here.
fn named(tag: Option<&str>) -> Option<&str> {
    tag.map(str::trim).filter(|it| !it.is_empty())
}

/// A cover, ready to be served.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cover {
    pub bytes: Vec<u8>,
    pub mime: String,
}

/// What a cover request names.
///
/// Almost always an album. A track with no album tag is its own subject,
/// because there is nothing to group it with and it may still carry a
/// picture of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Album { artist: String, album: String },
    Track { path: String },
}

impl Key {
    /// The key for a track, from the tags the library holds.
    ///
    /// The album artist is preferred over the artist, so a record whose
    /// tracks name different performers is still one album with one cover.
    #[must_use]
    pub fn of(
        album_artist: Option<&str>,
        artist: Option<&str>,
        album: Option<&str>,
        path: &str,
    ) -> Self {
        // Each is trimmed before the choice, not after: an album artist of
        // only spaces is not an album artist, and must fall through to the
        // performer rather than win and then vanish.
        let by = named(album_artist).or_else(|| named(artist));
        let album = named(album);
        match album {
            Some(album) => Key::Album {
                artist: by.unwrap_or_default().to_owned(),
                album: album.to_owned(),
            },
            None => Key::Track {
                path: path.to_owned(),
            },
        }
    }

    /// The key as it travels in a URL: hex, so that an album named with a
    /// slash, a space or a question mark needs no escaping rules that the
    /// two sides could read differently.
    #[must_use]
    pub fn encode(&self) -> String {
        let plain = match self {
            Key::Album { artist, album } => format!("A{SEPARATOR}{artist}{SEPARATOR}{album}"),
            Key::Track { path } => format!("T{SEPARATOR}{path}"),
        };
        to_hex(plain.as_bytes())
    }

    /// Reads back what [`Self::encode`] wrote. `None` for anything else,
    /// which is what a request for a made-up URL looks like.
    #[must_use]
    pub fn decode(encoded: &str) -> Option<Self> {
        let plain = String::from_utf8(from_hex(encoded)?).ok()?;
        let mut parts = plain.split(SEPARATOR);
        match parts.next()? {
            "A" => {
                let artist = parts.next()?.to_owned();
                let album = parts.next()?.to_owned();
                // A third separator would mean the album name held one,
                // which nothing can produce.
                if parts.next().is_some() {
                    return None;
                }
                Some(Key::Album { artist, album })
            }
            "T" => {
                let path = parts.next()?.to_owned();
                if parts.next().is_some() {
                    return None;
                }
                Some(Key::Track { path })
            }
            _ => None,
        }
    }
}

/// How long the webview may keep a cover before asking again.
///
/// Long enough that playing a record fetches its cover once rather than
/// once a track, short enough that a cover which changed because the tags
/// were edited comes back within the hour. Editing tags is #7, and when it
/// lands it should say so here rather than wait this out.
const CACHE_CONTROL: &str = "max-age=3600";

/// What a picture is called when its own media type cannot be trusted.
const UNKNOWN_MEDIA_TYPE: &str = "application/octet-stream";

/// Answers a request on the cover protocol.
///
/// The path of the URL is the key, as [`Key::encode`] wrote it. Anything
/// that is not a key we issued, and any album with no cover, is a 404: the
/// `img` falls back to whatever the panel put behind it, which is the same
/// outcome and needs no special case in Elm.
///
/// `look_up` is given the key and answers which files might hold the
/// cover. It is separate so that the caller can hold the library lock for
/// that and let go of it before the files are read: opening a music file
/// or an image on a network share is slow, and the library is what every
/// other command and the scanner are waiting on.
#[must_use]
pub fn respond(
    path: &str,
    look_up: impl FnOnce(&Key) -> DbResult<Candidates>,
) -> Response<Vec<u8>> {
    let found = Key::decode(path.trim_start_matches('/'))
        .and_then(|key| {
            look_up(&key)
                .inspect_err(|err| log::warn!("cannot look up a cover: {err}"))
                .ok()
        })
        .and_then(|candidates| read_cover(&candidates));
    let Some(cover) = found else {
        let mut response = Response::new(Vec::new());
        *response.status_mut() = StatusCode::NOT_FOUND;
        return response;
    };
    // Built by hand rather than with the builder, which answers a
    // `Result` that would have to be unwrapped: there is no failure to
    // report here, only a media type that may need replacing.
    let mut response = Response::new(cover.bytes);
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(media_type(&cover.mime))
            .unwrap_or(HeaderValue::from_static(UNKNOWN_MEDIA_TYPE)),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(CACHE_CONTROL),
    );
    response
}

/// The media type to declare for a picture.
///
/// A picture's media type comes out of the file's own tag, so it is
/// whatever the person who tagged it put there. Anything that is not a
/// plain `type/subtype` of unremarkable characters is not passed on: a
/// value carrying a newline would let a tagged file write its own headers.
fn media_type(mime: &str) -> &str {
    let plausible = mime.len() < 100
        && mime.matches('/').count() == 1
        && !mime.starts_with('/')
        && !mime.ends_with('/')
        && mime
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/+-._".contains(c));
    if plausible {
        mime
    } else {
        UNKNOWN_MEDIA_TYPE
    }
}

/// Finds the cover a key names, or `None` when the music does not carry
/// one.
///
/// # Errors
///
/// Returns an error when the library cannot be read. A file that cannot be
/// opened or holds no picture is not an error: it is one source coming up
/// empty, and the next one is tried.
pub fn resolve(db: &Db, key: &Key) -> DbResult<Option<Cover>> {
    Ok(read_cover(&candidates(db, key)?))
}

/// The files a cover might be in. Both `None` means the album has nothing
/// to look in, which is one of the ways a cover is simply not there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Candidates {
    /// A file the library says holds a picture.
    pub with_picture: Option<String>,
    /// A file to look beside, for an image in the album's folder.
    pub beside: Option<String>,
}

/// Asks the library which files are worth opening for this cover.
///
/// This is the only part that needs the library, and it is a pair of
/// indexed lookups; the reading of the files it names is not, and is
/// deliberately left to [`read_cover`].
///
/// # Errors
///
/// Returns an error when the library cannot be read.
pub fn candidates(db: &Db, key: &Key) -> DbResult<Candidates> {
    match key {
        // The library already knows which files hold a picture, so the
        // one to open is found rather than guessed at.
        Key::Album { artist, album } => Ok(Candidates {
            with_picture: db.album_track_with_cover(artist, album)?,
            beside: db.album_track(artist, album)?,
        }),
        // The path is taken as given rather than checked against the
        // library, which means this arm will open a file the library does
        // not know. Considered and left as it is: only Elm builds these
        // URLs and the CSP says who may ask for them, so reaching it needs
        // a webview already under someone else's control.
        //
        // The check to add, when one is added, is not "is this file in the
        // library" but "is this a file we are handling". Those are the same
        // set only until a file can be played without being scanned first,
        // and at that point the set is the library plus the player's queue.
        // Narrowing it to the library now would leave that feature playing
        // music under a blank sleeve.
        Key::Track { path } => Ok(Candidates {
            with_picture: Some(path.clone()),
            beside: Some(path.clone()),
        }),
    }
}

/// Opens the files and answers with the first cover found, or `None` when
/// neither holds one. Touches no database, so the caller may have let the
/// library lock go before calling it.
#[must_use]
pub fn read_cover(candidates: &Candidates) -> Option<Cover> {
    candidates
        .with_picture
        .as_deref()
        .and_then(|path| embedded_cover(Path::new(path)))
        .or_else(|| {
            candidates
                .beside
                .as_deref()
                .and_then(|path| beside_the_music(Path::new(path)))
        })
}

/// The picture inside a file, if it holds one.
fn embedded_cover(path: &Path) -> Option<Cover> {
    let tagged = lofty::read_from_path(path).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let picture = tag.pictures().first()?;
    Some(Cover {
        bytes: picture.data().to_vec(),
        mime: picture.mime_type().map_or_else(
            || "application/octet-stream".to_owned(),
            ToString::to_string,
        ),
    })
}

/// An image file in the folder the music sits in.
fn beside_the_music(track: &Path) -> Option<Cover> {
    let folder = track.parent()?;
    let found = cover_file(folder)?;
    let bytes = std::fs::read(&found).ok()?;
    Some(Cover {
        mime: mime_of(&found).to_owned(),
        bytes,
    })
}

/// The image file in `folder` that looks most like a cover.
///
/// The folder is read once and matched against the names, rather than
/// asking the file system about every name and extension in turn: that is
/// twenty questions per album on a network share.
fn cover_file(folder: &Path) -> Option<PathBuf> {
    let mut best: Option<((usize, usize), PathBuf)> = None;
    for entry in std::fs::read_dir(folder).ok()?.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|it| it.to_str()) else {
            continue;
        };
        let Some(extension) = path.extension().and_then(|it| it.to_str()) else {
            continue;
        };
        let lower_extension = extension.to_ascii_lowercase();
        let Some(by_extension) = COVER_EXTENSIONS
            .iter()
            .position(|known| *known == lower_extension)
        else {
            continue;
        };
        let stem = stem.to_ascii_lowercase();
        let Some(by_name) = COVER_NAMES.iter().position(|name| *name == stem) else {
            continue;
        };
        // The name first, then the extension. Ranking both means a folder
        // holding `cover.png` and `cover.jpg` always serves the same one,
        // rather than whichever the file system happened to list first.
        let rank = (by_name, by_extension);
        if best.as_ref().is_none_or(|(found, _)| rank < *found) {
            best = Some((rank, path));
        }
    }
    best.map(|(_, path)| path)
}

/// What to tell the webview an image file is. Guessed from the extension:
/// the alternative is sniffing the first bytes, and the browser will do
/// that anyway if we are wrong.
fn mime_of(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|it| it.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => "image/jpeg",
    }
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
        out
    })
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use lofty::config::WriteOptions;
    use lofty::picture::{MimeType, Picture, PictureType};
    use lofty::prelude::{Accessor, TagExt};
    use lofty::tag::{Tag, TagType};

    use super::{resolve, Key};
    use crate::db::{Db, DbError, FileStamp, TrackRecord};
    use crate::tags::TrackTags;

    /// A one-pixel PNG, so a test can tell one picture from another by its
    /// bytes rather than by hoping.
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    // KEYS

    #[test]
    fn a_key_survives_the_trip_through_a_url() {
        // Every character that would need escaping if the key were not hex.
        for key in [
            Key::Album {
                artist: "AC/DC".to_owned(),
                album: "Back in Black?".to_owned(),
            },
            Key::Album {
                artist: "Sigur Rós".to_owned(),
                album: "( )".to_owned(),
            },
            Key::Album {
                artist: String::new(),
                album: "100% #1 Hits & More".to_owned(),
            },
            Key::Track {
                path: "/music/no album/a b.mp3".to_owned(),
            },
        ] {
            assert_eq!(Key::decode(&key.encode()), Some(key.clone()), "{key:?}");
        }
    }

    #[test]
    fn a_key_that_was_made_up_is_refused() {
        // What a request for an invented URL looks like. None of these may
        // be taken for a real album.
        for made_up in [
            "",
            "zz",
            "abc",
            "41",
            &super::to_hex("X\u{001F}what".as_bytes()),
        ] {
            assert_eq!(Key::decode(made_up), None, "{made_up}");
        }
    }

    #[test]
    fn every_track_of_one_album_asks_for_the_same_cover() {
        let first = Key::of(None, Some("Alpha"), Some("Citrus"), "/music/1.mp3");
        let second = Key::of(None, Some("Alpha"), Some("Citrus"), "/music/2.mp3");
        assert_eq!(first, second, "the track must not be part of the key");
        assert_ne!(
            first,
            Key::of(None, Some("Alpha"), Some("Other"), "/music/1.mp3"),
            "a different album is a different cover"
        );
    }

    #[test]
    fn a_record_whose_tracks_name_different_artists_is_still_one_album() {
        // The library never groups by "various artists", so the album
        // artist is what holds such a record together.
        let one = Key::of(
            Some("Various"),
            Some("Alpha"),
            Some("Citrus"),
            "/music/1.mp3",
        );
        let other = Key::of(
            Some("Various"),
            Some("Beta"),
            Some("Citrus"),
            "/music/2.mp3",
        );
        assert_eq!(one, other);
    }

    #[test]
    fn a_track_with_no_album_stands_on_its_own() {
        let key = Key::of(None, Some("Alpha"), None, "/music/loose.mp3");
        assert_eq!(
            key,
            Key::Track {
                path: "/music/loose.mp3".to_owned()
            },
            "with no album to group by, the track is the subject"
        );
        assert_eq!(
            key,
            Key::of(None, Some("Alpha"), Some("   "), "/music/loose.mp3"),
            "an album of only spaces is no album either"
        );
    }

    // RESOLUTION

    fn library(dir: &Path) -> Db {
        let db = Db::open_in_memory().expect("db");
        db.add_folder(dir.to_str().expect("utf-8")).expect("folder");
        db
    }

    fn add(db: &Db, path: &Path, album: &str, has_cover: bool) {
        let folder = db.list_folders().expect("folders")[0].id;
        db.upsert_tracks(&[TrackRecord {
            folder_id: folder,
            stamp: FileStamp {
                path: path.to_str().expect("utf-8").to_owned(),
                mtime: 1,
                size: 2,
            },
            tags: TrackTags {
                artist: Some("The Satsumas".to_owned()),
                album: Some(album.to_owned()),
                has_embedded_cover: has_cover,
                ..TrackTags::default()
            },
        }])
        .expect("upsert");
    }

    /// A copy of the silence fixture carrying a picture.
    fn track_with_picture(dir: &Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::copy(crate::tags::tests::FIXTURE, &path).expect("copy");
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_album("Citrus".into());
        tag.push_picture(
            Picture::unchecked(PNG.to_vec())
                .pic_type(PictureType::CoverFront)
                .mime_type(MimeType::Png)
                .build(),
        );
        tag.save_to_path(&path, WriteOptions::default())
            .expect("tags");
        path
    }

    fn track_without_picture(dir: &Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::copy(crate::tags::tests::FIXTURE, &path).expect("copy");
        path
    }

    fn citrus() -> Key {
        Key::Album {
            artist: "The Satsumas".to_owned(),
            album: "Citrus".to_owned(),
        }
    }

    #[test]
    fn the_picture_in_the_file_is_the_cover() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = library(dir.path());
        add(
            &db,
            &track_with_picture(dir.path(), "1.mp3"),
            "Citrus",
            true,
        );

        let cover = resolve(&db, &citrus()).expect("resolve").expect("a cover");
        assert_eq!(cover.bytes, PNG, "the bytes must come from the file");
        assert_eq!(cover.mime, "image/png");
    }

    #[test]
    fn an_image_beside_the_music_is_used_when_the_file_holds_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = library(dir.path());
        add(
            &db,
            &track_without_picture(dir.path(), "1.mp3"),
            "Citrus",
            false,
        );
        std::fs::write(dir.path().join("cover.png"), PNG).expect("write");

        let cover = resolve(&db, &citrus()).expect("resolve").expect("a cover");
        assert_eq!(cover.bytes, PNG);
        assert_eq!(cover.mime, "image/png");
    }

    #[test]
    fn the_file_wins_over_the_folder() {
        // Both sources present and different, so the order is what is
        // being tested rather than which bytes happen to come back.
        let dir = tempfile::tempdir().expect("tempdir");
        let db = library(dir.path());
        add(
            &db,
            &track_with_picture(dir.path(), "1.mp3"),
            "Citrus",
            true,
        );
        std::fs::write(dir.path().join("cover.png"), b"not the picture in the file")
            .expect("write");

        let cover = resolve(&db, &citrus()).expect("resolve").expect("a cover");
        assert_eq!(cover.bytes, PNG, "the embedded picture comes first");
    }

    #[test]
    fn the_album_is_searched_for_the_track_that_carries_the_picture() {
        // The first track of the album has none; a later one does. Opening
        // only the first would find nothing.
        let dir = tempfile::tempdir().expect("tempdir");
        let db = library(dir.path());
        add(
            &db,
            &track_without_picture(dir.path(), "1.mp3"),
            "Citrus",
            false,
        );
        add(
            &db,
            &track_with_picture(dir.path(), "2.mp3"),
            "Citrus",
            true,
        );

        let cover = resolve(&db, &citrus()).expect("resolve").expect("a cover");
        assert_eq!(cover.bytes, PNG);
    }

    #[test]
    fn an_album_with_no_picture_anywhere_has_no_cover() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = library(dir.path());
        add(
            &db,
            &track_without_picture(dir.path(), "1.mp3"),
            "Citrus",
            false,
        );
        // An image that is not a cover by name must not be picked up.
        std::fs::write(dir.path().join("band photo.png"), PNG).expect("write");

        assert_eq!(resolve(&db, &citrus()).expect("resolve"), None);
    }

    #[test]
    fn another_albums_cover_is_never_served() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = library(dir.path());
        add(
            &db,
            &track_with_picture(dir.path(), "1.mp3"),
            "Citrus",
            true,
        );

        let other = Key::Album {
            artist: "The Satsumas".to_owned(),
            album: "Lemon".to_owned(),
        };
        assert_eq!(
            resolve(&db, &other).expect("resolve"),
            None,
            "an album with no tracks must not fall back to another"
        );
    }

    #[test]
    fn a_key_for_a_track_that_is_gone_is_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = library(dir.path());
        let key = Key::Track {
            path: dir.path().join("never existed.mp3").display().to_string(),
        };
        assert_eq!(resolve(&db, &key).expect("resolve"), None);
    }

    #[test]
    fn cover_is_preferred_over_the_other_names() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = library(dir.path());
        add(
            &db,
            &track_without_picture(dir.path(), "1.mp3"),
            "Citrus",
            false,
        );
        std::fs::write(dir.path().join("folder.png"), b"second choice").expect("write");
        std::fs::write(dir.path().join("cover.png"), PNG).expect("write");

        let cover = resolve(&db, &citrus()).expect("resolve").expect("a cover");
        assert_eq!(
            cover.bytes, PNG,
            "cover beats folder whatever order the disk lists them in"
        );
    }

    #[test]
    fn one_name_in_two_formats_always_serves_the_same_one() {
        // Nothing in the file system decides this: `read_dir` answers in
        // whatever order the platform likes, and it is not the order the
        // files were made in. Without a rule the served cover changes
        // between machines, and can change between folders on one.
        for order in [["cover.png", "cover.jpg"], ["cover.jpg", "cover.png"]] {
            let dir = tempfile::tempdir().expect("tempdir");
            let db = library(dir.path());
            add(
                &db,
                &track_without_picture(dir.path(), "1.mp3"),
                "Citrus",
                false,
            );
            for name in order {
                let bytes = if name.ends_with("png") { PNG } else { b"jpeg" };
                std::fs::write(dir.path().join(name), bytes).expect("write");
            }

            let cover = resolve(&db, &citrus()).expect("resolve").expect("a cover");
            assert_eq!(
                cover.bytes, PNG,
                "png wins over jpg, whichever was written first ({order:?})"
            );
            assert_eq!(cover.mime, "image/png");
        }
    }

    // SERVING

    /// Answers a request the way the protocol handler does, with the
    /// library consulted only for the lookup.
    fn serve(db: &Db, path: &str) -> tauri::http::Response<Vec<u8>> {
        super::respond(path, |key| super::candidates(db, key))
    }

    #[test]
    fn a_cover_is_served_with_what_it_is_and_how_long_it_keeps() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = library(dir.path());
        add(
            &db,
            &track_with_picture(dir.path(), "1.mp3"),
            "Citrus",
            true,
        );

        let response = serve(&db, &format!("/{}", citrus().encode()));
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "image/png");
        assert!(
            response.headers()["cache-control"]
                .to_str()
                .expect("ascii")
                .starts_with("max-age="),
            "without this the webview asks again for every track of the record"
        );
        assert_eq!(response.body(), PNG);
    }

    #[test]
    fn an_album_with_no_cover_is_not_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = library(dir.path());
        add(
            &db,
            &track_without_picture(dir.path(), "1.mp3"),
            "Citrus",
            false,
        );

        let response = serve(&db, &format!("/{}", citrus().encode()));
        assert_eq!(response.status(), 404);
        assert!(response.body().is_empty());
    }

    #[test]
    fn a_url_that_was_never_issued_is_not_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = library(dir.path());
        add(
            &db,
            &track_with_picture(dir.path(), "1.mp3"),
            "Citrus",
            true,
        );

        // Including one that decodes to a real album whose key was built
        // by hand rather than issued, which must still work, and rubbish,
        // which must not reach the library at all.
        for path in ["/", "/zz", "/not hex", "/414243"] {
            let response = serve(&db, path);
            assert_eq!(response.status(), 404, "{path}");
        }
    }

    #[test]
    fn a_library_that_cannot_be_read_serves_nothing() {
        // The file genuinely holds a picture, and the request genuinely
        // names it, so the only thing standing between the two is the
        // failed lookup. Asserting the 404 alone would pass just as well
        // if the error were swallowed and the file served anyway.
        let dir = tempfile::tempdir().expect("tempdir");
        let db = library(dir.path());
        let track = track_with_picture(dir.path(), "1.mp3");
        add(&db, &track, "Citrus", true);
        let key = Key::Track {
            path: track.display().to_string(),
        };
        let path = format!("/{}", key.encode());
        assert_eq!(
            serve(&db, &path).status(),
            200,
            "the file must be one that would otherwise be served"
        );

        let response = super::respond(&path, |_| Err(DbError::SchemaTooNew(99)));
        assert_eq!(response.status(), 404);
        assert!(response.body().is_empty());
    }

    #[test]
    fn a_media_type_from_a_tag_cannot_write_its_own_headers() {
        // The media type comes out of the file, so it is whatever whoever
        // tagged it put there.
        assert_eq!(super::media_type("image/png"), "image/png");
        assert_eq!(super::media_type("image/svg+xml"), "image/svg+xml");
        for hostile in [
            "image/png\r\nX-Whatever: 1",
            "image/png\nSet-Cookie: a=b",
            "",
            "image",
            "/png",
            "image/",
            "image/png; charset=<script>",
        ] {
            assert_eq!(
                super::media_type(hostile),
                "application/octet-stream",
                "{hostile:?} must not be passed on"
            );
        }
    }
}
