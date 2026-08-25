"""The Janus adapter's job is to pass binding facts through without editing them."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

import pytest

from mosaic.adapters.janus import READ_VERBS, JanusAdapter

#: Shapes taken from `janus list --state all --json` on mickey, 2026-08-24.
#: The live ledger's 13 gates are distributed kind=git 8, kind=none 4, kind=file 1.
GATES: list[dict[str, Any]] = [
    {
        "id": "g3410c2cff0f",
        "question": "Does Gate 1 mean the allowlist or a per-occurrence approval?",
        "state": "open",
        "kind": "authority",
        "raised_by": "kevin-lee+claude-code",
        "raised_at": "2026-08-24T13:05:52Z",
        "binding_kind": "file",
        "binding_locator": "docs/adr/0054-gate1.md",
        "binding_sha256": "9409066ef76a07748761e17dcaed64a67069496379a99f514f67acde1c7133c4",
        "ruling": None,
        "options": [{"option_id": "allowlist"}, {"option_id": "per-occurrence"}],
        "decay": "estimates stay dishonest while this is open",
        "consumer": "claude-code",
    },
    {
        "id": "g4deae14ca3d",
        "question": "Create the two admin-gated Athena automation rules?",
        "state": "approved",
        "kind": "authority",
        "raised_by": "kevin-lee+claude-code",
        "raised_at": "2026-08-23T10:00:00Z",
        "binding_kind": None,
        "binding_locator": None,
        "binding_sha256": None,
        "ruling": "approved",
        "options": [],
        "decay": None,
        "consumer": None,
    },
]


@dataclass
class FakeCLI:
    executable: str = "janus"
    calls: list[list[str]] | None = None

    def run_json(self, argv: list[str]) -> object:
        (self.calls if self.calls is not None else []).append(argv)
        return GATES


def _adapter() -> JanusAdapter:
    adapter = JanusAdapter()
    adapter._cli = FakeCLI(calls=[])  # type: ignore[assignment]
    return adapter


def test_mutating_verbs_are_not_in_the_allowlist() -> None:
    """The allowlist is the safety story, so it gets asserted, not reviewed.

    `board` is excluded despite reading like a read: it accepts --check, which
    runs decay checks and appends observations to the ledger. An allowlist that
    admits a verb because its usual invocation is harmless is not an allowlist.
    """

    for verb in ("decide", "raise", "withdraw", "expire", "supersede", "check", "board"):
        assert verb not in READ_VERBS, f"{verb!r} can change the ledger"


def test_binding_kind_none_is_not_reported_as_bound() -> None:
    """Four of thirteen live gates carry no binding at all.

    That category is invisible if an adapter reports only a boolean, and it is the
    category a reader most needs to see: a ruling bound to nothing cannot be
    re-checked against anything.
    """

    entries = _adapter().entries()
    unbound = [e for e in entries if e.binding["kind"] == "none"]
    assert len(unbound) == 1
    assert unbound[0].binding["sha256"] == ""


def test_binding_fields_are_passed_through_verbatim() -> None:
    entries = _adapter().entries()
    gate = next(e for e in entries if e.ref == "g3410c2cff0f")
    assert gate.binding == {
        "kind": "file",
        "locator": "docs/adr/0054-gate1.md",
        "sha256": "9409066ef76a07748761e17dcaed64a67069496379a99f514f67acde1c7133c4",
    }


def test_adapter_emits_no_authority_label() -> None:
    """Adapters describe; the authority page decides.

    An adapter that labelled its own entries would be deciding doctrine at the
    edge, in six places, with no page to point at.
    """

    for entry in _adapter().entries():
        assert entry.authority_label == "unlabelled"


def test_status_is_the_module_s_own_word() -> None:
    entries = _adapter().entries()
    assert {e.status for e in entries} == {"open", "approved"}


def test_canonical_link_is_re_runnable() -> None:
    """Janus has no URL, so its canonical link is the command that shows the gate."""

    gate = next(e for e in _adapter().entries() if e.ref == "g3410c2cff0f")
    assert gate.canonical_link == "janus show g3410c2cff0f"


def test_non_list_response_is_reported_not_swallowed() -> None:
    adapter = JanusAdapter()

    class Broken:
        executable = "janus"

        def run_json(self, argv: list[str]) -> object:
            return {"unexpected": "object"}

    adapter._cli = Broken()  # type: ignore[assignment]
    with pytest.raises(Exception) as caught:
        adapter.entries()
    assert "JSON array" in str(caught.value)
