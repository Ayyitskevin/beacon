//! The strict result profile `beacon.outcome-fidelity.result.v1`, and the five closed outcomes.
//!
//! Table §B/§B.1 at round 3 (SHA-256 `cf5cbf92…4d59`), R59.8 and R68/H5.
//!
//! Two rules run through everything here. **No provider-supplied field is trusted for identity**:
//! `packet_sha256` and `rubric_sha256` are compared against values the caller already holds, never
//! adopted. And **parsing creates no Evaluation**: a `Valid` outcome is parser output, not
//! authority to record anything.

use std::fmt;

use data_encoding::HEXLOWER;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::candidate_evaluation_packet::VerifiedEvaluationPacket;
use crate::model_invocation::{IncompleteCause, TransportObservation};
use crate::proposal::{is_human_text, sha256};

/// The profile every reply must name.
pub const PROFILE: &str = "beacon.outcome-fidelity.result.v1";

/// The response ceiling, `C`. A body of `C` bytes is admissible; the reader stops after byte
/// `C + 1`, and that one extra byte IS the bounded excess witness.
pub const RESPONSE_CEILING: usize = 65_536;
/// The exact observed length an over-limit witness carries.
pub const OVER_LIMIT_WITNESS: usize = RESPONSE_CEILING + 1;

const MAX_FINDINGS: usize = 64;
const MAX_SUMMARY_SCALARS: usize = 500;
const MAX_TEXT_SCALARS: usize = 2_000;
const MAX_EVIDENCE_REFERENCES: usize = 256;

const REQUIRED_KEYS: [&str; 8] = [
    "profile",
    "packet_sha256",
    "rubric_sha256",
    "criteria",
    "overall_verdict",
    "findings",
    "conflicts",
    "rationale",
];

/// The four rubric criterion IDs, in the resource's own file order
/// (`resources/evaluation/outcome-fidelity-v1.txt:7,10,13,16`).
pub const CRITERION_IDS: [&str; 4] = [
    "completion_fidelity",
    "evidence_fidelity",
    "honesty_states",
    "limitations_and_authority",
];

// ---------------------------------------------------------------------------------------------
// The closed vocabularies, copied whole rather than borrowed (`outcome.rs:137-140`'s rule)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Meets,
    RevisionRequested,
    DoesNotMeet,
    /// A first-class judgment, never a soft rejection and never a parser failure
    /// (`evaluation.rs:124-135`).
    Uncertain,
}

impl Verdict {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "meets" => Self::Meets,
            "revision_requested" => Self::RevisionRequested,
            "does_not_meet" => Self::DoesNotMeet,
            "uncertain" => Self::Uncertain,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingSeverity {
    Blocking,
    Concern,
    Note,
}

impl FindingSeverity {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "blocking" => Self::Blocking,
            "concern" => Self::Concern,
            "note" => Self::Note,
            _ => return None,
        })
    }
}

/// Why a complete body is not a result. Closed. `Rationale` and `Profile` are R59.8's additions:
/// the frozen list left two required keys with no code of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedCode {
    Encoding,
    Shape,
    UnknownOrDuplicateField,
    PacketDigest,
    RubricDigest,
    Criteria,
    Verdict,
    Finding,
    Conflict,
    EvidenceReference,
    Rationale,
    Profile,
}

