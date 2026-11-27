//! The copy-through core every OOXML editor in this crate is built on.
//!
//! An OOXML file (docx/xlsx/pptx) is a zip of XML parts plus binary media.
//! The one rule that makes editing safe is: **rewrite only the parts you
//! actually target, and copy every other entry through byte-for-byte.**
//! Styles, numbering, themes, fonts, images, headers, embedded objects and
//! content types are then preserved by construction rather than by a
//! regenerator remembering to emit them — which is the failure mode that makes
//! "parse to text, build a new document" silently destroy real files.

use std::collections::{BTreeMap, HashSet};
use std::io::{Cursor, Read, Write};

use quick_xml::events::Event;
use quick_xml::{Reader, Writer};

use crate::error::DocumentError;

/// Per-entry decompressed ceiling. Mirrors `ironclaw_extractors`' budget: a zip
/// bomb must not be able to turn a small upload into unbounded memory.
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
/// Cumulative decompressed ceiling across all entries of one archive.
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
/// Upper bound on parts in one archive, so a pathological entry count cannot
/// stall the editor before any size budget bites.
const MAX_ENTRIES: usize = 4096;
const ZIP_LANGUAGE_ENCODING_FLAG: u16 = 1 << 11;

fn zip_entry_name_compat(flags: u16, raw_name: &[u8], zip_name: &str) -> String {
    if flags & ZIP_LANGUAGE_ENCODING_FLAG != 0 {
        zip_name.to_owned()
    } else {
        oem_cp::code_table_type::TableType::Complete(&oem_cp::code_table::DECODING_TABLE_CP437)
            .decode_string_lossy(raw_name)
    }
}

fn central_directory_entry_name_compat(flags: u16, raw_name: &[u8], extra: &[u8]) -> String {
    const UNICODE_PATH_EXTRA: u16 = 0x7075;

    let original_name_crc = crc32fast::hash(raw_name);
    let mut offset = 0_usize;
    let mut unicode_path = None;
    while let Some(header_end) = offset.checked_add(4).filter(|end| *end <= extra.len()) {
        let field_id = u16::from_le_bytes([extra[offset], extra[offset + 1]]);
        let field_len = usize::from(u16::from_le_bytes([extra[offset + 2], extra[offset + 3]]));
        let Some(field_end) = header_end.checked_add(field_len) else {
            break;
        };
        if field_end > extra.len() {
            break;
        }

        if field_id == UNICODE_PATH_EXTRA && field_len >= 5 {
            let field = &extra[header_end..field_end];
            let name_crc = u32::from_le_bytes([field[1], field[2], field[3], field[4]]);
            if name_crc == original_name_crc
                && let Ok(name) = std::str::from_utf8(&field[5..])
            {
                unicode_path = Some(name.to_owned());
            }
        }
        offset = field_end;
    }

    if let Some(name) = unicode_path {
        return name;
    }

    let zip_name = match std::str::from_utf8(raw_name) {
        Ok(name) => name.to_owned(),
        Err(_) => {
            oem_cp::code_table_type::TableType::Complete(&oem_cp::code_table::DECODING_TABLE_CP437)
                .decode_string_lossy(raw_name)
        }
    };
    zip_entry_name_compat(flags, raw_name, &zip_name)
}

