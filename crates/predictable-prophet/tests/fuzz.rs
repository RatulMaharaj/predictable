//! Fuzzing the readers (`04-verify.md` §4: "fuzzed to never panic").
//!
//! `bytes -> (value, [Diagnostic])` is a *total* function. These are property
//! tests rather than a `cargo-fuzz` target so that they run in ordinary CI on
//! stable Rust; the corpus is deliberately adversarial (arbitrary bytes, lone
//! CRs, unterminated quotes, huge dimension extents, non-UTF-8) and every case
//! asserts the same three invariants:
//!
//! 1. the call returns — no panic, no unwrap, no arithmetic overflow;
//! 2. every diagnostic carries a registered code and a span inside the file;
//! 3. the value is self-consistent (row widths, cell counts, key arithmetic).

use predictable_diagnostics::Diagnostic;
use predictable_prophet::{read_fac, read_mpf, read_rpt, MpfOptions, Read, RptOptions};
use proptest::prelude::*;

fn check_diagnostics(diags: &[Diagnostic], len: usize) {
    for d in diags {
        assert!(
            predictable_diagnostics::registry::lookup(&d.code).is_some(),
            "unregistered code {}",
            d.code
        );
        assert!(d.doc_url.ends_with(&d.code), "{} has no anchor", d.code);
        for span in &d.spans {
            assert!(
                span.start <= span.end && span.end <= len,
                "{} span {}..{} outside a {len}-byte file",
                d.code,
                span.start,
                span.end
            );
        }
    }
}

fn check_mpf(read: &Read<predictable_prophet::MpfFile>, len: usize) {
    check_diagnostics(&read.diagnostics, len);
    for row in &read.value.rows {
        assert_eq!(row.len(), read.value.columns.len());
    }
    // Every distinct set belongs to a real column and respects the 64 cap.
    for (name, values) in &read.value.distinct {
        assert!(read.value.column(name).is_some());
        assert!(values.len() <= 64);
    }
    let _ = read.value.to_csv();
    let _ = read.value.to_fragment_toml();
}

fn check_fac(read: &Read<predictable_prophet::FacTable>, len: usize) {
    check_diagnostics(&read.diagnostics, len);
    let table = &read.value;
    let per_cell = table.values.len().max(1);
    if !table.dims.is_empty() && table.data.len() == table.cell_count() * per_cell {
        // A clean read: the key of every cell is inside the declared extents.
        for cell in 0..table.cell_count() {
            for (k, dim) in table.key_at(cell).iter().zip(&table.dims) {
                assert!((dim.lo..=dim.hi).contains(k));
            }
        }
        assert!(!read.diagnostics.iter().any(|d| d.code == "P0203"));
    }
    let _ = table.to_csv();
    let _ = table.to_fragment_toml();
}

