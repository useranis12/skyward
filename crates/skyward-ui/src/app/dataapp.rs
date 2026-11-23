//! The apps the owner installed, kept in the data partition: a folder each
//! under `/data/app`, holding a set of APKs, with the time it was installed
//! recorded in `/data/system/packages.xml`.
//!
//! Ported from skyward-pudding v0.3.15, which added the page; the reading of
//! a name out of a compiled manifest and its resource table is its work. The
//! icon decoding it also carried is left out here: its own reader returns no
//! icon, saying that the shapes icons come in -- vector, adaptive, WebP --
//! are more than it reads, so half the apps would have one and half not.
//! Showing the same mark for all of them is what it settled on, and what
//! this keeps.

use std::path::Path;

/// One app on the phone, the newest installed first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataApp {
    /// The package name, which is the folder's name without the hash after it.
    pub package: String,
    /// The folder in `/data/app`, or in the system's own partitions.
    pub dir: String,
    /// The name the app gives itself, falling back to the package name.
    pub label: String,
    /// Its version, from the APK's `versionName`, empty where there is none.
    pub version: String,
    /// When it was installed, in Unix seconds; 0 where the record has none.
    pub installed: i64,
    /// When it was last updated, in Unix seconds; the same.
    pub updated: i64,
    /// How many bytes the set of APKs takes.
    pub size: u64,
    /// Installed in the system's own partitions rather than by the owner, so
    /// that taking it away is not offered.
    pub system: bool,
}

/// The install times `/data/system/packages.xml` records, by package name:
/// when it was first installed and when it was last updated.
pub fn install_times(xml: &str) -> std::collections::HashMap<String, (i64, i64)> {
    let mut out = std::collections::HashMap::new();
    for chunk in xml.split("<package ").skip(1) {
        let head = match chunk.find('>') {
            Some(end) => &chunk[..end],
            None => continue,
        };
        let field = |name: &str| -> Option<&str> {
            let at = head.find(&format!("{name}=\""))?;
            let rest = &head[at + name.len() + 2..];
            let end = rest.find('"')?;
            Some(&rest[..end])
        };
        let Some(package) = field("name") else { continue };
        let when = |name: &str| field(name).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
        let first = when("first-install-time");
        let last = when("last-update-time");
        out.insert(package.to_string(), (first, last));
    }
    out
}

/// The `label` of the `application` in an APK's compiled `AndroidManifest.xml`,
/// where it is written out as words; one written as `@string/…` is not read
/// here, and the caller falls back to the package name.
pub fn literal_label(manifest: &[u8]) -> Option<String> {
    match label_of(manifest)? {
        Label::Text(text) => Some(text),
        Label::Id(_) => None,
    }
}

/// The resource id the label names, where the manifest writes it as
/// `@string/…` rather than as words.
pub fn label_id(manifest: &[u8]) -> Option<u32> {
    match label_of(manifest)? {
        Label::Id(id) => Some(id),
        Label::Text(_) => None,
    }
}

/// What the manifest's `application` gives as its label: words, or a
/// resource to look the words up by.
pub enum Label {
    Text(String),
    Id(u32),
}

