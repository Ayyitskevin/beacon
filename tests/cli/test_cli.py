"""The CLI surface: what it renders, and how it refuses."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from mosaic.cli import EXIT_ERROR, EXIT_OK, EXIT_UNAVAILABLE, EXIT_USAGE, main

FIXTURE = Path(__file__).parent / "fixtures" / "desk_sample.json"


class TestRender:
    def test_renders_a_file(self, capsys):
        assert main(["render", str(FIXTURE)]) == EXIT_OK
        out = capsys.readouterr().out
        assert "DESK · desk.v1 · 5 entries" in out
        assert "1 entry renders as authority" in out

    def test_renders_stdin(self, capsys, monkeypatch):
        import io

        monkeypatch.setattr("sys.stdin", io.StringIO(FIXTURE.read_text()))
        assert main(["render", "-"]) == EXIT_OK
        assert "DESK" in capsys.readouterr().out

    def test_json_format_is_valid_json(self, capsys):
        assert main(["render", str(FIXTURE), "--format", "json"]) == EXIT_OK
        parsed = json.loads(capsys.readouterr().out)
        assert parsed["authority_summary"]["total"] == 5
        assert parsed["authority_summary"]["renders_as_authority"] == 1

    def test_the_sample_desk_exercises_every_authority_path(self, capsys):
        """The fixture is a worked example, so it must cover the hard cases."""
        main(["render", str(FIXTURE), "--format", "json"])
        entries = json.loads(capsys.readouterr().out)["entries"]
        verdicts = [e["authority_verdict"] for e in entries]
        assert any(v["is_authority"] for v in verdicts)
        assert any("STALE" in v["display"] for v in verdicts)
        assert any(not v["is_authority"] and v["label"] == "verified" for v in verdicts)


class TestRefusalsAreSentences:
    def test_missing_file(self, capsys):
        assert main(["render", "/nonexistent/desk.json"]) == EXIT_ERROR
        err = capsys.readouterr().err
        assert "could not read" in err
        assert "Traceback" not in err

    def test_malformed_json_names_the_line(self, capsys, tmp_path):
        bad = tmp_path / "bad.json"
        bad.write_text('{"schema": "desk.v1", "entries": [}')
        assert main(["render", str(bad)]) == EXIT_ERROR
        assert "not valid JSON" in capsys.readouterr().err

    def test_empty_file(self, capsys, tmp_path):
        empty = tmp_path / "empty.json"
        empty.write_text("   \n")
        assert main(["render", str(empty)]) == EXIT_ERROR
        assert "was empty" in capsys.readouterr().err

    def test_json_array_is_not_a_desk(self, capsys, tmp_path):
        arr = tmp_path / "arr.json"
        arr.write_text("[1, 2, 3]")
        assert main(["render", str(arr)]) == EXIT_ERROR
        assert "not an object" in capsys.readouterr().err


class TestUnlandedLanesRefuseHonestly:
    """A missing lane must not look like an empty fleet."""

    @pytest.mark.parametrize("command", ["desk", "doctor"])
    def test_exit_69_and_names_the_lane(self, capsys, command):
        assert main([command]) == EXIT_UNAVAILABLE
        err = capsys.readouterr().err
        assert "not available yet" in err
        assert "lane" in err.lower()
        assert "Traceback" not in err

    @pytest.mark.parametrize("command", ["desk", "doctor"])
    def test_prints_nothing_to_stdout(self, capsys, command):
        main([command])
        assert capsys.readouterr().out == ""


class TestPlumbing:
    def test_empty_desk_says_so(self, capsys, tmp_path):
        doc = tmp_path / "empty_desk.json"
        doc.write_text('{"schema": "desk.v1", "entries": []}')
        assert main(["render", str(doc)]) == EXIT_OK
        out = capsys.readouterr().out
        assert out.strip()
        assert "no entries" in out

    def test_version(self, capsys):
        assert main(["version"]) == EXIT_OK
        assert "mosaic" in capsys.readouterr().out

    def test_no_command_prints_help(self, capsys):
        assert main([]) == EXIT_USAGE
        assert "usage:" in capsys.readouterr().out
