//! The spend attribution state table (alpha bullet 39).
//!
//! Four dimensions, three states, two eras. Every cell in that table is a probe here, and the
//! table is closed twice over: `Dimension::ALL` with an exhaustive `match` closes the dimension
//! axis, and the destructure in [`state_of`] closes the struct -- a fifth dimension on
//! `LineAttribution` does not compile until it is given a row.
//!
//! The three states are three answers to different questions. "This line was produced by agent X"
//! (attributed), "nobody recorded who produced this line" (unattributed), and "this line has no
//! agent to record" (not applicable) are not degrees of one another, and a reader that rendered
//! any two of them the same way would be reporting a fact the record does not carry. The v1 era is
//! the case that makes the distinction load-bearing: a v1 record cannot express any of the three,
//! so it reads as exactly one -- unattributed, with the reason naming its era -- and never as
//! `not_applicable`, which would invent a fact, and never refused, which would call an honest
//! older record broken.

use beacond::{
    LineAttribution, MalformedSpend, MeasuredSpendV1, SPEND_V1_ERA_REASON, SpendAttribution,
    SpendLine, SpendMeasurement, SpendOrigin, SpendSupersedes, SpendTotals,
};
use uuid::Uuid;

const MAGIC_V1: &[u8] = b"beacon.spend.v1\0";
const MAGIC_V2: &[u8] = b"beacon.spend.v2\0";

const WORKSPACE: &str = "00000000-0000-4000-8000-000000000001";
const MISSION: &str = "00000000-0000-4000-8000-000000000002";
const SPEND: &str = "00000000-0000-4000-8000-000000000003";
const AGENT: &str = "00000000-0000-4000-8000-000000000004";

const MEASURED_AT_MS: u64 = 1_788_372_724_000;
const UNATTRIBUTED_REASON: &str = "the ledger row named no agent.";

const TAG_ATTRIBUTED: u8 = 0x01;
const TAG_UNATTRIBUTED: u8 = 0x02;
const TAG_NOT_APPLICABLE: u8 = 0x03;

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("canonical uuid")
}

// ---------------------------------------------------------------------------------------------
// The two axes
// ---------------------------------------------------------------------------------------------

/// One dimension a spend line can be attributed along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dimension {
    Agent,
    Route,
    Provider,
    Model,
}

impl Dimension {
    const ALL: [Self; 4] = [Self::Agent, Self::Route, Self::Provider, Self::Model];

    fn name(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Route => "route",
            Self::Provider => "provider",
            Self::Model => "model",
        }
    }
}

/// One state a dimension can be in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Attributed,
    Unattributed,
    NotApplicable,
}

impl State {
    const ALL: [Self; 3] = [Self::Attributed, Self::Unattributed, Self::NotApplicable];

    fn name(self) -> &'static str {
        match self {
            Self::Attributed => "attributed",
            Self::Unattributed => "unattributed",
            Self::NotApplicable => "not_applicable",
        }
    }
}

/// Reading one dimension out of a line.
///
/// The destructure is the guard: `LineAttribution` is taken apart by name with no `..`, so a fifth
/// dimension fails to compile here until this table grows a row for it.
fn state_of(attribution: &LineAttribution, dimension: Dimension) -> &'static str {
    let LineAttribution {
        agent,
        route,
        provider,
        model,
    } = attribution;
    match dimension {
        Dimension::Agent => agent.name(),
        Dimension::Provider => provider.name(),
        Dimension::Route => route.name(),
        Dimension::Model => model.name(),
    }
}