fn label_of(manifest: &[u8]) -> Option<Label> {
    // A compiled AXML file is a string pool followed by the document as
    // chunks that index into it, so the label is found by walking the
    // chunks rather than by looking at the bytes beside its name.
    const RES_STRING_POOL: u16 = 0x0001;
    const RES_XML_START_ELEMENT: u16 = 0x0102;
    const STRING_POOL_UTF8: u32 = 0x100;
    if u16_at(manifest, 0)? != 0x0003 {
        return None;
    }
    let mut at = u16_at(manifest, 2)? as usize;
    let mut strings: Vec<String> = Vec::new();
    while at + 8 <= manifest.len() {
        let kind = u16_at(manifest, at)?;
        let header = u16_at(manifest, at + 2)? as usize;
        let size = u32_at(manifest, at + 4)? as usize;
        if size < 8 || at + size > manifest.len() {
            break;
        }
        if kind == RES_STRING_POOL {
            let count = u32_at(manifest, at + 8)? as usize;
            let flags = u32_at(manifest, at + 16)?;
            let start = u32_at(manifest, at + 20)? as usize;
            let utf8 = flags & STRING_POOL_UTF8 != 0;
            let offsets = at + header;
            for i in 0..count {
                let off = u32_at(manifest, offsets + i * 4)? as usize;
                let mut p = at + start + off;
                let text = if utf8 {
                    // A byte length, then a length in UTF-16 units, then the
                    // bytes; the first length is skipped.
                    p += 2;
                    let mut bytes = Vec::new();
                    while let Some(&b) = manifest.get(p) {
                        if b == 0 {
                            break;
                        }
                        bytes.push(b);
                        p += 1;
                    }
                    String::from_utf8_lossy(&bytes).into_owned()
                } else {
                    let n = u16_at(manifest, p)? as usize;
                    p += 2;
                    let mut units = Vec::with_capacity(n);
                    for k in 0..n {
                        units.push(u16_at(manifest, p + k * 2)?);
                    }
                    String::from_utf16_lossy(&units)
                };
                strings.push(text);
            }
        } else if kind == RES_XML_START_ELEMENT {
            let ext = at + header;
            let name_at = u32_at(manifest, ext + 4)? as usize;
            let attr_start = u16_at(manifest, ext + 8)? as usize;
            let attr_size = u16_at(manifest, ext + 10)? as usize;
            let count = u16_at(manifest, ext + 12)? as usize;
            let name = strings.get(name_at).map(String::as_str).unwrap_or("");
            if name == "application" && attr_size >= 20 {
                for i in 0..count {
                    let a = ext + attr_start + i * attr_size;
                    let key = strings.get(u32_at(manifest, a + 4)? as usize)?.clone();
                    if key != "label" {
                        continue;
                    }
                    let raw = u32_at(manifest, a + 8)? as usize;
                    let kind = manifest.get(a + 15).copied().unwrap_or(0);
                    // 0x03 is a string, whose value is in the raw slot; 0x01
                    // is a reference into the resource table, whose id is in
                    // the data slot (the raw slot is 0xffffffff then).
                    let data = u32_at(manifest, a + 16)? as usize;
                    match kind {
                        0x03 => return strings.get(raw).filter(|s| !s.is_empty()).cloned().map(Label::Text),
                        0x01 => return Some(Label::Id(data as u32)),
                        _ => return None,
                    }
                }
            }
        }
        at += size;
    }
    None
}

/// The words a resource id stands for in `resources.arsc`: found by package,
/// then type, then entry. An app whose label is written `@string/app_name`
/// is only named by this step.
pub fn resolve_string(arsc: &[u8], id: u32) -> Option<String> {
    const RES_TABLE: u16 = 0x0002;
    const RES_TABLE_PACKAGE: u16 = 0x0200;
    const RES_TABLE_TYPE: u16 = 0x0201;
    const FLAG_COMPACT: u16 = 0x0008;
    if u16_at(arsc, 0)? != RES_TABLE {
        return None;
    }
    let table_size = u32_at(arsc, 4)? as usize;
    let table_header = u16_at(arsc, 2)? as usize;
    // The table's own string pool sits right after its header.
    let global = table_header;
    let mut at = table_header;
    while at + 8 <= arsc.len().min(table_size) {
        let kind = u16_at(arsc, at)?;
        let header = u16_at(arsc, at + 2)? as usize;
        let size = u32_at(arsc, at + 4)? as usize;
        if size < 8 || at + size > arsc.len() {
            return None;
        }
        if kind == RES_TABLE_PACKAGE && u32_at(arsc, at + 8)? == id >> 24 {
            let mut p = at + header;
            while p + 8 <= at + size {
                let k = u16_at(arsc, p)?;
                let h = u16_at(arsc, p + 2)? as usize;
                let sz = u32_at(arsc, p + 4)? as usize;
                if sz < 8 || p + sz > at + size {
                    break;
                }
                if k == RES_TABLE_TYPE {
                    let type_id = arsc.get(p + 8).copied().unwrap_or(0) as u32;
                    // One type has a chunk per configuration -- density,
                    // language, and so on -- and the first is often empty,
                    // so each is looked at in turn.
                    if type_id == (id >> 16) & 0xff {
                        let entry_count = u32_at(arsc, p + 12)? as usize;
                        let entries_start = u32_at(arsc, p + 16)? as usize;
                        let index = (id & 0xffff) as usize;
                        if index < entry_count {
                            let offset = u32_at(arsc, p + h + index * 4)? as usize;
                            if offset != 0xffff_ffff {
                                let e = p + entries_start + offset;
                                let flags = u16_at(arsc, e + 2)?;
                                let (data_kind, data) = if flags & FLAG_COMPACT != 0 {
                                    // A compact entry keeps the value in the
                                    // entry itself, after the key.
                                    (arsc.get(e + 7).copied().unwrap_or(0), u32_at(arsc, e + 8)?)
                                } else {
                                    let esize = u16_at(arsc, e)? as usize;
                                    (arsc.get(e + esize + 3).copied().unwrap_or(0), u32_at(arsc, e + esize + 4)?)
                                };
                                // 0x03 is an index into the string pool.
                                if data_kind == 0x03
                                    && let Some(text) = pool_at(arsc, global, data as usize)
                                    && !text.is_empty()
                                {
                                    return Some(text);
                                }
                            }
                        }
                    }
                }
                p += sz;
            }
        }
        at += size;
    }
    None
}

