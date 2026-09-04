//! Measured spend v1, slice 1 (ADR 0042): the record's pure types and its canonical bytes.
//!
//! Every fixture is constructed. Slice 1 owns no database, no service and no event, so a test that
//! needed one would be testing a later slice.
//!
//! The canonical bytes are assembled twice: once by Beacon's encoder and once by
//! [`fixture_bytes_by_hand`], which follows ADR 0042's published table rather than calling that
//! encoder. That independence is the point -- a canonicalizer that quietly changed its layout would
//! agree with itself and disagree with this file.

use std::cmp::Ordering;

use beacond::{
    MalformedSpend, MeasuredSpendV1, SpendLine, SpendMeasurement, SpendOrigin, SpendSupersedes,
    SpendTotals, UncountedEntry, UncountedKind, compare_units,
};
use uuid::Uuid;

const MAGIC: &[u8] = b"beacon.spend.v1\0";

const WORKSPACE: &str = "00000000-0000-4000-8000-000000000001";
const MISSION: &str = "00000000-0000-4000-8000-000000000002";
const SPEND: &str = "00000000-0000-4000-8000-000000000003";

const MEASURED_AT_MS: u64 = 1_788_372_724_000;
const COVERAGE_FROM: u64 = 1;
const COVERAGE_TO: u64 = 4_210;

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("canonical uuid")
}

fn line(unit: &str, amount_minor: i64, minor_units_exponent: u8) -> SpendLine {
    SpendLine {
        unit: unit.to_owned(),
        amount_minor,
        minor_units_exponent,
    }
}

fn totals(lines: Vec<SpendLine>, counted_units: u64) -> SpendTotals {
    SpendTotals {
        counted_units,
        lines,
    }
}

/// One measured record: two units, one of them zero, with a superseded predecessor.
fn fixture() -> MeasuredSpendV1 {
    MeasuredSpendV1 {
        workspace_id: uuid(WORKSPACE),
        mission_id: uuid(MISSION),
        spend_id: uuid(SPEND),
        version: 2,
        origin: SpendOrigin::DaemonLedger,
        measured_at_ms: MEASURED_AT_MS,
        coverage_from_event_seq: COVERAGE_FROM,
        coverage_to_event_seq: COVERAGE_TO,
        measurement: SpendMeasurement::Measured(totals(
            vec![line("USD", 123_456, 2), line("tokens", 0, 0)],
            412,
        )),
        supersedes: SpendSupersedes::Record("aa".repeat(32)),
    }
}

fn with(measurement: SpendMeasurement) -> MeasuredSpendV1 {
    MeasuredSpendV1 {
        measurement,
        supersedes: SpendSupersedes::None,
        ..fixture()
    }
}

// ---------------------------------------------------------------------------------------------
// An independent encoder: ADR 0042's published table, not Beacon's encoder
// ---------------------------------------------------------------------------------------------

fn push_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u64).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

fn fixture_bytes_by_hand() -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(uuid(WORKSPACE).as_bytes());
    out.extend_from_slice(uuid(MISSION).as_bytes());
    out.extend_from_slice(uuid(SPEND).as_bytes());
    out.extend_from_slice(&2_u32.to_be_bytes());
    out.push(0x01); // origin = daemon_ledger
    out.extend_from_slice(&MEASURED_AT_MS.to_be_bytes());
    out.extend_from_slice(&COVERAGE_FROM.to_be_bytes());
    out.extend_from_slice(&COVERAGE_TO.to_be_bytes());

    out.push(0x01); // measurement = measured
    out.extend_from_slice(&412_u64.to_be_bytes());
    out.extend_from_slice(&2_u32.to_be_bytes());
    push_string(&mut out, "USD");
    out.extend_from_slice(&123_456_i64.to_be_bytes());
    out.push(2);
    push_string(&mut out, "tokens");
    out.extend_from_slice(&0_i64.to_be_bytes());
    out.push(0);

    out.push(0x01); // supersedes = record
    out.extend_from_slice(&[0xaa; 32]);
    out
}

