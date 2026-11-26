//! Images on the File screen and in the diffs (D268): which files are images, which two
//! versions a diff compares, what is known of each image, and the thread that decodes them off
//! the drawing. Drawing them is `kitty`'s; this knows nothing of the terminal.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::sync::mpsc::{self, Receiver, Sender};

use gm_schema::ImageAt;
use image::{ImageFormat, ImageReader};

use crate::data::{Group, Repo};

/// An image: its path and where it is read from.
pub type ImageKey = (String, ImageAt);

/// The longest side, in pixels, of an image sent to the terminal; a larger one is shrunk
/// first, since a terminal cell grid never shows more.
pub const MAX_SIDE: u32 = 2048;

/// The extensions of the formats shown (D268).
const EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// Whether `path` names an image the companion shows.
pub fn is_image(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, ext)| {
        EXTENSIONS
            .iter()
            .any(|known| ext.eq_ignore_ascii_case(known))
    })
}

/// What is known of an image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Picture {
    Reading,
    /// There is no such file there: an added image has no before, a deleted one no after.
    Missing,
    /// Too large to read; its size in bytes.
    TooLarge(u64),
    Failed(String),
    /// Read, and being decoded.
    Decoding {
        hash: u64,
    },
    Ready(Ready),
}

/// An image decoded, and how far it reached the terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ready {
    /// The id it has in the terminal.
    pub id: u32,
    pub format: &'static str,
    pub pixels: (u32, u32),
    pub bytes: u64,
    pub hash: u64,
    /// The PNG to send, until it is sent; `None` when images are not drawn.
    pub png: Option<Vec<u8>>,
    /// The cells it was placed over, once sent.
    pub placed: Option<(u16, u16)>,
}

/// The two images a diff of `path` in `group` compares, before and after; `old_path` names the
/// before of a rename. `None` for a side the group does not have, or while what it compares is
/// not known yet.
pub fn pair(
    repo: &Repo,
    group: Group,
    path: &str,
    old_path: Option<&str>,
) -> (Option<ImageKey>, Option<ImageKey>) {
    let commit = |rev: String| ImageAt::Commit { rev };
    let (before, after) = match group {
        Group::Untracked => (None, Some(ImageAt::Worktree)),
        Group::Unstaged => (Some(ImageAt::Index), Some(ImageAt::Worktree)),
        Group::Staged => (Some(commit("HEAD".to_owned())), Some(ImageAt::Index)),
        Group::Commit => match repo.commit.as_ref() {
            Some(detail) => (
                detail.parents.first().cloned().map(commit),
                Some(commit(detail.oid.clone())),
            ),
            None => (None, None),
        },
        Group::Branch => match repo.range.as_ref() {
            Some(range) => (
                Some(commit(range.from_oid.clone())),
                Some(commit(range.to_oid.clone())),
            ),
            None => (None, None),
        },
    };
    (
        before.map(|at| (old_path.unwrap_or(path).to_owned(), at)),
        after.map(|at| (path.to_owned(), at)),
    )
}

/// A fingerprint of `bytes`, to tell a changed image from the same one read again.
pub fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// An image decoded on the thread: its format, size and, when it is to be drawn, a PNG of at
/// most `MAX_SIDE` pixels a side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub format: &'static str,
    pub pixels: (u32, u32),
    pub bytes: u64,
    pub png: Option<Vec<u8>>,
}

/// Decodes `bytes`: only its header when `draw` is off; else the PNG to send, the file itself
/// when it is a PNG small enough, else the image decoded, shrunk and encoded again.
pub fn decode(bytes: &[u8], draw: bool) -> Result<Decoded, String> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    let Some(format) = reader.format() else {
        return Err("Not an image this can show.".to_owned());
    };
    let name = match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Gif => "GIF",
        ImageFormat::WebP => "WebP",
        ImageFormat::Bmp => "BMP",
        _ => return Err("Not an image this can show.".to_owned()),
    };
    let size = bytes.len() as u64;
    if !draw {
        let pixels = reader
            .into_dimensions()
            .map_err(|error| error.to_string())?;
        return Ok(Decoded {
            format: name,
            pixels,
            bytes: size,
            png: None,
        });
    }
    let image = reader.decode().map_err(|error| error.to_string())?;
    let pixels = (image.width(), image.height());
    let png = if format == ImageFormat::Png && pixels.0.max(pixels.1) <= MAX_SIDE {
        bytes.to_vec()
    } else {
        let image = if pixels.0.max(pixels.1) > MAX_SIDE {
            image.thumbnail(MAX_SIDE, MAX_SIDE)
        } else {
            image
        };
        let mut out = Cursor::new(Vec::new());
        image
            .write_to(&mut out, ImageFormat::Png)
            .map_err(|error| error.to_string())?;
        out.into_inner()
    };
    Ok(Decoded {
        format: name,
        pixels,
        bytes: size,
        png: Some(png),
    })
}

