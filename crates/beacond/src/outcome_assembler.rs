//! Outcome Document slice 3b, item 0b: the assembler `CollectedSources -> OutcomeManifestV1`.
//!
//! The collector reads every source through its owning verified reader and hands back identities,
//! digests and states. `prepare` takes a manifest and renders it. Nothing joined the two: `prepare`
//! took a hand-built manifest, and `VerifiedSources` had no producer. This module is the seam.
//!
//! Its whole discipline is gpt2 §2 "Read discipline", and it is short: **the assembler fills fields
//! from what the collector proved and REFUSES TO GUESS the rest.** It is not a second verifier --
//! the collector already refused a classification for another Mission, a spend for another Mission,
//! a source after the boundary. What the assembler adds is the refusal to invent: an absent
//! collection stays `Absent` (never an empty `Verified` vector); a collected value the manifest
//! grammar cannot carry without losing a distinction is a named refusal (never the nearest
//! representable thing); a set that is inconsistent with itself is refused by name.
//!
//! Pure over the collected value. The owner reads for the document's TEXT -- Mission title,
//! condition statements, Artifact titles, narrative summaries -- are a separate, impure producer
//! (`verified_sources_for`), because text has to be re-read through an owner and this function must
//! not need a connection.

use rusqlite::Connection;
use uuid::Uuid;

use crate::mission_completion::COMPLETION_CANONICALIZER;
use crate::mission_completion_store::{
    CompletionReadRefusal, declared_conditions_digest, verified_completion_within,
};
use crate::mission_impact::CANONICALIZER as MISSION_IMPACT_CANONICALIZER;
use crate::outcome::{
    AppendixRequest, CANONICALIZER, NarrativeRef, OutcomeManifestV1, OutcomeState, RENDERER_V1,
    ReferenceIdentity, SourceDigest, SourceKind, SourceRef, SpendMeasurementKind, SpendRef,
    SuccessCondition, VerifiedArtifact, VerifiedSources, VerifiedText,
};
use crate::outcome_collector::{CollectedDigest, CollectedNarrative, CollectedSources};
use crate::spend::CANONICALIZER as SPEND_CANONICALIZER;

const DIGEST_ALGORITHM: &str = "sha256";
/// The completion citation's schema version. `MissionCompletionV1` carries no version field; its
/// grammar version is the `v1` in its canonicalizer, the constant `prepare` checks it against.
const COMPLETION_SCHEMA_VERSION: u32 = 1;

/// Why a collected set did not become a manifest.
///
/// Every arm is a refusal to guess, and each names what it would have had to invent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssemblyRefusal {
    /// The collected set has no completion. There is no boundary, no accountable Resident and no
    /// subject; a manifest cannot be assembled from nothing and is not assembled from defaults.
    NoCompletion,
    /// The collected set has no Policy classification. The manifest's appendix policy is keyed on
    /// its high-impact bit; assembling without one would mean picking a bit.
    NoClassification,
    /// A collected source names a Workspace other than the one being assembled for. The collector
    /// refuses these already; the assembler checks that the values it was handed agree.
    CrossWorkspace { kind: &'static str, id: String },
    /// The completion recorded supporting objects for a condition and the collector carried only
    /// their count. The manifest cites objects, not counts, and a citation cannot be invented from
    /// a number -- so this refuses rather than emit `Absent` for support that exists.
    SupportNotCollected { ordinal: u32, count: usize },
    /// A spend was collected as measured, yet lacks the identity or coverage a measured record
    /// always carries. The collected set disagrees with itself.
    SpendInconsistent { detail: &'static str },
    /// The collected spend's measurement token is one this assembler does not know.
    SpendMeasurementUnknown { measurement: String },
}

impl std::fmt::Display for AssemblyRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCompletion => formatter.write_str("no completion was collected"),
            Self::NoClassification => formatter.write_str("no Policy classification was collected"),
            Self::CrossWorkspace { kind, id } => {
                write!(
                    formatter,
                    "the collected {kind} {id} names another Workspace"
                )
            }
            Self::SupportNotCollected { ordinal, count } => write!(
                formatter,
                "condition {ordinal} has {count} supporting object(s) the collector did not carry; \
                 a citation cannot be assembled from a count"
            ),
            Self::SpendInconsistent { detail } => {
                write!(
                    formatter,
                    "the collected spend disagrees with itself: {detail}"
                )
            }
            Self::SpendMeasurementUnknown { measurement } => {
                write!(formatter, "unknown spend measurement token: {measurement}")
            }
        }
    }
}

impl std::error::Error for AssemblyRefusal {}

