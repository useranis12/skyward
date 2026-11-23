//! The top-level Android Verified Boot metadata of a slot: its flags,
//! rollback index, and where a phone keeps it.
//!
//! A vbmeta image starts with a 256-byte header. Its flags are a
//! big-endian 32-bit word 120 bytes in; the low bits turn dm-verity and
//! verification off. Where the phone has no vbmeta partition, the image is
//! carried by the boot partition and located by the footer at its end.

use anyhow::{Context, Result, bail, ensure};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const HEADER_LEN: usize = 256;
const MAGIC: &[u8] = b"AVB0";
const FOOTER_LEN: usize = 64;
const FOOTER_MAGIC: &[u8] = b"AVBf";
const VBMETA_AT: usize = 20;
const FLAGS_AT: usize = 120;
const ROLLBACK_AT: usize = 112;

pub const VERITY_OFF: u32 = 1 << 0;
pub const VERIFICATION_OFF: u32 = 1 << 1;

/// What turning `check` on or off means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Verity,
    Verification,
}

impl Check {
    pub fn off(self) -> u32 {
        match self {
            Check::Verity => VERITY_OFF,
            Check::Verification => VERIFICATION_OFF,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Check::Verity => "verity",
            Check::Verification => "verification",
        }
    }
}

/// The top-level vbmeta image of the slot `suffix` and its flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Avb {
    pub path: PathBuf,
    pub offset: u64,
    pub len: u64,
    pub flags: u32,
    pub rollback_index: u64,
}

impl Avb {
    pub fn enabled(&self, check: Check) -> bool {
        self.flags & check.off() == 0
    }

    /// Read the top-level vbmeta image of `suffix` under `by_name`.
    pub fn read(by_name: &Path, suffix: &str) -> Result<Self> {
        let (path, offset) = locate(by_name, suffix)?;
        let mut file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        let header = read_header(&mut file, &path, offset)?;
        let auth = u64::from_be_bytes(header[12..20].try_into().expect("eight bytes"));
        let aux = u64::from_be_bytes(header[20..28].try_into().expect("eight bytes"));
        let flags = u32::from_be_bytes(
            header[FLAGS_AT..FLAGS_AT + 4]
                .try_into()
                .expect("four bytes"),
        );
        let rollback_index = u64::from_be_bytes(
            header[ROLLBACK_AT..ROLLBACK_AT + 8]
                .try_into()
                .expect("eight bytes"),
        );
        Ok(Self {
            path,
            offset,
            len: 256 + auth + aux,
            flags,
            rollback_index,
        })
    }

    /// Turn `check` on or off, after saving the current image and checking
    /// that the changed flags read back.
    pub fn set(&mut self, check: Check, on: bool, backup: &Path) -> Result<()> {
        ensure!(
            self.offset == 0,
            "{} carries vbmeta at offset {}, which this recovery will not rewrite",
            self.path.display(),
            self.offset
        );
        ensure!(
            self.len <= usize::MAX as u64,
            "{} is too large to back up",
            self.path.display()
        );
        save(self, backup).with_context(|| format!("back up {}", self.path.display()))?;
        let old = self.flags;
        let new = if on {
            old & !check.off()
        } else {
            old | check.off()
        };
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.path)
            .with_context(|| format!("open {}", self.path.display()))?;
        let mut header = read_header(&mut file, &self.path, self.offset)?;
        header[FLAGS_AT..FLAGS_AT + 4].copy_from_slice(&new.to_be_bytes());
        file.seek(SeekFrom::Start(self.offset))
            .and_then(|_| file.write_all(&header))
            .and_then(|()| file.sync_all())
            .with_context(|| format!("write {}", self.path.display()))?;
        let read = read_header(&mut file, &self.path, self.offset)?;
        let read_flags =
            u32::from_be_bytes(read[FLAGS_AT..FLAGS_AT + 4].try_into().expect("four bytes"));
        if read_flags != new {
            let mut restore = header;
            restore[FLAGS_AT..FLAGS_AT + 4].copy_from_slice(&old.to_be_bytes());
            let _ = file
                .seek(SeekFrom::Start(self.offset))
                .and_then(|_| file.write_all(&restore))
                .and_then(|()| file.sync_all());
            bail!(
                "{} did not keep the new {} flags; the old flags were written back",
                self.path.display(),
                check.name()
            );
        }
        self.flags = new;
        Ok(())
    }

    /// Save the whole image at `backup`, atomically enough for recovery:
    /// a damaged backup is never accepted as a restore source.
    pub fn backup(&self, backup: &Path) -> Result<()> {
        save(self, backup)
    }

    /// Restore a backed-up image, refusing one that is not a complete
    /// top-level vbmeta image or whose rollback index is older.
    pub fn restore(&mut self, backup: &Path, before: &Path) -> Result<()> {
        ensure!(
            self.offset == 0,
            "{} carries vbmeta at offset {}, which this recovery will not rewrite",
            self.path.display(),
            self.offset
        );
        let image = std::fs::read(backup).with_context(|| format!("read {}", backup.display()))?;
        ensure!(
            image.len() >= HEADER_LEN && image.starts_with(MAGIC),
            "{} is not a vbmeta image",
            backup.display()
        );
        let auth = u64::from_be_bytes(image[12..20].try_into().expect("eight bytes"));
        let aux = u64::from_be_bytes(image[20..28].try_into().expect("eight bytes"));
        let want = 256u64
            .checked_add(auth)
            .and_then(|n| n.checked_add(aux))
            .context("vbmeta lengths overflow")?;
        ensure!(
            want == image.len() as u64,
            "{} is {} bytes, but its header says {want}",
            backup.display(),
            image.len()
        );
        let rollback = u64::from_be_bytes(
            image[ROLLBACK_AT..ROLLBACK_AT + 8]
                .try_into()
                .expect("eight bytes"),
        );
        ensure!(
            rollback >= self.rollback_index,
            "{} has rollback index {rollback}, older than the phone's {}",
            backup.display(),
            self.rollback_index
        );
        save(self, before)
            .with_context(|| format!("back up {} before restoring", self.path.display()))?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.path)
            .with_context(|| format!("open {}", self.path.display()))?;
        file.seek(SeekFrom::Start(0))
            .and_then(|_| file.write_all(&image))
            .and_then(|()| file.sync_all())
            .with_context(|| format!("write {}", self.path.display()))?;
        let mut check = [0u8; HEADER_LEN];
        file.seek(SeekFrom::Start(0))
            .and_then(|_| file.read_exact(&mut check))?;
        ensure!(
            check == image[..HEADER_LEN],
            "{} did not keep the restored header",
            self.path.display()
        );
        self.flags = u32::from_be_bytes(
            check[FLAGS_AT..FLAGS_AT + 4]
                .try_into()
                .expect("four bytes"),
        );
        self.rollback_index = u64::from_be_bytes(
            check[ROLLBACK_AT..ROLLBACK_AT + 8]
                .try_into()
                .expect("eight bytes"),
        );
        Ok(())
    }
}

