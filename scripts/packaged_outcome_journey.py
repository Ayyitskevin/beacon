#!/usr/bin/env python3
"""The packaged Outcome journey: sequencing, evidence, digest comparison, summary.

The clean-host first-run harness (`scripts/clean-host-first-run`) proves the package
*starts*. This one proves an Operator can walk a Mission from setup to the exact retained
candidate bytes and that what the store reports about those bytes is true.

Everything that decides anything lives in this module behind functions, so it is testable
without an AppImage: only `AppImageDriver` touches a process, a socket or a screen. The
driver is the single untested boundary, deliberately, and it is the piece the first live run
exercises.

Two conventions are copied from the first-run harness rather than reinvented:

* the closed step vocabulary with `not_evaluated` rows written up front, so an unrun step is
  visible as unrun and can never be read as a pass;
* fail-closed exits -- a refusal writes the summary and stops the journey, and the summary is
  the evidence either way.

One convention is deliberately NOT copied. The first-run harness reports an absent AppImage
as exit 1 with an `input: absent` matrix row; here an absent or non-executable AppImage is
exit 2, an argument refusal, because this harness has no matrix to record it in and the
caller made a mistake before the journey existed.
"""

from __future__ import annotations

import base64
import hashlib
import json
import re
import sys
from collections.abc import Iterable, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

# `crates/beacond/src/artifact.rs:49,54`. The candidate document's content digest is taken over
# the retained bytes themselves, under this canonicalizer -- not over any rendering of them.
DIGEST_ALGORITHM = "sha256"
RAW_CONTENT_CANONICALIZER = "beacon.raw.v1"

_HEX_DIGEST = re.compile(r"\A[0-9a-f]{64}\Z")

STATUSES = ("not_evaluated", "observed", "absent", "broken", "not_yet")
ROW_KEYS = frozenset(
    {"status", "order", "exit", "detail", "artifacts", "commands", "digest", "lands_in"}
)


class JourneyRefusal(Exception):
    """A step could not be observed. Carries the row the summary must record."""

    def __init__(
        self,
        step: str,
        status: str,
        detail: str,
        artifacts: Sequence[str] = (),
        commands: Sequence[dict] = (),
    ) -> None:
        super().__init__(f"{step} is {status}: {detail}")
        if status not in {"absent", "broken"}:
            raise ValueError(f"a refusal is absent or broken, not {status!r}")
        self.step = step
        self.status = status
        self.detail = detail
        self.artifacts = list(artifacts)
        self.commands = list(commands)


@dataclass(frozen=True)
class Step:
    """One row of the journey. `order` is data, because JSON objects have no order."""

    name: str
    order: int
    slug: str


@dataclass(frozen=True)
class NotYet:
    """A step of PLAN.md's F5 sequence that does not exist on main yet.

    It is a row, never an omission: a journey that simply stopped after the eighth step and a
    journey that never planned the ninth are different claims.
    """

    name: str
    order: int
    lands_in: str


# PLAN.md (block15) F5 evidence sequence, with the intermediate steps the sequence assumes.
STEPS: tuple[Step, ...] = (
    Step("operator_setup_and_sign_in", 1, "operator-setup"),
    Step("starter_organization_with_author_and_reviewer", 2, "starter-organization"),
    Step("disposable_project_installed", 3, "disposable-project"),
    Step("policy_team_and_mission", 4, "policy-team-mission"),
    Step("mission_impact_classified", 5, "classification"),
    Step("mission_completion_recorded", 6, "completion"),
    Step("outcome_candidate_prepared", 7, "candidate-prepared"),
    Step("candidate_bytes_inspected", 8, "candidate-document"),
)

NOT_YET_STEPS: tuple[NotYet, ...] = (
    NotYet(
        "evaluation_requested",
        9,
        "F3 slice 3 -- the Evaluation request task, its Run kind, permit and migration 0038",
    ),
    NotYet("evaluation_recorded", 10, "F4a -- the record-Evaluation command arm"),
    NotYet("outcome_published", 11, "F4a -- the publish-Outcome command arm"),
    NotYet(
        "outcome_saved_to_file",
        12,
        "F4 -- Markdown and PDF save; blocked on Kevin's decision 4 (the file-save dependency)",
    ),
    NotYet("restart_and_compare", 13, "F5 -- after the four rows above land"),
)

STEP_NAMES = tuple(step.name for step in STEPS)


@dataclass(frozen=True)
class CommandOutcome:
    """What the driver did and what came back. `exit` is the driver's own exit, never inferred."""

    command: str
    exit: int
    response: dict | None = None
    detail: str = ""


class JourneyDriver(Protocol):
    """The only thing that touches the packaged application."""

    def request(self, command: str, /, **params: object) -> CommandOutcome:
        """Send one authenticated local command and return its whole response envelope."""

    def observe(
        self, needle: str, target: Path, *, redact_recovery_key: bool = False
    ) -> CommandOutcome:
        """Wait for `needle` in the window's OCR text and write the screenshot to `target`."""

    def act(self, action: str) -> CommandOutcome:
        """Perform one named UI action from `UI_ACTIONS`."""

    def capture(self, target: Path) -> CommandOutcome:
        """Write a screenshot of the window with no OCR anchor."""


# The closed set of UI actions. An action outside it is a programming error, not a runtime choice.
UI_ACTIONS = ("enter_passphrase", "confirm_recovery_saved")