fn reject_duplicate_central_directory_names(
    data: &[u8],
    central_directory_start: u64,
    archive_entry_count: usize,
    max_entries: usize,
) -> Result<(), DocumentError> {
    const CENTRAL_ENTRY: &[u8; 4] = b"PK\x01\x02";

    let mut cursor =
        usize::try_from(central_directory_start).map_err(|_| DocumentError::MalformedPart {
            part: "<archive>".to_string(),
            detail: "ZIP central directory offset is not addressable".to_string(),
        })?;
    if cursor > data.len() {
        return Err(DocumentError::MalformedPart {
            part: "<archive>".to_string(),
            detail: "ZIP central directory starts past the archive end".to_string(),
        });
    }

    let mut names = HashSet::with_capacity(archive_entry_count.min(max_entries));
    let mut raw_names = HashSet::with_capacity(archive_entry_count.min(max_entries));
    let mut parsed_entries = 0_usize;

    while data.get(cursor..cursor.saturating_add(CENTRAL_ENTRY.len())) == Some(CENTRAL_ENTRY) {
        let Some(header_end) = cursor.checked_add(46).filter(|end| *end <= data.len()) else {
            return Err(DocumentError::MalformedPart {
                part: "<archive>".to_string(),
                detail: "truncated ZIP central directory header".to_string(),
            });
        };

        parsed_entries =
            parsed_entries
                .checked_add(1)
                .ok_or_else(|| DocumentError::MalformedPart {
                    part: "<archive>".to_string(),
                    detail: "ZIP central directory entry count overflow".to_string(),
                })?;
        if parsed_entries > max_entries {
            return Err(DocumentError::PackageTooLarge {
                detail: format!("more than {max_entries} central directory entries"),
            });
        }

        let flags = u16::from_le_bytes([data[cursor + 8], data[cursor + 9]]);
        let name_len = usize::from(u16::from_le_bytes([data[cursor + 28], data[cursor + 29]]));
        let extra_len = usize::from(u16::from_le_bytes([data[cursor + 30], data[cursor + 31]]));
        let comment_len = usize::from(u16::from_le_bytes([data[cursor + 32], data[cursor + 33]]));
        let Some(name_end) = header_end.checked_add(name_len) else {
            return Err(DocumentError::MalformedPart {
                part: "<archive>".to_string(),
                detail: "ZIP central directory name length overflow".to_string(),
            });
        };
        let Some(extra_end) = name_end.checked_add(extra_len) else {
            return Err(DocumentError::MalformedPart {
                part: "<archive>".to_string(),
                detail: "ZIP central directory extra-field length overflow".to_string(),
            });
        };
        let Some(next) = extra_end
            .checked_add(comment_len)
            .filter(|end| *end <= data.len())
        else {
            return Err(DocumentError::MalformedPart {
                part: "<archive>".to_string(),
                detail: "truncated ZIP central directory entry".to_string(),
            });
        };
        let (Some(raw_name), Some(extra)) = (
            data.get(header_end..name_end),
            data.get(name_end..extra_end),
        ) else {
            return Err(DocumentError::MalformedPart {
                part: "<archive>".to_string(),
                detail: "truncated ZIP central directory name or extra field".to_string(),
            });
        };

        let name = central_directory_entry_name_compat(flags, raw_name, extra);
        if !raw_names.insert(raw_name) || !names.insert(name.clone()) {
            return Err(DocumentError::MalformedPart {
                part: name,
                detail: "duplicate zip entry name".to_string(),
            });
        }
        cursor = next;
    }

    if parsed_entries != archive_entry_count {
        let detail = if parsed_entries > archive_entry_count {
            "ZIP central directory entries collide after filename normalization"
        } else {
            "ZIP central directory entry count does not match the archive index"
        };
        return Err(DocumentError::MalformedPart {
            part: "<archive>".to_string(),
            detail: detail.to_string(),
        });
    }

    Ok(())
}

/// An OOXML package held in memory as ordered, named entries.
///
/// Order is preserved because `[Content_Types].xml` must remain the first
/// entry for some consumers. Untouched entry payloads remain byte-identical;
/// the ZIP container is deterministically recompressed on write.
pub(crate) struct OoxmlPackage {
    names: Vec<String>,
    entries: BTreeMap<String, Vec<u8>>,
}

impl OoxmlPackage {
    /// Read every entry of `data` into memory under the size budgets above.
    pub(crate) fn read(data: &[u8]) -> Result<Self, DocumentError> {
        Self::read_with_limits(data, MAX_ENTRY_BYTES, MAX_TOTAL_BYTES, MAX_ENTRIES)
    }