/// The cell: one dimension put into one state, the other three answered.
///
/// Every pair is written out. A new dimension or a new state fails to compile here rather than
/// silently taking some other cell's behaviour.
fn cell(dimension: Dimension, state: State) -> LineAttribution {
    let mut attribution = LineAttribution {
        agent: SpendAttribution::Attributed(uuid(AGENT)),
        route: SpendAttribution::Attributed("chat".to_owned()),
        provider: SpendAttribution::Attributed("local".to_owned()),
        model: SpendAttribution::Attributed("qwen3.8-27b".to_owned()),
    };
    let reason = || UNATTRIBUTED_REASON.to_owned();
    match (dimension, state) {
        (Dimension::Agent, State::Attributed) => {}
        (Dimension::Agent, State::Unattributed) => {
            attribution.agent = SpendAttribution::Unattributed(reason());
        }
        (Dimension::Agent, State::NotApplicable) => {
            attribution.agent = SpendAttribution::NotApplicable;
        }
        (Dimension::Route, State::Attributed) => {}
        (Dimension::Route, State::Unattributed) => {
            attribution.route = SpendAttribution::Unattributed(reason());
        }
        (Dimension::Route, State::NotApplicable) => {
            attribution.route = SpendAttribution::NotApplicable;
        }
        (Dimension::Provider, State::Attributed) => {}
        (Dimension::Provider, State::Unattributed) => {
            attribution.provider = SpendAttribution::Unattributed(reason());
        }
        (Dimension::Provider, State::NotApplicable) => {
            attribution.provider = SpendAttribution::NotApplicable;
        }
        (Dimension::Model, State::Attributed) => {}
        (Dimension::Model, State::Unattributed) => {
            attribution.model = SpendAttribution::Unattributed(reason());
        }
        (Dimension::Model, State::NotApplicable) => {
            attribution.model = SpendAttribution::NotApplicable;
        }
    }
    attribution
}

// ---------------------------------------------------------------------------------------------
// Records, and an era-v1 record assembled by hand
// ---------------------------------------------------------------------------------------------

fn record(attribution: LineAttribution) -> MeasuredSpendV1 {
    MeasuredSpendV1 {
        workspace_id: uuid(WORKSPACE),
        mission_id: uuid(MISSION),
        spend_id: uuid(SPEND),
        version: 2,
        origin: SpendOrigin::DaemonLedger,
        measured_at_ms: MEASURED_AT_MS,
        coverage_from_event_seq: 1,
        coverage_to_event_seq: 4,
        measurement: SpendMeasurement::Measured(SpendTotals {
            counted_units: 1,
            lines: vec![SpendLine {
                attribution,
                unit: "USD".to_owned(),
                amount_minor: 123_456,
                minor_units_exponent: 2,
            }],
        }),
        supersedes: SpendSupersedes::None,
    }
}

fn push_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u64).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

/// The header both eras share, up to and including the line count.
fn header(magic: &[u8]) -> Vec<u8> {
    let mut out = magic.to_vec();
    out.extend_from_slice(uuid(WORKSPACE).as_bytes());
    out.extend_from_slice(uuid(MISSION).as_bytes());
    out.extend_from_slice(uuid(SPEND).as_bytes());
    out.extend_from_slice(&2_u32.to_be_bytes());
    out.push(0x01); // origin = daemon_ledger
    out.extend_from_slice(&MEASURED_AT_MS.to_be_bytes());
    out.extend_from_slice(&1_u64.to_be_bytes());
    out.extend_from_slice(&4_u64.to_be_bytes());
    out.push(0x01); // measurement = measured
    out.extend_from_slice(&1_u64.to_be_bytes()); // counted_units
    out.extend_from_slice(&1_u32.to_be_bytes()); // one line
    out
}

fn line_body(out: &mut Vec<u8>) {
    push_string(out, "USD");
    out.extend_from_slice(&123_456_i64.to_be_bytes());
    out.push(2);
}

/// A record as the v1 era wrote them: no dimensions on the line at all.
fn v1_record_bytes() -> Vec<u8> {
    let mut out = header(MAGIC_V1);
    line_body(&mut out);
    out.push(0x00); // supersedes = none
    out
}

/// A v2 record whose four dimensions carry the given raw state tags, so a tag this build does not
/// know can be handed to the decoder.
fn v2_record_bytes_with_tags(tags: [u8; 4]) -> Vec<u8> {
    let mut out = header(MAGIC_V2);
    for (index, tag) in tags.into_iter().enumerate() {
        out.push(tag);
        match tag {
            TAG_ATTRIBUTED if index == 0 => out.extend_from_slice(uuid(AGENT).as_bytes()),
            TAG_ATTRIBUTED => push_string(&mut out, "local"),
            TAG_UNATTRIBUTED => push_string(&mut out, UNATTRIBUTED_REASON),
            // "Does not apply" carries no value: there is nothing to record.
            TAG_NOT_APPLICABLE => {}
            // Neither does a tag this build has never heard of -- which is exactly why an unknown
            // tag has to be refused rather than read as the state that happens to look like it.
            _ => {}
        }
    }
    line_body(&mut out);
    out.push(0x00); // supersedes = none
    out
}