/// Build the immutable manifest from what the collector proved.
///
/// `workspace_id` is the collector's own; every collected source must agree with it.
/// `appendix_request` is the caller's; the resolved appendix mode is derived, never stored.
/// Crate-private (sol1's P0-1): a `CollectedSources` is the collector's to build and this function
/// does not re-prove what the collector proved, so the only caller is `prepare_for_mission`, which
/// collects and assembles in one transaction. The unit tests below hand-build sets because the crate
/// may; nothing outside it can.
pub(crate) fn assemble(
    collected: &CollectedSources,
    workspace_id: Uuid,
    appendix_request: AppendixRequest,
) -> Result<OutcomeManifestV1, AssemblyRefusal> {
    let Some(completion) = &collected.completion else {
        return Err(AssemblyRefusal::NoCompletion);
    };
    let Some(classification) = &collected.classification else {
        return Err(AssemblyRefusal::NoClassification);
    };
    let boundary_event_seq = completion.boundary_event_seq;

    // Success conditions: the owner's ordinal IS the condition's stable identity, so the token is
    // its decimal spelling. The outcome maps through the owner's four arms without collapse
    // (R18.3). Citations: the collector carries only a COUNT of supporting objects. Zero is
    // `Absent` -- the completion recorded none, a fact about the Mission. More than zero is a
    // refusal, because the objects exist and were not collected, and a state that says "none"
    // would be a lie.
    let mut success_conditions = Vec::with_capacity(completion.condition_results.len());
    for condition in &completion.condition_results {
        if condition.support_count > 0 {
            return Err(AssemblyRefusal::SupportNotCollected {
                ordinal: condition.ordinal,
                count: condition.support_count,
            });
        }
        success_conditions.push(SuccessCondition {
            ordinal: condition.ordinal,
            condition_id: condition.ordinal.to_string(),
            outcome: condition.outcome.into(),
            citations: OutcomeState::Absent(
                "the completion recorded no supporting object for this condition.".to_owned(),
            ),
        });
    }

    // Delivered Artifacts: the collector already returns the STATE (`Absent` when the completion
    // delivered none); a verified list becomes citations by record digest, which is an Artifact's
    // whole identity (R16), under the Artifact module's own canonicalizer -- the one every
    // delivered record was proved to decode under at collection.
    let delivered_artifacts = match &completion.delivered_artifacts {
        OutcomeState::Verified(digests) => OutcomeState::Verified(
            digests
                .iter()
                .map(|digest_value| SourceRef {
                    workspace_id,
                    kind: SourceKind::Artifact,
                    identity: None,
                    digest: SourceDigest {
                        algorithm: DIGEST_ALGORITHM.to_owned(),
                        canonicalizer: crate::artifact::CANONICALIZER.to_owned(),
                        value: digest_value.clone(),
                    },
                    pinned_revision: OutcomeState::Absent(
                        "an Artifact record pins no content revision.".to_owned(),
                    ),
                })
                .collect(),
        ),
        OutcomeState::Absent(reason) => OutcomeState::Absent(reason.clone()),
        OutcomeState::Broken(reason) => OutcomeState::Broken(reason.clone()),
        OutcomeState::NotEvaluated(reason) => OutcomeState::NotEvaluated(reason.clone()),
    };

    let proposal_refs = cited_refs(&collected.proposals, workspace_id, "proposal", "Proposal")?;
    let ruling_refs = cited_refs(&collected.rulings, workspace_id, "ruling", "Ruling")?;
    let evaluation_refs = cited_refs(
        &collected.evaluations,
        workspace_id,
        "evaluation",
        "Evaluation",
    )?;

    for receipt in &collected.receipts {
        if receipt.workspace_id != workspace_id {
            return Err(AssemblyRefusal::CrossWorkspace {
                kind: "receipt",
                id: receipt.run_uuid.to_string(),
            });
        }
    }
    let receipt_refs = if collected.receipts.is_empty() {
        OutcomeState::Absent("no Receipt is cited at this boundary.".to_owned())
    } else {
        OutcomeState::Verified(collected.receipts.clone())
    };

    let spend_ref = spend_state(collected, workspace_id)?;

    Ok(OutcomeManifestV1 {
        workspace_id,
        mission_id: completion.mission_id,
        completion: SourceRef {
            workspace_id,
            kind: SourceKind::Completion,
            identity: Some(ReferenceIdentity {
                object_id: completion.completion_id,
                schema_version: COMPLETION_SCHEMA_VERSION,
            }),
            digest: SourceDigest {
                algorithm: DIGEST_ALGORITHM.to_owned(),
                canonicalizer: COMPLETION_CANONICALIZER.to_owned(),
                value: completion.digest_value.clone(),
            },
            pinned_revision: OutcomeState::Absent(
                "a completion record pins no content revision.".to_owned(),
            ),
        },
        policy_classification: SourceRef {
            workspace_id,
            kind: SourceKind::Policy,
            identity: Some(ReferenceIdentity {
                object_id: classification.classification_id,
                // The version the collector's owning read proved, copied -- never a constant
                // (R23.5). `prepare` re-reads the classification by this exact identity.
                schema_version: classification.version,
            }),
            digest: SourceDigest {
                algorithm: DIGEST_ALGORITHM.to_owned(),
                canonicalizer: MISSION_IMPACT_CANONICALIZER.to_owned(),
                value: classification.digest_value.clone(),
            },
            pinned_revision: OutcomeState::Absent(
                "a Policy classification pins no content revision.".to_owned(),
            ),
        },
        boundary_event_seq,
        completion_recorded_at_ms: completion.completed_at_ms,
        accountable_resident_id: completion.accountable_resident_id,
        // The collector produced this bit by `recheck_copied_bit`, so the copy and the check were
        // one call; `prepare` re-checks it against the record again at prepare time (R13.7).
        policy_high_impact: classification.policy_high_impact,
        appendix_request,
        canonicalizer_version: CANONICALIZER.to_owned(),
        renderer_version: RENDERER_V1.to_owned(),
        success_conditions,
        delivered_artifacts,
        proposal_refs,
        ruling_refs,
        evaluation_refs,
        receipt_refs,
        spend_ref,
        unfinished_work: narrative_state(&completion.unfinished_work, "unfinished work"),
        uncertainty: narrative_state(&completion.uncertainty, "uncertainty"),
    })
}

