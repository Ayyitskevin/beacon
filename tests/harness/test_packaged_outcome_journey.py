"""The packaged Outcome journey decides things; those decisions are tested without an AppImage.

The real run needs a packaged binary, a display and OCR. Everything that DECIDES -- the step
sequence, the evidence names, the digest comparison and the summary -- is separated from that
boundary on purpose, so it can be exercised here with a fake driver. What remains untested is
exactly `AppImageDriver`, and it decides nothing.

The digest tests are the point of the eighth step. A journey that reports the bytes are right
because two hex strings are the same LENGTH would pass every well-formed pair in the system.
"""

from __future__ import annotations

import base64
import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]

_SPEC = importlib.util.spec_from_file_location(
    "packaged_outcome_journey", REPO / "scripts" / "packaged_outcome_journey.py"
)
journey = importlib.util.module_from_spec(_SPEC)
sys.modules["packaged_outcome_journey"] = journey
assert _SPEC.loader is not None
_SPEC.loader.exec_module(journey)

AUTHOR = "11111111-1111-4111-8111-111111111111"
REVIEWER = "22222222-2222-4222-8222-222222222222"
PROJECT = "33333333-3333-4333-8333-333333333333"
TEAM = "44444444-4444-4444-8444-444444444444"
MISSION = "55555555-5555-4555-8555-555555555555"
MANIFEST = "66666666-6666-4666-8666-666666666666"

CONTENT = b"# Outcome candidate\n\nThe disposable journey Mission concluded.\n"
CONTENT_VALUE = hashlib.sha256(CONTENT).hexdigest()
MANIFEST_VALUE = hashlib.sha256(b"manifest").hexdigest()
ARTIFACT_VALUE = hashlib.sha256(b"artifact").hexdigest()


def _envelope(value: str, canonicalizer: str = "beacon.raw.v1") -> dict:
    return {"algorithm": "sha256", "canonicalizer": canonicalizer, "value": value}


def _prepared() -> dict:
    return {
        "missionId": MISSION,
        "appendixRequest": "policy_default",
        "appendixMode": "policy_state_not_high_impact",
        "manifestId": MANIFEST,
        "manifestDigest": _envelope(MANIFEST_VALUE, "beacon.manifest.v1"),
        "candidateArtifactDigest": _envelope(ARTIFACT_VALUE, "beacon.candidate.v1"),
        "candidateContentDigest": _envelope(CONTENT_VALUE),
        "candidateContentBytes": len(CONTENT),
        "mediaType": "text/markdown; charset=utf-8",
    }


def _document(**changes: object) -> dict:
    prepared = _prepared()
    document = {
        "missionId": MISSION,
        "manifestId": MANIFEST,
        "manifestDigest": prepared["manifestDigest"],
        "candidateArtifactDigest": prepared["candidateArtifactDigest"],
        "candidateContentDigest": prepared["candidateContentDigest"],
        "candidateContentBytes": len(CONTENT),
        "mediaType": prepared["mediaType"],
        "candidateContentBase64": base64.b64encode(CONTENT).decode("ascii"),
    }
    document.update(changes)
    return document


RESULTS: dict[str, dict] = {
    "status": {"status": {"phase": "unlocked", "workspaceAccess": True}},
    "starter_organization": {"organization": {"proposalVersion": 1, "decision": "pending"}},
    "accept_starter_organization": {"organization": {"proposalVersion": 1, "decision": "accepted"}},
    "residents": {
        "residents": [
            {"id": AUTHOR, "role": "builder", "lifecycle": "inactive", "latestDecisionId": None},
            {"id": REVIEWER, "role": "reviewer", "lifecycle": "inactive", "latestDecisionId": None},
        ]
    },
    "activate_resident": {"resident": {"lifecycle": "active", "latestDecisionId": "decision-1"}},
    "configure_resident": {"configuration": {"id": "configuration-1"}},
    "sample_project_status": {"status": {"installed": False}},
    "install_sample_project": {"result": {"outcome": "installed"}},
    "projects": {"projects": [{"id": PROJECT, "name": "Sample", "charterRevision": 1}]},
    "project": {"project": {"id": PROJECT, "charter": {"revision": 1}}},
    "publish_project_policy": {"policy": {"revision": 1, "charterRevision": 1}},
    "form_team": {"team": {"id": TEAM}},
    "admit_mission": {"mission": {"id": MISSION}},
    "classify_mission_impact": {"classification": {"latest": {"impact": "ordinary"}}},
    "preview_mission_completion": {"preview": {"intentSha256": "a" * 64}},
    "record_mission_completion": {"intent": {"intentSha256": "a" * 64}},
    "prepare_outcome_candidate": {"candidate": _prepared()},
    "read_outcome_candidate_document": {"document": _document()},
}


