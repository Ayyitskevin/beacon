import { invoke } from "@tauri-apps/api/core";
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";

import { MissionClassification } from "./MissionClassification";
import type { MissionView } from "./session";
import { expectNoWcag22Violations } from "./test/axe";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

// MOCKED CONTRACT ONLY. `classify_mission_impact` and `get_mission_classification` have no owner
// behind them yet (Opus is implementing classify+observe), so green here is FRONTEND behaviour and
// never an integration claim.
const MISSION = "99999999-9999-4999-8999-9999999c5000";
const PROJECT = "22222222-2222-4222-8222-222222222222";
const RESIDENT = "44444444-4444-4444-8444-444444444444";
const CONFIG = "55555555-5555-4555-8555-555555555555";
// Above Number.MAX_SAFE_INTEGER (9007199254740991) on purpose: a cursor that rounds is a cursor that
// lies, so every assertion about it is byte-exact against this literal.
const BIG_CURSOR = "9007199254740993";

function mission(id = MISSION): MissionView {
  return {
    id,
    workspaceId: "11111111-1111-4111-8111-111111111111",
    projectId: PROJECT,
    charterRevision: 3,
    teamId: "33333333-3333-4333-8333-333333333333",
    desiredOutcome: "Ship one reviewed feature",
    successConditions: ["Gates pass"],
    constraints: [],
    prohibitions: [],
    lifecycle: "admitted",
    admittedBy: "operator",
    admittedAtMs: 126,
    members: [{ residentId: RESIDENT, admissionConfigurationId: CONFIG }],
  } as unknown as MissionView;
}

const DIGEST_V1 = "d".repeat(64);

function digest(value: string) {
  return { algorithm: "sha256", canonicalizer: "beacon.mission-impact.v1", value };
}

/** One recorded classification, as the owner's twelve-field view spells it. */
function record(fields: {
  version?: number;
  impact?: "high" | "ordinary";
  basis?: "operator_declared" | "policy_rule";
  ruleId?: string | null;
  reason?: string;
  policyRevision?: number;
  charterRevision?: number;
  digestValue?: string;
}) {
  return {
    missionId: MISSION,
    classificationId: "66666666-6666-4666-8666-666666666666",
    version: fields.version ?? 1,
    impact: fields.impact ?? "ordinary",
    basis: fields.basis ?? "operator_declared",
    ruleId: fields.ruleId ?? null,
    reason: fields.reason ?? "No operator-visible surface changed.",
    policy: policy(fields.policyRevision ?? 2, fields.charterRevision ?? 2),
    classifiedBy: "operator",
    classifiedAtMs: 1_757_000_000_000,
    supersedes: { kind: "none" },
    digest: digest(fields.digestValue ?? DIGEST_V1),
  };
}

function policy(policyRevision: number, charterRevision: number) {
  return { projectId: PROJECT, policyRevision, charterRevision };
}

/** The observation the owner's `observe` returns: four strict camelCase keys. */
function observation(fields: {
  cursor?: string;
  publishedPolicy?: ReturnType<typeof policy> | null;
  latest?: unknown;
  missionId?: string;
}) {
  return {
    missionId: fields.missionId ?? MISSION,
    observedAtEventSeq: fields.cursor ?? BIG_CURSOR,
    publishedPolicy: fields.publishedPolicy === undefined ? policy(3, 2) : fields.publishedPolicy,
    latest: fields.latest === undefined ? null : fields.latest,
  };
}

beforeEach(() => {
  invokeMock.mockReset();
});

it("announces the read, then reports absence with the Policy a new classification would cite", async () => {
  let release!: (value: unknown) => void;
  invokeMock.mockImplementation((command) => {
    if (command !== "get_mission_classification") return Promise.reject(new Error("unexpected"));
    return new Promise((resolve) => {
      release = resolve;
    });
  });
  render(<MissionClassification mission={mission()} />);

  expect(
    await screen.findByText("Reading the classification"),
    "C5S.READ.PENDING_IS_ANNOUNCED",
  ).toBeVisible();

  await release(observation({}));

  const absence = await screen.findByText(
    `No classification is recorded for this Mission as of event ${BIG_CURSOR}.`,
  );
  expect(absence, "C5S.READ.ABSENCE_IS_QUALIFIED_BY_ITS_CURSOR").toBeVisible();
  // Byte-exact: a cursor rendered through a JS number would come back 9007199254740992.
  expect(
    document.body.textContent ?? "",
    "C5S.READ.CURSOR_SURVIVES_ABOVE_MAX_SAFE_INTEGER",
  ).toContain(BIG_CURSOR);
  expect(
    screen.getByText("This classification will cite Policy revision 3 (Charter revision 2)."),
    "C5S.POLICY.THE_FORM_CITES_THE_PUBLISHED_POLICY",
  ).toBeVisible();
});

