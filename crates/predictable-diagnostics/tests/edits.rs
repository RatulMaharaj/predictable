//! Byte-range suggested edits: applying them, and refusing to apply nonsense.

use predictable_diagnostics::{apply_edits, apply_edits_to_file, Edit, EditError};

const SRC: &str = "expr = \"premium_rate * num_pols_iff\"\n";

#[test]
fn single_replacement_applies() {
    let edits = [Edit::replace("m.pir", 23..35, "num_pols_if")];
    assert_eq!(
        apply_edits(SRC, &edits).unwrap(),
        "expr = \"premium_rate * num_pols_if\"\n"
    );
}

#[test]
fn insertion_and_deletion_are_edits_with_degenerate_ranges() {
    let src = "a b";
    assert_eq!(
        apply_edits(src, &[Edit::insert("f", 2, "x ")]).unwrap(),
        "a x b"
    );
    assert_eq!(apply_edits(src, &[Edit::delete("f", 0..2)]).unwrap(), "b");
}

#[test]
fn multiple_edits_apply_regardless_of_input_order() {
    let src = "return a + b\n";
    let forwards = [
        Edit::replace("f", 7..8, "retime(a, MID)"),
        Edit::replace("f", 11..12, "retime(b, MID)"),
    ];
    let backwards = [forwards[1].clone(), forwards[0].clone()];
    let expected = "return retime(a, MID) + retime(b, MID)\n";
    assert_eq!(apply_edits(src, &forwards).unwrap(), expected);
    assert_eq!(apply_edits(src, &backwards).unwrap(), expected);
}

#[test]
fn two_insertions_at_the_same_offset_keep_their_given_order() {
    let src = "x";
    let edits = [Edit::insert("f", 0, "a"), Edit::insert("f", 0, "b")];
    assert_eq!(apply_edits(src, &edits).unwrap(), "abx");
}

#[test]
fn overlapping_edits_are_rejected_rather_than_silently_ordered() {
    let edits = [
        Edit::replace("f", 0..10, "one"),
        Edit::replace("f", 5..15, "two"),
    ];
    let err = apply_edits(SRC, &edits).unwrap_err();
    assert!(
        matches!(
            err,
            EditError::Overlapping {
                first_start: 0,
                first_end: 10,
                second_start: 5,
                second_end: 15
            }
        ),
        "unexpected error: {err}"
    );
}

#[test]
fn an_insertion_inside_another_edits_range_overlaps() {
    let edits = [
        Edit::replace("f", 0..10, "one"),
        Edit::insert("f", 4, "two"),
    ];
    assert!(matches!(
        apply_edits(SRC, &edits).unwrap_err(),
        EditError::Overlapping { .. }
    ));
}

#[test]
fn adjacent_edits_do_not_overlap() {
    let src = "abcd";
    let edits = [Edit::replace("f", 0..2, "X"), Edit::replace("f", 2..4, "Y")];
    assert_eq!(apply_edits(src, &edits).unwrap(), "XY");
}

#[test]
fn out_of_bounds_and_inverted_ranges_are_rejected() {
    assert!(matches!(
        apply_edits("abc", &[Edit::replace("f", 2..99, "x")]).unwrap_err(),
        EditError::OutOfBounds { len: 3, .. }
    ));
    let inverted = Edit {
        file: "f".to_string(),
        start: 3,
        end: 1,
        replacement: "x".to_string(),
    };
    assert!(matches!(
        apply_edits("abc", &[inverted]).unwrap_err(),
        EditError::OutOfBounds { .. }
    ));
}

#[test]
fn offsets_inside_a_multibyte_character_are_rejected() {
    // "é" is two bytes; offset 1 is mid-character.
    let src = "café";
    assert!(matches!(
        apply_edits(src, &[Edit::replace("f", 3..4, "e")]).unwrap_err(),
        EditError::NotCharBoundary { offset: 4 }
    ));
    // The whole character is fine.
    assert_eq!(
        apply_edits(src, &[Edit::replace("f", 3..5, "e")]).unwrap(),
        "cafe"
    );
}

#[test]
fn edits_spanning_a_multibyte_prefix_keep_offsets_in_bytes() {
    let src = "π = 3.14";
    // "π" is two bytes, so "3.14" starts at byte 5, not byte 4.
    assert_eq!(src.find("3.14"), Some(5));
    assert_eq!(
        apply_edits(src, &[Edit::replace("f", 5..9, "3.14159")]).unwrap(),
        "π = 3.14159"
    );
}

#[test]
fn applying_to_a_named_file_rejects_edits_for_another_file() {
    let edits = [Edit::replace("other.pir", 0..1, "x")];
    let err = apply_edits_to_file(SRC, "m.pir", &edits).unwrap_err();
    assert!(matches!(err, EditError::WrongFile { .. }), "{err}");
}

#[test]
fn applying_a_suggestion_twice_is_not_idempotent_but_is_deterministic() {
    // Guards against accidental "apply until stable" loops in agent code: the
    // first application changes the text, so offsets from the first diagnostic
    // must not be reused.
    let once = apply_edits(SRC, &[Edit::replace("m.pir", 23..35, "num_pols_if")]).unwrap();
    let twice = apply_edits(&once, &[Edit::replace("m.pir", 23..35, "num_pols_if")]).unwrap();
    assert_ne!(once, twice);
    assert_eq!(
        twice,
        apply_edits(&once, &[Edit::replace("m.pir", 23..35, "num_pols_if")]).unwrap()
    );
}