// ---------------------------------------------------------------------------------------------
// The eleven load-bearing tests of the design's slice S1
// ---------------------------------------------------------------------------------------------

#[test]
fn spend_measurement_arms_stay_four_distinct_literals() {
    let arms = [
        SpendMeasurement::Measured(totals(vec![line("USD", 1, 2)], 1)),
        SpendMeasurement::PartiallyMeasured(
            totals(vec![line("USD", 1, 2)], 1),
            vec![UncountedEntry {
                kind: UncountedKind::ProviderDidNotPrice,
                detail: "the provider returned no price for three calls.".to_owned(),
            }],
        ),
        SpendMeasurement::Unmeasured("the spend owner has not measured this Mission.".to_owned()),
        SpendMeasurement::Broken("the ledger for this window contradicts itself.".to_owned()),
    ];

    let literals: Vec<&str> = arms.iter().map(SpendMeasurement::literal).collect();
    assert_eq!(
        literals,
        ["measured", "partially_measured", "unmeasured", "broken"]
    );
    let mut distinct = literals.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), 4, "two arms share a literal");

    // Four arms, four tags, four round trips: no arm decodes as another.
    for arm in arms {
        let record = with(arm.clone());
        let bytes = record.canonical_bytes().expect("encodes");
        let decoded = MeasuredSpendV1::from_canonical_bytes(&bytes).expect("decodes");
        assert_eq!(decoded.measurement, arm);
        assert_eq!(decoded.measurement.literal(), arm.literal());
    }

    // And no arm is spelled as a collapse of another.
    for collapse in ["n/a", "none", "null", "unknown", "zero"] {
        assert!(!literals.contains(&collapse));
    }
}

#[test]
fn spend_unmeasured_and_broken_have_no_amount_to_default_to_zero() {
    // The shape is the enforcement: neither arm has an amount field, so there is nowhere for a zero
    // to sit and no caller can leave one at its default.
    for arm in [
        SpendMeasurement::Unmeasured("nobody counted.".to_owned()),
        SpendMeasurement::Broken("the ledger is unusable.".to_owned()),
    ] {
        assert!(arm.totals().is_none(), "{} carried totals", arm.literal());
        assert!(arm.reason().is_some());

        // Nor do the encoded bytes contain a zero amount: the arm's payload is its reason and
        // nothing else.
        let bytes = with(arm.clone()).canonical_bytes().expect("encodes");
        let decoded = MeasuredSpendV1::from_canonical_bytes(&bytes).expect("decodes");
        assert!(decoded.measurement.totals().is_none());
    }

    // A measured arm does carry totals, so the absence above is a property of those two arms and
    // not of the accessor.
    assert!(
        SpendMeasurement::Measured(totals(vec![line("USD", 0, 2)], 0))
            .totals()
            .is_some()
    );
}

#[test]
fn spend_measured_zero_names_its_unit_and_is_not_unmeasured() {
    let zero = with(SpendMeasurement::Measured(totals(
        vec![line("USD", 0, 2)],
        412,
    )));
    let bytes = zero.canonical_bytes().expect("encodes");
    let decoded = MeasuredSpendV1::from_canonical_bytes(&bytes).expect("decodes");

    let counted = decoded
        .measurement
        .totals()
        .expect("measured carries totals");
    assert_eq!(counted.lines.len(), 1);
    assert_eq!(counted.lines[0].unit, "USD");
    assert_eq!(counted.lines[0].amount_minor, 0);
    assert_eq!(decoded.measurement.literal(), "measured");
    assert_ne!(decoded.measurement.literal(), "unmeasured");

    // "Measured zero over 412 priced units" and "measured zero over none" are different claims.
    assert_eq!(counted.counted_units, 412);
    let nothing_billable = with(SpendMeasurement::Measured(totals_of_zero()));
    assert_ne!(
        zero.digest().expect("digest").value,
        nothing_billable.digest().expect("digest").value
    );

    // A zero with no unit cannot be expressed at all: an empty line list is refused.
    let unitless = with(SpendMeasurement::Measured(totals(Vec::new(), 0)));
    assert_eq!(
        unitless.canonical_bytes().expect_err("refuses"),
        MalformedSpend::NoLines
    );
}