/// Narrative items by their completion-owned ordinal, which is their stable identity; the text is
/// the producer's business, read through the completion's own record.
fn narrative_state(items: &[CollectedNarrative], what: &str) -> OutcomeState<Vec<NarrativeRef>> {
    if items.is_empty() {
        return OutcomeState::Absent(format!("the completion recorded no {what}."));
    }
    OutcomeState::Verified(
        items
            .iter()
            .map(|item| NarrativeRef {
                ordinal: item.ordinal,
                item_id: item.ordinal.to_string(),
            })
            .collect(),
    )
}

/// Proposal, Ruling and Evaluation citations: `kind + workspace + digest`, identity `None` -- none
/// of the three carries a UUID inside its canonical bytes today (R18.5), and the digest envelope is
/// the owner's, carried by the collector, never spelled here.
fn cited_refs(
    collected: &[CollectedDigest],
    workspace_id: Uuid,
    kind_label: &'static str,
    kind_name: &str,
) -> Result<OutcomeState<Vec<SourceRef>>, AssemblyRefusal> {
    if collected.is_empty() {
        return Ok(OutcomeState::Absent(format!(
            "no {kind_name} is cited at this boundary."
        )));
    }
    let mut refs = Vec::with_capacity(collected.len());
    for entry in collected {
        if entry.workspace_id != workspace_id {
            return Err(AssemblyRefusal::CrossWorkspace {
                kind: kind_label,
                id: entry.digest_value.clone(),
            });
        }
        refs.push(SourceRef {
            workspace_id,
            kind: entry.kind,
            identity: None,
            digest: SourceDigest {
                algorithm: entry.digest_algorithm.clone(),
                canonicalizer: entry.digest_canonicalizer.clone(),
                value: entry.digest_value.clone(),
            },
            pinned_revision: OutcomeState::Absent(
                "this source type pins no content revision.".to_owned(),
            ),
        });
    }
    Ok(OutcomeState::Verified(refs))
}

