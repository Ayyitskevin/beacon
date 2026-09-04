//! Artifact v1, slice 1 (ADR 0043): the canonical grammar and the identity, nothing else.
//!
//! Slice 1 owns no store, no Event, and no content bytes, so a test that needed one would be
//! testing a later slice. The canonical bytes are assembled twice in this file: once by Beacon's
//! encoder through `canonical_bytes`, and once by [`vector_bytes_by_hand`], which follows ADR
//! 0043's published field table rather than calling that encoder. The pinned digest was produced
//! independently of both, in Python, from the same table. An encoder that quietly changed its
//! layout would agree with itself and disagree with this file.

use beacond::{
    Artifact, ArtifactContext, ArtifactDigest, ArtifactKind, ArtifactV1, MalformedArtifact,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const CANONICALIZER: &str = "beacon.artifact.v1";
const MAGIC: &[u8] = b"beacon.artifact.v1\0";
const RAW_CONTENT: &str = "beacon.raw.v1";
const MEDIA_MARKDOWN: &str = "text/markdown; charset=utf-8";

const WORKSPACE: &str = "00000000-0000-4000-8000-000000000001";
const PROJECT: &str = "00000000-0000-4000-8000-000000000002";
const MISSION: &str = "00000000-0000-4000-8000-000000000003";
const RESIDENT: &str = "00000000-0000-4000-8000-000000000004";
const CONFIG: &str = "00000000-0000-4000-8000-000000000005";

const RECORDED_AT_MS: u64 = 1_788_000_000_000;
const CONTENT_BYTES: u64 = 11_638;

/// ADR 0043's published vector: 221 bytes, produced independently in Python from the field table.
const VECTOR_SHA256: &str = "83e3056b4cc665a458d279fa8602f9b137f87cc8032b5a5037afab614ed4eff0";

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("canonical uuid")
}

fn content_digest(value: [u8; 32]) -> ArtifactDigest {
    ArtifactDigest {
        algorithm: "sha256".to_owned(),
        canonicalizer: RAW_CONTENT.to_owned(),
        value: value.iter().map(|byte| format!("{byte:02x}")).collect(),
    }
}

/// The vector record: one outcome-document Artifact, mission-scoped, first of its chain.
fn vector_artifact() -> ArtifactV1 {
    ArtifactV1 {
        workspace_id: uuid(WORKSPACE),
        kind: ArtifactKind::OutcomeDocument,
        media_type: MEDIA_MARKDOWN.to_owned(),
        content_digest: content_digest([0x66; 32]),
        content_bytes: CONTENT_BYTES,
        accountable_resident_id: uuid(RESIDENT),
        accountable_configuration_id: uuid(CONFIG),
        context: ArtifactContext::Mission {
            project_id: uuid(PROJECT),
            mission_id: uuid(MISSION),
        },
        supersedes: None,
        recorded_at_ms: RECORDED_AT_MS,
    }
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

// ---------------------------------------------------------------------------------------------
// An independent assembly: ADR 0043's published table, not Beacon's encoder
// ---------------------------------------------------------------------------------------------

fn push_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u64).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

/// The vector record's canonical bytes, assembled from ADR 0043's table by hand.
fn vector_bytes_by_hand() -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(uuid(WORKSPACE).as_bytes());
    out.push(0x01); // kind = outcome_document
    push_string(&mut out, MEDIA_MARKDOWN);
    push_string(&mut out, "sha256");
    push_string(&mut out, RAW_CONTENT);
    out.extend_from_slice(&[0x66; 32]);
    out.extend_from_slice(&CONTENT_BYTES.to_be_bytes());
    out.extend_from_slice(uuid(RESIDENT).as_bytes());
    out.extend_from_slice(uuid(CONFIG).as_bytes());
    out.push(0x01); // context = mission
    out.extend_from_slice(uuid(PROJECT).as_bytes());
    out.extend_from_slice(uuid(MISSION).as_bytes());
    out.push(0x00); // supersedes = none
    out.extend_from_slice(&RECORDED_AT_MS.to_be_bytes());
    out
}

/// Finds a needle in the canonical bytes, for refusal probes at exact positions.
fn find(bytes: &[u8], needle: &[u8]) -> usize {
    bytes
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("needle present")
}

// ---------------------------------------------------------------------------------------------
// The slice-1 tests of the design, one per bullet
// ---------------------------------------------------------------------------------------------