@dataclass
class JourneyPlan:
    """The fixture values one journey uses. No secret is ever written to evidence.

    Every draft here is submitted verbatim; nothing in this module derives an Operator
    judgment. The values are a fixture, not a default.
    """

    project_name: str = "Disposable journey Project"
    team_name: str = "Journey team"
    desired_outcome: str = "Walk one Mission from admission to its retained candidate bytes."
    success_conditions: tuple[str, ...] = ("The candidate document is retained and readable.",)
    impact: str = "ordinary"
    classification_reason: str = "A disposable journey Mission with no external effect."
    disposition: str = "concluded"
    summary: str = "Completed on the journey harness."
    spend_not_measured: str = "no meter on this disposable Mission"
    appendix_request: str = "policy_default"
    author_capabilities: tuple[str, ...] = ("artifact.produce",)
    reviewer_capabilities: tuple[str, ...] = ("artifact.evaluate",)
    # The Local model this journey activates. `None` requires the disposable host to expose exactly
    # one, so an ambiguous host is a loud refusal rather than an arbitrary pick.
    model_name: str | None = None
    # `ProviderFeature`, snake_case (`local_model_activation.rs:22-27`). Configuration REQUIRES
    # Completion (`resident_configuration.rs:400-404`).
    approved_features: tuple[str, ...] = ("completion",)


def evidence_name(order: int, slug: str, suffix: str) -> str:
    """`NN-slug.ext`, the first-run harness's own naming, with the F5 order as the prefix."""
    if not 1 <= order <= 99:
        raise ValueError(f"order outside the two-digit evidence prefix: {order}")
    if not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", slug):
        raise ValueError(f"evidence slug is not lower-kebab: {slug!r}")
    if not re.fullmatch(r"[a-z0-9]+", suffix):
        raise ValueError(f"evidence suffix is not a bare extension: {suffix!r}")
    return f"{order:02d}-{slug}.{suffix}"


def new_summary(appimage: str) -> dict:
    """Every row exists before the first step runs, so an unrun row is visibly unrun."""
    steps: dict[str, dict] = {}
    for step in STEPS:
        steps[step.name] = {
            "status": "not_evaluated",
            "order": step.order,
            "exit": None,
            "detail": "",
            "artifacts": [],
            "commands": [],
            "digest": None,
            "lands_in": None,
        }
    for pending in NOT_YET_STEPS:
        steps[pending.name] = {
            "status": "not_yet",
            "order": pending.order,
            "exit": None,
            "detail": "this step of the F5 sequence does not exist on main yet",
            "artifacts": [],
            "commands": [],
            "digest": None,
            "lands_in": pending.lands_in,
        }
    return {"appimage": appimage, "steps": steps}


def validate_summary(summary: dict) -> None:
    """The closed schema. A row that grew a key is a broken summary, not a richer one."""
    if set(summary) != {"appimage", "steps"}:
        raise ValueError(f"summary top level is not closed: {sorted(summary)}")
    steps = summary["steps"]
    expected = {step.name for step in STEPS} | {pending.name for pending in NOT_YET_STEPS}
    if set(steps) != expected:
        raise ValueError(f"summary does not name every step: {sorted(set(steps) ^ expected)}")
    orders = sorted(row["order"] for row in steps.values())
    if orders != list(range(1, len(expected) + 1)):
        raise ValueError(f"step orders are not 1..{len(expected)}: {orders}")
    for name, row in steps.items():
        if set(row) != ROW_KEYS:
            raise ValueError(f"{name} row is not closed: {sorted(set(row) ^ ROW_KEYS)}")
        if row["status"] not in STATUSES:
            raise ValueError(f"{name} has an unknown status: {row['status']!r}")
        for command in row["commands"]:
            if set(command) != {"command", "exit"}:
                raise ValueError(f"{name} has a malformed command row: {sorted(command)}")


def write_summary(path: Path, summary: dict) -> None:
    """Written after EVERY step, so an interrupted journey still leaves a truthful summary."""
    validate_summary(summary)
    with path.open("w", encoding="utf-8") as handle:
        json.dump(summary, handle, indent=2, sort_keys=True)
        handle.write("\n")


def record_step(
    summary: dict,
    step: str,
    status: str,
    *,
    exit_code: int | None,
    detail: str,
    artifacts: Sequence[str] = (),
    commands: Sequence[dict] = (),
    digest: dict | None = None,
) -> None:
    if step not in STEP_NAMES:
        raise ValueError(f"unknown step: {step!r}")
    if status not in {"observed", "absent", "broken"}:
        raise ValueError(f"a run step is observed, absent or broken, not {status!r}")
    row = summary["steps"][step]
    row["status"] = status
    row["exit"] = exit_code
    row["detail"] = detail
    row["artifacts"] = list(artifacts)
    row["commands"] = [dict(command) for command in commands]
    row["digest"] = digest


def digest_matches(expected: str, observed: str) -> bool:
    """Compare two digests as BYTES.

    Not as strings and never as lengths: every sha256 digest in this system is 64 lowercase hex
    characters, so a length comparison is true for every pair of well-formed digests and would
    accept the one thing this whole step exists to refuse.
    """
    return _digest_bytes(expected) == _digest_bytes(observed)


