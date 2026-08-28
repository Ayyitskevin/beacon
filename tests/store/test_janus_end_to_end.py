"""One real capability through the whole stack.

Frozen corpus -> reader -> vocabulary -> envelope -> integrity. If this passes,
the pieces compose; the design phase's argument was that they would, and this
is the only thing that settles it.
"""

from __future__ import annotations

import json
import re
import sqlite3
from pathlib import Path

import pytest

from beacon_legacy.store.integrity import orphaned_claims
from beacon_legacy.store.readers import MigrationReader, UnknownSourceValue
from beacon_legacy.store.readers_impl.janus_load import STATE_POLARITY, load
from beacon_legacy.store.readers_impl.janus_reader import JanusReader
from beacon_legacy.store.reconcile import Resolution, reconcile_vocabularies
from beacon_legacy.store.schema import apply_schema

CORPUS = (
    Path(__file__).resolve().parents[2]
    / "src"
    / "beacon_legacy"
    / "store"
    / "fixtures"
    / "corpus"
    / "janus.json"
)


@pytest.fixture
def records():
    return JanusReader(CORPUS).read()


@pytest.fixture
def loaded(records):
    conn = sqlite3.connect(":memory:")
    apply_schema(conn)
    load(conn, records, "2026-08-25T14:00:00Z")
    return conn


class TestTheReaderReads:
    def test_it_satisfies_the_migration_protocol(self):
        assert isinstance(JanusReader(CORPUS), MigrationReader)

    def test_it_reads_the_whole_corpus(self, records):
        assert len(records) == 16

    def test_it_refuses_an_unknown_state_rather_than_defaulting(self, tmp_path):
        """Totality check 3 at the migration ingress."""
        doc = json.loads(CORPUS.read_text())
        doc["payload"]["list --state all"][0]["state"] = "probably_fine"
        bad = tmp_path / "janus.json"
        bad.write_text(json.dumps(doc))
        with pytest.raises(UnknownSourceValue, match="probably_fine"):
            JanusReader(bad).read()

    def test_it_refuses_an_unknown_binding_kind(self, tmp_path):
        doc = json.loads(CORPUS.read_text())
        doc["payload"]["list --state all"][0]["binding_kind"] = "vibes"
        bad = tmp_path / "janus.json"
        bad.write_text(json.dumps(doc))
        with pytest.raises(UnknownSourceValue, match="vibes"):
            JanusReader(bad).read()


class TestTheCollapseThatStartedThisIsNowImpossible:
    """Five superseded gates once rendered identically to live approved ones."""

    def test_janus_own_word_survives_into_the_store(self, loaded):
        counts = dict(loaded.execute("SELECT status, count(*) FROM record GROUP BY status"))
        assert counts == {"approved": 8, "open": 3, "superseded": 5}

    def test_superseded_is_terminal_and_open_is_not(self, loaded):
        rows = dict(
            loaded.execute(
                "SELECT m.polarity, count(*) FROM record r JOIN vocabulary_member m "
                "ON m.capability=r.capability AND m.kind=r.kind AND m.member=r.status "
                "GROUP BY m.polarity"
            )
        )
        assert rows == {"active": 3, "terminal_negative": 5, "terminal_positive": 8}

    def test_the_word_and_the_polarity_are_separate_facts(self):
        """The polarity table decides how a status SORTS, never what it is
        called. Collapsing them is what made superseded read as live."""
        assert STATE_POLARITY["superseded"] == "terminal_negative"
        assert STATE_POLARITY["approved"] == "terminal_positive"
        assert STATE_POLARITY["superseded"] != STATE_POLARITY["approved"]

    def test_every_terminal_negative_state_is_representable(self):
        """Three of them, none of which fits a forward-progress lifecycle."""
        terminal = {s for s, p in STATE_POLARITY.items() if p == "terminal_negative"}
        assert terminal == {"refused", "withdrawn", "expired", "superseded"}


