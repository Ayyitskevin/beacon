import { invoke } from "@tauri-apps/api/core";
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";

import { MissionOutcomePreparation } from "./MissionOutcomePreparation";
import { expectNoWcag22Violations } from "./test/axe";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

// MOCKED CONTRACT ONLY. `prepare_outcome_candidate` has no owner behind it yet: C6 backend and
// native wait on C5's native release. Green here is FRONTEND behaviour and never an end-to-end
// claim about preparation.
const MISSION = "99999999-9999-4999-8999-9999999c6000";

function receipt(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    missionId: MISSION,
    appendixRequest: "include",
    appendixMode: "included",
    manifestId: "66666666-6666-4666-8666-666666666666",
    manifestDigest: { algorithm: "sha256", canonicalizer: "beacon.outcome.manifest.v2", value: "a".repeat(64) },
    candidateArtifactDigest: { algorithm: "sha256", canonicalizer: "beacon.artifact.v1", value: "b".repeat(64) },
    candidateContentDigest: { algorithm: "sha256", canonicalizer: "beacon.raw.v1", value: "c".repeat(64) },
    candidateContentBytes: 4_096,
    mediaType: "text/markdown; charset=utf-8",
    ...overrides,
  };
}

beforeEach(() => {
  cleanup();
  invokeMock.mockReset();
});

it("C6.PREPARE.THE_HUMAN_CHOICE_IS_SENT: an explicit appendix choice reaches the command once", async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValue(receipt());
  render(<MissionOutcomePreparation missionId={MISSION} />);

  // Nothing is preselected, and nothing runs before a click.
  expect(invokeMock, "C6.PREPARE.NO_INVOCATION_BEFORE_CLICK").not.toHaveBeenCalled();
  const prepare = screen.getByRole("button", { name: "Prepare candidate" });
  expect(prepare, "C6.PREPARE.NO_DEFAULT_CHOICE").toBeDisabled();

  await user.click(screen.getByRole("radio", { name: /Include verification appendix/ }));
  await user.click(prepare);

  expect(invokeMock.mock.calls, "C6.PREPARE.THE_HUMAN_CHOICE_IS_SENT: exactly one invocation").toHaveLength(1);
  expect(invokeMock.mock.calls[0], "C6.PREPARE.THE_HUMAN_CHOICE_IS_SENT: the held Mission and choice").toEqual([
    "prepare_outcome_candidate",
    { missionId: MISSION, appendixRequest: "include" },
  ]);
  expect(
    await screen.findByText("Outcome candidate prepared. Preparing does not evaluate or publish it."),
    "C6.PREPARE.SUCCESS_SENTENCE_IS_ABOUT_THE_ACTION",
  ).toBeInTheDocument();
});

const UNKNOWN_COPY =
  "Beacon could not confirm whether the candidate was prepared. No automatic retry was made. You may explicitly prepare again with the same appendix choice.";
const LIMIT_COPY = "The candidate is prepared, but this screen cannot display its document yet.";

async function chooseAndPrepare(user: ReturnType<typeof userEvent.setup>, label: RegExp) {
  await user.click(screen.getByRole("radio", { name: label }));
  await user.click(screen.getByRole("button", { name: "Prepare candidate" }));
}

it("C6.PREPARE.NOTHING_RUNS_ON_MOUNT: the neutral state is not a claim of absence", () => {
  render(<MissionOutcomePreparation missionId={MISSION} />);
  expect(invokeMock, "C6.PREPARE.NOTHING_RUNS_ON_MOUNT").not.toHaveBeenCalled();
  expect(screen.getByText("Preparation status is not loaded."), "C6.PREPARE.NOTHING_RUNS_ON_MOUNT")
    .toBeInTheDocument();
  expect(screen.queryByText(/no candidate/i), "C6.PREPARE.NOTHING_RUNS_ON_MOUNT: never 'no candidate'")
    .toBeNull();
});

