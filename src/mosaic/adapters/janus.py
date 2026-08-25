"""Janus -- the gates a human has not yet ruled on.

Read through the `janus` CLI, not by opening ~/.janus/janus.db. Going through a
module's own interface is not filesystem reach-through; opening its database is.
The allowlist below is where that line is drawn in code: `janus` can rule on a
gate, withdraw one, expire one, and run decay checks that append observations.
None of those verbs are here, so no argv this adapter can assemble will reach them.

`board` is deliberately excluded despite reading like a read: it accepts `--check`,
which runs decay checks and writes observations to the ledger. An allowlist that
admits a verb because its usual invocation is harmless is not an allowlist.
"""

from __future__ import annotations

from .base import Entry, ModuleProbe, entry
from .transport import ReadOnlyCommand, Unreachable

READ_VERBS = frozenset({"list", "show", "stats", "doctor"})


class JanusAdapter:
    module = "janus"

    def __init__(self, executable: str = "janus") -> None:
        self._cli = ReadOnlyCommand(
            module=self.module,
            executable=executable,
            read_verbs=READ_VERBS,
        )

    def probe(self) -> ModuleProbe:
        try:
            gates = self._cli.run_json(["list", "--state", "all", "--json"])
        except Unreachable as exc:
            return ModuleProbe(
                module=self.module,
                reachable=False,
                address=exc.address,
                address_source="PATH",
                detail=exc.reason,
            )
        count = len(gates) if isinstance(gates, list) else 0
        return ModuleProbe(
            module=self.module,
            reachable=True,
            address=f"{self._cli.executable} (local CLI)",
            address_source="PATH",
            detail=f"{count} gates in the ledger",
        )

    def entries(self) -> list[Entry]:
        gates = self._cli.run_json(["list", "--state", "all", "--json"])
        if not isinstance(gates, list):
            raise Unreachable(self.module, self._cli.executable, "list --json was not a JSON array")

        out: list[Entry] = []
        for gate in gates:
            gate_id = gate.get("id", "")
            # binding_* is passed through verbatim rather than collapsed into a
            # boolean. Lane A's label set is fitted to these values: a gate bound
            # to a git object is not the same claim as one bound to nothing, and
            # an adapter that reported only "bound: true" would erase the
            # difference before the doctrine could act on it.
            binding = {
                "kind": gate.get("binding_kind") or "none",
                "locator": gate.get("binding_locator") or "",
                "sha256": gate.get("binding_sha256") or "",
            }
            out.append(
                entry(
                    source=self.module,
                    kind="gate",
                    ref=gate_id,
                    title=gate.get("question", ""),
                    status=gate.get("state", ""),
                    owner=gate.get("raised_by"),
                    timestamp=gate.get("raised_at"),
                    schema_version=None,  # janus list --json carries no schema field
                    canonical_link=f"janus show {gate_id}",
                    binding=binding,
                    extra={
                        "gate_kind": gate.get("kind"),
                        "decay": gate.get("decay"),
                        "consumer": gate.get("consumer"),
                        "ruling": gate.get("ruling"),
                        "option_count": len(gate.get("options") or []),
                    },
                )
            )
        return out
