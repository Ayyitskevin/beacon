//! The C4 request DTO's wire shape, at the serde boundary.
//!
//! Outcome-independent: no store, no account, no Mission. Every expected key and every rejection is
//! a literal written from `C4-interface.md`, never a shape read back out of the derive.

use beacond::{CompletionIntentRequest, SupportRefDto};

const RUN_ID: &str = "00000000-0000-4000-8000-00000000000a";
const MISSION: &str = "00000000-0000-4000-8000-000000000001";
const RESIDENT: &str = "00000000-0000-4000-8000-000000000002";
const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// One complete, contract-shaped request. Every key present; both optionals explicitly null.
fn contract_request_json() -> String {
    format!(
        r#"{{"missionId":"{MISSION}","accountableResidentId":"{RESIDENT}","disposition":"concluded",
"conditionResults":[{{"ordinal":0,"outcome":"met","support":[{{"receipt":{{"runId":"{RUN_ID}","receiptVersion":7,"receiptBytesSha256":"{DIGEST_A}"}}}}],"note":null}}],
"deliveredArtifacts":["{DIGEST_B}"],
"unfinishedWork":[{{"ordinal":0,"kind":"deferred_scope","conditionOrdinal":null,"summary":"Deferred."}}],
"uncertainty":[],
"spend":{{"notMeasured":"No spend record exists."}},
"summary":"One reviewed feature shipped."}}"#
    )
    .replace('\n', "")
}

#[test]
fn c4_the_contract_request_shape_deserializes() {
    let request: CompletionIntentRequest = serde_json::from_str(&contract_request_json())
        .expect("C4.OWNER.INPUT_DOMAINS: the contract shape");
    assert_eq!(request.mission_id.to_string(), MISSION);
    assert_eq!(request.condition_results.len(), 1);
    assert!(request.condition_results[0].note.is_none());
    assert!(request.unfinished_work[0].condition_ordinal.is_none());
}

#[test]
fn c4_an_omitted_optional_key_refuses_while_explicit_null_passes() {
    // The contract says every field is required and an absent value is written as explicit null.
    // Serde supplies `None` for a MISSING `Option` field unless the type refuses it, so this is the
    // bar that tells "the Operator recorded no note" from "the client forgot the key".
    for (field, omitted) in [
        (
            "conditionResults[].note",
            contract_request_json().replace(r#","note":null"#, ""),
        ),
        (
            "unfinishedWork[].conditionOrdinal",
            contract_request_json().replace(r#""conditionOrdinal":null,"#, ""),
        ),
    ] {
        assert!(
            omitted != contract_request_json(),
            "C4.OWNER.INPUT_DOMAINS.EXPLICIT_NULL fixture: {field} was not actually omitted"
        );
        assert!(
            serde_json::from_str::<CompletionIntentRequest>(&omitted).is_err(),
            "C4.OWNER.INPUT_DOMAINS.EXPLICIT_NULL: omitting {field} was accepted; json={omitted}"
        );
    }
    // And the same request WITH the keys, explicitly null, still passes.
    assert!(
        serde_json::from_str::<CompletionIntentRequest>(&contract_request_json()).is_ok(),
        "C4.OWNER.INPUT_DOMAINS.EXPLICIT_NULL: explicit null must pass"
    );
}

#[test]
fn c4_every_support_arm_uses_the_contracts_camel_case_field_spellings() {
    // Literal contract JSON, not serde-generated expectations. A struct-variant's FIELDS are not
    // renamed by an enum-level rename_all, so this is where that would surface.
    let receipt = format!(
        r#"{{"receipt":{{"runId":"{RUN_ID}","receiptVersion":7,"receiptBytesSha256":"{DIGEST_A}"}}}}"#
    );
    let parsed: SupportRefDto = serde_json::from_str(&receipt)
        .expect("C4.OWNER.INPUT_DOMAINS.WIRE_SHAPE: the contract Receipt spelling");
    match &parsed {
        SupportRefDto::Receipt {
            run_id,
            receipt_version,
            receipt_bytes_sha256,
        } => {
            assert_eq!(run_id.to_string(), RUN_ID);
            assert_eq!(*receipt_version, 7);
            assert_eq!(receipt_bytes_sha256, DIGEST_A);
        }
        other => panic!("C4.OWNER.INPUT_DOMAINS.WIRE_SHAPE: {other:?}"),
    }
    // Round trip: what the owner emits must be what the contract spells.
    let emitted = serde_json::to_string(&parsed).unwrap();
    assert_eq!(
        emitted, receipt,
        "C4.OWNER.INPUT_DOMAINS.WIRE_SHAPE: the emitted Receipt does not match the contract"
    );

    for (case, json) in [
        ("artifact", format!(r#"{{"artifact":"{DIGEST_A}"}}"#)),
        ("evaluation", format!(r#"{{"evaluation":"{DIGEST_A}"}}"#)),
        ("ruling", format!(r#"{{"ruling":"{DIGEST_A}"}}"#)),
    ] {
        let parsed: SupportRefDto = serde_json::from_str(&json)
            .unwrap_or_else(|error| panic!("C4.OWNER.INPUT_DOMAINS.WIRE_SHAPE {case}: {error}"));
        assert_eq!(
            serde_json::to_string(&parsed).unwrap(),
            json,
            "C4.OWNER.INPUT_DOMAINS.WIRE_SHAPE {case}"
        );
    }
}

#[test]
fn c4_the_spend_arms_use_the_contracts_camel_case_field_spellings() {
    let recorded = format!(
        r#"{{"recorded":{{"spendId":"{RUN_ID}","spendVersion":3,"spendSha256":"{DIGEST_B}"}}}}"#
    );
    let with_recorded =
        contract_request_json().replace(r#"{"notMeasured":"No spend record exists."}"#, &recorded);
    let parsed: CompletionIntentRequest = serde_json::from_str(&with_recorded)
        .expect("C4.OWNER.INPUT_DOMAINS.WIRE_SHAPE: the contract Recorded spelling");
    assert_eq!(
        serde_json::to_string(&parsed.spend).unwrap(),
        recorded,
        "C4.OWNER.INPUT_DOMAINS.WIRE_SHAPE: the emitted spend does not match the contract"
    );
}

#[test]
fn c4_an_unknown_field_or_tag_refuses() {
    let extra = contract_request_json().replace(
        r#""summary":"One reviewed feature shipped.""#,
        r#""summary":"One reviewed feature shipped.","surprise":1"#,
    );
    assert!(
        serde_json::from_str::<CompletionIntentRequest>(&extra).is_err(),
        "C4.OWNER.INPUT_DOMAINS: an unknown field was accepted"
    );
    assert!(
        serde_json::from_str::<SupportRefDto>(r#"{"invention":"x"}"#).is_err(),
        "C4.OWNER.INPUT_DOMAINS: an unknown support tag was accepted"
    );
}
