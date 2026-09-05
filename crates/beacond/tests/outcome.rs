//! Outcome Document v1, slice 1 (ADR 0040): the pure manifest model and the Markdown renderer.
//!
//! Every fixture here is constructed. Slice 1 owns no database, no Activity, and no PDF, so a test
//! that needed one would be testing a later slice.
//!
//! The canonical bytes are assembled twice in this file: once by Beacon's encoder, through
//! `canonical_bytes`, and once by [`manifest_bytes_by_hand`], which follows ADR 0040's published
//! table rather than calling that encoder. The independence is the point -- a canonicalizer that
//! quietly changed its layout would agree with itself and disagree with this file.

/// A per-thread allocation counter, so a test can measure what a call path costs.
///
/// gpt2's confirmation measured the citation census with one of these and found the refusal path
/// scaling with input it had already decided to refuse. Thread-local rather than global because
/// tests run in parallel and a global counter would measure the whole binary; each test owns its
/// thread, so each measures only itself. The initializer is `const` and every read is a `try_with`,
/// so the counter can never allocate or panic from inside the allocator.
mod counting {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static ALLOCATED: Cell<usize> = const { Cell::new(0) };
    }

    pub struct Counting;

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let _ = ALLOCATED.try_with(|counter| counter.set(counter.get() + layout.size()));
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            unsafe { System.dealloc(pointer, layout) }
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let _ = ALLOCATED.try_with(|counter| {
                counter.set(counter.get() + new_size.saturating_sub(layout.size()))
            });
            unsafe { System.realloc(pointer, layout, new_size) }
        }
    }

    /// Bytes allocated on this thread while `body` ran.
    pub fn measure(body: impl FnOnce()) -> usize {
        let before = ALLOCATED.with(Cell::get);
        body();
        ALLOCATED.with(Cell::get).saturating_sub(before)
    }
}