#[test]
fn a_known_artifact_fixture_reproduces_its_published_digest() {
    let bytes = vector_artifact().canonical_bytes().expect("encodes");
    assert_eq!(
        bytes,
        vector_bytes_by_hand(),
        "Beacon's encoder and ADR 0043's published table disagree"
    );
    assert_eq!(bytes.len(), 221, "the published vector's byte count moved");
    assert_eq!(
        sha256_hex(&bytes),
        VECTOR_SHA256,
        "the published vector's digest moved"
    );
    let digest = vector_artifact().digest().expect("digest");
    assert_eq!(digest.value, VECTOR_SHA256);
    assert_eq!(digest.algorithm, "sha256");
    assert_eq!(digest.canonicalizer, CANONICALIZER);
    // The identity is proved, not stored: the bytes-proved wrapper agrees.
    let proved = Artifact::from_canonical_bytes(&bytes).expect("verified");
    assert_eq!(proved.digest().value, VECTOR_SHA256);
    assert_eq!(proved.canonical_bytes(), bytes.as_slice());
    assert_eq!(proved.payload(), &vector_artifact());
}

#[test]
fn each_identity_bearing_field_changes_the_digest() {
    let base = vector_artifact().digest().expect("digest").value;
    let mut seen = vec![base];

    let mut mutations: Vec<(&str, ArtifactV1)> = Vec::new();
    let mut moved = vector_artifact();
    moved.workspace_id = uuid(PROJECT);
    mutations.push(("workspace", moved));
    let mut moved = vector_artifact();
    moved.content_digest = content_digest([0x67; 32]);
    mutations.push(("content digest value", moved));
    let mut moved = vector_artifact();
    moved.content_bytes += 1;
    mutations.push(("content byte count", moved));
    let mut moved = vector_artifact();
    moved.accountable_resident_id = uuid(CONFIG);
    mutations.push(("accountable Resident", moved));
    let mut moved = vector_artifact();
    moved.accountable_configuration_id = uuid(RESIDENT);
    mutations.push(("accountable Configuration", moved));
    let mut moved = vector_artifact();
    moved.context = ArtifactContext::None;
    mutations.push(("context", moved));
    let mut moved = vector_artifact();
    moved.supersedes = Some([0x99; 32]);
    mutations.push(("supersedes", moved));
    let mut moved = vector_artifact();
    moved.recorded_at_ms += 1;
    mutations.push(("recorded time", moved));

    for (label, manifest) in &mutations {
        let digest = manifest.digest().expect("digest").value;
        assert!(
            !seen.contains(&digest),
            "changing the {label} did not change the Artifact identity"
        );
        seen.push(digest);
    }
    assert_eq!(seen.len(), mutations.len() + 1);

    // The kind and the media type are identity-bearing too, but a writer cannot vary them alone:
    // the kind/media relation is closed, so the only spellings are refused ones (proved by the
    // refusal tests below), and a flipped kind byte in the bytes is an unknown tag (also below).
}

#[test]
fn one_changed_byte_trailing_byte_or_unknown_tag_is_refused() {
    let bytes = vector_bytes_by_hand();

    let mut broken_magic = bytes.clone();
    broken_magic[0] ^= 0xff;
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&broken_magic).expect_err("refuses"),
        MalformedArtifact::Magic
    );

    let mut trailing = bytes.clone();
    trailing.push(0x00);
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&trailing).expect_err("refuses"),
        MalformedArtifact::TrailingBytes
    );

    let mut truncated = bytes.clone();
    truncated.pop();
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&truncated).expect_err("refuses"),
        MalformedArtifact::Truncated
    );

    // The kind byte sits immediately after the magic and the Workspace UUID. An unassigned tag
    // there is refused, not defaulted.
    let kind_offset = MAGIC.len() + 16;
    assert_eq!(bytes[kind_offset], 0x01);
    let mut unknown_tag = bytes.clone();
    unknown_tag[kind_offset] = 0x07;
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&unknown_tag).expect_err("refuses"),
        MalformedArtifact::UnknownTag
    );

    // A changed byte inside the content digest value changes the identity but still decodes:
    // it is a different Artifact, never a mistaken reading of this one.
    let mut other = bytes.clone();
    let digest_offset = find(&bytes, &[0x66; 32]);
    other[digest_offset] = 0x67;
    let decoded = ArtifactV1::from_canonical_bytes(&other).expect("decodes");
    assert_ne!(
        decoded.digest().expect("digest").value,
        VECTOR_SHA256,
        "a changed content digest byte did not change the identity"
    );
}

#[test]
fn a_zero_filled_buffer_is_not_a_valid_record() {
    let zeros = vec![0x00; 221];
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&zeros).expect_err("refuses"),
        MalformedArtifact::Magic
    );
    // With the magic intact, a zero-filled body fails at the kind tag: 0x00 is unassigned there,
    // so a zeroed buffer can never decode as a valid record.
    let mut body_zeros = vec![0x00; 221];
    body_zeros[..MAGIC.len()].copy_from_slice(MAGIC);
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&body_zeros).expect_err("refuses"),
        MalformedArtifact::UnknownTag
    );
}

