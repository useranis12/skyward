//! Reading one value out of the settings file Android keeps for a user.
//!
//! The file is a compiled (binary) XML: a chunk header, a string pool, then the
//! document as chunks that index into that pool. Text is never stored beside the
//! key it belongs to, so a value cannot be found by looking at the bytes that
//! follow the key -- the pool is shared, and what sits after `device_name` is
//! whichever string happened to be added next, not its value. Reading it that
//! way is how a phone came to be called `com.xiaomi.bluetooth`.
//!
//! Both shapes are handled: the compiled one, which is what Android writes, and
//! plain text XML, which some versions keep instead.

/// The XML chunk that every compiled document starts with.
const RES_XML_TYPE: u16 = 0x0003;
/// The chunk holding every string the document uses.
const RES_STRING_POOL: u16 = 0x0001;
/// An element opening, with its attributes.
const RES_XML_START_ELEMENT: u16 = 0x0102;
/// An element closing.
const RES_XML_END_ELEMENT: u16 = 0x0103;
/// The text inside an element.
const RES_XML_CDATA: u16 = 0x0104;
/// Set in the pool's flags where its strings are UTF-8 rather than UTF-16.
const UTF8_FLAG: u32 = 0x100;

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// The value kept for `key` in `bytes`, whichever shape the file takes.
pub fn value(bytes: &[u8], key: &str) -> Option<String> {
    compiled(bytes, key).or_else(|| text(bytes, key))
}

/// The value read out of a compiled settings file, or `None` where `bytes` is
/// not one or does not hold the key.
fn compiled(bytes: &[u8], key: &str) -> Option<String> {
    if u16_at(bytes, 0)? != RES_XML_TYPE {
        return None;
    }
    let total = (u32_at(bytes, 4)? as usize).min(bytes.len());
    // The string pool is the first chunk after the document header.
    if u16_at(bytes, 8)? != RES_STRING_POOL {
        return None;
    }
    let pool_header = u16_at(bytes, 10)? as usize;
    let pool_size = u32_at(bytes, 12)? as usize;
    let count = u32_at(bytes, 16)? as usize;
    let flags = u32_at(bytes, 24)?;
    let strings_start = u32_at(bytes, 28)? as usize;
    let base = 8 + strings_start;
    let mut strings = Vec::with_capacity(count);
    for i in 0..count {
        let off = u32_at(bytes, 8 + pool_header + i * 4)? as usize;
        strings.push(pool_string(bytes, base + off, flags)?);
    }
    // Then the document, chunk by chunk, until the value's element is passed.
    let mut at = 8 + pool_size;
    let mut wanted = false;
    while at + 8 <= total {
        let kind = u16_at(bytes, at)?;
        let size = u32_at(bytes, at + 4)? as usize;
        if size < 8 {
            break;
        }
        match kind {
            RES_XML_START_ELEMENT => {
                let name = u32_at(bytes, at + 20)? as usize;
                if strings.get(name).is_some_and(|s| s == "string") {
                    let attr_start = u16_at(bytes, at + 24)? as usize;
                    let attr_size = u16_at(bytes, at + 26)? as usize;
                    let attr_count = u16_at(bytes, at + 28)? as usize;
                    for a in 0..attr_count {
                        // Attributes follow the chunk's line and comment: each
                        // is its namespace, name and raw value, then its type.
                        let off = at + 16 + attr_start + a * attr_size;
                        let a_name = u32_at(bytes, off + 4)? as usize;
                        let a_raw = u32_at(bytes, off + 8)? as usize;
                        if strings.get(a_name).is_some_and(|s| s == "name")
                            && strings.get(a_raw).is_some_and(|s| s == key)
                        {
                            wanted = true;
                        }
                    }
                }
            }
            // The text of the element just opened is the value wanted.
            RES_XML_CDATA if wanted => {
                return strings.get(u32_at(bytes, at + 16)? as usize).cloned();
            }
            RES_XML_END_ELEMENT => wanted = false,
            _ => {}
        }
        at += size;
    }
    None
}

/// The string `at` in the pool, in whichever encoding the pool declares.
fn pool_string(bytes: &[u8], at: usize, flags: u32) -> Option<String> {
    if flags & UTF8_FLAG != 0 {
        // A UTF-8 pool keeps the length in UTF-16 units, then in bytes.
        let (_, at) = pool_len(bytes, at)?;
        let (len, at) = pool_len(bytes, at)?;
        Some(std::str::from_utf8(bytes.get(at..at + len)?).ok()?.to_string())
    } else {
        let (len, at) = pool_len16(bytes, at)?;
        let raw = bytes.get(at..at + len * 2)?;
        let units: Vec<u16> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        String::from_utf16(&units).ok()
    }
}

