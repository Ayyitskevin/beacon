//! Outcome Document v1, slice 1 (ADR 0040): the pure manifest model and the Markdown renderer.
//!
//! Every fixture here is constructed. Slice 1 owns no database, no Activity, and no PDF, so a test
//! that needed one would be testing a later slice.
//!
//! The canonical bytes are assembled twice in this file: once by Beacon's encoder, through
//! `canonical_bytes`, and once by [`manifest_bytes_by_hand`], which follows ADR 0040's published
//! table rather than calling that encoder. The independence is the point -- a canonicalizer that
//! quietly changed its layout would agree with itself and disagree with this file.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use beacond::{
    AppendixRequest, ConditionResult, MalformedOutcome, NarrativeRef, OutcomeBound,
    OutcomeManifestV1, OutcomeRefusal, OutcomeState, PinnedRevision, ReceiptRef, RevisionKind,
    SourceDigest, SourceKind, SourceRef, SpendRef, SuccessCondition, VerifiedArtifact,
    VerifiedSources, VerifiedText, compare_canonical_keys, render,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const CANONICALIZER: &str = "beacon.outcome.manifest.v1";
const RENDERER: &str = "beacon.outcome.md.v1";
const MAGIC: &[u8] = b"beacon.outcome.manifest.v1\0";
const RECEIPT_CHECKSUM_LABEL: &str = "beacon.outcome.receipt-checksum.v1";

const WORKSPACE: &str = "00000000-0000-4000-8000-000000000001";
const MISSION: &str = "00000000-0000-4000-8000-000000000002";
const COMPLETION: &str = "00000000-0000-4000-8000-000000000003";
const POLICY: &str = "00000000-0000-4000-8000-000000000004";
const RESIDENT: &str = "00000000-0000-4000-8000-000000000005";
const ARTIFACT: &str = "00000000-0000-4000-8000-000000000006";
const PROPOSAL: &str = "00000000-0000-4000-8000-000000000007";
const RULING: &str = "00000000-0000-4000-8000-000000000008";
const RUN: &str = "00000000-0000-4000-8000-000000000009";
const SPEND: &str = "00000000-0000-4000-8000-00000000000a";

/// 2026-09-02T18:12:04.000Z, as a stored UTC millisecond value.
const RECORDED_AT_MS: u64 = 1_788_372_724_000;
const BOUNDARY: u64 = 4_210;

const BASE_REVISION: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";

/// The one process-global lock for the tests that mutate ambient state. Nothing else in this
/// binary reads the environment or the working directory, so the mutation is contained here.
fn ambient_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("canonical uuid")
}

fn hex32(byte: &str) -> String {
    byte.repeat(32)
}

fn digest(canonicalizer: &str, value: String) -> SourceDigest {
    SourceDigest {
        algorithm: "sha256".to_owned(),
        canonicalizer: canonicalizer.to_owned(),
        value,
    }
}

fn source(kind: SourceKind, id: &str, canonicalizer: &str, byte: &str, version: u32) -> SourceRef {
    SourceRef {
        workspace_id: uuid(WORKSPACE),
        kind,
        object_id: uuid(id),
        schema_version: version,
        digest: digest(canonicalizer, hex32(byte)),
        pinned_revision: OutcomeState::Absent(
            "this source type pins no content revision.".to_owned(),
        ),
    }
}

fn text(id: &str, body: &str) -> VerifiedText {
    VerifiedText {
        id: id.to_owned(),
        text: body.to_owned(),
    }
}

/// The vector manifest: one Mission outcome that exercises all four honesty states, an empty
/// verified collection, an escaped title, and an included appendix.
fn vector_manifest() -> OutcomeManifestV1 {
    let artifact = SourceRef {
        pinned_revision: OutcomeState::Verified(PinnedRevision {
            kind: RevisionKind::GitCommit,
            value: BASE_REVISION.to_owned(),
        }),
        ..source(
            SourceKind::Artifact,
            ARTIFACT,
            "beacon.artifact.v1",
            "44",
            1,
        )
    };
    OutcomeManifestV1 {
        workspace_id: uuid(WORKSPACE),
        mission: source(SourceKind::Mission, MISSION, "beacon.mission.v1", "11", 1),
        completion: source(
            SourceKind::Completion,
            COMPLETION,
            "beacon.completion.v1",
            "22",
            1,
        ),
        policy_classification: source(SourceKind::Policy, POLICY, "beacon.policy.v1", "33", 1),
        boundary_event_seq: BOUNDARY,
        completion_recorded_at_ms: RECORDED_AT_MS,
        accountable_resident_id: uuid(RESIDENT),
        policy_high_impact: true,
        appendix_request: AppendixRequest::PolicyDefault,
        canonicalizer_version: CANONICALIZER.to_owned(),
        renderer_version: RENDERER.to_owned(),
        success_conditions: vec![
            SuccessCondition {
                ordinal: 1,
                condition_id: "sc-alpha".to_owned(),
                outcome: OutcomeState::Verified(ConditionResult::Met),
                citations: OutcomeState::Verified(vec![artifact.clone()]),
            },
            SuccessCondition {
                ordinal: 2,
                condition_id: "sc-beta".to_owned(),
                outcome: OutcomeState::NotEvaluated(
                    "no Evaluation names this condition at the boundary event.".to_owned(),
                ),
                citations: OutcomeState::Absent(
                    "the completion recorded no supporting object for this condition.".to_owned(),
                ),
            },
            SuccessCondition {
                ordinal: 3,
                condition_id: "sc-gamma".to_owned(),
                outcome: OutcomeState::Broken(
                    "the delivered check harness exited before producing a result.".to_owned(),
                ),
                citations: OutcomeState::Verified(Vec::new()),
            },
        ],
        delivered_artifacts: OutcomeState::Verified(vec![artifact]),
        proposal_refs: OutcomeState::Verified(vec![source(
            SourceKind::Proposal,
            PROPOSAL,
            "beacon.proposal.v1",
            "55",
            1,
        )]),
        ruling_refs: OutcomeState::Verified(vec![source(
            SourceKind::Ruling,
            RULING,
            "beacon.ruling.v1",
            "66",
            1,
        )]),
        evaluation_refs: OutcomeState::Absent(
            "no Evaluation subject arm exists for an Artifact digest at this boundary.".to_owned(),
        ),
        receipt_refs: OutcomeState::Verified(vec![ReceiptRef {
            workspace_id: uuid(WORKSPACE),
            run_uuid: uuid(RUN),
            receipt_version: 2,
            outcome_checksum: hex32("77"),
        }]),
        spend_ref: OutcomeState::NotEvaluated(
            "the spend owner has not measured this Mission.".to_owned(),
        ),
        unfinished_work: OutcomeState::Verified(vec![NarrativeRef {
            ordinal: 1,
            item_id: "uw-docs".to_owned(),
        }]),
        uncertainty: OutcomeState::Verified(vec![NarrativeRef {
            ordinal: 1,
            item_id: "un-latency".to_owned(),
        }]),
    }
}

/// The Mission title deliberately carries raw HTML, a table delimiter, and CommonMark emphasis
/// markers. A stored value must never reach the document as structure.
const VECTOR_TITLE: &str =
    "Ship the exporter <script>alert(1)</script> | pipe & amp *star* _under_ [link]";

