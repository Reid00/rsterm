use std::collections::VecDeque;
use std::fmt;

pub const MAX_HISTORY_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_HISTORY_RECORDS: usize = 1000;
pub const MAX_RECORD_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HistorySource {
    Bash,
    #[default]
    Zsh,
}

impl HistorySource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bash => "Bash · ~/.bash_history",
            Self::Zsh => "Zsh · ~/.zsh_history",
        }
    }

    pub(crate) fn filename(self) -> &'static str {
        match self {
            Self::Bash => ".bash_history",
            Self::Zsh => ".zsh_history",
        }
    }
}

#[derive(Clone)]
pub struct HistorySnapshot {
    pub source: HistorySource,
    pub entries: Vec<HistoryEntry>,
    pub truncated: bool,
    pub notes: Vec<String>,
}

impl fmt::Debug for HistorySnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HistorySnapshot")
            .field("source", &self.source)
            .field("entries", &self.entries.len())
            .field("truncated", &self.truncated)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct HistoryEntry {
    pub id: u64,
    pub timestamp: Option<i64>,
    pub preview: String,
    text: Option<String>,
    boundary_known: bool,
    incomplete: bool,
}

impl fmt::Debug for HistoryEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HistoryEntry")
            .field("id", &self.id)
            .field("bytes", &self.text.as_ref().map(String::len))
            .field("incomplete", &self.incomplete)
            .finish_non_exhaustive()
    }
}

impl HistoryEntry {
    pub fn is_boundary_known(&self) -> bool {
        self.boundary_known
    }

    pub fn is_incomplete(&self) -> bool {
        self.incomplete
    }

    pub fn copy_text(&self) -> Option<&str> {
        (!self.incomplete).then_some(self.text.as_deref()).flatten()
    }

    pub fn insertion_text(&self) -> Option<&str> {
        if !self.boundary_known || self.incomplete {
            return None;
        }
        let text = self.text.as_deref()?;
        (!text.is_empty() && !text.chars().any(unsafe_character)).then_some(text)
    }

    pub fn safety_note(&self) -> Option<&'static str> {
        if self.incomplete {
            Some("Incomplete or oversized history record; filling is disabled")
        } else if self.text.is_none() {
            Some("Invalid history encoding; copying and filling are disabled")
        } else if !self.boundary_known {
            Some("Bash line boundaries are ambiguous; copy only")
        } else if self.insertion_text().is_none() {
            Some("Multiline or control characters; filling is disabled")
        } else {
            None
        }
    }
}