#[global_allocator]
static ALLOCATOR: counting::Counting = counting::Counting;

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use beacond::{
    AppendixRequest, ConditionResult, MalformedOutcome, NarrativeRef, OutcomeBound,
    OutcomeManifestV1, OutcomeRefusal, OutcomeState, PinnedRevision, ReceiptRef, RevisionKind,
    SourceDigest, SourceKind, SourceRef, SpendMeasurementKind, SpendRef, SuccessCondition,
    VerifiedArtifact, VerifiedSources, VerifiedText, compare_canonical_keys, render,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const CANONICALIZER: &str = "beacon.outcome.manifest.v2";
const RENDERER: &str = "beacon.outcome.md.v1";
const MAGIC: &[u8] = b"beacon.outcome.manifest.v2\0";
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

/// R18.5: the identity is present exactly where the owning record carries one. The helper keeps
/// its arguments so call sites read as they did; for a kind whose records have no UUID the `id` and
/// `version` are simply not cited, because the manifest never mints one.
fn identity_for(kind: SourceKind, id: &str, version: u32) -> Option<beacond::ReferenceIdentity> {
    match kind {
        SourceKind::Completion | SourceKind::Policy | SourceKind::Spend => {
            Some(beacond::ReferenceIdentity {
                object_id: uuid(id),
                schema_version: version,
            })
        }
        _ => None,
    }
}

fn source(kind: SourceKind, id: &str, canonicalizer: &str, byte: &str, version: u32) -> SourceRef {
    SourceRef {
        workspace_id: uuid(WORKSPACE),
        kind,
        identity: identity_for(kind, id, version),
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
        mission_id: uuid(MISSION),
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
            digest_value: hex32("44"),
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
    // Presence tag, then the identity only for the kinds whose records carry one: 0x02 completion,
    // 0x07 policy, 0x08 spend.
    if matches!(kind, 0x02 | 0x07 | 0x08) {
        out.push(0x02);
        out.extend_from_slice(uuid(id).as_bytes());
        out.extend_from_slice(&version.to_be_bytes());
    } else {
        out.push(0x01);
    }
    push_digest(out, canonicalizer, &hex32(byte));
    push_absent(out, "this source type pins no content revision.");
}

fn push_pinned_artifact(out: &mut Vec<u8>) {
    out.extend_from_slice(uuid(WORKSPACE).as_bytes());
    out.push(0x03);
    // v2: presence tag, none -- an Artifact carries no reference identity (R16, R18.5).
    out.push(0x01);
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
    out.extend_from_slice(uuid(MISSION).as_bytes());
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

/// A manifest citing exactly `artifacts` delivered Artifacts on top of the two identities every
/// manifest cites: completion and Policy classification. (v1 also cited the Mission; v2 cites it by
/// identity, outside the census -- R18.4.)
fn bounded_manifest(artifacts: usize) -> (OutcomeManifestV1, VerifiedSources) {
    let refs: Vec<SourceRef> = (0..artifacts)
        .map(|index| SourceRef {
            workspace_id: uuid(WORKSPACE),
            kind: SourceKind::Artifact,
            identity: None,
            digest: digest("beacon.artifact.v1", format!("{index:064x}")),
            pinned_revision: OutcomeState::Absent("no revision.".to_owned()),
        })
        .collect();
    let values: Vec<VerifiedArtifact> = refs
        .iter()
        .map(|reference| VerifiedArtifact {
            digest_value: reference.digest.value.clone(),
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
    // Distinct from the first delivered Artifact by its DIGEST, which is an Artifact's whole
    // identity (R16); v1 also overrode a UUID here that the record never carried.
    let extra = SourceRef {
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

    // Two references that differ only by digest. With no object id to share (an Artifact's identity
    // IS its digest, R16), the digest is the whole key, and dropping it from the key would leave
    // these two in insertion order.
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
        digest_value: extra.digest.value.clone(),
        media_type: "text/plain".to_owned(),
        title: "Second delivered Artifact".to_owned(),
    });
    // One verified value per cited digest: in v1 these two shared an object id and one value served
    // both, which was the join this renderer no longer has.
    sources.artifacts.push(VerifiedArtifact {
        digest_value: hex32("bb"),
        media_type: "text/plain".to_owned(),
        title: "Second delivered Artifact, superseded".to_owned(),
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
        digest_value: hex32("44"),
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
    let (mut bounded, mut bounded_sources) = bounded_manifest(253);
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
        digest_value: base.digest.value.clone(),
        media_type: "text/plain".to_owned(),
        title: "twin".to_owned(),
    });
    // 2 identities + 253 + 2 = 257.
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
        measurement: SpendMeasurementKind::Measured,
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

/// R13.5 and gpt2's specification review, P1.
///
/// Multi-line narrative text is accepted, not refused. LF structure is preserved through
/// CommonMark hard breaks and paragraph breaks, no raw HTML appears, nothing is silently altered,
/// and every other control character is refused by a refusal that names itself rather than by one
/// collapsed name shared with "empty" and "too long".
#[test]
fn outcome_accepts_multiline_narrative_and_names_every_text_refusal() {
    let narrative = concat!(
        "The runbook was not updated.\n",
        "Two paragraphs follow.\n",
        "\n",
        "The second one mentions <script> and | on its own line."
    );
    let mut manifest = vector_manifest();
    manifest.policy_high_impact = false;
    let mut sources = vector_sources();
    sources.unfinished_work = vec![text("uw-docs", narrative)];

    let document = render(&manifest, &sources).expect("multi-line narrative renders");
    let unfinished = section(&document, "## 6. Unfinished work", "## 7. Uncertainty");

    // LF structure survives: a trailing backslash is a CommonMark hard break, a blank line is a
    // paragraph break. Neither is a trailing space, which the byte rules forbid.
    assert!(
        unfinished.contains(concat!(
            "The runbook was not updated.\\\n",
            "Two paragraphs follow.\n",
            "\n",
            "The second one mentions"
        )),
        "the line structure was not preserved: {unfinished}"
    );
    // Every line is still escaped, so nothing in a stored value becomes structure.
    assert!(unfinished.contains("\\<script\\> and \\| on its own line."));
    assert!(!unfinished.contains("<script>"));
    // No trailing whitespace anywhere.
    for line in document.lines() {
        assert_eq!(line.trim_end(), line, "trailing whitespace in {line:?}");
    }
    // And nothing was added or dropped: every scalar of the stored value survives, in order.
    let strip = |value: &str| -> String {
        value
            .chars()
            .filter(|character| !matches!(character, '\\' | '\n'))
            .collect()
    };
    assert!(
        strip(unfinished).contains(&strip(narrative)),
        "the stored text was altered"
    );

    // TAB is content, wherever it sits. gpt2's confirmation: the integration test exercised only an
    // interior TAB, while ADR 0040 says a *leading* TAB is preserved -- and the trim rule refused
    // one as `UntrimmedText`.
    let mut tabbed = vector_sources();
    tabbed.unfinished_work = vec![text("uw-docs", "column one\tcolumn two")];
    render(&manifest, &tabbed).expect("an interior TAB is content");

    let mut leading = vector_sources();
    leading.unfinished_work = vec![text("uw-docs", "\tleading tab")];
    let document = render(&manifest, &leading).expect("a leading TAB is content");
    assert!(
        document.contains("\n\tleading tab\n"),
        "the leading TAB was not preserved into the document"
    );

    // Whitespace the document cannot carry is still refused, and each case says which rule it
    // broke. A trailing TAB would put trailing whitespace in the bytes, which the byte rules
    // forbid; leading or trailing SPACE and LF render identically to their absence, so accepting
    // them would let two stored values become one document.
    for padded in [
        " leading space",
        "trailing space ",
        "trailing tab\t",
        "\nleading newline",
        "trailing newline\n",
    ] {
        let mut hostile = vector_sources();
        hostile.unfinished_work = vec![text("uw-docs", padded)];
        assert_eq!(
            render(&manifest, &hostile).expect_err("refuses"),
            OutcomeRefusal::Malformed(MalformedOutcome::UntrimmedText),
            "padding {padded:?} was not refused by name"
        );
    }

    // A run of blank lines is kept one for one. Collapsing it would make two stored values one
    // document, which is the alteration a paragraph break must not perform.
    let mut one_break = vector_sources();
    one_break.unfinished_work = vec![text("uw-docs", "first\n\nsecond")];
    let mut two_breaks = vector_sources();
    two_breaks.unfinished_work = vec![text("uw-docs", "first\n\n\nsecond")];
    assert_ne!(
        render(&manifest, &one_break).expect("renders"),
        render(&manifest, &two_breaks).expect("renders"),
        "two blank lines and three rendered the same document"
    );

    // Every other control character is refused, and the refusal names itself.
    for control in ['\u{0}', '\u{7}', '\r', '\u{1b}', '\u{85}'] {
        let mut hostile = vector_sources();
        hostile.unfinished_work = vec![text("uw-docs", &format!("before{control}after"))];
        assert_eq!(
            render(&manifest, &hostile).expect_err("refuses"),
            OutcomeRefusal::Malformed(MalformedOutcome::ControlCharacter),
            "control character {control:?} was not refused by name"
        );
    }

    // The other text refusals are no longer spelled with one collapsed name.
    let mut empty = vector_sources();
    empty.unfinished_work = vec![text("uw-docs", "")];
    assert_eq!(
        render(&manifest, &empty).expect_err("refuses"),
        OutcomeRefusal::Malformed(MalformedOutcome::EmptyText)
    );
    let mut padded = vector_sources();
    padded.unfinished_work = vec![text("uw-docs", "  padded  ")];
    assert_eq!(
        render(&manifest, &padded).expect_err("refuses"),
        OutcomeRefusal::Malformed(MalformedOutcome::UntrimmedText)
    );
    let mut long = vector_sources();
    long.unfinished_work = vec![text("uw-docs", &"x".repeat(2_001))];
    assert_eq!(
        render(&manifest, &long).expect_err("refuses"),
        OutcomeRefusal::Malformed(MalformedOutcome::TextTooLong)
    );

    // A label that renders inside a table cell still refuses a line break, and says so by name.
    let mut titled = vector_sources();
    titled.mission_title = "two\nlines".to_owned();
    assert_eq!(
        render(&manifest, &titled).expect_err("refuses"),
        OutcomeRefusal::Malformed(MalformedOutcome::LineBreakInLabel)
    );
    assert_eq!(
        MalformedOutcome::LineBreakInLabel.as_str(),
        "line_break_in_label"
    );
    assert_eq!(
        MalformedOutcome::ControlCharacter.as_str(),
        "control_character"
    );
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

    // The canonical bytes agree with the document. kimi P2-2: the previous form of this assertion
    // searched for a NUL-terminated key in a length-prefixed grammar, so its first disjunct was
    // always true and the whole assertion could not fail. Locate each condition's exact encoded
    // entry -- ordinal, length prefix, id -- and compare positions.
    let bytes = manifest.canonical_bytes().expect("encodes");
    let encoded = |ordinal: u32, id: &str| {
        let mut entry = ordinal.to_be_bytes().to_vec();
        entry.extend_from_slice(&(id.len() as u64).to_be_bytes());
        entry.extend_from_slice(id.as_bytes());
        bytes
            .windows(entry.len())
            .position(|window| window == entry.as_slice())
            .unwrap_or_else(|| panic!("the entry for {id} is in the canonical bytes"))
    };
    assert!(
        encoded(1, "a-z") < encoded(1, "ab"),
        "the canonical bytes ordered `ab` before `a-z`, which is collation order, not code point"
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

/// R18.3: the owner's four arms reach the manifest without collapse. Exhaustive over the owner's
/// enum ON PURPOSE -- a fifth `ConditionOutcome` arm makes this test stop compiling, which is how
/// a closed vocabulary is extended by a named change rather than mapped to something by default.
#[test]
fn owner_condition_outcomes_map_without_collapsing_inconclusive_into_not_evaluated() {
    use beacond::ConditionOutcome;
    for outcome in [
        ConditionOutcome::Met,
        ConditionOutcome::NotMet,
        ConditionOutcome::NotAssessed,
        ConditionOutcome::AssessmentInconclusive,
    ] {
        let mapped: OutcomeState<ConditionResult> = outcome.into();
        match outcome {
            ConditionOutcome::Met => {
                assert_eq!(mapped, OutcomeState::Verified(ConditionResult::Met));
            }
            ConditionOutcome::NotMet => {
                assert_eq!(mapped, OutcomeState::Verified(ConditionResult::NotMet));
            }
            // Evaluated, no verdict: a STATE of its own, never a reason string beside a different
            // state. This is the collapse the invariant forbids, and the assertion that catches it.
            ConditionOutcome::AssessmentInconclusive => {
                assert_eq!(
                    mapped,
                    OutcomeState::Verified(ConditionResult::Inconclusive),
                    "assessment_inconclusive collapsed onto another state: {mapped:?}"
                );
            }
            // Nobody looked: the same state as the manifest's `not_evaluated`, in the owner's
            // spelling.
            ConditionOutcome::NotAssessed => {
                assert_eq!(
                    mapped,
                    OutcomeState::NotEvaluated("not_assessed".to_owned())
                );
            }
        }
    }

    // And the new arm is its own literal in the document, round-trips through the bytes, and is
    // distinct from both neighbours on the page.
    let mut manifest = vector_manifest();
    manifest.success_conditions[0].outcome = OutcomeState::Verified(ConditionResult::Inconclusive);
    let decoded = OutcomeManifestV1::from_canonical_bytes(&manifest.canonical_bytes().unwrap())
        .expect("decodes");
    assert_eq!(decoded, manifest);
    let document = render(&manifest, &vector_sources()).expect("renders");
    assert!(document.contains("`assessment_inconclusive`"), "{document}");
    assert!(document.contains("`not_evaluated`"), "{document}");
}

/// R18.4 closed the front door -- `mission: SourceRef` is gone -- and this closes the side door: a
/// `SourceRef` of kind Mission inside a condition's citations would carry a Mission digest nothing
/// can verify, which is the very field the grammar revision removed. The tag stays assigned and is
/// never reused (R13.8); no reference may carry it. Refused by name on encode AND on decode, so a
/// byte sequence built by hand cannot get in either.
/// R18.5, and R15's "never minted": a reference carries a UUID + schema version exactly when the
/// owning record carries them inside its own canonical bytes. Proposal, Ruling, Evaluation and
/// Artifact records carry none today, so a manifest that cites one WITH an identity has invented
/// it; Completion, Policy and Spend records carry one, so a citation WITHOUT it has dropped it.
/// Both directions, both codec sides, each refusal by name -- and presence is a tag in the bytes,
/// so the day a record gains its UUID under R15 the rule flips without a byte moving.
#[test]
fn reference_identity_is_present_exactly_where_the_owner_carries_one() {
    let minted = beacond::ReferenceIdentity {
        object_id: uuid(PROPOSAL),
        schema_version: 1,
    };
    for kind in [
        SourceKind::Proposal,
        SourceKind::Ruling,
        SourceKind::Evaluation,
        SourceKind::Artifact,
    ] {
        let mut manifest = vector_manifest();
        let mut reference = source(kind, PROPOSAL, "beacon.proposal.v1", "55", 1);
        assert!(
            reference.identity.is_none(),
            "{kind:?} should cite no identity"
        );
        reference.identity = Some(minted);
        manifest.success_conditions[0].citations = OutcomeState::Verified(vec![reference]);
        assert_eq!(
            manifest
                .canonical_bytes()
                .expect_err("a minted identity encoded"),
            MalformedOutcome::ReferenceIdentityNotCarried,
            "{kind:?}"
        );
    }
    for kind in [
        SourceKind::Completion,
        SourceKind::Policy,
        SourceKind::Spend,
    ] {
        let mut manifest = vector_manifest();
        let mut reference = source(kind, COMPLETION, "beacon.completion.v1", "22", 1);
        assert!(
            reference.identity.is_some(),
            "{kind:?} should cite its identity"
        );
        reference.identity = None;
        manifest.success_conditions[0].citations = OutcomeState::Verified(vec![reference]);
        assert_eq!(
            manifest
                .canonical_bytes()
                .expect_err("a dropped identity encoded"),
            MalformedOutcome::ReferenceIdentityRequired,
            "{kind:?}"
        );
    }

    // Decode side: flip the presence tag of the vector's Proposal citation from none (0x01) to
    // some (0x02). The bytes that follow are then read as a UUID + version the record never had,
    // and the shape rule refuses before the digest is even looked at.
    let bytes = vector_manifest().canonical_bytes().unwrap();
    let proposal_kind_tag = find_in(&bytes, b"beacon.proposal.v1") - (32 + 8 + 6 + 8) - 1 - 1;
    assert_eq!(
        bytes[proposal_kind_tag], 0x04,
        "the probe did not land on the Proposal kind tag"
    );
    assert_eq!(
        bytes[proposal_kind_tag + 1],
        0x01,
        "the Proposal citation should carry no identity"
    );
    let mut minted_bytes = bytes.clone();
    minted_bytes[proposal_kind_tag + 1] = 0x02;
    assert!(
        matches!(
            OutcomeManifestV1::from_canonical_bytes(&minted_bytes),
            Err(MalformedOutcome::ReferenceIdentityNotCarried) | Err(MalformedOutcome::Truncated)
        ),
        "a decoder that reads a minted identity: {:?}",
        OutcomeManifestV1::from_canonical_bytes(&minted_bytes)
    );
    // An unassigned presence tag is refused as such.
    let mut bad_tag = bytes.clone();
    bad_tag[proposal_kind_tag + 1] = 0x07;
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&bad_tag).expect_err("refuses"),
        MalformedOutcome::UnknownTag
    );
}

#[test]
fn a_mission_kind_source_ref_is_refused_anywhere_a_citation_can_appear() {
    let mut manifest = vector_manifest();
    manifest.success_conditions[0].citations = OutcomeState::Verified(vec![source(
        SourceKind::Mission,
        MISSION,
        "beacon.mission.v1",
        "11",
        1,
    )]);
    assert_eq!(
        manifest
            .canonical_bytes()
            .expect_err("a Mission citation encoded"),
        MalformedOutcome::SourceKind
    );

    // The decode side: take the frozen-shape bytes and retag one delivered Artifact citation as a
    // Mission. The tag byte is the one right after that reference's Workspace UUID.
    let good = vector_manifest();
    let bytes = good.canonical_bytes().unwrap();
    // v2 layout: kind tag, identity presence tag (1 byte, none for an Artifact), sha256 (32),
    // algorithm token (8 + 6), canonicalizer token prefix (8), then the text found here.
    let artifact_tag = find_in(&bytes, b"beacon.artifact.v1") - (8 + 6 + 8 + 32) - 1 - 1;
    assert_eq!(
        bytes[artifact_tag], 0x03,
        "the probe did not land on the Artifact kind tag"
    );
    let mut retagged = bytes.clone();
    retagged[artifact_tag] = 0x01;
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&retagged).expect_err("a Mission citation decoded"),
        MalformedOutcome::SourceKind
    );
}

fn find_in(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("needle present")
}

#[test]
fn sol1_an_unknown_state_tag_is_named_before_its_non_reason_payload() {
    let mut bytes = vector_manifest().canonical_bytes().unwrap();
    let condition_id = find_in(&bytes, b"sc-alpha");
    let outcome_state_tag = condition_id + b"sc-alpha".len();
    assert_eq!(
        bytes[outcome_state_tag], 0x01,
        "probe missed verified state"
    );
    bytes[outcome_state_tag] = 0x09;

    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&bytes).expect_err("unknown tag decoded"),
        MalformedOutcome::UnknownTag,
        "the decoder consumed a reason payload before classifying its closed state tag"
    );
}

#[test]
fn sol1_v2_remaining_strictness_and_identity_matrix() {
    let manifest = vector_manifest();
    let bytes = manifest.canonical_bytes().unwrap();

    // An `identity: none` tag cannot make the following 20 identity bytes disappear: the kind's
    // shape is rejected before those bytes can be reinterpreted as a digest.
    let completion_canonicalizer = find_in(&bytes, b"beacon.completion.v1");
    let completion_kind = completion_canonicalizer - (8 + 6 + 8 + 32 + 4 + 16 + 1 + 1);
    assert_eq!(bytes[completion_kind], 0x02);
    assert_eq!(bytes[completion_kind + 1], 0x02);
    let mut identity_dropped_but_payload_left = bytes.clone();
    identity_dropped_but_payload_left[completion_kind + 1] = 0x01;
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&identity_dropped_but_payload_left)
            .expect_err("identity disappeared"),
        MalformedOutcome::ReferenceIdentityRequired
    );

    // The last string gives a clean past-end length and a one-byte/non-fixed-width length attempt.
    let final_id = find_in(&bytes, b"un-latency");
    let mut past_end = bytes.clone();
    past_end[final_id - 8..final_id].copy_from_slice(&11_u64.to_be_bytes());
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&past_end).expect_err("past-end length decoded"),
        MalformedOutcome::Truncated
    );
    let mut short_length = bytes.clone();
    short_length.drain(final_id - 8..final_id - 1);
    assert!(OutcomeManifestV1::from_canonical_bytes(&short_length).is_err());

    let mut invalid_statement_id = bytes.clone();
    invalid_statement_id[find_in(&bytes, b"sc-alpha")] = 0xff;
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&invalid_statement_id)
            .expect_err("non-UTF-8 condition id decoded"),
        MalformedOutcome::InvalidUtf8
    );

    // Duplicate one complete canonical key (ordinal + equal-length id) without changing the
    // following value, so only strict ordering can refuse it.
    let gamma = find_in(&bytes, b"sc-gamma");
    let mut duplicate = bytes.clone();
    duplicate[gamma - 12..gamma - 8].copy_from_slice(&1_u32.to_be_bytes());
    duplicate[gamma..gamma + 8].copy_from_slice(b"sc-alpha");
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&duplicate).expect_err("duplicate key decoded"),
        MalformedOutcome::NonCanonicalOrder
    );

    let mut dropped = manifest.clone();
    dropped.completion.identity = None;
    assert_eq!(
        dropped
            .digest()
            .expect_err("identity-free reference hashed"),
        MalformedOutcome::ReferenceIdentityRequired
    );
    let mut changed = manifest.clone();
    changed.completion.identity.as_mut().unwrap().object_id = Uuid::from_u128(0xabc123);
    assert_ne!(manifest.digest().unwrap(), changed.digest().unwrap());

    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(b"beacon.outcome.manifest.v1\0")
            .expect_err("retired magic decoded"),
        MalformedOutcome::RetiredVersion
    );
    let mut v2_magic_v1_profile = bytes.clone();
    let profile = MAGIC.len() + 8;
    v2_magic_v1_profile[profile + CANONICALIZER.len() - 1] = b'1';
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&v2_magic_v1_profile)
            .expect_err("mixed version decoded"),
        MalformedOutcome::UnknownVersion
    );
}

