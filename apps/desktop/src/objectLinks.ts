import type { MissionView, ProjectView } from "./session";

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

export type LinkContext = {
  phase: LinkPhase;
  /** This Installation's Workspace, or null before it is known. */
  workspaceId: string | null;
  projects: readonly ProjectView[] | null;
  missions: readonly MissionView[] | null;
};

/** What should happen to a link, as six distinct facts.
 *
 *  `absent` and `otherWorkspace` are separate because one says the object does
 *  not exist and the other says nothing at all about whether it exists.
 *  `malformed` is separate from `absent` because a mistyped identifier and a
 *  deleted Project are different facts, and collapsing absent, broken and
 *  not-checked is a defect this codebase has repaired more than once. */
export type LinkOutcome =
  | { outcome: "open"; link: Exclude<ObjectLink, { kind: "malformed" }> }
  | { outcome: "awaitSetup"; retained: ObjectLink }
  | { outcome: "awaitUnlock"; retained: ObjectLink }
  | { outcome: "otherWorkspace"; workspaceId: string }
  | { outcome: "absent"; missing: "project" | "mission"; id: string }
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
  const [workspaceId, kind, projectId, missionSegment, missionId, ...rest] = segments;
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

  // Not this Installation's Workspace. NOT absence: the object may well exist,
  // in the Installation that owns it.
  if (context.workspaceId === null || context.workspaceId !== link.workspaceId) {
    return { outcome: "otherWorkspace", workspaceId: link.workspaceId };
  }
  if (link.kind === "briefing") return { outcome: "open", link };

  const project = context.projects?.find((candidate) => candidate.id === link.projectId);
  if (!project) return { outcome: "absent", missing: "project", id: link.projectId };
  if (link.kind === "project") return { outcome: "open", link };

  const mission = context.missions?.find((candidate) => candidate.id === link.missionId);
  if (!mission) return { outcome: "absent", missing: "mission", id: link.missionId };
  return { outcome: "open", link };
}