    fn read_with_limits(
        data: &[u8],
        max_entry_bytes: u64,
        max_total_bytes: u64,
        max_entries: usize,
    ) -> Result<Self, DocumentError> {
        let mut archive = zip::ZipArchive::new(Cursor::new(data)).map_err(|source| {
            DocumentError::OoxmlArchive {
                operation: "open archive",
                source,
            }
        })?;
        if archive.len() > max_entries {
            return Err(DocumentError::PackageTooLarge {
                detail: format!("{} entries exceeds the {max_entries} limit", archive.len()),
            });
        }
        reject_duplicate_central_directory_names(
            data,
            archive.central_directory_start(),
            archive.len(),
            max_entries,
        )?;

        let mut names = Vec::with_capacity(archive.len());
        let mut entries = BTreeMap::new();
        let mut seen_names = HashSet::with_capacity(archive.len());
        let mut total: u64 = 0;
        for index in 0..archive.len() {
            let mut file =
                archive
                    .by_index(index)
                    .map_err(|source| DocumentError::OoxmlArchive {
                        operation: "read archive entry",
                        source,
                    })?;
            let zip_name = file.name().map_err(|source| DocumentError::OoxmlArchive {
                operation: "read archive entry name",
                source,
            })?;
            let name =
                zip_entry_name_compat(file.flags().as_u16(), file.name_raw(), zip_name.as_ref());
            // Directory names participate too: duplicate path entries must
            // fail before the package skips directories or overwrites files.
            if !seen_names.insert(name.clone()) {
                return Err(DocumentError::MalformedPart {
                    part: name,
                    detail: "duplicate zip entry name".to_string(),
                });
            }
            if file.is_dir() {
                continue;
            }
            // The declared size is a hint a hostile archive can lie about, so
            // it is only a fast pre-check; `take` below enforces the real cap
            // against bytes actually read.
            if file.size() > max_entry_bytes {
                return Err(DocumentError::PackageTooLarge {
                    detail: format!("entry {name} declares {} bytes", file.size()),
                });
            }
            let mut bytes = Vec::new();
            let read = std::io::copy(&mut file.by_ref().take(max_entry_bytes + 1), &mut bytes)
                .map_err(|source| DocumentError::OoxmlIo {
                    operation: "decompress archive entry",
                    source,
                })?;
            if read > max_entry_bytes {
                return Err(DocumentError::PackageTooLarge {
                    detail: format!("entry {name} exceeds {max_entry_bytes} bytes"),
                });
            }
            total += read;
            if total > max_total_bytes {
                return Err(DocumentError::PackageTooLarge {
                    detail: format!("archive exceeds {max_total_bytes} decompressed bytes"),
                });
            }
            names.push(name.clone());
            entries.insert(name, bytes);
        }
        Ok(Self { names, entries })
    }

    pub(crate) fn part(&self, name: &str) -> Result<&[u8], DocumentError> {
        self.entries
            .get(name)
            .map(Vec::as_slice)
            .ok_or_else(|| DocumentError::MissingPart {
                part: name.to_string(),
            })
    }

    pub(crate) fn part_str(&self, name: &str) -> Result<String, DocumentError> {
        let bytes = self.part(name)?;
        String::from_utf8(bytes.to_vec()).map_err(|error| DocumentError::MalformedPart {
            part: name.to_string(),
            detail: format!("not valid UTF-8: {error}"),
        })
    }

    pub(crate) fn has_part(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Replace one part's bytes. The entry keeps its position in the archive.
    pub(crate) fn replace_part(&mut self, name: &str, bytes: Vec<u8>) -> Result<(), DocumentError> {
        if !self.entries.contains_key(name) {
            return Err(DocumentError::MissingPart {
                part: name.to_string(),
            });
        }
        self.entries.insert(name.to_string(), bytes);
        Ok(())
    }

    /// Add a new part at the end of the archive. Used by the pptx slide clone,
    /// which genuinely adds parts rather than rewriting existing ones.
    pub(crate) fn insert_part(&mut self, name: &str, bytes: Vec<u8>) {
        if !self.entries.contains_key(name) {
            self.names.push(name.to_string());
        }
        self.entries.insert(name.to_string(), bytes);
    }

    /// Names in archive order, for callers that need to scan a family of parts
    /// (`ppt/slides/slideN.xml`, `xl/worksheets/sheetN.xml`).
    pub(crate) fn names(&self) -> &[String] {
        &self.names
    }

    /// Serialize back to a zip.
    ///
    /// Deflate and archive order are both fixed, and no timestamps are written,
    /// so serializing the same in-memory package is deterministic. Untouched
    /// entry payloads remain byte-identical even though ZIP records are rebuilt.
    pub(crate) fn write(&self) -> Result<Vec<u8>, DocumentError> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut cursor);
            let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .last_modified_time(
                    zip::DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0)
                        .map_err(|error| DocumentError::Write(error.to_string()))?,
                );
            for name in &self.names {
                let Some(bytes) = self.entries.get(name) else {
                    continue;
                };
                zip.start_file(name, options)
                    .map_err(|source| DocumentError::OoxmlArchive {
                        operation: "start output entry",
                        source,
                    })?;
                zip.write_all(bytes)
                    .map_err(|source| DocumentError::OoxmlIo {
                        operation: "write output entry",
                        source,
                    })?;
            }
            zip.finish().map_err(|source| DocumentError::OoxmlArchive {
                operation: "finish output archive",
                source,
            })?;
        }
        Ok(cursor.into_inner())
    }
}