/// R23.1: the spend owner's measurement token travels in the manifest, all four arms of it, and a
/// partial measurement is rendered as its own state beside its amount and coverage -- never as a
/// complete spend. An unassigned measurement tag is refused by name.
#[test]
fn a_partial_spend_measurement_is_its_own_state_beside_its_amount() {
    let spend = |measurement: SpendMeasurementKind| SpendRef {
        workspace_id: uuid(WORKSPACE),
        spend_id: uuid(SPEND),
        schema_version: 1,
        digest: digest("beacon.spend.v1", hex32("88")),
        amount_minor: 123_456,
        currency: "USD".to_owned(),
        minor_units_exponent: 2,
        coverage_from_event_seq: 1,
        coverage_to_event_seq: 3_000,
        measurement,
    };
    // Every arm round-trips, and each changes the bytes: four tokens, four identities.
    let mut seen = std::collections::BTreeSet::new();
    for kind in [
        SpendMeasurementKind::Measured,
        SpendMeasurementKind::PartiallyMeasured,
        SpendMeasurementKind::Unmeasured,
        SpendMeasurementKind::Broken,
    ] {
        let mut manifest = vector_manifest();
        manifest.spend_ref = OutcomeState::Verified(spend(kind));
        let bytes = manifest.canonical_bytes().expect("encodes");
        assert_eq!(
            OutcomeManifestV1::from_canonical_bytes(&bytes).expect("decodes"),
            manifest
        );
        assert!(
            seen.insert(bytes),
            "{kind:?} shares its bytes with another arm"
        );
    }

    // The partial one says so in the document, beside its amount and coverage, and is not the
    // complete one's word.
    let mut partial = vector_manifest();
    partial.spend_ref = OutcomeState::Verified(spend(SpendMeasurementKind::PartiallyMeasured));
    let document = render(&partial, &vector_sources()).expect("renders");
    let section = section(&document, "## 5. Actual spend", "## 6. Unfinished work");
    assert!(
        section.contains("| measurement | `partially_measured` |"),
        "{section}"
    );
    assert!(
        section.contains("| amount | `1234.56` `USD` |"),
        "{section}"
    );
    assert!(
        section.contains("| coverage | events `1` through `3000` |"),
        "{section}"
    );
    assert!(section.contains("floor over that window"), "{section}");
    assert!(
        !section.contains("| measurement | `measured` |"),
        "{section}"
    );
    let mut complete = vector_manifest();
    complete.spend_ref = OutcomeState::Verified(spend(SpendMeasurementKind::Measured));
    let complete_document = render(&complete, &vector_sources()).unwrap();
    let complete_section = section_of(&complete_document);
    assert!(
        complete_section.contains("| measurement | `measured` |"),
        "{complete_section}"
    );
    assert!(
        !complete_section.contains("floor over that window"),
        "{complete_section}"
    );

    // An unassigned measurement tag is refused by name. The tag is the one byte the two encodings
    // above differ in.
    let measured_bytes = complete.canonical_bytes().unwrap();
    let partial_bytes = partial.canonical_bytes().unwrap();
    assert_eq!(measured_bytes.len(), partial_bytes.len());
    let differing: Vec<usize> = (0..measured_bytes.len())
        .filter(|&i| measured_bytes[i] != partial_bytes[i])
        .collect();
    assert_eq!(
        differing.len(),
        1,
        "the arms differ in more than the tag byte"
    );
    let mut unknown = measured_bytes.clone();
    unknown[differing[0]] = 0x09;
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&unknown).expect_err("refuses"),
        MalformedOutcome::UnknownTag
    );
}

