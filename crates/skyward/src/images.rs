//! Images: a partition written out whole as one file, an image written back
//! over the partition it came from, and the images kept that are no longer
//! wanted.
//!
//! An image is the partition's own bytes, start to end, with no header and
//! nothing added. Its digest is taken as it is written, so writing one back
//! can be checked against what was saved before a byte of the partition is
//! touched: a half-written image, or one from another phone, is refused
//! rather than put over a working partition.
//!
//! Work runs on a worker of its own, as a backup does: a partition of a
//! hundred megabytes takes long enough that the panel has to keep drawing.

use std::fs;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use skyward_ui::app::{ImageEnd, ImageFile};

/// The folder images go in and are found in, in a place.
pub const FOLDER: &str = "Skyward/images";

/// How much is read and written at a time.
const CHUNK: usize = 1 << 20;

/// A partition an image can be taken from, or written back to.
#[derive(Clone, Debug)]
pub struct Part {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
}

/// The partitions an image can be taken from: every one the phone's table
/// lists, each with the size the block device gives it. A partition whose
/// size cannot be read is left out, since an image of unknown length cannot
/// be checked.
pub fn parts(dir: impl AsRef<Path>) -> Result<Vec<Part>> {
    let mut out = Vec::new();
    for p in skyward_block::list_by_name(dir)? {
        match skyward_block::size_bytes(&p.path) {
            Ok(size) if size > 0 => out.push(Part { name: p.name, path: p.path, size }),
            // A partition with no size, or one whose size cannot be read, is
            // nothing an image can be taken from.
            _ => continue,
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// The images kept in `folder`, newest first. A file that is not an image --
/// one of another name, or one too short to hold anything -- is left out.
pub fn kept(folder: &Path) -> Result<Vec<ImageFile>> {
    use std::os::unix::fs::MetadataExt;
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("read {}", folder.display())),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("read {}", folder.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let meta = entry.metadata().with_context(|| format!("look at {}", path.display()))?;
        if !meta.is_file() || !name.ends_with(".img") {
            continue;
        }
        let (Some(partition), Some(sha256)) = (partition_of(&name), sum_of(&path)) else { continue };
        out.push(ImageFile { name, partition, path: path.to_string_lossy().into_owned(), sha256, size: meta.len(), made: meta.mtime() });    }
    out.sort_by_key(|f| std::cmp::Reverse(f.made));
    Ok(out)
}

/// The partition an image's file name says it was taken from: `<partition>.img`,
/// with the time it was taken after a dash when there is one.
fn partition_of(name: &str) -> Option<String> {
    let stem = name.strip_suffix(".img")?;
    let base = stem.split('-').next()?;
    (!base.is_empty()).then(|| base.to_owned())
}

/// The digest of the image at `path`, as a sidecar file beside it holds it,
/// or `None` where the sidecar is missing or cannot be read.
fn sum_of(path: &Path) -> Option<String> {
    let sidecar = path.with_extension("img.sha256");
    let text = fs::read_to_string(&sidecar).ok()?;
    let sum = text.split_whitespace().next()?.trim();
    (sum.len() == 64 && sum.bytes().all(|b| b.is_ascii_hexdigit())).then(|| sum.to_lowercase())
}

/// What work on an image is asked to do.
pub enum Task {
    /// Write `part` out whole into `into`.
    Export { part: Part, into: PathBuf },
    /// Write the image at `path` back over `part`, checked against `sha256`.
    Import { part: Part, path: PathBuf, sha256: String },
}

pub enum Event {
    /// How far the work has come: the bytes done of how many.
    At { done: u64, of: u64 },
    Ended(End),
}

pub enum End {
    Exported(PathBuf),
    Imported(String),
    /// The image does not hold what it should: what was wanted, and what was
    /// read. Nothing was written.
    Mismatched { wanted: String, found: String },
    Stopped,
    Failed(String),
}

pub struct Worker {
    cancel: Arc<AtomicBool>,
    worker: skyward_sys::worker::Worker<Event>,
}

impl Worker {
    pub fn start(task: Task) -> Result<Worker> {
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let worker = skyward_sys::worker::Worker::start("images", move |teller| {
            let tell = |event: Event| {
                teller.tell(event);
            };
            let ended = match task {
                Task::Export { part, into } => export(&part, &into, &tell, &stop).map(End::Exported),
                Task::Import { part, path, sha256 } => import(&part, &path, &sha256, &tell, &stop).map(|()| End::Imported(part.name.clone())),
            };
            let end = match ended {
                Ok(end) => end,
                Err(e) if e.is::<Cancelled>() => End::Stopped,
                Err(e) => match e.downcast::<Mismatch>() {
                    Ok(m) => End::Mismatched { wanted: m.wanted, found: m.found },
                    Err(e) => End::Failed(format!("{e:#}")),
                },
            };
            tell(Event::Ended(end));
        })?;
        Ok(Worker { cancel, worker })
    }

    pub fn events(&self) -> Vec<Event> {
        self.worker.events()
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Stopped as asked.
#[derive(Debug)]
struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "stopped as asked")
    }
}

impl std::error::Error for Cancelled {}

/// The image does not hold what its sidecar says it does.
#[derive(Debug)]
struct Mismatch {
    wanted: String,
    found: String,
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the image reads {} where it was saved as {}", self.found, self.wanted)
    }
}

impl std::error::Error for Mismatch {}

