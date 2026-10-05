//! Pure byte-binding witnesses for the frozen IMPLEMENTATION-R1.md interface.
//!
//! These tests establish source/span equality only, never Evidence admission, truth, attribution,
//! access, persistence or an end-to-end Knowledge workflow. Fixtures are inert in-memory bytes.
//! Expected SHA-256 values were calculated independently with Python's hashlib, not Beacon's
//! hashing implementation. Hand-counted multibyte offsets distinguish bytes from characters.

use beacond::{
    MAX_SOURCE_QUOTE_BYTES, MAX_SOURCE_SNAPSHOT_BYTES, SourceSpanDigest, SourceSpanError,
    verify_source_span,
};

const SNAPSHOT: &str = "prefix café 🦀\n";
const SNAPSHOT_SHA256: &str = "3a29fc43912b5ace30e4ebb4a3c57364d186c2f671e318df46b892c47fdf8d88";
const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const NFC_SHA256: &str = "850f7dc43910ff890f8879c0ed26fe697c93a067ad93a7d50f466a7028a9bf4e";
const NFD_SHA256: &str = "81ef060bcd98adc7824eb5c1ada83c32491b16018e11e79f00ab9d09e04b015a";
const ONE_MIB_A_SHA256: &str = "9bc1b2a288b26af7257a36277ae3816a7d4f16e89c1e7e77d0a5c48bad62b360";
const FROZEN_BYTE_CAP: usize = 1_048_576;

fn raw_digest(hex: &str) -> SourceSpanDigest<'static> {
    assert_eq!(hex.len(), 64, "fixture SHA-256 must have 32 bytes");
    SourceSpanDigest {
        algorithm: "sha256",
        canonicalizer: "beacon.raw.v1",
        value: std::array::from_fn(|index| {
            u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
                .expect("independently computed fixture hex")
        }),
    }
}

fn assert_refusal(
    snapshot: &[u8],
    expected: &SourceSpanDigest<'_>,
    start: u64,
    end: u64,
    quote: &[u8],
    wanted: SourceSpanError,
) {
    // SourceSpanMatch deliberately has no assumed Debug/PartialEq implementation. A success is
    // never printed: these observations require the specific refusal, not merely any error.
    match verify_source_span(snapshot, expected, start, end, quote) {
        Ok(_) => panic!("expected {wanted:?}, received a source-span match"),
        Err(actual) => assert_eq!(actual, wanted),
    }
}

#[test]
fn exact_multibyte_span_returns_the_full_raw_digest_byte_range_and_quote() {
    let snapshot = SNAPSHOT.as_bytes();
    let expected = raw_digest(SNAPSHOT_SHA256);
    assert_eq!(snapshot.len(), 18);
    assert_eq!("café 🦀".len(), 10);

    let matched = verify_source_span(snapshot, &expected, 7, 17, "café 🦀".as_bytes())
        .expect("the exact independently bound byte span matches");
    assert_eq!(matched.digest(), expected.value);
    assert_eq!(matched.range(), 7..17);
    assert_eq!(matched.quote(), "café 🦀");
    assert_eq!(matched.quote().as_ptr(), snapshot[7..17].as_ptr());
}

#[test]
fn a_match_borrows_snapshot_after_the_separate_quote_and_digest_inputs_are_dropped() {
    let snapshot = SNAPSHOT.as_bytes().to_vec();
    let matched = {
        let expected = raw_digest(SNAPSHOT_SHA256);
        let quote = "café 🦀".as_bytes().to_vec();
        assert_ne!(quote.as_ptr(), snapshot[7..17].as_ptr());
        verify_source_span(&snapshot, &expected, 7, 17, &quote)
            .expect("the quote argument is not the borrowed result's owner")
    };

    assert_eq!(matched.quote(), "café 🦀");
    assert_eq!(matched.quote().as_ptr(), snapshot[7..17].as_ptr());
    assert_eq!(matched.range(), 7..17);
    assert_eq!(matched.digest(), raw_digest(SNAPSHOT_SHA256).value);
}

#[test]
fn different_ranges_share_the_same_complete_snapshot_digest() {
    let expected = raw_digest(ABC_SHA256);
    for (start, end, quote) in [(0, 1, b"a"), (1, 2, b"b"), (2, 3, b"c")] {
        let matched = verify_source_span(b"abc", &expected, start, end, quote)
            .expect("each exact byte span cites the same complete snapshot");
        assert_eq!(matched.digest(), expected.value);
        assert_eq!(matched.range(), start as usize..end as usize);
        assert_eq!(matched.quote().as_bytes(), quote);
    }
}

