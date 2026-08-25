"""Make THIS worktree's `src/` win over any editable install on sys.path.

Not a convenience. The venv in the sibling checkout has `mosaic` installed
editable pointing at *that* tree, so without this the suite silently tests the
other worktree's code — a mutation applied here would be invisible and the
tests would report green on code they never loaded. That exact trap cost a
reviewer a false green earlier today.

`insert(0, ...)` rather than `append` for the same reason: site-packages must
lose.
"""

from __future__ import annotations

import sys
from pathlib import Path

_SRC = Path(__file__).resolve().parents[2] / "src"
if sys.path[:1] != [str(_SRC)]:
    sys.path.insert(0, str(_SRC))
