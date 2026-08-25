"""The migration-reader contract, and the premise it now rests on."""

from __future__ import annotations

import pytest

from mosaic.store.readers import (
    MigrationReader,
    SourceRecord,
    SourceSurface,
    absent_is_not_loss,
)


class _Reader:
    origin = "janus"
    surface = SourceSurface(
        kind="local_db",
        boundary="process: uid without write permission on ~/.janus/janus.db",
        justification="Janus has no HTTP surface; its decay checks are derived and "
        "their inputs (binding, ruling state) are columns here.",
    )

    def read(self) -> list[SourceRecord]:
        return []


class TestTheInterfaceCannotWrite:
    def test_the_protocol_offers_exactly_one_verb(self):
        """Read-only as a property of the shape, not of each reader remembering."""
        surface = {n for n in dir(MigrationReader) if not n.startswith("_")}
        assert surface == {"read"}, (
            f"the reader protocol grew a member: {sorted(surface)}. Every "
            f"addition is a new thing Mosaic can ask of a retiring system."
        )

    def test_a_conforming_reader_satisfies_the_protocol(self):
        assert isinstance(_Reader(), MigrationReader)


class TestEverySurfaceStatesItsBoundaryAndItsReason:
    def test_a_surface_must_justify_itself(self):
        """A surface chosen by convention is a decision nobody made."""
        s = _Reader().surface
        assert s.boundary and s.justification

    @pytest.mark.parametrize(
        "kind,boundary_must_mention",
        [
            ("file", "process"),
            ("local_db", "process"),
            ("http", "credential"),
            ("subprocess", "allowlist"),
        ],
    )
    def test_the_boundary_differs_by_surface(self, kind, boundary_must_mention):
        """One uniform rule is unimplementable: a process boundary buys nothing
        against a credentialed endpoint, where the same bearer can POST."""
        boundaries = {
            "file": "process: uid without write permission",
            "local_db": "process: read-only connection plus uid",
            "http": "credential: read-only-scoped token bound to one endpoint",
            "subprocess": "allowlist: verbs the transport owns, not the caller",
        }
        assert boundary_must_mention in boundaries[kind]


class TestTheOriginWordSurvives:
    def test_a_record_carries_the_source_s_own_status_verbatim(self):
        r = SourceRecord(origin="janus", origin_ref="g1", origin_status="superseded")
        assert r.origin_status == "superseded"

    def test_it_is_carried_unconditionally_not_only_on_failure(self):
        """The Janus collapse happened on a mapping that SUCCEEDED."""
        for word in ("approved", "superseded", "withdrawn", "expired", "open"):
            assert SourceRecord("janus", "g1", word).origin_status == word

    def test_derived_inputs_have_a_home_on_the_record(self):
        r = SourceRecord(
            "athena", "MWS-2", "open", derived_inputs={"expires_at": "2026-08-25T00:00:00Z"}
        )
        assert r.derived_inputs["expires_at"]


class TestAddedMembersAreNotDataLoss:
    """Measured: only Janus has terminal-negative as a status *value*."""

    def test_a_mosaic_added_member_absent_from_the_source_is_not_loss(self):
        assert absent_is_not_loss(member_added_by_mosaic=True, source_had_value=False) is False

    def test_a_real_member_missing_from_the_source_IS_loss(self):
        assert absent_is_not_loss(member_added_by_mosaic=False, source_had_value=False) is True

    def test_presence_is_never_loss(self):
        for added in (True, False):
            assert absent_is_not_loss(member_added_by_mosaic=added, source_had_value=True) is False

    def test_the_marker_is_read_never_inferred_from_absence(self):
        """Both calls see source_had_value=False; only the marker separates them."""
        assert absent_is_not_loss(True, False) != absent_is_not_loss(False, False)
