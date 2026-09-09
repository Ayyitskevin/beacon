import { invoke } from "@tauri-apps/api/core";
import { act, fireEvent, render, screen } from "@testing-library/react";
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

it("astra: unavailable recovery provides an actual read-only retry", async () => {
  const user = userEvent.setup();
  let writes=0; let reads=0;
  await recordWith(user, {
    preview_mission_completion: args => Promise.resolve(previewFor((args as {request:unknown}).request)),
    record_mission_completion: () => { writes++; return Promise.reject({code:"completion_unavailable"}); },
    get_mission_completion_intent: () => { reads++; return reads === 1 ? Promise.reject({code:"completion_unavailable"}) : Promise.resolve(null); },
  });
  await screen.findByText("The recorded completion could not be read. Retry the read.");
  const retry=screen.queryByRole("button", {name:/retry.*read|read.*retry/i});
  expect(retry, "C4.RECOVERY.READ_RETRY_IS_AN_ACTION").not.toBeNull();
  await user.click(retry!);
  expect(reads, "C4.RECOVERY.RETRY_READS").toBe(2);
  expect(writes, "C4.RECOVERY.RETRY_NEVER_WRITES").toBe(1);
});

/* This project's own rule (`test/axe.test.tsx`): a file that renders a surface runs the WCAG bar.
   Root's adopted probes import the helper without calling it, so this is the one call that keeps the
   derived surface-coverage bar honest rather than excluded. The transient completion states are
   witnessed in `MissionCompletionCompose.test.tsx`. */
it("the composed surface in this file meets WCAG 2.1 AA", async () => {
  invokeMock.mockImplementation(() => new Promise<unknown>(() => {}));
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await expectNoWcagViolations(document.body);
});