/// The spend, as a state. Absent is never `$0`; partial is never complete.
fn spend_state(
    collected: &CollectedSources,
    workspace_id: Uuid,
) -> Result<OutcomeState<SpendRef>, AssemblyRefusal> {
    let Some(spend) = &collected.spend else {
        // The collector returns `Some` whenever a completion was requested; `None` means the
        // completion was collected without its spend, which `collect` does not do. A manifest must
        // still say something, and "nothing was collected" is not "nothing was measured".
        return Ok(OutcomeState::NotEvaluated(
            "no spend was collected for this completion.".to_owned(),
        ));
    };
    match spend.measurement.as_str() {
        "not_measured" | "unmeasured" => {
            return Ok(match &spend.amount {
                OutcomeState::NotEvaluated(reason) => OutcomeState::NotEvaluated(reason.clone()),
                _ => OutcomeState::NotEvaluated(
                    "the completion recorded no spend measurement.".to_owned(),
                ),
            });
        }
        "broken" => {
            return Ok(match &spend.amount {
                OutcomeState::Broken(reason) => OutcomeState::Broken(reason.clone()),
                _ => OutcomeState::Broken("the spend measurement is broken.".to_owned()),
            });
        }
        // Complete and partial are two arms of one closed vocabulary (R23.1), never one arm with
        // a footnote: the token rides in the manifest and the document prints it beside the amount.
        "measured" | "partially_measured" => {}
        other => {
            return Err(AssemblyRefusal::SpendMeasurementUnknown {
                measurement: other.to_owned(),
            });
        }
    }
    let (Some(spend_id), Some(version), Some(digest_value), Some((from, to))) = (
        spend.spend_id,
        spend.version,
        &spend.digest_value,
        spend.coverage,
    ) else {
        return Err(AssemblyRefusal::SpendInconsistent {
            detail: "measured, but without identity, digest or coverage",
        });
    };
    let amount = match &spend.amount {
        OutcomeState::Verified(amount) => amount,
        OutcomeState::Absent(reason) => return Ok(OutcomeState::Absent(reason.clone())),
        OutcomeState::Broken(reason) => return Ok(OutcomeState::Broken(reason.clone())),
        OutcomeState::NotEvaluated(reason) => {
            return Ok(OutcomeState::NotEvaluated(reason.clone()));
        }
    };
    let measurement = if spend.measurement == "partially_measured" {
        SpendMeasurementKind::PartiallyMeasured
    } else {
        SpendMeasurementKind::Measured
    };
    Ok(OutcomeState::Verified(SpendRef {
        workspace_id,
        spend_id,
        schema_version: version,
        digest: SourceDigest {
            algorithm: DIGEST_ALGORITHM.to_owned(),
            canonicalizer: SPEND_CANONICALIZER.to_owned(),
            value: digest_value.clone(),
        },
        amount_minor: amount.amount_minor,
        currency: amount.unit.clone(),
        minor_units_exponent: amount.minor_units_exponent,
        coverage_from_event_seq: from,
        coverage_to_event_seq: to,
        measurement,
    }))
}

// ---------------------------------------------------------------------------------------------
// The text: read through the owners, never a projection
// ---------------------------------------------------------------------------------------------

/// The literal an Artifact's title cell carries. `ArtifactV1` has no title field -- an Artifact is
/// content-addressed bytes with a media type -- so there is nothing to read, and a blank is the one
/// thing this document never prints for an absence.
pub const ARTIFACT_TITLE_ABSENT: &str = "absent (an Artifact record carries no title)";

/// Why the document's text could not be produced.
///
/// A text that cannot be re-read through its owner is refused by name. `VerifiedSources` carries
/// plain strings, so there is no `Broken` slot inside it to fill; the honest alternative to a blank
/// is to not produce the sources at all and say which text and why.
#[derive(Debug)]
pub enum SourcesRefusal {
    /// The completion the manifest is frozen on was refused by its owning reader.
    Completion(CompletionReadRefusal),
    /// This Workspace admitted no Mission with the manifest's identity.
    MissionNotAdmitted,
    /// The Mission's stored row does not agree with the Event that admitted it.
    MissionUnverified,
    /// The Mission's declared conditions are not the ones the completion was written against: the
    /// completion's own `declared_conditions_sha256` says so.
    MissionDeclarationMoved,
    /// The manifest cites a condition ordinal the Mission never declared.
    ConditionStatementMissing { ordinal: u32 },
    /// The manifest cites a narrative item the completion does not carry.
    NarrativeMissing { what: &'static str, ordinal: u32 },
    /// A delivered Artifact the manifest cites does not read back through its owner.
    ArtifactUnreadable { digest: String, reason: String },
    /// A delivered Artifact the manifest cites is not in this Workspace's store.
    ArtifactAbsent { digest: String },
    /// The completion's own bytes could not be re-encoded to be measured.
    CompletionBytes,
}

impl std::fmt::Display for SourcesRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Completion(refusal) => write!(formatter, "the completion: {refusal}"),
            Self::MissionNotAdmitted => {
                formatter.write_str("this Workspace admitted no such Mission")
            }
            Self::MissionUnverified => {
                formatter.write_str("the Mission's row disagrees with its admission Event")
            }
            Self::MissionDeclarationMoved => formatter.write_str(
                "the Mission's declared conditions are not the ones the completion recorded",
            ),
            Self::ConditionStatementMissing { ordinal } => {
                write!(
                    formatter,
                    "the Mission declares no condition at ordinal {ordinal}"
                )
            }
            Self::NarrativeMissing { what, ordinal } => {
                write!(
                    formatter,
                    "the completion carries no {what} item at ordinal {ordinal}"
                )
            }
            Self::ArtifactUnreadable { digest, reason } => {
                write!(
                    formatter,
                    "delivered Artifact {digest} does not read back: {reason}"
                )
            }
            Self::ArtifactAbsent { digest } => {
                write!(
                    formatter,
                    "delivered Artifact {digest} is not in this Workspace"
                )
            }
            Self::CompletionBytes => formatter.write_str("the completion's bytes do not re-encode"),
        }
    }
}

