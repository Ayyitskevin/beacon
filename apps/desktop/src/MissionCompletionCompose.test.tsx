import { invoke } from "@tauri-apps/api/core";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";

import { MissionCompletionCompose } from "./MissionCompletionCompose";
import type { MissionView } from "./session";
import { expectNoWcagViolations } from "./test/axe";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

// MOCKED CONTRACT ONLY. `preview_mission_completion`, `record_mission_completion` and
// `get_mission_completion_intent` have no owner; green here is frontend behaviour, never integration.
const MISSION = "88888888-8888-4888-8888-8888888c4000";
const OTHER_MISSION = "88888888-8888-4888-8888-8888888c4111";
const RESIDENT = "44444444-4444-4444-8444-444444444444";
const CONFIG = "55555555-5555-4555-8555-555555555555";
const RUN = "66666666-6666-4666-8666-666666666666";
const SPEND = "77777777-7777-4777-8777-777777777777";
const A = "a".repeat(64);
const B = "b".repeat(64);
const C = "c".repeat(64);
const D = "d".repeat(64);
const TOKEN = "e".repeat(64);
const OTHER_TOKEN = "f".repeat(64);

function mission(id = MISSION): MissionView {
  return {
    id,
    workspaceId: "11111111-1111-4111-8111-111111111111",
    projectId: "22222222-2222-4222-8222-222222222222",
    charterRevision: 1,
    teamId: "33333333-3333-4333-8333-333333333333",
    desiredOutcome: "Ship it",
    successConditions: ["Gates pass", "A reviewer signs"],
    constraints: [],
    prohibitions: [],
    lifecycle: "admitted",
    admittedBy: "operator",
    admittedAtMs: 126,
    members: [{ residentId: RESIDENT, admissionConfigurationId: CONFIG }],
  } as unknown as MissionView;
}

function previewFor(request: unknown, intent = TOKEN) {
  return {
    request,
    intentSha256: intent,
    workspaceId: "11111111-1111-4111-8111-111111111111",
    projectId: "22222222-2222-4222-8222-222222222222",
    actor: "operator",
    accountableResidentConfigurationId: CONFIG,
    declaredConditionsSha256: D,
    conditions: ["Gates pass", "A reviewer signs"],
  };
}

async function choose(user: ReturnType<typeof userEvent.setup>, label: string, option: string) {
  await user.click(screen.getByLabelText(label));
  await user.click(await screen.findByRole("option", { name: option }));
}

/** Fills every required control. Optional evidence is added by the caller. */
async function fillMinimum(user: ReturnType<typeof userEvent.setup>) {
  await choose(user, "Accountable Resident", RESIDENT);
  await choose(user, "Disposition", "Concluded");
  await choose(user, "Outcome for condition 1", "Met");
  await choose(user, "Outcome for condition 2", "Not met");
  await choose(user, "Spend", "Not measured");
  await user.type(screen.getByLabelText("Why spend was not measured"), "no meter on this Mission");
  await user.type(screen.getByLabelText("Completion summary"), "Shipped it.");
}

beforeEach(() => {
  invokeMock.mockReset();
});

it("selects nothing by default: no Resident, disposition, assessment or spend shape", async () => {
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  for (const label of [
    "Accountable Resident",
    "Disposition",
    "Outcome for condition 1",
    "Outcome for condition 2",
    "Spend",
  ]) {
    expect(screen.getByLabelText(label), `C4.COMPOSE.NOTHING_PRESELECTED: ${label}`).toHaveValue("");
  }
  expect(
    screen.getByRole("button", { name: "Review completion" }),
    "C4.COMPOSE.REVIEW_NEEDS_A_COMPLETE_REQUEST",
  ).toBeDisabled();
  await expectNoWcagViolations(document.body);
});

