"""Capturing a fixture corpus from the six live systems, before they retire.

Once a source system is gone, its migration reader's correctness is
unverifiable forever — there is nothing left to check it against. So the corpus
is cheap now and impossible later, which makes it R3 (buy reversibility before
you need it) applied to the migration.

Three guarantees, in the order they matter:

1. **Read-only.** GET and declared read verbs only. These are live production
   systems; a capture that perturbs one has broken the thing it was preserving.
2. **Scrub before write.** The pipeline is read -> scrub in memory -> verify ->
   write. There is no path that writes raw. A file that existed unscrubbed for
   four seconds is a file an editor swap or an interrupted run can preserve.
3. **Fail closed.** If the secret list cannot be loaded, the capture REFUSES
   rather than proceeding. "Nothing to check against" is not "nothing found" —
   that is this project's own absent-vs-clean distinction applied to its own
   safety check, and getting it backwards would write unscrubbed production
   data to disk while reporting success.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

from .scrub import contains_known_secret, known_secret_values, scrub


class CaptureRefused(Exception):
    """The capture did not run, and nothing was written."""


@dataclass(frozen=True)
class Capture:
    origin: str
    surface: str
    payload: object
    note: str = ""


def write_fixture(capture: Capture, directory: Path, known: set[str] | None = None) -> Path:
    """Scrub, verify, then write. Refuses rather than writing anything unsafe.

    Raises:
        CaptureRefused: if the secret list is empty (nothing to check against),
            or if a known secret survives scrubbing. Both refuse *before* any
            file is created.
    """
    known = known_secret_values() if known is None else known

    if not known:
        raise CaptureRefused(
            "no known secret values could be loaded, so scrubbing cannot be "
            "verified. Refusing to write: an unverifiable capture of live "
            "production data is not the same as a clean one."
        )

    scrubbed = scrub(capture.payload, known=known)

    if contains_known_secret(scrubbed, known=known):
        raise CaptureRefused(
            f"a known secret survived scrubbing in the {capture.origin} capture. "
            f"Refusing to write. (The value is deliberately not named here — "
            f"reporting it would leak it into this error and any log carrying it.)"
        )

    directory.mkdir(parents=True, exist_ok=True)
    path = directory / f"{capture.origin}.json"
    path.write_text(
        json.dumps(
            {
                "origin": capture.origin,
                "surface": capture.surface,
                "note": capture.note,
                "scrubbed": True,
                "payload": scrubbed,
            },
            indent=2,
            sort_keys=True,
            ensure_ascii=False,
            default=str,
        )
        + "\n"
    )
    return path