impl std::error::Error for SourcesRefusal {}

impl From<CompletionReadRefusal> for SourcesRefusal {
    fn from(refusal: CompletionReadRefusal) -> Self {
        Self::Completion(refusal)
    }
}

/// The values the renderer prints beside the manifest, each read through its owner.
///
/// - the Mission title and every condition statement come from the Mission's admission record,
///   proved against its Event (`mission::verified_mission_declaration`), and the statements are
///   proved a second time against the completion's own `declared_conditions_sha256` -- the seam
///   check nobody else makes, because it spans two owners;
/// - every narrative summary comes from the completion's canonical bytes
///   (`verified_completion_within`), never from `CollectedNarrative`, which carries no text;
/// - every delivered Artifact's media type comes from its record through the Artifact reader; the
///   title is the literal [`ARTIFACT_TITLE_ABSENT`], because the record has none.
///
/// `verified_source_bytes` counts what was actually read through owners: the completion's bytes,
/// the Mission's title and statements, and each delivered Artifact's content. The renderer holds
/// the bound; this reports the measurement.
pub(crate) fn verified_sources_for(
    connection: &Connection,
    workspace_id: Uuid,
    manifest: &OutcomeManifestV1,
) -> Result<VerifiedSources, SourcesRefusal> {
    let completion =
        verified_completion_within(connection, workspace_id, &manifest.completion.digest.value)?;
    let mut bytes_read: u64 = completion
        .canonical_bytes()
        .map_err(|_| SourcesRefusal::CompletionBytes)?
        .len() as u64;

    let declaration =
        crate::mission::verified_mission_declaration(connection, workspace_id, manifest.mission_id)
            .map_err(|_| SourcesRefusal::MissionUnverified)?
            .ok_or(SourcesRefusal::MissionNotAdmitted)?;
    // Two owners, one fact: the conditions the Mission declares must be the ones the completion
    // was written against. The completion carries the digest; the Mission carries the text.
    if declared_conditions_digest(&declaration.success_conditions)
        != completion.declared_conditions_sha256
    {
        return Err(SourcesRefusal::MissionDeclarationMoved);
    }
    bytes_read += declaration.desired_outcome.len() as u64;

    let mut condition_statements = Vec::with_capacity(manifest.success_conditions.len());
    for condition in &manifest.success_conditions {
        let text = declaration
            .success_conditions
            .get(condition.ordinal as usize)
            .ok_or(SourcesRefusal::ConditionStatementMissing {
                ordinal: condition.ordinal,
            })?;
        bytes_read += text.len() as u64;
        condition_statements.push(VerifiedText {
            id: condition.condition_id.clone(),
            text: text.clone(),
        });
    }

    let narrative = |refs: &OutcomeState<Vec<NarrativeRef>>,
                     what: &'static str,
                     summaries: &[(u32, &str)]|
     -> Result<Vec<VerifiedText>, SourcesRefusal> {
        let Some(refs) = refs.verified() else {
            return Ok(Vec::new());
        };
        refs.iter()
            .map(|reference| {
                summaries
                    .iter()
                    .find(|(ordinal, _)| *ordinal == reference.ordinal)
                    .map(|(_, summary)| VerifiedText {
                        id: reference.item_id.clone(),
                        text: (*summary).to_owned(),
                    })
                    .ok_or(SourcesRefusal::NarrativeMissing {
                        what,
                        ordinal: reference.ordinal,
                    })
            })
            .collect()
    };
    let unfinished: Vec<(u32, &str)> = completion
        .unfinished_work
        .iter()
        .map(|item| (item.ordinal, item.summary.as_str()))
        .collect();
    let uncertain: Vec<(u32, &str)> = completion
        .uncertainty
        .iter()
        .map(|item| (item.ordinal, item.summary.as_str()))
        .collect();
    let unfinished_work = narrative(&manifest.unfinished_work, "unfinished work", &unfinished)?;
    let uncertainty = narrative(&manifest.uncertainty, "uncertainty", &uncertain)?;

    let mut artifacts = Vec::new();
    if let Some(refs) = manifest.delivered_artifacts.verified() {
        for reference in refs {
            let record = crate::artifact::verified_artifact_within(
                connection,
                workspace_id,
                &reference.digest.value,
            )
            .map_err(|error| SourcesRefusal::ArtifactUnreadable {
                digest: reference.digest.value.clone(),
                reason: format!("{error:?}"),
            })?
            .ok_or_else(|| SourcesRefusal::ArtifactAbsent {
                digest: reference.digest.value.clone(),
            })?;
            bytes_read += record.content().len() as u64;
            artifacts.push(VerifiedArtifact {
                digest_value: record.digest().value.clone(),
                media_type: record.payload().media_type.clone(),
                title: ARTIFACT_TITLE_ABSENT.to_owned(),
            });
        }
    }

    Ok(VerifiedSources {
        mission_title: declaration.desired_outcome,
        condition_statements,
        artifacts,
        unfinished_work,
        uncertainty,
        verified_source_bytes: bytes_read,
    })
}

