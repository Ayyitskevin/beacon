//! Mission-plan Invocation, slice C1: the record's canonical bytes and the finite transport
//! grammar.
//!
//! Every fixture is constructed. Slice C1 owns no database, no service and no Event, so a test that
//! needed one would be testing a later slice — and the storage, durability, restart, permit and
//! lock bars are deliberately absent here rather than approximated.
//!
//! The admission's canonical bytes are assembled twice: once by Beacon's encoder and once by
//! [`admission_bytes_by_hand`], which follows the documented field order rather than calling that
//! encoder. That independence is the point — a canonicalizer that quietly changed its layout would
//! agree with itself and disagree with this file.
//!
//! The grammar is closed twice over as well: the acceptor is driven across every string of length
//! 0..=4 over `{H, P, C, I}` and compared against [`ADMITTED_WORDS`], a literal set written from
//! the conductor's disposition rather than read out of the implementation.

use beacond::{
    AppendRefusal, Appended, IncompleteCause, InvocationAdmissionV1, InvocationObservationV1,
    MalformedInvocation, ObservationKind, ReadCeiling, RetainedAuthority, RetainedHeader,
    RetainedLimits, TransitionRefusal, TransportHistory, TransportObservation, TransportStatus,
};
use uuid::Uuid;

const INVOCATION: &str = "00000000-0000-4000-8000-000000000001";
const WORKSPACE: &str = "00000000-0000-4000-8000-000000000002";
const PROJECT: &str = "00000000-0000-4000-8000-000000000003";
const MISSION: &str = "00000000-0000-4000-8000-000000000004";
const RESIDENT: &str = "00000000-0000-4000-8000-000000000005";
const CONFIGURATION: &str = "00000000-0000-4000-8000-000000000006";
const ACTIVATION: &str = "00000000-0000-4000-8000-000000000007";
const OTHER_INVOCATION: &str = "00000000-0000-4000-8000-00000000000f";

const AUTHORITY_REFERENCE: &str = "beacon.invocation_authority.resolved.v1";
const AUTHORITY_DIGEST: [u8; 32] = [0x11; 32];
const PROMPT_PROFILE: &str = "beacon.mission-plan.prompt.v1";
const REQUEST_DIGEST: [u8; 32] = [0x22; 32];
const REQUEST_BYTES_LEN: u32 = 1_024;
const LIMIT_REQUEST: u32 = 4_096;
const LIMIT_RESPONSE: u32 = 8_192;
const LIMIT_PREFIX: u32 = 512;
const ADMITTED_AT_MS: i64 = 1_788_372_724_000;

/// The eight admitted transport words, written from `DN30-astra-disposition.md` §1 and not derived
/// from the acceptor: `A`, `AH`, `AHP`, `AHC`, `AHI`, `AHPC`, `AHPI`, `AI`, with the leading `A`
/// (the admission itself) dropped because every history starts admitted.
const ADMITTED_WORDS: [&str; 8] = ["", "H", "HC", "HI", "HP", "HPC", "HPI", "I"];

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("canonical uuid")
}

/// A ceiling large enough for every fixture here, supplied by this caller because there is no
/// default and no record may choose one.
fn ceiling() -> ReadCeiling {
    ReadCeiling::new(64 * 1024, 16 * 1024)
}

fn admission() -> InvocationAdmissionV1 {
    InvocationAdmissionV1 {
        invocation_id: uuid(INVOCATION),
        workspace_id: uuid(WORKSPACE),
        project_id: uuid(PROJECT),
        mission_id: uuid(MISSION),
        resident_id: uuid(RESIDENT),
        admitted_configuration_id: uuid(CONFIGURATION),
        activation_id: uuid(ACTIVATION),
        authority: RetainedAuthority {
            reference: AUTHORITY_REFERENCE.to_owned(),
            claimed_digest: AUTHORITY_DIGEST,
        },
        prompt_profile: PROMPT_PROFILE.to_owned(),
        request_digest: REQUEST_DIGEST,
        request_bytes_len: REQUEST_BYTES_LEN,
        limits: RetainedLimits {
            request_bytes: LIMIT_REQUEST,
            response_bytes: LIMIT_RESPONSE,
            prefix_bytes: LIMIT_PREFIX,
        },
        admitted_at_ms: ADMITTED_AT_MS,
    }
}

