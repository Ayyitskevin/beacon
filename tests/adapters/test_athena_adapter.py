"""The Athena adapter composes; it does not compute."""

from __future__ import annotations

from typing import Any

from mosaic.adapters.athena import AthenaAdapter
from mosaic.adapters.transport import Unreachable

VERSION = {
    "version": "0.1.0a1",
    "commit": "eeb8ae361bb60f81984ef72f57a81c996c9307f4",
    "tree_state": "clean",
    "source": "git-checkout",
}

ISSUES: list[dict[str, Any]] = [
    {
        "id": 2,
        "key": "MWS-2",
        "title": "Gremlin hunt: the iambic light meter",
        "status": "open",
        "priority": "medium",
        "project_name": "Mickey workshop",
        "assignee_name": "Claude",
        "assignee_is_agent": True,
        "created_at": "2026-08-22 02:44:10",
        "labels": [],
    }
]


class FakeHTTP:
    def __init__(self, routes: dict[str, object]) -> None:
        self.routes = routes
        self.requested: list[str] = []

    def get_json(self, path: str) -> object:
        self.requested.append(path)
        for prefix, payload in self.routes.items():
            if path.startswith(prefix):
                if isinstance(payload, Exception):
                    raise payload
                return payload
        raise Unreachable("athena", path, "HTTP 404")


def _adapter(routes: dict[str, object]) -> AthenaAdapter:
    adapter = AthenaAdapter()
    adapter._http = FakeHTTP(routes)  # type: ignore[assignment]
    return adapter


def test_capabilities_report_the_service_commit_and_tree_state() -> None:
    """tree_state is provenance, not trivia.

    A module serving from a modified checkout is serving something no commit
    describes. That belongs on the desk beside its answers, not in a log.
    """

    capability = _adapter({"/version": VERSION}).capabilities()
    assert capability.listener is not None
    assert capability.raw["commit"] == VERSION["commit"]
    assert capability.raw["tree_state"] == "clean"


def test_capabilities_never_raise_for_an_unreachable_module() -> None:
    """Discovery failing is information, not an exception.

    `listener=None` is Minerva's own convention for "nothing is serving", and it
    is the honest answer here too.
    """

    adapter = _adapter({"/version": Unreachable("athena", "http://x.invalid/version", "refused")})
    assert adapter.capabilities().listener is None


def test_read_reports_unreachability_instead_of_raising() -> None:
    adapter = _adapter({"/version": Unreachable("athena", "http://x.invalid/version", "refused")})
    entries, unreachable = adapter.read()
    assert entries == ()
    assert len(unreachable) == 1
    # The address must survive into the document: "connection refused" without
    # an address is the least useful error available.
    assert unreachable[0].detail == "http://x.invalid/version"


def test_capabilities_name_the_token_source_never_the_token() -> None:
    capability = _adapter({"/version": VERSION}).capabilities()
    assert set(capability.raw) >= {"address_source", "token_source", "identity_scoped"}
    assert "token" not in {k for k in capability.raw if k != "token_source"}


def test_status_is_not_normalised() -> None:
    """Athena's status word survives into the entry unchanged.

    Mapping module statuses into a cross-module vocabulary is composition, which
    is Lane A's step. An adapter that normalises has already decided, in a place
    with no page to point at and no reviewer looking.
    """

    entries, _ = _adapter({"/version": VERSION, "/issues": ISSUES}).read()
    entry = entries[0]
    # Athena's own word survives verbatim; the Lifecycle value is the lossy part.
    assert entry.payload["module_status"] == "open"
    assert entry.source == "athena"
    assert entry.owner == "Claude"


def test_canonical_link_points_at_the_owning_module() -> None:
    entries, _ = _adapter({"/version": VERSION, "/issues": ISSUES}).read()
    assert entries[0].link.endswith("/aegis/issues/2")
    assert entries[0].provenance.tree_state == "clean"


def test_an_issue_carries_no_authority_at_all() -> None:
    """An Athena issue is work, not an approval.

    `authority=None` is the contract's common case, and it is deliberately not
    the same as an approval that failed to verify. Athena's approvals are a
    separate mechanism on /approvals and are not read here.
    """

    entries, _ = _adapter({"/version": VERSION, "/issues": ISSUES}).read()
    assert entries[0].authority is None


def test_unexpected_payload_shape_is_reported_not_raised() -> None:
    adapter = _adapter({"/version": VERSION, "/issues": {"surprise": True}})
    entries, unreachable = adapter.read()
    assert entries == ()
    assert "neither a list" in unreachable[0].reason
