"""Pin `sys.path` to THIS worktree before anything imports `mosaic`.

This has to live at the tests/ root, and per-directory conftests cannot
replace it. pytest collects alphabetically, so `tests/adapters/` is walked
first; it has no conftest, `mosaic` is imported from whatever is already on
`sys.path` — the sibling checkout's editable install — and it lands in
`sys.modules`. Every later conftest inserting this worktree's `src/` is then
too late: the package object already exists, bound to the other tree, and
`mosaic.store` does not exist there. The failure reads as "no module named
mosaic.store", which sounds like a missing file rather than a wrong tree.

The same trap in its quieter form has now caught three sessions today: a
mutation applied to one checkout while the suite imported another, so the
tests reported green on code they never loaded. Green against the wrong tree
is worse than red.

`insert(0, ...)` rather than `append`: site-packages must lose.
"""

from __future__ import annotations

import sys
from pathlib import Path

_SRC = Path(__file__).resolve().parent.parent / "src"
if str(_SRC) not in sys.path[:1]:
    sys.path.insert(0, str(_SRC))
