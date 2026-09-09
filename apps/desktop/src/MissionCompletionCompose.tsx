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
import { useMemo, useState } from "react";

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
const withinShort = (text: string) => scalars(text) <= SHORT_TEXT_SCALARS;

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
  completion_unavailable: "The Mission completion could not be verified.",
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
    if (!Number.isSafeInteger(version) || version < 0) return null;
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
    const relation =
      draft.conditionOrdinal === "" ? null : Number(draft.conditionOrdinal);
    if (relation !== null && !Number.isSafeInteger(relation)) return null;
    // Dense from zero: the ordinal is the item's position, never a number the Operator picks.
    items.push({ ordinal: index, kind: draft.kind as K, conditionOrdinal: relation, summary: draft.summary });
  }
  return items;
}

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
  // Every asynchronous leg is keyed to this. A leg from an earlier attempt or an earlier Mission
  // finds a different value and writes nothing.
  const attempt = useMemo(() => ({ current: 0 }), []);

  /** Any edit discards the preview: what was reviewed is no longer what would be sent. */
  function edited<T>(set: (value: T) => void) {
    return (value: T) => {
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
      if (!withinShort(notes[ordinal])) return null;
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
        ? Number.isSafeInteger(version) && version >= 0
          ? { recorded: { spendId, spendVersion: version, spendSha256: spendSha } }
          : null
        : { notMeasured: spendReason };
    if (spend === null) return null;
    if (spendKind === "notMeasured" && !withinShort(spendReason)) return null;
    if (unfinishedItems.some((item) => !withinShort(item.summary))) return null;
    if (uncertaintyItems.some((item) => !withinShort(item.summary))) return null;
    if (scalars(summary) > SUMMARY_SCALARS) return null;
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
    attempt.current += 1;
    const mine = attempt.current;
    const missionId = mission.id;
    setPending("preview");
    setFailure("");
    previewMissionCompletion(value).then(
      (loaded) => {
        if (attempt.current !== mine || mission.id !== missionId) return;
        setPending("");
        if (loaded.request.missionId !== mission.id) {
          setFailure("The preview describes a different Mission.");
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
        if (attempt.current !== mine || mission.id !== missionId) return;
        setPending("");
        setFailure(describeCompletionWriteError(error));
      },
    );
  }

  function readBack(reviewed: string, mine: number, missionId: string) {
    setPending("recovery");
    getMissionCompletionIntent(mission.id).then(
      (token) => {
        if (attempt.current !== mine || mission.id !== missionId) return;
        setPending("");
        if (token === null) setOutcome("not_visible");
        else if (token.intentSha256 === reviewed) {
          setOutcome("recorded");
          onRecorded();
        } else setOutcome("conflicting");
      },
      (error: unknown) => {
        if (attempt.current !== mine || mission.id !== missionId) return;
        setPending("");
        setOutcome("unavailable");
        setFailure(describeCompletionWriteError(error));
      },
    );
  }

  function runRecord() {
    if (preview === null || !confirmed) return;
    attempt.current += 1;
    const mine = attempt.current;
    const missionId = mission.id;
    const reviewed = preview.intentSha256;
    setPending("record");
    setFailure("");
    // The FROZEN reviewed request, never a re-read of the live form.
    recordMissionCompletion(preview.request, reviewed).then(
      (ack) => {
        if (attempt.current !== mine || mission.id !== missionId) return;
        if (ack.intentSha256 === reviewed) {
          setPending("");
          setOutcome("recorded");
          onRecorded();
          return;
        }
        // A differing acknowledgement is not accepted as success just because it arrived.
        readBack(reviewed, mine, missionId);
      },
      (error: unknown) => {
        if (attempt.current !== mine || mission.id !== missionId) return;
        setFailure(describeCompletionWriteError(error));
        // The form and the frozen reviewed token are preserved; the READ decides what happened.
        readBack(reviewed, mine, missionId);
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
              <Field label={`${label} condition ${index + 1}`} hint="Blank for none">
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
        <Button appearance="primary" disabled={!ready || pending !== ""} onClick={runPreview}>
          Review completion
        </Button>
      ) : (
        <div aria-label="Completion review">
          <h4 className={classes.heading}>Review — this is the whole record</h4>
          <p className={classes.body}>
            Beacon assigns the completion identity, the recorded time and the boundary when it
            records. A completion cannot currently be replaced once recorded.
          </p>
          <p className={classes.body}>
            Accountable Resident {preview.request.accountableResidentId} · admitted configuration{" "}
            {preview.accountableResidentConfigurationId} ·{" "}
            {preview.request.disposition === "concluded" ? "Concluded" : "Abandoned"}
          </p>
          <ol className={classes.list} aria-label="Reviewed condition results">
            {preview.request.conditionResults.map((result) => (
              <li key={result.ordinal} className={classes.body}>
                {preview.conditions[result.ordinal]} —{" "}
                {OUTCOMES.find(([value]) => value === result.outcome)?.[1]} ·{" "}
                {result.support.length === 0
                  ? "none cited"
                  : `${result.support.length} cited`}
                {result.note !== null && ` · ${result.note}`}
              </li>
            ))}
          </ol>
          <p className={classes.body}>
            {preview.request.deliveredArtifacts.length === 0
              ? "No delivered Artifacts cited"
              : `${preview.request.deliveredArtifacts.length} delivered Artifacts cited`}
            {" · "}
            {preview.request.unfinishedWork.length === 0
              ? "No unfinished work recorded"
              : `${preview.request.unfinishedWork.length} unfinished items recorded`}
            {" · "}
            {preview.request.uncertainty.length === 0
              ? "No uncertainty recorded"
              : `${preview.request.uncertainty.length} uncertainty items recorded`}
          </p>
          <p className={classes.body}>
            {"notMeasured" in preview.request.spend
              ? `Spend not measured: ${preview.request.spend.notMeasured}`
              : `Spend record ${preview.request.spend.recorded.spendId} v${preview.request.spend.recorded.spendVersion}`}
          </p>
          <p className={classes.body}>{preview.request.summary}</p>
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
        <p role="alert" className={classes.body}>
          The recorded completion could not be read. Retry the read.
        </p>
      )}
    </section>
  );
}
