"""The gate's sidecar staging must fail loudly.

`scripts/gate` enters `set +e` before running its checks, so a helper that
runs several commands reports only the last one. Staging builds `beacond`,
makes a directory, and copies the binary; with a stale binary already present
from an earlier build, a FAILED release build still ends in a successful `cp`,
and the gate prints `ok stage sidecar` and exits 0.

That is the exact class of harness defect the gate's own header enumerates: a
construct whose failure mode is "quietly does nothing". The code under test
fails loudly; the harness checking it did not.

Everything this test reads or writes lives under `tmp_path`: the scripts under
test are copied there, the stale binary is constructed there rather than
borrowed from whatever the last build left behind, and anything staging writes
lands there too. The active checkout is neither read for state nor written to.
"""

from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SCRIPTS = ("gate", "stage-sidecar")
FAILING_BUILD = "build --release -p beacond"
STALE_BINARY = b"a supervisor left behind by an earlier build\n"

FAKE_CARGO = f"""#!/bin/sh
# Fails only the sidecar release build. Every other cargo invocation succeeds,
# so the gate reaches its remaining checks and the assertion is about staging.
if [ "$*" = "{FAILING_BUILD}" ]; then
    echo "injected sidecar build failure" >&2
    exit 42
fi
exit 0
"""


def _isolated_checkout(tmp_path: Path) -> Path:
    """A root holding the real scripts, a stale binary, and nothing else.

    The scripts resolve their own directory, so copying them is enough to make
    them operate on this root instead of the repository they came from.
    """
    root = tmp_path / "checkout"
    (root / "scripts").mkdir(parents=True)
    for script in SCRIPTS:
        destination = root / "scripts" / script
        shutil.copy2(REPO / "scripts" / script, destination)
        destination.chmod(0o755)

    stale = root / "target" / "release" / "beacond"
    stale.parent.mkdir(parents=True)
    stale.write_bytes(STALE_BINARY)
    stale.chmod(0o755)

    binaries = tmp_path / "bin"
    binaries.mkdir()
    cargo = binaries / "cargo"
    cargo.write_text(FAKE_CARGO)
    cargo.chmod(0o755)
    return root


def test_a_failed_sidecar_build_with_a_stale_binary_fails_the_gate(tmp_path: Path) -> None:
    root = _isolated_checkout(tmp_path)
    environment = dict(os.environ)
    environment["PATH"] = f"{tmp_path / 'bin'}{os.pathsep}{environment['PATH']}"

    result = subprocess.run(
        [str(root / "scripts" / "gate"), "rust"],
        cwd=root,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )

    assert "injected sidecar build failure" in result.stderr
    # The staging step must own its build's failure, not the copy that followed it.
    assert "FAIL  stage sidecar" in result.stdout, result.stdout
    assert "ok    stage sidecar" not in result.stdout, result.stdout
    assert result.returncode != 0, result.stdout
    # The stale binary is the fixture's own, so a copy that should never have
    # happened is visible here rather than hidden in the developer's checkout.
    assert (root / "target" / "release" / "beacond").read_bytes() == STALE_BINARY
    assert not (root / "target" / "sidecar").exists()
