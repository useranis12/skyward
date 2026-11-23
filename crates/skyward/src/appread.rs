//! The apps on the phone read on a thread of their own, so that the page is
//! drawn and turned while they come: walking a hundred and eighty folders and
//! reading a name out of each one's compiled manifest takes long enough to be
//! seen.
//!
//! Ported from skyward-pudding v0.3.15.

use anyhow::{Context, Result};
use skyward_ui::app::dataapp::{self as app, DataApp};
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};

/// Where the owner's own apps are kept, and where the record of when each was
/// installed is.
const DATA_APPS: &str = "/data/app";
const PACKAGES_XML: &str = "/data/system/packages.xml";

/// The system's own partitions, as the recovery mounts them. Each entry is an
/// `app` or `priv-app` folder where the phone has them.
const SYSTEM_ROOTS: [&str; 8] = [
    "/mnt/system/system/app",
    "/mnt/system/system/priv-app",
    "/mnt/product/app",
    "/mnt/product/priv-app",
    "/mnt/system_ext/app",
    "/mnt/system_ext/priv-app",
    "/vendor/app",
    "/odm/app",
];

/// What the reading thread tells the page as it goes.
pub enum Event {
    /// The apps' folders, with each one's package name, size and times, before
    /// any name is read: this is what the list is drawn from, so that it comes
    /// up at once rather than after every name.
    Folders(Vec<DataApp>),
    /// What the system's own partitions hold, for the page that only looks.
    System(Vec<DataApp>),
    /// One app's name, by its place in the list.
    Name(usize, String),
}

/// Reading the apps on a thread of its own. Letting it go asks the reading to
/// stop at the next app, and waits for it to.
pub struct Reader {
    reader: skyward_sys::worker::Worker<Event>,
    /// The reading watches this end. Letting it go is how the reading is told
    /// to stop, so it is let go before the thread is waited for.
    stop: Option<Sender<()>>,
}

impl Reader {
    /// Read the apps in the data partition, telling them as they are read.
    pub fn start_data() -> Result<Reader> {
        let (stop, stopped) = mpsc::channel::<()>();
        let reader = skyward_sys::worker::Worker::start("data apps", move |tell| {
            let apps = folders();
            let total = apps.len();
            tell.tell(Event::Folders(apps.clone()));
            let mut read = 0;
            for (at, a) in apps.iter().enumerate() {
                // The page has gone, and with it the end this watches: nothing
                // more is read.
                if matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Disconnected)) {
                    return;
                }
                if let Some(label) = name_of(Path::new(&a.dir), &a.package) {
                    tell.tell(Event::Name(at, label));
                    read += 1;
                }
            }
            let _ = skyward_sys::kmsg(&format!("data apps: {read} of {total} names read"));
        })?;
        Ok(Reader { reader, stop: Some(stop) })
    }

    /// Read what the system's own partitions hold.
    pub fn start_system() -> Result<Reader> {
        let reader = skyward_sys::worker::Worker::start("system apps", move |tell| {
            tell.tell(Event::System(system_apps()));
        })?;
        Ok(Reader { reader, stop: None })
    }

    /// Readable while events wait.
    pub fn fd(&self) -> RawFd {
        self.reader.fd()
    }

    /// The events since the last call.
    pub fn events(&self) -> Vec<Event> {
        self.reader.events()
    }

    /// Whether the reading has ended.
    pub fn ended(&self) -> bool {
        self.reader.ended()
    }
}

impl Drop for Reader {
    /// The reading is told to stop before the thread is waited for: letting
    /// the worker go joins it, and it would otherwise read every name still
    /// left on a page nobody is looking at.
    fn drop(&mut self) {
        let _ = self.stop.take();
    }
}

