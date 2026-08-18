//! The modelpoint reader the wasm build uses instead of `predictable-io`.
//!
//! `predictable-io` pulls in `arrow` and `parquet`; both are excellent and
//! neither belongs in a 1.5 MB budget (`03-engine.md` §9). A modelpoint CSV is a
//! header row and typed columns whose types the *IR* declares (`01-ir.md`
//! §2.10) — nothing is inferred here, so the only thing this module owns is
//! RFC 4180 field splitting.
//!
//! Divergence from `predictable-io` would be a real hazard, so the native test
//! suite checks a case both ways: `tests/session.rs` asserts the values this
//! reader produces are bit-identical to the ones `predictable run` wrote for
//! the same file.

/// Split RFC 4180 text into rows of fields.
///
/// Quoted fields may contain commas, newlines and doubled quotes. A trailing
/// newline does not produce an empty final row; a blank line in the middle does
/// (a row of one empty field), because silently dropping rows would shift every
/// row index after it and `first_row` is part of the result ordering.
pub fn rows(text: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    let mut any = false;
    while let Some(c) = chars.next() {
        any = true;
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                out.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    // A final row without a trailing newline still ends a row, and so does a
    // final trailing comma (an empty last field is a field).
    if !field.is_empty() || !row.is_empty() || (any && text.ends_with(',')) {
        row.push(field);
        out.push(row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::rows;

    #[test]
    fn plain_rows_split_on_commas() {
        assert_eq!(
            rows("a,b\n1,2\n"),
            vec![vec!["a", "b"], vec!["1", "2"]]
                .into_iter()
                .map(|r| r.into_iter().map(String::from).collect::<Vec<_>>())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn quoted_fields_keep_commas_newlines_and_doubled_quotes() {
        let parsed = rows("k,v\n\"a,b\",\"line\nbreak\"\n\"say \"\"hi\"\"\",2\n");
        assert_eq!(
            parsed[1],
            vec!["a,b".to_string(), "line\nbreak".to_string()]
        );
        assert_eq!(parsed[2][0], "say \"hi\"");
    }

    #[test]
    fn a_missing_trailing_newline_still_yields_the_last_row() {
        assert_eq!(rows("a\n1")[1], vec!["1".to_string()]);
    }

    #[test]
    fn crlf_is_not_part_of_the_value() {
        assert_eq!(rows("a,b\r\n1,2\r\n")[1], vec!["1".to_string(), "2".into()]);
    }
}