fn totals_of_zero() -> SpendTotals {
    totals(vec![line("USD", 0, 2)], 0)
}

#[test]
fn spend_lines_are_unit_unique_and_sort_by_code_point_not_locale() {
    // A collation that treats `-` as ignorable -- the shape ICU uses at primary strength -- puts
    // `ab` before `a-z`; code point puts `a-z` first.
    fn punctuation_insensitive(left: &str, right: &str) -> Ordering {
        left.replace('-', "").cmp(&right.replace('-', ""))
    }
    assert_eq!(compare_units("a-z", "ab"), Ordering::Less);
    assert_eq!(punctuation_insensitive("a-z", "ab"), Ordering::Greater);

    let canonical = with(SpendMeasurement::Measured(totals(
        vec![line("a-z", 1, 0), line("ab", 2, 0)],
        2,
    )));
    let bytes = canonical
        .canonical_bytes()
        .expect("code-point order encodes");
    assert!(MeasuredSpendV1::from_canonical_bytes(&bytes).is_ok());

    // Collation order is refused rather than silently re-sorted: two orders of one measurement must
    // not both encode.
    let collated = with(SpendMeasurement::Measured(totals(
        vec![line("ab", 2, 0), line("a-z", 1, 0)],
        2,
    )));
    assert_eq!(
        collated.canonical_bytes().expect_err("refuses"),
        MalformedSpend::UnitOrder
    );

    // One unit, two amounts, is a question rather than a record.
    let repeated = with(SpendMeasurement::Measured(totals(
        vec![line("USD", 1, 2), line("USD", 2, 2)],
        2,
    )));
    assert_eq!(
        repeated.canonical_bytes().expect_err("refuses"),
        MalformedSpend::UnitNotUnique
    );
}

#[test]
fn spend_unit_tokens_are_stored_exactly_as_measured_and_never_case_folded() {
    // R13.6: `USD` and `usd` are two units. Both are legal, both are stored as measured, and the
    // record that carries both is not the record that carries one.
    let both = with(SpendMeasurement::Measured(totals(
        vec![line("USD", 100, 2), line("usd", 200, 2)],
        2,
    )));
    let bytes = both.canonical_bytes().expect("two spellings are two units");
    let decoded = MeasuredSpendV1::from_canonical_bytes(&bytes).expect("decodes");
    let lines = &decoded.measurement.totals().expect("totals").lines;
    assert_eq!(lines[0].unit, "USD", "the stored spelling was folded");
    assert_eq!(lines[1].unit, "usd", "the stored spelling was folded");

    // Code point puts uppercase first, so the pair is already canonical in that order and the
    // reverse is refused -- a fold would have made them one unit and refused as `UnitNotUnique`.
    assert_eq!(compare_units("USD", "usd"), Ordering::Less);

    let upper = with(SpendMeasurement::Measured(totals(
        vec![line("USD", 100, 2)],
        1,
    )));
    let lower = with(SpendMeasurement::Measured(totals(
        vec![line("usd", 100, 2)],
        1,
    )));
    assert_ne!(
        upper.digest().expect("digest").value,
        lower.digest().expect("digest").value,
        "two spellings produced one identity"
    );
}

