"""The OCR fallback must hit the current fixed-window Admit label.

The fixture is the Admit row from host OCR of the export-cleared, post-unlock
Ubuntu 22.04 screenshot at source 1cdf328d. It is geometry evidence, not a
claim that the runtime's Tesseract 4 recognized this word (it did not).
Screenshot SHA256:
b1bbee113a72ac164519e29a8ae56cdaceb9197c656003a54c7403d4d2f9a63e
AppImage SHA256:
2d2637c8b56119189d8c4e8f22f6bd2f39e27fb3ec5ad8786ad072a09a47e370
"""

from __future__ import annotations

import csv
import os
import subprocess
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]


def test_admit_fallback_hits_the_measured_current_starter_button(tmp_path: Path):
    fixture = Path(__file__).with_name("fixtures") / "first-run-admit-current.tsv"
    with fixture.open(encoding="utf-8", newline="") as handle:
        rows = list(csv.DictReader(handle, delimiter="\t"))
    assert len(rows) == 1 and rows[0]["text"] == "Admit"
    left, top, width, height = (int(rows[0][field]) for field in ("left", "top", "width", "height"))
    assert width > 0 and height > 0

    # Only the external mouse tool is replaced; execute the driver's real helper.
    xdotool = tmp_path / "xdotool"
    xdotool.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n', encoding="utf-8")
    xdotool.chmod(0o755)
    result = subprocess.run(
        [
            "bash",
            "-c",
            'source "$1"; first_run_click_admit_fallback',
            "admit-fallback-probe",
            str(REPO / "scripts" / "clean-host-first-run-state"),
        ],
        env={**os.environ, "PATH": f"{tmp_path}:{os.environ['PATH']}"},
        capture_output=True,
        text=True,
        check=True,
    )
    arguments = result.stdout.splitlines()
    assert len(arguments) == 6
    assert arguments[:2] == ["mousemove", "--sync"]
    assert arguments[4:] == ["click", "1"]
    x, y = map(int, arguments[2:4])
    assert left <= x < left + width and top <= y < top + height, (
        f"Admit fallback ({x}, {y}) misses measured current label "
        f"({left}, {top}, {width}, {height})"
    )