fn vector_sources() -> VerifiedSources {
    VerifiedSources {
        mission_title: VECTOR_TITLE.to_owned(),
        condition_statements: vec![
            text(
                "sc-alpha",
                "The exporter writes canonical Markdown for every completed Mission.",
            ),
            text(
                "sc-beta",
                "A reviewer who is not the author evaluates the candidate.",
            ),
            text("sc-gamma", "The check harness runs on a clean host."),
        ],
        artifacts: vec![VerifiedArtifact {
            object_id: uuid(ARTIFACT),
            media_type: "text/markdown; charset=utf-8".to_owned(),
            title: "Exporter design note (v3)".to_owned(),
        }],
        unfinished_work: vec![text(
            "uw-docs",
            "The operator runbook was not updated for the new export path.",
        )],
        uncertainty: vec![text(
            "un-latency",
            "Render latency was measured on one host only.",
        )],
        verified_source_bytes: 40_960,
    }
}

fn rendered() -> String {
    render(&vector_manifest(), &vector_sources()).expect("the vector manifest renders")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn section<'a>(document: &'a str, heading: &str, next: &str) -> &'a str {
    let start = document.find(heading).expect("section present");
    let end = document[start..]
        .find(next)
        .map(|offset| start + offset)
        .unwrap_or(document.len());
    &document[start..end]
}

// ---------------------------------------------------------------------------------------------
// An independent encoder: ADR 0040's published table, not Beacon's encoder
// ---------------------------------------------------------------------------------------------

fn push_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u64).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

fn push_hex32(out: &mut Vec<u8>, value: &str) {
    let bytes: Vec<u8> = (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).expect("hex"))
        .collect();
    assert_eq!(bytes.len(), 32);
    out.extend_from_slice(&bytes);
}

fn push_digest(out: &mut Vec<u8>, canonicalizer: &str, value: &str) {
    push_hex32(out, value);
    push_string(out, "sha256");
    push_string(out, canonicalizer);
}

fn push_absent(out: &mut Vec<u8>, reason: &str) {
    out.push(0x02);
    push_string(out, reason);
}

fn push_source(
    out: &mut Vec<u8>,
    kind: u8,
    id: &str,
    canonicalizer: &str,
    byte: &str,
    version: u32,
) {
    out.extend_from_slice(uuid(WORKSPACE).as_bytes());
    out.push(kind);
    out.extend_from_slice(uuid(id).as_bytes());
    out.extend_from_slice(&version.to_be_bytes());
    push_digest(out, canonicalizer, &hex32(byte));
    push_absent(out, "this source type pins no content revision.");
}

fn push_pinned_artifact(out: &mut Vec<u8>) {
    out.extend_from_slice(uuid(WORKSPACE).as_bytes());
    out.push(0x03);
    out.extend_from_slice(uuid(ARTIFACT).as_bytes());
    out.extend_from_slice(&1_u32.to_be_bytes());
    push_digest(out, "beacon.artifact.v1", &hex32("44"));
    out.push(0x01);
    out.push(0x01);
    push_string(out, BASE_REVISION);
}

/// The vector manifest's canonical bytes, assembled from ADR 0040's table by hand.
fn manifest_bytes_by_hand() -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    push_string(&mut out, CANONICALIZER);
    push_string(&mut out, RENDERER);
    out.extend_from_slice(uuid(WORKSPACE).as_bytes());
    push_source(&mut out, 0x01, MISSION, "beacon.mission.v1", "11", 1);
    push_source(&mut out, 0x02, COMPLETION, "beacon.completion.v1", "22", 1);
    push_source(&mut out, 0x07, POLICY, "beacon.policy.v1", "33", 1);
    out.extend_from_slice(&BOUNDARY.to_be_bytes());
    out.extend_from_slice(&RECORDED_AT_MS.to_be_bytes());
    out.extend_from_slice(uuid(RESIDENT).as_bytes());
    out.push(0x01);
    out.push(0x01);

    out.extend_from_slice(&3_u32.to_be_bytes());

    out.extend_from_slice(&1_u32.to_be_bytes());
    push_string(&mut out, "sc-alpha");
    out.push(0x01);
    out.push(0x01);
    out.push(0x01);
    out.extend_from_slice(&1_u32.to_be_bytes());
    push_pinned_artifact(&mut out);

    out.extend_from_slice(&2_u32.to_be_bytes());
    push_string(&mut out, "sc-beta");
    out.push(0x04);
    push_string(
        &mut out,
        "no Evaluation names this condition at the boundary event.",
    );
    push_absent(
        &mut out,
        "the completion recorded no supporting object for this condition.",
    );

    out.extend_from_slice(&3_u32.to_be_bytes());
    push_string(&mut out, "sc-gamma");
    out.push(0x03);
    push_string(
        &mut out,
        "the delivered check harness exited before producing a result.",
    );
    out.push(0x01);
    out.extend_from_slice(&0_u32.to_be_bytes());

    out.push(0x01);
    out.extend_from_slice(&1_u32.to_be_bytes());
    push_pinned_artifact(&mut out);

    out.push(0x01);
    out.extend_from_slice(&1_u32.to_be_bytes());
    push_source(&mut out, 0x04, PROPOSAL, "beacon.proposal.v1", "55", 1);

    out.push(0x01);
    out.extend_from_slice(&1_u32.to_be_bytes());
    push_source(&mut out, 0x05, RULING, "beacon.ruling.v1", "66", 1);

    push_absent(
        &mut out,
        "no Evaluation subject arm exists for an Artifact digest at this boundary.",
    );

    out.push(0x01);
    out.extend_from_slice(&1_u32.to_be_bytes());
    out.extend_from_slice(uuid(WORKSPACE).as_bytes());
    out.extend_from_slice(uuid(RUN).as_bytes());
    out.extend_from_slice(&2_u32.to_be_bytes());
    push_hex32(&mut out, &hex32("77"));
    push_string(&mut out, "sha256");
    push_string(&mut out, RECEIPT_CHECKSUM_LABEL);

    out.push(0x04);
    push_string(&mut out, "the spend owner has not measured this Mission.");

    out.push(0x01);
    out.extend_from_slice(&1_u32.to_be_bytes());
    out.extend_from_slice(&1_u32.to_be_bytes());
    push_string(&mut out, "uw-docs");

    out.push(0x01);
    out.extend_from_slice(&1_u32.to_be_bytes());
    out.extend_from_slice(&1_u32.to_be_bytes());
    push_string(&mut out, "un-latency");

    out
}

// ---------------------------------------------------------------------------------------------
// Shapes for the bound tests
// ---------------------------------------------------------------------------------------------