#[test]
fn spend_canonical_bytes_follow_adr_0034_0037_strict_round_trip_rules() {
    let record = fixture();
    let bytes = record.canonical_bytes().expect("encodes");

    // An independent assembly of ADR 0042's table produces the same bytes.
    assert_eq!(
        bytes,
        fixture_bytes_by_hand(),
        "Beacon's encoder and ADR 0042's published table disagree"
    );

    // Exact decode / re-encode byte round trip.
    let decoded = MeasuredSpendV1::from_canonical_bytes(&bytes).expect("decodes");
    assert_eq!(decoded, record);
    assert_eq!(decoded.canonical_bytes().expect("re-encodes"), bytes);

    let mut broken_magic = bytes.clone();
    broken_magic[0] ^= 0xff;
    assert_eq!(
        MeasuredSpendV1::from_canonical_bytes(&broken_magic).expect_err("refuses"),
        MalformedSpend::Magic
    );

    let mut trailing = bytes.clone();
    trailing.push(0x00);
    assert_eq!(
        MeasuredSpendV1::from_canonical_bytes(&trailing).expect_err("refuses"),
        MalformedSpend::TrailingBytes
    );

    let mut truncated = bytes.clone();
    truncated.pop();
    assert_eq!(
        MeasuredSpendV1::from_canonical_bytes(&truncated).expect_err("refuses"),
        MalformedSpend::Truncated
    );

    // The measurement tag sits after the magic, three UUIDs, a u32, the origin tag, and three u64s.
    let measurement_tag = MAGIC.len() + 16 * 3 + 4 + 1 + 8 * 3;
    let mut unknown_tag = bytes.clone();
    assert_eq!(unknown_tag[measurement_tag], 0x01);
    unknown_tag[measurement_tag] = 0x09;
    assert_eq!(
        MeasuredSpendV1::from_canonical_bytes(&unknown_tag).expect_err("refuses"),
        MalformedSpend::UnknownTag
    );

    let find = |needle: &[u8]| {
        bytes
            .windows(needle.len())
            .position(|window| window == needle)
            .expect("needle present")
    };

    let mut over_limit = bytes.clone();
    let unit = find(b"USD");
    over_limit[unit - 8..unit].copy_from_slice(&u64::MAX.to_be_bytes());
    assert_eq!(
        MeasuredSpendV1::from_canonical_bytes(&over_limit).expect_err("refuses"),
        MalformedSpend::OverLimitLength
    );

    let mut invalid_utf8 = bytes.clone();
    invalid_utf8[find(b"tokens")] = 0xff;
    assert_eq!(
        MeasuredSpendV1::from_canonical_bytes(&invalid_utf8).expect_err("refuses"),
        MalformedSpend::InvalidUtf8
    );

    // Non-canonical unit order is refused rather than silently re-sorted: two orders of one
    // measurement must not both decode. The pair is spliced rather than re-encoded, because the
    // encoder is the thing under test.
    let mut swapped = MAGIC.to_vec();
    swapped.extend_from_slice(&bytes[MAGIC.len()..measurement_tag + 1 + 8 + 4]);
    let mut tokens_line = Vec::new();
    push_string(&mut tokens_line, "tokens");
    tokens_line.extend_from_slice(&0_i64.to_be_bytes());
    tokens_line.push(0);
    let mut usd_line = Vec::new();
    push_string(&mut usd_line, "USD");
    usd_line.extend_from_slice(&123_456_i64.to_be_bytes());
    usd_line.push(2);
    swapped.extend_from_slice(&tokens_line);
    swapped.extend_from_slice(&usd_line);
    swapped.push(0x01);
    swapped.extend_from_slice(&[0xaa; 32]);
    assert_ne!(swapped, bytes);
    assert_eq!(
        MeasuredSpendV1::from_canonical_bytes(&swapped).expect_err("refuses"),
        MalformedSpend::UnitOrder
    );

    // A declared count is proved against the remaining input before an entry is read. The count
    // is the cap itself, so the cap cannot be what refuses it, and the first entry is malformed, so
    // a decoder that read entries first would answer `UnitCharset` instead.
    let mut hostile = bytes.clone();
    let line_count = measurement_tag + 1 + 8;
    assert_eq!(&hostile[line_count..line_count + 4], &2_u32.to_be_bytes());
    hostile[line_count..line_count + 4].copy_from_slice(&32_u32.to_be_bytes());
    hostile[find(b"USD")] = b'!';
    assert_eq!(
        MeasuredSpendV1::from_canonical_bytes(&hostile).expect_err("refuses"),
        MalformedSpend::Truncated,
        "a declared count was trusted before it was checked against the input"
    );
}

/// One named change to a record, applied to a clone so each mutation stands alone.
type Mutation = Box<dyn Fn(&mut MeasuredSpendV1)>;