#[cfg(test)]
mod tests {
    //! Slice 3b item 0b, item 1: the pure assembler `CollectedSources -> OutcomeManifestV1`.
    //!
    //! Hand-built collected values, because the assembler is pure over them: what it does with a
    //! collected set is observable without a store, and every refusal here has to be reachable
    //! without first breaking a store to arrange it. In-crate, not under `tests/`, because a
    //! hand-built `CollectedSources` is exactly what the seal on that type forbids outside the
    //! crate (sol1's P0-1) -- the crate may build one to test its own pure function; nobody else
    //! may build one to feed it.

    use super::*;
    use crate::outcome_collector::CollectorProof;
    use crate::{
        AppendixRequest, AssemblyRefusal, CollectedClassification, CollectedCompletion,
        CollectedCondition, CollectedDigest, CollectedNarrative, CollectedSources, CollectedSpend,
        ConditionOutcome, ConditionResult, OutcomeState, ReceiptRef, SourceKind, SpendAmount,
        SpendMeasurementKind, VerifiedSources, VerifiedText, render,
    };
    use uuid::Uuid;

    fn hex(byte: &str) -> String {
        byte.repeat(32)
    }

    fn workspace() -> Uuid {
        Uuid::from_u128(0x1)
    }

    /// One completion, concluded, with every honesty state among its four conditions and nothing else
    /// collected beside it.
    fn collected() -> CollectedSources {
        CollectedSources {
            proposals: Vec::new(),
            rulings: Vec::new(),
            evaluations: Vec::new(),
            receipts: Vec::new(),
            completion: Some(CollectedCompletion {
                digest_value: hex("22"),
                completion_id: Uuid::from_u128(0x22),
                mission_id: Uuid::from_u128(0x11),
                project_id: Uuid::from_u128(0x10),
                accountable_resident_id: Uuid::from_u128(0x77),
                boundary_event_seq: 40,
                completed_at_ms: 1_788_400_000_000,
                condition_results: vec![
                    CollectedCondition {
                        ordinal: 0,
                        outcome: ConditionOutcome::Met,
                        support_count: 0,
                    },
                    CollectedCondition {
                        ordinal: 1,
                        outcome: ConditionOutcome::NotMet,
                        support_count: 0,
                    },
                    CollectedCondition {
                        ordinal: 2,
                        outcome: ConditionOutcome::AssessmentInconclusive,
                        support_count: 0,
                    },
                    CollectedCondition {
                        ordinal: 3,
                        outcome: ConditionOutcome::NotAssessed,
                        support_count: 0,
                    },
                ],
                delivered_artifacts: OutcomeState::Absent(
                    "the completion delivered no Artifacts".to_owned(),
                ),
                unfinished_work: vec![CollectedNarrative {
                    ordinal: 0,
                    kind: "deferred_scope".to_owned(),
                    condition_ordinal: Some(1),
                }],
                uncertainty: Vec::new(),
            }),
            classification: Some(CollectedClassification {
                digest_value: hex("33"),
                classification_id: Uuid::from_u128(0x33),
                version: 1,
                mission_id: Uuid::from_u128(0x11),
                project_id: Uuid::from_u128(0x10),
                policy_revision: 1,
                charter_revision: 1,
                policy_high_impact: false,
            }),
            spend: Some(CollectedSpend {
                digest_value: None,
                spend_id: None,
                version: None,
                measurement: "not_measured".to_owned(),
                coverage: None,
                amount: OutcomeState::NotEvaluated("no ledger was read.".to_owned()),
            }),
            boundary_event_seq: Some(40),
            proof: CollectorProof,
        }
    }

    fn sources_for(manifest: &crate::OutcomeManifestV1) -> VerifiedSources {
        VerifiedSources {
            mission_title: "Ship one reviewed feature.".to_owned(),
            condition_statements: manifest
                .success_conditions
                .iter()
                .map(|condition| VerifiedText {
                    id: condition.condition_id.clone(),
                    text: format!("Condition {}", condition.ordinal),
                })
                .collect(),
            artifacts: Vec::new(),
            unfinished_work: vec![VerifiedText {
                id: "0".to_owned(),
                text: "The second condition's probe was deferred.".to_owned(),
            }],
            uncertainty: Vec::new(),
            verified_source_bytes: 1_024,
        }
    }