/// A manifest citing exactly `artifacts` delivered Artifacts on top of the three identities every
/// manifest cites: Mission, completion, and Policy classification.
fn bounded_manifest(artifacts: usize) -> (OutcomeManifestV1, VerifiedSources) {
    let refs: Vec<SourceRef> = (0..artifacts)
        .map(|index| SourceRef {
            workspace_id: uuid(WORKSPACE),
            kind: SourceKind::Artifact,
            object_id: Uuid::from_u128(0x1000_0000_u128 + index as u128),
            schema_version: 1,
            digest: digest("beacon.artifact.v1", format!("{index:064x}")),
            pinned_revision: OutcomeState::Absent("no revision.".to_owned()),
        })
        .collect();
    let values: Vec<VerifiedArtifact> = refs
        .iter()
        .map(|reference| VerifiedArtifact {
            object_id: reference.object_id,
            media_type: "text/plain".to_owned(),
            title: "delivered".to_owned(),
        })
        .collect();

    let mut manifest = vector_manifest();
    manifest.success_conditions = vec![SuccessCondition {
        ordinal: 1,
        condition_id: "sc-alpha".to_owned(),
        outcome: OutcomeState::Verified(ConditionResult::Met),
        citations: OutcomeState::Verified(Vec::new()),
    }];
    manifest.delivered_artifacts = OutcomeState::Verified(refs);
    manifest.proposal_refs = OutcomeState::Absent("none.".to_owned());
    manifest.ruling_refs = OutcomeState::Absent("none.".to_owned());
    manifest.receipt_refs = OutcomeState::Absent("none.".to_owned());
    manifest.unfinished_work = OutcomeState::Absent("none.".to_owned());
    manifest.uncertainty = OutcomeState::Absent("none.".to_owned());

    let sources = VerifiedSources {
        mission_title: "Bounded".to_owned(),
        condition_statements: vec![text("sc-alpha", "One condition.")],
        artifacts: values,
        unfinished_work: Vec::new(),
        uncertainty: Vec::new(),
        verified_source_bytes: 1_024,
    };
    (manifest, sources)
}

/// A manifest whose success conditions carry long statements, so the emitted document grows
/// without citing a single extra object.
fn wordy_manifest(conditions: usize, statement: usize) -> (OutcomeManifestV1, VerifiedSources) {
    let (mut manifest, mut sources) = bounded_manifest(0);
    manifest.delivered_artifacts = OutcomeState::Absent("none.".to_owned());
    manifest.success_conditions = (0..conditions)
        .map(|index| SuccessCondition {
            ordinal: index as u32 + 1,
            condition_id: format!("sc-{index}"),
            outcome: OutcomeState::Verified(ConditionResult::Met),
            citations: OutcomeState::Verified(Vec::new()),
        })
        .collect();
    sources.artifacts = Vec::new();
    sources.condition_statements = (0..conditions)
        .map(|index| text(&format!("sc-{index}"), &"x".repeat(statement)))
        .collect();
    (manifest, sources)
}