class TestDerivedValuesTravelAsInputs:
    def test_the_decay_verdict_is_not_stored(self, loaded):
        """It is clock-derived: storing it freezes a verdict stale when written."""
        for (body,) in loaded.execute("SELECT body FROM record"):
            assert "decay_verdict" not in json.loads(body)

    def test_its_inputs_are(self, loaded):
        with_inputs = [
            json.loads(u)
            for (u,) in loaded.execute("SELECT body_unmapped FROM record")
            if json.loads(u).get("observations")
        ]
        assert with_inputs, "no gate carried decay inputs; the fixture should have some"
        for payload in with_inputs:
            assert "decay_check" in payload
            assert "horizon" in payload


class TestTheStackHoldsAfterLoading:
    def test_binding_kinds_survive_intact(self, loaded):
        kinds = [
            json.loads(b)["binding_kind"] for (b,) in loaded.execute("SELECT body FROM record")
        ]
        assert kinds.count("git") == 9
        assert kinds.count("file") == 3
        assert kinds.count(None) == 4

    def test_integrity_rules_still_hold(self, loaded):
        assert orphaned_claims(loaded) == []

    def test_every_vocabulary_still_reconciles(self, loaded):
        reports = reconcile_vocabularies(loaded)
        assert reports
        assert all(r.resolution is Resolution.MATCHED for r in reports)

    def test_provenance_names_the_source_for_every_record(self, loaded):
        """A record that cannot name its source is a bug, and after Janus
        retires nothing else can supply it."""
        rows = list(loaded.execute("SELECT provenance_origin, provenance_source_ref FROM record"))
        assert len(rows) == 16
        assert all(o == "migrated:janus" and ref for o, ref in rows)


class TestTheReadersCopyOfTheSourceVocabularyIsVerified:
    """A reader necessarily holds a copy of its source's vocabulary.

    `JANUS_STATES` is a hand-written frozenset of another system's states. We
    cannot import it — Janus is a separate program — so this is the one place
    a second copy is unavoidable rather than a defect. What is avoidable is
    leaving it unchecked.

    The source can verify it, and only while the source still exists: `janus
    list --help` prints its own state vocabulary. After Janus retires, this
    test becomes unrunnable and the copy becomes unverifiable forever, which
    is precisely the argument for the fixture corpus.
    """

    def test_the_readers_state_set_matches_what_janus_declares(self):
        import shutil
        import subprocess

        from beacon_legacy.store.readers_impl.janus_reader import JANUS_STATES

        if not shutil.which("janus"):
            pytest.skip("janus is not on PATH; cannot verify against the source")

        help_text = subprocess.run(
            ["janus", "list", "--help"], capture_output=True, text=True, timeout=10
        ).stdout
        match = re.search(r"\{([a-z,]+)\}", help_text)
        assert match, "janus list --help no longer prints its state vocabulary"

        declared = set(match.group(1).split(","))
        # `all` is a filter alias meaning "every state", not a state a gate can
        # be in. Excluded deliberately: a reader that accepted it as a status
        # would store a filter keyword as a fact about a record.
        declared.discard("all")

        assert declared == JANUS_STATES, (
            f"the reader's copy of Janus's states has drifted. "
            f"Janus declares {sorted(declared)}; the reader holds "
            f"{sorted(JANUS_STATES)}. Fix the reader, not this test."
        )

    def test_a_truncated_title_is_recoverable_from_the_body(self, loaded):
        """Titles are capped at 200; five real gates exceed it. That is only
        acceptable because the full text survives in `body` — a cap that
        discarded the remainder would be silent loss in the field a human
        reads first."""
        rows = list(loaded.execute("SELECT title, body FROM record"))
        truncated = [(t, json.loads(b)["question"]) for t, b in rows if len(t) == 200]
        assert truncated, "the fixture should contain at least one long question"
        for title, question in truncated:
            assert question.startswith(title)
            assert len(question) > len(title)