def _digest_bytes(value: str) -> bytes:
    if not isinstance(value, str) or not _HEX_DIGEST.fullmatch(value):
        raise JourneyRefusal(
            "candidate_bytes_inspected",
            "broken",
            f"a digest value is not 64 lowercase hex characters: {value!r}",
        )
    return bytes.fromhex(value)


def envelopes_match(expected: dict, observed: dict) -> bool:
    """A digest without its algorithm and canonicalizer is not an identity."""
    for envelope in (expected, observed):
        if set(envelope) != {"algorithm", "canonicalizer", "value"}:
            raise JourneyRefusal(
                "candidate_bytes_inspected",
                "broken",
                f"a digest envelope is not whole: {sorted(envelope)}",
            )
    return (
        expected["algorithm"] == observed["algorithm"]
        and expected["canonicalizer"] == observed["canonicalizer"]
        and digest_matches(expected["value"], observed["value"])
    )


def decode_candidate_content(document: dict) -> bytes:
    """Decode the exact retained bytes, refusing any non-canonical encoding of them.

    `base64.b64decode(validate=True)` still accepts a final quantum whose unused tail bits are
    set, which decodes to the same bytes from a different string. Re-encoding and comparing is
    the only check that refuses it, and the document's identity is its byte sequence.
    """
    encoded = document["candidateContentBase64"]
    if not isinstance(encoded, str):
        raise JourneyRefusal(
            "candidate_bytes_inspected", "broken", "candidateContentBase64 is not a string"
        )
    try:
        raw = base64.b64decode(encoded, validate=True)
    except (ValueError, base64.binascii.Error) as error:
        raise JourneyRefusal(
            "candidate_bytes_inspected", "broken", f"candidateContentBase64 is not base64: {error}"
        ) from error
    if base64.b64encode(raw).decode("ascii") != encoded:
        raise JourneyRefusal(
            "candidate_bytes_inspected",
            "broken",
            "candidateContentBase64 is not the canonical encoding of its own bytes",
        )
    return raw


def compare_candidate_bytes(prepared: dict, document: dict) -> dict:
    """The point of the eighth step: are the saved bytes the ones the store reports?

    Three independent claims, each able to fail on its own:
      1. the read document is the SAME candidate the preparation receipt described;
      2. the bytes decode canonically and are exactly as many as the store says;
      3. sha256 over those bytes IS the content digest the store reports.
    """
    step = "candidate_bytes_inspected"
    for field_name in ("missionId", "manifestId"):
        if prepared[field_name] != document[field_name]:
            raise JourneyRefusal(
                step,
                "broken",
                f"the read document is a different candidate: {field_name} "
                f"{prepared[field_name]!r} prepared, {document[field_name]!r} read",
            )
    for field_name in ("manifestDigest", "candidateArtifactDigest", "candidateContentDigest"):
        if not envelopes_match(prepared[field_name], document[field_name]):
            raise JourneyRefusal(
                step,
                "broken",
                f"{field_name} disagrees between the preparation receipt and the read document",
            )

    content_digest = document["candidateContentDigest"]
    if (
        content_digest["algorithm"] != DIGEST_ALGORITHM
        or content_digest["canonicalizer"] != RAW_CONTENT_CANONICALIZER
    ):
        raise JourneyRefusal(
            step,
            "broken",
            "the content digest is not sha256 over raw retained bytes: "
            f"{content_digest['algorithm']}/{content_digest['canonicalizer']}",
        )

    raw = decode_candidate_content(document)
    if len(raw) != document["candidateContentBytes"]:
        raise JourneyRefusal(
            step,
            "broken",
            f"the store reports {document['candidateContentBytes']} content bytes; "
            f"{len(raw)} decoded",
        )
    recomputed = hashlib.sha256(raw).hexdigest()
    if not digest_matches(content_digest["value"], recomputed):
        raise JourneyRefusal(
            step,
            "broken",
            "sha256 over the retained bytes is not the content digest the store reports",
        )
    return {
        "manifest_digest": dict(document["manifestDigest"]),
        "candidate_artifact_digest": dict(document["candidateArtifactDigest"]),
        "candidate_content_digest": dict(content_digest),
        "candidate_content_bytes": document["candidateContentBytes"],
        "recomputed_content_value": recomputed,
        "media_type": document["mediaType"],
    }


def select_local_model(models: Iterable[dict], wanted: str | None) -> dict:
    """Pick the Local model this journey activates, or refuse loudly.

    `DiscoveredLocalModel` carries `name` and `digest` (`resource_discovery.rs:50-58`), which are
    exactly what `inspect_local_model` takes. An unnamed plan requires the host to expose exactly
    one model: picking the first of several would make the journey's evidence depend on the order
    a provider happened to list its models in.
    """
    rows = list(models)
    step = "starter_organization_with_author_and_reviewer"
    if wanted is not None:
        matching = [row for row in rows if row.get("name") == wanted]
        if len(matching) != 1:
            raise JourneyRefusal(
                step,
                "absent",
                f"the plan names the Local model {wanted!r}; the host offers "
                f"{sorted(str(row.get('name')) for row in rows)}",
            )
        return matching[0]
    if len(rows) != 1:
        raise JourneyRefusal(
            step,
            "absent",
            f"the plan names no Local model and the host offers {len(rows)}, not one",
        )
    return rows[0]