fn bound_refusal(refusal: OutcomeRefusal) -> beacond::BoundRefusal {
    match refusal {
        OutcomeRefusal::Bound(bound) => bound,
        other => panic!("expected a bound refusal, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// The seventeen load-bearing tests of ADR 0040 slice 1
// ---------------------------------------------------------------------------------------------

#[test]
fn outcome_markdown_is_byte_identical_for_same_manifest_across_insertion_orders() {
    let forward = vector_manifest();
    let mut reversed = vector_manifest();
    reversed.success_conditions.reverse();
    if let OutcomeState::Verified(refs) = &mut reversed.delivered_artifacts {
        refs.reverse();
    }

    // A second delivered Artifact, inserted in each order, so reversing is observable.
    let extra = SourceRef {
        object_id: uuid(SPEND),
        digest: digest("beacon.artifact.v1", hex32("aa")),
        ..source(
            SourceKind::Artifact,
            ARTIFACT,
            "beacon.artifact.v1",
            "aa",
            1,
        )
    };
    let mut ascending = forward.clone();
    let mut descending = reversed.clone();
    if let OutcomeState::Verified(refs) = &mut ascending.delivered_artifacts {
        refs.push(extra.clone());
    }
    if let OutcomeState::Verified(refs) = &mut descending.delivered_artifacts {
        refs.insert(0, extra.clone());
    }

    // Two references to one object id that differ only by digest. The digest is part of the sort
    // key, so dropping it from the key would leave these two in insertion order.
    let superseded = SourceRef {
        digest: digest("beacon.artifact.v1", hex32("bb")),
        ..extra.clone()
    };
    if let OutcomeState::Verified(refs) = &mut ascending.delivered_artifacts {
        refs.push(superseded.clone());
    }
    if let OutcomeState::Verified(refs) = &mut descending.delivered_artifacts {
        refs.insert(0, superseded);
    }

    let mut sources = vector_sources();
    sources.artifacts.push(VerifiedArtifact {
        object_id: extra.object_id,
        media_type: "text/plain".to_owned(),
        title: "Second delivered Artifact".to_owned(),
    });

    let left = ascending.canonical_bytes().expect("encodes");
    let right = descending.canonical_bytes().expect("encodes");
    assert_eq!(left, right, "insertion order reached the canonical bytes");
    assert_eq!(
        ascending.digest().expect("digest"),
        descending.digest().expect("digest")
    );
    assert_eq!(
        render(&ascending, &sources).expect("renders"),
        render(&descending, &sources).expect("renders"),
        "insertion order reached the document"
    );
}

/// gpt2's counterexample (spec review, P1): two references equal in kind, ID, schema and digest
/// value but differing in digest canonicalizer are two objects, not one spelling of one.
///
/// `SourceDigest` is a three-part envelope and the canonicalizer is a validated input, so a key
/// built from the value alone makes `artifact.v1` and `artifact.v2` collide: the encoder rejects
/// them as a repeated canonical key, and the census would merge them into one cited object.
#[test]
fn outcome_source_identity_includes_the_whole_digest_envelope() {
    let mut manifest = vector_manifest();
    let first = source(
        SourceKind::Artifact,
        ARTIFACT,
        "beacon.artifact.v1",
        "44",
        1,
    );
    let second = SourceRef {
        digest: digest("beacon.artifact.v2", hex32("44")),
        ..first.clone()
    };
    manifest.delivered_artifacts = OutcomeState::Verified(vec![first.clone(), second.clone()]);
    let mut sources = vector_sources();
    sources.artifacts = vec![VerifiedArtifact {
        object_id: uuid(ARTIFACT),
        media_type: "text/plain".to_owned(),
        title: "One object, two canonicalizers".to_owned(),
    }];

    // They must encode: two distinct keys, strictly ascending.
    let bytes = manifest
        .canonical_bytes()
        .expect("two canonicalizers are two canonical keys, not a repeat");
    let decoded = OutcomeManifestV1::from_canonical_bytes(&bytes).expect("decodes");
    assert_eq!(decoded.canonical_bytes().expect("re-encodes"), bytes);

    // And insertion order must not reach the identity.
    let mut reversed = manifest.clone();
    reversed.delivered_artifacts = OutcomeState::Verified(vec![second, first]);
    assert_eq!(reversed.canonical_bytes().expect("encodes"), bytes);

    // The census counts them as two, and the appendix prints two rows.
    let document = render(&manifest, &sources).expect("renders");
    let objects = section(&document, "### 10.1 Cited objects", "### 10.2");
    assert!(objects.contains("`beacon.artifact.v1`"));
    assert!(objects.contains("`beacon.artifact.v2`"));

    // The census bound sees two objects where a value-only key would see one.
    let (mut bounded, mut bounded_sources) = bounded_manifest(252);
    let base = source(
        SourceKind::Artifact,
        PROPOSAL,
        "beacon.artifact.v1",
        "aa",
        1,
    );
    let twin = SourceRef {
        digest: digest("beacon.artifact.v2", hex32("aa")),
        ..base.clone()
    };
    if let OutcomeState::Verified(refs) = &mut bounded.delivered_artifacts {
        refs.push(base.clone());
        refs.push(twin);
    }
    bounded_sources.artifacts.push(VerifiedArtifact {
        object_id: base.object_id,
        media_type: "text/plain".to_owned(),
        title: "twin".to_owned(),
    });
    // 3 identities + 252 + 2 = 257.
    let refusal = bound_refusal(render(&bounded, &bounded_sources).expect_err("257 refuse"));
    assert_eq!(refusal.bound, OutcomeBound::CitationCount);
    assert_eq!(
        refusal.observed_at_least, 257,
        "the census merged two canonicalizers into one cited object"
    );
}

#[test]
fn outcome_markdown_uses_utf8_lf_no_trailing_space_and_one_final_newline() {
    let document = rendered();
    assert!(!document.contains('\r'), "a CR reached the document");
    assert!(
        !document.starts_with('\u{feff}'),
        "a byte order mark reached the document"
    );
    assert!(document.ends_with('\n'), "no final newline");
    assert!(!document.ends_with("\n\n"), "more than one final newline");
    for (number, line) in document.lines().enumerate() {
        assert_eq!(
            line.trim_end(),
            line,
            "line {} carries trailing whitespace",
            number + 1
        );
    }
    assert_eq!(
        std::str::from_utf8(document.as_bytes()).expect("utf-8"),
        document
    );
}

#[test]
fn outcome_sections_and_front_matter_have_fixed_order() {
    let document = rendered();
    let keys = [
        "renderer_version:",
        "canonicalizer_version:",
        "manifest_digest_algorithm:",
        "manifest_digest:",
        "manifest_bytes:",
        "workspace_id:",
        "mission_id:",
        "completion_id:",
        "boundary_event_seq:",
        "accountable_resident_id:",
        "policy_classification_id:",
        "policy_high_impact:",
        "appendix_request:",
        "appendix_mode:",
    ];
    let front_matter_end = document[4..].find("\n---\n").expect("front matter closes") + 4;
    let front_matter = &document[..front_matter_end];
    let mut cursor = 0;
    for key in keys {
        let position = front_matter[cursor..]
            .find(key)
            .unwrap_or_else(|| panic!("front matter key {key} is missing or out of order"))
            + cursor;
        cursor = position + key.len();
    }

    let sections = [
        "\n## 1. Mission and completion identity\n",
        "\n## 2. Success conditions\n",
        "\n## 3. Delivered Artifacts\n",
        "\n## 4. Evaluations and cited support\n",
        "\n## 5. Actual spend and coverage\n",
        "\n## 6. Unfinished work\n",
        "\n## 7. Uncertainty\n",
        "\n## 8. Rulings and Proposal lineage\n",
        "\n## 9. Linked Receipts\n",
        "\n## 10. Verification appendix\n",
    ];
    let mut cursor = 0;
    for heading in sections {
        let position = document[cursor..]
            .find(heading)
            .unwrap_or_else(|| panic!("section {heading:?} is missing or out of order"))
            + cursor;
        assert_eq!(
            document.matches(heading).count(),
            1,
            "section {heading:?} appears more than once"
        );
        cursor = position + heading.len();
    }
}

#[test]
fn outcome_renders_absent_broken_and_not_evaluated_as_distinct_literal_states() {
    let document = rendered();
    for literal in ["`verified`", "`absent`", "`broken`", "`not_evaluated`"] {
        assert!(
            document.contains(literal),
            "the {literal} state never reached the document"
        );
    }
    // None of the four may be spelled as one of the others, or as nothing at all.
    // An em dash, an "N/A", or an empty cell in place of a state is the collapse §5 forbids. The
    // table delimiter row is `| --- |`, so a bare "--" is not a usable probe here.
    for collapse in [
        "N/A", "n/a", "\u{2014}", "\u{2013}", "TBD", "null", "None", "unknown",
    ] {
        assert!(
            !document.contains(collapse),
            "a state was collapsed into {collapse:?}"
        );
    }
    assert!(
        !document.contains("|  |"),
        "a state was collapsed into an empty table cell"
    );
    assert!(
        !document.contains("| |"),
        "a state was collapsed into an empty table cell"
    );

    // The three non-verified literals are three different words with three different reasons.
    let conditions = section(&document, "## 2. Success conditions", "## 3. Delivered");
    assert!(conditions.contains("- state: `not_evaluated` (reason: no Evaluation names this"));
    assert!(conditions.contains("- state: `broken` (reason: the delivered check harness"));
    assert!(conditions.contains("- state: `verified` (`met`)"));
    assert!(conditions.contains("- citations: `absent` (reason: the completion recorded no"));

    // A verified collection with zero entries is not absence, and says so.
    assert!(
        conditions
            .contains("zero entries were recorded at the boundary event. This is not `absent`."),
        "an empty verified collection was rendered as absence"
    );

    // The same holds in a reference table, which is a different emission path.
    let mut manifest = vector_manifest();
    manifest.ruling_refs = OutcomeState::Verified(Vec::new());
    let document = render(&manifest, &vector_sources()).expect("renders");
    let rulings = section(&document, "### 8.1 Rulings", "### 8.2 Proposal lineage");
    assert!(
        rulings.contains("zero entries were recorded at the boundary event. This is not `absent`."),
        "an empty verified reference table was rendered as absence: {rulings}"
    );
    assert!(
        !rulings.contains("`absent` (reason"),
        "an empty verified reference table claimed a domain absence"
    );

    // And in a narrative list.
    let mut manifest = vector_manifest();
    manifest.unfinished_work = OutcomeState::Verified(Vec::new());
    let mut sources = vector_sources();
    sources.unfinished_work = Vec::new();
    let document = render(&manifest, &sources).expect("renders");
    let unfinished = section(&document, "## 6. Unfinished work", "## 7. Uncertainty");
    assert!(
        unfinished
            .contains("zero entries were recorded at the boundary event. This is not `absent`."),
        "an empty verified narrative list was rendered as absence: {unfinished}"
    );
}

#[test]
fn outcome_never_turns_missing_spend_into_zero() {
    let document = rendered();
    let spend = section(&document, "## 5. Actual spend", "## 6. Unfinished work");
    assert!(spend.contains("`not_evaluated`"));
    assert!(
        spend.contains("An unmeasured or absent spend record is not zero spend."),
        "the document did not say that a missing measurement is not zero"
    );
    assert!(
        !spend.contains("| amount |"),
        "an amount row appeared for an unmeasured spend"
    );
    for zero in ["0.00", "0.0", " 0 ", "`0`", "$0", "0 USD"] {
        assert!(
            !spend.contains(zero),
            "a missing spend measurement rendered as {zero:?}"
        );
    }

    // And a measured spend renders its stored minor units exactly, through integer arithmetic.
    let mut manifest = vector_manifest();
    manifest.spend_ref = OutcomeState::Verified(SpendRef {
        workspace_id: uuid(WORKSPACE),
        spend_id: uuid(SPEND),
        schema_version: 1,
        digest: digest("beacon.spend.v1", hex32("88")),
        amount_minor: 123_456,
        currency: "USD".to_owned(),
        minor_units_exponent: 2,
        coverage_from_event_seq: 1,
        coverage_to_event_seq: BOUNDARY,
    });
    let measured = render(&manifest, &vector_sources()).expect("renders");
    let spend = section(&measured, "## 5. Actual spend", "## 6. Unfinished work");
    assert!(spend.contains("| amount | `1234.56` `USD` |"));
    assert!(spend.contains("| coverage | events `1` through `4210` |"));
}

#[test]
fn outcome_commonmark_escaping_blocks_raw_html_and_table_injection() {
    let document = rendered();
    assert!(
        !document.contains("<script>"),
        "raw HTML survived into the document"
    );
    assert!(
        document.contains("\\<script\\>alert(1)\\</script\\>"),
        "the stored HTML was not escaped as literal text"
    );
    assert!(
        document.contains("\\| pipe \\& amp \\*star\\* \\_under\\_ \\[link\\]"),
        "a stored value could still open a table cell or an inline construct"
    );

    // The Mission declaration row still has exactly the two cells its shape declares.
    let row = document
        .lines()
        .find(|line| line.starts_with("| Mission declaration |"))
        .expect("the Mission declaration row is present");
    let unescaped_pipes = row
        .char_indices()
        .filter(|(index, character)| {
            *character == '|' && (*index == 0 || !row[..*index].ends_with('\\'))
        })
        .count();
    // Two cells means three unescaped delimiters: leading, separating, trailing. A stored value
    // that reached the structure would add a fourth.
    assert_eq!(
        unescaped_pipes, 3,
        "a stored title added a table cell: {row}"
    );

    // A leading list or heading marker cannot open a block either.
    let mut sources = vector_sources();
    sources.mission_title = "- not a list item".to_owned();
    let mut manifest = vector_manifest();
    manifest.policy_high_impact = false;
    let document = render(&manifest, &sources).expect("renders");
    assert!(document.contains("| Mission declaration | \\- not a list item |"));

    let mut sources = vector_sources();
    sources.mission_title = "1. not an ordered list".to_owned();
    let document = render(&manifest, &sources).expect("renders");
    assert!(document.contains("| Mission declaration | 1\\. not an ordered list |"));
}

#[test]
fn outcome_renderer_version_selects_the_exact_commonmark_escaping_function() {
    // The known version escapes, and the document says which function did it.
    let document = rendered();
    assert!(document.contains("renderer_version: beacon.outcome.md.v1"));
    assert!(document.contains("| renderer version | `beacon.outcome.md.v1` |"));

    // An unknown version is refused, never silently escaped by whatever function is newest.
    for unknown in ["beacon.outcome.md.v2", "beacon.outcome.md", "", "md"] {
        let mut manifest = vector_manifest();
        manifest.renderer_version = unknown.to_owned();
        assert_eq!(
            manifest.canonical_bytes().expect_err("refuses"),
            MalformedOutcome::UnknownVersion,
            "renderer version {unknown:?} was accepted by the canonicalizer"
        );
        assert_eq!(
            render(&manifest, &vector_sources()).expect_err("refuses"),
            OutcomeRefusal::Malformed(MalformedOutcome::UnknownVersion),
            "renderer version {unknown:?} was accepted by the renderer"
        );
    }

    // The escaping function is part of the identity: a document cannot claim one version's bytes
    // while another version's function produced them, because the version is inside the hashed
    // manifest.
    let bytes = vector_manifest().canonical_bytes().expect("encodes");
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("beacon.outcome.md.v1"),
        "the renderer version is not covered by the manifest digest"
    );
}

#[test]
fn outcome_ignores_clock_locale_timezone_hashmap_and_filesystem_ambient_state() {
    let guard = ambient_lock().lock().expect("ambient lock");
    let baseline = rendered();

    // The clock moves. A renderer that stamped `now` would disagree with itself.
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1_100) {
        std::thread::yield_now();
    }
    assert_eq!(baseline, rendered(), "the wall clock reached the document");

    // Timezone and locale move. SAFETY: this binary's only environment reader is this test, and
    // the ambient lock serializes it against the one other test that mutates ambient state.
    let previous_tz = std::env::var("TZ").ok();
    let previous_lang = std::env::var("LANG").ok();
    for (timezone, locale) in [
        ("Pacific/Kiritimati", "tr_TR.UTF-8"),
        ("America/Los_Angeles", "C"),
    ] {
        unsafe {
            std::env::set_var("TZ", timezone);
            std::env::set_var("LANG", locale);
        }
        assert_eq!(
            baseline,
            rendered(),
            "TZ={timezone} LANG={locale} reached the document"
        );
    }
    unsafe {
        match previous_tz {
            Some(value) => std::env::set_var("TZ", value),
            None => std::env::remove_var("TZ"),
        }
        match previous_lang {
            Some(value) => std::env::set_var("LANG", value),
            None => std::env::remove_var("LANG"),
        }
    }

    // The working directory moves.
    let previous_dir = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(std::env::temp_dir()).expect("chdir");
    let moved = rendered();
    std::env::set_current_dir(&previous_dir).expect("chdir back");
    assert_eq!(
        baseline, moved,
        "the working directory reached the document"
    );

    // Hash-map iteration order cannot reach the document, because the manifest sorts before the
    // bytes exist. Feeding the arrays out of a HashMap proves the renderer never inherits it.
    let mut shuffled = vector_manifest();
    let mut bag: HashMap<String, SuccessCondition> = HashMap::new();
    for condition in vector_manifest().success_conditions {
        bag.insert(condition.condition_id.clone(), condition);
    }
    shuffled.success_conditions = bag.into_values().collect();
    assert_eq!(
        baseline,
        render(&shuffled, &vector_sources()).expect("renders"),
        "hash-map iteration order reached the document"
    );

    // The stored timestamp still renders, and it renders as the stored value.
    assert!(baseline.contains("| completion recorded at | `2026-09-02T18:12:04.000Z` |"));
    drop(guard);
}

#[test]
fn outcome_sort_uses_canonical_lowercase_code_point_order_not_locale_or_database_collation() {
    // A collation that treats `-` as ignorable -- the shape ICU uses at primary strength, and the
    // shape a SQLite collation can be given -- disagrees with code point order on these two keys.
    fn punctuation_insensitive(left: &str, right: &str) -> Ordering {
        let strip = |value: &str| value.replace('-', "");
        strip(left).cmp(&strip(right))
    }
    assert_eq!(compare_canonical_keys("a-z", "ab"), Ordering::Less);
    assert_eq!(punctuation_insensitive("a-z", "ab"), Ordering::Greater);

    let mut manifest = vector_manifest();
    manifest.success_conditions = vec![
        SuccessCondition {
            ordinal: 1,
            condition_id: "ab".to_owned(),
            outcome: OutcomeState::Verified(ConditionResult::Met),
            citations: OutcomeState::Verified(Vec::new()),
        },
        SuccessCondition {
            ordinal: 1,
            condition_id: "a-z".to_owned(),
            outcome: OutcomeState::Verified(ConditionResult::Met),
            citations: OutcomeState::Verified(Vec::new()),
        },
    ];
    let mut sources = vector_sources();
    sources.condition_statements = vec![
        text("ab", "Second by code point."),
        text("a-z", "First by code point."),
    ];

    let document = render(&manifest, &sources).expect("renders");
    let first = document.find("### 2.1 `a-z`").expect("a-z is first");
    let second = document.find("### 2.2 `ab`").expect("ab is second");
    assert!(
        first < second,
        "the renderer sorted by a collation rather than by code point"
    );

    // The canonical bytes agree with the document, and the ordinal is still the primary key.
    let bytes = manifest.canonical_bytes().expect("encodes");
    let text_bytes = String::from_utf8_lossy(&bytes);
    assert!(
        text_bytes.find("a-z").expect("present") < text_bytes.find("ab\u{0}").unwrap_or(usize::MAX)
            || text_bytes.find("a-z").expect("present") < text_bytes.rfind("ab").expect("present")
    );

    // A key that is not already canonical lowercase is refused rather than folded: folding would
    // let two spellings name one identity.
    let mut uppercase = manifest.clone();
    uppercase.success_conditions[0].condition_id = "AB".to_owned();
    assert_eq!(
        uppercase.canonical_bytes().expect_err("refuses"),
        MalformedOutcome::Token
    );
}

#[test]
fn outcome_manifest_canonical_bytes_follow_adr_0034_0037_strict_round_trip_rules() {
    let manifest = vector_manifest();
    let bytes = manifest.canonical_bytes().expect("encodes");

    // An independent assembly of ADR 0040's table produces the same bytes.
    assert_eq!(
        bytes,
        manifest_bytes_by_hand(),
        "Beacon's encoder and ADR 0040's published table disagree"
    );

    // Exact decode / re-encode byte round trip.
    let decoded = OutcomeManifestV1::from_canonical_bytes(&bytes).expect("decodes");
    assert_eq!(decoded.canonical_bytes().expect("re-encodes"), bytes);
    assert_eq!(decoded, manifest);

    let find = |needle: &[u8]| {
        bytes
            .windows(needle.len())
            .position(|window| window == needle)
            .expect("needle present")
    };

    let mut broken_magic = bytes.clone();
    broken_magic[0] ^= 0xff;
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&broken_magic).expect_err("refuses"),
        MalformedOutcome::Magic
    );

    let mut trailing = bytes.clone();
    trailing.push(0x00);
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&trailing).expect_err("refuses"),
        MalformedOutcome::TrailingBytes
    );

    let mut truncated = bytes.clone();
    truncated.pop();
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&truncated).expect_err("refuses"),
        MalformedOutcome::Truncated
    );

    // The byte after the Receipt checksum label is the spend state tag.
    let mut unknown_tag = bytes.clone();
    let spend_tag = find(RECEIPT_CHECKSUM_LABEL.as_bytes()) + RECEIPT_CHECKSUM_LABEL.len();
    assert_eq!(unknown_tag[spend_tag], 0x04);
    unknown_tag[spend_tag] = 0x09;
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&unknown_tag).expect_err("refuses"),
        MalformedOutcome::UnknownTag
    );

    let mut over_limit = bytes.clone();
    let condition = find(b"sc-alpha");
    over_limit[condition - 8..condition].copy_from_slice(&u64::MAX.to_be_bytes());
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&over_limit).expect_err("refuses"),
        MalformedOutcome::OverLimitLength
    );

    let mut invalid_utf8 = bytes.clone();
    invalid_utf8[find(b"beacon.mission.v1")] = 0xff;
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&invalid_utf8).expect_err("refuses"),
        MalformedOutcome::InvalidUtf8
    );

    let mut wrong_envelope = bytes.clone();
    let algorithm = find(b"sha256");
    wrong_envelope[algorithm..algorithm + 6].copy_from_slice(b"sha255");
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&wrong_envelope).expect_err("refuses"),
        MalformedOutcome::DigestEnvelope
    );

    let mut wrong_canonicalizer = manifest.clone();
    wrong_canonicalizer.canonicalizer_version = "beacon.outcome.manifest.v2".to_owned();
    assert_eq!(
        wrong_canonicalizer.canonical_bytes().expect_err("refuses"),
        MalformedOutcome::UnknownVersion
    );

    // Non-canonical array order is refused rather than silently re-sorted: two orders of one
    // manifest must not both decode.
    let mut ordered = manifest.clone();
    ordered.unfinished_work = OutcomeState::Verified(vec![
        NarrativeRef {
            ordinal: 1,
            item_id: "uw-a".to_owned(),
        },
        NarrativeRef {
            ordinal: 2,
            item_id: "uw-b".to_owned(),
        },
    ]);
    let mut sources = vector_sources();
    sources.unfinished_work = vec![text("uw-a", "First item."), text("uw-b", "Second item.")];
    let ordered_bytes = ordered.canonical_bytes().expect("encodes");
    let entry = |ordinal: u32, id: &str| {
        let mut out = ordinal.to_be_bytes().to_vec();
        out.extend_from_slice(&(id.len() as u64).to_be_bytes());
        out.extend_from_slice(id.as_bytes());
        out
    };
    let mut ascending = entry(1, "uw-a");
    ascending.extend_from_slice(&entry(2, "uw-b"));
    let mut descending = entry(2, "uw-b");
    descending.extend_from_slice(&entry(1, "uw-a"));
    let position = ordered_bytes
        .windows(ascending.len())
        .position(|window| window == ascending.as_slice())
        .expect("the sorted pair is in the canonical bytes");
    let mut swapped = ordered_bytes.clone();
    swapped[position..position + descending.len()].copy_from_slice(&descending);
    assert_ne!(swapped, ordered_bytes);
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&swapped).expect_err("refuses"),
        MalformedOutcome::NonCanonicalOrder
    );

    // A manifest with no declared success conditions is refused, not rendered empty.
    let mut conditionless = manifest.clone();
    conditionless.success_conditions.clear();
    assert_eq!(
        conditionless.canonical_bytes().expect_err("refuses"),
        MalformedOutcome::NoSuccessConditions
    );

    // A reference that reaches outside the manifest's Workspace aborts rather than rendering.
    let mut foreign = manifest.clone();
    foreign.mission.workspace_id = Uuid::from_u128(0xdead_beef);
    assert_eq!(
        render(&foreign, &vector_sources()).expect_err("refuses"),
        OutcomeRefusal::Malformed(MalformedOutcome::CrossWorkspace)
    );
}

