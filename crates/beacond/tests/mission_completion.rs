//! `MissionCompletionV1` slice 2 — the record, encode/decode only.
//!
//! One test per bullet of `A-opus-mission-completion-design.md` §12 "Slice 2". No store, no
//! migration, no verified reader: every check here is one the bytes alone can settle, and every
//! check that would compare this record against the Mission it names is the next slice's.

use beacond::{
    CompletionActor, CompletionDisposition, CompletionSupersedes, ConditionOutcome,
    ConditionResultRecord, MalformedCompletion, MissionCompletionV1, NarrativeItem, SpendReference,
    SupportRef, UncertaintyKind, UnfinishedKind,
};
use uuid::Uuid;

const MAGIC: &[u8] = b"beacon.mission_completion.v1\0";

const WORKSPACE: &str = "00000000-0000-4000-8000-000000000001";
const PROJECT: &str = "00000000-0000-4000-8000-000000000002";
const MISSION: &str = "00000000-0000-4000-8000-000000000003";
const COMPLETION: &str = "00000000-0000-4000-8000-000000000004";
const RESIDENT: &str = "00000000-0000-4000-8000-000000000005";
const CONFIGURATION: &str = "00000000-0000-4000-8000-000000000006";
const SPEND: &str = "00000000-0000-4000-8000-000000000007";
const RUN: &str = "00000000-0000-4000-8000-000000000008";

const BOUNDARY: u64 = 4_210;
const COMPLETED_AT_MS: u64 = 1_788_400_000_000;

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("canonical uuid")
}

fn digest(byte: u8) -> [u8; 32] {
    [byte; 32]
}

/// A completion that exercises every arm the grammar has: both outcomes with support and without,
/// a note present and absent, three support kinds, a delivered Artifact, one unfinished item, one
/// uncertainty item, a recorded spend, and a superseded predecessor.
fn fixture() -> MissionCompletionV1 {
    MissionCompletionV1 {
        workspace_id: uuid(WORKSPACE),
        project_id: uuid(PROJECT),
        mission_id: uuid(MISSION),
        completion_id: uuid(COMPLETION),
        actor: CompletionActor::Operator,
        accountable_resident_id: uuid(RESIDENT),
        accountable_resident_configuration_id: uuid(CONFIGURATION),
        boundary_event_seq: BOUNDARY,
        disposition: CompletionDisposition::Concluded,
        declared_conditions_sha256: digest(0x11),
        condition_results: vec![
            ConditionResultRecord {
                ordinal: 0,
                outcome: ConditionOutcome::Met,
                support: vec![
                    SupportRef::Artifact(digest(0x22)),
                    SupportRef::Receipt {
                        run_id: uuid(RUN),
                        receipt_version: 3,
                        receipt_bytes_sha256: digest(0x33),
                    },
                    SupportRef::Evaluation(digest(0x44)),
                ],
                note: Some("Every declared gate passed.".to_owned()),
            },
            ConditionResultRecord {
                ordinal: 1,
                outcome: ConditionOutcome::AssessmentInconclusive,
                support: vec![],
                note: None,
            },
        ],
        delivered_artifacts: vec![digest(0x55), digest(0x66)],
        unfinished_work: vec![NarrativeItem {
            ordinal: 0,
            kind: UnfinishedKind::DeferredScope,
            condition_ordinal: Some(1),
            summary: "The second condition's probe was deferred.".to_owned(),
        }],
        uncertainty: vec![NarrativeItem {
            ordinal: 0,
            kind: UncertaintyKind::UnmeasuredQuantity,
            condition_ordinal: None,
            summary: "Nothing measured model spend for this window.".to_owned(),
        }],
        spend: SpendReference::Recorded {
            spend_id: uuid(SPEND),
            spend_version: 2,
            spend_sha256: digest(0x77),
        },
        supersedes: CompletionSupersedes::Record(digest(0x88)),
        summary: "One reviewed feature shipped; the second condition is unresolved.".to_owned(),
        completed_at_ms: COMPLETED_AT_MS,
    }
}

/// The same record with every optional arm on its other side: `not_measured` spend, no predecessor,
/// no support, no note, no narrative.
fn minimal() -> MissionCompletionV1 {
    MissionCompletionV1 {
        condition_results: vec![ConditionResultRecord {
            ordinal: 0,
            outcome: ConditionOutcome::NotAssessed,
            support: vec![],
            note: None,
        }],
        delivered_artifacts: vec![],
        unfinished_work: vec![],
        uncertainty: vec![],
        spend: SpendReference::NotMeasured("No spend record exists for this Mission.".to_owned()),
        supersedes: CompletionSupersedes::None,
        disposition: CompletionDisposition::Abandoned,
        ..fixture()
    }
}

