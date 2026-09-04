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
# `desktop-tests` is here for the same reason `stage-sidecar` is: `gate` calls it by a
# repo-relative path, so a checkout without it fails the desktop stage with "No such
# file or directory" -- which is what this fixture caught when it was added.
SCRIPTS = ("gate", "stage-sidecar", "desktop-tests")
FAILING_BUILD = "build --release -p beacond"
STALE_BINARY = b"a supervisor left behind by an earlier build\n"

FAKE_CARGO = f"""#!/bin/sh
# Fails only the sidecar release build. Every other cargo invocation succeeds,
# so the gate reaches its remaining checks and the assertion is about staging.
# Matched as a prefix: the real call carries --message-format after these words.
case "$*" in
    "{FAILING_BUILD}"*)
        echo "injected sidecar build failure" >&2
        exit 42
        ;;
esac
exit 0
"""

# Succeeds, writes the freshly built binary ONLY under the relocated target
# directory, and reports that path the way cargo's JSON output does.
RELOCATED_CARGO = """#!/bin/sh
mkdir -p "$CARGO_TARGET_DIR/release"
printf '%s' "$RELOCATED_BYTES" > "$CARGO_TARGET_DIR/release/beacond"
chmod +x "$CARGO_TARGET_DIR/release/beacond"
head='{"reason":"compiler-artifact","target":{"name":"beacond","kind":["bin"]}'
printf '%s,"executable":"%s"}\\n' "$head" "$CARGO_TARGET_DIR/release/beacond"
exit 0
"""

# Succeeds and reports no executable at all: there is nothing to stage, and a
# fixed path would have quietly supplied one.
SILENT_CARGO = """#!/bin/sh
echo '{"reason":"build-finished","success":true}'
exit 0
"""

# Succeeds and reports TWO matching beacond bins at different paths. Nothing in
# the stream makes either authoritative, so choosing by order decides artifact
# identity by luck.
AMBIGUOUS_CARGO = """#!/bin/sh
mkdir -p "$CARGO_TARGET_DIR/one" "$CARGO_TARGET_DIR/two"
printf 'first' > "$CARGO_TARGET_DIR/one/beacond"
printf 'second' > "$CARGO_TARGET_DIR/two/beacond"
chmod +x "$CARGO_TARGET_DIR/one/beacond" "$CARGO_TARGET_DIR/two/beacond"
head='{"reason":"compiler-artifact","target":{"name":"beacond","kind":["bin"]}'
printf '%s,"executable":"%s"}\\n' "$head" "$CARGO_TARGET_DIR/one/beacond"
printf '%s,"executable":"%s"}\\n' "$head" "$CARGO_TARGET_DIR/two/beacond"
exit 0
"""

# The same path reported twice is one artifact, not two, and must still stage.
REPEATED_CARGO = """#!/bin/sh
mkdir -p "$CARGO_TARGET_DIR/release"
printf '%s' "$RELOCATED_BYTES" > "$CARGO_TARGET_DIR/release/beacond"
chmod +x "$CARGO_TARGET_DIR/release/beacond"
head='{"reason":"compiler-artifact","target":{"name":"beacond","kind":["bin"]}'
printf '%s,"executable":"%s"}\\n' "$head" "$CARGO_TARGET_DIR/release/beacond"
printf '%s,"executable":"%s"}\\n' "$head" "$CARGO_TARGET_DIR/release/beacond"
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

    _write_cargo(tmp_path, FAKE_CARGO)
    return root


def _write_cargo(tmp_path: Path, body: str) -> None:
    """The only cargo on PATH for a run. Each test supplies the behaviour its
    wrong answer needs; nothing else about the scripts is replaced."""
    binaries = tmp_path / "bin"
    binaries.mkdir(exist_ok=True)
    cargo = binaries / "cargo"
    cargo.write_text(body)
    cargo.chmod(0o755)


def _run_staging(
    root: Path, tmp_path: Path, extra_environment: dict[str, str]
) -> subprocess.CompletedProcess[str]:
    environment = dict(os.environ)
    environment["PATH"] = f"{tmp_path / 'bin'}{os.pathsep}{environment['PATH']}"
    environment.update(extra_environment)
    return subprocess.run(
        [str(root / "scripts" / "stage-sidecar")],
        cwd=root,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )


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


def test_a_relocated_build_stages_the_binary_it_produced(tmp_path: Path) -> None:
    """Cargo's output directory moves; the staged bytes must move with it.

    A successful build under CARGO_TARGET_DIR, with older bytes still at the
    default path, used to stage the older bytes and exit 0 -- and the sidecar
    exists so the packaged supervisor is the one just built.
    """
    root = _isolated_checkout(tmp_path)
    _write_cargo(tmp_path, RELOCATED_CARGO)
    fresh = "the supervisor this build produced\n"

    result = _run_staging(
        root,
        tmp_path,
        {
            "CARGO_TARGET_DIR": str(tmp_path / "relocated-target"),
            "RELOCATED_BYTES": fresh,
        },
    )

    assert result.returncode == 0, result.stderr
    staged = next((root / "target" / "sidecar").iterdir())
    assert staged.read_bytes() == fresh.encode(), "staged the stale default-path binary"
    assert staged.read_bytes() != STALE_BINARY
    # The stale binary is untouched, so the difference is the source, not a race.
    assert (root / "target" / "release" / "beacond").read_bytes() == STALE_BINARY


def test_a_build_that_reports_no_executable_refuses_to_stage(tmp_path: Path) -> None:
    root = _isolated_checkout(tmp_path)
    _write_cargo(tmp_path, SILENT_CARGO)

    result = _run_staging(root, tmp_path, {})

    assert result.returncode != 0
    assert "cargo produced no beacond executable" in result.stderr
    # No fallback: nothing was staged from the binary at the default path.
    assert not (root / "target" / "sidecar").exists()


def test_two_matching_executables_refuse_rather_than_pick_one(tmp_path: Path) -> None:
    """Ambiguity is not a default. Selecting the last message would make the
    packaged supervisor depend on Cargo's emission order, which proves
    nothing about which build produced it."""
    root = _isolated_checkout(tmp_path)
    _write_cargo(tmp_path, AMBIGUOUS_CARGO)

    result = _run_staging(root, tmp_path, {"CARGO_TARGET_DIR": str(tmp_path / "ambiguous-target")})

    assert result.returncode != 0
    assert "cargo reported 2 different beacond executables" in result.stderr
    assert "refusing to choose" in result.stderr
    # Both candidates are named, so the operator can see what was ambiguous.
    assert "/one/beacond" in result.stderr
    assert "/two/beacond" in result.stderr
    assert not (root / "target" / "sidecar").exists()


def test_the_same_executable_reported_twice_is_one_artifact(tmp_path: Path) -> None:
    root = _isolated_checkout(tmp_path)
    _write_cargo(tmp_path, REPEATED_CARGO)
    fresh = "the supervisor this build produced\n"

    result = _run_staging(
        root,
        tmp_path,
        {
            "CARGO_TARGET_DIR": str(tmp_path / "repeated-target"),
            "RELOCATED_BYTES": fresh,
        },
    )

    # Refusing here would make the guard above satisfiable by refusing
    # everything, which is not the property being asked for.
    assert result.returncode == 0, result.stderr
    staged = next((root / "target" / "sidecar").iterdir())
    assert staged.read_bytes() == fresh.encode()