/// One named change to a manifest, applied to a clone so each mutation stands alone.
type Mutation = Box<dyn Fn(&mut OutcomeManifestV1)>;

#[test]
fn outcome_manifest_digest_changes_for_each_identity_version_digest_or_boundary_change() {
    let base = vector_manifest();
    let mut seen = vec![base.digest().expect("digest").value];

    let mutations: Vec<(&str, Mutation)> = vec![
        (
            "workspace identity",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                let other = Uuid::from_u128(0xfeed);
                manifest.workspace_id = other;
                manifest.mission.workspace_id = other;
                manifest.completion.workspace_id = other;
                manifest.policy_classification.workspace_id = other;
                if let OutcomeState::Verified(refs) = &mut manifest.delivered_artifacts {
                    for reference in refs {
                        reference.workspace_id = other;
                    }
                }
                for condition in &mut manifest.success_conditions {
                    if let OutcomeState::Verified(refs) = &mut condition.citations {
                        for reference in refs {
                            reference.workspace_id = other;
                        }
                    }
                }
                if let OutcomeState::Verified(refs) = &mut manifest.proposal_refs {
                    for reference in refs {
                        reference.workspace_id = other;
                    }
                }
                if let OutcomeState::Verified(refs) = &mut manifest.ruling_refs {
                    for reference in refs {
                        reference.workspace_id = other;
                    }
                }
                if let OutcomeState::Verified(refs) = &mut manifest.receipt_refs {
                    for reference in refs {
                        reference.workspace_id = other;
                    }
                }
            }),
        ),
        (
            "mission identity",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                manifest.mission.object_id = Uuid::from_u128(0xabc1);
            }),
        ),
        (
            "completion schema version",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                manifest.completion.schema_version = 2;
            }),
        ),
        (
            "completion digest",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                manifest.completion.digest.value = "9".repeat(64);
            }),
        ),
        (
            "completion canonicalizer",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                manifest.completion.digest.canonicalizer = "beacon.completion.v2".to_owned();
            }),
        ),
        (
            "boundary event sequence",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                manifest.boundary_event_seq += 1;
            }),
        ),
        (
            "stored completion timestamp",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                manifest.completion_recorded_at_ms += 1;
            }),
        ),
        (
            "accountable Resident",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                manifest.accountable_resident_id = Uuid::from_u128(0xabc2);
            }),
        ),
        (
            "policy high impact",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                manifest.policy_high_impact = false;
            }),
        ),
        (
            "appendix request",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                manifest.appendix_request = AppendixRequest::Include;
            }),
        ),
        (
            "condition outcome",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                manifest.success_conditions[0].outcome =
                    OutcomeState::Verified(ConditionResult::NotMet);
            }),
        ),
        (
            "condition identity",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                manifest.success_conditions[0].condition_id = "sc-alpha2".to_owned();
            }),
        ),
        (
            "delivered artifact digest",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                if let OutcomeState::Verified(refs) = &mut manifest.delivered_artifacts {
                    refs[0].digest.value = "b".repeat(64);
                }
            }),
        ),
        (
            "pinned revision",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                if let OutcomeState::Verified(refs) = &mut manifest.delivered_artifacts {
                    refs[0].pinned_revision = OutcomeState::Verified(PinnedRevision {
                        kind: RevisionKind::GitCommit,
                        value: "b".repeat(40),
                    });
                }
            }),
        ),
        (
            "receipt checksum",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                if let OutcomeState::Verified(refs) = &mut manifest.receipt_refs {
                    refs[0].outcome_checksum = "c".repeat(64);
                }
            }),
        ),
        (
            "receipt version",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                if let OutcomeState::Verified(refs) = &mut manifest.receipt_refs {
                    refs[0].receipt_version = 3;
                }
            }),
        ),
        (
            "honesty state reason",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                manifest.spend_ref =
                    OutcomeState::NotEvaluated("a different stated reason.".to_owned());
            }),
        ),
        (
            "honesty state arm",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                manifest.spend_ref = OutcomeState::Absent(
                    "the spend owner has not measured this Mission.".to_owned(),
                );
            }),
        ),
    ];

    for (label, mutate) in &mutations {
        let mut manifest = base.clone();
        mutate(&mut manifest);
        let digest = manifest.digest().expect("digest").value;
        assert!(
            !seen.contains(&digest),
            "changing the {label} did not change the manifest digest"
        );
        seen.push(digest);
    }
    assert_eq!(seen.len(), mutations.len() + 1);
}