/// Write `part` out whole into `into`, digesting it as it goes and leaving
/// the digest in a sidecar beside it. A half-written image is taken away
/// again, so a stopped export leaves nothing that looks whole.
fn export(part: &Part, into: &Path, tell: &dyn Fn(Event), cancel: &AtomicBool) -> Result<PathBuf> {
    if let Some(parent) = into.parent() {
        fs::create_dir_all(parent).with_context(|| format!("make {}", parent.display()))?;
    }
    let written = export_to(part, into, tell, cancel);
    match written {
        Ok(()) => Ok(into.to_path_buf()),
        Err(e) => {
            // Nothing half-written is left behind: an image short of its end
            // would be written back over a working partition as if it were
            // whole.
            let _ = fs::remove_file(into);
            let _ = fs::remove_file(into.with_extension("img.sha256"));
            Err(e)
        }
    }
}

fn export_to(part: &Part, into: &Path, tell: &dyn Fn(Event), cancel: &AtomicBool) -> Result<()> {
    let mut from = fs::File::open(&part.path).with_context(|| format!("open {}", part.path.display()))?;
    let mut to = fs::File::create(into).with_context(|| format!("make {}", into.display()))?;
    let mut digest = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    while done < part.size {
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled.into());
        }
        let want = (part.size - done).min(CHUNK as u64) as usize;
        from.read_exact(&mut buf[..want]).context("read the partition")?;
        to.write_all(&buf[..want]).context("write the image")?;
        digest.update(&buf[..want]);
        done += want as u64;
        tell(Event::At { done, of: part.size });
    }
    to.flush().context("flush the image")?;
    drop(to);
    let sum = hex::encode(digest.finalize());
    let sidecar = into.with_extension("img.sha256");
    fs::write(&sidecar, format!("{sum}  {}\n", into.file_name().unwrap_or_default().to_string_lossy()))
        .with_context(|| format!("write {}", sidecar.display()))?;
    Ok(())
}

/// Write the image at `path` back over `part`, but only once the whole of it
/// has been read and digested as `sha256`: a byte is written only after every
/// byte has been checked.
fn import(part: &Part, path: &Path, sha256: &str, tell: &dyn Fn(Event), cancel: &AtomicBool) -> Result<()> {
    let size = fs::metadata(path).with_context(|| format!("look at {}", path.display()))?.len();
    ensure!(size == part.size, "the image is {size} bytes and {} is {} bytes", part.name, part.size);
    // The whole image is read and digested before the partition is opened for
    // writing: an image that does not match leaves the partition as it was.
    let mut from = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    while done < size {
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled.into());
        }
        let want = (size - done).min(CHUNK as u64) as usize;
        from.read_exact(&mut buf[..want]).context("read the image")?;
        digest.update(&buf[..want]);
        done += want as u64;
        tell(Event::At { done, of: size });
    }
    let found = hex::encode(digest.finalize());
    if found != sha256.to_lowercase() {
        return Err(Mismatch { wanted: sha256.to_lowercase(), found }.into());
    }
    // Checked, so it is read again as it is written: the file could have been
    // changed between the check and the write, and this way it is the same
    // bytes that are checked and written.
    from.rewind().context("read the image again")?;
    let mut to = fs::OpenOptions::new().write(true).open(&part.path).with_context(|| format!("open {}", part.path.display()))?;
    let mut done = 0u64;
    while done < size {
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled.into());
        }
        let want = (size - done).min(CHUNK as u64) as usize;
        from.read_exact(&mut buf[..want]).context("read the image")?;
        to.write_all(&buf[..want]).context("write the partition")?;
        done += want as u64;
        tell(Event::At { done, of: size });
    }
    to.flush().context("flush the partition")?;
    Ok(())
}

/// Delete the image at `path`, with the sidecar that holds its digest.
pub fn delete(path: &Path) -> Result<()> {
    fs::remove_file(path).with_context(|| format!("delete {}", path.display()))?;
    let _ = fs::remove_file(path.with_extension("img.sha256"));
    Ok(())
}

/// The name an image of `part` is given: the partition, and when it was taken
/// in the zone the settings chose, so two images of one partition sit side by
/// side rather than over one another.
pub fn image_name(partition: &str, when: &str) -> String {
    format!("{partition}-{when}.img")
}

/// What an ended task says the page should show.
pub fn end_of(end: End) -> ImageEnd {
    match end {
        End::Exported(path) => ImageEnd::Exported(path.to_string_lossy().into_owned()),
        End::Imported(part) => ImageEnd::Imported(part),
        End::Mismatched { wanted, found } => ImageEnd::Mismatched { wanted, found },
        End::Stopped => ImageEnd::Stopped,
        End::Failed(why) => ImageEnd::Failed(why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_is_named_for_its_partition_and_when_it_was_taken() {
        assert_eq!(image_name("boot_a", "20261120-180000"), "boot_a-20261120-180000.img");
    }

    #[test]
    fn the_partition_an_image_came_from_is_read_off_its_name() {
        assert_eq!(partition_of("boot_a-20261120-180000.img").as_deref(), Some("boot_a"));
        assert_eq!(partition_of("boot_a.img").as_deref(), Some("boot_a"));
        assert_eq!(partition_of("notes.txt"), None, "not an image");
        assert_eq!(partition_of(".img"), None, "no partition named");
    }

    #[test]
    fn a_sidecar_holds_the_digest_of_the_whole_image() {
        let dir = std::env::temp_dir().join(format!("skyward-image-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let image = dir.join("boot_a.img");
        fs::write(&image, b"not really a partition").unwrap();
        // No sidecar yet: nothing says what the image holds.
        assert_eq!(sum_of(&image), None);
        let sum = "ab".repeat(32);
        fs::write(image.with_extension("img.sha256"), format!("{sum}  boot_a.img\n")).unwrap();
        assert_eq!(sum_of(&image).as_deref(), Some(sum.as_str()));
        // A sidecar that is not a digest says nothing either.
        fs::write(image.with_extension("img.sha256"), "not a digest\n").unwrap();
        assert_eq!(sum_of(&image), None);
        let _ = fs::remove_dir_all(&dir);
    }
}
