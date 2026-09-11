import {
  Button,
  Dropdown,
  Field,
  Input,
  Option,
  Textarea,
  makeStyles,
  tokens,
} from "@fluentui/react-components";
import { useEffect, useRef, useState } from "react";

import {
  type CompletionConditionInput,
  type CompletionIntentRequest,
  type CompletionNarrativeInput,
  type CompletionPreview,
  type MissionView,
  type SupportRefDto,
  type UncertaintyKindToken,
  type UnfinishedKindToken,
  describeLocalAccountError,
  getMissionCompletionIntent,
  previewMissionCompletion,
  recordMissionCompletion,
} from "./session";

const useStyles = makeStyles({
  section: { marginTop: tokens.spacingVerticalL },
  heading: { fontSize: tokens.fontSizeBase300, marginTop: 0, marginBottom: tokens.spacingVerticalXS },
  body: { fontSize: tokens.fontSizeBase200, marginTop: 0, marginBottom: tokens.spacingVerticalXS },
  row: { display: "flex", columnGap: tokens.spacingHorizontalS, flexWrap: "wrap", alignItems: "end" },
  block: { marginBottom: tokens.spacingVerticalS },
  list: { marginTop: 0, paddingLeft: tokens.spacingHorizontalL },
});

const OUTCOMES = [
  ["met", "Met"],
  ["not_met", "Not met"],
  ["not_assessed", "Not assessed"],
  ["assessment_inconclusive", "Assessment inconclusive"],
] as const;
const UNFINISHED: readonly (readonly [UnfinishedKindToken, string])[] = [
  ["condition_not_met", "Condition not met"],
  ["deferred_scope", "Deferred scope"],
  ["known_defect", "Known defect"],
  ["blocked_dependency", "Blocked dependency"],
];
const UNCERTAINTY: readonly (readonly [UncertaintyKindToken, string])[] = [
  ["unverified_claim", "Unverified claim"],
  ["unmeasured_quantity", "Unmeasured quantity"],
  ["conflicting_evidence", "Conflicting evidence"],
  ["outside_scope_of_checks", "Outside scope of checks"],
];

/** Every reference here was typed by the Operator. No picker exists for any class yet, and this
 *  screen opens none of them, so the label is the honest one. */
// The OWNER's bounds (`mission_completion.rs:124-133`), counted in Unicode SCALARS. A JS `.length`
// bound would accept half as much astral text as it should, and the owner would then refuse a
// request this screen called complete.
const SHORT_TEXT_SCALARS = 500;
const SUMMARY_SCALARS = 2_000;
const scalars = (text: string) => Array.from(text).length;

/** The owner's text grammar, `require_text` at `mission_completion.rs:489-506`: nonempty, trimmed,
 *  within the scalar bound, and no control character other than LF or TAB. An upper bound alone is
 *  not the grammar -- a blank or untrimmed summary is a request the owner refuses, so this screen
 *  must not call it complete. */
const isOwnerText = (text: string, maxScalars: number) =>
  text !== "" &&
  text.trim() === text &&
  scalars(text) <= maxScalars &&
  !Array.from(text).some(
    (character) => /\p{Cc}/u.test(character) && character !== "\n" && character !== "\t",
  );
const isShortText = (text: string) => isOwnerText(text, SHORT_TEXT_SCALARS);

/** Counts, versions and ordinals are lossless `u32` in the owner's codec. `2**32` is a safe JS
 *  integer and an out-of-domain completion value. */
const U32_MAX = 4_294_967_295;
const isU32 = (value: number) => Number.isSafeInteger(value) && value >= 0 && value <= U32_MAX;

/** One support reference written out in full: its class and every value it carries. A count is not
 *  a citation, and the Operator is confirming a permanent record. */
function supportText(ref: SupportRefDto): string {
  if ("artifact" in ref) return `Artifact ${ref.artifact}`;
  if ("evaluation" in ref) return `Evaluation ${ref.evaluation}`;
  if ("ruling" in ref) return `Ruling ${ref.ruling}`;
  return (
    `Receipt run ${ref.receipt.runId} version ${ref.receipt.receiptVersion}` +
    ` bytes ${ref.receipt.receiptBytesSha256}`
  );
}

/** Narrative relations are shown in the Operator's 1-based condition numbering, the same numbering
 *  the compose labels use. */
const relationText = (conditionOrdinal: number | null) =>
  conditionOrdinal === null ? "no condition named" : `condition ${conditionOrdinal + 1}`;

