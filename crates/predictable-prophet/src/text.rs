//! Byte-level scanning shared by the three readers.
//!
//! Everything here works on the **original file bytes**, never on a decoded
//! `String`, so that every span a reader emits is a byte offset into the file the
//! user actually has on disk — which is the whole point of the `P0xxx`
//! diagnostics (`04-verify.md` §4). Only individual field slices are decoded, and
//! Windows-1252 decoding is one source byte per character, so decoding can never
//! move an offset.

/// Which decoding was used for a file's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// The bytes were valid UTF-8.
    Utf8,
    /// The bytes were not valid UTF-8 and were decoded as Windows-1252.
    Windows1252,
}

impl Encoding {
    /// The name used in messages and in `meta.source.encoding`.
    pub fn as_str(self) -> &'static str {
        match self {
            Encoding::Utf8 => "utf-8",
            Encoding::Windows1252 => "windows-1252",
        }
    }
}

/// The Windows-1252 mapping for `0x80..=0x9F`; the rest of the range is Latin-1,
/// i.e. the code point equals the byte. `0x81/0x8D/0x8F/0x90/0x9D` are unassigned
/// and map to U+FFFD rather than failing — a reader never fails on encoding.
const CP1252_HIGH: [char; 32] = [
    '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}', '\u{017D}', '\u{FFFD}',
    '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{FFFD}',
];

/// A UTF-8 byte-order mark.
pub const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// Decode a slice, preferring UTF-8 and falling back to Windows-1252.
///
/// Total: there is no input for which this returns an error.
pub fn decode(bytes: &[u8]) -> String {
    match core::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes
            .iter()
            .map(|b| match b {
                0x80..=0x9F => CP1252_HIGH[(b - 0x80) as usize],
                other => *other as char,
            })
            .collect(),
    }
}

/// The file's encoding and, when it is not UTF-8, the offset of the first byte
/// that made it so — which is what `P0101` points at.
pub fn detect_encoding(bytes: &[u8]) -> (Encoding, Option<usize>) {
    match core::str::from_utf8(bytes) {
        Ok(_) => (Encoding::Utf8, None),
        Err(e) => (Encoding::Windows1252, Some(e.valid_up_to())),
    }
}

/// One physical line, with its absolute byte range in the file (excluding the
/// terminator) and its 1-based number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line<'a> {
    /// The line's bytes, without the terminator.
    pub bytes: &'a [u8],
    /// Absolute start offset in the file.
    pub start: usize,
    /// Absolute end offset in the file (exclusive), before the terminator.
    pub end: usize,
    /// 1-based physical line number.
    pub number: usize,
}

impl<'a> Line<'a> {
    /// The decoded text of the whole line.
    pub fn text(&self) -> String {
        decode(self.bytes)
    }

    /// The line with leading and trailing ASCII whitespace removed, span adjusted.
    pub fn trimmed(&self) -> Line<'a> {
        let mut lo = 0usize;
        let mut hi = self.bytes.len();
        while lo < hi && self.bytes[lo].is_ascii_whitespace() {
            lo += 1;
        }
        while hi > lo && self.bytes[hi - 1].is_ascii_whitespace() {
            hi -= 1;
        }
        Line {
            bytes: &self.bytes[lo..hi],
            start: self.start + lo,
            end: self.start + hi,
            number: self.number,
        }
    }

    /// True for a line with no non-whitespace bytes.
    pub fn is_blank(&self) -> bool {
        self.bytes.iter().all(|b| b.is_ascii_whitespace())
    }

    /// The whole line as a span range.
    pub fn range(&self) -> core::ops::Range<usize> {
        self.start..self.end
    }
}

