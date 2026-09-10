//! C1-C11 and the §B.1 truth table, from the byte table frozen at round 3
//! (`B16-opus2-packet-byte-table.md`, SHA-256 `cf5cbf92…4d59`).
//!
//! Row names are the frozen ones. Where a row's case needs a live store fixture it is named in the
//! build report's "narrowed or deferred" section, never silently dropped.

use beacond::{
    ActivationFacts, CRITERION_IDS, EntryBody, EntryKind, EvidenceEntry, FidelityPacketRefusal,
    HeaderFacts, IncompleteCause, MalformedCode, MalformedFidelityPacket, OVER_LIMIT_WITNESS,
    PROFILE, PacketCap, PacketGap, PacketInputs, RESPONSE_CEILING, RecipeFacts, RequestKey,
    ResultConflicts, ResultOutcome, ResultVerdict, RubricFacts, SubjectFacts, TerminalObservation,
    TransportObservation, VerifiedEvaluationPacket, WireDigest, admits_entry_count, classify,
    encode_packet, min_entry_bytes,
};
use uuid::Uuid;

const RUBRIC_DIGEST: [u8; 32] = [3_u8; 32];

/// An INDEPENDENT hash, so the assertions below do not compare the implementation with itself.
fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let mut out = [0_u8; 32];
    out.copy_from_slice(&hasher.finalize());
    out
}