#[test]
fn spend_digest_changes_for_each_identity_version_amount_unit_or_coverage_change() {
    let base = fixture();
    let mut seen = vec![base.digest().expect("digest").value];

    let mutations: Vec<(&str, Mutation)> = vec![
        (
            "workspace",
            Box::new(|record: &mut MeasuredSpendV1| record.workspace_id = Uuid::from_u128(0xfeed)),
        ),
        (
            "mission",
            Box::new(|record: &mut MeasuredSpendV1| record.mission_id = Uuid::from_u128(0xbeef)),
        ),
        (
            "spend reference identity",
            Box::new(|record: &mut MeasuredSpendV1| record.spend_id = Uuid::from_u128(0xcafe)),
        ),
        (
            "version",
            Box::new(|record: &mut MeasuredSpendV1| record.version = 3),
        ),
        (
            "origin",
            Box::new(|record: &mut MeasuredSpendV1| record.origin = SpendOrigin::OperatorDeclared),
        ),
        (
            "measured-at timestamp",
            Box::new(|record: &mut MeasuredSpendV1| record.measured_at_ms += 1),
        ),
        (
            "coverage start",
            Box::new(|record: &mut MeasuredSpendV1| record.coverage_from_event_seq = 0),
        ),
        (
            "coverage end",
            Box::new(|record: &mut MeasuredSpendV1| record.coverage_to_event_seq += 1),
        ),
        (
            "amount",
            Box::new(|record: &mut MeasuredSpendV1| {
                record.measurement =
                    SpendMeasurement::Measured(totals(vec![line("USD", 123_457, 2)], 412));
            }),
        ),
        (
            "minor-units exponent",
            Box::new(|record: &mut MeasuredSpendV1| {
                record.measurement =
                    SpendMeasurement::Measured(totals(vec![line("USD", 123_456, 3)], 412));
            }),
        ),
        (
            "unit",
            Box::new(|record: &mut MeasuredSpendV1| {
                record.measurement =
                    SpendMeasurement::Measured(totals(vec![line("EUR", 123_456, 2)], 412));
            }),
        ),
        (
            "counted units",
            Box::new(|record: &mut MeasuredSpendV1| {
                record.measurement =
                    SpendMeasurement::Measured(totals(vec![line("USD", 123_456, 2)], 413));
            }),
        ),
        (
            "measurement arm",
            Box::new(|record: &mut MeasuredSpendV1| {
                record.measurement = SpendMeasurement::Unmeasured("nobody counted.".to_owned());
            }),
        ),
        (
            "supersession",
            Box::new(|record: &mut MeasuredSpendV1| record.supersedes = SpendSupersedes::None),
        ),
    ];

    for (label, mutate) in &mutations {
        let mut record = base.clone();
        mutate(&mut record);
        let digest = record.digest().expect("digest").value;
        assert!(
            !seen.contains(&digest),
            "changing the {label} did not change the record's digest"
        );
        seen.push(digest);
    }
    assert_eq!(seen.len(), mutations.len() + 1);
}

#[test]
fn spend_coverage_refuses_a_window_that_ends_before_it_starts() {
    let mut backwards = fixture();
    backwards.coverage_from_event_seq = 4_210;
    backwards.coverage_to_event_seq = 4_209;
    assert_eq!(
        backwards.canonical_bytes().expect_err("refuses"),
        MalformedSpend::CoverageRange
    );

    // A window of one event is legal: a Mission can be measured over a single boundary.
    let mut single = fixture();
    single.coverage_from_event_seq = 4_210;
    single.coverage_to_event_seq = 4_210;
    let bytes = single.canonical_bytes().expect("an equal pair encodes");

    // And the decoder refuses it too, not only the encoder: bytes that never came from this
    // encoder must not decode into a record the encoder would have refused.
    let mut spliced = bytes.clone();
    let start = MAGIC.len() + 16 * 3 + 4 + 1 + 8;
    spliced[start + 8..start + 16].copy_from_slice(&4_209_u64.to_be_bytes());
    assert_eq!(
        MeasuredSpendV1::from_canonical_bytes(&spliced).expect_err("refuses"),
        MalformedSpend::CoverageRange
    );
}

