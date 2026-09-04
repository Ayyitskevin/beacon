//! Mission impact classification v1, slice 1 (ADR 0044): pure types and canonical bytes.
//!
//! Every fixture is constructed. Slice 1 owns no database, no service and no event.
//!
//! The canonical bytes are assembled twice: once by Beacon's encoder and once by
//! [`fixture_bytes_by_hand`], which follows ADR 0044's published table rather than calling that
//! encoder. A canonicalizer that quietly changed its layout would agree with itself and disagree
//! with this file.

use beacond::{
    ClassificationActor, ClassificationBasis, ClassificationSupersedes, ImpactLevel,
    MalformedClassification, MissionImpactClassificationV1, PolicyRevisionRef,
};
use uuid::Uuid;

const MAGIC: &[u8] = b"beacon.mission-impact.v1\0";

const WORKSPACE: &str = "00000000-0000-4000-8000-000000000001";
const MISSION: &str = "00000000-0000-4000-8000-000000000002";
const CLASSIFICATION: &str = "00000000-0000-4000-8000-000000000003";
const PROJECT: &str = "00000000-0000-4000-8000-000000000004";

const CLASSIFIED_AT_MS: u64 = 1_788_372_724_000;
const REASON: &str = "The Mission dispatches into the Operator's own repository.";

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("canonical uuid")
}

fn fixture() -> MissionImpactClassificationV1 {
    MissionImpactClassificationV1 {
        workspace_id: uuid(WORKSPACE),
        mission_id: uuid(MISSION),
        classification_id: uuid(CLASSIFICATION),
        version: 2,
        impact: ImpactLevel::High,
        basis: ClassificationBasis::OperatorDeclared,
        policy: PolicyRevisionRef {
            project_id: uuid(PROJECT),
            policy_revision: 3,
            charter_revision: 5,
        },
        rule_id: None,
        reason: REASON.to_owned(),
        classified_by: ClassificationActor::Operator,
        classified_at_ms: CLASSIFIED_AT_MS,
        supersedes: ClassificationSupersedes::Record("bb".repeat(32)),
    }
}

fn push_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u64).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

fn fixture_bytes_by_hand() -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(uuid(WORKSPACE).as_bytes());
    out.extend_from_slice(uuid(MISSION).as_bytes());
    out.extend_from_slice(uuid(CLASSIFICATION).as_bytes());
    out.extend_from_slice(&2_u32.to_be_bytes());
    out.push(0x01); // impact = high
    out.push(0x01); // basis = operator_declared
    out.extend_from_slice(uuid(PROJECT).as_bytes());
    out.extend_from_slice(&3_u32.to_be_bytes());
    out.extend_from_slice(&5_u32.to_be_bytes());
    out.extend_from_slice(&0_u64.to_be_bytes()); // rule_id absent
    push_string(&mut out, REASON);
    out.push(0x01); // classified_by = operator
    out.extend_from_slice(&CLASSIFIED_AT_MS.to_be_bytes());
    out.push(0x01); // supersedes = record
    out.extend_from_slice(&[0xbb; 32]);
    out
}

// ---------------------------------------------------------------------------------------------
// The five load-bearing tests of the design's slice P1
// ---------------------------------------------------------------------------------------------

#[test]
fn impact_level_has_exactly_two_arms_and_no_unclassified_default() {
    // Both arms encode and round-trip, and they are two different documents.
    let mut high = fixture();
    high.impact = ImpactLevel::High;
    let mut ordinary = fixture();
    ordinary.impact = ImpactLevel::Ordinary;

    for record in [&high, &ordinary] {
        let bytes = record.canonical_bytes().expect("encodes");
        let decoded = MissionImpactClassificationV1::from_canonical_bytes(&bytes).expect("decodes");
        assert_eq!(&decoded, record);
    }
    assert_ne!(
        high.digest().expect("digest").value,
        ordinary.digest().expect("digest").value
    );
    assert_eq!(ImpactLevel::High.as_str(), "high");
    assert_eq!(ImpactLevel::Ordinary.as_str(), "ordinary");

    // The bit is reachable only through the record, so a caller cannot hold it without the identity
    // and digest the manifest must cite beside it (R13.7).
    assert!(high.appendix_is_mandatory());
    assert!(!ordinary.appendix_is_mandatory());

    // There is no third arm, and in particular no tag that could mean "not decided". `0x00` is the
    // one a default would most plausibly take, and it is refused like any other.
    let bytes = high.canonical_bytes().expect("encodes");
    let impact_tag = MAGIC.len() + 16 * 3 + 4;
    assert_eq!(bytes[impact_tag], 0x01);
    for unassigned in [0x00_u8, 0x03, 0x04, 0xff] {
        let mut third = bytes.clone();
        third[impact_tag] = unassigned;
        assert_eq!(
            MissionImpactClassificationV1::from_canonical_bytes(&third).expect_err("refuses"),
            MalformedClassification::UnknownTag,
            "impact tag {unassigned:#04x} was not refused"
        );
    }
}