it("keeps an unreadable observation distinct from an empty one, and offers only a read", async () => {
  const user = userEvent.setup();
  const commands: string[] = [];
  invokeMock.mockImplementation((command) => {
    commands.push(String(command));
    return Promise.reject({ code: "mission_classification_unavailable" });
  });
  render(<MissionClassification mission={mission()} />);

  expect(
    await screen.findByText("The Mission classification could not be verified."),
    "C5S.READ.UNREADABLE_IS_NOT_EMPTY",
  ).toBeVisible();
  expect(
    screen.queryByText(/No classification is recorded/),
    "C5S.READ.UNREADABLE_IS_NOT_EMPTY",
  ).toBeNull();
  expect(
    screen.queryByLabelText("Impact"),
    "C5S.READ.UNREADABLE_OFFERS_NO_FORM",
  ).toBeNull();

  await user.click(screen.getByRole("button", { name: "Retry the read" }));
  expect(commands, "C5S.READ.RETRY_IS_A_READ").toEqual([
    "get_mission_classification",
    "get_mission_classification",
  ]);
});

it("shows the Policy prerequisite and no form when no Policy is published", async () => {
  const user = userEvent.setup();
  const openPolicy = vi.fn();
  invokeMock.mockImplementation(() =>
    Promise.resolve(observation({ publishedPolicy: null })),
  );
  render(<MissionClassification mission={mission()} onOpenProjectPolicy={openPolicy} />);

  expect(
    await screen.findByText(
      "No Team Lead Policy is published for this Project. A classification cites the Policy that decided it.",
    ),
    "C5S.POLICY.PREREQUISITE_ROUTES_NOT_REFUSES",
  ).toBeVisible();
  expect(
    screen.queryByLabelText("Impact"),
    "C5S.POLICY.PREREQUISITE_OFFERS_NO_FORM",
  ).toBeNull();

  await user.click(screen.getByRole("button", { name: "Open Project Policy" }));
  expect(openPolicy, "C5S.POLICY.PREREQUISITE_ROUTES_NOT_REFUSES").toHaveBeenCalledTimes(1);
});

async function choose(user: ReturnType<typeof userEvent.setup>, label: string, option: string) {
  await user.click(screen.getByLabelText(label));
  await user.click(await screen.findByRole("option", { name: option }));
}

it("sends exactly the five keys, citing the published Policy the Operator reviewed", async () => {
  const user = userEvent.setup();
  const writes: unknown[] = [];
  invokeMock.mockImplementation((command, args) => {
    if (command === "get_mission_classification") {
      // A recorded record citing Policy revision 2 while revision 3 is published: the form must echo
      // 3, the one a NEW classification would cite, never the historical 2.
      return Promise.resolve(
        observation({ publishedPolicy: policy(3, 2), latest: null }),
      );
    }
    if (command === "classify_mission_impact") {
      writes.push(args);
      return new Promise(() => {});
    }
    return Promise.reject(new Error("unexpected"));
  });
  render(<MissionClassification mission={mission()} />);
  await screen.findByText(/No classification is recorded/);

  await choose(user, "Impact", "High");
  await user.type(screen.getByLabelText("Reason"), "Touches the operator credential path.");
  await user.click(screen.getByRole("button", { name: "Record classification" }));

  expect(writes, "C5S.WRITE.FIVE_KEYS_EXACTLY").toEqual([
    {
      request: {
        missionId: MISSION,
        impact: "high",
        reason: "Touches the operator credential path.",
        expectedPolicyRevision: 3,
        supersedes: { kind: "none" },
      },
    },
  ]);
});

it("preselects no impact and requires the Operator's own reason", async () => {
  const user = userEvent.setup();
  invokeMock.mockImplementation((command) =>
    command === "get_mission_classification"
      ? Promise.resolve(observation({}))
      : Promise.reject(new Error("unexpected")),
  );
  render(<MissionClassification mission={mission()} />);
  await screen.findByText(/No classification is recorded/);

  expect(screen.getByLabelText("Impact"), "C5S.WRITE.NOTHING_IS_PRESELECTED").toHaveValue("");
  expect(screen.getByLabelText("Reason"), "C5S.WRITE.NOTHING_IS_PRESELECTED").toHaveValue("");
  expect(
    screen.getByRole("button", { name: "Record classification" }),
    "C5S.WRITE.NOTHING_IS_PRESELECTED",
  ).toBeDisabled();

  await choose(user, "Impact", "Ordinary");
  expect(
    screen.getByRole("button", { name: "Record classification" }),
    "C5S.WRITE.THE_REASON_IS_REQUIRED",
  ).toBeDisabled();

  await user.type(screen.getByLabelText("Reason"), "No operator-visible surface changed.");
  expect(
    screen.getByRole("button", { name: "Record classification" }),
    "C5S.WRITE.A_COMPLETE_JUDGMENT_CAN_BE_RECORDED",
  ).toBeEnabled();
});

