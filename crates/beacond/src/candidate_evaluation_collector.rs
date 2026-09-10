//! Collecting the Outcome-fidelity packet: every entry through its owning reader, ONCE.
//!
//! **A digest is never sufficient provenance (R68/H1).** Owner ceilings and codec constants say
//! what a record may weigh; they say nothing about whether this Workspace recorded it. Every S4
//! entry is therefore read through the STRONGEST Event-verifying owner reader that exists, and
//! where none existed this slice added one beside the owner's own reader:
//!
//! | Kind | Reader |
//! |---|---|
//! | `0x01` Mission | `mission::verified_mission_admission` |
//! | `0x02` Completion | `mission_completion_store::verified_completion_with_event_bytes` (**new**) |
//! | `0x03` Artifact | `artifact::verified_artifact_within` |
//! | `0x04` Proposal | `proposal::verified_proposal_within` |
//! | `0x05` Ruling | `ruling::verified_ruling_within` |
//! | `0x06` Evaluation | `evaluation::verified_evaluation_with_event_within` (**new**, both subjects) |
//! | `0x07` Policy | `mission_impact::verified_classification_with_event_within` (**new**) |
//! | `0x08` Spend | `spend::verified_spend_with_event_within` (**new**) |
//! | `0x09` Receipt | `outcome_collector::Collector::receipt_with_bytes` (**new** byte seam) |
//!
//! **Collected once (R68/H2).** This runs inside the admission transaction. Its output is retained
//! as the attempt's `input_bytes`; a retry or a read re-uses those bytes and never calls this
//! again. Nothing here admits, requests, or writes: this slice reads and encodes.

use rusqlite::Connection;
use uuid::Uuid;

use crate::candidate_evaluation_packet::{
    EntryBody, EntryKind, EvidenceEntry, FidelityPacketRefusal, WireDigest,
};

/// One citation the manifest makes, as the collector receives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Citation {
    Mission(Uuid),
    Record { kind: EntryKind, digest: String },
    Receipt(Uuid),
}

fn owner_refused(kind: EntryKind) -> FidelityPacketRefusal {
    FidelityPacketRefusal::OwnerRefused {
        kind: kind.as_str(),
    }
}

fn absent(kind: EntryKind) -> FidelityPacketRefusal {
    FidelityPacketRefusal::EssentialSourceAbsent {
        kind: kind.as_str(),
    }
}

fn wire(canonicalizer: &str, value: &str) -> Result<WireDigest, FidelityPacketRefusal> {
    let decoded = data_encoding::HEXLOWER
        .decode(value.as_bytes())
        .map_err(|_| {
            FidelityPacketRefusal::Malformed(
                crate::candidate_evaluation_packet::MalformedFidelityPacket::DigestValue,
            )
        })?;
    if decoded.len() != 32 {
        return Err(FidelityPacketRefusal::Malformed(
            crate::candidate_evaluation_packet::MalformedFidelityPacket::DigestValue,
        ));
    }
    let mut raw = [0_u8; 32];
    raw.copy_from_slice(&decoded);
    Ok(WireDigest {
        canonicalizer: canonicalizer.to_owned(),
        value: raw,
    })
}

/// Read ONE citation through its owning Event-verifying reader.
///
/// Every arm returns the owner's bytes, never a projection: `OwnerRefused` when the owner refuses,
/// `EssentialSourceAbsent` when the owner says the record is not here. The two are different facts
/// and the packet keeps them apart.
pub fn collect_entry_within(
    connection: &Connection,
    workspace_id: Uuid,
    citation: &Citation,
) -> Result<EvidenceEntry, FidelityPacketRefusal> {
    match citation {
        Citation::Mission(mission_id) => {
            let mission =
                crate::mission::verified_mission_admission(connection, workspace_id, *mission_id)
                    .map_err(|_| owner_refused(EntryKind::Mission))?
                    .ok_or(FidelityPacketRefusal::MissionAbsent)?;
            // R59.5: constraints and prohibitions travel, because `limitations_and_authority`
            // cannot be judged without what the Mission forbade.
            Ok(EvidenceEntry {
                kind: EntryKind::Mission,
                body: EntryBody::Mission {
                    mission_id: mission.id,
                    project_id: mission.project_id,
                    desired_outcome: mission.desired_outcome,
                    success_conditions: mission.success_conditions,
                    constraints: mission.constraints,
                    prohibitions: mission.prohibitions,
                },
            })
        }
        Citation::Record { kind, digest } => {
            collect_record_within(connection, workspace_id, *kind, digest)
        }
        Citation::Receipt(_) => {
            // The collector's owner registry covers preflight and execution only
            // (`outcome_collector.rs:331-334`). R59.6: a candidate citing a dispatch Receipt
            // refuses BY NAME until that seam lands as its own slice, and the refusal is the
            // recorded boundary of F3's first subset rather than a gap papered over. The byte
            // seam is `Collector::receipt_with_bytes`; wiring it needs the Collector's own
            // snapshot, which the caller owns.
            Err(FidelityPacketRefusal::OwnerRefused {
                kind: EntryKind::Receipt.as_str(),
            })
        }
    }
}