const labelFor = (
  table: readonly (readonly [string, string])[],
  kind: string,
) => table.find(([token]) => token === kind)?.[1] ?? kind;

const MANUAL_LABEL = "Reference entered manually; supporting content is not verified here.";

const CODE_COPY: Record<string, string> = {
  completion_intent_changed: "The reviewed details changed. Review the completion again.",
  completion_already_recorded:
    "A completion is already recorded for this Mission. Read the recorded completion.",
  completion_resident_not_in_admission_roster: "Choose a Resident admitted to this Mission.",
  completion_condition_coverage_incomplete: "Assess every declared condition explicitly.",
  completion_support_over_limit:
    "This condition cites more support than a completion can record.",
  completion_text_over_limit: "Some completion text exceeds the supported length.",
  completion_spend_reason_required: "Explain why spend was not measured.",
  // Grammar, not length: a malformed identity, digest, tag or integer, or the same entry twice in a
  // set. Telling the Operator to shorten text would send them to fix what was never wrong.
  completion_invalid_details: "Some completion details are invalid. Review the completion again.",
  // The SHARED unavailable code, with C3's exact sentence. One condition, one code, whichever
  // completion operation asked: the caller already knows which one it invoked.
  mission_completion_unavailable: "The Mission completion could not be verified.",
};

function describeCompletionWriteError(error: unknown): string {
  const code =
    typeof error === "object" && error !== null && !Array.isArray(error)
      ? (error as Record<string, unknown>).code
      : undefined;
  if (typeof code === "string" && code in CODE_COPY) return CODE_COPY[code];
  // Anything else keeps the meaning it already had: a session or authentication refusal relabelled
  // as a completion problem would be a false explanation.
  return describeLocalAccountError(error);
}

type SupportDraft = {
  kind: "artifact" | "receipt" | "evaluation" | "ruling" | "";
  digest: string;
  runId: string;
  receiptVersion: string;
  receiptBytesSha256: string;
};

type NarrativeDraft = { kind: string; conditionOrdinal: string; summary: string };

const EMPTY_SUPPORT: SupportDraft = {
  kind: "",
  digest: "",
  runId: "",
  receiptVersion: "",
  receiptBytesSha256: "",
};

function supportFromDraft(draft: SupportDraft): SupportRefDto | null {
  if (draft.kind === "artifact") return { artifact: draft.digest };
  if (draft.kind === "evaluation") return { evaluation: draft.digest };
  if (draft.kind === "ruling") return { ruling: draft.digest };
  if (draft.kind === "receipt") {
    const version = Number(draft.receiptVersion);
    if (!isU32(version)) return null;
    return {
      receipt: {
        runId: draft.runId,
        receiptVersion: version,
        receiptBytesSha256: draft.receiptBytesSha256,
      },
    };
  }
  return null;
}

function narrativeFromDrafts<K extends string>(drafts: NarrativeDraft[]): CompletionNarrativeInput<K>[] | null {
  const items: CompletionNarrativeInput<K>[] = [];
  for (const [index, draft] of drafts.entries()) {
    if (draft.kind === "") return null;
    // The Operator types the condition NUMBER they can see on this screen ("condition 2"), and the
    // owner stores the ordinal (1). Review renders the number again, so all three agree; a raw
    // 0-based box under a 1-based screen would silently cite the neighbouring condition.
    const relation =
      draft.conditionOrdinal === "" ? null : Number(draft.conditionOrdinal) - 1;
    if (relation !== null && !isU32(relation)) return null;
    if (!isShortText(draft.summary)) return null;
    // Dense from zero: the ordinal is the item's position, never a number the Operator picks.
    items.push({ ordinal: index, kind: draft.kind as K, conditionOrdinal: relation, summary: draft.summary });
  }
  return items;
}

/** A single asynchronous leg: which attempt it is, and which draft it was derived from. */
type Leg = { attempt: number; draft: number };

