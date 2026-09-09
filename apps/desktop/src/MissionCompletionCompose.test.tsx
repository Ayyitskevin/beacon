import { invoke } from "@tauri-apps/api/core";
import { act, render, screen, within } from "@testing-library/react";
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
  await user.type(screen.getByLabelText("Unfinished work condition 1"), "1");
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
  ).toBeVisible();
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
});

it("preserves the form and reads back when the record itself fails", async () => {
  const writes: unknown[] = [];
  await recordWith(userEvent.setup(), {
    preview_mission_completion: (args) =>
      Promise.resolve(previewFor((args as { request: unknown }).request)),
    record_mission_completion: (args) => {
      writes.push(args);
      return Promise.reject({ code: "completion_unavailable" });
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
});

it("offers a read Retry, never a repeated write, when the read-back is unavailable", async () => {
  const writes: unknown[] = [];
  await recordWith(userEvent.setup(), {
    preview_mission_completion: (args) =>
      Promise.resolve(previewFor((args as { request: unknown }).request)),
    record_mission_completion: (args) => {
      writes.push(args);
      return Promise.reject({ code: "completion_unavailable" });
    },
    get_mission_completion_intent: () => Promise.reject({ code: "completion_unavailable" }),
  });
  expect(
    await screen.findByText("The recorded completion could not be read. Retry the read."),
    "C4.RECOVERY.UNAVAILABLE_OFFERS_READ_RETRY",
  ).toBeVisible();
  expect(writes.length, "C4.RECOVERY.NO_AUTOMATIC_SECOND_WRITE").toBe(1);
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

it("a stale preview success or rejection for another Mission replaces nothing", async () => {
  const user = userEvent.setup();
  let releaseFirst!: (value: unknown) => void;
  let rejectFirst!: (cause: unknown) => void;
  const first = new Promise<unknown>((resolve, reject) => {
    releaseFirst = resolve;
    rejectFirst = reject;
  });
  invokeMock.mockImplementation((command, args) => {
    if (command !== "preview_mission_completion") return Promise.reject(new Error("no"));
    return (args as { request: { missionId: string } }).request.missionId === MISSION
      ? first
      : new Promise<unknown>(() => {});
  });
  const view = render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await fillMinimum(user);
  await user.click(screen.getByRole("button", { name: "Review completion" }));
  expect(screen.queryByText("Deriving the completion for review")).not.toBeNull();

  // The SAME mounted instance, a different Mission: the guard is the only thing between the old
  // answer and this Mission's state.
  view.rerender(<MissionCompletionCompose mission={mission(OTHER_MISSION)} onRecorded={vi.fn()} />);

  await act(async () => {
    releaseFirst(previewFor({ missionId: MISSION }));
    await Promise.resolve();
    await Promise.resolve();
  });
  expect(
    screen.queryByLabelText("Completion review"),
    "C4.STALE.PREVIEW_SUCCESS_DOES_NOT_REPLACE",
  ).toBeNull();

  await act(async () => {
    rejectFirst({ code: "completion_intent_changed" });
    await Promise.resolve();
    await Promise.resolve();
  });
  expect(
    screen.queryByText("The reviewed details changed. Review the completion again."),
    "C4.STALE.PREVIEW_REJECTION_PLANTS_NO_FAILURE",
  ).toBeNull();
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
