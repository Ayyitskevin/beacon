import { invoke } from "@tauri-apps/api/core";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";

import { ProjectMissions } from "./ProjectMissions";
import { expectNoWcagViolations } from "./test/axe";
import type { MissionView, ProjectView, TeamView } from "./session";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const workspaceId = "11111111-1111-4111-8111-111111111111";
const projectId = "33333333-3333-4333-8333-333333333333";
const teamId = "55555555-5555-4555-8555-555555555555";
const residentId = "22222222-2222-4222-8222-222222222222";
const configurationId = "77777777-7777-4777-8777-777777777777";
const otherTeamId = "99999999-9999-4999-8999-999999999999";

// The Project charter carries DIFFERENT constraints and prohibitions from the Mission, so a card
// that rendered the charter's lists instead of the Mission's would fail rather than coincide.
const project: ProjectView = {
  id: projectId,
  workspaceId,
  name: "Beacon Alpha",
  charter: {
    revision: 2,
    purpose: "Build a trustworthy local software product.",
    desiredOutcomes: ["Ship an attributable Mission"],
    constraints: ["CHARTER constraint, not the Mission's"],
    prohibitions: ["CHARTER prohibition, not the Mission's"],
    autonomyProfile: "supervised",
    budgetBoundary: "no_spend",
    externalEffects: "prohibited",
    authoredAtMs: 123,
  },
  createdAtMs: 120,
} as unknown as ProjectView;

const team: TeamView = {
  id: teamId,
  workspaceId,
  projectId,
  name: "Alpha Team",
  formedBy: "operator",
  formedAtMs: 125,
  members: [
    {
      residentId,
      formationConfigurationId: configurationId,
      currentConfigurationId: configurationId,
      eligibleNow: true,
      ineligibilityReasons: [],
    },
  ],
} as unknown as TeamView;

const fullMission: MissionView = {
  id: "88888888-8888-4888-8888-888888888888",
  workspaceId,
  projectId,
  charterRevision: 2,
  teamId,
  desiredOutcome: "Ship one reviewed Beacon feature",
  successConditions: ["FIRST condition authored", "SECOND condition authored"],
  constraints: ["FIRST constraint authored", "SECOND constraint authored"],
  prohibitions: ["FIRST prohibition authored", "SECOND prohibition authored"],
  lifecycle: "admitted",
  admittedBy: "operator",
  admittedAtMs: 126,
  members: [{ residentId, admissionConfigurationId: configurationId }],
} as unknown as MissionView;

const bareMission: MissionView = {
  ...fullMission,
  id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
  teamId: otherTeamId,
  desiredOutcome: "Second Mission on this Project",
  successConditions: ["Only condition"],
  constraints: [],
  prohibitions: [],
} as unknown as MissionView;

function renderMissions(missions: MissionView[], teams: TeamView[] | null = [team]) {
  return render(
    <ProjectMissions
      readFailure=""
      onRetryRead={vi.fn()}
      project={project}
      teams={teams}
      missions={missions}
      linkedMissionId={null}
      composerOpen={false}
      busy={false}
      onOpenComposer={() => {}}
      onCancel={() => {}}
      onSubmit={() => {}}
      onCopyObjectLink={() => {}}
    />,
  );
}

const cardFor = (outcome: string): HTMLElement => {
  const heading = screen.getByText(outcome);
  const card = heading.closest("article");
  if (card === null) throw new Error(`no Mission card for ${outcome}`);
  return card;
};

const textsOf = (list: HTMLElement): string[] =>
  within(list)
    .getAllByRole("listitem")
    .map((item) => item.textContent?.trim() ?? "");

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockImplementation((command) => {
    if (command === "mission_publications") return Promise.resolve([]);
    return Promise.reject(new Error(`unexpected command: ${String(command)}`));
  });
});