it("says plainly that a later classification cannot change an earlier completion cut", async () => {
  invokeMock.mockImplementation(() => Promise.resolve(observation({})));
  render(<MissionClassification mission={mission()} />);
  expect(
    await screen.findByText(
      "A classification recorded now does not change an Outcome already cut at an earlier event.",
    ),
    "C5S.TEMPORAL.A_LATER_CLASSIFICATION_CHANGES_NO_EARLIER_CUT",
  ).toBeVisible();
});

it("keeps the recorded Policy citation in the past tense, distinct from the one a new write cites", async () => {
  invokeMock.mockImplementation(() =>
    Promise.resolve(
      observation({ publishedPolicy: policy(3, 2), latest: record({ policyRevision: 2 }) }),
    ),
  );
  render(<MissionClassification mission={mission()} />);

  expect(
    await screen.findByText(
      "Recorded Ordinary impact, version 1, citing Policy revision 2 (Charter revision 2).",
    ),
    "C5S.POLICY.HISTORICAL_CITATION_IS_NEVER_THE_NEW_ONE",
  ).toBeVisible();
  expect(
    screen.getByText("This classification will cite Policy revision 3 (Charter revision 2)."),
    "C5S.POLICY.HISTORICAL_CITATION_IS_NEVER_THE_NEW_ONE",
  ).toBeVisible();
  expect(
    screen.getByText("No operator-visible surface changed."),
    "C5S.RECORD.THE_REASON_IS_SHOWN_VERBATIM",
  ).toBeVisible();
});

it("projects a recorded policy_rule basis and its rule identity without claiming a rule ran", async () => {
  invokeMock.mockImplementation(() =>
    Promise.resolve(
      observation({
        latest: record({ basis: "policy_rule", ruleId: "security.credential-path" }),
      }),
    ),
  );
  render(<MissionClassification mission={mission()} />);

  expect(
    await screen.findByText("Recorded basis: Policy rule security.credential-path."),
    "C5S.LEGACY.RECORDED_BASIS_AND_RULE_ARE_PROJECTED",
  ).toBeVisible();
  const text = document.body.textContent ?? "";
  for (const claim of ["applied", "evaluated", "re-checked"]) {
    expect(text, "C5S.LEGACY.NO_RULE_IS_CLAIMED_TO_HAVE_RUN").not.toContain(claim);
  }
});

it("names the digest the read returned when reclassifying", async () => {
  const user = userEvent.setup();
  const writes: unknown[] = [];
  invokeMock.mockImplementation((command, args) => {
    if (command === "get_mission_classification") {
      return Promise.resolve(observation({ latest: record({ digestValue: DIGEST_V1 }) }));
    }
    if (command === "classify_mission_impact") {
      writes.push(args);
      return new Promise(() => {});
    }
    return Promise.reject(new Error("unexpected"));
  });
  render(<MissionClassification mission={mission()} />);
  await screen.findByText(/Recorded Ordinary impact/);

  await choose(user, "Impact", "High");
  await user.type(screen.getByLabelText("Reason"), "A later review found a credential path.");
  await user.click(screen.getByRole("button", { name: "Record classification" }));

  expect(writes, "C5S.WRITE.PREDECESSOR_COMES_FROM_THE_READ").toEqual([
    {
      request: {
        missionId: MISSION,
        impact: "high",
        reason: "A later review found a credential path.",
        expectedPolicyRevision: 3,
        supersedes: { kind: "record", predecessorDigest: DIGEST_V1 },
      },
    },
  ]);
});

/** Fill a complete judgment and submit it, with the write held or failing as the caller decides. */
async function submitWith(
  write: (args: unknown) => Promise<unknown>,
  reads: unknown[] = [observation({})],
) {
  const user = userEvent.setup();
  const invoked: string[] = [];
  let readIndex = 0;
  invokeMock.mockImplementation((command, args) => {
    invoked.push(String(command));
    if (command === "get_mission_classification") {
      const next = reads[Math.min(readIndex, reads.length - 1)];
      readIndex += 1;
      return Promise.resolve(next);
    }
    if (command === "classify_mission_impact") return write(args);
    return Promise.reject(new Error("unexpected"));
  });
  render(<MissionClassification mission={mission()} />);
  await screen.findByLabelText("Record impact classification");
  await choose(user, "Impact", "High");
  await user.type(screen.getByLabelText("Reason"), "Touches the operator credential path.");
  await user.click(screen.getByRole("button", { name: "Record classification" }));
  return { user, invoked };
}