fn decoded_attribution(bytes: &[u8]) -> LineAttribution {
    let record = MeasuredSpendV1::from_canonical_bytes(bytes).expect("decodes");
    let totals = record.measurement.totals().expect("measured").clone();
    totals
        .lines
        .into_iter()
        .next()
        .expect("one line")
        .attribution
}

// ---------------------------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------------------------

/// Era v2, every cell: the state a line was written with is the state it is read back as.
#[test]
fn in_the_v2_era_every_dimension_reads_back_in_the_state_it_was_written_in() {
    for dimension in Dimension::ALL {
        for state in State::ALL {
            let written = cell(dimension, state);
            let bytes = record(written.clone())
                .canonical_bytes()
                .unwrap_or_else(|error| {
                    panic!("{}/{} encodes: {error:?}", dimension.name(), state.name())
                });
            let read = decoded_attribution(&bytes);

            assert_eq!(
                state_of(&read, dimension),
                state.name(),
                "{} was written as {} and read back as {}",
                dimension.name(),
                state.name(),
                state_of(&read, dimension)
            );
            assert_eq!(
                read,
                written,
                "{}/{} did not survive the round trip whole",
                dimension.name(),
                state.name()
            );

            // The other three dimensions are untouched by this cell: a decoder that spread one
            // dimension's state across the line would pass the assertion above and fail here.
            for other in Dimension::ALL
                .into_iter()
                .filter(|other| *other != dimension)
            {
                assert_eq!(
                    state_of(&read, other),
                    State::Attributed.name(),
                    "setting {} to {} also moved {}",
                    dimension.name(),
                    state.name(),
                    other.name()
                );
            }
        }
    }
}

/// Era v1, every cell: the record is read, not refused, and every dimension reads as unattributed
/// with its era named. The state axis is degenerate here on purpose -- that is the fact being
/// pinned. A v1 record cannot express `attributed` or `not_applicable`, so it must never be read
/// as either.
#[test]
fn in_the_v1_era_every_dimension_reads_as_unattributed_and_the_record_is_never_refused() {
    let bytes = v1_record_bytes();
    let read = decoded_attribution(&bytes);

    for dimension in Dimension::ALL {
        assert_eq!(
            state_of(&read, dimension),
            State::Unattributed.name(),
            "a v1 record read {} as something its era could not record",
            dimension.name()
        );
    }

    // And the reason names the era rather than pretending someone looked and found nothing.
    let LineAttribution {
        agent,
        route,
        provider,
        model,
    } = &read;
    for (name, reason) in [
        ("agent", unattributed_reason_of_uuid(agent)),
        ("route", unattributed_reason_of_text(route)),
        ("provider", unattributed_reason_of_text(provider)),
        ("model", unattributed_reason_of_text(model)),
    ] {
        assert_eq!(
            reason, SPEND_V1_ERA_REASON,
            "{name}'s v1 reason does not name the era"
        );
    }
}

fn unattributed_reason_of_uuid(attribution: &SpendAttribution<Uuid>) -> &str {
    match attribution {
        SpendAttribution::Unattributed(reason) => reason,
        other => panic!("expected unattributed, got {}", other.name()),
    }
}

fn unattributed_reason_of_text(attribution: &SpendAttribution<String>) -> &str {
    match attribution {
        SpendAttribution::Unattributed(reason) => reason,
        other => panic!("expected unattributed, got {}", other.name()),
    }
}

/// The refusal row. A state tag this build does not know is refused by name -- never read as one
/// of the three states, and never dropped.
#[test]
fn a_state_tag_this_build_does_not_know_is_refused_rather_than_collapsed() {
    for (index, dimension) in Dimension::ALL.into_iter().enumerate() {
        let mut tags = [TAG_ATTRIBUTED; 4];
        tags[index] = 0x04;
        let error = MeasuredSpendV1::from_canonical_bytes(&v2_record_bytes_with_tags(tags))
            .expect_err("an unknown state tag is refused");
        assert!(
            matches!(error, MalformedSpend::UnknownTag),
            "{} refused an unknown state as {error:?} rather than as an unknown tag",
            dimension.name()
        );
    }
}

