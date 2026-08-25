"""The capture must refuse rather than write something it cannot vouch for."""

from __future__ import annotations

import json

import pytest

from mosaic.store.fixtures.capture import Capture, CaptureRefused, write_fixture
from mosaic.store.fixtures.scrub import REDACTED

FAKE = {"FAKE_SECRET_VALUE_abcdef123456"}


class TestFailsClosed:
    def test_an_empty_secret_list_refuses_the_capture(self, tmp_path):
        """'Nothing to check against' is not 'nothing found'."""
        with pytest.raises(CaptureRefused, match="cannot be verified"):
            write_fixture(Capture("athena", "http", {"a": 1}), tmp_path, known=set())

    def test_nothing_is_written_when_it_refuses(self, tmp_path):
        with pytest.raises(CaptureRefused):
            write_fixture(Capture("athena", "http", {"a": 1}), tmp_path, known=set())
        assert list(tmp_path.iterdir()) == []

    def test_the_error_never_names_the_secret(self, tmp_path):
        """Reporting which secret leaked would leak it into the error."""
        try:
            write_fixture(Capture("x", "http", {"a": 1}), tmp_path, known=set())
        except CaptureRefused as exc:
            assert not any(s in str(exc) for s in FAKE)


class TestScrubBeforeWrite:
    def test_a_secret_never_reaches_disk(self, tmp_path):
        payload = {"body": f"key={sorted(FAKE)[0]}", "token": "anything"}
        path = write_fixture(Capture("athena", "http", payload), tmp_path, known=FAKE)
        text = path.read_text()
        assert not any(s in text for s in FAKE)
        assert REDACTED in text

    def test_the_fixture_records_that_it_was_scrubbed(self, tmp_path):
        path = write_fixture(Capture("janus", "subprocess", {"a": 1}), tmp_path, known=FAKE)
        assert json.loads(path.read_text())["scrubbed"] is True

    def test_ordinary_content_survives_capture(self, tmp_path):
        payload = {"ref": "g3689e1cf957", "status": "superseded", "sha": "a" * 64}
        path = write_fixture(Capture("janus", "subprocess", payload), tmp_path, known=FAKE)
        assert json.loads(path.read_text())["payload"] == payload