    #[test]
    fn an_assembled_manifest_is_grammatical_and_carries_only_what_was_collected() {
        let manifest =
            assemble(&collected(), workspace(), AppendixRequest::PolicyDefault).expect("assembles");
        // Identity, not a digest, for the Mission (R18.4); the completion and Policy carry theirs.
        assert_eq!(manifest.mission_id, Uuid::from_u128(0x11));
        assert_eq!(manifest.boundary_event_seq, 40);
        assert_eq!(manifest.accountable_resident_id, Uuid::from_u128(0x77));
        assert_eq!(
            manifest
                .completion
                .identity
                .map(|identity| identity.object_id),
            Some(Uuid::from_u128(0x22))
        );
        assert_eq!(
            manifest.completion.digest.canonicalizer,
            "beacon.mission_completion.v1"
        );
        assert_eq!(
            manifest.policy_classification.digest.canonicalizer,
            "beacon.mission-impact.v1"
        );
        assert!(!manifest.policy_high_impact);
        // The owner's four arms, none collapsed (R18.3).
        let outcomes: Vec<_> = manifest
            .success_conditions
            .iter()
            .map(|condition| condition.outcome.clone())
            .collect();
        assert_eq!(
            outcomes,
            vec![
                OutcomeState::Verified(ConditionResult::Met),
                OutcomeState::Verified(ConditionResult::NotMet),
                OutcomeState::Verified(ConditionResult::Inconclusive),
                OutcomeState::NotEvaluated("not_assessed".to_owned()),
            ]
        );
        // Its tokens are the owner's ordinals.
        assert_eq!(manifest.success_conditions[2].condition_id, "2");
        // And the whole thing encodes under the v2 grammar and renders.
        let bytes = manifest.canonical_bytes().expect("encodes");
        assert_eq!(
            crate::OutcomeManifestV1::from_canonical_bytes(&bytes).expect("decodes"),
            manifest
        );
        let document = render(&manifest, &sources_for(&manifest)).expect("renders");
        assert!(document.contains("`assessment_inconclusive`"), "{document}");
        assert!(document.contains("`not_evaluated`"), "{document}");
    }

    #[test]
    fn assembler_refuses_to_guess_an_absent_field() {
        let workspace = workspace();
        let base = collected();

        // Nothing collected for a lane is `Absent`, never an empty verified list: "the Mission cited
        // no Proposal" and "no Proposal record exists" are different claims, and an empty `Verified`
        // would render the second as if it were a finding (acceptance mutant 2).
        let manifest = assemble(&base, workspace, AppendixRequest::PolicyDefault).unwrap();
        assert!(
            matches!(manifest.proposal_refs, OutcomeState::Absent(_)),
            "{:?}",
            manifest.proposal_refs
        );
        assert!(matches!(manifest.ruling_refs, OutcomeState::Absent(_)));
        assert!(matches!(manifest.evaluation_refs, OutcomeState::Absent(_)));
        assert!(matches!(manifest.receipt_refs, OutcomeState::Absent(_)));
        assert!(matches!(manifest.uncertainty, OutcomeState::Absent(_)));
        // What the collector already stated as a state passes through as that state.
        assert!(matches!(
            manifest.delivered_artifacts,
            OutcomeState::Absent(_)
        ));
        assert!(matches!(manifest.spend_ref, OutcomeState::NotEvaluated(_)));

        // No completion: nothing to assemble, nothing defaulted.
        let mut no_completion = base.clone();
        no_completion.completion = None;
        assert_eq!(
            assemble(&no_completion, workspace, AppendixRequest::PolicyDefault).unwrap_err(),
            AssemblyRefusal::NoCompletion
        );
        // No classification: the appendix bit would have to be picked.
        let mut no_policy = base.clone();
        no_policy.classification = None;
        assert_eq!(
            assemble(&no_policy, workspace, AppendixRequest::PolicyDefault).unwrap_err(),
            AssemblyRefusal::NoClassification
        );
        // Support the collector counted but did not carry: a citation cannot come from a number.
        let mut counted = base.clone();
        counted.completion.as_mut().unwrap().condition_results[0].support_count = 2;
        assert_eq!(
            assemble(&counted, workspace, AppendixRequest::PolicyDefault).unwrap_err(),
            AssemblyRefusal::SupportNotCollected {
                ordinal: 0,
                count: 2
            }
        );
        // A collected source from another Workspace: the set disagrees with itself.
        let mut foreign = base.clone();
        foreign.proposals.push(CollectedDigest {
            kind: SourceKind::Proposal,
            workspace_id: Uuid::from_u128(0xdead),
            digest_algorithm: "sha256".to_owned(),
            digest_canonicalizer: "beacon.proposal.v1".to_owned(),
            digest_value: hex("55"),
        });
        assert!(matches!(
            assemble(&foreign, workspace, AppendixRequest::PolicyDefault).unwrap_err(),
            AssemblyRefusal::CrossWorkspace {
                kind: "proposal",
                ..
            }
        ));
    }