/// What [`transform_xml`] should do with the event it was handed.
pub(crate) enum EventAction<'a> {
    /// Write the event through unchanged. The overwhelmingly common case.
    Keep,
    /// Write these events instead of the original one. An empty slice deletes.
    Replace(Vec<Event<'a>>),
    /// Drop the original and write nothing.
    Drop,
}

/// Stream `xml` through `handle`, writing every event back out.
///
/// This is the whole reason the crate uses an event parser: an event the
/// handler does not claim is re-emitted exactly as it was read, so attribute
/// order, namespace prefixes, self-closing forms, and inter-element whitespace
/// all survive. Only claimed events change.
pub(crate) fn transform_xml<F>(xml: &str, mut handle: F) -> Result<String, DocumentError>
where
    F: for<'a> FnMut(&Event<'a>) -> Result<EventAction<'static>, DocumentError>,
{
    let mut reader = Reader::from_str(xml);
    let config = reader.config_mut();
    config.trim_text(false);
    config.check_end_names = true;

    let mut writer = Writer::new(Cursor::new(Vec::new()));
    loop {
        let event = reader.read_event().map_err(|source| DocumentError::Xml {
            part: "xml".to_string(),
            source,
        })?;
        if matches!(event, Event::Eof) {
            break;
        }
        match handle(&event)? {
            EventAction::Keep => {
                writer
                    .write_event(event)
                    .map_err(|source| DocumentError::OoxmlIo {
                        operation: "write transformed XML",
                        source,
                    })?;
            }
            EventAction::Replace(events) => {
                for replacement in events {
                    writer
                        .write_event(replacement)
                        .map_err(|source| DocumentError::OoxmlIo {
                            operation: "write transformed XML",
                            source,
                        })?;
                }
            }
            EventAction::Drop => {}
        }
    }
    String::from_utf8(writer.into_inner().into_inner()).map_err(|error| {
        DocumentError::Write(format!("transformed XML is not valid UTF-8: {error}"))
    })
}

