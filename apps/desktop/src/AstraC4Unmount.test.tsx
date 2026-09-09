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

async function submitThenUnmount() {
 const user=userEvent.setup();const onRecorded=vi.fn();let done!:(v:unknown)=>void;let no!:(e:unknown)=>void;let reads=0;let writes=0;
 invokeMock.mockImplementation((cmd,args)=>{
  if(cmd === "preview_mission_completion") return Promise.resolve(previewFor((args as {request:unknown}).request));
  if(cmd === "record_mission_completion") {writes++;return new Promise((ok,bad)=>{done=ok;no=bad;});}
  if(cmd === "get_mission_completion_intent") {reads++;return Promise.resolve(null);}
  return Promise.reject(new Error("unexpected command"));
 });
 const view=render(<MissionCompletionCompose mission={mission()} onRecorded={onRecorded}/>);
 await fillMinimum(user);await user.click(screen.getByRole("button",{name:"Review completion"}));
 await screen.findByLabelText("Completion review");
 await user.click(screen.getByRole("button",{name:"Confirm this record cannot be replaced"}));
 await user.click(screen.getByRole("button",{name:"Record completion"}));
 expect(writes,"C4.UNMOUNT.SUBMITTED_CONTROL").toBe(1);
 view.unmount();return {done,no,onRecorded,getReads:()=>reads};
}
it("astra: an unmounted composer never calls its former parent on a late acknowledgement",async()=>{
 const hold=await submitThenUnmount();
 await act(async()=>{hold.done({intentSha256:TOKEN});});
 expect(hold.onRecorded,"C4.LIFETIME.UNMOUNT_RETIRES_PARENT_CALLBACK").not.toHaveBeenCalled();
});
it("astra: an unmounted composer never starts a new recovery read on a late refusal",async()=>{
 const hold=await submitThenUnmount();
 await act(async()=>{hold.no({code:"completion_unavailable"});});
 expect(hold.getReads(),"C4.LIFETIME.UNMOUNT_STARTS_NO_NEW_READ").toBe(0);
});

/* The axe census (`test/axe.test.tsx`) derives its surface list from the files that render a
   component and requires each to run the WCAG bar; root's adopted probe imports the helper without
   calling it. This is that one call -- the same addition made to the other two adopted files. The
   completion states themselves are witnessed in `MissionCompletionCompose.test.tsx`; nothing here
   duplicates that coverage. */
it("the composed surface in this file meets WCAG 2.1 AA", async () => {
  invokeMock.mockImplementation(() => new Promise<unknown>(() => {}));
  render(<MissionCompletionCompose mission={mission()} onRecorded={vi.fn()} />);
  await expectNoWcagViolations(document.body);
});