// ---------------------------------------------------------------------------------------------
// An independent encoder: the module's published field table, not the module's encoder
// ---------------------------------------------------------------------------------------------

fn push_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u64).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

/// The fixture's bytes, assembled from the field table in `mission_completion`'s module docs and
/// the fixture's declared values, without calling the encoder.
fn fixture_bytes_by_hand() -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    for id in [WORKSPACE, PROJECT, MISSION, COMPLETION] {
        out.extend_from_slice(uuid(id).as_bytes());
    }
    out.push(0x01); // actor = operator
    out.extend_from_slice(uuid(RESIDENT).as_bytes());
    out.extend_from_slice(uuid(CONFIGURATION).as_bytes());
    out.extend_from_slice(&BOUNDARY.to_be_bytes());
    out.push(0x01); // disposition = concluded
    out.extend_from_slice(&digest(0x11));

    out.extend_from_slice(&2_u32.to_be_bytes()); // condition count
    // condition 0
    out.extend_from_slice(&0_u32.to_be_bytes());
    out.push(0x01); // met
    out.extend_from_slice(&3_u32.to_be_bytes()); // support count
    out.push(0x01);
    out.extend_from_slice(&digest(0x22));
    out.push(0x02);
    out.extend_from_slice(uuid(RUN).as_bytes());
    out.extend_from_slice(&3_u32.to_be_bytes());
    out.extend_from_slice(&digest(0x33));
    out.push(0x03);
    out.extend_from_slice(&digest(0x44));
    out.push(0x01); // note = some
    push_string(&mut out, "Every declared gate passed.");
    // condition 1
    out.extend_from_slice(&1_u32.to_be_bytes());
    out.push(0x04); // assessment_inconclusive
    out.extend_from_slice(&0_u32.to_be_bytes());
    out.push(0x00); // note = none

    out.extend_from_slice(&2_u32.to_be_bytes()); // delivered artifacts
    out.extend_from_slice(&digest(0x55));
    out.extend_from_slice(&digest(0x66));

    out.extend_from_slice(&1_u32.to_be_bytes()); // unfinished work
    out.extend_from_slice(&0_u32.to_be_bytes());
    out.push(0x02); // deferred_scope
    out.push(0x01); // condition_ordinal = some
    out.extend_from_slice(&1_u32.to_be_bytes());
    push_string(&mut out, "The second condition's probe was deferred.");

    out.extend_from_slice(&1_u32.to_be_bytes()); // uncertainty
    out.extend_from_slice(&0_u32.to_be_bytes());
    out.push(0x02); // unmeasured_quantity
    out.push(0x00); // condition_ordinal = none
    push_string(&mut out, "Nothing measured model spend for this window.");

    out.push(0x01); // spend = recorded
    out.extend_from_slice(uuid(SPEND).as_bytes());
    out.extend_from_slice(&2_u32.to_be_bytes());
    out.extend_from_slice(&digest(0x77));

    out.push(0x01); // supersedes = record
    out.extend_from_slice(&digest(0x88));

    push_string(
        &mut out,
        "One reviewed feature shipped; the second condition is unresolved.",
    );
    out.extend_from_slice(&COMPLETED_AT_MS.to_be_bytes());
    out
}

// ---------------------------------------------------------------------------------------------
// One test per design bullet
// ---------------------------------------------------------------------------------------------

#[test]
fn the_canonical_vector_is_exactly_these_bytes_and_a_second_encoder_reproduces_them() {
    // Bullet 1. The pinned literals, and -- the part that makes them evidence -- a second assembly
    // of the same record from the published field table without calling the encoder. A grammar
    // described but not published has no identity: a reviewer writing an encoder from a field list
    // alone must land on these bytes.
    let bytes = fixture().canonical_bytes().expect("the fixture encodes");
    assert_eq!(bytes.len(), 707, "the published canonical byte count");
    assert_eq!(
        fixture().digest().expect("digest").value,
        "705034373a787f7936d2bc2d086f5ef0295bbea5aa02ef3ccc66fb8deb951dfe",
        "the published SHA-256"
    );
    assert_eq!(
        bytes,
        fixture_bytes_by_hand(),
        "the encoder and the published table disagree"
    );

    // The envelope travels with the value: a digest without the function that produced it names
    // nothing.
    let digest = fixture().digest().unwrap();
    assert_eq!(digest.algorithm, "sha256");
    assert_eq!(digest.canonicalizer, "beacon.mission_completion.v1");
}