fn collect_record_within(
    connection: &Connection,
    workspace_id: Uuid,
    kind: EntryKind,
    digest: &str,
) -> Result<EvidenceEntry, FidelityPacketRefusal> {
    let (canonicalizer, bytes) = match kind {
        EntryKind::Completion => {
            // The by-digest reader plus its recording Event: `verified_completion_within` "does
            // NOT open the payload" of that Event (`mission_completion_store.rs:1222-1226`), so
            // this slice composed the two beside it rather than settling for the weaker read.
            let completion = crate::mission_completion_store::verified_completion_with_event_bytes(
                connection,
                workspace_id,
                digest,
            )
            .map_err(|_| owner_refused(kind))?
            .ok_or_else(|| absent(kind))?;
            (
                crate::mission_completion::COMPLETION_CANONICALIZER.to_owned(),
                completion,
            )
        }
        EntryKind::Proposal => {
            let proposal =
                crate::proposal::verified_proposal_within(connection, workspace_id, digest)
                    .map_err(|_| owner_refused(kind))?
                    .ok_or_else(|| absent(kind))?;
            (
                proposal.digest().canonicalizer.clone(),
                proposal.canonical_bytes().to_vec(),
            )
        }
        EntryKind::Ruling => {
            let ruling = crate::ruling::verified_ruling_within(connection, workspace_id, digest)
                .map_err(|_| owner_refused(kind))?
                .ok_or_else(|| absent(kind))?;
            (
                ruling.digest().canonicalizer.clone(),
                ruling.canonical_bytes().to_vec(),
            )
        }
        EntryKind::Evaluation => {
            let evaluation = crate::evaluation::verified_evaluation_with_event_within(
                connection,
                workspace_id,
                digest,
            )
            .map_err(|_| owner_refused(kind))?
            .ok_or_else(|| absent(kind))?;
            (
                evaluation.digest().canonicalizer.clone(),
                evaluation.canonical_bytes().to_vec(),
            )
        }
        EntryKind::Policy => {
            let classification = crate::mission_impact::verified_classification_with_event_within(
                connection,
                workspace_id,
                digest,
            )
            .map_err(|_| owner_refused(kind))?
            .ok_or_else(|| absent(kind))?;
            (
                classification.digest().canonicalizer.clone(),
                classification.canonical_bytes().to_vec(),
            )
        }
        EntryKind::Spend => {
            let spend =
                crate::spend::verified_spend_with_event_within(connection, workspace_id, digest)
                    .map_err(|_| owner_refused(kind))?
                    .ok_or_else(|| absent(kind))?;
            (
                spend.digest().canonicalizer.clone(),
                spend.canonical_bytes().to_vec(),
            )
        }
        EntryKind::Artifact => {
            let artifact =
                crate::artifact::verified_artifact_within(connection, workspace_id, digest)
                    .map_err(|_| owner_refused(kind))?
                    .ok_or_else(|| absent(kind))?;
            let record_bytes = artifact
                .payload()
                .canonical_bytes()
                .map_err(|_| owner_refused(kind))?;
            return Ok(EvidenceEntry {
                kind,
                body: EntryBody::Artifact {
                    record_digest: wire(
                        &artifact.digest().canonicalizer,
                        &artifact.digest().value,
                    )?,
                    record_bytes,
                    content_digest: wire(
                        &artifact.payload().content_digest.canonicalizer,
                        &artifact.payload().content_digest.value,
                    )?,
                    content_bytes: artifact.content().to_vec(),
                },
            });
        }
        EntryKind::Mission | EntryKind::Receipt => return Err(owner_refused(kind)),
    };
    Ok(EvidenceEntry {
        kind,
        body: EntryBody::Record {
            digest: wire(&canonicalizer, digest)?,
            bytes,
        },
    })
}
