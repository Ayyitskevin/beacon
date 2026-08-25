"""Composition rules: purity, and honesty about what is missing."""

from __future__ import annotations

import json

from mosaic.compose import compose, to_document
from mosaic.contracts import (
    Attachment,
    Authority,
    Decider,
    Enforcement,
    Entry,
    Lifecycle,
    Provenance,
    Unreachable,
)

PROV = Provenance(
    source="git:/example/checkouts/icarus@af41796",
    read_at="2026-08-24T23:30:00+00:00",
    module_version="af41796",
    tree_state="clean",
)


def _entry(
    ref: str,
    timestamp: str,
    source: str = "janus",
    status: str = "approved",
    authority: Authority | None = ...,  # type: ignore[assignment]
) -> Entry:
    if authority is ...:
        authority = Authority(
            decider=Decider.HUMAN,
            attachment=Attachment.BYTES_VERIFIED,
            enforcement=Enforcement.REPORTED,
            lifecycle=Lifecycle.AUTHORIZED,
            mechanism="janus.ruling",
            meaning="a human ruled on these exact bytes",
        )
    return Entry(
        source=source,
        owner="kevin",
        title=f"gate {ref}",
        status=status,
        timestamp=timestamp,
        schema_version="gate.v1",
        ref=ref,
        provenance=PROV,
        authority=authority,
    )


class TestPurity:
    def test_compose_is_deterministic(self):
        """Same input, same document -- no clock, no network, no randomness.
        `generated_at` is an argument precisely so this holds."""
        results = [((_entry("a", "2026-08-24T10:00:00Z"),), ())]
        first = compose(results, generated_at="2026-08-24T23:00:00Z")
        second = compose(results, generated_at="2026-08-24T23:00:00Z")
        assert first == second

    def test_document_is_json_serializable(self):
        """Renderers consume a plain mapping, so it must survive json.dumps
        with no custom encoder. StrEnum members serialize as their values."""
        desk = compose(
            [((_entry("a", "2026-08-24T10:00:00Z"),), ())],
            generated_at="2026-08-24T23:00:00Z",
        )
        blob = json.dumps(to_document(desk))
        assert '"authorized"' in blob
        assert '"bytes-verified"' in blob


class TestMissingJoinsAreResults:
    def test_unreachable_survives_composition(self):
        """A failing join is a RESULT, not a bug. v0's whole value is naming
        precisely which joins do not exist yet, so dropping these would defeat
        the point of the slice."""
        missing = Unreachable(module="icarus", reason="repo-only, no listener")
        desk = compose([((), (missing,))], generated_at="2026-08-24T23:00:00Z")
        assert desk.unreachable == (missing,)
        assert not desk.complete

    def test_empty_read_differs_from_failed_read(self):
        """An adapter that read nothing and one that could not read are
        different facts about the world. A desk rendering them alike lies by
        omission."""
        empty = compose([((), ())], generated_at="2026-08-24T23:00:00Z")
        failed = compose(
            [((), (Unreachable(module="buzz", reason="connection refused"),))],
            generated_at="2026-08-24T23:00:00Z",
        )
        assert empty.entries == failed.entries == ()
        assert empty.complete
        assert not failed.complete

    def test_partial_read_keeps_both_halves(self):
        """One adapter reaching some surfaces and missing others is the normal
        case, not an error state."""
        desk = compose(
            [
                (
                    (_entry("a", "2026-08-24T10:00:00Z"),),
                    (Unreachable(module="athena", reason="join absent"),),
                )
            ],
            generated_at="2026-08-24T23:00:00Z",
        )
        assert len(desk.entries) == 1
        assert len(desk.unreachable) == 1


class TestStatusIsTheModulesOwnWord:
    """`Entry.status` must never be Mosaic's vocabulary."""

    def test_module_status_survives_verbatim(self):
        """Janus has three terminal-negative states with no home in an
        approval enum. When `status` was typed `Lifecycle`, all three mapped
        onto PROPOSED and 4 of 13 live gates rendered identically to a live
        open gate -- a question nobody answered and a question the world moved
        past, wearing one value."""
        for word in ("superseded", "withdrawn", "expired", "in_progress", "done"):
            desk = compose(
                [((_entry("a", "2026-08-24T10:00:00Z", status=word),), ())],
                generated_at="2026-08-24T23:00:00Z",
            )
            assert desk.entries[0].status == word

    def test_status_does_not_borrow_the_approval_vocabulary(self):
        """An Athena issue is work, not an approval. Forcing `done` into a
        five-value approval enum either understates it or asserts a check
        nobody ran. The approval vocabulary lives on `Authority.lifecycle`,
        populated only when there IS an approval."""
        desk = compose(
            [((_entry("a", "2026-08-24T10:00:00Z", status="done", authority=None),), ())],
            generated_at="2026-08-24T23:00:00Z",
        )
        entry = desk.entries[0]
        assert entry.status == "done"
        assert entry.authority is None

    def test_status_and_lifecycle_are_not_the_same_field(self):
        """They were duplicated for four commits. Two fields holding one
        concept can disagree, and the loser was populated by a lossy mapping
        table living in adapters -- doctrine in the wrong place."""
        entry = _entry("a", "2026-08-24T10:00:00Z", status="superseded")
        assert entry.status == "superseded"
        assert entry.authority is not None
        assert entry.authority.lifecycle is Lifecycle.AUTHORIZED


class TestOrdering:
    def test_entries_are_ordered_by_recency(self):
        results = [
            (
                (
                    _entry("old", "2026-08-20T10:00:00Z"),
                    _entry("new", "2026-08-24T10:00:00Z"),
                ),
                (),
            )
        ]
        desk = compose(results, generated_at="2026-08-24T23:00:00Z")
        assert [e.ref for e in desk.entries] == ["new", "old"]

    def test_ordering_ignores_authority_strength(self):
        """Recency, explicitly NOT importance. An ordering that floated
        "stronger" approvals to the top would be the authority ranking this
        project refuses, smuggled in as a display detail."""
        weak = _entry("weak", "2026-08-24T12:00:00Z", source="buzz")
        strong = _entry("strong", "2026-08-24T09:00:00Z", source="icarus")
        desk = compose([((strong, weak), ())], generated_at="2026-08-24T23:00:00Z")
        assert [e.ref for e in desk.entries] == ["weak", "strong"]
