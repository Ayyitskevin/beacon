import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  admitMission,
  bindProjectRepository,
  preflightProjectChecks,
  recentReceipts,
  activateLocalModel,
  activateResident,
  clearResidentConfiguration,
  configureResident,
  beginLocalPasswordSetup,
  acceptStarterOrganization,
  createProject,
  deactivateLocalModel,
  discoverLocalModels,
  liveDataEncryptionStatus,
  failedReadReport,
  verbatimFailure,
  completeLocalOperatorSetup,
  describeLocalAccountError,
  discardStarterOrganization,
  formTeam,
  loadAuthenticationCapabilities,
  inspectLocalModel,
  loadLocalModelActivations,
  loadMission,
  loadMissions,
  loadPlatformSessionMonitorStatus,
  loadProject,
  loadProjectCharterHistory,
  loadProjectPolicy,
  loadProjectPolicyHistory,
  loadProjectRepository,
  loadProjectRepositoryHistory,
  loadProjects,
  loadResidents,
  loadResidentCapabilityCatalog,
  loadResidentConfigurations,
  loadSessionPolicy,
  loadSessionStatus,
  loadStarterOrganization,
  loadTeam,
  loadTeams,
  lockLocalSession,
  recordLocalOperatorActivity,
  recoverLocalOperatorAccount,
  publishProjectPolicy,
  type ProjectPolicyDraft,
  type RepositoryDraft,
  reactivateResident,
  retireResident,
  reviseProjectCharter,
  setLocalSessionTimeout,
  signInWithLocalPassword,
  suspendResident,
} from "./session";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