/// The three states are three names, and none of them is a spelling of "we do not know" that a
/// reader could read as zero, empty, or absent.
#[test]
fn the_three_states_stay_three_distinct_names_with_no_collapse_spelling() {
    let names: Vec<&str> = State::ALL.into_iter().map(State::name).collect();
    assert_eq!(names, ["attributed", "unattributed", "not_applicable"]);

    let mut distinct = names.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), 3, "two states share a name");

    for collapse in ["none", "null", "unknown", "n/a", "", "zero", "empty"] {
        assert!(
            !names.contains(&collapse),
            "a state is spelled {collapse:?}, which a reader would read as absence"
        );
    }

    // The names the record reports are the names the table uses.
    for state in State::ALL {
        for dimension in Dimension::ALL {
            assert_eq!(state_of(&cell(dimension, state), dimension), state.name());
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The line floor, measured
// ---------------------------------------------------------------------------------------------

fn minimal_line(unit: &str) -> SpendLine {
    SpendLine {
        attribution: LineAttribution {
            agent: SpendAttribution::NotApplicable,
            route: SpendAttribution::NotApplicable,
            provider: SpendAttribution::NotApplicable,
            model: SpendAttribution::NotApplicable,
        },
        unit: unit.to_owned(),
        amount_minor: 0,
        minor_units_exponent: 0,
    }
}

fn record_of(lines: Vec<SpendLine>) -> Vec<u8> {
    let mut record = record(LineAttribution::unattributed("unused"));
    record.measurement = SpendMeasurement::Measured(SpendTotals {
        counted_units: 1,
        lines,
    });
    record.canonical_bytes().expect("encodes")
}

/// What a v1 line costs at its smallest, by the same subtraction, on bytes this file assembles
/// rather than on anything the encoder can still produce.
fn v1_minimal_line_bytes(unit: &str) -> Vec<u8> {
    let mut out = Vec::new();
    push_string(&mut out, unit);
    out.extend_from_slice(&0_i64.to_be_bytes());
    out.push(0);
    out
}

/// The decoder proves a declared line count against the input it has left, and it needs a floor --
/// the smallest a line can be -- to do that. This measures that floor instead of restating the
/// constant the decoder uses, so a layout change that made lines cheaper shows up here as a number
/// that moved rather than as a guard that quietly stopped guarding.
#[test]
fn the_smallest_a_spend_line_can_be_is_measured_and_v2_costs_exactly_one_tag_per_dimension() {
    let one = record_of(vec![minimal_line("A")]).len();
    let two = record_of(vec![minimal_line("A"), minimal_line("B")]).len();
    let v2_floor = two - one;

    let v1_floor = v1_minimal_line_bytes("A").len();

    assert_eq!(
        v1_floor, 18,
        "an eight-byte length, one byte of unit, an i64 and an exponent"
    );
    assert_eq!(
        v2_floor,
        v1_floor + Dimension::ALL.len(),
        "a v2 line at its smallest costs one state tag per dimension and nothing else"
    );

    // And the floor is load-bearing: a count no remaining input could hold is refused before a
    // single line is read. The count stays under the decoder's cap on purpose -- a count over the
    // cap would be refused by the cap, which is a different guard.
    let mut hostile = record_of(vec![minimal_line("A")]);
    let first_line: Vec<u8> = [0x03; 4]
        .into_iter()
        .chain(1_u64.to_be_bytes())
        .chain(*b"A")
        .collect();
    let count_at = hostile
        .windows(first_line.len())
        .position(|window| window == first_line)
        .expect("the one line is present")
        - 4;
    assert_eq!(&hostile[count_at..count_at + 4], &1_u32.to_be_bytes());
    hostile[count_at..count_at + 4].copy_from_slice(&32_u32.to_be_bytes());
    assert_eq!(
        MeasuredSpendV1::from_canonical_bytes(&hostile).expect_err("refuses"),
        MalformedSpend::Truncated,
        "a declared count was trusted before it was measured against the input"
    );
}
