import { describe, expect, it } from "vitest";

import {
  MAX_OBJECT_LINK_CHARS,
  OBJECT_LINK_SCHEME,
  parseObjectLink,
  routeObjectLink,
  type LinkContext,
  type ObjectLink,
} from "./objectLinks";
import type { MissionView, ProjectView } from "./session";

const WORKSPACE = "1a2b3c4d-5e6f-4a8b-9c0d-1e2f3a4b5c6d";
const PROJECT = "44444444-4444-4444-8444-444444444444";
const MISSION = "55555555-5555-4555-8555-555555555555";

const project = { id: PROJECT, workspaceId: WORKSPACE } as ProjectView;
const mission = { id: MISSION, workspaceId: WORKSPACE, projectId: PROJECT } as MissionView;

const context = (overrides: Partial<LinkContext> = {}): LinkContext => ({
  phase: "unlocked",
  workspaceId: WORKSPACE,
  projects: [project],
  missions: [mission],
  ...overrides,
});

const link = (raw: string) => parseObjectLink(raw);
const briefing = `${OBJECT_LINK_SCHEME}/${WORKSPACE}/briefing`;
const projectLink = `${OBJECT_LINK_SCHEME}/${WORKSPACE}/project/${PROJECT}`;
const missionLink = `${projectLink}/mission/${MISSION}`;

describe("the object-link grammar", () => {
  it("reads the three kinds that have surfaces", () => {
    expect(link(briefing)).toEqual({ kind: "briefing", workspaceId: WORKSPACE });
    expect(link(projectLink)).toEqual({ kind: "project", workspaceId: WORKSPACE, projectId: PROJECT });
    expect(link(missionLink)).toEqual({
      kind: "mission",
      workspaceId: WORKSPACE,
      projectId: PROJECT,
      missionId: MISSION,
    });
  });

  it("refuses a kind that has no surface", () => {
    // A link that parses to something nothing can open is a promise the
    // interface cannot keep. The kind set is the surface set.
    expect(link(`${OBJECT_LINK_SCHEME}/${WORKSPACE}/ruling/${"a".repeat(64)}`)).toEqual({
      kind: "malformed",
      reason: "unknown_kind",
    });
    expect(link(`${OBJECT_LINK_SCHEME}/${WORKSPACE}/project/${PROJECT}/run/${MISSION}`)).toEqual({
      kind: "malformed",
      reason: "unknown_kind",
    });
  });

  it("refuses anything that merely contains the word", () => {
    for (const raw of [
      `https://beacon/${WORKSPACE}/briefing`,
      `beacon-x:/${WORKSPACE}/briefing`,
      `${WORKSPACE}/briefing`,
      `beacon:${WORKSPACE}/briefing`,
      "",
    ]) {
      expect(link(raw)).toEqual({ kind: "malformed", reason: "wrong_scheme" });
    }
  });

  it("refuses an identifier it cannot read, rather than calling the object missing", () => {
    // A mistyped UUID and a deleted Project are different facts.
    expect(link(`${OBJECT_LINK_SCHEME}/not-a-uuid/briefing`)).toEqual({
      kind: "malformed",
      reason: "malformed_identifier",
    });
    expect(link(`${OBJECT_LINK_SCHEME}/${WORKSPACE}/project/nope`)).toEqual({
      kind: "malformed",
      reason: "malformed_identifier",
    });
    expect(link(`${projectLink}/mission/nope`)).toEqual({
      kind: "malformed",
      reason: "malformed_identifier",
    });
  });

  it("refuses what it did not understand instead of ignoring it", () => {
    expect(link(`${briefing}/extra`)).toEqual({ kind: "malformed", reason: "trailing_segments" });
    expect(link(`${missionLink}/extra`)).toEqual({ kind: "malformed", reason: "trailing_segments" });
  });

  it("checks the bound before it does the parsing", () => {
    // An operating system can hand a process an arbitrarily long argument. The
    // reason must be `too_long`, not whatever the oversized tail happens to
    // look like once it has been split.
    const oversize = `${projectLink}/${"x".repeat(MAX_OBJECT_LINK_CHARS)}`;
    expect(oversize.length).toBeGreaterThan(MAX_OBJECT_LINK_CHARS);
    expect(link(oversize)).toEqual({ kind: "malformed", reason: "too_long" });
  });

  it("never throws, whatever it is given", () => {
    for (const raw of ["", "/", "beacon:", "beacon:/", "beacon://///", "\u0000", "beacon:/…/…"]) {
      expect(() => parseObjectLink(raw)).not.toThrow();
      expect(parseObjectLink(raw).kind).toBe("malformed");
    }
  });
});

