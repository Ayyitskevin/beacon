/** The one scheme Beacon answers to, in one place.
 *
 *  Slice 1 registers nothing with the operating system: this parses a string
 *  the process already holds. Registering it externally makes it a public
 *  contract and is a separate, ruled decision -- which is exactly why the name
 *  lives in a constant a rename can move in one line. */
export const OBJECT_LINK_SCHEME = "beacon:";

/** An operating system can hand a process an arbitrarily long argument. The
 *  bound is checked BEFORE parsing, because a parser that measures after
 *  splitting has already done the work it was meant to refuse. */
export const MAX_OBJECT_LINK_CHARS = 512;

/** Why a string is not a link. "Malformed" alone cannot tell an operator
 *  whether to fix what they pasted or stop trying to use it. */
export type MalformedReason =
  | "too_long"
  | "wrong_scheme"
  | "unknown_kind"
  | "malformed_identifier"
  | "trailing_segments";

/** A link this grammar admits, or a statement of why it does not.
 *
 *  Closed, with no `unknown` arm, so a new object kind is a compile error at
 *  every consumer rather than a silent fallthrough to Briefing. */
export type ObjectLink =
  | { kind: "briefing"; workspaceId: string }
  | { kind: "project"; workspaceId: string; projectId: string }
  | { kind: "mission"; workspaceId: string; projectId: string; missionId: string }
  | { kind: "malformed"; reason: MalformedReason };

/** The session states a link can arrive in. Setup-required and locked are kept
 *  apart because the operator's next action differs. */
export type LinkPhase = "operator_setup_required" | "locked" | "unlocked";

/** What the router needs of a Project, and of a Mission -- nothing more.
 *
 *  These are declared structurally rather than imported from `session.ts`
 *  because the shell holds two different Project types: `App.tsx:286` keeps
 *  `ProjectSummary[]` from `loadProjects()`, while a selection produces a
 *  `ProjectView`. Naming either one would force the other through a cast at the
 *  wiring, and a cast is how a router ends up routing a shape nothing produces.
 *  `ProjectSummary`, `ProjectView` and `MissionView` all satisfy these, which
 *  the compile-checked fixture in the test file pins.
 *
 *  `LinkProject` REFUSES a `workspaceId` rather than merely omitting one. The
 *  reading is the scope -- `loadProjects()` returns the current Workspace's
 *  Projects -- so a row has no parent to state, and a type that silently
 *  ACCEPTED one would be ignoring a value it can see. That is the shape of
 *  guard this codebase has been bitten by three times: a check keyed on
 *  something next to the property. `workspaceId?: never` makes the erasure a
 *  compile error instead of a comment, so a `ProjectView` carrying a foreign
 *  Workspace cannot enter the reading at all. `ProjectSummary`, which states no
 *  parent, still assigns with no cast.
 *
 *  `LinkMission` is the opposite case and takes the opposite rule: `MissionView`
 *  states both parents, so both are checked. A type either states a parent and
 *  is checked on it, or cannot state one. */
export type LinkProject = { readonly id: string; readonly workspaceId?: never };
export type LinkMission = {
  readonly id: string;
  readonly workspaceId: string;
  readonly projectId: string;
};

/** The Missions the shell holds, and the Project they were loaded for.
 *
 *  `loadMissions(projectId)` is Project-scoped (`session.ts:744`), so a Mission
 *  array is evidence about exactly one Project. Carrying the array alone would
 *  let a lookup for Project B be answered out of Project A's rows -- and answer
 *  "absent" with a straight face. */
export type LoadedMissions = {
  readonly projectId: string;
  readonly items: readonly LinkMission[];
};

/** The Projects the shell holds, and the Workspace they were loaded for.
 *
 *  The same shape as `LoadedMissions`, for the same reason one layer up:
 *  "the list is the current Workspace's" was a precondition stated in prose,
 *  and prose does not refuse a reading carried over from another Workspace.
 *  Naming the provenance makes it a fact the router checks. */
export type LoadedProjects = {
  readonly workspaceId: string;
  readonly items: readonly LinkProject[];
};

export type LinkContext = {
  phase: LinkPhase;
  /** This Installation's Workspace, or null before it is known. */
  workspaceId: string | null;
  /** One Workspace's Projects, or null before they are loaded. */
  projects: LoadedProjects | null;
  /** One Project's Missions, or null before any are loaded. */
  missions: LoadedMissions | null;
};

/** What should happen to a link, as six distinct facts.
 *
 *  `absent` and `otherWorkspace` are separate because one says the object does
 *  not exist and the other says nothing at all about whether it exists.
 *  `notLoaded` is separate from BOTH because it is not a finding: the shell has
 *  not loaded that state yet, so there is no evidence to report. A null
 *  collection folded into `absent` claims an object does not exist, and a null
 *  Workspace folded into `otherWorkspace` claims the link belongs to another
 *  Installation -- definite claims drawn from nothing having been looked at.
 *  `wrongScope` is separate again: the object WAS found, and it names a parent
 *  other than the one the link declares. That is a contradiction, and it is
 *  refused rather than resolved in either direction -- opening it puts the
 *  operator on an object from another Project believing it is the one they
 *  clicked, and calling it absent denies a row that is right there.
 *  `malformed` is separate from `absent` because a mistyped identifier and a
 *  deleted Project are different facts, and collapsing absent, broken and
 *  not-checked is a defect this codebase has repaired more than once. */