/// The local name of an element, with any namespace prefix stripped.
///
/// OOXML producers vary the prefix they bind (`w:p` vs `w14:p` vs a default
/// namespace), so matching on the local name is what makes the editors work on
/// documents from more than one producer.
pub(crate) fn local_name(raw: &[u8]) -> &[u8] {
    match raw.iter().position(|byte| *byte == b':') {
        Some(index) => &raw[index + 1..],
        None => raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut cursor);
            let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
            for (name, bytes) in entries {
                zip.start_file(*name, options).unwrap();
                zip.write_all(bytes).unwrap();
            }
            zip.finish().unwrap();
        }
        cursor.into_inner()
    }

    fn replace_zip_entry_name(archive: &mut [u8], old_name: &[u8], new_name: &[u8]) -> usize {
        assert_eq!(old_name.len(), new_name.len());
        let mut replaced = 0;
        let mut offset = 0;
        while offset + old_name.len() <= archive.len() {
            if &archive[offset..offset + old_name.len()] == old_name {
                archive[offset..offset + old_name.len()].copy_from_slice(new_name);
                replaced += 1;
                offset += old_name.len();
            } else {
                offset += 1;
            }
        }
        replaced
    }

    fn set_utf8_flag_on_second_entry(archive: &mut [u8]) {
        for (signature, flags_offset) in [(&b"PK\x03\x04"[..], 6), (&b"PK\x01\x02"[..], 8)] {
            let headers: Vec<_> = archive
                .windows(signature.len())
                .enumerate()
                .filter_map(|(offset, bytes)| (bytes == signature).then_some(offset))
                .collect();
            let flags_start = headers[1] + flags_offset;
            let flags = u16::from_le_bytes([archive[flags_start], archive[flags_start + 1]]);
            archive[flags_start..flags_start + 2]
                .copy_from_slice(&(flags | ZIP_LANGUAGE_ENCODING_FLAG).to_le_bytes());
        }
    }

    fn add_unicode_path_extra_to_first_central_entry(archive: &mut Vec<u8>, unicode_name: &[u8]) {
        let central_headers: Vec<_> = archive
            .windows(4)
            .enumerate()
            .filter_map(|(offset, bytes)| (bytes == b"PK\x01\x02").then_some(offset))
            .collect();
        assert_eq!(central_headers.len(), 2);

        let first = central_headers[0];
        let name_len = usize::from(u16::from_le_bytes([
            archive[first + 28],
            archive[first + 29],
        ]));
        let old_extra_len = usize::from(u16::from_le_bytes([
            archive[first + 30],
            archive[first + 31],
        ]));
        let name_start = first + 46;
        let raw_name = &archive[name_start..name_start + name_len];
        let mut field = Vec::with_capacity(11 + unicode_name.len());
        field.extend_from_slice(&0x7075_u16.to_le_bytes());
        field.extend_from_slice(&(5_u16 + unicode_name.len() as u16).to_le_bytes());
        field.push(1);
        field.extend_from_slice(&crc32fast::hash(raw_name).to_le_bytes());
        field.extend_from_slice(unicode_name);

        let insertion = name_start + name_len + old_extra_len;
        let field_len = field.len();
        archive.splice(insertion..insertion, field);
        archive[first + 30..first + 32]
            .copy_from_slice(&((old_extra_len + field_len) as u16).to_le_bytes());

        let eocd = archive
            .windows(4)
            .rposition(|bytes| bytes == b"PK\x05\x06")
            .expect("fixture EOCD");
        let directory_size = u32::from_le_bytes(archive[eocd + 12..eocd + 16].try_into().unwrap());
        archive[eocd + 12..eocd + 16]
            .copy_from_slice(&(directory_size + field_len as u32).to_le_bytes());
    }

    #[test]
    fn repeated_serialization_is_deterministic() {
        let original = zip_with(&[("a.xml", b"<a/>"), ("media/x.bin", &[0u8, 1, 2, 255])]);
        let first = OoxmlPackage::read(&original).unwrap().write().unwrap();
        let second = OoxmlPackage::read(&first).unwrap().write().unwrap();
        assert_eq!(first, second, "re-writing must be deterministic");
    }

    #[test]
    fn duplicate_archive_entry_names_fail_loudly() {
        // Generated by Python's zipfile, because zip::ZipWriter correctly
        // refuses to create duplicate names itself.
        let archive = [
            80, 75, 3, 4, 20, 0, 0, 0, 0, 0, 205, 181, 13, 93, 87, 238, 113, 146, 5, 0, 0, 0, 5, 0,
            0, 0, 8, 0, 0, 0, 115, 97, 109, 101, 46, 120, 109, 108, 102, 105, 114, 115, 116, 80,
            75, 3, 4, 20, 0, 0, 0, 0, 0, 205, 181, 13, 93, 105, 17, 31, 182, 6, 0, 0, 0, 6, 0, 0,
            0, 8, 0, 0, 0, 115, 97, 109, 101, 46, 120, 109, 108, 115, 101, 99, 111, 110, 100, 80,
            75, 1, 2, 20, 3, 20, 0, 0, 0, 0, 0, 205, 181, 13, 93, 87, 238, 113, 146, 5, 0, 0, 0, 5,
            0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 1, 0, 0, 0, 0, 115, 97, 109, 101, 46,
            120, 109, 108, 80, 75, 1, 2, 20, 3, 20, 0, 0, 0, 0, 0, 205, 181, 13, 93, 105, 17, 31,
            182, 6, 0, 0, 0, 6, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 1, 43, 0, 0, 0,
            115, 97, 109, 101, 46, 120, 109, 108, 80, 75, 5, 6, 0, 0, 0, 0, 2, 0, 2, 0, 108, 0, 0,
            0, 87, 0, 0, 0, 0, 0,
        ];
        match OoxmlPackage::read(&archive) {
            Err(DocumentError::MalformedPart { part, .. }) if part == "same.xml" => {}
            Err(error) => panic!("wrong duplicate-entry error: {error:?}"),
            Ok(package) => panic!("duplicate entry was accepted: {:?}", package.names),
        }
    }

    #[test]
    fn duplicate_raw_archive_names_with_different_flags_fail_loudly() {
        let mut archive = zip_with(&[("aa", b"first"), ("bb", b"second")]);
        assert_eq!(replace_zip_entry_name(&mut archive, b"aa", b"\xc3\xa9"), 2);
        assert_eq!(replace_zip_entry_name(&mut archive, b"bb", b"\xc3\xa9"), 2);
        set_utf8_flag_on_second_entry(&mut archive);

        match OoxmlPackage::read(&archive) {
            Err(DocumentError::MalformedPart { part, detail })
                if part == "é" && detail == "duplicate zip entry name" => {}
            Err(error) => panic!("wrong duplicate-raw-name error: {error:?}"),
            Ok(package) => panic!("duplicate raw names were accepted: {:?}", package.names),
        }
    }

    #[test]
    fn duplicate_raw_archive_names_in_self_extracting_archive_fail_loudly() {
        let mut archive = zip_with(&[("aa", b"first"), ("bb", b"second")]);
        assert_eq!(replace_zip_entry_name(&mut archive, b"aa", b"\xc3\xa9"), 2);
        assert_eq!(replace_zip_entry_name(&mut archive, b"bb", b"\xc3\xa9"), 2);
        set_utf8_flag_on_second_entry(&mut archive);
        archive.splice(0..0, b"self-extracting-prefix".to_vec());

        match OoxmlPackage::read(&archive) {
            Err(DocumentError::MalformedPart { part, detail })
                if part == "é" && detail == "duplicate zip entry name" => {}
            Err(error) => panic!("wrong duplicate-name error in prefixed archive: {error:?}"),
            Ok(package) => panic!(
                "duplicate names in a prefixed archive were accepted: {:?}",
                package.names
            ),
        }
    }

    #[test]
    fn unicode_path_index_collision_fails_loudly() {
        let mut archive = zip_with(&[("aa", b"first"), ("bb", b"second")]);
        assert_eq!(replace_zip_entry_name(&mut archive, b"bb", b"\xc3\xa9"), 2);
        add_unicode_path_extra_to_first_central_entry(&mut archive, "é".as_bytes());

        match OoxmlPackage::read(&archive) {
            Err(DocumentError::MalformedPart { part, detail })
                if part == "<archive>"
                    && detail
                        == "ZIP central directory entries collide after filename normalization" => {
            }
            Err(error) => panic!("wrong normalized-index collision error: {error:?}"),
            Ok(package) => panic!(
                "entries with a normalized ZIP index collision were accepted: {:?}",
                package.names
            ),
        }
    }

    #[test]
    fn unique_central_directory_unicode_path_remains_readable() {
        let mut archive = zip_with(&[("original.xml", b"first"), ("other.xml", b"second")]);
        add_unicode_path_extra_to_first_central_entry(&mut archive, b"renamed.xml");

        let package = OoxmlPackage::read(&archive)
            .expect("a unique central-directory Unicode Path remains a valid entry name");

        assert_eq!(
            package.entries.get("renamed.xml").map(Vec::as_slice),
            Some(b"first".as_slice())
        );
        assert_eq!(
            package.entries.get("other.xml").map(Vec::as_slice),
            Some(b"second".as_slice())
        );
    }

    #[test]
    fn duplicate_archive_directory_names_fail_loudly() {
        let mut archive = zip_with(&[("aa/", b""), ("bb/", b"")]);
        assert_eq!(
            replace_zip_entry_name(&mut archive, b"bb/", b"aa/"),
            2,
            "the second local and central directory names must be patched"
        );

        match OoxmlPackage::read(&archive) {
            Err(DocumentError::MalformedPart { part, .. }) if part == "aa/" => {}
            Err(error) => panic!("wrong duplicate-directory error: {error:?}"),
            Ok(package) => panic!("duplicate directory was accepted: {:?}", package.names),
        }
    }

    #[test]
    fn central_directory_name_compat_honors_utf8_cp437_and_unicode_path() {
        let legacy_name = b"\xc3\xa9";
        assert_eq!(
            central_directory_entry_name_compat(0, legacy_name, &[]),
            "├⌐"
        );
        assert_eq!(
            central_directory_entry_name_compat(ZIP_LANGUAGE_ENCODING_FLAG, legacy_name, &[]),
            "é"
        );

        let unicode_name = b"corrected.xml";
        let mut extra = vec![0x75, 0x70];
        extra.extend_from_slice(&(1_u16 + 4 + unicode_name.len() as u16).to_le_bytes());
        extra.push(1);
        extra.extend_from_slice(&crc32fast::hash(legacy_name).to_le_bytes());
        extra.extend_from_slice(unicode_name);
        assert_eq!(
            central_directory_entry_name_compat(0, legacy_name, &extra),
            "corrected.xml"
        );

        extra[5] ^= 1;
        assert_eq!(
            central_directory_entry_name_compat(0, legacy_name, &extra),
            "├⌐",
            "a Unicode Path extra with a stale CRC must not rename the entry"
        );
    }

    #[test]
    fn replacing_one_part_leaves_every_other_entry_byte_identical() {
        let media: &[u8] = &[0u8, 1, 2, 255, 254];
        let original = zip_with(&[("a.xml", b"<a/>"), ("media/x.bin", media)]);
        let mut package = OoxmlPackage::read(&original).unwrap();
        package
            .replace_part("a.xml", b"<a>edited</a>".to_vec())
            .unwrap();
        let edited = OoxmlPackage::read(&package.write().unwrap()).unwrap();
        assert_eq!(edited.part("a.xml").unwrap(), b"<a>edited</a>");
        assert_eq!(
            edited.part("media/x.bin").unwrap(),
            media,
            "an untouched binary part must survive bit-for-bit"
        );
    }

    #[test]
    fn round_trip_preserves_legacy_cp437_entry_name() {
        // ZIP 8 decoded flag-clear names as CP437. Keep an unrelated member's
        // identity unchanged when an OOXML edit rewrites the archive.
        let mut original = zip_with(&[("a.xml", b"<a/>"), ("aa", b"legacy payload")]);
        assert_eq!(
            replace_zip_entry_name(&mut original, b"aa", b"\xC3\xA9"),
            2,
            "both ZIP filename records must be patched"
        );

        let package = OoxmlPackage::read(&original).expect("read legacy package");
        let rewritten = package.write().expect("rewrite package");
        let mut archive = zip::ZipArchive::new(Cursor::new(rewritten)).expect("open rewritten ZIP");
        let entry = archive.by_index(1).expect("read untouched legacy member");
        assert_eq!(entry.name().expect("decode rewritten name"), "├⌐");
        let mut payload = Vec::new();
        entry
            .take(MAX_ENTRY_BYTES)
            .read_to_end(&mut payload)
            .unwrap();
        assert_eq!(payload, b"legacy payload");
    }

    #[test]
    fn transform_preserves_attribute_order_and_self_closing_forms() {
        // A DOM round trip is exactly what normalizes these; the event
        // transform must not.
        let xml = r#"<?xml version="1.0"?><w:p z="3" a="1"><w:r/><w:t>hi</w:t></w:p>"#;
        let out = transform_xml(xml, |_| Ok(EventAction::Keep)).unwrap();
        assert_eq!(out, xml);
    }

    #[test]
    fn transform_can_drop_a_targeted_event() {
        let xml = "<root><keep/><drop/></root>";
        let out = transform_xml(xml, |event| {
            if let Event::Empty(tag) = event
                && local_name(tag.name().as_ref()) == b"drop"
            {
                return Ok(EventAction::Drop);
            }
            Ok(EventAction::Keep)
        })
        .unwrap();
        assert_eq!(out, "<root><keep/></root>");
    }

    #[test]
    fn oversized_entry_is_rejected_rather_than_buffered() {
        let package = crate::test_fixtures::package(&[("large.xml", vec![b'x'; 9])]);
        let result = OoxmlPackage::read_with_limits(&package, 8, 64, 4);
        assert!(matches!(result, Err(DocumentError::PackageTooLarge { .. })));
    }

    #[test]
    fn local_name_strips_any_producer_prefix() {
        assert_eq!(local_name(b"w:ins"), b"ins");
        assert_eq!(local_name(b"ins"), b"ins");
        assert_eq!(local_name(b"w14:paraId"), b"paraId");
    }
}
