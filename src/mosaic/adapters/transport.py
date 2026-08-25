"""Read-only transports.

Mosaic reads six modules it does not own. "Read-only" here is a property of the
code, not a promise in a README: this module exposes exactly one HTTP verb and
one subprocess entry point, and the subprocess entry point refuses any verb a
caller has not named in an allowlist up front.

If you find yourself wanting to add a write here, that is the design working.
Mosaic composes; the owning module decides.
"""

from __future__ import annotations

import json
import subprocess
import urllib.error
import urllib.request
from dataclasses import dataclass

DEFAULT_TIMEOUT = 5.0


class Unreachable(Exception):
    """A module did not answer.

    Always carries the address that was actually tried. "Connection refused"
    without an address is the least useful error message available, and the
    fleet's modules disagree about whether they live on loopback or the tailnet
    -- so the address is the first thing anyone debugging this needs.
    """

    def __init__(self, module: str, address: str, reason: str) -> None:
        self.module = module
        self.address = address
        self.reason = reason
        super().__init__(f"{module} did not answer at {address}: {reason}")


@dataclass(frozen=True)
class ReadOnlyHTTP:
    """GET, and nothing else.

    The method is not a parameter. There is no code path through this class that
    sends anything other than a GET, which is what makes the read-only claim
    checkable by a test rather than by review.
    """

    module: str
    base_url: str
    token: str | None = None
    timeout: float = DEFAULT_TIMEOUT

    def get_json(self, path: str) -> object:
        url = self.base_url.rstrip("/") + "/" + path.lstrip("/")
        request = urllib.request.Request(url, method="GET")
        request.add_header("Accept", "application/json")
        if self.token:
            request.add_header("Authorization", f"Bearer {self.token}")
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                body = response.read()
        except urllib.error.HTTPError as exc:
            raise Unreachable(self.module, url, f"HTTP {exc.code}") from exc
        except urllib.error.URLError as exc:
            raise Unreachable(self.module, url, str(exc.reason)) from exc
        except TimeoutError as exc:
            raise Unreachable(self.module, url, f"timed out after {self.timeout}s") from exc
        try:
            return json.loads(body)
        except json.JSONDecodeError as exc:
            raise Unreachable(self.module, url, f"answered with non-JSON: {exc}") from exc


@dataclass(frozen=True)
class ReadOnlyCommand:
    """A module read through its own CLI, restricted to verbs declared up front.

    Two modules have no HTTP read surface worth using: Janus is a local CLI over
    SQLite, and Buzz's conversation reads go through `buzz`. Reading their
    databases directly would be reaching into another module's live state; going
    through the owner's own interface is not.

    `read_verbs` is the whole safety story. `janus` can rule on a gate and
    withdraw one; this class will refuse to run either, because neither appears
    in the allowlist an adapter declares.
    """

    module: str
    executable: str
    read_verbs: frozenset[str]
    timeout: float = DEFAULT_TIMEOUT

    def run_json(self, argv: list[str]) -> object:
        if not argv:
            raise ValueError(f"{self.module}: no verb given")
        verb = argv[0]
        if verb not in self.read_verbs:
            raise PermissionError(
                f"{self.module}: '{verb}' is not a declared read verb "
                f"(allowed: {', '.join(sorted(self.read_verbs))})"
            )
        command = [self.executable, *argv]
        printable = " ".join(command)
        try:
            done = subprocess.run(
                command,
                capture_output=True,
                text=True,
                timeout=self.timeout,
                check=False,
                shell=False,
            )
        except FileNotFoundError as exc:
            raise Unreachable(self.module, self.executable, "not on PATH") from exc
        except subprocess.TimeoutExpired as exc:
            raise Unreachable(self.module, printable, f"timed out after {self.timeout}s") from exc
        if done.returncode != 0:
            detail = (done.stderr or done.stdout).strip().splitlines()
            reason = detail[0] if detail else f"exit {done.returncode}"
            raise Unreachable(self.module, printable, reason)
        try:
            return json.loads(done.stdout)
        except json.JSONDecodeError as exc:
            raise Unreachable(self.module, printable, f"answered with non-JSON: {exc}") from exc