class FakeDriver:
    """Answers every journey command from a table. Records what it was asked."""

    def __init__(self, overrides: dict | None = None, exits: dict | None = None) -> None:
        self.overrides = overrides or {}
        self.exits = exits or {}
        self.asked: list[str] = []
        self.params: list[dict] = []
        self.actions: list[str] = []

    def request(self, command: str, /, **params: object) -> journey.CommandOutcome:
        self.asked.append(command)
        self.params.append(dict(params))
        if command in self.exits:
            return journey.CommandOutcome(command, self.exits[command], None, "fake refusal")
        result = json.loads(json.dumps(RESULTS[command]))
        result["kind"] = journey.COMMAND_RESULT_KIND[command]
        if command in self.overrides:
            result = self.overrides[command](result)
        return journey.CommandOutcome(command, 0, {"protocol": 1, "result": result, "error": None})

    def observe(
        self, needle: str, target: Path, *, redact_recovery_key: bool = False
    ) -> journey.CommandOutcome:
        target.write_bytes(b"screenshot")
        return journey.CommandOutcome(f"observe:{needle}", 0)

    def capture(self, target: Path) -> journey.CommandOutcome:
        target.write_bytes(b"screenshot")
        return journey.CommandOutcome("capture", 0)

    def act(self, action: str) -> journey.CommandOutcome:
        self.actions.append(action)
        return journey.CommandOutcome(f"act:{action}", 0)


def _run(tmp_path: Path, driver: FakeDriver) -> tuple[int, dict]:
    evidence = tmp_path / "evidence"
    evidence.mkdir()
    status = journey.run_journey(driver, evidence, "/nowhere/Beacon.AppImage")
    return status, json.loads((evidence / "summary.json").read_text())


# -- the summary ------------------------------------------------------------------------------


def test_every_f5_step_exists_as_a_row_before_the_first_step_runs() -> None:
    summary = journey.new_summary("/nowhere/Beacon.AppImage")
    journey.validate_summary(summary)

    rows = summary["steps"]
    assert len(rows) == 13
    assert sorted(row["order"] for row in rows.values()) == list(range(1, 14))
    ran = {name: row["status"] for name, row in rows.items() if row["order"] <= 8}
    assert set(ran.values()) == {"not_evaluated"}


def test_the_not_yet_rows_name_the_slice_that_lands_them() -> None:
    rows = journey.new_summary("x")["steps"]
    pending = {name: row for name, row in rows.items() if row["status"] == "not_yet"}

    assert sorted(pending) == [
        "evaluation_recorded",
        "evaluation_requested",
        "outcome_published",
        "outcome_saved_to_file",
        "restart_and_compare",
    ]
    for name, row in pending.items():
        assert row["lands_in"], f"{name} does not say which slice lands it"
        assert row["exit"] is None


def test_a_row_that_grew_a_key_is_a_broken_summary() -> None:
    summary = journey.new_summary("x")
    summary["steps"]["outcome_published"]["observed_anyway"] = True

    try:
        journey.validate_summary(summary)
    except ValueError as error:
        assert "not closed" in str(error)
    else:  # pragma: no cover - the assertion above is the test
        raise AssertionError("a row outside the closed schema was accepted")


def test_evidence_names_carry_the_f5_order() -> None:
    assert journey.evidence_name(8, "candidate-document", "json") == "08-candidate-document.json"
    for bad in [(0, "a", "png"), (100, "a", "png"), (1, "Not-Kebab", "png"), (1, "a", ".png")]:
        try:
            journey.evidence_name(*bad)
        except ValueError:
            continue
        raise AssertionError(f"an invalid evidence name was accepted: {bad}")


# -- the whole journey ------------------------------------------------------------------------


def test_a_complete_journey_observes_all_eight_steps_and_records_the_digest(tmp_path: Path) -> None:
    driver = FakeDriver()

    status, summary = _run(tmp_path, driver)

    assert status == 0
    rows = summary["steps"]
    observed = {name: row["status"] for name, row in rows.items() if row["order"] <= 8}
    assert set(observed.values()) == {"observed"}, observed
    assert {name: row["status"] for name, row in rows.items() if row["order"] > 8} == {
        "evaluation_requested": "not_yet",
        "evaluation_recorded": "not_yet",
        "outcome_published": "not_yet",
        "outcome_saved_to_file": "not_yet",
        "restart_and_compare": "not_yet",
    }
    digest = rows["candidate_bytes_inspected"]["digest"]
    assert digest["recomputed_content_value"] == CONTENT_VALUE
    assert digest["candidate_content_bytes"] == len(CONTENT)
    assert driver.actions == ["enter_passphrase", "confirm_recovery_saved"]
    # Every command the journey sent is named in its own step's row, with its own exit.
    ordered = sorted(rows.values(), key=lambda step: step["order"])
    named = [row["command"] for step in ordered for row in step["commands"]]
    assert driver.asked == [name for name in named if name in journey.COMMAND_RESULT_KIND]


