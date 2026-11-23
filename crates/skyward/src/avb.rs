//! Reading and changing the running slot's top-level Android Verified Boot
//! metadata.

use crate::settings;
use anyhow::{Context, Result, bail};
use skyward_ui::app::AvbInfo;
use skyward_vab::avb::{Avb, Check};
use std::fs;
use std::path::{Path, PathBuf};

const BACKUP: &str = "/metadata/skyward/vbmeta.img";
const BEFORE_RESTORE: &str = "/metadata/skyward/vbmeta.before-restore.img";

/// Read the running slot's AVB state. A bootloader state or backup that
/// cannot be read leaves the AVB flags readable on the page.
pub fn read() -> Result<AvbInfo> {
    let avb = running()?;
    let unlocked = unlocked().unwrap_or_else(|e| {
        let _ = skyward_sys::kmsg(&format!("reading the bootloader state: {e:#}"));
        None
    });
    let backup = backup_exists().unwrap_or_else(|e| {
        let _ = skyward_sys::kmsg(&format!("looking for the vbmeta backup: {e:#}"));
        false
    });
    Ok(AvbInfo {
        verity: avb.enabled(Check::Verity),
        verification: avb.enabled(Check::Verification),
        rollback_index: avb.rollback_index,
        unlocked,
        backup,
    })
}

/// Turn one AVB check on or off, saving the current image first.
pub fn set(check: Check, on: bool) -> Result<()> {
    ensure_unlocked()?;
    with_writable_metadata(|| {
        let mut avb = running()?;
        avb.set(check, on, Path::new(BACKUP))
            .with_context(|| format!("change {} {}", check.name(), if on { "on" } else { "off" }))
    })
}

/// Save the running slot's top-level vbmeta image without changing it.
pub fn backup() -> Result<()> {
    ensure_unlocked()?;
    with_writable_metadata(|| running()?.backup(Path::new(BACKUP)))
}

/// Restore the saved image, keeping the image it replaces.
pub fn restore() -> Result<()> {
    ensure_unlocked()?;
    with_writable_metadata(|| running()?.restore(Path::new(BACKUP), Path::new(BEFORE_RESTORE)))
}

fn running() -> Result<Avb> {
    let slot = skyward_vab::current_slot()?.context("not an A/B phone")?;
    Avb::read(Path::new(skyward_block::BY_NAME), slot.suffix())
}

fn unlocked() -> Result<Option<bool>> {
    skyward_info::bootloader_unlocked()
}

fn ensure_unlocked() -> Result<()> {
    match unlocked()? {
        Some(true) => Ok(()),
        Some(false) => bail!("the bootloader is locked"),
        None => bail!("the bootloader's lock state is unknown"),
    }
}

fn backup_exists() -> Result<bool> {
    settings::with_metadata(|| Ok(fs::metadata(PathBuf::from(BACKUP)).is_ok()))
}

fn with_writable_metadata<T>(work: impl FnOnce() -> Result<T>) -> Result<T> {
    settings::with_metadata(|| {
        skyward_vab::metadata_writable(true)?;
        let done = work();
        let back = skyward_vab::metadata_writable(false);
        match (done, back) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(e), _) => Err(e),
            (Ok(_), Err(e)) => Err(e).context("make /metadata read-only again"),
        }
    })
}