#[test]
fn changing_source_outside_the_quote_invalidates_the_original_snapshot_digest() {
    let expected = raw_digest(SNAPSHOT_SHA256);
    let mut changed_prefix = SNAPSHOT.as_bytes().to_vec();
    changed_prefix[0] = b'P';
    let mut changed_suffix = SNAPSHOT.as_bytes().to_vec();
    changed_suffix[17] = b'!';
    let mut appended_suffix = SNAPSHOT.as_bytes().to_vec();
    appended_suffix.push(b'x');

    for changed in [changed_prefix, changed_suffix, appended_suffix] {
        assert_eq!(&changed[7..17], "café 🦀".as_bytes());
        assert_refusal(
            &changed,
            &expected,
            7,
            17,
            "café 🦀".as_bytes(),
            SourceSpanError::DigestMismatch,
        );
    }
}

#[test]
fn one_wrong_digest_byte_is_not_a_verified_identity() {
    let mut expected = raw_digest(SNAPSHOT_SHA256);
    expected.value[31] ^= 1;
    assert_refusal(
        SNAPSHOT.as_bytes(),
        &expected,
        7,
        17,
        "café 🦀".as_bytes(),
        SourceSpanError::DigestMismatch,
    );
}

#[test]
fn a_quote_digest_cannot_replace_the_complete_snapshot_digest() {
    // SHA-256 of b"b", independently computed, not SHA-256 of b"abc".
    let quote_digest =
        raw_digest("3e23e8160039594a33894f6564e1b1348bbd7a0088d42c4acb73eeaed59c009d");
    assert_refusal(
        b"abc",
        &quote_digest,
        1,
        2,
        b"b",
        SourceSpanError::DigestMismatch,
    );
}

#[test]
fn character_coordinates_do_not_get_reinterpreted_as_byte_coordinates() {
    let expected = raw_digest("ce6aeb3715a4484be929ac6cd04af03e3a925391fb4f7cfed812e8803ec4354f");
    let snapshot = "aé🦀z".as_bytes();
    assert_eq!(snapshot.len(), 8);
    let matched = verify_source_span(snapshot, &expected, 1, 7, "é🦀".as_bytes())
        .expect("the six-byte, two-character quote uses byte coordinates");
    assert_eq!(matched.range(), 1..7);
    assert_eq!(matched.quote(), "é🦀");

    // 1..3 names two characters but only the bytes for é. A character-offset mutant would accept.
    assert_refusal(
        snapshot,
        &expected,
        1,
        3,
        "é🦀".as_bytes(),
        SourceSpanError::QuoteMismatch,
    );
}

#[test]
fn digest_algorithm_spelling_is_exact_and_has_no_fallback() {
    for algorithm in [
        "", "SHA256", "sha-256", "sha512", " sha256", "sha256 ", "sha256\0",
    ] {
        let expected = SourceSpanDigest {
            algorithm,
            canonicalizer: "beacon.raw.v1",
            value: raw_digest(ABC_SHA256).value,
        };
        assert_refusal(
            b"abc",
            &expected,
            0,
            3,
            b"abc",
            SourceSpanError::UnsupportedDigestAlgorithm,
        );
    }
}

#[test]
fn canonicalizer_spelling_is_exact_and_has_no_normalized_alias() {
    for canonicalizer in [
        "",
        "beacon.raw.v2",
        "beacon.artifact.v1",
        "BEACON.RAW.V1",
        " beacon.raw.v1",
        "beacon.raw.v1 ",
        "beacon.raw.v1\0",
    ] {
        let expected = SourceSpanDigest {
            algorithm: "sha256",
            canonicalizer,
            value: raw_digest(ABC_SHA256).value,
        };
        assert_refusal(
            b"abc",
            &expected,
            0,
            3,
            b"abc",
            SourceSpanError::UnsupportedCanonicalizer,
        );
    }
}

#[test]
fn the_two_exported_byte_caps_match_the_frozen_one_mib_contract() {
    assert_eq!(MAX_SOURCE_SNAPSHOT_BYTES, FROZEN_BYTE_CAP);
    assert_eq!(MAX_SOURCE_QUOTE_BYTES, FROZEN_BYTE_CAP);
}