#[test]
fn every_closed_vocabulary_refuses_an_unassigned_tag() {
    // Bullet 2, including `0x00` where absence is not a value and `0x05` in `outcome` -- the
    // reserved-but-unassigned `unsupported_reference`, which must be refused rather than quietly
    // accepted by a reader that has not heard of it.
    let bytes = fixture().canonical_bytes().unwrap();
    let at = |needle: &[u8], skip: usize| -> usize {
        bytes
            .windows(needle.len())
            .enumerate()
            .filter(|(_, window)| *window == needle)
            .map(|(index, _)| index)
            .nth(skip)
            .expect("the tag is in the vector")
    };

    // Each entry: a byte offset that holds a closed tag, and a value that vocabulary never assigns.
    let actor_at = MAGIC.len() + 16 * 4;
    let disposition_at = actor_at + 1 + 16 * 2 + 8;
    let outcome_at = disposition_at + 1 + 32 + 4 + 4;
    for (offset, unassigned, what) in [
        (actor_at, 0x02_u8, "actor"),
        (disposition_at, 0x03, "disposition"),
        (
            outcome_at,
            0x05,
            "outcome (the reserved unsupported_reference)",
        ),
        (outcome_at, 0x00, "outcome (zero is not a value here)"),
    ] {
        let mut tampered = bytes.clone();
        tampered[offset] = unassigned;
        assert_eq!(
            MissionCompletionV1::from_canonical_bytes(&tampered),
            Err(MalformedCompletion::UnknownTag),
            "{what} accepted an unassigned tag"
        );
    }

    // ...and the same for the support kind, found by its own position rather than computed.
    let support_kind_at = at(&digest(0x22), 0) - 1;
    let mut tampered = bytes.clone();
    tampered[support_kind_at] = 0x05;
    assert_eq!(
        MissionCompletionV1::from_canonical_bytes(&tampered),
        Err(MalformedCompletion::UnknownTag),
        "support kind accepted an unassigned tag"
    );
}

#[test]
fn the_decoder_refuses_malformed_input_by_name() {
    // Bullet 3: trailing bytes, a short buffer, a count exceeding the remaining input, invalid
    // UTF-8, and non-ascending condition results. Five faults, five names -- one name for all five
    // would tell a caller nothing about which it hit.
    let bytes = fixture().canonical_bytes().unwrap();

    let mut trailing = bytes.clone();
    trailing.push(0x00);
    assert_eq!(
        MissionCompletionV1::from_canonical_bytes(&trailing),
        Err(MalformedCompletion::TrailingBytes)
    );

    assert_eq!(
        MissionCompletionV1::from_canonical_bytes(&bytes[..bytes.len() - 1]),
        Err(MalformedCompletion::Truncated)
    );

    // A declared condition count inside the field's limit that the remaining input cannot hold.
    // Refused before the first entry is read and before anything reserves capacity for it -- so the
    // buffer is cut off right after the count, and twenty conditions cannot fit in zero bytes.
    let count_at = MAGIC.len() + 16 * 4 + 1 + 16 * 2 + 8 + 1 + 32;
    let mut over = bytes[..count_at + 4].to_vec();
    over[count_at..count_at + 4].copy_from_slice(&20_u32.to_be_bytes());
    assert_eq!(
        MissionCompletionV1::from_canonical_bytes(&over),
        Err(MalformedCompletion::Truncated)
    );

    // A count past the field's own limit is a different refusal from one the input cannot hold.
    let mut over_limit = bytes[..count_at + 4].to_vec();
    over_limit[count_at..count_at + 4].copy_from_slice(&21_u32.to_be_bytes());
    assert_eq!(
        MissionCompletionV1::from_canonical_bytes(&over_limit),
        Err(MalformedCompletion::OverLimitLength)
    );

    // Invalid UTF-8 inside the summary.
    let summary = "One reviewed feature shipped; the second condition is unresolved.";
    let summary_at = bytes
        .windows(summary.len())
        .position(|window| window == summary.as_bytes())
        .expect("the summary is in the vector");
    let mut invalid = bytes.clone();
    invalid[summary_at] = 0xff;
    assert_eq!(
        MissionCompletionV1::from_canonical_bytes(&invalid),
        Err(MalformedCompletion::InvalidUtf8)
    );

    // Non-ascending condition ordinals: the ordinal IS the condition's identity, so a record whose
    // results are out of order is a record about different conditions.
    let mut swapped = fixture();
    swapped.condition_results.swap(0, 1);
    assert_eq!(
        swapped.canonical_bytes(),
        Err(MalformedCompletion::ConditionOrdinals)
    );
}

