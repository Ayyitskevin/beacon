"""The gate's sidecar staging must fail loudly.

`scripts/gate` enters `set +e` before running its checks, so a helper that
runs several commands reports only the last one. The staging helper builds
`beacond`, makes a directory, and copies the binary; with a stale binary
already present from an earlier build, a FAILED release build still ends in a
successful `cp`, and the gate prints `ok stage sidecar` and exits 0.

That is the exact class of harness defect the script's own header enumerates:
a construct whose failure mode is "quietly does nothing". The code under test
fails loudly; the harness checking it did not.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
FAILING_BUILD = "build --release -p beacond"

FAKE_CARGO = f"""#!/bin/sh
# Fails only the sidecar release build. Every other cargo invocation succeeds,
# so the gate reaches its remaining checks and the assertion is about staging.
if [ "$*" = "{FAILING_BUILD}" ]; then
    echo "injected sidecar build failure" >&2
    exit 42
fi
exit 0
"""


def _fake_cargo_path(tmp_path: Path) -> Path:
    binaries = tmp_path / "bin"
    binaries.mkdir()
    cargo = binaries / "cargo"
    cargo.write_text(FAKE_CARGO)
    cargo.chmod(0o755)
    return binaries


def test_a_failed_sidecar_build_with_a_stale_binary_fails_the_gate(tmp_path: Path) -> None:
    release = REPO / "target" / "release" / "beacond"
    created_stale = not release.exists()
    if created_stale:
        release.parent.mkdir(parents=True, exist_ok=True)
        release.write_bytes(b"stale supervisor from an earlier build\n")

    environment = dict(os.environ)
    environment["PATH"] = f"{_fake_cargo_path(tmp_path)}{os.pathsep}{environment['PATH']}"
    try:
        result = subprocess.run(
            [str(REPO / "scripts" / "gate"), "rust"],
            cwd=REPO,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
    finally:
        if created_stale:
            release.unlink(missing_ok=True)

    assert "injected sidecar build failure" in result.stderr
    # The staging step must own its build's failure, not the copy that followed it.
    assert "FAIL  stage sidecar" in result.stdout, result.stdout
    assert "ok    stage sidecar" not in result.stdout, result.stdout
    assert result.returncode != 0, result.stdout
