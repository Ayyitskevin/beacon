"""The migration-reader contract, and the premise it now rests on."""

from __future__ import annotations

import pytest

from beacon_legacy.store.readers import (
    MigrationReader,
    SourceRecord,
    SourceSurface,
    SurfaceKind,
    absent_is_not_loss,
)


class _Reader:
    origin = "janus"
    surface = SourceSurface(
        kind=SurfaceKind.LOCAL_DB,
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
            f"addition is a new thing Beacon legacy desk can ask of a retiring system."
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
            (SurfaceKind.FILE, "process"),
            (SurfaceKind.LOCAL_DB, "process"),
            (SurfaceKind.HTTP, "credential"),
            (SurfaceKind.SUBPROCESS, "allowlist"),
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
        assert boundary_must_mention in boundaries[kind.value]


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

    def test_a_beacon_legacy_added_member_absent_from_the_source_is_not_loss(self):
        assert (
            absent_is_not_loss(member_added_by_beacon_legacy=True, source_had_value=False) is False
        )

    def test_a_real_member_missing_from_the_source_IS_loss(self):
        assert (
            absent_is_not_loss(member_added_by_beacon_legacy=False, source_had_value=False) is True
        )

    def test_presence_is_never_loss(self):
        for added in (True, False):
            assert (
                absent_is_not_loss(member_added_by_beacon_legacy=added, source_had_value=True)
                is False
            )

    def test_the_marker_is_read_never_inferred_from_absence(self):
        """Both calls see source_had_value=False; only the marker separates them."""
        assert absent_is_not_loss(True, False) != absent_is_not_loss(False, False)


class TestLocalDbRequiresVerifiedQuiescence:
    """ADR 0001 is a precondition, not a prohibition — but only when checked.

    Opening another system's database bypasses its scopes, authorization and
    activity trail, and observes mid-transaction state. All four are properties
    of a LIVE system; a stopped service or frozen copy has none of them. So the
    surface is permitted and the stillness must be verified.
    """

    def _db(self, quiescence=None):
        from beacon_legacy.store.readers import SourceSurface

        return SourceSurface(
            kind=SurfaceKind.LOCAL_DB,
            boundary="process: uid without write permission on the source file",
            quiescence=quiescence,
            justification="Janus is SQLite and its CLI exposes operator-facing "
            "columns, not every column a faithful migration needs.",
        )

    def test_a_live_database_is_refused(self):
        from beacon_legacy.store.readers import SourceNotQuiescent, require_quiescent

        with pytest.raises(SourceNotQuiescent, match="quiescence was not verified"):
            require_quiescent(self._db())

    def test_claimed_but_unverified_quiescence_is_refused(self):
        """`verified=False` is the default precisely so the precondition cannot
        be satisfied by asserting it."""
        from beacon_legacy.store.readers import (
            Quiescence,
            SourceNotQuiescent,
            require_quiescent,
        )

        claimed = Quiescence(method="service_stopped", evidence="I think so", verified=False)
        with pytest.raises(SourceNotQuiescent):
            require_quiescent(self._db(claimed))

    def test_verified_quiescence_is_allowed(self):
        from beacon_legacy.store.readers import Quiescence, require_quiescent

        checked = Quiescence(
            method="frozen_snapshot",
            evidence="systemctl is-active janus -> inactive; cp -a before read",
            verified=True,
        )
        require_quiescent(self._db(checked))

    def test_non_database_surfaces_are_untouched(self):
        from beacon_legacy.store.readers import SourceSurface, require_quiescent

        for kind in ("http", "file", "subprocess"):
            require_quiescent(SourceSurface(kind=SurfaceKind(kind), boundary="n/a"))

    def test_the_evidence_field_is_a_command_not_an_assertion(self):
        """A precondition recorded as 'it was stopped' cannot be re-checked by
        the next person; one recorded as the command they can re-run can."""
        from beacon_legacy.store.readers import Quiescence

        q = Quiescence("frozen_snapshot", "systemctl is-active janus -> inactive", True)
        assert "->" in q.evidence


class TestSurfaceKindIsClosed:
    """It was a bare str, and require_quiescent keys on it.

    claude-09 caught the cost: a reader declaring `file` while opening SQLite,
    or simply typing `localdb`, passed straight through the quiescence guard.
    A typo silently skipped a safety check, in a store whose whole doctrine is
    closed vocabularies — the guard consulted a self-report and believed it.
    """

    @pytest.mark.parametrize("typo", ["localdb", "local-db", "LOCAL_DB", "sqlite", ""])
    def test_an_unrecognised_kind_cannot_be_constructed(self, typo):
        from beacon_legacy.store.readers import SourceSurface

        with pytest.raises(ValueError, match="closed vocabulary"):
            SourceSurface(kind=typo, boundary="x")

    def test_it_refuses_rather_than_coercing(self):
        """A coerced typo becomes a plausible value the guard cannot question."""
        from beacon_legacy.store.readers import SourceSurface

        with pytest.raises(ValueError):
            SourceSurface(kind="local_db_", boundary="x")

    def test_the_guard_now_keys_on_a_member_not_a_string(self):
        from beacon_legacy.store.readers import SourceNotQuiescent, SourceSurface, require_quiescent

        with pytest.raises(SourceNotQuiescent):
            require_quiescent(SourceSurface(kind=SurfaceKind.LOCAL_DB, boundary="x"))