#[test]
fn an_exact_cap_snapshot_can_verify_a_small_quote() {
    let snapshot = vec![b'a'; FROZEN_BYTE_CAP];
    let expected = raw_digest(ONE_MIB_A_SHA256);
    let matched = verify_source_span(&snapshot, &expected, 0, 1, b"a")
        .expect("the snapshot size cap is inclusive");
    assert_eq!(matched.digest(), expected.value);
    assert_eq!(matched.range(), 0..1);
    assert_eq!(matched.quote(), "a");
}

#[test]
fn snapshot_and_quote_exactly_at_both_caps_are_accepted() {
    let snapshot = vec![b'a'; FROZEN_BYTE_CAP];
    let expected = raw_digest(ONE_MIB_A_SHA256);
    let matched = verify_source_span(&snapshot, &expected, 0, FROZEN_BYTE_CAP as u64, &snapshot)
        .expect("both length caps are inclusive");
    assert_eq!(matched.digest(), expected.value);
    assert_eq!(matched.range(), 0..FROZEN_BYTE_CAP);
    assert_eq!(matched.quote().len(), FROZEN_BYTE_CAP);
    assert_eq!(matched.quote().as_ptr(), snapshot.as_ptr());
}

#[test]
fn an_over_cap_snapshot_is_refused_before_envelope_encoding_or_hashing() {
    let snapshot = vec![0xff; FROZEN_BYTE_CAP + 1];
    let expected = SourceSpanDigest {
        algorithm: "unsupported",
        canonicalizer: "unsupported",
        value: [0; 32],
    };
    assert_refusal(
        &snapshot,
        &expected,
        0,
        0,
        b"",
        SourceSpanError::SnapshotTooLarge,
    );
}

#[test]
fn an_over_cap_quote_is_refused_before_envelope_encoding_or_hashing() {
    let quote = vec![0xff; FROZEN_BYTE_CAP + 1];
    let expected = SourceSpanDigest {
        algorithm: "unsupported",
        canonicalizer: "unsupported",
        value: [0; 32],
    };
    assert_refusal(
        b"abc",
        &expected,
        0,
        3,
        &quote,
        SourceSpanError::QuoteTooLarge,
    );
}

#[test]
fn an_exact_cap_invalid_snapshot_is_an_encoding_error_not_a_size_error() {
    let mut snapshot = vec![b'a'; FROZEN_BYTE_CAP];
    snapshot[FROZEN_BYTE_CAP - 1] = 0xff;
    assert_refusal(
        &snapshot,
        &raw_digest(ONE_MIB_A_SHA256),
        0,
        0,
        b"",
        SourceSpanError::InvalidSnapshotEncoding,
    );
}

#[test]
fn an_exact_cap_invalid_quote_is_an_encoding_error_not_a_size_error() {
    let quote = vec![0xff; FROZEN_BYTE_CAP];
    assert_refusal(
        b"",
        &raw_digest(EMPTY_SHA256),
        0,
        0,
        &quote,
        SourceSpanError::InvalidQuoteEncoding,
    );
}

#[test]
fn invalid_snapshot_utf8_is_refused_even_outside_the_requested_slice() {
    for snapshot in [
        &[0xff][..],
        &[b'a', 0x80, b'b'][..],
        &[0xc3][..],
        &[0xed, 0xa0, 0x80][..],
        &[b'a', b'b', b'c', 0xff][..],
    ] {
        assert_refusal(
            snapshot,
            &raw_digest(ABC_SHA256),
            0,
            0,
            b"",
            SourceSpanError::InvalidSnapshotEncoding,
        );
    }
}

#[test]
fn invalid_quote_utf8_has_its_named_error_with_a_valid_snapshot() {
    for quote in [&[0xff][..], &[0xc3][..], &[0xed, 0xa0, 0x80][..]] {
        assert_refusal(
            b"abc",
            &raw_digest(ABC_SHA256),
            0,
            0,
            quote,
            SourceSpanError::InvalidQuoteEncoding,
        );
    }
}