/// The apps the owner installed, newest first, with each one's package name,
/// how much room it takes and when it came.
///
/// The name is left as the package name here and read afterwards: walking the
/// folders is a few milliseconds, and reading every name is not.
fn folders() -> Vec<DataApp> {
    let times = std::fs::read_to_string(PACKAGES_XML).map(|text| app::install_times(&text)).unwrap_or_default();
    let Ok(top) = std::fs::read_dir(DATA_APPS) else { return Vec::new() };
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in top.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // A folder whose name begins `~~` holds the ones that were replaced
        // rather than removed; what is in it is what the phone runs.
        if entry.file_name().to_string_lossy().starts_with("~~") {
            let inner = std::fs::read_dir(&path).into_iter().flatten().flatten();
            dirs.extend(inner.map(|e| e.path()).filter(|p| p.is_dir()));
        } else {
            dirs.push(path);
        }
    }
    let mut out = Vec::new();
    for path in dirs {
        let Some(apk) = apk_in(&path) else { continue };
        let dir_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let package = app::package_of(&dir_name);
        // A package the record does not name -- installed by hand, or with its
        // record cleared -- falls back on when the APK was last written, which
        // is when it was installed or updated. The record keeps milliseconds
        // and a file's time is in seconds, which is what the page shows.
        let (installed, updated) = match times.get(&package).copied() {
            Some((first, last)) if first > 0 => (first / 1000, last / 1000),
            _ => {
                let when = std::fs::metadata(&apk)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                (when, when)
            }
        };
        out.push(DataApp {
            label: package.clone(),
            package,
            dir: path.to_string_lossy().into_owned(),
            version: String::new(),
            installed,
            updated,
            size: app::size_of(&path),
            system: false,
        });
    }
    out.sort_by(|a, b| b.installed.cmp(&a.installed).then_with(|| a.package.cmp(&b.package)));
    out
}

/// The apps the system's own partitions hold.
fn system_apps() -> Vec<DataApp> {
    let mut out = Vec::new();
    for root in SYSTEM_ROOTS {
        let Ok(list) = std::fs::read_dir(root) else { continue };
        for entry in list.flatten() {
            let path = entry.path();
            // An app sits either in a folder of its own or as a bare APK.
            let (dir, apk) = if path.is_dir() {
                (path.clone(), apk_in(&path))
            } else if path.extension().is_some_and(|e| e == "apk") {
                (path.parent().map(Path::to_path_buf).unwrap_or_else(|| path.clone()), Some(path.clone()))
            } else {
                continue;
            };
            let Some(apk) = apk else { continue };
            let stem = apk.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            // `base.apk` says nothing of the package; the folder does.
            let package = if stem.eq_ignore_ascii_case("base") {
                dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            } else {
                stem
            };
            if package.is_empty() {
                continue;
            }
            out.push(DataApp {
                label: package.clone(),
                package,
                dir: dir.to_string_lossy().into_owned(),
                version: String::new(),
                installed: 0,
                updated: 0,
                size: 0,
                system: true,
            });
        }
    }
    out.sort_by(|a, b| a.package.cmp(&b.package));
    out
}

/// The name an app gives itself, read from its compiled manifest and, where
/// that names a resource, from its resource table.
///
/// An app whose name cannot be read keeps the package name it was listed
/// under, so that nothing is said rather than something wrong.
fn name_of(dir: &Path, package: &str) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(apk_in(dir)?).ok()?;
    let mut zip = zip::ZipArchive::new(file).ok()?;
    let mut manifest = Vec::new();
    zip.by_name("AndroidManifest.xml").ok()?.read_to_end(&mut manifest).ok()?;
    let mut label = app::literal_label(&manifest);
    if label.is_none()
        && let Some(id) = app::label_id(&manifest)
    {
        let mut arsc = Vec::new();
        if let Ok(mut entry) = zip.by_name("resources.arsc") {
            let _ = entry.read_to_end(&mut arsc);
        }
        label = app::resolve_string(&arsc, id);
    }
    label.filter(|text| !text.trim().is_empty()).or_else(|| Some(package.to_string()))
}

/// The APK in a folder: `base.apk` where it is there, else the first one.
fn apk_in(dir: &Path) -> Option<PathBuf> {
    let base = dir.join("base.apk");
    if base.is_file() {
        return Some(base);
    }
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "apk"))
}

/// Take an app off: its folder in the data partition, and what it kept in
/// `/data/data`.
///
/// An app in the system's own partitions is refused, and nothing is touched:
/// taking one of them away keeps the system from starting.
pub fn remove(package: &str, dir: &str) -> Result<()> {
    if !dir.starts_with(DATA_APPS) {
        return Err(anyhow::anyhow!("{package} is in the system's partitions and is not removed here"));
    }
    for at in [dir.to_string(), format!("/data/data/{package}")] {
        match std::fs::remove_dir_all(&at) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("remove {at}")),
        }
    }
    Ok(())
}
