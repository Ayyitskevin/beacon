"""The Janus adapter's job is to pass binding facts through without editing them."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from mosaic.adapters.janus import JanusAdapter
from mosaic.adapters.transport import READ_VERBS
from mosaic.contracts import Attachment, Decider, Enforcement, Lifecycle

#: Shapes taken from `janus list --state all --json` on node-a, 2026-08-24.
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
    {
        "id": "g55daf244a78",
        "question": "A question the world moved past",
        "state": "superseded",
        "kind": "authority",
        "raised_by": "kevin-lee",
        "raised_at": "2026-08-22T09:00:00Z",
        "binding_kind": "git",
        "binding_locator": "docs/adr/0001.md",
        "binding_sha256": "a" * 64,
        # A superseded gate still carries a ruling object. Reading "has a ruling"
        # as "was authorized" is the bug this fixture exists to catch.
        "ruling": {"gate_id": "g55daf244a78", "state": "superseded"},
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
        assert verb not in READ_VERBS["janus"], f"{verb!r} can change the ledger"


def _entries():
    entries, unreachable = _adapter().read()
    assert unreachable == ()
    return entries


def test_read_never_raises_for_an_unreachable_module() -> None:
    """An empty read and a failed read are different facts about the world.

    The contract requires `((), (Unreachable(...),))` rather than an exception,
    so a desk can say "Janus did not answer" instead of "Janus had nothing".
    """

    from mosaic.adapters.transport import Unreachable as TransportUnreachable

    adapter = JanusAdapter()

    class Dead:
        executable = "janus"

        def run_json(self, argv: list[str]) -> object:
            raise TransportUnreachable("janus", "janus list", "not on PATH")

    adapter._cli = Dead()  # type: ignore[assignment]
    entries, unreachable = adapter.read()
    assert entries == ()
    assert len(unreachable) == 1
    assert "not on PATH" in unreachable[0].reason


def test_an_unbound_ruling_never_assures() -> None:
    """4 of 13 live gates are approved and bound to nothing.

    A ruling bound to nothing can never be re-checked against anything, by
    anyone, ever -- so it must not render as a positive assurance no matter what
    its lifecycle says.
    """

    unbound = next(e for e in _entries() if e.ref == "g4deae14ca3d")
    assert unbound.authority is not None
    assert unbound.authority.attachment is Attachment.UNBOUND
    assert unbound.authority.assures() is False


def test_a_gate_with_no_ruling_carries_no_authority() -> None:
    """An open gate is a question, not a weak answer.

    `authority=None` is not the same as an approval that failed to verify, and
    the contract is explicit that a renderer must not show them alike.
    """

    open_gate = next(e for e in _entries() if e.ref == "g3410c2cff0f")
    assert open_gate.authority is None
    # It describes itself in Janus's vocabulary, borrowing no approval word.
    assert open_gate.status == "open"


def test_janus_reports_drift_rather_than_enforcing_it() -> None:
    """Janus states drift and stops there, so the operator has to act.

    Encoding this as REPORTED rather than ENFORCED is the whole reason the
    enforcement axis exists: Icarus enforcing and Janus reporting are different
    obligations for whoever is reading the desk.
    """

    ruled = next(e for e in _entries() if e.authority is not None)
    assert ruled.authority.enforcement is Enforcement.REPORTED
    assert ruled.authority.decider is Decider.HUMAN
    assert ruled.authority.mechanism == "janus.ruling"


def test_a_superseded_ruling_is_never_reported_as_authorized() -> None:
    """The regression: five live gates once claimed an authorization nobody gave.

    A superseded gate carries a `ruling` object whose own state is "superseded".
    An adapter that treats the mere presence of a ruling as authorization reports
    it as `authorized` -- which is the exact collapse this project exists to
    prevent, committed by the tool that exists to prevent it.

    The lifecycle is read from the RULING's own state, never from a constant and
    never from "does a ruling exist".
    """

    superseded = next(e for e in _entries() if e.ref == "g55daf244a78")
    assert superseded.authority is not None
    assert superseded.authority.lifecycle is Lifecycle.REVIEWED
    assert superseded.authority.lifecycle is not Lifecycle.AUTHORIZED
    # And Janus's own word still reaches the reader untouched.
    assert superseded.status == "superseded"


def test_only_an_approved_ruling_reaches_authorized() -> None:
    lifecycles = {e.status: e.authority.lifecycle for e in _entries() if e.authority is not None}
    assert lifecycles["approved"] is Lifecycle.AUTHORIZED
    for status, lifecycle in lifecycles.items():
        if status != "approved":
            assert lifecycle is not Lifecycle.AUTHORIZED, (
                f"a {status!r} gate is claiming authorization"
            )


def test_status_is_janus_s_own_word_not_an_approval_value() -> None:
    """The regression this replaced a mapping table to prevent.

    `status` used to be typed as the roadmap's five-value approval enum, which
    had no terminal-negative value: `withdrawn`, `expired` and `superseded` all
    landed on `proposed`, so a third of the live ledger rendered exactly like a
    live open gate. The word now survives, and nothing maps it.
    """

    statuses = {e.status for e in _entries()}
    assert statuses == {"open", "approved", "superseded"}
    for entry in _entries():
        assert not isinstance(entry.status, Lifecycle), (
            f"{entry.ref} carries an approval value in a field that holds Janus's own word"
        )


def test_binding_kind_none_is_not_reported_as_bound() -> None:
    """Four of thirteen live gates carry no binding at all.

    That category is invisible if an adapter reports only a boolean, and it is the
    category a reader most needs to see: a ruling bound to nothing cannot be
    re-checked against anything.
    """

    unbound = [e for e in _entries() if e.payload["binding_kind"] == "none"]
    assert len(unbound) == 1
    assert unbound[0].payload["binding_sha256"] == ""


def test_binding_fields_are_passed_through_verbatim() -> None:
    gate = next(e for e in _entries() if e.ref == "g3410c2cff0f")
    assert gate.payload["binding_kind"] == "file"
    assert gate.payload["binding_locator"] == "docs/adr/0054-gate1.md"
    assert gate.payload["binding_sha256"].startswith("9409066ef76a")


def test_adapter_uses_only_contract_values() -> None:
    """Adapters describe; the authority page decides.

    No adapter invents a label. Every value here comes from the contract enums,
    so the day Lane A changes the vocabulary, this lane changes with it instead
    of drifting alongside it.
    """

    for entry in _entries():
        if entry.authority is not None:
            assert isinstance(entry.authority.attachment, Attachment)
            assert isinstance(entry.authority.lifecycle, Lifecycle)


def test_status_is_the_module_s_own_word() -> None:
    assert {e.status for e in _entries()} == {"open", "approved", "superseded"}


def test_canonical_link_is_re_runnable() -> None:
    """Janus has no URL, so its canonical link is the command that shows the gate."""

    gate = next(e for e in _entries() if e.ref == "g3410c2cff0f")
    assert gate.link == "janus show g3410c2cff0f"
    assert gate.provenance.source.endswith("list --state all --json")


def test_non_list_response_is_reported_not_swallowed() -> None:
    adapter = JanusAdapter()

    class Broken:
        executable = "janus"

        def run_json(self, argv: list[str]) -> object:
            return {"unexpected": "object"}

    adapter._cli = Broken()  # type: ignore[assignment]
    entries, unreachable = adapter.read()
    assert entries == ()
    assert "JSON array" in unreachable[0].reason
