import {
  Button,
  Checkbox,
  Field,
  FluentProvider,
  Input,
  makeStyles,
  shorthands,
  tokens,
} from "@fluentui/react-components";
import { type FormEvent, useEffect, useRef, useState } from "react";

import { LiveDataEncryptionStatus } from "./LiveDataEncryption";

import {
  BriefingReceipts,
} from "./BriefingReceipts";

import {
  acceptStarterOrganization,
  activateLocalModel,
  activateResident,
  type AuthenticationCapabilities,
  type BeaconCapability,
  type BeaconCapabilityDescriptor,
  type DesktopSessionStatus,
  beginLocalPasswordSetup,
  clearResidentConfiguration,
  completeLocalOperatorSetup,
  configureResident,
  createProject,
  admitMission,
  formTeam,
  deactivateLocalModel,
  discoverLocalModels,
  describeLocalAccountError,
  isPreflightUnavailable,
  failedReadReport,
  discardStarterOrganization,
  loadAuthenticationCapabilities,
  inspectLocalModel,
  type LiveDataEncryptionReport,
  liveDataEncryptionStatus,
  loadLocalModelActivations,
  loadMissions,
  loadPlatformSessionMonitorStatus,
  loadProject,
  loadProjectCharterHistory,
  loadProjectPolicy,
  loadProjectPolicyHistory,
  loadProjectRepository,
  recentReceipts as readRecentReceipts,
  type RecentReceiptPage,
  preflightProjectChecks,
  loadProjectRepositoryHistory,
  loadProjects,
  loadResidentCapabilityCatalog,
  loadResidentConfigurations,
  loadResidents,
  loadSessionPolicy,
  loadSessionStatus,
  loadStarterOrganization,
  loadTeams,
  lockLocalSession,
  type PlatformSessionMonitorStatus,
  type LocalModelDiscoveryReport,
  type LocalModelActivation,
  type LocalModelActivationReview,
  type ProviderFeature,
  type ProjectCharter,
  type ProjectCharterDraft,
  type ProjectDraft,
  type ProjectPolicy,
  type ProjectPolicyDraft,
  type PreflightReceipt,
  type ProjectRepository,
  type RepositoryDraft,
  type ProjectSummary,
  type ProjectView,
  type MissionDraft,
  type MissionView,
  type TeamDraft,
  type TeamView,
  recordLocalOperatorActivity,
  reactivateResident,
  recoverLocalOperatorAccount,
  reviseProjectCharter,
  publishProjectPolicy,
  bindProjectRepository,
  retireResident,
  setLocalSessionTimeout,
  signInWithLocalPassword,
  suspendResident,
  type StarterOrganizationView,
  type ResidentAgentView,
  type ResidentConfiguration,
  type StarterResidentDraft,
  type StarterRole,
} from "./session";
import { LocalModels } from "./LocalModels";
import {
  formatObjectLink,
  parseObjectLink,
  routeObjectLink,
  type AddressableObjectLink,
  type LinkContext,
  type LinkOutcome,
  type ObjectLink,
} from "./objectLinks";
import { ProjectsWorkspace } from "./Projects";
import { ResidentConfigurations } from "./ResidentConfigurations";
import { darkTheme, lightTheme } from "./theme";

const applications = ["Briefing", "Projects", "Conversations", "Knowledge", "Software"] as const;
type Application = (typeof applications)[number];
const availableApplications = new Set<Application>(["Briefing", "Projects"]);
const themeChoices = [
  { value: "light", label: "Light", accessibleLabel: "Use light theme" },
  { value: "dark", label: "Dark", accessibleLabel: "Use dark theme" },
  { value: "system", label: "System", accessibleLabel: "Follow system theme" },
] as const;

type ThemeChoice = (typeof themeChoices)[number]["value"];
type SessionViewState = "loading" | "ready" | "error";