describe("desktop session transport", () => {
  beforeEach(() => invokeMock.mockReset());

  it.each([
    ["operator_setup_required", false, false],
    ["locked", false, false],
    ["unlocked", true, true],
  ] as const)("accepts the exact %s status shape", async (phase, workspaceAccess, operatorCommands) => {
    const exactStatus = { phase, workspaceAccess, operatorCommands };
    invokeMock.mockResolvedValue(exactStatus);

    await expect(loadSessionStatus()).resolves.toEqual(exactStatus);
    expect(invokeMock).toHaveBeenCalledWith("session_status");
  });

  it.each([
    ["mismatched setup access", { phase: "operator_setup_required", workspaceAccess: true, operatorCommands: false }],
    ["mismatched unlocked access", { phase: "unlocked", workspaceAccess: false, operatorCommands: true }],
    ["undeclared fields", { phase: "locked", workspaceAccess: false, operatorCommands: false, workspaceName: "Personal" }],
    ["a non-object response", null],
  ])("rejects %s rather than widening the session boundary", async (_case, response) => {
    invokeMock.mockResolvedValue(response);

    await expect(loadSessionStatus()).rejects.toThrow("invalid session status");
  });

  it("accepts only the closed authentication-capability shape", async () => {
    const capabilities = {
      preferredMethod: "passkey",
      passkey: { available: false, reason: "unsupported_linux_webkitgtk" },
      passwordFallback: { available: true, scope: "same_machine" },
    } as const;
    invokeMock.mockResolvedValue(capabilities);

    await expect(loadAuthenticationCapabilities()).resolves.toEqual(capabilities);
    expect(invokeMock).toHaveBeenCalledWith("authentication_capabilities");
  });

  it("sends secrets only in the command that consumes them and validates one-time output", async () => {
    invokeMock.mockResolvedValueOnce({ recoveryKey: "ABCD-EFGH-IJKL-MNOP-QRST-UVWX-YZ23-4567" });

    await expect(beginLocalPasswordSetup("one secret", "one secret")).resolves.toEqual({
      recoveryKey: "ABCD-EFGH-IJKL-MNOP-QRST-UVWX-YZ23-4567",
    });
    expect(invokeMock).toHaveBeenCalledWith("begin_password_setup", {
      password: "one secret",
      confirmation: "one secret",
    });
  });

  it("maps setup, sign-in, recovery, and lock to narrow local commands", async () => {
    const unlocked = { phase: "unlocked", workspaceAccess: true, operatorCommands: true };
    const locked = { phase: "locked", workspaceAccess: false, operatorCommands: false };
    invokeMock
      .mockResolvedValueOnce(unlocked)
      .mockResolvedValueOnce(unlocked)
      .mockResolvedValueOnce({ recoveryKey: "NEW-RECOVERY-KEY" })
      .mockResolvedValueOnce(locked);

    await expect(completeLocalOperatorSetup(true)).resolves.toEqual(unlocked);
    await expect(signInWithLocalPassword("local password")).resolves.toEqual(unlocked);
    await expect(
      recoverLocalOperatorAccount("old key", "new password", "new password"),
    ).resolves.toEqual({ recoveryKey: "NEW-RECOVERY-KEY" });
    await expect(lockLocalSession()).resolves.toEqual(locked);

    expect(invokeMock.mock.calls).toEqual([
      ["complete_operator_setup", { recoverySaved: true }],
      ["authenticate_password", { password: "local password" }],
      [
        "recover_operator_account",
        { recoveryKey: "old key", newPassword: "new password", confirmation: "new password" },
      ],
      ["lock_session"],
    ]);
  });

  it("validates session policy and platform monitor responses exactly", async () => {
    invokeMock
      .mockResolvedValueOnce({ inactivityTimeoutMinutes: 30 })
      .mockResolvedValueOnce({ inactivityTimeoutMinutes: 90 })
      .mockResolvedValueOnce({ screenLock: "active", suspend: "unavailable" })
      .mockResolvedValueOnce(null);

    await expect(loadSessionPolicy()).resolves.toEqual({ inactivityTimeoutMinutes: 30 });
    await expect(setLocalSessionTimeout(90)).resolves.toEqual({ inactivityTimeoutMinutes: 90 });
    await expect(loadPlatformSessionMonitorStatus()).resolves.toEqual({
      screenLock: "active",
      suspend: "unavailable",
    });
    await expect(recordLocalOperatorActivity()).resolves.toBeUndefined();
    expect(invokeMock.mock.calls).toEqual([
      ["session_policy"],
      ["set_session_timeout", { inactivityTimeoutMinutes: 90 }],
      ["platform_session_monitor_status"],
      ["record_operator_activity"],
    ]);
  });

  it("maps the Starter Organization commands and rejects widened roster results", async () => {
    const proposed = {
      workspaceId: "11111111-1111-4111-8111-111111111111",
      proposalVersion: 1,
      decision: "proposed",
      templates: [
        { role: "lead", description: "Coordinates Missions.", recommended: true },
        { role: "builder", description: "Produces Artifacts.", recommended: true },
        { role: "reviewer", description: "Evaluates work.", recommended: true },
        { role: "researcher", description: "Develops Evidence.", recommended: false },
      ],
      residents: [],
    } as const;
    const accepted = {
      ...proposed,
      decision: "accepted",
      residents: [
        {
          id: "22222222-2222-4222-8222-222222222222",
          role: "lead",
          displayName: "Northstar",
          lifecycle: "inactive",
        },
      ],
    } as const;
    const discarded = { ...proposed, decision: "discarded" } as const;
    invokeMock
      .mockResolvedValueOnce(proposed)
      .mockResolvedValueOnce(accepted)
      .mockResolvedValueOnce(discarded)
      .mockResolvedValueOnce({ ...proposed, authority: "ambient" });

    await expect(loadStarterOrganization()).resolves.toEqual(proposed);
    await expect(
      acceptStarterOrganization(1, [{ role: "lead", displayName: "Northstar" }]),
    ).resolves.toEqual(accepted);
    await expect(discardStarterOrganization(1)).resolves.toEqual(discarded);
    await expect(loadStarterOrganization()).rejects.toThrow("invalid Starter Organization");
    expect(invokeMock.mock.calls).toEqual([
      ["starter_organization"],
      [
        "accept_starter_organization",
        { proposalVersion: 1, residents: [{ role: "lead", displayName: "Northstar" }] },
      ],
      ["discard_starter_organization", { proposalVersion: 1 }],
      ["starter_organization"],
    ]);
  });

  it("maps the closed Resident lifecycle without accepting authority fields", async () => {
    const inactive = {
      id: "22222222-2222-4222-8222-222222222222",
      workspaceId: "11111111-1111-4111-8111-111111111111",
      role: "lead",
      displayName: "Northstar",
      lifecycle: "inactive",
      latestDecisionId: null,
      createdAtMs: 100,
      updatedAtMs: 100,
    } as const;
    const active = {
      ...inactive,
      lifecycle: "active",
      latestDecisionId: "33333333-3333-4333-8333-333333333333",
      updatedAtMs: 110,
    } as const;
    const suspended = {
      ...active,
      lifecycle: "suspended",
      latestDecisionId: "44444444-4444-4444-8444-444444444444",
      updatedAtMs: 120,
    } as const;
    const reactivated = {
      ...active,
      latestDecisionId: "55555555-5555-4555-8555-555555555555",
      updatedAtMs: 130,
    } as const;
    const retired = {
      ...reactivated,
      lifecycle: "retired",
      latestDecisionId: "66666666-6666-4666-8666-666666666666",
      updatedAtMs: 140,
    } as const;
    invokeMock
      .mockResolvedValueOnce([inactive])
      .mockResolvedValueOnce(active)
      .mockResolvedValueOnce(suspended)
      .mockResolvedValueOnce(reactivated)
      .mockResolvedValueOnce(retired)
      .mockResolvedValueOnce([{ ...retired, authority: "ambient" }])
      .mockResolvedValueOnce({ ...inactive, latestDecisionId: active.latestDecisionId });

    await expect(loadResidents()).resolves.toEqual([inactive]);
    await expect(activateResident(inactive.id, null)).resolves.toEqual(active);
    await expect(suspendResident(active.id, active.latestDecisionId)).resolves.toEqual(suspended);
    await expect(
      reactivateResident(suspended.id, suspended.latestDecisionId),
    ).resolves.toEqual(reactivated);
    await expect(retireResident(reactivated.id, reactivated.latestDecisionId)).resolves.toEqual(
      retired,
    );
    await expect(loadResidents()).rejects.toThrow("invalid Resident roster");
    await expect(activateResident(inactive.id, null)).rejects.toThrow(
      "invalid Resident lifecycle response",
    );
    expect(invokeMock.mock.calls).toEqual([
      ["residents"],
      ["activate_resident", { residentId: inactive.id, expectedDecisionId: null }],
      [
        "suspend_resident",
        { residentId: active.id, expectedDecisionId: active.latestDecisionId },
      ],
      [
        "reactivate_resident",
        { residentId: suspended.id, expectedDecisionId: suspended.latestDecisionId },
      ],
      [
        "retire_resident",
        { residentId: reactivated.id, expectedDecisionId: reactivated.latestDecisionId },
      ],
      ["residents"],
      ["activate_resident", { residentId: inactive.id, expectedDecisionId: null }],
    ]);
  });

  it("maps Project commands and accepts only the closed Charter shape", async () => {
    const project = {
      id: "33333333-3333-4333-8333-333333333333",
      workspaceId: "11111111-1111-4111-8111-111111111111",
      name: "Beacon Alpha",
      charter: {
        revision: 1,
        purpose: "Build a trustworthy local software product.",
        desiredOutcomes: ["Ship a verified vertical slice"],
        constraints: ["Keep Workspace data local"],
        prohibitions: ["Do not deploy publicly"],
        autonomyProfile: "supervised",
        budgetBoundary: "no_spend",
        externalEffects: "prohibited",
        authoredAtMs: 123,
      },
      createdAtMs: 123,
    } as const;
    const summary = {
      id: project.id,
      name: project.name,
      charterRevision: 1,
      autonomyProfile: "supervised",
      createdAtMs: 123,
    } as const;
    const draft = {
      name: project.name,
      charter: {
        purpose: project.charter.purpose,
        desiredOutcomes: [...project.charter.desiredOutcomes],
        constraints: [...project.charter.constraints],
        prohibitions: [...project.charter.prohibitions],
        autonomyProfile: "supervised" as const,
      },
    };
    invokeMock
      .mockResolvedValueOnce([summary])
      .mockResolvedValueOnce(project)
      .mockResolvedValueOnce(project)
      .mockResolvedValueOnce([project.charter])
      .mockResolvedValueOnce({
        ...project,
        charter: { ...project.charter, revision: 2, purpose: "Revised purpose" },
      })
      .mockResolvedValueOnce([{ ...project.charter, revision: 2 }])
      .mockResolvedValueOnce({ ...project, executionAuthority: "ambient" });

    await expect(loadProjects()).resolves.toEqual([summary]);
    await expect(loadProject(project.id)).resolves.toEqual(project);
    await expect(createProject(draft)).resolves.toEqual(project);
    await expect(loadProjectCharterHistory(project.id)).resolves.toEqual([project.charter]);
    await expect(
      reviseProjectCharter(project.id, 1, {
        ...draft.charter,
        purpose: "Revised purpose",
      }),
    ).resolves.toMatchObject({ charter: { revision: 2, purpose: "Revised purpose" } });
    await expect(loadProjectCharterHistory(project.id)).rejects.toThrow(
      "invalid Project Charter history",
    );
    await expect(loadProject(project.id)).rejects.toThrow("invalid Project");
    expect(invokeMock.mock.calls).toEqual([
      ["projects"],
      ["project", { projectId: project.id }],
      ["create_project", { draft }],
      ["project_charter_history", { projectId: project.id }],
      [
        "revise_project_charter",
        {
          projectId: project.id,
          expectedCurrentRevision: 1,
          draft: { ...draft.charter, purpose: "Revised purpose" },
        },
      ],
      ["project_charter_history", { projectId: project.id }],
      ["project", { projectId: project.id }],
    ]);
  });

  it("maps Project Policy and rejects widened, unordered, or incomplete criteria", async () => {
    const policy = {
      workspaceId: "11111111-1111-4111-8111-111111111111",
      projectId: "33333333-3333-4333-8333-333333333333",
      revision: 1,
      charterRevision: 2,
      teamLeadSelection: {
        requiredCapabilities: ["work.coordinate_mission", "evidence.develop"],
        availabilityRule: "current_team_eligibility",
        trackRecordRequirements: [
          { evidenceKind: "mission_outcome", minimumCount: 1 },
          { evidenceKind: "evaluation", minimumCount: 2 },
        ],
        conflictRule: "require_no_declared_conflict",
        ambiguityRule: "require_operator_decision",
      },
      authoredBy: "operator",
      authoredAtMs: 987,
      appliesToCurrentCharter: true,
    } as const;
    const draft: ProjectPolicyDraft = {
      teamLeadSelection: {
        ...policy.teamLeadSelection,
        requiredCapabilities: [...policy.teamLeadSelection.requiredCapabilities],
        trackRecordRequirements: policy.teamLeadSelection.trackRecordRequirements.map(
          (requirement) => ({ ...requirement }),
        ),
      },
    };
    invokeMock
      .mockResolvedValueOnce(policy)
      .mockResolvedValueOnce([policy])
      .mockResolvedValueOnce(policy)
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce({ ...policy, leadResidentId: "22222222-2222-4222-8222-222222222222" })
      .mockResolvedValueOnce({
        ...policy,
        teamLeadSelection: {
          ...policy.teamLeadSelection,
          requiredCapabilities: ["evidence.develop"],
        },
      })
      .mockResolvedValueOnce({
        ...policy,
        teamLeadSelection: {
          ...policy.teamLeadSelection,
          trackRecordRequirements: [
            { evidenceKind: "mission_outcome", minimumCount: 1 },
            { evidenceKind: "mission_outcome", minimumCount: 2 },
          ],
        },
      });

    await expect(loadProjectPolicy(policy.projectId)).resolves.toEqual(policy);
    await expect(loadProjectPolicyHistory(policy.projectId)).resolves.toEqual([policy]);
    await expect(
      publishProjectPolicy(policy.projectId, policy.charterRevision, null, draft),
    ).resolves.toEqual(policy);
    await expect(loadProjectPolicy(policy.projectId)).resolves.toBeNull();
    await expect(loadProjectPolicy(policy.projectId)).rejects.toThrow("invalid Project Policy");
    await expect(loadProjectPolicy(policy.projectId)).rejects.toThrow("invalid Project Policy");
    await expect(loadProjectPolicy(policy.projectId)).rejects.toThrow("invalid Project Policy");
    expect(invokeMock.mock.calls).toEqual([
      ["project_policy", { projectId: policy.projectId }],
      ["project_policy_history", { projectId: policy.projectId }],
      [
        "publish_project_policy",
        {
          projectId: policy.projectId,
          expectedCharterRevision: policy.charterRevision,
          expectedPolicyRevision: null,
          draft,
        },
      ],
      ["project_policy", { projectId: policy.projectId }],
      ["project_policy", { projectId: policy.projectId }],
      ["project_policy", { projectId: policy.projectId }],
      ["project_policy", { projectId: policy.projectId }],
    ]);
  });

  it("maps Project Repository bindings and rejects widened or unordered declarations", async () => {
    const repository = {
      workspaceId: "11111111-1111-4111-8111-111111111111",
      projectId: "33333333-3333-4333-8333-333333333333",
      revision: 1,
      charterRevision: 2,
      sourceKind: "local_path",
      source: "/home/operator/projects/sample-tracker",
      baseRevision: "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
      declaredChecks: [
        { name: "pytest", argv: ["pytest", "tests/test_invariants.py", "-q"], workingDirectory: "." },
        { name: "ruff", argv: ["ruff", "check", "."], workingDirectory: "." },
      ],
      authorKind: "operator",
      authoredAtMs: 987,
      appliesToCurrentCharter: true,
    } as const;
    const draft: RepositoryDraft = {
      sourceKind: repository.sourceKind,
      source: repository.source,
      baseRevision: repository.baseRevision,
      declaredChecks: repository.declaredChecks.map((check) => ({
        ...check,
        argv: [...check.argv],
      })),
    };
    invokeMock
      .mockResolvedValueOnce(repository)
      .mockResolvedValueOnce([repository])
      .mockResolvedValueOnce(repository)
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce({ ...repository, worktreePath: "/tmp/beacon-worktree" })
      .mockResolvedValueOnce({ ...repository, baseRevision: "0123456" })
      .mockResolvedValueOnce({
        ...repository,
        declaredChecks: [repository.declaredChecks[1], repository.declaredChecks[0]],
      })
      .mockResolvedValueOnce({
        ...repository,
        declaredChecks: [
          { name: "pytest", argv: [], workingDirectory: "." },
        ],
      })
      .mockResolvedValueOnce({
        ...repository,
        declaredChecks: [
          { name: "pytest", argv: ["pytest"], workingDirectory: "../outside" },
        ],
      });

    await expect(loadProjectRepository(repository.projectId)).resolves.toEqual(repository);
    await expect(loadProjectRepositoryHistory(repository.projectId)).resolves.toEqual([repository]);
    await expect(
      bindProjectRepository(repository.projectId, repository.charterRevision, null, draft),
    ).resolves.toEqual(repository);
    await expect(loadProjectRepository(repository.projectId)).resolves.toBeNull();
    for (let attempt = 0; attempt < 5; attempt += 1) {
      await expect(loadProjectRepository(repository.projectId)).rejects.toThrow(
        "invalid Project Repository",
      );
    }
    expect(invokeMock.mock.calls).toEqual([
      ["current_project_repository", { projectId: repository.projectId }],
      ["project_repository_history", { projectId: repository.projectId }],
      [
        "bind_project_repository",
        {
          projectId: repository.projectId,
          expectedCharterRevision: repository.charterRevision,
          expectedPriorRepositoryRevision: null,
          draft,
        },
      ],
      ["current_project_repository", { projectId: repository.projectId }],
      ["current_project_repository", { projectId: repository.projectId }],
      ["current_project_repository", { projectId: repository.projectId }],
      ["current_project_repository", { projectId: repository.projectId }],
      ["current_project_repository", { projectId: repository.projectId }],
      ["current_project_repository", { projectId: repository.projectId }],
    ]);
  });

  it("maps Team commands and rejects authority beyond immutable formation evidence", async () => {
    const team = {
      id: "55555555-5555-4555-8555-555555555555",
      workspaceId: "11111111-1111-4111-8111-111111111111",
      projectId: "33333333-3333-4333-8333-333333333333",
      name: "Alpha Team",
      formedBy: "operator",
      formedAtMs: 456,
      members: [
        {
          residentId: "22222222-2222-4222-8222-222222222222",
          formationConfigurationId: "77777777-7777-4777-8777-777777777777",
          currentConfigurationId: "77777777-7777-4777-8777-777777777777",
          eligibleNow: true,
          ineligibilityReasons: [],
        },
      ],
    } as const;
    const draft = {
      projectId: team.projectId,
      name: team.name,
      members: [
        {
          residentId: team.members[0].residentId,
          expectedConfigurationId: team.members[0].formationConfigurationId,
        },
      ],
    };
    invokeMock
      .mockResolvedValueOnce([team])
      .mockResolvedValueOnce(team)
      .mockResolvedValueOnce(team)
      .mockResolvedValueOnce({ ...team, leadResidentId: team.members[0].residentId })
      .mockResolvedValueOnce({ ...team, members: [] });

    await expect(loadTeams(team.projectId)).resolves.toEqual([team]);
    await expect(loadTeam(team.id)).resolves.toEqual(team);
    await expect(formTeam(draft)).resolves.toEqual(team);
    await expect(loadTeam(team.id)).rejects.toThrow("invalid Team");
    await expect(loadTeams(team.projectId)).rejects.toThrow("invalid Team list");
    expect(invokeMock.mock.calls).toEqual([
      ["teams", { projectId: team.projectId }],
      ["team", { teamId: team.id }],
      ["form_team", { draft }],
      ["team", { teamId: team.id }],
      ["teams", { projectId: team.projectId }],
    ]);
  });

  it("maps Mission admission and rejects widened or incomplete authority records", async () => {
    const mission = {
      id: "88888888-8888-4888-8888-888888888888",
      workspaceId: "11111111-1111-4111-8111-111111111111",
      projectId: "33333333-3333-4333-8333-333333333333",
      charterRevision: 2,
      teamId: "55555555-5555-4555-8555-555555555555",
      desiredOutcome: "Ship one reviewed feature",
      successConditions: ["Relevant gates pass"],
      constraints: ["Keep authority explicit"],
      prohibitions: ["Do not deploy"],
      lifecycle: "admitted",
      admittedBy: "operator",
      admittedAtMs: 789,
      members: [
        {
          residentId: "22222222-2222-4222-8222-222222222222",
          admissionConfigurationId: "77777777-7777-4777-8777-777777777777",
        },
      ],
    } as const;
    const draft = {
      projectId: mission.projectId,
      expectedCharterRevision: mission.charterRevision,
      teamId: mission.teamId,
      desiredOutcome: mission.desiredOutcome,
      successConditions: [...mission.successConditions],
      constraints: [...mission.constraints],
      prohibitions: [...mission.prohibitions],
      members: mission.members.map((member) => ({
        residentId: member.residentId,
        expectedConfigurationId: member.admissionConfigurationId,
      })),
    };
    invokeMock
      .mockResolvedValueOnce([mission])
      .mockResolvedValueOnce(mission)
      .mockResolvedValueOnce(mission)
      .mockResolvedValueOnce({ ...mission, leadResidentId: mission.members[0].residentId })
      .mockResolvedValueOnce({ ...mission, successConditions: [] });

    await expect(loadMissions(mission.projectId)).resolves.toEqual([mission]);
    await expect(loadMission(mission.id)).resolves.toEqual(mission);
    await expect(admitMission(draft)).resolves.toEqual(mission);
    await expect(loadMission(mission.id)).rejects.toThrow("invalid Mission");
    await expect(loadMissions(mission.projectId)).rejects.toThrow("invalid Mission list");
    expect(invokeMock.mock.calls).toEqual([
      ["missions", { projectId: mission.projectId }],
      ["mission", { missionId: mission.id }],
      ["admit_mission", { draft }],
      ["mission", { missionId: mission.id }],
      ["missions", { projectId: mission.projectId }],
    ]);
  });

  it("maps local-model discovery and accepts only inactive loopback resources", async () => {
    const report = {
      status: "available",
      endpoint: "http://127.0.0.1:11434",
      observedAtMs: 123,
      models: [
        {
          provider: "ollama",
          activation: "inactive",
          name: "qwen3.8:27b",
          digest: "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643",
          sizeBytes: 17_741_872_154,
          modifiedAt: "2026-08-14T14:38:20.13373148-04:00",
          details: {
            format: "gguf",
            family: "qwen35",
            parameterSize: "27.3B",
            quantizationLevel: "Q4_K_M",
          },
        },
      ],
    } as const;
    invokeMock
      .mockResolvedValueOnce(report)
      .mockResolvedValueOnce({ ...report, endpoint: "http://192.0.2.1:11434" })
      .mockResolvedValueOnce({
        ...report,
        models: [{ ...report.models[0], activation: "active" }],
      })
      .mockResolvedValueOnce({ ...report, executionAuthority: "ambient" });

    await expect(discoverLocalModels()).resolves.toEqual(report);
    await expect(discoverLocalModels()).rejects.toThrow("invalid local-model discovery report");
    await expect(discoverLocalModels()).rejects.toThrow("invalid local-model discovery report");
    await expect(discoverLocalModels()).rejects.toThrow("invalid local-model discovery report");
    expect(invokeMock.mock.calls).toEqual([
      ["discover_local_models"],
      ["discover_local_models"],
      ["discover_local_models"],
      ["discover_local_models"],
    ]);
  });

  it.each([
    ["unsorted models", ["zeta:latest", "alpha:latest"]],
    ["duplicate model names", ["alpha:latest", "alpha:latest"]],
  ])("rejects local-model discovery with %s", async (_label, names) => {
    const template = {
      provider: "ollama",
      activation: "inactive",
      digest: "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643",
      sizeBytes: 1,
      modifiedAt: "2026-08-14T14:38:20Z",
      details: {
        format: "gguf",
        family: "qwen35",
        parameterSize: "27.3B",
        quantizationLevel: "Q4_K_M",
      },
    };
    invokeMock.mockResolvedValue({
      status: "available",
      endpoint: "http://127.0.0.1:11434",
      observedAtMs: 123,
      models: names.map((name) => ({ ...template, name })),
    });

    await expect(discoverLocalModels()).rejects.toThrow("invalid local-model discovery report");
  });

  it("maps local-model review and activation through closed, authority-free shapes", async () => {
    const review = {
      binding: {
        provider: "ollama",
        endpoint: "http://127.0.0.1:11434",
        modelName: "qwen3.8:27b",
        modelDigest: "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643",
        inspectionDigest:
          "12340167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643",
      },
      providerFeatures: ["completion", "tools"],
      unrecognizedFeatures: ["future_audio"],
    } as const;
    const active = {
      id: "44444444-4444-4444-8444-444444444444",
      workspaceId: "11111111-1111-4111-8111-111111111111",
      ...review,
      approvedFeatures: ["completion"],
      state: "active",
      predecessorId: null,
      decidedAtMs: 123,
    } as const;
    const inactive = {
      ...active,
      id: "55555555-5555-4555-8555-555555555555",
      state: "inactive",
      predecessorId: active.id,
      decidedAtMs: 124,
    } as const;
    invokeMock
      .mockResolvedValueOnce(review)
      .mockResolvedValueOnce(active)
      .mockResolvedValueOnce([active])
      .mockResolvedValueOnce(inactive)
      .mockResolvedValueOnce({ ...active, routeId: "ambient" });

    await expect(
      inspectLocalModel(review.binding.modelName, review.binding.modelDigest),
    ).resolves.toEqual(review);
    await expect(activateLocalModel(review, ["completion"])).resolves.toEqual(active);
    await expect(loadLocalModelActivations()).resolves.toEqual([active]);
    await expect(deactivateLocalModel(active.id)).resolves.toEqual(inactive);
    await expect(loadLocalModelActivations()).rejects.toThrow(
      "invalid local-model activation list",
    );
    expect(invokeMock.mock.calls).toEqual([
      [
        "inspect_local_model",
        { modelName: review.binding.modelName, expectedDigest: review.binding.modelDigest },
      ],
      ["activate_local_model", { review, approvedFeatures: ["completion"] }],
      ["local_model_activations"],
      ["deactivate_local_model", { expectedActivationId: active.id }],
      ["local_model_activations"],
    ]);
  });

  it.each([
    ["unknown approved feature", { approvedFeatures: ["future_audio"] }],
    ["unsorted provider features", { providerFeatures: ["tools", "completion"] }],
    ["non-loopback binding", { binding: { endpoint: "http://192.0.2.1:11434" } }],
    ["inactive decision without a predecessor", { state: "inactive" }],
  ])("rejects local-model activation with %s", async (_label, change) => {
    const review = {
      binding: {
        provider: "ollama",
        endpoint: "http://127.0.0.1:11434",
        modelName: "qwen3.8:27b",
        modelDigest: "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643",
        inspectionDigest:
          "12340167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643",
      },
      providerFeatures: ["completion", "tools"],
      unrecognizedFeatures: ["future_audio"],
    };
    const activation = {
      id: "44444444-4444-4444-8444-444444444444",
      workspaceId: "11111111-1111-4111-8111-111111111111",
      ...review,
      approvedFeatures: ["completion"],
      state: "active",
      predecessorId: null,
      decidedAtMs: 123,
    };
    const response = "binding" in change
      ? { ...activation, binding: { ...activation.binding, ...change.binding } }
      : { ...activation, ...change };
    invokeMock.mockResolvedValue([response]);

    await expect(loadLocalModelActivations()).rejects.toThrow(
      "invalid local-model activation list",
    );
  });

  it.each([
    ["a widened safety boundary", { budgetBoundary: "unlimited" }],
    ["a changed external-effects boundary", { externalEffects: "allowed" }],
    ["an unsupported autonomy profile", { autonomyProfile: "bounded" }],
    ["a control character in the name", { name: "Beacon\nAlpha" }],
  ])("rejects Project responses with %s", async (_label, change) => {
    const project = {
      id: "33333333-3333-4333-8333-333333333333",
      workspaceId: "11111111-1111-4111-8111-111111111111",
      name: "Beacon Alpha",
      charter: {
        revision: 1,
        purpose: "Build a trustworthy local software product.",
        desiredOutcomes: ["Ship a verified vertical slice"],
        constraints: [],
        prohibitions: [],
        autonomyProfile: "supervised",
        budgetBoundary: "no_spend",
        externalEffects: "prohibited",
        authoredAtMs: 123,
      },
      createdAtMs: 123,
    };
    const response = "name" in change
      ? { ...project, ...change }
      : { ...project, charter: { ...project.charter, ...change } };
    invokeMock.mockResolvedValue(response);

    await expect(loadProject(project.id)).rejects.toThrow("invalid Project");
  });

  it.each([
    [
      "changed recommendation defaults",
      {
        workspaceId: "11111111-1111-4111-8111-111111111111",
        proposalVersion: 1,
        decision: "proposed",
        templates: [
          { role: "lead", description: "Coordinates Missions.", recommended: false },
          { role: "builder", description: "Produces Artifacts.", recommended: true },
          { role: "reviewer", description: "Evaluates work.", recommended: true },
          { role: "researcher", description: "Develops Evidence.", recommended: false },
        ],
        residents: [],
      },
    ],
    [
      "a control character in a Display Name",
      {
        workspaceId: "11111111-1111-4111-8111-111111111111",
        proposalVersion: 1,
        decision: "accepted",
        templates: [
          { role: "lead", description: "Coordinates Missions.", recommended: true },
          { role: "builder", description: "Produces Artifacts.", recommended: true },
          { role: "reviewer", description: "Evaluates work.", recommended: true },
          { role: "researcher", description: "Develops Evidence.", recommended: false },
        ],
        residents: [
          {
            id: "22222222-2222-4222-8222-222222222222",
            role: "lead",
            displayName: "North\nstar",
            lifecycle: "inactive",
          },
        ],
      },
    ],
  ])("rejects %s in a Starter Organization result", async (_case, response) => {
    invokeMock.mockResolvedValue(response);

    await expect(loadStarterOrganization()).rejects.toThrow("invalid Starter Organization");
  });

  it.each([
    { inactivityTimeoutMinutes: 0 },
    { inactivityTimeoutMinutes: 1_441 },
    { inactivityTimeoutMinutes: 30, extra: true },
  ])("rejects an invalid session policy response", async (response) => {
    invokeMock.mockResolvedValue(response);

    await expect(loadSessionPolicy()).rejects.toThrow("invalid session policy");
  });

  it("maps only known local error codes to Operator-facing guidance", () => {
    expect(describeLocalAccountError({ code: "password_confirmation_mismatch" })).toBe(
      "The password confirmation does not match.",
    );
    expect(
      describeLocalAccountError({ code: "authentication_throttled", retryAfterMs: 1_001 }),
    ).toBe("Wait 2 seconds before trying again.");
    expect(describeLocalAccountError({ code: "starter_organization_unavailable" })).toBe(
      "Beacon could not verify the Starter Organization state.",
    );
    expect(describeLocalAccountError({ code: "stale_resident_decision" })).toBe(
      "The Resident changed. Reload the roster before trying again.",
    );
    expect(describeLocalAccountError({ code: "stale_charter_revision" })).toBe(
      "This Charter changed. Reload the current revision before publishing.",
    );
    expect(describeLocalAccountError({ code: "unchanged_charter" })).toBe(
      "Change at least one Charter field before publishing a revision.",
    );
    expect(describeLocalAccountError({ code: "stale_resident_configuration" })).toBe(
      "The Resident configuration changed. Reload it before trying again.",
    );
    expect(describeLocalAccountError({ code: "stale_team_member_configuration" })).toBe(
      "A Resident configuration changed. Reload the Team form before trying again.",
    );
    expect(describeLocalAccountError({ code: "stale_team_member_configuration" })).toBe(
      "A Resident configuration changed. Reload the Team form before trying again.",
    );
    expect(describeLocalAccountError({ code: "invented", message: "render me" })).not.toContain(
      "render me",
    );
  });

  it("maps only the closed Resident Configuration contract without widening authority", async () => {
    const catalog = [
      {
        capability: "work.create_mission",
        label: "Create Missions",
        description: "Request creation through Work Policy.",
      },
      {
        capability: "work.form_team",
        label: "Form Teams",
        description: "Request a named Team through Work Policy.",
      },
      {
        capability: "work.coordinate_mission",
        label: "Coordinate Missions",
        description: "Coordinate accepted Mission work through Work Policy.",
      },
      {
        capability: "work.assign_work",
        label: "Assign Work",
        description: "Assign bounded work through Work Policy.",
      },
      {
        capability: "artifact.produce",
        label: "Produce Artifacts",
        description: "Produce reviewable Mission artifacts.",
      },
      {
        capability: "artifact.evaluate",
        label: "Evaluate Artifacts",
        description: "Evaluate artifacts without approving effects.",
      },
      {
        capability: "evidence.develop",
        label: "Develop Evidence",
        description: "Develop evidence for operator review.",
      },
    ] as const;
    const configured = {
      id: "77777777-7777-4777-8777-777777777777",
      workspaceId: "11111111-1111-4111-8111-111111111111",
      residentId: "22222222-2222-4222-8222-222222222222",
      state: "configured",
      binding: {
        activationId: "44444444-4444-4444-8444-444444444444",
        capabilities: ["work.create_mission", "work.form_team"],
        budgetBoundary: "no_spend",
      },
      predecessorId: null,
      decidedAtMs: 123,
      eligibleForTeam: true,
      ineligibilityReasons: [],
    } as const;
    const cleared = {
      ...configured,
      id: "88888888-8888-4888-8888-888888888888",
      state: "cleared",
      binding: null,
      predecessorId: configured.id,
      eligibleForTeam: false,
      ineligibilityReasons: ["configuration_cleared"],
    } as const;
    invokeMock
      .mockResolvedValueOnce(catalog)
      .mockResolvedValueOnce([configured])
      .mockResolvedValueOnce(configured)
      .mockResolvedValueOnce(cleared)
      .mockResolvedValueOnce({ ...configured, executionAuthority: true });

    await expect(loadResidentCapabilityCatalog()).resolves.toEqual(catalog);
    await expect(loadResidentConfigurations()).resolves.toEqual([configured]);
    await expect(
      configureResident(
        configured.residentId,
        null,
        configured.binding.activationId,
        configured.binding.capabilities,
      ),
    ).resolves.toEqual(configured);
    await expect(
      clearResidentConfiguration(configured.residentId, configured.id),
    ).resolves.toEqual(cleared);
    await expect(loadResidentConfigurations()).rejects.toThrow(
      "invalid Resident configuration list",
    );
    expect(invokeMock.mock.calls).toEqual([
      ["resident_capability_catalog"],
      ["resident_configurations"],
      [
        "configure_resident",
        {
          residentId: configured.residentId,
          expectedDecisionId: null,
          activationId: configured.binding.activationId,
          capabilities: configured.binding.capabilities,
        },
      ],
      [
        "clear_resident_configuration",
        { residentId: configured.residentId, expectedDecisionId: configured.id },
      ],
      ["resident_configurations"],
    ]);
  });

  it("accepts an exact preflight Receipt and rejects one that hides what it skipped", async () => {
    const absent = (reason: string) => ({ kind: "absent", reason });
    const receipt = {
      version: 1,
      runId: "7f1d5a9c-3b2e-4c8a-9d6f-1e2b3c4d5e6f",
      workspaceId: "1a2b3c4d-5e6f-4a8b-9c0d-1e2f3a4b5c6d",
      projectId: "2b3c4d5e-6f7a-4b8c-9d0e-2f3a4b5c6d7e",
      repositoryRevision: 1,
      charterRevision: 1,
      outcome: "sandbox_unavailable",
      sourceKind: "local_path",
      source: "/home/operator/project",
      baseRevision: "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
      resolvedChecks: [
        {
          name: "pytest",
          argv: ["pytest", "-q"],
          declaredWorkingDirectory: ".",
          resolvedWorkingDirectory: { kind: "present", value: "/home/operator/project" },
        },
      ],
      sandboxVersion: "none",
      policyCharterRevision: 1,
      policyRepositoryRevision: 1,
      startedAtMs: 1,
      endedAtMs: 2,
      notVerified: ["base revision reachability from a ref (object presence verified only)"],
      unavailable: {
        proposalDigest: absent("no Proposal surface exists"),
        rulingDigest: absent("no Proposal surface exists"),
        resultingRevision: absent("preflight refuses to execute, so nothing ran"),
        filesystemEffects: absent("preflight refuses to execute, so nothing ran"),
        diffDigest: absent("preflight refuses to execute, so nothing ran"),
        modelInvocations: absent("preflight performs no model inference"),
        costUsdMicros: absent("preflight performs no model inference"),
      },
    };
    invokeMock.mockResolvedValue(receipt);
    await expect(preflightProjectChecks(receipt.projectId)).resolves.toEqual(receipt);
    expect(invokeMock).toHaveBeenCalledWith("preflight_project_checks", {
      projectId: receipt.projectId,
    });

    // A Receipt that cannot say what it skipped is not evidence.
    const { notVerified: _dropped, ...withoutNotVerified } = receipt;
    invokeMock.mockResolvedValue(withoutNotVerified);
    await expect(preflightProjectChecks(receipt.projectId)).rejects.toThrow(
      "invalid preflight Receipt",
    );

    invokeMock.mockResolvedValue({ ...receipt, surprise: true });
    await expect(preflightProjectChecks(receipt.projectId)).rejects.toThrow(
      "invalid preflight Receipt",
    );

    // An unavailable value is not an empty one.
    invokeMock.mockResolvedValue({
      ...receipt,
      unavailable: { ...receipt.unavailable, proposalDigest: { kind: "present", value: "" } },
    });
    await expect(preflightProjectChecks(receipt.projectId)).resolves.toBeTruthy();
    invokeMock.mockResolvedValue({
      ...receipt,
      unavailable: { ...receipt.unavailable, proposalDigest: { kind: "absent" } },
    });
    await expect(preflightProjectChecks(receipt.projectId)).rejects.toThrow(
      "invalid preflight Receipt",
    );
  });

  // opus's P1-2. The adapter carries the whole boundary contract -- order, the
  // window bound, the closed per-kind outcome, timestamp representability --
  // and none of it was tested; three of four mutants survived the suite. The
  // panel tests cannot cover this and correctly do not try: `BriefingReceipts`
  // takes an already-validated page.
  //
  // C26's lesson applies here, and it is why every case below drives
  // `recentReceipts()` rather than a helper: the property is "a page the daemon
  // could not have produced is a refused read", and the thing that decides that
  // is the exported Query, not a predicate sitting next to it.
  describe("the recent-Receipt adapter", () => {
    const row = (
      runId: string,
      startedAtMs: number,
      outcome: Record<string, unknown>,
    ) => ({
      runId,
      projectId: "2b3c4d5e-6f7a-4b8c-9d0e-2f3a4b5c6d7e",
      projectName: "Task tracker",
      version: outcome.kind === "check_preflight" ? 1 : 2,
      outcome,
      startedAtMs,
      endedAtMs: startedAtMs + 1,
      recordedAtMs: startedAtMs + 1,
    });
    const newest = row("cccccccc-3333-4333-8333-333333333333", 300, {
      kind: "declared_checks",
      summary: "completed",
    });
    const oldest = row("aaaaaaaa-1111-4111-8111-111111111111", 100, {
      kind: "check_preflight",
      outcome: "verified",
    });
    const page = (overrides: Record<string, unknown> = {}) => ({
      receipts: [newest, oldest],
      runsInFlightInWindow: 0,
      runsUnconfirmedInWindow: 0,
      runWindowApplied: 50,
      moreRunsExist: false,
      ...overrides,
    });
    const refused = async (patch: Record<string, unknown>) => {
      invokeMock.mockResolvedValue({ ...page(), ...patch });
      await expect(recentReceipts(50)).rejects.toThrow("invalid recent Receipt page");
    };

    it("accepts the exact page the daemon produces", async () => {
      const exact = page();
      invokeMock.mockResolvedValue(exact);
      await expect(recentReceipts(50)).resolves.toEqual(exact);
      expect(invokeMock).toHaveBeenCalledWith("recent_receipts", { window: 50 });
    });

    it("refuses a page the daemon did not order", async () => {
      // The daemon orders by started_at_ms DESC then run id DESC. Re-sorting
      // here would put the ordering policy back in the interface; refusing is
      // what makes the comment that says so true.
      await refused({ receipts: [oldest, newest] });
      // Ties break on the run id, descending, by code point.
      const tie = [
        row("bbbbbbbb-2222-4222-8222-222222222222", 200, {
          kind: "check_preflight",
          outcome: "verified",
        }),
        row("cccccccc-3333-4333-8333-333333333333", 200, {
          kind: "check_preflight",
          outcome: "verified",
        }),
      ];
      await refused({ receipts: tie });
    });

    it("refuses more rows than the window the daemon says it walked", async () => {
      await refused({ receipts: [newest, oldest], runWindowApplied: 1 });
    });

    it("refuses an outcome that is not closed for its kind", async () => {
      // The stored `outcome` column is one CHECK shared by both kinds, so a
      // flat union would let these through. Each is a page the reader cannot
      // have produced.
      await refused({
        receipts: [row("cccccccc-3333-4333-8333-333333333333", 300, { kind: "nonsense" })],
      });
      await refused({
        receipts: [
          row("cccccccc-3333-4333-8333-333333333333", 300, {
            kind: "check_preflight",
            summary: "completed",
          }),
        ],
      });
      await refused({
        receipts: [
          row("cccccccc-3333-4333-8333-333333333333", 300, {
            kind: "declared_checks",
            outcome: "verified",
          }),
        ],
      });
      await refused({
        receipts: [
          row("cccccccc-3333-4333-8333-333333333333", 300, {
            kind: "check_preflight",
            outcome: "completed",
          }),
        ],
      });
      // Both arms at once is a widened vocabulary wearing a closed one.
      await refused({
        receipts: [
          row("cccccccc-3333-4333-8333-333333333333", 300, {
            kind: "check_preflight",
            outcome: "verified",
            summary: "completed",
          }),
        ],
      });
    });

    it("refuses a timestamp the interface cannot render", async () => {
      // The gap `4e1ff8b` named: a safe non-negative integer that is not a
      // representable instant. `toISOString` raises on it, and an uncaught
      // throw inside render removes the panel.
      await refused({
        receipts: [
          row("cccccccc-3333-4333-8333-333333333333", 9e15, {
            kind: "check_preflight",
            outcome: "verified",
          }),
        ],
      });
    });

    it("refuses counts outside the u32 the daemon counts in", async () => {
      await refused({ runsUnconfirmedInWindow: 4_294_967_296 });
      await refused({ runWindowApplied: 4_294_967_296 });
      await refused({ moreRunsExist: "yes" });
    });

    it("refuses a page with a key the daemon does not send", async () => {
      await refused({ surprise: true });
    });
  });

  it("refuses a live-data encryption report whose shape it does not recognise", async () => {
    const report = {
      conclusion: {
        state: "disabled",
        reason: "no_crypt_on_any_path",
        subjects: [{ role: "device", value: "259:2", shortened: false }],
        omittedSubjects: 0,
      },
      mountPoint: { role: "location", value: "/", shortened: false },
      evidence: [
        { source: "stat(live store)", observation: "st_dev = 259:2", truncated: false },
      ],
      omittedEvidence: 0,
      notVerified: ["filesystem-level encryption such as fscrypt or ext4 encryption"],
      observedAtMs: 1_756_000_000_000,
    };
    invokeMock.mockResolvedValue(report);
    await expect(liveDataEncryptionStatus()).resolves.toEqual(report);
    expect(invokeMock).toHaveBeenCalledWith("live_data_encryption_status");

    const refused = async (patch: Record<string, unknown>) => {
      invokeMock.mockResolvedValue({ ...report, ...patch });
      await expect(liveDataEncryptionStatus()).rejects.toThrow(
        "invalid live-data encryption report",
      );
    };

    // A fourth state, or a reason outside the closed set, is a widened
    // vocabulary rather than a richer answer.
    await refused({ conclusion: { ...report.conclusion, state: "partial" } });
    await refused({ conclusion: { ...report.conclusion, reason: "something_new" } });

    // State and reason must agree: `enabled` means crypt covers every path and
    // nothing else does, in both directions.
    await refused({ conclusion: { ...report.conclusion, state: "enabled" } });
    await refused({
      conclusion: { ...report.conclusion, reason: "crypt_covers_every_path" },
    });

    // The bound is structural: a subject longer than the shape allows is a
    // report this reader cannot have produced.
    await refused({
      conclusion: {
        ...report.conclusion,
        subjects: [{ role: "device", value: "d".repeat(25), shortened: false }],
      },
    });
    await refused({
      conclusion: {
        ...report.conclusion,
        subjects: Array.from({ length: 4 }, () => ({
          role: "device",
          value: "8:1",
          shortened: false,
        })),
      },
    });
    await refused({
      conclusion: {
        ...report.conclusion,
        subjects: [{ role: "nonsense", value: "8:1", shortened: false }],
      },
    });
    // The flag is part of the shape, not decoration.
    await refused({
      conclusion: {
        ...report.conclusion,
        subjects: [{ role: "device", value: "8:1" }],
      },
    });
    await refused({
      evidence: [{ source: "s", observation: "o" }],
    });

    // Four shapes the Rust reader cannot produce, all of which the round-six
    // adapter resolved successfully. Checking generic lengths is not checking
    // the wire shape.
    //
    // One over `u32::MAX`, which is the type the omission count crosses on.
    await refused({
      conclusion: { ...report.conclusion, omittedSubjects: 4_294_967_296 },
    });
    await refused({ omittedEvidence: 4_294_967_296 });
    // A device role whose value is not a device number.
    await refused({
      conclusion: {
        ...report.conclusion,
        subjects: [{ role: "device", value: "not-a-device", shortened: false }],
      },
    });
    // A short value claiming a cut: no constructor shortens a device number,
    // which is at most 21 characters against a bound of 24.
    await refused({
      conclusion: {
        ...report.conclusion,
        subjects: [{ role: "device", value: "8:1", shortened: true }],
      },
    });
    // The Rust constructor emits canonical decimal device numbers. Values
    // that merely parse are aliases the reader cannot have produced, and the
    // two checked doors must agree about refusing them.
    for (const value of ["+8:1", "08:01"]) {
      await refused({
        conclusion: {
          ...report.conclusion,
          subjects: [{ role: "device", value, shortened: false }],
        },
      });
    }
    // Omission counts are truthful only after the corresponding list filled.
    await refused({ conclusion: { ...report.conclusion, omittedSubjects: 1 } });
    await refused({ omittedEvidence: 1 });
    // A volume named by a device rather than by where it is mounted.
    await refused({
      mountPoint: { role: "device", value: "8:1", shortened: false },
    });
    // A reason whose sentence names a role no subject carries: the panel would
    // render "an unnamed subject", which is a sentence that says nothing.
    await refused({ conclusion: { ...report.conclusion, subjects: [] } });
    await refused({
      conclusion: {
        state: "unknown",
        reason: "some_paths_uncovered",
        subjects: [{ role: "mapping", value: "vg0", shortened: false }],
        omittedSubjects: 0,
      },
    });

    // The largest report the shape allows is accepted, and the entry past it
    // is not: the evidence cap is part of the interface, like the subject cap.
    const widest = {
      source: "s".repeat(200),
      observation: "o".repeat(200),
      truncated: true,
    };
    invokeMock.mockResolvedValue({
      ...report,
      evidence: Array.from({ length: 32 }, () => widest),
      omittedEvidence: 4_294_967_295,
    });
    await expect(liveDataEncryptionStatus()).resolves.toBeTruthy();
    await refused({ evidence: Array.from({ length: 33 }, () => widest) });
    await refused({
      evidence: [{ ...widest, observation: "o".repeat(201) }],
    });

    await refused({ surprise: true });
  });

  it("turns a failed reading into a report the adapter accepts", async () => {
    // The failure used to reach the panel as an unbounded free-text prop
    // beside a MISSING report: outside the closed vocabulary, rendered by a
    // second code path, and bounded by nothing. Round-tripping it through the
    // real adapter is the proof that the client-built report is the same shape
    // a read one is.
    const built = failedReadReport(new Error("x".repeat(500)), 1_756_000_000_000);
    invokeMock.mockResolvedValue(built);
    await expect(liveDataEncryptionStatus()).resolves.toEqual(built);

    expect(built.conclusion.reason).toBe("initial_read_failed");
    expect(built.conclusion.state).toBe("unknown");
    // Fidelity where it fits, and the flag where it does not.
    expect(built.evidence[0].observation).toHaveLength(200);
    expect(built.evidence[0].truncated).toBe(true);
    expect(built.conclusion.subjects[0].shortened).toBe(true);

    const short = failedReadReport(new Error("transport closed"), 1);
    invokeMock.mockResolvedValue(short);
    await expect(liveDataEncryptionStatus()).resolves.toEqual(short);
    expect(short.evidence[0].observation).toBe("transport closed");
    expect(short.evidence[0].truncated).toBe(false);
  });

  it("keeps a structured failure's own message rather than its code", () => {
    // `code` is a label for the class; `message` is the only text that says
    // what failed. Preferring the label discarded it -- the same substitution
    // this diagnostic exists to avoid, one level up.
    expect(verbatimFailure({ code: "diagnostic_failed", message: "sysfs read denied" })).toBe(
      "sysfs read denied",
    );
    expect(verbatimFailure({ code: "diagnostic_failed" })).toBe("diagnostic_failed");
    expect(verbatimFailure(new Error("original diagnostic failure"))).toBe(
      "original diagnostic failure",
    );
    expect(verbatimFailure({})).toBe("no reason was reported");
  });
});