#[test]
fn unsupported_envelopes_are_refused_before_encoding_checks() {
    let wrong_algorithm = SourceSpanDigest {
        algorithm: "sha512",
        canonicalizer: "beacon.raw.v1",
        value: [0; 32],
    };
    assert_refusal(
        &[0xff],
        &wrong_algorithm,
        0,
        0,
        b"",
        SourceSpanError::UnsupportedDigestAlgorithm,
    );
    let wrong_canonicalizer = SourceSpanDigest {
        algorithm: "sha256",
        canonicalizer: "beacon.raw.v2",
        value: [0; 32],
    };
    assert_refusal(
        b"abc",
        &wrong_canonicalizer,
        0,
        0,
        &[0xff],
        SourceSpanError::UnsupportedCanonicalizer,
    );
}

#[test]
fn encoding_checks_precede_digest_and_range_checks() {
    let wrong_digest = raw_digest(EMPTY_SHA256);
    assert_refusal(
        &[0xff],
        &wrong_digest,
        u64::MAX,
        u64::MAX,
        b"",
        SourceSpanError::InvalidSnapshotEncoding,
    );
    assert_refusal(
        b"abc",
        &wrong_digest,
        u64::MAX,
        u64::MAX,
        &[0xff],
        SourceSpanError::InvalidQuoteEncoding,
    );
}

#[test]
fn complete_snapshot_digest_is_checked_before_invalid_offsets() {
    assert_refusal(
        b"abc",
        &raw_digest(EMPTY_SHA256),
        u64::MAX,
        u64::MAX,
        b"",
        SourceSpanError::DigestMismatch,
    );
}

#[test]
fn reversed_out_of_bounds_and_unrepresentable_ranges_are_refused() {
    let expected = raw_digest(ABC_SHA256);
    for (start, end) in [
        (2, 1),
        (0, 4),
        (4, 4),
        (u64::MAX, u64::MAX),
        (u64::MAX, 0),
        (0, u64::MAX),
        (1_u64 << 32, (1_u64 << 32) + 1),
    ] {
        assert_refusal(
            b"abc",
            &expected,
            start,
            end,
            b"a",
            SourceSpanError::InvalidRange,
        );
    }
}

#[test]
fn either_endpoint_inside_a_multibyte_codepoint_is_an_invalid_range() {
    let expected = raw_digest(SNAPSHOT_SHA256);
    for (start, end) in [(11, 12), (10, 11), (14, 17), (13, 16)] {
        assert_refusal(
            SNAPSHOT.as_bytes(),
            &expected,
            start,
            end,
            b"",
            SourceSpanError::InvalidRange,
        );
    }
}

#[test]
fn a_valid_range_with_wrong_quote_bytes_is_not_a_match() {
    let expected = raw_digest(SNAPSHOT_SHA256);
    for quote in [b"cafe crab".as_slice(), "cafe 🦀".as_bytes(), b"", b"cafe"] {
        assert_refusal(
            SNAPSHOT.as_bytes(),
            &expected,
            7,
            17,
            quote,
            SourceSpanError::QuoteMismatch,
        );
    }
}

#[test]
fn newline_spellings_are_compared_byte_for_byte_without_fallback() {
    let snapshot = b"one\r\ntwo\n";
    let expected = raw_digest("611912c05d6113dd9932610cf2d5876c6f05324e05f326a40620dd4f3fd4ac0d");
    let matched = verify_source_span(snapshot, &expected, 0, 9, snapshot)
        .expect("exact CRLF and LF bytes remain unchanged");
    assert_eq!(matched.quote().as_bytes(), snapshot);
    assert_eq!(matched.digest(), expected.value);
    for quote in [b"one\ntwo\n".as_slice(), b"one\r\ntwo", b"one\rtwo\n"] {
        assert_refusal(
            snapshot,
            &expected,
            0,
            9,
            quote,
            SourceSpanError::QuoteMismatch,
        );
    }
}

#[test]
fn unicode_normalization_is_not_applied_in_either_quote_direction() {
    for (snapshot, hex, different_quote) in [
        ("café", NFC_SHA256, "cafe\u{0301}"),
        ("cafe\u{0301}", NFD_SHA256, "café"),
    ] {
        let expected = raw_digest(hex);
        let matched = verify_source_span(
            snapshot.as_bytes(),
            &expected,
            0,
            snapshot.len() as u64,
            snapshot.as_bytes(),
        )
        .expect("each exact Unicode encoding has its own raw identity");
        assert_eq!(matched.quote(), snapshot);
        assert_eq!(matched.digest(), expected.value);
        assert_refusal(
            snapshot.as_bytes(),
            &expected,
            0,
            snapshot.len() as u64,
            different_quote.as_bytes(),
            SourceSpanError::QuoteMismatch,
        );
    }
}