/// The admission's canonical bytes, assembled from the documented field order by hand.
fn admission_bytes_by_hand() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"beacon.model_invocation.admission.v1\0");
    for id in [
        INVOCATION,
        WORKSPACE,
        PROJECT,
        MISSION,
        RESIDENT,
        CONFIGURATION,
        ACTIVATION,
    ] {
        bytes.extend_from_slice(uuid(id).as_bytes());
    }
    push_framed(&mut bytes, AUTHORITY_REFERENCE.as_bytes());
    bytes.extend_from_slice(&AUTHORITY_DIGEST);
    push_framed(&mut bytes, PROMPT_PROFILE.as_bytes());
    bytes.extend_from_slice(&REQUEST_DIGEST);
    bytes.extend_from_slice(&REQUEST_BYTES_LEN.to_be_bytes());
    bytes.extend_from_slice(&LIMIT_REQUEST.to_be_bytes());
    bytes.extend_from_slice(&LIMIT_RESPONSE.to_be_bytes());
    bytes.extend_from_slice(&LIMIT_PREFIX.to_be_bytes());
    bytes.extend_from_slice(&ADMITTED_AT_MS.to_be_bytes());
    bytes
}

fn push_framed(out: &mut Vec<u8>, value: &[u8]) {
    out.extend_from_slice(&(value.len() as u64).to_be_bytes());
    out.extend_from_slice(value);
}

fn headers(status: u16) -> TransportObservation {
    TransportObservation::Headers {
        status,
        headers: vec![RetainedHeader {
            name: "content-type".to_owned(),
            value: "application/json".to_owned(),
        }],
    }
}

fn observation_for(kind: ObservationKind) -> TransportObservation {
    match kind {
        ObservationKind::Headers => headers(200),
        ObservationKind::BodyPrefix => TransportObservation::BodyPrefix {
            bytes: b"{\"plan\":".to_vec(),
        },
        ObservationKind::BodyComplete => TransportObservation::BodyComplete {
            bytes: b"{\"plan\":\"draft\"}".to_vec(),
            elapsed_ms: 41,
        },
        ObservationKind::BodyIncomplete => TransportObservation::BodyIncomplete {
            prefix: b"{\"plan\":".to_vec(),
            cause: IncompleteCause::DeadlineElapsed,
        },
    }
}

fn kind_of(letter: char) -> ObservationKind {
    match letter {
        'H' => ObservationKind::Headers,
        'P' => ObservationKind::BodyPrefix,
        'C' => ObservationKind::BodyComplete,
        'I' => ObservationKind::BodyIncomplete,
        other => panic!("not a grammar letter: {other}"),
    }
}

/// Drive the acceptor across one word. A word is admitted only when every letter became a NEW row:
/// an idempotent repeat spells a shorter word, and is a separate question.
fn admits(word: &str) -> bool {
    let mut history = TransportHistory::admitted(&admission());
    for letter in word.chars() {
        match history.append(observation_for(kind_of(letter))) {
            Ok(Appended::Recorded { .. }) => {}
            _ => return false,
        }
    }
    history.word() == word
}

fn history_of(word: &str) -> TransportHistory {
    let mut history = TransportHistory::admitted(&admission());
    for letter in word.chars() {
        history
            .append(observation_for(kind_of(letter)))
            .unwrap_or_else(|error| panic!("{word}: {error}"));
    }
    history
}

// ---------------------------------------------------------------------------------------------
// The admission record
// ---------------------------------------------------------------------------------------------

#[test]
fn the_admission_canonical_bytes_match_a_record_assembled_by_hand() {
    let encoded = admission().canonical_bytes().expect("canonical bytes");
    assert_eq!(encoded, admission_bytes_by_hand());
}

#[test]
fn an_admission_round_trips_through_its_own_canonical_bytes() {
    let admission = admission();
    let encoded = admission.canonical_bytes().expect("canonical bytes");
    let decoded =
        InvocationAdmissionV1::from_canonical_bytes(&encoded, ceiling()).expect("decodes");
    assert_eq!(decoded, admission);
}

#[test]
fn the_admission_digest_commits_to_the_retained_authority_reference_and_its_claimed_digest() {
    let base = admission().digest().expect("digest");

    let mut other_reference = admission();
    other_reference.authority.reference = "beacon.invocation_authority.resolved.v2".to_owned();
    let renamed = other_reference.digest().expect("digest");

    let mut other_digest = admission();
    other_digest.authority.claimed_digest = [0x12; 32];
    let rehashed = other_digest.digest().expect("digest");

    assert_ne!(
        base, renamed,
        "the reference is part of the admission's identity"
    );
    assert_ne!(
        base, rehashed,
        "the claimed digest is part of the admission's identity"
    );
    assert_ne!(renamed, rehashed);
}

