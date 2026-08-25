"""The Athena adapter composes; it does not compute."""

from __future__ import annotations

from typing import Any

import pytest

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


def test_probe_reports_the_service_commit_and_tree_state() -> None:
    """tree_state is provenance, not trivia.

    A module serving from a modified checkout is serving something no commit
    describes. That belongs on the desk beside its answers, not in a log.
    """

    probe = _adapter({"/version": VERSION}).probe()
    assert probe.reachable is True
    assert probe.commit == VERSION["commit"]
    assert probe.tree_state == "clean"


def test_unreachable_probe_still_names_the_address() -> None:
    adapter = _adapter({"/version": Unreachable("athena", "http://x.invalid/version", "refused")})
    probe = adapter.probe()
    assert probe.reachable is False
    assert probe.address == "http://x.invalid/version"
    assert "refused" in probe.detail


def test_status_is_not_normalised() -> None:
    """Athena's status word survives into the entry unchanged.

    Mapping module statuses into a cross-module vocabulary is composition, which
    is Lane A's step. An adapter that normalises has already decided, in a place
    with no page to point at and no reviewer looking.
    """

    entry = _adapter({"/version": VERSION, "/issues": ISSUES}).entries()[0]
    assert entry.status == "open"
    assert entry.source == "athena"
    assert entry.owner == "Claude"


def test_canonical_link_points_at_the_owning_module() -> None:
    entry = _adapter({"/version": VERSION, "/issues": ISSUES}).entries()[0]
    assert entry.canonical_link.endswith("/aegis/issues/2")


def test_entries_carry_no_authority_label() -> None:
    entry = _adapter({"/version": VERSION, "/issues": ISSUES}).entries()[0]
    assert entry.authority_label == "unlabelled"


def test_unexpected_payload_shape_is_reported() -> None:
    adapter = _adapter({"/version": VERSION, "/issues": {"surprise": True}})
    with pytest.raises(Unreachable) as caught:
        adapter.entries()
    assert "neither a list" in str(caught.value)