fn check_rpt(read: &Read<predictable_prophet::RptResults>, len: usize) {
    check_diagnostics(&read.diagnostics, len);
    for row in &read.value.rows {
        assert_eq!(row.values.len(), read.value.components.len());
    }
    // No rows may exist unless the period base was an explicit, recorded decision.
    if read.value.period_base.is_none() {
        assert!(read.value.rows.is_empty());
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Arbitrary bytes: the readers must survive anything on disk.
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let len = bytes.len();
        check_mpf(&read_mpf(&bytes, "f.mpf", &MpfOptions::default()), len);
        check_fac(&read_fac(&bytes, "f.fac"), len);
        check_rpt(&read_rpt(&bytes, "f.rpt", &RptOptions::default()), len);
    }

    /// Bytes drawn from the alphabet that actually appears in Prophet exports —
    /// far more likely to reach the deep paths than uniform noise.
    #[test]
    fn prophet_shaped_noise_never_panics(
        text in proptest::collection::vec(
            prop::sample::select(vec![
                "*", ",", "\r\n", "\n", "\r", "!", "#", "\"", "%", "-", "/", ".", " ",
                "VARIABLE_TYPES", "OUTPUT_FORMAT", "NUMLINES", "DATE_FORMAT", "DIMENSIONS",
                "DIM1", "DIM2", "DATA", "VALUES", "PERIOD", "RUN", "TIME_UNITS", "NUM_PERIODS",
                "POL_NUM", "AGE", "SEX", "I", "N", "S", "D", "B", "T", "0", "1", "12", "1e999",
                "9999999999999999999999", "01/02/2024", "20240601", "\u{00a3}", "\u{fffd}",
            ]),
            0..80,
        ),
    ) {
        let bytes = text.concat().into_bytes();
        let len = bytes.len();
        check_mpf(&read_mpf(&bytes, "f.mpf", &MpfOptions::default()), len);
        check_fac(&read_fac(&bytes, "f.fac"), len);
        check_rpt(&read_rpt(&bytes, "f.rpt", &RptOptions::default()), len);
    }

    /// A generated, well-formed `.MPF` always reads back its own values.
    #[test]
    fn generated_mpf_round_trips(
        rows in proptest::collection::vec((0i64..1000, -1e6f64..1e6), 1..20),
        crlf in any::<bool>(),
    ) {
        let nl = if crlf { "\r\n" } else { "\n" };
        let mut src = format!("! generated{nl}VARIABLE_TYPES,I,N{nl}NUMLINES,{}{nl}POL,AMOUNT{nl}", rows.len());
        for (i, amount) in &rows {
            src.push_str(&format!("*,{i},{amount}{nl}"));
        }
        let read = read_mpf(src.as_bytes(), "g.mpf", &MpfOptions::default());
        check_mpf(&read, src.len());
        prop_assert!(!read.has_errors(), "{:?}", read.codes());
        prop_assert_eq!(read.value.rows.len(), rows.len());
        for (r, (i, amount)) in rows.iter().enumerate() {
            prop_assert_eq!(
                read.value.cell(r, "pol"),
                Some(&predictable_prophet::Cell::Int(*i))
            );
            prop_assert_eq!(
                read.value.cell(r, "amount"),
                Some(&predictable_prophet::Cell::Float(*amount))
            );
        }
    }

    /// A generated `.fac` either matches its declared extents exactly or says so.
    #[test]
    fn generated_fac_counts_are_exact(
        n1 in 1i64..6,
        n2 in 1i64..6,
        surplus in 0usize..4,
    ) {
        let expected = (n1 * n2) as usize;
        let mut src = format!("DIMENSIONS, 2\nDIM1, AGE, 1, {n1}\nDIM2, SEX, 1, {n2}\nDATA\n");
        for i in 0..expected + surplus {
            src.push_str(&format!("{}.5\n", i));
        }
        let read = read_fac(src.as_bytes(), "g.fac");
        check_fac(&read, src.len());
        let complained = read.diagnostics.iter().any(|d| d.code == "P0203");
        prop_assert_eq!(complained, surplus != 0);
        prop_assert_eq!(read.value.cell_count(), expected);
    }

    /// The `.rpt` reader rebases `t` by exactly the stated base, or refuses.
    #[test]
    fn generated_rpt_rebases_by_the_stated_base(
        first in 0i64..4,
        periods in 1usize..8,
        base in 0i64..4,
    ) {
        let mut src = String::from("RUN, G\nPERIOD,POL_NUM,BEL\n");
        for p in 0..periods {
            src.push_str(&format!("{},POL1,{}.0\n", first + p as i64, p));
        }
        let read = read_rpt(
            src.as_bytes(),
            "g.rpt",
            &RptOptions { period_base: Some(base), ..RptOptions::default() },
        );
        check_rpt(&read, src.len());
        prop_assert_eq!(read.value.rows.len(), periods);
        for (i, row) in read.value.rows.iter().enumerate() {
            prop_assert_eq!(row.t, (first + i as i64 - base) as i32);
        }
    }
}

/// Regressions found by the generators above, kept as fixed cases.
#[test]
fn known_awkward_inputs() {
    for src in [
        &b""[..],
        b"\r",
        b"\n\n\n",
        b"*",
        b"*,,,,",
        b"\"unterminated",
        b"VARIABLE_TYPES\n\n*\n",
        b"DIMENSIONS, 9999999999999999999\nDATA\n1\n",
        b"DIMENSIONS, 2\nDIM1, A, -9223372036854775808, 9223372036854775807\nDIM2, B, 0, 0\nDATA\n1\n",
        b"PERIOD\n0\n",
        b"\xff\xfe\x00\x00",
        b"\xef\xbb\xbfVARIABLE_TYPES,S\nP\n*,x\n",
    ] {
        let len = src.len();
        check_mpf(&read_mpf(src, "f.mpf", &MpfOptions::default()), len);
        check_fac(&read_fac(src, "f.fac"), len);
        check_rpt(&read_rpt(src, "f.rpt", &RptOptions::default()), len);
    }
}