export function MissionCompletionCompose({
  mission,
  onRecorded,
}: {
  mission: MissionView;
  /** An explicit local signal: the C3 read re-runs because this told it to, never on a timer. */
  onRecorded: () => void;
}) {
  const classes = useStyles();
  // NOTHING starts selected. A pre-chosen Resident, disposition, outcome or spend state would be
  // this screen deciding something only the Operator can.
  const [resident, setResident] = useState("");
  const [disposition, setDisposition] = useState<"" | "concluded" | "abandoned">("");
  const [outcomes, setOutcomes] = useState<string[]>(() => mission.successConditions.map(() => ""));
  const [notes, setNotes] = useState<string[]>(() => mission.successConditions.map(() => ""));
  const [support, setSupport] = useState<SupportDraft[][]>(() =>
    mission.successConditions.map(() => []),
  );
  const [artifacts, setArtifacts] = useState("");
  const [unfinished, setUnfinished] = useState<NarrativeDraft[]>([]);
  const [uncertainty, setUncertainty] = useState<NarrativeDraft[]>([]);
  const [spendKind, setSpendKind] = useState<"" | "recorded" | "notMeasured">("");
  const [spendId, setSpendId] = useState("");
  const [spendVersion, setSpendVersion] = useState("");
  const [spendSha, setSpendSha] = useState("");
  const [spendReason, setSpendReason] = useState("");
  const [summary, setSummary] = useState("");

  const [preview, setPreview] = useState<CompletionPreview | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const [pending, setPending] = useState<"" | "preview" | "record" | "recovery">("");
  const [failure, setFailure] = useState("");
  const [outcome, setOutcome] = useState<
    "" | "recorded" | "conflicting" | "not_visible" | "unavailable"
  >("");
  // The reviewed token of the write that was actually submitted. Retained so an unavailable read can
  // be retried as a READ of that same submission; a further write is only ever an explicit Operator
  // action on the preserved review, never this screen's idea.
  const [submitted, setSubmitted] = useState("");
  /** The control the review was opened from, so cancelling returns the keyboard where it was. */
  const reviewTrigger = useRef<HTMLButtonElement | null>(null);
  const returnFocus = useRef(false);
  // Lifetime counters, read at CALLBACK time through a ref -- never compared against a value
  // captured in the same render, which would move with them and could never disagree.
  //
  // TWO counters because an edit and a Mission change retire DIFFERENT things. `attempt` retires a
  // leg outright: a newer leg owns the screen, or this is no longer that Mission, so the answer is
  // written nowhere. `draft` retires only a PREVIEW's content: a preview describes the draft it was
  // derived from, and after an edit that draft is gone. A record's outcome is never retired by an
  // edit -- ignoring a late result does not cancel a native write, so the write is still owed a
  // truthful answer even if the Operator typed while it was in flight.
  const live = useRef({ missionId: mission.id, attempt: 0, draft: 0 });

  /** Start an asynchronous leg. Everything older is now stale. */
  function fresh(): Leg {
    live.current.attempt += 1;
    return { attempt: live.current.attempt, draft: live.current.draft };
  }

  /** A newer leg started, or this is no longer that Mission: write nothing at all. */
  function stale(mine: Leg): boolean {
    return live.current.attempt !== mine.attempt;
  }

  /** The draft moved on, so a preview of it describes something that no longer exists. */
  function describesAGoneDraft(mine: Leg): boolean {
    return live.current.draft !== mine.draft;
  }

  function resetDraft() {
    setResident("");
    setDisposition("");
    setOutcomes(mission.successConditions.map(() => ""));
    setNotes(mission.successConditions.map(() => ""));
    setSupport(mission.successConditions.map(() => []));
    setArtifacts("");
    setUnfinished([]);
    setUncertainty([]);
    setSpendKind("");
    setSpendId("");
    setSpendVersion("");
    setSpendSha("");
    setSpendReason("");
    setSummary("");
    setPreview(null);
    setConfirmed(false);
    setPending("");
    setFailure("");
    setOutcome("");
  }

  // Cancelling the review removes the control that had focus. Focus is returned to the control the
  // review was opened from once it is back on screen; leaving it on <body> loses a keyboard user's
  // place entirely.
  useEffect(() => {
    if (preview !== null || !returnFocus.current) return;
    returnFocus.current = false;
    reviewTrigger.current?.focus();
  }, [preview]);

  // Unmount retires the current attempt, by the same rule as a Mission change.
  //
  // `setState` after unmount is a silent no-op, but two effects of a late answer are not state:
  // `onRecorded` is the PARENT's callback and a read-back is a NEW request. Retiring the attempt is
  // what stops both from escaping a closed composer. It does not cancel a write already dispatched,
  // and claims nothing about one. (A StrictMode remount runs this cleanup once at mount, with no leg
  // in flight, so that bump retires nothing.)
  useEffect(() => {
    const lifetime = live;
    return () => {
      lifetime.current.attempt += 1;
    };
  }, []);

  // A different Mission inherits NOTHING: not an assessment, not a Resident, not a preview. The
  // draft is retained only while the Mission is the same one.
  useEffect(() => {
    if (live.current.missionId === mission.id) return;
    live.current.missionId = mission.id;
    live.current.attempt += 1;
    resetDraft();
  }, [mission.id]);

  /** Any edit discards the preview AND retires any leg still running: what was reviewed is no longer
   *  what would be sent, and a preview that arrives after the edit describes a draft that is gone. */
  function edited<T>(set: (value: T) => void) {
    return (value: T) => {
      live.current.draft += 1;
      setPreview(null);
      setConfirmed(false);
      setOutcome("");
      set(value);
    };
  }

  function request(): CompletionIntentRequest | null {
    if (resident === "" || disposition === "" || spendKind === "") return null;
    const conditionResults: CompletionConditionInput[] = [];
    for (const [ordinal] of mission.successConditions.entries()) {
      const chosen = outcomes[ordinal];
      if (chosen === "") return null;
      const refs: SupportRefDto[] = [];
      for (const draft of support[ordinal] ?? []) {
        const ref = supportFromDraft(draft);
        if (ref === null) return null;
        refs.push(ref);
      }
      if (notes[ordinal] !== "" && !isShortText(notes[ordinal])) return null;
      conditionResults.push({
        ordinal,
        outcome: chosen as CompletionConditionInput["outcome"],
        support: refs,
        note: notes[ordinal] === "" ? null : notes[ordinal],
      });
    }
    const unfinishedItems = narrativeFromDrafts<UnfinishedKindToken>(unfinished);
    const uncertaintyItems = narrativeFromDrafts<UncertaintyKindToken>(uncertainty);
    if (unfinishedItems === null || uncertaintyItems === null) return null;
    const version = Number(spendVersion);
    const spend =
      spendKind === "recorded"
        ? isU32(version)
          ? { recorded: { spendId, spendVersion: version, spendSha256: spendSha } }
          : null
        : { notMeasured: spendReason };
    if (spend === null) return null;
    if (spendKind === "notMeasured" && !isShortText(spendReason)) return null;
    if (unfinishedItems.some((item) => !isShortText(item.summary))) return null;
    if (uncertaintyItems.some((item) => !isShortText(item.summary))) return null;
    if (!isOwnerText(summary, SUMMARY_SCALARS)) return null;
    return {
      missionId: mission.id,
      accountableResidentId: resident,
      disposition,
      conditionResults,
      deliveredArtifacts: artifacts
        .split(/\s+/)
        .map((entry) => entry.trim())
        .filter((entry) => entry !== ""),
      unfinishedWork: unfinishedItems,
      uncertainty: uncertaintyItems,
      spend,
      summary,
    };
  }

  const ready = request() !== null;

  function runPreview() {
    const value = request();
    if (value === null) return;
    const mine = fresh();
    setPending("preview");
    setFailure("");
    previewMissionCompletion(value).then(
      (loaded) => {
        if (stale(mine)) return;
        setPending("");
        if (describesAGoneDraft(mine)) return;
        if (loaded.request.missionId !== mission.id) {
          setFailure("The preview describes a different Mission.");
          return;
        }
        // A structurally valid preview is still the wrong record if it is bound somewhere else. The
        // Operator is about to confirm a permanent record: Workspace, Project and the historical
        // admitted configuration are part of what is being confirmed, not decoration.
        if (loaded.workspaceId !== mission.workspaceId) {
          setFailure("The preview describes a different Workspace.");
          return;
        }
        if (loaded.projectId !== mission.projectId) {
          setFailure("The preview describes a different Project.");
          return;
        }
        const admitted = mission.members.find(
          (member) => member.residentId === value.accountableResidentId,
        );
        if (
          admitted === undefined ||
          loaded.accountableResidentConfigurationId !== admitted.admissionConfigurationId
        ) {
          setFailure("The preview names a different admitted configuration for that Resident.");
          return;
        }
        if (
          loaded.conditions.length !== mission.successConditions.length ||
          loaded.conditions.some((text, index) => text !== mission.successConditions[index])
        ) {
          setFailure("The Mission's declared conditions changed. Review the completion again.");
          return;
        }
        if (loaded.request.accountableResidentId !== value.accountableResidentId) {
          setFailure("The preview names a different accountable Resident.");
          return;
        }
        setPreview(loaded);
      },
      (error: unknown) => {
        if (stale(mine)) return;
        setPending("");
        if (describesAGoneDraft(mine)) return;
        setFailure(describeCompletionWriteError(error));
      },
    );
  }

  function readBack(reviewed: string, mine: Leg) {
    setPending("recovery");
    getMissionCompletionIntent(mission.id).then(
      (token) => {
        if (stale(mine)) return;
        setPending("");
        if (token === null) setOutcome("not_visible");
        else if (token.intentSha256 === reviewed) {
          setOutcome("recorded");
          onRecorded();
        } else setOutcome("conflicting");
      },
      (error: unknown) => {
        if (stale(mine)) return;
        setPending("");
        setOutcome("unavailable");
        setFailure(describeCompletionWriteError(error));
      },
    );
  }

  /** A second READ of the submission that was already made. It never writes: the submitted review
   *  is preserved, and recording again stays an explicit Operator action on it. */
  function retryRead() {
    if (submitted === "" || pending !== "") return;
    const mine = fresh();
    setOutcome("");
    setFailure("");
    readBack(submitted, mine);
  }

  function runRecord() {
    if (preview === null || !confirmed) return;
    const mine = fresh();
    const reviewed = preview.intentSha256;
    setSubmitted(reviewed);
    setPending("record");
    setFailure("");
    // The FROZEN reviewed request, never a re-read of the live form.
    recordMissionCompletion(preview.request, reviewed).then(
      (ack) => {
        if (stale(mine)) return;
        if (ack.intentSha256 === reviewed) {
          setPending("");
          setOutcome("recorded");
          onRecorded();
          return;
        }
        // A differing acknowledgement is not accepted as success just because it arrived.
        readBack(reviewed, mine);
      },
      (error: unknown) => {
        // Ignoring a late result does NOT cancel a native write; the read-back is what establishes
        // what happened, and it is still owed even when this leg is the current one.
        if (stale(mine)) return;
        setFailure(describeCompletionWriteError(error));
        // The form and the frozen reviewed token are preserved; the READ decides what happened.
        readBack(reviewed, mine);
      },
    );
  }

  return (
    <section className={classes.section} aria-label="Record Mission completion">
      <h4 className={classes.heading}>Record completion</h4>
      <p className={classes.body}>{MANUAL_LABEL}</p>

      <Field label="Accountable Resident" className={classes.block}>
        <Dropdown
          aria-label="Accountable Resident"
          placeholder="Choose an admitted Resident"
          selectedOptions={resident === "" ? [] : [resident]}
          onOptionSelect={(_e, data) => edited(setResident)(data.optionValue ?? "")}
        >
          {mission.members.map((member) => (
            <Option key={member.residentId} value={member.residentId}>
              {member.residentId}
            </Option>
          ))}
        </Dropdown>
      </Field>

      <Field label="Disposition" className={classes.block}>
        <Dropdown
          aria-label="Disposition"
          placeholder="Choose how the Mission finished"
          selectedOptions={disposition === "" ? [] : [disposition]}
          onOptionSelect={(_e, data) =>
            edited(setDisposition)((data.optionValue ?? "") as "" | "concluded" | "abandoned")
          }
        >
          <Option value="concluded">Concluded</Option>
          <Option value="abandoned">Abandoned</Option>
        </Dropdown>
      </Field>

      {mission.successConditions.map((condition, ordinal) => (
        <div key={`${condition}-${ordinal}`} className={classes.block}>
          <p className={classes.body}>{condition}</p>
          <div className={classes.row}>
            <Field label={`Outcome for condition ${ordinal + 1}`}>
              <Dropdown
                aria-label={`Outcome for condition ${ordinal + 1}`}
                placeholder="Assess this condition"
                selectedOptions={outcomes[ordinal] === "" ? [] : [outcomes[ordinal]]}
                onOptionSelect={(_e, data) =>
                  edited((next: string[]) => setOutcomes(next))(
                    outcomes.map((value, index) => (index === ordinal ? data.optionValue ?? "" : value)),
                  )
                }
              >
                {OUTCOMES.map(([value, label]) => (
                  <Option key={value} value={value}>
                    {label}
                  </Option>
                ))}
              </Dropdown>
            </Field>
            <Field label={`Note for condition ${ordinal + 1}`}>
              <Input
                aria-label={`Note for condition ${ordinal + 1}`}
                value={notes[ordinal]}
                onChange={(_e, data) =>
                  edited((next: string[]) => setNotes(next))(
                    notes.map((value, index) => (index === ordinal ? data.value : value)),
                  )
                }
              />
            </Field>
            <Button
              size="small"
              onClick={() =>
                edited((next: SupportDraft[][]) => setSupport(next))(
                  support.map((refs, index) =>
                    index === ordinal ? [...refs, { ...EMPTY_SUPPORT }] : refs,
                  ),
                )
              }
            >
              Add support to condition {ordinal + 1}
            </Button>
          </div>
          {(support[ordinal] ?? []).map((draft, refIndex) => (
            <div key={refIndex} className={classes.row}>
              <Field label="Reference kind">
                <Dropdown
                  aria-label={`Reference kind ${ordinal + 1}.${refIndex + 1}`}
                  placeholder="Choose a reference kind"
                  selectedOptions={draft.kind === "" ? [] : [draft.kind]}
                  onOptionSelect={(_e, data) =>
                    edited((next: SupportDraft[][]) => setSupport(next))(
                      support.map((refs, index) =>
                        index === ordinal
                          ? refs.map((value, i) =>
                              i === refIndex
                                ? { ...value, kind: (data.optionValue ?? "") as SupportDraft["kind"] }
                                : value,
                            )
                          : refs,
                      ),
                    )
                  }
                >
                  <Option value="artifact">Artifact</Option>
                  <Option value="receipt">Receipt</Option>
                  <Option value="evaluation">Evaluation</Option>
                  <Option value="ruling">Ruling</Option>
                </Dropdown>
              </Field>
              {draft.kind === "receipt" ? (
                <>
                  <Field label="Run id">
                    <Input
                      aria-label={`Run id ${ordinal + 1}.${refIndex + 1}`}
                      value={draft.runId}
                      onChange={(_e, data) =>
                        edited((next: SupportDraft[][]) => setSupport(next))(
                          support.map((refs, index) =>
                            index === ordinal
                              ? refs.map((v, i) => (i === refIndex ? { ...v, runId: data.value } : v))
                              : refs,
                          ),
                        )
                      }
                    />
                  </Field>
                  <Field label="Receipt version">
                    <Input
                      aria-label={`Receipt version ${ordinal + 1}.${refIndex + 1}`}
                      value={draft.receiptVersion}
                      onChange={(_e, data) =>
                        edited((next: SupportDraft[][]) => setSupport(next))(
                          support.map((refs, index) =>
                            index === ordinal
                              ? refs.map((v, i) =>
                                  i === refIndex ? { ...v, receiptVersion: data.value } : v,
                                )
                              : refs,
                          ),
                        )
                      }
                    />
                  </Field>
                  <Field label="Receipt bytes digest">
                    <Input
                      aria-label={`Receipt bytes digest ${ordinal + 1}.${refIndex + 1}`}
                      value={draft.receiptBytesSha256}
                      onChange={(_e, data) =>
                        edited((next: SupportDraft[][]) => setSupport(next))(
                          support.map((refs, index) =>
                            index === ordinal
                              ? refs.map((v, i) =>
                                  i === refIndex ? { ...v, receiptBytesSha256: data.value } : v,
                                )
                              : refs,
                          ),
                        )
                      }
                    />
                  </Field>
                </>
              ) : (
                <Field label="Record digest">
                  <Input
                    aria-label={`Record digest ${ordinal + 1}.${refIndex + 1}`}
                    value={draft.digest}
                    onChange={(_e, data) =>
                      edited((next: SupportDraft[][]) => setSupport(next))(
                        support.map((refs, index) =>
                          index === ordinal
                            ? refs.map((v, i) => (i === refIndex ? { ...v, digest: data.value } : v))
                            : refs,
                        ),
                      )
                    }
                  />
                </Field>
              )}
            </div>
          ))}
        </div>
      ))}

      <Field label="Delivered Artifact digests" hint="One per line" className={classes.block}>
        <Textarea
          aria-label="Delivered Artifact digests"
          value={artifacts}
          onChange={(_e, data) => edited(setArtifacts)(data.value)}
        />
      </Field>

      {(
        [
          ["Unfinished work", unfinished, setUnfinished, UNFINISHED] as const,
          ["Uncertainty", uncertainty, setUncertainty, UNCERTAINTY] as const,
        ] as const
      ).map(([label, drafts, set, kinds]) => (
        <div key={label} className={classes.block}>
          <Button
            size="small"
            onClick={() =>
              edited((next: NarrativeDraft[]) => set(next))([
                ...drafts,
                { kind: "", conditionOrdinal: "", summary: "" },
              ])
            }
          >
            Add {label.toLowerCase()} item
          </Button>
          {drafts.map((draft, index) => (
            <div key={index} className={classes.row}>
              <Field label={`${label} kind ${index + 1}`}>
                <Dropdown
                  aria-label={`${label} kind ${index + 1}`}
                  placeholder="Choose a kind"
                  selectedOptions={draft.kind === "" ? [] : [draft.kind]}
                  onOptionSelect={(_e, data) =>
                    edited((next: NarrativeDraft[]) => set(next))(
                      drafts.map((value, i) =>
                        i === index ? { ...value, kind: data.optionValue ?? "" } : value,
                      ),
                    )
                  }
                >
                  {kinds.map(([value, text]) => (
                    <Option key={value} value={value}>
                      {text}
                    </Option>
                  ))}
                </Dropdown>
              </Field>
              <Field
                label={`${label} condition ${index + 1}`}
                hint="Condition number as numbered above; blank for none"
              >
                <Input
                  aria-label={`${label} condition ${index + 1}`}
                  value={draft.conditionOrdinal}
                  onChange={(_e, data) =>
                    edited((next: NarrativeDraft[]) => set(next))(
                      drafts.map((value, i) =>
                        i === index ? { ...value, conditionOrdinal: data.value } : value,
                      ),
                    )
                  }
                />
              </Field>
              <Field label={`${label} summary ${index + 1}`}>
                <Input
                  aria-label={`${label} summary ${index + 1}`}
                  value={draft.summary}
                  onChange={(_e, data) =>
                    edited((next: NarrativeDraft[]) => set(next))(
                      drafts.map((value, i) =>
                        i === index ? { ...value, summary: data.value } : value,
                      ),
                    )
                  }
                />
              </Field>
            </div>
          ))}
        </div>
      ))}

      <Field label="Spend" className={classes.block}>
        <Dropdown
          aria-label="Spend"
          placeholder="Choose how spend is reported"
          selectedOptions={spendKind === "" ? [] : [spendKind]}
          onOptionSelect={(_e, data) =>
            edited(setSpendKind)((data.optionValue ?? "") as "" | "recorded" | "notMeasured")
          }
        >
          <Option value="recorded">A measured Spend record</Option>
          <Option value="notMeasured">Not measured</Option>
        </Dropdown>
      </Field>
      {spendKind === "recorded" && (
        <div className={classes.row}>
          <Field label="Spend id">
            <Input aria-label="Spend id" value={spendId} onChange={(_e, d) => edited(setSpendId)(d.value)} />
          </Field>
          <Field label="Spend version">
            <Input
              aria-label="Spend version"
              value={spendVersion}
              onChange={(_e, d) => edited(setSpendVersion)(d.value)}
            />
          </Field>
          <Field label="Spend digest">
            <Input aria-label="Spend digest" value={spendSha} onChange={(_e, d) => edited(setSpendSha)(d.value)} />
          </Field>
        </div>
      )}
      {spendKind === "notMeasured" && (
        <Field label="Why spend was not measured" className={classes.block}>
          <Input
            aria-label="Why spend was not measured"
            value={spendReason}
            onChange={(_e, d) => edited(setSpendReason)(d.value)}
          />
        </Field>
      )}

      <Field label="Completion summary" className={classes.block}>
        <Textarea
          aria-label="Completion summary"
          value={summary}
          onChange={(_e, d) => edited(setSummary)(d.value)}
        />
      </Field>

      {failure !== "" && (
        <p role="alert" className={classes.body}>
          {failure}
        </p>
      )}

      {pending === "preview" && (
        <p role="status" className={classes.body}>
          Deriving the completion for review
        </p>
      )}
      {pending === "record" && (
        <p role="status" className={classes.body}>
          Recording the completion
        </p>
      )}
      {pending === "recovery" && (
        <p role="status" className={classes.body}>
          Reading back what was recorded
        </p>
      )}

      {preview === null ? (
        <Button
          appearance="primary"
          disabled={!ready || pending !== ""}
          onClick={runPreview}
          ref={reviewTrigger}
        >
          Review completion
        </Button>
      ) : (
        <div aria-label="Completion review">
          <h4 className={classes.heading}>Review — this is the whole record</h4>
          <p className={classes.body}>
            Beacon assigns the completion identity, the recorded time and the boundary when it
            records. A completion cannot currently be replaced once recorded.
          </p>
          {/* Every value the owner normalized, written out. A count is not a citation and a digest
              the Operator never saw is not a reviewed binding, so the whole record is here as
              readable detail -- never as raw JSON, which is not a review. */}
          <p className={classes.body}>
            Workspace {preview.workspaceId} · Project {preview.projectId} · Mission{" "}
            {preview.request.missionId}
          </p>
          <p className={classes.body}>
            Authored by the authenticated {preview.actor} · declared conditions digest{" "}
            {preview.declaredConditionsSha256}
          </p>
          <p className={classes.body}>
            Accountable Resident {preview.request.accountableResidentId} · admitted configuration{" "}
            {preview.accountableResidentConfigurationId} ·{" "}
            {preview.request.disposition === "concluded" ? "Concluded" : "Abandoned"}
          </p>
          <ol className={classes.list} aria-label="Reviewed condition results">
            {preview.request.conditionResults.map((result) => (
              <li key={result.ordinal} className={classes.body}>
                Condition {result.ordinal + 1}: {preview.conditions[result.ordinal]} —{" "}
                {labelFor(OUTCOMES, result.outcome)}
                {result.note === null ? " · no note" : ` · note: ${result.note}`}
                {result.support.length === 0 ? (
                  " · none cited"
                ) : (
                  <ul className={classes.list} aria-label={`Support cited for condition ${result.ordinal + 1}`}>
                    {result.support.map((ref) => (
                      <li key={supportText(ref)} className={classes.body}>
                        {supportText(ref)}
                      </li>
                    ))}
                  </ul>
                )}
              </li>
            ))}
          </ol>
          {preview.request.deliveredArtifacts.length === 0 ? (
            <p className={classes.body}>No delivered Artifacts cited</p>
          ) : (
            <ul className={classes.list} aria-label="Reviewed delivered Artifacts">
              {preview.request.deliveredArtifacts.map((digest) => (
                <li key={digest} className={classes.body}>
                  Delivered Artifact {digest}
                </li>
              ))}
            </ul>
          )}
          {preview.request.unfinishedWork.length === 0 ? (
            <p className={classes.body}>No unfinished work recorded</p>
          ) : (
            <ul className={classes.list} aria-label="Reviewed unfinished work">
              {preview.request.unfinishedWork.map((item) => (
                <li key={item.ordinal} className={classes.body}>
                  {labelFor(UNFINISHED, item.kind)} · {relationText(item.conditionOrdinal)} ·{" "}
                  {item.summary}
                </li>
              ))}
            </ul>
          )}
          {preview.request.uncertainty.length === 0 ? (
            <p className={classes.body}>No uncertainty recorded</p>
          ) : (
            <ul className={classes.list} aria-label="Reviewed uncertainty">
              {preview.request.uncertainty.map((item) => (
                <li key={item.ordinal} className={classes.body}>
                  {labelFor(UNCERTAINTY, item.kind)} · {relationText(item.conditionOrdinal)} ·{" "}
                  {item.summary}
                </li>
              ))}
            </ul>
          )}
          <p className={classes.body}>
            {"notMeasured" in preview.request.spend
              ? `Spend not measured: ${preview.request.spend.notMeasured}`
              : `Spend record ${preview.request.spend.recorded.spendId} version ` +
                `${preview.request.spend.recorded.spendVersion} bytes ` +
                `${preview.request.spend.recorded.spendSha256}`}
          </p>
          <p className={classes.body}>Summary: {preview.request.summary}</p>
          <div className={classes.row}>
            <Button
              appearance={confirmed ? "primary" : "secondary"}
              onClick={() => setConfirmed(!confirmed)}
            >
              {confirmed
                ? "Permanence confirmed"
                : "Confirm this record cannot be replaced"}
            </Button>
            <Button
              appearance="primary"
              disabled={!confirmed || pending !== ""}
              onClick={runRecord}
            >
              Record completion
            </Button>
            <Button
              onClick={() => {
                returnFocus.current = true;
                setPreview(null);
                setConfirmed(false);
              }}
            >
              Cancel review
            </Button>
          </div>
        </div>
      )}

      {outcome === "recorded" && (
        <p role="status" className={classes.body}>
          A recorded completion matches the reviewed details.
        </p>
      )}
      {outcome === "conflicting" && (
        <p role="alert" className={classes.body}>
          A different completion is already recorded for this Mission. It was not replaced.
        </p>
      )}
      {outcome === "not_visible" && (
        <p role="alert" className={classes.body}>
          No completion is visible yet. The reviewed details are preserved; recording again is your
          choice.
        </p>
      )}
      {outcome === "unavailable" && (
        <>
          <p role="alert" className={classes.body}>
            The recorded completion could not be read. Retry the read.
          </p>
          <Button disabled={submitted === "" || pending !== ""} onClick={retryRead}>
            Retry the read
          </Button>
        </>
      )}
    </section>
  );
}