it("leaves a lost reply unknown, and offers a read rather than another write", async () => {
  const { user, invoked } = await submitWith(() =>
    Promise.reject(new Error("the local request did not complete")),
  );

  expect(
    await screen.findByText("Whether your submission was recorded is still unknown."),
    "C5S.RECOVERY.UNKNOWN_IS_UNKNOWN",
  ).toBeVisible();
  const text = document.body.textContent ?? "";
  expect(text, "C5S.RECOVERY.UNKNOWN_IS_UNKNOWN").not.toContain("A classification was recorded.");
  expect(text, "C5S.RECOVERY.UNKNOWN_IS_UNKNOWN").not.toContain("could not be recorded");
  expect(
    invoked.filter((command) => command === "classify_mission_impact").length,
    "C5S.RECOVERY.NO_AUTOMATIC_SECOND_WRITE",
  ).toBe(1);

  await user.click(screen.getByRole("button", { name: "Read the classification" }));
  expect(invoked, "C5S.RECOVERY.READ_IS_THE_ONLY_FOLLOW_UP").toEqual([
    "get_mission_classification",
    "classify_mission_impact",
    "get_mission_classification",
  ]);
  expect(
    invoked.filter((command) => command === "classify_mission_impact").length,
    "C5S.RECOVERY.NO_AUTOMATIC_SECOND_WRITE",
  ).toBe(1);
});

it("treats an unverifiable write answer as unknown, not as a refusal that wrote nothing", async () => {
  await submitWith(() => Promise.reject({ code: "mission_classification_unavailable" }));
  expect(
    await screen.findByText("Whether your submission was recorded is still unknown."),
    "C5S.RECOVERY.UNVERIFIABLE_WRITE_IS_UNKNOWN",
  ).toBeVisible();
});

it("calls a matching record a match of content, never proof of this attempt", async () => {
  const matching = record({
    version: 2,
    impact: "high",
    reason: "Touches the operator credential path.",
    policyRevision: 3,
    digestValue: "e".repeat(64),
  });
  matching.supersedes = { kind: "none" } as never;
  const { user } = await submitWith(
    () => Promise.reject(new Error("lost")),
    [observation({}), observation({ latest: matching })],
  );
  await user.click(screen.getByRole("button", { name: "Read the classification" }));

  // The unknown line is present in EVERY recovery outcome, so awaiting it reaches the state without
  // deciding which line rendered; the claim itself is then a named containment assertion, which fails
  // by name instead of by `findByText` throwing.
  await screen.findByText("Whether your submission was recorded is still unknown.");
  expect(document.body.textContent ?? "", "C5S.RECOVERY.MATCH_IS_NOT_ATTRIBUTION").toContain(
    "A recorded classification matches what you submitted. This is not proof that this attempt recorded it.",
  );
});

it("reports a different latest record without claiming our write did or did not happen", async () => {
  const other = record({
    version: 3,
    impact: "ordinary",
    reason: "Someone else classified this.",
    policyRevision: 3,
    digestValue: "f".repeat(64),
  });
  other.supersedes = { kind: "none" } as never;
  const { user } = await submitWith(
    () => Promise.reject(new Error("lost")),
    [observation({}), observation({ latest: other })],
  );
  await user.click(screen.getByRole("button", { name: "Read the classification" }));

  await screen.findByText("Whether your submission was recorded is still unknown.");
  const text = document.body.textContent ?? "";
  expect(text, "C5S.RECOVERY.DIFFERENT_LATEST_IS_NOT_A_MUTATION_CLAIM").toContain(
    `The latest recorded classification for this Mission is version 3, Ordinary, as of event ${BIG_CURSOR}.`,
  );
  expect(text, "C5S.RECOVERY.DIFFERENT_LATEST_IS_NOT_A_MUTATION_CLAIM").not.toContain(
    "Nothing was replaced",
  );
});

const OTHER_MISSION = "99999999-9999-4999-8999-9999999c5111";

it("ignores an observation for the Mission the Operator has left", async () => {
  let release!: (value: unknown) => void;
  let asked = 0;
  invokeMock.mockImplementation((command, args) => {
    if (command !== "get_mission_classification") return Promise.reject(new Error("unexpected"));
    asked += 1;
    if ((args as { missionId: string }).missionId === MISSION) {
      return new Promise((resolve) => {
        release = resolve;
      });
    }
    return new Promise(() => {});
  });
  const view = render(<MissionClassification mission={mission()} />);
  await screen.findByText("Reading the classification");
  view.rerender(<MissionClassification mission={mission(OTHER_MISSION)} />);

  await act(async () => {
    release(observation({ latest: record({ version: 7 }) }));
  });
  expect(
    screen.queryByText(/Recorded Ordinary impact, version 7/),
    "C5S.LIFETIME.OLD_MISSION_ANSWER_IS_IGNORED",
  ).toBeNull();
  expect(asked, "C5S.LIFETIME.EACH_MISSION_ASKS_FOR_ITS_OWN").toBe(2);
});