#[test]
fn outcome_renderer_refuses_257th_citation_with_observed_at_least_count() {
    // Three identities are always cited: Mission, completion, and Policy classification.
    let (manifest, sources) = bounded_manifest(253);
    let document = render(&manifest, &sources).expect("256 cited objects render");
    assert!(document.contains("## 3. Delivered Artifacts"));

    let (manifest, sources) = bounded_manifest(254);
    let refusal = bound_refusal(render(&manifest, &sources).expect_err("257 refuse"));
    assert_eq!(refusal.bound, OutcomeBound::CitationCount);
    assert_eq!(refusal.bound.as_str(), "citation_count");
    assert_eq!(refusal.configured, 256);
    assert_eq!(
        refusal.observed_at_least, 257,
        "the refusal did not report an observed-at-least count"
    );

    // One object cited twice is one cited object: the census counts objects, not rows.
    let (mut manifest, sources) = bounded_manifest(253);
    if let OutcomeState::Verified(refs) = &manifest.delivered_artifacts {
        let repeated = refs[0].clone();
        manifest.success_conditions[0].citations = OutcomeState::Verified(vec![repeated]);
    }
    render(&manifest, &sources).expect("a repeated citation is not a second object");
}

#[test]
fn outcome_renderer_refuses_source_bytes_over_16_mib() {
    let (manifest, mut sources) = bounded_manifest(1);
    sources.verified_source_bytes = 16 * 1024 * 1024;
    render(&manifest, &sources).expect("exactly 16 MiB renders");

    sources.verified_source_bytes = 16 * 1024 * 1024 + 1;
    let refusal = bound_refusal(render(&manifest, &sources).expect_err("over 16 MiB refuses"));
    assert_eq!(refusal.bound, OutcomeBound::VerifiedSourceBytes);
    assert_eq!(refusal.bound.as_str(), "verified_source_bytes");
    assert_eq!(refusal.configured, 16 * 1024 * 1024);
    assert_eq!(refusal.observed_at_least, 16 * 1024 * 1024 + 1);
}