def test_the_journey_walks_the_f5_order(tmp_path: Path) -> None:
    driver = FakeDriver()

    _run(tmp_path, driver)

    assert driver.asked == [
        "status",
        "starter_organization",
        "accept_starter_organization",
        "residents",
        "activate_resident",
        "configure_resident",
        "activate_resident",
        "configure_resident",
        "sample_project_status",
        "install_sample_project",
        "projects",
        "project",
        "publish_project_policy",
        "form_team",
        "admit_mission",
        "classify_mission_impact",
        "preview_mission_completion",
        "record_mission_completion",
        "prepare_outcome_candidate",
        "read_outcome_candidate_document",
    ]


def test_a_refused_step_stops_the_journey_and_leaves_later_steps_not_evaluated(
    tmp_path: Path,
) -> None:
    driver = FakeDriver(exits={"admit_mission": 1})

    status, summary = _run(tmp_path, driver)

    assert status == 1
    rows = summary["steps"]
    assert rows["policy_team_and_mission"]["status"] == "broken"
    assert rows["policy_team_and_mission"]["exit"] == 1
    assert "admit_mission exited 1" in rows["policy_team_and_mission"]["detail"]
    assert rows["mission_impact_classified"]["status"] == "not_evaluated"
    assert rows["candidate_bytes_inspected"]["status"] == "not_evaluated"
    # The commands that DID run are still named, with their own exits.
    assert [row["command"] for row in rows["policy_team_and_mission"]["commands"]] == [
        "publish_project_policy",
        "form_team",
        "admit_mission",
    ]


def test_a_result_of_the_wrong_kind_is_a_broken_step(tmp_path: Path) -> None:
    def wrong_kind(result: dict) -> dict:
        result["kind"] = "session_status"
        return result

    status, summary = _run(tmp_path, FakeDriver(overrides={"form_team": wrong_kind}))

    assert status == 1
    row = summary["steps"]["policy_team_and_mission"]
    assert row["status"] == "broken"
    assert "answered 'session_status', not 'team'" in row["detail"]


def test_a_credential_bearing_response_is_never_written_to_evidence() -> None:
    assert "begin_password_setup" in journey.REDACTED_COMMANDS
    assert "recover_with_key" in journey.REDACTED_COMMANDS
    # And no credential-bearing command is in the journey's own vocabulary at all.
    assert journey.REDACTED_COMMANDS.isdisjoint(journey.COMMAND_RESULT_KIND)


# -- the digest comparison --------------------------------------------------------------------


def test_a_digest_that_differs_only_in_bytes_is_refused() -> None:
    left = "0" * 64
    right = "0" * 63 + "1"

    assert len(left) == len(right)
    assert journey.digest_matches(left, left) is True
    assert journey.digest_matches(left, right) is False


def test_a_store_reporting_a_different_content_digest_is_refused(tmp_path: Path) -> None:
    other = hashlib.sha256(b"different bytes entirely").hexdigest()

    def relabel(result: dict) -> dict:
        result["document"]["candidateContentDigest"] = _envelope(other)
        return result

    def relabel_prepared(result: dict) -> dict:
        result["candidate"]["candidateContentDigest"] = _envelope(other)
        return result

    status, summary = _run(
        tmp_path,
        FakeDriver(
            overrides={
                "read_outcome_candidate_document": relabel,
                "prepare_outcome_candidate": relabel_prepared,
            }
        ),
    )

    assert status == 1
    row = summary["steps"]["candidate_bytes_inspected"]
    assert row["status"] == "broken"
    assert "sha256 over the retained bytes is not the content digest" in row["detail"]
    assert row["digest"] is None


def test_a_receipt_and_a_document_that_disagree_are_refused(tmp_path: Path) -> None:
    def relabel(result: dict) -> dict:
        result["document"]["candidateArtifactDigest"] = _envelope(
            hashlib.sha256(b"another artifact").hexdigest(), "beacon.candidate.v1"
        )
        return result

    status, summary = _run(
        tmp_path, FakeDriver(overrides={"read_outcome_candidate_document": relabel})
    )

    assert status == 1
    assert (
        "candidateArtifactDigest disagrees"
        in (summary["steps"]["candidate_bytes_inspected"]["detail"])
    )


def test_a_content_length_that_disagrees_with_the_store_is_refused(tmp_path: Path) -> None:
    def shorten(result: dict) -> dict:
        result["document"]["candidateContentBytes"] = len(CONTENT) - 1
        return result

    status, summary = _run(
        tmp_path, FakeDriver(overrides={"read_outcome_candidate_document": shorten})
    )

    assert status == 1
    detail = summary["steps"]["candidate_bytes_inspected"]["detail"]
    assert f"the store reports {len(CONTENT) - 1} content bytes" in detail