#[test]
fn normalized_snapshot_bytes_do_not_preserve_the_original_raw_digest() {
    assert_refusal(
        "cafe\u{0301}".as_bytes(),
        &raw_digest(NFC_SHA256),
        0,
        6,
        "cafe\u{0301}".as_bytes(),
        SourceSpanError::DigestMismatch,
    );
}

#[test]
fn valid_utf8_control_bytes_are_not_sanitized_or_removed() {
    let snapshot = b"a\0b\r\n";
    let expected = raw_digest("eee4d3a83335b4ab5ef32addb24ce2f696624d7c6c64e8a3c4d1eaf48b0dc5de");
    let matched = verify_source_span(snapshot, &expected, 0, 5, snapshot)
        .expect("NUL, CR and LF are exact valid UTF-8 bytes");
    assert_eq!(matched.quote().as_bytes(), snapshot);
    assert_eq!(matched.range(), 0..5);
    let nul = verify_source_span(snapshot, &expected, 1, 2, b"\0")
        .expect("a control byte can be the exact source span");
    assert_eq!(nul.quote().as_bytes(), b"\0");
    assert_eq!(nul.quote().as_ptr(), snapshot[1..2].as_ptr());
}

#[test]
fn an_empty_snapshot_and_quote_establish_only_empty_byte_equality() {
    let expected = raw_digest(EMPTY_SHA256);
    let matched = verify_source_span(b"", &expected, 0, 0, b"")
        .expect("empty equality is a byte fact, not Evidence admission");
    assert_eq!(matched.digest(), expected.value);
    assert_eq!(matched.range(), 0..0);
    assert_eq!(matched.quote(), "");
}

#[test]
fn an_empty_slice_does_not_skip_complete_snapshot_digest_verification() {
    assert_refusal(
        b"",
        &raw_digest(ABC_SHA256),
        0,
        0,
        b"",
        SourceSpanError::DigestMismatch,
    );
    assert_refusal(
        b"abc",
        &raw_digest(EMPTY_SHA256),
        1,
        1,
        b"",
        SourceSpanError::DigestMismatch,
    );
}

#[test]
fn empty_ranges_at_utf8_boundaries_return_the_same_snapshot_borrow() {
    let snapshot = SNAPSHOT.as_bytes();
    let expected = raw_digest(SNAPSHOT_SHA256);
    for position in [0, 7, 10, 12, 13, 17, 18] {
        let matched = verify_source_span(snapshot, &expected, position, position, b"")
            .expect("an empty range is valid only at a real UTF-8 boundary");
        let position = position as usize;
        assert_eq!(matched.range(), position..position);
        assert_eq!(matched.quote(), "");
        assert_eq!(
            matched.quote().as_ptr(),
            snapshot[position..position].as_ptr()
        );
        assert_eq!(matched.digest(), expected.value);
    }
}

#[test]
fn an_empty_range_inside_a_codepoint_is_still_invalid() {
    let expected = raw_digest(SNAPSHOT_SHA256);
    for position in [11, 14, 15, 16] {
        assert_refusal(
            SNAPSHOT.as_bytes(),
            &expected,
            position,
            position,
            b"",
            SourceSpanError::InvalidRange,
        );
    }
}

#[test]
fn nonempty_quote_does_not_match_an_empty_range_or_empty_snapshot() {
    for (snapshot, hex, position) in [
        (b"".as_slice(), EMPTY_SHA256, 0),
        (b"abc".as_slice(), ABC_SHA256, 1),
    ] {
        assert_refusal(
            snapshot,
            &raw_digest(hex),
            position,
            position,
            b"a",
            SourceSpanError::QuoteMismatch,
        );
    }
}

#[test]
fn refusal_diagnostics_do_not_echo_supplied_quote_bytes() {
    let quote = b"SYNTHETIC-QUOTE-CANARY";
    let error = match verify_source_span(b"abc", &raw_digest(ABC_SHA256), 0, 3, quote) {
        Ok(_) => panic!("a different synthetic quote must refuse"),
        Err(error) => error,
    };
    assert_eq!(error, SourceSpanError::QuoteMismatch);
    let diagnostic = format!("{error:?}");
    assert!(!diagnostic.contains("SYNTHETIC-QUOTE-CANARY"));
    assert!(!diagnostic.contains("abc"));
}