#[test]
fn a_record_inside_every_field_bound_but_over_the_envelope_is_refused_by_the_encoder() {
    // Bullet 4. The per-field limits admit roughly 188,000 bytes -- 20 conditions with 500-scalar
    // notes, plus 32 unfinished and 32 uncertainty items at 500 scalars each -- so a grammar with
    // only per-field bounds is not total. This is refused by the ENCODER, before anything reaches a
    // store, and every value in it is individually legal.
    // 500 SCALARS of a four-byte character, not 500 ASCII bytes. The per-field bounds are in
    // Unicode scalars and the envelope is in bytes, so a test that used ASCII would build a 45 KiB
    // record and prove nothing about a 64 KiB bound.
    let long = "\u{1D11E}".repeat(500);
    let mut record = fixture();
    record.condition_results = (0..20)
        .map(|ordinal| ConditionResultRecord {
            ordinal,
            outcome: ConditionOutcome::Met,
            support: vec![],
            note: Some(long.clone()),
        })
        .collect();
    record.unfinished_work = (0..32)
        .map(|ordinal| NarrativeItem {
            ordinal,
            kind: UnfinishedKind::KnownDefect,
            condition_ordinal: None,
            summary: long.clone(),
        })
        .collect();
    record.uncertainty = (0..32)
        .map(|ordinal| NarrativeItem {
            ordinal,
            kind: UncertaintyKind::UnverifiedClaim,
            condition_ordinal: None,
            summary: long.clone(),
        })
        .collect();
    record.summary = "\u{1D11E}".repeat(2_000);

    assert_eq!(
        record.canonical_bytes(),
        Err(MalformedCompletion::RecordTooLarge),
        "a record every field bound accepts must still be refused by the envelope"
    );

    // The control: the same shape one narrative item smaller encodes, so the refusal above is the
    // envelope and not some per-field limit firing first.
    let mut smaller = record.clone();
    smaller.condition_results.truncate(2);
    smaller.unfinished_work.truncate(1);
    smaller.uncertainty.truncate(1);
    assert!(smaller.canonical_bytes().is_ok());
}

#[test]
fn absence_encodes_as_a_value_and_a_missing_field_is_refused() {
    // Bullet 5. `note = none`, `condition_ordinal = none`, `supersedes = none` and
    // `spend = not_measured` are all VALUES: they occupy a byte. A decoder handed the record with
    // that byte removed must refuse rather than infer the absence.
    let bytes = minimal()
        .canonical_bytes()
        .expect("the minimal record encodes");
    assert_eq!(
        MissionCompletionV1::from_canonical_bytes(&bytes).unwrap(),
        minimal()
    );

    // Every `none` tag is a 0x00 byte in the stream. Removing any one of them shifts everything
    // after it, and the decoder must not accept the result.
    let zero_positions: Vec<usize> = bytes
        .iter()
        .enumerate()
        .filter(|(index, byte)| **byte == 0x00 && *index > MAGIC.len() + 16 * 4)
        .map(|(index, _)| index)
        .collect();
    assert!(
        !zero_positions.is_empty(),
        "the minimal record should carry absence tags"
    );
    for position in zero_positions {
        let mut without = bytes.clone();
        without.remove(position);
        assert!(
            MissionCompletionV1::from_canonical_bytes(&without).is_err(),
            "the decoder inferred an absence from a byte that was not there (offset {position})"
        );
    }
}

#[test]
fn decode_and_re_encode_are_byte_identical_for_every_arm() {
    // Bullet 6. Both fixtures together cover every tag arm the grammar has, and an exact round trip
    // is what proves a caller's bytes are the record's bytes -- the property that lets a store hold
    // one and a reader prove it.
    for record in [fixture(), minimal()] {
        let bytes = record.canonical_bytes().expect("encodes");
        let decoded = MissionCompletionV1::from_canonical_bytes(&bytes).expect("decodes");
        assert_eq!(decoded, record);
        assert_eq!(decoded.canonical_bytes().unwrap(), bytes);
        assert_eq!(decoded.digest().unwrap(), record.digest().unwrap());
    }
}