def select_author_and_reviewer(residents: Iterable[dict]) -> tuple[dict, dict]:
    """Two DISTINCT Residents, one to produce and one to evaluate.

    Roles come from the accepted starter organization (`StarterRole`,
    `crates/beacond/src/starter_organization.rs:15-20`); which of them the harness calls the
    author is the harness's own selection, not an owner concept. What the owner would refuse,
    and what this refuses, is the same Resident standing on both sides.
    """
    rows = list(residents)
    reviewers = [row for row in rows if row.get("role") == "reviewer"]
    authors = [row for row in rows if row.get("role") in {"builder", "lead"}]
    if not reviewers or not authors:
        raise JourneyRefusal(
            "starter_organization_with_author_and_reviewer",
            "absent",
            "the accepted starter organization has no reviewer, or nobody to author: "
            f"roles {sorted({str(row.get('role')) for row in rows})}",
        )
    author, reviewer = authors[0], reviewers[0]
    if author["id"] == reviewer["id"]:
        raise JourneyRefusal(
            "starter_organization_with_author_and_reviewer",
            "broken",
            f"the author and the reviewer are the same Resident: {author['id']}",
        )
    return author, reviewer


# Command -> result `kind`, read from the dispatch match in
# `crates/beacond/src/local_transport.rs`. A response whose kind is not the one this command
# produces is a broken step, not a surprising success.
COMMAND_RESULT_KIND = {
    "status": "session_status",
    "starter_organization": "starter_organization",
    "accept_starter_organization": "starter_organization",
    "residents": "residents",
    "discover_local_models": "local_model_discovery",
    "inspect_local_model": "local_model_activation_review",
    "activate_local_model": "local_model_activation",
    "activate_resident": "resident",
    "configure_resident": "resident_configuration",
    "sample_project_status": "sample_project_status",
    "install_sample_project": "sample_project_install_result",
    "projects": "projects",
    "project": "project",
    "publish_project_policy": "project_policy",
    "form_team": "team",
    "admit_mission": "mission",
    "classify_mission_impact": "mission_classification",
    "preview_mission_completion": "mission_completion_preview",
    "record_mission_completion": "mission_completion_acknowledgement",
    "prepare_outcome_candidate": "prepared_outcome_candidate",
    "read_outcome_candidate_document": "outcome_candidate_document",
}

# Responses that carry a credential. Their exit is evidence; their body never is. The first-run
# harness scans evidence for a Recovery Key after the fact; this refuses to write one at all.
REDACTED_COMMANDS = frozenset({"begin_password_setup", "recover_with_key"})