#[test]
fn impact_classification_canonical_bytes_follow_adr_0034_0037_strict_round_trip_rules() {
    let record = fixture();
    let bytes = record.canonical_bytes().expect("encodes");

    assert_eq!(
        bytes,
        fixture_bytes_by_hand(),
        "Beacon's encoder and ADR 0044's published table disagree"
    );

    let decoded = MissionImpactClassificationV1::from_canonical_bytes(&bytes).expect("decodes");
    assert_eq!(decoded, record);
    assert_eq!(decoded.canonical_bytes().expect("re-encodes"), bytes);

    let mut broken_magic = bytes.clone();
    broken_magic[0] ^= 0xff;
    assert_eq!(
        MissionImpactClassificationV1::from_canonical_bytes(&broken_magic).expect_err("refuses"),
        MalformedClassification::Magic
    );

    let mut trailing = bytes.clone();
    trailing.push(0x00);
    assert_eq!(
        MissionImpactClassificationV1::from_canonical_bytes(&trailing).expect_err("refuses"),
        MalformedClassification::TrailingBytes
    );

    let mut truncated = bytes.clone();
    truncated.pop();
    assert_eq!(
        MissionImpactClassificationV1::from_canonical_bytes(&truncated).expect_err("refuses"),
        MalformedClassification::Truncated
    );

    let find = |needle: &[u8]| {
        bytes
            .windows(needle.len())
            .position(|window| window == needle)
            .expect("needle present")
    };

    let mut over_limit = bytes.clone();
    let reason = find(REASON.as_bytes());
    over_limit[reason - 8..reason].copy_from_slice(&u64::MAX.to_be_bytes());
    assert_eq!(
        MissionImpactClassificationV1::from_canonical_bytes(&over_limit).expect_err("refuses"),
        MalformedClassification::OverLimitLength
    );

    let mut invalid_utf8 = bytes.clone();
    invalid_utf8[reason] = 0xff;
    assert_eq!(
        MissionImpactClassificationV1::from_canonical_bytes(&invalid_utf8).expect_err("refuses"),
        MalformedClassification::InvalidUtf8
    );

    // Revisions are contiguous from one, in both directions.
    let mut zero_policy = record.clone();
    zero_policy.policy.policy_revision = 0;
    assert_eq!(
        zero_policy.canonical_bytes().expect_err("refuses"),
        MalformedClassification::Revision
    );
    let mut spliced = bytes.clone();
    let policy_revision = MAGIC.len() + 16 * 3 + 4 + 1 + 1 + 16;
    spliced[policy_revision..policy_revision + 4].copy_from_slice(&0_u32.to_be_bytes());
    assert_eq!(
        MissionImpactClassificationV1::from_canonical_bytes(&spliced).expect_err("refuses"),
        MalformedClassification::Revision
    );

    // A superseded digest that is not lowercase 64-character hexadecimal is refused by name.
    let mut bad_digest = record.clone();
    bad_digest.supersedes = ClassificationSupersedes::Record("BB".repeat(32));
    assert_eq!(
        bad_digest.canonical_bytes().expect_err("refuses"),
        MalformedClassification::SupersedesDigest
    );
}

