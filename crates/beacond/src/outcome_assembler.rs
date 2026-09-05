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

use uuid::Uuid;

use crate::mission_completion::COMPLETION_CANONICALIZER;
use crate::mission_impact::CANONICALIZER as MISSION_IMPACT_CANONICALIZER;
use crate::outcome::{
    AppendixRequest, CANONICALIZER, NarrativeRef, OutcomeManifestV1, OutcomeState, RENDERER_V1,
    ReferenceIdentity, SourceDigest, SourceKind, SourceRef, SpendRef, SuccessCondition,
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
    /// The completion's spend was `partially_measured` (R21.3), and `SpendRef` has no way to say
    /// so. Rendering it as a complete measurement is exactly the laundering R21.3 forbids, so the
    /// assembler refuses and names the window rather than choosing the nearest representable state.
    SpendPartialityNotRepresentable {
        coverage_to_event_seq: u64,
        boundary_event_seq: u64,
    },
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
            Self::SpendPartialityNotRepresentable {
                coverage_to_event_seq,
                boundary_event_seq,
            } => write!(
                formatter,
                "the spend is partially measured (coverage ends at event {coverage_to_event_seq}, \
                 the boundary is {boundary_event_seq}) and the manifest grammar cannot say so"
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
pub fn assemble(
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

    let spend_ref = spend_state(collected, workspace_id, boundary_event_seq)?;

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
                // The owner's grammar is v1 and the collector's citation reader
                // (`verified_classification_for_citation`) refuses any other version at
                // collection; the collector does not carry the number separately. Stated rather
                // than assumed.
                schema_version: 1,
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
    boundary_event_seq: u64,
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
        "partially_measured" => {
            return Err(AssemblyRefusal::SpendPartialityNotRepresentable {
                coverage_to_event_seq: spend.coverage.map(|(_, to)| to).unwrap_or_default(),
                boundary_event_seq,
            });
        }
        "measured" => {}
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
    }))
}
