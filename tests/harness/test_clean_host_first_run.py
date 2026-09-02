"""The clean-host driver must distinguish observed, broken, and unrun rows.

The real UI run is necessarily a manual packaging check, but its outer matrix
is deterministic bookkeeping. These tests replace Docker only: the actual
script still owns input validation, environment selection, continuation after
a broken host, and its final exit status. That keeps a failed middle row from
being flattened into either a green matrix or three unevaluated rows.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]

FAKE_DOCKER = """#!/bin/sh
case " $* " in
    *" debian:12 "*) [ "${DEBIAN_EXIT:-0}" -eq 0 ] || exit "$DEBIAN_EXIT" ;;
esac

evidence=
for argument in "$@"; do
    case "$argument" in
        *:/evidence) evidence=${argument%:/evidence} ;;
    esac
done
[ -n "$evidence" ] || exit 97

# This is the reviewer's dead-inner-driver mutant: Docker itself succeeds, but
# the mounted driver exits before it materializes any evidence.
[ "${DEAD_INNER_DRIVER:-0}" -eq 0 ] || exit 0

for artifact in \
    01-operator-setup.png 01-session-before.json \
    02-recovery-confirmation-redacted.png \
    03-starter-organization.png 03-session-unlocked.json \
    04-starter-organization.json \
    05-authenticated-briefing.png 05-residents.json
do
    : > "$evidence/$artifact"
done
cat > "$evidence/result.json" <<'JSON'
{
  "steps": {
    "operator_setup_form": {
      "status": "observed",
      "detail": "setup form",
      "artifacts": ["01-operator-setup.png", "01-session-before.json"]
    },
    "recovery_key_confirmation": {
      "status": "observed",
      "detail": "recovery confirmation",
      "artifacts": ["02-recovery-confirmation-redacted.png"]
    },
    "account_and_workspace": {
      "status": "observed",
      "detail": "workspace unlocked",
      "artifacts": ["03-starter-organization.png", "03-session-unlocked.json"]
    },
    "starter_organization": {
      "status": "observed",
      "detail": "organization accepted",
      "artifacts": ["04-starter-organization.json", "05-authenticated-briefing.png"]
    },
    "authenticated_briefing": {
      "status": "observed",
      "detail": "briefing rendered",
      "artifacts": ["05-authenticated-briefing.png", "05-residents.json"]
    }
  }
}
JSON
exit 0
"""


def _isolated_harness(tmp_path: Path) -> tuple[Path, dict[str, str]]:
    root = tmp_path / "checkout"
    scripts = root / "scripts"
    scripts.mkdir(parents=True)
    for name in ("clean-host-first-run", "clean-host-first-run-container"):
        destination = scripts / name
        shutil.copy2(REPO / "scripts" / name, destination)
        destination.chmod(0o755)

    binaries = tmp_path / "bin"
    binaries.mkdir()
    docker = binaries / "docker"
    docker.write_text(FAKE_DOCKER)
    docker.chmod(0o755)

    appimage = tmp_path / "Beacon.AppImage"
    appimage.write_bytes(b"packaged application fixture\n")
    environment = dict(os.environ)
    environment["PATH"] = f"{binaries}{os.pathsep}{environment['PATH']}"
    return root, environment


def _run(
    root: Path,
    environment: dict[str, str],
    appimage: Path,
    output: Path,
    selection: str = "all",
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            str(root / "scripts" / "clean-host-first-run"),
            str(appimage),
            str(output),
            selection,
        ],
        cwd=root,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )


def test_a_broken_middle_host_is_recorded_and_does_not_skip_the_last(tmp_path: Path) -> None:
    root, environment = _isolated_harness(tmp_path)
    environment["DEBIAN_EXIT"] = "42"
    output = tmp_path / "evidence"

    result = _run(root, environment, tmp_path / "Beacon.AppImage", output)

    assert result.returncode != 0
    matrix = json.loads((output / "matrix.json").read_text())
    states = {name: row["status"] for name, row in matrix["environments"].items()}
    assert states == {
        "ubuntu-22.04": "observed",
        "debian-12": "broken",
        "ubuntu-24.04": "observed",
    }
    assert matrix["environments"]["debian-12"]["exit_code"] == "42"
    assert "BROKEN debian-12" in result.stderr


def test_a_single_host_run_leaves_other_hosts_not_evaluated(tmp_path: Path) -> None:
    root, environment = _isolated_harness(tmp_path)
    output = tmp_path / "evidence"

    result = _run(
        root,
        environment,
        tmp_path / "Beacon.AppImage",
        output,
        "debian-12",
    )

    assert result.returncode == 0, result.stderr
    matrix = json.loads((output / "matrix.json").read_text())
    states = {name: row["status"] for name, row in matrix["environments"].items()}
    assert states == {
        "ubuntu-22.04": "not_evaluated",
        "debian-12": "observed",
        "ubuntu-24.04": "not_evaluated",
    }


def test_a_dead_inner_driver_cannot_create_an_observed_host(tmp_path: Path) -> None:
    root, environment = _isolated_harness(tmp_path)
    environment["DEAD_INNER_DRIVER"] = "1"
    output = tmp_path / "evidence"

    result = _run(
        root,
        environment,
        tmp_path / "Beacon.AppImage",
        output,
        "ubuntu-22.04",
    )

    assert result.returncode != 0
    matrix = json.loads((output / "matrix.json").read_text())
    row = matrix["environments"]["ubuntu-22.04"]
    assert row["status"] == "broken"
    assert row["evidence_error"] == "result.json is absent"
    assert not (output / "ubuntu-22.04" / "result.json").exists()
    assert "inner evidence is broken" in result.stderr


def test_an_absent_appimage_is_not_reported_as_a_broken_host(tmp_path: Path) -> None:
    root, environment = _isolated_harness(tmp_path)
    output = tmp_path / "evidence"

    result = _run(root, environment, tmp_path / "absent.AppImage", output)

    assert result.returncode != 0
    matrix = json.loads((output / "matrix.json").read_text())
    assert matrix["input"] == "absent"
    assert {row["status"] for row in matrix["environments"].values()} == {"not_evaluated"}
    assert "AppImage is absent" in result.stderr
