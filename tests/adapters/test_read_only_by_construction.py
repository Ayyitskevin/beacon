"""Read-only has to be a property of the code, not a line in the README.

Mosaic reads six modules it does not own, and its entire licence to do so rests
on being unable to change them. "We were careful" does not survive a refactor by
someone who has not read the doctrine, so these tests fail the build instead.

**What these tests actually prove, stated precisely.** They prove no adapter has
an I/O path *of its own* -- no declared import of a transport, a database driver,
or a process launcher -- so every effect must go through `transport.py`, whose two
entry points are then pinned by AST. They do NOT prove "no path exists": Python
can always reach the filesystem, and `open()` and `pathlib` are reachable without
any import at all. The transport seam is the thing under test, not the language.

Overstating that guarantee is its own risk, because someone who believes the
strong version stops looking. This wording is the correction after claude-2f
reviewed the first version and broke it: the original allowlist was supplied by
the adapter being constrained, which made it circular. See `transport.READ_VERBS`.
"""

from __future__ import annotations

import ast
import pathlib

import pytest

PACKAGE = pathlib.Path(__file__).resolve().parents[2] / "src" / "mosaic"
ADAPTER_DIR = PACKAGE / "adapters"
TRANSPORT = ADAPTER_DIR / "transport.py"

#: Every adapter must reach the outside world through transport.py. If an adapter
#: imports one of these directly it has its own I/O path, and transport's
#: guarantees stop being the whole story.
FORBIDDEN_IN_ADAPTERS = {
    "subprocess",
    "sqlite3",
    "socket",
    "http",
    "http.client",
    "urllib",
    "urllib.request",
    "requests",
    "httpx",
    # `os.system` and `os.popen` are a full shell with no subprocess import.
    "os",
    # Indirection defeats an import scan unless the indirection itself is
    # banned: `importlib.import_module("sqlite3")` opened and wrote to a
    # database while passing every other check in this file.
    "importlib",
    # A filesystem write path that an import scan would otherwise wave through.
    "pathlib",
    "shutil",
    "tempfile",
}

#: ADR 0001: a module's own interface is readable; its live database is not.
#: sqlite3 anywhere in Mosaic means someone opened janus.db or athena.db.
FORBIDDEN_EVERYWHERE = {"sqlite3"}

WRITE_METHODS = {"POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE", "CONNECT"}


def _python_files(root: pathlib.Path) -> list[pathlib.Path]:
    return sorted(p for p in root.rglob("*.py") if "__pycache__" not in p.parts)


def _imported_modules(tree: ast.AST) -> set[str]:
    found: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                found.add(alias.name)
                found.add(alias.name.split(".")[0])
        elif isinstance(node, ast.ImportFrom) and node.module and node.level == 0:
            found.add(node.module)
            found.add(node.module.split(".")[0])
    return found


ADAPTER_FILES = [p for p in _python_files(ADAPTER_DIR) if p != TRANSPORT]


@pytest.mark.parametrize("path", ADAPTER_FILES, ids=lambda p: p.name)
def test_adapters_have_no_io_of_their_own(path: pathlib.Path) -> None:
    imported = _imported_modules(ast.parse(path.read_text()))
    leaked = imported & FORBIDDEN_IN_ADAPTERS
    assert not leaked, (
        f"{path.name} imports {sorted(leaked)} directly. Adapters must reach the "
        f"outside world only through transport.py, whose read-only guarantees are "
        f"what the rest of this test file verifies."
    )


@pytest.mark.parametrize("path", _python_files(PACKAGE), ids=lambda p: p.name)
def test_no_direct_database_access_anywhere(path: pathlib.Path) -> None:
    imported = _imported_modules(ast.parse(path.read_text()))
    leaked = imported & FORBIDDEN_EVERYWHERE
    assert not leaked, (
        f"{path.name} imports {sorted(leaked)}. Opening another module's database "
        f"bypasses its scopes, its authorization, and its activity trail, and "
        f"observes mid-transaction state. ADR 0001 forbids it."
    )


def test_adapters_do_not_reach_for_dynamic_import() -> None:
    """`__import__` is a builtin, so it has no import statement to scan for.

    Banning the `importlib` module is not enough on its own: the builtin reaches
    the same place with no import at all.
    """

    for path in ADAPTER_FILES:
        tree = ast.parse(path.read_text())
        for node in ast.walk(tree):
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Name):
                assert node.func.id != "__import__", (
                    f"{path.name} calls __import__, which reaches any module without "
                    f"an import statement for this scan to find"
                )


def test_the_cli_allowlist_is_owned_by_the_transport_not_its_callers() -> None:
    """The verb allowlist must be a literal here, not an argument from the caller.

    The first version took `read_verbs` as a constructor parameter, so the adapter
    being restricted chose its own restrictions. claude-2f broke it in review by
    writing an adapter that declared `{"decide", "withdraw"}` and reached
    `subprocess.run` while passing every test in this file.

    Reading the table out of the AST is the same technique that already worked for
    the HTTP method: the guarantee is only as good as the thing the test can see.
    """

    from mosaic.adapters.transport import READ_VERBS, ReadOnlyCommand

    mutating = {"decide", "raise", "withdraw", "expire", "supersede", "check", "board"}
    for module, verbs in READ_VERBS.items():
        assert not (verbs & mutating), f"{module} may run {sorted(verbs & mutating)}"

    # A caller must not be able to widen it.
    fields = getattr(ReadOnlyCommand, "__dataclass_fields__", {})
    assert "read_verbs" not in fields, (
        "ReadOnlyCommand accepts read_verbs as a field again, which puts the "
        "allowlist back in the hands of the code it constrains"
    )

    # A module with no entry may run nothing -- fail closed, not open.
    assert ReadOnlyCommand(module="unregistered", executable="true").read_verbs == frozenset()


def test_transport_sends_no_verb_but_get() -> None:
    """The HTTP method is a literal, not a parameter, and it is GET.

    Checked at the AST level rather than by grep so that a method built from a
    variable is caught too -- that is precisely how a write would get added
    without anyone noticing the diff.
    """

    tree = ast.parse(TRANSPORT.read_text())
    methods_seen: list[str] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        for keyword in node.keywords:
            if keyword.arg != "method":
                continue
            assert isinstance(keyword.value, ast.Constant), (
                "transport.py builds an HTTP method from a non-literal. The method "
                "must be a constant so that this test can prove what it is."
            )
            methods_seen.append(keyword.value.value)

    assert methods_seen, "transport.py sets no explicit HTTP method; urllib would default silently"
    assert set(methods_seen) == {"GET"}, f"transport.py can send {sorted(set(methods_seen))}"


def test_no_write_verb_appears_anywhere_in_transport() -> None:
    """Belt and braces: no write verb survives even as a bare string constant."""

    tree = ast.parse(TRANSPORT.read_text())
    for node in ast.walk(tree):
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            assert node.value.upper() not in WRITE_METHODS, (
                f"transport.py contains the string {node.value!r}"
            )


def test_adapter_protocol_exposes_no_write_verb() -> None:
    """The interface itself must offer nothing that changes anything.

    An adapter cannot be asked to write if there is no method to call, which is a
    stronger guarantee than every adapter independently declining to implement one.
    """

    from mosaic.contracts import Adapter

    callables = {name for name in dir(Adapter) if not name.startswith("_")}
    attributes = set(getattr(Adapter, "__annotations__", {}))
    surface = callables | attributes
    assert surface == {"module", "capabilities", "read"}, (
        f"the Adapter protocol grew a member: {sorted(surface)}. Every addition here "
        f"is a new thing Mosaic can ask of a module it does not own."
    )
