"""`--no-venv` must skip the Python stages out loud, and never widen the green.

A fresh review worktree pays about ten minutes building a `.venv` before it can
compile Rust or build the desktop app, and neither stage uses the Python tree --
CONTRIBUTING says so and exempts `rust` and `desktop` individually. `--no-venv`
makes that split executable for `all`.

The risk it introduces is the one `scripts/gate`'s own header is entirely about:
a stage that did not run must never be indistinguishable from one that passed.
So these tests assert the absences as hard as the presences -- every skipped
stage is named on stdout, a run that skipped anything does NOT print the
unqualified "all checks passed", and asking for a Python stage BY NAME under
`--no-venv` is refused rather than quietly satisfied with exit 0.

Everything a run reads or writes lives under `tmp_path`: the scripts under test
are copied there, `cargo` and `npm` are stubs on PATH, and the root deliberately
has no `.venv`. The active checkout is neither read for state nor written to.
`rustc` is the real one -- stage-sidecar only asks it for the host triple.
"""

from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SCRIPTS = ("gate", "stage-sidecar")
SIDECAR_BUILD = "build --release -p beacond"
BUILT_BYTES = "the supervisor this stub build produced\n"

# Succeeds, records that it ran, and reports one beacond artifact the way
# cargo's JSON stream does so stage-sidecar has a path to stage.
PASSING_CARGO = f"""#!/bin/sh
echo "cargo $*" >> "$STUB_LOG"
case "$*" in
    "{SIDECAR_BUILD}"*)
        mkdir -p "$STUB_OUT/release"
        printf '%s' "$STUB_BUILT_BYTES" > "$STUB_OUT/release/beacond"
        chmod +x "$STUB_OUT/release/beacond"
        head='{{"reason":"compiler-artifact","target":{{"name":"beacond","kind":["bin"]}}'
        printf '%s,"executable":"%s"}}\\n' "$head" "$STUB_OUT/release/beacond"
        ;;
esac
exit 0
"""

# Identical, except the workspace test run fails the way a real broken test does.
# `--no-venv rust` must carry that outcome out, not absorb it.
FAILING_TEST_CARGO = PASSING_CARGO.replace(
    "exit 0\n",
    """case "$*" in
    "test --workspace"*)
        echo "injected cargo test failure" >&2
        exit 101
        ;;
esac
exit 0
""",
)

PASSING_NPM = """#!/bin/sh
echo "npm $*" >> "$STUB_LOG"
exit 0
"""


def _checkout(tmp_path: Path, cargo: str = PASSING_CARGO) -> Path:
    """A root holding the real scripts, a desktop directory, and NO .venv.

    The scripts resolve their own directory, so copying them is enough to make
    them operate on this root instead of the repository they came from.
    """
    root = tmp_path / "checkout"
    (root / "scripts").mkdir(parents=True)
    for script in SCRIPTS:
        destination = root / "scripts" / script
        shutil.copy2(REPO / "scripts" / script, destination)
        destination.chmod(0o755)
    # run_in_desktop cd's here; the stub npm is what actually answers.
    (root / "apps" / "desktop").mkdir(parents=True)
    assert not (root / ".venv").exists(), "the fixture's whole point is a venv-less root"

    binaries = tmp_path / "bin"
    binaries.mkdir(exist_ok=True)
    for name, body in (("cargo", cargo), ("npm", PASSING_NPM)):
        stub = binaries / name
        stub.write_text(body)
        stub.chmod(0o755)
    return root