class _Journey:
    """One run. Holds the summary and the current step's evidence, nothing else."""

    def __init__(
        self, driver: JourneyDriver, evidence_dir: Path, plan: JourneyPlan, appimage: str
    ) -> None:
        self.driver = driver
        self.evidence_dir = evidence_dir
        self.plan = plan
        self.summary = new_summary(appimage)
        self.summary_path = evidence_dir / "summary.json"
        self._step: Step = STEPS[0]
        self._commands: list[dict] = []
        self._artifacts: list[str] = []

    # -- evidence ---------------------------------------------------------------------------

    def _begin(self, step: Step) -> None:
        self._step = step
        self._commands = []
        self._artifacts = []

    def _write_json(self, tag: str, document: object) -> str:
        name = evidence_name(self._step.order, f"{self._step.slug}-{tag}", "json")
        with (self.evidence_dir / name).open("x", encoding="utf-8") as handle:
            json.dump(document, handle, indent=2, sort_keys=True)
            handle.write("\n")
        self._artifacts.append(name)
        return name

    def _refuse(self, status: str, detail: str) -> JourneyRefusal:
        return JourneyRefusal(self._step.name, status, detail, self._artifacts, self._commands)

    # -- driver -----------------------------------------------------------------------------

    def _request(self, tag: str, command: str, /, **params: object) -> dict:
        if command not in COMMAND_RESULT_KIND:
            raise ValueError(f"command outside the journey's vocabulary: {command!r}")
        outcome = self.driver.request(command, **params)
        self._commands.append({"command": command, "exit": outcome.exit})
        if command not in REDACTED_COMMANDS and outcome.response is not None:
            self._write_json(tag, outcome.response)
        if outcome.exit != 0:
            raise self._refuse("broken", f"{command} exited {outcome.exit}: {outcome.detail}")
        response = outcome.response
        if not isinstance(response, dict):
            raise self._refuse("broken", f"{command} returned no response envelope")
        if response.get("error") is not None:
            raise self._refuse("broken", f"{command} refused: {response['error']}")
        result = response.get("result")
        if not isinstance(result, dict):
            raise self._refuse("broken", f"{command} returned no result")
        expected = COMMAND_RESULT_KIND[command]
        if result.get("kind") != expected:
            raise self._refuse(
                "broken", f"{command} answered {result.get('kind')!r}, not {expected!r}"
            )
        return result

    def _observe(self, needle: str, tag: str, *, redact_recovery_key: bool = False) -> None:
        name = evidence_name(self._step.order, f"{self._step.slug}-{tag}", "png")
        outcome = self.driver.observe(
            needle, self.evidence_dir / name, redact_recovery_key=redact_recovery_key
        )
        self._commands.append({"command": f"observe:{needle}", "exit": outcome.exit})
        if (self.evidence_dir / name).exists():
            self._artifacts.append(name)
        if outcome.exit != 0:
            raise self._refuse("absent", f"the interface never showed {needle!r}: {outcome.detail}")

    def _act(self, action: str) -> None:
        if action not in UI_ACTIONS:
            raise ValueError(f"UI action outside the closed set: {action!r}")
        outcome = self.driver.act(action)
        self._commands.append({"command": f"act:{action}", "exit": outcome.exit})
        if outcome.exit != 0:
            raise self._refuse("broken", f"the UI action {action} exited {outcome.exit}")

    def _field(self, document: dict, *path: str) -> object:
        """Read a field that must be there. A missing one is a loud refusal, never a default."""
        cursor: object = document
        for key in path:
            if not isinstance(cursor, dict) or key not in cursor:
                raise self._refuse("broken", f"the response has no {'.'.join(path)}")
            cursor = cursor[key]
        return cursor

    def _finish(self, detail: str, digest: dict | None = None) -> None:
        record_step(
            self.summary,
            self._step.name,
            "observed",
            exit_code=0,
            detail=detail,
            artifacts=self._artifacts,
            commands=self._commands,
            digest=digest,
        )
        write_summary(self.summary_path, self.summary)

    # -- the eight steps that exist on main today -------------------------------------------

    def operator_setup_and_sign_in(self) -> None:
        self._begin(STEPS[0])
        self._observe("Set up your operator account", "form")
        self._act("enter_passphrase")
        self._observe(
            "Save this one-time Recovery Key", "recovery-redacted", redact_recovery_key=True
        )
        self._act("confirm_recovery_saved")
        self._observe("Build your starter organization", "unlocked")
        status = self._request("session", "status")
        phase = self._field(status, "status", "phase")
        access = self._field(status, "status", "workspaceAccess")
        if (phase, access) != ("unlocked", True):
            raise self._refuse(
                "broken", f"the session is {phase!r} with workspaceAccess {access!r}"
            )
        self._finish("the Operator account was created through the UI and unlocked its Workspace")

    def starter_organization_with_author_and_reviewer(self) -> None:
        self._begin(STEPS[1])
        proposal = self._request("proposal", "starter_organization")
        version = self._field(proposal, "organization", "proposalVersion")
        accepted = self._request(
            "accepted",
            "accept_starter_organization",
            proposal_version=version,
            residents=[
                {"role": "builder", "displayName": "Journey author"},
                {"role": "reviewer", "displayName": "Journey reviewer"},
            ],
        )
        decision = self._field(accepted, "organization", "decision")
        if decision != "accepted":
            raise self._refuse("broken", f"the starter organization is {decision!r}")
        residents = self._field(self._request("residents", "residents"), "residents")
        try:
            author, reviewer = select_author_and_reviewer(residents)
        except JourneyRefusal as refusal:
            raise self._refuse(refusal.status, refusal.detail) from refusal
        self.activation_id = self._activate_local_model()
        self.author = self._enable(author, self.plan.author_capabilities, "author")
        self.reviewer = self._enable(reviewer, self.plan.reviewer_capabilities, "reviewer")
        self._observe("Residents in this Workspace", "briefing")
        self._finish(
            f"two distinct Residents: author {self.author['resident_id']} produces, "
            f"reviewer {self.reviewer['resident_id']} evaluates"
        )

    def _activate_local_model(self) -> str:
        """Activate one Local model and return ITS id.

        Configuring a Resident needs a current active Local-model activation with Completion
        approved -- `configure_resident` resolves `activation_id` through `activation_by_id` and
        refuses `ModelActivationUnavailable` otherwise (`resident_configuration.rs:387-404`). That
        id is minted by `ActivateLocalModel` (`local_transport.rs:1401-1404`,
        `local_model_activation.rs:99-109`) and has nothing to do with the Resident lifecycle.

        PROVIDER PREREQUISITE: `discover_local_models` and `inspect_local_model` read the local
        provider (`local_model_activation.rs:328-337` calls discovery, then the provider's show
        endpoint). The journey therefore needs a reachable local provider -- and it GENERATES
        nothing: both are metadata reads, and no completion is ever requested.
        """
        report = self._request("discovery", "discover_local_models")
        models = self._field(report, "report", "models")
        try:
            model = select_local_model(models, self.plan.model_name)
        except JourneyRefusal as refusal:
            raise self._refuse(refusal.status, refusal.detail) from refusal
        review = self._request(
            "model-review",
            "inspect_local_model",
            model_name=model["name"],
            expected_digest=model["digest"],
        )
        activation = self._request(
            "model-activated",
            "activate_local_model",
            review=self._field(review, "review"),
            approved_features=list(self.plan.approved_features),
        )
        state = self._field(activation, "activation", "state")
        if state != "active":
            raise self._refuse("broken", f"the Local model activation is {state!r}, not active")
        return self._field(activation, "activation", "id")

    def _enable(self, resident: dict, capabilities: Sequence[str], tag: str) -> dict:
        """Activate one Resident and configure it, returning the ids a Team draft needs."""
        activated = self._request(
            f"{tag}-activated",
            "activate_resident",
            resident_id=resident["id"],
            expected_decision_id=resident.get("latestDecisionId"),
        )
        decision_id = self._field(activated, "resident", "latestDecisionId")
        lifecycle = self._field(activated, "resident", "lifecycle")
        if lifecycle != "active":
            raise self._refuse("broken", f"the {tag} Resident is {lifecycle!r}, not active")
        activation_id = self.activation_id
        # Two different owners' identities. `require_expected` compares `expected_decision_id` to
        # the latest CONFIGURATION predecessor (`resident_configuration.rs:385-386,517-523`), while
        # `activation_id` is resolved through `activation_by_id` (`:387-404`). A value that is both
        # is a threading mistake, and it is the one this journey nearly shipped: sending the
        # lifecycle decision id would have failed `StaleDecision` first and hidden the second fault.
        if activation_id == decision_id:
            raise self._refuse(
                "broken",
                "activation_id is the Resident lifecycle decision id "
                f"({decision_id!r}); it must be the Local-model activation id",
            )
        configured = self._request(
            f"{tag}-configured",
            "configure_resident",
            resident_id=resident["id"],
            # This journey configures each Resident exactly once, so the latest CONFIGURATION
            # predecessor is absent and the guard's expected value is null. A second configuration
            # would have to carry the prior configuration decision id, and this journey never does
            # one (`resident_configuration.rs:385-386,517-523`).
            expected_decision_id=None,
            activation_id=activation_id,
            capabilities=list(capabilities),
        )
        return {
            "resident_id": resident["id"],
            "configuration_id": self._field(configured, "configuration", "id"),
        }

    def disposable_project_installed(self) -> None:
        self._begin(STEPS[2])
        self._request("sample-status", "sample_project_status")
        self._request("sample-installed", "install_sample_project")
        projects = self._field(self._request("projects", "projects"), "projects")
        if not isinstance(projects, list) or len(projects) != 1:
            raise self._refuse(
                "broken",
                "the disposable store must hold exactly one Project for this journey; it holds "
                f"{len(projects) if isinstance(projects, list) else 'a non-list'}",
            )
        self.project_id = projects[0]["id"]
        project = self._request("project", "project", project_id=self.project_id)
        self.charter_revision = self._field(project, "project", "charter", "revision")
        self._capture("selected")
        self._finish(f"the installed sample Project {self.project_id} is the journey's Project")

    def policy_team_and_mission(self) -> None:
        self._begin(STEPS[3])
        policy = self._request(
            "policy",
            "publish_project_policy",
            project_id=self.project_id,
            expected_charter_revision=self.charter_revision,
            expected_policy_revision=None,
            draft={
                "teamLeadSelection": {
                    "requiredCapabilities": ["work.coordinate_mission"],
                    "availabilityRule": "current_team_eligibility",
                    "trackRecordRequirements": [],
                    "conflictRule": "require_no_declared_conflict",
                    "ambiguityRule": "require_operator_decision",
                }
            },
        )
        self.policy_revision = self._field(policy, "policy", "revision")
        members = [
            {
                "residentId": person["resident_id"],
                "expectedConfigurationId": person["configuration_id"],
            }
            for person in (self.author, self.reviewer)
        ]
        team = self._request(
            "team",
            "form_team",
            draft={
                "projectId": self.project_id,
                "name": self.plan.team_name,
                "members": members,
            },
        )
        self.team_id = self._field(team, "team", "id")
        mission = self._request(
            "mission",
            "admit_mission",
            draft={
                "projectId": self.project_id,
                "expectedCharterRevision": self.charter_revision,
                "teamId": self.team_id,
                "desiredOutcome": self.plan.desired_outcome,
                "successConditions": list(self.plan.success_conditions),
                "constraints": [],
                "prohibitions": [],
                "members": members,
            },
        )
        self.mission_id = self._field(mission, "mission", "id")
        self._capture("admitted")
        self._finish(
            f"Policy revision {self.policy_revision}, Team {self.team_id}, "
            f"Mission {self.mission_id}"
        )

    def mission_impact_classified(self) -> None:
        self._begin(STEPS[4])
        self._request(
            "classified",
            "classify_mission_impact",
            request={
                "missionId": self.mission_id,
                "impact": self.plan.impact,
                "reason": self.plan.classification_reason,
                "expectedPolicyRevision": self.policy_revision,
                "supersedes": {"kind": "none"},
            },
        )
        self._capture("recorded")
        self._finish(
            f"the Mission's impact is {self.plan.impact} against Policy revision "
            f"{self.policy_revision}"
        )

    def _completion_request(self) -> dict:
        return {
            "missionId": self.mission_id,
            "accountableResidentId": self.author["resident_id"],
            "disposition": self.plan.disposition,
            "conditionResults": [
                {"ordinal": ordinal, "outcome": "met", "support": [], "note": None}
                for ordinal, _ in enumerate(self.plan.success_conditions)
            ],
            "deliveredArtifacts": [],
            "unfinishedWork": [],
            "uncertainty": [],
            "spend": {"notMeasured": self.plan.spend_not_measured},
            "summary": self.plan.summary,
        }

    def mission_completion_recorded(self) -> None:
        self._begin(STEPS[5])
        request = self._completion_request()
        preview = self._request("preview", "preview_mission_completion", request=request)
        token = self._field(preview, "preview", "intentSha256")
        self._request(
            "recorded",
            "record_mission_completion",
            request=request,
            reviewed_intent_sha256=token,
        )
        self._capture("recorded")
        self._finish("the completion was recorded under the token the Operator reviewed")

    def outcome_candidate_prepared(self) -> None:
        self._begin(STEPS[6])
        prepared = self._request(
            "prepared",
            "prepare_outcome_candidate",
            mission_id=self.mission_id,
            appendix_request=self.plan.appendix_request,
        )
        self.prepared = self._field(prepared, "candidate")
        self._capture("prepared")
        self._finish(
            f"one candidate prepared: manifest {self.prepared['manifestId']}, appendix "
            f"{self.prepared['appendixMode']}"
        )

    def candidate_bytes_inspected(self) -> None:
        self._begin(STEPS[7])
        document = self._field(
            self._request(
                "document",
                "read_outcome_candidate_document",
                manifest_digest=self.prepared["manifestDigest"]["value"],
            ),
            "document",
        )
        try:
            digest = compare_candidate_bytes(self.prepared, document)
        except JourneyRefusal as refusal:
            raise self._refuse(refusal.status, refusal.detail) from refusal
        self._capture("document")
        self._finish(
            "the retained bytes are exactly what the store reports: sha256 over the decoded "
            f"content is {digest['recomputed_content_value']}",
            digest=digest,
        )

    def _capture(self, tag: str) -> None:
        """A screenshot with no OCR anchor.

        The four anchors this harness asserts are the four the first-run harness already proved
        on three hosts. The later screens' exact strings are not verified anywhere this harness
        can read, and asserting a guessed string would fail a working journey.
        """
        name = evidence_name(self._step.order, f"{self._step.slug}-{tag}", "png")
        outcome = self.driver.capture(self.evidence_dir / name)
        self._commands.append({"command": "capture", "exit": outcome.exit})
        if (self.evidence_dir / name).exists():
            self._artifacts.append(name)
        if outcome.exit != 0:
            raise self._refuse("broken", f"the screenshot could not be taken: {outcome.detail}")