impl MalformedCode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Encoding => "Encoding",
            Self::Shape => "Shape",
            Self::UnknownOrDuplicateField => "UnknownOrDuplicateField",
            Self::PacketDigest => "PacketDigest",
            Self::RubricDigest => "RubricDigest",
            Self::Criteria => "Criteria",
            Self::Verdict => "Verdict",
            Self::Finding => "Finding",
            Self::Conflict => "Conflict",
            Self::EvidenceReference => "EvidenceReference",
            Self::Rationale => "Rationale",
            Self::Profile => "Profile",
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The parsed judgment
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CriterionVerdict {
    pub id: String,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultFinding {
    pub criterion: String,
    pub severity: FindingSeverity,
    pub summary: String,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflicts {
    None,
    Declared(String),
}

/// One Resident's judgment, exactly as the bytes carried it. `overall_verdict` is the Resident's
/// own and is NEVER derived from `criteria`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Judgment {
    pub criteria: Vec<CriterionVerdict>,
    pub overall_verdict: Verdict,
    pub findings: Vec<ResultFinding>,
    pub conflicts: Conflicts,
    pub rationale: String,
}

// ---------------------------------------------------------------------------------------------
// The terminal observation, and the sixth state made unrepresentable
// ---------------------------------------------------------------------------------------------

/// Exactly the two observations that end a transport attempt (`model_invocation.rs:293-296`).
///
/// The classifier takes THIS, not a `TransportObservation`, so an attempt with no terminal
/// observation — `unconfirmed` — has nothing to hand it and cannot be classified as anything. The
/// sixth state is a type error rather than a rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalObservation {
    Complete {
        bytes: Vec<u8>,
        elapsed_ms: u64,
    },
    Incomplete {
        prefix: Vec<u8>,
        cause: IncompleteCause,
    },
}

/// A transport observation that does not end the attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotTerminal;

/// A stored observation that contradicts its own stop (R68/H5).
///
/// This is a PURPOSE-LEVEL refinement, not C2 grammar: C2 admits a `BodyIncomplete` of any length
/// with any cause, and this module narrows nothing there. The adapter never writes one — it reads
/// with `limit(C + 1)`, so `ResponseOverLimit` is emitted only with exactly `C + 1` bytes. The
/// purpose's verified reader refuses a stored one as corruption, and it is NEVER `Missing`:
/// `EVALUATOR-CONTRACT.md:48` requires corrupt data to refuse rather than normalise to absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IncoherentObservation;

impl fmt::Display for IncoherentObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("IncoherentObservation")
    }
}

impl TerminalObservation {
    /// `Headers` and `BodyPrefix` cannot satisfy this, so they can never reach a classifier.
    pub fn from_transport(observation: &TransportObservation) -> Result<Self, NotTerminal> {
        match observation {
            TransportObservation::Headers { .. } | TransportObservation::BodyPrefix { .. } => {
                Err(NotTerminal)
            }
            TransportObservation::BodyComplete { bytes, elapsed_ms } => Ok(Self::Complete {
                bytes: bytes.clone(),
                elapsed_ms: *elapsed_ms,
            }),
            TransportObservation::BodyIncomplete { prefix, cause } => Ok(Self::Incomplete {
                prefix: prefix.clone(),
                cause: *cause,
            }),
        }
    }

    fn observed_len(&self) -> usize {
        match self {
            Self::Complete { bytes, .. } => bytes.len(),
            Self::Incomplete { prefix, .. } => prefix.len(),
        }
    }

    fn observed_bytes(&self) -> &[u8] {
        match self {
            Self::Complete { bytes, .. } => bytes,
            Self::Incomplete { prefix, .. } => prefix,
        }
    }

    /// Refuse a record that contradicts its own stop, BEFORE classification.
    pub fn require_coherent(&self) -> Result<(), IncoherentObservation> {
        if let Self::Incomplete { prefix, cause } = self
            && *cause == IncompleteCause::ResponseOverLimit
            && prefix.len() != OVER_LIMIT_WITNESS
        {
            return Err(IncoherentObservation);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// The five closed outcomes
// ---------------------------------------------------------------------------------------------

/// Total over coherent terminal observations. Classification reads LENGTH and ARM only; `cause` is
/// retained and reported, never determinative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultOutcome {
    /// Zero bytes observed AND no completion.
    Missing,
    /// A prefix, hashed prefix-only.
    Incomplete {
        prefix_sha256: [u8; 32],
        observed_bytes: usize,
    },
    /// The bounded excess witness. The digest names exactly `C + 1` bytes and NOTHING more.
    OverLimit {
        prefix_sha256: [u8; 32],
        observed_bytes: usize,
    },
    Malformed {
        code: MalformedCode,
        complete_sha256: [u8; 32],
    },
    Valid {
        judgment: Box<Judgment>,
        complete_sha256: [u8; 32],
    },
}

impl ResultOutcome {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Missing => "Missing",
            Self::Incomplete { .. } => "Incomplete",
            Self::OverLimit { .. } => "OverLimit",
            Self::Malformed { .. } => "Malformed",
            Self::Valid { .. } => "Valid",
        }
    }
}