// ---------------------------------------------------------------------------------------------
// Two different bounds
// ---------------------------------------------------------------------------------------------

#[test]
fn a_record_longer_than_the_callers_read_ceiling_refuses_before_any_field_is_read() {
    // Deliberately not a record at all: if the ceiling were checked after parsing began, the magic
    // would be the fault reported.
    let nonsense = vec![0xff_u8; 65];
    let refusal = InvocationAdmissionV1::from_canonical_bytes(&nonsense, ReadCeiling::new(64, 16))
        .expect_err("over the caller's ceiling");
    assert_eq!(refusal, MalformedInvocation::OverReadCeiling);
}

#[test]
fn a_records_own_recorded_limits_cannot_raise_the_ceiling_that_bounds_its_parse() {
    // The record's envelope admits 8192 response bytes; this caller will allocate 16.
    let observation = InvocationObservationV1 {
        invocation_id: uuid(INVOCATION),
        sequence: 2,
        observation: TransportObservation::BodyComplete {
            bytes: vec![0x7b; 4_096],
            elapsed_ms: 7,
        },
    };
    let encoded = observation.canonical_bytes().expect("canonical bytes");
    assert!(
        u32::try_from(encoded.len()).expect("fits") < LIMIT_RESPONSE + 4_096,
        "the fixture is within the record's own recorded envelope"
    );

    let narrow = ReadCeiling::new(64 * 1024, 16);
    let refusal = InvocationObservationV1::from_canonical_bytes(&encoded, narrow)
        .expect_err("over the caller's retained-field ceiling");
    assert_eq!(refusal, MalformedInvocation::OverRetainedFieldCeiling);

    // Same bytes, same record, a caller willing to allocate them: it reads.
    let decoded =
        InvocationObservationV1::from_canonical_bytes(&encoded, ceiling()).expect("decodes");
    assert_eq!(decoded, observation);
}

#[test]
fn a_retained_length_over_the_admitted_envelope_refuses_by_a_different_name_than_the_ceiling() {
    let mut history = TransportHistory::admitted(&admission());
    history.append(headers(200)).expect("headers");

    // Within this caller's 16 KiB retained-field ceiling, over the record's own 8192-byte envelope.
    let refusal = history
        .append(TransportObservation::BodyComplete {
            bytes: vec![0x7b; usize::try_from(LIMIT_RESPONSE).expect("fits") + 1],
            elapsed_ms: 9,
        })
        .expect_err("over the admitted envelope");
    assert_eq!(
        refusal,
        AppendRefusal::Malformed(MalformedInvocation::RetainedExceedsAdmittedLimit)
    );
}

#[test]
fn an_unknown_observation_kind_tag_is_named_before_a_later_over_limit_field_is_read() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"beacon.model_invocation.observation.v1\0");
    bytes.extend_from_slice(uuid(INVOCATION).as_bytes());
    bytes.extend_from_slice(&1_u32.to_be_bytes());
    bytes.push(0x09); // not one of the four closed kinds
    // A retained field that would be far over any ceiling, placed after the unknown tag.
    bytes.extend_from_slice(&u64::MAX.to_be_bytes());

    let refusal =
        InvocationObservationV1::from_canonical_bytes(&bytes, ceiling()).expect_err("unknown kind");
    assert_eq!(refusal, MalformedInvocation::UnknownObservationTag);
}

// ---------------------------------------------------------------------------------------------
// The finite grammar
// ---------------------------------------------------------------------------------------------

#[test]
fn the_transport_grammar_admits_exactly_the_eight_words() {
    let letters = ['H', 'P', 'C', 'I'];
    let mut words = vec![String::new()];
    let mut frontier = vec![String::new()];
    for _ in 0..4 {
        let mut next = Vec::new();
        for word in &frontier {
            for letter in letters {
                let mut candidate = word.clone();
                candidate.push(letter);
                next.push(candidate);
            }
        }
        words.extend(next.iter().cloned());
        frontier = next;
    }
    assert_eq!(
        words.len(),
        341,
        "every word of length 0..=4 over four letters"
    );

    let mut admitted: Vec<String> = words.iter().filter(|word| admits(word)).cloned().collect();
    admitted.sort();
    let mut expected = ADMITTED_WORDS.map(str::to_owned).to_vec();
    expected.sort();
    assert_eq!(admitted, expected);
}