#[test]
fn outcome_renderer_refuses_markdown_over_1_mib_without_partial_output() {
    let (manifest, sources) = wordy_manifest(400, 2_000);
    let document = render(&manifest, &sources).expect("just under the bound renders");
    assert!(
        document.len() < 1024 * 1024,
        "the fixture is already over the bound"
    );

    let (manifest, sources) = wordy_manifest(700, 2_000);
    let refusal = render(&manifest, &sources).expect_err("over 1 MiB refuses");
    // There is no arm that carries bytes and a refusal together: the buffer is dropped, so a
    // partial document cannot leave this function.
    let refusal = bound_refusal(refusal);
    assert_eq!(refusal.bound, OutcomeBound::EmittedMarkdownBytes);
    assert_eq!(refusal.bound.as_str(), "emitted_markdown_bytes");
    assert_eq!(refusal.configured, 1024 * 1024);
    assert!(
        refusal.observed_at_least > 1024 * 1024,
        "the refusal reported {} emitted bytes, which is not over the bound",
        refusal.observed_at_least
    );
}

/// gpt2's specification review, P1: the bounds exist to prevent work, so they must run before the
/// work they prevent.
///
/// Order is observable without measuring allocation. A manifest that is both over a bound *and*
/// impossible to canonicalize can only report one of the two, and which one it reports says which
/// ran first. Canonicalization is where every supplied collection is cloned, sorted, and serialized
/// into a fresh byte vector, so a bound checked after it has already paid for what it was meant to
/// refuse.
#[test]
fn outcome_bounds_are_checked_before_the_work_they_prevent() {
    // The citation bound runs before canonicalization.
    let (mut manifest, sources) = bounded_manifest(254);
    manifest.renderer_version = "beacon.outcome.md.v99".to_owned();
    assert_eq!(
        manifest
            .canonical_bytes()
            .expect_err("this manifest cannot encode"),
        MalformedOutcome::UnknownVersion
    );
    let refusal = render(&manifest, &sources).expect_err("refuses");
    assert_eq!(
        bound_refusal(refusal).bound,
        OutcomeBound::CitationCount,
        "the manifest was canonicalized before its citation bound was checked"
    );

    // The source-bytes bound runs before canonicalization too.
    let (mut manifest, mut sources) = bounded_manifest(1);
    manifest.renderer_version = "beacon.outcome.md.v99".to_owned();
    sources.verified_source_bytes = 16 * 1024 * 1024 + 1;
    let refusal = render(&manifest, &sources).expect_err("refuses");
    assert_eq!(
        bound_refusal(refusal).bound,
        OutcomeBound::VerifiedSourceBytes,
        "the manifest was canonicalized before its source-byte bound was checked"
    );

    // And the two keep their order relative to each other: the cheaper one bites first.
    let (mut manifest, mut sources) = bounded_manifest(254);
    manifest.renderer_version = "beacon.outcome.md.v99".to_owned();
    sources.verified_source_bytes = 16 * 1024 * 1024 + 1;
    assert_eq!(
        bound_refusal(render(&manifest, &sources).expect_err("refuses")).bound,
        OutcomeBound::CitationCount
    );
}