it("C6.PREPARE.THE_RECEIPT_IS_HELD_IDENTITIES: nine fields, shown as identities and nothing more", async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValue(receipt());
  render(<MissionOutcomePreparation missionId={MISSION} />);
  await chooseAndPrepare(user, /Include verification appendix/);

  expect(await screen.findByText(LIMIT_COPY), "C6.PREPARE.THE_RECEIPT_IS_HELD_IDENTITIES").toBeInTheDocument();
  for (const shown of ["a".repeat(64), "b".repeat(64), "c".repeat(64), "66666666-6666-4666-8666-666666666666"]) {
    expect(screen.getByText(shown), `C6.PREPARE.THE_RECEIPT_IS_HELD_IDENTITIES: ${shown.slice(0, 8)}`).toBeInTheDocument();
  }
  expect(screen.getByText(/4096 bytes/), "C6.PREPARE.THE_RECEIPT_IS_HELD_IDENTITIES: the byte count").toBeInTheDocument();
  // No verdict, no completeness, no "current", and NO claim about evaluation or publication of the
  // candidate itself: an idempotent repeat can return a candidate that is already published.
  for (const forbidden of [/not evaluated/i, /not published/i, /ready to publish/i, /\bcurrent\b/i, /success/i]) {
    expect(screen.queryByText(forbidden), `C6.PREPARE.NO_GLOBAL_STATE_CLAIM: ${forbidden}`).toBeNull();
  }
});

it("C6.PREPARE.THE_MODE_IS_THE_OWNERS: a policy-default request may still resolve to included", async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValue(receipt({ appendixRequest: "policy_default", appendixMode: "included" }));
  render(<MissionOutcomePreparation missionId={MISSION} />);
  await chooseAndPrepare(user, /Use Policy default/);

  expect(
    await screen.findByText(/Policy default — the verification appendix is included\./),
    "C6.PREPARE.THE_MODE_IS_THE_OWNERS: the request and the resolved mode are two facts",
  ).toBeInTheDocument();
});

it("C6.PREPARE.AN_INCOHERENT_REPLY_IS_UNCONFIRMED: mismatch and shape faults never become absence", async () => {
  const faults: readonly (readonly [string, unknown])[] = [
    ["another Mission", receipt({ missionId: "88888888-8888-4888-8888-888888888888" })],
    ["another appendix request", receipt({ appendixRequest: "policy_default" })],
    ["include resolved to excluded", receipt({ appendixMode: "policy_state_not_high_impact" })],
    ["a foreign canonicalizer", receipt({ manifestDigest: { algorithm: "sha256", canonicalizer: "beacon.artifact.v1", value: "a".repeat(64) } })],
    ["an oversized byte count", receipt({ candidateContentBytes: 1_048_577 })],
    ["a tenth key", { ...receipt(), isPublished: false }],
    ["a missing key", (() => { const { mediaType: _unused, ...rest } = receipt(); return rest; })()],
    ["the wrong media type", receipt({ mediaType: "text/plain" })],
  ];

  for (const [label, reply] of faults) {
    cleanup();
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(reply);
    const user = userEvent.setup();
    render(<MissionOutcomePreparation missionId={MISSION} />);
    await chooseAndPrepare(user, /Include verification appendix/);

    expect(await screen.findByText(UNKNOWN_COPY), `C6.PREPARE.AN_INCOHERENT_REPLY_IS_UNCONFIRMED: ${label}`)
      .toBeInTheDocument();
    expect(screen.queryByText(LIMIT_COPY), `C6.PREPARE.AN_INCOHERENT_REPLY_IS_UNCONFIRMED: ${label} showed a receipt`).toBeNull();
    // The client's own diagnostic is never the Operator's sentence.
    expect(
      screen.queryByText(/Beacon received an invalid Outcome candidate\./),
      `C6.PREPARE.NO_RAW_DIAGNOSTIC: ${label}`,
    ).toBeNull();
  }
});

