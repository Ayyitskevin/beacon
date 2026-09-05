//! Slice 3b item 0b, item 1: the pure assembler `CollectedSources -> OutcomeManifestV1`.
//!
//! Hand-built collected values, because the assembler is pure over them: what it does with a
//! collected set is observable without a store, and every refusal here has to be reachable without
//! first breaking a store to arrange it.

use beacond::{
    AppendixRequest, AssemblyRefusal, CollectedClassification, CollectedCompletion,
    CollectedCondition, CollectedDigest, CollectedNarrative, CollectedSources, CollectedSpend,
    ConditionOutcome, ConditionResult, OutcomeState, ReceiptRef, SourceKind, SpendAmount,
    SpendMeasurementKind, VerifiedSources, VerifiedText, assemble, render,
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
    }
}

fn sources_for(manifest: &beacond::OutcomeManifestV1) -> VerifiedSources {
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
        beacond::OutcomeManifestV1::from_canonical_bytes(&bytes).expect("decodes"),
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