/// Classify one coherent terminal observation. Precedence is the table's §B.1:
/// 1. any observed length `> C` is `OverLimit`, in EITHER arm;
/// 2. `Incomplete` with zero bytes is `Missing`;
/// 3. `Incomplete` with `1..=C` is `Incomplete`;
/// 4. `Complete` within `C` that fails the grammar is `Malformed`;
/// 5. `Complete` within `C` that passes is `Valid`.
pub fn classify(
    observation: &TerminalObservation,
    packet: &VerifiedEvaluationPacket,
    held_rubric_digest: &[u8; 32],
) -> Result<ResultOutcome, IncoherentObservation> {
    observation.require_coherent()?;

    // Rule 1 first, and in both arms. A `Complete` longer than the ceiling is unreachable from our
    // own adapter (it reads `limit(C + 1)`), but the C2 grammar admits one — `encoder.bytes(bytes,
    // usize::MAX)` at `model_invocation.rs:1029` sets no bound — so a stored record can carry one
    // and totality has to hold over stored records, not only over what today's adapter produces.
    if observation.observed_len() > RESPONSE_CEILING {
        let witness = &observation.observed_bytes()[..OVER_LIMIT_WITNESS];
        return Ok(ResultOutcome::OverLimit {
            prefix_sha256: sha256(witness),
            observed_bytes: OVER_LIMIT_WITNESS,
        });
    }

    Ok(match observation {
        TerminalObservation::Incomplete { prefix, .. } if prefix.is_empty() => {
            ResultOutcome::Missing
        }
        TerminalObservation::Incomplete { prefix, .. } => ResultOutcome::Incomplete {
            prefix_sha256: sha256(prefix),
            observed_bytes: prefix.len(),
        },
        TerminalObservation::Complete { bytes, .. } => {
            let complete_sha256 = sha256(bytes);
            match parse_result(bytes, packet, held_rubric_digest) {
                Ok(judgment) => ResultOutcome::Valid {
                    judgment: Box::new(judgment),
                    complete_sha256,
                },
                Err(code) => ResultOutcome::Malformed {
                    code,
                    complete_sha256,
                },
            }
        }
    })
}

// ---------------------------------------------------------------------------------------------
// The strict parser
// ---------------------------------------------------------------------------------------------

/// A JSON tree that preserves every object pair at every depth, duplicates included.
///
/// `serde_json::Value` inserts nested keys into a map and therefore loses a duplicate before the
/// profile can reject it. This tree keeps maps as ordered pair vectors recursively.
enum JsonNode {
    Scalar,
    String(String),
    Array(Vec<JsonNode>),
    Object(Vec<(String, JsonNode)>),
}

impl JsonNode {
    fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }

    fn as_object(&self) -> Option<&[(String, Self)]> {
        match self {
            Self::Object(pairs) => Some(pairs),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for JsonNode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NodeVisitor;
        impl<'de> Visitor<'de> for NodeVisitor {
            type Value = JsonNode;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON value")
            }

            fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
                Ok(JsonNode::Scalar)
            }

            fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
                Ok(JsonNode::Scalar)
            }

            fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
                Ok(JsonNode::Scalar)
            }

            fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
                Ok(JsonNode::Scalar)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(JsonNode::Scalar)
            }

            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(JsonNode::Scalar)
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                Ok(JsonNode::String(value.to_owned()))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
                Ok(JsonNode::String(value))
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<JsonNode>()? {
                    values.push(value);
                }
                Ok(JsonNode::Array(values))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<JsonNode, A::Error> {
                let mut pairs = Vec::new();
                while let Some(pair) = map.next_entry::<String, JsonNode>()? {
                    pairs.push(pair);
                }
                Ok(JsonNode::Object(pairs))
            }
        }
        deserializer.deserialize_any(NodeVisitor)
    }
}