#[test]
fn spend_partially_measured_requires_at_least_one_named_uncounted_entry() {
    let gapless = with(SpendMeasurement::PartiallyMeasured(
        totals(vec![line("USD", 1, 2)], 1),
        Vec::new(),
    ));
    assert_eq!(
        gapless.canonical_bytes().expect_err("refuses"),
        MalformedSpend::NoUncountedEntry,
        "partial with no named gap is measured, and must not be spelled partial"
    );

    let named = with(SpendMeasurement::PartiallyMeasured(
        totals(vec![line("USD", 1, 2)], 1),
        vec![UncountedEntry {
            kind: UncountedKind::ProviderDidNotPrice,
            detail: "three calls returned no price.".to_owned(),
        }],
    ));
    let bytes = named.canonical_bytes().expect("one named gap encodes");
    let decoded = MeasuredSpendV1::from_canonical_bytes(&bytes).expect("decodes");
    match decoded.measurement {
        SpendMeasurement::PartiallyMeasured(_, gaps) => {
            assert_eq!(gaps.len(), 1);
            assert_eq!(gaps[0].kind.as_str(), "provider_did_not_price");
        }
        other => panic!("expected partially_measured, got {}", other.literal()),
    }

    // The gap vocabulary is closed: an unassigned kind tag is refused, not defaulted.
    let mut unknown = bytes.clone();
    let position = bytes
        .windows(30)
        .position(|window| window.starts_with(b"\x01\x00\x00\x00\x00\x00\x00\x00\x1e"))
        .expect("the entry's kind tag and detail length are in the bytes");
    unknown[position] = 0x09;
    assert_eq!(
        MeasuredSpendV1::from_canonical_bytes(&unknown).expect_err("refuses"),
        MalformedSpend::UnknownTag
    );
}

#[test]
fn spend_bounds_each_name_themselves_and_are_distinct_from_absence() {
    let long_unit = "u".repeat(17);
    let long_text = "x".repeat(2_001);
    let big_detail = "y".repeat(2_000);

    let over: Vec<(&str, MeasuredSpendV1, MalformedSpend)> = vec![
        (
            "lines",
            with(SpendMeasurement::Measured(totals(
                (0..33)
                    .map(|index| line(&format!("u{index:02}"), 1, 0))
                    .collect(),
                33,
            ))),
            MalformedSpend::TooManyLines,
        ),
        (
            "uncounted entries",
            with(SpendMeasurement::PartiallyMeasured(
                totals(vec![line("USD", 1, 2)], 1),
                (0..65)
                    .map(|index| UncountedEntry {
                        kind: UncountedKind::OutsideRetention,
                        detail: format!("gap {index}."),
                    })
                    .collect(),
            )),
            MalformedSpend::TooManyUncountedEntries,
        ),
        (
            "unit length",
            with(SpendMeasurement::Measured(totals(
                vec![line(&long_unit, 1, 0)],
                1,
            ))),
            MalformedSpend::UnitTooLong,
        ),
        (
            "text length",
            with(SpendMeasurement::Unmeasured(long_text.clone())),
            MalformedSpend::TextTooLong,
        ),
        (
            "minor-units exponent",
            with(SpendMeasurement::Measured(totals(
                vec![line("USD", 1, 10)],
                1,
            ))),
            MalformedSpend::MinorUnitsExponent,
        ),
        (
            "record envelope",
            with(SpendMeasurement::PartiallyMeasured(
                totals(vec![line("USD", 1, 2)], 1),
                (0..64)
                    .map(|_| UncountedEntry {
                        kind: UncountedKind::OutsideRetention,
                        detail: big_detail.clone(),
                    })
                    .collect(),
            )),
            MalformedSpend::RecordTooLarge,
        ),
    ];

    let mut names = Vec::new();
    for (label, record, expected) in &over {
        assert_eq!(
            record.canonical_bytes().expect_err("refuses"),
            *expected,
            "the {label} bound did not name itself"
        );
        names.push(expected.as_str());
    }
    let mut distinct = names.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), names.len(), "two bounds share a name");

    // Absence of a refusal is distinguishable from every one of them: a record just under each
    // bound encodes. Without this, deleting a bound and deleting its test look the same.
    let under: Vec<(&str, MeasuredSpendV1)> = vec![
        (
            "lines",
            with(SpendMeasurement::Measured(totals(
                (0..32)
                    .map(|index| line(&format!("u{index:02}"), 1, 0))
                    .collect(),
                32,
            ))),
        ),
        (
            "uncounted entries",
            with(SpendMeasurement::PartiallyMeasured(
                totals(vec![line("USD", 1, 2)], 1),
                (0..64)
                    .map(|index| UncountedEntry {
                        kind: UncountedKind::OutsideRetention,
                        detail: format!("gap {index}."),
                    })
                    .collect(),
            )),
        ),
        (
            "unit length",
            with(SpendMeasurement::Measured(totals(
                vec![line(&"u".repeat(16), 1, 0)],
                1,
            ))),
        ),
        (
            "text length",
            with(SpendMeasurement::Unmeasured("x".repeat(2_000))),
        ),
        (
            "minor-units exponent",
            with(SpendMeasurement::Measured(totals(
                vec![line("USD", 1, 9)],
                1,
            ))),
        ),
    ];
    for (label, record) in &under {
        let bytes = record
            .canonical_bytes()
            .unwrap_or_else(|refusal| panic!("just under the {label} bound refused: {refusal}"));
        assert!(MeasuredSpendV1::from_canonical_bytes(&bytes).is_ok());
    }

    // The per-field limits alone admit far more than the store's envelope, so the owned total bound
    // is load-bearing rather than decorative: 64 gaps at the text limit is about eight times it.
    let largest_by_field_limits = 64 * (1 + 8 + 2_000);
    assert!(
        largest_by_field_limits > 65_536,
        "the record envelope would be unreachable, and its test would measure nothing"
    );
}