it("ignores a write answer for the Mission the Operator has left", async () => {
  const user = userEvent.setup();
  let settle!: { reject: (cause: unknown) => void; resolve: (value: unknown) => void };
  invokeMock.mockImplementation((command, args) => {
    if (command === "get_mission_classification") {
      return Promise.resolve(
        observation({ missionId: (args as { missionId: string }).missionId, latest: null }),
      );
    }
    if (command === "classify_mission_impact") {
      return new Promise((resolve, reject) => {
        settle = { resolve, reject };
      });
    }
    return Promise.reject(new Error("unexpected"));
  });
  const view = render(<MissionClassification mission={mission()} />);
  await screen.findByLabelText("Record impact classification");
  await choose(user, "Impact", "High");
  await user.type(screen.getByLabelText("Reason"), "Touches the operator credential path.");
  await user.click(screen.getByRole("button", { name: "Record classification" }));
  await screen.findByText("Recording the classification");

  view.rerender(<MissionClassification mission={mission(OTHER_MISSION)} />);
  await screen.findByLabelText("Record impact classification");

  await act(async () => {
    settle.reject(new Error("lost"));
  });
  expect(
    screen.queryByText("Whether your submission was recorded is still unknown."),
    "C5S.LIFETIME.OLD_MISSION_WRITE_ANSWER_IS_IGNORED",
  ).toBeNull();
  expect(
    screen.queryByText("Recording the classification"),
    "C5S.LIFETIME.OLD_MISSION_WRITE_ANSWER_IS_IGNORED",
  ).toBeNull();
});

it("starts no read-back when a late write answer arrives after unmount", async () => {
  const user = userEvent.setup();
  let settle!: (cause: unknown) => void;
  const invoked: string[] = [];
  invokeMock.mockImplementation((command) => {
    invoked.push(String(command));
    if (command === "get_mission_classification") return Promise.resolve(observation({}));
    if (command === "classify_mission_impact") {
      return new Promise((_resolve, reject) => {
        settle = reject;
      });
    }
    return Promise.reject(new Error("unexpected"));
  });
  const view = render(<MissionClassification mission={mission()} />);
  await screen.findByLabelText("Record impact classification");
  await choose(user, "Impact", "Ordinary");
  await user.type(screen.getByLabelText("Reason"), "No operator-visible surface changed.");
  await user.click(screen.getByRole("button", { name: "Record classification" }));
  await screen.findByText("Recording the classification");

  view.unmount();
  await act(async () => {
    settle(new Error("lost"));
  });
  expect(invoked, "C5S.LIFETIME.UNMOUNT_STARTS_NO_NEW_READ").toEqual([
    "get_mission_classification",
    "classify_mission_impact",
  ]);
});

it("gives a different Mission no inherited judgment", async () => {
  const user = userEvent.setup();
  invokeMock.mockImplementation((command, args) =>
    command === "get_mission_classification"
      ? Promise.resolve(observation({ missionId: (args as { missionId: string }).missionId }))
      : Promise.reject(new Error("unexpected")),
  );
  const view = render(<MissionClassification mission={mission()} />);
  await screen.findByLabelText("Record impact classification");
  await choose(user, "Impact", "High");
  await user.type(screen.getByLabelText("Reason"), "Touches the operator credential path.");

  view.rerender(<MissionClassification mission={mission(OTHER_MISSION)} />);
  await screen.findByLabelText("Record impact classification");

  expect(
    screen.getByLabelText("Impact"),
    "C5S.LIFETIME.NEW_MISSION_INHERITS_NO_JUDGMENT",
  ).toHaveValue("");
  expect(
    screen.getByLabelText("Reason"),
    "C5S.LIFETIME.NEW_MISSION_INHERITS_NO_JUDGMENT",
  ).toHaveValue("");
});

/* The nine codes, transcribed from C5-interface.md and the corrected C5S table rather than read off
   the component: a table generated from the map under test would agree with any map. */
const CALLER_LINES: readonly (readonly [string, string])[] = [
  [
    "classification_stale_policy_revision",
    "The Project Policy changed. Review the Policy and classify again.",
  ],
  ["classification_no_policy_published", "No Team Lead Policy is published for this Project."],
  [
    "classification_already_classified",
    "A classification is already recorded for this Mission. Reclassify it by naming the recorded one.",
  ],
  [
    "classification_predecessor_not_latest",
    "That classification is no longer the latest. Read the classification and name the current one.",
  ],
  [
    "classification_predecessor_unproven",
    "The classification named as replaced could not be verified.",
  ],
  ["classification_version_ceiling_reached", "This Mission cannot record another classification."],
  ["classification_text_over_limit", "The reason exceeds the supported length."],
  [
    "classification_invalid_details",
    "Some classification details are invalid. Review them and try again.",
  ],
];