const useStyles = makeStyles({
  provider: {
    minHeight: "100dvh",
    backgroundColor: tokens.colorNeutralBackground2,
    color: tokens.colorNeutralForeground1,
  },
  shell: {
    minHeight: "100dvh",
    display: "grid",
    gridTemplateColumns: "220px minmax(0, 1fr) 280px",
  },
  rail: {
    display: "flex",
    flexDirection: "column",
    rowGap: tokens.spacingVerticalXS,
    backgroundColor: tokens.colorNeutralBackground1,
    ...shorthands.borderRight(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    ...shorthands.padding(tokens.spacingVerticalXXL, tokens.spacingHorizontalL),
  },
  brand: {
    fontSize: tokens.fontSizeBase500,
    fontWeight: tokens.fontWeightSemibold,
    marginBottom: tokens.spacingVerticalXXL,
  },
  navigationButton: { justifyContent: "flex-start" },
  main: {
    display: "grid",
    alignContent: "center",
    minWidth: 0,
    ...shorthands.padding(tokens.spacingVerticalXXXL, tokens.spacingHorizontalXXXL),
  },
  attention: {
    width: "min(100%, 640px)",
    ...shorthands.borderLeft("4px", "solid", tokens.colorBrandBackground),
    paddingLeft: tokens.spacingHorizontalXXL,
  },
  heading: {
    fontSize: tokens.fontSizeHero800,
    lineHeight: tokens.lineHeightHero800,
    letterSpacing: "-0.03em",
    marginTop: 0,
    marginBottom: tokens.spacingVerticalL,
  },
  body: {
    maxWidth: "58ch",
    color: tokens.colorNeutralForeground2,
    fontSize: tokens.fontSizeBase400,
    lineHeight: tokens.lineHeightBase400,
  },
  methodNotice: {
    marginTop: tokens.spacingVerticalL,
    marginBottom: tokens.spacingVerticalL,
    ...shorthands.padding(tokens.spacingVerticalM, tokens.spacingHorizontalM),
    backgroundColor: tokens.colorNeutralBackground3,
    ...shorthands.borderRadius(tokens.borderRadiusMedium),
  },
  form: {
    display: "grid",
    rowGap: tokens.spacingVerticalM,
    maxWidth: "420px",
    marginTop: tokens.spacingVerticalL,
  },
  actions: {
    display: "flex",
    columnGap: tokens.spacingHorizontalS,
    alignItems: "center",
    marginTop: tokens.spacingVerticalS,
  },
  recoveryKey: {
    display: "block",
    fontFamily: "ui-monospace, SFMono-Regular, Consolas, monospace",
    fontSize: tokens.fontSizeBase400,
    letterSpacing: "0.04em",
    overflowWrap: "anywhere",
    userSelect: "all",
    marginTop: tokens.spacingVerticalL,
    marginBottom: tokens.spacingVerticalL,
    ...shorthands.padding(tokens.spacingVerticalL, tokens.spacingHorizontalL),
    backgroundColor: tokens.colorNeutralBackground3,
    ...shorthands.borderRadius(tokens.borderRadiusMedium),
  },
  actionError: { color: tokens.colorPaletteRedForeground1 },
  inspector: {
    backgroundColor: tokens.colorNeutralBackground1,
    ...shorthands.borderLeft(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    ...shorthands.padding(tokens.spacingVerticalXXL, tokens.spacingHorizontalL),
  },
  inspectorHeading: { fontSize: tokens.fontSizeBase400, marginTop: 0 },
  status: { color: tokens.colorNeutralForeground2 },
  themeHeading: {
    fontSize: tokens.fontSizeBase300,
    marginTop: tokens.spacingVerticalXXL,
    marginBottom: tokens.spacingVerticalS,
  },
  themeControls: {
    display: "grid",
    gridTemplateColumns: "repeat(3, minmax(0, 1fr))",
    columnGap: tokens.spacingHorizontalXS,
  },
  settingsForm: {
    display: "grid",
    rowGap: tokens.spacingVerticalS,
    marginTop: tokens.spacingVerticalS,
  },
  monitorWarning: { color: tokens.colorPaletteDarkOrangeForeground1 },
  starterSection: { width: "min(100%, 760px)" },
  roster: {
    display: "grid",
    marginTop: tokens.spacingVerticalXL,
    ...shorthands.borderTop(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
  },
  rosterRow: {
    display: "grid",
    gridTemplateColumns: "minmax(220px, 1fr) minmax(220px, 0.8fr)",
    columnGap: tokens.spacingHorizontalXXL,
    rowGap: tokens.spacingVerticalS,
    alignItems: "center",
    ...shorthands.padding(tokens.spacingVerticalL, 0),
    ...shorthands.borderBottom(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
  },
  roleSummary: { display: "grid", rowGap: tokens.spacingVerticalXS },
  roleDescription: { color: tokens.colorNeutralForeground2, margin: 0 },
  rosterField: { minWidth: 0 },
  briefingList: {
    display: "grid",
    rowGap: tokens.spacingVerticalS,
    marginTop: tokens.spacingVerticalL,
    marginBottom: tokens.spacingVerticalL,
    paddingLeft: tokens.spacingHorizontalXXL,
  },
  residentRow: {
    display: "grid",
    gridTemplateColumns: "minmax(0, 1fr) auto",
    columnGap: tokens.spacingHorizontalM,
    rowGap: tokens.spacingVerticalXS,
    alignItems: "center",
  },
  residentSummary: { display: "grid", rowGap: tokens.spacingVerticalXXS },
  residentActions: { display: "flex", columnGap: tokens.spacingHorizontalXS },
  lifecycleStatus: { color: tokens.colorNeutralForeground2 },
  retirementConfirm: {
    marginTop: tokens.spacingVerticalL,
    ...shorthands.padding(tokens.spacingVerticalM, tokens.spacingHorizontalM),
    ...shorthands.border(tokens.strokeWidthThin, "solid", tokens.colorPaletteRedBorder1),
    ...shorthands.borderRadius(tokens.borderRadiusMedium),
  },
});

/** What Briefing asks the daemon to walk. The daemon clamps and reports what
 *  it applied; this is a request, not a bound. */
const RECENT_RECEIPT_RUN_WINDOW = 50;

export function App() {
  const classes = useStyles();
  const mainContentRef = useRef<HTMLElement>(null);
  const [viewState, setViewState] = useState<SessionViewState>("loading");
  const [sessionAttempt, setSessionAttempt] = useState(0);
  const [session, setSession] = useState<DesktopSessionStatus | null>(null);
  const [foregroundVerified, setForegroundVerified] = useState(false);
  const foregroundEpoch = useRef(0);
  const foregroundEligible = useRef(true);
  const [capabilities, setCapabilities] = useState<AuthenticationCapabilities | null>(null);
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [recoveryKey, setRecoveryKey] = useState("");
  const [recoverySaved, setRecoverySaved] = useState(false);
  const [recoveryMode, setRecoveryMode] = useState(false);
  const [replacementKey, setReplacementKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState("");
  const [timeoutMinutes, setTimeoutMinutes] = useState("30");
  const [platformMonitor, setPlatformMonitor] =
    useState<PlatformSessionMonitorStatus | null>(null);
  const [starterOrganization, setStarterOrganization] =
    useState<StarterOrganizationView | null>(null);
  const [residents, setResidents] = useState<ResidentAgentView[] | null>(null);
  const [activeApplication, setActiveApplication] = useState<Application>("Briefing");
  const [objectLinkInput, setObjectLinkInput] = useState("");
  const [pendingObjectLink, setPendingObjectLink] = useState<ObjectLink | null>(null);
  const [objectLinkMessage, setObjectLinkMessage] = useState("");
  const objectLinkEpoch = useRef(0);
  const openingLinkedProject = useRef("");
  const [linkedMissionId, setLinkedMissionId] = useState<string | null>(null);
  const [projects, setProjects] = useState<ProjectSummary[] | null>(null);
  const [selectedProject, setSelectedProject] = useState<ProjectView | null>(null);
  const [charterHistory, setCharterHistory] = useState<ProjectCharter[] | null>(null);
  const [projectPolicy, setProjectPolicy] = useState<ProjectPolicy | null>(null);
  const [projectPolicyHistory, setProjectPolicyHistory] = useState<ProjectPolicy[] | null>(null);
  const [projectRepository, setProjectRepository] = useState<ProjectRepository | null>(null);
  const [preflightReceipt, setPreflightReceipt] = useState<PreflightReceipt | null>(null);
  // Which adoption the displayed preflight belongs to. Bumped only where a Project is actually
  // adopted, and read again after the await, so a response that was already in flight when the
  // operator moved on cannot publish itself over the Project now on screen.
  //
  // A generation and NOT the Project id: a same-id re-open is a fresh read under this component's
  // own unconditional invalidation, so an observation started before that re-open would pass an
  // id comparison and republish itself anyway. A counter cannot collide with itself.
  //
  // A ref and NOT a captured value: the whole point is to read the CURRENT state after awaiting,
  // which a value captured in the closure cannot do.
  const preflightGeneration = useRef(0);
  const [projectRepositoryHistory, setProjectRepositoryHistory] = useState<
    ProjectRepository[] | null
  >(null);
  const [teams, setTeams] = useState<TeamView[] | null>(null);
  const [missions, setMissions] = useState<MissionView[] | null>(null);
  const [teamConfigurations, setTeamConfigurations] =
    useState<ResidentConfiguration[] | null>(null);
  const [projectComposerOpen, setProjectComposerOpen] = useState(false);
  const [charterComposerOpen, setCharterComposerOpen] = useState(false);
  const [policyComposerOpen, setPolicyComposerOpen] = useState(false);
  const [repositoryComposerOpen, setRepositoryComposerOpen] = useState(false);
  const [teamComposerOpen, setTeamComposerOpen] = useState(false);
  const [missionComposerOpen, setMissionComposerOpen] = useState(false);
  const [localModelDiscovery, setLocalModelDiscovery] =
    useState<LocalModelDiscoveryReport | null>(null);
  const [localModelActivations, setLocalModelActivations] =
    useState<LocalModelActivation[] | null>(null);
  const [localModelReview, setLocalModelReview] =
    useState<LocalModelActivationReview | null>(null);
  const [discoveringLocalModels, setDiscoveringLocalModels] = useState(false);
  const localModelDiscoveryEpoch = useRef(0);
  const [recentReceiptPage, setRecentReceiptPage] = useState<RecentReceiptPage | null>(null);
  const [loadingRecentReceipts, setLoadingRecentReceipts] = useState(false);
  const [recentReceiptFailure, setRecentReceiptFailure] = useState("");
  const recentReceiptEpoch = useRef(0);
  const [liveDataEncryption, setLiveDataEncryption] =
    useState<LiveDataEncryptionReport | null>(null);
  const [recheckingLiveDataEncryption, setRecheckingLiveDataEncryption] = useState(false);
  const liveDataEncryptionEpoch = useRef(0);
  const [residentCapabilityCatalog, setResidentCapabilityCatalog] =
    useState<BeaconCapabilityDescriptor[] | null>(null);
  const [residentConfigurations, setResidentConfigurations] =
    useState<ResidentConfiguration[] | null>(null);
  const [loadingResidentConfigurations, setLoadingResidentConfigurations] = useState(false);
  const [themeChoice, setThemeChoice] = useState<ThemeChoice>("light");
  const [systemPrefersDark, setSystemPrefersDark] = useState(
    () =>
      typeof window.matchMedia === "function" &&
      window.matchMedia("(prefers-color-scheme: dark)").matches,
  );
  const resolvedTheme =
    themeChoice === "system" ? (systemPrefersDark ? "dark" : "light") : themeChoice;
  const unlocked = session?.phase === "unlocked" && foregroundVerified;

  function linkContext(): LinkContext {
    const phase =
      session?.phase === "unlocked" && !foregroundVerified ? "locked" : (session?.phase ?? "locked");
    const workspaceId = starterOrganization?.workspaceId ?? null;
    return {
      phase,
      workspaceId,
      projects:
        workspaceId !== null && projects !== null
          ? { workspaceId, items: projects }
          : null,
      missions:
        selectedProject !== null && missions !== null
          ? { projectId: selectedProject.id, items: missions }
          : null,
    };
  }

  function submitObjectLink(event: FormEvent) {
    event.preventDefault();
    const link = parseObjectLink(objectLinkInput.trim());
    objectLinkEpoch.current += 1;
    setPendingObjectLink(link);
    setObjectLinkMessage(describeObjectLinkOutcome(routeObjectLink(link, linkContext())));
  }

  async function copyObjectLink(link: AddressableObjectLink) {
    const text = formatObjectLink(link);
    setObjectLinkInput(text);
    try {
      if (!navigator.clipboard?.writeText) throw new Error("clipboard unavailable");
      await navigator.clipboard.writeText(text);
      setObjectLinkMessage(
        `Copied ${link.kind === "project" ? "Project" : link.kind === "mission" ? "Mission" : "Briefing"} link.`,
      );
    } catch {
      setObjectLinkMessage("Beacon could not copy the link. You can copy it from the link field.");
    }
  }

  useEffect(() => {
    if (pendingObjectLink === null) return;
    const outcome = routeObjectLink(pendingObjectLink, linkContext());
    setObjectLinkMessage(describeObjectLinkOutcome(outcome));

    if (outcome.outcome === "awaitSetup" || outcome.outcome === "awaitUnlock") return;
    if (
      outcome.outcome === "malformed" ||
      outcome.outcome === "otherWorkspace" ||
      outcome.outcome === "absent" ||
      outcome.outcome === "wrongScope"
    ) {
      setPendingObjectLink(null);
      return;
    }

    if (outcome.outcome === "notLoaded") {
      if (outcome.awaiting === "workspace") return;
      setActiveApplication("Projects");
      if (
        outcome.awaiting === "missions" &&
        pendingObjectLink.kind === "mission" &&
        selectedProject?.id !== pendingObjectLink.projectId
      ) {
        openLinkedProject(pendingObjectLink.projectId, objectLinkEpoch.current);
      }
      return;
    }

    if (outcome.link.kind === "briefing") {
      setActiveApplication("Briefing");
      setLinkedMissionId(null);
      setObjectLinkMessage("Opened Briefing from the Beacon link.");
      setPendingObjectLink(null);
      return;
    }

    setActiveApplication("Projects");
    if (selectedProject?.id !== outcome.link.projectId) {
      openLinkedProject(outcome.link.projectId, objectLinkEpoch.current);
      return;
    }

    if (outcome.link.kind === "mission") {
      setLinkedMissionId(outcome.link.missionId);
      setObjectLinkMessage("Opened the named Mission from the Beacon link.");
    } else {
      setLinkedMissionId(null);
      setObjectLinkMessage("Opened the named Project from the Beacon link.");
    }
    setPendingObjectLink(null);
  }, [
    foregroundVerified,
    missions,
    pendingObjectLink,
    projects,
    selectedProject?.id,
    session?.phase,
    starterOrganization?.workspaceId,
  ]);

  function openLinkedProject(projectId: string, expectedEpoch: number) {
    const requestKey = `${expectedEpoch}:${projectId}`;
    if (openingLinkedProject.current === requestKey) return;
    openingLinkedProject.current = requestKey;
    void openProject(projectId).then((opened) => {
      if (objectLinkEpoch.current !== expectedEpoch) return;
      if (!opened) {
        setObjectLinkMessage("Beacon could not open the Project named by that link.");
        setPendingObjectLink(null);
      }
    }).finally(() => {
      if (openingLinkedProject.current === requestKey) openingLinkedProject.current = "";
    });
  }

  // Briefing's Receipts already carry the Project id they were recorded against. Opening one is
  // the existing `openProject` read and nothing else: no extra Receipt request, no refresh, no
  // synthesised link. Only a successful, fully validated read moves the operator to Projects; a
  // refusal leaves them on Briefing with `runAction`'s error already on screen and no Project lens
  // drawn, because `openProject` writes no state until every join has agreed.
  function openProjectFromReceipt(projectId: string) {
    void openProject(projectId).then((opened) => {
      if (opened) setActiveApplication("Projects");
    });
  }

  function applyVerifiedSession(nextSession: DesktopSessionStatus, expectedEpoch: number) {
    setSession(nextSession);
    if (nextSession.phase !== "unlocked") {
      setStarterOrganization(null);
      setResidents(null);
      setProjects(null);
      setSelectedProject(null);
      setCharterHistory(null);
      setProjectPolicy(null);
      setProjectPolicyHistory(null);
      setProjectRepository(null);
    setPreflightReceipt(null);
      setPreflightReceipt(null);
      setProjectRepositoryHistory(null);
      setTeams(null);
      setMissions(null);
      setTeamConfigurations(null);
      setProjectComposerOpen(false);
      setCharterComposerOpen(false);
      setPolicyComposerOpen(false);
      setRepositoryComposerOpen(false);
      setTeamComposerOpen(false);
      setMissionComposerOpen(false);
      setActiveApplication("Briefing");
    }
    setForegroundVerified(
      nextSession.phase === "unlocked" &&
        foregroundEligible.current &&
        foregroundEpoch.current === expectedEpoch &&
        document.visibilityState !== "hidden",
    );
  }

  useEffect(() => {
    let active = true;
    const verificationEpoch = foregroundEpoch.current;
    setViewState("loading");
    Promise.all([loadSessionStatus(), loadAuthenticationCapabilities()]).then(
      ([nextSession, nextCapabilities]) => {
        if (!active) return;
        applyVerifiedSession(nextSession, verificationEpoch);
        setCapabilities(nextCapabilities);
        setViewState("ready");
      },
      () => {
        if (active) setViewState("error");
      },
    );
    return () => {
      active = false;
    };
  }, [sessionAttempt]);

  useEffect(() => {
    if (!unlocked) return;
    let active = true;
    setStarterOrganization(null);
    setResidents(null);
    setProjects(null);
    setSelectedProject(null);
    setCharterHistory(null);
    setProjectPolicy(null);
    setProjectPolicyHistory(null);
    setProjectRepository(null);
    setPreflightReceipt(null);
    setProjectRepositoryHistory(null);
    setTeams(null);
    setMissions(null);
    setTeamConfigurations(null);
    setProjectComposerOpen(false);
    setCharterComposerOpen(false);
    setPolicyComposerOpen(false);
    setRepositoryComposerOpen(false);
    setTeamComposerOpen(false);
    setMissionComposerOpen(false);
    setActiveApplication("Briefing");
    Promise.all([
      loadSessionPolicy(),
      loadPlatformSessionMonitorStatus(),
      loadStarterOrganization(),
      loadResidents(),
    ]).then(
      ([policy, monitor, organization, loadedResidents]) => {
        if (!active) return;
        setTimeoutMinutes(String(policy.inactivityTimeoutMinutes));
        setPlatformMonitor(monitor);
        setStarterOrganization(organization);
        setResidents(loadedResidents);
      },
      () => {
        if (!active) return;
        setSession(null);
        setForegroundVerified(false);
        setStarterOrganization(null);
        setResidents(null);
        setProjects(null);
        setSelectedProject(null);
        setCharterHistory(null);
        setProjectPolicy(null);
        setProjectPolicyHistory(null);
        setProjectRepository(null);
    setPreflightReceipt(null);
      setPreflightReceipt(null);
        setProjectRepositoryHistory(null);
        setTeams(null);
        setMissions(null);
        setTeamConfigurations(null);
        setProjectComposerOpen(false);
        setCharterComposerOpen(false);
        setPolicyComposerOpen(false);
        setRepositoryComposerOpen(false);
        setTeamComposerOpen(false);
        setMissionComposerOpen(false);
        setViewState("error");
      },
    );
    return () => {
      active = false;
    };
  }, [unlocked]);

  // Loaded once when the Workspace opens and re-read only on the explicit
  // control. Nothing polls, so nothing can move focus under a keyboard user.
  useEffect(() => {
    if (
      !unlocked ||
      activeApplication !== "Briefing" ||
      !starterOrganization ||
      starterOrganization.decision === "proposed" ||
      recentReceiptPage !== null ||
      loadingRecentReceipts ||
      recentReceiptFailure !== ""
    ) {
      return;
    }
    void loadRecentReceipts();
    // Same dependency shape as the Projects loader: this fires on the state it
    // reads, never on a timer.
  }, [
    activeApplication,
    loadingRecentReceipts,
    recentReceiptFailure,
    recentReceiptPage,
    starterOrganization,
    unlocked,
  ]);

  // The Installation's live-data encryption diagnostic. Read once the
  // Workspace opens and re-read only on an explicit recheck, so the warning
  // stays visible until the condition itself changes.
  useEffect(() => {
    if (!unlocked) return;
    const requestEpoch = ++liveDataEncryptionEpoch.current;
    void liveDataEncryptionStatus().then(
      (report) => {
        if (requestEpoch === liveDataEncryptionEpoch.current) {
          setLiveDataEncryption(report);
        }
      },
      // A swallowed failure left no status, no warning, no error and no
      // Recheck: the operator could not see that the diagnostic had failed,
      // and could not retry it. The warning ADR 0017 asks to stay visible
      // cannot stay visible if the first read decides silence.
      (error) => {
        if (requestEpoch === liveDataEncryptionEpoch.current) {
          // A REPORT, not a free-text line beside a missing one: the reason is
          // in the closed vocabulary and the original failure text is bounded,
          // flagged evidence. One rendering path, one shape, one bound.
          setLiveDataEncryption(failedReadReport(error, Date.now()));
        }
      },
    );
  }, [unlocked]);

  useEffect(() => {
    if (unlocked) return;
    recentReceiptEpoch.current += 1;
    setRecentReceiptPage(null);
    setLoadingRecentReceipts(false);
    setRecentReceiptFailure("");
    localModelDiscoveryEpoch.current += 1;
    setLocalModelDiscovery(null);
    setLocalModelActivations(null);
    setLocalModelReview(null);
    setDiscoveringLocalModels(false);
    liveDataEncryptionEpoch.current += 1;
    setLiveDataEncryption(null);
    setRecheckingLiveDataEncryption(false);
    setResidentCapabilityCatalog(null);
    setResidentConfigurations(null);
    setLoadingResidentConfigurations(false);
  }, [unlocked]);

  useEffect(() => {
    if (
      !unlocked ||
      activeApplication !== "Projects" ||
      !starterOrganization ||
      starterOrganization.decision === "proposed" ||
      projects !== null
    ) {
      return;
    }
    let active = true;
    void loadProjects().then(
      (loaded) => {
        if (active) setProjects(loaded);
      },
      (error) => {
        if (active) setActionError(describeLocalAccountError(error));
      },
    );
    return () => {
      active = false;
    };
  }, [activeApplication, projects, starterOrganization, unlocked]);

  useEffect(() => {
    if (!unlocked || activeApplication !== "Projects" || !selectedProject) return;
    let active = true;
    setTeams(null);
    setMissions(null);
    setTeamConfigurations(null);
    setPolicyComposerOpen(false);
    setRepositoryComposerOpen(false);
    setTeamComposerOpen(false);
    setMissionComposerOpen(false);
    void Promise.all([loadTeams(selectedProject.id), loadMissions(selectedProject.id)]).then(
      ([loadedTeams, loadedMissions]) => {
        if (!active) return;
        setTeams(loadedTeams);
        setMissions(loadedMissions);
      },
      (error) => {
        if (active) setActionError(describeLocalAccountError(error));
      },
    );
    return () => {
      active = false;
    };
  }, [activeApplication, selectedProject?.id, unlocked]);

  useEffect(() => {
    if (!unlocked) return;
    let active = true;
    let checking = false;
    const timer = window.setInterval(() => {
      if (checking) return;
      checking = true;
      void Promise.all([loadSessionStatus(), loadPlatformSessionMonitorStatus()]).then(
        ([nextSession, monitor]) => {
          if (!active) return;
          setSession(nextSession);
          if (nextSession.phase !== "unlocked") {
            setForegroundVerified(false);
            setStarterOrganization(null);
            setResidents(null);
            setProjects(null);
            setSelectedProject(null);
            setCharterHistory(null);
            setProjectPolicy(null);
            setProjectPolicyHistory(null);
            setProjectRepository(null);
    setPreflightReceipt(null);
      setPreflightReceipt(null);
            setProjectRepositoryHistory(null);
            setTeams(null);
            setMissions(null);
            setTeamConfigurations(null);
            setProjectComposerOpen(false);
            setCharterComposerOpen(false);
            setPolicyComposerOpen(false);
            setRepositoryComposerOpen(false);
            setTeamComposerOpen(false);
            setMissionComposerOpen(false);
            setActiveApplication("Briefing");
          }
          setPlatformMonitor(monitor);
        },
        () => {
          if (!active) return;
          setSession(null);
          setForegroundVerified(false);
          setStarterOrganization(null);
          setResidents(null);
          setProjects(null);
          setSelectedProject(null);
          setCharterHistory(null);
          setProjectPolicy(null);
          setProjectPolicyHistory(null);
          setProjectRepository(null);
    setPreflightReceipt(null);
      setPreflightReceipt(null);
          setProjectRepositoryHistory(null);
          setTeams(null);
          setMissions(null);
          setTeamConfigurations(null);
          setProjectComposerOpen(false);
          setCharterComposerOpen(false);
          setPolicyComposerOpen(false);
          setRepositoryComposerOpen(false);
          setTeamComposerOpen(false);
          setMissionComposerOpen(false);
          setViewState("error");
        },
      ).finally(() => {
        checking = false;
      });
    }, 1_000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [unlocked]);

  useEffect(() => {
    if (!unlocked) return;
    let active = true;
    let renewing = false;
    let lastRenewal = 0;
    const renew = () => {
      const now = Date.now();
      if (renewing || now - lastRenewal < 30_000) return;
      renewing = true;
      void recordLocalOperatorActivity().then(
        () => {
          lastRenewal = Date.now();
        },
        () => {
          if (!active) return;
          setSession(null);
          setForegroundVerified(false);
          setStarterOrganization(null);
          setResidents(null);
          setProjects(null);
          setSelectedProject(null);
          setCharterHistory(null);
          setProjectPolicy(null);
          setProjectPolicyHistory(null);
          setProjectRepository(null);
    setPreflightReceipt(null);
      setPreflightReceipt(null);
          setProjectRepositoryHistory(null);
          setTeams(null);
          setMissions(null);
          setTeamConfigurations(null);
          setProjectComposerOpen(false);
          setCharterComposerOpen(false);
          setPolicyComposerOpen(false);
          setRepositoryComposerOpen(false);
          setTeamComposerOpen(false);
          setMissionComposerOpen(false);
          setViewState("error");
        },
      ).finally(() => {
        renewing = false;
      });
    };
    document.addEventListener("pointerdown", renew);
    document.addEventListener("keydown", renew);
    return () => {
      active = false;
      document.removeEventListener("pointerdown", renew);
      document.removeEventListener("keydown", renew);
    };
  }, [unlocked]);

  useEffect(() => {
    let active = true;
    const conceal = () => {
      foregroundEligible.current = false;
      foregroundEpoch.current += 1;
      setForegroundVerified(false);
    };
    const verifyBeforeReveal = () => {
      if (document.visibilityState === "hidden") return;
      foregroundEligible.current = true;
      const verificationEpoch = ++foregroundEpoch.current;
      setForegroundVerified(false);
      void loadSessionStatus().then(
        (nextSession) => {
          if (!active) return;
          applyVerifiedSession(nextSession, verificationEpoch);
        },
        () => {
          if (!active) return;
          setSession(null);
          setForegroundVerified(false);
          setViewState("error");
        },
      );
    };
    const handleVisibilityChange = () => {
      if (document.visibilityState === "hidden") conceal();
      else verifyBeforeReveal();
    };
    window.addEventListener("blur", conceal);
    window.addEventListener("focus", verifyBeforeReveal);
    document.addEventListener("visibilitychange", handleVisibilityChange);
    return () => {
      active = false;
      window.removeEventListener("blur", conceal);
      window.removeEventListener("focus", verifyBeforeReveal);
      document.removeEventListener("visibilitychange", handleVisibilityChange);
    };
  }, []);

  useEffect(() => {
    document.documentElement.dataset.theme = resolvedTheme;
    document.documentElement.style.colorScheme = resolvedTheme;
  }, [resolvedTheme]);

  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    const updateSystemTheme = (event: MediaQueryListEvent) => setSystemPrefersDark(event.matches);
    setSystemPrefersDark(mediaQuery.matches);
    mediaQuery.addEventListener("change", updateSystemTheme);
    return () => mediaQuery.removeEventListener("change", updateSystemTheme);
  }, []);

  async function submitSetup(event: FormEvent) {
    event.preventDefault();
    const submittedPassword = password;
    const submittedConfirmation = confirmation;
    setPassword("");
    setConfirmation("");
    await runAction(async () => {
      const output = await beginLocalPasswordSetup(submittedPassword, submittedConfirmation);
      setRecoveryKey(output.recoveryKey);
    });
  }

  async function finishSetup() {
    await runAction(async () => {
      const verificationEpoch = foregroundEpoch.current;
      const nextSession = await completeLocalOperatorSetup(recoverySaved);
      setRecoveryKey("");
      setRecoverySaved(false);
      applyVerifiedSession(nextSession, verificationEpoch);
    });
  }

  async function submitSignIn(event: FormEvent) {
    event.preventDefault();
    const submittedPassword = password;
    setPassword("");
    await runAction(async () => {
      const verificationEpoch = foregroundEpoch.current;
      const nextSession = await signInWithLocalPassword(submittedPassword);
      applyVerifiedSession(nextSession, verificationEpoch);
    });
  }

  async function submitRecovery(event: FormEvent) {
    event.preventDefault();
    const submittedKey = recoveryKey;
    const submittedPassword = password;
    const submittedConfirmation = confirmation;
    setRecoveryKey("");
    setPassword("");
    setConfirmation("");
    await runAction(async () => {
      const verificationEpoch = foregroundEpoch.current;
      const output = await recoverLocalOperatorAccount(
        submittedKey,
        submittedPassword,
        submittedConfirmation,
      );
      setReplacementKey(output.recoveryKey);
      const nextSession = await loadSessionStatus();
      applyVerifiedSession(nextSession, verificationEpoch);
    });
  }

  async function lockSession() {
    await runAction(async () => {
      setSession(await lockLocalSession());
      setForegroundVerified(false);
      setStarterOrganization(null);
      setResidents(null);
      setProjects(null);
      setSelectedProject(null);
      setCharterHistory(null);
      setProjectPolicy(null);
      setProjectPolicyHistory(null);
      setProjectRepository(null);
    setPreflightReceipt(null);
      setPreflightReceipt(null);
      setProjectRepositoryHistory(null);
      setTeams(null);
      setMissions(null);
      setTeamConfigurations(null);
      setProjectComposerOpen(false);
      setCharterComposerOpen(false);
      setPolicyComposerOpen(false);
      setRepositoryComposerOpen(false);
      setTeamComposerOpen(false);
      setMissionComposerOpen(false);
      setLocalModelDiscovery(null);
      setLocalModelActivations(null);
      setLocalModelReview(null);
      setDiscoveringLocalModels(false);
      setActiveApplication("Briefing");
      setRecoveryMode(false);
      setReplacementKey("");
    });
  }

  async function checkLocalModels() {
    const requestEpoch = ++localModelDiscoveryEpoch.current;
    setDiscoveringLocalModels(true);
    setActionError("");
    try {
      const [report, activations] = await Promise.all([
        discoverLocalModels(),
        loadLocalModelActivations(),
      ]);
      if (requestEpoch === localModelDiscoveryEpoch.current) {
        setLocalModelDiscovery(report);
        setLocalModelActivations(activations);
        setLocalModelReview(null);
      }
    } catch (error) {
      if (requestEpoch === localModelDiscoveryEpoch.current) {
        setActionError(describeLocalAccountError(error));
      }
    } finally {
      if (requestEpoch === localModelDiscoveryEpoch.current) {
        setDiscoveringLocalModels(false);
      }
    }
  }

  async function recheckLiveDataEncryption() {
    const requestEpoch = ++liveDataEncryptionEpoch.current;
    setRecheckingLiveDataEncryption(true);
    try {
      const report = await liveDataEncryptionStatus();
      if (requestEpoch === liveDataEncryptionEpoch.current) {
        setLiveDataEncryption(report);
      }
    } catch (error) {
      if (requestEpoch === liveDataEncryptionEpoch.current) {
        // Both surfaces: the action error for the operator's current gesture,
        // and the panel's own report so the failure outlives the click.
        setLiveDataEncryption(failedReadReport(error, Date.now()));
        setActionError(describeLocalAccountError(error));
      }
    } finally {
      if (requestEpoch === liveDataEncryptionEpoch.current) {
        setRecheckingLiveDataEncryption(false);
      }
    }
  }

  async function reviewLocalModel(modelName: string, expectedDigest: string) {
    const requestEpoch = localModelDiscoveryEpoch.current;
    await runAction(async () => {
      const review = await inspectLocalModel(modelName, expectedDigest);
      if (requestEpoch === localModelDiscoveryEpoch.current) setLocalModelReview(review);
    });
  }

  async function activateReviewedLocalModel(
    review: LocalModelActivationReview,
    approvedFeatures: readonly ProviderFeature[],
  ) {
    const requestEpoch = localModelDiscoveryEpoch.current;
    await runAction(async () => {
      const activation = await activateLocalModel(review, approvedFeatures);
      if (requestEpoch !== localModelDiscoveryEpoch.current) return;
      setLocalModelActivations((current) => replaceLocalModelActivation(current, activation));
      if (residentConfigurations !== null) {
        setResidentConfigurations(await loadResidentConfigurations());
      }
      setLocalModelReview(null);
    });
  }

  async function deactivateActiveLocalModel(expectedActivationId: string) {
    const requestEpoch = localModelDiscoveryEpoch.current;
    await runAction(async () => {
      const activation = await deactivateLocalModel(expectedActivationId);
      if (requestEpoch !== localModelDiscoveryEpoch.current) return;
      setLocalModelActivations((current) => replaceLocalModelActivation(current, activation));
      if (residentConfigurations !== null) {
        setResidentConfigurations(await loadResidentConfigurations());
      }
      setLocalModelReview(null);
    });
  }

  async function saveSessionPolicy(event: FormEvent) {
    event.preventDefault();
    const value = Number(timeoutMinutes);
    await runAction(async () => {
      const policy = await setLocalSessionTimeout(value);
      setTimeoutMinutes(String(policy.inactivityTimeoutMinutes));
    });
  }

  async function acceptStarterRoster(residents: StarterResidentDraft[]) {
    if (!starterOrganization) return;
    await runAction(async () => {
      const organization = await acceptStarterOrganization(
        starterOrganization.proposalVersion,
        residents,
      );
      const admitted = await loadResidents();
      setStarterOrganization(organization);
      setResidents(admitted);
    });
  }

  async function discardStarterRoster() {
    if (!starterOrganization) return;
    await runAction(async () => {
      setStarterOrganization(
        await discardStarterOrganization(starterOrganization.proposalVersion),
      );
      setResidents([]);
    });
  }

  async function changeResident(
    action: (
      residentId: string,
      expectedDecisionId: string | null,
    ) => Promise<ResidentAgentView>,
    resident: ResidentAgentView,
  ) {
    await runAction(async () => {
      const changed = await action(resident.id, resident.latestDecisionId);
      setResidents((current) =>
        current?.map((item) => (item.id === changed.id ? changed : item)) ?? null,
      );
      if (residentConfigurations !== null) {
        setResidentConfigurations(await loadResidentConfigurations());
      }
    });
  }

  /** Briefing's recent Receipts: one Workspace-scoped read, bounded by the
   *  daemon.
   *
   *  This used to fan out over `projects` and merge Project-scoped pages, which
   *  put a domain join, a bound and an ordering policy in the interface --
   *  the three things AGENTS.md:173 keeps out of a UI app. The panel now
   *  renders what one Query answered and adds nothing to it.
   */
  async function loadRecentReceipts() {
    const requestEpoch = ++recentReceiptEpoch.current;
    setLoadingRecentReceipts(true);
    setRecentReceiptFailure("");
    try {
      const page = await readRecentReceipts(RECENT_RECEIPT_RUN_WINDOW);
      if (requestEpoch !== recentReceiptEpoch.current) return;
      setRecentReceiptPage(page);
    } catch (error) {
      if (requestEpoch !== recentReceiptEpoch.current) return;
      // Kept where the panel can show it, so a failed read is never mistaken
      // for a Workspace with no Receipts.
      setRecentReceiptPage(null);
      // `preflight_unavailable` is reused by the daemon for this read, and its
      // shared copy says "Beacon could not record the preflight" -- false for a
      // Query that records nothing. Translated at this seam rather than
      // reworded globally, so the preflight command keeps its own sentence.
      setRecentReceiptFailure(
        isPreflightUnavailable(error)
          ? "Beacon could not read recent Run and Receipt state."
          : describeLocalAccountError(error),
      );
    } finally {
      if (requestEpoch === recentReceiptEpoch.current) setLoadingRecentReceipts(false);
    }
  }

  async function refreshResidents() {
    await runAction(async () => {
      const [loadedResidents, configurations] = await Promise.all([
        loadResidents(),
        residentConfigurations === null
          ? Promise.resolve(null)
          : loadResidentConfigurations(),
      ]);
      setResidents(loadedResidents);
      if (configurations !== null) setResidentConfigurations(configurations);
    });
  }

  async function reviewResidentConfigurations() {
    setLoadingResidentConfigurations(true);
    await runAction(async () => {
      const [catalog, configurations, activations] = await Promise.all([
        loadResidentCapabilityCatalog(),
        loadResidentConfigurations(),
        loadLocalModelActivations(),
      ]);
      setResidentCapabilityCatalog(catalog);
      setResidentConfigurations(configurations);
      setLocalModelActivations(activations);
    });
    setLoadingResidentConfigurations(false);
  }

  async function saveResidentConfiguration(
    residentId: string,
    expectedDecisionId: string | null,
    activationId: string,
    selectedCapabilities: readonly BeaconCapability[],
  ) {
    await runAction(async () => {
      const configuration = await configureResident(
        residentId,
        expectedDecisionId,
        activationId,
        selectedCapabilities,
      );
      setResidentConfigurations((current) =>
        replaceResidentConfiguration(current, configuration),
      );
    });
  }

  async function removeResidentConfiguration(
    residentId: string,
    expectedDecisionId: string,
  ) {
    await runAction(async () => {
      const configuration = await clearResidentConfiguration(
        residentId,
        expectedDecisionId,
      );
      setResidentConfigurations((current) =>
        replaceResidentConfiguration(current, configuration),
      );
    });
  }

  async function openProject(projectId: string): Promise<boolean> {
    let opened = false;
    await runAction(async () => {
      const [project, history, policy, policyHistory, repository, repositoryHistory] =
        await Promise.all([
          loadProject(projectId),
          loadProjectCharterHistory(projectId),
          loadProjectPolicy(projectId),
          loadProjectPolicyHistory(projectId),
          loadProjectRepository(projectId),
          loadProjectRepositoryHistory(projectId),
        ]);
      if (history.at(-1)?.revision !== project.charter.revision) {
        throw new Error("Beacon received inconsistent Project Charter history.");
      }
      if ((policyHistory.at(-1)?.revision ?? null) !== (policy?.revision ?? null)) {
        throw new Error("Beacon received inconsistent Project Policy history.");
      }
      if ((repositoryHistory.at(-1)?.revision ?? null) !== (repository?.revision ?? null)) {
        throw new Error("Beacon received inconsistent Project Repository history.");
      }
      setSelectedProject(project);
      setCharterHistory(history);
      setProjectPolicy(policy);
      setProjectPolicyHistory(policyHistory);
      setProjectRepository(repository);
      setProjectRepositoryHistory(repositoryHistory);
      setTeams(null);
      setMissions(null);
      setTeamConfigurations(null);
      setProjectComposerOpen(false);
      setCharterComposerOpen(false);
      setPolicyComposerOpen(false);
      setRepositoryComposerOpen(false);
      setTeamComposerOpen(false);
      setMissionComposerOpen(false);
      setLinkedMissionId(null);
      // The preflight Receipt on screen is Project-scoped and ephemeral: CheckPreflightSection
      // speaks about "this Project" (CheckPreflight.tsx:78) and renders the stored commands and
      // repository revision without ever receiving a Project key, so leaving it in place showed
      // the Project just closed underneath the one just opened. Cleared beside the other
      // Project-scoped state, and unconditionally -- re-opening the SAME Project is a fresh read
      // too, and a Receipt from before it is stale in the same way, only harder to notice.
      //
      // Placement matters: this runs only after every Charter, Policy and Repository join has
      // agreed, so a failed or refused open leaves the existing display untouched rather than
      // blanking it on the way to an error.
      setPreflightReceipt(null);
      // Same place, same condition: this runs only after every Charter, Policy and Repository join
      // has agreed, so a REFUSED open bumps nothing and a valid A observation still in flight stays
      // publishable under A. Only an actual adoption retires the older observation.
      preflightGeneration.current += 1;
      opened = true;
    });
    return opened;
  }

  // Leave the open Project for the portfolio the owner already holds. It reads nothing: no
  // Projects re-fetch and no new navigation policy -- the list state is untouched, so returning to
  // it is free and the rows are the ones already on screen.
  //
  // It resets the transient per-Project detail and form state, plus the preflight display. That is
  // the same FAMILY `openProject` replaces when it adopts a Project, but not a literal equivalence:
  // on main `openProject` did not clear the preflight display at all -- that is a separate pending
  // change -- and neither the membership nor the order is asserted to match. Stored records are not
  // touched.
  function closeProject() {
    setSelectedProject(null);
    setCharterHistory(null);
    setProjectPolicy(null);
    setProjectPolicyHistory(null);
    setProjectRepository(null);
    setProjectRepositoryHistory(null);
    setTeams(null);
    setMissions(null);
    setTeamConfigurations(null);
    setPreflightReceipt(null);
    setProjectComposerOpen(false);
    setCharterComposerOpen(false);
    setPolicyComposerOpen(false);
    setRepositoryComposerOpen(false);
    setTeamComposerOpen(false);
    setMissionComposerOpen(false);
    setLinkedMissionId(null);
  }

  async function createProjectFromCharter(draft: ProjectDraft) {
    await runAction(async () => {
      const created = await createProject(draft);
      setProjects((current) => [
        {
          id: created.id,
          name: created.name,
          charterRevision: created.charter.revision,
          autonomyProfile: created.charter.autonomyProfile,
          createdAtMs: created.createdAtMs,
        },
        ...(current ?? []),
      ]);
      setSelectedProject(created);
      setCharterHistory([created.charter]);
      setProjectPolicy(null);
      setProjectPolicyHistory([]);
      setProjectRepository(null);
    setPreflightReceipt(null);
      setPreflightReceipt(null);
      setProjectRepositoryHistory([]);
      setTeams([]);
      setMissions([]);
      setTeamConfigurations(null);
      setProjectComposerOpen(false);
      setCharterComposerOpen(false);
      setPolicyComposerOpen(false);
      setRepositoryComposerOpen(false);
      setTeamComposerOpen(false);
      setMissionComposerOpen(false);
    });
  }

  async function publishCharterRevision(draft: ProjectCharterDraft) {
    if (!selectedProject) return;
    await runAction(async () => {
      const revised = await reviseProjectCharter(
        selectedProject.id,
        selectedProject.charter.revision,
        draft,
      );
      setSelectedProject(revised);
      setCharterHistory((current) => [...(current ?? []), revised.charter]);
      setProjectPolicy((current) =>
        current ? { ...current, appliesToCurrentCharter: false } : null,
      );
      setProjectPolicyHistory((current) =>
        current?.map((policy) => ({ ...policy, appliesToCurrentCharter: false })) ?? null,
      );
      setProjectRepository((current) =>
        current ? { ...current, appliesToCurrentCharter: false } : null,
      );
      setProjectRepositoryHistory((current) =>
        current?.map((repository) => ({ ...repository, appliesToCurrentCharter: false })) ?? null,
      );
      setProjects((current) =>
        current?.map((project) =>
          project.id === revised.id
            ? {
                ...project,
                charterRevision: revised.charter.revision,
                autonomyProfile: revised.charter.autonomyProfile,
              }
            : project,
        ) ?? null,
      );
      setCharterComposerOpen(false);
    });
  }

  async function publishTeamLeadPolicy(draft: ProjectPolicyDraft) {
    if (!selectedProject) return;
    await runAction(async () => {
      const published = await publishProjectPolicy(
        selectedProject.id,
        selectedProject.charter.revision,
        projectPolicy?.revision ?? null,
        draft,
      );
      setProjectPolicy(published);
      setProjectPolicyHistory((current) => [...(current ?? []), published]);
      setPolicyComposerOpen(false);
      setRepositoryComposerOpen(false);
    });
  }

  async function bindRepository(draft: RepositoryDraft) {
    if (!selectedProject) return;
    await runAction(async () => {
      const bound = await bindProjectRepository(
        selectedProject.id,
        selectedProject.charter.revision,
        projectRepository?.revision ?? null,
        draft,
      );
      setProjectRepository(bound);
      setProjectRepositoryHistory((current) => [...(current ?? []), bound]);
      // The Receipt described the previous binding, so it no longer applies.
      setPreflightReceipt(null);
      setRepositoryComposerOpen(false);
    });
  }

  async function preflightChecks() {
    if (!selectedProject) return;
    const generation = preflightGeneration.current;
    await runAction(async () => {
      const receipt = await preflightProjectChecks(selectedProject.id);
      // The observation is published only if the Project it was started for is still the adopted
      // one. Nothing is filtered by text and no valid result is overwritten first: a retired
      // observation simply never becomes the display.
      if (preflightGeneration.current !== generation) return;
      setPreflightReceipt(receipt);
    });
  }

  async function prepareTeamFormation() {
    await runAction(async () => {
      setTeamConfigurations(await loadResidentConfigurations());
      setCharterComposerOpen(false);
      setPolicyComposerOpen(false);
      setRepositoryComposerOpen(false);
      setMissionComposerOpen(false);
      setTeamComposerOpen(true);
    });
  }

  async function formProjectTeam(draft: TeamDraft) {
    await runAction(async () => {
      const formed = await formTeam(draft);
      setTeams((current) =>
        [...(current ?? []), formed].sort(
          (left, right) => left.formedAtMs - right.formedAtMs || left.id.localeCompare(right.id),
        ),
      );
      setTeamComposerOpen(false);
      setTeamConfigurations(null);
    });
  }

  function openMissionComposer() {
    setCharterComposerOpen(false);
    setPolicyComposerOpen(false);
    setRepositoryComposerOpen(false);
    setTeamComposerOpen(false);
    setTeamConfigurations(null);
    setMissionComposerOpen(true);
  }

  async function admitProjectMission(draft: MissionDraft) {
    await runAction(async () => {
      const admitted = await admitMission(draft);
      setMissions((current) =>
        [...(current ?? []), admitted].sort(
          (left, right) =>
            left.admittedAtMs - right.admittedAtMs || left.id.localeCompare(right.id),
        ),
      );
      setMissionComposerOpen(false);
    });
  }

  async function runAction(action: () => Promise<void>) {
    setBusy(true);
    setActionError("");
    try {
      await action();
    } catch (error) {
      setActionError(describeLocalAccountError(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <FluentProvider
      className={classes.provider}
      theme={resolvedTheme === "dark" ? darkTheme : lightTheme}
    >
      <a href="#main-content" onClick={() => mainContentRef.current?.focus()}>
        Skip to main content
      </a>
      <div className={`${classes.shell} desktop-shell`}>
        <nav className={classes.rail} aria-label="Beacon applications">
          <div className={classes.brand}>Beacon</div>
          {applications.map((application) => (
            <Button
              key={application}
              className={classes.navigationButton}
              appearance="subtle"
              aria-current={activeApplication === application ? "page" : undefined}
              disabled={
                !unlocked ||
                !availableApplications.has(application) ||
                !starterOrganization ||
                starterOrganization.decision === "proposed"
              }
              onClick={() => {
                setActionError("");
                setActiveApplication(application);
              }}
            >
              {application}
            </Button>
          ))}
        </nav>
        <main ref={mainContentRef} id="main-content" className={classes.main} tabIndex={-1}>
          {viewState === "error" ? (
            <div role="alert">
              <p>Beacon could not verify the local session.</p>
              <Button appearance="primary" onClick={() => setSessionAttempt((value) => value + 1)}>
                Check again
              </Button>
            </div>
          ) : viewState === "loading" || !session || !capabilities ? (
            <p role="status">Checking local session</p>
          ) : session.phase === "unlocked" && !foregroundVerified ? (
            <section className={classes.attention} aria-labelledby="concealed-heading">
              <h1 id="concealed-heading" className={classes.heading}>
                Beacon is concealed
              </h1>
              <p className={classes.body}>Rechecking the local session before showing Workspace data.</p>
            </section>
          ) : session.phase === "operator_setup_required" ? (
            <section className={classes.attention} aria-labelledby="setup-heading">
              <h1 id="setup-heading" className={classes.heading}>
                Set up your operator account
              </h1>
              {recoveryKey ? (
                <RecoveryKeyStep
                  classes={classes}
                  recoveryKey={recoveryKey}
                  saved={recoverySaved}
                  busy={busy}
                  error={actionError}
                  onSavedChange={setRecoverySaved}
                  onFinish={finishSetup}
                />
              ) : (
                <>
                  <p className={classes.body}>
                    The Owner account stays on this computer. Your password and Recovery Key are
                    never sent to a cloud service.
                  </p>
                  <PasskeyNotice capabilities={capabilities} className={classes.methodNotice} />
                  <PasswordForm
                    classes={classes}
                    password={password}
                    confirmation={confirmation}
                    busy={busy}
                    error={actionError}
                    submitLabel="Create recovery key"
                    onPasswordChange={setPassword}
                    onConfirmationChange={setConfirmation}
                    onSubmit={submitSetup}
                  />
                </>
              )}
            </section>
          ) : session.phase === "locked" ? (
            <section className={classes.attention} aria-labelledby="locked-heading">
              <h1 id="locked-heading" className={classes.heading}>
                Welcome back
              </h1>
              <PasskeyNotice capabilities={capabilities} className={classes.methodNotice} />
              {recoveryMode ? (
                <form className={classes.form} onSubmit={submitRecovery}>
                  <Field label="Recovery Key" required>
                    <Input
                      aria-label="Recovery Key"
                      value={recoveryKey}
                      autoComplete="off"
                      onChange={(_, data) => setRecoveryKey(data.value)}
                      required
                    />
                  </Field>
                  <PasswordFields
                    password={password}
                    confirmation={confirmation}
                    onPasswordChange={setPassword}
                    onConfirmationChange={setConfirmation}
                  />
                  {actionError && <p className={classes.actionError}>{actionError}</p>}
                  <div className={classes.actions}>
                    <Button appearance="primary" type="submit" disabled={busy}>
                      Recover account
                    </Button>
                    <Button type="button" disabled={busy} onClick={() => setRecoveryMode(false)}>
                      Back
                    </Button>
                  </div>
                </form>
              ) : (
                <form className={classes.form} onSubmit={submitSignIn}>
                  <Field label="Password" required>
                    <Input
                      aria-label="Password"
                      type="password"
                      value={password}
                      autoComplete="current-password"
                      onChange={(_, data) => setPassword(data.value)}
                      required
                    />
                  </Field>
                  {actionError && <p className={classes.actionError}>{actionError}</p>}
                  <div className={classes.actions}>
                    <Button appearance="primary" type="submit" disabled={busy}>
                      Unlock Beacon
                    </Button>
                    <Button type="button" appearance="subtle" onClick={() => setRecoveryMode(true)}>
                      Use Recovery Key
                    </Button>
                  </div>
                </form>
              )}
            </section>
          ) : !starterOrganization ? (
            <p role="status">Loading Briefing</p>
          ) : starterOrganization.decision === "proposed" ? (
            <StarterOrganizationStep
              key={starterOrganization.proposalVersion}
              classes={classes}
              organization={starterOrganization}
              busy={busy}
              error={actionError}
              onAccept={acceptStarterRoster}
              onDiscard={discardStarterRoster}
            />
          ) : activeApplication === "Projects" ? (
            <ProjectsWorkspace
              projects={projects}
              selectedProject={selectedProject}
              charterHistory={charterHistory}
              projectPolicy={projectPolicy}
              projectPolicyHistory={projectPolicyHistory}
              projectRepository={projectRepository}
              projectRepositoryHistory={projectRepositoryHistory}
              teams={teams}
              missions={missions}
              linkedMissionId={linkedMissionId}
              residents={residents}
              teamConfigurations={teamConfigurations}
              composerOpen={projectComposerOpen}
              charterComposerOpen={charterComposerOpen}
              policyComposerOpen={policyComposerOpen}
              repositoryComposerOpen={repositoryComposerOpen}
              preflightReceipt={preflightReceipt}
              teamComposerOpen={teamComposerOpen}
              missionComposerOpen={missionComposerOpen}
              busy={busy}
              error={actionError}
              onCreate={() => {
                setSelectedProject(null);
                setCharterHistory(null);
                setProjectPolicy(null);
                setProjectPolicyHistory(null);
                setProjectRepository(null);
    setPreflightReceipt(null);
      setPreflightReceipt(null);
                setProjectRepositoryHistory(null);
                setTeams(null);
                setMissions(null);
                setTeamConfigurations(null);
                setProjectComposerOpen(true);
                setCharterComposerOpen(false);
                setPolicyComposerOpen(false);
                setRepositoryComposerOpen(false);
                setTeamComposerOpen(false);
                setMissionComposerOpen(false);
              }}
              onCancelCreate={() => setProjectComposerOpen(false)}
              onOpenProject={openProject}
              onCloseProject={closeProject}
              onCopyObjectLink={(link) => void copyObjectLink(link)}
              onSubmitProject={createProjectFromCharter}
              onReviseCharter={() => {
                setPolicyComposerOpen(false);
                setRepositoryComposerOpen(false);
                setTeamComposerOpen(false);
                setTeamConfigurations(null);
                setMissionComposerOpen(false);
                setCharterComposerOpen(true);
              }}
              onCancelCharterRevision={() => setCharterComposerOpen(false)}
              onSubmitCharterRevision={publishCharterRevision}
              onOpenPolicyComposer={() => {
                setCharterComposerOpen(false);
                setRepositoryComposerOpen(false);
                setTeamComposerOpen(false);
                setTeamConfigurations(null);
                setMissionComposerOpen(false);
                setPolicyComposerOpen(true);
              }}
              onCancelPolicyComposer={() => setPolicyComposerOpen(false)}
              onSubmitProjectPolicy={(draft) => void publishTeamLeadPolicy(draft)}
              onOpenRepositoryComposer={() => {
                setCharterComposerOpen(false);
                setPolicyComposerOpen(false);
                setTeamComposerOpen(false);
                setTeamConfigurations(null);
                setMissionComposerOpen(false);
                setRepositoryComposerOpen(true);
              }}
              onCancelRepositoryComposer={() => setRepositoryComposerOpen(false)}
              onSubmitProjectRepository={(draft) => void bindRepository(draft)}
              onPreflightChecks={() => void preflightChecks()}
              onOpenTeamComposer={() => void prepareTeamFormation()}
              onCancelTeamComposer={() => {
                setTeamComposerOpen(false);
                setTeamConfigurations(null);
              }}
              onSubmitTeam={(draft) => void formProjectTeam(draft)}
              onOpenMissionComposer={openMissionComposer}
              onCancelMissionComposer={() => setMissionComposerOpen(false)}
              onSubmitMission={(draft) => void admitProjectMission(draft)}
              onSampleProjectInstalled={() => setProjects(null)}
            />
          ) : (
            <Briefing
              classes={classes}
              organization={starterOrganization}
              residents={residents}
              replacementKey={replacementKey}
              busy={busy}
              error={actionError}
              localModelDiscovery={localModelDiscovery}
              localModelActivations={localModelActivations}
              localModelReview={localModelReview}
              discoveringLocalModels={discoveringLocalModels}
              residentCapabilityCatalog={residentCapabilityCatalog}
              residentConfigurations={residentConfigurations}
              loadingResidentConfigurations={loadingResidentConfigurations}
              onCheckLocalModels={checkLocalModels}
              onReviewLocalModel={reviewLocalModel}
              onCancelLocalModelReview={() => setLocalModelReview(null)}
              onActivateLocalModel={activateReviewedLocalModel}
              onDeactivateLocalModel={deactivateActiveLocalModel}
              onReviewResidentConfigurations={reviewResidentConfigurations}
              onConfigureResident={saveResidentConfiguration}
              onClearResidentConfiguration={removeResidentConfiguration}
              onActivateResident={(resident) => changeResident(activateResident, resident)}
              onSuspendResident={(resident) => changeResident(suspendResident, resident)}
              onReactivateResident={(resident) => changeResident(reactivateResident, resident)}
              onRetireResident={(resident) => changeResident(retireResident, resident)}
              onRefreshResidents={refreshResidents}
              onOpenReceiptProject={openProjectFromReceipt}
              recentReceiptPage={recentReceiptPage}
              loadingRecentReceipts={loadingRecentReceipts}
              recentReceiptFailure={recentReceiptFailure}
              onRefreshReceipts={loadRecentReceipts}
              onLock={lockSession}
            />
          )}
        </main>
        <aside className={`${classes.inspector} desktop-inspector`} aria-labelledby="installation-heading">
          <h2 id="installation-heading" className={classes.inspectorHeading}>
            Installation
          </h2>
          <p className={classes.status}>
            Workspace access {unlocked ? "open" : "locked"}
          </p>
          <p className={classes.status}>External network activity off</p>
          {unlocked && (
            <>
              <h3 className={classes.themeHeading}>Session</h3>
              <form className={classes.settingsForm} onSubmit={saveSessionPolicy}>
                <Field label="Lock after inactivity" hint="Minutes, from 1 to 1,440">
                  <Input
                    aria-label="Lock after inactivity in minutes"
                    type="number"
                    min={1}
                    max={1440}
                    value={timeoutMinutes}
                    onChange={(_, data) => setTimeoutMinutes(data.value)}
                    required
                  />
                </Field>
                <Button type="submit" size="small" disabled={busy}>
                  Save timeout
                </Button>
              </form>
              {platformMonitor &&
                (platformMonitor.screenLock === "unavailable" ||
                  platformMonitor.suspend === "unavailable") && (
                  <p className={classes.monitorWarning} role="status">
                    Automatic OS lock monitoring is unavailable. Manual and inactivity locking
                    remain active.
                  </p>
                )}
              {platformMonitor &&
                platformMonitor.screenLock !== "unavailable" &&
                platformMonitor.suspend !== "unavailable" && (
                  <p className={classes.status}>
                    OS lock monitoring {platformMonitor.screenLock === "active" &&
                    platformMonitor.suspend === "active"
                      ? "active"
                      : "starting"}
                  </p>
                )}
              <LiveDataEncryptionStatus
                report={liveDataEncryption}
                busy={recheckingLiveDataEncryption}
                onRecheck={recheckLiveDataEncryption}
              />
            </>
          )}
          <h3 className={classes.themeHeading}>Appearance</h3>
          <div className={classes.themeControls} role="group" aria-label="Appearance">
            {themeChoices.map((choice) => (
              <Button
                key={choice.value}
                aria-label={choice.accessibleLabel}
                aria-pressed={themeChoice === choice.value}
                appearance={themeChoice === choice.value ? "primary" : "subtle"}
                size="small"
                onClick={() => setThemeChoice(choice.value)}
              >
                {choice.label}
              </Button>
            ))}
          </div>
          <h3 className={classes.themeHeading}>Beacon links</h3>
          <form className={classes.settingsForm} onSubmit={submitObjectLink}>
            <Field
              label="Beacon link"
              hint="Paste a link copied from a Project or Mission in this Installation."
            >
              <Input
                aria-label="Beacon link"
                value={objectLinkInput}
                onChange={(_, data) => setObjectLinkInput(data.value)}
              />
            </Field>
            <Button type="submit" size="small" disabled={!session || objectLinkInput.trim() === ""}>
              Open Beacon link
            </Button>
          </form>
          {objectLinkMessage && <p role="status">{objectLinkMessage}</p>}
        </aside>
      </div>
    </FluentProvider>
  );
}

function describeObjectLinkOutcome(outcome: LinkOutcome): string {
  switch (outcome.outcome) {
    case "awaitSetup":
      return "Finish setup to open the saved Beacon link.";
    case "awaitUnlock":
      return "Unlock Beacon to open the saved Beacon link.";
    case "otherWorkspace":
      return "Refused: this Beacon link belongs to another Workspace.";
    case "absent":
      return `No ${outcome.missing === "project" ? "Project" : "Mission"} with that identity exists in this Workspace.`;
    case "notLoaded":
      return `Beacon has not evaluated ${outcome.awaiting} yet. The saved link will open after it loads.`;
    case "wrongScope":
      return "Refused: the Mission does not belong to the Project named by this Beacon link.";
    case "malformed":
      return `Beacon did not recognize that link (${outcome.reason.replaceAll("_", " ")}).`;
    case "open":
      return `Opening ${outcome.link.kind === "briefing" ? "Briefing" : outcome.link.kind === "project" ? "Project" : "Mission"}.`;
  }
}

type StyleClasses = ReturnType<typeof useStyles>;

const starterRoleLabels: Record<StarterRole, string> = {
  lead: "Lead",
  builder: "Builder",
  reviewer: "Reviewer",
  researcher: "Researcher",
};

function StarterOrganizationStep({
  classes,
  organization,
  busy,
  error,
  onAccept,
  onDiscard,
}: {
  classes: StyleClasses;
  organization: StarterOrganizationView;
  busy: boolean;
  error: string;
  onAccept: (residents: StarterResidentDraft[]) => void;
  onDiscard: () => void;
}) {
  const [included, setIncluded] = useState<Record<StarterRole, boolean>>(() =>
    Object.fromEntries(
      organization.templates.map((template) => [template.role, template.recommended]),
    ) as Record<StarterRole, boolean>,
  );
  const [displayNames, setDisplayNames] = useState<Record<StarterRole, string>>({
    lead: "",
    builder: "",
    reviewer: "",
    researcher: "",
  });
  const [confirmingDiscard, setConfirmingDiscard] = useState(false);
  const selectedCount = Object.values(included).filter(Boolean).length;

  function submit(event: FormEvent) {
    event.preventDefault();
    if (confirmingDiscard) return;
    onAccept(
      organization.templates
        .filter((template) => included[template.role])
        .map((template) => ({
          role: template.role,
          displayName: displayNames[template.role].trim() || null,
        })),
    );
  }

  return (
    <section className={classes.starterSection} aria-labelledby="starter-heading">
      <h1 id="starter-heading" className={classes.heading}>
        Build your starter organization
      </h1>
      <p className={classes.body}>
        Review the proposed Residents. Admission creates inactive identities only. No model,
        capability, budget, or execution authority is assigned here.
      </p>
      <form aria-label="Starter Organization proposal" onSubmit={submit}>
        <div className={classes.roster}>
          {organization.templates.map((template) => {
            const label = starterRoleLabels[template.role];
            return (
              <div className={`${classes.rosterRow} starter-roster-row`} key={template.role}>
                <div className={classes.roleSummary}>
                  <Checkbox
                    checked={included[template.role]}
                    disabled={busy || confirmingDiscard}
                    label={`Include ${label}`}
                    onChange={(_, data) =>
                      setIncluded((current) => ({
                        ...current,
                        [template.role]: data.checked === true,
                      }))
                    }
                  />
                  <p className={classes.roleDescription}>{template.description}</p>
                </div>
                <Field
                  className={classes.rosterField}
                  label={`${label} Display Name`}
                  hint="Optional. Identity and authority never depend on this name."
                >
                  <Input
                    aria-label={`${label} Display Name`}
                    value={displayNames[template.role]}
                    maxLength={80}
                    disabled={!included[template.role] || busy || confirmingDiscard}
                    onChange={(_, data) =>
                      setDisplayNames((current) => ({
                        ...current,
                        [template.role]: data.value,
                      }))
                    }
                  />
                </Field>
              </div>
            );
          })}
        </div>
        {error && (
          <p className={classes.actionError} role="alert">
            {error}
          </p>
        )}
        <div className={classes.actions}>
          {confirmingDiscard ? (
            <>
              <Button type="button" appearance="primary" disabled={busy} onClick={onDiscard}>
                Confirm start without Residents
              </Button>
              <Button type="button" disabled={busy} onClick={() => setConfirmingDiscard(false)}>
                Keep proposal
              </Button>
            </>
          ) : (
            <>
              <Button appearance="primary" type="submit" disabled={busy || selectedCount === 0}>
                Admit selected Residents
              </Button>
              <Button type="button" disabled={busy} onClick={() => setConfirmingDiscard(true)}>
                Start without Residents
              </Button>
            </>
          )}
        </div>
        {confirmingDiscard && (
          <p className={classes.body} role="status">
            This closes the one-time proposal without creating any Residents.
          </p>
        )}
      </form>
    </section>
  );
}

function replaceLocalModelActivation(
  current: LocalModelActivation[] | null,
  replacement: LocalModelActivation,
): LocalModelActivation[] {
  return [
    ...(current ?? []).filter(
      (activation) =>
        activation.binding.endpoint !== replacement.binding.endpoint ||
        activation.binding.modelName !== replacement.binding.modelName,
    ),
    replacement,
  ].sort(
    (left, right) =>
      left.binding.modelName.localeCompare(right.binding.modelName) ||
      left.binding.endpoint.localeCompare(right.binding.endpoint),
  );
}

function replaceResidentConfiguration(
  current: ResidentConfiguration[] | null,
  replacement: ResidentConfiguration,
): ResidentConfiguration[] {
  return [
    ...(current ?? []).filter(
      (configuration) => configuration.residentId !== replacement.residentId,
    ),
    replacement,
  ].sort((left, right) => left.residentId.localeCompare(right.residentId));
}

function Briefing({
  classes,
  organization,
  residents,
  replacementKey,
  busy,
  error,
  localModelDiscovery,
  localModelActivations,
  localModelReview,
  discoveringLocalModels,
  residentCapabilityCatalog,
  residentConfigurations,
  loadingResidentConfigurations,
  onCheckLocalModels,
  onReviewLocalModel,
  onCancelLocalModelReview,
  onActivateLocalModel,
  onDeactivateLocalModel,
  onReviewResidentConfigurations,
  onConfigureResident,
  onClearResidentConfiguration,
  onActivateResident,
  onSuspendResident,
  onReactivateResident,
  onRetireResident,
  onRefreshResidents,
  recentReceiptPage,
  loadingRecentReceipts,
  recentReceiptFailure,
  onRefreshReceipts,
  onOpenReceiptProject,
  onLock,
}: {
  classes: StyleClasses;
  organization: StarterOrganizationView;
  residents: ResidentAgentView[] | null;
  replacementKey: string;
  busy: boolean;
  error: string;
  localModelDiscovery: LocalModelDiscoveryReport | null;
  localModelActivations: LocalModelActivation[] | null;
  localModelReview: LocalModelActivationReview | null;
  discoveringLocalModels: boolean;
  residentCapabilityCatalog: BeaconCapabilityDescriptor[] | null;
  residentConfigurations: ResidentConfiguration[] | null;
  loadingResidentConfigurations: boolean;
  onCheckLocalModels: () => void;
  onReviewLocalModel: (modelName: string, expectedDigest: string) => void;
  onCancelLocalModelReview: () => void;
  onActivateLocalModel: (
    review: LocalModelActivationReview,
    approvedFeatures: readonly ProviderFeature[],
  ) => void;
  onDeactivateLocalModel: (expectedActivationId: string) => void;
  onReviewResidentConfigurations: () => void;
  onConfigureResident: (
    residentId: string,
    expectedDecisionId: string | null,
    activationId: string,
    capabilities: readonly BeaconCapability[],
  ) => void;
  onClearResidentConfiguration: (
    residentId: string,
    expectedDecisionId: string,
  ) => void;
  onActivateResident: (resident: ResidentAgentView) => void;
  onSuspendResident: (resident: ResidentAgentView) => void;
  onReactivateResident: (resident: ResidentAgentView) => void;
  onRetireResident: (resident: ResidentAgentView) => void;
  onRefreshResidents: () => void;
  recentReceiptPage: RecentReceiptPage | null;
  loadingRecentReceipts: boolean;
  recentReceiptFailure: string;
  onRefreshReceipts: () => void;
  onOpenReceiptProject: (projectId: string) => void;
  onLock: () => void;
}) {
  const [retirementCandidateId, setRetirementCandidateId] = useState<string | null>(null);
  const retirementCandidate =
    residents?.find((resident) => resident.id === retirementCandidateId) ?? null;
  const count = residents?.length ?? 0;
  return (
    <section className={classes.attention} aria-labelledby="briefing-heading">
      <h1 id="briefing-heading" className={classes.heading}>
        Briefing
      </h1>
      <p className={classes.body}>
        {organization.decision === "accepted"
          ? `${count} ${count === 1 ? "Resident" : "Residents"} in this Workspace`
          : "Starter organization skipped"}
      </p>
      {residents === null ? (
        <p role="status">Loading Residents</p>
      ) : count > 0 ? (
        <ul className={classes.briefingList} aria-label="Resident roster">
          {residents.map((resident) => (
            <li key={resident.id} className={classes.residentRow}>
              <span className={classes.residentSummary}>
                <span>
                  {starterRoleLabels[resident.role]}
                  {resident.displayName ? `: ${resident.displayName}` : ""}
                </span>
                <span className={classes.lifecycleStatus}>{resident.lifecycle}</span>
              </span>
              <span className={classes.residentActions}>
                {resident.lifecycle === "inactive" && (
                  <Button
                    size="small"
                    disabled={busy}
                    onClick={() => onActivateResident(resident)}
                  >
                    Activate
                  </Button>
                )}
                {resident.lifecycle === "active" && (
                  <Button
                    size="small"
                    disabled={busy}
                    onClick={() => onSuspendResident(resident)}
                  >
                    Suspend
                  </Button>
                )}
                {resident.lifecycle === "suspended" && (
                  <Button
                    size="small"
                    disabled={busy}
                    onClick={() => onReactivateResident(resident)}
                  >
                    Reactivate
                  </Button>
                )}
                {resident.lifecycle !== "retired" && (
                  <Button
                    size="small"
                    appearance="subtle"
                    disabled={busy}
                    onClick={() => setRetirementCandidateId(resident.id)}
                  >
                    Retire
                  </Button>
                )}
              </span>
            </li>
          ))}
        </ul>
      ) : null}
      <p className={classes.body}>
        Lifecycle controls availability only. No Resident has execution authority yet.
      </p>
      <Button size="small" disabled={busy || residents === null} onClick={onRefreshResidents}>
        Refresh Residents
      </Button>
      <BriefingReceipts
        page={recentReceiptPage}
        failure={recentReceiptFailure}
        loading={loadingRecentReceipts}
        disabled={busy}
        onRefresh={onRefreshReceipts}
        onOpenProject={onOpenReceiptProject}
      />
      {retirementCandidate && (
        <section
          className={classes.retirementConfirm}
          role="alertdialog"
          aria-labelledby="retire-resident-heading"
          aria-describedby="retire-resident-description"
        >
          <h2 id="retire-resident-heading">Permanently retire this Resident?</h2>
          <p id="retire-resident-description">
            {retirementCandidate.displayName ?? starterRoleLabels[retirementCandidate.role]} cannot
            be reactivated after retirement. Its identity and history remain auditable.
          </p>
          <div className={classes.actions}>
            <Button
              appearance="primary"
              disabled={busy}
              onClick={() => {
                onRetireResident(retirementCandidate);
                setRetirementCandidateId(null);
              }}
            >
              Permanently retire
            </Button>
            <Button disabled={busy} onClick={() => setRetirementCandidateId(null)}>
              Cancel
            </Button>
          </div>
        </section>
      )}
      <ResidentConfigurations
        residents={residents ?? []}
        catalog={residentCapabilityCatalog}
        configurations={residentConfigurations}
        activations={localModelActivations}
        busy={busy}
        loading={loadingResidentConfigurations}
        onLoad={onReviewResidentConfigurations}
        onConfigure={onConfigureResident}
        onClear={onClearResidentConfiguration}
      />
      <LocalModels
        report={localModelDiscovery}
        activations={localModelActivations}
        review={localModelReview}
        busy={busy || discoveringLocalModels}
        discovering={discoveringLocalModels}
        onRefresh={onCheckLocalModels}
        onReview={onReviewLocalModel}
        onCancelReview={onCancelLocalModelReview}
        onActivate={onActivateLocalModel}
        onDeactivate={onDeactivateLocalModel}
      />
      {replacementKey && (
        <>
          <p>A new one-time Recovery Key replaced the old key. Save it offline now.</p>
          <code className={classes.recoveryKey}>{replacementKey}</code>
        </>
      )}
      {error && <p className={classes.actionError}>{error}</p>}
      <div className={classes.actions}>
        <Button onClick={onLock} disabled={busy}>
          Lock Beacon
        </Button>
      </div>
    </section>
  );
}

function PasskeyNotice({
  capabilities,
  className,
}: {
  capabilities: AuthenticationCapabilities;
  className: string;
}) {
  return (
    <div className={className}>
      <strong>Passkey preferred</strong>
      <div>
        {capabilities.passkey.reason === "unsupported_linux_webkitgtk"
          ? "Passkeys are not available on this Linux build. Use the same-machine password fallback."
          : "A native passkey adapter is not available in this build. Use the same-machine password fallback."}
      </div>
    </div>
  );
}

function PasswordFields({
  password,
  confirmation,
  onPasswordChange,
  onConfirmationChange,
}: {
  password: string;
  confirmation: string;
  onPasswordChange: (value: string) => void;
  onConfirmationChange: (value: string) => void;
}) {
  return (
    <>
      <Field label="Password" hint="Use at least 15 characters. Spaces and Unicode are welcome." required>
        <Input
          aria-label="Password"
          type="password"
          value={password}
          minLength={15}
          maxLength={1024}
          autoComplete="new-password"
          onChange={(_, data) => onPasswordChange(data.value)}
          required
        />
      </Field>
      <Field label="Confirm password" required>
        <Input
          aria-label="Confirm password"
          type="password"
          value={confirmation}
          minLength={15}
          maxLength={1024}
          autoComplete="new-password"
          onChange={(_, data) => onConfirmationChange(data.value)}
          required
        />
      </Field>
    </>
  );
}

function PasswordForm({
  classes,
  password,
  confirmation,
  busy,
  error,
  submitLabel,
  onPasswordChange,
  onConfirmationChange,
  onSubmit,
}: {
  classes: StyleClasses;
  password: string;
  confirmation: string;
  busy: boolean;
  error: string;
  submitLabel: string;
  onPasswordChange: (value: string) => void;
  onConfirmationChange: (value: string) => void;
  onSubmit: (event: FormEvent) => void;
}) {
  return (
    <form className={classes.form} onSubmit={onSubmit}>
      <PasswordFields
        password={password}
        confirmation={confirmation}
        onPasswordChange={onPasswordChange}
        onConfirmationChange={onConfirmationChange}
      />
      {error && <p className={classes.actionError}>{error}</p>}
      <div className={classes.actions}>
        <Button appearance="primary" type="submit" disabled={busy}>
          {submitLabel}
        </Button>
      </div>
    </form>
  );
}

function RecoveryKeyStep({
  classes,
  recoveryKey,
  saved,
  busy,
  error,
  onSavedChange,
  onFinish,
}: {
  classes: StyleClasses;
  recoveryKey: string;
  saved: boolean;
  busy: boolean;
  error: string;
  onSavedChange: (value: boolean) => void;
  onFinish: () => void;
}) {
  return (
    <>
      <p className={classes.body}>
        Save this one-time Recovery Key somewhere offline. Beacon stores only a verifier and cannot
        show this key again.
      </p>
      <code className={classes.recoveryKey}>{recoveryKey}</code>
      <Checkbox
        checked={saved}
        label="I saved this Recovery Key offline"
        onChange={(_, data) => onSavedChange(data.checked === true)}
      />
      {error && <p className={classes.actionError}>{error}</p>}
      <div className={classes.actions}>
        <Button appearance="primary" onClick={onFinish} disabled={!saved || busy}>
          Finish setup
        </Button>
      </div>
    </>
  );
}
