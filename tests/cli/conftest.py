"""Make ``src/`` importable when the package has not been installed.

CI installs the package (``pip install -e .[dev]``), so this is only a
convenience for running ``pytest`` in a bare checkout. It lives in this lane's
test directory rather than at ``tests/`` root so that three lanes writing tests
in parallel never contend for one file.
"""

from __future__ import annotations

import sys
from pathlib import Path

_SRC = Path(__file__).resolve().parents[2] / "src"
if str(_SRC) not in sys.path:
    sys.path.insert(0, str(_SRC))