#[test]
fn outcome_each_bound_names_itself_and_is_distinct_from_other_bounds_and_absence() {
    let names = [
        OutcomeBound::CitationCount.as_str(),
        OutcomeBound::VerifiedSourceBytes.as_str(),
        OutcomeBound::EmittedMarkdownBytes.as_str(),
    ];
    let mut sorted_names = names.to_vec();
    sorted_names.sort_unstable();
    sorted_names.dedup();
    assert_eq!(sorted_names.len(), 3, "two bounds share a name");

    // Absence of a refusal is distinguishable from every bound: a manifest just under all three
    // renders.
    let (manifest, sources) = bounded_manifest(253);
    render(&manifest, &sources).expect("under every bound");

    // Each over-limit input names its own bound, and never one of the other two.
    let (over_citations, sources) = bounded_manifest(254);
    assert_eq!(
        bound_refusal(render(&over_citations, &sources).expect_err("refuses")).bound,
        OutcomeBound::CitationCount
    );

    let (manifest, mut over_bytes) = bounded_manifest(1);
    over_bytes.verified_source_bytes = 16 * 1024 * 1024 + 1;
    assert_eq!(
        bound_refusal(render(&manifest, &over_bytes).expect_err("refuses")).bound,
        OutcomeBound::VerifiedSourceBytes
    );

    let (over_markdown, sources) = wordy_manifest(700, 2_000);
    assert_eq!(
        bound_refusal(render(&over_markdown, &sources).expect_err("refuses")).bound,
        OutcomeBound::EmittedMarkdownBytes
    );

    // Two bounds exhausted at once still name the one that bit, not a blend of the two. The
    // citation census runs first and says so.
    let (mut both, mut sources) = bounded_manifest(254);
    sources.verified_source_bytes = 16 * 1024 * 1024 + 1;
    both.uncertainty = OutcomeState::Absent("none.".to_owned());
    let refusal = bound_refusal(render(&both, &sources).expect_err("refuses"));
    assert_eq!(refusal.bound, OutcomeBound::CitationCount);
    assert_eq!(refusal.configured, 256);
}

#[test]
fn outcome_renderer_does_not_embed_its_own_content_digest() {
    let document = rendered();
    let own_digest = sha256_hex(document.as_bytes());
    assert!(
        !document.contains(&own_digest),
        "the document embedded its own content digest, which is a self-hash cycle"
    );
    for key in [
        "content_digest:",
        "document_digest:",
        "outcome_digest:",
        "candidate_artifact_digest:",
        "self_digest:",
    ] {
        assert!(
            !document.contains(key),
            "front matter carries {key}, which cannot exist before the bytes it covers"
        );
    }
    // The manifest digest it does carry is the manifest's, and a reader can recompute it from the
    // bytes the appendix embeds.
    let manifest = vector_manifest();
    let bytes = manifest.canonical_bytes().expect("encodes");
    let manifest_digest = sha256_hex(&bytes);
    assert!(document.contains(&format!("manifest_digest: {manifest_digest}")));
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let embedded: String = document
        .lines()
        .filter(|line| line.len() == 64 && line.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .collect::<Vec<_>>()
        .join("");
    assert!(
        hex.starts_with(&embedded) && !embedded.is_empty(),
        "the appendix did not embed the exact canonical manifest bytes"
    );
}

// ---------------------------------------------------------------------------------------------
// The frozen independent vector
// ---------------------------------------------------------------------------------------------

fn vector_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/outcome")
        .join(name)
}

/// Regenerates the frozen vector. Not part of the suite: run it deliberately, read the diff, and
/// commit the new bytes only when the change to the renderer was intended.
///
///     cargo test -p beacond --test outcome -- --ignored --exact regenerate_outcome_vector
#[test]
#[ignore = "regenerates a frozen fixture; run deliberately, never as part of the suite"]
fn regenerate_outcome_vector() {
    let document = rendered();
    std::fs::create_dir_all(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/outcome"),
    )
    .expect("fixture directory");
    std::fs::write(vector_path("vector-v1.md"), &document).expect("write vector");
    std::fs::write(
        vector_path("vector-v1.sha256"),
        format!("{}  vector-v1.md\n", sha256_hex(document.as_bytes())),
    )
    .expect("write digest");
}

#[test]
fn independent_outcome_vector_matches_frozen_expected_markdown_bytes_and_sha256() {
    let expected =
        std::fs::read(vector_path("vector-v1.md")).expect("the frozen vector is committed");
    let recorded = std::fs::read_to_string(vector_path("vector-v1.sha256"))
        .expect("the frozen digest is committed");
    let recorded = recorded
        .split_whitespace()
        .next()
        .expect("digest line")
        .to_owned();

    // The recorded digest covers the frozen bytes, not the renderer: a fixture whose digest was
    // taken from a fresh render would agree with itself and prove nothing.
    assert_eq!(
        sha256_hex(&expected),
        recorded,
        "the frozen vector and its recorded SHA-256 disagree"
    );

    let document = rendered();
    assert_eq!(
        String::from_utf8(expected).expect("utf-8"),
        document,
        "the renderer no longer produces the frozen vector bytes; if the change was intended, \
         regenerate with: cargo test -p beacond --test outcome -- --ignored --exact \
         regenerate_outcome_vector"
    );
    assert_eq!(sha256_hex(document.as_bytes()), recorded);
}