it("sends every citation kind and both narrative lists exactly as entered", async () => {
  const user = userEvent.setup();
  const sent: unknown[] = [];
  invokeMock.mockImplementation((command, args) => {
    if (command === "preview_mission_completion") {
      sent.push(args);
      return Promise.resolve(previewFor((args as { request: unknown }).request));
    }
    return Promise.reject(new Error(`unexpected ${String(command)}`));
  });
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);

  await user.click(screen.getByRole("button", { name: "Add support to condition 1" }));
  await choose(user, "Reference kind 1.1", "Artifact");
  await user.type(screen.getByLabelText("Record digest 1.1"), A);
  await user.click(screen.getByRole("button", { name: "Add support to condition 1" }));
  await choose(user, "Reference kind 1.2", "Receipt");
  await user.type(screen.getByLabelText("Run id 1.2"), RUN);
  await user.type(screen.getByLabelText("Receipt version 1.2"), "2");
  await user.type(screen.getByLabelText("Receipt bytes digest 1.2"), B);
  await user.click(screen.getByRole("button", { name: "Add support to condition 2" }));
  await choose(user, "Reference kind 2.1", "Evaluation");
  await user.type(screen.getByLabelText("Record digest 2.1"), C);
  await user.click(screen.getByRole("button", { name: "Add support to condition 2" }));
  await choose(user, "Reference kind 2.2", "Ruling");
  await user.type(screen.getByLabelText("Record digest 2.2"), D);
  await user.type(screen.getByLabelText("Delivered Artifact digests"), A);
  await user.click(screen.getByRole("button", { name: "Add unfinished work item" }));
  await choose(user, "Unfinished work kind 1", "Known defect");
  // The Operator types the condition NUMBER shown on screen; the owner stores the ordinal below it.
  await user.type(screen.getByLabelText("Unfinished work condition 1"), "2");
  await user.type(screen.getByLabelText("Unfinished work summary 1"), "one defect stands");
  await user.click(screen.getByRole("button", { name: "Add uncertainty item" }));
  await choose(user, "Uncertainty kind 1", "Unverified claim");
  await user.type(screen.getByLabelText("Uncertainty summary 1"), "nobody measured it");

  await user.click(screen.getByRole("button", { name: "Review completion" }));
  await screen.findByLabelText("Completion review");

  expect(sent, "C4.COMPOSE.EVERY_FIELD_SURVIVES").toEqual([
    {
      request: {
        missionId: MISSION,
        accountableResidentId: RESIDENT,
        disposition: "concluded",
        conditionResults: [
          {
            ordinal: 0,
            outcome: "met",
            support: [
              { artifact: A },
              { receipt: { runId: RUN, receiptVersion: 2, receiptBytesSha256: B } },
            ],
            note: null,
          },
          {
            ordinal: 1,
            outcome: "not_met",
            support: [{ evaluation: C }, { ruling: D }],
            note: null,
          },
        ],
        deliveredArtifacts: [A],
        unfinishedWork: [
          { ordinal: 0, kind: "known_defect", conditionOrdinal: 1, summary: "one defect stands" },
        ],
        uncertainty: [
          {
            ordinal: 0,
            kind: "unverified_claim",
            conditionOrdinal: null,
            summary: "nobody measured it",
          },
        ],
        spend: { notMeasured: "no meter on this Mission" },
        summary: "Shipped it.",
      },
    },
  ]);
});

it("renders all four condition outcomes as distinct choices", async () => {
  const user = userEvent.setup();
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await user.click(screen.getByLabelText("Outcome for condition 1"));
  for (const label of ["Met", "Not met", "Not assessed", "Assessment inconclusive"]) {
    expect(
      await screen.findByRole("option", { name: label }),
      `C4.COMPOSE.FOUR_DISTINCT_OUTCOMES: ${label}`,
    ).toBeVisible();
  }
});

it("cannot record before a preview and an explicit permanence confirmation", async () => {
  const user = userEvent.setup();
  invokeMock.mockImplementation((command, args) =>
    command === "preview_mission_completion"
      ? Promise.resolve(previewFor((args as { request: unknown }).request))
      : Promise.reject(new Error("no")),
  );
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);
  expect(
    screen.queryByRole("button", { name: "Record completion" }),
    "C4.RECORD.NO_RECORD_BEFORE_PREVIEW",
  ).toBeNull();

  await user.click(screen.getByRole("button", { name: "Review completion" }));
  const record = await screen.findByRole("button", { name: "Record completion" });
  expect(record, "C4.RECORD.NO_RECORD_BEFORE_CONFIRMATION").toBeDisabled();
  await user.click(screen.getByRole("button", { name: "Confirm this record cannot be replaced" }));
  expect(
    screen.getByRole("button", { name: "Record completion" }),
    "C4.RECORD.CONFIRMATION_ENABLES_RECORD",
  ).toBeEnabled();
  await expectNoWcagViolations(document.body);
});

it("discards the preview as soon as any field is edited", async () => {
  const user = userEvent.setup();
  invokeMock.mockImplementation((command, args) =>
    command === "preview_mission_completion"
      ? Promise.resolve(previewFor((args as { request: unknown }).request))
      : Promise.reject(new Error("no")),
  );
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Review completion" }));
  await screen.findByLabelText("Completion review");
  await user.type(screen.getByLabelText("Completion summary"), " and then some.");
  expect(
    screen.queryByLabelText("Completion review"),
    "C4.REVIEW.EDIT_DISCARDS_THE_PREVIEW",
  ).toBeNull();
  expect(
    screen.getByRole("button", { name: "Review completion" }),
    "C4.REVIEW.EDIT_DISCARDS_THE_PREVIEW",
  ).toBeEnabled();
});

it("returns focus to the review control when the review is cancelled", async () => {
  const user = userEvent.setup();
  invokeMock.mockImplementation((command, args) =>
    command === "preview_mission_completion"
      ? Promise.resolve(previewFor((args as { request: unknown }).request))
      : Promise.reject(new Error("no")),
  );
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);
  const review = screen.getByRole("button", { name: "Review completion" });
  review.focus();
  await user.keyboard("{Enter}");
  await screen.findByLabelText("Completion review");
  await user.click(screen.getByRole("button", { name: "Cancel review" }));
  expect(
    screen.getByRole("button", { name: "Review completion" }),
    "C4.REVIEW.CANCEL_RETURNS_TO_COMPOSE",
  ).toHaveFocus();
});

