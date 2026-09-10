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

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use uuid::Uuid;

use crate::candidate_evaluation_packet::{
    EntryBody, EntryKind, EvidenceEntry, FidelityPacketRefusal, PacketCap, PacketInputs,
    VerifiedEvaluationPacket, WireDigest,
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

fn checked_len(value: i64, kind: EntryKind) -> Result<u64, FidelityPacketRefusal> {
    u64::try_from(value).map_err(|_| owner_refused(kind))
}

fn record_declaration(
    connection: &Connection,
    workspace_id: Uuid,
    kind: EntryKind,
    digest: &str,
) -> Result<u64, FidelityPacketRefusal> {
    let table = match kind {
        EntryKind::Completion => "mission_completions",
        EntryKind::Proposal => "proposals",
        EntryKind::Ruling => "rulings",
        EntryKind::Evaluation => "evaluations",
        EntryKind::Policy => "mission_impact_classifications",
        EntryKind::Spend => "measured_spend",
        EntryKind::Mission | EntryKind::Artifact | EntryKind::Receipt => {
            return Err(owner_refused(kind));
        }
    };
    let sql = format!(
        "SELECT length(CAST(digest_canonicalizer AS BLOB)),
                length(canonical_bytes)
         FROM {table} WHERE workspace_id = ?1 AND digest_value = ?2"
    );
    let row: Option<(i64, i64)> = connection
        .query_row(&sql, params![workspace_id.to_string(), digest], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()
        .map_err(|_| owner_refused(kind))?;
    let Some((canonicalizer, bytes)) = row else {
        return Err(absent(kind));
    };
    let canonicalizer = checked_len(canonicalizer, kind)?;
    let bytes = checked_len(bytes, kind)?;
    if bytes > crate::candidate_evaluation_packet::MAX_OWNER_RECORD_BYTES as u64 {
        return Err(FidelityPacketRefusal::SourceOverOwnerCeiling {
            kind: kind.as_str(),
        });
    }
    Ok(crate::candidate_evaluation_packet::digest_charge(canonicalizer) + 8 + bytes)
}

fn declared_entry_charge(
    connection: &Connection,
    workspace_id: Uuid,
    citation: &Citation,
) -> Result<u64, FidelityPacketRefusal> {
    match citation {
        Citation::Mission(mission_id) => {
            let row: Option<(i64, i64, i64, i64, i64, i64, i64)> = connection
                .query_row(
                    "SELECT length(CAST(m.desired_outcome AS BLOB)),
                            json_array_length(m.success_conditions_json),
                            coalesce((SELECT sum(8 + length(CAST(value AS BLOB)))
                                      FROM json_each(m.success_conditions_json)), 0),
                            json_array_length(m.constraints_json),
                            coalesce((SELECT sum(8 + length(CAST(value AS BLOB)))
                                      FROM json_each(m.constraints_json)), 0),
                            json_array_length(m.prohibitions_json),
                            coalesce((SELECT sum(8 + length(CAST(value AS BLOB)))
                                      FROM json_each(m.prohibitions_json)), 0)
                     FROM missions m WHERE m.workspace_id = ?1 AND m.id = ?2",
                    params![workspace_id.to_string(), mission_id.to_string()],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                        ))
                    },
                )
                .optional()
                .map_err(|_| owner_refused(EntryKind::Mission))?;
            let Some((desired, _success_count, success, _constraint_count, constraints, _prohibition_count, prohibitions)) = row else {
                return Err(FidelityPacketRefusal::MissionAbsent);
            };
            let desired = checked_len(desired, EntryKind::Mission)?;
            if desired > crate::candidate_evaluation_packet::MAX_MISSION_TEXT_BYTES as u64 {
                return Err(FidelityPacketRefusal::SourceOverOwnerCeiling {
                    kind: EntryKind::Mission.as_str(),
                });
            }
            Ok(16
                + 16
                + 8
                + desired
                + 4
                + checked_len(success, EntryKind::Mission)?
                + 4
                + checked_len(constraints, EntryKind::Mission)?
                + 4
                + checked_len(prohibitions, EntryKind::Mission)?)
        }
        Citation::Record {
            kind: EntryKind::Artifact,
            digest,
        } => {
            let row: Option<(i64, i64, i64)> = connection
                .query_row(
                    "SELECT length(CAST(digest_canonicalizer AS BLOB)),
                            length(canonical_bytes), length(content_inline)
                     FROM artifacts WHERE workspace_id = ?1 AND digest_value = ?2",
                    params![workspace_id.to_string(), digest],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(|_| owner_refused(EntryKind::Artifact))?;
            let Some((canonicalizer, record, content)) = row else {
                return Err(absent(EntryKind::Artifact));
            };
            let canonicalizer = checked_len(canonicalizer, EntryKind::Artifact)?;
            let record = checked_len(record, EntryKind::Artifact)?;
            let content = checked_len(content, EntryKind::Artifact)?;
            if record > crate::candidate_evaluation_packet::MAX_ARTIFACT_RECORD_BYTES as u64
                || content > crate::candidate_evaluation_packet::MAX_ARTIFACT_CONTENT_BYTES as u64
            {
                return Err(FidelityPacketRefusal::SourceOverOwnerCeiling {
                    kind: EntryKind::Artifact.as_str(),
                });
            }
            Ok(crate::candidate_evaluation_packet::digest_charge(canonicalizer)
                + 8
                + record
                + crate::candidate_evaluation_packet::digest_charge(
                    crate::candidate_evaluation_packet::RAW_CANONICALIZER.len() as u64,
                )
                + 8
                + content)
        }
        Citation::Record { kind, digest } => {
            record_declaration(connection, workspace_id, *kind, digest)
        }
        Citation::Receipt(run) => {
            let bytes: Option<i64> = connection
                .query_row(
                    "SELECT length(CAST(receipt_json AS BLOB)) FROM receipts
                     WHERE workspace_id = ?1 AND run_id = ?2",
                    params![workspace_id.to_string(), run.to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| owner_refused(EntryKind::Receipt))?;
            let Some(bytes) = bytes else {
                return Err(absent(EntryKind::Receipt));
            };
            let bytes = checked_len(bytes, EntryKind::Receipt)?;
            if bytes > crate::candidate_evaluation_packet::MAX_RECEIPT_JSON_BYTES as u64 {
                return Err(FidelityPacketRefusal::SourceOverOwnerCeiling {
                    kind: EntryKind::Receipt.as_str(),
                });
            }
            Ok(16
                + 4
                + 8
                + crate::outcome::RECEIPT_CHECKSUM_LABEL.len() as u64
                + 32
                + 8
                + bytes)
        }
    }
}

/// Declare and charge every S4 source before reading any owner bytes, then collect exactly once.
pub fn collect_within(
    transaction: &Transaction<'_>,
    workspace_id: Uuid,
    mut inputs: PacketInputs,
    citations: &[Citation],
    cap: PacketCap,
) -> Result<VerifiedEvaluationPacket, FidelityPacketRefusal> {
    if !inputs.evidence.is_empty() {
        return Err(FidelityPacketRefusal::ScopeMismatch);
    }
    let declarations = citations
        .iter()
        .map(|citation| declared_entry_charge(transaction, workspace_id, citation))
        .collect::<Result<Vec<_>, _>>()?;
    crate::candidate_evaluation_packet::charge_declared_packet(&inputs, &declarations, cap)?;
    inputs.evidence = citations
        .iter()
        .map(|citation| collect_entry_within(transaction, workspace_id, citation))
        .collect::<Result<Vec<_>, _>>()?;
    crate::candidate_evaluation_packet::encode_packet(&inputs, cap)
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

fn raw_sha256(value: &str) -> Result<[u8; 32], FidelityPacketRefusal> {
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
    Ok(raw)
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
        Citation::Receipt(run) => {
            let collector = crate::outcome_collector::Collector::new(connection, workspace_id);
            match collector.receipt_with_bytes(*run, None) {
                Ok((reference, receipt_json)) => Ok(EvidenceEntry {
                    kind: EntryKind::Receipt,
                    body: EntryBody::Receipt {
                        run_uuid: reference.run_uuid,
                        version: reference.receipt_version,
                        checksum_label: crate::outcome::RECEIPT_CHECKSUM_LABEL.to_owned(),
                        checksum: raw_sha256(&reference.outcome_checksum)?,
                        receipt_json,
                    },
                }),
                Err(crate::outcome_collector::CollectorRefusal::Absent { .. }) => {
                    Err(absent(EntryKind::Receipt))
                }
                Err(crate::outcome_collector::CollectorRefusal::Unpositioned { .. })
                    if collector
                        .receipt_has_unregistered_dispatch_owner(*run)
                        .map_err(|_| owner_refused(EntryKind::Receipt))? =>
                {
                    Err(FidelityPacketRefusal::ReceiptOwnerUnregistered { run: *run })
                }
                Err(_) => Err(owner_refused(EntryKind::Receipt)),
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    #[test]
    fn dispatch_receipt_refuses_by_run_instead_of_the_wholesale_receipt_arm() {
        let connection = Connection::open_in_memory().expect("memory store");
        connection
            .execute_batch(
                "CREATE TABLE receipts (
                    workspace_id TEXT NOT NULL,
                    run_id TEXT NOT NULL,
                    version INTEGER NOT NULL,
                    receipt_json TEXT NOT NULL,
                    PRIMARY KEY (workspace_id, run_id)
                 );
                 CREATE TABLE events (
                    sequence INTEGER PRIMARY KEY,
                    workspace_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    payload_json TEXT NOT NULL,
                    occurred_at_ms INTEGER NOT NULL
                 );",
            )
            .expect("minimal owner tables");
        let workspace = Uuid::from_u128(1);
        let run = Uuid::from_u128(2);
        connection
            .execute(
                "INSERT INTO receipts (workspace_id, run_id, version, receipt_json)
                 VALUES (?1, ?2, 3, ?3)",
                params![workspace.to_string(), run.to_string(), r#"{"version":3}"#],
            )
            .expect("dispatch receipt");
        connection
            .execute(
                "INSERT INTO events
                 (sequence, workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (1, ?1, 'proposal.dispatched', ?2, 1)",
                params![
                    workspace.to_string(),
                    format!(r#"{{"runId":"{run}"}}"#),
                ],
            )
            .expect("dispatch event");

        assert_eq!(
            collect_entry_within(&connection, workspace, &Citation::Receipt(run)),
            Err(FidelityPacketRefusal::ReceiptOwnerUnregistered { run })
        );
    }
}