/// The package name inside a folder's name, `<package>-<hash>`.
pub fn package_of(dir: &str) -> String {
    // A package name is letters, digits and dots, so the first `-` begins
    // the hash. Cutting at the last one would take part of the hash for the
    // name: the hash is base64, and base64 has `-` in it.
    match dir.find('-') {
        Some(at) if at > 0 => dir[..at].to_string(),
        _ => dir.to_string(),
    }
}

/// How many bytes the set of APKs in `dir` takes.
pub fn size_of(dir: &Path) -> u64 {
    let mut total = 0;
    if let Ok(list) = std::fs::read_dir(dir) {
        for entry in list.flatten() {
            if let Ok(meta) = entry.metadata()
                && meta.is_file()
            {
                total += meta.len();
            }
        }
    }
    total
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// The `n`th string of the pool whose chunk starts at `at`.
fn pool_at(d: &[u8], at: usize, n: usize) -> Option<String> {
    const STRING_POOL_UTF8: u32 = 0x100;
    let count = u32_at(d, at + 8)? as usize;
    if n >= count {
        return None;
    }
    let flags = u32_at(d, at + 16)?;
    let start = u32_at(d, at + 20)? as usize;
    let header = u16_at(d, at + 2)? as usize;
    let off = u32_at(d, at + header + n * 4)? as usize;
    let mut p = at + start + off;
    if flags & STRING_POOL_UTF8 != 0 {
        p += 2;
        let mut bytes = Vec::new();
        while let Some(&b) = d.get(p) {
            if b == 0 {
                break;
            }
            bytes.push(b);
            p += 1;
        }
        Some(String::from_utf8_lossy(&bytes).into_owned())
    } else {
        let len = u16_at(d, p)? as usize;
        p += 2;
        let mut units = Vec::with_capacity(len);
        for k in 0..len {
            units.push(u16_at(d, p + k * 2)?);
        }
        Some(String::from_utf16_lossy(&units))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version='1.0' encoding='utf-8' standalone='yes' ?>
<packages>
<package name="com.example.one" codePath="/data/app/~~abc/com.example.one-xyz" first-install-time="1763000000000" last-update-time="1763100000000" />
<package name="com.example.two" codePath="/data/app/~~def/com.example.two-uvw" first-install-time="1762000000000" />
</packages>"#;

    #[test]
    fn the_install_times_are_read_by_package_name() {
        let times = install_times(XML);
        assert_eq!(times["com.example.one"], (1763000000000, 1763100000000));
        assert_eq!(times["com.example.two"], (1762000000000, 0), "an update never made reads as none");
    }

    #[test]
    fn a_folder_name_gives_up_its_package_name() {
        assert_eq!(package_of("com.example.one-xyz123"), "com.example.one");
        // The hash is base64 and carries `-` of its own, so the name ends at
        // the first one, not the last.
        assert_eq!(package_of("io.github.mangi.eta.beta-hWRNOoUANvr2r8ZCjq-nOg=="), "io.github.mangi.eta.beta");
        assert_eq!(package_of("nodash"), "nodash", "a folder with no hash keeps its whole name");
    }

    #[test]
    fn a_manifest_that_is_not_compiled_is_not_read() {
        assert!(literal_label(b"<manifest/>").is_none());
        assert!(label_id(b"<manifest/>").is_none());
        assert!(resolve_string(b"not a table", 0x7f010001).is_none());
    }

    #[test]
    fn a_resource_table_that_does_not_hold_the_id_gives_nothing() {
        // A table header and nothing else: the id's package is not in it.
        let mut arsc = Vec::new();
        arsc.extend_from_slice(&0x0002u16.to_le_bytes());
        arsc.extend_from_slice(&12u16.to_le_bytes());
        arsc.extend_from_slice(&12u32.to_le_bytes());
        assert!(resolve_string(&arsc, 0x7f010001).is_none());
    }
}
