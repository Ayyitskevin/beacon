"""Janus -- the gates a human has not yet ruled on.

Read through the `janus` CLI, not by opening `~/.janus/janus.db`. Going through a
module's own interface is not filesystem reach-through; opening its database is
(ADR 0001). The verb allowlist lives in `transport.READ_VERBS`, not here -- an
adapter does not get to choose its own restrictions.

Janus is where the authority axes are most visible, so this adapter is the one
that populates `Authority` most fully. It reports what the ledger says and
nothing more: it does not re-check whether a binding still holds, because that is
a `janus show` per gate and should be a deliberate act, not a side effect of
reading a desk. Every bound gate therefore reports `BYTES_UNCHECKED` -- a fact
about this run, honestly labelled, rather than an assurance nobody earned.
"""

from __future__ import annotations

from typing import Any

from .base import (
    Attachment,
    Authority,
    Capability,
    Decider,
    Enforcement,
    Entry,
    Lifecycle,
    Unreachable,
    provenance,
    unreachable_from,
)
from .transport import ReadOnlyCommand
from .transport import Unreachable as TransportUnreachable

#: Janus's own state words (`open`, `approved`, `refused`, `withdrawn`, `expired`,
#: `superseded`) are passed through as `Entry.status` verbatim. They used to be
#: mapped onto the roadmap's five-value Lifecycle, which had no terminal-negative
#: value -- so `withdrawn`, `expired` and `superseded` all landed on PROPOSED and a
#: third of the live ledger rendered like a live open gate. Lane A retyped the
#: field to `str`; the approval vocabulary now lives only on `Authority.lifecycle`,
#: where it is populated only when there actually is an approval.
#:
#: Janus's own state words that mean the decision is closed and no longer in force.
#:
#: This is the fact `Lifecycle` cannot carry. All four land on REVIEWED — correctly,
#: because the mandated vocabulary has no terminal-negative value and an adapter must
#: not default upward — which leaves them indistinguishable from a decision somebody
#: is actively reviewing. The adapter is the only place that can still see the
#: difference, because it is the only place holding Janus's own word.
_TERMINAL_STATES: frozenset[str] = frozenset({"refused", "withdrawn", "expired", "superseded"})


#: A ruling's own state -> where it sits in the APPROVAL vocabulary.
#:
#: This is a mapping table, and it belongs here in a way the deleted
#: `Entry.status` one did not: `Authority.lifecycle` *is* the approval axis, so
#: translating Janus's ruling states into it is the honest job, not a forced fit.
#:
#: Only `approved` yields AUTHORIZED. A `superseded` ruling records that the world
#: moved past the question, and 5 of 13 live gates are in that state -- rendering
#: them as authorized would claim a human authorized something they did not, which
#: is the exact collapse this project exists to prevent. Anything unrecognised
#: falls to REVIEWED: a human engaged and the result was not an authorization.
_RULING_LIFECYCLE: dict[str, Lifecycle] = {
    "approved": Lifecycle.AUTHORIZED,
    "refused": Lifecycle.REVIEWED,
    "withdrawn": Lifecycle.REVIEWED,
    "expired": Lifecycle.REVIEWED,
    "superseded": Lifecycle.REVIEWED,
}


#: Janus binding kinds -> what the ruling is pinned to.
#:
#: `none` is the case the live ledger actually exercises and the abstract type
#: system would have missed: 4 of 13 gates are bound to nothing, and all four are
#: approved. A ruling bound to nothing can never be re-checked against anything,
#: by anyone, ever -- which is why it is UNBOUND rather than merely unchecked.
_ATTACHMENT: dict[str, Attachment] = {
    "git": Attachment.BYTES_UNCHECKED,
    "file": Attachment.BYTES_UNCHECKED,
    "text": Attachment.BYTES_UNVERIFIABLE,
    "none": Attachment.UNBOUND,
}


