"""The committed corpus must stay free of secrets, permanently.

Capture-time scrubbing protects the moment of capture. This protects every
moment after: a fixture edited by hand, a new capture added by someone reading
a different runbook, or a source that starts returning a credential it did not
return today.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

import pytest

from mosaic.store.fixtures.scrub import known_secret_values

CORPUS = Path(__file__).resolve().parents[2] / "src" / "mosaic" / "store" / "fixtures" / "corpus"
FIXTURES = sorted(CORPUS.glob("*.json"))

SECRET_SHAPES = re.compile(
    r"Bearer \S|sk-ant-[A-Za-z0-9]|ghp_[A-Za-z0-9]|xai-[A-Za-z0-9]"
    r"|eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\."
    r"|-----BEGIN [A-Z ]*PRIVATE KEY-----"
)


def test_the_corpus_is_not_empty():
    """Otherwise every test below passes over nothing — a filter that matches
    nothing reads exactly like a clean result."""
    assert FIXTURES, f"no fixtures found in {CORPUS}"


@pytest.mark.parametrize("path", FIXTURES, ids=lambda p: p.name)
class TestNoFixtureCarriesASecret:
    def test_no_live_secret_appears(self, path):
        known = known_secret_values()
        if not known:
            pytest.skip("no secret sources readable on this host")
        raw = path.read_text()
        # Asserts on a boolean; never names or prints the offending value.
        assert not any(secret in raw for secret in known)

    def test_no_secret_shape_appears(self, path):
        assert SECRET_SHAPES.search(path.read_text()) is None

    def test_it_records_that_it_was_scrubbed(self, path):
        assert json.loads(path.read_text())["scrubbed"] is True


class TestTheCorpusIsActuallyUseful:
    """Over-scrubbing is a real cost. A corpus of redactions verifies nothing."""

    def test_janus_captured_every_state_not_just_the_default_view(self):
        payload = json.loads((CORPUS / "janus.json").read_text())["payload"]
        assert "list --state all" in payload
        assert "list --state superseded" in payload
        assert len(payload["list --state all"]) > len(payload["list --state open"])

    def test_the_states_that_collapsed_are_represented(self):
        """superseded and approved are the two this project found rendering
        identically. A corpus without them cannot catch the regression."""
        payload = json.loads((CORPUS / "janus.json").read_text())["payload"]
        assert len(payload["list --state superseded"]) >= 1
        assert len(payload["list --state approved"]) >= 1

    def test_athena_captured_the_unfiltered_set_not_only_open(self):
        """The unfiltered read returns 60; status=open returns 26. Capturing
        only the default view would leave a reader unverified against the
        statuses that are not open — which is where the collapses live."""
        payload = json.loads((CORPUS / "athena.json").read_text())["payload"]
        every = payload["/issues?limit=100"]
        just_open = payload["/issues?status=open&limit=100"]
        assert isinstance(every, list) and isinstance(just_open, list)
        assert len(every) > len(just_open)

    def test_the_invalid_filter_value_is_recorded_not_used(self):
        """Measured during capture: /issues?status=all returns 0 issues
        silently rather than erroring. An invalid filter value that reads as
        an empty result is the vacuous-filter shape, in Athena's own API — so
        the corpus records the hazard and does not depend on that path."""
        doc = json.loads((CORPUS / "athena.json").read_text())
        assert "status=all returns 0 silently" in doc["note"]
        assert "/issues?status=all" not in doc["payload"]

    def test_each_fixture_records_the_surface_it_came_from(self):
        for path in FIXTURES:
            doc = json.loads(path.read_text())
            assert doc["surface"], f"{path.name} does not say where it came from"
            assert doc["origin"]