/// One length in a UTF-8 pool: a byte, or two where the first has its high bit
/// set and the second carries the rest.
fn pool_len(bytes: &[u8], at: usize) -> Option<(usize, usize)> {
    let first = *bytes.get(at)?;
    if first & 0x80 != 0 {
        let second = *bytes.get(at + 1)?;
        Some((((first & 0x7f) as usize) << 8 | second as usize, at + 2))
    } else {
        Some((first as usize, at + 1))
    }
}

/// One length in a UTF-16 pool, which is the same scheme over two bytes.
fn pool_len16(bytes: &[u8], at: usize) -> Option<(usize, usize)> {
    let first = u16_at(bytes, at)?;
    if first & 0x8000 != 0 {
        let second = u16_at(bytes, at + 2)?;
        Some((((first & 0x7fff) as usize) << 16 | second as usize, at + 4))
    } else {
        Some((first as usize, at + 2))
    }
}

/// The value read out of a plain text settings file, for the versions that keep
/// one: the element whose `name` attribute is `key`, and the text inside it.
fn text(bytes: &[u8], key: &str) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    // The quotes are part of the needle, so `device_name_backup` is not taken
    // for `device_name`.
    let needle = format!("name=\"{key}\"");
    let after = text.get(text.find(&needle)? + needle.len()..)?;
    let body = after.get(after.find('>')? + 1..)?;
    let value = unescape(body.get(..body.find('<')?)?.trim());
    (!value.is_empty()).then_some(value)
}

