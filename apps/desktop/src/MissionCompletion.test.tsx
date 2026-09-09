import { invoke } from "@tauri-apps/api/core";
import { act, render, screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";

import { MissionCompletion } from "./MissionCompletion";
import type { MissionView } from "./session";
import { expectNoWcagViolations } from "./test/axe";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

// The same instance kept mounted across a Mission change. Through `ProjectMissions` each Mission
// card is keyed by its own id, so leaving a Project UNMOUNTS the component and a late `setState` is
// a silent no-op -- which means the App-level bars witness the unmount path and NOT the lifetime
// guard. Re-rendering this component with a different `mission` prop is the path where the guard is
// the only thing standing between a stale answer and the current Mission's state.
const FIRST = "88888888-8888-4888-8888-8888888c3000";
const SECOND = "88888888-8888-4888-8888-8888888c3111";

function mission(id: string, outcome: string): MissionView {
  return {
    id,
    workspaceId: "11111111-1111-4111-8111-111111111111",
    projectId: "22222222-2222-4222-8222-222222222222",
    charterRevision: 1,
    teamId: "33333333-3333-4333-8333-333333333333",
    desiredOutcome: outcome,
    successConditions: ["Only condition"],
    constraints: [],
    prohibitions: [],
    lifecycle: "admitted",
    admittedBy: "operator",
    admittedAtMs: 126,
    members: [
      {
        residentId: "44444444-4444-4444-8444-444444444444",
        admissionConfigurationId: "55555555-5555-4555-8555-555555555555",
      },
    ],
  } as unknown as MissionView;
}

function completionFor(missionId: string) {
  return {
    missionId,
    completionId: "66666666-7777-4888-8999-aaaaaaaaaaaa",
    actor: "operator",
    accountableResidentId: "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff",
    accountableResidentConfigurationId: "cccccccc-dddd-4eee-8fff-000000000000",
    disposition: "concluded",
    conditionResults: [{ ordinal: 0, outcome: "met", supportCount: 0, note: null }],
    deliveredArtifactCount: 0,
    unfinishedWorkCount: 0,
  };
}

beforeEach(() => {
  invokeMock.mockReset();
});

it("renders the held stale payload when it IS the current Mission", async () => {
  // The control that makes the two bars below mean something: this exact payload renders, so its
  // absence there is the guard and not an invalid fixture.
  invokeMock.mockImplementation(() => Promise.resolve(completionFor(FIRST)));
  const { container } = render(<MissionCompletion mission={mission(FIRST, "First")} />);
  expect(await screen.findByText("Concluded"), "C3.UNIT.STALE_PAYLOAD_IS_VALID").toBeVisible();
  await expectNoWcagViolations(container);
});

it("a stale success for the previous Mission cannot replace or release the current read", async () => {
  let releaseFirst!: (value: unknown) => void;
  const first = new Promise<unknown>((resolve) => {
    releaseFirst = resolve;
  });
  const second = new Promise<unknown>(() => {});
  invokeMock.mockImplementation((_command, args) =>
    (args as { missionId: string }).missionId === FIRST ? first : second,
  );

  const view = render(<MissionCompletion mission={mission(FIRST, "First")} />);
  expect(screen.queryByText("Loading the Mission completion")).not.toBeNull();
  // SAME instance, different Mission: the effect cleans up and re-runs, and the component stays
  // mounted, so a late write would really land.
  view.rerender(<MissionCompletion mission={mission(SECOND, "Second")} />);

  await act(async () => {
    releaseFirst(completionFor(FIRST));
    await Promise.resolve();
    await Promise.resolve();
  });

  expect(screen.queryByText("Concluded"), "C3.UNIT.STALE_SUCCESS_DOES_NOT_REPLACE").toBeNull();
  expect(
    screen.queryByText("Loading the Mission completion"),
    "C3.UNIT.STALE_SUCCESS_DOES_NOT_RELEASE_PENDING",
  ).not.toBeNull();
});

it("a stale rejection for the previous Mission plants no failure on the current read", async () => {
  let rejectFirst!: (cause: unknown) => void;
  const first = new Promise<unknown>((_resolve, reject) => {
    rejectFirst = reject;
  });
  const second = new Promise<unknown>(() => {});
  invokeMock.mockImplementation((_command, args) =>
    (args as { missionId: string }).missionId === FIRST ? first : second,
  );

  const view = render(<MissionCompletion mission={mission(FIRST, "First")} />);
  view.rerender(<MissionCompletion mission={mission(SECOND, "Second")} />);

  await act(async () => {
    rejectFirst(new Error("the local request failed"));
    await Promise.resolve();
    await Promise.resolve();
  });

  expect(
    screen.queryByRole("button", { name: "Retry Mission completion" }),
    "C3.UNIT.STALE_REJECTION_PLANTS_NO_FAILURE",
  ).toBeNull();
  expect(
    screen.queryByText("Loading the Mission completion"),
    "C3.UNIT.STALE_REJECTION_DOES_NOT_RELEASE_PENDING",
  ).not.toBeNull();
});
