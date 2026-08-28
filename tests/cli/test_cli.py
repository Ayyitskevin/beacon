"""The CLI surface: what it renders, and how it refuses."""

from __future__ import annotations

import json
import sys
import types
from pathlib import Path
from typing import ClassVar
from unittest import mock

import pytest

from beacon_legacy.cli import EXIT_ERROR, EXIT_OK, EXIT_UNAVAILABLE, EXIT_USAGE, main

FIXTURE = Path(__file__).parent / "fixtures" / "desk_sample.json"


class TestRender:
    def test_renders_a_file(self, capsys):
        assert main(["render", str(FIXTURE)]) == EXIT_OK
        out = capsys.readouterr().out
        assert "DESK · desk.v1 · 10 entries" in out
        assert "need you to act" in out

    def test_renders_stdin(self, capsys, monkeypatch):
        import io

        monkeypatch.setattr("sys.stdin", io.StringIO(FIXTURE.read_text()))
        assert main(["render", "-"]) == EXIT_OK
        assert "DESK" in capsys.readouterr().out

    def test_json_format_is_valid_json(self, capsys):
        assert main(["render", str(FIXTURE), "--format", "json"]) == EXIT_OK
        parsed = json.loads(capsys.readouterr().out)
        assert sum(parsed["operator_actions"].values()) == 10
        assert parsed["operator_actions"]["act"] == 1

    def test_the_sample_desk_covers_the_states_that_matter(self, capsys):
        main(["render", str(FIXTURE), "--format", "json"])
        views = [e["authority_view"] for e in json.loads(capsys.readouterr().out)["entries"]]
        actions = {v["operator_action"] for v in views}
        assert actions == {"act", "decide", "none"}, "fixture must exercise all three"
        # The inversion the whole doctrine turns on: a reported drift demands a
        # human, an enforced drift does not.
        assert any(
            v["operator_action"] == "act"
            and v["attachment"] == "bytes-drifted"
            and v["enforcement"] == "reported"
            for v in views
        )
        assert any(
            v["operator_action"] == "none"
            and v["attachment"] == "bytes-drifted"
            and v["enforcement"] == "enforced"
            for v in views
        )
        assert any(v["attachment"] == "unbound" for v in views)
        # Most entries carry no decision at all, and that is not a failure.
        assert any(not v["has_approval"] and not v["unreadable"] for v in views)


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
    """A missing lane must not look like an empty fleet.

    Absence is simulated rather than assumed. An earlier version of these tests
    asserted that `doctor` exits 69 because `beacon_legacy.doctor` did not exist yet —
    which measured the state of a sibling lane rather than this CLI's contract,
    and started failing the moment Lane B landed. Forcing the ImportError keeps
    the refusal path under test forever, in both worlds.
    """

    LANES: ClassVar[dict[str, str]] = {
        "desk": "beacon_legacy.compose",
        "doctor": "beacon_legacy.doctor",
    }

    @pytest.fixture
    def absent(self, monkeypatch):
        def _absent(module_path):
            # sys.modules[name] = None makes `import name` raise ImportError,
            # which is exactly what a lane that has not landed looks like.
            monkeypatch.setitem(sys.modules, module_path, None)

        return _absent

    @pytest.mark.parametrize("command", sorted(LANES))
    def test_exit_69_and_names_the_lane(self, capsys, absent, command):
        absent(self.LANES[command])
        assert main([command]) == EXIT_UNAVAILABLE
        err = capsys.readouterr().err
        assert "not available yet" in err
        assert "lane" in err.lower()
        assert "Traceback" not in err

    @pytest.mark.parametrize("command", sorted(LANES))
    def test_prints_nothing_to_stdout(self, capsys, absent, command):
        absent(self.LANES[command])
        main([command])
        assert capsys.readouterr().out == ""

    def test_a_landed_lane_is_distinguishable_from_a_missing_one(self, capsys, absent):
        """Exit 69 must mean 'not built', never 'built and found nothing'."""
        absent("beacon_legacy.compose")
        assert main(["desk"]) == EXIT_UNAVAILABLE
        capsys.readouterr()

        empty = types.ModuleType("beacon_legacy.compose")
        empty.compose_desk = lambda: {"schema": "desk.v1", "entries": []}
        with mock.patch.dict(sys.modules, {"beacon_legacy.compose": empty}):
            assert main(["desk"]) == EXIT_OK
        assert "no entries" in capsys.readouterr().out


class TestTheSeamsSiblingLanesPlugInto:
    """The two function names Lanes A and B were told to match."""

    def test_desk_renders_whatever_compose_returns(self, capsys):
        fake = types.ModuleType("beacon_legacy.compose")
        fake.compose_desk = lambda: {
            "schema": "desk.v1",
            "entries": [
                {
                    "source": "janus",
                    "owner": "kevin",
                    "title": "seam check",
                    "status": "approved",
                    "link": "janus://gates/x",
                    "authority": {
                        "decider": "human",
                        "attachment": "bytes-drifted",
                        "enforcement": "reported",
                        "lifecycle": "authorized",
                        "mechanism": "janus.ruling",
                        "meaning": "a human ruled on specific bytes",
                    },
                }
            ],
        }
        with mock.patch.dict(sys.modules, {"beacon_legacy.compose": fake}):
            assert main(["desk"]) == EXIT_OK
        out = capsys.readouterr().out
        assert "seam check" in out
        assert "need you to act" in out

    def test_doctor_text_uses_the_lane_renderer_not_json(self, capsys):
        """Text mode must not silently fall back to dumping JSON."""
        fake = types.ModuleType("beacon_legacy.doctor")
        fake.run_doctor = lambda: {"schema": "beacon_legacy.doctor.v1", "adapters": []}
        fake.render_text = lambda report: f"DOCTOR · {report['schema']} · human-readable"
        with mock.patch.dict(sys.modules, {"beacon_legacy.doctor": fake}):
            assert main(["doctor"]) == EXIT_OK
        out = capsys.readouterr().out
        assert "human-readable" in out
        assert "{" not in out

    def test_doctor_json_does_not_need_the_text_renderer(self, capsys):
        fake = types.ModuleType("beacon_legacy.doctor")
        fake.run_doctor = lambda: {"schema": "beacon_legacy.doctor.v1", "adapters": []}
        with mock.patch.dict(sys.modules, {"beacon_legacy.doctor": fake}):
            assert main(["doctor", "--format", "json"]) == EXIT_OK
        assert json.loads(capsys.readouterr().out)["schema"] == "beacon_legacy.doctor.v1"

    def test_doctor_text_refuses_when_only_render_text_is_missing(self, capsys):
        """A half-landed lane refuses in text mode but still works in JSON."""
        fake = types.ModuleType("beacon_legacy.doctor")
        fake.run_doctor = lambda: {"schema": "beacon_legacy.doctor.v1", "adapters": []}
        with mock.patch.dict(sys.modules, {"beacon_legacy.doctor": fake}):
            assert main(["doctor"]) == EXIT_UNAVAILABLE
            err = capsys.readouterr().err
            assert "render_text" in err
            assert main(["doctor", "--format", "json"]) == EXIT_OK


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
        assert "beacon-legacy-desk" in capsys.readouterr().out

    def test_no_command_prints_help(self, capsys):
        assert main([]) == EXIT_USAGE
        assert "usage:" in capsys.readouterr().out