/// `what` with the XML entities a text settings file writes put back, so a name
/// the owner gave the phone reads as they typed it.
fn unescape(what: &str) -> String {
    what.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A compiled document holding one `<string name="device_name">` entry,
    /// with `device_name` and its value kept apart in the pool -- the shape that
    /// made the old byte walk read the neighbouring string.
    fn compiled_settings(pool: &[&str], key: &str, value: &str) -> Vec<u8> {
        let idx = |s: &str| pool.iter().position(|p| *p == s).unwrap() as u32;
        // The string pool: its header, one offset per string, then the strings.
        let mut data = Vec::new();
        let mut offsets = Vec::new();
        for s in pool {
            offsets.push(data.len() as u32);
            data.push(s.chars().count() as u8);
            data.push(s.len() as u8);
            data.extend(s.as_bytes());
            data.push(0);
        }
        while data.len() % 4 != 0 {
            data.push(0);
        }
        let header = 28usize;
        let strings_start = header + 4 * pool.len();
        let mut pool_body = Vec::new();
        pool_body.extend((pool.len() as u32).to_le_bytes());
        pool_body.extend(0u32.to_le_bytes());
        pool_body.extend(UTF8_FLAG.to_le_bytes());
        pool_body.extend((strings_start as u32).to_le_bytes());
        pool_body.extend(0u32.to_le_bytes());
        for o in &offsets {
            pool_body.extend(o.to_le_bytes());
        }
        pool_body.extend(&data);
        // A chunk header carries the chunk's own size: its header and its body.
        let mut pool_full = Vec::new();
        pool_full.extend(RES_STRING_POOL.to_le_bytes());
        pool_full.extend((header as u16).to_le_bytes());
        pool_full.extend(((8 + pool_body.len()) as u32).to_le_bytes());
        pool_full.extend(&pool_body);

        let node = |kind: u16, header_size: u16, body: Vec<u8>| -> Vec<u8> {
            let mut out = Vec::new();
            out.extend(kind.to_le_bytes());
            out.extend(header_size.to_le_bytes());
            out.extend(((8 + 8 + body.len()) as u32).to_le_bytes());
            out.extend(1u32.to_le_bytes());
            out.extend(0xffff_ffffu32.to_le_bytes());
            out.extend(body);
            out
        };
        let start = |name: u32, attrs: &[(u32, u32, u32)]| -> Vec<u8> {
            let mut body = Vec::new();
            body.extend(0xffff_ffffu32.to_le_bytes());
            body.extend(name.to_le_bytes());
            body.extend(20u16.to_le_bytes());
            body.extend(20u16.to_le_bytes());
            body.extend((attrs.len() as u16).to_le_bytes());
            body.extend(0u16.to_le_bytes());
            body.extend(0u16.to_le_bytes());
            body.extend(0u16.to_le_bytes());
            for (a_name, a_raw, a_data) in attrs {
                body.extend(0xffff_ffffu32.to_le_bytes());
                body.extend(a_name.to_le_bytes());
                body.extend(a_raw.to_le_bytes());
                body.extend(8u16.to_le_bytes());
                body.extend([0u8, 0x03]);
                body.extend(a_data.to_le_bytes());
            }
            body
        };

        let mut body = Vec::new();
        body.extend(node(RES_XML_START_ELEMENT, 36, start(idx("map"), &[])));
        body.extend(node(
            RES_XML_START_ELEMENT,
            36,
            start(idx("string"), &[(idx("name"), idx(key), idx(key))]),
        ));
        body.extend(node(RES_XML_CDATA, 20, idx(value).to_le_bytes().to_vec()));
        body.extend(node(RES_XML_END_ELEMENT, 24, {
            let mut b = Vec::new();
            b.extend(0xffff_ffffu32.to_le_bytes());
            b.extend(idx("string").to_le_bytes());
            b
        }));
        body.extend(node(RES_XML_END_ELEMENT, 24, {
            let mut b = Vec::new();
            b.extend(0xffff_ffffu32.to_le_bytes());
            b.extend(idx("map").to_le_bytes());
            b
        }));

        let mut out = Vec::new();
        out.extend(RES_XML_TYPE.to_le_bytes());
        out.extend(8u16.to_le_bytes());
        out.extend(((8 + pool_full.len() + body.len()) as u32).to_le_bytes());
        out.extend(&pool_full);
        out.extend(&body);
        out
    }

    #[test]
    fn the_value_is_read_from_its_own_element() {
        let pool = ["map", "string", "name", "device_name", "Xiaomi 17"];
        let bytes = compiled_settings(&pool, "device_name", "Xiaomi 17");
        assert_eq!(value(&bytes, "device_name").as_deref(), Some("Xiaomi 17"));
    }

    #[test]
    fn a_value_kept_away_from_its_key_is_still_read() {
        // The pool holds the name before the key, and a package name right
        // after it: what the phone showed as `com.xiaomi.bluetooth`. The value
        // is found through the document, not by what follows the key.
        let pool = [
            "map",
            "string",
            "name",
            "Xiaomi 17",
            "some_setting",
            "device_name",
            "com.xiaomi.bluetooth",
        ];
        let bytes = compiled_settings(&pool, "device_name", "Xiaomi 17");
        assert_eq!(
            value(&bytes, "device_name").as_deref(),
            Some("Xiaomi 17"),
            "the value is the element's text, not the string after the key"
        );
    }

    #[test]
    fn a_key_the_file_does_not_hold_is_none() {
        let pool = ["map", "string", "name", "device_name", "Xiaomi 17"];
        let bytes = compiled_settings(&pool, "device_name", "Xiaomi 17");
        assert_eq!(value(&bytes, "bluetooth_name"), None);
    }

    #[test]
    fn a_plain_text_settings_file_is_read_too() {
        let text = br#"<?xml version='1.0' encoding='utf-8' standalone='yes' ?>
<map>
    <string name="bluetooth_name">Xiaomi Smart Band 9 56B0</string>
    <string name="device_name">Xiaomi 17</string>
</map>
"#;
        assert_eq!(value(text, "device_name").as_deref(), Some("Xiaomi 17"));
        assert_eq!(
            value(text, "bluetooth_name").as_deref(),
            Some("Xiaomi Smart Band 9 56B0")
        );
    }

    #[test]
    fn an_escaped_name_in_a_text_file_reads_as_it_was_typed() {
        let text = br#"<map><string name="device_name">Tom &amp; Jerry</string></map>"#;
        assert_eq!(value(text, "device_name").as_deref(), Some("Tom & Jerry"));
    }

    #[test]
    fn a_key_that_is_a_prefix_of_another_is_not_taken_for_it() {
        // `device_name_backup` holds the same letters; the quotes keep the two
        // apart, so the real one is read.
        let text = br#"<map><string name="device_name_backup">wrong</string><string name="device_name">right</string></map>"#;
        assert_eq!(value(text, "device_name").as_deref(), Some("right"));
    }

    #[test]
    fn an_empty_element_is_none() {
        let text = br#"<map><string name="device_name"></string></map>"#;
        assert_eq!(value(text, "device_name"), None);
    }

    #[test]
    fn a_file_of_another_shape_is_none() {
        assert_eq!(value(b"nothing here", "device_name"), None);
        assert_eq!(value(&[], "device_name"), None);
    }
}