it("shows the exact saved success conditions, constraints and prohibitions in authored order", () => {
  renderMissions([fullMission]);
  const card = within(cardFor(fullMission.desiredOutcome));

  expect(
    textsOf(card.getByRole("list", { name: "Success conditions" })),
    "B9.MISSION.SAVED_LISTS_ARE_EXACT",
  ).toEqual([...fullMission.successConditions]);
  expect(
    textsOf(card.getByRole("list", { name: "Mission constraints" })),
    "B9.MISSION.SAVED_LISTS_ARE_EXACT",
  ).toEqual([...fullMission.constraints]);
  expect(
    textsOf(card.getByRole("list", { name: "Mission prohibitions" })),
    "B9.MISSION.SAVED_LISTS_ARE_EXACT",
  ).toEqual([...fullMission.prohibitions]);

  expect(
    card.queryByText(/CHARTER constraint/),
    "B9.MISSION.NO_CHARTER_SUBSTITUTION",
  ).not.toBeInTheDocument();
  expect(
    card.queryByText(/CHARTER prohibition/),
    "B9.MISSION.NO_CHARTER_SUBSTITUTION",
  ).not.toBeInTheDocument();
});

it("says None recorded for empty constraints and prohibitions instead of inventing them", () => {
  renderMissions([bareMission]);
  const card = within(cardFor(bareMission.desiredOutcome));

  expect(
    textsOf(card.getByRole("list", { name: "Success conditions" })),
    "B9.MISSION.EMPTY_IS_HONEST",
  ).toEqual(["Only condition"]);
  expect(
    card.queryByRole("list", { name: "Mission constraints" }),
    "B9.MISSION.EMPTY_IS_HONEST",
  ).not.toBeInTheDocument();
  expect(
    card.getAllByText("None recorded.").length,
    "B9.MISSION.EMPTY_IS_HONEST",
  ).toBe(2);
});

it("retains the admitted Team and member identities, naming a Team only on an exact id match", async () => {
  const user = userEvent.setup();
  renderMissions([fullMission, bareMission]);

  const matched = within(cardFor(fullMission.desiredOutcome));
  await user.click(matched.getByText("Recorded identities"));
  expect(matched.getByText(/Alpha Team/), "B9.MISSION.TEAM_NAME_ONLY_ON_EXACT_MATCH").toBeVisible();
  expect(matched.getByText(new RegExp(teamId)), "B9.MISSION.IDENTITIES_RETAINED").toBeVisible();
  expect(matched.getByText(new RegExp(residentId)), "B9.MISSION.IDENTITIES_RETAINED").toBeVisible();
  expect(
    matched.getByText(new RegExp(configurationId)),
    "B9.MISSION.IDENTITIES_RETAINED",
  ).toBeVisible();
  // Scoped to the lifecycle line rather than the whole card: a page-wide regex would also read a
  // rendered published Outcome document's own words as a lifecycle claim, which they are not.
  const lifecycle = matched.getByText(/^Lifecycle:/);
  expect(lifecycle, "B9.MISSION.LIFECYCLE_NOT_RUNNING").toHaveTextContent("Lifecycle: Admitted.");
  expect(
    /running|in progress|complete|started no work|unstarted/i.test(lifecycle.textContent ?? ""),
    "B9.MISSION.LIFECYCLE_NOT_RUNNING",
  ).toBe(false);

  // No Team in the loaded list matches this Mission's recorded teamId. The id must survive, and
  // nothing may suggest the recorded binding itself is gone.
  const unmatched = within(cardFor(bareMission.desiredOutcome));
  await user.click(unmatched.getByText("Recorded identities"));
  expect(unmatched.getByText(new RegExp(otherTeamId)), "B9.MISSION.IDENTITIES_RETAINED").toBeVisible();
  expect(
    unmatched.queryByText(/Alpha Team/),
    "B9.MISSION.TEAM_NAME_ONLY_ON_EXACT_MATCH",
  ).not.toBeInTheDocument();
  expect(
    unmatched.queryByText(/deleted|removed|missing|no longer|unknown/i),
    "B9.MISSION.ABSENT_LOOKUP_IS_NOT_A_LOST_BINDING",
  ).not.toBeInTheDocument();
});