    #[test]
    fn a_partially_measured_spend_maps_to_its_own_state_with_its_coverage_intact() {
        // R21.3 by way of R23.1: a window that ends before the boundary is `partially_measured`, a
        // state of its own. Until the grammar carried the token the assembler refused this shape by
        // name (`SpendPartialityNotRepresentable`, RED-then-removed); now the token rides in the
        // `SpendRef` and the document prints it beside the amount, and the assembler maps -- never to
        // `Measured`, never to a footnote.
        let mut partial = collected();
        partial.spend = Some(CollectedSpend {
            digest_value: Some(hex("66")),
            spend_id: Some(Uuid::from_u128(0x66)),
            version: Some(1),
            measurement: "partially_measured".to_owned(),
            coverage: Some((1, 30)),
            amount: OutcomeState::Verified(SpendAmount {
                unit: "USD".to_owned(),
                amount_minor: 12_345,
                minor_units_exponent: 2,
                counted_units: 400,
            }),
        });
        let manifest = assemble(&partial, workspace(), AppendixRequest::PolicyDefault).unwrap();
        let spend = manifest
            .spend_ref
            .verified()
            .expect("a partial measurement is a Verified state");
        assert_eq!(spend.measurement, SpendMeasurementKind::PartiallyMeasured);
        assert_eq!(
            (spend.coverage_from_event_seq, spend.coverage_to_event_seq),
            (1, 30)
        );
        assert!(
            spend.coverage_to_event_seq < manifest.boundary_event_seq,
            "the window ends before the cut"
        );
        assert_eq!(spend.amount_minor, 12_345);
        let document = render(&manifest, &sources_for(&manifest)).expect("renders");
        assert!(
            document.contains("| measurement | `partially_measured` |"),
            "{document}"
        );
        assert!(document.contains("floor over that window"), "{document}");

        // The complete measurement, same shape, is the other arm -- and only the token differs.
        let mut complete = partial.clone();
        complete.spend.as_mut().unwrap().measurement = "measured".to_owned();
        complete.spend.as_mut().unwrap().coverage = Some((1, 40));
        let manifest = assemble(&complete, workspace(), AppendixRequest::PolicyDefault).unwrap();
        let spend = manifest
            .spend_ref
            .verified()
            .expect("a complete measurement is Verified");
        assert_eq!(spend.measurement, SpendMeasurementKind::Measured);
        assert_eq!(
            (spend.coverage_from_event_seq, spend.coverage_to_event_seq),
            (1, 40)
        );
        assert_eq!(spend.currency, "USD");
    }

    /// R23.5: the Policy citation's schema version is the version the collector's owning read proved,
    /// copied -- never a constant. A second classification of a Mission is version 2, and a citation
    /// that still said 1 would name a record that is not the one collected.
    #[test]
    fn the_policy_citation_carries_the_collected_classifications_version() {
        let mut second = collected();
        second.classification.as_mut().unwrap().version = 2;
        let manifest = assemble(&second, workspace(), AppendixRequest::PolicyDefault).unwrap();
        assert_eq!(
            manifest
                .policy_classification
                .identity
                .map(|identity| identity.schema_version),
            Some(2)
        );
    }

    #[test]
    fn cited_sources_keep_the_envelope_the_owner_returned() {
        // A Ruling has two grammars on main; the assembler must not pick one. The envelope the
        // collector carried from the owning reader is the envelope the manifest cites.
        let mut with_ruling = collected();
        with_ruling.rulings.push(CollectedDigest {
            kind: SourceKind::Ruling,
            workspace_id: workspace(),
            digest_algorithm: "sha256".to_owned(),
            digest_canonicalizer: "beacon.ruling.v2".to_owned(),
            digest_value: hex("88"),
        });
        with_ruling.receipts.push(ReceiptRef {
            workspace_id: workspace(),
            run_uuid: Uuid::from_u128(0x99),
            receipt_version: 2,
            outcome_checksum: hex("99"),
        });
        let manifest = assemble(&with_ruling, workspace(), AppendixRequest::Include).unwrap();
        let rulings = manifest.ruling_refs.verified().expect("cited");
        assert_eq!(rulings[0].digest.canonicalizer, "beacon.ruling.v2");
        assert!(
            rulings[0].identity.is_none(),
            "a Ruling carries no reference identity yet (R18.5)"
        );
        assert_eq!(manifest.receipt_refs.verified().map(Vec::len), Some(1));
        manifest
            .canonical_bytes()
            .expect("encodes with a v2 Ruling citation");
    }
}
