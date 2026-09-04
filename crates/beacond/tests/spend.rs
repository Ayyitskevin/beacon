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