JOURNEY_STEP_METHODS = tuple(step.name for step in STEPS)


def run_journey(
    driver: JourneyDriver,
    evidence_dir: Path,
    appimage: str,
    plan: JourneyPlan | None = None,
) -> int:
    """Walk the journey. Returns the process exit status: 0 observed, 1 refused."""
    journey = _Journey(driver, evidence_dir, plan or JourneyPlan(), appimage)
    write_summary(journey.summary_path, journey.summary)
    for name in JOURNEY_STEP_METHODS:
        try:
            getattr(journey, name)()
        except JourneyRefusal as refusal:
            record_step(
                journey.summary,
                refusal.step,
                refusal.status,
                exit_code=1,
                detail=refusal.detail,
                artifacts=refusal.artifacts,
                commands=refusal.commands,
            )
            write_summary(journey.summary_path, journey.summary)
            print(
                f"packaged-outcome-journey: {refusal.step} is {refusal.status}: {refusal.detail}",
                file=sys.stderr,
            )
            return 1
    return 0


# ---------------------------------------------------------------------------------------------
# The only part that touches a process, a socket or a screen. It is deliberately the whole
# untested boundary: nothing above it needs an AppImage, and nothing in it decides anything.
# ---------------------------------------------------------------------------------------------


class AppImageDriver:
    """Drive the running packaged application over its own local socket and X display.

    NOT EXERCISED YET: no AppImage existed when this was written, and building one needs the
    compiler slot. Every method here is I/O; the first live run is what proves it.
    """

    def __init__(self, data_dir: Path, display: str, passphrase: str, work_dir: Path) -> None:
        self.data_dir = data_dir
        self.display = display
        self.passphrase = passphrase
        self.work_dir = work_dir

    # -- socket -----------------------------------------------------------------------------

    def _endpoint(self) -> tuple[Path, str]:
        sockets = sorted(self.data_dir.rglob("beacond.sock"))
        if len(sockets) != 1:
            raise JourneyRefusal(
                STEPS[0].name, "broken", f"expected one beacond.sock under {self.data_dir}"
            )
        runtime = sockets[0].parent
        return sockets[0], (runtime / "client.token").read_text(encoding="ascii")

    def request(self, command: str, /, **params: object) -> CommandOutcome:
        import socket as socket_module

        # SNAKE_CASE, verbatim. `LocalCommand` carries `rename_all = "snake_case"` and NO
        # `rename_all_fields` (`local_transport.rs:1223`), and `rename_all` renames the VARIANTS,
        # not a struct variant's own fields -- so `model_name`/`expected_digest` are exactly that on
        # the wire (`local_transport.rs:8521-8523`). The nested DTO bodies this journey sends
        # (`draft`, `request`, `review`) are camelCase because THOSE structs declare it.
        body: dict[str, object] = {"name": command}
        body.update(params)
        try:
            path, token = self._endpoint()
            client = socket_module.socket(socket_module.AF_UNIX, socket_module.SOCK_STREAM)
            client.settimeout(30)
            client.connect(str(path))
            client.sendall(
                json.dumps(
                    {"protocol": 1, "token": token, "command": body}, separators=(",", ":")
                ).encode()
            )
            client.shutdown(socket_module.SHUT_WR)
            chunks = []
            while True:
                chunk = client.recv(65536)
                if not chunk:
                    break
                chunks.append(chunk)
            response = json.loads(b"".join(chunks))
        except (OSError, ValueError) as error:
            return CommandOutcome(command, 1, None, f"{type(error).__name__}: {error}")
        return CommandOutcome(command, 0, response)

    # -- screen -----------------------------------------------------------------------------

    def _window(self) -> str:
        import subprocess

        found = subprocess.run(
            ["xdotool", "search", "--onlyvisible", "--name", "^Beacon$"],
            capture_output=True,
            text=True,
            check=False,
            env={"DISPLAY": self.display, "PATH": "/usr/bin:/bin"},
        )
        windows = found.stdout.split()
        if not windows:
            raise JourneyRefusal(STEPS[0].name, "absent", "no Beacon window is mapped")
        return windows[0]

    def capture(self, target: Path) -> CommandOutcome:
        import subprocess

        try:
            window = self._window()
        except JourneyRefusal as refusal:
            return CommandOutcome("capture", 1, None, refusal.detail)
        result = subprocess.run(
            ["import", "-display", self.display, "-window", window, str(target)],
            capture_output=True,
            check=False,
        )
        return CommandOutcome("capture", result.returncode, None, result.stderr.decode()[:200])

    def observe(
        self, needle: str, target: Path, *, redact_recovery_key: bool = False
    ) -> CommandOutcome:
        """Wait for `needle` in the window's OCR text, then write the screenshot.

        The Recovery Key band is covered by `scripts/clean-host-first-run-band.py`, the same
        two-anchor band the first-run harness uses. On a timeout with redaction requested, only
        a top crop that cannot include the key is written -- the first-run rule, unchanged.
        """
        import subprocess
        import time

        raw = self.work_dir / "current.png"
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            outcome = self.capture(raw)
            if outcome.exit != 0:
                return CommandOutcome(f"observe:{needle}", outcome.exit, None, outcome.detail)
            text = subprocess.run(
                ["tesseract", str(raw), "stdout"], capture_output=True, check=False
            )
            if needle.encode() in text.stdout:
                if redact_recovery_key:
                    return self._redact(raw, target, needle)
                target.write_bytes(raw.read_bytes())
                return CommandOutcome(f"observe:{needle}", 0)
            time.sleep(0.5)
        if redact_recovery_key and raw.is_file():
            subprocess.run(
                ["convert", str(raw), "-crop", "1280x300+0+0", "+repage", str(target)],
                check=False,
            )
        elif raw.is_file():
            target.write_bytes(raw.read_bytes())
        return CommandOutcome(f"observe:{needle}", 1, None, "timed out")

    def _redact(self, raw: Path, target: Path, needle: str) -> CommandOutcome:
        import subprocess

        band_script = Path(__file__).resolve().parent / "clean-host-first-run-band.py"
        tsv = subprocess.run(
            ["tesseract", str(raw), "stdout", "--psm", "6", "tsv"],
            capture_output=True,
            check=False,
        )
        band = subprocess.run(
            ["python3", str(band_script)], input=tsv.stdout, capture_output=True, check=False
        )
        if band.returncode != 0:
            return CommandOutcome(
                f"observe:{needle}", 1, None, "the Recovery Key band could not be identified"
            )
        top, bottom = band.stdout.decode().split()
        drawn = subprocess.run(
            [
                "convert",
                str(raw),
                "-fill",
                "#1f2937",
                "-draw",
                f"rectangle 245,{top} 990,{bottom}",
                str(target),
            ],
            capture_output=True,
            check=False,
        )
        return CommandOutcome(f"observe:{needle}", drawn.returncode)

    def act(self, action: str) -> CommandOutcome:
        import subprocess

        environment = {"DISPLAY": self.display, "PATH": "/usr/bin:/bin"}
        if action == "enter_passphrase":
            script = [
                ["xdotool", "mousemove", "--sync", "490", "451", "click", "1"],
                ["xdotool", "key", "--clearmodifiers", "ctrl+a"],
                ["xdotool", "type", "--clearmodifiers", "--delay", "25", "--", self.passphrase],
                ["xdotool", "mousemove", "--sync", "490", "539", "click", "1"],
                ["xdotool", "key", "--clearmodifiers", "ctrl+a"],
                ["xdotool", "type", "--clearmodifiers", "--delay", "25", "--", self.passphrase],
                ["xdotool", "key", "--clearmodifiers", "Return"],
            ]
        elif action == "confirm_recovery_saved":
            script = [
                ["xdotool", "key", "--clearmodifiers", "Tab"],
                ["xdotool", "key", "--clearmodifiers", "Return"],
            ]
        else:
            raise ValueError(f"UI action outside the closed set: {action!r}")
        for command in script:
            result = subprocess.run(command, capture_output=True, check=False, env=environment)
            if result.returncode != 0:
                return CommandOutcome(
                    f"act:{action}", result.returncode, None, result.stderr.decode()[:200]
                )
        return CommandOutcome(f"act:{action}", 0)


def main(argv: Sequence[str]) -> int:
    import argparse

    parser = argparse.ArgumentParser(prog="packaged-outcome-journey")
    parser.add_argument("--appimage", required=True)
    parser.add_argument("--evidence-dir", required=True, type=Path)
    parser.add_argument("--data-dir", required=True, type=Path)
    parser.add_argument("--display", default=":99")
    parser.add_argument("--passphrase", required=True)
    parser.add_argument("--work-dir", required=True, type=Path)
    arguments = parser.parse_args(list(argv))
    driver = AppImageDriver(
        arguments.data_dir, arguments.display, arguments.passphrase, arguments.work_dir
    )
    return run_journey(driver, arguments.evidence_dir, arguments.appimage)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
