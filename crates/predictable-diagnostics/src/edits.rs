//! Applying suggested edits.
//!
//! Suggestions are literal byte-range replacements so that an agent can apply
//! them mechanically and re-check (`01-ir.md` §7). This module is that
//! application step, with the checks that make it safe: in-bounds, on a UTF-8
//! character boundary, and non-overlapping.

use crate::model::Edit;

/// Why a set of edits could not be applied.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    /// The range runs past the end of the source, or `start > end`.
    #[error("edit range {start}..{end} is out of bounds for a {len}-byte source")]
    OutOfBounds {
        /// Start offset of the offending edit.
        start: usize,
        /// End offset of the offending edit.
        end: usize,
        /// Length of the source in bytes.
        len: usize,
    },
    /// An offset falls inside a multi-byte character.
    #[error("edit offset {offset} is not a UTF-8 character boundary")]
    NotCharBoundary {
        /// The offending offset.
        offset: usize,
    },
    /// Two edits touch the same bytes, so the result would depend on order.
    #[error("edits {first_start}..{first_end} and {second_start}..{second_end} overlap")]
    Overlapping {
        /// Start of the earlier edit.
        first_start: usize,
        /// End of the earlier edit.
        first_end: usize,
        /// Start of the later edit.
        second_start: usize,
        /// End of the later edit.
        second_end: usize,
    },
    /// An edit named a file other than the one being patched.
    #[error("edit targets file `{file}`, but `{expected}` is being patched")]
    WrongFile {
        /// File the edit named.
        file: String,
        /// File being patched.
        expected: String,
    },
}

/// Apply `edits` to `source`, returning the patched text.
///
/// Edits may be given in any order; they are applied by ascending start offset.
/// Two insertions at the same offset are applied in the order given, which makes
/// the result deterministic. Any overlap of non-empty ranges is an error rather
/// than a silent last-writer-wins.
pub fn apply_edits(source: &str, edits: &[Edit]) -> Result<String, EditError> {
    let mut ordered: Vec<(usize, &Edit)> = edits.iter().enumerate().collect();
    ordered.sort_by_key(|(i, e)| (e.start, e.end, *i));

    let len = source.len();
    let mut out = String::with_capacity(len);
    let mut cursor = 0usize;
    let mut previous: Option<(usize, usize)> = None;

    for (_, edit) in &ordered {
        if edit.start > edit.end || edit.end > len {
            return Err(EditError::OutOfBounds {
                start: edit.start,
                end: edit.end,
                len,
            });
        }
        for offset in [edit.start, edit.end] {
            if !source.is_char_boundary(offset) {
                return Err(EditError::NotCharBoundary { offset });
            }
        }
        if let Some((ps, pe)) = previous {
            // Overlap means the later edit starts strictly before the previous
            // one ended. Two zero-width inserts at the same point do not overlap.
            if edit.start < pe {
                return Err(EditError::Overlapping {
                    first_start: ps,
                    first_end: pe,
                    second_start: edit.start,
                    second_end: edit.end,
                });
            }
        }
        out.push_str(&source[cursor..edit.start]);
        out.push_str(&edit.replacement);
        cursor = edit.end;
        previous = Some((edit.start, edit.end));
    }
    out.push_str(&source[cursor..]);
    Ok(out)
}

/// Apply only those edits that target `file`, erroring on any edit that does not.
pub fn apply_edits_to_file(source: &str, file: &str, edits: &[Edit]) -> Result<String, EditError> {
    if let Some(bad) = edits.iter().find(|e| e.file != file) {
        return Err(EditError::WrongFile {
            file: bad.file.clone(),
            expected: file.to_string(),
        });
    }
    apply_edits(source, edits)
}