it("gives every definite refusal its exact fixed line, and keeps a session refusal truthful", async () => {
  for (const [code, line] of CALLER_LINES) {
    await submitWith(() => Promise.reject({ code }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent, `C5S.COPY.EXACT_LINE: ${code}`).toBe(line);
    cleanup();
  }
  // An existing account refusal keeps its own meaning and is never relabelled a classification fault.
  await submitWith(() => Promise.reject({ code: "authentication_failed" }));
  const alert = await screen.findByRole("alert");
  expect(alert.textContent, "C5S.COPY.SESSION_REFUSALS_STAY_TRUTHFUL").toBe(
    "That credential was not accepted.",
  );
});

it("renders the fixed line and nothing the refusal payload carried with it", async () => {
  const needle = "R42_PRIVATE_/home/operator/beacon.sqlite3";
  await submitWith(() =>
    Promise.reject({
      code: "classification_invalid_details",
      message: `${needle} column=private_column`,
      field: "supersedes.predecessorDigest",
      detail: { path: needle, sql: `SELECT * FROM mission_impact_classifications -- ${needle}` },
    }),
  );
  const alert = await screen.findByRole("alert");
  expect(alert.textContent, "C5S.COPY.RAW_PAYLOAD_IS_NOT_RENDERED").toBe(
    "Some classification details are invalid. Review them and try again.",
  );
  const text = document.body.textContent ?? "";
  expect(text, "C5S.COPY.RAW_PAYLOAD_IS_NOT_RENDERED").not.toContain(needle);
  expect(text, "C5S.COPY.RAW_PAYLOAD_IS_NOT_RENDERED").not.toContain("predecessorDigest");
});

it("shows the classification it just recorded and re-reads the snapshot once", async () => {
  const recordedView = record({ version: 2, impact: "high", reason: "Touches the operator credential path." });
  const { invoked } = await submitWith(() => Promise.resolve(recordedView));

  expect(
    await screen.findByText("A classification is recorded at version 2."),
    "C5S.RECORD.SUCCESS_IS_SHOWN_AND_REREAD",
  ).toBeVisible();
  expect(invoked, "C5S.RECORD.SUCCESS_IS_SHOWN_AND_REREAD").toEqual([
    "get_mission_classification",
    "classify_mission_impact",
    "get_mission_classification",
  ]);
});

it("is keyboard operable: every control takes focus and the form submits from the keyboard", async () => {
  const user = userEvent.setup();
  const writes: unknown[] = [];
  invokeMock.mockImplementation((command, args) => {
    if (command === "get_mission_classification") return Promise.resolve(observation({}));
    if (command === "classify_mission_impact") {
      writes.push(args);
      return new Promise(() => {});
    }
    return Promise.reject(new Error("unexpected"));
  });
  render(<MissionClassification mission={mission()} onOpenProjectPolicy={vi.fn()} />);
  await screen.findByLabelText("Record impact classification");

  // Each control takes focus. A full Tab-ORDER walk is deliberately not asserted here: Fluent's
  // Dropdown does not advance focus under jsdom (measured -- six tabs stay on its button), so a
  // tab-order assertion would be testing the widget's jsdom behaviour rather than this surface's.
  // Tab order therefore remains a MANUAL check, and nothing here claims one was done.
  for (const label of ["Impact", "Reason"]) {
    screen.getByLabelText(label).focus();
    expect(screen.getByLabelText(label), `C5S.KEYBOARD.CONTROL_TAKES_FOCUS: ${label}`).toHaveFocus();
  }

  await choose(user, "Impact", "High");
  await user.type(screen.getByLabelText("Reason"), "Touches the operator credential path.");
  const submit = screen.getByRole("button", { name: "Record classification" });
  submit.focus();
  await user.keyboard("{Enter}");
  expect(writes.length, "C5S.KEYBOARD.THE_FORM_SUBMITS_FROM_THE_KEYBOARD").toBe(1);
});

/* WCAG 2.2 AA (AGENTS.md:117) over every state this surface can actually reach. The list is the
   enumeration, not a number chosen to match prose: a state that cannot be reached is not in it. */
it("axe: every reachable state meets WCAG 2.2 AA", async () => {
  const user = userEvent.setup();

  // 1 read pending
  invokeMock.mockImplementation(() => new Promise(() => {}));
  render(<MissionClassification mission={mission()} />);
  await screen.findByText("Reading the classification");
  await expectNoWcag22Violations(document.body);
  cleanup();

  // 2 read unavailable, with its retry
  invokeMock.mockImplementation(() =>
    Promise.reject({ code: "mission_classification_unavailable" }),
  );
  render(<MissionClassification mission={mission()} />);
  await screen.findByRole("button", { name: "Retry the read" });
  await expectNoWcag22Violations(document.body);
  cleanup();

  // 3 no Policy published: the prerequisite and its navigation
  invokeMock.mockImplementation(() => Promise.resolve(observation({ publishedPolicy: null })));
  render(<MissionClassification mission={mission()} onOpenProjectPolicy={vi.fn()} />);
  await screen.findByRole("button", { name: "Open Project Policy" });
  await expectNoWcag22Violations(document.body);
  cleanup();

  // 4 Policy published, no record: absence and the form
  invokeMock.mockImplementation(() => Promise.resolve(observation({})));
  render(<MissionClassification mission={mission()} />);
  await screen.findByLabelText("Record impact classification");
  await expectNoWcag22Violations(document.body);
  cleanup();

  // 5 a recorded operator declaration
  invokeMock.mockImplementation(() => Promise.resolve(observation({ latest: record({}) })));
  render(<MissionClassification mission={mission()} />);
  await screen.findByText(/Recorded Ordinary impact/);
  await expectNoWcag22Violations(document.body);
  cleanup();

  // 6 a recorded legacy policy_rule row
  invokeMock.mockImplementation(() =>
    Promise.resolve(
      observation({ latest: record({ basis: "policy_rule", ruleId: "security.credential-path" }) }),
    ),
  );
  render(<MissionClassification mission={mission()} />);
  await screen.findByText(/Recorded basis: Policy rule/);
  await expectNoWcag22Violations(document.body);
  cleanup();

  // 7 write pending
  await submitWith(() => new Promise(() => {}));
  await screen.findByText("Recording the classification");
  await expectNoWcag22Violations(document.body);
  cleanup();

  // 8 a definite write refusal
  await submitWith(() => Promise.reject({ code: "classification_already_classified" }));
  await screen.findByRole("alert");
  await expectNoWcag22Violations(document.body);
  cleanup();

  // 9 recorded
  await submitWith(() => Promise.resolve(record({ version: 2 })));
  await screen.findByText(/A classification is recorded at version/);
  await expectNoWcag22Violations(document.body);
  cleanup();

  // 10 unknown, before any read
  const unknown = await submitWith(() => Promise.reject(new Error("lost")));
  await screen.findByText("Whether your submission was recorded is still unknown.");
  await expectNoWcag22Violations(document.body);
  // 11 unknown, read pending -> 12 unknown, read shows nothing
  await unknown.user.click(screen.getByRole("button", { name: "Read the classification" }));
  await expectNoWcag22Violations(document.body);
  cleanup();

  // 13 unknown, read shows a matching record
  const matching = record({
    version: 2,
    impact: "high",
    reason: "Touches the operator credential path.",
    policyRevision: 3,
  });
  const match = await submitWith(() => Promise.reject(new Error("lost")), [
    observation({}),
    observation({ latest: matching }),
  ]);
  await match.user.click(screen.getByRole("button", { name: "Read the classification" }));
  await screen.findByText(/matches what you submitted/);
  await expectNoWcag22Violations(document.body);
  cleanup();

  // 14 unknown, read shows a different record
  const different = await submitWith(() => Promise.reject(new Error("lost")), [
    observation({}),
    observation({ latest: record({ version: 5, reason: "Someone else classified this." }) }),
  ]);
  await different.user.click(screen.getByRole("button", { name: "Read the classification" }));
  await screen.findByText(/The latest recorded classification for this Mission is version 5/);
  await expectNoWcag22Violations(document.body);
  cleanup();

  // 15 unknown, and the read itself refused
  let reads = 0;
  const refusedRead = await submitWith(
    () => Promise.reject(new Error("lost")),
    [observation({})],
  );
  invokeMock.mockImplementation((command) => {
    if (command !== "get_mission_classification") return Promise.reject(new Error("unexpected"));
    reads += 1;
    return Promise.reject({ code: "mission_classification_unavailable" });
  });
  await refusedRead.user.click(screen.getByRole("button", { name: "Read the classification" }));
  await screen.findByText("The Mission classification could not be verified.");
  expect(reads, "C5S.AXE.THE_REFUSED_READ_STATE_IS_REACHED").toBe(1);
  await expectNoWcag22Violations(document.body);
  void user;
});

/* ------------------------------------------------------------------------------------------------
   Root's three executed probes, ADOPTED VERBATIM from
   `R13/astra-c5-frontend-probes.tsx` -- names and assertion strings unchanged, and not derived from
   this implementation. Root ran them appended to a fixture copy named
   `MissionClassification.astra-probe.test.tsx`; here they live beside the fixtures they use, so
   `-t ^name$` reproduces each one in this file instead. Their positive siblings follow each.
   ------------------------------------------------------------------------------------------------ */
it("astra_c5_unknown_submission_cannot_be_resubmitted_before_observation", async () => {
  const { user, invoked } = await submitWith(() => Promise.reject(new Error("lost after send")));
  await screen.findByText("Whether your submission was recorded is still unknown.");
  await user.click(screen.getByRole("button", { name: "Record classification" }));
  expect(invoked.filter((name) => name === "classify_mission_impact"),
    "C5.ASTRA.UNKNOWN_OFFERS_READ_NOT_REPLAY").toHaveLength(1);
});

it("astra_c5_stale_policy_refusal_offers_a_new_observation", async () => {
  await submitWith(() => Promise.reject({ code: "classification_stale_policy_revision" }));
  await screen.findByText("The Project Policy changed. Review the Policy and classify again.");
  expect(screen.queryByRole("button", { name: /(?:retry the read|read the classification|refresh|review.*policy)/i }),
    "C5.ASTRA.STALE_POLICY_HAS_AN_ACTIONABLE_READ").not.toBeNull();
});

it("astra_c5_an_observation_cannot_render_another_missions_record", async () => {
  const alien = { ...record({ reason: "This belongs to Mission B." }), missionId: OTHER_MISSION };
  invokeMock.mockResolvedValue(observation({ latest: alien }));
  render(<MissionClassification mission={mission()} />);
  await screen.findByText(/(?:This belongs to Mission B\.|request.*not.*complete|invalid|could not)/i);
  expect(document.body.textContent ?? "", "C5.ASTRA.OBSERVATION_OWNS_ITS_NESTED_MISSION")
    .not.toContain("This belongs to Mission B.");
});

it("lets a new attempt go out once a fresh observation has been read", async () => {
  const fresher = record({ version: 2, digestValue: "b".repeat(64), policyRevision: 3 });
  const writes: unknown[] = [];
  const { user } = await submitWith(
    (args) => {
      writes.push(args);
      return Promise.reject(new Error("lost after send"));
    },
    [observation({}), observation({ latest: fresher, cursor: "9007199254740994" })],
  );
  await screen.findByText("Whether your submission was recorded is still unknown.");

  await user.click(screen.getByRole("button", { name: "Read the classification" }));
  await screen.findByText(/Recorded Ordinary impact, version 2/);

  // A deliberate new judgment on the FRESH observation: enabled again, and naming what was just read.
  const again = screen.getByRole("button", { name: "Record classification" });
  expect(again, "C5S.RECOVERY.A_FRESH_OBSERVATION_REOPENS_THE_JUDGMENT").toBeEnabled();
  await user.click(again);
  expect(writes.length, "C5S.RECOVERY.A_FRESH_OBSERVATION_REOPENS_THE_JUDGMENT").toBe(2);
  expect(
    (writes[1] as { request: { supersedes: unknown; expectedPolicyRevision: number } }).request,
    "C5S.RECOVERY.THE_NEW_ATTEMPT_NAMES_WHAT_WAS_READ",
  ).toMatchObject({
    supersedes: { kind: "record", predecessorDigest: "b".repeat(64) },
    expectedPolicyRevision: 3,
  });
});

it("names the newly read Policy and predecessor after a stale-Policy refusal", async () => {
  const writes: unknown[] = [];
  const moved = record({ version: 4, digestValue: "c".repeat(64), policyRevision: 9 });
  const { user } = await submitWith(
    (args) => {
      writes.push(args);
      return writes.length === 1
        ? Promise.reject({ code: "classification_stale_policy_revision" })
        : new Promise(() => {});
    },
    [observation({}), observation({ publishedPolicy: policy(9, 2), latest: moved })],
  );
  await screen.findByText("The Project Policy changed. Review the Policy and classify again.");

  await user.click(screen.getByRole("button", { name: "Read the classification" }));
  await screen.findByText(/Recorded Ordinary impact, version 4/);
  await choose(user, "Impact", "High");
  await user.click(screen.getByRole("button", { name: "Record classification" }));

  expect(
    (writes[1] as { request: { supersedes: unknown; expectedPolicyRevision: number } }).request,
    "C5S.REFUSAL.THE_NEXT_REQUEST_NAMES_THE_NEWLY_READ_BINDINGS",
  ).toMatchObject({
    supersedes: { kind: "record", predecessorDigest: "c".repeat(64) },
    expectedPolicyRevision: 9,
  });
});

it("renders a coherent nested record, which is the positive twin of the alien one", async () => {
  invokeMock.mockImplementation(() =>
    Promise.resolve(observation({ latest: record({ reason: "This belongs to this Mission." }) })),
  );
  render(<MissionClassification mission={mission()} />);
  expect(
    await screen.findByText("This belongs to this Mission."),
    "C5S.READ.A_COHERENT_NESTED_MISSION_RENDERS",
  ).toBeVisible();
});

it("refuses an alien nested record by its own independent literal", async () => {
  const alien = { ...record({ reason: "This belongs to Mission B." }), missionId: OTHER_MISSION };
  invokeMock.mockImplementation(() => Promise.resolve(observation({ latest: alien })));
  render(<MissionClassification mission={mission()} />);

  // The literal is held here, independently of the component's own copy. It is the app-wide line, NOT
  // the guard's own message: a shape fault's internal text never reaches a caller, so
  // "Beacon received an invalid Mission classification." stays inside `session.ts` and the Operator
  // sees the generic local-request line. Measured, not assumed -- asserting the internal message was
  // this test's first RED.
  expect(
    await screen.findByText("Beacon could not complete that local request."),
    "C5S.READ.AN_ALIEN_NESTED_MISSION_IS_REFUSED",
  ).toBeVisible();
  expect(
    document.body.textContent ?? "",
    "C5S.READ.AN_ALIEN_NESTED_MISSION_IS_REFUSED",
  ).not.toContain("This belongs to Mission B.");
  expect(
    screen.queryByLabelText("Impact"),
    "C5S.READ.AN_ALIEN_NESTED_MISSION_OFFERS_NO_FORM",
  ).toBeNull();
});