/// A job for the decoding thread: which image, its bytes, its fingerprint, and whether it is
/// drawn.
pub type DecodeJob = (ImageKey, Vec<u8>, u64, bool);

/// What the thread gives back for a job.
pub type DecodeDone = (ImageKey, u64, Result<Decoded, String>);

/// The thread that decodes images, so a large one never holds the screen.
pub struct Decoder {
    jobs: Sender<DecodeJob>,
    pub done: Receiver<DecodeDone>,
}

impl Decoder {
    /// Starts the thread. It ends when the `Decoder` is dropped.
    pub fn start() -> std::io::Result<Self> {
        let (jobs, inbox) = mpsc::channel::<DecodeJob>();
        let (out, done) = mpsc::channel();
        std::thread::Builder::new()
            .name("gm-tui-images".into())
            .spawn(move || {
                for (key, bytes, hash, draw) in inbox {
                    if out.send((key, hash, decode(&bytes, draw))).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self { jobs, done })
    }

    pub fn decode(&self, job: DecodeJob) {
        let _ = self.jobs.send(job);
    }
}

impl crate::ui::App {
    /// Reads `key` again; what is known of it shows until the answer comes.
    pub(crate) fn read_image(&mut self, key: ImageKey) {
        self.repo
            .images
            .entry(key.clone())
            .or_insert(Picture::Reading);
        let (path, at) = key;
        self.push_command(gm_schema::CommandKind::ReadImage { path, at });
    }

    /// The engine's answer for `key`: bytes go to the decoder unless they are the image already
    /// decoded.
    pub(crate) fn image_read(&mut self, key: ImageKey, image: &gm_schema::ImageBytes) {
        let picture = match image {
            gm_schema::ImageBytes::Missing => Picture::Missing,
            gm_schema::ImageBytes::TooLarge { size } => Picture::TooLarge(*size),
            gm_schema::ImageBytes::Bytes { data } => {
                let hash = fingerprint(data);
                match self.repo.images.get(&key) {
                    Some(Picture::Ready(ready)) if ready.hash == hash => return,
                    Some(Picture::Decoding { hash: was }) if *was == hash => return,
                    _ => {}
                }
                self.decode_jobs
                    .push((key.clone(), data.clone(), hash, self.graphics.is_some()));
                // The image on screen stays until the new one is decoded.
                if matches!(self.repo.images.get(&key), Some(Picture::Ready(_))) {
                    return;
                }
                Picture::Decoding { hash }
            }
        };
        self.replace_image(key, picture);
    }

    /// The decoder's answer for `key`, read with fingerprint `hash`.
    pub fn decoded(&mut self, key: ImageKey, hash: u64, result: Result<Decoded, String>) {
        let picture = match result {
            Ok(decoded) => {
                self.image_seq += 1;
                Picture::Ready(Ready {
                    id: crate::kitty::image_id(std::process::id(), self.image_seq),
                    format: decoded.format,
                    pixels: decoded.pixels,
                    bytes: decoded.bytes,
                    hash,
                    png: decoded.png,
                    placed: None,
                })
            }
            Err(reason) => Picture::Failed(reason),
        };
        self.replace_image(key, picture);
        self.redraw();
    }

    /// Puts `picture` in place of what `key` held, deleting an image the terminal has.
    fn replace_image(&mut self, key: ImageKey, picture: Picture) {
        if let Some(Picture::Ready(old)) = self.repo.images.insert(key, picture)
            && old.placed.is_some()
            && let Some(support) = self.graphics
        {
            self.image_ids.retain(|id| *id != old.id);
            self.push_request(crate::ui::Request::Graphics(crate::kitty::delete(
                support, old.id,
            )));
        }
    }

    /// Image `key` is drawn over `cols` by `rows` cells: it is sent the first time, placed
    /// again when its size changed.
    pub(crate) fn show_image(&mut self, key: &ImageKey, cols: u16, rows: u16) {
        let Some(support) = self.graphics else {
            return;
        };
        let Some(Picture::Ready(ready)) = self.repo.images.get_mut(key) else {
            return;
        };
        let command = match (ready.placed, ready.png.take()) {
            (Some(size), _) if size == (cols, rows) => return,
            (Some(_), _) => crate::kitty::place(support, ready.id, cols, rows),
            (None, Some(png)) => crate::kitty::transmit(support, ready.id, &png, cols, rows),
            (None, None) => return,
        };
        ready.placed = Some((cols, rows));
        let id = ready.id;
        if !self.image_ids.contains(&id) {
            self.image_ids.push(id);
        }
        self.push_request(crate::ui::Request::Graphics(command));
    }

    pub fn take_decode_jobs(&mut self) -> Vec<DecodeJob> {
        std::mem::take(&mut self.decode_jobs)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A PNG of `width` by `height` pixels.
    pub(crate) fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([200, 30, 30, 255]));
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut out, ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn images_are_told_by_their_extension() {
        assert!(is_image("assets/logo.PNG"));
        assert!(is_image("a/b.jpeg"));
        assert!(!is_image("src/main.rs"));
        assert!(!is_image("png"));
        assert!(!is_image("icon.svg"));
    }

    #[test]
    fn a_small_png_is_sent_as_it_is_and_its_header_is_enough_when_not_drawn() {
        let bytes = png(4, 3);
        let drawn = decode(&bytes, true).unwrap();
        assert_eq!((drawn.format, drawn.pixels), ("PNG", (4, 3)));
        assert_eq!(drawn.png.as_deref(), Some(bytes.as_slice()));
        let named = decode(&bytes, false).unwrap();
        assert_eq!(named.png, None);
        assert_eq!(named.bytes, bytes.len() as u64);
    }

    #[test]
    fn a_large_image_is_shrunk_and_other_formats_become_png() {
        let bytes = png(MAX_SIDE * 2, 10);
        let decoded = decode(&bytes, true).unwrap();
        assert_eq!(decoded.pixels, (MAX_SIDE * 2, 10));
        let sent = decode(&decoded.png.unwrap(), false).unwrap();
        assert_eq!(sent.pixels.0, MAX_SIDE);
        let mut bmp = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::new(5, 5))
            .write_to(&mut bmp, ImageFormat::Bmp)
            .unwrap();
        let decoded = decode(bmp.get_ref(), true).unwrap();
        assert_eq!(decoded.format, "BMP");
        assert!(decoded.png.unwrap().starts_with(b"\x89PNG"));
    }

    #[test]
    fn what_is_not_an_image_says_so() {
        assert!(decode(b"plain text", true).is_err());
    }

    #[test]
    fn a_diff_compares_the_two_sides_of_its_group() {
        let repo = Repo::default();
        let at =
            |pair: (Option<ImageKey>, Option<ImageKey>)| (pair.0.map(|k| k.1), pair.1.map(|k| k.1));
        assert_eq!(
            at(pair(&repo, Group::Unstaged, "a.png", None)),
            (Some(ImageAt::Index), Some(ImageAt::Worktree))
        );
        assert_eq!(
            at(pair(&repo, Group::Staged, "a.png", None)),
            (
                Some(ImageAt::Commit {
                    rev: "HEAD".to_owned()
                }),
                Some(ImageAt::Index)
            )
        );
        assert_eq!(
            at(pair(&repo, Group::Untracked, "a.png", None)),
            (None, Some(ImageAt::Worktree))
        );
        // Not known yet: the commit and the branch are still being read.
        assert_eq!(pair(&repo, Group::Commit, "a.png", None), (None, None));
        // A rename reads its before under the old name.
        let (before, after) = pair(&repo, Group::Unstaged, "new.png", Some("old.png"));
        assert_eq!(before.map(|k| k.0).as_deref(), Some("old.png"));
        assert_eq!(after.map(|k| k.0).as_deref(), Some("new.png"));
    }
}