#[test]
fn an_admitted_invocation_headers_alone_and_a_complete_empty_body_are_three_distinct_facts() {
    let admitted = history_of("").status();
    let started = history_of("H").status();

    let mut complete_empty = TransportHistory::admitted(&admission());
    complete_empty.append(headers(200)).expect("headers");
    complete_empty
        .append(TransportObservation::BodyComplete {
            bytes: Vec::new(),
            elapsed_ms: 3,
        })
        .expect("complete empty");

    assert_eq!(admitted, TransportStatus::PotentiallyAttempted);
    assert_eq!(started, TransportStatus::ResponseStarted { status: 200 });
    assert_eq!(
        complete_empty.status(),
        TransportStatus::CompleteEmpty { status: 200 }
    );
    assert_ne!(admitted, started);
    assert_ne!(started, complete_empty.status());
    assert_ne!(admitted, complete_empty.status());
}

#[test]
fn a_complete_terminal_after_an_incomplete_one_refuses_in_transition_validation() {
    let mut history = history_of("HI");
    let refusal = history
        .append(observation_for(ObservationKind::BodyComplete))
        .expect_err("the transport observations are already ended");
    assert_eq!(
        refusal,
        AppendRefusal::Transition(TransitionRefusal::AfterTerminal {
            terminal: ObservationKind::BodyIncomplete,
            offered: ObservationKind::BodyComplete,
        })
    );
}

#[test]
fn an_incomplete_terminal_after_a_complete_one_refuses_in_transition_validation() {
    let mut history = history_of("HC");
    let refusal = history
        .append(observation_for(ObservationKind::BodyIncomplete))
        .expect_err("the transport observations are already ended");
    assert_eq!(
        refusal,
        AppendRefusal::Transition(TransitionRefusal::AfterTerminal {
            terminal: ObservationKind::BodyComplete,
            offered: ObservationKind::BodyIncomplete,
        })
    );
}

#[test]
fn headers_after_a_terminal_are_refused_rather_than_reopening_the_attempt() {
    let mut history = history_of("I");
    let refusal = history
        .append(observation_for(ObservationKind::Headers))
        .expect_err("a terminal ends the transport observations");
    assert_eq!(
        refusal,
        AppendRefusal::Transition(TransitionRefusal::AfterTerminal {
            terminal: ObservationKind::BodyIncomplete,
            offered: ObservationKind::Headers,
        })
    );
}

#[test]
fn a_body_prefix_with_no_headers_before_it_is_refused() {
    let mut history = TransportHistory::admitted(&admission());
    let refusal = history
        .append(observation_for(ObservationKind::BodyPrefix))
        .expect_err("a prefix requires headers");
    assert_eq!(
        refusal,
        AppendRefusal::Transition(TransitionRefusal::PrefixWithoutHeaders)
    );
}

#[test]
fn a_complete_body_with_no_headers_before_it_is_refused() {
    let mut history = TransportHistory::admitted(&admission());
    let refusal = history
        .append(observation_for(ObservationKind::BodyComplete))
        .expect_err("a complete body requires headers");
    assert_eq!(
        refusal,
        AppendRefusal::Transition(TransitionRefusal::CompleteWithoutHeaders)
    );
}

#[test]
fn re_appending_the_identical_record_adds_no_row() {
    let mut history = history_of("H");
    let record = InvocationObservationV1 {
        invocation_id: uuid(INVOCATION),
        sequence: 1,
        observation: headers(200),
    };
    let again = history.append_record(record).expect("idempotent");
    assert_eq!(again, Appended::AlreadyPresent { sequence: 1 });
    assert_eq!(history.len(), 1);
    assert_eq!(history.word(), "H");
}

#[test]
fn the_same_identity_with_different_bytes_refuses_as_an_observation_conflict() {
    let mut history = history_of("H");
    let refusal = history
        .append_record(InvocationObservationV1 {
            invocation_id: uuid(INVOCATION),
            sequence: 1,
            observation: headers(500),
        })
        .expect_err("different bytes at the same identity");
    match refusal {
        AppendRefusal::IdentityConflict {
            kind,
            recorded_digest,
            offered_digest,
        } => {
            assert_eq!(kind, ObservationKind::Headers);
            assert_ne!(recorded_digest, offered_digest);
        }
        other => panic!("expected an observation conflict, got {other}"),
    }
    assert_eq!(history.len(), 1);
}

#[test]
fn a_repeated_kind_at_a_new_sequence_is_refused_as_a_repeat_not_as_a_byte_conflict() {
    let mut history = history_of("H");
    let refusal = history
        .append(headers(200))
        .expect_err("one headers row per Invocation");
    assert_eq!(
        refusal,
        AppendRefusal::Transition(TransitionRefusal::RepeatedKind(ObservationKind::Headers))
    );
    assert_eq!(history.len(), 1);
}