it("C6.PREPARE.THE_THREE_SEQUENCING_REFUSALS_ARE_DEFINITE: each says its own fixed sentence", async () => {
  const cases: readonly (readonly [string, string])[] = [
    ["outcome_not_completed", "This Mission has no recorded completion to prepare."],
    ["outcome_not_classified", "This Mission has no impact classification."],
    [
      "outcome_no_classification_at_completion",
      "This Mission was classified only after completion. This completion cannot be prepared as an Outcome, and another classification will not change its boundary.",
    ],
  ];
  for (const [code, sentence] of cases) {
    cleanup();
    invokeMock.mockReset();
    invokeMock.mockRejectedValue({ code });
    const user = userEvent.setup();
    render(<MissionOutcomePreparation missionId={MISSION} />);
    await chooseAndPrepare(user, /Include verification appendix/);

    expect(await screen.findByText(sentence), `C6.PREPARE.THE_THREE_SEQUENCING_REFUSALS_ARE_DEFINITE: ${code}`)
      .toBeInTheDocument();
    // Definite: it happened before any write, so it must NOT offer the unknown recovery.
    expect(screen.queryByText(UNKNOWN_COPY), `C6.PREPARE.A_DEFINITE_REFUSAL_IS_NOT_UNKNOWN: ${code}`).toBeNull();
    expect(
      screen.queryByRole("button", { name: "Prepare again with the same choice" }),
      `C6.PREPARE.A_DEFINITE_REFUSAL_IS_NOT_UNKNOWN: ${code}`,
    ).toBeNull();
  }
});

it("C6.PREPARE.UNAVAILABLE_STAYS_UNKNOWN: an explicit repeat sends the same intent, once per click", async () => {
  const user = userEvent.setup();
  invokeMock.mockRejectedValue({ code: "outcome_preparation_unavailable" });
  render(<MissionOutcomePreparation missionId={MISSION} />);
  await chooseAndPrepare(user, /Include verification appendix/);

  expect(
    await screen.findByText("Beacon could not verify the records needed to prepare this Outcome candidate."),
    "C6.PREPARE.UNAVAILABLE_STAYS_UNKNOWN: the code's own sentence",
  ).toBeInTheDocument();
  expect(screen.getByText(UNKNOWN_COPY), "C6.PREPARE.UNAVAILABLE_STAYS_UNKNOWN: and the unknown outcome").toBeInTheDocument();
  expect(invokeMock.mock.calls, "C6.PREPARE.NO_AUTOMATIC_WRITE").toHaveLength(1);

  await user.click(screen.getByRole("button", { name: "Prepare again with the same choice" }));
  expect(invokeMock.mock.calls, "C6.PREPARE.ONE_INVOCATION_PER_CLICK").toHaveLength(2);
  expect(invokeMock.mock.calls[1], "C6.PREPARE.THE_REPEAT_IS_THE_SAME_INTENT").toEqual(invokeMock.mock.calls[0]);
});

it("C6.PREPARE.A_CHANGED_CHOICE_IS_A_NEW_INTENT: old feedback does not survive it", async () => {
  const user = userEvent.setup();
  invokeMock.mockRejectedValue({ code: "outcome_preparation_unavailable" });
  render(<MissionOutcomePreparation missionId={MISSION} />);
  await chooseAndPrepare(user, /Include verification appendix/);
  expect(await screen.findByText(UNKNOWN_COPY), "C6.PREPARE.A_CHANGED_CHOICE_IS_A_NEW_INTENT control").toBeInTheDocument();

  await user.click(screen.getByRole("radio", { name: /Use Policy default/ }));
  expect(screen.queryByText(UNKNOWN_COPY), "C6.PREPARE.A_CHANGED_CHOICE_IS_A_NEW_INTENT").toBeNull();
  expect(
    screen.getByText("Preparation status is not loaded."),
    "C6.PREPARE.A_CHANGED_CHOICE_IS_A_NEW_INTENT: back to neutral",
  ).toBeInTheDocument();
  expect(invokeMock.mock.calls, "C6.PREPARE.A_CHANGED_CHOICE_INVOKES_NOTHING").toHaveLength(1);
});

it("C6.PREPARE.PENDING_BLOCKS_A_SECOND_CLICK: one attempt per press", async () => {
  const user = userEvent.setup();
  let release: ((value: unknown) => void) | undefined;
  invokeMock.mockImplementation(() => new Promise((resolve) => { release = resolve; }));
  render(<MissionOutcomePreparation missionId={MISSION} />);
  await chooseAndPrepare(user, /Include verification appendix/);

  const button = screen.getByRole("button", { name: "Prepare candidate" });
  expect(button, "C6.PREPARE.PENDING_BLOCKS_A_SECOND_CLICK").toBeDisabled();
  expect(screen.getByText("Preparing Outcome candidate…"), "C6.PREPARE.PENDING_IS_VISIBLE").toBeInTheDocument();
  await user.click(button);
  expect(invokeMock.mock.calls, "C6.PREPARE.PENDING_BLOCKS_A_SECOND_CLICK").toHaveLength(1);

  await act(async () => {
    release?.(receipt());
    await Promise.resolve();
  });
  expect(await screen.findByText(LIMIT_COPY), "C6.PREPARE.PENDING_RESOLVES").toBeInTheDocument();
});