fn section_of(document: &str) -> &str {
    section(document, "## 5. Actual spend", "## 6. Unfinished work")
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
    invalid_utf8[find(b"beacon.completion.v1")] = 0xff;
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

    // gpt2's M24: the digest envelope's canonicalizer is a validated token, not free text. Without
    // that validation an uppercase or punctuated spelling encodes, and two spellings of one
    // canonicalizer become two identities.
    let mut folded_canonicalizer = manifest.clone();
    folded_canonicalizer.completion.digest.canonicalizer = "Beacon.Completion.V1".to_owned();
    assert_eq!(
        folded_canonicalizer.canonical_bytes().expect_err("refuses"),
        MalformedOutcome::DigestEnvelope
    );
    let mut spaced_canonicalizer = manifest.clone();
    spaced_canonicalizer.completion.digest.canonicalizer = "beacon completion v1".to_owned();
    assert_eq!(
        spaced_canonicalizer.canonical_bytes().expect_err("refuses"),
        MalformedOutcome::DigestEnvelope
    );

    // A grammar nobody has heard of is `UnknownVersion`; the grammar this one REPLACED is
    // `RetiredVersion`. Two names, because a caller holding v1 bytes and a caller holding bytes
    // from the future need different advice. (This probe used to spell "unknown" as `.v2` -- which
    // became the truth under R18.3-5 and turned the probe into a no-op; the token is now one that
    // will not be assigned.)
    let mut wrong_canonicalizer = manifest.clone();
    wrong_canonicalizer.canonicalizer_version = "beacon.outcome.manifest.v9".to_owned();
    assert_eq!(
        wrong_canonicalizer.canonical_bytes().expect_err("refuses"),
        MalformedOutcome::UnknownVersion
    );
    let mut retired_canonicalizer = manifest.clone();
    retired_canonicalizer.canonicalizer_version = beacond::RETIRED_CANONICALIZER.to_owned();
    assert_eq!(
        retired_canonicalizer
            .canonical_bytes()
            .expect_err("refuses"),
        MalformedOutcome::RetiredVersion
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

    // kimi P2-1: a hostile declared count is refused by arithmetic against the remaining input,
    // before the first entry is read. Order is observable: the first entry here is itself
    // malformed, so a decoder that reads entries before checking the count reports the entry's
    // refusal instead of `Truncated`.
    let mut hostile = manifest.clone();
    hostile.unfinished_work = OutcomeState::Verified(vec![NarrativeRef {
        ordinal: 1,
        item_id: "uw-a".to_owned(),
    }]);
    let hostile_bytes = hostile.canonical_bytes().expect("encodes");
    let mut entry = 1_u32.to_be_bytes().to_vec();
    entry.extend_from_slice(&4_u64.to_be_bytes());
    entry.extend_from_slice(b"uw-a");
    let entry_start = hostile_bytes
        .windows(entry.len())
        .position(|window| window == entry.as_slice())
        .expect("the unfinished-work entry is in the bytes");
    let mut inflated = hostile_bytes.clone();
    // The declared count sits immediately before its first entry.
    inflated[entry_start - 4..entry_start].copy_from_slice(&4_096_u32.to_be_bytes());
    // Corrupt that first entry's id, so a decoder that reads entries before checking the count
    // reports `Token` rather than `Truncated`.
    inflated[entry_start + 12] = b'U';
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&inflated).expect_err("refuses"),
        MalformedOutcome::Truncated,
        "the declared count was trusted and the first entry was read before it was checked"
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
    foreign.completion.workspace_id = Uuid::from_u128(0xdead_beef);
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
                manifest.mission_id = Uuid::from_u128(0xabc1);
            }),
        ),
        (
            "completion schema version",
            Box::new(|manifest: &mut OutcomeManifestV1| {
                manifest
                    .completion
                    .identity
                    .as_mut()
                    .unwrap()
                    .schema_version = 2;
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
    // Two identities are always cited: completion and Policy classification (the Mission is an
    // identity, not a citation, since v2).
    let (manifest, sources) = bounded_manifest(254);
    let document = render(&manifest, &sources).expect("256 cited objects render");
    assert!(document.contains("## 3. Delivered Artifacts"));

    // 2 identities (completion, Policy) + 255 artifacts = 257. The Mission left the census with the
    // v2 grammar: it is an identity, not a citation.
    let (manifest, sources) = bounded_manifest(255);
    let refusal = bound_refusal(render(&manifest, &sources).expect_err("257 refuse"));
    assert_eq!(refusal.bound, OutcomeBound::CitationCount);
    assert_eq!(refusal.bound.as_str(), "citation_count");
    assert_eq!(refusal.configured, 256);
    assert_eq!(
        refusal.observed_at_least, 257,
        "the refusal did not report an observed-at-least count"
    );

    // R13.8 and gpt2's M23: Receipts are their own census lane, not a borrowed `0x00`. The
    // counterexample is a Receipt and a completion that agree on every other part of the key, so a
    // census that put them in one lane would count them as one cited object and let 257 through.
    // (v1 staged this on the Mission reference; v2 cites the Mission by identity, so the completion
    // plays the part.)
    let (mut collide, mut collide_sources) = bounded_manifest(254);
    collide.completion.identity = Some(beacond::ReferenceIdentity {
        object_id: uuid(RUN),
        schema_version: 2,
    });
    collide.completion.digest = digest("beacon.outcome.receipt-checksum.v1", hex32("77"));
    collide.receipt_refs = OutcomeState::Verified(vec![ReceiptRef {
        workspace_id: uuid(WORKSPACE),
        run_uuid: uuid(RUN),
        receipt_version: 2,
        outcome_checksum: hex32("77"),
    }]);
    collide_sources.mission_title = "One Mission, one Receipt, one key apart".to_owned();
    // 2 identities + 254 artifacts + 1 Receipt = 257, and only a distinct Receipt lane keeps it so.
    let refusal = bound_refusal(render(&collide, &collide_sources).expect_err("257 refuse"));
    assert_eq!(refusal.bound, OutcomeBound::CitationCount);
    assert_eq!(
        refusal.observed_at_least, 257,
        "the Receipt was counted in a source lane and merged with the completion"
    );

    // One object cited twice is one cited object: the census counts objects, not rows.
    let (mut manifest, sources) = bounded_manifest(254);
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
    let (mut manifest, sources) = bounded_manifest(255);
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
    let (mut manifest, mut sources) = bounded_manifest(255);
    manifest.renderer_version = "beacon.outcome.md.v99".to_owned();
    sources.verified_source_bytes = 16 * 1024 * 1024 + 1;
    assert_eq!(
        bound_refusal(render(&manifest, &sources).expect_err("refuses")).bound,
        OutcomeBound::CitationCount
    );
}

/// gpt2's confirmation of OD-2, P1 still HOLD: a bound that refuses at 257 must not first walk to
/// 50,000.
///
/// Precedence over canonicalization was already proved by
/// `outcome_bounds_are_checked_before_the_work_they_prevent`; this measures the census's own cost.
/// Both manifests are built before either measurement starts, so what is compared is the refusal
/// path and nothing else.
#[test]
fn outcome_citation_bound_refuses_without_walking_past_it() {
    let (just_over, just_over_sources) = bounded_manifest(255);
    let (far_over, far_over_sources) = bounded_manifest(50_000);

    // One warm-up so a lazily initialized static is not charged to the first measurement.
    let _ = render(&just_over, &just_over_sources);

    let near = counting::measure(|| {
        assert!(render(&just_over, &just_over_sources).is_err());
    });
    let far = counting::measure(|| {
        assert!(render(&far_over, &far_over_sources).is_err());
    });

    assert!(
        far <= near * 2,
        "the citation census walked past its own bound: {near} bytes for 257 citations, {far} for \
         50,003 -- a refusal path that scales with input it has already decided to refuse"
    );
}

#[test]
fn outcome_collection_length_refuses_before_cloning_and_sorting_the_overlimit_array() {
    let (mut near, sources) = bounded_manifest(1);
    let repeated = match &near.delivered_artifacts {
        OutcomeState::Verified(refs) => refs[0].clone(),
        other => panic!("fixture has no verified Artifact: {other:?}"),
    };
    near.delivered_artifacts = OutcomeState::Verified(vec![repeated.clone(); 4_097]);
    let mut far = near.clone();
    far.delivered_artifacts = OutcomeState::Verified(vec![repeated; 50_000]);

    let near_allocated = counting::measure(|| {
        assert!(render(&near, &sources).is_err());
    });
    let far_allocated = counting::measure(|| {
        assert!(render(&far, &sources).is_err());
    });

    assert!(
        far_allocated <= near_allocated * 2,
        "the 4,096-entry cap ran after cloning/sorting: {near_allocated} bytes at 4,097, \
         {far_allocated} bytes at 50,000",
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
    let (manifest, sources) = bounded_manifest(254);
    render(&manifest, &sources).expect("under every bound");

    // Each over-limit input names its own bound, and never one of the other two.
    let (over_citations, sources) = bounded_manifest(255);
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
    let (mut both, mut sources) = bounded_manifest(255);
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
    // kimi P2-3: the previous form kept only 64-character lines, so it dropped the final short
    // chunk and `starts_with` still passed. Take the fenced block whole and compare it exactly.
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let fence = document
        .split("\n```\n")
        .nth(1)
        .expect("the manifest proof is fenced");
    let embedded: String = fence.lines().collect::<Vec<_>>().join("");
    assert!(
        embedded.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "the fenced manifest proof is not hexadecimal"
    );
    assert_eq!(
        embedded, hex,
        "the appendix did not embed the exact canonical manifest bytes"
    );
    assert_eq!(embedded.len(), bytes.len() * 2);
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
///
/// **The v2 slot is filled by a NON-AUTHOR seat** (the K-2 rule, kimi's slice-3 read §5, R18.3-5):
/// the author of a grammar revision does not also author the bytes that prove it. The seat named
/// for v2 is kimi. Until it runs, `vector-v2.md` does not exist and the frozen-vector test below
/// fails, on purpose -- that failure is the slot, and an encoder that disagrees with the vector
/// kimi builds is the proof the fold was read rather than assumed.
#[test]
#[ignore = "regenerates a frozen fixture; run deliberately, by a non-author seat, never as part of the suite"]
fn regenerate_outcome_vector() {
    let document = rendered();
    std::fs::create_dir_all(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/outcome"),
    )
    .expect("fixture directory");
    std::fs::write(vector_path("vector-v2.md"), &document).expect("write vector");
    std::fs::write(
        vector_path("vector-v2.sha256"),
        format!("{}  vector-v2.md\n", sha256_hex(document.as_bytes())),
    )
    .expect("write digest");
}

/// The v1 grammar is retired, and retired means refused by name: not `Magic` (these bytes ARE a
/// manifest, under a grammar this build no longer holds) and not `UnknownVersion` (this build has
/// heard of v1 -- it is the grammar it replaced).
///
/// The bytes are the real thing. `manifest-v1.bin` was extracted from the retired v1 vector's own
/// verification appendix -- the proof block every document embeds so a reader can recompute the
/// manifest digest -- and checked against that document's recorded digest and byte length before
/// it was committed. Nothing here was produced by a v1 encoder this tree no longer has.
#[test]
fn retired_v1_manifest_bytes_are_refused_by_name() {
    let bytes = std::fs::read(vector_path("manifest-v1.bin")).expect("the v1 bytes are committed");
    assert!(
        bytes.starts_with(b"beacon.outcome.manifest.v1\0"),
        "the fixture is not v1 bytes"
    );
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(&bytes).expect_err("v1 bytes decoded under v2"),
        MalformedOutcome::RetiredVersion,
    );
    // And the two neighbours stay distinct: garbage is `Magic`, a grammar nobody has heard of is
    // `UnknownVersion`. Three states, three names.
    assert_eq!(
        OutcomeManifestV1::from_canonical_bytes(b"not a manifest at all").expect_err("garbage"),
        MalformedOutcome::Magic,
    );
    let mut future = bytes.clone();
    future[..b"beacon.outcome.manifest.v1\0".len()]
        .copy_from_slice(b"beacon.outcome.manifest.v9\0");
    assert!(
        !matches!(
            OutcomeManifestV1::from_canonical_bytes(&future),
            Err(MalformedOutcome::RetiredVersion)
        ),
        "a grammar nobody has heard of was reported as the retired one"
    );
}

#[test]
fn independent_outcome_vector_matches_frozen_expected_markdown_bytes_and_sha256() {
    // THE EMPTY v2 SLOT. This fails until kimi -- a non-author seat -- runs `regenerate_outcome_vector`
    // against its own read of the R18.3-5 grammar and commits `vector-v2.md` + `vector-v2.sha256`.
    // A vector built by the grammar's author would agree with the author's encoder and prove
    // nothing; the mismatch, if there is one, is the finding.
    let expected = std::fs::read(vector_path("vector-v2.md")).expect(
        "vector-v2.md is not committed: the v2 vector slot is EMPTY by design and is kimi's to fill \
         (non-author seat, K-2 rule); run `cargo test -p beacond --test outcome -- --ignored --exact \
         regenerate_outcome_vector` from an independent read of R18.3-5, review the bytes, commit",
    );
    let recorded = std::fs::read_to_string(vector_path("vector-v2.sha256"))
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