#[test]
fn a_five_hundred_status_with_a_complete_body_keeps_both_facts() {
    let body = b"{\"error\":\"upstream refused\"}".to_vec();
    let mut history = TransportHistory::admitted(&admission());
    history.append(headers(500)).expect("headers");
    history
        .append(TransportObservation::BodyComplete {
            bytes: body.clone(),
            elapsed_ms: 12,
        })
        .expect("complete body");

    assert_eq!(
        history.status(),
        TransportStatus::CompleteRetained {
            status: 500,
            bytes: body,
        }
    );
}

#[test]
fn an_incomplete_terminal_before_any_headers_has_no_status_to_report() {
    let history = history_of("I");
    assert_eq!(
        history.status(),
        TransportStatus::Incomplete {
            status: None,
            cause: IncompleteCause::DeadlineElapsed,
            prefix: b"{\"plan\":".to_vec(),
        }
    );
}

#[test]
fn an_incomplete_terminal_after_a_prefix_keeps_the_status_and_the_exact_prefix() {
    let history = history_of("HPI");
    assert_eq!(
        history.status(),
        TransportStatus::Incomplete {
            status: Some(200),
            cause: IncompleteCause::DeadlineElapsed,
            prefix: b"{\"plan\":".to_vec(),
        }
    );
}

#[test]
fn an_observation_naming_another_invocation_is_refused() {
    let mut history = TransportHistory::admitted(&admission());
    let refusal = history
        .append_record(InvocationObservationV1 {
            invocation_id: uuid(OTHER_INVOCATION),
            sequence: 1,
            observation: headers(200),
        })
        .expect_err("another Invocation's observation");
    assert_eq!(
        refusal,
        AppendRefusal::ForeignInvocation {
            expected: uuid(INVOCATION),
            offered: uuid(OTHER_INVOCATION),
        }
    );
}

#[test]
fn a_history_reports_a_terminal_only_when_it_holds_one() {
    for word in ADMITTED_WORDS {
        let status = history_of(word).status();
        let terminal = matches!(
            status,
            TransportStatus::CompleteEmpty { .. }
                | TransportStatus::CompleteRetained { .. }
                | TransportStatus::Incomplete { .. }
        );
        assert_eq!(
            terminal,
            word.contains('C') || word.contains('I'),
            "{word} reported terminal={terminal}"
        );
    }
}

#[test]
fn no_admitted_word_holds_more_than_three_transport_rows() {
    for word in ADMITTED_WORDS {
        assert!(
            history_of(word).len() <= 3,
            "{word} holds more than three rows"
        );
    }
}

// Astra's fixed-head C1 review probes: behavior only, held fixture values.
#[test]
fn astra_n29_the_first_observation_sequence_is_one() {
    let mut history = TransportHistory::admitted(&admission());
    let result = history.append_record(InvocationObservationV1 {
        invocation_id: uuid(INVOCATION), sequence: 2, observation: headers(200),
    });
    assert!(result.is_err(), "N29.SEQUENCE.FIRST_MUST_BE_ONE: {result:?}");
}

#[test]
fn astra_n29_later_observation_sequence_must_increase() {
    let mut history = history_of("H");
    let result = history.append_record(InvocationObservationV1 {
        invocation_id: uuid(INVOCATION), sequence: 1,
        observation: TransportObservation::BodyPrefix { bytes: b"first".to_vec() },
    });
    assert!(result.is_err(), "N29.SEQUENCE.LATER_MUST_INCREASE: {result:?}");
}

#[test]
fn astra_n29_complete_bytes_extend_the_observed_prefix() {
    let mut history = history_of("HP");
    let result = history.append(TransportObservation::BodyComplete {
        bytes: b"a different response".to_vec(), elapsed_ms: 1,
    });
    assert!(result.is_err(), "N29.PREFIX.COMPLETE_MUST_EXTEND_OBSERVED_PREFIX: {result:?}");
}

#[test]
fn astra_n29_incomplete_bytes_preserve_the_observed_prefix() {
    let mut history = history_of("HP");
    let result = history.append(TransportObservation::BodyIncomplete {
        prefix: vec![], cause: IncompleteCause::BodyReadRefused,
    });
    assert!(result.is_err(), "N29.PREFIX.INCOMPLETE_MUST_PRESERVE_OBSERVED_PREFIX: {result:?}");
}