async function recordWith(
  user: ReturnType<typeof userEvent.setup>,
  handlers: Record<string, (args: unknown) => Promise<unknown>>,
  onRecorded = vi.fn(),
) {
  invokeMock.mockImplementation((command, args) => {
    const handler = handlers[String(command)];
    if (handler) return handler(args);
    return Promise.reject(new Error(`unexpected ${String(command)}`));
  });
  render(<MissionCompletionCompose mission={mission()} onRecorded={onRecorded} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Review completion" }));
  await screen.findByLabelText("Completion review");
  await user.click(screen.getByRole("button", { name: "Confirm this record cannot be replaced" }));
  await user.click(screen.getByRole("button", { name: "Record completion" }));
  return onRecorded;
}

it("accepts an acknowledgement whose token equals the reviewed one", async () => {
  const user = userEvent.setup();
  const onRecorded = await recordWith(userEvent.setup(), {
    preview_mission_completion: (args) =>
      Promise.resolve(previewFor((args as { request: unknown }).request)),
    record_mission_completion: () => Promise.resolve({ intentSha256: TOKEN }),
  });
  expect(
    await screen.findByText("A recorded completion matches the reviewed details."),
    "C4.RECORD.MATCHING_TOKEN_IS_CONTENT_EQUALITY",
  ).toBeVisible();
  expect(onRecorded, "C4.RECORD.EXPLICIT_REFRESH_SIGNAL").toHaveBeenCalledTimes(1);
  // State 7 of 10: recorded match.
  await expectNoWcagViolations(document.body);
  void user;
});

it("does not accept a differing acknowledgement as success, and reads back instead", async () => {
  const reads: unknown[] = [];
  await recordWith(userEvent.setup(), {
    preview_mission_completion: (args) =>
      Promise.resolve(previewFor((args as { request: unknown }).request)),
    record_mission_completion: () => Promise.resolve({ intentSha256: OTHER_TOKEN }),
    get_mission_completion_intent: (args) => {
      reads.push(args);
      return Promise.resolve({ intentSha256: OTHER_TOKEN });
    },
  });
  expect(
    await screen.findByText(
      "A different completion is already recorded for this Mission. It was not replaced.",
    ),
    "C4.RECORD.DIFFERING_TOKEN_IS_NOT_SUCCESS",
  ).toBeVisible();
  expect(reads, "C4.RECOVERY.READS_BACK_THE_SAME_MISSION").toEqual([{ missionId: MISSION }]);
  // State 8 of 10: a conflicting record.
  await expectNoWcagViolations(document.body);
});

it("preserves the form and reads back when the record itself fails", async () => {
  const writes: unknown[] = [];
  await recordWith(userEvent.setup(), {
    preview_mission_completion: (args) =>
      Promise.resolve(previewFor((args as { request: unknown }).request)),
    record_mission_completion: (args) => {
      writes.push(args);
      return Promise.reject({ code: "mission_completion_unavailable" });
    },
    get_mission_completion_intent: () => Promise.resolve(null),
  });
  expect(
    await screen.findByText(
      "No completion is visible yet. The reviewed details are preserved; recording again is your choice.",
    ),
    "C4.RECOVERY.NULL_IS_NOT_VISIBLE_YET",
  ).toBeVisible();
  expect(
    screen.getByLabelText("Completion summary"),
    "C4.RECOVERY.FORM_IS_PRESERVED",
  ).toHaveValue("Shipped it.");
  expect(writes.length, "C4.RECOVERY.NO_AUTOMATIC_SECOND_WRITE").toBe(1);
  // State 9 of 10: nothing visible yet.
  await expectNoWcagViolations(document.body);
});

it("offers a read Retry, never a repeated write, when the read-back is unavailable", async () => {
  const writes: unknown[] = [];
  await recordWith(userEvent.setup(), {
    preview_mission_completion: (args) =>
      Promise.resolve(previewFor((args as { request: unknown }).request)),
    record_mission_completion: (args) => {
      writes.push(args);
      return Promise.reject({ code: "mission_completion_unavailable" });
    },
    get_mission_completion_intent: () => Promise.reject({ code: "mission_completion_unavailable" }),
  });
  expect(
    await screen.findByText("The recorded completion could not be read. Retry the read."),
    "C4.RECOVERY.UNAVAILABLE_OFFERS_READ_RETRY",
  ).toBeVisible();
  expect(
    screen.getByRole("button", { name: "Retry the read" }),
    "C4.RECOVERY.UNAVAILABLE_OFFERS_A_READ_CONTROL",
  ).toBeEnabled();
  expect(writes.length, "C4.RECOVERY.NO_AUTOMATIC_SECOND_WRITE").toBe(1);
  // State 10 of 10: the read-back could not be run.
  await expectNoWcagViolations(document.body);
});

/** Fill and ask for a preview only: used where the preview itself refuses, so no review appears. */
async function previewRefusedBy(code: string) {
  const user = userEvent.setup();
  invokeMock.mockImplementation((command) =>
    command === "preview_mission_completion"
      ? Promise.reject({ code })
      : Promise.reject(new Error("no")),
  );
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Review completion" }));
}

it("keeps an existing session refusal truthful rather than relabelling it", async () => {
  await previewRefusedBy("authentication_failed");
  expect(
    await screen.findByText("That credential was not accepted."),
    "C4.COPY.EXISTING_REFUSALS_STAY_TRUTHFUL",
  ).toBeVisible();
});

it("gives each new completion code its own fixed copy", async () => {
  await previewRefusedBy("completion_intent_changed");
  expect(
    await screen.findByText("The reviewed details changed. Review the completion again."),
    "C4.COPY.NEW_CODES_HAVE_FIXED_COPY",
  ).toBeVisible();
});

/* The combined stale-preview case that used to stand here was a FALSE GREEN, measured by root: its
   "success" payload was an invalid preview the component would have refused anyway, and its second
   leg rejected a promise that had already been resolved, which does nothing at all. Root's
   `AstraC4Lifetimes.test.tsx` now owns the two preview legs with a genuinely held request; the legs
   below are the record and read-back siblings it does not cover. */

/** Holds one command open and hands back its settle functions. Every leg below keeps its own
 *  promise: a single shared promise cannot be rejected after it has resolved, which is how the
 *  removed test passed while the component was wrong. */
function held() {
  let settle!: { resolve: (value: unknown) => void; reject: (cause: unknown) => void };
  const promise = new Promise<unknown>((resolve, reject) => {
    settle = { resolve, reject };
  });
  return { promise, settle };
}

/** Fills and reviews for MISSION, then records, with `record_mission_completion` held open. */
async function recordHeldForMission(onRecorded = vi.fn()) {
  const user = userEvent.setup();
  const write = held();
  const reads: unknown[] = [];
  let writes = 0;
  invokeMock.mockImplementation((command, args) => {
    if (command === "preview_mission_completion") {
      return Promise.resolve(previewFor((args as { request: unknown }).request));
    }
    if (command === "record_mission_completion") {
      writes += 1;
      return write.promise;
    }
    if (command === "get_mission_completion_intent") {
      reads.push(args);
      return Promise.resolve(null);
    }
    return Promise.reject(new Error(`unexpected ${String(command)}`));
  });
  const view = render(<MissionCompletionCompose mission={mission()} onRecorded={onRecorded} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Review completion" }));
  await screen.findByLabelText("Completion review");
  await user.click(screen.getByRole("button", { name: "Confirm this record cannot be replaced" }));
  await user.click(screen.getByRole("button", { name: "Record completion" }));
  await screen.findByText("Recording the completion");
  return { user, view, write, reads, onRecorded, writes: () => writes };
}

it("a matching acknowledgement for the Mission the Operator left records nothing here", async () => {
  const { view, write, onRecorded } = await recordHeldForMission();
  view.rerender(<MissionCompletionCompose mission={mission(OTHER_MISSION)} onRecorded={onRecorded} />);
  await act(async () => {
    write.settle.resolve({ intentSha256: TOKEN });
  });
  expect(
    screen.queryByText("A recorded completion matches the reviewed details."),
    "C4.LIFETIME.OLD_MISSION_ACK_IS_NOT_THIS_MISSION_RECORDED",
  ).toBeNull();
  expect(onRecorded, "C4.LIFETIME.OLD_MISSION_ACK_REFRESHES_NOTHING").not.toHaveBeenCalled();
});

it("a record refusal for the Mission the Operator left plants no failure and no read", async () => {
  const { view, write, reads, onRecorded } = await recordHeldForMission();
  view.rerender(<MissionCompletionCompose mission={mission(OTHER_MISSION)} onRecorded={onRecorded} />);
  await act(async () => {
    write.settle.reject({ code: "mission_completion_unavailable" });
  });
  expect(
    screen.queryByText("The Mission completion could not be verified."),
    "C4.LIFETIME.OLD_MISSION_REFUSAL_LABELS_NOTHING",
  ).toBeNull();
  expect(reads, "C4.LIFETIME.OLD_MISSION_REFUSAL_READS_NOTHING_BACK").toEqual([]);
});

it("an edit while the write is in flight still reports what the write did", async () => {
  const { user, write, onRecorded, writes } = await recordHeldForMission();
  // Ignoring a late result would NOT cancel the write. The draft changed, so the review is gone --
  // but the submitted write is still owed a truthful answer.
  await user.type(screen.getByLabelText("Completion summary"), " Typed while recording.");
  await act(async () => {
    write.settle.resolve({ intentSha256: TOKEN });
  });
  expect(
    await screen.findByText("A recorded completion matches the reviewed details."),
    "C4.LIFETIME.EDIT_DURING_WRITE_STILL_REPORTS_THE_WRITE",
  ).toBeVisible();
  expect(onRecorded, "C4.LIFETIME.EDIT_DURING_WRITE_STILL_REFRESHES").toHaveBeenCalledTimes(1);
  expect(writes(), "C4.LIFETIME.EDIT_DURING_WRITE_NEVER_WRITES_AGAIN").toBe(1);
});

it("a read-back for the Mission the Operator left reports nothing here", async () => {
  const user = userEvent.setup();
  const read = held();
  const onRecorded = vi.fn();
  invokeMock.mockImplementation((command, args) => {
    if (command === "preview_mission_completion") {
      return Promise.resolve(previewFor((args as { request: unknown }).request));
    }
    if (command === "record_mission_completion") {
      return Promise.reject({ code: "mission_completion_unavailable" });
    }
    if (command === "get_mission_completion_intent") return read.promise;
    return Promise.reject(new Error(`unexpected ${String(command)}`));
  });
  const view = render(<MissionCompletionCompose mission={mission()} onRecorded={onRecorded} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Review completion" }));
  await screen.findByLabelText("Completion review");
  await user.click(screen.getByRole("button", { name: "Confirm this record cannot be replaced" }));
  await user.click(screen.getByRole("button", { name: "Record completion" }));
  await screen.findByText("Reading back what was recorded");
  view.rerender(<MissionCompletionCompose mission={mission(OTHER_MISSION)} onRecorded={onRecorded} />);
  await act(async () => {
    read.settle.resolve({ intentSha256: TOKEN });
  });
  expect(
    screen.queryByText("A recorded completion matches the reviewed details."),
    "C4.LIFETIME.OLD_MISSION_READ_BACK_REPORTS_NOTHING",
  ).toBeNull();
  expect(onRecorded, "C4.LIFETIME.OLD_MISSION_READ_BACK_REFRESHES_NOTHING").not.toHaveBeenCalled();
});

it("an older leg cannot clear the pending action of the Mission now on screen", async () => {
  const user = userEvent.setup();
  const write = held();
  const secondPreview = held();
  let previews = 0;
  invokeMock.mockImplementation((command, args) => {
    if (command === "preview_mission_completion") {
      previews += 1;
      return previews === 1
        ? Promise.resolve(previewFor((args as { request: unknown }).request))
        : secondPreview.promise;
    }
    if (command === "record_mission_completion") return write.promise;
    return Promise.reject(new Error(`unexpected ${String(command)}`));
  });
  const onRecorded = vi.fn();
  const view = render(<MissionCompletionCompose mission={mission()} onRecorded={onRecorded} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Review completion" }));
  await screen.findByLabelText("Completion review");
  await user.click(screen.getByRole("button", { name: "Confirm this record cannot be replaced" }));
  await user.click(screen.getByRole("button", { name: "Record completion" }));
  await screen.findByText("Recording the completion");

  view.rerender(<MissionCompletionCompose mission={mission(OTHER_MISSION)} onRecorded={onRecorded} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Review completion" }));
  await screen.findByText("Deriving the completion for review");

  await act(async () => {
    write.settle.resolve({ intentSha256: TOKEN });
  });
  expect(
    screen.queryByText("Deriving the completion for review"),
    "C4.LIFETIME.OLD_LEG_CANNOT_CLEAR_A_NEWER_PENDING",
  ).not.toBeNull();
  expect(
    screen.queryByText("A recorded completion matches the reviewed details."),
    "C4.LIFETIME.OLD_LEG_REPORTS_NO_OUTCOME_HERE",
  ).toBeNull();
});

it("a different Mission inherits no assessment, no Resident and no review", async () => {
  const user = userEvent.setup();
  invokeMock.mockImplementation((command, args) =>
    command === "preview_mission_completion"
      ? Promise.resolve(previewFor((args as { request: unknown }).request))
      : Promise.reject(new Error("no")),
  );
  const view = render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Review completion" }));
  await screen.findByLabelText("Completion review");

  view.rerender(<MissionCompletionCompose mission={mission(OTHER_MISSION)} onRecorded={vi.fn()} />);

  expect(
    screen.queryByLabelText("Completion review"),
    "C4.LIFETIME.NEW_MISSION_INHERITS_NO_REVIEW",
  ).toBeNull();
  expect(
    screen.getByLabelText("Completion summary"),
    "C4.LIFETIME.NEW_MISSION_INHERITS_NO_SUMMARY",
  ).toHaveValue("");
  expect(
    screen.queryByLabelText("Why spend was not measured"),
    "C4.LIFETIME.NEW_MISSION_INHERITS_NO_SPEND_REASON",
  ).toBeNull();
  for (const label of ["Accountable Resident", "Disposition", "Outcome for condition 1", "Spend"]) {
    expect(
      screen.getByLabelText(label),
      `C4.LIFETIME.NEW_MISSION_INHERITS_NO_CHOICE: ${label}`,
    ).toHaveValue("");
  }
  expect(
    screen.getByRole("button", { name: "Review completion" }),
    "C4.LIFETIME.NEW_MISSION_CANNOT_BE_REVIEWED_YET",
  ).toBeDisabled();
});

it("refuses a preview whose declared conditions no longer match this Mission", async () => {
  const user = userEvent.setup();
  invokeMock.mockImplementation((command, args) =>
    command === "preview_mission_completion"
      ? Promise.resolve({
          ...previewFor((args as { request: unknown }).request),
          conditions: ["Gates pass", "A DIFFERENT condition"],
        })
      : Promise.reject(new Error("no")),
  );
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Review completion" }));
  expect(
    await screen.findByText(
      "The Mission's declared conditions changed. Review the completion again.",
    ),
    "C4.REVIEW.CHANGED_DECLARATION_IS_REFUSED",
  ).toBeVisible();
  expect(screen.queryByLabelText("Completion review")).toBeNull();
});

it("refuses a malformed preview rather than reviewing it", async () => {
  const user = userEvent.setup();
  invokeMock.mockImplementation((command, args) =>
    command === "preview_mission_completion"
      ? Promise.resolve({
          ...previewFor((args as { request: unknown }).request),
          intentSha256: "not-a-digest",
        })
      : Promise.reject(new Error("no")),
  );
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Review completion" }));
  expect(
    await screen.findByText("Beacon could not complete that local request."),
    "C4.VALIDATION.MALFORMED_PREVIEW_IS_REFUSED",
  ).toBeVisible();
  expect(screen.queryByLabelText("Completion review")).toBeNull();
});

it("does not send a support reference whose kind was never chosen", async () => {
  const user = userEvent.setup();
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Add support to condition 1" }));
  expect(
    screen.getByRole("button", { name: "Review completion" }),
    "C4.VALIDATION.INCOMPLETE_REFERENCE_BLOCKS_REVIEW",
  ).toBeDisabled();
});

it("labels every reference as manually entered and unverified here", async () => {
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  expect(
    screen.getByText("Reference entered manually; supporting content is not verified here."),
    "C4.COPY.REFERENCES_ARE_LABELLED_UNVERIFIED",
  ).toBeVisible();
});

it("accepts a note at the owner's 500-scalar bound and refuses 501", async () => {
  const user = userEvent.setup();
  // Astral: one scalar, two UTF-16 units. A `.length` bound would drift by a factor of two.
  const at = "\u{1F5FF}".repeat(500);
  expect(at.length, "C4.TEXT.BOUND_IS_SCALARS_NOT_UTF16").toBe(1_000);
  invokeMock.mockImplementation((command, args) =>
    command === "preview_mission_completion"
      ? Promise.resolve(previewFor((args as { request: unknown }).request))
      : Promise.reject(new Error("no")),
  );
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);
  const note = screen.getByLabelText("Note for condition 1");
  await act(async () => {
    await user.clear(note);
  });
  fireEvent.change(note, { target: { value: at } });
  expect(
    screen.getByRole("button", { name: "Review completion" }),
    "C4.TEXT.AT_THE_OWNER_BOUND_IS_ACCEPTED",
  ).toBeEnabled();

  fireEvent.change(note, { target: { value: `${at}\u{1F5FF}` } });
  expect(
    screen.getByRole("button", { name: "Review completion" }),
    "C4.TEXT.OVER_THE_OWNER_BOUND_IS_REFUSED",
  ).toBeDisabled();
});

it("blue refuses an owner preview bound to a different Project", async () => {
  const user = userEvent.setup();
  invokeMock.mockImplementation((command, args) =>
    command === "preview_mission_completion"
      ? Promise.resolve({
          ...previewFor((args as { request: unknown }).request),
          projectId: "99999999-9999-4999-8999-999999999999",
        })
      : Promise.reject(new Error("no")),
  );
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Review completion" }));
  expect(
    screen.queryByLabelText("Completion review"),
    "C4.REVIEW.DIFFERENT_PROJECT_IS_REFUSED",
  ).toBeNull();
});

it("blue refuses an out-of-domain u32 in the owner preview", async () => {
  const user = userEvent.setup();
  invokeMock.mockImplementation((command, args) => {
    if (command !== "preview_mission_completion") return Promise.reject(new Error("no"));
    const request = (args as { request: Record<string, unknown> }).request;
    const conditionResults = request.conditionResults as Array<Record<string, unknown>>;
    return Promise.resolve(
      previewFor({
        ...request,
        conditionResults: [
          {
            ...conditionResults[0],
            support: [
              {
                receipt: {
                  runId: RUN,
                  receiptVersion: 4_294_967_296,
                  receiptBytesSha256: B,
                },
              },
            ],
          },
          conditionResults[1],
        ],
      }),
    );
  });
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Review completion" }));
  expect(
    screen.queryByLabelText("Completion review"),
    "C4.VALIDATION.U32_ABOVE_MAX_IS_REFUSED",
  ).toBeNull();
});

it("blue review exposes the exact normalized request and every binding", async () => {
  const user = userEvent.setup();
  invokeMock.mockImplementation((command, args) => {
    if (command !== "preview_mission_completion") return Promise.reject(new Error("no"));
    const request = (args as { request: Record<string, unknown> }).request;
    const conditionResults = request.conditionResults as Array<Record<string, unknown>>;
    return Promise.resolve(
      previewFor({
        ...request,
        conditionResults: [
          {
            ...conditionResults[0],
            support: [
              { artifact: A },
              { receipt: { runId: RUN, receiptVersion: 2, receiptBytesSha256: B } },
            ],
          },
          {
            ...conditionResults[1],
            support: [{ evaluation: C }, { ruling: D }],
          },
        ],
        deliveredArtifacts: [A],
        unfinishedWork: [
          { ordinal: 0, kind: "known_defect", conditionOrdinal: 1, summary: "exact unfinished" },
        ],
        uncertainty: [
          {
            ordinal: 0,
            kind: "unverified_claim",
            conditionOrdinal: null,
            summary: "exact uncertainty",
          },
        ],
        spend: { recorded: { spendId: SPEND, spendVersion: 2, spendSha256: C } },
      }),
    );
  });
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Review completion" }));
  const review = await screen.findByLabelText("Completion review");
  const text = review.textContent ?? "";
  const required = [
    "11111111-1111-4111-8111-111111111111",
    "22222222-2222-4222-8222-222222222222",
    MISSION,
    "operator",
    D,
    A,
    RUN,
    B,
    C,
    "exact unfinished",
    "condition 2",
    "exact uncertainty",
    SPEND,
  ];
  expect(
    required.filter((value) => !text.includes(value)),
    "C4.REVIEW.EXACT_NORMALIZED_INTENT_AND_BINDINGS_ARE_VISIBLE",
  ).toEqual([]);
});

it("blue refuses blank mandatory summary before invoking preview", async () => {
  const user = userEvent.setup();
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);
  await user.clear(screen.getByLabelText("Completion summary"));
  expect(
    screen.getByRole("button", { name: "Review completion" }),
    "C4.VALIDATION.EMPTY_REQUIRED_TEXT_IS_REFUSED_BEFORE_INVOKE",
  ).toBeDisabled();
});

/* WCAG 2.1 AA over every rendered state this screen has, not only the two it started with. The
   states are: compose, review, preview pending, preview refusal, record pending, read-back pending,
   recorded match, conflicting record, nothing visible yet, and read unavailable. Compose and review
   are witnessed in the tests above; the four transient states are witnessed here because no other
   test holds them open long enough to run the bar. */
async function reachPending(command: string, hold: Promise<unknown>) {
  const user = userEvent.setup();
  invokeMock.mockImplementation((name, args) => {
    if (name === command) return hold;
    if (name === "preview_mission_completion") {
      return Promise.resolve(previewFor((args as { request: unknown }).request));
    }
    return Promise.reject(new Error(`unexpected ${String(name)}`));
  });
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Review completion" }));
  return user;
}

it("axe: the preview-pending state", async () => {
  await reachPending("preview_mission_completion", new Promise<unknown>(() => {}));
  await screen.findByText("Deriving the completion for review");
  await expectNoWcagViolations(document.body);
});

it("axe: the preview-refusal state", async () => {
  await previewRefusedBy("completion_condition_coverage_incomplete");
  await screen.findByText("Assess every declared condition explicitly.");
  await expectNoWcagViolations(document.body);
});

it("axe: the record-pending state", async () => {
  const user = await reachPending("record_mission_completion", new Promise<unknown>(() => {}));
  await screen.findByLabelText("Completion review");
  await user.click(screen.getByRole("button", { name: "Confirm this record cannot be replaced" }));
  await user.click(screen.getByRole("button", { name: "Record completion" }));
  await screen.findByText("Recording the completion");
  await expectNoWcagViolations(document.body);
});

it("axe: the read-back-pending state", async () => {
  const user = userEvent.setup();
  invokeMock.mockImplementation((command, args) => {
    if (command === "preview_mission_completion") {
      return Promise.resolve(previewFor((args as { request: unknown }).request));
    }
    if (command === "record_mission_completion") {
      return Promise.reject({ code: "mission_completion_unavailable" });
    }
    if (command === "get_mission_completion_intent") return new Promise<unknown>(() => {});
    return Promise.reject(new Error(`unexpected ${String(command)}`));
  });
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Review completion" }));
  await screen.findByLabelText("Completion review");
  await user.click(screen.getByRole("button", { name: "Confirm this record cannot be replaced" }));
  await user.click(screen.getByRole("button", { name: "Record completion" }));
  await screen.findByText("Reading back what was recorded");
  await expectNoWcagViolations(document.body);
});

it("sends the exact frozen reviewed request and token on the record leg", async () => {
  const user = userEvent.setup();
  const writes: unknown[] = [];
  invokeMock.mockImplementation((command, args) => {
    if (command === "preview_mission_completion") {
      return Promise.resolve(previewFor((args as { request: unknown }).request));
    }
    if (command === "record_mission_completion") {
      writes.push(args);
      return Promise.resolve({ intentSha256: TOKEN });
    }
    return Promise.reject(new Error(`unexpected ${String(command)}`));
  });
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Add support to condition 2" }));
  await choose(user, "Reference kind 2.1", "Receipt");
  await user.type(screen.getByLabelText("Run id 2.1"), RUN);
  await user.type(screen.getByLabelText("Receipt version 2.1"), "2");
  await user.type(screen.getByLabelText("Receipt bytes digest 2.1"), B);
  await user.click(screen.getByRole("button", { name: "Add uncertainty item" }));
  await choose(user, "Uncertainty kind 1", "Conflicting evidence");
  await user.type(screen.getByLabelText("Uncertainty condition 1"), "1");
  await user.type(screen.getByLabelText("Uncertainty summary 1"), "two runs disagree");
  await user.click(screen.getByRole("button", { name: "Review completion" }));
  await screen.findByLabelText("Completion review");
  await user.click(screen.getByRole("button", { name: "Confirm this record cannot be replaced" }));
  await user.click(screen.getByRole("button", { name: "Record completion" }));
  await screen.findByText("A recorded completion matches the reviewed details.");

  // Count for count, the WRITE leg -- not only the preview leg. An independent literal, so a wrong
  // captured request cannot agree with itself.
  expect(writes, "C4.RECORD.EXACT_FROZEN_REQUEST_AND_TOKEN").toEqual([
    {
      request: {
        missionId: MISSION,
        accountableResidentId: RESIDENT,
        disposition: "concluded",
        conditionResults: [
          { ordinal: 0, outcome: "met", support: [], note: null },
          {
            ordinal: 1,
            outcome: "not_met",
            support: [{ receipt: { runId: RUN, receiptVersion: 2, receiptBytesSha256: B } }],
            note: null,
          },
        ],
        deliveredArtifacts: [],
        unfinishedWork: [],
        uncertainty: [
          {
            ordinal: 0,
            kind: "conflicting_evidence",
            conditionOrdinal: 0,
            summary: "two runs disagree",
          },
        ],
        spend: { notMeasured: "no meter on this Mission" },
        summary: "Shipped it.",
      },
      reviewedIntentSha256: TOKEN,
    },
  ]);
});

/* The closed caller table, transcribed from `C4-interface.md`'s dated addendum rather than read off
   the component: a table generated from the map under test would agree with any map. `mission_
   completion_unavailable` is C3's existing shared code and keeps C3's exact sentence; the proposed
   `completion_unavailable` alias is retired and must no longer produce completion copy. */
const CALLER_LINES: readonly (readonly [string, string])[] = [
  ["completion_intent_changed", "The reviewed details changed. Review the completion again."],
  [
    "completion_already_recorded",
    "A completion is already recorded for this Mission. Read the recorded completion.",
  ],
  ["completion_resident_not_in_admission_roster", "Choose a Resident admitted to this Mission."],
  ["completion_condition_coverage_incomplete", "Assess every declared condition explicitly."],
  [
    "completion_support_over_limit",
    "This condition cites more support than a completion can record.",
  ],
  ["completion_text_over_limit", "Some completion text exceeds the supported length."],
  ["completion_spend_reason_required", "Explain why spend was not measured."],
  [
    "completion_invalid_details",
    "Some completion details are invalid. Review the completion again.",
  ],
  ["mission_completion_unavailable", "The Mission completion could not be verified."],
];

it("gives every completion code its exact fixed line, through the real projection", async () => {
  for (const [code, line] of CALLER_LINES) {
    await previewRefusedBy(code);
    // The ALERT's whole text, not a substring match: a line that merely contains the sentence is a
    // different line, and `findByText` would fail before the assertion name could be printed.
    const alert = await screen.findByRole("alert");
    expect(alert.textContent, `C4.COPY.EXACT_LINE: ${code}`).toBe(line);
    cleanup();
  }
});

it("no longer answers the retired completion_unavailable proposal with completion copy", async () => {
  await previewRefusedBy("completion_unavailable");
  const alert = await screen.findByRole("alert");
  expect(alert.textContent, "C4.COPY.RETIRED_ALIAS_IS_NOT_COMPLETION_COPY").toBe(
    "Beacon could not complete that local request.",
  );
});

it("renders the fixed line and nothing the payload carried with it", async () => {
  const user = userEvent.setup();
  const needle = "R42_PRIVATE_/home/operator/beacon.sqlite3";
  invokeMock.mockImplementation((command) =>
    command === "preview_mission_completion"
      ? Promise.reject({
          code: "completion_invalid_details",
          // Everything a real refusal might carry beside its code. None of it is caller copy.
          message: `${needle} column=private_column`,
          field: "conditionResults[3].support[2].receipt.runId",
          detail: { path: needle, sql: `SELECT * FROM completions -- ${needle}` },
        })
      : Promise.reject(new Error("no")),
  );
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Review completion" }));

  const alert = await screen.findByRole("alert");
  expect(alert.textContent, "C4.COPY.RAW_PAYLOAD_IS_NOT_RENDERED").toBe(
    "Some completion details are invalid. Review the completion again.",
  );
  expect(
    document.body.textContent ?? "",
    "C4.COPY.RAW_PAYLOAD_IS_NOT_RENDERED",
  ).not.toContain(needle);
  expect(
    document.body.textContent ?? "",
    "C4.COPY.RAW_PAYLOAD_IS_NOT_RENDERED",
  ).not.toContain("conditionResults[3]");
});