#[test]
fn a_media_type_the_kind_does_not_own_is_refused_by_the_codec() {
    // Encode direction: a caller cannot pair `outcome_document` with another media type.
    let mut wrong = vector_artifact();
    wrong.media_type = "text/plain".to_owned();
    assert_eq!(
        wrong.canonical_bytes().expect_err("refuses"),
        MalformedArtifact::MediaTypeForKind
    );

    // Decode direction: hand-built bytes naming another media type are refused at the same rule,
    // so a record that could not be written cannot be read back either.
    let mut bytes = vector_bytes_by_hand();
    let start = find(&bytes, MEDIA_MARKDOWN.as_bytes());
    let replacement = b"text/plain; charset=utf-8   ";
    assert_eq!(replacement.len(), MEDIA_MARKDOWN.len());
    bytes[start..start + MEDIA_MARKDOWN.len()].copy_from_slice(replacement);
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&bytes).expect_err("refuses"),
        MalformedArtifact::MediaTypeForKind
    );
}

#[test]
fn an_unassigned_kind_tag_is_refused() {
    let kind_offset = MAGIC.len() + 16;
    for tag in [0x00_u8, 0x02, 0xff] {
        let mut bytes = vector_bytes_by_hand();
        bytes[kind_offset] = tag;
        assert_eq!(
            ArtifactV1::from_canonical_bytes(&bytes).expect_err("refuses"),
            MalformedArtifact::UnknownTag,
            "kind tag {tag:#04x} was accepted"
        );
    }
}

#[test]
fn a_record_over_4_kib_is_refused_before_any_allocation() {
    // The envelope is checked before the magic: an over-long input is refused as over-limit, not
    // as whatever its first bytes happen to fail.
    let mut over = vector_bytes_by_hand();
    over.resize(4_097, 0x00);
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&over).expect_err("refuses"),
        MalformedArtifact::OverLimitLength
    );
    // The field table admits at most ~260 bytes, so the encoder cannot reach the envelope; the
    // bound is defensive and stated, in ADR 0038's shape.
    assert!(vector_bytes_by_hand().len() <= 4_096);
}

#[test]
fn a_declared_length_that_cannot_fit_the_remaining_input_is_refused_by_arithmetic() {
    let bytes = vector_bytes_by_hand();
    // The media type's u64 length prefix sits right after the kind byte.
    let length_offset = MAGIC.len() + 16 + 1;

    // Beyond the field's own limit: refused as over-limit before the remaining input is consulted.
    let mut over_limit = bytes.clone();
    over_limit[length_offset..length_offset + 8]
        .copy_from_slice(&u64::MAX.to_be_bytes());
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&over_limit).expect_err("refuses"),
        MalformedArtifact::OverLimitLength
    );

    // Within the field's limit but beyond what remains: refused as truncated, by arithmetic,
    // before any byte is copied. u64::MAX above also proves the length arithmetic cannot overflow.
    let mut foreshortened = bytes.clone();
    foreshortened.truncate(length_offset + 8 + 4);
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&foreshortened).expect_err("refuses"),
        MalformedArtifact::Truncated
    );
}

#[test]
fn the_decoder_consumes_the_entire_input_and_reencode_is_byte_identical() {
    let bytes = vector_bytes_by_hand();
    let decoded = ArtifactV1::from_canonical_bytes(&bytes).expect("decodes");
    assert_eq!(decoded, vector_artifact());
    assert_eq!(
        decoded.canonical_bytes().expect("re-encodes"),
        bytes,
        "decode and re-encode are not byte-identical"
    );
}

#[test]
fn none_arms_encode_as_values_and_a_missing_field_refuses() {
    // `context = none` and `supersedes = none` are values, never missing fields.
    let mut contextless = vector_artifact();
    contextless.context = ArtifactContext::None;
    let bytes = contextless.canonical_bytes().expect("encodes");
    let decoded = ArtifactV1::from_canonical_bytes(&bytes).expect("decodes");
    assert_eq!(decoded, contextless);
    assert_eq!(decoded.context, ArtifactContext::None);

    // Removing the supersedes tag entirely leaves the record ending early: refused, not defaulted.
    let mut missing = bytes;
    missing.pop();
    missing.pop();
    missing.pop();
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&missing).expect_err("refuses"),
        MalformedArtifact::Truncated
    );
}

#[test]
fn a_mission_scoped_context_carries_the_project_and_mission() {
    let artifact = vector_artifact();
    let bytes = artifact.canonical_bytes().expect("encodes");
    let decoded = ArtifactV1::from_canonical_bytes(&bytes).expect("decodes");
    assert_eq!(
        decoded.context,
        ArtifactContext::Mission {
            project_id: uuid(PROJECT),
            mission_id: uuid(MISSION),
        }
    );
    // The scope is inside the identity: the same record without it is a different Artifact.
    let mut contextless = vector_artifact();
    contextless.context = ArtifactContext::None;
    assert_ne!(
        contextless.digest().expect("digest").value,
        artifact.digest().expect("digest").value
    );
}