fn fixture_blob(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn fixture_digest(out: &mut Vec<u8>, canonicalizer: &str, fill: u8) {
    fixture_blob(out, b"sha256");
    fixture_blob(out, canonicalizer.as_bytes());
    out.extend_from_slice(&[fill; 32]);
}

fn fixture_frame(out: &mut Vec<u8>, tag: u8, body: Vec<u8>) {
    out.push(tag);
    out.extend_from_slice(&(body.len() as u64).to_be_bytes());
    out.extend_from_slice(&body);
}

/// Independently transcribed from frozen table §A; it calls no production encoder/decoder helper.
fn table_fixture_bytes() -> Vec<u8> {
    let mut out = b"beacon.outcome-fidelity.input.v1\0".to_vec();

    let mut s1 = Vec::new();
    s1.extend_from_slice(Uuid::from_u128(1).as_bytes());
    s1.extend_from_slice(&[1; 32]);
    s1.extend_from_slice(&[2; 32]);
    fixture_blob(&mut s1, b"outcome-fidelity-draft");
    s1.extend_from_slice(&1_u32.to_be_bytes());
    fixture_blob(&mut s1, b"beacon.raw.v1");
    s1.extend_from_slice(&[3; 32]);
    s1.extend_from_slice(Uuid::from_u128(10).as_bytes());
    s1.extend_from_slice(Uuid::from_u128(11).as_bytes());
    s1.push(0x02);
    s1.extend_from_slice(Uuid::from_u128(11).as_bytes());
    s1.push(0x01);
    s1.extend_from_slice(&1_u32.to_be_bytes());
    s1.push(0x06);
    s1.push(0x02);
    s1.extend_from_slice(Uuid::from_u128(20).as_bytes());
    s1.extend_from_slice(Uuid::from_u128(20).as_bytes());
    s1.push(0x01);
    s1.extend_from_slice(&0_u32.to_be_bytes());
    s1.push(0x01);
    fixture_frame(&mut out, 0x01, s1);

    let mut s2 = Vec::new();
    fixture_digest(&mut s2, "beacon.artifact.v1", 4);
    fixture_blob(&mut s2, b"artifact-record");
    fixture_digest(&mut s2, "beacon.raw.v1", 5);
    fixture_blob(&mut s2, b"# outcome candidate");
    s2.extend_from_slice(&42_u64.to_be_bytes());
    fixture_frame(&mut out, 0x02, s2);

    let mut s3 = Vec::new();
    fixture_blob(&mut s3, b"manifest-bytes");
    fixture_digest(&mut s3, "beacon.outcome.manifest.v2", 6);
    fixture_blob(&mut s3, b"beacon.outcome.md.v1");
    fixture_frame(&mut out, 0x03, s3);

    let mut s4 = Vec::new();
    s4.extend_from_slice(&9_u32.to_be_bytes());
    let mut mission = Vec::new();
    mission.extend_from_slice(Uuid::from_u128(1).as_bytes());
    mission.extend_from_slice(Uuid::from_u128(99).as_bytes());
    fixture_blob(&mut mission, b"ship the thing");
    for item in [b"condition one".as_slice(), b"no network", b"never publish"] {
        mission.extend_from_slice(&1_u32.to_be_bytes());
        fixture_blob(&mut mission, item);
    }
    fixture_frame(&mut s4, 0x01, mission);
    for (tag, canonicalizer, fill, size) in [
        (0x02, "beacon.mission_completion.v1", 2, 8),
        (0x04, "beacon.proposal.v1", 4, 8),
        (0x05, "beacon.ruling.v2", 5, 24),
        (0x06, "beacon.evaluation.v1", 6, 8),
        (0x07, "beacon.mission_impact.v1", 7, 8),
        (0x08, "beacon.spend.v2", 8, 8),
    ] {
        let mut record = Vec::new();
        fixture_digest(&mut record, canonicalizer, fill);
        fixture_blob(&mut record, &vec![fill; size]);
        fixture_frame(&mut s4, tag, record);
        if tag == 0x02 {
            let mut artifact = Vec::new();
            fixture_digest(&mut artifact, "beacon.artifact.v1", 3);
            fixture_blob(&mut artifact, &[3; 4]);
            fixture_digest(&mut artifact, "beacon.raw.v1", 3);
            fixture_blob(&mut artifact, &[3; 4]);
            fixture_frame(&mut s4, 0x03, artifact);
        }
    }
    let mut receipt = Vec::new();
    receipt.extend_from_slice(Uuid::from_u128(200).as_bytes());
    receipt.extend_from_slice(&1_u32.to_be_bytes());
    fixture_blob(&mut receipt, b"beacon.outcome.receipt-checksum.v1");
    receipt.extend_from_slice(&[7; 32]);
    fixture_blob(&mut receipt, br#"{"version":1}"#);
    fixture_frame(&mut s4, 0x09, receipt);
    fixture_frame(&mut out, 0x04, s4);

    let mut s5 = Vec::new();
    fixture_blob(&mut s5, b"rubric bytes");
    s5.extend_from_slice(&4_u32.to_be_bytes());
    for id in CRITERION_IDS {
        fixture_blob(&mut s5, id.as_bytes());
    }
    fixture_frame(&mut out, 0x05, s5);
    out
}

fn cap() -> PacketCap {
    PacketCap::new(64 * 1024 * 1024)
}

fn digest(canonicalizer: &str, fill: u8) -> WireDigest {
    WireDigest {
        canonicalizer: canonicalizer.to_owned(),
        value: [fill; 32],
    }
}

fn header() -> HeaderFacts {
    HeaderFacts {
        workspace_id: Uuid::from_u128(1),
        manifest_digest: [1_u8; 32],
        candidate_record_digest: [2_u8; 32],
        rubric_id: "outcome-fidelity-draft".to_owned(),
        rubric_version: 1,
        rubric_digest: RUBRIC_DIGEST,
        admitted: beacond::Evaluator {
            resident_id: Uuid::from_u128(10),
            configuration_id: Uuid::from_u128(11),
        },
        current_lifecycle: beacond::ResidentLifecycle::Active,
        current_configuration_id: Uuid::from_u128(11),
        current_configuration_state: beacond::ResidentConfigurationState::Configured,
        capabilities: vec![beacond::BeaconCapability::ArtifactEvaluate],
        activation: Some(ActivationFacts {
            configured_activation_id: Uuid::from_u128(20),
            latest_activation_id: Uuid::from_u128(20),
            latest_state: beacond::LocalModelActivationState::Active,
        }),
        gaps: Vec::new(),
    }
}

fn inputs(evidence: Vec<EvidenceEntry>) -> PacketInputs {
    PacketInputs {
        header: header(),
        subject: SubjectFacts {
            record_digest: digest("beacon.artifact.v1", 4),
            record_bytes: b"artifact-record".to_vec(),
            content_digest: digest("beacon.raw.v1", 5),
            content_bytes: b"# outcome candidate".to_vec(),
            creation_event_seq: 42,
        },
        recipe: RecipeFacts {
            manifest_bytes: b"manifest-bytes".to_vec(),
            manifest_digest: digest("beacon.outcome.manifest.v2", 6),
            renderer_version: "beacon.outcome.md.v1".to_owned(),
        },
        evidence,
        rubric: RubricFacts {
            bytes: b"rubric bytes".to_vec(),
            criterion_ids: CRITERION_IDS.iter().map(|id| (*id).to_owned()).collect(),
        },
    }
}

fn record_entry(kind: EntryKind, canonicalizer: &str, fill: u8) -> EvidenceEntry {
    // A Ruling's stored row is 17..65_536 (`0022_candidates.sql:168`), so the fixture respects the
    // owner's floor rather than encoding a row the store itself would refuse.
    let size = if matches!(kind, EntryKind::Ruling) {
        24
    } else {
        8
    };
    EvidenceEntry {
        kind,
        body: EntryBody::Record {
            digest: digest(canonicalizer, fill),
            bytes: vec![fill; size],
        },
    }
}

fn artifact_entry(fill: u8) -> EvidenceEntry {
    EvidenceEntry {
        kind: EntryKind::Artifact,
        body: EntryBody::Artifact {
            record_digest: digest("beacon.artifact.v1", fill),
            record_bytes: vec![fill; 4],
            content_digest: digest("beacon.raw.v1", fill),
            content_bytes: vec![fill; 4],
        },
    }
}

fn mission_entry(id: u128) -> EvidenceEntry {
    EvidenceEntry {
        kind: EntryKind::Mission,
        body: EntryBody::Mission {
            mission_id: Uuid::from_u128(id),
            project_id: Uuid::from_u128(99),
            desired_outcome: "ship the thing".to_owned(),
            success_conditions: vec!["condition one".to_owned()],
            constraints: vec!["no network".to_owned()],
            prohibitions: vec!["never publish".to_owned()],
        },
    }
}

fn receipt_entry(run: u128, version: u32) -> EvidenceEntry {
    EvidenceEntry {
        kind: EntryKind::Receipt,
        body: EntryBody::Receipt {
            run_uuid: Uuid::from_u128(run),
            version,
            checksum_label: "beacon.outcome.receipt-checksum.v1".to_owned(),
            checksum: [7_u8; 32],
            receipt_json: format!(r#"{{"version":{version}}}"#).into_bytes(),
        },
    }
}

/// One of each kind, deliberately in DESCENDING kind order so insertion order cannot coincide
/// with the frozen sort.
fn one_of_each_reversed() -> Vec<EvidenceEntry> {
    vec![
        receipt_entry(200, 1),
        record_entry(EntryKind::Spend, "beacon.spend.v2", 8),
        record_entry(EntryKind::Policy, "beacon.mission_impact.v1", 7),
        record_entry(EntryKind::Evaluation, "beacon.evaluation.v1", 6),
        record_entry(EntryKind::Ruling, "beacon.ruling.v2", 5),
        record_entry(EntryKind::Proposal, "beacon.proposal.v1", 4),
        artifact_entry(3),
        record_entry(EntryKind::Completion, "beacon.mission_completion.v1", 2),
        mission_entry(1),
    ]
}

fn packet(evidence: Vec<EvidenceEntry>) -> VerifiedEvaluationPacket {
    encode_packet(&inputs(evidence), cap()).expect("packet encodes")
}

// ---------------------------------------------------------------------------------------------
// Result-body helpers
// ---------------------------------------------------------------------------------------------

fn criteria_json(verdict: &str) -> String {
    CRITERION_IDS
        .iter()
        .map(|id| format!(r#"{{"id":"{id}","verdict":"{verdict}"}}"#))
        .collect::<Vec<_>>()
        .join(",")
}

struct Body {
    profile: String,
    packet_sha256: String,
    rubric_sha256: String,
    criteria: String,
    overall: String,
    findings: String,
    conflicts: String,
    rationale: String,
}

impl Body {
    fn valid(packet: &VerifiedEvaluationPacket) -> Self {
        Self {
            profile: PROFILE.to_owned(),
            packet_sha256: packet.digest().value.clone(),
            rubric_sha256: data_encoding::HEXLOWER.encode(&RUBRIC_DIGEST),
            criteria: criteria_json("meets"),
            overall: "meets".to_owned(),
            findings: String::new(),
            conflicts: r#"{"state":"none"}"#.to_owned(),
            rationale: "the document matches its frozen sources".to_owned(),
        }
    }

    fn render(&self) -> String {
        format!(
            r#"{{"profile":"{}","packet_sha256":"{}","rubric_sha256":"{}","criteria":[{}],"overall_verdict":"{}","findings":[{}],"conflicts":{},"rationale":"{}"}}"#,
            self.profile,
            self.packet_sha256,
            self.rubric_sha256,
            self.criteria,
            self.overall,
            self.findings,
            self.conflicts,
            self.rationale
        )
    }
}

fn complete(bytes: &[u8]) -> TerminalObservation {
    TerminalObservation::Complete {
        bytes: bytes.to_vec(),
        elapsed_ms: 1,
    }
}

fn incomplete(prefix: Vec<u8>, cause: IncompleteCause) -> TerminalObservation {
    TerminalObservation::Incomplete { prefix, cause }
}

fn outcome(observation: &TerminalObservation, packet: &VerifiedEvaluationPacket) -> ResultOutcome {
    classify(observation, packet, &RUBRIC_DIGEST).expect("coherent observation")
}

fn malformed_code(
    observation: &TerminalObservation,
    packet: &VerifiedEvaluationPacket,
) -> MalformedCode {
    match outcome(observation, packet) {
        ResultOutcome::Malformed { code, .. } => code,
        other => panic!("expected Malformed, got {}", other.name()),
    }
}

// ---------------------------------------------------------------------------------------------
// C1
// ---------------------------------------------------------------------------------------------

#[test]
fn packet_contains_exact_subject_and_independent_owner_bytes_in_canonical_order() {
    let built = packet(one_of_each_reversed());
    assert_eq!(
        built.bytes(),
        table_fixture_bytes(),
        "the full packet must equal the independently transcribed frozen table"
    );

    // S4 order is the frozen sort, not the insertion order.
    let kinds: Vec<EntryKind> = built.entries().iter().map(|entry| entry.kind).collect();
    assert_eq!(
        kinds,
        vec![
            EntryKind::Mission,
            EntryKind::Completion,
            EntryKind::Artifact,
            EntryKind::Proposal,
            EntryKind::Ruling,
            EntryKind::Evaluation,
            EntryKind::Policy,
            EntryKind::Spend,
            EntryKind::Receipt,
        ],
        "entries must sort by owner kind tag, never by insertion order"
    );
    // Ordinals are dense, zero-based, and the label is the decimal string a reply must cite.
    for (index, entry) in built.entries().iter().enumerate() {
        assert_eq!(entry.ordinal as usize, index);
        assert_eq!(entry.label, index.to_string());
    }

    // The subject's exact bytes are present in the packet, unaltered.
    assert!(
        built
            .bytes()
            .windows(19)
            .any(|window| window == b"# outcome candidate"),
        "S2 must carry the candidate's exact content bytes"
    );

    // Byte-for-byte stability against a second build of the same inputs.
    let again = packet(one_of_each_reversed());
    assert_eq!(built.bytes(), again.bytes());
    assert_eq!(built.digest().value, again.digest().value);
}

// ---------------------------------------------------------------------------------------------
// C2
// ---------------------------------------------------------------------------------------------

#[test]
fn packet_preserves_owner_scoped_receipt_versions_and_exact_stored_json() {
    // Each admitted version is carried exactly, and the exact stored JSON travels with it.
    for version in 1..=4_u32 {
        let built = packet(vec![receipt_entry(300, version)]);
        let decoded = beacond::decode_packet(built.bytes()).expect("the packet decodes");
        let [entry] = decoded.inputs().evidence.as_slice() else {
            panic!("the receipt fixture must decode to exactly one entry");
        };
        match &entry.body {
            EntryBody::Receipt {
                version: carried_version,
                receipt_json,
                ..
            } => {
                assert_eq!(*carried_version, version, "the Receipt slot carries its version");
                assert_eq!(
                    receipt_json,
                    format!(r#"{{"version":{version}}}"#).as_bytes(),
                    "the exact stored receipt_json must travel, not a re-serialisation"
                );
            }
            other => panic!("expected Receipt body, got {other:?}"),
        }
    }
    // A version outside the SQL domain refuses by its own name.
    let refusal = encode_packet(&inputs(vec![receipt_entry(300, 5)]), cap())
        .expect_err("version 5 is outside 1..=4");
    assert_eq!(
        refusal,
        FidelityPacketRefusal::Malformed(MalformedFidelityPacket::ReceiptVersion)
    );
}

// ---------------------------------------------------------------------------------------------
// C3
// ---------------------------------------------------------------------------------------------

#[test]
fn packet_refuses_missing_or_oversized_owner_source_before_returning_partial_bytes() {
    // One source over its owner ceiling: refused, and no packet bytes exist to inspect.
    let oversized = EvidenceEntry {
        kind: EntryKind::Completion,
        body: EntryBody::Record {
            digest: digest("beacon.mission_completion.v1", 2),
            bytes: vec![0_u8; 65_537],
        },
    };
    let refusal = encode_packet(&inputs(vec![oversized]), cap())
        .expect_err("a source over its owner ceiling refuses");
    assert_eq!(
        refusal,
        FidelityPacketRefusal::SourceOverOwnerCeiling { kind: "completion" }
    );

    // Individually legal sources whose AGGREGATE alone exceeds the caller cap.
    let each = record_entry(EntryKind::Proposal, "beacon.proposal.v1", 4);
    let many: Vec<EvidenceEntry> = (0..64)
        .map(|index| EvidenceEntry {
            kind: EntryKind::Proposal,
            body: EntryBody::Record {
                digest: digest("beacon.proposal.v1", index as u8),
                bytes: vec![index as u8; 1024],
            },
        })
        .collect();
    let small = PacketCap::new(4_096);
    match encode_packet(&inputs(many), small) {
        Err(FidelityPacketRefusal::AggregateOverCap { cap, charged }) => {
            assert_eq!(cap, 4_096);
            assert!(charged > cap, "the refusal names charged and cap");
        }
        other => panic!("expected AggregateOverCap, got {other:?}"),
    }

    // R59.4: repeated BYTE-EQUAL sources are charged pre-dedup, so the charge alone crosses the cap
    // even though the encoded packet would deduplicate them to one.
    let duplicates: Vec<EvidenceEntry> = std::iter::repeat_n(each.clone(), 200).collect();
    let deduped = packet(vec![each]);
    let just_over = PacketCap::new(deduped.bytes().len() as u64 + 4_096);
    match encode_packet(&inputs(duplicates), just_over) {
        Err(FidelityPacketRefusal::AggregateOverCap { cap, charged }) => {
            assert!(
                charged > cap,
                "the pre-dedup charge must cross a cap the deduplicated encoding fits under"
            );
        }
        other => panic!("expected AggregateOverCap from the pre-dedup charge, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// C4, C5, C6, C7
// ---------------------------------------------------------------------------------------------

#[test]
fn valid_uncertain_response_is_a_complete_judgment_not_a_failure() {
    let built = packet(one_of_each_reversed());
    let mut body = Body::valid(&built);
    body.criteria = criteria_json("uncertain");
    body.overall = "uncertain".to_owned();
    let rendered = body.render();
    match outcome(&complete(rendered.as_bytes()), &built) {
        ResultOutcome::Valid { judgment, .. } => {
            assert_eq!(judgment.overall_verdict, ResultVerdict::Uncertain);
            assert!(
                judgment
                    .criteria
                    .iter()
                    .all(|criterion| criterion.verdict == ResultVerdict::Uncertain)
            );
        }
        other => panic!("uncertain is a complete judgment, got {}", other.name()),
    }
}

#[test]
fn overlimit_response_hashes_only_the_bounded_prefix_and_creates_no_evaluation() {
    let built = packet(one_of_each_reversed());
    let long = vec![b'x'; 200_000];

    // The adapter's shape: an Incomplete carrying exactly C + 1 retained bytes.
    let witness = incomplete(
        long[..OVER_LIMIT_WITNESS].to_vec(),
        IncompleteCause::ResponseOverLimit,
    );
    let ceiling_only = sha256(&long[..RESPONSE_CEILING]);
    match outcome(&witness, &built) {
        ResultOutcome::OverLimit {
            prefix_sha256,
            observed_bytes,
        } => {
            assert_eq!(observed_bytes, OVER_LIMIT_WITNESS);
            assert_eq!(prefix_sha256, sha256(&long[..OVER_LIMIT_WITNESS]));
            assert_ne!(
                prefix_sha256, ceiling_only,
                "the digest must name C + 1 bytes, not the ceiling"
            );
        }
        other => panic!("expected OverLimit, got {}", other.name()),
    }

    // A retained COMPLETE observation longer than the ceiling is over-limit too: the C2 grammar
    // admits one even though this adapter never emits one.
    match outcome(&complete(&long), &built) {
        ResultOutcome::OverLimit {
            prefix_sha256,
            observed_bytes,
        } => {
            assert_eq!(observed_bytes, OVER_LIMIT_WITNESS);
            assert_eq!(prefix_sha256, sha256(&long[..OVER_LIMIT_WITNESS]));
            assert_ne!(prefix_sha256, ceiling_only);
        }
        other => panic!(
            "complete-over-ceiling must be OverLimit, got {}",
            other.name()
        ),
    }
}

#[test]
fn missing_malformed_or_out_of_packet_response_never_becomes_a_valid_result() {
    let built = packet(one_of_each_reversed());

    // Zero-byte incomplete is Missing, and is NOT the complete-empty case.
    assert_eq!(
        outcome(
            &incomplete(Vec::new(), IncompleteCause::TransportRefused),
            &built
        ),
        ResultOutcome::Missing
    );

    // An unknown key, a missing key, and a duplicated key all refuse under one code.
    let base = Body::valid(&built).render();
    let unknown = base.replace(r#""rationale":"#, r#""extra":1,"rationale":"#);
    assert_eq!(
        malformed_code(&complete(unknown.as_bytes()), &built),
        MalformedCode::UnknownOrDuplicateField
    );
    let missing = base.replace(
        r#","rationale":"the document matches its frozen sources""#,
        "",
    );
    assert_eq!(
        malformed_code(&complete(missing.as_bytes()), &built),
        MalformedCode::UnknownOrDuplicateField
    );
    let duplicated = base.replace(r#""overall_verdict""#, r#""profile":"x","overall_verdict""#);
    assert_eq!(
        malformed_code(&complete(duplicated.as_bytes()), &built),
        MalformedCode::UnknownOrDuplicateField
    );

    // Key closure applies at every object depth; serde_json::Value must not collapse duplicates.
    let nested_cases = [
        base.replace(
            r#""id":"completion_fidelity""#,
            r#""id":"completion_fidelity","id":"completion_fidelity""#,
        ),
        base.replace(r#""verdict":"meets""#, r#""verdict":"meets","extra":true"#),
        base.replace(r#""state":"none""#, r#""state":"none","state":"none""#),
        base.replace(r#","verdict":"meets""#, ""),
    ];
    for nested in nested_cases {
        assert_eq!(
            malformed_code(&complete(nested.as_bytes()), &built),
            MalformedCode::UnknownOrDuplicateField,
            "nested unknown, missing, and duplicate keys share the frozen refusal"
        );
    }

    // Identity is compared against HELD values, never adopted.
    let mut wrong_packet = Body::valid(&built);
    wrong_packet.packet_sha256 = data_encoding::HEXLOWER.encode(&[0xAB_u8; 32]);
    assert_eq!(
        malformed_code(&complete(wrong_packet.render().as_bytes()), &built),
        MalformedCode::PacketDigest
    );
    let mut wrong_rubric = Body::valid(&built);
    wrong_rubric.rubric_sha256 = data_encoding::HEXLOWER.encode(&[0xCD_u8; 32]);
    assert_eq!(
        malformed_code(&complete(wrong_rubric.render().as_bytes()), &built),
        MalformedCode::RubricDigest
    );
    let mut wrong_profile = Body::valid(&built);
    wrong_profile.profile = "beacon.outcome-fidelity.result.v2".to_owned();
    assert_eq!(
        malformed_code(&complete(wrong_profile.render().as_bytes()), &built),
        MalformedCode::Profile
    );

    // An evidence ordinal the packet does not carry.
    let count = built.entries().len();
    let mut out_of_packet = Body::valid(&built);
    out_of_packet.findings = format!(
        r#"{{"criterion":"honesty_states","severity":"note","summary":"s","evidence":["{count}"]}}"#
    );
    assert_eq!(
        malformed_code(&complete(out_of_packet.render().as_bytes()), &built),
        MalformedCode::EvidenceReference
    );

    // Malformed JSON syntax is Shape.
    assert_eq!(
        malformed_code(&complete(b"{"), &built),
        MalformedCode::Shape
    );
}

#[test]
fn completed_empty_response_is_malformed_not_missing() {
    let built = packet(one_of_each_reversed());
    match outcome(&complete(&[]), &built) {
        ResultOutcome::Malformed {
            code,
            complete_sha256,
        } => {
            assert_eq!(code, MalformedCode::Shape);
            assert_eq!(
                data_encoding::HEXLOWER.encode(&complete_sha256),
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                "the digest is over zero bytes"
            );
        }
        other => panic!(
            "a transport-complete empty body is Malformed, got {}",
            other.name()
        ),
    }
}

// ---------------------------------------------------------------------------------------------
// C8, C9, C10, C11
// ---------------------------------------------------------------------------------------------

#[test]
fn packet_min_entry_bytes_are_the_measured_minimum() {
    // The nine-kind measurement itself is `candidate_evaluation_packet::tests::
    // min_entry_bytes_is_the_measured_minimum` (it needs the encoder's private inputs). This bar
    // owns the other half: that the constants are USED, before anything reserves.
    assert!(min_entry_bytes(EntryKind::Receipt) > min_entry_bytes(EntryKind::Mission));
    assert!(admits_entry_count(2, 10_000).is_ok());
    assert_eq!(
        admits_entry_count(256, 100),
        Err(MalformedFidelityPacket::EntryCount),
        "T:C8 names this EntryCount: the fault is the declared count, not the input length"
    );
    assert_eq!(
        admits_entry_count(257, u64::MAX),
        Err(MalformedFidelityPacket::EntryCount)
    );
}

#[test]
fn valid_result_text_is_recordable_human_text() {
    let built = packet(one_of_each_reversed());
    let finding = |summary: &str| {
        format!(
            r#"{{"criterion":"honesty_states","severity":"note","summary":"{summary}","evidence":[]}}"#
        )
    };

    for summary in ["", " x", "x\\u0007y"] {
        let mut body = Body::valid(&built);
        body.findings = finding(summary);
        assert_eq!(
            malformed_code(&complete(body.render().as_bytes()), &built),
            MalformedCode::Finding,
            "summary {summary:?} is not recordable human text"
        );
    }

    // A single space trims to empty, so it is not human text either.
    let mut body = Body::valid(&built);
    body.rationale = " ".to_owned();
    assert_eq!(
        malformed_code(&complete(body.render().as_bytes()), &built),
        MalformedCode::Rationale
    );

    // Declared conflict text takes the same predicate, under its own code.
    for text in ["", " leading", "trailing ", "control\\u0007char"] {
        let mut body = Body::valid(&built);
        body.conflicts = format!(r#"{{"state":"declared","text":"{text}"}}"#);
        assert_eq!(
            malformed_code(&complete(body.render().as_bytes()), &built),
            MalformedCode::Conflict,
            "conflict text {text:?} is not recordable human text"
        );
    }

    // The positive control: declared conflicts with real text parse.
    let mut body = Body::valid(&built);
    body.conflicts =
        r#"{"state":"declared","text":"the evaluator wrote this candidate"}"#.to_owned();
    match outcome(&complete(body.render().as_bytes()), &built) {
        ResultOutcome::Valid { judgment, .. } => assert_eq!(
            judgment.conflicts,
            ResultConflicts::Declared("the evaluator wrote this candidate".to_owned())
        ),
        other => panic!("declared conflicts are valid, got {}", other.name()),
    }
}

#[test]
fn overall_verdict_is_never_derived_from_criteria_verdicts() {
    let built = packet(one_of_each_reversed());
    let mut body = Body::valid(&built);
    body.criteria = criteria_json("meets");
    body.overall = "does_not_meet".to_owned();
    match outcome(&complete(body.render().as_bytes()), &built) {
        ResultOutcome::Valid { judgment, .. } => {
            assert_eq!(judgment.overall_verdict, ResultVerdict::DoesNotMeet);
            assert!(
                judgment
                    .criteria
                    .iter()
                    .all(|criterion| criterion.verdict == ResultVerdict::Meets),
                "both halves survive; nothing is aggregated"
            );
        }
        other => panic!(
            "a disagreeing overall verdict is still valid, got {}",
            other.name()
        ),
    }
}

#[test]
fn evidence_entry_order_is_the_frozen_sort_not_collection_order() {
    // (a) Two collections of the same key with unchanged live state: identical bytes, one digest.
    let forward = one_of_each_reversed();
    let mut shuffled = forward.clone();
    shuffled.rotate_left(4);
    shuffled.swap(0, 8);
    let first = packet(forward);
    let second = packet(shuffled);
    assert_eq!(first.bytes(), second.bytes());
    assert_eq!(first.digest().value, second.digest().value);

    // (b) R68/H2. A live-state change between collections yields a DIFFERENT packet but the SAME
    // request key, so admission returns the held attempt rather than minting a second one.
    let mut moved = inputs(one_of_each_reversed());
    moved.header.current_lifecycle = beacond::ResidentLifecycle::Suspended;
    moved.header.gaps = vec![PacketGap::ResidentNotActive];
    let after = encode_packet(&moved, cap()).expect("packet encodes");
    assert_ne!(
        first.digest().value,
        after.digest().value,
        "admission-time evaluator facts are part of what the packet binds"
    );

    let key_of = |source: &PacketInputs| RequestKey {
        workspace_id: source.header.workspace_id,
        manifest_digest: source.header.manifest_digest,
        candidate_record_digest: source.header.candidate_record_digest,
        evaluator: source.header.admitted,
        rubric_digest: source.header.rubric_digest,
        purpose: "outcome_fidelity_evaluation".to_owned(),
    };
    assert_eq!(
        key_of(&inputs(Vec::new())).digest(),
        key_of(&moved).digest(),
        "the request key carries no moving fact, so idempotency survives the change"
    );
    moved.header.current_configuration_state = beacond::ResidentConfigurationState::Cleared;
    moved.header.activation = Some(ActivationFacts {
        configured_activation_id: Uuid::from_u128(20),
        latest_activation_id: Uuid::from_u128(21),
        latest_state: beacond::LocalModelActivationState::Inactive,
    });
    let cleared_and_superseded = encode_packet(&moved, cap()).expect("packet encodes");
    assert_ne!(after.bytes(), cleared_and_superseded.bytes());
    assert_eq!(
        key_of(&moved).digest(),
        key_of(&inputs(Vec::new())).digest(),
        "configuration clearing and activation supersession remain moving packet facts"
    );
}

// ---------------------------------------------------------------------------------------------
// §B.1 truth table
// ---------------------------------------------------------------------------------------------

/// Blue's sixteen rows, resolved: eleven reachable, each with exactly ONE outcome; four impossible
/// (`B = 0` cannot also be over the ceiling); one defined-but-unreachable (zero bytes are not
/// valid JSON, so `P = 1` cannot hold there).
#[test]
fn every_reachable_truth_table_row_has_exactly_one_outcome() {
    let built = packet(one_of_each_reversed());
    let valid = Body::valid(&built).render();
    let long = vec![b'y'; 200_000];
    let mut parseable_over_ceiling = valid.as_bytes().to_vec();
    parseable_over_ceiling.resize(OVER_LIMIT_WITNESS, b' ');

    // (B, T, W, P, expected outcome name)
    let rows: Vec<(bool, bool, bool, bool, &str, TerminalObservation)> = vec![
        (
            false,
            false,
            true,
            false,
            "Missing",
            incomplete(Vec::new(), IncompleteCause::TransportRefused),
        ),
        (
            false,
            false,
            true,
            true,
            "Missing",
            incomplete(Vec::new(), IncompleteCause::DeadlineElapsed),
        ),
        (false, true, true, false, "Malformed", complete(&[])),
        (
            true,
            false,
            false,
            false,
            "OverLimit",
            incomplete(
                long[..OVER_LIMIT_WITNESS].to_vec(),
                IncompleteCause::ResponseOverLimit,
            ),
        ),
        (
            true,
            false,
            false,
            true,
            "OverLimit",
            incomplete(
                parseable_over_ceiling.clone(),
                IncompleteCause::ResponseOverLimit,
            ),
        ),
        (
            true,
            false,
            true,
            false,
            "Incomplete",
            incomplete(b"{\"pro".to_vec(), IncompleteCause::CancelRequested),
        ),
        (
            true,
            false,
            true,
            true,
            "Incomplete",
            incomplete(valid.as_bytes().to_vec(), IncompleteCause::BodyReadRefused),
        ),
        (true, true, false, false, "OverLimit", complete(&long)),
        (
            true,
            true,
            false,
            true,
            "OverLimit",
            complete(&parseable_over_ceiling),
        ),
        (
            true,
            true,
            true,
            false,
            "Malformed",
            complete(b"{\"profile\":1}"),
        ),
        (true, true, true, true, "Valid", complete(valid.as_bytes())),
    ];

    for (b, t, w, p, expected, observation) in rows {
        let got = outcome(&observation, &built);
        assert_eq!(
            got.name(),
            expected,
            "row B={b} T={t} W={w} P={p} must classify as {expected}"
        );
    }

    // A prefix that happens to parse is STILL Incomplete: completeness is a transport fact, not a
    // grammar fact.
    assert_eq!(
        outcome(
            &incomplete(valid.as_bytes().to_vec(), IncompleteCause::DeadlineElapsed),
            &built
        )
        .name(),
        "Incomplete"
    );
}

/// The sixth state is a type error, not a rule: a non-terminal observation cannot be classified.
#[test]
fn a_non_terminal_observation_cannot_be_classified() {
    assert!(
        TerminalObservation::from_transport(&TransportObservation::Headers {
            status: 200,
            headers: Vec::new(),
        })
        .is_err()
    );
    assert!(
        TerminalObservation::from_transport(&TransportObservation::BodyPrefix {
            bytes: b"partial".to_vec(),
        })
        .is_err()
    );
    assert!(
        TerminalObservation::from_transport(&TransportObservation::BodyComplete {
            bytes: Vec::new(),
            elapsed_ms: 0,
        })
        .is_ok()
    );
}

/// R68/H5. The adapter never writes one; a stored one refuses as corruption, and is never Missing.
#[test]
fn an_incoherent_over_limit_observation_refuses_and_is_never_missing() {
    let built = packet(one_of_each_reversed());
    let short = incomplete(Vec::new(), IncompleteCause::ResponseOverLimit);
    assert!(
        classify(&short, &built, &RUBRIC_DIGEST).is_err(),
        "an over-limit stop retaining zero bytes contradicts itself and refuses"
    );
    let nearly = incomplete(
        vec![b'z'; RESPONSE_CEILING],
        IncompleteCause::ResponseOverLimit,
    );
    assert!(classify(&nearly, &built, &RUBRIC_DIGEST).is_err());
    // The coherent one classifies.
    let exact = incomplete(
        vec![b'z'; OVER_LIMIT_WITNESS],
        IncompleteCause::ResponseOverLimit,
    );
    assert_eq!(outcome(&exact, &built).name(), "OverLimit");
}

// ---------------------------------------------------------------------------------------------
// F3 — the decoder is the only reader, and it raises the framing refusals
// ---------------------------------------------------------------------------------------------

#[test]
fn the_decoder_round_trips_the_encoder_and_raises_every_framing_refusal() {
    let built = packet(one_of_each_reversed());
    let decoded =
        beacond::decode_packet(built.bytes()).expect("a packet this encoder wrote decodes");
    assert_eq!(decoded.entries().len(), 9);
    assert_eq!(
        encode_packet(decoded.inputs(), cap())
            .expect("re-encodes")
            .bytes(),
        built.bytes(),
        "decode then encode must reproduce the exact bytes"
    );

    let mut wrong = built.bytes().to_vec();
    wrong[0] = b'X';
    assert_eq!(
        beacond::decode_packet(&wrong),
        Err(MalformedFidelityPacket::Magic)
    );

    let mut trailing = built.bytes().to_vec();
    trailing.push(0);
    assert_eq!(
        beacond::decode_packet(&trailing),
        Err(MalformedFidelityPacket::TrailingBytes)
    );

    let mut reordered = built.bytes().to_vec();
    reordered[33] = 0x02;
    assert_eq!(
        beacond::decode_packet(&reordered),
        Err(MalformedFidelityPacket::SectionOrder)
    );

    // SectionLengthMismatch: S1's body carries one byte its fields do not consume -- distinct from
    // Truncated (a field running off the end) and TrailingBytes (slack after the last section).
    let declared = u64::from_be_bytes(built.bytes()[34..42].try_into().unwrap());
    let mut slack = built.bytes().to_vec();
    slack.insert(42 + declared as usize, 0x00);
    slack[34..42].copy_from_slice(&(declared + 1).to_be_bytes());
    assert_eq!(
        beacond::decode_packet(&slack),
        Err(MalformedFidelityPacket::SectionLengthMismatch)
    );

    let mut short = built.bytes().to_vec();
    short.remove(42 + declared as usize - 1);
    short[34..42].copy_from_slice(&(declared - 1).to_be_bytes());
    assert_eq!(
        beacond::decode_packet(&short),
        Err(MalformedFidelityPacket::Truncated)
    );

    // InvalidUtf8 in a length-prefixed token.
    let needle = b"outcome-fidelity-draft";
    let at = built
        .bytes()
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("the rubric id is in the header");
    let mut bad_utf8 = built.bytes().to_vec();
    bad_utf8[at] = 0xFF;
    assert_eq!(
        beacond::decode_packet(&bad_utf8),
        Err(MalformedFidelityPacket::InvalidUtf8)
    );

    // A declared count larger than the input can hold refuses before any reserve.
    assert_eq!(
        admits_entry_count(256, 100),
        Err(MalformedFidelityPacket::EntryCount)
    );
}

#[test]
fn the_decoder_pins_every_canonicalizer_the_table_pins() {
    let mut source = inputs(one_of_each_reversed());
    source.subject.record_digest.canonicalizer = "beacon.artifact.v2".to_owned();
    assert_eq!(
        encode_packet(&source, cap()),
        Err(FidelityPacketRefusal::Malformed(
            MalformedFidelityPacket::CanonicalizerNotAdmitted
        ))
    );

    let mut source = inputs(one_of_each_reversed());
    source.subject.content_digest.canonicalizer = "beacon.cooked.v1".to_owned();
    assert!(encode_packet(&source, cap()).is_err());

    let mut source = inputs(one_of_each_reversed());
    source.recipe.renderer_version = "beacon.outcome.md.v2".to_owned();
    assert_eq!(
        encode_packet(&source, cap()),
        Err(FidelityPacketRefusal::Malformed(
            MalformedFidelityPacket::CanonicalizerNotAdmitted
        ))
    );

    assert!(
        encode_packet(
            &inputs(vec![record_entry(EntryKind::Ruling, "beacon.ruling.v3", 5)]),
            cap()
        )
        .is_err()
    );
    assert!(
        encode_packet(
            &inputs(vec![record_entry(EntryKind::Spend, "beacon.spend.v3", 8)]),
            cap()
        )
        .is_err()
    );

    // The Ruling stored-row floor of 17.
    let short_ruling = EvidenceEntry {
        kind: EntryKind::Ruling,
        body: EntryBody::Record {
            digest: digest("beacon.ruling.v2", 5),
            bytes: vec![0_u8; 16],
        },
    };
    assert!(encode_packet(&inputs(vec![short_ruling]), cap()).is_err());

    // S5 requires the four criteria, exactly, in file order.
    let mut source = inputs(one_of_each_reversed());
    source.rubric.criterion_ids = vec!["completion_fidelity".to_owned()];
    assert!(encode_packet(&source, cap()).is_err());
    let mut source = inputs(one_of_each_reversed());
    source.rubric.criterion_ids.reverse();
    assert!(encode_packet(&source, cap()).is_err());
}