#[test]
fn impact_digest_changes_for_each_identity_version_impact_basis_or_policy_revision_change() {
    let base = fixture();
    let mut seen = vec![base.digest().expect("digest").value];

    let mutations: Vec<(&str, Mutation)> = vec![
        (
            "workspace",
            Box::new(|r: &mut MissionImpactClassificationV1| {
                r.workspace_id = Uuid::from_u128(0xfe)
            }),
        ),
        (
            "mission",
            Box::new(|r: &mut MissionImpactClassificationV1| r.mission_id = Uuid::from_u128(0xbe)),
        ),
        (
            "classification reference identity",
            Box::new(|r: &mut MissionImpactClassificationV1| {
                r.classification_id = Uuid::from_u128(0xca)
            }),
        ),
        (
            "version",
            Box::new(|r: &mut MissionImpactClassificationV1| r.version = 3),
        ),
        (
            "impact",
            Box::new(|r: &mut MissionImpactClassificationV1| r.impact = ImpactLevel::Ordinary),
        ),
        (
            "basis",
            Box::new(|r: &mut MissionImpactClassificationV1| {
                r.basis = ClassificationBasis::PolicyRule;
                r.rule_id = Some("high-impact-repository-write".to_owned());
            }),
        ),
        (
            "policy project",
            Box::new(|r: &mut MissionImpactClassificationV1| {
                r.policy.project_id = Uuid::from_u128(0xda)
            }),
        ),
        (
            "policy revision",
            Box::new(|r: &mut MissionImpactClassificationV1| r.policy.policy_revision = 4),
        ),
        (
            "charter revision",
            Box::new(|r: &mut MissionImpactClassificationV1| r.policy.charter_revision = 6),
        ),
        (
            "reason",
            Box::new(|r: &mut MissionImpactClassificationV1| {
                r.reason = "A different stated reason.".to_owned()
            }),
        ),
        (
            "classified-at timestamp",
            Box::new(|r: &mut MissionImpactClassificationV1| r.classified_at_ms += 1),
        ),
        (
            "supersession",
            Box::new(|r: &mut MissionImpactClassificationV1| {
                r.supersedes = ClassificationSupersedes::None
            }),
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

/// One named change to a record, applied to a clone so each mutation stands alone.
type Mutation = Box<dyn Fn(&mut MissionImpactClassificationV1)>;

#[test]
fn impact_rule_id_is_required_for_policy_rule_and_refused_for_operator_declared() {
    // The grammar has the `policy_rule` arm; the Command path is slice 2's and refuses it until a
    // rule engine exists. That is ADR 0034's `bound`-descriptor pattern: define the tag, refuse it
    // at submission.
    let mut ruled = fixture();
    ruled.basis = ClassificationBasis::PolicyRule;
    ruled.rule_id = Some("high-impact-repository-write".to_owned());
    let bytes = ruled.canonical_bytes().expect("a named rule encodes");
    let decoded = MissionImpactClassificationV1::from_canonical_bytes(&bytes).expect("decodes");
    assert_eq!(decoded.basis, ClassificationBasis::PolicyRule);
    assert_eq!(
        decoded.rule_id.as_deref(),
        Some("high-impact-repository-write")
    );

    // A policy rule that names none cannot be audited.
    let mut anonymous = fixture();
    anonymous.basis = ClassificationBasis::PolicyRule;
    anonymous.rule_id = None;
    assert_eq!(
        anonymous.canonical_bytes().expect_err("refuses"),
        MalformedClassification::RuleIdBasisMismatch
    );

    // An Operator declaration that names one claims a provenance it does not have.
    let mut borrowed = fixture();
    borrowed.rule_id = Some("high-impact-repository-write".to_owned());
    assert_eq!(
        borrowed.canonical_bytes().expect_err("refuses"),
        MalformedClassification::RuleIdBasisMismatch
    );

    // Absence encodes as a value, so an empty rule id is not a second spelling of absence.
    let mut empty = fixture();
    empty.basis = ClassificationBasis::PolicyRule;
    empty.rule_id = Some(String::new());
    assert_eq!(
        empty.canonical_bytes().expect_err("refuses"),
        MalformedClassification::RuleId
    );

    // And the decoder enforces the pairing too, not only the encoder: bytes that never came from
    // this encoder must not decode into a record it would have refused.
    let mut mismatched = bytes.clone();
    let basis_tag = MAGIC.len() + 16 * 3 + 4 + 1;
    assert_eq!(mismatched[basis_tag], 0x02);
    mismatched[basis_tag] = 0x01;
    assert_eq!(
        MissionImpactClassificationV1::from_canonical_bytes(&mismatched).expect_err("refuses"),
        MalformedClassification::RuleIdBasisMismatch
    );

    for bad in ["Rule-One", "rule one", "rule/one", &"r".repeat(65)] {
        let mut hostile = fixture();
        hostile.basis = ClassificationBasis::PolicyRule;
        hostile.rule_id = Some(bad.to_owned());
        assert_eq!(
            hostile.canonical_bytes().expect_err("refuses"),
            MalformedClassification::RuleId,
            "rule id {bad:?} was accepted"
        );
    }
}

#[test]
fn impact_copies_both_policy_revision_and_charter_revision() {
    let record = fixture();
    let bytes = record.canonical_bytes().expect("encodes");
    let decoded = MissionImpactClassificationV1::from_canonical_bytes(&bytes).expect("decodes");

    assert_eq!(decoded.policy.project_id, uuid(PROJECT));
    assert_eq!(decoded.policy.policy_revision, 3);
    assert_eq!(
        decoded.policy.charter_revision, 5,
        "the charter revision was not copied beside the Policy revision"
    );

    // Both are inside the hashed bytes, so a copy that drifted from its source is detectable rather
    // than merely wrong: changing either alone changes the identity.
    let mut other_policy = record.clone();
    other_policy.policy.policy_revision = 4;
    let mut other_charter = record.clone();
    other_charter.policy.charter_revision = 6;
    let digests = [
        record.digest().expect("digest").value,
        other_policy.digest().expect("digest").value,
        other_charter.digest().expect("digest").value,
    ];
    let mut distinct = digests.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        3,
        "the two revisions are not independently covered by the digest"
    );

    // Policy is cited by revision, never by digest, because Policy has none. The record carries no
    // policy digest field for a caller to invent one in.
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("beacon.policy"));
}