#[test]
fn spend_has_no_cross_unit_total_and_no_within_budget_predicate() {
    // Two units stay two lines through a round trip. Nothing sums them, because Beacon holds no
    // rate it could freeze and a summed figure is one no ledger can reproduce.
    let bytes = fixture().canonical_bytes().expect("encodes");
    let decoded = MeasuredSpendV1::from_canonical_bytes(&bytes).expect("decodes");
    let counted = decoded.measurement.totals().expect("totals");
    assert_eq!(counted.lines.len(), 2);
    assert_ne!(counted.lines[0].unit, counted.lines[1].unit);
    assert_eq!(counted.lines[0].minor_units_exponent, 2);
    assert_eq!(counted.lines[1].minor_units_exponent, 0);

    // And the module offers no shortcut past them. This reads the source because "there is
    // deliberately no X" is otherwise a sentence in a comment: adding one of these turns it red.
    let module = include_str!("../src/spend.rs");
    // `fn totals(` is the accessor that returns the lines; `fn total(` would be the one that
    // collapses them, which is the difference this list is about.
    for forbidden in [
        "fn total(",
        "fn sum(",
        "fn is_within_budget",
        "fn within_budget",
        "fn amount(",
        "fn overspent",
    ] {
        assert!(
            !module.contains(forbidden),
            "the module grew `{forbidden}`, which is the single number a consumer would read \
             instead of the check it owes"
        );
    }
}

/// ADR 0042 publishes this record's byte count and digest. Pinning them here makes those numbers
/// load-bearing: a layout change fails a named test instead of leaving a document that quietly
/// describes a grammar the code no longer has.
#[test]
fn a_known_spend_fixture_reproduces_its_published_digest() {
    let bytes = fixture().canonical_bytes().expect("encodes");
    assert_eq!(bytes.len(), 182, "ADR 0042 publishes 182 canonical bytes");
    assert_eq!(
        fixture().digest().expect("digest").value,
        "9461d5deeba593e027dae523f3040515feea0ab5cb7b35cbf120a119ff7e774b"
    );

    // The envelope travels with the value: a digest without the function that produced it names
    // nothing.
    let digest = fixture().digest().expect("digest");
    assert_eq!(digest.algorithm, "sha256");
    assert_eq!(digest.canonicalizer, "beacon.spend.v1");
}