it("C6.PREPARE.A_STALE_REPLY_NEVER_LANDS: Mission A to B to A retires the first attempt", async () => {
  const user = userEvent.setup();
  const other = "77777777-7777-4777-8777-777777777777";
  let release: ((value: unknown) => void) | undefined;
  invokeMock.mockImplementation(() => new Promise((resolve) => { release = resolve; }));
  const view = render(<MissionOutcomePreparation missionId={MISSION} />);
  await chooseAndPrepare(user, /Include verification appendix/);

  view.rerender(<MissionOutcomePreparation missionId={other} />);
  view.rerender(<MissionOutcomePreparation missionId={MISSION} />);

  await act(async () => {
    release?.(receipt());
    await Promise.resolve();
  });
  expect(screen.queryByText(LIMIT_COPY), "C6.PREPARE.A_STALE_REPLY_NEVER_LANDS").toBeNull();
  expect(
    screen.getByText("Preparation status is not loaded."),
    "C6.PREPARE.A_STALE_REPLY_NEVER_LANDS: the Mission that came back inherits nothing",
  ).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Prepare candidate" }), "C6.PREPARE.A_STALE_REPLY_NEVER_LANDS").toBeDisabled();
});

it("C6.PREPARE.UNMOUNT_RETIRES_THE_ATTEMPT: a reply after unmount changes nothing", async () => {
  const user = userEvent.setup();
  let release: ((value: unknown) => void) | undefined;
  invokeMock.mockImplementation(() => new Promise((resolve) => { release = resolve; }));
  const view = render(<MissionOutcomePreparation missionId={MISSION} />);
  await chooseAndPrepare(user, /Include verification appendix/);
  view.unmount();

  await act(async () => {
    release?.(receipt());
    await Promise.resolve();
  });
  expect(screen.queryByText(LIMIT_COPY), "C6.PREPARE.UNMOUNT_RETIRES_THE_ATTEMPT").toBeNull();
});

it("C6.PREPARE.KEYBOARD_REACHES_THE_CHOICE_AND_THE_ACTION", async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValue(receipt());
  render(<MissionOutcomePreparation missionId={MISSION} />);

  await user.tab();
  const first = screen.getByRole("radio", { name: /Use Policy default/ });
  expect(first, "C6.PREPARE.KEYBOARD_REACHES_THE_CHOICE_AND_THE_ACTION: the group takes focus").toHaveFocus();
  await user.keyboard("{ArrowDown}");
  expect(
    screen.getByRole("radio", { name: /Include verification appendix/ }),
    "C6.PREPARE.KEYBOARD_REACHES_THE_CHOICE_AND_THE_ACTION: arrow keys move within the group",
  ).toBeChecked();
  await user.tab();
  const action = screen.getByRole("button", { name: "Prepare candidate" });
  expect(action, "C6.PREPARE.KEYBOARD_REACHES_THE_CHOICE_AND_THE_ACTION: the action is next").toHaveFocus();
  await user.keyboard("{Enter}");
  expect(invokeMock.mock.calls, "C6.PREPARE.KEYBOARD_REACHES_THE_CHOICE_AND_THE_ACTION: Enter prepares").toHaveLength(1);
});

it("C6.PREPARE.THE_SURFACE_MEETS_WCAG_2_2", async () => {
  invokeMock.mockResolvedValue(receipt());
  const user = userEvent.setup();
  render(<MissionOutcomePreparation missionId={MISSION} />);
  await chooseAndPrepare(user, /Include verification appendix/);
  await screen.findByText(LIMIT_COPY);
  await expectNoWcag22Violations(document.body);
});