it("keeps every Mission's sections inside its own card and adds no duplicate element ids", () => {
  const { container } = renderMissions([fullMission, bareMission]);

  const full = within(cardFor(fullMission.desiredOutcome));
  const bare = within(cardFor(bareMission.desiredOutcome));
  expect(
    textsOf(full.getByRole("list", { name: "Success conditions" })),
    "B9.MISSION.SECTIONS_STAY_WITH_THEIR_CARD",
  ).toEqual([...fullMission.successConditions]);
  expect(
    textsOf(bare.getByRole("list", { name: "Success conditions" })),
    "B9.MISSION.SECTIONS_STAY_WITH_THEIR_CARD",
  ).toEqual(["Only condition"]);

  // This slice adds NO element ids, which is how it guarantees it introduces no duplicates across
  // repeated Mission cards. At the base of this slice, `MissionOutcomes` used a static heading id
  // once per card; the companion Outcome change now gives each section its own heading id. The
  // Outcome section is a SIBLING of the brief, so this witness scopes only the newly rendered brief.
  // The Outcome component's own witness verifies its heading association.
  //
  // The first assertion is the harness check. An earlier version filtered on
  // `closest("section[aria-labelledby]") === null`, which rejected every node -- the enclosing
  // ProjectMissions section matches it -- so the bar passed on an empty array no matter what was
  // rendered. A witness that cannot fail is not a witness.
  const briefRoots = Array.from(container.querySelectorAll('section[aria-label="Mission brief"]'));
  expect(briefRoots.length, "B9.MISSION.ID_WITNESS_SELECTS_SOMETHING").toBe(2);
  const briefIds = briefRoots.flatMap((root) => [
    ...(root.id === "" ? [] : [root.id]),
    ...Array.from(root.querySelectorAll("[id]")).map((node) => node.id),
  ]);
  expect(briefIds, "B9.MISSION.BRIEF_ADDS_NO_IDS").toEqual([]);
});

it("passes WCAG 2.1 AA with the saved brief on screen", async () => {
  const user = userEvent.setup();
  const { container } = renderMissions([fullMission, bareMission]);
  await user.click(within(cardFor(fullMission.desiredOutcome)).getByText("Recorded identities"));
  await expectNoWcagViolations(container);
});

it("does not infer unstarted work from a published Mission's admission lifecycle", async () => {
  invokeMock.mockImplementation((command) => {
    if (command === "mission_publications") return Promise.resolve([{
      publicationId: "44444444-4444-4444-8444-444444444444",
      publicationVersion: 1,
      candidateArtifactDigest: "a".repeat(64),
      evaluationDigest: "b".repeat(64),
      isCurrent: true,
    }]);
    if (command === "outcome_document") return Promise.resolve({
      bytes: Array.from(new TextEncoder().encode("# Recorded completed Mission\n")),
      digest: { algorithm: "sha256", canonicalizer: "beacon.artifact.v1", value: "a".repeat(64) },
      publicationVersion: 1,
      publicationVersionCount: 1,
      mediaType: "text/markdown; charset=utf-8",
    });
    return Promise.reject(new Error(`unexpected command: ${String(command)}`));
  });
  const user = userEvent.setup();
  renderMissions([fullMission]);
  expect(await screen.findByLabelText("Outcome Document")).toHaveTextContent("Recorded completed Mission");
  await user.click(screen.getByText("Recorded identities"));
  expect(screen.getByText(/Lifecycle: Admitted/)).toBeVisible();
  expect(screen.queryByText(/started no work/i), "B9.MISSION.ADMISSION_IS_NOT_WORK_STATE").not.toBeInTheDocument();
});