def test_a_non_canonical_base64_tail_is_refused() -> None:
    payload = b"beaco"  # five bytes, so the final quantum is padded with ONE "="
    canonical = base64.b64encode(payload).decode("ascii")
    assert canonical.endswith("=") and not canonical.endswith("==")
    # Same bytes, unused tail bits set: b64decode(validate=True) accepts it.
    alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
    index = alphabet.index(canonical[-2])
    # The last data character carries two unused bits; only those move.
    noncanonical = canonical[:-2] + alphabet[(index & ~3) + ((index & 3) + 1) % 4] + canonical[-1]
    assert noncanonical != canonical
    assert base64.b64decode(noncanonical, validate=True) == payload

    document = _document(candidateContentBase64=noncanonical)
    try:
        journey.decode_candidate_content(document)
    except journey.JourneyRefusal as refusal:
        assert "canonical encoding" in refusal.detail
    else:  # pragma: no cover - the assertion above is the test
        raise AssertionError("a non-canonical base64 encoding was accepted")


def test_a_digest_that_is_not_lowercase_hex_is_refused() -> None:
    for bad in [
        CONTENT_VALUE.upper(),
        CONTENT_VALUE[:-1],
        CONTENT_VALUE + "0",
        "sha256:" + CONTENT_VALUE,
    ]:
        try:
            journey.digest_matches(CONTENT_VALUE, bad)
        except journey.JourneyRefusal:
            continue
        raise AssertionError(f"a malformed digest was compared rather than refused: {bad!r}")


# -- the two Residents ------------------------------------------------------------------------


def test_the_same_resident_cannot_be_both_author_and_reviewer() -> None:
    same = [{"id": AUTHOR, "role": "builder"}, {"id": AUTHOR, "role": "reviewer"}]

    try:
        journey.select_author_and_reviewer(same)
    except journey.JourneyRefusal as refusal:
        assert refusal.status == "broken"
        assert "the same Resident" in refusal.detail
    else:  # pragma: no cover - the assertion above is the test
        raise AssertionError("one Resident was accepted as both author and reviewer")


def test_a_roster_with_no_reviewer_is_absent_not_broken() -> None:
    try:
        journey.select_author_and_reviewer([{"id": AUTHOR, "role": "builder"}])
    except journey.JourneyRefusal as refusal:
        assert refusal.status == "absent"
    else:  # pragma: no cover - the assertion above is the test
        raise AssertionError("a roster with no reviewer was accepted")


# -- the entry point's argument refusals ------------------------------------------------------


def _isolated(tmp_path: Path) -> Path:
    root = tmp_path / "checkout"
    (root / "scripts").mkdir(parents=True)
    for name in (
        "packaged-outcome-journey",
        "packaged_outcome_journey.py",
        "clean-host-first-run-state",
        "clean-host-first-run-band.py",
    ):
        destination = root / "scripts" / name
        shutil.copy2(REPO / "scripts" / name, destination)
        destination.chmod(0o755)
    return root


def _entry(root: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(root / "scripts" / "packaged-outcome-journey"), *arguments],
        cwd=root,
        env=dict(os.environ),
        capture_output=True,
        text=True,
        check=False,
    )


def test_an_absent_appimage_refuses_with_exit_2(tmp_path: Path) -> None:
    root = _isolated(tmp_path)

    result = _entry(root, str(tmp_path / "absent.AppImage"), str(tmp_path / "evidence"))

    assert result.returncode == 2
    assert "AppImage is absent" in result.stderr
    assert not (tmp_path / "evidence").exists()


def test_a_non_executable_appimage_refuses_with_exit_2(tmp_path: Path) -> None:
    root = _isolated(tmp_path)
    appimage = tmp_path / "Beacon.AppImage"
    appimage.write_bytes(b"not executable\n")
    appimage.chmod(0o644)

    result = _entry(root, str(appimage), str(tmp_path / "evidence"))

    assert result.returncode == 2
    assert "is not executable" in result.stderr
    assert not (tmp_path / "evidence").exists()


def test_an_existing_evidence_directory_refuses_with_exit_2(tmp_path: Path) -> None:
    root = _isolated(tmp_path)
    appimage = tmp_path / "Beacon.AppImage"
    appimage.write_bytes(b"#!/bin/sh\nexit 0\n")
    appimage.chmod(0o755)
    evidence = tmp_path / "evidence"
    evidence.mkdir()

    result = _entry(root, str(appimage), str(evidence))

    assert result.returncode == 2
    assert "evidence directory already exists" in result.stderr


def test_wrong_argument_count_refuses_with_exit_2(tmp_path: Path) -> None:
    root = _isolated(tmp_path)

    result = _entry(root, str(tmp_path / "one"))

    assert result.returncode == 2
    assert "usage: scripts/packaged-outcome-journey" in result.stderr