class JanusAdapter:
    module = "janus"

    def __init__(self, executable: str = "janus") -> None:
        self._cli = ReadOnlyCommand(module=self.module, executable=executable)

    # -- discovery ---------------------------------------------------------

    def capabilities(self) -> Capability:
        try:
            gates = self._cli.run_json(["list", "--state", "all", "--json"])
        except TransportUnreachable:
            # Discovery failing is information, not an exception. `listener=None`
            # is Minerva's convention for "nothing is serving" and it is the
            # honest answer for a CLI that is not on PATH.
            return Capability(module=self.module, listener=None)
        count = len(gates) if isinstance(gates, list) else 0
        return Capability(
            module=self.module,
            surfaces=("list", "show", "stats", "doctor"),
            listener=f"{self._cli.executable} (local CLI)",
            raw={"gate_count": count},
        )

    # -- reading -----------------------------------------------------------

    def read(self) -> tuple[tuple[Entry, ...], tuple[Unreachable, ...]]:
        source = f"{self._cli.executable} list --state all --json"
        try:
            gates = self._cli.run_json(["list", "--state", "all", "--json"])
        except TransportUnreachable as exc:
            return (), (unreachable_from(exc, self.module),)

        if not isinstance(gates, list):
            return (), (
                Unreachable(
                    module=self.module,
                    reason="`janus list --json` did not answer with a JSON array",
                    detail=source,
                ),
            )

        return tuple(self._entry(gate, source) for gate in gates), ()

    def _entry(self, gate: dict[str, Any], source: str) -> Entry:
        gate_id = str(gate.get("id", ""))
        state = str(gate.get("state", ""))
        kind = gate.get("binding_kind") or "none"

        return Entry(
            source=self.module,
            owner=str(gate.get("raised_by") or "unknown"),
            title=str(gate.get("question", "")),
            status=state,
            timestamp=str(gate.get("raised_at") or ""),
            schema_version="janus.gate.v1",
            ref=gate_id,
            provenance=provenance(source),
            link=f"janus show {gate_id}",
            authority=self._authority(gate, state, kind),
            payload={
                "binding_kind": kind,
                "binding_locator": gate.get("binding_locator") or "",
                "binding_sha256": gate.get("binding_sha256") or "",
                "gate_kind": gate.get("kind"),
                "decay": gate.get("decay"),
                "consumer": gate.get("consumer"),
                "ruling": gate.get("ruling"),
                "option_count": len(gate.get("options") or []),
            },
        )

    def _authority(self, gate: dict[str, Any], state: str, kind: str) -> Authority | None:
        ruling = gate.get("ruling")
        if not ruling:
            # No ruling means no approval at all -- which the contract says must
            # not render like an approval that failed to verify. An open gate is
            # a question, not a weak answer.
            return None

        return Authority(
            decider=Decider.HUMAN,
            attachment=_ATTACHMENT.get(kind, Attachment.UNBOUND),
            # Janus states drift and stops there, on purpose: enforcing would put
            # it in the permission path. So the operator, not the module, acts.
            enforcement=Enforcement.REPORTED,
            # Taken from the RULING's own state, not the gate's and never a
            # constant. A superseded gate still carries a ruling object; reading
            # "it has a ruling" as "it was authorized" is how five gates claimed
            # an authorization nobody gave.
            lifecycle=_RULING_LIFECYCLE.get(
                str(ruling.get("state") or state) if isinstance(ruling, dict) else state,
                Lifecycle.REVIEWED,
            ),
            # Supplied here rather than derived downstream: by the time an entry
            # reaches composition its terminality has already been flattened into
            # REVIEWED, and nothing downstream can recover it.
            spent=str(ruling.get("state") or state) in _TERMINAL_STATES
            if isinstance(ruling, dict)
            else state in _TERMINAL_STATES,
            mechanism="janus.ruling",
            meaning=(
                "Janus records pending authority; it does not grant authority. "
                "A ruling binds a SHA-256 rather than a name, and reading a "
                "ruling is not authority."
            ),
        )