/// Split a file into lines, accepting LF, CRLF and lone CR, and skipping a
/// leading UTF-8 BOM (whose bytes stay counted in every offset).
pub fn lines(bytes: &[u8]) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut i = if bytes.starts_with(BOM) { BOM.len() } else { 0 };
    let mut start = i;
    let mut number = 1usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                out.push(Line {
                    bytes: &bytes[start..i],
                    start,
                    end: i,
                    number,
                });
                number += 1;
                i += 1;
                start = i;
            }
            b'\r' => {
                out.push(Line {
                    bytes: &bytes[start..i],
                    start,
                    end: i,
                    number,
                });
                number += 1;
                i += if bytes.get(i + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < bytes.len() {
        out.push(Line {
            bytes: &bytes[start..],
            start,
            end: bytes.len(),
            number,
        });
    }
    out
}

/// One comma-separated field: its decoded text (unquoted, trimmed) and the byte
/// range of the raw field, quotes included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// Decoded, unquoted, whitespace-trimmed text.
    pub text: String,
    /// Absolute start offset of the raw field.
    pub start: usize,
    /// Absolute end offset (exclusive) of the raw field.
    pub end: usize,
}

impl Field {
    /// The field as a span range.
    pub fn range(&self) -> core::ops::Range<usize> {
        self.start..self.end
    }

    /// The field's text upper-cased — header keys are matched case-insensitively.
    pub fn key(&self) -> String {
        self.text.to_ascii_uppercase()
    }

    /// True when the text is empty (a null cell).
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// Split one line into comma-separated fields.
///
/// Quoting is the CSV convention Prophet exports use: a field may be wrapped in
/// `"`, inside which commas are literal and `""` is an escaped quote. Whitespace
/// outside the quotes is trimmed; whitespace inside is kept.
pub fn split_fields(line: &Line<'_>) -> Vec<Field> {
    let bytes = line.bytes;
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut field_start = 0usize;
    let mut text: Vec<u8> = Vec::new();
    let mut quoted = false;
    let mut in_quotes = false;

    let flush = |out: &mut Vec<Field>, text: &mut Vec<u8>, quoted: bool, lo: usize, hi: usize| {
        let (mut lo2, mut hi2) = (lo, hi);
        while lo2 < hi2 && bytes[lo2].is_ascii_whitespace() {
            lo2 += 1;
        }
        while hi2 > lo2 && bytes[hi2 - 1].is_ascii_whitespace() {
            hi2 -= 1;
        }
        let decoded = decode(text);
        let decoded = if quoted {
            decoded
        } else {
            decoded.trim().to_string()
        };
        out.push(Field {
            text: decoded,
            start: line.start + lo2,
            end: line.start + hi2,
        });
        text.clear();
    };

    while i < bytes.len() {
        let b = bytes[i];
        if in_quotes {
            if b == b'"' {
                if bytes.get(i + 1) == Some(&b'"') {
                    text.push(b'"');
                    i += 2;
                    continue;
                }
                in_quotes = false;
            } else {
                text.push(b);
            }
            i += 1;
            continue;
        }
        match b {
            b'"' => {
                in_quotes = true;
                quoted = true;
            }
            b',' => {
                flush(&mut out, &mut text, quoted, field_start, i);
                quoted = false;
                field_start = i + 1;
            }
            _ => text.push(b),
        }
        i += 1;
    }
    flush(&mut out, &mut text, quoted, field_start, bytes.len());
    out
}

/// `SUM_ASSURED` → `sum_assured`. Anything that is not `[a-z0-9]` becomes `_`,
/// runs collapse, and leading/trailing `_` are dropped. An empty result becomes
/// `field`, so a name is never the empty string.
pub fn snake_case(name: &str) -> String {
    let mut out = String::new();
    let mut prev_us = true;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_us = false;
        } else if !prev_us {
            out.push('_');
            prev_us = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        "field".to_string()
    } else {
        out
    }
}

/// True when `s` looks like a column name rather than a value: an ASCII letter or
/// `_` followed by letters, digits, `_`, `.` or `-`. This is the test the
/// name-line rules of §4.1.4 and §4.3 use, so a numeric first data row can never
/// be mistaken for a header.
pub fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' || c == ' ')
}