fn locate(by_name: &Path, suffix: &str) -> Result<(PathBuf, u64)> {
    let vbmeta = by_name.join(format!("vbmeta{suffix}"));
    if vbmeta.exists() {
        return Ok((vbmeta, 0));
    }
    let boot = by_name.join(format!("boot{suffix}"));
    let mut file = File::open(&boot).with_context(|| format!("open {}", boot.display()))?;
    let mut footer = [0u8; FOOTER_LEN];
    file.seek(SeekFrom::End(-(FOOTER_LEN as i64)))
        .and_then(|_| file.read_exact(&mut footer))
        .with_context(|| format!("read footer of {}", boot.display()))?;
    ensure!(
        footer.starts_with(FOOTER_MAGIC),
        "{} has no vbmeta footer",
        boot.display()
    );
    let at = u64::from_be_bytes(
        footer[VBMETA_AT..VBMETA_AT + 8]
            .try_into()
            .expect("eight bytes"),
    );
    Ok((boot, at))
}

fn read_header(file: &mut File, path: &Path, offset: u64) -> Result<[u8; HEADER_LEN]> {
    let mut header = [0; HEADER_LEN];
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.read_exact(&mut header))
        .with_context(|| format!("read header of {}", path.display()))?;
    ensure!(
        header.starts_with(MAGIC),
        "{} holds no vbmeta header",
        path.display()
    );
    Ok(header)
}

fn save(avb: &Avb, path: &Path) -> Result<()> {
    ensure!(
        avb.len >= HEADER_LEN as u64,
        "{} has an invalid vbmeta length",
        avb.path.display()
    );
    let mut source =
        File::open(&avb.path).with_context(|| format!("open {}", avb.path.display()))?;
    let mut image = vec![0u8; avb.len as usize];
    source
        .seek(SeekFrom::Start(avb.offset))
        .and_then(|_| source.read_exact(&mut image))
        .with_context(|| format!("read {}", avb.path.display()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let temporary = path.with_extension("new");
    let mut out =
        File::create(&temporary).with_context(|| format!("create {}", temporary.display()))?;
    out.write_all(&image)
        .and_then(|()| out.sync_all())
        .with_context(|| format!("write {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_are_the_low_bits_of_the_big_endian_word() {
        let mut avb = Avb {
            path: PathBuf::new(),
            offset: 0,
            len: 256,
            flags: VERITY_OFF,
            rollback_index: 7,
        };
        assert!(!avb.enabled(Check::Verity));
        assert!(avb.enabled(Check::Verification));
        avb.flags |= VERIFICATION_OFF;
        assert!(!avb.enabled(Check::Verification));
    }

    #[test]
    fn a_footer_pointing_at_a_header_is_found() {
        let dir = std::env::temp_dir().join(format!("skyward-avb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut boot = vec![0u8; 1024];
        boot[512..516].copy_from_slice(MAGIC);
        boot[512 + 12..512 + 20].copy_from_slice(&0u64.to_be_bytes());
        boot[512 + 20..512 + 28].copy_from_slice(&0u64.to_be_bytes());
        boot[512 + 112..512 + 120].copy_from_slice(&9u64.to_be_bytes());
        boot[1024 - FOOTER_LEN..1024 - FOOTER_LEN + 4].copy_from_slice(FOOTER_MAGIC);
        boot[1024 - FOOTER_LEN + VBMETA_AT..1024 - FOOTER_LEN + VBMETA_AT + 8]
            .copy_from_slice(&512u64.to_be_bytes());
        std::fs::write(dir.join("boot_a"), boot).unwrap();
        let avb = Avb::read(&dir, "_a").unwrap();
        assert_eq!(avb.offset, 512);
        assert_eq!(avb.rollback_index, 9);
        let _ = std::fs::remove_dir_all(dir);
    }
}