def _run(root: Path, tmp_path: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    environment = dict(os.environ)
    environment["PATH"] = f"{tmp_path / 'bin'}{os.pathsep}{environment['PATH']}"
    environment["STUB_LOG"] = str(tmp_path / "stub.log")
    environment["STUB_OUT"] = str(tmp_path / "stub-target")
    environment["STUB_BUILT_BYTES"] = BUILT_BYTES
    # A venv inherited from the caller's shell would defeat the fixture.
    environment.pop("VIRTUAL_ENV", None)
    return subprocess.run(
        [str(root / "scripts" / "gate"), *arguments],
        cwd=root,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )


def _stub_calls(tmp_path: Path) -> str:
    log = tmp_path / "stub.log"
    return log.read_text() if log.exists() else ""


def test_no_venv_all_runs_the_exempt_stages_and_names_every_skipped_one(tmp_path: Path) -> None:
    root = _checkout(tmp_path)

    result = _run(root, tmp_path, "--no-venv", "all")

    assert result.returncode == 0, result.stdout + result.stderr
    # Every skipped stage is named, with a reason, on stdout.
    for stage in ("lint", "format", "test"):
        assert f"skip  {stage}" in result.stdout, result.stdout
    assert result.stdout.count("needs .venv") == 3, result.stdout
    # The exempt stages really ran, rather than the whole gate being skipped.
    assert "ok    cargo test" in result.stdout, result.stdout
    assert "ok    desktop tests" in result.stdout, result.stdout
    assert "cargo test --workspace" in _stub_calls(tmp_path)
    assert "npm test" in _stub_calls(tmp_path)
    # A partial pass must not read like a full one.
    assert "SKIPPED lint, format, test" in result.stdout, result.stdout
    assert "this is not a full gate" in result.stdout, result.stdout
    assert "gate: all checks passed." not in result.stdout, result.stdout


def test_all_without_the_flag_still_refuses_when_there_is_no_venv(tmp_path: Path) -> None:
    """The existing refusal is the false-green guard; --no-venv must not relax it."""
    root = _checkout(tmp_path)

    result = _run(root, tmp_path, "all")

    assert result.returncode == 2, result.stdout
    assert "no .venv" in result.stderr, result.stderr
    assert "false-green" in result.stderr, result.stderr
    # It refuses BEFORE running anything: no check reported, no stub invoked.
    assert "ok    " not in result.stdout, result.stdout
    assert "skip  " not in result.stdout, result.stdout
    assert _stub_calls(tmp_path) == ""


def test_no_venv_rust_runs_cargo_and_passes_when_cargo_passes(tmp_path: Path) -> None:
    root = _checkout(tmp_path)

    result = _run(root, tmp_path, "--no-venv", "rust")

    assert result.returncode == 0, result.stdout + result.stderr
    assert "cargo build --workspace" in _stub_calls(tmp_path)
    # Nothing was skipped, so this run is a full 'rust' stage and says so plainly.
    assert "gate: all checks passed." in result.stdout, result.stdout
    assert "skip  " not in result.stdout, result.stdout


def test_no_venv_rust_carries_out_a_cargo_failure(tmp_path: Path) -> None:
    """Without this, the test above is satisfiable by a gate that never fails."""
    root = _checkout(tmp_path, cargo=FAILING_TEST_CARGO)

    result = _run(root, tmp_path, "--no-venv", "rust")

    assert result.returncode != 0, result.stdout
    assert "injected cargo test failure" in result.stderr
    assert "FAIL  cargo test" in result.stdout, result.stdout
    assert "ok    cargo test" not in result.stdout, result.stdout
    assert "gate: all checks passed." not in result.stdout, result.stdout


def test_a_python_stage_named_with_no_venv_is_refused_not_skipped(tmp_path: Path) -> None:
    """Exiting 0 on the stage the caller explicitly asked for is the false green
    in its purest form, so naming one under --no-venv is a usage error."""
    root = _checkout(tmp_path)

    result = _run(root, tmp_path, "--no-venv", "lint")

    assert result.returncode == 2, result.stdout
    assert "stage 'lint'" in result.stderr, result.stderr
    assert "--no-venv cannot run it" in result.stderr, result.stderr
    assert "skip  " not in result.stdout, result.stdout
    assert "gate: all checks passed." not in result.stdout, result.stdout


def test_help_documents_the_flag_and_which_stages_need_the_venv(tmp_path: Path) -> None:
    root = _checkout(tmp_path)

    result = _run(root, tmp_path, "--help")

    assert result.returncode == 0, result.stderr
    assert "--no-venv" in result.stdout, result.stdout
    assert "needs .venv" in result.stdout, result.stdout
    assert _stub_calls(tmp_path) == "", "--help must not run a gate"
