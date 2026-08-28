"""Scrubbing secrets out of captured fixtures, before they ever reach disk.

**Order matters and is the whole design.** Capture reads a source, scrubs in
memory, and only then writes. There is deliberately no write-then-scrub path,
because that leaves a window in which an unscrubbed fixture exists on disk —
and a file that existed for four seconds is a file that a backup, an editor
swap file, or an interrupted run can preserve indefinitely.

Two independent mechanisms, because either alone fails a real case:

* **By key name.** A field called `token` is a secret whatever it contains.
  Catches secrets whose shape we have never seen.
* **By known value.** The actual live secret strings on this machine are read
  at scrub time and matched exactly. This catches a secret sitting in a field
  nobody thought to name suspiciously — the case pattern-matching misses.

Deliberately NOT scrubbed: 64-hex strings. Buzz event ids and Janus binding
digests are 64-hex and are load-bearing fixture content; blanket-scrubbing hex
would destroy the fixtures' value to protect against a shape that the two
mechanisms above already cover. Over-scrubbing has a real cost and it is not
free safety.

The known-value list is read into memory and never written anywhere, including
into error messages: a scrubber that reports *which* secret it found has
leaked it into a log.
"""

from __future__ import annotations

import re
from pathlib import Path

REDACTED = "[REDACTED]"

#: Key-name words meaning "secret", matched as WHOLE underscore-separated
#: parts rather than as substrings.
#:
#: Substring matching was the first version and it destroyed a real fixture:
#: `prompt_tokens` and `completion_tokens` contain "token", so Vulcan's entire
#: usage corpus came back with 155 redactions where the token COUNTS should
#: be. A usage reader cannot be built from that, and the failure is silent --
#: the fixture looks scrubbed and careful, and is useless.
#:
#: This is the over-scrubbing cost the module docstring warns about, shipped
#: by the module that warns about it. Over-scrubbing is not free safety: it
#: destroys what the corpus exists to preserve, and unlike under-scrubbing
#: nothing downstream ever complains.
SECRET_KEY_WORDS = frozenset(
    {
        "token",
        "secret",
        "password",
        "passwd",
        "authorization",
        "bearer",
        "cookie",
        "credential",
        "credentials",
    }
)

#: Whole key names that are secrets. Separate because their PARTS are too
#: common to match on: "key" alone would redact `sort_key`, "api" would redact
#: `api_version`.
SECRET_KEY_NAMES = frozenset(
    {
        "api_key",
        "apikey",
        "api_keys",
        "private_key",
        "privatekey",
        "secret_key",
        "access_key",
        "auth_header",
        "session_id",
        "sessionid",
        "auth",
        "authentication",
    }
)


def _is_secret_key(key: str) -> bool:
    """Whether a field name means its value is a secret.

    Parts are compared exactly, so `tokens` (a count) does not match `token`
    (a credential) -- the whole difference between a usable fixture and 155
    redactions.
    """
    lowered = str(key).lower()
    if lowered in SECRET_KEY_NAMES:
        return True
    return any(part in SECRET_KEY_WORDS for part in lowered.split("_"))


#: Value shapes that are secrets regardless of the field they sit in.
SECRET_VALUE_PATTERNS = (
    re.compile(r"\bBearer\s+\S+", re.I),
    re.compile(r"\bsk-[A-Za-z0-9_-]{16,}"),  # OpenAI-style
    re.compile(r"\bghp_[A-Za-z0-9]{20,}"),  # GitHub PAT
    re.compile(r"\bgithub_pat_[A-Za-z0-9_]{20,}"),
    re.compile(r"\bxai-[A-Za-z0-9]{16,}"),
    re.compile(r"\bsk-ant-[A-Za-z0-9_-]{16,}"),  # Anthropic
    re.compile(r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]+"),  # JWT
    re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----", re.S),
    re.compile(r"://[^/\s:@]+:[^/\s@]+@"),  # credentials in a URL
)

#: Files whose values are live secrets on this machine. Read at scrub time so
#: the actual strings are matched, not merely their shapes.
KNOWN_SECRET_SOURCES = (Path.home() / ".config" / "beacon" / "fixture-secrets.env",)


def known_secret_values(sources=KNOWN_SECRET_SOURCES) -> set[str]:
    """Live secret values on this machine, for exact matching.

    Held in memory only. Never written, never logged, never included in an
    exception message — a scrubber that names the secret it caught has leaked
    it somewhere new.
    """
    values: set[str] = set()
    for path in sources:
        try:
            text = path.read_text()
        except OSError:
            continue
        for line in text.splitlines():
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            _, sep, value = line.partition("=")
            candidate = (value if sep else line).strip().strip("\"'")
            # Short values produce false positives against ordinary content.
            if len(candidate) >= 12:
                values.add(candidate)
    return values


def _scrub_text(text: str, known: set[str]) -> str:
    for secret in known:
        if secret in text:
            text = text.replace(secret, REDACTED)
    for pattern in SECRET_VALUE_PATTERNS:
        text = pattern.sub(REDACTED, text)
    return text


def scrub(value, known: set[str] | None = None):
    """Return `value` with secrets replaced, recursively. Never mutates input."""
    if known is None:
        known = known_secret_values()

    if isinstance(value, dict):
        out = {}
        for key, inner in value.items():
            if _is_secret_key(key):
                # Redact by key regardless of the value's shape, but keep the
                # key: a fixture that silently drops fields no longer describes
                # the source's actual response shape.
                out[key] = REDACTED if inner is not None else None
            else:
                out[key] = scrub(inner, known)
        return out
    if isinstance(value, list):
        return [scrub(v, known) for v in value]
    if isinstance(value, str):
        return _scrub_text(value, known)
    return value


def contains_known_secret(value, known: set[str] | None = None) -> bool:
    """Whether any live secret survives anywhere in `value`.

    The verification half. Returns a boolean and never the offending string.
    """
    if known is None:
        known = known_secret_values()
    if not known:
        return False
    import json

    blob = json.dumps(value, default=str)
    return any(secret in blob for secret in known)