fn unsafe_character(ch: char) -> bool {
    ch.is_control()
        || matches!(ch, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

fn preview(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text
            .chars()
            .flat_map(|ch| {
                if unsafe_character(ch) {
                    format!("\\u{{{:x}}}", ch as u32)
                        .chars()
                        .collect::<Vec<_>>()
                } else {
                    vec![ch]
                }
            })
            .collect(),
        Err(_) => bytes
            .iter()
            .flat_map(|byte| {
                if byte.is_ascii_graphic() || *byte == b' ' {
                    vec![char::from(*byte)]
                } else {
                    format!("\\x{byte:02x}").chars().collect()
                }
            })
            .collect(),
    }
}

fn entry(id: usize, timestamp: Option<i64>, bytes: Vec<u8>, boundary_known: bool) -> HistoryEntry {
    let incomplete = bytes.len() > MAX_RECORD_BYTES;
    let bytes = &bytes[..bytes.len().min(MAX_RECORD_BYTES)];
    HistoryEntry {
        id: id as u64 + 1,
        timestamp,
        preview: preview(bytes),
        text: std::str::from_utf8(bytes).ok().map(str::to_owned),
        boundary_known,
        incomplete,
    }
}

pub fn parse_history(source: HistorySource, bytes: &[u8], tail_truncated: bool) -> HistorySnapshot {
    let mut snapshot = HistorySnapshot {
        source,
        entries: Vec::new(),
        truncated: tail_truncated,
        notes: Vec::new(),
    };
    let mut records = VecDeque::with_capacity(MAX_HISTORY_RECORDS);
    let bounded = &bytes[bytes.len().saturating_sub(MAX_HISTORY_BYTES)..];
    let cut = tail_truncated || bytes.len() > MAX_HISTORY_BYTES;
    let complete_end = bounded
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    if complete_end < bounded.len() {
        snapshot.truncated = true;
        snapshot
            .notes
            .push("An incomplete final record was omitted".into());
    }
    let lines: Vec<_> = bounded[..complete_end]
        .split_inclusive(|byte| *byte == b'\n')
        .collect();
    match source {
        HistorySource::Bash => {
            let has_timestamps = lines.iter().any(|line| bash_timestamp(line).is_some());
            if has_timestamps {
                let mut current: Option<(usize, i64, Vec<u8>)> = None;
                for (index, line) in lines.iter().enumerate().skip(usize::from(cut)) {
                    if let Some(timestamp) = bash_timestamp(line) {
                        if let Some((id, timestamp, mut data)) = current.take() {
                            data.pop();
                            if !data.is_empty() {
                                push_record(
                                    &mut records,
                                    entry(id, Some(timestamp), data, true),
                                    &mut snapshot.truncated,
                                );
                            }
                        }
                        current = Some((index, timestamp, Vec::new()));
                    } else if let Some((_, _, data)) = &mut current {
                        data.extend_from_slice(line);
                    }
                }
                if let Some((id, timestamp, mut data)) = current {
                    if complete_end == bounded.len() {
                        data.pop();
                        if !data.is_empty() {
                            push_record(
                                &mut records,
                                entry(id, Some(timestamp), data, true),
                                &mut snapshot.truncated,
                            );
                        }
                    } else {
                        snapshot.truncated = true;
                    }
                }
                if cut {
                    snapshot
                        .notes
                        .push("The leading partial record was omitted".into());
                }
            } else {
                snapshot.notes.push(
                    "Plain Bash history has ambiguous multiline boundaries; entries are copy only"
                        .into(),
                );
                for (index, line) in lines.iter().enumerate().skip(usize::from(cut)) {
                    let data = &line[..line.len() - 1];
                    if !data.is_empty() {
                        push_record(
                            &mut records,
                            entry(index, None, data.to_vec(), false),
                            &mut snapshot.truncated,
                        );
                    }
                }
            }
        }
        HistorySource::Zsh => {
            let mut current = Vec::new();
            let mut start = 0;
            let mut discard = cut;
            for (index, line) in lines.iter().enumerate() {
                if current.is_empty() {
                    start = index;
                }
                let body = &line[..line.len() - 1];
                let continuation = body.last() == Some(&b'\\');
                current.extend_from_slice(if continuation {
                    &body[..body.len() - 1]
                } else {
                    body
                });
                if continuation {
                    current.push(b'\n');
                    continue;
                }
                if !discard && !current.is_empty() {
                    let (timestamp, body) = zsh_metadata(&current);
                    let decoded = unmetafy(body);
                    match decoded {
                        Some(decoded) => push_record(
                            &mut records,
                            entry(start, timestamp, decoded, true),
                            &mut snapshot.truncated,
                        ),
                        None => {
                            let mut record = entry(start, timestamp, body.to_vec(), true);
                            record.text = None;
                            push_record(&mut records, record, &mut snapshot.truncated);
                        }
                    }
                }
                current.clear();
                discard = false;
            }
            if !current.is_empty() {
                snapshot.truncated = true;
                snapshot
                    .notes
                    .push("An incomplete continued record was omitted".into());
            }
            if cut {
                snapshot
                    .notes
                    .push("The leading partial record was omitted".into());
            }
        }
    }
    snapshot.entries = records.into_iter().collect();
    snapshot.truncated |= cut;
    if snapshot.truncated {
        snapshot
            .notes
            .push("Only the bounded, most recent saved history is shown".into());
    }
    snapshot.entries.reverse();
    snapshot
}

fn push_record(records: &mut VecDeque<HistoryEntry>, entry: HistoryEntry, truncated: &mut bool) {
    *truncated |= entry.incomplete;
    if records.len() == MAX_HISTORY_RECORDS {
        records.pop_front();
        *truncated = true;
    }
    records.push_back(entry);
}

fn bash_timestamp(line: &[u8]) -> Option<i64> {
    let body = line.strip_prefix(b"#")?.strip_suffix(b"\n")?;
    if body.is_empty() || !body.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(body).ok()?.parse().ok()
}

fn zsh_metadata(bytes: &[u8]) -> (Option<i64>, &[u8]) {
    let Some(value) = bytes.strip_prefix(b": ") else {
        return (None, bytes);
    };
    let Some(colon) = value.iter().position(|byte| *byte == b':') else {
        return (None, bytes);
    };
    let Some(semicolon) = value[colon + 1..]
        .iter()
        .position(|byte| *byte == b';')
        .map(|index| index + colon + 1)
    else {
        return (None, bytes);
    };
    if !value[..colon].iter().all(u8::is_ascii_digit)
        || !value[colon + 1..semicolon].iter().all(u8::is_ascii_digit)
    {
        return (None, bytes);
    }
    let timestamp = std::str::from_utf8(&value[..colon])
        .ok()
        .and_then(|text| text.parse().ok());
    (timestamp, &value[semicolon + 1..])
}

fn unmetafy(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut bytes = bytes.iter();
    while let Some(byte) = bytes.next() {
        decoded.push(if *byte == 0x83 {
            *bytes.next()? ^ 32
        } else {
            *byte
        });
    }
    Some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_preserves_duplicates_and_blocks_ambiguous_lines() {
        let snapshot = parse_history(HistorySource::Bash, b"echo same\necho same\n", false);
        assert_eq!(snapshot.entries.len(), 2);
        assert_ne!(snapshot.entries[0].id, snapshot.entries[1].id);
        assert_eq!(snapshot.entries[0].copy_text(), Some("echo same"));
        assert_eq!(snapshot.entries[0].insertion_text(), None);
        let framed = parse_history(
            HistorySource::Bash,
            b"#100\necho one\n#200\necho two\nline two\n",
            false,
        );
        assert_eq!(framed.entries[0].timestamp, Some(200));
        assert_eq!(framed.entries[0].insertion_text(), None);
        assert_eq!(framed.entries[1].insertion_text(), Some("echo one"));
    }

    #[test]
    fn zsh_metadata_continuations_metafied_and_unsafe_unicode() {
        let snapshot = parse_history(
            HistorySource::Zsh,
            b": 123:0;echo hello\n: 124:1;echo multi\\\nline\n: 125:0;echo \x83\xe3\x83\x89\n",
            false,
        );
        assert_eq!(snapshot.entries[2].insertion_text(), Some("echo hello"));
        assert_eq!(snapshot.entries[1].copy_text(), Some("echo multi\nline"));
        assert_eq!(snapshot.entries[1].insertion_text(), None);
        assert_eq!(snapshot.entries[0].copy_text(), Some("echo é"));
        let unsafe_text = parse_history(
            HistorySource::Zsh,
            "echo \u{202e}bad\necho \u{0085}bad\n".as_bytes(),
            false,
        );
        assert!(unsafe_text
            .entries
            .iter()
            .all(|record| record.insertion_text().is_none()));
        assert!(!unsafe_text.entries[1].preview.contains('\u{202e}'));
    }

    #[test]
    fn zsh_embedded_newlines_preserve_existing_backslashes_and_never_split() {
        for existing_slashes in 1..=3 {
            let original = format!("echo first{}\nsecond", "\\".repeat(existing_slashes));
            let stored = format!(
                ": 123:0;echo first{}\nsecond\n",
                "\\".repeat(existing_slashes + 1)
            );
            let snapshot = parse_history(HistorySource::Zsh, stored.as_bytes(), false);
            assert_eq!(snapshot.entries.len(), 1);
            assert_eq!(snapshot.entries[0].copy_text(), Some(original.as_str()));
            assert_eq!(snapshot.entries[0].insertion_text(), None);
            let tail = format!("{stored}: 124:0;echo complete\n");
            let truncated = parse_history(HistorySource::Zsh, tail.as_bytes(), true);
            assert_eq!(truncated.entries.len(), 1);
            assert_eq!(truncated.entries[0].insertion_text(), Some("echo complete"));
            let incomplete = format!(
                ": 123:0;echo first{}\nsecond",
                "\\".repeat(existing_slashes + 1)
            );
            let eof = parse_history(HistorySource::Zsh, incomplete.as_bytes(), false);
            assert!(eof.entries.is_empty());
            assert!(eof.truncated);
            let continued_eof =
                format!(": 123:0;echo first{}\n", "\\".repeat(existing_slashes + 1));
            let eof = parse_history(HistorySource::Zsh, continued_eof.as_bytes(), false);
            assert!(eof.entries.is_empty());
            assert!(eof.truncated);
        }
    }

    #[test]
    fn tails_invalid_encoding_oversized_and_debug_are_safe() {
        let snapshot = parse_history(
            HistorySource::Zsh,
            b"partial\necho secret\nbroken\xff\nincomplete",
            true,
        );
        assert_eq!(snapshot.entries.len(), 2);
        assert_eq!(snapshot.entries[0].copy_text(), None);
        assert_eq!(snapshot.entries[1].insertion_text(), Some("echo secret"));
        assert!(!format!("{snapshot:?}").contains("secret"));
        assert!(!format!("{:?}", snapshot.entries[1]).contains("secret"));
        let mut bytes = vec![b'x'; MAX_RECORD_BYTES + 1];
        bytes.push(b'\n');
        assert!(parse_history(HistorySource::Zsh, &bytes, false).entries[0]
            .copy_text()
            .is_none());
    }
}