describe("routing an object link", () => {
  it("keeps setup-required and locked apart", () => {
    // The pair most likely to be collapsed by a later simplification, and the
    // operator's next action differs: complete setup, or unlock.
    const parsed = link(projectLink) as ObjectLink;
    expect(routeObjectLink(parsed, context({ phase: "operator_setup_required" }))).toEqual({
      outcome: "awaitSetup",
      retained: parsed,
    });
    expect(routeObjectLink(parsed, context({ phase: "locked" }))).toEqual({
      outcome: "awaitUnlock",
      retained: parsed,
    });
  });

  it("retains the link across authentication rather than dropping it", () => {
    const parsed = link(missionLink);
    const held = routeObjectLink(parsed, context({ phase: "locked" }));
    expect(held).toEqual({ outcome: "awaitUnlock", retained: parsed });
    // The same retained link opens once the session is unlocked; nothing about
    // it granted the unlock.
    expect(routeObjectLink(parsed, context())).toEqual({ outcome: "open", link: parsed });
  });

  it("says another Workspace, never absent", () => {
    // The object may well exist, in the Installation that owns it. Reporting it
    // as missing would be a claim this reading cannot make.
    const elsewhere = link(`${OBJECT_LINK_SCHEME}/${MISSION}/project/${PROJECT}`);
    expect(routeObjectLink(elsewhere, context())).toEqual({
      outcome: "otherWorkspace",
      workspaceId: MISSION,
    });
    // Also before this Installation's own Workspace is known.
    expect(routeObjectLink(link(projectLink), context({ workspaceId: null }))).toEqual({
      outcome: "otherWorkspace",
      workspaceId: WORKSPACE,
    });
  });

  it("names the object that is missing, and does not land on Briefing", () => {
    expect(routeObjectLink(link(projectLink), context({ projects: [] }))).toEqual({
      outcome: "absent",
      missing: "project",
      id: PROJECT,
    });
    // A Mission link whose Project is absent names the PROJECT: the Mission is
    // not the thing that is missing.
    expect(routeObjectLink(link(missionLink), context({ projects: [] }))).toEqual({
      outcome: "absent",
      missing: "project",
      id: PROJECT,
    });
    expect(routeObjectLink(link(missionLink), context({ missions: [] }))).toEqual({
      outcome: "absent",
      missing: "mission",
      id: MISSION,
    });
    // Not-yet-loaded is not emptiness, and both are honestly "absent" rather
    // than silently opening something else.
    expect(routeObjectLink(link(projectLink), context({ projects: null }))).toEqual({
      outcome: "absent",
      missing: "project",
      id: PROJECT,
    });
  });

  it("opens what it can, and only that", () => {
    for (const raw of [briefing, projectLink, missionLink]) {
      const parsed = link(raw);
      expect(routeObjectLink(parsed, context())).toEqual({ outcome: "open", link: parsed });
    }
  });

  it("carries a malformed reason through instead of turning it into absence", () => {
    expect(routeObjectLink(link("https://elsewhere"), context())).toEqual({
      outcome: "malformed",
      reason: "wrong_scheme",
    });
    // And a malformed link is refused before the session is consulted, so a
    // locked session cannot make a broken link look like something to retry.
    expect(routeObjectLink(link("https://elsewhere"), context({ phase: "locked" }))).toEqual({
      outcome: "malformed",
      reason: "wrong_scheme",
    });
  });
});