fn digest_matches(supplied: &str, held: &[u8; 32]) -> bool {
    supplied.len() == 64 && supplied == HEXLOWER.encode(held)
}

fn text(value: &JsonNode) -> Result<&str, MalformedCode> {
    value.as_str().ok_or(MalformedCode::Shape)
}

fn exact_object<'a>(
    value: &'a JsonNode,
    required: &[&str],
    wrong_shape: MalformedCode,
) -> Result<&'a [(String, JsonNode)], MalformedCode> {
    let pairs = value.as_object().ok_or(wrong_shape)?;
    let mut seen: Vec<&str> = pairs.iter().map(|(key, _)| key.as_str()).collect();
    seen.sort_unstable();
    let mut expected = required.to_vec();
    expected.sort_unstable();
    if seen != expected {
        return Err(MalformedCode::UnknownOrDuplicateField);
    }
    Ok(pairs)
}

fn field<'a>(pairs: &'a [(String, JsonNode)], name: &str) -> &'a JsonNode {
    &pairs
        .iter()
        .find(|(key, _)| key == name)
        .expect("key set already proved equal")
        .1
}

/// Parse one already-bounded complete body under the strict profile.
///
/// Private: `classify` owns the public RESPONSE_CEILING boundary, so no caller can parse an
/// unbounded provider response by bypassing the classifier.
fn parse_result(
    bytes: &[u8],
    packet: &VerifiedEvaluationPacket,
    held_rubric_digest: &[u8; 32],
) -> Result<Judgment, MalformedCode> {
    // 1. Encoding, before anything is parsed.
    let text_body = std::str::from_utf8(bytes).map_err(|_| MalformedCode::Encoding)?;

    // 2. Syntax and root shape. Malformed JSON syntax — a lone `{`, an unterminated string, a
    //    trailing comma — is `Shape`, the same code as a well-formed document of the wrong shape,
    //    because both mean "these complete bytes are not a result".
    let root: JsonNode = serde_json::from_str(text_body).map_err(|_| MalformedCode::Shape)?;

    // 3. Exactly the eight required keys, each exactly once.
    let pairs = exact_object(&root, &REQUIRED_KEYS, MalformedCode::Shape)?;
    let get = |name: &str| field(pairs, name);

    // 4. Identity: compared against HELD values, never adopted from the reply.
    if text(get("profile"))? != PROFILE {
        return Err(MalformedCode::Profile);
    }
    let held_packet_digest = HEXLOWER
        .decode(packet.digest().value.as_bytes())
        .map_err(|_| MalformedCode::PacketDigest)?;
    let mut held_packet = [0_u8; 32];
    if held_packet_digest.len() != 32 {
        return Err(MalformedCode::PacketDigest);
    }
    held_packet.copy_from_slice(&held_packet_digest);
    if !digest_matches(text(get("packet_sha256"))?, &held_packet) {
        return Err(MalformedCode::PacketDigest);
    }
    if !digest_matches(text(get("rubric_sha256"))?, held_rubric_digest) {
        return Err(MalformedCode::RubricDigest);
    }

    // 5. Criteria: the four IDs exactly once, in rubric file order.
    let criteria_value = get("criteria").as_array().ok_or(MalformedCode::Criteria)?;
    if criteria_value.len() != CRITERION_IDS.len() {
        return Err(MalformedCode::Criteria);
    }
    let mut criteria = Vec::with_capacity(CRITERION_IDS.len());
    for (index, entry) in criteria_value.iter().enumerate() {
        let object = exact_object(entry, &["id", "verdict"], MalformedCode::Criteria)?;
        let id = field(object, "id")
            .as_str()
            .ok_or(MalformedCode::Criteria)?;
        if id != CRITERION_IDS[index] {
            return Err(MalformedCode::Criteria);
        }
        let verdict = field(object, "verdict")
            .as_str()
            .ok_or(MalformedCode::Verdict)?;
        criteria.push(CriterionVerdict {
            id: id.to_owned(),
            verdict: Verdict::parse(verdict).ok_or(MalformedCode::Verdict)?,
        });
    }

    // 6. The overall verdict is the Resident's own. It is read, never computed from `criteria`.
    let overall_verdict =
        Verdict::parse(text(get("overall_verdict"))?).ok_or(MalformedCode::Verdict)?;

    // 7. Findings, order preserved and never re-sorted.
    let findings_value = get("findings").as_array().ok_or(MalformedCode::Finding)?;
    if findings_value.len() > MAX_FINDINGS {
        return Err(MalformedCode::Finding);
    }
    let mut findings = Vec::with_capacity(findings_value.len());
    let mut references = 0_usize;
    for entry in findings_value {
        let object = exact_object(
            entry,
            &["criterion", "severity", "summary", "evidence"],
            MalformedCode::Finding,
        )?;
        let criterion = field(object, "criterion")
            .as_str()
            .ok_or(MalformedCode::Finding)?;
        if !CRITERION_IDS.contains(&criterion) {
            return Err(MalformedCode::Finding);
        }
        let severity = field(object, "severity")
            .as_str()
            .ok_or(MalformedCode::Finding)?;
        let severity = FindingSeverity::parse(severity).ok_or(MalformedCode::Finding)?;
        let summary = field(object, "summary")
            .as_str()
            .ok_or(MalformedCode::Finding)?;
        // Enforced at PARSE, not only at record. Without this a leading-space or empty summary is
        // `Valid` here and refused by `evaluation.rs:1184-1186` at `record` — a valid, paid-for,
        // unrecordable judgment.
        if !is_human_text(summary, MAX_SUMMARY_SCALARS) {
            return Err(MalformedCode::Finding);
        }
        let evidence_value = field(object, "evidence")
            .as_array()
            .ok_or(MalformedCode::Finding)?;
        let mut evidence = Vec::with_capacity(evidence_value.len());
        for reference in evidence_value {
            let label = reference.as_str().ok_or(MalformedCode::EvidenceReference)?;
            if !packet.resolves(label) {
                return Err(MalformedCode::EvidenceReference);
            }
            references += 1;
            if references > MAX_EVIDENCE_REFERENCES {
                return Err(MalformedCode::EvidenceReference);
            }
            evidence.push(label.to_owned());
        }
        findings.push(ResultFinding {
            criterion: criterion.to_owned(),
            severity,
            summary: summary.to_owned(),
            evidence,
        });
    }

    // 8. Conflicts: absence is a value, so it encodes as one.
    let conflicts_value = get("conflicts");
    let conflicts_pairs = conflicts_value
        .as_object()
        .ok_or(MalformedCode::Conflict)?;
    if conflicts_pairs.iter().filter(|(key, _)| key == "state").count() != 1 {
        return Err(MalformedCode::UnknownOrDuplicateField);
    }
    let state = field(conflicts_pairs, "state")
        .as_str()
        .ok_or(MalformedCode::Conflict)?;
    let conflicts = match state {
        "none" => {
            exact_object(conflicts_value, &["state"], MalformedCode::Conflict)?;
            Conflicts::None
        }
        "declared" => {
            let object =
                exact_object(conflicts_value, &["state", "text"], MalformedCode::Conflict)?;
            let declared = field(object, "text")
                .as_str()
                .ok_or(MalformedCode::Conflict)?;
            if !is_human_text(declared, MAX_TEXT_SCALARS) {
                return Err(MalformedCode::Conflict);
            }
            Conflicts::Declared(declared.to_owned())
        }
        _ => return Err(MalformedCode::Conflict),
    };

    // 9. Rationale — R59.8's own code.
    let rationale = get("rationale").as_str().ok_or(MalformedCode::Rationale)?;
    if !is_human_text(rationale, MAX_TEXT_SCALARS) {
        return Err(MalformedCode::Rationale);
    }

    Ok(Judgment {
        criteria,
        overall_verdict,
        findings,
        conflicts,
        rationale: rationale.to_owned(),
    })
}