export type LinkOutcome =
  | { outcome: "open"; link: Exclude<ObjectLink, { kind: "malformed" }> }
  | { outcome: "awaitSetup"; retained: ObjectLink }
  | { outcome: "awaitUnlock"; retained: ObjectLink }
  | { outcome: "otherWorkspace"; workspaceId: string }
  | { outcome: "absent"; missing: "project" | "mission"; id: string }
  | { outcome: "notLoaded"; awaiting: "workspace" | "projects" | "missions" }
  | { outcome: "wrongScope"; object: "mission"; id: string }
  | { outcome: "malformed"; reason: MalformedReason };

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

/** Total: every input maps to a variant and nothing throws.
 *
 *  `new URL()` signals failure by throwing, and an uncaught throw on the launch
 *  path takes the shell down before anything can report it -- worse than a
 *  wrong string, because the surface that would have shown the error is the one
 *  that died. So this parses by hand and returns its refusals. */
export function parseObjectLink(raw: string): ObjectLink {
  if (raw.length > MAX_OBJECT_LINK_CHARS) return { kind: "malformed", reason: "too_long" };
  if (!raw.startsWith(`${OBJECT_LINK_SCHEME}/`)) {
    return { kind: "malformed", reason: "wrong_scheme" };
  }
  const segments = raw.slice(OBJECT_LINK_SCHEME.length + 1).split("/");
  const [rawWorkspaceId, kind, rawProjectId, missionSegment, rawMissionId, ...rest] = segments;

  // The predicate below accepts either case, so the accepted SPELLING must not
  // become the object's identity: routing compares strings, and an uppercase
  // spelling of the current Workspace would report `otherWorkspace` while an
  // uppercase Project reported `absent` -- two objects where there is one.
  // Canonical lowercase is what the daemon emits, so the conversion happens
  // once here rather than at every comparison downstream. Refusing uppercase
  // was the alternative; it would tell an operator who retyped a perfectly
  // legible identifier that it is malformed.
  const workspaceId = rawWorkspaceId?.toLowerCase();
  const projectId = rawProjectId?.toLowerCase();
  const missionId = rawMissionId?.toLowerCase();
  if (!UUID.test(workspaceId ?? "")) return { kind: "malformed", reason: "malformed_identifier" };

  if (kind === "briefing") {
    return segments.length === 2
      ? { kind: "briefing", workspaceId }
      : { kind: "malformed", reason: "trailing_segments" };
  }
  if (kind !== "project") return { kind: "malformed", reason: "unknown_kind" };
  if (!UUID.test(projectId ?? "")) return { kind: "malformed", reason: "malformed_identifier" };
  if (segments.length === 3) return { kind: "project", workspaceId, projectId };

  // The Project segment is required on a Mission link: the shell selects a
  // Project and then a Mission, and no Query resolves a Mission without one.
  if (missionSegment !== "mission") return { kind: "malformed", reason: "unknown_kind" };
  if (!UUID.test(missionId ?? "")) return { kind: "malformed", reason: "malformed_identifier" };
  if (rest.length > 0) return { kind: "malformed", reason: "trailing_segments" };
  return { kind: "mission", workspaceId, projectId, missionId };
}

/** Pure: no I/O, so every outcome is reachable in a test without a store, a
 *  socket, or a rendered tree.
 *
 *  A retained link grants nothing. It is a string that selects a view;
 *  authentication happens first, as criterion 10 requires, and every panel it
 *  lands on still reads through the authenticated Queries that enforce their
 *  own scope. */
export function routeObjectLink(link: ObjectLink, context: LinkContext): LinkOutcome {
  if (link.kind === "malformed") return { outcome: "malformed", reason: link.reason };
  if (context.phase === "operator_setup_required") {
    return { outcome: "awaitSetup", retained: link };
  }
  if (context.phase === "locked") return { outcome: "awaitUnlock", retained: link };

  // Before this Installation's own Workspace is known there is nothing to
  // compare against -- which is not the same as having compared and differed.
  if (context.workspaceId === null) return { outcome: "notLoaded", awaiting: "workspace" };

  // Not this Installation's Workspace. NOT absence: the object may well exist,
  // in the Installation that owns it.
  if (context.workspaceId !== link.workspaceId) {
    return { outcome: "otherWorkspace", workspaceId: link.workspaceId };
  }
  if (link.kind === "briefing") return { outcome: "open", link };

  // Projects loaded for a different Workspace say nothing about this one.
  if (context.projects === null || context.projects.workspaceId !== link.workspaceId) {
    return { outcome: "notLoaded", awaiting: "projects" };
  }
  const project = context.projects.items.find((candidate) => candidate.id === link.projectId);
  if (!project) return { outcome: "absent", missing: "project", id: link.projectId };
  if (link.kind === "project") return { outcome: "open", link };

  // Missions loaded for a different Project say nothing about this one.
  if (context.missions === null || context.missions.projectId !== link.projectId) {
    return { outcome: "notLoaded", awaiting: "missions" };
  }
  const mission = context.missions.items.find((candidate) => candidate.id === link.missionId);
  if (!mission) return { outcome: "absent", missing: "mission", id: link.missionId };

  // Found, but it names a different parent than the link does. A Mission states
  // its Workspace and its Project, so a nested link cannot escape the parents it
  // declares by matching an id alone.
  if (mission.workspaceId !== link.workspaceId || mission.projectId !== link.projectId) {
    return { outcome: "wrongScope", object: "mission", id: link.missionId };
  }
  return { outcome: "open", link };
}
