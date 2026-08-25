"""The Janus migration reader: gates into records, read-only, faithfully.

The first reader built end-to-end, and the proof that the stack composes:
envelope + authority + integrity + reader interface + fixture corpus, for one
real capability, against a frozen corpus rather than a live service.

Janus is the right first one because it has the richest authority shape — it
is the only source that represents terminal-negative as a status *value*, and
its bindings exercise every attachment state the model has.

Three rules this reader exists to demonstrate, each traced to a defect found
earlier:

1. **The origin word is carried unconditionally.** Janus's `superseded`
   mapped cleanly onto a lifecycle value once and the information died inside
   a mapping that SUCCEEDED. So `origin_status` holds Janus's own word, always,
   not only when mapping fails.
2. **An unrecognised value is refused, never defaulted.** Totality check 3 at
   the migration ingress, tested against the owning lane's ONE definition.
3. **The inputs of derived values are carried, not the verdicts.** Janus's
   decay is a re-runnable predicate plus observations; its verdict is computed
   at read. Storing the verdict would freeze a clock-derived value.
"""

from __future__ import annotations

import json
from pathlib import Path

from ..readers import (
    Quiescence,
    SourceRecord,
    SourceSurface,
    SurfaceKind,
    UnknownSourceValue,
    require_quiescent,
)

#: Janus's own gate states. Read from the source's vocabulary, not invented:
#: `janus list --state` accepts exactly these.
JANUS_STATES = frozenset(
    {"open", "approved", "refused", "expired", "withdrawn", "superseded"}
)

#: Janus's binding kinds, including the one the live ledger actually
#: exercises. `none` is not absence of the field — it is Janus recording that
#: the ruling is bound to nothing, which is a different and weaker fact than a
#: binding nobody checked.
JANUS_BINDING_KINDS = frozenset({"git", "file", "text", "none"})


class JanusReader:
    """Reads Janus gates from a frozen corpus or a quiesced source.

    The corpus path is the default because a reader whose correctness can only
    be checked against a live system stops being checkable the moment that
    system retires — which is the whole reason the corpus exists.
    """

    origin = "migrated:janus"

    def __init__(self, corpus: Path) -> None:
        self.corpus = corpus
        self.surface = SourceSurface(
            kind=SurfaceKind.FILE,
            boundary="process: the corpus is a frozen file, opened read-only",
            quiescence=Quiescence(
                method="frozen_snapshot",
                evidence=f"captured to {corpus.name} on 2026-08-25; not the live ledger",
                verified=True,
            ),
            justification=(
                "Janus's CLI exposes every column this reader needs — binding kind, "
                "locator and sha256 are all present in `list --json`, verified "
                "against the live ledger. So the derived values' inputs are "
                "reachable through this surface and no direct database read is "
                "warranted."
            ),
        )

    def read(self) -> list[SourceRecord]:
        require_quiescent(self.surface)
        payload = json.loads(self.corpus.read_text())["payload"]
        gates = payload["list --state all"]
        return [self._to_record(gate) for gate in gates]

    def _to_record(self, gate: dict) -> SourceRecord:
        state = gate.get("state")
        if state not in JANUS_STATES:
            raise UnknownSourceValue(
                f"gate {gate.get('id')} has state {state!r}, which is not one of "
                f"{sorted(JANUS_STATES)}. Refused rather than defaulted: mapping an "
                f"unknown state onto a known one is how a terminal gate becomes a "
                f"live one during a migration nobody re-reads."
            )

        binding_kind = gate.get("binding_kind")
        # None is the absence of a binding record; the string 'none' would be
        # Janus asserting there is nothing to bind. Both mean unbound here, and
        # they are read separately rather than conflated by a falsy check.
        if binding_kind is not None and binding_kind not in JANUS_BINDING_KINDS:
            raise UnknownSourceValue(
                f"gate {gate.get('id')} has binding_kind {binding_kind!r}, "
                f"not one of {sorted(JANUS_BINDING_KINDS)}."
            )

        return SourceRecord(
            origin=self.origin,
            origin_ref=gate["id"],
            # Janus's OWN word, verbatim and unconditional. `superseded`,
            # `withdrawn` and `expired` have no home in a forward-progress
            # lifecycle vocabulary, and this is the last layer that holds them.
            origin_status=state,
            payload={
                "question": gate.get("question", ""),
                "kind": gate.get("kind", ""),
                "raised_at": gate.get("raised_at", ""),
                "raised_by": gate.get("raised_by", ""),
                "binding_kind": binding_kind,
                "binding_locator": gate.get("binding_locator"),
                "binding_sha256": gate.get("binding_sha256"),
                "ruling": gate.get("ruling"),
                "options": gate.get("options") or [],
                "cites": gate.get("cites") or [],
            },
            derived_inputs={
                # Janus computes a decay VERDICT at read time from these. The
                # verdict is clock-derived and must never be stored; its inputs
                # must be, or it can be trusted afterwards but never recomputed.
                "horizon": gate.get("horizon"),
                "decay_check": gate.get("decay_check"),
                "observations": gate.get("observations") or [],
                "check_revisions": gate.get("check_revisions") or [],
            },
        )
