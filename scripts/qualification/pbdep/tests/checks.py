"""Source-only controls for accepted B02/B03/B04 seams; all remain UNRUN."""

import hashlib
import json
import ast
import errno
import inspect
import os
import stat
import tempfile
import unittest
import sys
import importlib
import contextvars
import posixpath
import base64
import zlib
from copy import deepcopy
from pathlib import Path, PurePosixPath
from types import SimpleNamespace
from unittest.mock import patch

OWNER_IMPORTS_READY = False
D = C = R = S = W = QW = P = E = None
_OWNER_PACKAGE = None
_BOUND_ROOTS = None
_FIXTURE_ROOT = None
_TMP_ROOT = None
_TEMPORARY_ROOT = None
_TEMPORARY_ROOTS = ()
_ACTIVE_SELECTOR = contextvars.ContextVar("pbdep_active_selector", default=None)
_ACTIVE_LINK_PAIR = contextvars.ContextVar("pbdep_active_link_pair", default=None)
_AUDIT_BOUNDARY = None


NEW_CONTROL_IDS = (
    "B05-NE01",
    "B05-NE02",
    "B05-NE03",
    "B05-NE04",
    "B05-NE05",
    "B05-ENV01",
    "B05-ENV02",
    "B05-ENV03",
    "B05-ENV04",
    "B05-ENV05",
    "B05-ENV06",
    "B05-ENV07",
    "B05-EX01",
    "B05-EX02",
    "B05-NE06",
    "B05-NE07",
    "B05-ADM01",
    "B05-ADM02",
    "B05-QUAL01",
    "B05-PHASE01",
    "B05-HIST01",
    "B05-D-ADAPT01",
    "B05-D-RAW01",
    "B05-D-FS01",
    "B05-D-PREIMAGE01",
    "B05-D-COMPOSE01",
    "B05-D-LEGACY01",
    "B05-D-RD-INV01",
    "B05-ENTRY01",
    "B05-ENTRY02",
    "B05-PRIOR01",
    "B05-SOURCE01",
    "B05-GATE01",
    "B05-ROSTER01",
    "B05-NAMES01",
    "B05-HIST02",
    "B05-QW01",
    "B05-ENTRY03",
    "B05-ENV08",
    "B05-ENV09",
    "B05-ENV10",
    "B05-A2-01",
    "B05-Q01",
    "B05-Q02",
    "B05-Q03",
    "B05-Q04",
    "B05-Q05",
    "B05-B03-START01",
    "B05-B03-ROSTER01",
    "B05-D19-CANCEL01",
    "B05-D22-PREIMAGE01",
    "B05-D23-CANCEL01",
    "B05-D24-INODE01",
    "B05-D31-CANONICAL01",
    "B05-D04-MIGRATION01",
    "B05-A2-ARTIFACT01",
    "B05-B02-API01",
    "B05-B02-RAW01",
    "B05-Q06",
    "B05-PRIOR02",
    "B05-PRIOR03",
    "B05-QUAL02",
    "B05-B03-ROSTER02",
    "B05-B03-GATE02",
    "B05-B03-RAW01",
    "B05-PRIOR04",
    "B05-PRIOR-CHRON01",
    "B05-A2-JOINS02",
    "B05-Q-AGG-PRE01",
    "B05-B03-CAPTURE03",
    "B05-B03-LIFECYCLE01",
    "B05-PRIOR-CAPTURE01",
    "B05-A2-PROVENANCE01",
    "B05-A2-CLASSIFY01",
    "B05-B03-RAW-RUNTIME01",
)

_FIXED_ENVIRONMENT = {
    "CARGO_BUILD_JOBS": "2",
    "CARGO_NET_OFFLINE": "true",
    "CARGO_TERM_COLOR": "never",
    "LANG": "C.UTF-8",
    "LC_ALL": "C",
    "PYTHONDONTWRITEBYTECODE": "1",
    "PYTHONNOUSERSITE": "1",
    "RUST_TEST_THREADS": "1",
}
_SUPERVISE_SOURCE_SHA256 = "add620d26bcb21b522e113904cea585c40009cce212100384aebe1ae70c64978"
_SUPERVISOR_SCHEMA_MAP_SHA256 = "71dfe7b4ddcd5737509fdd4168843c3cc658bdf7cacf6f2e4b0b164eac81d814"

# Independent source-contract projection for the direct synthetic A2 classifier.
_A2_PREFIX = "artifact::artifact_identity_contract::"
_A2_EVIDENCE_CONTRACT = "phase-b.original-a2-evidence.v1"
_A2_BACKTRACE_NOTE = "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace"
_A2_CASE_ORACLE = (
    ("different-accountability", "equal_stamp_different_accountability_records_twice",
     (1788000000000, 1788000000000), 2, None, None),
    ("equal", "equal_stamp_identical_inputs_require_two_records",
     (1788000000000, 1788000000000), 1,
     "PHASE_A_RED_EQUAL_STAMP_DISTINCTNESS", 372),
    ("increasing", "increasing_stamp_identical_inputs_record_twice",
     (1788000000000, 1788000000001), 2, None, None),
    ("nonadjacent", "nonadjacent_repeated_stamp_requires_three_records",
     (1788000000000, 1788000000001, 1788000000000), 2,
     "PHASE_A_RED_NONADJACENT_DISTINCTNESS", 428),
    ("reopen", "verified_read_reopen_preserves_observed_cardinality",
     (1788000000000, 1788000000001, 1788000000000), 2, None, None),
)
_A2_ROSTER_PINS = (
    "80176813d5049c5db9ec2750ea6ec343603cfbc9d8ded6a3843fc2b74b28dc27",
    "464efb7b12934af12c91a537130a0303ed1a1bda73595a0ff1e2c0765e0caf9f",
    "be9e58b9677ab50ba2c954c3435f9d0f0d45d37674a01be5e929d6fe1d169641",
)
_A2_REQUIRED_SOURCE_NAMES = (
    "Cargo.lock", "crates/beacond/src/artifact.rs",
    "crates/beacond/src/artifact_identity_contract.rs",
    "crates/beacond/src/pilot_admission.rs", "crates/beacond/src/pilot_authority.rs",
    "crates/beacond/src/pilot_store.rs",
)

# Exact retained identifier-only listing preimages pinned by accepted C.ROSTER_PINS.
# They are inputs to the listing consumer control, not a current roster observation.
_A2_ROSTER_PREIMAGE_B64 = {
    'full': (
        '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'
    ),
    'pilot': (
        'eNqtWNt26ygMfT9fNCs/wyJYtmkwMFySZr5+tgBf0tPWTtqHrtW6ZiNtSVuSvTYuCdlNOkbt7OmU'
        'KKZ4Ovny/OxciilIL9Qow0AiOGOiyF44q0hYPjpIbYW2yYmG8cd/i6nylI1M+orzdBO6E9J2Imal'
        'KEYX8PBKQQSKlESOcqBdvBDIpnq7HcSQZejEmXoXSJxzNwCnl8acpbrsQHXZG61kIqGM1JOQfYIp'
        '3kVdzFXGRRI6Fr/ZQNvJs9mzj969Dne83+dI4C7iFyNuLlxgXRIXIh9BdLYdgYbkwO3Z6AEM7VJZ'
        'XVXSpwxflZu8IT7GNmrgeVgIag6hkFRjvT4myZGm6EklAOEH9Dl13CLQHal4jMhuDBGuF9lerLvZ'
        'RmZ5/xDmRDLCy67mxCsetmxT0nL8zncvYxQuI8GRgJw5KsP/7n4IzI/3iFQxP2Q90ITy4cuTngi5'
        'okzukCUoEGReCQcnWrhSPIgHb0KCHVdpdAeUCIfVCN4mB7ed1UqQd2o8CpcnjmSClVEEeXskjKtk'
        'B8jSIFv1cEndGCeNZIWXKIXZOCOz3bVpA+XUBVZBKCIdPhQc4h2V84ePIBIqaJ+gStmCC2eu1B0+'
        'HDTkA+qW0+iCTvfDBxcVYmHsdEzaqsSpGyHCQDyWpwteEaDjdg9kKRyp9eXErLxPe2rxFP5UkT56'
        'yMsi98jwJkkieuf648c57TgNj54I9G/GP6AYiZ60lUXUWdz3ylln6DnXsm0R+G832puexlVZy0+g'
        '5SZD0754VUFA6G7oDEtr4w57Kx1p0z2bIu0BsrM+B7/psH0gbuJ7AoOb+akomtCO1iCjbNFpqg0s'
        'Y0fzszbANjqsCj0LcSnL1tWf69dz76N3nRapfmiWDM3a/0koZuNPp+oUImDE4yTkpnOJSn1BKkS4'
        'VkjRIiYnUNXbx8nm5SvmylfO9nrIVTZ+9Z40IguYK10HiPI+ipdnE77rt6F9PhfxfQ1781y5kYpQ'
        'jRBwh1T5EdrnRCtDMszl9yPcjjbZ8iUgo4grmHdhpVNaHiowCW0HvfZSyedCL1w/jNgGeh/cG/4D'
        '4hCR+xyfWbjKDIERQMclwY/BD0FO00rbpt2yCEVoRxqr4b280Mel5jns1uAcBpWQbQGVOD1OGBhV'
        'edwbd3sNOwGxJSo2NG04jLPA8EVjXlYnTkD6uJZcmeOc+n+wwg2Be0hrU0ZPRZ6aC8CXUWn90dzD'
        'oLzZhNYBXdkIij5sd6cdrEDK8U73e9Y1xBdN6/V73bhKn6t9YS4djJf2IQ6fy/l3sHNRtkmHQRBh'
        'G2WVPN7BN8vsd0h+6VnOdNX/mtozf5USWB1AxAFAkGO+mOQjmbIvckt8FQn1vAx1Da6D2J15EnoC'
        'dp3x5u8A201l9/jGitcwOLfinAv8p1XasEEo8FyErcs1lmUEyP4w9vqZhD8gRC/VZqwpkW1bbueW'
        'rxT8GeVINic5eVSDciZP7PWdl8dltge0C36UtqXRN4BvkYeAv7vB3pG62Qd6K4G32az9id5RAcUI'
        'kIlfO61KU+X58Dtc8MvEL1y1klr1+bZL/ZcQ3sj1s84yEe/BrUBrLNcyLVt7Gxnq54UX4GAaTzFx'
        '3G6fFSmu3bW48ed/hD8Uqg=='
    ),
    'overlay': (
        'eNqV0MEOwjAIBuC7j7SXIYxSxUzaUTrj20vnPJq4S0MD+fIDmktG8mnCowJJrC7+Airqtvd47bhA'
        'c3xUSJIzW4wAEpWujrMsY9yYiqUG/hTiC56GP22Kv2jt3gJcuxgHWL74n6woGWMTvf62h3cqqxbF'
        'dEcaqxtXRud08EfS2P1mzCfDbmySJayIPJ5SWaGGxrYFWea9SEBoSRTHrS9v22awnw=='
    ),
}
_A2_ROSTER_PREIMAGE_SHA256 = {
    'full': '80176813d5049c5db9ec2750ea6ec343603cfbc9d8ded6a3843fc2b74b28dc27',
    'pilot': '464efb7b12934af12c91a537130a0303ed1a1bda73595a0ff1e2c0765e0caf9f',
    'overlay': 'be9e58b9677ab50ba2c954c3435f9d0f0d45d37674a01be5e929d6fe1d169641',
}

# Exact source inputs accepted with B04-R1, compressed only for compact
# in-file fixture storage. These bytes are not producer results.
_Q_FIXED_SOURCE_B64 = {
    'expected_rows': (
        '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'
    ),
    'legacy_id_index': (
        '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'
    ),
    'physical_predicates': (
        'eNq1WG1v2zYQ/p5foeqTvZmBnaAvCZoCxVasxYJ22Ir1g2cItETbXGRSJenYbpf/vjtSkkmJdroC'
        '86dGPN499/bcsQVbJFm12mue0zLLVyy/0wNB12yUKKY3pRklbFex3LAi00ZW8Jluh9dnCfwyxT5v'
        'uGIDs6/YwIkPE66TgudwD7UkPybptWI545UhKJYOw6vu1vmSmUG6XcmSZQXTORMFFSbTzGRsx006'
        'TG5ukvS33z/8+eZ99uant+9uf04TKorEv17ximVMLlKL4aPasJG11f5aQI2/ZCPuhNwKIiQRbGfI'
        'gu/MRp0EqTdzxWjFVHbPFF9wVhzs9SBtKTeZrAyXQtdOfHr/4e3r97/8k2WfXt/epl6YSi4YXTKy'
        'lsVpBJAGp6yTmVYT/kkApZaiVlQyWjCV3ATo3Mes4kUtpfK4hGIQFZFHcNnMOykbBS6MDUJt71Uy'
        'tn+6AskbEd9pK0h4wYThZl/rX0iVQGjLAqSTFgaWQraWQhopeA6hS1m+4mURfmvKoCNZ0kpjpFgu'
        'RQHJuG5r456WkLrAcWt72EqE7lr5oUVmfVmUkkLdo59ralbnXC+44KYVxANn49VNMvZ8N3zNSAr/'
        '9Mx1Mj7tuzhDNc1pPDIz3whTay6g1qUCyWj+gozLZTbfGwYRChLaGmzPZ1iBJRMDJAQ/o3JJNP9y'
        'soRRi17Ri6fPXCGvqF6VfH7uvlmN5yu2K/iSaTPoanefH3Wl4Dqn4HSRPeZUTHLWzVYtFGWxnu01'
        'M7SghmYSWAIKZFtbnktZBqb7gjauloItXzQC49QPQ/OVHPRbRLncCKM7XczuobkydxRF7o5a6rYA'
        'gXvb74Dja2pZr8Be0nwJBaVtWykllU4fPGj2ErS0LWrtNXRODVtKtXc9fUqd15xHQ9xoc3EtuTYH'
        'pnG4p43ILEh697Cb5wa/l2f88YUTeRLk5DqYMC3Yno2mVVzWPWAvb5LJxYuIeeBcpMSWwV1okkPr'
        '17GanfnosGIqqQ3E+45YiTQSy0yD7MApGCXTrzB5GUy968NdHL2bClNiwwAnH/54Y9U9zPw6ZDua'
        'm9pQjbP0scgN+vIdQOzFrKIKguDD+JXt5xLa8B0GSW0q8x8BQfTkRuWsh6mdpfYgwYpvwTSXtI/k'
        'o6LQe5qWLi44eYD90VCWl1RrlCnAydxApglwh6grXEg4mlw+9MMxwXLAOnERaarDli0tyzpcSm5H'
        'HbhD22BwgL1VX/ZrCvHYXaUTGc0iSWkq7SaZzvyOaSsOj/yO2YgGTav+LAipVQAhvWzr1FJAAW0E'
        'G6clgTtelnC//TS06JLJWayvGvKYBUuQPQw6aV2V0EWkUhJyp4kVqH33ayLWzr057Jus49IcNQQ2'
        'i8QsOPOD9oUpSSwzk5xW0Yw4k17LNxj6KLE6HEPKbbMNF91pJ7dT+3mGe1m4Gje/w7LW6LGrhVv9'
        'Wm1esTlIw2Dbxy8ERnnAn2AY2WvqoeipiTiG3YCjCK8Ph/18W8R4eG7/bHdREJwEJco/bxipV+w0'
        'strh/j+w4A4eRyBi4XguWF5vLaq8v9niNfK35MIzilo1ZN6p7RTRkZFis4KXjqUXzxpYoBdj8kiO'
        'D9qc8ajC+sjqHEyejpIrP9nu1Itr0Fy2yzvjaBQOhVGPkmPDHx4mqIugfXLlVmoq9oNYJg613kKH'
        'MwIqgmR2oh7SJaNiU5EmKpExAnvi5UV6HCl0egPWU9wsa8Q9av3dAu69TMZHcv//udoi6vgaHQz4'
        'PITVdPI8YE8hBXLZ5HnqKHscjmMO/HZ4OAaVAS8SeJmoPRaBPwsOGkfB/nuCnb+Lgr25Fbj/7UPq'
        'BKLYu+2xZ9tZtFe/SWPk1TvrkxGoIyBEQITY/23pViKmCmlaSwVheYQPw6EU4dRaG9TMlFyNMPbj'
        'WVA73YC6CpqOa1m/kg7/LWQx68hqHtMYdtQ9vG74Gvw7sH1kFHUGfsP4fhywza6G8W6tjTwJuvRr'
        '2Kan+rM/YpzGB/sOsxT80HngE7grFfhtVkwQjEFkzTnOWXSLAvN0lyY/JJcXFvAY+Oj0+xiW0xeT'
        'Z35DVYot+M5WWSm1foRNTio/3qqe5pgXg3nLK3+JNAhAyzeuzOZp8KQuOUQSZ5lcPvq8b0ZWVjF6'
        'h1s5LJrePD472sPxi+7JP+6w4+kpWe/Hz8PVq5bJYR+c0/wOHPkXGCvy2w=='
    ),
}
_Q_FIXED_SOURCE_SHA256 = {
    'expected_rows': 'a710ec4396e9480583a6cf50825d64761a7f101b580cb081ca01a253c7e803c1',
    'legacy_id_index': '4a3dd5442300becba6164b30f8bd61eba20a6470abcf32099be5a8d4e0fc5304',
    'physical_predicates': '4f3b2ddadb9b8191aed2e8b876b5006204efaba8f3944ae8ab7cf8d7ea74db3e',
}


def _q_fixed_source_bytes(name):
    raw = zlib.decompress(base64.b64decode("".join(_Q_FIXED_SOURCE_B64[name]),
                                         validate=True))
    if hashlib.sha256(raw).hexdigest() != _Q_FIXED_SOURCE_SHA256[name]:
        raise AssertionError("independent Q source fixture pin drift: " + name)
    return raw

def _a2_roster_preimage(name):
    raw = zlib.decompress(base64.b64decode(
        "".join(_A2_ROSTER_PREIMAGE_B64[name]), validate=True))
    if hashlib.sha256(raw).hexdigest() != _A2_ROSTER_PREIMAGE_SHA256[name]:
        raise AssertionError("retained A2 roster fixture pin drift: " + name)
    return raw

_ORDINARY_LOG = b"ordinary\n"
_REQUEST_PIN_HASHES = {
    "grant": "3492ad65d05a973fef8c825521eeb41ae64625a672a8aeeeabc696e16d62a020",
    "reference": "52367a6622b19f08825e915fad80c542ad4f4c34dbcebad9f5007994b3e39208",
    "reservation": "9efa077f044e7a779c655f8d8e8760cacd16ce2347b6aa10e45b18fb5325fdd6",
    "prior": "e5bade0e979cb8cf6e53303dae696c6631a5c82ce60386d7a602f85fb249fcc0",
}


def _request_pins():
    if type(_FIXTURE_ROOT) is not str:
        raise RuntimeError("synthetic fixture roots are not bound")
    return {name: {"path": _FIXTURE_ROOT + "/pins/" + name + ".json",
                   "sha256": digest}
            for name, digest in _REQUEST_PIN_HASHES.items()}


def _canonical_absolute(value):
    if type(value) is not str or not value.startswith("/"):
        raise RuntimeError("runner path must be an absolute string")
    path = PurePosixPath(value)
    if str(path) != value or any(part in (".", "..") for part in path.parts):
        raise RuntimeError("runner path is not canonical")
    return path


def _root_record(value):
    if type(value) is not dict or set(value) != {"path", "dev", "ino", "uid", "mode"}:
        raise RuntimeError("runner root record schema drift")
    path = _canonical_absolute(value["path"])
    for key in ("dev", "ino", "uid", "mode"):
        item = value[key]
        if type(item) is not str or not item.isdecimal():
            raise RuntimeError("runner root identity type drift")
    if value["mode"] != "448":
        raise RuntimeError("runner root must be private mode 0700")
    return {**value, "path": str(path)}


def _parent_identity_record(value):
    if type(value) is not dict or set(value) != {"path", "dev", "ino", "uid", "mode"}:
        raise RuntimeError("runner parent identity schema drift")
    path = _canonical_absolute(value["path"])
    for key in ("dev", "ino", "uid", "mode"):
        item = value[key]
        if type(item) is not str or not item.isdecimal():
            raise RuntimeError("runner parent identity type drift")
    return {**value, "path": str(path)}


def _capture_parent_identity(path):
    path = Path(path)
    for component in reversed((path, *path.parents)):
        observed = component.lstat()
        if stat.S_ISLNK(observed.st_mode) or not stat.S_ISDIR(observed.st_mode):
            raise RunnerBoundaryError("runner root parent ancestry is not real directories")
    observed = path.lstat()
    return {"path": str(path), "dev": str(observed.st_dev),
        "ino": str(observed.st_ino), "uid": str(observed.st_uid),
        "mode": str(stat.S_IMODE(observed.st_mode))}


def _verify_parent_identity(record):
    path = Path(record["path"])
    observed = path.lstat()
    identity = {"path": str(path), "dev": str(observed.st_dev),
        "ino": str(observed.st_ino), "uid": str(observed.st_uid),
        "mode": str(stat.S_IMODE(observed.st_mode))}
    if (identity != record or stat.S_ISLNK(observed.st_mode)
            or not stat.S_ISDIR(observed.st_mode)):
        raise RuntimeError("runner parent identity drift")
    return path


def _bind_fixture_roots(receipt):
    """Bind only a future runner's exact root receipt; no path is guessed here."""
    global _BOUND_ROOTS, _FIXTURE_ROOT, _TMP_ROOT, _TEMPORARY_ROOT, _TEMPORARY_ROOTS
    if OWNER_IMPORTS_READY:
        raise RuntimeError("root binding must precede accepted owner imports")
    if type(receipt) is not dict or set(receipt) != {
            "schema_version", "container", "fixture_root", "ordinary_root", "temporary_roots",
            "container_parent", "temporary_parent"}:
        raise RuntimeError("runner root receipt schema drift")
    if receipt["schema_version"] != "pbdep.synthetic-root-binding.v1":
        raise RuntimeError("runner root receipt version drift")
    container = _root_record(receipt["container"])
    fixture = _root_record(receipt["fixture_root"])
    ordinary = _root_record(receipt["ordinary_root"])
    container_parent = _parent_identity_record(receipt["container_parent"])
    temporary_parent = _parent_identity_record(receipt["temporary_parent"])
    temporary = receipt["temporary_roots"]
    if type(temporary) is not list or len(temporary) != 4:
        raise RuntimeError("exactly four separately bound short roots are required")
    temporary = tuple(_root_record(item) for item in temporary)
    container_path = PurePosixPath(container["path"])
    fixture_path = PurePosixPath(fixture["path"])
    ordinary_path = PurePosixPath(ordinary["path"])
    if len(os.fsencode(container["path"])) <= 107:
        raise RuntimeError("ordinary fixture container is not the required long root")
    if PurePosixPath("/tmp") in container_path.parents:
        raise RuntimeError("long fixture container must be outside /tmp")
    if fixture_path.parent != container_path or ordinary_path.parent != container_path:
        raise RuntimeError("fixture and ordinary roots must be direct container children")
    if PurePosixPath(container_parent["path"]) != container_path.parent or \
            PurePosixPath(temporary_parent["path"]) != PurePosixPath("/tmp"):
        raise RuntimeError("runner parent identities do not join the named roots")
    _verify_parent_identity(container_parent)
    _verify_parent_identity(temporary_parent)
    roots = (container, fixture, ordinary, *temporary)
    root_paths = [PurePosixPath(item["path"]) for item in roots]
    for index, first in enumerate(root_paths):
        for second in root_paths[index + 1:]:
            if first == second or first in second.parents or second in first.parents:
                if not (first == container_path and second in (fixture_path, ordinary_path)):
                    raise RuntimeError("runner roots must be unique and non-ancestral")
    for item in temporary:
        path = PurePosixPath(item["path"])
        if path.parent != PurePosixPath("/tmp") or len(os.fsencode(item["path"])) > 9:
            raise RuntimeError("temporary resource roots must be direct /tmp children of at most 9 bytes")
    _BOUND_ROOTS = {"container": container, "fixture_root": fixture,
        "ordinary_root": ordinary, "temporary_roots": temporary,
        "container_parent": container_parent, "temporary_parent": temporary_parent}
    _FIXTURE_ROOT = fixture["path"]
    _TMP_ROOT = ordinary["path"]
    _TEMPORARY_ROOTS = tuple(item["path"] for item in temporary)
    _TEMPORARY_ROOT = _TEMPORARY_ROOTS[0]


def _verify_bound_root(record):
    """Future slot check for a runner-created 0700 directory identity."""
    path = Path(record["path"])
    for component in reversed((path, *path.parents)):
        observed = component.lstat()
        if stat.S_ISLNK(observed.st_mode) or not stat.S_ISDIR(observed.st_mode):
            raise RuntimeError("bound root ancestry contains a non-directory or symlink")
    observed = path.lstat()
    identity = {"path": str(path), "dev": str(observed.st_dev),
        "ino": str(observed.st_ino), "uid": str(observed.st_uid),
        "mode": str(stat.S_IMODE(observed.st_mode))}
    if (identity != record or not stat.S_ISDIR(observed.st_mode)
            or observed.st_uid != os.getuid()):
        raise RuntimeError("runner root identity drift")
    return path


def _validate_root_paths(spec):
    if type(spec) is not dict or set(spec) != {
            "schema_version", "container", "fixture_root", "ordinary_root", "temporary_roots"}:
        raise RunnerBoundaryError("runner root-path schema drift")
    if spec["schema_version"] != "pbdep.synthetic-root-paths.v1":
        raise RunnerBoundaryError("runner root-path version drift")
    paths = {name: _canonical_absolute(spec[name]) for name in
             ("container", "fixture_root", "ordinary_root")}
    temporary = spec["temporary_roots"]
    if type(temporary) is not list or len(temporary) != 4:
        raise RunnerBoundaryError("four explicit temporary root paths are required")
    temporary = tuple(_canonical_absolute(path) for path in temporary)
    container = paths["container"]
    if len(os.fsencode(str(container))) <= 107 or PurePosixPath("/tmp") in container.parents:
        raise RunnerBoundaryError("long fixture container must be outside /tmp")
    for name in ("fixture_root", "ordinary_root"):
        if paths[name].parent != container:
            raise RunnerBoundaryError("fixture roots must be direct container children")
    if any(path.parent != PurePosixPath("/tmp") or len(os.fsencode(str(path))) > 9
           or not path.name.isascii() or not path.name.isalnum()
           or not 1 <= len(path.name) <= 4 for path in temporary):
        raise RunnerBoundaryError("temporary roots must be direct /tmp children of at most 9 bytes")
    all_paths = (container, paths["fixture_root"], paths["ordinary_root"], *temporary)
    if len(set(all_paths)) != len(all_paths):
        raise RunnerBoundaryError("runner root paths must be unique")
    for index, first in enumerate(all_paths):
        for second in all_paths[index + 1:]:
            if first in second.parents or second in first.parents:
                if not (first == container and second in
                        (paths["fixture_root"], paths["ordinary_root"])):
                    raise RunnerBoundaryError("runner roots must be non-ancestral")
    return {"schema_version": "pbdep.synthetic-root-paths.v1",
        "container": str(container), "fixture_root": str(paths["fixture_root"]),
        "ordinary_root": str(paths["ordinary_root"]),
        "temporary_roots": [str(path) for path in temporary]}


def _create_runner_roots(spec, *, custody, profile_sha256):
    """Create-only roots with locally flushed custody before each next mkdir."""
    spec = _validate_root_paths(spec)
    paths = [spec["container"], spec["fixture_root"], spec["ordinary_root"],
             *spec["temporary_roots"]]
    parents = [str(Path(paths[0]).parent), str(Path(paths[3]).parent)]
    plan = {"paths": paths, "parents": parents}
    encoded = json.dumps(plan, sort_keys=True, ensure_ascii=True, allow_nan=False)
    if len(encoded.encode("ascii")) > _ROOT_CUSTODY_PLAN_MAX_BYTES:
        custody["creation_state"] = "PLAN_OVER_CAP_NO_EFFECT"
        custody["plan_sha256"] = hashlib.sha256(encoded.encode("ascii")).hexdigest()
        raise RunnerBoundaryError("root custody plan exceeds terminal retention reserve")
    if (type(profile_sha256) is not str or len(profile_sha256) != 64
            or any(char not in "0123456789abcdef" for char in profile_sha256)):
        raise RunnerBoundaryError("custody stream requires the validated raw profile pin")
    stream = custody["stream"]
    if stream["state"] != "NOT_STARTED":
        raise RunnerBoundaryError("custody stream cannot restart")
    stream["profile_sha256"] = profile_sha256
    stream["state"] = "READY"
    custody["paths"] = [{"path": path, "parent_check": "NOT_ATTEMPTED",
        "mkdir_intent": False, "mkdir_attempted": False, "mkdir_returned": False,
        "call_observation": "UNPROVEN", "creation": "NOT_ATTEMPTED",
        "identity": None, "verification": "UNPROVEN"} for path in paths]
    custody["parents"] = [{"path": path, "capture_intent": False,
        "capture_returned": False, "identity": None} for path in parents]
    custody["creation_state"] = "IN_PROGRESS"
    active = None
    try:
        _emit_custody_frame(stream, "ROOT_PLAN", None, plan)
        for index, parent in enumerate(custody["parents"]):
            active = parent
            custody["stage"] = "CAPTURE_PARENT"
            parent["capture_intent"] = True
            _emit_custody_frame(stream, "PARENT_INTENT", index, parent)
            parent["identity"] = _capture_parent_identity(parent["path"])
            parent["capture_returned"] = True
            _emit_custody_frame(stream, "PARENT_OBSERVED", index, parent)
        container_parent, temporary_parent = (parent["identity"]
                                              for parent in custody["parents"])
        receipt = {"schema_version": "pbdep.synthetic-root-binding.v1",
            "container": None, "fixture_root": None, "ordinary_root": None,
            "temporary_roots": [], "container_parent": container_parent,
            "temporary_parent": temporary_parent}
        names = ("container", "fixture_root", "ordinary_root")
        for index, entry in enumerate(custody["paths"]):
            active = entry
            path = Path(entry["path"])
            parent = path.parent
            custody["stage"] = "VERIFY_PARENT"
            entry["parent_check"] = "ATTEMPTED_UNPROVEN"
            if index == 0:
                if parent != Path(container_parent["path"]):
                    raise RunnerBoundaryError("container path changed after parent binding")
                _verify_parent_identity(container_parent)
            elif index < 3:
                if receipt["container"] is None or parent != Path(receipt["container"]["path"]):
                    raise RunnerBoundaryError("fixture parent does not join the created container")
                _verify_bound_root(receipt["container"])
            else:
                if parent != Path(temporary_parent["path"]):
                    raise RunnerBoundaryError("temporary root path changed after parent binding")
                _verify_parent_identity(temporary_parent)
            entry["parent_check"] = "VERIFIED"
            custody["stage"] = "MKDIR"
            entry["mkdir_intent"] = True
            entry["mkdir_attempted"] = None  # Intent cannot prove a call began.
            entry["creation"] = "INTENT_ONLY_EFFECT_UNKNOWN"
            _emit_custody_frame(stream, "MKDIR_INTENT", index, entry)
            custody["stage"] = "VERIFY_PARENT_AFTER_INTENT"
            if index == 0:
                _verify_parent_identity(container_parent)
            elif index < 3:
                _verify_bound_root(receipt["container"])
            else:
                _verify_parent_identity(temporary_parent)
            entry["parent_check"] = "VERIFIED_AFTER_INTENT_FLUSH"
            custody["stage"] = "MKDIR"
            # A retained intent does not prove this call began or had an effect.
            try:
                path.mkdir(mode=0o700, parents=False, exist_ok=False)
            except FileExistsError:
                entry["mkdir_attempted"] = True
                entry["call_observation"] = "RAISED"
                entry["creation"] = "COLLISION_NOT_ADOPTED"
                raise
            except BaseException:
                entry["call_observation"] = "RAISED_OR_INTERRUPTED_EFFECT_UNKNOWN"
                entry["creation"] = "ATTEMPT_OR_EFFECT_UNKNOWN"
                raise
            entry["mkdir_attempted"] = True
            entry["mkdir_returned"] = True
            entry["call_observation"] = "RETURNED"
            entry["creation"] = "CREATED_IDENTITY_UNPROVEN"
            _emit_custody_frame(stream, "MKDIR_RETURNED", index, entry)
            custody["stage"] = "LSTAT_CREATED_ROOT"
            observed = path.lstat()
            identity = {"path": str(path), "dev": str(observed.st_dev),
                "ino": str(observed.st_ino), "uid": str(observed.st_uid),
                "mode": str(stat.S_IMODE(observed.st_mode))}
            entry["identity"] = identity
            _emit_custody_frame(stream, "ROOT_IDENTITY_OBSERVED", index, entry)
            custody["stage"] = "VERIFY_CREATED_ROOT"
            if not stat.S_ISDIR(observed.st_mode) or identity["mode"] != "448" or \
                    identity["uid"] != str(os.getuid()):
                raise RunnerBoundaryError("new runner root failed private-directory checks")
            entry["verification"] = "VERIFIED_AT_CREATION"
            if index < 3:
                receipt[names[index]] = identity
            else:
                receipt["temporary_roots"].append(identity)
            _emit_custody_frame(stream, "ROOT_VERIFIED", index, entry)
        custody["creation_state"] = "COMPLETE_AT_CREATION"
        custody["stage"] = "COMPLETE"
        _emit_custody_frame(stream, "ROOT_COMPLETE", None, {
            "created_roots": 7, "verified_roots": 7,
            "root_receipt_sha256": hashlib.sha256(_runner_json(receipt).encode("ascii")).hexdigest()})
        return receipt
    except BaseException as exc:
        custody["creation_state"] = "STOP_PARTIAL_OR_UNPROVEN"
        custody["exception_type"] = type(exc).__name__[:128]
        if stream["state"] == "READY" and stream["pending_frame"] is None:
            _emit_custody_frame(stream, "ROOT_FAILURE", None, {
                "stage": custody["stage"], "exception_type": custody["exception_type"],
                "active_observation": active})
        # Never retry a lost frame, continue creation, adopt a collision, or clean up.
        raise


class RunnerBoundaryError(RuntimeError):
    """Fail-closed runner/profile error; not an accepted-owner refusal."""


_OWNER_SOURCE_RELATIVE = (
    "scripts/qualification/pbdep/discovery.py",
    "scripts/qualification/pbdep/contract.py",
    "scripts/qualification/pbdep/runtime.py",
    "scripts/qualification/pbdep/supervise.py",
    "scripts/qualification/pbdep/whole_program.py",
    "scripts/qualification/pbdep/policy.py",
    "scripts/qualification/pbdep/entry.py",
    "scripts/qualification/pbdep/qualify_whole_program.py",
    "scripts/qualification/pbdep/tests/checks.py",
)
_DATA_READ_RELATIVE = (
    "scripts/qualification/pbdep/tests/controls.json",
    "scripts/qualification/pbdep/tests/fixture-files.json",
    "scripts/qualification/pbdep/tests/replay-map.json",
)
_PROCESS_EVENTS = frozenset(("os.fork", "os.forkpty", "os.system", "os.posix_spawn",
    "os.spawn", "os.exec", "os.kill", "subprocess.Popen", "subprocess.run"))
_WRITE_EVENTS = frozenset(("os.mkdir", "os.remove", "os.rmdir", "os.rename", "os.replace",
    "os.symlink", "os.link", "os.chmod", "os.chown", "os.truncate"))
_BOUNDARY_EVENTS = frozenset(("open", "os.listdir", "os.scandir", "os.stat", "os.chmod",
    "os.chown", "os.truncate", "os.mkdir", "os.remove", "os.rmdir", "os.rename",
    "os.replace", "os.symlink", "os.link"))


def _lexical_path(value):
    try:
        path = os.fsdecode(os.fspath(value))
    except TypeError:
        return None
    if not path.startswith("/") or posixpath.normpath(path) != path:
        return None
    if any(part in (".", "..") for part in PurePosixPath(path).parts):
        return None
    return path


def _under(path, root):
    try:
        return posixpath.commonpath((path, root)) == root
    except ValueError:
        return False


def _audit_allowed(event, args, *, read_files, write_roots, ordinary_root,
                   slot, hardlink_allowance, active_selector=None, active_pair=None,
                   link_count=0, read_directories=frozenset(), identity_paths=frozenset()):
    """Pure policy used by the pre-import canaries and the installed tripwire."""
    if event in _PROCESS_EVENTS or event.startswith("subprocess.") or event.startswith("socket."):
        return False
    if event.startswith("os.") and event not in _BOUNDARY_EVENTS and event not in _WRITE_EVENTS \
            and event != "os.urandom":
        return False
    if event == "ctypes.dlopen":
        return False
    if event.startswith("ctypes."):
        return False
    if event == "import" and len(args) > 1 and args[1] is not None:
        return _lexical_path(args[1]) in read_files
    if event == "open":
        if not args:
            return False
        path = _lexical_path(args[0])
        if path is None:
            return False
        mode = args[1] if len(args) > 1 else "r"
        flags = args[2] if len(args) > 2 and type(args[2]) is int else 0
        writes = (type(mode) is str and any(char in mode for char in "wax+")) or bool(
            flags & (os.O_WRONLY | os.O_RDWR | os.O_CREAT | os.O_TRUNC | os.O_APPEND))
        return (any(_under(path, root) for root in write_roots) if writes else
            path in read_files or any(_under(path, root) for root in write_roots))
    if event in ("os.listdir", "os.scandir", "os.stat"):
        if not args:
            return False
        path = _lexical_path(args[0])
        if path is None:
            return False
        if event == "os.stat":
            return path in read_files or path in read_directories or path in identity_paths or any(
                _under(path, root) for root in write_roots)
        return path in read_directories or any(_under(path, root) for root in write_roots)
    if event in _WRITE_EVENTS:
        if event == "os.link":
            allowance = hardlink_allowance
            if (allowance is None or slot not in ("SQ3", "SQ4") or
                    active_selector != "test_b05_d_fs01_synthetic_dependency_component_controls" or
                    active_pair is None or len(args) < 2 or link_count >= 1):
                return False
            source, target = _lexical_path(args[0]), _lexical_path(args[1])
            if (source, target) != active_pair or args[2:4] not in (
                    (), (None, None), (-1, -1)):
                return False
            if not (_under(source, ordinary_root) and _under(target, ordinary_root)):
                return False
            return PurePosixPath(source).parent == PurePosixPath(target).parent and \
                PurePosixPath(source).name == "lib.rs" and \
                PurePosixPath(target).name == "same-bytes.rs" and \
                allowance == {"slot": slot, "control_id": "B05-D21-INODE01",
                              "max_calls": 1}
        paths = []
        if event == "os.symlink":
            if len(args) < 2 or type(args[0]) is not str or args[0].startswith("/") or \
                    ".." in PurePosixPath(args[0]).parts:
                return False
            paths.append(args[1])
        elif event in ("os.rename", "os.replace"):
            if len(args) < 2:
                return False
            paths.extend((args[0], args[1]))
        else:
            if not args:
                return False
            paths.append(args[0])
        if len(args) >= 4 and any(item is not None for item in args[-2:]):
            return False
        normalized = [_lexical_path(path) for path in paths]
        return all(path is not None and any(_under(path, root) for root in write_roots)
                   for path in normalized)
    return True


def _install_audit_guard(profile, read_files):
    global _AUDIT_BOUNDARY
    roots = _BOUND_ROOTS
    if roots is None:
        raise RunnerBoundaryError("root receipt must bind before guard installation")
    write_roots = (roots["fixture_root"]["path"], roots["ordinary_root"]["path"],
                   *(item["path"] for item in roots["temporary_roots"]))
    read_directories = frozenset(str(PurePosixPath(path).parent) for path in read_files)
    identity_paths = set()
    for root in (roots["container"], roots["fixture_root"], roots["ordinary_root"],
                 *roots["temporary_roots"]):
        current = PurePosixPath(root["path"])
        identity_paths.add(str(current))
        identity_paths.update(str(parent) for parent in current.parents)
    hardlink_allowance = profile["hardlink_allowance"]
    state = {"link_count": 0, "read_files": frozenset(read_files),
        "write_roots": tuple(write_roots), "ordinary_root": roots["ordinary_root"]["path"],
        "slot": profile["slot"], "hardlink_allowance": hardlink_allowance}
    state["read_directories"] = read_directories
    state["identity_paths"] = frozenset(identity_paths)
    _AUDIT_BOUNDARY = state

    def hook(event, args):
        active_pair = _ACTIVE_LINK_PAIR.get()
        allowed = _audit_allowed(event, args, read_files=state["read_files"],
            write_roots=state["write_roots"], ordinary_root=state["ordinary_root"],
            slot=state["slot"], hardlink_allowance=state["hardlink_allowance"],
            active_selector=_ACTIVE_SELECTOR.get(), active_pair=active_pair,
            link_count=state["link_count"], read_directories=state["read_directories"],
            identity_paths=state["identity_paths"])
        if not allowed:
            raise PermissionError("B05 synthetic runner audit boundary refused " + event)
        if event == "os.link":
            state["link_count"] += 1

    sys.addaudithook(hook)
    return state


def _guard_canaries(profile, read_files):
    roots = _BOUND_ROOTS
    if roots is None:
        raise RunnerBoundaryError("guard canaries require bound roots")
    write_roots = (roots["fixture_root"]["path"], roots["ordinary_root"]["path"],
                   *(item["path"] for item in roots["temporary_roots"]))
    ordinary = roots["ordinary_root"]["path"]
    canary = ordinary + "/canary.txt"
    try:
        with open(canary, "xb") as stream:
            stream.write(b"runner-canary\n")
        with open(canary, "rb") as stream:
            if stream.read() != b"runner-canary\n":
                raise RunnerBoundaryError("runner owned-file read canary mismatch")
        os.remove(canary)
    except OSError as exc:
        raise RunnerBoundaryError("runner owned-root read/write/cleanup canary failed") from exc

    def must_be_denied(label, action):
        try:
            action()
        except (PermissionError, OSError):
            return
        raise RunnerBoundaryError("runner denied-capability canary was accepted: " + label)

    # Submit the exact audit events without performing an external read, write,
    # process, network call, or link.  If the installed hook does not refuse an
    # event, must_be_denied raises before any corresponding operation occurs.
    must_be_denied("external read", lambda: sys.audit("open", "/etc/passwd", "rb", 0))
    must_be_denied("external write", lambda: sys.audit(
        "open", "/tmp/pb05-outside", "xb", os.O_WRONLY | os.O_CREAT | os.O_EXCL))
    must_be_denied("process", lambda: sys.audit("os.system", "true"))
    must_be_denied("fork", lambda: sys.audit("os.fork"))
    must_be_denied("subprocess", lambda: sys.audit(
        "subprocess.Popen", "/fixture/forbidden", (), None, None))
    must_be_denied("network", lambda: sys.audit("socket.connect", object(), ("127.0.0.1", 1)))
    must_be_denied("ctypes load", lambda: sys.audit("ctypes.dlopen", "/fixture/foreign.so"))
    must_be_denied("hard link", lambda: sys.audit("os.link", ordinary + "/lib.rs",
        ordinary + "/same-bytes.rs", None, None))

    receipt = {"schema_version": "pbdep.synthetic-root-binding.v1",
        "container": roots["container"], "fixture_root": roots["fixture_root"],
        "ordinary_root": roots["ordinary_root"],
        "temporary_roots": list(roots["temporary_roots"]),
        "container_parent": roots["container_parent"],
        "temporary_parent": roots["temporary_parent"]}

    def rejected_profile(candidate, expected_message, label):
        raw = (json.dumps(candidate, sort_keys=True, separators=(",", ":"),
                          allow_nan=False) + "\n").encode("utf-8")
        try:
            _validate_runner_profile(raw, hashlib.sha256(raw).hexdigest())
        except RunnerBoundaryError as exc:
            if str(exc) != expected_message:
                raise RunnerBoundaryError("runner selector canary reached the wrong refusal: " + label) from exc
        else:
            raise RunnerBoundaryError("runner selector canary accepted invalid profile: " + label)

    later = deepcopy(profile)
    later["slot"] = "SQ1"
    later.pop("root_paths", None)
    later["root_receipt"] = receipt
    later["hardlink_allowance"] = None
    duplicate = deepcopy(later)
    duplicate["selectors"] = [_EXPLICIT_SELECTORS[0], _EXPLICIT_SELECTORS[0]]
    rejected_profile(duplicate, "selector is duplicate or not explicitly listed", "duplicate")
    unknown = deepcopy(later)
    unknown["selectors"] = ["test_unknown_selector"]
    rejected_profile(unknown, "selector is duplicate or not explicitly listed", "unknown")
    empty = deepcopy(later)
    empty["selectors"] = []
    rejected_profile(empty, "slot selector inventory differs from its frozen source partition", "empty")
    partial = deepcopy(later)
    partial["selectors"] = list(_SLOT_SELECTORS["SQ1"][:1])
    rejected_profile(partial, "slot selector inventory differs from its frozen source partition", "partial")


def _create_d21_hardlink(source, destination):
    state = _AUDIT_BOUNDARY
    if (state is None or _ACTIVE_SELECTOR.get() !=
            "test_b05_d_fs01_synthetic_dependency_component_controls"):
        raise PermissionError("D21 hard-link allowance is not active for this selector")
    allowance = state["hardlink_allowance"]
    if allowance != {"slot": state["slot"], "control_id": "B05-D21-INODE01",
                     "max_calls": 1} or state["slot"] not in ("SQ3", "SQ4"):
        raise PermissionError("D21 hard-link allowance is not bound for this slot")
    pair = (_lexical_path(source), _lexical_path(destination))
    if (None in pair or not _under(pair[0], state["ordinary_root"]) or
            not _under(pair[1], state["ordinary_root"]) or
            PurePosixPath(pair[0]).parent != PurePosixPath(pair[1]).parent or
            PurePosixPath(pair[0]).name != "lib.rs" or
            PurePosixPath(pair[1]).name != "same-bytes.rs"):
        raise PermissionError("D21 hard-link paths do not match the finite fixture")
    token = _ACTIVE_LINK_PAIR.set(pair)
    try:
        os.link(source, destination)
    finally:
        _ACTIVE_LINK_PAIR.reset(token)


def _pin(name):
    return {
        "path": _FIXTURE_ROOT + "/pins/" + name + ".json",
        "sha256": hashlib.sha256(name.encode("ascii")).hexdigest(),
    }


def _environment():
    value = dict(_FIXED_ENVIRONMENT)
    value.update({
        "HOME": _FIXTURE_ROOT + "/home",
        "CARGO_HOME": _FIXTURE_ROOT + "/cargo-home",
        "RUSTUP_HOME": _FIXTURE_ROOT + "/rustup-home",
        "TMPDIR": _TEMPORARY_ROOT,
        "PATH": _FIXTURE_ROOT + "/tools",
        "CARGO_TARGET_DIR": _FIXTURE_ROOT + "/target",
    })
    return value


def _request():
    pins = {name: dict(value) for name, value in _request_pins().items()}
    environment = _environment()
    source_root = _FIXTURE_ROOT + "/source"
    target_root = _FIXTURE_ROOT + "/target"
    return {
        "schema_version": "pbdep.discovery-request.v1",
        "chain_id": "b" * 64,
        **pins,
        "subject_head": "a" * 40,
        "source_root": source_root,
        "target_root": target_root,
        "source_manifest": _pin("source-manifest"),
        "source_directory": _pin("source-directory"),
        "target_directory": _pin("target-directory"),
        "tmp": {"path": _TEMPORARY_ROOT},
        "state_directory": {"path": _FIXTURE_ROOT + "/state"},
        "environment": environment,
        "python": _FIXTURE_ROOT + "/tools/python3",
        "cargo": _FIXTURE_ROOT + "/tools/cargo",
        "rustfmt": _FIXTURE_ROOT + "/tools/rustfmt",
        "immutable_files": [],
        "selection_inputs": {"absent_paths": [], "aliases": []},
        "module_paths": [],
        "raw_cap": 1048576,
    }


def _supervisor_record():
    return {
        "leader_pid": 42,
        "leader_returncode": 0,
        "leader_exit_monotonic": 1.3,
        "whole_descendant_set_exit": "PROVEN_ECHILD",
        "echild_monotonic": 1.4,
        "pipe_eof": True,
        "pipe_eof_monotonic": 1.2,
        "subreaper_verified": True,
        "wait_options": "WNOHANG|__WALL",
        "elapsed_seconds": 0.5,
        "stop": None,
        "metadata_overflow": False,
        "event_counts": {"reaped": 1, "signals": 0, "errors": 0},
        "reaped": [{"pid": 42, "status": 0, "exit_code": 0, "at": 1.25}],
        "signals": [],
        "errors": [],
        "resource_peak_sampled": 17,
        "log_bytes": len(_ORDINARY_LOG),
        "discarded_log_bytes": 0,
        "log_sha256": "5851c12c0f86669211a8c56f41c43922327f5e6a035a9943c99a77e3ae5554eb",
    }


def _reservation():
    return {
        "schema_version": "pbdep.reservation.v1", "program_sha256": "a" * 64,
        "master_start_utc_seconds": 100, "master_expiry_utc_seconds": 40000,
        "exit_seconds": 60,
        "slots": {"1": 600, "2": 30, "3": 180, "4": 600, "5": 30,
                  "6": 180, "7": 600, "8": 30, "9": 180, "10": 180,
                  "11": 600, "12": 1800, "13": 90},
        "phases": {
            "qualification": {"slots": (), "prep": 0, "close": 0, "total": 600},
            "a2": {"slots": ("1", "2", "3"), "prep": 150, "close": 150, "total": 1290},
            "format": {"slots": ("13",), "prep": 120, "close": 60, "total": 330},
            "base": {"slots": ("4", "5", "6"), "prep": 150, "close": 150, "total": 1290},
            "candidate": {"slots": ("7", "8", "9"), "prep": 150, "close": 150, "total": 1290},
            "integration": {"slots": ("10",), "prep": 150, "close": 90, "total": 480},
            "consumers": {"slots": ("11",), "prep": 150, "close": 150, "total": 960},
            "gate": {"slots": ("12",), "prep": 750, "close": 150, "total": 2760},
        },
    }


def _history(sample_count=0, *, vanished=(), peak=0, types=None):
    entries = list(vanished)
    counts = {"entry-stat": 0, "directory-scan": 0}
    for item in entries:
        counts[item["pathname_class"]] += 1
    return {
        "sample_count": sample_count, "peak_total_bytes": peak,
        "peak_temporary_bytes": 0, "peak_capture_bytes": 0,
        "vanished_by_class": counts, "vanished": entries,
        "sampled_entry_types": types or {"directory": 0, "regular": 0,
            "symlink": 0, "socket": 0},
    }


def _runtime_dependency_fixture(root, token, canonical, content):
    fixture = QW._fixture("base")
    data = deepcopy(fixture["data"])
    digest = hashlib.sha256(content).hexdigest()
    data.update({"source_root": root, "target_root": root + "-target",
                 "dep_tokens": [token], "dep_bindings": [{
                     "raw_token": token, "canonical_relative": canonical,
                     "source_sha256": digest}]})
    manifest = {"schema_version": "phase-b.source-manifest.v1",
                "subject_head": data["subject_head"], "source_root": root,
                "files": {canonical: digest}}
    manifest_raw = QW._j(manifest)
    data["manifest_sha256"] = hashlib.sha256(manifest_raw).hexdigest()
    data["source_manifest"] = {"path": root + "/SOURCE-MANIFEST.json",
                               "sha256": data["manifest_sha256"]}
    binding_raw = QW._phase_binding_raw("base", data)
    reference = QW._reference("base", data, phase_binding_raw=binding_raw)
    return data, manifest, reference, binding_raw, digest


def _write_fixture_file(root, relative, content):
    path = Path(root) / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)
    return path


def _independent_observation_preimage(root, data, canonical, content):
    """Build the expected prior observation from OS metadata before the call."""
    root_path = Path(root)
    root_stat = root_path.lstat()
    directories = {}
    components = canonical.split("/")[:-1]
    current = root_path
    for component in components:
        current = current / component
        observed = current.lstat()
        directories["/".join(current.relative_to(root_path).parts)] = {
            "path": str(current), "dev": str(observed.st_dev), "ino": str(observed.st_ino),
            "mode": str(stat.S_IMODE(observed.st_mode)), "uid": str(observed.st_uid),
        }
    file_path = root_path / canonical
    file_stat = file_path.lstat()
    binding_sha = hashlib.sha256(QW._j(data["dep_bindings"])).hexdigest()
    return {
        "schema_version": "phase-b.dep-observation.v1",
        "source_root": {"path": str(root_path), "dev": str(root_stat.st_dev),
            "ino": str(root_stat.st_ino), "mode": str(stat.S_IMODE(root_stat.st_mode)),
            "uid": str(root_stat.st_uid)},
        "subject_head": data["subject_head"], "manifest_sha256": data["manifest_sha256"],
        "dep_binding_schema": data["dep_binding_schema"],
        "dep_bindings_sha256": binding_sha, "directories": directories,
        "files": {canonical: {"path": str(file_path), "sha256": hashlib.sha256(content).hexdigest(),
            "dev": str(file_stat.st_dev), "ino": str(file_stat.st_ino),
            "size": str(file_stat.st_size), "mtime_ns": str(file_stat.st_mtime_ns),
            "mode": str(stat.S_IMODE(file_stat.st_mode)), "uid": str(file_stat.st_uid),
            "nlink": str(file_stat.st_nlink)}},
    }


def _artifact_returned(workspace, resident, configuration, stamp, sequence):
    """Build one synthetic A2 artifact record from the documented byte grammar."""
    content = b"# Task Tracker outcome\n\nThe exporter landed.\n"
    canonical = (b"beacon.artifact.v1\0" + bytes.fromhex(workspace.replace("-", ""))
        + b"\x01" + (28).to_bytes(8, "big") + b"text/markdown; charset=utf-8"
        + (6).to_bytes(8, "big") + b"sha256" + (13).to_bytes(8, "big")
        + b"beacon.raw.v1" + hashlib.sha256(content).digest()
        + len(content).to_bytes(8, "big")
        + bytes.fromhex(resident.replace("-", ""))
        + bytes.fromhex(configuration.replace("-", "")) + b"\0\0"
        + stamp.to_bytes(8, "big"))
    return {"digest": {"algorithm": "sha256", "canonicalizer": "beacon.artifact.v1",
                       "value": hashlib.sha256(canonical).hexdigest()},
        "canonical_hex": canonical.hex(), "recorded_at_ms": stamp,
        "creation_event_seq": sequence, "content_hex": content.hex(),
        "resident": resident, "configuration": configuration}


def _a2_direct_output(*, duplicate_case_identity=False, fixture_case_drift=False,
                      panic_frame_drift=False, assertion_text_drift=False):
    """Build direct classifier bytes and oracle from the independent literal table."""
    store_root = "/fixture/a2-store"
    workspaces = tuple("123e4567-e89b-12d3-a456-4266141741%02d" % n for n in range(5))
    stores = tuple("123e4567-e89b-12d3-a456-4266141742%02d" % n for n in range(5))
    projects = tuple("123e4567-e89b-12d3-a456-4266141743%02d" % n for n in range(5))
    missions = tuple("123e4567-e89b-12d3-a456-4266141744%02d" % n for n in range(5))
    residents = tuple("123e4567-e89b-12d3-a456-4266141745%02d" % n for n in range(6))
    configurations = tuple("123e4567-e89b-12d3-a456-4266141746%02d" % n for n in range(6))
    lines = ["", "running 5 tests"]
    expected_cases = []
    for index, case in enumerate(_A2_CASE_ORACLE):
        label, suffix, stamps, distinct, assertion, line_number = case
        identity_index = 0 if duplicate_case_identity and index == 4 else index
        workspace = workspaces[identity_index]
        store = store_root + "/pa-" + stores[identity_index] + ".sqlite3"
        if label == "different-accountability":
            identities = ((residents[0], configurations[0]),
                          (residents[5], configurations[5]))
        else:
            identities = ((residents[identity_index], configurations[identity_index]),)
        sequence_by_identity = {}
        returned = []
        for record_index, stamp in enumerate(stamps):
            identity = identities[record_index] if len(identities) > 1 else identities[0]
            key = (stamp, *identity)
            sequence_by_identity.setdefault(key, len(sequence_by_identity) + 1)
            returned.append(_artifact_returned(workspace, identity[0], identity[1],
                stamp, sequence_by_identity[key]))
        digests = [item["digest"]["value"] for item in returned]
        rows, events, seen = [], [], set()
        for item in returned:
            digest = item["digest"]["value"]
            if digest in seen:
                continue
            seen.add(digest)
            rows.append({"digest": digest, "canonical_hex": item["canonical_hex"],
                "recorded_at_ms": item["recorded_at_ms"],
                "creation_event_seq": item["creation_event_seq"]})
            payload = {"artifact": {"digest": item["digest"],
                "canonicalBytesBase64": base64.b64encode(
                    bytes.fromhex(item["canonical_hex"])).decode("ascii"),
                "contentDigest": {"algorithm": "sha256", "canonicalizer": "beacon.raw.v1",
                    "value": "3acdb041be74dbe9a5aefba38832121cd62dda3a88003ccd283b4801090043ac"},
                "contentBytes": 45, "actor": "owner",
                "recordedAtMs": item["recorded_at_ms"]}}
            events.append({"sequence": item["creation_event_seq"],
                "payload_json": json.dumps(payload, sort_keys=True, separators=(",", ":")),
                "occurred_at_ms": item["recorded_at_ms"]})
        witness = {"schema_version": "artifact.identity.phase_a.v1", "case": label,
            "store": store, "workspace": workspace, "fixture_project": projects[identity_index],
            "fixture_mission": missions[identity_index], "timestamps": list(stamps),
            "digests": digests, "canonical_hex": [item["canonical_hex"] for item in returned],
            "reopen_read_result": ({"state": "observed", "row_count": 2,
                "event_count": 2, "verified_digests": digests, "records_equal": True,
                "contract_expected_independent_records": 3,
                "observed_distinct_records": 2, "observation_is_not_contract_acceptance": True}
                if label == "reopen" else {"state": "not_requested"}),
            "returned": returned, "row_count": distinct, "event_count": distinct,
            "rows": rows, "events": events}
        fixture_line = json.dumps({"case": label, "retained": True},
                                  sort_keys=True, separators=(",", ":"))
        if fixture_case_drift and index == 0:
            fixture_line = json.dumps({"case": label + "-drift", "retained": True},
                                      sort_keys=True, separators=(",", ":"))
        witness_line = json.dumps(witness, sort_keys=True, separators=(",", ":"))
        full_name = _A2_PREFIX + suffix
        lines.extend(("test " + full_name + " ... PHASE_A_FIXTURE " + fixture_line,
                      "PHASEA_WITNESS " + witness_line))
        if assertion is None:
            lines.append("ok")
        else:
            observed_line = line_number + 1 if panic_frame_drift and index == 1 else line_number
            observed_assertion = assertion + "_drift" if assertion_text_drift and index == 1 else assertion
            lines.extend(("", "thread '" + full_name + "' (" + str(1200 + index)
                + ") panicked at crates/beacond/src/artifact_identity_contract.rs:"
                + str(observed_line) + ":5:",
                "assertion `left != right` failed: " + observed_assertion,
                '  left: "' + digests[0] + '"', ' right: "' + digests[0] + '"'))
            if label == "equal":
                lines.append(_A2_BACKTRACE_NOTE)
            lines.append("FAILED")
        expected_cases.append({"case": label, "store": store, "workspace": workspace,
            "digests": digests, "row_count": distinct, "event_count": distinct})
    lines.extend(("", "failures:", "", "failures:",
        "    " + _A2_PREFIX + _A2_CASE_ORACLE[1][1],
        "    " + _A2_PREFIX + _A2_CASE_ORACLE[3][1], "",
        "test result: FAILED. 3 passed; 2 failed; 0 ignored; 0 measured; "
        "513 filtered out; finished in 1.00s", ""))
    raw = ("\n".join(lines) + "\n").encode("utf-8")
    expected = {"scope": "behavior-only", "classification": "EXPECTED_ORIGINAL_CONTRACT_RED",
        "framing": "direct", "raw_sha256": hashlib.sha256(raw).hexdigest(),
        "passed": 3, "failed": 2, "filtered_out": 513, "cases": expected_cases,
        "not_product_acceptance": True, "provenance_checked": False}
    return store_root, raw, expected


def _q_pin(name, raw):
    return {"path": "/fixture/q/" + name,
            "sha256": hashlib.sha256(raw).hexdigest()}


def _q_row_call(expected, observed, family, context, evidence_by_path, producer_raw):
    """Call the real finite q_row consumer with independently pinned test bytes."""
    def retained(pin, _inventory, cap=None):
        raw = evidence_by_path.get(pin["path"])
        if raw is None or hashlib.sha256(raw).hexdigest() != pin["sha256"]:
            raise AssertionError("synthetic q_row pin is not independently bound")
        if cap is not None and len(raw) > cap:
            raise AssertionError("synthetic q_row fixture exceeds its declared cap")
        return raw

    producer_role = D.Q_FRAGMENT_ROLES[family]
    with patch.object(D, "evidence_bytes", side_effect=retained):
        return D.q_row(expected, observed, family,
            {producer_role: (expected["oracle_source"], producer_raw)}, context)


def _historical_d01_parent_refusal(raw_token):
    """Row-specific predecessor comparator; never used as current authority."""
    if type(raw_token) is str and raw_token == "crates/beacond/src/../sample-project/Cargo.lock":
        return {"scenario": "predecessor-only:D01.2", "outcome": "REJECT_TYPED",
                "code": "PATH", "provenance": "deliberate-contract"}
    return None


def _custody_refresh_environment(environment, state_path, *, repeat=1,
                                  mutate_bound_environment=False):
    """Exercise refresh with fixed caller pins and the accepted root joins live."""
    request = _request()
    request["environment"] = deepcopy(environment)
    request["state_directory"] = {"path": str(state_path)}
    tool_paths = (request["python"], request["cargo"], request["rustfmt"])
    request["immutable_files"] = [
        {"path": path, "sha256": hashlib.sha256(path.encode()).hexdigest(),
         "dev": "7", "ino": str(index + 10), "size": "1", "mtime_ns": "1",
         "mode": "493", "uid": "1000", "nlink": "1"}
        for index, path in enumerate(tool_paths)
    ]
    request["selection_inputs"] = {"absent_paths": [], "aliases": []}
    request["module_paths"] = {}
    reference = QW._reference()
    reference["operator"]["instrument_sha256"] = {}
    roots = []
    for index, (key, kind, path) in enumerate((
        ("source_directory", "workspace", request["source_root"]),
        ("target_directory", "workspace", request["target_root"]),
        ("tmp", "temporary", _TEMPORARY_ROOT),
    )):
        directory = {"path": path, "dev": "7", "ino": str(index + 1),
                     "uid": "1000", "mode": "448"}
        request[key] = deepcopy(directory)
        roots.append({"directory": directory, "kind": kind})
    reference["operator"]["resource_roots"] = roots
    reservation = _reservation()
    request_pin = _pin("request")
    context = object.__new__(D.Custody)
    context.request = deepcopy(request)
    context.reference = reference
    context.clock = SimpleNamespace(reservation=reservation, check=lambda: None)
    context.state = Path(state_path)
    context.stage = "discovery"
    context.root = context.state / "discovery"
    context.request_pin = request_pin
    context.request_raw = R.encode(request)
    context.common_pin = request_pin
    context.grant_pin = request["grant"]
    context.reference_pin = request["reference"]
    context.inventory = {"files": {}, "directories": {}, "total_bytes": 0}
    context.armed = False
    context.reservation_pin = request["reservation"]
    context.history = _history()
    context.last_scan = None
    context.spent = []
    context.lock = None

    def pinned(spec, *_args, **_kwargs):
        if spec["path"] == request_pin["path"]:
            return R.encode(request)
        if spec["path"] == request["reference"]["path"]:
            return R.encode(reference)
        if spec["path"] == request["reservation"]["path"]:
            return R.encode(reservation)
        return b"synthetic-pinned-input"

    def observed(path, *_args, **_kwargs):
        if type(path) is dict:
            record = path
        else:
            record = next(item for item in request["immutable_files"] if item["path"] == path)
        return record, b"synthetic-tool-bytes"

    with patch.object(D.R, "require_umask", return_value=None), \
         patch.object(D.R, "pinned", side_effect=pinned), \
         patch.object(D.R, "verify_file_pins", return_value=None), \
         patch.object(D.W, "validate_reference", return_value=None), \
         patch.object(D.R, "file_observation", side_effect=observed), \
         patch.object(D.R, "directory", side_effect=lambda spec, **_kwargs: Path(spec["path"])):
        for index in range(repeat):
            if index == 1 and mutate_bound_environment:
                context.request["environment"]["CARGO_NET_OFFLINE"] = "false"
            context.refresh()
        return None


class B05AcceptedB04Controls(unittest.TestCase):
    """Additive source controls; these are not historical PASS rows."""

    def setUp(self):
        if not OWNER_IMPORTS_READY:
            raise RunnerBoundaryError("controls require explicit guarded runner bootstrap")
        self._selector_token = _ACTIVE_SELECTOR.set(self._testMethodName)

    def tearDown(self):
        token = getattr(self, "_selector_token", None)
        if token is not None:
            _ACTIVE_SELECTOR.reset(token)

    def _request_call(self, request):
        return D.check_request(request, grant_pin=_request_pins()["grant"],
            reference_pin=_request_pins()["reference"],
            reservation_pin=_request_pins()["reservation"], prior_pin=_request_pins()["prior"])

    def _expect_stop(self, call, code):
        with self.assertRaises(R.Stop) as caught:
            call()
        self.assertIs(type(caught.exception), R.Stop)
        self.assertEqual(str(caught.exception), code)
        self.assertEqual(caught.exception.provenance, "deliberate-contract")
        self.assertIsNone(caught.exception.refusal_context)

    def _expect_rejection(self, call, code):
        with self.assertRaises(W.Rejection) as caught:
            call()
        self.assertIs(type(caught.exception), W.Rejection)
        self.assertEqual(str(caught.exception), code)
        self.assertEqual(caught.exception.provenance, "deliberate-contract")

    def _expect_policy(self, call, code):
        with self.assertRaises(P.Refusal) as caught:
            call()
        self.assertIs(type(caught.exception), P.Refusal)
        self.assertEqual(str(caught.exception), code)

    def test_b05_qw01_historical_354_runner_is_ordered_and_independently_pinned(self):
        rows = QW.run_whole_program_controls()
        ids = [row["id"] for row in rows]
        raw_ids = json.dumps(ids, separators=(",", ":"), ensure_ascii=True).encode("utf-8")
        self.assertEqual(len(rows), 354)
        self.assertEqual(hashlib.sha256(raw_ids).hexdigest(),
            "a45ef0d756cd79f923c3ff4163b887666a4e198a77b54d2ccea0b6951d94d050")
        self.assertEqual(ids, list(QW.EXPECTED_CONTROL_IDS))
        self.assertTrue(all(row == {"id": control_id, "status": "PASS"}
                            for row, control_id in zip(rows, ids, strict=True)))

    def test_b05_ne01_native_nested_events_reach_supervisor_record(self):
        value = _supervisor_record()
        self.assertEqual(hashlib.sha256(_ORDINARY_LOG).hexdigest(), value["log_sha256"])
        self.assertIs(D.validate_supervisor_record(value), value)
        normalized = R.normalize_capture(value, _ORDINARY_LOG, 100, 200, 0)
        self.assertEqual(normalized, {"exit_code": 0, "whole_descendant_set_exit": "PROVEN_ECHILD",
            "pipe_eof": True, "subreaper_verified": True, "discarded_log_bytes": 0,
            "metadata_overflow": False, "errors": [], "event_error_count": 0,
            "stop": None, "log_sha256": hashlib.sha256(_ORDINARY_LOG).hexdigest(),
            "log_bytes": len(_ORDINARY_LOG), "start_monotonic_ns": "100",
            "end_monotonic_ns": "200"})

    def test_b05_ne02_native_classified_error_cannot_be_success(self):
        value = _supervisor_record()
        value["errors"] = [{"where": "emergency_cleanup", "type": "ResourcePathError",
                            "pathname_class": "entry-stat", "errno": 2}]
        value["event_counts"]["errors"] = 1
        self.assertIs(D.validate_supervisor_record(value), value)
        self._expect_stop(lambda: R.normalize_capture(value, _ORDINARY_LOG, 100, 200, 0),
                          "custody_or_unexpected_exit")

    def test_b05_ne03_physical_exit_eof_and_time_axes_are_separate(self):
        value = _supervisor_record()
        value["whole_descendant_set_exit"] = "UNKNOWN"
        self.assertIs(D.validate_supervisor_record(value), value)
        self._expect_stop(lambda: R.normalize_capture(value, _ORDINARY_LOG, 100, 200, 0),
                          "custody_or_unexpected_exit")
        value = _supervisor_record()
        value["pipe_eof"] = False
        self.assertIs(D.validate_supervisor_record(value), value)
        self._expect_stop(lambda: R.normalize_capture(value, _ORDINARY_LOG, 100, 200, 0),
                          "custody_or_unexpected_exit")
        value = _supervisor_record()
        value["elapsed_seconds"] = None
        self._expect_stop(lambda: R.normalize_capture(value, _ORDINARY_LOG, 100, 200, 0),
                          "capture_metadata")

        for name in ("reaped", "signals", "errors"):
            value = _supervisor_record()
            value["metadata_overflow"] = True
            value["stop"] = "metadata_overflow"
            value["event_counts"][name] = len(value[name]) + 1
            self.assertIs(D.validate_supervisor_record(value), value, name)
            self._expect_stop(lambda value=value: R.normalize_capture(
                value, _ORDINARY_LOG, 100, 200, 0), "capture_metadata")

        overflow_without_count_divergence = _supervisor_record()
        overflow_without_count_divergence["metadata_overflow"] = True
        overflow_without_count_divergence["stop"] = "metadata_overflow"
        self.assertIs(D.validate_supervisor_record(overflow_without_count_divergence),
                      overflow_without_count_divergence)
        self._expect_stop(lambda: R.normalize_capture(overflow_without_count_divergence,
            _ORDINARY_LOG, 100, 200, 0), "custody_or_unexpected_exit")

    def test_b05_ne04_event_shape_integer_and_float_boundaries(self):
        timed = {"leader_exit_monotonic": 1.0, "echild_monotonic": 2.0,
            "pipe_eof_monotonic": 3.0, "elapsed_seconds": 4.0,
            "reaped": [{"at": 5.0}], "signals": [{"at": 6.0}]}
        D.supervisor_float_scope(timed, wrapped=False)
        D.supervisor_float_scope({"supervisor": timed}, wrapped=True)
        with self.assertRaises(R.Stop) as caught:
            D.supervisor_float_scope({"supervisor": {"resource_peak_sampled": 1.0}},
                                     wrapped=True)
        self.assertIs(type(caught.exception), R.Stop)
        self.assertEqual(str(caught.exception), "b04_supervisor_float_scope")

        malformed_cases = (
            (lambda value: value.pop("wait_options"), "record_shape"),
            (lambda value: value.update({"extra_event": 1}), "record_shape"),
            (lambda value: value["reaped"][0].update({"extra_event": 1}), "record_shape"),
            (lambda value: value["reaped"][0].update({"status": True}),
             "b04_supervisor_reaped"),
            (lambda value: value.update({"resource_peak_sampled": 17.5}), "b04_integer"),
            (lambda value: value.update({"leader_exit_monotonic": -0.1}),
             "b04_supervisor_time"),
            (lambda value: value.update({"elapsed_seconds": float("nan")}),
             "b04_supervisor_time"),
            (lambda value: value.update({"errors": [{"where": "emergency_cleanup",
                "type": "ResourcePathError", "pathname_class": "entry-stat"}],
                "event_counts": {"reaped": 1, "signals": 0, "errors": 1}}),
             "record_shape"),
        )
        for mutate, code in malformed_cases:
            value = _supervisor_record()
            mutate(value)
            self._expect_stop(lambda value=value: D.validate_supervisor_record(value), code)

        value = _supervisor_record()
        value["leader_pid"] = True
        self._expect_stop(lambda: R.normalize_capture(value, _ORDINARY_LOG, 100, 200, 0),
                          "capture_metadata")
        value = _supervisor_record()
        value["event_counts"]["signals"] = True
        self._expect_stop(lambda: R.normalize_capture(value, _ORDINARY_LOG, 100, 200, 0),
                          "capture_metadata")
        value = _supervisor_record()
        value["resource_peak_sampled"] = True
        self._expect_stop(lambda: R.normalize_capture(value, _ORDINARY_LOG, 100, 200, 0),
                          "capture_metadata")
        value = _supervisor_record()
        value["elapsed_seconds"] = float("nan")
        self._expect_stop(lambda: R.normalize_capture(value, _ORDINARY_LOG, 100, 200, 0),
                          "capture_metadata")

        nullable_fields = ("leader_pid", "leader_returncode", "leader_exit_monotonic",
                           "echild_monotonic", "pipe_eof_monotonic", "stop")
        for field in nullable_fields:
            value = _supervisor_record()
            value[field] = None
            self.assertIs(D.validate_supervisor_record(value), value, field)
        for field in D.SUPERVISOR_TIMES:
            for invalid, code in ((True, "b04_supervisor_time"),
                                  (-0.1, "b04_supervisor_time"),
                                  (float("inf"), "b04_supervisor_time"),
                                  (float("nan"), "b04_supervisor_time")):
                value = _supervisor_record()
                value[field] = invalid
                self._expect_stop(lambda value=value: D.validate_supervisor_record(value), code)

        # Nested event timestamps share the closed finite-time grammar but are
        # distinct fields from the wrapper's top-level monotonic markers.
        for event_name, event_field in (("reaped", "at"), ("signals", "at")):
            for invalid in (True, -0.1, float("inf"), float("nan")):
                value = _supervisor_record()
                if event_name == "reaped":
                    value[event_name][0][event_field] = invalid
                else:
                    value[event_name] = [{"pid": 42, "signal": 15, "at": invalid}]
                    value["event_counts"][event_name] = 1
                self._expect_stop(lambda value=value: D.validate_supervisor_record(value),
                                  "b04_supervisor_time")

        # The retained prior-RAW consumer parses its own wrapper before the
        # supervisor validator. Keep a valid read baseline, then isolate the
        # nested event timestamp at that real parser/validator seam. JSON
        # non-finite constants are mapped by prior_capture's ValueError
        # handler to b04_prior_raw; finite negative floats are refused by the
        # native float scope.
        prior_start = {"schema_version": "pbdep.prior-slot-start.v2",
            "phase": "qualification", "slot": "Q", "argv": ["/fixture/tools/python3"],
            "cwd": "/fixture/prior-source", "environment_sha256": "a" * 64,
            "tools_sha256": "b" * 64, "sources_sha256": "c" * 64,
            "reference_sha256": "d" * 64, "binding_sha256": "e" * 64,
            "useful_seconds": 60, "exit_seconds": 60, "raw_cap": 4096,
            "recorded_monotonic_ns": "100", "slot_spent_even_if_fork_fails": True}
        prior_supervisor = _supervisor_record()
        prior_raw_record = {"schema_version": "pbdep.raw-custody.v1", "slot": "Q",
            "wrapper_before_run_monotonic_ns": "200",
            "wrapper_after_run_monotonic_ns": "300", "boundary_is_not_exact_fork": True,
            "stream": "merged-stdout-stderr", "manager_status": "NOT_OBSERVED_BY_ADAPTER",
            "aggregate_resource_state_at_terminal_write": "UNKNOWN",
            "supervisor": prior_supervisor, "resource_samples": _history(),
            "last_sampled_resources": None}
        prior_paths = {"start": "/fixture/prior-start.json",
            "raw": "/fixture/prior-raw.json", "log": "/fixture/prior-log.txt"}
        prior_spec = {name: {"path": path, "sha256": "0" * 64}
            for name, path in prior_paths.items()}
        prior_log = _ORDINARY_LOG

        def prior_bytes(pin, _cap=None):
            if pin["path"] == prior_paths["raw"]:
                raw_value = prior_raw_record
                return json.dumps(raw_value, sort_keys=True, separators=(",", ":"),
                                  allow_nan=True).encode("utf-8")
            if pin["path"] == prior_paths["log"]:
                return prior_log
            raise AssertionError("unexpected prior-capture evidence pin")

        with patch.object(D, "read_json", return_value=prior_start), \
             patch.object(R, "pinned", side_effect=prior_bytes):
            parsed_start, parsed_raw = D.prior_capture(prior_spec)
            self.assertEqual(parsed_start, prior_start)
            self.assertEqual(parsed_raw["supervisor"], prior_supervisor)

        for event_name in ("reaped", "signals"):
            for invalid, expected_code in ((True, "b04_supervisor_time"),
                    (-0.1, "b04_supervisor_float_scope"),
                    (float("inf"), "b04_prior_raw"),
                    (float("nan"), "b04_prior_raw")):
                changed_raw = deepcopy(prior_raw_record)
                changed_supervisor = changed_raw["supervisor"]
                if event_name == "reaped":
                    changed_supervisor["reaped"][0]["at"] = invalid
                else:
                    changed_supervisor["signals"] = [
                        {"pid": 42, "signal": 15, "at": invalid}]
                    changed_supervisor["event_counts"]["signals"] = 1
                prior_raw_record = changed_raw
                with patch.object(D, "read_json", return_value=prior_start), \
                     patch.object(R, "pinned", side_effect=prior_bytes):
                    self._expect_stop(lambda: D.prior_capture(prior_spec), expected_code)
                prior_raw_record = {"schema_version": "pbdep.raw-custody.v1", "slot": "Q",
                    "wrapper_before_run_monotonic_ns": "200",
                    "wrapper_after_run_monotonic_ns": "300", "boundary_is_not_exact_fork": True,
                    "stream": "merged-stdout-stderr", "manager_status": "NOT_OBSERVED_BY_ADAPTER",
                    "aggregate_resource_state_at_terminal_write": "UNKNOWN",
                    "supervisor": prior_supervisor, "resource_samples": _history(),
                    "last_sampled_resources": None}

        integer_fields = ("log_bytes", "discarded_log_bytes", "resource_peak_sampled")
        for field in integer_fields:
            for invalid in (True, -1, 2**63):
                value = _supervisor_record()
                value[field] = invalid
                self._expect_stop(lambda value=value: D.validate_supervisor_record(value),
                                  "b04_integer")
        for name in ("reaped", "signals", "errors"):
            for invalid in (True, -1, 2**63):
                value = _supervisor_record()
                value["event_counts"][name] = invalid
                self._expect_stop(lambda value=value: D.validate_supervisor_record(value),
                                  "b04_integer")
            value = _supervisor_record()
            value[name] = [None] * 129
            self._expect_stop(lambda value=value: D.validate_supervisor_record(value),
                              "b04_supervisor_events")

        for name in ("pipe_eof", "subreaper_verified", "metadata_overflow"):
            value = _supervisor_record()
            value[name] = 1
            self._expect_stop(lambda value=value: D.validate_supervisor_record(value),
                              "b04_supervisor_scalar")
        for name, invalid in (("whole_descendant_set_exit", "PARTIAL"),
                              ("wait_options", "WNOHANG"),
                              ("stop", "unlisted-stop"),
                              ("subreaper_verified", False)):
            value = _supervisor_record()
            value[name] = invalid
            self._expect_stop(lambda value=value: D.validate_supervisor_record(value),
                              "b04_supervisor_scalar")
        for name, invalid in (("leader_pid", 0), ("leader_returncode", True)):
            value = _supervisor_record()
            value[name] = invalid
            self._expect_stop(lambda value=value: D.validate_supervisor_record(value),
                              "b04_supervisor_leader")
        value = _supervisor_record()
        value["event_counts"].pop("signals")
        self._expect_stop(lambda: D.validate_supervisor_record(value), "record_shape")
        value = _supervisor_record()
        value["event_counts"]["unknown"] = 0
        self._expect_stop(lambda: D.validate_supervisor_record(value), "record_shape")
        value = _supervisor_record()
        value["signals"] = [{}]
        self._expect_stop(lambda: D.validate_supervisor_record(value), "record_shape")
        value = _supervisor_record()
        value["errors"] = [None]
        self._expect_stop(lambda: D.validate_supervisor_record(value),
                          "b04_supervisor_error")
        value = _supervisor_record()
        value["errors"] = [{"where": "emergency_cleanup", "type": "ResourcePathError",
                             "pathname_class": "entry-stat", "errno": None}]
        value["event_counts"]["errors"] = 1
        self.assertIs(D.validate_supervisor_record(value), value)
        self._expect_stop(lambda: R.normalize_capture(value, _ORDINARY_LOG, 100, 200, 0),
                          "custody_or_unexpected_exit")

        value = _supervisor_record()
        value["signals"] = [{"pid": 42, "signal": 15, "at": 1.25}]
        value["event_counts"]["signals"] = 1
        D.supervisor_float_scope(value, wrapped=False)
        self.assertIs(D.validate_supervisor_record(value), value)
        for field, invalid in (("pid", True), ("signal", False), ("at", -0.1)):
            value = _supervisor_record()
            value["signals"] = [{"pid": 42, "signal": 15, "at": 1.25}]
            value["event_counts"]["signals"] = 1
            value["signals"][0][field] = invalid
            code = "b04_supervisor_time" if field == "at" else "b04_supervisor_signal"
            self._expect_stop(lambda value=value: D.validate_supervisor_record(value), code)

    def test_b05_ne05_full_log_bytes_and_digest_are_joined(self):
        value = _supervisor_record()
        value["log_bytes"] += 1
        self._expect_stop(lambda: R.normalize_capture(value, _ORDINARY_LOG, 100, 200, 0),
                          "capture_identity")
        value = _supervisor_record()
        value["log_sha256"] = "0" * 64
        self._expect_stop(lambda: R.normalize_capture(value, _ORDINARY_LOG, 100, 200, 0),
                          "capture_identity")

    def test_b05_ne06_unknown_native_receipt_cannot_normalize_as_success(self):
        value = _supervisor_record()
        value.update({"whole_descendant_set_exit": "UNKNOWN", "pipe_eof": False,
                      "stop": "descendant_exit_or_capture_unknown"})
        self.assertIs(D.validate_supervisor_record(value), value)
        self._expect_stop(lambda: R.normalize_capture(value, _ORDINARY_LOG, 100, 200, 0),
                          "custody_or_unexpected_exit")

    def test_b05_env01_discovery_request_record_passes_accepted_entry_guard(self):
        request = _request()
        self.assertIs(self._request_call(request), request)

    def test_b05_env02_discovery_refresh_rejects_forbidden_environment_name(self):
        with tempfile.TemporaryDirectory(prefix="pbdep-b05-env02-", dir=_TMP_ROOT) as parent:
            state_path = Path(parent) / "state"
            self.assertIsNone(_custody_refresh_environment(_environment(), state_path))
            bad = _environment()
            bad["LD_PRELOAD"] = _FIXTURE_ROOT + "/synthetic.so"
            self._expect_stop(lambda: _custody_refresh_environment(bad, state_path),
                              "record_shape")

    def test_b05_env03_initial_request_path_grammar_and_target_join(self):
        baseline = _request()
        self.assertIs(self._request_call(baseline), baseline)
        for field in ("source_root", "target_root", "python", "cargo", "rustfmt"):
            changed = deepcopy(baseline)
            changed[field] = "/fixture/../escaped"
            self._expect_rejection(lambda changed=changed: self._request_call(changed), "PATH")
        for field in ("HOME", "CARGO_HOME", "RUSTUP_HOME", "TMPDIR", "PATH",
                      "CARGO_TARGET_DIR"):
            changed = deepcopy(baseline)
            changed["environment"][field] = "/fixture/../escaped"
            self._expect_rejection(lambda changed=changed: self._request_call(changed), "PATH")

        changed = deepcopy(baseline)
        changed["environment"]["CARGO_TARGET_DIR"] = _FIXTURE_ROOT + "/different-target"
        self._expect_stop(lambda: self._request_call(changed), "b04_environment_target")

    def test_b05_env04_discovery_fixed_literals_are_checked_at_refresh(self):
        valid = _environment()
        with tempfile.TemporaryDirectory(prefix="pbdep-b05-env04-", dir=_TMP_ROOT) as parent:
            state_path = Path(parent) / "state"
            # The same independently bound request survives both initial and
            # later refreshes; no ambient environment is inherited.
            self.assertIsNone(_custody_refresh_environment(valid, state_path, repeat=2))
            with self.assertRaises(R.Stop) as caught:
                _custody_refresh_environment(valid, state_path, repeat=2,
                    mutate_bound_environment=True)
            self.assertIs(type(caught.exception), R.Stop)
            self.assertEqual(str(caught.exception), "b04_request_changed")
            for key in _FIXED_ENVIRONMENT:
                changed = dict(valid)
                changed[key] = "synthetic-drift"
                self._expect_stop(lambda changed=changed: _custody_refresh_environment(
                    changed, state_path), "b04_environment_literal")

    def test_b05_env05_discovery_tmpdir_joins_request_temporary_root(self):
        value = _environment()
        value["TMPDIR"] = _FIXTURE_ROOT + "/other-tmp"
        with tempfile.TemporaryDirectory(prefix="pbdep-b05-env05-", dir=_TMP_ROOT) as parent:
            state_path = Path(parent) / "state"
            self._expect_stop(lambda: _custody_refresh_environment(
                value, state_path), "b04_environment_tmp")
            self.assertFalse(state_path.exists())

    def test_b05_env06_discovery_target_joins_own_registered_root(self):
        for phase in ("discovery", "witness"):
            request = _request()
            self.assertIsNone(D.cleared_environment(request["environment"], request,
                                                    phase=phase))
            changed = deepcopy(request["environment"])
            changed["CARGO_TARGET_DIR"] = _FIXTURE_ROOT + "/other-target"
            self._expect_stop(lambda changed=changed, request=request, phase=phase:
                D.cleared_environment(changed, request, phase=phase), "b04_environment_target")
        for phase in ("qualification", "format"):
            environment = _environment()
            del environment["CARGO_TARGET_DIR"]
            binding = {"tmp": {"path": _TEMPORARY_ROOT},
                       "environment": environment}
            self.assertIsNone(D.cleared_environment(environment, binding, phase=phase))
            extra_target = {**environment, "CARGO_TARGET_DIR": _FIXTURE_ROOT + "/target"}
            self._expect_stop(lambda extra_target=extra_target, binding=binding, phase=phase:
                D.cleared_environment(extra_target, binding, phase=phase), "record_shape")
        path_binding = {"tmp": {"path": _TEMPORARY_ROOT},
                        "environment": _environment()}
        for path_value in (
                "/fixture/tools:/fixture/tools",
                ":".join("/fixture/tool" + str(index) for index in range(33))):
            duplicate_or_over = {**path_binding["environment"], "PATH": path_value}
            self._expect_stop(lambda duplicate_or_over=duplicate_or_over:
                D.cleared_environment(duplicate_or_over, path_binding, phase="discovery"),
                "b04_environment_path")
        for path_value in ("", ".", "/", "relative/tool", "/fixture/../tools",
                           "/fixture/sourcé"):
            malformed_path = {**path_binding["environment"], "PATH": path_value}
            self._expect_rejection(lambda malformed_path=malformed_path:
                D.cleared_environment(malformed_path, path_binding, phase="discovery"),
                "PATH")
        too_long = {**path_binding["environment"], "PATH": "x" * 8193}
        self._expect_stop(lambda: D.cleared_environment(too_long, path_binding,
            phase="discovery"), "b04_environment")
        value = _environment()
        value["CARGO_TARGET_DIR"] = _FIXTURE_ROOT + "/other-target"
        with tempfile.TemporaryDirectory(prefix="pbdep-b05-env06-", dir=_TMP_ROOT) as parent:
            state_path = Path(parent) / "state"
            self._expect_stop(lambda: _custody_refresh_environment(
                value, state_path), "b04_environment_target")
            self.assertFalse(state_path.exists())

    def test_b05_env07_phase_data_path_uses_whole_program_rejection(self):
        format_data = QW._format_data()
        raw, reference = QW._phase_context("format", format_data)
        result = W.validate_phase_data("format", format_data,
            reference=reference, phase_binding_raw=raw)
        self.assertEqual(result["scope"], "phase-data-only")
        for malformed in (
                "relative/source", "/fixture/../escaped-source", "//fixture/source",
                "/fixture/source/", "", "/fixture/sourcé", "/", "."):
            data = deepcopy(QW._fixture("candidate")["data"])
            data["source_root"] = malformed
            # Freeze the malformed public input's own raw and canonical pins before
            # the call, so the only changed field is the path grammar under test.
            raw, reference = QW._phase_context("candidate", data)
            self._expect_rejection(lambda data=data, raw=raw, reference=reference:
                W.validate_phase_data("candidate", data, reference=reference,
                    phase_binding_raw=raw), "PATH")

    def test_b05_env08_later_phase_target_joins_data_and_registered_root(self):
        """The accepted binding and target/root joins remain live for every later phase."""
        def trial(phase, environment_target, register_target=True):
            source_root = "/fixture/later-source"
            target_root = "/fixture/" + phase + "-target"
            data = (QW._fixture(phase)["data"] if phase in ("base", "candidate")
                    else QW._later_data(phase))
            data["source_root"] = source_root
            data["target_root"] = target_root
            environment = _environment()
            environment["CARGO_TARGET_DIR"] = environment_target
            tmp = {"path": _TEMPORARY_ROOT, "dev": "7", "ino": "8",
                   "uid": "1000", "mode": "448"}
            source = {"path": source_root, "dev": "7", "ino": "9",
                      "uid": "1000", "mode": "448"}
            roots = [{"directory": source, "kind": "workspace"},
                     {"directory": tmp, "kind": "temporary"}]
            target = {"path": target_root, "dev": "7", "ino": "10",
                      "uid": "1000", "mode": "448"}
            if register_target:
                roots.append({"directory": target, "kind": "workspace"})
            operator = QW._reference()["operator"]
            operator["resource_roots"] = deepcopy(roots)
            authority = {name: operator[name] for name in
                         ("authority", "packet", "acceptance", "review", "disposition")}
            step_zero = _pin("step-zero")
            freeze = _pin("source-freeze")
            tools = ("/fixture/tools/python3", "/fixture/tools/cargo", "/fixture/tools/rustfmt")
            immutable = [{"path": path, "sha256": hashlib.sha256(path.encode()).hexdigest(),
                "dev": "7", "ino": str(index + 20), "size": "1", "mtime_ns": "1",
                "mode": "493", "uid": "1000", "nlink": "1"}
                for index, path in enumerate(tools)]
            binding = {"schema_version": "phase-b.phase-binding.v2", "phase": phase,
                "authority_pins": {name: pin["sha256"] for name, pin in authority.items()},
                "cwd": source_root, "environment": environment, "python": tools[0],
                "immutable_files": immutable, "tmp": tmp,
                "selection_inputs": {"absent_paths": [], "aliases": []},
                "cargo": tools[1], "rustfmt": tools[2], "data": data,
                "step_zero_acceptance": step_zero, "source_freeze_review": freeze,
                "operator_phase_attestation": {"step_zero_read_by": "Luna",
                    "freeze_read_by": "Astra", "issuer": "Muse",
                    "step_zero_sha256": step_zero["sha256"],
                    "freeze_sha256": freeze["sha256"],
                    "data_sha256": R.digest(R.encode(data)), "grants_authority": False}}
            raw = R.encode(binding)
            phase_pin = {"path": "/fixture/phase-bindings/" + phase + ".json",
                         "sha256": hashlib.sha256(raw).hexdigest()}
            reference = QW._reference(phase, data, operator=operator,
                                      phase_binding_raw=raw)
            record = {"phase": phase, "phase_binding": phase_pin,
                      "state_directory": {"path": "/fixture/state"}}
            def read_evidence(spec, *_args, **_kwargs):
                return raw if spec == phase_pin else b"synthetic-evidence-bytes"

            records = {item["path"]: item for item in immutable}

            def observe(path, *_args, **_kwargs):
                if type(path) is not str or path not in records:
                    raise AssertionError("file_observation must receive a pinned path")
                return records[path], b"synthetic-tool-bytes"

            with patch.object(D, "evidence_bytes", side_effect=read_evidence), \
                 patch.object(D.R, "validate_binding_links", wraps=R.validate_binding_links) as binding_links, \
                 patch.object(D.R, "directory", return_value=None), \
                 patch.object(D.R, "path_check", return_value=Path(source_root)), \
                 patch.object(D.R, "file_observation", side_effect=observe):
                bound, accepted_raw = D.prerequisite_binding(record, reference, {})
            binding_links.assert_called_once_with(reference["operator"], bound,
                accepted_raw, reference=reference)
            return bound, accepted_raw

        for phase in ("base", "candidate", "integration", "consumers", "gate"):
            target_root = "/fixture/" + phase + "-target"
            bound, raw = trial(phase, target_root)
            self.assertEqual(bound["data"]["target_root"], target_root)
            self.assertTrue(raw)
            self._expect_stop(lambda phase=phase: trial(
                phase, "/fixture/" + phase + "-other-target"), "b04_prior_target_root")
            self._expect_stop(lambda phase=phase, target_root=target_root: trial(
                phase, target_root, register_target=False), "b04_prior_target_root")

    def test_b05_env09_a2_target_joins_native_provenance_and_registered_root(self):
        def consume(environment_target, register_target=True,
                    native_version="pbdep.provenance-binding.v2", *,
                    authority_drift=False, manifest_drift=False,
                    command_drift=False, result_drift=False, roster_drift=False):
            target_root = "/fixture/a2-target"
            source_root = "/fixture/a2-source"
            listing_raw = _a2_roster_preimage("full")
            full_roster = listing_raw + b"x" if roster_drift else listing_raw
            listing = b"".join(name + b": test\n" for name in listing_raw.splitlines())
            pilot_roster = _a2_roster_preimage("pilot")
            overlay_roster = _a2_roster_preimage("overlay")
            role_names = (
                "original_reference", "provenance_binding", "full_roster", "pilot_roster",
                "overlay_roster", "manifest", "depfile", "admission", "pre",
                "completion", "result")
            state_root = "/fixture/a2-consumer-state"
            roles = {name: {"path": state_root + "/" + name + ".json",
                "sha256": "0" * 64} for name in role_names}
            operator = {name: _pin(name) for name in
                ("authority", "packet", "acceptance", "review")}
            roots = [{"directory": {"path": source_root, "dev": "7", "ino": "8",
                "uid": "1000", "mode": "448"}, "kind": "workspace"},
                {"directory": {"path": "/fixture/tmp", "dev": "7", "ino": "9",
                "uid": "1000", "mode": "448"}, "kind": "temporary"}]
            if register_target:
                roots.append({"directory": {"path": target_root, "dev": "7", "ino": "10",
                    "uid": "1000", "mode": "448"}, "kind": "workspace"})
            reference = {"subject_head": "a" * 40,
                "authority_sha256": operator["authority"]["sha256"],
                "bound_packet_sha256": operator["packet"]["sha256"],
                "acceptance_sha256": operator["acceptance"]["sha256"],
                "review_sha256": operator["review"]["sha256"]}
            reference_raw = R.encode(reference)
            roles["original_reference"]["sha256"] = hashlib.sha256(reference_raw).hexdigest()
            record_operator = deepcopy(operator)
            if authority_drift:
                record_operator["authority"]["sha256"] = "c" * 64
            record = {"_reference": {"operator": {**record_operator,
                "resource_roots": roots}}}
            binding = {"data": {"original_reference": roles["original_reference"]},
                "cwd": source_root, "cargo": "/fixture/tools/cargo",
                "python": "/fixture/tools/python3", "rustfmt": "/fixture/tools/rustfmt",
                "tmp": {"path": "/fixture/tmp"},
                "environment": {"CARGO_TARGET_DIR": environment_target}}
            manifest_raw = b"synthetic manifest\n"
            roles["manifest"]["sha256"] = hashlib.sha256(manifest_raw).hexdigest()
            native = {"schema_version": native_version,
                "source_root": binding["cwd"], "subject_head": reference["subject_head"],
                "source_manifest_sha256": ("d" * 64 if manifest_drift
                    else roles["manifest"]["sha256"]),
                "target_root": target_root}
            native_raw = R.encode(native)
            depfile = b"synthetic A2 depfile\n"
            capture_values = ({"kind": "compile"}, {"kind": "list"}, {"kind": "witness"})
            binary_path = target_root + "/debug/deps/a2-test"
            expected_compile = ("/fixture/tools/cargo", "test", "--locked", "--offline",
                "-p", "beacond", "--lib", "--no-run", "--message-format=json")
            commands = (expected_compile, (binary_path, "--list", "--format", "terse"),
                (binary_path, "artifact::artifact_identity_contract::", "--nocapture",
                 "--test-threads=1"))
            before = {"binary": None, "_hashes": {}, "tools": {}}
            captures = [{"log": b"synthetic retained log\n", "capture": value,
                "start": {"argv": list(command), "cwd": binding["cwd"],
                    "environment_sha256": R.digest(R.encode(binding["environment"])),
                    "tools_sha256": R.digest(R.encode(before["tools"])),
                    "sources_sha256": R.digest(R.encode(before["_hashes"]))}}
                for command, value in zip(commands, capture_values, strict=True)]
            captures[1]["log"] = listing
            if command_drift:
                captures[0]["start"]["argv"][0] += "-cross-program"
            admission = R.encode({"compile": capture_values[0], "inventory": capture_values[1]})
            completion = R.encode({"capture": capture_values[2]})
            payloads = {"original_reference": reference_raw,
                "provenance_binding": native_raw, "full_roster": full_roster,
                "pilot_roster": pilot_roster, "overlay_roster": overlay_roster,
                "manifest": manifest_raw, "depfile": depfile,
                "admission": admission, "pre": b"synthetic pre\n", "completion": completion}
            outcome = {"scope": "synthetic-A2-consumer-join-only"}
            after = {"binary": {"path": binary_path},
                "_hashes": {}, "depfile_sha256": hashlib.sha256(depfile).hexdigest()}
            evidence = {"roles": roles}
            result_value = deepcopy(outcome)
            if result_drift:
                result_value["scope"] = "synthetic-stale-result"
            payloads["result"] = R.encode(result_value)
            for role, raw in payloads.items():
                roles[role]["sha256"] = hashlib.sha256(raw).hexdigest()
            inventory = {"state_directory": {"path": state_root}, "files": {},
                "directories": {}, "total_bytes": 0}
            payloads_by_path = {}
            for role, pin in roles.items():
                raw = payloads[role]
                relative = str(Path(pin["path"]).relative_to(state_root))
                inventory["files"][relative] = {"path": pin["path"],
                    "sha256": pin["sha256"], "dev": "7", "ino": str(100 + len(inventory["files"])),
                    "size": str(len(raw)), "mtime_ns": "1", "mode": "384",
                    "uid": "1000", "nlink": "1"}
                inventory["total_bytes"] += len(raw)
                payloads_by_path[pin["path"]] = raw

            def pinned(spec, cap=R.CONTROL_CAP, *_args, **_kwargs):
                raw = payloads_by_path.get(spec["path"])
                if raw is None or len(raw) > cap or hashlib.sha256(raw).hexdigest() != spec["sha256"]:
                    raise AssertionError("synthetic retained A2 input is not independently pinned")
                return raw
            with patch.object(D.C, "validate_listing", wraps=C.validate_listing) as listing_validator, \
                 patch.object(D.C, "validate_provenance", return_value={}) as provenance, \
                 patch.object(D.C, "validate_witness_completion", return_value=None), \
                 patch.object(D.C, "classify_a2", return_value=outcome), \
                 patch.object(D.R, "pinned", side_effect=pinned):
                result = D.consume_a2(record, binding, evidence, captures, before, after, inventory)
                listing_validator.assert_called_once()
                return result, provenance

        roster_metadata = json.loads(Path(__file__).with_name("fixture-files.json")
            .read_text(encoding="utf-8"))["a2_listing_preimages"]
        self.assertEqual(roster_metadata, [
            {"role": "full", "retained_locator":
                "reports/beacon-i1-phaseB-20261020/r2/EXPECTED-ROSTER.txt",
             "sha256": "80176813d5049c5db9ec2750ea6ec343603cfbc9d8ded6a3843fc2b74b28dc27",
             "bytes": 40524, "rows": 518, "identifier_only_sorted_unique_lf": True},
            {"role": "pilot", "retained_locator":
                "reports/beacon-i1-phaseB-20261020/r2/PILOT-ROSTER.txt",
             "sha256": "464efb7b12934af12c91a537130a0303ed1a1bda73595a0ff1e2c0765e0caf9f",
             "bytes": 5120, "rows": 68, "identifier_only_sorted_unique_lf": True},
            {"role": "overlay", "retained_locator":
                "reports/beacon-i1-phaseB-20261020/r2/EXPECTED-OVERLAY.txt",
             "sha256": "be9e58b9677ab50ba2c954c3435f9d0f0d45d37674a01be5e929d6fe1d169641",
             "bytes": 439, "rows": 5, "identifier_only_sorted_unique_lf": True},
        ])
        result, provenance = consume("/fixture/a2-target")
        self.assertEqual(result[1], {"scope": "synthetic-A2-consumer-join-only"})
        provenance.assert_called_once()
        with self.assertRaises(R.Stop) as caught:
            consume("/fixture/a2-target", native_version="pbdep.provenance-binding.v1")
        self.assertIs(type(caught.exception), R.Stop)
        self.assertEqual(str(caught.exception), "b04_a2_native_v2")
        for environment_target, register_target in (
                ("/fixture/other-target", True), ("/fixture/a2-target", False)):
            with self.assertRaises(R.Stop) as caught:
                consume(environment_target, register_target)
            self.assertIs(type(caught.exception), R.Stop)
            self.assertEqual(str(caught.exception), "b04_a2_source")

        # The accepted listing validator receives the exact retained roster
        # bytes at the aggregate consumer; only the full-roster pin is changed.
        with self.assertRaises(C.Rejection) as caught:
            consume("/fixture/a2-target", roster_drift=True)
        self.assertIs(type(caught.exception), C.Rejection)
        self.assertIs(caught.exception.code, C.Code.PIN)

        # These supplied synthetic records reach the current A2 joins. The
        # negative cases mutate one authority, source, command, or result join.
        with self.assertRaises(R.Stop) as caught:
            consume("/fixture/a2-target", authority_drift=True)
        self.assertEqual(str(caught.exception), "b04_a2_authority")
        with self.assertRaises(R.Stop) as caught:
            consume("/fixture/a2-target", manifest_drift=True)
        self.assertEqual(str(caught.exception), "b04_a2_source")
        with self.assertRaises(R.Stop) as caught:
            consume("/fixture/a2-target", command_drift=True)
        self.assertEqual(str(caught.exception), "b04_prior_command_semantics")
        with self.assertRaises(W.Rejection) as caught:
            consume("/fixture/a2-target", result_drift=True)
        self.assertIs(type(caught.exception), W.Rejection)
        self.assertEqual(str(caught.exception), "PHASE_DATA")

    def test_b05_a2_provenance01_pre_and_post_witness_joins_use_accepted_consumers(self):
        source_root = "/fixture/a2-contract-source"
        target_root = "/fixture/a2-contract-target"
        subject_head = "a" * 40
        executable = target_root + "/debug/deps/beacond-0123456789abcdef"
        self.assertEqual(tuple(sorted(C.REQUIRED_SOURCE_NAMES)),
                         tuple(sorted(_A2_REQUIRED_SOURCE_NAMES)))
        self.assertEqual(tuple(C.ROSTER_PINS), _A2_ROSTER_PINS)
        source_names = sorted(set(_A2_REQUIRED_SOURCE_NAMES) | {
            "Cargo.lock", "Cargo.toml", "rust-toolchain.toml",
            "crates/beacond/Cargo.toml", "crates/beacond/src/lib.rs"})
        source_hashes = {name: hashlib.sha256(("synthetic-source:" + name).encode()).hexdigest()
                         for name in source_names}
        required_source_pins = {name: source_hashes[name]
                                for name in _A2_REQUIRED_SOURCE_NAMES}
        authority = {name: hashlib.sha256(("synthetic-" + name).encode()).hexdigest()
                     for name in ("authority", "packet", "acceptance", "review")}
        source_manifest = {"schema_version": "phase-b.source-manifest.v1",
            "subject_head": subject_head, "source_root": source_root,
            "files": source_hashes}
        manifest_raw = R.encode(source_manifest)
        manifest_sha256 = hashlib.sha256(manifest_raw).hexdigest()
        reference = {"schema_version": "pbdep.original-reference.v2",
            "evidence_contract": "phase-b.original-a2-evidence.v1",
            "binding_sha256": "0" * 64, "subject_head": subject_head,
            "authority_sha256": authority["authority"],
            "bound_packet_sha256": authority["packet"],
            "acceptance_sha256": authority["acceptance"],
            "review_sha256": authority["review"],
            "source_manifest_sha256": manifest_sha256,
            "required_source_pins": required_source_pins}
        tokens = sorted({"crates/beacond/src/lib.rs"} | {
            name for name in _A2_REQUIRED_SOURCE_NAMES
            if name.startswith("crates/beacond/src/")})
        binding = {"schema_version": "pbdep.provenance-binding.v2",
            "subject_head": subject_head, "source_root": source_root,
            "target_root": target_root, "authority_sha256": authority["authority"],
            "bound_packet_sha256": authority["packet"],
            "acceptance_sha256": authority["acceptance"],
            "review_sha256": authority["review"],
            "source_manifest_sha256": manifest_sha256,
            "roster_sha256": list(_A2_ROSTER_PINS), "dep_tokens": tokens}
        binding_raw = R.encode(binding)
        binding_sha256 = hashlib.sha256(binding_raw).hexdigest()
        reference["binding_sha256"] = binding_sha256

        artifact = {"reason": "compiler-artifact",
            "target": {"name": "beacond", "kind": ["lib"], "crate_types": ["lib"],
                "edition": "2024", "test": True,
                "src_path": source_root + "/crates/beacond/src/lib.rs"},
            "profile": {"test": True}, "fresh": False,
            "manifest_path": source_root + "/crates/beacond/Cargo.toml",
            "package_id": "path+file://" + source_root + "/crates/beacond#0.1.0",
            "executable": executable, "filenames": [executable]}
        compiler_log = (json.dumps(artifact, sort_keys=True, separators=(",", ":")) + "\n"
            + json.dumps({"reason": "build-finished", "success": True},
                         sort_keys=True, separators=(",", ":")) + "\n"
            + "    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.00s\n").encode()
        depfile_tokens = tokens
        dependencies = " ".join(depfile_tokens)
        depfile = (executable + ".d: " + dependencies + "\n\n"
            + executable + ": " + dependencies + "\n\n"
            + "".join(token + ":\n" for token in depfile_tokens)
            + "\n# env-dep:CARGO_MANIFEST_DIR=" + source_root + "/crates/beacond\n"
            + "# env-dep:CARGO_PKG_VERSION=0.1.0\n").encode()
        full_roster = _a2_roster_preimage("full")
        listing = b"".join(name + b": test\n" for name in full_roster.splitlines())
        binary = {"path": executable,
            "sha256": hashlib.sha256(b"synthetic binary bytes").hexdigest(),
            "dev": "7", "ino": "11", "size": "22", "mtime_ns": "1"}

        def capture(log, start, end, exit_code=0):
            return {"exit_code": exit_code,
                "whole_descendant_set_exit": "PROVEN_ECHILD", "pipe_eof": True,
                "subreaper_verified": True, "discarded_log_bytes": 0,
                "metadata_overflow": False, "errors": [], "event_error_count": 0,
                "stop": None, "log_sha256": hashlib.sha256(log).hexdigest(),
                "log_bytes": len(log), "start_monotonic_ns": str(start),
                "end_monotonic_ns": str(end)}

        compile_capture = capture(compiler_log, 100, 200)
        listing_capture = capture(listing, 201, 300)
        admission = {"schema_version": "pbdep.pre-witness-admission.v2",
            "binding_sha256": binding_sha256,
            "authority_sha256": authority["authority"],
            "bound_packet_sha256": authority["packet"],
            "acceptance_sha256": authority["acceptance"],
            "review_sha256": authority["review"], "subject_head": subject_head,
            "source_manifest_sha256": manifest_sha256,
            "source_hashes": source_hashes, "binary": binary,
            "depfile_sha256": hashlib.sha256(depfile).hexdigest(),
            "compiler_log_sha256": hashlib.sha256(compiler_log).hexdigest(),
            "listing_sha256": hashlib.sha256(listing).hexdigest(),
            "roster_sha256": list(_A2_ROSTER_PINS), "compile": compile_capture,
            "inventory": listing_capture, "recorded_monotonic_ns": "301"}
        admission_raw = R.encode(admission)
        pre = {"schema_version": "phase-b.pre-witness.v1",
            "admission_sha256": hashlib.sha256(admission_raw).hexdigest(),
            "binding_sha256": binding_sha256, "binary": binary,
            "source_hashes": source_hashes,
            "depfile_sha256": hashlib.sha256(depfile).hexdigest(),
            "recorded_monotonic_ns": "302"}
        pre_raw = R.encode(pre)
        provenance = C.validate_provenance(compiler_log, depfile, listing, manifest_raw,
            admission_raw, pre_raw, binding=binding_raw,
            binding_sha256=binding_sha256, observed_source_hashes=source_hashes,
            observed_binary=binary, reference=reference)
        self.assertEqual(provenance["scope"], "pre-dispatch-provenance-joins-only")
        self.assertEqual(provenance["binding_sha256"], binding_sha256)
        self.assertEqual(provenance["source_hashes"], source_hashes)

        # Native-v2 is a closed consumer schema. Rebind each mutated byte
        # document to an independently rebuilt reference so the only changed
        # predicate is the version or closed-key check under C.validate_provenance.
        for mutation in ("old-version", "extra-key"):
            changed_binding = json.loads(binding_raw.decode("utf-8"))
            if mutation == "old-version":
                changed_binding["schema_version"] = "pbdep.provenance-binding.v1"
            else:
                changed_binding["unlisted"] = "synthetic-extra"
            changed_binding_raw = R.encode(changed_binding)
            changed_binding_sha = hashlib.sha256(changed_binding_raw).hexdigest()
            changed_reference = deepcopy(reference)
            changed_reference["binding_sha256"] = changed_binding_sha
            with self.assertRaises(C.Rejection) as caught:
                C.validate_provenance(compiler_log, depfile, listing, manifest_raw,
                    admission_raw, pre_raw, binding=changed_binding_raw,
                    binding_sha256=changed_binding_sha,
                    observed_source_hashes=source_hashes, observed_binary=binary,
                    reference=changed_reference)
            self.assertIs(type(caught.exception), C.Rejection)
            self.assertIs(caught.exception.code, C.Code.ADMISSION)

        witness = b"synthetic retained witness bytes\n"
        witness_capture = capture(witness, 303, 304, exit_code=101)
        completion = {"schema_version": "phase-b.witness-completion.v1",
            "binding_sha256": binding_sha256,
            "admission_sha256": provenance["admission_sha256"],
            "pre_witness_sha256": provenance["pre_witness_sha256"],
            "capture": witness_capture}
        completion_raw = R.encode(completion)
        completed = C.validate_witness_completion(witness, completion_raw,
            provenance=provenance, observed_binary=binary,
            observed_source_hashes=source_hashes,
            observed_depfile_sha256=hashlib.sha256(depfile).hexdigest(),
            reference=reference)
        self.assertEqual(completed["scope"], "post-witness-custody-only")
        self.assertEqual(completed["raw_sha256"], hashlib.sha256(witness).hexdigest())

        for mutation in ("old-version", "extra-key"):
            changed_completion = deepcopy(completion)
            if mutation == "old-version":
                changed_completion["schema_version"] = "pbdep.witness-completion.v0"
            else:
                changed_completion["unlisted"] = "synthetic-extra"
            with self.assertRaises(C.Rejection) as caught:
                C.validate_witness_completion(witness, R.encode(changed_completion),
                    provenance=provenance, observed_binary=binary,
                    observed_source_hashes=source_hashes,
                    observed_depfile_sha256=hashlib.sha256(depfile).hexdigest(),
                    reference=reference)
            self.assertIs(type(caught.exception), C.Rejection)
            self.assertIs(caught.exception.code, C.Code.CUSTODY)

        # The aggregate path keeps all four accepted C consumers live. Only the
        # exogenous pinned-file read is supplied; source, command, roster,
        # provenance, completion and classification joins remain real.
        store_root, aggregate_witness, expected_outcome = _a2_direct_output()
        # The aggregate receipt is bound to the final five-case bytes. Keep
        # the short witness above as a separate pre-aggregate consumer smoke.
        aggregate_witness_capture = capture(aggregate_witness, 303, 304, exit_code=101)
        aggregate_completion = {"schema_version": "phase-b.witness-completion.v1",
            "binding_sha256": binding_sha256,
            "admission_sha256": provenance["admission_sha256"],
            "pre_witness_sha256": provenance["pre_witness_sha256"],
            "capture": aggregate_witness_capture}
        aggregate_completion_raw = R.encode(aggregate_completion)
        state_root = "/fixture/a2-aggregate-state"
        role_names = ("original_reference", "provenance_binding", "full_roster",
            "pilot_roster", "overlay_roster", "manifest", "depfile", "admission",
            "pre", "completion", "result")
        roles = {name: {"path": state_root + "/" + name + ".json", "sha256": "0" * 64}
                 for name in role_names}
        full_roster = _a2_roster_preimage("full")
        pilot_roster = _a2_roster_preimage("pilot")
        overlay_roster = _a2_roster_preimage("overlay")
        outcome_raw = R.encode(expected_outcome)
        payloads = {
            "original_reference": R.encode(reference),
            "provenance_binding": binding_raw,
            "full_roster": full_roster,
            "pilot_roster": pilot_roster,
            "overlay_roster": overlay_roster,
            "manifest": manifest_raw,
            "depfile": depfile,
            "admission": admission_raw,
            "pre": pre_raw,
            "completion": aggregate_completion_raw,
            "result": outcome_raw,
        }
        inventory = {"state_directory": {"path": state_root}, "files": {},
            "directories": {}, "total_bytes": 0}
        bytes_by_path = {}
        for role, raw in payloads.items():
            pin = roles[role]
            pin["sha256"] = hashlib.sha256(raw).hexdigest()
            relative = str(Path(pin["path"]).relative_to(state_root))
            inventory["files"][relative] = {"path": pin["path"],
                "sha256": pin["sha256"], "dev": "7", "ino": str(100 + len(bytes_by_path)),
                "size": str(len(raw)), "mtime_ns": "1", "mode": "384",
                "uid": "1000", "nlink": "1"}
            inventory["total_bytes"] += len(raw)
            bytes_by_path[pin["path"]] = raw
        binding_value = {"cwd": source_root, "cargo": "/fixture/tools/cargo",
            "python": "/fixture/tools/python3", "rustfmt": "/fixture/tools/rustfmt",
            "environment": {"CARGO_TARGET_DIR": target_root},
            "tmp": {"path": store_root}, "data": {"original_reference":
                roles["original_reference"]}}
        operator = {name: {"sha256": authority[name]} for name in
            ("authority", "packet", "acceptance", "review")}
        operator["resource_roots"] = [{"directory": {"path": target_root},
            "kind": "workspace"}]
        record = {"_reference": {"operator": operator}}
        before = {"binary": None, "_hashes": source_hashes, "tools": {}}
        after = {"binary": binary, "_hashes": source_hashes,
            "depfile_sha256": hashlib.sha256(depfile).hexdigest()}
        commands = (
            ("/fixture/tools/cargo", "test", "--locked", "--offline", "-p",
                "beacond", "--lib", "--no-run", "--message-format=json"),
            (executable, "--list", "--format", "terse"),
            (executable, _A2_PREFIX, "--nocapture", "--test-threads=1"),
        )
        logs = (compiler_log, listing, aggregate_witness)
        captures = []
        for command, log, capture_record in zip(commands, logs,
                (compile_capture, listing_capture, aggregate_witness_capture), strict=True):
            captures.append({"log": log, "capture": capture_record,
                "start": {"argv": list(command), "cwd": source_root,
                    "environment_sha256": R.digest(R.encode(binding_value["environment"])),
                    "tools_sha256": R.digest(R.encode(before["tools"])),
                    "sources_sha256": R.digest(R.encode(source_hashes))}})

        def pinned_synthetic(pin, cap=R.CONTROL_CAP):
            raw = bytes_by_path.get(pin["path"])
            if raw is None or len(raw) > cap or hashlib.sha256(raw).hexdigest() != pin["sha256"]:
                raise AssertionError("A2 synthetic evidence bytes do not match their independent pin")
            return raw

        evidence = {"roles": roles}
        with patch.object(R, "pinned", side_effect=pinned_synthetic), \
             patch.object(C, "validate_listing", wraps=C.validate_listing) as listing_call, \
             patch.object(C, "validate_provenance", wraps=C.validate_provenance) as provenance_call, \
             patch.object(C, "validate_witness_completion",
                          wraps=C.validate_witness_completion) as completion_call, \
             patch.object(C, "classify_a2", wraps=C.classify_a2) as classify_call:
            aggregate_commands, aggregate_outcome = D.consume_a2(record, binding_value,
                evidence, captures, before, after, inventory)
        self.assertEqual(aggregate_commands, commands)
        self.assertEqual(aggregate_outcome, expected_outcome)
        listing_call.assert_called_once()
        provenance_call.assert_called_once()
        completion_call.assert_called_once()
        classify_call.assert_called_once()

        # Listing suffix is a native contract input. The valid listing above
        # reaches the complete aggregate chain; change only one suffix and
        # require the accepted parser's roster refusal.
        malformed_listing = listing.replace(b": test\n", b": invalid\n", 1)
        with self.assertRaises(C.Rejection) as caught:
            C.validate_listing(malformed_listing, full_roster=full_roster,
                pilot_roster=pilot_roster, overlay_roster=overlay_roster,
                roster_sha256=C.ROSTER_PINS,
                evidence_contract="phase-b.original-a2-evidence.v1")
        self.assertIs(type(caught.exception), C.Rejection)
        self.assertIs(caught.exception.code, C.Code.ROSTER)

        # Keep the complete A2 record and all unrelated joins fixed, then
        # remove only --offline from the independently listed compile argv.
        changed_captures = deepcopy(captures)
        changed_captures[0]["start"]["argv"][3] = "--online"
        with patch.object(R, "pinned", side_effect=pinned_synthetic):
            self._expect_stop(lambda: D.consume_a2(record, binding_value,
                evidence, changed_captures, before, after, inventory),
                "b04_prior_command_semantics")

        changed_manifest = manifest_raw + b" "
        with self.assertRaises(C.Rejection) as caught:
            C.validate_provenance(compiler_log, depfile, listing, changed_manifest,
                admission_raw, pre_raw, binding=binding_raw,
                binding_sha256=binding_sha256, observed_source_hashes=source_hashes,
                observed_binary=binary, reference=reference)
        self.assertIs(type(caught.exception), C.Rejection)
        self.assertIs(caught.exception.code, C.Code.SOURCE)
        changed_binary = {**binary, "ino": "12"}
        with self.assertRaises(C.Rejection) as caught:
            C.validate_witness_completion(aggregate_witness, aggregate_completion_raw,
                provenance=provenance, observed_binary=changed_binary,
                observed_source_hashes=source_hashes,
                observed_depfile_sha256=hashlib.sha256(depfile).hexdigest(),
                reference=reference)
        self.assertIs(type(caught.exception), C.Rejection)
        self.assertIs(caught.exception.code, C.Code.BINARY)

    def test_b05_a2_classify01_direct_framing_uses_independent_synthetic_witnesses(self):
        self.assertEqual(C.PREFIX, _A2_PREFIX)
        self.assertEqual(C.CASES, _A2_CASE_ORACLE)
        self.assertEqual(C.BACKTRACE_NOTE, _A2_BACKTRACE_NOTE)
        self.assertEqual(C.ORIGINAL_EVIDENCE_CONTRACT, _A2_EVIDENCE_CONTRACT)
        store_root, raw, expected = _a2_direct_output()
        result = C.classify_a2(raw, exit_code=101, framing="direct", filtered_out=513,
            store_root=store_root, evidence_contract=_A2_EVIDENCE_CONTRACT,
            historical_prefix=())
        self.assertEqual(result, expected)
        self.assertEqual((result["passed"], result["failed"], result["filtered_out"]),
                         (3, 2, 513))
        self.assertTrue(result["not_product_acceptance"])

        duplicate_root, duplicate_raw, _ = _a2_direct_output(duplicate_case_identity=True)
        with self.assertRaises(C.Rejection) as caught:
            C.classify_a2(duplicate_raw, exit_code=101, framing="direct", filtered_out=513,
                store_root=duplicate_root, evidence_contract=_A2_EVIDENCE_CONTRACT,
                historical_prefix=())
        self.assertIs(type(caught.exception), C.Rejection)
        self.assertIs(caught.exception.code, C.Code.WITNESS)

        for oracle_mutation, expected_code in (
                ({"fixture_case_drift": True}, C.Code.FRAME),
                ({"panic_frame_drift": True}, C.Code.OUTCOME),
                ({"assertion_text_drift": True}, C.Code.OUTCOME)):
            changed_root, changed_raw, _unused_expected = _a2_direct_output(**oracle_mutation)
            with self.assertRaises(C.Rejection) as caught:
                C.classify_a2(changed_raw, exit_code=101, framing="direct", filtered_out=513,
                    store_root=changed_root, evidence_contract=_A2_EVIDENCE_CONTRACT,
                    historical_prefix=())
            self.assertIs(type(caught.exception), C.Rejection)
            self.assertIs(caught.exception.code, expected_code)

        with self.assertRaises(C.Rejection) as caught:
            C.classify_a2(raw, exit_code=101, framing="direct", filtered_out=512,
                store_root=store_root, evidence_contract=_A2_EVIDENCE_CONTRACT,
                historical_prefix=())
        self.assertIs(type(caught.exception), C.Rejection)
        self.assertIs(caught.exception.code, C.Code.SUMMARY)

    def test_b05_q_aggregate01_family_registry_is_checked_before_inventory(self):
        """A pinned synthetic prefix reaches the actual first aggregate registry guard."""
        state_root = "/fixture/q-aggregate-state"

        def trial(*, drift_first_family=False, drift_first_row_count=False,
                  over_total_row_cap=False):
            bytes_by_path = {}
            inventory_files = {}

            def pin_file(relative, raw):
                path = state_root + "/" + relative
                digest = hashlib.sha256(raw).hexdigest()
                bytes_by_path[path] = raw
                inventory_files[relative] = {"sha256": digest}
                return {"path": path, "sha256": digest}

            producer_raw = {}
            producer_rows = []
            immutable_files = []
            for role in sorted(D.Q_PRODUCERS):
                if role == "physical-predicates":
                    raw = _q_fixed_source_bytes("physical_predicates")
                elif role == "expected-rows":
                    raw = _q_fixed_source_bytes("expected_rows")
                elif role == "legacy-id-index":
                    raw = _q_fixed_source_bytes("legacy_id_index")
                else:
                    raw = ("synthetic pinned Q source fixture: " + role + "\n").encode("ascii")
                if role == "qualifier":
                    relative = "scripts/qualification/pbdep/qualify_whole_program.py"
                else:
                    relative = "producer/" + role + ".source"
                pin = pin_file(relative, raw)
                producer_raw[role] = (pin, raw)
                producer_rows.append({"role": role, **pin})
                immutable_files.append({"path": pin["path"], "sha256": pin["sha256"]})

            subject_raw = b"synthetic fixed subject module source\n"
            subject_pin = pin_file("subject/runtime.py", subject_raw)
            immutable_files.append({"path": subject_pin["path"],
                                    "sha256": subject_pin["sha256"]})
            subject_closure = [{"role": "runtime.py", **subject_pin}]
            producer_closure = producer_rows
            operator = {"instrument_sha256": {"runtime.py": subject_pin["sha256"]},
                        "resource_roots": []}
            record = {"program_sha256": "a" * 64,
                "phase_binding": {"path": state_root + "/phase-binding.json",
                                  "sha256": hashlib.sha256(b"fixed phase binding\n").hexdigest()},
                "state_directory": {"path": state_root},
                "_reference": {"operator": operator}}

            role_names = ("input", "oracle", "results", "fixture")
            roles = {name: {"path": state_root + "/evidence/" + name + ".json",
                            "sha256": "0" * 64} for name in role_names}
            review_pin = pin_file("evidence/review-pin.txt", b"independent Q profile review fixture\n")
            families = [{"family": family, "control_ids": [], "rows": []}
                for family in D.Q_FAMILIES]
            actual_families = [{"family": family, "rows": []}
                for family in D.Q_FAMILIES]
            if drift_first_family:
                families[0]["family"] = D.Q_FAMILIES[1]
            if drift_first_row_count:
                families[0]["rows"] = [{}]
            if over_total_row_cap:
                family_index = D.Q_FAMILIES.index("successor")
                source_pin = producer_raw["successor-controls"][0]
                rows = []
                observed_rows = []
                for index in range(2049):
                    control_id = "SUC" + str(index).zfill(4)
                    observation_id = control_id + "-1"
                    rows.append({"control_id": control_id,
                        "observation_id": observation_id, "kind": "VALUE_EQUAL",
                        "expected": {"value": {"tag": "none"}}, "inputs": [],
                        "oracle_source": source_pin})
                    observed_rows.append({"control_id": control_id,
                        "observation_id": observation_id, "kind": "VALUE_EQUAL",
                        "inputs_sha256": R.digest(R.encode([])),
                        "oracle_source_sha256": source_pin["sha256"],
                        "observed": {"value": {"tag": "none"}}})
                families[family_index]["control_ids"] = [
                    "SUC" + str(index).zfill(4) for index in range(2049)]
                families[family_index]["rows"] = rows
                actual_families[family_index]["rows"] = observed_rows
            oracle_value = {"schema_version": "pbdep.q-oracle.v1",
                "profile": D.Q_PROFILE,
                "subject_source_closure": subject_closure,
                "producer_source_closure": producer_closure,
                "families": families, "review_pin": review_pin}
            fixture_value = {"schema_version": "pbdep.q-fixture-root.v1", "role": "D22",
                "directory": {"path": state_root, "dev": "7", "ino": "11",
                              "uid": "1000", "mode": "448"}}
            raw_oracle = R.encode(oracle_value)
            roles["oracle"]["sha256"] = hashlib.sha256(raw_oracle).hexdigest()
            raw_fixture = R.encode(fixture_value)
            roles["fixture"]["sha256"] = hashlib.sha256(raw_fixture).hexdigest()

            environment = _environment()
            environment.pop("CARGO_TARGET_DIR")
            cwd = "/fixture/q-cwd"
            input_value = {"schema_version": "pbdep.q-input.v1", "profile": D.Q_PROFILE,
                "program_sha256": record["program_sha256"],
                "phase_binding_path": record["phase_binding"]["path"],
                "oracle": roles["oracle"],
                "subject_source_closure": subject_closure,
                "producer_source_closure": producer_closure,
                "cwd": cwd, "environment": environment, "tools": {},
                "fixture": roles["fixture"]}
            raw_input = R.encode(input_value)
            roles["input"]["sha256"] = hashlib.sha256(raw_input).hexdigest()
            raw_start = R.encode({"schema_version": "pbdep.b04-start.v1"})
            start_pin = pin_file("capture/qualification-START.json", raw_start)
            raw_record = R.encode({"schema_version": "pbdep.b04-raw.v1"})
            raw_pin = pin_file("capture/qualification-RAW.json", raw_record)
            result_value = {"schema_version": "pbdep.q-results.v1",
                "profile": D.Q_PROFILE,
                "oracle_sha256": roles["oracle"]["sha256"],
                "subject_source_closure_sha256": R.digest(R.encode(subject_closure)),
                "producer_source_closure_sha256": R.digest(R.encode(producer_closure)),
                "binding_sha256": record["phase_binding"]["sha256"],
                "input_sha256": roles["input"]["sha256"],
                "start_sha256": start_pin["sha256"], "raw_sha256": raw_pin["sha256"],
                "families": actual_families}
            raw_results = R.encode(result_value)
            roles["results"]["sha256"] = hashlib.sha256(raw_results).hexdigest()
            pin_file("evidence/input.json", raw_input)
            pin_file("evidence/oracle.json", raw_oracle)
            pin_file("evidence/fixture.json", raw_fixture)
            pin_file("evidence/results.json", raw_results)

            binding = {"cwd": cwd, "tmp": {"path": _TEMPORARY_ROOT},
                "environment": environment,
                "python": _FIXTURE_ROOT + "/tools/python3",
                "immutable_files": immutable_files,
                "data": {"oracle": roles["oracle"], "input": roles["input"]}}
            command = (binding["python"], "-B", "-I",
                producer_raw["qualifier"][0]["path"], "--evidence-input",
                roles["input"]["path"])
            before_hashes = {"runtime.py": subject_pin["sha256"]}
            before = {"tools": {}, "binary": None, "_hashes": before_hashes,
                "sources": {"runtime.py": subject_pin}}
            after = {"binary": None, "_hashes": dict(before_hashes),
                "sources": {"runtime.py": subject_pin}}
            captures = [{"start": {"argv": list(command), "cwd": cwd,
                "environment_sha256": R.digest(R.encode(environment)),
                "tools_sha256": R.digest(R.encode(before["tools"])),
                "sources_sha256": R.digest(R.encode(before_hashes))},
                "capture": {"start_monotonic_ns": "100", "end_monotonic_ns": "200"},
                "spec": {"start": start_pin, "raw": raw_pin}}]
            evidence = {"roles": roles}
            inventory = {"state_directory": {"path": state_root}, "files": inventory_files}

            def pinned(pin, cap=R.CONTROL_CAP):
                raw = bytes_by_path[pin["path"]]
                if hashlib.sha256(raw).hexdigest() != pin["sha256"] or len(raw) > cap:
                    raise AssertionError("synthetic Q evidence pin mismatch")
                return raw

            with patch.object(R, "pinned", side_effect=pinned), \
                 patch.object(R, "directory", side_effect=lambda spec, **_kwargs:
                    Path(spec["path"])):
                return D.consume_q(record, binding, evidence, captures, before, after, inventory)

        # All source/command/input joins and the first family pass are coherent.
        # The intentionally absent independent family inventory is the next
        # guard; changing one expected family identity must fail at the earlier
        # registry guard instead.
        self._expect_stop(lambda: trial(), "b04_q_inventory")
        self._expect_stop(lambda: trial(drift_first_family=True), "b04_q_registry")
        self._expect_stop(lambda: trial(drift_first_row_count=True), "b04_q_registry")
        self._expect_stop(lambda: trial(over_total_row_cap=True), "b04_q_total_rows")

    def test_b05_q_aggregate02_full_synthetic_chain_and_isolated_join_guards(self):
        """Exercise all aggregate families with fixed synthetic consumer records."""
        state_root = "/fixture/q-aggregate-complete"

        def trial(mutation=None, cardinality_family=None):
            bytes_by_path = {}
            inventory_files = {}

            def pin_file(relative, raw):
                path = state_root + "/" + relative
                digest = hashlib.sha256(raw).hexdigest()
                bytes_by_path[path] = raw
                inventory_files[relative] = {"sha256": digest}
                return {"path": path, "sha256": digest}

            def pin_json(relative, value):
                return pin_file(relative, R.encode(value))

            expected_document = json.loads(_q_fixed_source_bytes("expected_rows"))
            legacy = json.loads(_q_fixed_source_bytes("legacy_id_index"))
            generic_raw = b"independent aggregate row input\n"
            generic_input = pin_file("evidence/row-input.bin", generic_raw)
            fixture_value = {"schema_version": "pbdep.q-fixture-root.v1", "role": "D22",
                "directory": {"path": state_root, "dev": "7", "ino": "11",
                              "uid": "1000", "mode": "448"}}
            roles = {
                name: {"path": state_root + "/evidence/" + name + ".json",
                       "sha256": "0" * 64}
                for name in ("input", "oracle", "results", "fixture")}
            fixture_raw = R.encode(fixture_value)
            roles["fixture"]["sha256"] = hashlib.sha256(fixture_raw).hexdigest()
            bytes_by_path[roles["fixture"]["path"]] = fixture_raw
            inventory_files["evidence/fixture.json"] = {
                "sha256": roles["fixture"]["sha256"]}
            review_pin = pin_file("evidence/review-pin.txt",
                                  b"independent aggregate profile fixture\n")

            producer_raw = {}
            producer_closure = []
            immutable_files = []
            family_fragments = {}
            for family, role in D.Q_FRAGMENT_ROLES.items():
                fragment = ("fixed synthetic consumer assertion for " + family + "\n").encode("ascii")
                family_fragments[family] = fragment
            for role in sorted(D.Q_PRODUCERS):
                if role == "physical-predicates":
                    raw = _q_fixed_source_bytes("physical_predicates")
                    if mutation == "fixed-source":
                        # Rebind every fixture join below, but keep the owner's fixed pin.
                        raw += b"\n"
                elif role == "expected-rows":
                    raw = _q_fixed_source_bytes("expected_rows")
                elif role == "legacy-id-index":
                    raw = _q_fixed_source_bytes("legacy_id_index")
                else:
                    family = next((name for name, source_role in D.Q_FRAGMENT_ROLES.items()
                                   if source_role == role), None)
                    suffix = family_fragments[family] if family else b""
                    raw = ("synthetic pinned Q source fixture: " + role + "\n").encode("ascii") + suffix
                relative = ("scripts/qualification/pbdep/qualify_whole_program.py"
                    if role == "qualifier" else "producer/" + role + ".source")
                pin = pin_file(relative, raw)
                producer_raw[role] = (pin, raw)
                producer_closure.append({"role": role, **pin})
                immutable_files.append({"path": pin["path"], "sha256": pin["sha256"]})

            subject_raw = b"synthetic fixed subject module source\n"
            subject_pin = pin_file("subject/runtime.py", subject_raw)
            immutable_files.append({"path": subject_pin["path"],
                                    "sha256": subject_pin["sha256"]})
            subject_closure = [{"role": "runtime.py", **subject_pin}]
            operator = {"instrument_sha256": {"runtime.py": subject_pin["sha256"]},
                "resource_roots": [{"directory": fixture_value["directory"],
                                    "kind": "workspace"}]}
            record = {"program_sha256": "a" * 64,
                "phase_binding": {"path": state_root + "/phase-binding.json",
                    "sha256": hashlib.sha256(b"fixed phase binding\n").hexdigest()},
                "state_directory": {"path": state_root}, "_reference": {"operator": operator}}
            if mutation == "program-replay":
                record["program_sha256"] = "b" * 64

            raw_start = R.encode({"schema_version": "pbdep.b04-start.v1"})
            start_pin = pin_file("capture/qualification-START.json", raw_start)
            raw_record = R.encode({"schema_version": "pbdep.b04-raw.v1"})
            raw_pin = pin_file("capture/qualification-RAW.json", raw_record)
            capture_command = ("/fixture/tools/python3", "-B", "-I",
                producer_raw["qualifier"][0]["path"], "--evidence-input", roles["input"]["path"])
            captures = [{"start": {"argv": list(capture_command), "cwd": "/fixture/q-cwd",
                "environment_sha256": "0" * 64, "tools_sha256": "0" * 64,
                "sources_sha256": "0" * 64},
                "capture": {"start_monotonic_ns": "100", "end_monotonic_ns": "200"},
                "spec": {"start": start_pin, "raw": raw_pin}}]

            runtime_log = b"ordinary\n"
            runtime_log_pin = pin_file("evidence/runtime-ordinary.log", runtime_log)
            runtime_record = _supervisor_record()
            runtime_records = {}
            for case in D.Q_CASES:
                value = deepcopy(runtime_record)
                raw_log = runtime_log if case == "ordinary" else b""
                if case in ("detached", "killed-detached"):
                    value["reaped"] = [
                        {"pid": 42, "status": 0, "exit_code": 0, "at": 1.25},
                        {"pid": 43, "status": 0, "exit_code": (-9 if case == "killed-detached" else 0), "at": 1.30},
                        {"pid": 44, "status": 0, "exit_code": 0, "at": 1.35}]
                    value["event_counts"]["reaped"] = 3
                    value["leader_exit_monotonic"] = 1.3
                    value["pipe_eof_monotonic"] = 1.2
                    value["echild_monotonic"] = 1.4
                    if case == "killed-detached":
                        value["stop"] = "useful_time_cap"
                        value["signals"] = [
                            {"pid": 43, "signal": 15, "at": 1.31},
                            {"pid": 43, "signal": 9, "at": 1.32}]
                        value["event_counts"]["signals"] = 2
                elif case == "nonzero17":
                    value["leader_returncode"] = 17
                    value["reaped"][0]["status"] = 17 << 8
                    value["reaped"][0]["exit_code"] = 17
                elif case in ("postfork-error", "outer-error", "resource-error"):
                    value["leader_returncode"] = -15
                    value["reaped"][0]["status"] = 15
                    value["reaped"][0]["exit_code"] = -15
                    value["signals"] = [{"pid": 42, "signal": 15, "at": 1.31}]
                    value["event_counts"]["signals"] = 1
                    value["stop"] = "instrument_error"
                    if case == "postfork-error":
                        value["errors"] = [{"where": "postfork_setup", "type": "OSError"}]
                    elif case == "outer-error":
                        value["errors"] = [{"where": "outer_parent", "type": "KeyboardInterrupt"}]
                        value["resource_peak_sampled"] = 0
                    else:
                        value["errors"] = [{"where": "resources", "type": "TraversalError",
                            "pathname_class": "directory-scan", "errno": 13}]
                        value["resource_peak_sampled"] = 0
                    value["event_counts"]["errors"] = 1
                elif case == "metadata0":
                    value["metadata_overflow"] = True
                    value["stop"] = "metadata_overflow"
                    value["reaped"] = []
                    value["event_counts"]["reaped"] = 1
                elif case == "log32":
                    raw_log = b"x" * 32
                    value["stop"] = "log_cap_prefix_only"
                    value["log_bytes"] = len(raw_log)
                    value["discarded_log_bytes"] = 1
                value["log_bytes"] = len(raw_log)
                value["log_sha256"] = hashlib.sha256(raw_log).hexdigest()
                runtime_pin = pin_json("evidence/runtime-" + case + ".json", value)
                log_pin = pin_file("evidence/runtime-" + case + ".log", raw_log)
                runtime_records[case] = (runtime_pin, log_pin)

            qd_projection = expected_document["discovery_d_controls"]
            rd_projection = expected_document["replay_observations"]
            family_ids = {
                "wrapper": legacy["families"]["wrapper"]["id_order"],
                "original-contract": legacy["families"]["original_contract"]["result_ids"],
                "whole-program": legacy["families"]["whole_program"]["id_order"],
                "policy": legacy["families"]["policy"]["observed_case_ids"],
                "runtime-custody": legacy["families"]["runtime_custody"]["case_ids"],
                "pbdep-qd": [row["control_id"] for row in qd_projection],
                "pbdep-rd": [row["observation_id"] for row in rd_projection],
                "successor": ["SUC001"]}

            def make_row(family, control_id, observation_id, fixed_triple=None):
                source_role = D.Q_FRAGMENT_ROLES[family]
                source_pin, source_bytes = producer_raw[source_role]
                source_relative = "oracle/" + family + ".txt"
                source_pin = pin_file(source_relative, family_fragments[family])
                inputs = [generic_input]
                if family in ("wrapper", "original-contract"):
                    kind = "ASSERTION_RETURN"
                    expected = {"assertion_sha256": source_pin["sha256"], "return_kind": "none"}
                    observed = deepcopy(expected)
                elif family in ("whole-program", "policy", "successor"):
                    kind = "VALUE_EQUAL"
                    expected = {"value": {"tag": "none"}}
                    observed = deepcopy(expected)
                elif family == "runtime-custody":
                    kind = "RUNTIME_CASE"
                    runtime_pin, log_pin = runtime_records[control_id]
                    expected = {"case": control_id}
                    observed = {"case": control_id, "supervisor": runtime_pin, "raw_log": log_pin}
                else:
                    kind = "REFUSAL_TRIPLE"
                    expected = {**fixed_triple, "context": None}
                    observed = deepcopy(expected)
                    if control_id == "D22" and observation_id == "D22.1":
                        inputs = sorted([generic_input, roles["fixture"]], key=lambda pin: pin["path"])
                        preimage = {"schema_version": "pbdep.q-parent-preimage.v1",
                            "program_sha256": record["program_sha256"],
                            "binding_sha256": record["phase_binding"]["sha256"],
                            "case_id": "D22.1",
                            "parent_path": state_root + "/source/a", "parent_dev": "7",
                            "parent_ino": "11", "observed_ns": "120", "call_started_ns": "130"}
                        pre_pin = pin_json("evidence/d22-preimage.json", preimage)
                        context_expected = {"kind": "D22_MISSING",
                            "raw_token": "a/cancelled/../leaf.txt", "component": "cancelled",
                            "errno": 2, "parent_suffix": "source/a"}
                        context_observed = {"kind": "D22_MISSING",
                            "raw_token": "a/cancelled/../leaf.txt", "component": "cancelled",
                            "errno": 2, "parent_path": state_root + "/source/a",
                            "parent_dev": "7", "parent_ino": "11",
                            "pre_observation": pre_pin}
                        expected["context"] = context_expected
                        observed["context"] = context_observed
                row = {"control_id": control_id, "observation_id": observation_id,
                    "kind": kind, "expected": expected, "inputs": inputs,
                    "oracle_source": source_pin}
                result = {"control_id": control_id, "observation_id": observation_id,
                    "kind": kind, "inputs_sha256": R.digest(R.encode(inputs)),
                    "oracle_source_sha256": source_pin["sha256"], "observed": observed}
                return row, result

            oracle_families = []
            actual_families = []
            for family in D.Q_FAMILIES:
                ids = family_ids[family]
                expected_rows = []
                actual_rows = []
                if family in ("pbdep-qd", "pbdep-rd"):
                    projected = ([(item["control_id"], observation["observation_id"],
                        observation["expected"])
                        for item in qd_projection for observation in item["observations"]]
                        if family == "pbdep-qd" else
                        [(item["observation_id"], item["observation_id"], item["expected"])
                         for item in rd_projection])
                    for control_id, observation_id, triple in projected:
                        row, observed = make_row(family, control_id, observation_id, triple)
                        expected_rows.append(row); actual_rows.append(observed)
                else:
                    for control_id in ids:
                        row, observed = make_row(family, control_id, control_id + ".1")
                        expected_rows.append(row); actual_rows.append(observed)
                oracle_families.append({"family": family, "control_ids": ids,
                                        "rows": expected_rows})
                actual_families.append({"family": family, "rows": actual_rows})

            if mutation in ("family-missing", "family-extra"):
                family_index = ("wrapper", "original-contract", "whole-program", "policy",
                    "runtime-custody", "pbdep-qd", "pbdep-rd", "successor").index(cardinality_family)
                self.assertEqual(oracle_families[family_index]["family"], cardinality_family)
                self.assertEqual(actual_families[family_index]["family"], cardinality_family)
                if mutation == "family-missing":
                    oracle_families.pop(family_index)
                    actual_families.pop(family_index)
                else:
                    oracle_families.append(deepcopy(oracle_families[family_index]))
                    actual_families.append(deepcopy(actual_families[family_index]))

            if mutation == "family-order":
                oracle_families[0], oracle_families[1] = oracle_families[1], oracle_families[0]
                actual_families[0], actual_families[1] = actual_families[1], actual_families[0]
            elif mutation == "family-row-count":
                actual_families[0]["rows"].pop()
            elif mutation == "row-identity":
                actual_families[0]["rows"][0]["control_id"] = "WQ01.changed"
            elif mutation == "expected-row-extra":
                oracle_families[0]["rows"][0]["extra"] = False
            elif mutation == "observed-row-extra":
                actual_families[0]["rows"][0]["extra"] = False
            elif mutation == "oversized-row":
                row = oracle_families[-1]["rows"][0]
                observed = actual_families[-1]["rows"][0]
                row["expected"] = {"value": {"tag": "text", "value": "x" * R.RECEIPT_CAP}}
                observed["observed"] = deepcopy(row["expected"])

            oracle_value = {"schema_version": "pbdep.q-oracle.v1", "profile": D.Q_PROFILE,
                "subject_source_closure": subject_closure,
                "producer_source_closure": producer_closure,
                "families": oracle_families, "review_pin": review_pin}
            if mutation == "oracle-extra-key":
                oracle_value["unlisted"] = False
            raw_oracle = R.encode(oracle_value)
            roles["oracle"]["sha256"] = hashlib.sha256(raw_oracle).hexdigest()
            bytes_by_path[roles["oracle"]["path"]] = raw_oracle
            inventory_files["evidence/oracle.json"] = {"sha256": roles["oracle"]["sha256"]}

            environment = _environment()
            environment.pop("CARGO_TARGET_DIR")
            cwd = "/fixture/q-cwd"
            input_value = {"schema_version": "pbdep.q-input.v1", "profile": D.Q_PROFILE,
                "program_sha256": "a" * 64,
                "phase_binding_path": record["phase_binding"]["path"],
                "oracle": roles["oracle"], "subject_source_closure": subject_closure,
                "producer_source_closure": producer_closure, "cwd": cwd,
                "environment": environment, "tools": {}, "fixture": roles["fixture"]}
            if mutation == "environment-replay":
                input_value["environment"]["LC_ALL"] = "C.UTF-8"
            raw_input = R.encode(input_value)
            roles["input"]["sha256"] = hashlib.sha256(raw_input).hexdigest()
            bytes_by_path[roles["input"]["path"]] = raw_input
            inventory_files["evidence/input.json"] = {"sha256": roles["input"]["sha256"]}

            result_value = {"schema_version": "pbdep.q-results.v1", "profile": D.Q_PROFILE,
                "oracle_sha256": roles["oracle"]["sha256"],
                "subject_source_closure_sha256": R.digest(R.encode(subject_closure)),
                "producer_source_closure_sha256": R.digest(R.encode(producer_closure)),
                "binding_sha256": record["phase_binding"]["sha256"],
                "input_sha256": roles["input"]["sha256"],
                "start_sha256": start_pin["sha256"], "raw_sha256": raw_pin["sha256"],
                "families": actual_families}
            if mutation == "result-join":
                result_value["raw_sha256"] = "0" * 64
            if mutation == "result-extra-key":
                result_value["unlisted"] = False
            raw_results = R.encode(result_value)
            roles["results"]["sha256"] = hashlib.sha256(raw_results).hexdigest()
            bytes_by_path[roles["results"]["path"]] = raw_results
            inventory_files["evidence/results.json"] = {"sha256": roles["results"]["sha256"]}
            pin_file("evidence/fixture.json", fixture_raw)

            binding_environment = environment if mutation != "environment-replay" else _environment()
            binding_environment.pop("CARGO_TARGET_DIR", None)
            binding = {"cwd": cwd, "tmp": {"path": _TEMPORARY_ROOT},
                "environment": binding_environment, "python": "/fixture/tools/python3",
                "immutable_files": immutable_files,
                "data": {"oracle": roles["oracle"], "input": roles["input"]}}
            capture_command = (binding["python"], "-B", "-I",
                producer_raw["qualifier"][0]["path"], "--evidence-input", roles["input"]["path"])
            captures[0]["start"].update({"argv": list(capture_command), "cwd": cwd,
                "environment_sha256": R.digest(R.encode(binding_environment)),
                "tools_sha256": R.digest(R.encode({})),
                "sources_sha256": R.digest(R.encode({"runtime.py": subject_pin["sha256"]}))})
            if mutation == "tool-replay":
                captures[0]["start"]["tools_sha256"] = "0" * 64
            inventory = {"state_directory": {"path": state_root}, "files": inventory_files}

            def pinned(pin, cap=R.CONTROL_CAP):
                raw = bytes_by_path[pin["path"]]
                if hashlib.sha256(raw).hexdigest() != pin["sha256"] or len(raw) > cap:
                    raise AssertionError("synthetic Q evidence pin mismatch")
                return raw

            before_hashes = {"runtime.py": subject_pin["sha256"]}
            before = {"tools": {}, "binary": None, "_hashes": before_hashes,
                "sources": {"runtime.py": subject_pin}}
            after = {"binary": None, "_hashes": dict(before_hashes),
                "sources": {"runtime.py": subject_pin}}
            if mutation == "source-replay":
                operator["instrument_sha256"]["runtime.py"] = "f" * 64
            evidence = {"roles": roles}
            with patch.object(R, "pinned", side_effect=pinned), \
                 patch.object(R, "directory", side_effect=lambda spec, **_kwargs:
                    Path(spec["path"])):
                return D.consume_q(record, binding, evidence, captures, before, after, inventory)

        expected_command = ("/fixture/tools/python3", "-B", "-I",
            state_root + "/scripts/qualification/pbdep/qualify_whole_program.py",
            "--evidence-input", state_root + "/evidence/input.json")
        self.assertEqual(trial(), ((expected_command,),
            {"scope": "current-synthetic-qualification-evidence-only", "rows": 972}))
        self._expect_stop(lambda: trial("family-order"), "b04_q_registry")
        self._expect_stop(lambda: trial("family-row-count"), "b04_q_registry")
        self._expect_rejection(lambda: trial("row-identity"), "PHASE_DATA")
        self._expect_stop(lambda: trial("oracle-extra-key"), "record_shape")
        self._expect_stop(lambda: trial("expected-row-extra"), "record_shape")
        self._expect_stop(lambda: trial("observed-row-extra"), "record_shape")
        self._expect_stop(lambda: trial("program-replay"), "b04_q_input")
        self._expect_stop(lambda: trial("environment-replay"), "b04_environment_literal")
        self._expect_stop(lambda: trial("tool-replay"), "b04_prior_command_semantics")
        self._expect_stop(lambda: trial("source-replay"), "b04_q_subject_role")
        self._expect_stop(lambda: trial("fixed-source"), "b04_q_fixed_source")
        self._expect_stop(lambda: trial("result-join"), "b04_q_result_join")
        self._expect_stop(lambda: trial("result-extra-key"), "record_shape")
        self._expect_stop(lambda: trial("oversized-row"), "b04_q_row_cap")

        # Local SC01 cardinality subcases; the global ID/selector inventory is unchanged.
        cardinality_cases = (
            ("SC01-MISSING-01", "family-missing", "wrapper"),
            ("SC01-MISSING-02", "family-missing", "original-contract"),
            ("SC01-MISSING-03", "family-missing", "whole-program"),
            ("SC01-MISSING-04", "family-missing", "policy"),
            ("SC01-MISSING-05", "family-missing", "runtime-custody"),
            ("SC01-MISSING-06", "family-missing", "pbdep-qd"),
            ("SC01-MISSING-07", "family-missing", "pbdep-rd"),
            ("SC01-MISSING-08", "family-missing", "successor"),
            ("SC01-EXTRA-01", "family-extra", "wrapper"),
            ("SC01-EXTRA-02", "family-extra", "original-contract"),
            ("SC01-EXTRA-03", "family-extra", "whole-program"),
            ("SC01-EXTRA-04", "family-extra", "policy"),
            ("SC01-EXTRA-05", "family-extra", "runtime-custody"),
            ("SC01-EXTRA-06", "family-extra", "pbdep-qd"),
            ("SC01-EXTRA-07", "family-extra", "pbdep-rd"),
            ("SC01-EXTRA-08", "family-extra", "successor"))
        for case_id, mutation, family in cardinality_cases:
            with self.subTest(case=case_id, family=family):
                self._expect_stop(
                    lambda mutation=mutation, family=family: trial(mutation, family),
                    "b04_q_families")

    def test_b05_env10_prior_q_input_environment_is_checked_before_oracle_consumption(self):
        class ReachedOracle(Exception):
            pass

        roles = {name: _pin(name) for name in ("input", "oracle", "results", "fixture")}
        record = {"program_sha256": "a" * 64,
                  "phase_binding": {"path": "/fixture/q-phase-binding.json"}}
        environment = _environment()
        del environment["CARGO_TARGET_DIR"]
        binding = {"cwd": "/fixture/q-cwd", "tmp": {"path": _TEMPORARY_ROOT},
            "environment": environment,
            "data": {"oracle": roles["oracle"], "input": roles["input"]}}
        input_value = {"schema_version": "pbdep.q-input.v1", "profile": D.Q_PROFILE,
            "program_sha256": record["program_sha256"],
            "phase_binding_path": record["phase_binding"]["path"],
            "oracle": roles["oracle"],
            "subject_source_closure": {}, "producer_source_closure": {},
            "fixture": roles["fixture"], "cwd": binding["cwd"],
            "environment": environment, "tools": {}}
        evidence = {"roles": roles}
        before = {"tools": {}, "binary": None}
        after = {"binary": None}
        calls = []

        def read_json(_spec, _inventory):
            calls.append(None)
            if len(calls) == 1:
                return input_value
            raise ReachedOracle

        with patch.object(D, "evidence_json", side_effect=read_json):
            with self.assertRaises(ReachedOracle):
                D.consume_q(record, binding, evidence, [], before, after, {})
        self.assertEqual(len(calls), 2)

        invalid = deepcopy(input_value)
        invalid["environment"]["CARGO_TARGET_DIR"] = _FIXTURE_ROOT + "/target"
        calls.clear()
        with patch.object(D, "evidence_json", side_effect=lambda *_args: (calls.append(None), invalid)[1]):
            self._expect_stop(lambda: D.consume_q(record, binding, evidence, [], before, after, {}),
                              "record_shape")
        self.assertEqual(len(calls), 1)

    def test_b05_entry01_initial_request_rejection_precedes_any_state_write(self):
        # A valid independent request must pass the real initial request guard
        # and a real anchored GENESIS consumer before the custody constructor
        # sentinel.  The sentinel is only an entry-boundary observation; it is
        # not discovery, START, or witness evidence.
        class ReachedCustodyBoundary(Exception):
            pass

        positive_request = _request()
        positive_reservation = _reservation()
        positive_reservation["chain_id"] = positive_request["chain_id"]
        positive_reservation["anchor"] = {"wall_ns": "100000000000",
            "monotonic_ns": "1000000", "boot_id": "synthetic-boot-id"}
        positive_reference = QW._reference()
        positive_state = positive_request["state_directory"]
        positive_inventory = {"state_directory": positive_state, "files": {},
            "directories": {}, "total_bytes": 0}
        positive_prior = {"schema_version": "pbdep.b04-prior.v2", "kind": "GENESIS",
            "program_sha256": positive_reservation["program_sha256"],
            "chain_id": positive_reservation["chain_id"],
            "reservation_sha256": R.digest(R.encode(positive_reservation)),
            "state_directory": positive_state, "resources": _history(),
            "terminal_captures": [], "inventory": positive_inventory,
            "phase_history": []}
        positive_pins = _request_pins()
        read_values = {positive_pins["reference"]["path"]: positive_reference,
            positive_pins["reservation"]["path"]: positive_reservation,
            positive_pins["prior"]["path"]: positive_prior}
        custody_arguments = []

        def read_positive(pin):
            return deepcopy(read_values[pin["path"]])

        def pinned_positive(pin, *_args, **_kwargs):
            if pin["path"] == positive_pins["reservation"]["path"]:
                return R.encode(positive_reservation)
            raise AssertionError("unexpected positive entry pin read")

        def custody_sentinel(*args):
            custody_arguments.append(args)
            raise ReachedCustodyBoundary()

        with patch.object(E, "external_inputs", return_value=positive_request), \
             patch.object(E.time, "monotonic_ns", return_value=1_100_000), \
             patch.object(D, "read_json", side_effect=read_positive), \
             patch.object(D, "state_inventory", return_value=positive_inventory), \
             patch.object(R, "pinned", side_effect=pinned_positive), \
             patch.object(D, "Custody", side_effect=custody_sentinel):
            with self.assertRaises(ReachedCustodyBoundary):
                E._discover(request_pin=_pin("request"), grant_pin=positive_pins["grant"],
                    reference_pin=positive_pins["reference"],
                    reservation_pin=positive_pins["reservation"],
                    prior_pin=positive_pins["prior"])
        self.assertEqual(len(custody_arguments), 1)
        self.assertEqual(custody_arguments[0][0], positive_request)
        self.assertEqual(custody_arguments[0][1], positive_reference)
        self.assertEqual(custody_arguments[0][2], positive_reservation)
        self.assertEqual(custody_arguments[0][3], "discovery")
        self.assertEqual(custody_arguments[0][4], 1_100_000)
        self.assertEqual(custody_arguments[0][5], positive_prior["resources"])

        with tempfile.TemporaryDirectory(prefix="pbdep-b05-entry01-", dir=_TMP_ROOT) as parent:
            request = _request()
            request["target_root"] = request["source_root"]
            state_path = Path(parent) / "state"
            request["state_directory"] = {"path": str(state_path)}
            kwargs = {"request_pin": _pin("request"),
                "grant_pin": _request_pins()["grant"],
                "reference_pin": _request_pins()["reference"],
                "reservation_pin": _request_pins()["reservation"],
                "prior_pin": _request_pins()["prior"]}
            with patch.object(E, "external_inputs", return_value=request), \
                 patch.object(E.time, "monotonic_ns", return_value=1):
                self._expect_stop(lambda: E._discover(**kwargs), "b04_root_overlap")
            self.assertFalse(state_path.exists())

    def test_b05_entry02_refresh_refusal_emits_no_start_receipt(self):
        context = object.__new__(D.Custody)
        context.lock = object()

        def reject_refresh():
            raise R.Stop("synthetic-refresh-rejection")

        context.refresh = reject_refresh
        with patch.object(R, "persist") as persist:
            self._expect_stop(lambda: D.Custody.capture(
                context, "compile", ("/fixture/cargo",), lambda: None),
                "synthetic-refresh-rejection")
            persist.assert_not_called()

    def test_b05_entry03_initial_environment_refusal_precedes_writes_on_both_paths(self):
        mutations = (
            ("missing-target", lambda request: request["environment"].pop("CARGO_TARGET_DIR"),
             "stop", "record_shape"),
            ("extra-benign", lambda request: request["environment"].update(
                {"UNDECLARED": "fixture-only"}), "stop", "record_shape"),
            ("credential-name-only", lambda request: request["environment"].update(
                {"AWS_SECRET_ACCESS_KEY": "fixture-only"}), "stop", "record_shape"),
            ("wrong-literal", lambda request: request["environment"].update(
                {"CARGO_NET_OFFLINE": "false"}), "stop", "b04_environment_literal"),
            ("target-drift", lambda request: request["environment"].update(
                {"CARGO_TARGET_DIR": _FIXTURE_ROOT + "/drifted-target"}),
             "stop", "b04_environment_target"),
            ("source-path-grammar", lambda request: request.update(
                {"source_root": "/fixture/../escaped"}), "rejection", "PATH"),
            ("home-path-grammar", lambda request: request["environment"].update(
                {"HOME": "/fixture/../escaped"}), "rejection", "PATH"),
        )
        for caller in ("discover", "adopt"):
            for label, mutate, kind, code in mutations:
                with tempfile.TemporaryDirectory(
                        prefix="pbdep-b05-entry03-", dir=_TMP_ROOT) as parent:
                    invalid = _request()
                    mutate(invalid)
                    state_path = Path(parent) / (caller + "-" + label)
                    invalid["state_directory"] = {"path": str(state_path)}
                    if caller == "discover":
                        kwargs = {"request_pin": _pin("request"),
                            "grant_pin": _request_pins()["grant"],
                            "reference_pin": _request_pins()["reference"],
                            "reservation_pin": _request_pins()["reservation"],
                            "prior_pin": _request_pins()["prior"]}
                        with patch.object(E, "external_inputs", return_value=invalid), \
                             patch.object(E.time, "monotonic_ns", return_value=1):
                            call = lambda: E._discover(**kwargs)
                            (self._expect_stop if kind == "stop" else self._expect_rejection)(
                                call, code)
                    else:
                        chain = "b" * 64
                        reservation = _reservation()
                        reservation["chain_id"] = chain
                        adopt_request = {"chain_id": chain,
                            "reservation": _request_pins()["reservation"],
                            "prior": _request_pins()["prior"],
                            "discovery_request": _pin("discovery-request"),
                            "discovery_result": _pin("discovery-result"),
                            "phase_binding": _pin("phase-binding")}
                        freeze = {"schema_version": "pbdep.b04-freeze.v1",
                            "chain_id": chain,
                            "program_sha256": reservation["program_sha256"],
                            "reservation": adopt_request["reservation"],
                            "prior": adopt_request["prior"],
                            "discovery_request": adopt_request["discovery_request"],
                            "discovery_result": adopt_request["discovery_result"],
                            "discovery_close": _pin("discovery-close"),
                            "state_directory": invalid["state_directory"],
                            "retained_inventory": {}, "resources": _history(),
                            "expected": {}}
                        freeze_raw = R.encode(freeze)
                        with patch.object(E.R, "pinned", return_value=freeze_raw), \
                             patch.object(E, "external_inputs", return_value=invalid):
                            call = lambda: E._adopt(request=adopt_request,
                                request_pin=_pin("adopt-request"), freeze_pin=_pin("freeze"),
                                reference={}, reservation=reservation, started=1)
                            (self._expect_stop if kind == "stop" else self._expect_rejection)(
                                call, code)
                    self.assertFalse(state_path.exists())

    def test_b05_prior01_anchored_v2_genesis_joins_complete_inventory(self):
        reservation = _reservation()
        reservation["chain_id"] = "b" * 64
        reservation["anchor"] = {"wall_ns": "100000000000", "monotonic_ns": "1000000",
                                 "boot_id": "synthetic-boot-id"}
        observed_entry_ns = 1_100_000
        state_spec = {"path": _FIXTURE_ROOT + "/prior-state"}
        directory_record = {"path": state_spec["path"] + "/discovery", "dev": "7",
            "ino": "12", "uid": "1000", "mode": "448"}
        inventory = {"state_directory": state_spec,
            "files": {}, "directories": {}, "total_bytes": 0}
        value = {"schema_version": "pbdep.b04-prior.v2", "kind": "GENESIS",
            "program_sha256": reservation["program_sha256"], "chain_id": reservation["chain_id"],
            "reservation_sha256": R.digest(R.encode(reservation)), "state_directory": state_spec,
            "resources": _history(), "terminal_captures": [], "inventory": inventory,
            "phase_history": []}
        with patch.object(D, "read_json", return_value=value), \
             patch.object(D, "state_inventory", return_value=inventory):
            self.assertIs(D.prior_history(_request_pins()["prior"], reservation=reservation,
                state_spec=state_spec, observed_entry_ns=observed_entry_ns), value)
        drift_cases = []
        file_record = {"path": state_spec["path"] + "/retained.json",
            "sha256": "a" * 64, "dev": "7", "ino": "11", "size": "1",
            "mtime_ns": "1", "mode": "384", "uid": "1000", "nlink": "1"}
        added_file = deepcopy(value)
        added_file["inventory"]["files"]["retained.json"] = file_record
        added_file["inventory"]["total_bytes"] = 1
        drift_cases.append(added_file)
        added_directory = deepcopy(value)
        added_directory["inventory"]["directories"]["added"] = {
            **directory_record, "path": state_spec["path"] + "/added"}
        drift_cases.append(added_directory)
        sampled = deepcopy(value)
        sampled["resources"] = _history(sample_count=1)
        drift_cases.append(sampled)
        for drifted in drift_cases:
            with patch.object(D, "read_json", return_value=drifted), \
                 patch.object(D, "state_inventory", return_value=drifted["inventory"]):
                self._expect_stop(lambda: D.prior_history(_request_pins()["prior"],
                    reservation=reservation, state_spec=state_spec,
                    observed_entry_ns=observed_entry_ns), "b04_prior_genesis")
        upper = D.master_interval(reservation)[1]
        with patch.object(D, "read_json", return_value=value), \
             patch.object(D, "state_inventory", return_value=inventory):
            self._expect_stop(lambda: D.prior_history(_request_pins()["prior"],
                reservation=reservation, state_spec=state_spec,
                observed_entry_ns=upper + 1), "b04_history_master_interval")

    def test_b05_prior02_anchored_interval_and_capture_order_edges(self):
        reservation = _reservation()
        reservation["anchor"] = {"wall_ns": "100000000000",
            "monotonic_ns": "1000000", "boot_id": "synthetic-boot-id"}
        lower, upper = D.master_interval(reservation)
        self.assertEqual(lower, 1_000_000)
        D.chronological_interval(lower, lower, reservation=reservation,
                                observed_entry_ns=lower)
        D.chronological_interval(upper, upper, reservation=reservation,
                                observed_entry_ns=upper)
        for begin, end, observed in (
                (lower - 1, lower, lower),
                (lower, upper, upper + 1),
                (upper, upper, upper + 1),
                (lower + 1, lower, upper)):
            self._expect_stop(lambda begin=begin, end=end, observed=observed:
                D.chronological_interval(begin, end, reservation=reservation,
                    observed_entry_ns=observed), "b04_history_master_interval")
        bad_anchor = deepcopy(reservation)
        bad_anchor["anchor"]["wall_ns"] = str(
            bad_anchor["master_expiry_utc_seconds"] * D.NANO)
        self._expect_stop(lambda: D.master_interval(bad_anchor),
                          "b04_history_master_anchor")

        binding = {"cwd": "/fixture/source", "environment": {"LC_ALL": "C"}}
        tools = {"python": {"path": "/fixture/tools/python3", "sha256": "a" * 64}}
        sources = {"runtime.py": "b" * 64}
        before = {"tools": tools, "_hashes": sources}
        commands = (("/fixture/tools/python3", "-B", "-I"),
                    ("/fixture/tools/python3", "-B", "-I", "--second"))
        captures = []
        for argv in commands:
            captures.append({"start": {"argv": list(argv), "cwd": binding["cwd"],
                "environment_sha256": R.digest(R.encode(binding["environment"])),
                "tools_sha256": R.digest(R.encode(tools)),
                "sources_sha256": R.digest(R.encode(sources))}})
        D.verify_prior_commands(binding, captures, commands, before)
        self._expect_stop(lambda: D.verify_prior_commands(binding,
            list(reversed(captures)), commands, before), "b04_prior_command_semantics")

    def test_b05_prior03_discovery_inventory_is_a_fixed_observation(self):
        reservation = _reservation()
        reservation["chain_id"] = "c" * 64
        reservation["anchor"] = {"wall_ns": "100000000000",
            "monotonic_ns": "1000000", "boot_id": "synthetic-boot-id"}
        observed_entry_ns = 1_100_000
        state_spec = {"path": _FIXTURE_ROOT + "/prior-state"}
        prior_inventory = {"state_directory": state_spec, "files": {},
            "directories": {}, "total_bytes": 0}
        discovery_inventory = {"state_directory": state_spec,
            "files": {"retained.json": {"path": state_spec["path"] + "/retained.json",
                "sha256": "a" * 64, "dev": "7", "ino": "11", "size": "1",
                "mtime_ns": "1", "mode": "384", "uid": "1000", "nlink": "1"}},
            "directories": {"discovery": {"path": state_spec["path"] + "/discovery",
                "dev": "7", "ino": "12", "uid": "1000", "mode": "448"}},
            "total_bytes": 1}
        value = {"schema_version": "pbdep.b04-prior.v2", "kind": "GENESIS",
            "program_sha256": reservation["program_sha256"],
            "chain_id": reservation["chain_id"],
            "reservation_sha256": R.digest(R.encode(reservation)),
            "state_directory": state_spec, "resources": _history(),
            "terminal_captures": [], "inventory": prior_inventory,
            "phase_history": []}
        with patch.object(D, "read_json", return_value=value), \
             patch.object(D, "state_inventory", return_value=deepcopy(discovery_inventory)):
            self.assertIs(D.prior_history(_request_pins()["prior"], reservation=reservation,
                state_spec=state_spec, observed_entry_ns=observed_entry_ns,
                discovery_inventory=discovery_inventory), value)

        changed = deepcopy(discovery_inventory)
        changed["files"]["retained.json"]["sha256"] = "b" * 64
        added_file = deepcopy(discovery_inventory)
        added_file["files"]["added.json"] = {**discovery_inventory["files"]["retained.json"],
            "path": state_spec["path"] + "/added.json"}
        removed_file = deepcopy(discovery_inventory)
        del removed_file["files"]["retained.json"]
        added_directory = deepcopy(discovery_inventory)
        added_directory["directories"]["added"] = {
            **discovery_inventory["directories"]["discovery"],
            "path": state_spec["path"] + "/added"}
        removed_directory = deepcopy(discovery_inventory)
        del removed_directory["directories"]["discovery"]
        for drifted in (changed, added_file, removed_file, added_directory, removed_directory):
            with patch.object(D, "read_json", return_value=value), \
                 patch.object(D, "state_inventory", return_value=drifted):
                self._expect_stop(lambda: D.prior_history(_request_pins()["prior"],
                    reservation=reservation, state_spec=state_spec,
                    observed_entry_ns=observed_entry_ns,
                    discovery_inventory=discovery_inventory), "b04_prior_inventory")

    def test_b05_prior04_multiphase_records_reach_isolated_overlap_guard(self):
        """Supplied ordered phase records reach lineage before overlap mutation."""
        def fixture(*, overlap=False, first_start=1_000_001,
                    second_start=None, second_end=1_000_004,
                    phase_order_drift=False, capture_order_drift=False,
                    capture_overlap=False, capture_outside_phase=False,
                    capture_after_entry=False, raw_schema_drift=False,
                    raw_event_variant=None):
            reservation = _reservation()
            reservation["chain_id"] = "e" * 64
            reservation["anchor"] = {"wall_ns": "100000000000",
                "monotonic_ns": "1000000", "boot_id": "synthetic-boot-id"}
            state_spec = {"path": "/fixture/prior-state"}
            policy = {key: deepcopy(reservation[key]) for key in (
                "schema_version", "program_sha256", "master_start_utc_seconds",
                "master_expiry_utc_seconds", "exit_seconds", "slots", "phases")}
            policy["phases"] = {name: {**deepcopy(item), "slots": list(item["slots"])}
                for name, item in reservation["phases"].items()}
            raw_by_path = {}
            objects_by_path = {}
            files = {}

            def retain(name, value, raw=None):
                path = state_spec["path"] + "/" + name
                encoded = R.encode(value) if raw is None else raw
                pin = {"path": path, "sha256": hashlib.sha256(encoded).hexdigest()}
                raw_by_path[path] = encoded
                objects_by_path[path] = deepcopy(value)
                relative = str(Path(path).relative_to(state_spec["path"]))
                files[relative] = {"path": path, "sha256": pin["sha256"],
                    "dev": "7", "ino": str(100 + len(files)),
                    "size": str(len(encoded)), "mtime_ns": "1", "mode": "384",
                    "uid": "1000", "nlink": "1"}
                return pin

            policy_pin = retain("policy.json", policy)
            phases = []
            captures = []
            previous_record_pin = None
            operator = QW._operator()
            for index, phase in enumerate(W.PHASES):
                binding_raw = QW._phase_binding_raw(phase, {"synthetic": phase})
                binding_pin = retain(phase + "-binding.json", {}, raw=binding_raw)
                operator = deepcopy(operator)
                operator["phase_bindings"][phase] = {"state": "BOUND", **binding_pin}
                reference = QW._reference(operator=operator)
                reference_pin = retain(phase + "-reference.json", reference)
                semantic = {"schema_version": "pbdep.prior-semantic.v1",
                    "phase": phase, "synthetic_fixture": True}
                semantic_pin = retain(phase + "-semantic.json", semantic)
                artifact_pin = retain(phase + "-artifact.json",
                    {"schema_version": "pbdep.synthetic-artifact.v1", "phase": phase})
                declared_slots = ("Q",) if phase == "qualification" else \
                    tuple(policy["phases"][phase]["slots"])
                phase_duration = len(declared_slots)
                start_ns = first_start + index * 100
                if index == 1:
                    start_ns = (first_start if overlap else first_start + 100) \
                        if second_start is None else second_start
                end_ns = start_ns + phase_duration
                if index == 1 and second_end != 1_000_004:
                    end_ns = second_end
                elapsed_ns = phase_duration
                capture_artifacts = []
                for slot_index, slot in enumerate(declared_slots):
                    tick = start_ns + slot_index
                    if capture_overlap and len(captures) == 2:
                        tick = int(captures[-1]["start"]["recorded_monotonic_ns"])
                    if capture_outside_phase and not captures:
                        tick = start_ns + phase_duration + 1
                    if capture_after_entry and not captures:
                        tick = 1_100_001
                    log = b"synthetic prior capture bytes\n"
                    expected_exit = 101 if phase == "a2" and slot == "3" else 0
                    supervisor = _supervisor_record()
                    if raw_event_variant == "reaped-bool" and not captures:
                        supervisor["reaped"][0]["at"] = True
                    elif raw_event_variant in ("signal-valid", "signal-negative",
                                               "signal-infinity") and not captures:
                        event_at = (1.5 if raw_event_variant == "signal-valid" else
                                    -0.1 if raw_event_variant == "signal-negative" else float("inf"))
                        supervisor["signals"] = [{"pid": 42, "signal": 15, "at": event_at}]
                        supervisor["event_counts"]["signals"] = 1
                    elif raw_event_variant == "reaped-nan" and not captures:
                        supervisor["reaped"][0]["at"] = float("nan")
                    supervisor["leader_returncode"] = expected_exit
                    supervisor["reaped"][0]["status"] = expected_exit
                    supervisor["reaped"][0]["exit_code"] = expected_exit
                    supervisor["log_bytes"] = len(log)
                    supervisor["log_sha256"] = hashlib.sha256(log).hexdigest()
                    start = {"schema_version": "pbdep.prior-slot-start.v2",
                        "phase": phase, "slot": slot, "argv": ["/fixture/tools/python3"],
                        "cwd": "/fixture/prior-source", "environment_sha256": "a" * 64,
                        "tools_sha256": "b" * 64, "sources_sha256": "c" * 64,
                        "reference_sha256": hashlib.sha256(R.encode(reference)).hexdigest(),
                        "binding_sha256": binding_pin["sha256"],
                        "useful_seconds": 1, "exit_seconds": 60, "raw_cap": 4096,
                        "recorded_monotonic_ns": str(tick),
                        "slot_spent_even_if_fork_fails": True}
                    raw = {"schema_version": "pbdep.raw-custody.v1", "slot": slot,
                        "wrapper_before_run_monotonic_ns": str(tick),
                        "wrapper_after_run_monotonic_ns": str(tick + 1),
                        "boundary_is_not_exact_fork": True,
                        "stream": "merged-stdout-stderr",
                        "manager_status": "NOT_OBSERVED_BY_ADAPTER",
                        "aggregate_resource_state_at_terminal_write": "UNKNOWN",
                        "supervisor": supervisor, "resource_samples": _history(),
                        "last_sampled_resources": None}
                    if raw_schema_drift and not captures:
                        raw["aggregate_resource_status_at_terminal_write"] = \
                            raw.pop("aggregate_resource_state_at_terminal_write")
                    start_pin = retain("capture/" + phase + "-" + slot + "-START.json", start)
                    raw_bytes = None
                    if raw_event_variant in ("reaped-nan", "signal-infinity"):
                        raw_bytes = json.dumps(raw, sort_keys=True, separators=(",", ":"),
                            allow_nan=True).encode("ascii")
                    raw_pin = retain("capture/" + phase + "-" + slot + "-RAW.json",
                                     raw, raw=raw_bytes)
                    log_pin = retain("capture/" + phase + "-" + slot + ".log", {}, raw=log)
                    capture = {"start": start_pin, "raw": raw_pin, "log": log_pin}
                    captures.append(capture)
                    capture_artifacts.extend((start_pin, raw_pin, log_pin))
                record = {"schema_version": "pbdep.b04-external-phase.v2",
                    "phase": phase, "program_sha256": reservation["program_sha256"],
                    "reservation_sha256": R.digest(R.encode(reservation)),
                    "master_anchor": reservation["anchor"],
                    "master_expiry_utc_seconds": reservation["master_expiry_utc_seconds"],
                    "state_directory": state_spec,
                    "predecessor_sha256": (reservation["program_sha256"]
                        if previous_record_pin is None else previous_record_pin["sha256"]),
                    "status": "TERMINAL", "artifacts": [artifact_pin, *capture_artifacts],
                    "resources": _history(), "policy_reservation": policy_pin,
                    "phase_started_monotonic_ns": str(start_ns),
                    "phase_finished_monotonic_ns": str(end_ns),
                    "elapsed_ns": str(elapsed_ns), "prep_used_ns": "0",
                    "close_used_ns": "0", "child_wrapper_ns": str(elapsed_ns),
                    "spent": list(policy["phases"][phase]["slots"]),
                    "reference": reference_pin, "phase_binding": binding_pin,
                    "semantic_evidence": semantic_pin}
                record_pin = retain(phase + "-phase.json", record)
                phases.append({"phase": phase, "record": record_pin})
                previous_record_pin = record_pin

            if phase_order_drift:
                phases[0], phases[1] = phases[1], phases[0]

            if capture_order_drift:
                captures[0], captures[1] = captures[1], captures[0]
            inventory = {"state_directory": state_spec, "files": files,
                "directories": {}, "total_bytes": sum(int(item["size"])
                    for item in files.values())}
            prior = {"schema_version": "pbdep.b04-prior.v2", "kind": "TERMINAL",
                "program_sha256": reservation["program_sha256"],
                "chain_id": reservation["chain_id"],
                "reservation_sha256": R.digest(R.encode(reservation)),
                "state_directory": state_spec, "resources": _history(),
                "terminal_captures": captures,
                "inventory": inventory,
                "phase_history": phases}
            prior_pin = {"path": state_spec["path"] + "/PRIOR.json",
                "sha256": R.digest(R.encode(prior))}
            objects_by_path[prior_pin["path"]] = prior

            def read_json(pin):
                return deepcopy(objects_by_path[pin["path"]])

            def pinned(pin, *_args, **_kwargs):
                return raw_by_path[pin["path"]]

            return reservation, state_spec, prior, prior_pin, inventory, read_json, pinned

        for overlap, expected_code in ((False, "phase-consumer-sentinel"),
                                       (True, "b04_prior_phase_overlap")):
            (reservation, state_spec, prior, prior_pin, inventory,
             read_json, pinned) = fixture(overlap=overlap)
            with patch.object(D, "read_json", side_effect=read_json), \
                 patch.object(D, "state_inventory", return_value=inventory), \
                 patch.object(R, "pinned", side_effect=pinned):
                if expected_code == "phase-consumer-sentinel":
                    visited = []
                    def semantic_sentinel(record, _captures, _inventory):
                        visited.append(record["phase"])
                        raise RuntimeError("phase-consumer-sentinel")
                    with patch.object(D, "consume_prior_phase", side_effect=semantic_sentinel):
                        with self.assertRaisesRegex(RuntimeError, "^phase-consumer-sentinel$"):
                            D.prior_history(prior_pin, reservation=reservation,
                                state_spec=state_spec, observed_entry_ns=1_100_000)
                    self.assertEqual(visited, ["qualification"])
                else:
                    self._expect_stop(lambda: D.prior_history(prior_pin,
                        reservation=reservation, state_spec=state_spec,
                        observed_entry_ns=1_100_000), expected_code)

        # Keep the phase-record prefix coherent, then isolate each anchored
        # boundary before the later terminal-capture sentinel.
        for options in ({"first_start": 999_999},
                        {"second_end": 1_100_001}):
            (reservation, state_spec, prior, prior_pin, inventory,
             read_json, pinned) = fixture(**options)
            with patch.object(D, "read_json", side_effect=read_json), \
                 patch.object(D, "state_inventory", return_value=inventory), \
                 patch.object(R, "pinned", side_effect=pinned):
                self._expect_stop(lambda: D.prior_history(prior_pin,
                    reservation=reservation, state_spec=state_spec,
                    observed_entry_ns=1_100_000), "b04_history_master_interval")

        (reservation, state_spec, prior, prior_pin, inventory,
         read_json, pinned) = fixture(phase_order_drift=True)
        with patch.object(D, "read_json", side_effect=read_json), \
             patch.object(D, "state_inventory", return_value=inventory), \
             patch.object(R, "pinned", side_effect=pinned):
            self._expect_stop(lambda: D.prior_history(prior_pin,
                reservation=reservation, state_spec=state_spec,
                observed_entry_ns=1_100_000), "b04_prior_phase_order")

        (reservation, state_spec, prior, prior_pin, inventory,
         read_json, pinned) = fixture(capture_order_drift=True)
        with patch.object(D, "read_json", side_effect=read_json), \
             patch.object(D, "state_inventory", return_value=inventory), \
             patch.object(R, "pinned", side_effect=pinned):
            self._expect_stop(lambda: D.prior_history(prior_pin,
                reservation=reservation, state_spec=state_spec,
                observed_entry_ns=1_100_000), "b04_prior_capture_sequence")

        # Keep the raw bytes, pin, phase artifact list, and retained inventory
        # internally consistent while mutating exactly the B03 aggregate RAW
        # key. This reaches the accepted prior_capture closed-key parser rather
        # than a digest, inventory, or chronology refusal.
        (reservation, state_spec, prior, prior_pin, inventory,
         read_json, pinned) = fixture(raw_schema_drift=True)
        with patch.object(D, "read_json", side_effect=read_json), \
             patch.object(D, "state_inventory", return_value=inventory), \
             patch.object(R, "pinned", side_effect=pinned):
            self._expect_stop(lambda: D.prior_history(prior_pin,
                reservation=reservation, state_spec=state_spec,
                observed_entry_ns=1_100_000), "record_shape")

        # A valid nested signal event reaches the next semantic consumer before
        # its isolated timestamp mutations are scored.
        (reservation, state_spec, prior, prior_pin, inventory,
         read_json, pinned) = fixture(raw_event_variant="signal-valid")
        with patch.object(D, "read_json", side_effect=read_json), \
             patch.object(D, "state_inventory", return_value=inventory), \
             patch.object(R, "pinned", side_effect=pinned):
            visited = []
            def signal_sentinel(record, _captures, _inventory):
                visited.append(record["phase"])
                raise RuntimeError("phase-consumer-sentinel")
            with patch.object(D, "consume_prior_phase", side_effect=signal_sentinel):
                with self.assertRaisesRegex(RuntimeError, "^phase-consumer-sentinel$"):
                    D.prior_history(prior_pin, reservation=reservation,
                        state_spec=state_spec, observed_entry_ns=1_100_000)
            self.assertEqual(visited, ["qualification"])

        # Nested event timestamps traverse strict prior RAW parsing with
        # internally consistent byte, pin, phase-artifact, and inventory joins.
        for variant, expected_code in (
                ("reaped-bool", "b04_supervisor_time"),
                ("signal-negative", "b04_supervisor_float_scope"),
                ("reaped-nan", "b04_prior_raw"),
                ("signal-infinity", "b04_prior_raw")):
            (reservation, state_spec, prior, prior_pin, inventory,
             read_json, pinned) = fixture(raw_event_variant=variant)
            with patch.object(D, "read_json", side_effect=read_json), \
                 patch.object(D, "state_inventory", return_value=inventory), \
                 patch.object(R, "pinned", side_effect=pinned):
                self._expect_stop(lambda: D.prior_history(prior_pin,
                    reservation=reservation, state_spec=state_spec,
                    observed_entry_ns=1_100_000), expected_code)

        # Keep every phase, capture pin, and raw record coherent while changing
        # one capture time edge. The accepted chronological/capture guards must
        # refuse before the semantic-consumer sentinel.
        for option, expected_code in (
                ({"capture_overlap": True}, "b04_prior_capture_overlap"),
                ({"capture_outside_phase": True}, "b04_prior_capture_order"),
                ({"capture_after_entry": True}, "b04_history_master_interval")):
            (reservation, state_spec, prior, prior_pin, inventory,
             read_json, pinned) = fixture(**option)
            with patch.object(D, "read_json", side_effect=read_json), \
                 patch.object(D, "state_inventory", return_value=inventory), \
                 patch.object(R, "pinned", side_effect=pinned), \
                 patch.object(D, "consume_prior_phase") as semantic_consumer:
                self._expect_stop(lambda: D.prior_history(prior_pin,
                    reservation=reservation, state_spec=state_spec,
                    observed_entry_ns=1_100_000), expected_code)
                semantic_consumer.assert_not_called()

    def test_b05_source01_synthetic_source_snapshot_joins_manifest_and_git_pin(self):
        with tempfile.TemporaryDirectory(prefix="pbdep-b05-source01-", dir=_TMP_ROOT) as parent:
            root = Path(parent) / "source"
            root.mkdir()
            (root / ".git").write_bytes(b"synthetic git marker\n")
            source = root / "src/lib.rs"
            source.parent.mkdir()
            source.write_bytes(b"synthetic source\n")
            manifest = {"schema_version": "phase-b.source-manifest.v1",
                "subject_head": "a" * 40, "source_root": str(root),
                "files": {"src/lib.rs": hashlib.sha256(source.read_bytes()).hexdigest()}}
            manifest_raw = QW._j(manifest)
            manifest_path = Path(parent) / "SOURCE-MANIFEST.json"
            manifest_path.write_bytes(manifest_raw)
            request = _request()
            request["source_root"] = str(root)
            request["target_root"] = str(root / "target")
            request["subject_head"] = manifest["subject_head"]
            request["source_manifest"] = {"path": str(manifest_path),
                "sha256": hashlib.sha256(manifest_raw).hexdigest()}
            before = R.source_inventory(str(root), excluded_target=str(root / "target"))
            request["immutable_files"] = [before["git"]]
            raw, observed, snapshot = D.source_snapshot(request)
            self.assertEqual(raw, manifest_raw)
            self.assertEqual(observed, manifest)
            self.assertEqual(snapshot["files"]["src/lib.rs"]["sha256"],
                             manifest["files"]["src/lib.rs"])
            self.assertEqual(snapshot["git"], before["git"])
            source.write_bytes(b"drifted synthetic source\n")
            self._expect_stop(lambda: D.source_snapshot(request), "b04_source_drift")

    def test_b05_gate01_prior_gate_pin_has_matching_and_isolated_drift_cases(self):
        gate_bytes = b"synthetic scripts/gate source B\n"
        other_bytes = b"synthetic independent gate pin A\n"
        gate_digest = hashlib.sha256(gate_bytes).hexdigest()
        other_digest = hashlib.sha256(other_bytes).hexdigest()
        manifest = {"schema_version": "phase-b.source-manifest.v1",
            "subject_head": "a" * 40, "source_root": "/fixture/source",
            "files": {"scripts/gate": gate_digest}}
        manifest_raw = QW._j(manifest)
        manifest_digest = hashlib.sha256(manifest_raw).hexdigest()
        data = {"phase": "gate", "source_root": manifest["source_root"],
            "subject_head": manifest["subject_head"], "manifest_sha256": manifest_digest,
            "source_manifest": {"path": "/fixture/source-manifest.json",
                "sha256": manifest_digest}, "gate_sha256": gate_digest}
        record = {"phase": "gate", "_reference": {}, "_binding_raw": b"synthetic binding"}
        binding = {"data": data, "cargo": "/fixture/tools/cargo",
                   "rustfmt": "/fixture/tools/rustfmt"}
        evidence = {"roles": {"result": _pin("gate-result")}}
        captures = [{"log": b"synthetic gate output\n"}]
        observed = {"binary": None, "_hashes": manifest["files"]}
        inventory = {}
        outcome = {"scope": "synthetic-gate-consumer-control"}
        command = (("/fixture/tools/cargo", "test", "--locked"),)

        def consume(current_data):
            current_binding = {**binding, "data": current_data}
            with patch.object(D.W, "commands", return_value=command), \
                 patch.object(D, "verify_prior_commands", return_value=None), \
                 patch.object(D, "evidence_bytes", return_value=manifest_raw), \
                 patch.object(D.W, "validate_gate", return_value=outcome) as validate_gate, \
                 patch.object(D, "evidence_json", return_value=outcome):
                result = D.consume_later(record, current_binding, evidence, captures,
                    observed, observed, inventory)
            return result, validate_gate

        result, validate_gate = consume(data)
        self.assertEqual(result, (command, outcome))
        validate_gate.assert_called_once()

        drifted = deepcopy(data)
        drifted["gate_sha256"] = other_digest
        with patch.object(D.W, "commands", return_value=command), \
             patch.object(D, "verify_prior_commands", return_value=None), \
             patch.object(D, "evidence_bytes", return_value=manifest_raw), \
             patch.object(D.W, "validate_gate", return_value=outcome) as validate_gate, \
             patch.object(D, "evidence_json", return_value=outcome):
            self._expect_stop(lambda: D.consume_later(record, {**binding, "data": drifted},
                evidence, captures, observed, observed, inventory), "b04_prior_gate_source")
        validate_gate.assert_not_called()

    def test_b05_names01_current_original_reference_and_v2_consumers_are_closed(self):
        source = Path(D.__file__).with_name("contract.py").read_text(encoding="utf-8")
        tree = ast.parse(source)
        original = next(node for node in tree.body
            if isinstance(node, ast.FunctionDef) and node.name == "_original_reference")
        reference_keys = next(node for node in ast.walk(original)
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
            and node.func.id == "_keys" and len(node.args) > 1
            and isinstance(node.args[0], ast.Name) and node.args[0].id == "reference")
        expected_keys = {
            "schema_version", "evidence_contract", "binding_sha256", "subject_head",
            "authority_sha256", "bound_packet_sha256", "acceptance_sha256",
            "review_sha256", "source_manifest_sha256", "required_source_pins"}
        self.assertEqual({item.value for item in reference_keys.args[1].elts
                          if isinstance(item, ast.Constant)}, expected_keys)
        original_literals = {node.value for node in ast.walk(original)
                             if isinstance(node, ast.Constant) and type(node.value) is str}
        self.assertIn("pbdep.original-reference.v2", original_literals)
        self.assertNotIn("muse_acceptance_sha256", original_literals)

        provenance = next(node for node in tree.body
            if isinstance(node, ast.FunctionDef) and node.name == "validate_provenance")
        provenance_literals = {node.value for node in ast.walk(provenance)
                               if isinstance(node, ast.Constant) and type(node.value) is str}
        self.assertTrue({"pbdep.provenance-binding.v2", "pbdep.pre-witness-admission.v2"}
                        <= provenance_literals)
        self.assertNotIn("muse_acceptance_sha256", provenance_literals)

    def test_b05_roster01_both_roster_pins_are_wired_before_first_capture(self):
        source = Path(E.__file__).read_text(encoding="utf-8")
        tree = ast.parse(source)
        witness = next(node for node in tree.body
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == "_witness")
        current = next(node for node in witness.body
            if isinstance(node, ast.FunctionDef) and node.name == "current")
        action = next(node for node in witness.body
            if isinstance(node, ast.FunctionDef) and node.name == "action")

        def pinned_roster(node, name):
            return (isinstance(node, ast.Call)
                and isinstance(node.func, ast.Attribute)
                and isinstance(node.func.value, ast.Name) and node.func.value.id == "R"
                and node.func.attr == "pinned" and node.args
                and isinstance(node.args[0], ast.Subscript)
                and isinstance(node.args[0].value, ast.Name) and node.args[0].value.id == "data"
                and isinstance(node.args[0].slice, ast.Constant)
                and node.args[0].slice.value == name)

        full_lines = [node.lineno for node in ast.walk(current)
                      if pinned_roster(node, "full_roster")]
        selected_lines = [node.lineno for node in ast.walk(current)
                          if pinned_roster(node, "selected_roster")]
        self.assertTrue(full_lines and selected_lines)
        current_calls = [node.lineno for node in ast.walk(action)
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
            and node.func.id == "current"]
        capture_calls = [node for node in ast.walk(action)
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name) and node.func.value.id == "context"
            and node.func.attr == "capture"]
        self.assertTrue(current_calls and capture_calls)
        first_capture = min(node.lineno for node in capture_calls)
        self.assertLess(min(current_calls), first_capture)
        self.assertTrue(any(isinstance(argument, ast.Name) and argument.id == "current"
            for node in capture_calls for argument in node.args))
        phase_literals = {node.value for node in ast.walk(witness)
                          if isinstance(node, ast.Constant) and type(node.value) is str}
        self.assertTrue({"base", "candidate", "full_roster", "selected_roster"}
                        <= phase_literals)

    def test_b05_ex01_resource_path_error_is_runtime_owner(self):
        self.assertTrue(issubclass(R.ResourcePathError, OSError))
        self.assertEqual(R.ResourcePathError.__module__, R.__name__)

    def test_b05_ex02_supervisor_does_not_alias_runtime_exception(self):
        self.assertFalse(hasattr(S, "ResourcePathError"))
        self.assertIsNot(getattr(S, "ResourcePathError", None), R.ResourcePathError)

    def test_b05_ne07_supervisor_positive_is_source_and_schema_pinned(self):
        source = Path(S.__file__).read_bytes()
        self.assertEqual(hashlib.sha256(source).hexdigest(), _SUPERVISE_SOURCE_SHA256)
        fixture = json.loads(Path(__file__).with_name("fixture-files.json").read_text(encoding="utf-8"))
        producer = fixture["producer_schema"]
        self.assertEqual(producer["supervise_source_sha256"], _SUPERVISE_SOURCE_SHA256)
        self.assertEqual(producer["schema_map_sha256"], _SUPERVISOR_SCHEMA_MAP_SHA256)
        projection = producer["projection"]
        self.assertEqual(projection, {
            "top_level_keys": ["leader_pid", "leader_returncode", "leader_exit_monotonic",
                "whole_descendant_set_exit", "echild_monotonic", "pipe_eof",
                "pipe_eof_monotonic", "subreaper_verified", "wait_options",
                "elapsed_seconds", "stop", "metadata_overflow", "event_counts",
                "reaped", "signals", "errors", "resource_peak_sampled", "log_bytes",
                "discarded_log_bytes", "log_sha256"],
            "reaped_keys": ["pid", "status", "exit_code", "at"],
            "signals_keys": ["pid", "signal", "at"],
            "ordinary_error_keys": ["where", "type"],
            "classified_error_keys": ["where", "type", "pathname_class", "errno"],
        })
        value = _supervisor_record()
        self.assertEqual(set(value), set(projection["top_level_keys"]))
        self.assertTrue(all(set(item) == set(projection["reaped_keys"])
                            for item in value["reaped"]))
        self.assertTrue(all(set(item) == set(projection["signals_keys"])
                            for item in value["signals"]))
        self.assertTrue(all(set(item) == set(projection["ordinary_error_keys"])
                            for item in value["errors"]))

    def test_b05_adm01_standalone_admission_positive_uses_exact_current_join(self):
        f = QW._fixture("candidate")
        result = W.validate_admission(f["compiler"], f["listing"], f["admission"],
            manifest_raw=QW._j(f["manifest"]), data=f["data"],
            compilation=f["compilation"], inventory=f["inventory"],
            reference=f["reference"], phase_binding_raw=f["phase_binding_raw"])
        expected = {"scope": "repaired-admission-joins-only",
            "admission_sha256": hashlib.sha256(QW._j(f["admission"])).hexdigest(),
            "reference_sha256": hashlib.sha256(QW._j(f["reference"])).hexdigest()}
        self.assertTrue(QW._exact(result, expected))

    def test_b05_adm02_binary_and_depfile_malformations_are_consistent_and_isolated(self):
        f = QW._fixture("candidate")

        def admission_call(compilation, admission):
            return W.validate_admission(f["compiler"], f["listing"], admission,
                manifest_raw=QW._j(f["manifest"]), data=f["data"],
                compilation=compilation, inventory=f["inventory"],
                reference=f["reference"], phase_binding_raw=f["phase_binding_raw"])

        mutations = (
            ("missing", "missing", None, "SHAPE"),
            ("extra", "extra", None, "SHAPE"),
            ("path", "path", "/fixture/targets/candidate/debug/../escape", "PATH"),
            ("digest", "sha256", "A" * 64, "PIN"),
            ("metadata-type", "mtime_ns", 1, "NUMBER"),
            ("metadata-noncanonical", "ino", "00", "NUMBER"),
            ("metadata-zero", "ino", "0", "BINARY"),
            ("size-cap", "size", str(1024 ** 3 + 1), "BINARY"),
        )
        for label, key, value, code in mutations:
            compilation = deepcopy(f["compilation"])
            admission = deepcopy(f["admission"])
            binary = deepcopy(compilation["binary"])
            if label == "missing":
                del binary["ino"]
            elif label == "extra":
                binary["mode"] = "493"
            else:
                binary[key] = value
            compilation["binary"] = binary
            admission["binary"] = deepcopy(binary)
            self._expect_rejection(lambda compilation=compilation, admission=admission:
                admission_call(compilation, admission), code)

        valid_digest = f["compilation"]["depfile_sha256"]
        self.assertEqual(len(valid_digest), 64)
        for invalid in (None, "0" * 65, "A" * 64, ""):
            compilation = deepcopy(f["compilation"])
            admission = deepcopy(f["admission"])
            compilation["depfile_sha256"] = invalid
            admission["depfile_sha256"] = invalid
            self._expect_rejection(lambda compilation=compilation, admission=admission:
                admission_call(compilation, admission), "PIN")

    def test_b05_phase01_raw_envelope_and_canonical_data_are_independently_bound(self):
        f = QW._fixture("candidate")
        data = deepcopy(f["data"])
        binding = json.loads(QW._phase_binding_raw("candidate", data).decode("utf-8"))
        distinct_raw = (json.dumps(binding, sort_keys=False, separators=(", ", ": "))
                        + "\n").encode("utf-8")
        reference = QW._reference("candidate", data, phase_binding_raw=distinct_raw)
        result = W.validate_phase_data("candidate", data, reference=reference,
                                       phase_binding_raw=distinct_raw)
        expected = {"scope": "phase-data-only", "phase": "candidate",
                    "data_sha256": hashlib.sha256(QW._j(data)).hexdigest()}
        self.assertTrue(QW._exact(result, expected))
        self.assertNotEqual(hashlib.sha256(distinct_raw).hexdigest(),
                            hashlib.sha256(QW._j(data)).hexdigest())

        changed_raw = distinct_raw.replace(b'"phase": "candidate"',
                                           b'"phase": "base"', 1)
        self._expect_rejection(lambda: W.validate_phase_data("candidate", data,
            reference=reference, phase_binding_raw=changed_raw), "PHASE_BINDING")
        self._expect_rejection(lambda: W.validate_phase_data("candidate", data,
            reference=reference, phase_binding_raw=QW._j(data)), "PHASE_BINDING")
        self._expect_rejection(lambda: W.validate_phase_data("candidate", data,
            reference=reference, phase_binding_raw=distinct_raw.decode("utf-8")), "BOUND")
        wrong_canonical = deepcopy(reference)
        wrong_canonical["phase_data_sha256"]["candidate"] = "0" * 64
        self._expect_rejection(lambda: W.validate_phase_data("candidate", data,
            reference=wrong_canonical, phase_binding_raw=distinct_raw), "REFERENCE_DRIFT")

        duplicate_key = distinct_raw.replace(b'"phase": "candidate"',
            b'"phase": "candidate", "phase": "candidate"', 1)
        duplicate_reference = QW._reference("candidate", data,
                                            phase_binding_raw=duplicate_key)
        self._expect_rejection(lambda: W.validate_phase_data("candidate", data,
            reference=duplicate_reference, phase_binding_raw=duplicate_key), "PHASE_BINDING")
        tuple_data = deepcopy(data)
        tuple_data["dep_tokens"] = tuple(tuple_data["dep_tokens"])
        tuple_raw = QW._phase_binding_raw("candidate", tuple_data)
        tuple_reference = QW._reference("candidate", tuple_data, phase_binding_raw=tuple_raw)
        self._expect_rejection(lambda: W.validate_phase_data("candidate", tuple_data,
            reference=tuple_reference, phase_binding_raw=tuple_raw), "PHASE_DATA")

        required = ("validate_phase_data", "commands", "validate_inventory",
            "validate_compilation", "validate_admission", "validate_pre_witness",
            "validate_completion", "validate_cargo_tests", "validate_gate")
        for name in required:
            parameters = inspect.signature(getattr(W, name)).parameters
            self.assertIn("phase_binding_raw", parameters)
            self.assertIs(parameters["phase_binding_raw"].default, inspect.Parameter.empty)
            self.assertIn("reference", parameters)
        self.assertEqual(QW._BoundWholeProgram._DATA_APIS, frozenset(required))

    def test_b05_qual01_zero_usage_and_phase_allocation_boundaries(self):
        reservation = _reservation()
        self.assertEqual(P.close_phase("qualification", elapsed_phase_ns=1, spent=(),
            prep_used_ns=0, close_used_ns=0, reservation=reservation), "WINDOW_COMPLETE")
        for prep, close in ((1, 0), (0, 1), (10 ** 30, 0), (0, 10 ** 30),
                            (10 ** 30, 10 ** 30)):
            self._expect_policy(lambda prep=prep, close=close: P.close_phase(
                "qualification", elapsed_phase_ns=1, spent=(), prep_used_ns=prep,
                close_used_ns=close, reservation=reservation), "suballocation_exhausted")
        for prep, close in ((True, 0), (0, -1), (-1, 0)):
            self._expect_policy(lambda prep=prep, close=close: P.close_phase(
                "qualification", elapsed_phase_ns=1, spent=(), prep_used_ns=prep,
                close_used_ns=close, reservation=reservation),
                "preparation_time_type" if prep is True or prep < 0 else "close_time_type")
        self.assertEqual(P.admit_slot("base", "4", (), elapsed_phase_ns=0,
            remaining_master_ns=10 ** 14, prep_used_ns=0, reservation=reservation), (600, 60))
        self._expect_policy(lambda: P.admit_slot("base", "4", (), elapsed_phase_ns=0,
            remaining_master_ns=10 ** 14, prep_used_ns=150 * P.NANO + 1,
            reservation=reservation), "phase_preparation_exhausted")

    def test_b05_qual02_all_phase_reservation_and_order_conservation(self):
        reservation = _reservation()
        expected_order = ("qualification", "a2", "format", "base", "candidate",
            "integration", "consumers", "gate")
        expected_slot_ceilings = {"1": 600, "2": 30, "3": 180, "4": 600,
            "5": 30, "6": 180, "7": 600, "8": 30, "9": 180,
            "10": 180, "11": 600, "12": 1800, "13": 90}
        nano = 1_000_000_000
        exit_seconds = 60
        self.assertEqual(P.ORDER, expected_order)
        self.assertEqual(P.NANO, nano)
        self.assertEqual(P.EXIT_SECONDS, exit_seconds)
        self.assertEqual(P.SLOT_CEILINGS, expected_slot_ceilings)
        self.assertIs(P.validate_reservation(reservation), reservation)
        phases = reservation["phases"]
        for index, phase in enumerate(expected_order):
            completed = expected_order[:index]
            reserve_seconds = sum(phases[name]["total"] for name in expected_order[index:])
            self.assertEqual(P.phase_reservation(phase, completed,
                reservation=reservation), reserve_seconds)
            self.assertEqual(P.admit_phase(phase, completed,
                remaining_master_ns=reserve_seconds * nano,
                reservation=reservation), phases[phase]["total"] * nano)
            self._expect_policy(lambda phase=phase, completed=completed:
                P.phase_reservation(phase, (*completed, "replay"),
                    reservation=reservation), "phase_order_or_replay")
            self._expect_policy(lambda phase=phase, completed=completed,
                    reserve_seconds=reserve_seconds: P.admit_phase(phase, completed,
                remaining_master_ns=reserve_seconds * nano - 1,
                reservation=reservation), "master_reserves_do_not_fit")
            spent = phases[phase]["slots"]
            self.assertEqual(P.close_phase(phase, elapsed_phase_ns=1, spent=spent,
                prep_used_ns=phases[phase]["prep"] * nano,
                close_used_ns=phases[phase]["close"] * nano,
                reservation=reservation), "WINDOW_COMPLETE")
            self._expect_policy(lambda phase=phase, spent=spent:
                P.close_phase(phase, elapsed_phase_ns=phases[phase]["total"] * nano + 1,
                    spent=spent, prep_used_ns=0, close_used_ns=0,
                    reservation=reservation), "phase_expired")
            for field in ("prep", "close"):
                overuse = phases[phase][field] * nano + 1
                kwargs = {"prep_used_ns": 0, "close_used_ns": 0}
                kwargs[field + "_used_ns"] = overuse
                self._expect_policy(lambda phase=phase, spent=spent, kwargs=kwargs:
                    P.close_phase(phase, elapsed_phase_ns=1, spent=spent,
                        reservation=reservation, **kwargs), "suballocation_exhausted")
            if phase != "qualification":
                later_seconds = sum(phases[name]["total"]
                    for name in expected_order[index + 1:])
                for slot_index, slot in enumerate(spent):
                    prefix = spent[:slot_index]
                    remaining_slots = spent[slot_index:]
                    own_seconds = (sum(reservation["slots"][name] + exit_seconds
                        for name in remaining_slots) + phases[phase]["prep"]
                        + phases[phase]["close"])
                    self.assertEqual(P.admit_slot(phase, slot, prefix, elapsed_phase_ns=0,
                        remaining_master_ns=(own_seconds + later_seconds) * nano,
                        prep_used_ns=0, reservation=reservation),
                        (reservation["slots"][slot], exit_seconds))
                    self._expect_policy(lambda phase=phase, slot=slot, prefix=prefix,
                            own_seconds=own_seconds, later_seconds=later_seconds:
                        P.admit_slot(phase, slot, prefix, elapsed_phase_ns=0,
                            remaining_master_ns=(own_seconds + later_seconds) * nano - 1,
                            prep_used_ns=0, reservation=reservation), "later_reserves_do_not_fit")
                first = spent[0]
                prep_used = phases[phase]["prep"] * nano
                own_after_prep = (sum(reservation["slots"][name] + exit_seconds
                    for name in spent) + phases[phase]["prep"]
                    + phases[phase]["close"]) * nano - prep_used
                self.assertEqual(P.admit_slot(phase, first, (), elapsed_phase_ns=0,
                    remaining_master_ns=own_after_prep + later_seconds * nano,
                    prep_used_ns=prep_used, reservation=reservation),
                    (reservation["slots"][first], exit_seconds))
                self._expect_policy(lambda phase=phase, first=first,
                        prep_used=prep_used, later_seconds=later_seconds:
                    P.admit_slot(phase, first, (), elapsed_phase_ns=0,
                        remaining_master_ns=10 ** 15,
                        prep_used_ns=prep_used + 1, reservation=reservation),
                    "phase_preparation_exhausted")
                if len(spent) > 1:
                    self._expect_policy(lambda phase=phase, spent=spent:
                        P.admit_slot(phase, spent[1], (spent[1],), elapsed_phase_ns=0,
                            remaining_master_ns=10 ** 15, prep_used_ns=0,
                            reservation=reservation), "slot_order")
                else:
                    only = spent[0]
                    self._expect_policy(lambda phase=phase, only=only:
                        P.admit_slot(phase, only, (only,), elapsed_phase_ns=0,
                            remaining_master_ns=10 ** 15, prep_used_ns=0,
                            reservation=reservation), "slot_replay_or_unknown")
                wrong_spent = tuple(reversed(spent)) if len(spent) > 1 else ()
                self._expect_policy(lambda phase=phase, wrong_spent=wrong_spent:
                    P.close_phase(phase, elapsed_phase_ns=1, spent=wrong_spent,
                        prep_used_ns=0, close_used_ns=0, reservation=reservation),
                    "incomplete_phase")

        for slot, ceiling in expected_slot_ceilings.items():
            over_slot = deepcopy(reservation)
            over_slot["slots"][slot] = ceiling + 1
            self._expect_policy(lambda over_slot=over_slot:
                P.validate_reservation(over_slot), "slot_ceiling")
        for phase in expected_order:
            broken_total = deepcopy(reservation)
            if phase == "qualification":
                broken_total["phases"][phase]["total"] = 0
                expected_total_error = "phase_reserve"
            else:
                broken_total["phases"][phase]["total"] -= 1
                expected_total_error = "phase_reserve"
            self._expect_policy(lambda broken_total=broken_total:
                P.validate_reservation(broken_total), expected_total_error)
            for field in ("prep", "close"):
                over_ceiling = deepcopy(reservation)
                over_ceiling["phases"][phase][field] += 1
                self._expect_policy(lambda over_ceiling=over_ceiling:
                    P.validate_reservation(over_ceiling), "phase_ceiling")
            broken_slots = deepcopy(reservation)
            phase_slots = broken_slots["phases"][phase]["slots"]
            if len(phase_slots) > 1:
                broken_slots["phases"][phase]["slots"] = tuple(reversed(phase_slots))
            elif phase_slots:
                broken_slots["phases"][phase]["slots"] = (*phase_slots, phase_slots[0])
            else:
                broken_slots["phases"][phase]["slots"] = ("1",)
            self._expect_policy(lambda broken_slots=broken_slots:
                P.validate_reservation(broken_slots), "reservation_phase_slots")

    def test_b05_hist01_resource_history_is_cumulative_at_both_caps(self):
        first = {"pathname_class": "entry-stat", "relative_path_sha256": "a" * 64}
        second = {"pathname_class": "directory-scan", "relative_path_sha256": "b" * 64}
        before = _history(1, vanished=[first], peak=128,
            types={"directory": 1, "regular": 2, "symlink": 0, "socket": 0})
        after = _history(2, vanished=[first, second], peak=128,
            types={"directory": 2, "regular": 3, "symlink": 0, "socket": 0})
        self.assertIsNone(D.history_extends(before, after))
        repeated = [first for _ in range(128)]
        at_cap = _history(8192, vanished=repeated)
        self.assertIs(D.validate_history(at_cap), at_cap)
        self._expect_stop(lambda: D.validate_history(_history(8193)), "b04_integer")
        self._expect_stop(lambda: D.validate_history(_history(1, vanished=[*repeated, second])),
                          "b04_history_cap")
        reset = _history(1, vanished=[first], peak=128,
            types={"directory": 1, "regular": 2, "symlink": 0, "socket": 0})
        self._expect_stop(lambda: D.history_extends(after, reset), "b04_history_reset")

    def test_b05_hist02_custody_resources_accumulates_scans_and_enforces_caps(self):
        first = {"pathname_class": "entry-stat", "relative_path_sha256": "a" * 64}
        second = {"pathname_class": "directory-scan", "relative_path_sha256": "b" * 64}

        def scan(total, temporary, capture, vanished, entry_count, directory_count):
            return {"total_bytes": total, "temporary_bytes": temporary,
                "capture_bytes": capture, "vanished_count": len(vanished),
                "vanished": list(vanished),
                "vanished_by_class": {"entry-stat": entry_count,
                    "directory-scan": directory_count},
                "sampled_entry_types": {"directory": directory_count,
                    "regular": entry_count, "symlink": 0, "socket": 0}}

        root = _FIXTURE_ROOT + "/tmp"
        context = object.__new__(D.Custody)
        context.reference = {"operator": {"resource_roots": [
            {"kind": "temporary", "directory": {"path": root}}]}}
        context.request = {"state_directory": {"path": _FIXTURE_ROOT + "/state"}}
        context.history = _history()
        context.last_scan = None
        context.clock = SimpleNamespace(check=lambda: None)
        facts = (scan(80, 20, 10, [first], 1, 0),
                 scan(60, 10, 5, [second], 0, 1))
        with patch.object(D.R, "scan_resources", side_effect=facts):
            self.assertEqual(D.Custody.resources(context, initial=True), 80)
            self.assertEqual(D.Custody.resources(context), 60)
        self.assertEqual(context.history["sample_count"], 2)
        self.assertEqual((context.history["peak_total_bytes"],
            context.history["peak_temporary_bytes"], context.history["peak_capture_bytes"]),
            (80, 20, 10))
        self.assertEqual(context.history["vanished"], [first, second])
        self.assertEqual(context.history["vanished_by_class"],
            {"entry-stat": 1, "directory-scan": 1})
        self.assertEqual(context.history["sampled_entry_types"],
            {"directory": 1, "regular": 1, "symlink": 0, "socket": 0})
        self.assertEqual(context.last_scan, facts[1])

        at_sample_cap = object.__new__(D.Custody)
        at_sample_cap.reference = context.reference
        at_sample_cap.request = context.request
        at_sample_cap.history = _history(8191)
        at_sample_cap.last_scan = None
        at_sample_cap.clock = context.clock
        with patch.object(D.R, "scan_resources", return_value=scan(0, 0, 0, [], 0, 0)):
            D.Custody.resources(at_sample_cap)
        self.assertEqual(at_sample_cap.history["sample_count"], 8192)

        over_sample_cap = object.__new__(D.Custody)
        over_sample_cap.reference = context.reference
        over_sample_cap.request = context.request
        over_sample_cap.history = _history(8192)
        over_sample_cap.last_scan = None
        over_sample_cap.clock = context.clock
        with patch.object(D.R, "scan_resources", return_value=scan(0, 0, 0, [], 0, 0)):
            self._expect_stop(lambda: D.Custody.resources(over_sample_cap), "b04_history_cap")

        at_vanished_cap = object.__new__(D.Custody)
        at_vanished_cap.reference = context.reference
        at_vanished_cap.request = context.request
        at_vanished_cap.history = _history(1, vanished=[first] * 128)
        at_vanished_cap.last_scan = None
        at_vanished_cap.clock = context.clock
        with patch.object(D.R, "scan_resources",
                          return_value=scan(0, 0, 0, [second], 0, 1)):
            self._expect_stop(lambda: D.Custody.resources(at_vanished_cap), "b04_history_cap")

        # Later-phase runtime custody is a distinct owner: ObservationSession,
        # with its own accumulator and refusal names. These supplied scan facts
        # exercise consumer aggregation only; they do not assert a host scan.
        runtime_context = object.__new__(R.ObservationSession)
        runtime_context.reference = context.reference
        runtime_context.state_spec = context.request["state_directory"]
        runtime_context.temporary_roots = ()
        runtime_context.resource_facts = {
            "sample_count": 0, "peak_total_bytes": 0,
            "peak_temporary_bytes": 0, "peak_capture_bytes": 0,
            "vanished_by_class": {"entry-stat": 0, "directory-scan": 0},
            "vanished": [], "sampled_entry_types": {"directory": 0,
                "regular": 0, "symlink": 0, "socket": 0}}
        runtime_context.last_resource_scan = None
        runtime_facts = (scan(80, 20, 10, [first], 1, 0),
                         scan(60, 10, 5, [second], 0, 1))
        with patch.object(R, "scan_resources", side_effect=runtime_facts):
            self.assertEqual(R.ObservationSession.resources(runtime_context, initial=True), 80)
            self.assertEqual(R.ObservationSession.resources(runtime_context), 60)
        self.assertEqual(runtime_context.resource_facts["sample_count"], 2)
        self.assertEqual((runtime_context.resource_facts["peak_total_bytes"],
            runtime_context.resource_facts["peak_temporary_bytes"],
            runtime_context.resource_facts["peak_capture_bytes"]), (80, 20, 10))
        self.assertEqual(runtime_context.resource_facts["vanished"], [first, second])
        self.assertEqual(runtime_context.last_resource_scan, runtime_facts[1])

        runtime_at_sample_cap = object.__new__(R.ObservationSession)
        runtime_at_sample_cap.reference = context.reference
        runtime_at_sample_cap.state_spec = context.request["state_directory"]
        runtime_at_sample_cap.temporary_roots = ()
        runtime_at_sample_cap.resource_facts = deepcopy(runtime_context.resource_facts)
        runtime_at_sample_cap.resource_facts.update({"sample_count": 8191, "vanished": []})
        runtime_at_sample_cap.last_resource_scan = None
        with patch.object(R, "scan_resources", return_value=scan(0, 0, 0, [], 0, 0)):
            R.ObservationSession.resources(runtime_at_sample_cap)
        self.assertEqual(runtime_at_sample_cap.resource_facts["sample_count"], 8192)

        runtime_over_sample_cap = object.__new__(R.ObservationSession)
        runtime_over_sample_cap.reference = context.reference
        runtime_over_sample_cap.state_spec = context.request["state_directory"]
        runtime_over_sample_cap.temporary_roots = ()
        runtime_over_sample_cap.resource_facts = deepcopy(runtime_context.resource_facts)
        runtime_over_sample_cap.resource_facts["sample_count"] = 8192
        runtime_over_sample_cap.last_resource_scan = None
        with patch.object(R, "scan_resources", return_value=scan(0, 0, 0, [], 0, 0)):
            self._expect_stop(lambda: R.ObservationSession.resources(
                runtime_over_sample_cap), "resource_sample_metadata_cap")

        runtime_over_vanished_cap = object.__new__(R.ObservationSession)
        runtime_over_vanished_cap.reference = context.reference
        runtime_over_vanished_cap.state_spec = context.request["state_directory"]
        runtime_over_vanished_cap.temporary_roots = ()
        runtime_over_vanished_cap.resource_facts = deepcopy(runtime_context.resource_facts)
        runtime_over_vanished_cap.resource_facts["vanished"] = [first] * 128
        runtime_over_vanished_cap.last_resource_scan = None
        with patch.object(R, "scan_resources", return_value=scan(0, 0, 0, [second], 0, 1)):
            self._expect_stop(lambda: R.ObservationSession.resources(
                runtime_over_vanished_cap), "vanished_metadata_cap")

    def test_b05_d_adapt01_current_dependency_grammar_and_typed_bindings(self):
        source_hash = hashlib.sha256(b"synthetic dependency source").hexdigest()
        token = "crates/beacond/src/lib.rs"
        binding = {"raw_token": token, "canonical_relative": token,
                   "source_sha256": source_hash}
        result = W.validate_dependency_bindings([token], "phase-b.dep-binding.v1",
            [binding], sources={token: source_hash})
        expected = {"scope": "supplied-dependency-map-only",
            "dep_binding_schema": "phase-b.dep-binding.v1",
            "dep_bindings_sha256": hashlib.sha256(QW._j([binding])).hexdigest(),
            "canonical_names": [token]}
        self.assertTrue(QW._exact(result, expected))

        parent = "crates/beacond/src/../sample-project/Cargo.lock"
        parent_canonical = "crates/beacond/sample-project/Cargo.lock"
        parent_binding = {"raw_token": parent, "canonical_relative": parent_canonical,
                          "source_sha256": source_hash}
        self.assertEqual(W.validate_dependency_bindings([parent], "phase-b.dep-binding.v1",
            [parent_binding])["canonical_names"], [parent_canonical])

        def reject(tokens, schema, bindings, code, sources=None):
            self._expect_rejection(lambda: W.validate_dependency_bindings(
                tokens, schema, bindings, sources=sources), code)

        reject([token, token], "phase-b.dep-binding.v1", [binding, binding], "DEP_DUPLICATE")
        alias_tokens = ["crates/beacond/src/../artifact.rs", "crates/beacond/artifact.rs"]
        alias_rows = [{"raw_token": name, "canonical_relative": "crates/beacond/artifact.rs",
                       "source_sha256": source_hash} for name in alias_tokens]
        reject(alias_tokens, "phase-b.dep-binding.v1", alias_rows, "DEP_ALIAS")
        for invalid, code in (("../escape.rs", "DEP_ESCAPE"),
                              ("crates/../../escape/../leaf.rs", "DEP_ESCAPE"),
                              ("/absolute.rs", "DEP_PATH"),
                              ("crates/beacond/src\\lib.rs", "DEP_PATH"),
                              ("crates/beacond/src/\\\n../sample-project/Cargo.lock", "DEP_PATH")):
            row = {"raw_token": invalid, "canonical_relative": "leaf.rs",
                   "source_sha256": source_hash}
            reject([invalid], "phase-b.dep-binding.v1", [row], code)
        wrong = {"raw_token": token, "canonical_relative": "other.rs", "source_sha256": source_hash}
        reject([token], "phase-b.dep-binding.v1", [wrong], "DEP_BINDING")
        missing_source = {"other.rs": source_hash}
        reject([token], "phase-b.dep-binding.v1", [binding], "DEP_SOURCE", missing_source)
        wrong_digest = {token: "b" * 64}
        reject([token], "phase-b.dep-binding.v1", [binding], "DEP_DIGEST", wrong_digest)
        for schema in ("phase-b.dep-binding.v0", "phase-b.dep-binding.unknown"):
            reject([token], schema, [binding], "DEP_VERSION")
        typed = {"raw_token": token, "canonical_relative": 7, "source_sha256": source_hash}
        reject([token], "phase-b.dep-binding.v1", [typed], "DEP_BINDING")
        extra = {**binding, "approval": True}
        reject([token], "phase-b.dep-binding.v1", [extra], "SHAPE")
        canonical_short = "crates/beacond/sample-project/Cargo.lock"
        long_token = "a/../" * 220 + canonical_short
        self.assertGreater(len(long_token.encode("ascii")), 1024)
        self.assertEqual(os.path.normpath(long_token), canonical_short)
        reject([long_token], "phase-b.dep-binding.v1", [{"raw_token": long_token,
            "canonical_relative": canonical_short, "source_sha256": source_hash}], "DEP_PATH")

        f = QW._fixture("base")
        missing_schema = deepcopy(f["data"])
        del missing_schema["dep_binding_schema"]
        raw, reference = QW._phase_context("base", missing_schema)
        self._expect_rejection(lambda: W.validate_phase_data("base", missing_schema,
            reference=reference, phase_binding_raw=raw), "SHAPE")

        data = deepcopy(f["data"])
        manifest = deepcopy(f["manifest"])
        dropped = "crates/beacond/src/artifact_phase_b_contract.rs"
        manifest["files"].pop(dropped)
        data["dep_tokens"] = [name for name in data["dep_tokens"] if name != dropped]
        data["dep_bindings"] = [row for row in data["dep_bindings"]
                                 if row["raw_token"] != dropped]
        manifest_raw = QW._j(manifest)
        manifest_digest = hashlib.sha256(manifest_raw).hexdigest()
        data["manifest_sha256"] = manifest_digest
        data["source_manifest"]["sha256"] = manifest_digest
        raw, reference = QW._phase_context("base", data)
        self._expect_rejection(lambda: W.validate_compilation(f["compiler"], f["depfile"],
            manifest_raw=manifest_raw, data=data, observed_binary=f["binary"],
            observed_dependencies=f["dependency_observation"], reference=reference,
            phase_binding_raw=raw), "DEPFILE")

    def test_b05_d_rd_inventory01_independent_exact_triples_are_preserved(self):
        tests = Path(__file__).parent
        controls_path = tests / "controls.json"
        replay_path = tests / "replay-map.json"
        controls_raw = controls_path.read_bytes()
        replay_raw = replay_path.read_bytes()
        self.assertEqual(hashlib.sha256(controls_raw).hexdigest(),
            "5519c8a9dab10faec878e2f4b1e1cce2b463cf53dc6b455ad270e64caca7249e")
        self.assertEqual(hashlib.sha256(replay_raw).hexdigest(),
            "f474977ce4d9d86cbde10ea72791e7b244cfce314134964309961f039b6c925a")
        controls = json.loads(controls_raw.decode("utf-8"))
        replay = json.loads(replay_raw.decode("utf-8"))
        control_ids = [row["id"] for row in controls["controls"]]
        qd = [{"control_id": row["id"], "observation_id": item["observation_id"],
               "scenario": item["scenario"], "expected": item["expected"]}
              for row in controls["controls"] for item in row["observations"]]
        rd = [{"observation_id": item["observation_id"], "scenario": item["scenario"],
               "expected": item["expected"]} for item in replay["replay_observations"]]
        projection = {"qd_observations": qd, "rd_observations": rd}
        projection_raw = json.dumps(projection, sort_keys=True, separators=(",", ":"),
                                    ensure_ascii=True).encode("utf-8")
        self.assertEqual(control_ids, ["D%02d" % number for number in range(1, 33)])
        self.assertEqual(len(qd), 36)
        self.assertEqual([row["observation_id"] for row in rd],
                         ["RD%02d" % number for number in range(1, 6)])
        self.assertEqual(hashlib.sha256(projection_raw).hexdigest(),
            "512f91114ed006e0a7231cfe22124e75c9868995eb2a6044235f463630ced007")

    def test_b05_d_legacy01_named_predecessor_parent_rule_stays_non_authoritative(self):
        token = "crates/beacond/src/lib.rs"
        digest = hashlib.sha256(b"independent canonical source").hexdigest()
        binding = {"raw_token": token, "canonical_relative": token,
                   "source_sha256": digest}
        self.assertEqual(W.validate_dependency_bindings([token], "phase-b.dep-binding.v1",
            [binding], sources={token: digest})["canonical_names"], [token])
        controls = json.loads(Path(__file__).with_name("controls.json").read_text(encoding="utf-8"))
        row = next(row for control in controls["controls"] if control["id"] == "D01"
                   for row in control["observations"] if row["observation_id"] == "D01.2")
        expected = {"outcome": "REJECT_TYPED", "code": "PATH",
                    "provenance": "deliberate-contract"}
        self.assertEqual(row["expected"], expected)
        refusal = _historical_d01_parent_refusal(row["input"]["raw_token"])
        self.assertEqual(refusal, {"scenario": "predecessor-only:D01.2", **expected})
        self.assertIsNone(_historical_d01_parent_refusal(token))

    def test_b05_d_raw01_current_raw_depfile_adapter_preserves_exact_grammar(self):
        f = QW._fixture("candidate")
        kwargs = {"source_root": f["data"]["source_root"],
            "executable": f["binary"]["path"], "dep_tokens": f["data"]["dep_tokens"],
            "dep_binding_schema": f["data"]["dep_binding_schema"],
            "dep_bindings": f["data"]["dep_bindings"], "source_hashes": f["manifest"]["files"]}
        result = W.validate_dependency_artifact(f["depfile"], **kwargs)
        expected = {"scope": "dependency-artifact-and-supplied-map-only",
            "dep_binding_schema": f["data"]["dep_binding_schema"],
            "dep_bindings_sha256": hashlib.sha256(QW._j(f["data"]["dep_bindings"])).hexdigest(),
            "canonical_names": list(f["data"]["dep_tokens"]),
            "depfile_sha256": hashlib.sha256(f["depfile"]).hexdigest()}
        self.assertTrue(QW._exact(result, expected))
        forged = f["depfile"].replace(b"crates/beacond/src/lib.rs",
                                       b"crates/beacond/src/./lib.rs")
        reordered = f["depfile"].replace(
            b"crates/beacond/src/lib.rs crates/beacond/src/artifact.rs",
            b"crates/beacond/src/artifact.rs crates/beacond/src/lib.rs")
        missing = f["depfile"].replace(b" crates/beacond/src/artifact.rs", b"", 1)
        extra = f["depfile"].replace(b".d: ", b".d: crates/beacond/src/extra.rs ", 1)
        for raw in (forged, reordered, missing, extra):
            self._expect_rejection(lambda raw=raw: W.validate_dependency_artifact(raw, **kwargs),
                                   "DEPFILE")

    def test_b05_d_fs01_synthetic_dependency_component_controls(self):
        content = b"pbdep fixture\n"
        normal = "crates/beacond/src/lib.rs"

        with tempfile.TemporaryDirectory(prefix="pbdep-b05-d-fs-positive-", dir=_TMP_ROOT) as root:
            _write_fixture_file(root, normal, content)
            data, manifest, reference, binding_raw, _ = _runtime_dependency_fixture(
                root, normal, normal, content)
            expected = _independent_observation_preimage(root, data, normal, content)
            self.assertEqual(R.observe_dependencies(manifest, data, reference=reference,
                phase_binding_raw=binding_raw, expected=expected), expected)

        with tempfile.TemporaryDirectory(prefix="pbdep-b05-d19-", dir=_TMP_ROOT) as root:
            tree = Path(root) / "crates/beacond"
            canonical = "crates/beacond/sample-project/Cargo.lock"
            _write_fixture_file(root, canonical, content)
            os.symlink("sample-project", tree / "src")
            token = "crates/beacond/src/../sample-project/Cargo.lock"
            data, manifest, reference, binding_raw, _ = _runtime_dependency_fixture(
                root, token, canonical, content)
            self._expect_stop(lambda: R.observe_dependencies(manifest, data,
                reference=reference, phase_binding_raw=binding_raw), "dependency_directory")

        with tempfile.TemporaryDirectory(prefix="pbdep-b05-d20-", dir=_TMP_ROOT) as root:
            _write_fixture_file(root, "crates/beacond/src/real.rs", content)
            os.symlink("real.rs", Path(root) / normal)
            data, manifest, reference, binding_raw, _ = _runtime_dependency_fixture(
                root, normal, normal, content)
            self._expect_stop(lambda: R.observe_dependencies(manifest, data,
                reference=reference, phase_binding_raw=binding_raw), "dependency_terminal")

        with tempfile.TemporaryDirectory(prefix="pbdep-b05-d21-", dir=_TMP_ROOT) as root:
            file_path = _write_fixture_file(root, normal, content)
            _create_d21_hardlink(file_path, file_path.with_name("same-bytes.rs"))
            data, manifest, reference, binding_raw, _ = _runtime_dependency_fixture(
                root, normal, normal, content)
            self._expect_stop(lambda: R.observe_dependencies(manifest, data,
                reference=reference, phase_binding_raw=binding_raw), "dependency_terminal")

        with tempfile.TemporaryDirectory(prefix="pbdep-b05-d23-", dir=_TMP_ROOT) as root:
            canonical = "crates/beacond/src/sample-project/Cargo.lock"
            _write_fixture_file(root, canonical, content)
            _write_fixture_file(root, "crates/beacond/src/nondirectory", content)
            token = "crates/beacond/src/nondirectory/../sample-project/Cargo.lock"
            data, manifest, reference, binding_raw, _ = _runtime_dependency_fixture(
                root, token, canonical, content)
            self._expect_stop(lambda: R.observe_dependencies(manifest, data,
                reference=reference, phase_binding_raw=binding_raw), "dependency_directory")

        with tempfile.TemporaryDirectory(prefix="pbdep-b05-d24-", dir=_TMP_ROOT) as root:
            file_path = _write_fixture_file(root, normal, content)
            data, manifest, reference, binding_raw, _ = _runtime_dependency_fixture(
                root, normal, normal, content)
            expected = _independent_observation_preimage(root, data, normal, content)
            replacement = file_path.with_name("same-bytes-new-identity.rs")
            replacement.write_bytes(content)
            self.assertNotEqual(str(replacement.lstat().st_ino), expected["files"][normal]["ino"])
            os.replace(replacement, file_path)
            self.assertEqual(file_path.read_bytes(), content)
            self._expect_stop(lambda: R.observe_dependencies(manifest, data,
                reference=reference, phase_binding_raw=binding_raw, expected=expected),
                "dependency_observation_drift")

    def test_b05_d_preimage01_missing_component_preimage_precedes_refusal(self):
        content = b"pbdep fixture\n"
        canonical = "crates/beacond/sample-project/Cargo.lock"
        token = "crates/beacond/src/../sample-project/Cargo.lock"
        with tempfile.TemporaryDirectory(prefix="pbdep-b05-d22-positive-", dir=_TMP_ROOT) as root:
            _write_fixture_file(root, canonical, content)
            data, manifest, reference, binding_raw, _ = _runtime_dependency_fixture(
                root, canonical, canonical, content)
            expected = _independent_observation_preimage(root, data, canonical, content)
            self.assertEqual(R.observe_dependencies(manifest, data, reference=reference,
                phase_binding_raw=binding_raw, expected=expected), expected)

        with tempfile.TemporaryDirectory(prefix="pbdep-b05-d22-", dir=_TMP_ROOT) as root:
            _write_fixture_file(root, canonical, content)
            data, manifest, reference, binding_raw, _ = _runtime_dependency_fixture(
                root, token, canonical, content)
            parent = Path(root) / "crates/beacond"
            before = parent.lstat()
            preimage = {"parent_path": str(parent), "parent_dev": str(before.st_dev),
                        "parent_ino": str(before.st_ino)}
            self.assertEqual(token, "crates/beacond/src/../sample-project/Cargo.lock")
            expected = {"raw_token": token, "component": "src",
                "parent_path": str(parent), "parent_dev": str(before.st_dev),
                "parent_ino": str(before.st_ino), "errno": errno.ENOENT}
            with self.assertRaises(R.Stop) as caught:
                R.observe_dependencies(manifest, data, reference=reference,
                    phase_binding_raw=binding_raw)
            self.assertIs(type(caught.exception), R.Stop)
            self.assertEqual(str(caught.exception), "dependency_missing_component")
            self.assertEqual(caught.exception.provenance, "verified-missing-component")
            self.assertEqual(caught.exception.refusal_context, expected)
            self.assertEqual({key: expected[key] for key in preimage}, preimage)

        f = QW._fixture("candidate")
        pre = deepcopy(f["pre"])
        pre["dep_bindings_sha256"] = "0" * 64
        self._expect_rejection(lambda: W.validate_pre_witness(f["compiler"], f["listing"],
            f["admission"], pre, manifest_raw=QW._j(f["manifest"]), data=f["data"],
            compilation=f["compilation"], inventory=f["inventory"],
            reference=f["reference"], phase_binding_raw=f["phase_binding_raw"]), "PROVENANCE")

        provenance = deepcopy(f["provenance"])
        provenance["dep_bindings_sha256"] = "0" * 64
        self._expect_rejection(lambda: W.validate_completion(f["selected_output"], f["capture"],
            provenance=provenance, observed_binary=f["binary"],
            observed_sources=f["manifest"]["files"],
            observed_depfile_sha256=hashlib.sha256(f["depfile"]).hexdigest(),
            observed_dependencies=f["dependency_observation"], data=f["data"],
            manifest_raw=QW._j(f["manifest"]), reference=f["reference"],
            phase_binding_raw=f["phase_binding_raw"]), "DEP_COMPLETION")

    def test_b05_d_compose01_current_synthetic_base_and_candidate_chains(self):
        for phase in ("base", "candidate"):
            f = QW._fixture(phase)
            compiled = W.validate_compilation(f["compiler"], f["depfile"],
                manifest_raw=QW._j(f["manifest"]), data=f["data"],
                observed_binary=f["binary"], observed_dependencies=f["dependency_observation"],
                reference=f["reference"], phase_binding_raw=f["phase_binding_raw"])
            self.assertTrue(QW._exact(compiled, f["compilation"]))
            inventory = W.validate_inventory(f["listing"], full_roster=f["full"],
                selected_roster=f["selected"], case_map=f["data"]["case_map"], data=f["data"],
                reference=f["reference"], phase_binding_raw=f["phase_binding_raw"])
            self.assertTrue(QW._exact(inventory, f["inventory"]))
            admitted = W.validate_admission(f["compiler"], f["listing"], f["admission"],
                manifest_raw=QW._j(f["manifest"]), data=f["data"], compilation=compiled,
                inventory=inventory, reference=f["reference"],
                phase_binding_raw=f["phase_binding_raw"])
            expected_admission = {"scope": "repaired-admission-joins-only",
                "admission_sha256": hashlib.sha256(QW._j(f["admission"])).hexdigest(),
                "reference_sha256": hashlib.sha256(QW._j(f["reference"])).hexdigest()}
            self.assertTrue(QW._exact(admitted, expected_admission))
            provenance = W.validate_pre_witness(f["compiler"], f["listing"], f["admission"],
                f["pre"], manifest_raw=QW._j(f["manifest"]), data=f["data"],
                compilation=compiled, inventory=inventory, reference=f["reference"],
                phase_binding_raw=f["phase_binding_raw"])
            self.assertTrue(QW._exact(provenance, f["provenance"]))
            completion = {"schema_version": "pbdep.repaired-completion.v3",
                **f["dependency_joins"], "depfile_sha256": hashlib.sha256(f["depfile"]).hexdigest(),
                "scope": "repaired-post-witness-custody-only",
                "raw_sha256": hashlib.sha256(f["selected_output"]).hexdigest(),
                "pre_sha256": f["provenance"]["pre_sha256"]}
            completed = W.validate_completion(f["selected_output"], f["capture"],
                provenance=provenance, observed_binary=f["binary"],
                observed_sources=f["manifest"]["files"],
                observed_depfile_sha256=hashlib.sha256(f["depfile"]).hexdigest(),
                observed_dependencies=f["dependency_observation"], data=f["data"],
                manifest_raw=QW._j(f["manifest"]), reference=f["reference"],
                phase_binding_raw=f["phase_binding_raw"])
            self.assertTrue(QW._exact(completed, completion))

    def test_b05_q01_tagged_values_preserve_native_type_and_reject_unlisted_values(self):
        fragment = b"independent fixed typed-value predicate\n"
        source_pin = _q_pin("predicate.txt", fragment)
        fixture_raw = b"tagged-value input fixture\n"
        fixture_pin = _q_pin("value-input.bin", fixture_raw)
        byte_raw = b"retained byte value\n"
        byte_pin = _q_pin("value-bytes.bin", byte_raw)
        inputs = sorted((fixture_pin, byte_pin), key=lambda pin: pin["path"])
        expected_value = {"tag": "map", "items": [
            ["bool", {"tag": "bool", "value": True}],
            ["bytes", {"tag": "bytes", "pin": byte_pin, "length": len(byte_raw)}],
            ["int", {"tag": "int", "value": "1"}],
            ["list", {"tag": "list", "items": [{"tag": "none"}]}],
            ["map", {"tag": "map", "items": [["k", {"tag": "text", "value": "v"}]]}],
            ["none", {"tag": "none"}],
            ["text", {"tag": "text", "value": "UTF-8 fixture"}],
            ["tuple", {"tag": "tuple", "items": [{"tag": "bool", "value": False}]}],
        ]}
        expected = {"control_id": "SUC001", "observation_id": "SUC001.1",
            "kind": "VALUE_EQUAL", "expected": {"value": expected_value},
            "inputs": inputs, "oracle_source": source_pin}
        observed = {"control_id": expected["control_id"],
            "observation_id": expected["observation_id"], "kind": expected["kind"],
            "inputs_sha256": R.digest(R.encode(inputs)),
            "oracle_source_sha256": source_pin["sha256"],
            "observed": {"value": deepcopy(expected_value)}}
        evidence = {fixture_pin["path"]: fixture_raw, byte_pin["path"]: byte_raw,
                    source_pin["path"]: fragment}
        producer = b"source assertion: " + fragment
        self.assertIsNone(_q_row_call(expected, observed, "whole-program",
                                      {"inventory": {}}, evidence, producer))

        type_changed = deepcopy(observed)
        type_changed["observed"]["value"]["items"][0][1] = {
            "tag": "int", "value": "1"}
        self._expect_rejection(lambda: _q_row_call(expected, type_changed, "whole-program",
            {"inventory": {}}, evidence, producer), "PHASE_DATA")
        unsupported = deepcopy(expected)
        unsupported["expected"]["value"] = {"tag": "float", "value": "1.0"}
        unsupported_observed = deepcopy(observed)
        unsupported_observed["observed"]["value"] = deepcopy(unsupported["expected"]["value"])
        self._expect_stop(lambda: _q_row_call(unsupported, unsupported_observed,
            "whole-program", {"inventory": {}}, evidence, producer),
            "b04_q_unsupported_value")

        oversized_text = deepcopy(expected)
        oversized_text["expected"]["value"] = {"tag": "text", "value": "x" * 65537}
        oversized_text_observed = deepcopy(observed)
        oversized_text_observed["observed"]["value"] = deepcopy(
            oversized_text["expected"]["value"])
        self._expect_stop(lambda: _q_row_call(oversized_text, oversized_text_observed,
            "whole-program", {"inventory": {}}, evidence, producer), "b04_q_text")

        oversized_bytes = deepcopy(expected)
        oversized_bytes["expected"]["value"] = {"tag": "bytes", "pin": byte_pin,
                                                   "length": 65537}
        oversized_bytes_observed = deepcopy(observed)
        oversized_bytes_observed["observed"]["value"] = deepcopy(
            oversized_bytes["expected"]["value"])
        self._expect_stop(lambda: _q_row_call(oversized_bytes, oversized_bytes_observed,
            "whole-program", {"inventory": {}}, evidence, producer), "b04_integer")

        oversized_items = deepcopy(expected)
        oversized_items["expected"]["value"] = {"tag": "list", "items": [
            {"tag": "none"} for _ in range(1025)]}
        oversized_items_observed = deepcopy(observed)
        oversized_items_observed["observed"]["value"] = deepcopy(
            oversized_items["expected"]["value"])
        self._expect_stop(lambda: _q_row_call(oversized_items, oversized_items_observed,
            "whole-program", {"inventory": {}}, evidence, producer), "b04_q_items")

        def tagged_row(value):
            row_expected = deepcopy(expected)
            row_expected["expected"]["value"] = value
            row_observed = deepcopy(observed)
            row_observed["observed"]["value"] = deepcopy(value)
            return row_expected, row_observed

        def nested_lists(levels):
            value = {"tag": "none"}
            for _ in range(levels):
                value = {"tag": "list", "items": [value]}
            return value

        depth_limit, depth_limit_observed = tagged_row(nested_lists(12))
        self.assertIsNone(_q_row_call(depth_limit, depth_limit_observed,
            "whole-program", {"inventory": {}}, evidence, producer))
        depth_over, depth_over_observed = tagged_row(nested_lists(13))
        self._expect_stop(lambda: _q_row_call(depth_over, depth_over_observed,
            "whole-program", {"inventory": {}}, evidence, producer), "b04_q_value_cap")

        # Per tree: one root + two child lists + 1,023 + 1,022 leaves = 2,048.
        # Both trees share the 4,096-node budget; local fanout stays below 1,024.
        node_limit_value = {"tag": "list", "items": [
            {"tag": "list", "items": [{"tag": "none"} for _ in range(1023)]},
            {"tag": "list", "items": [{"tag": "none"} for _ in range(1022)]}]}
        node_limit, node_limit_observed = tagged_row(node_limit_value)
        self.assertIsNone(_q_row_call(node_limit, node_limit_observed,
            "whole-program", {"inventory": {}}, evidence, producer))
        # One extra observed leaf gives 4,097 combined nodes, with fanout 1,023.
        node_over, node_over_observed = tagged_row(node_limit_value)
        node_over_observed["observed"]["value"]["items"][1]["items"].append({"tag": "none"})
        self._expect_stop(lambda: _q_row_call(node_over, node_over_observed,
            "whole-program", {"inventory": {}}, evidence, producer), "b04_q_value_cap")

        duplicate_map = deepcopy(expected)
        duplicate_map["expected"]["value"] = {"tag": "map", "items": [
            ["same", {"tag": "none"}], ["same", {"tag": "none"}]]}
        duplicate_map_observed = deepcopy(observed)
        duplicate_map_observed["observed"]["value"] = deepcopy(
            duplicate_map["expected"]["value"])
        self._expect_stop(lambda: _q_row_call(duplicate_map, duplicate_map_observed,
            "whole-program", {"inventory": {}}, evidence, producer), "b04_q_map")

    def test_b05_q06_assertion_return_is_closed_to_two_source_families(self):
        fragment = b"closed assertion-return predicate\n"
        source_pin = _q_pin("assertion-return-source.txt", fragment)
        fixture_raw = b"assertion input fixture\n"
        fixture_pin = _q_pin("assertion-input.bin", fixture_raw)
        inputs = [fixture_pin]
        expected_value = {"assertion_sha256": source_pin["sha256"], "return_kind": "none"}
        expected = {"control_id": "ASSERTION01", "observation_id": "ASSERTION01.1",
            "kind": "ASSERTION_RETURN", "expected": expected_value,
            "inputs": inputs, "oracle_source": source_pin}
        observed = {"control_id": expected["control_id"],
            "observation_id": expected["observation_id"], "kind": expected["kind"],
            "inputs_sha256": R.digest(R.encode(inputs)),
            "oracle_source_sha256": source_pin["sha256"],
            "observed": deepcopy(expected_value)}
        evidence = {source_pin["path"]: fragment, fixture_pin["path"]: fixture_raw}
        for family in ("wrapper", "original-contract"):
            producer = b"source-closed " + fragment
            self.assertIsNone(_q_row_call(expected, observed, family,
                {"inventory": {}}, evidence, producer))
            changed = deepcopy(observed)
            changed["observed"]["return_kind"] = "value"
            self._expect_rejection(lambda: _q_row_call(expected, changed, family,
                {"inventory": {}}, evidence, producer), "PHASE_DATA")
        self._expect_stop(lambda: _q_row_call(expected, observed, "successor",
            {"inventory": {}}, evidence, b"source-closed " + fragment),
            "b04_q_assertion_family")

    def test_b05_q02_class_only_and_source_closed_exception_predicates(self):
        fragment = b"fixed source-closed exception assertion\n"
        source_pin = _q_pin("exception-predicate.txt", fragment)
        fixture_raw = b"exception input\n"
        fixture_pin = _q_pin("exception-input.bin", fixture_raw)
        inputs = [fixture_pin]
        expected = {"control_id": "SUC002", "observation_id": "SUC002.1",
            "kind": "CLOSED_REJECTION",
            "expected": {"exception_role": "whole_program.Rejection"},
            "inputs": inputs, "oracle_source": source_pin}
        observed = {"control_id": expected["control_id"],
            "observation_id": expected["observation_id"], "kind": expected["kind"],
            "inputs_sha256": R.digest(R.encode(inputs)),
            "oracle_source_sha256": source_pin["sha256"],
            "observed": {"exception_role": "whole_program.Rejection", "code": "SHAPE"}}
        evidence = {fixture_pin["path"]: fixture_raw, source_pin["path"]: fragment}
        self.assertIsNone(_q_row_call(expected, observed, "whole-program",
            {"inventory": {}}, evidence, b"expected class-only refusal " + fragment))
        changed_code = deepcopy(observed)
        changed_code["observed"]["code"] = "PATH"
        self.assertIsNone(_q_row_call(expected, changed_code, "whole-program",
            {"inventory": {}}, evidence, b"expected class-only refusal " + fragment))
        changed_role = deepcopy(observed)
        changed_role["observed"]["exception_role"] = "contract.Rejection"
        self._expect_stop(lambda: _q_row_call(expected, changed_role, "whole-program",
            {"inventory": {}}, evidence, b"expected class-only refusal " + fragment),
            "b04_q_closed_rejection")

        runtime_source = Path(R.__file__).read_bytes()
        runtime_pin = _q_pin("runtime.py", runtime_source)
        exception_inputs = sorted((fixture_pin, runtime_pin), key=lambda pin: pin["path"])
        class_expected = {"control_id": "SUC002", "observation_id": "SUC002.2",
            "kind": "EXCEPTION_CLASS", "expected": {"exception_role": "runtime.ResourcePathError"},
            "inputs": exception_inputs, "oracle_source": source_pin}
        class_observed = {"control_id": class_expected["control_id"],
            "observation_id": class_expected["observation_id"], "kind": class_expected["kind"],
            "inputs_sha256": R.digest(R.encode(exception_inputs)),
            "oracle_source_sha256": source_pin["sha256"],
            "observed": deepcopy(class_expected["expected"])}
        class_evidence = {**evidence, runtime_pin["path"]: runtime_source}
        class_context = {"inventory": {}, "subject": {"runtime.py": (runtime_pin, runtime_source)}}
        self.assertIsNone(_q_row_call(class_expected, class_observed, "whole-program",
            class_context, class_evidence, b"runtime.ResourcePathError" + fragment))

        wrong_module_source = Path(S.__file__).read_bytes()
        wrong_module_pin = _q_pin("runtime.py", wrong_module_source)
        wrong_inputs = sorted((fixture_pin, wrong_module_pin), key=lambda pin: pin["path"])
        wrong_expected = {**class_expected, "inputs": wrong_inputs}
        wrong_observed = {**class_observed,
            "inputs_sha256": R.digest(R.encode(wrong_inputs))}
        wrong_evidence = {**evidence, wrong_module_pin["path"]: wrong_module_source}
        wrong_context = {"inventory": {},
            "subject": {"runtime.py": (wrong_module_pin, wrong_module_source)}}
        self._expect_stop(lambda: _q_row_call(wrong_expected, wrong_observed, "whole-program",
            wrong_context, wrong_evidence, b"runtime.ResourcePathError" + fragment),
            "b04_q_exception_source")
        wrong_role = {**class_expected,
            "expected": {"exception_role": "supervise.ResourcePathError"}}
        wrong_role_observed = {**class_observed,
            "observed": {"exception_role": "supervise.ResourcePathError"}}
        self._expect_stop(lambda: _q_row_call(wrong_role, wrong_role_observed,
            "whole-program", class_context, class_evidence,
            b"supervise.ResourcePathError" + fragment), "b04_q_exception_role")

    def test_b05_q03_refusal_text_and_exact_refusal_triple_consumers(self):
        fragment = b"exact refusal/triple predicates\n"
        source_pin = _q_pin("refusal-predicate.txt", fragment)
        fixture_raw = b"refusal fixture\n"
        fixture_pin = _q_pin("refusal-input.bin", fixture_raw)
        inputs = [fixture_pin]
        expected = {"control_id": "SUC003", "observation_id": "SUC003.1",
            "kind": "REFUSAL_TEXT", "expected": {"exception_role": "policy.Refusal",
                "text": "synthetic fixed refusal text"}, "inputs": inputs,
            "oracle_source": source_pin}
        observed = {"control_id": expected["control_id"],
            "observation_id": expected["observation_id"], "kind": expected["kind"],
            "inputs_sha256": R.digest(R.encode(inputs)),
            "oracle_source_sha256": source_pin["sha256"],
            "observed": deepcopy(expected["expected"])}
        evidence = {fixture_pin["path"]: fixture_raw, source_pin["path"]: fragment}
        producer = b"policy refusal assertion " + fragment
        self.assertIsNone(_q_row_call(expected, observed, "policy",
            {"inventory": {}}, evidence, producer))
        changed_text = deepcopy(observed)
        changed_text["observed"]["text"] += " drift"
        self._expect_rejection(lambda: _q_row_call(expected, changed_text, "policy",
            {"inventory": {}}, evidence, producer), "PHASE_DATA")

        triple_expected = {"outcome": "REJECT_TYPED", "code": "PATH",
            "provenance": "deliberate-contract", "context": None}
        triple_row = {"control_id": "SUC003", "observation_id": "SUC003.2",
            "kind": "REFUSAL_TRIPLE", "expected": triple_expected,
            "inputs": inputs, "oracle_source": source_pin}
        triple_observed = {"control_id": triple_row["control_id"],
            "observation_id": triple_row["observation_id"], "kind": triple_row["kind"],
            "inputs_sha256": R.digest(R.encode(inputs)),
            "oracle_source_sha256": source_pin["sha256"],
            "observed": deepcopy(triple_expected)}
        self.assertIsNone(_q_row_call(triple_row, triple_observed, "pbdep-qd",
            {"inventory": {}}, evidence, b"QD exact triple assertion " + fragment))
        triple_observed["observed"]["provenance"] = "converted-exception"
        self._expect_rejection(lambda: _q_row_call(triple_row, triple_observed, "pbdep-qd",
            {"inventory": {}}, evidence, b"QD exact triple assertion " + fragment),
            "PHASE_DATA")

    def test_b05_q04_d22_preimage_is_a_pre_call_join_not_a_predicted_context(self):
        fragment = b"D22 independently retained preimage predicate\n"
        source_pin = _q_pin("d22-predicate.txt", fragment)
        fixture_raw = b"D22 fixture identity\n"
        fixture_pin = _q_pin("d22-fixture.json", fixture_raw)
        input_raw = b"D22 input\n"
        input_pin = _q_pin("d22-input.bin", input_raw)
        fixture_root = "/fixture/q/D22"
        parent_path = fixture_root + "/source/a"
        preimage = {"schema_version": "pbdep.q-parent-preimage.v1",
            "program_sha256": "a" * 64, "binding_sha256": "b" * 64,
            "case_id": "D22.1", "parent_path": parent_path, "parent_dev": "7",
            "parent_ino": "8", "observed_ns": "120", "call_started_ns": "130"}
        preimage_raw = R.encode(preimage)
        preimage_pin = _q_pin("d22-preimage.json", preimage_raw)
        inputs = sorted((fixture_pin, input_pin), key=lambda pin: pin["path"])
        expected_context = {"kind": "D22_MISSING",
            "raw_token": "a/cancelled/../leaf.txt", "component": "cancelled",
            "errno": errno.ENOENT, "parent_suffix": "source/a"}
        observed_context = {"kind": "D22_MISSING",
            "raw_token": "a/cancelled/../leaf.txt", "component": "cancelled",
            "errno": errno.ENOENT, "parent_path": parent_path, "parent_dev": "7",
            "parent_ino": "8", "pre_observation": preimage_pin}
        refusal = {"outcome": "REJECT_TYPED", "code": "dependency_missing_component",
            "provenance": "verified-missing-component", "context": observed_context}
        expected = {"control_id": "D22", "observation_id": "D22.1",
            "kind": "REFUSAL_TRIPLE", "expected": {"outcome": "REJECT_TYPED",
                "code": "dependency_missing_component", "provenance": "verified-missing-component",
                "context": expected_context}, "inputs": inputs, "oracle_source": source_pin}
        observed = {"control_id": "D22", "observation_id": "D22.1",
            "kind": "REFUSAL_TRIPLE", "inputs_sha256": R.digest(R.encode(inputs)),
            "oracle_source_sha256": source_pin["sha256"], "observed": refusal}
        context = {"inventory": {}, "record": {"program_sha256": "a" * 64,
            "phase_binding": {"sha256": "b" * 64}},
            "fixture": {"directory": {"path": fixture_root}},
            "fixture_pin": fixture_pin, "capture_begin": 100, "capture_end": 200}
        evidence = {source_pin["path"]: fragment, fixture_pin["path"]: fixture_raw,
            input_pin["path"]: input_raw, preimage_pin["path"]: preimage_raw}
        producer = b"D22 preimage-before-prediction " + fragment
        self.assertIsNone(_q_row_call(expected, observed, "pbdep-qd", context,
                                      evidence, producer))

        drifted_preimage = {**preimage, "parent_ino": "9"}
        drifted_raw = R.encode(drifted_preimage)
        drifted_pin = _q_pin("d22-preimage-drift.json", drifted_raw)
        drifted_observed = deepcopy(observed)
        drifted_observed["observed"]["context"]["pre_observation"] = drifted_pin
        drifted_evidence = {**evidence, drifted_pin["path"]: drifted_raw}
        self._expect_rejection(lambda: _q_row_call(expected, drifted_observed, "pbdep-qd",
            context, drifted_evidence, producer), "PHASE_DATA")

    def test_b05_q05_runtime_case_consumer_keeps_unknown_exit_non_success(self):
        fragment = b"ordinary supervisor case predicate\n"
        source_pin = _q_pin("runtime-predicate.txt", fragment)
        fixture_raw = b"runtime fixture\n"
        fixture_pin = _q_pin("runtime-input.bin", fixture_raw)
        supervisor = _supervisor_record()
        supervisor_raw = R.encode(supervisor)
        supervisor_pin = _q_pin("ordinary-supervisor.json", supervisor_raw)
        log_pin = _q_pin("ordinary.log", _ORDINARY_LOG)
        inputs = [fixture_pin]
        expected = {"control_id": "ordinary", "observation_id": "ordinary.1",
            "kind": "RUNTIME_CASE", "expected": {"case": "ordinary"},
            "inputs": inputs, "oracle_source": source_pin}
        observed = {"control_id": expected["control_id"],
            "observation_id": expected["observation_id"], "kind": expected["kind"],
            "inputs_sha256": R.digest(R.encode(inputs)),
            "oracle_source_sha256": source_pin["sha256"],
            "observed": {"case": "ordinary", "supervisor": supervisor_pin,
                         "raw_log": log_pin}}
        evidence = {source_pin["path"]: fragment, fixture_pin["path"]: fixture_raw,
            supervisor_pin["path"]: supervisor_raw, log_pin["path"]: _ORDINARY_LOG}
        producer = b"nine-case finite supervisor consumer " + fragment
        self.assertIsNone(_q_row_call(expected, observed, "runtime-custody",
            {"inventory": {}}, evidence, producer))

        unknown = deepcopy(supervisor)
        unknown["whole_descendant_set_exit"] = "UNKNOWN"
        unknown["pipe_eof"] = False
        unknown["stop"] = "descendant_exit_or_capture_unknown"
        unknown_raw = R.encode(unknown)
        unknown_pin = _q_pin("unknown-supervisor.json", unknown_raw)
        unknown_observed = deepcopy(observed)
        unknown_observed["observed"]["supervisor"] = unknown_pin
        unknown_evidence = {**evidence, unknown_pin["path"]: unknown_raw}
        self._expect_stop(lambda: _q_row_call(expected, unknown_observed, "runtime-custody",
            {"inventory": {}}, unknown_evidence, producer), "b04_runtime_physical")

        drifted_counts = deepcopy(supervisor)
        drifted_counts["event_counts"]["reaped"] = 0
        drifted_counts_raw = R.encode(drifted_counts)
        drifted_counts_pin = _q_pin("ordinary-supervisor-count-drift.json", drifted_counts_raw)
        drifted_counts_observed = deepcopy(observed)
        drifted_counts_observed["observed"]["supervisor"] = drifted_counts_pin
        drifted_counts_evidence = {**evidence, drifted_counts_pin["path"]: drifted_counts_raw}
        self._expect_stop(lambda: _q_row_call(expected, drifted_counts_observed,
            "runtime-custody", {"inventory": {}}, drifted_counts_evidence, producer),
            "b04_runtime_counts")

    def test_b05_b02_api01_all_nine_data_apis_consume_the_exact_raw_envelope(self):
        fixture = QW._fixture("candidate")
        data = fixture["data"]
        raw = fixture["phase_binding_raw"]
        reference = fixture["reference"]
        compilation = W.validate_compilation(fixture["compiler"], fixture["depfile"],
            manifest_raw=QW._j(fixture["manifest"]), data=data,
            observed_binary=fixture["binary"],
            observed_dependencies=fixture["dependency_observation"],
            reference=reference, phase_binding_raw=raw)
        inventory = W.validate_inventory(fixture["listing"],
            full_roster=fixture["full"], selected_roster=fixture["selected"],
            case_map=data["case_map"], data=data, reference=reference,
            phase_binding_raw=raw)
        provenance = W.validate_pre_witness(fixture["compiler"], fixture["listing"],
            fixture["admission"], fixture["pre"], manifest_raw=QW._j(fixture["manifest"]),
            data=data, compilation=compilation, inventory=inventory,
            reference=reference, phase_binding_raw=raw)

        integration = QW._later_data("integration")
        integration_raw, integration_reference = QW._phase_context("integration", integration)
        gate = QW._later_data("gate")
        gate_raw, gate_reference = QW._phase_context("gate", gate)

        def api_call(function, phase, args, kwargs):
            phase_context = {
                "candidate": (data, raw, reference),
                "integration": (integration, integration_raw, integration_reference),
                "gate": (gate, gate_raw, gate_reference),
            }[phase]
            _data, good_raw, good_reference = phase_context

            def invoke(raw_value=None, *, reference_value=None, omit_raw=False,
                       data_value=None):
                call_args = list(args)
                call_kwargs = deepcopy(kwargs)
                call_kwargs["reference"] = (good_reference if reference_value is None
                                              else reference_value)
                if not omit_raw:
                    call_kwargs["phase_binding_raw"] = raw_value
                if data_value is not None:
                    if function in (W.validate_phase_data, W.commands):
                        call_args[1] = data_value
                    else:
                        call_kwargs["data"] = data_value
                return function(*call_args, **call_kwargs)

            return invoke, good_raw, good_reference, phase, _data

        cases = {
            "validate_phase_data": api_call(W.validate_phase_data, "candidate",
                ("candidate", data), {}),
            "commands": api_call(W.commands, "candidate",
                ("candidate", data, QW._TOOLS), {}),
            "validate_inventory": api_call(W.validate_inventory, "candidate",
                (fixture["listing"],), {"full_roster": fixture["full"],
                    "selected_roster": fixture["selected"], "case_map": data["case_map"],
                    "data": data}),
            "validate_compilation": api_call(W.validate_compilation, "candidate",
                (fixture["compiler"], fixture["depfile"]), {"manifest_raw": QW._j(fixture["manifest"]),
                    "data": data, "observed_binary": fixture["binary"],
                    "observed_dependencies": fixture["dependency_observation"]}),
            "validate_admission": api_call(W.validate_admission, "candidate",
                (fixture["compiler"], fixture["listing"], fixture["admission"]),
                {"manifest_raw": QW._j(fixture["manifest"]), "data": data,
                    "compilation": compilation, "inventory": inventory}),
            "validate_pre_witness": api_call(W.validate_pre_witness, "candidate",
                (fixture["compiler"], fixture["listing"], fixture["admission"], fixture["pre"]),
                {"manifest_raw": QW._j(fixture["manifest"]), "data": data,
                    "compilation": compilation, "inventory": inventory}),
            "validate_completion": api_call(W.validate_completion, "candidate",
                (fixture["selected_output"], fixture["capture"]),
                {"provenance": provenance, "observed_binary": fixture["binary"],
                    "observed_sources": fixture["manifest"]["files"],
                    "observed_depfile_sha256": hashlib.sha256(fixture["depfile"]).hexdigest(),
                    "observed_dependencies": fixture["dependency_observation"],
                    "data": data, "manifest_raw": QW._j(fixture["manifest"])}),
            "validate_cargo_tests": api_call(W.validate_cargo_tests, "integration",
                (QW._cargo(integration),), {"data": integration, "exit_code": 0}),
            "validate_gate": api_call(W.validate_gate, "gate",
                (QW._gate(gate),), {"data": gate, "exit_code": 0}),
        }
        self.assertEqual(tuple(cases), ("validate_phase_data", "commands",
            "validate_inventory", "validate_compilation", "validate_admission",
            "validate_pre_witness", "validate_completion", "validate_cargo_tests", "validate_gate"))
        for name, (invoke, good_raw, good_reference, phase, baseline_data) in cases.items():
            self.assertIsNotNone(invoke(good_raw), name)
            with self.assertRaises(W.Rejection) as caught:
                invoke(None)
            self.assertEqual(str(caught.exception), "BOUND", name)
            with self.assertRaises(W.Rejection) as caught:
                invoke(b"")
            self.assertEqual(str(caught.exception), "BOUND", name)
            with self.assertRaises(W.Rejection) as caught:
                invoke(b"a" * 64)
            self.assertEqual(str(caught.exception), "PHASE_BINDING", name)
            envelope = json.loads(good_raw.decode("utf-8"))
            envelope["cwd"] += "/independent-drift"
            changed_raw = R.encode(envelope)
            with self.assertRaises(W.Rejection) as caught:
                invoke(changed_raw)
            self.assertEqual(str(caught.exception), "PHASE_BINDING", name)
            unbound_reference = deepcopy(good_reference)
            phase = envelope["phase"]
            unbound_reference["operator"]["phase_bindings"][phase] = {"state": "UNBOUND"}
            unbound_reference["phase_data_sha256"][phase] = "UNBOUND"
            with self.assertRaises(W.Rejection) as caught:
                invoke(good_raw, reference_value=unbound_reference)
            self.assertEqual(str(caught.exception), "NOT_BOUND", name)

            missing_entry = deepcopy(good_reference)
            del missing_entry["operator"]["phase_bindings"][phase]
            with self.assertRaises(W.Rejection) as caught:
                invoke(good_raw, reference_value=missing_entry)
            self.assertEqual(str(caught.exception), "SHAPE", name)

            operator_unbound = deepcopy(good_reference)
            operator_unbound["operator"]["phase_bindings"][phase] = {"state": "UNBOUND"}
            with self.assertRaises(W.Rejection) as caught:
                invoke(good_raw, reference_value=operator_unbound)
            self.assertEqual(str(caught.exception), "NOT_BOUND", name)

            canonical_unbound = deepcopy(good_reference)
            canonical_unbound["phase_data_sha256"][phase] = "UNBOUND"
            with self.assertRaises(W.Rejection) as caught:
                invoke(good_raw, reference_value=canonical_unbound)
            self.assertEqual(str(caught.exception), "REFERENCE_DRIFT", name)

            legacy_key_reference = deepcopy(good_reference)
            legacy_key_reference["operator"]["muse_acceptance_sha256"] = "a" * 64
            with self.assertRaises(W.Rejection) as caught:
                invoke(good_raw, reference_value=legacy_key_reference)
            self.assertEqual(str(caught.exception), "SHAPE", name)

            with self.assertRaises(W.Rejection) as caught:
                invoke(omit_raw=True)
            self.assertIs(type(caught.exception), W.Rejection, name)
            self.assertEqual(str(caught.exception), "SHAPE", name)
            self.assertEqual(caught.exception.provenance, "converted-exception", name)

            malformed = json.loads(good_raw.decode("utf-8"))
            malformed["unrecognized_outer_key"] = "synthetic"
            malformed_raw = R.encode(malformed)
            malformed_reference = QW._reference(phase,
                {"candidate": data, "integration": integration, "gate": gate}[phase],
                phase_binding_raw=malformed_raw)
            with self.assertRaises(W.Rejection) as caught:
                invoke(malformed_raw, reference_value=malformed_reference)
            self.assertEqual(str(caught.exception), "SHAPE", name)

            def rebind_raw(raw_value):
                rebound = deepcopy(good_reference)
                rebound["operator"]["phase_bindings"][phase]["sha256"] = \
                    hashlib.sha256(raw_value).hexdigest()
                return rebound

            invalid_raw_cases = (
                (b"\xff", "PHASE_BINDING"),
                (b"{", "PHASE_BINDING"),
                (b'{"phase":NaN}\n', "PHASE_BINDING"),
                (b" " * (C.MAX_JSON + 1), "BOUND"),
            )
            for invalid_raw, expected_code in invalid_raw_cases:
                with self.assertRaises(W.Rejection) as caught:
                    invoke(invalid_raw, reference_value=rebind_raw(invalid_raw))
                self.assertEqual(str(caught.exception), expected_code, name)

            wrong_version = json.loads(good_raw.decode("utf-8"))
            wrong_version["schema_version"] = "pbdep.phase-binding.unknown"
            wrong_version_raw = R.encode(wrong_version)
            with self.assertRaises(W.Rejection) as caught:
                invoke(wrong_version_raw,
                       reference_value=rebind_raw(wrong_version_raw))
            self.assertEqual(str(caught.exception), "PHASE_BINDING", name)

            wrong_phase = json.loads(good_raw.decode("utf-8"))
            wrong_phase["phase"] = "gate" if phase != "gate" else "integration"
            wrong_phase_raw = R.encode(wrong_phase)
            with self.assertRaises(W.Rejection) as caught:
                invoke(wrong_phase_raw, reference_value=rebind_raw(wrong_phase_raw))
            self.assertEqual(str(caught.exception), "PHASE_BINDING", name)

            bool_int_binding = json.loads(good_raw.decode("utf-8"))
            bool_int_binding["data"]["subject_head"] = 1
            bool_int_raw = R.encode(bool_int_binding)
            bool_int_data = deepcopy(baseline_data)
            bool_int_data["subject_head"] = True
            with self.assertRaises(W.Rejection) as caught:
                invoke(bool_int_raw, reference_value=rebind_raw(bool_int_raw),
                       data_value=bool_int_data)
            self.assertEqual(str(caught.exception), "PHASE_DATA", name)

        nested_nonstring = deepcopy(data)
        nested_nonstring["case_map"][1] = "synthetic-nonstring-key"
        self._expect_rejection(lambda: W.validate_phase_data("candidate", nested_nonstring,
            reference=reference, phase_binding_raw=raw), "PHASE_DATA")
        nested_extra = deepcopy(data)
        nested_extra["case_map"]["unexpected"] = "synthetic-extra-key"
        self._expect_rejection(lambda: W.validate_phase_data("candidate", nested_extra,
            reference=reference, phase_binding_raw=raw), "PHASE_DATA")
        nested_numeric = deepcopy(data)
        nested_numeric["dep_bindings"][0]["source_sha256"] = 1
        self._expect_rejection(lambda: W.validate_phase_data("candidate", nested_numeric,
            reference=reference, phase_binding_raw=raw), "PHASE_DATA")

    def test_b05_b03_roster01_base_candidate_full_selected_missing_and_drift_matrix(self):
        for phase in ("base", "candidate"):
            fixture = QW._fixture(phase)
            data = fixture["data"]
            raw = fixture["phase_binding_raw"]
            reference = fixture["reference"]
            kwargs = {"case_map": data["case_map"], "data": data,
                      "reference": reference, "phase_binding_raw": raw}
            valid = W.validate_inventory(fixture["listing"],
                full_roster=fixture["full"], selected_roster=fixture["selected"], **kwargs)
            self.assertEqual(valid["selected_count"], 31)
            for role in ("full", "selected"):
                for variant in ("missing", "drift"):
                    call_values = {"full_roster": fixture["full"],
                                   "selected_roster": fixture["selected"]}
                    name = "full_roster" if role == "full" else "selected_roster"
                    if variant == "missing":
                        call_values[name] = None
                        expected_code = "TYPE"
                    else:
                        source = call_values[name]
                        line = source.splitlines(keepends=True)[0]
                        replacement = b"drifted::roster: test\n"
                        self.assertEqual(line.endswith(b"\n"), True)
                        call_values[name] = source.replace(line, replacement, 1)
                        expected_code = "REFERENCE_DRIFT"
                    with self.assertRaises(W.Rejection) as caught:
                        W.validate_inventory(fixture["listing"], **call_values, **kwargs)
                    self.assertEqual(str(caught.exception), expected_code,
                                     (phase, role, variant))

    def test_b05_b03_runtime02_owner_rechecks_roster_and_gate_pins(self):
        if _BOUND_ROOTS is None:
            raise RuntimeError("runner must bind roots before selecting controls")
        temporary_root = _verify_bound_root(_BOUND_ROOTS["temporary_roots"][0])
        self.assertEqual(str(temporary_root), _TEMPORARY_ROOT)

        def identity(path):
            observed = path.lstat()
            return {"path": str(path), "dev": str(observed.st_dev),
                "ino": str(observed.st_ino), "uid": str(observed.st_uid),
                "mode": str(stat.S_IMODE(observed.st_mode))}

        try:
            with tempfile.TemporaryDirectory(prefix="pbdep-b05-runtime-roster-",
                                              dir=_TMP_ROOT) as parent:
                root = Path(parent)
                source_root = root / "source"
                target_root = root / "target"
                state_root = root / "state"
                phase_roots = {}
                for path in (source_root, target_root, state_root):
                    path.mkdir(mode=0o700)
                for phase in ("base", "candidate", "gate"):
                    phase_roots[phase] = state_root / phase
                    phase_roots[phase].mkdir(mode=0o700)

                for phase in ("base", "candidate", "gate"):
                    if phase == "gate":
                        fixture = {"data": QW._later_data("gate"),
                                   "manifest": QW._fixture("base")["manifest"]}
                    else:
                        fixture = QW._fixture(phase)
                    data = deepcopy(fixture["data"])

                    def remap(value):
                        if type(value) is str and value.startswith("/fixture"):
                            return str(root) + value[len("/fixture"):]
                        if type(value) is list:
                            return [remap(item) for item in value]
                        if type(value) is dict:
                            return {key: remap(item) for key, item in value.items()}
                        return value

                    data = remap(data)
                    data["source_root"] = str(source_root)
                    data["target_root"] = str(target_root)
                    gate_raw = b"synthetic scripts/gate source\n"
                    if phase == "gate":
                        data["gate_sha256"] = hashlib.sha256(gate_raw).hexdigest()
                    manifest = deepcopy(fixture["manifest"])
                    manifest["source_root"] = str(source_root)
                    manifest_raw = QW._j(manifest)
                    manifest_digest = hashlib.sha256(manifest_raw).hexdigest()
                    data["manifest_sha256"] = manifest_digest
                    data["source_manifest"]["sha256"] = manifest_digest

                    operator = deepcopy(QW._reference()["operator"])
                    operator["resource_roots"] = [
                        {"directory": identity(source_root), "kind": "workspace"},
                        {"directory": identity(target_root), "kind": "workspace"},
                        {"directory": identity(temporary_root), "kind": "temporary"}]
                    state_spec = identity(state_root)
                    tmp_spec = identity(temporary_root)
                    environment = _environment()
                    environment.update({"HOME": str(root / "home"),
                        "CARGO_HOME": str(root / "cargo-home"),
                        "RUSTUP_HOME": str(root / "rustup-home"),
                        "TMPDIR": str(temporary_root), "PATH": str(root / "tools"),
                        "CARGO_TARGET_DIR": str(target_root)})
                    tools = (str(root / "tools/python3"), str(root / "tools/cargo"),
                             str(root / "tools/rustfmt"))
                    immutable = [{"path": path,
                        "sha256": hashlib.sha256(path.encode("ascii")).hexdigest(),
                        "dev": "7", "ino": str(20 + index), "size": "10",
                        "mtime_ns": "1", "mode": "493", "uid": "1000", "nlink": "1"}
                        for index, path in enumerate(tools)]
                    step_zero, freeze = _pin("step-zero"), _pin("source-freeze")
                    binding = {"schema_version": "phase-b.phase-binding.v2",
                        "phase": phase,
                        "authority_pins": {name: operator[name]["sha256"] for name in
                            ("authority", "packet", "acceptance", "review", "disposition")},
                        "cwd": str(source_root), "environment": environment,
                        "python": tools[0], "immutable_files": immutable,
                        "tmp": tmp_spec, "selection_inputs": {
                            "absent_paths": [], "aliases": []},
                        "cargo": tools[1], "rustfmt": tools[2], "data": data,
                        "step_zero_acceptance": step_zero,
                        "source_freeze_review": freeze,
                        "operator_phase_attestation": {
                            "step_zero_read_by": "Luna", "freeze_read_by": "Astra",
                            "issuer": "Muse", "step_zero_sha256": step_zero["sha256"],
                            "freeze_sha256": freeze["sha256"],
                            "data_sha256": R.digest(R.encode(data)),
                            "grants_authority": False}}
                    binding_raw = R.encode(binding)
                    reference = QW._reference(phase, data, operator=operator,
                                               phase_binding_raw=binding_raw)
                    phase_pin = reference["operator"]["phase_bindings"][phase]
                    observations = {}

                    def supply(pin, raw):
                        observations[pin["path"]] = (pin["sha256"], raw)

                    for pin in (operator["authority"], operator["packet"],
                                operator["acceptance"], operator["review"],
                                operator["disposition"], step_zero, freeze,
                                phase_pin):
                        supply(pin, pin["path"].encode("ascii"))
                    supply(phase_pin, binding_raw)
                    if phase in ("base", "candidate"):
                        supply(data["full_roster"], fixture["full"])
                        supply(data["selected_roster"], fixture["selected"])
                    else:
                        gate_path = str(source_root / "scripts/gate")
                        observations[gate_path] = (data["gate_sha256"], gate_raw)
                    for name, expected_hash in operator["instrument_sha256"].items():
                        supply({"path": "/fixture/modules/" + name,
                                "sha256": expected_hash}, name.encode("ascii"))
                    for record in immutable:
                        observations[record["path"]] = (record["sha256"],
                            b"synthetic single-link tool bytes", record)

                    def observe(path, _cap, *, retain=False, **_kwargs):
                        path = str(path)
                        if path not in observations:
                            raise AssertionError("unmapped synthetic observation path")
                        item = observations[path]
                        expected_hash, raw = item[:2]
                        record = item[2] if len(item) == 3 else {
                            "path": path, "sha256": expected_hash, "dev": "7",
                            "ino": "30", "size": str(len(raw)), "mtime_ns": "1",
                            "mode": "384", "uid": "1000", "nlink": "1"}
                        return record, raw if retain else None

                    observation_bindings = {"schema_version": "pbdep.observation-bindings.v1",
                        "state_directory": state_spec,
                        "module_paths": {name: "/fixture/modules/" + name
                                         for name in operator["instrument_sha256"]}}
                    session = R.ObservationSession(phase, reference=reference,
                        phase_binding_raw=binding_raw,
                        observation_bindings=observation_bindings,
                        reservation=_reservation())
                    def assert_capture_refused_before_start(session, expected_code):
                        events = []
                        starts = []

                        def persist_terminal(path, value, *, state_root):
                            events.append((Path(path).name, deepcopy(value)))
                            return R.encode(value)

                        slots = session.reservation["phases"][session.phase]["slots"]
                        with patch.object(R, "persist", side_effect=persist_terminal), \
                             patch.object(R, "consume_slot", side_effect=lambda *args, **kwargs:
                                 starts.append((args, kwargs))), \
                             patch.object(R.supervise, "run") as child:
                            with self.assertRaises(R.Stop) as caught:
                                session.capture(slots[0], session.commands()[0],
                                    useful_seconds=1, exit_seconds=60)
                        self.assertIs(type(caught.exception), R.Stop)
                        self.assertEqual(str(caught.exception), expected_code)
                        self.assertEqual([name for name, _value in events], ["STOP.json"])
                        self.assertEqual(starts, [])
                        child.assert_not_called()

                    with patch.object(R, "require_umask", return_value=None), \
                         patch.object(R, "file_observation", side_effect=observe):
                        self.assertEqual(session.verify_inputs()["scope"],
                                         "observed-input-pins-only")
                        if phase == "gate":
                            changed_gate = gate_raw + b"synthetic gate drift\n"
                            observations[gate_path] = (
                                hashlib.sha256(changed_gate).hexdigest(), changed_gate)
                            with self.assertRaises(R.Stop) as caught:
                                session.stable()
                            self.assertEqual(str(caught.exception), "gate_script_drift")
                            assert_capture_refused_before_start(session, "gate_script_drift")

                            expected_pin_data = deepcopy(data)
                            expected_pin_data["gate_sha256"] = hashlib.sha256(
                                b"independent expected gate bytes\n").hexdigest()
                            expected_pin_binding = deepcopy(binding)
                            expected_pin_binding["data"] = expected_pin_data
                            expected_pin_binding["operator_phase_attestation"]["data_sha256"] = \
                                R.digest(R.encode(expected_pin_data))
                            expected_pin_raw = R.encode(expected_pin_binding)
                            expected_pin_reference = QW._reference("gate", expected_pin_data,
                                operator=deepcopy(operator),
                                phase_binding_raw=expected_pin_raw)
                            expected_pin_path = expected_pin_reference["operator"][
                                "phase_bindings"]["gate"]
                            observations[expected_pin_path["path"]] = (
                                expected_pin_path["sha256"], expected_pin_raw)
                            observations[gate_path] = (data["gate_sha256"], gate_raw)
                            expected_pin_session = R.ObservationSession("gate",
                                reference=expected_pin_reference,
                                phase_binding_raw=expected_pin_raw,
                                observation_bindings=observation_bindings,
                                reservation=_reservation())
                            with self.assertRaises(R.Stop) as caught:
                                expected_pin_session.stable()
                            self.assertEqual(str(caught.exception), "gate_script_drift")
                            assert_capture_refused_before_start(
                                expected_pin_session, "gate_script_drift")

                            def missing_gate(path, _cap, *, retain=False, **_kwargs):
                                if str(path) == gate_path:
                                    raise FileNotFoundError(gate_path)
                                return observe(path, _cap, retain=retain, **_kwargs)
                            with patch.object(R, "file_observation",
                                              side_effect=missing_gate):
                                with self.assertRaises(FileNotFoundError):
                                    expected_pin_session.stable()
                                assert_capture_refused_before_start(
                                    expected_pin_session, "adapter_capture_failure")
                            continue
                        for role in ("full_roster", "selected_roster"):
                            pin = data[role]
                            positive_hash, positive_raw = observations[pin["path"]]
                            for variant in ("missing", "drift"):
                                if variant == "missing":
                                    observations[pin["path"]] = None
                                else:
                                    changed = positive_raw + b"synthetic-roster-drift\n"
                                    observations[pin["path"]] = (
                                        hashlib.sha256(changed).hexdigest(), changed)
                                if variant == "missing":
                                    def missing_observation(path, _cap, *, retain=False, **_kwargs):
                                        if str(path) == pin["path"]:
                                            raise FileNotFoundError(str(path))
                                        return observe(path, _cap, retain=retain, **_kwargs)
                                    with patch.object(R, "file_observation",
                                                      side_effect=missing_observation):
                                        with self.assertRaises(FileNotFoundError):
                                            session.stable()
                                        assert_capture_refused_before_start(
                                            session, "adapter_capture_failure")
                                else:
                                    with self.assertRaises(R.Stop) as caught:
                                        session.stable()
                                    self.assertEqual(str(caught.exception), "pin_drift",
                                                     (phase, role, variant))
                                    assert_capture_refused_before_start(session, "pin_drift")
                                observations[pin["path"]] = (positive_hash, positive_raw)
        finally:
            _verify_bound_root(_BOUND_ROOTS["temporary_roots"][0])

    def test_b05_b03_start01_stable_gate_precedes_start_and_is_rechecked_before_child(self):
        events = []
        request = {"cargo": "/fixture/tools/cargo", "source_root": "/fixture/source",
            "environment": _environment(), "raw_cap": 4096, "chain_id": "c" * 64}
        reservation = {"program_sha256": "a" * 64,
            "chain_id": "c" * 64,
            "allocation_ids": {"compile": "synthetic-allocation"}}
        context = object.__new__(D.Custody)
        context.lock = object()
        context.request = request
        context.request_pin = _pin("capture-request")
        context.reservation_pin = _pin("capture-reservation")
        context.state = Path("/fixture/state")
        context.root = Path("/fixture/state/discovery")
        context.spent = []
        context.refresh = lambda: events.append("refresh")
        context.resources = lambda **_kwargs: events.append("resources")
        context.verify_retained = lambda: events.append("retained")
        context.remember = lambda path: events.append(("remember", str(path)))
        context.clock = SimpleNamespace(reservation=reservation,
            admit=lambda _attempt, _spent: (30, 60), running_since=None, child_ns=0)
        stable_calls = []

        def stable():
            stable_calls.append(None)
            events.append("stable")
            if len(stable_calls) == 2:
                raise R.Stop("synthetic-later-freshness-refusal")

        command = D.compile_command(request)
        persisted_start = []

        def persist_start(path, value, *, state_root):
            self.assertEqual(Path(path), context.root / "compile-START.json")
            self.assertEqual(state_root, context.state)
            persisted_start.append(deepcopy(value))
            events.append(("persist", Path(path).name))
            return R.encode(value)

        with patch.object(R, "retained_bytes", return_value=0), \
             patch.object(R, "capture_budget", return_value=4096), \
             patch.object(R, "persist", side_effect=persist_start), \
             patch.object(D.supervise, "run") as child:
            with self.assertRaises(R.Stop) as caught:
                D.Custody.capture(context, "compile", command, stable)
        self.assertEqual(str(caught.exception), "synthetic-later-freshness-refusal")
        self.assertEqual(events[:3], ["refresh", "stable", "resources"])
        self.assertEqual(len(persisted_start), 1)
        self.assertEqual(persisted_start[0]["chain_id"], request["chain_id"])
        self.assertEqual(persisted_start[0]["allocation_id"],
                         reservation["allocation_ids"]["compile"])
        start_events = [index for index, item in enumerate(events)
                        if item == ("persist", "compile-START.json")]
        self.assertEqual(len(start_events), 1)
        self.assertEqual(events[start_events[0] + 2:start_events[0] + 4],
                         ["refresh", "stable"])
        child.assert_not_called()

    def test_b05_b03_lifecycle01_post_start_inventory_drift_precedes_child(self):
        state = Path("/fixture/state")
        request = {"cargo": "/fixture/tools/cargo", "source_root": "/fixture/source",
            "environment": _environment(), "raw_cap": 4096, "chain_id": "e" * 64}
        request["state_directory"] = {"path": str(state)}
        reservation = {"program_sha256": "a" * 64,
            "allocation_ids": {"compile": "synthetic-allocation"}}
        command = D.compile_command(request)
        base_inventory = {"state_directory": {"path": str(state)}, "files": {},
            "directories": {"discovery": {"path": str(state / "discovery"),
                "dev": "7", "ino": "8", "uid": "1000", "mode": "448"}},
            "total_bytes": 0}

        def trial(variant):
            context = object.__new__(D.Custody)
            context.lock = object()
            context.request = deepcopy(request)
            context.request_pin = _pin("lifecycle-request")
            context.reservation_pin = _pin("lifecycle-reservation")
            context.state = state
            context.root = state / "discovery"
            context.spent = []
            context.inventory = deepcopy(base_inventory)
            context.refresh = lambda: None
            context.resources = lambda **_kwargs: None
            context.clock = SimpleNamespace(reservation=reservation,
                admit=lambda _attempt, _spent: (30, 60), running_since=None, child_ns=0)
            persisted = []
            inventory_reads = []

            def state_inventory(_spec):
                current = deepcopy(context.inventory)
                inventory_reads.append(None)
                if len(inventory_reads) == 2:
                    relative = "discovery/compile-START.json"
                    if variant == "change":
                        current["files"][relative]["ino"] = "9999"
                    elif variant == "add":
                        current["files"]["discovery/external.json"] = {
                            "path": str(state / "discovery/external.json"),
                            "sha256": "f" * 64, "dev": "7", "ino": "9000",
                            "size": "1", "mtime_ns": "1", "mode": "384",
                            "uid": "1000", "nlink": "1"}
                        current["total_bytes"] += 1
                    elif variant == "remove":
                        del current["files"][relative]
                        current["total_bytes"] = 0
                return current

            def persist_start(path, value, *, state_root):
                self.assertEqual(Path(path), context.root / "compile-START.json")
                self.assertEqual(state_root, state)
                persisted.append(deepcopy(value))
                return R.encode(value)

            def observe(path, _cap, *, retain=False, **_kwargs):
                raw = R.encode(persisted[-1])
                record = {"path": str(path), "sha256": hashlib.sha256(raw).hexdigest(),
                    "dev": "7", "ino": "10", "size": str(len(raw)),
                    "mtime_ns": "1", "mode": "384", "uid": "1000", "nlink": "1"}
                return record, raw if retain else None

            with patch.object(D, "state_inventory", side_effect=state_inventory), \
                 patch.object(R, "persist", side_effect=persist_start), \
                 patch.object(R, "file_observation", side_effect=observe), \
                 patch.object(R, "retained_bytes", return_value=0), \
                 patch.object(R, "capture_budget", return_value=4096), \
                 patch.object(D.supervise, "run") as child:
                self._expect_stop(lambda: D.Custody.capture(
                    context, "compile", command, lambda: None),
                    "b04_retained_inventory_drift")
                self.assertEqual(len(persisted), 1)
                self.assertEqual(len(inventory_reads), 2)
                child.assert_not_called()

        for variant in ("change", "add", "remove"):
            trial(variant)

        # Exercise the accepted Clock.admit/check code with fixed time values.
        # The first admission fits; the second expires only after START and a
        # matching second retained-inventory comparison, before child dispatch.
        clock_reservation = {"schema_version": "pbdep.b04-reservation.v1",
            "program_sha256": "a" * 64, "chain_id": request["chain_id"],
            "allocation_ids": {"compile": "1" * 64, "list": "2" * 64,
                "witness": "3" * 64},
            "anchor": {"wall_ns": str(100 * D.NANO), "monotonic_ns": "1000000",
                "boot_id": "synthetic-boot-id"},
            "master_expiry_utc_seconds": 40000,
            "discovery": {"prep": 120, "close": 60, "total": 360,
                "compile": 30, "list": 30},
            "witness": {"total": 270}, "review_seconds": 60,
            "return_seconds": 60, "later_seconds": 60}
        context = object.__new__(D.Custody)
        context.lock = object()
        context.request = deepcopy(request)
        context.request_pin = _pin("lifecycle-clock-request")
        context.reservation_pin = _pin("lifecycle-clock-reservation")
        context.state, context.root = state, state / "discovery"
        context.spent = []
        context.inventory = deepcopy(base_inventory)
        context.refresh = lambda: None
        context.resources = lambda **_kwargs: None
        clock = object.__new__(D.Clock)
        clock.reservation = clock_reservation
        clock.stage, clock.started, clock.child_ns = "discovery", 1_000_000, 0
        clock.close_started = clock.running_since = None
        clock.aw, clock.am = 100 * D.NANO, 1_000_000
        context.clock = clock
        persisted = []
        inventory_reads = []

        def matching_inventory(_spec):
            inventory_reads.append(None)
            return deepcopy(context.inventory)

        def persist_start(path, value, *, state_root):
            persisted.append(deepcopy(value))
            return R.encode(value)

        def observe(path, _cap, *, retain=False, **_kwargs):
            raw = R.encode(persisted[-1])
            record = {"path": str(path), "sha256": hashlib.sha256(raw).hexdigest(),
                "dev": "7", "ino": "10", "size": str(len(raw)),
                "mtime_ns": "1", "mode": "384", "uid": "1000", "nlink": "1"}
            return record, raw if retain else None

        with patch.object(D, "state_inventory", side_effect=matching_inventory), \
             patch.object(R, "persist", side_effect=persist_start), \
             patch.object(R, "file_observation", side_effect=observe), \
             patch.object(R, "retained_bytes", return_value=0), \
             patch.object(R, "capture_budget", return_value=4096), \
             patch.object(R, "boot_id", return_value="synthetic-boot-id"), \
             patch.object(D.time, "monotonic_ns", side_effect=(1_000_001, 1_000_002,
                 1_000_003, 1_000_004, 1_000_005, 1_000_006, 1_000_007,
                 1_000_000 + 360 * D.NANO + 1)), \
             patch.object(D.time, "time_ns", return_value=100 * D.NANO + 2), \
             patch.object(D.supervise, "run") as child:
            with self.assertRaises(R.Stop) as caught:
                D.Custody.capture(context, "compile", command, lambda: None)
        self.assertEqual(str(caught.exception), "b04_stage_expired")
        self.assertEqual(len(persisted), 1)
        self.assertEqual(len(inventory_reads), 2)
        child.assert_not_called()

    def test_b05_b03_raw01_retains_supervisor_and_log_before_native_parse(self):
        events = []
        request = {"cargo": "/fixture/tools/cargo", "source_root": "/fixture/source",
            "environment": _environment(), "raw_cap": 4096, "chain_id": "d" * 64}
        reservation = {"program_sha256": "a" * 64, "chain_id": request["chain_id"],
            "allocation_ids": {"compile": "synthetic-allocation"}}
        context = object.__new__(D.Custody)
        context.lock = object()
        context.request = request
        context.request_pin = _pin("capture-request")
        context.reservation_pin = _pin("capture-reservation")
        context.state = Path("/fixture/state")
        context.root = context.state / "discovery"
        context.spent = []
        context.history = _history()
        context.last_scan = {"total_bytes": 0}
        context.refresh = lambda: events.append("refresh")
        context.resources = lambda **_kwargs: events.append("resources")
        context.verify_retained = lambda: events.append("retained")
        context.remember = lambda path: events.append(("remember", Path(path).name))
        context.clock = SimpleNamespace(reservation=reservation,
            admit=lambda _attempt, _spent: (30, 60), check=lambda: events.append("clock"),
            running_since=None, child_ns=0)

        def persist_owned(name, value):
            return R.persist(context.root / name, value, state_root=context.state)

        context.persist = persist_owned
        command = D.compile_command(request)
        receipt = _supervisor_record()
        retained = []

        def persist(path, value, *, state_root):
            name = Path(path).name
            events.append(("persist", name))
            retained.append((name, deepcopy(value)))
            return R.encode(value)

        def observe(path, _cap, *, retain=False, **_kwargs):
            name = Path(path).name
            events.append(("observe", name, retain))
            raw = _ORDINARY_LOG if name == "compile.log" else b"synthetic retained bytes\n"
            record = {"path": str(path), "sha256": hashlib.sha256(raw).hexdigest(),
                "dev": "7", "ino": "8", "size": str(len(raw)), "mtime_ns": "1",
                "mode": "384", "uid": "1000", "nlink": "1"}
            return record, raw if retain else None

        validate = D.validate_supervisor_record

        def record_validate(value):
            events.append("native-schema-validate")
            return validate(value)

        normalize = R.normalize_capture

        def capture_normalize(value, raw, before, after, expected_exit):
            events.append("production-normalize")
            return normalize(value, raw, before, after, expected_exit)

        with patch.object(R, "retained_bytes", return_value=0), \
             patch.object(R, "capture_budget", return_value=4096), \
             patch.object(R, "persist", side_effect=persist), \
             patch.object(R, "file_observation", side_effect=observe), \
             patch.object(R, "sync_directory", side_effect=lambda _path: events.append("sync")), \
             patch.object(R, "normalize_capture", side_effect=capture_normalize), \
             patch.object(D, "validate_supervisor_record", side_effect=record_validate), \
             patch.object(D.time, "monotonic_ns", side_effect=(100, 200)), \
             patch.object(D.supervise, "run", return_value=receipt) as child:
            raw_log, normalized, spec = D.Custody.capture(context, "compile", command,
                lambda: events.append("stable"))

        child.assert_called_once()
        self.assertEqual(raw_log, _ORDINARY_LOG)
        self.assertEqual(normalized["exit_code"], 0)
        self.assertIn("raw", spec)
        raw_record = next(value for name, value in retained if name == "compile-RAW.json")
        self.assertEqual(raw_record["supervisor"], receipt)
        self.assertEqual(raw_record["manager_status"], D.AXES["manager_status"])
        self.assertEqual(raw_record["aggregate_resource_status"],
                         D.AXES["aggregate_resource_status"])
        raw_index = events.index(("persist", "compile-RAW.json"))
        log_index = events.index(("observe", "compile.log", True))
        schema_index = events.index("native-schema-validate")
        normalize_index = events.index("production-normalize")
        self.assertLess(raw_index, log_index)
        self.assertLess(log_index, schema_index)
        self.assertLess(schema_index, normalize_index)

    def test_b05_b03_raw01_runtime_owner_persists_axes_before_normalization(self):
        """Runtime owner retains raw receipt/resource axes before parsing it."""
        data = QW._format_data()
        binding_value = json.loads(QW._phase_binding_raw("format", data).decode("utf-8"))
        binding_value["cargo"] = "/fixture/tools/cargo"
        binding_value["rustfmt"] = "/fixture/tools/rustfmt"
        binding_raw = R.encode(binding_value)
        reference = QW._reference("format", data, phase_binding_raw=binding_raw)
        session = object.__new__(R.ObservationSession)
        session.phase = "format"
        session.reference = reference
        session.binding_raw = binding_raw
        session.binding = binding_value
        session.data = data
        session.reservation = _reservation()
        session.state_spec = {"path": "/fixture/runtime-state"}
        session.state_root = Path(session.state_spec["path"])
        session.phase_root = session.state_root / "format"
        session.temporary_roots = (_TEMPORARY_ROOT,)
        session.format_completed = False
        session.spent = []
        session.captures = {}
        session.resource_facts = {"sample_count": 0, "peak_total_bytes": 0,
            "peak_temporary_bytes": 0, "peak_capture_bytes": 0,
            "vanished_by_class": {"entry-stat": 0, "directory-scan": 0},
            "vanished": [], "sampled_entry_types": {"directory": 0,
                "regular": 0, "symlink": 0, "socket": 0}}
        session.last_resource_scan = None
        # The independent runtime input controls exercise verify_inputs; this
        # test isolates only the ordering within the accepted capture owner.
        session.verify_inputs = lambda: None

        events, retained = [], []
        facts = {"total_bytes": 12, "temporary_bytes": 3, "capture_bytes": 4,
            "vanished_count": 0, "vanished": [],
            "vanished_by_class": {"entry-stat": 0, "directory-scan": 0},
            "sampled_entry_types": {"directory": 1, "regular": 2,
                "symlink": 0, "socket": 0}}
        receipt = _supervisor_record()
        normalizer = R.normalize_capture

        def persist(path, value, *, state_root):
            events.append(("persist", Path(path).name))
            retained.append((Path(path).name, deepcopy(value)))
            return R.encode(value)

        def observe(path, _cap, *, retain=False, **_kwargs):
            name = Path(path).name
            events.append(("observe", name, retain))
            raw = _ORDINARY_LOG if name == "13.log" else b"synthetic format source\n"
            record = {"path": str(path), "sha256": hashlib.sha256(raw).hexdigest(),
                "dev": "7", "ino": "8", "size": str(len(raw)), "mtime_ns": "1",
                "mode": "384", "uid": "1000", "nlink": "1"}
            return record, raw if retain else None

        def normalize(value, raw, before, after, expected_exit):
            events.append("normalize")
            return normalizer(value, raw, before, after, expected_exit)

        def child(*_args, **_kwargs):
            events.append("supervisor-return")
            return receipt

        command = session.commands()[0]
        with patch.object(R, "scan_resources", return_value=facts), \
             patch.object(R, "retained_bytes", return_value=0), \
             patch.object(R, "capture_budget", return_value=4096), \
             patch.object(R, "consume_slot", side_effect=lambda *_args, **_kwargs:
                 events.append("START")), \
             patch.object(R, "persist", side_effect=persist), \
             patch.object(R, "file_observation", side_effect=observe), \
             patch.object(R, "sync_directory", side_effect=lambda _path: events.append("sync")), \
             patch.object(R, "normalize_capture", side_effect=normalize), \
             patch.object(R.supervise, "run", side_effect=child), \
             patch.object(R.time, "monotonic_ns", side_effect=(100, 200, 300)):
            result = R.ObservationSession._capture(session, "13", command,
                useful_seconds=1, exit_seconds=60)

        raw_record = next(value for name, value in retained if name == "13-RAW.json")
        self.assertEqual(raw_record["supervisor"], receipt)
        self.assertEqual(raw_record["supervisor"]["leader_returncode"], 0)
        self.assertEqual(raw_record["supervisor"]["whole_descendant_set_exit"],
                         "PROVEN_ECHILD")
        self.assertIs(raw_record["supervisor"]["pipe_eof"], True)
        self.assertEqual(raw_record["supervisor"]["log_bytes"], len(_ORDINARY_LOG))
        self.assertEqual(raw_record["supervisor"]["discarded_log_bytes"], 0)
        self.assertEqual(raw_record["manager_status"], "NOT_OBSERVED_BY_ADAPTER")
        self.assertEqual(raw_record["aggregate_resource_state_at_terminal_write"], "UNKNOWN")
        self.assertEqual(raw_record["resource_samples"], {
            "sample_count": 1, "peak_total_bytes": 12,
            "peak_temporary_bytes": 3, "peak_capture_bytes": 4,
            "vanished_by_class": {"entry-stat": 0, "directory-scan": 0},
            "vanished": [], "sampled_entry_types": {
                "directory": 1, "regular": 2, "symlink": 0, "socket": 0}})
        self.assertEqual(raw_record["last_sampled_resources"], facts)
        self.assertEqual(result["manager_status"], "NOT_OBSERVED_BY_ADAPTER")
        self.assertEqual(result["aggregate_resource_state_at_terminal_write"], "UNKNOWN")
        self.assertEqual(result["raw"], _ORDINARY_LOG)
        self.assertEqual(result["supervisor"], receipt)
        self.assertLess(events.index(("persist", "13-RAW.json")),
                        events.index(("observe", "13.log", True)))
        self.assertLess(events.index(("observe", "13.log", True)), events.index("normalize"))
        self.assertLess(events.index(("persist", "13-RAW.json")),
                        events.index("normalize"))

    def test_b05_b02_raw01_phase_raw_and_canonical_data_pins_are_independent(self):
        fixture = QW._fixture("candidate")
        data = deepcopy(fixture["data"])
        raw = fixture["phase_binding_raw"]
        reference = fixture["reference"]
        changed_data = deepcopy(data)
        changed_data["dep_tokens"] = [*changed_data["dep_tokens"],
            "crates/beacond/src/added.rs"]
        changed_binding = json.loads(raw.decode("utf-8"))
        changed_binding["data"] = changed_data
        changed_raw = R.encode(changed_binding)
        raw_bound_only = QW._reference("candidate", changed_data,
            phase_binding_raw=changed_raw)
        raw_bound_only["phase_data_sha256"]["candidate"] = reference["phase_data_sha256"]["candidate"]
        with self.assertRaises(W.Rejection) as caught:
            W.validate_phase_data("candidate", changed_data, reference=raw_bound_only,
                phase_binding_raw=changed_raw)
        self.assertEqual(str(caught.exception), "REFERENCE_DRIFT")

    def test_b05_a2_artifact01_native_artifact_rows_bind_canonical_bytes_and_events(self):
        stamp = 1_788_000_000_000
        workspace = "123e4567-e89b-12d3-a456-426614174000"
        project = "123e4567-e89b-12d3-a456-426614174001"
        mission = "123e4567-e89b-12d3-a456-426614174002"
        resident = "123e4567-e89b-12d3-a456-426614174003"
        configuration = "123e4567-e89b-12d3-a456-426614174004"
        store_root = "/fixture/a2-store"
        store = store_root + "/pa-123e4567-e89b-12d3-a456-426614174005.sqlite3"
        case = ("increasing", "increasing_stamp_identical_inputs_record_twice",
                (stamp, stamp + 1), 2, None, None)
        returned = [_artifact_returned(workspace, resident, configuration, at, index + 1)
                    for index, at in enumerate((stamp, stamp + 1))]
        digests = [record["digest"]["value"] for record in returned]
        rows = [{"digest": record["digest"]["value"],
            "canonical_hex": record["canonical_hex"],
            "recorded_at_ms": record["recorded_at_ms"],
            "creation_event_seq": record["creation_event_seq"]} for record in returned]
        events = []
        content_bytes = b"# Task Tracker outcome\n\nThe exporter landed.\n"
        for record in returned:
            payload = {"artifact": {"digest": record["digest"],
                "canonicalBytesBase64": base64.b64encode(bytes.fromhex(record["canonical_hex"])).decode("ascii"),
                "contentDigest": {"algorithm": "sha256", "canonicalizer": "beacon.raw.v1",
                    "value": hashlib.sha256(content_bytes).hexdigest()},
                "contentBytes": len(content_bytes), "actor": "owner",
                "recordedAtMs": record["recorded_at_ms"]}}
            events.append({"sequence": record["creation_event_seq"],
                "payload_json": json.dumps(payload, sort_keys=True, separators=(",", ":")),
                "occurred_at_ms": record["recorded_at_ms"]})
        witness = {"schema_version": "artifact.identity.phase_a.v1", "case": "increasing",
            "store": store, "workspace": workspace, "fixture_project": project,
            "fixture_mission": mission, "timestamps": [stamp, stamp + 1],
            "digests": digests, "canonical_hex": [r["canonical_hex"] for r in returned],
            "reopen_read_result": {"state": "not_requested"}, "returned": returned,
            "row_count": 2, "event_count": 2, "rows": rows, "events": events}
        result = C._witness(witness, case, store_root)
        self.assertEqual(result, {"case": "increasing", "store": store,
            "workspace": workspace, "digests": digests, "row_count": 2, "event_count": 2})

        drifted = deepcopy(witness)
        payload = json.loads(drifted["events"][0]["payload_json"])
        payload["artifact"]["contentBytes"] -= 1
        drifted["events"][0]["payload_json"] = json.dumps(payload, sort_keys=True,
            separators=(",", ":"))
        with self.assertRaises(C.Rejection) as caught:
            C._witness(drifted, case, store_root)
        self.assertIs(type(caught.exception), C.Rejection)
        self.assertIs(caught.exception.code, C.Code.ROW_EVENT)

    def test_b05_d04_migration01_synthetic_sql_bytes_are_manifest_and_runtime_joined(self):
        sql = b"CREATE TABLE synthetic_items (id INTEGER PRIMARY KEY);\n"
        canonical = "crates/beacond/migrations/0001_initial.sql"
        token = "crates/beacond/src/../migrations/0001_initial.sql"
        with tempfile.TemporaryDirectory(prefix="pbdep-b05-migration-", dir=_TMP_ROOT) as root:
            _write_fixture_file(root, canonical, sql)
            (Path(root) / "crates/beacond/src").mkdir(parents=True)
            data, manifest, reference, binding_raw, _ = _runtime_dependency_fixture(
                root, token, canonical, sql)
            executable = root + "-target/debug/deps/beacond-0123456789abcdef"
            depfile = ((executable + ".d: " + token + "\n\n" + executable + ": " + token
                + "\n\n" + token + ":\n\n# env-dep:CARGO_MANIFEST_DIR="
                + root + "/crates/beacond\n# env-dep:CARGO_PKG_VERSION=0.1.0\n").encode())
            artifact = W.validate_dependency_artifact(depfile, source_root=root,
                executable=executable, dep_tokens=data["dep_tokens"],
                dep_binding_schema=data["dep_binding_schema"],
                dep_bindings=data["dep_bindings"], source_hashes=manifest["files"])
            self.assertEqual(artifact["depfile_sha256"], hashlib.sha256(depfile).hexdigest())
            expected = _independent_observation_preimage(root, data, canonical, sql)
            source_directory = Path(root) / "crates/beacond/src"
            source_directory_stat = source_directory.lstat()
            expected["directories"]["crates/beacond/src"] = {
                "path": str(source_directory), "dev": str(source_directory_stat.st_dev),
                "ino": str(source_directory_stat.st_ino),
                "mode": str(stat.S_IMODE(source_directory_stat.st_mode)),
                "uid": str(source_directory_stat.st_uid)}
            self.assertEqual(R.observe_dependencies(manifest, data, reference=reference,
                phase_binding_raw=binding_raw, expected=expected), expected)
            path = Path(root) / canonical
            path.write_bytes(b"CREATE TABLE drifted_items (id INTEGER PRIMARY KEY);\n")
            self._expect_stop(lambda: R.observe_dependencies(manifest, data,
                reference=reference, phase_binding_raw=binding_raw, expected=expected),
                "dependency_canonical_join")


_EXPLICIT_SELECTORS = (
    "test_b05_qw01_historical_354_runner_is_ordered_and_independently_pinned",
    "test_b05_ne01_native_nested_events_reach_supervisor_record",
    "test_b05_ne02_native_classified_error_cannot_be_success",
    "test_b05_ne03_physical_exit_eof_and_time_axes_are_separate",
    "test_b05_ne04_event_shape_integer_and_float_boundaries",
    "test_b05_ne05_full_log_bytes_and_digest_are_joined",
    "test_b05_ne06_unknown_native_receipt_cannot_normalize_as_success",
    "test_b05_env01_discovery_request_record_passes_accepted_entry_guard",
    "test_b05_env02_discovery_refresh_rejects_forbidden_environment_name",
    "test_b05_env03_initial_request_path_grammar_and_target_join",
    "test_b05_env04_discovery_fixed_literals_are_checked_at_refresh",
    "test_b05_env05_discovery_tmpdir_joins_request_temporary_root",
    "test_b05_env06_discovery_target_joins_own_registered_root",
    "test_b05_env07_phase_data_path_uses_whole_program_rejection",
    "test_b05_env08_later_phase_target_joins_data_and_registered_root",
    "test_b05_env09_a2_target_joins_native_provenance_and_registered_root",
    "test_b05_a2_provenance01_pre_and_post_witness_joins_use_accepted_consumers",
    "test_b05_a2_classify01_direct_framing_uses_independent_synthetic_witnesses",
    "test_b05_q_aggregate01_family_registry_is_checked_before_inventory",
    "test_b05_q_aggregate02_full_synthetic_chain_and_isolated_join_guards",
    "test_b05_env10_prior_q_input_environment_is_checked_before_oracle_consumption",
    "test_b05_entry01_initial_request_rejection_precedes_any_state_write",
    "test_b05_entry02_refresh_refusal_emits_no_start_receipt",
    "test_b05_entry03_initial_environment_refusal_precedes_writes_on_both_paths",
    "test_b05_prior01_anchored_v2_genesis_joins_complete_inventory",
    "test_b05_prior02_anchored_interval_and_capture_order_edges",
    "test_b05_prior03_discovery_inventory_is_a_fixed_observation",
    "test_b05_prior04_multiphase_records_reach_isolated_overlap_guard",
    "test_b05_source01_synthetic_source_snapshot_joins_manifest_and_git_pin",
    "test_b05_gate01_prior_gate_pin_has_matching_and_isolated_drift_cases",
    "test_b05_names01_current_original_reference_and_v2_consumers_are_closed",
    "test_b05_roster01_both_roster_pins_are_wired_before_first_capture",
    "test_b05_ex01_resource_path_error_is_runtime_owner",
    "test_b05_ex02_supervisor_does_not_alias_runtime_exception",
    "test_b05_ne07_supervisor_positive_is_source_and_schema_pinned",
    "test_b05_adm01_standalone_admission_positive_uses_exact_current_join",
    "test_b05_adm02_binary_and_depfile_malformations_are_consistent_and_isolated",
    "test_b05_phase01_raw_envelope_and_canonical_data_are_independently_bound",
    "test_b05_qual01_zero_usage_and_phase_allocation_boundaries",
    "test_b05_qual02_all_phase_reservation_and_order_conservation",
    "test_b05_hist01_resource_history_is_cumulative_at_both_caps",
    "test_b05_hist02_custody_resources_accumulates_scans_and_enforces_caps",
    "test_b05_d_adapt01_current_dependency_grammar_and_typed_bindings",
    "test_b05_d_rd_inventory01_independent_exact_triples_are_preserved",
    "test_b05_d_legacy01_named_predecessor_parent_rule_stays_non_authoritative",
    "test_b05_d_raw01_current_raw_depfile_adapter_preserves_exact_grammar",
    "test_b05_d_fs01_synthetic_dependency_component_controls",
    "test_b05_d_preimage01_missing_component_preimage_precedes_refusal",
    "test_b05_d_compose01_current_synthetic_base_and_candidate_chains",
    "test_b05_q01_tagged_values_preserve_native_type_and_reject_unlisted_values",
    "test_b05_q06_assertion_return_is_closed_to_two_source_families",
    "test_b05_q02_class_only_and_source_closed_exception_predicates",
    "test_b05_q03_refusal_text_and_exact_refusal_triple_consumers",
    "test_b05_q04_d22_preimage_is_a_pre_call_join_not_a_predicted_context",
    "test_b05_q05_runtime_case_consumer_keeps_unknown_exit_non_success",
    "test_b05_b02_api01_all_nine_data_apis_consume_the_exact_raw_envelope",
    "test_b05_b03_roster01_base_candidate_full_selected_missing_and_drift_matrix",
    "test_b05_b03_runtime02_owner_rechecks_roster_and_gate_pins",
    "test_b05_b03_start01_stable_gate_precedes_start_and_is_rechecked_before_child",
    "test_b05_b03_lifecycle01_post_start_inventory_drift_precedes_child",
    "test_b05_b03_raw01_retains_supervisor_and_log_before_native_parse",
    "test_b05_b03_raw01_runtime_owner_persists_axes_before_normalization",
    "test_b05_b02_raw01_phase_raw_and_canonical_data_pins_are_independent",
    "test_b05_a2_artifact01_native_artifact_rows_bind_canonical_bytes_and_events",
    "test_b05_d04_migration01_synthetic_sql_bytes_are_manifest_and_runtime_joined",
)

_SLOT_SELECTOR_GROUPS = {
    "SQ1": frozenset((
        "test_b05_qw01_historical_354_runner_is_ordered_and_independently_pinned",
        "test_b05_ne01_native_nested_events_reach_supervisor_record",
        "test_b05_ne02_native_classified_error_cannot_be_success",
        "test_b05_ne03_physical_exit_eof_and_time_axes_are_separate",
        "test_b05_ne04_event_shape_integer_and_float_boundaries",
        "test_b05_ne05_full_log_bytes_and_digest_are_joined",
        "test_b05_ne06_unknown_native_receipt_cannot_normalize_as_success",
        "test_b05_env07_phase_data_path_uses_whole_program_rejection",
        "test_b05_ne07_supervisor_positive_is_source_and_schema_pinned",
        "test_b05_adm01_standalone_admission_positive_uses_exact_current_join",
        "test_b05_adm02_binary_and_depfile_malformations_are_consistent_and_isolated",
        "test_b05_phase01_raw_envelope_and_canonical_data_are_independently_bound",
        "test_b05_qual01_zero_usage_and_phase_allocation_boundaries",
        "test_b05_qual02_all_phase_reservation_and_order_conservation",
        "test_b05_d_adapt01_current_dependency_grammar_and_typed_bindings",
        "test_b05_d_rd_inventory01_independent_exact_triples_are_preserved",
        "test_b05_d_legacy01_named_predecessor_parent_rule_stays_non_authoritative",
        "test_b05_d_raw01_current_raw_depfile_adapter_preserves_exact_grammar",
        "test_b05_q01_tagged_values_preserve_native_type_and_reject_unlisted_values",
        "test_b05_q06_assertion_return_is_closed_to_two_source_families",
        "test_b05_q02_class_only_and_source_closed_exception_predicates",
        "test_b05_q03_refusal_text_and_exact_refusal_triple_consumers",
        "test_b05_q04_d22_preimage_is_a_pre_call_join_not_a_predicted_context",
        "test_b05_q05_runtime_case_consumer_keeps_unknown_exit_non_success",
        "test_b05_b02_api01_all_nine_data_apis_consume_the_exact_raw_envelope",
        "test_b05_b02_raw01_phase_raw_and_canonical_data_pins_are_independent",
        "test_b05_env10_prior_q_input_environment_is_checked_before_oracle_consumption",
        "test_b05_q_aggregate01_family_registry_is_checked_before_inventory",
        "test_b05_q_aggregate02_full_synthetic_chain_and_isolated_join_guards",
        "test_b05_ex01_resource_path_error_is_runtime_owner",
        "test_b05_ex02_supervisor_does_not_alias_runtime_exception",
    )),
    "SQ2": frozenset((
        "test_b05_env09_a2_target_joins_native_provenance_and_registered_root",
        "test_b05_a2_provenance01_pre_and_post_witness_joins_use_accepted_consumers",
        "test_b05_a2_classify01_direct_framing_uses_independent_synthetic_witnesses",
        "test_b05_names01_current_original_reference_and_v2_consumers_are_closed",
        "test_b05_roster01_both_roster_pins_are_wired_before_first_capture",
        "test_b05_a2_artifact01_native_artifact_rows_bind_canonical_bytes_and_events",
    )),
    "SQ3": frozenset((
        "test_b05_env01_discovery_request_record_passes_accepted_entry_guard",
        "test_b05_env02_discovery_refresh_rejects_forbidden_environment_name",
        "test_b05_env03_initial_request_path_grammar_and_target_join",
        "test_b05_env04_discovery_fixed_literals_are_checked_at_refresh",
        "test_b05_env05_discovery_tmpdir_joins_request_temporary_root",
        "test_b05_env06_discovery_target_joins_own_registered_root",
        "test_b05_env08_later_phase_target_joins_data_and_registered_root",
        "test_b05_entry01_initial_request_rejection_precedes_any_state_write",
        "test_b05_entry02_refresh_refusal_emits_no_start_receipt",
        "test_b05_entry03_initial_environment_refusal_precedes_writes_on_both_paths",
        "test_b05_prior01_anchored_v2_genesis_joins_complete_inventory",
        "test_b05_prior02_anchored_interval_and_capture_order_edges",
        "test_b05_prior03_discovery_inventory_is_a_fixed_observation",
        "test_b05_prior04_multiphase_records_reach_isolated_overlap_guard",
        "test_b05_source01_synthetic_source_snapshot_joins_manifest_and_git_pin",
        "test_b05_gate01_prior_gate_pin_has_matching_and_isolated_drift_cases",
        "test_b05_hist01_resource_history_is_cumulative_at_both_caps",
        "test_b05_hist02_custody_resources_accumulates_scans_and_enforces_caps",
        "test_b05_d_fs01_synthetic_dependency_component_controls",
        "test_b05_d_preimage01_missing_component_preimage_precedes_refusal",
        "test_b05_d_compose01_current_synthetic_base_and_candidate_chains",
        "test_b05_b03_roster01_base_candidate_full_selected_missing_and_drift_matrix",
        "test_b05_b03_runtime02_owner_rechecks_roster_and_gate_pins",
        "test_b05_b03_start01_stable_gate_precedes_start_and_is_rechecked_before_child",
        "test_b05_b03_lifecycle01_post_start_inventory_drift_precedes_child",
        "test_b05_b03_raw01_retains_supervisor_and_log_before_native_parse",
        "test_b05_b03_raw01_runtime_owner_persists_axes_before_normalization",
        "test_b05_d04_migration01_synthetic_sql_bytes_are_manifest_and_runtime_joined",
    )),
}
if set().union(*_SLOT_SELECTOR_GROUPS.values()) != set(_EXPLICIT_SELECTORS):
    raise RuntimeError("finite slot groups do not conserve explicit selectors")
if sum(len(group) for group in _SLOT_SELECTOR_GROUPS.values()) != len(_EXPLICIT_SELECTORS):
    raise RuntimeError("finite slot groups overlap")
_SLOT_SELECTORS = {slot: tuple(name for name in _EXPLICIT_SELECTORS if name in group)
                   for slot, group in _SLOT_SELECTOR_GROUPS.items()}
_SLOT_SELECTORS["SQ4"] = _EXPLICIT_SELECTORS


def _strict_profile_json(raw):
    def unique_pairs(pairs):
        value = {}
        for key, item in pairs:
            if key in value:
                raise RunnerBoundaryError("runner profile has a duplicate JSON key")
            value[key] = item
        return value
    def reject_constant(_value):
        raise RunnerBoundaryError("runner profile has a non-finite JSON constant")
    try:
        return json.loads(raw.decode("utf-8"), object_pairs_hook=unique_pairs,
                          parse_constant=reject_constant)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise RunnerBoundaryError("runner profile is not strict UTF-8 JSON") from exc


def _validate_runner_profile(raw, expected_sha256):
    if type(raw) is not bytes or not 0 < len(raw) <= 262144:
        raise RunnerBoundaryError("runner profile byte cap or type is invalid")
    if type(expected_sha256) is not str or len(expected_sha256) != 64 or \
            any(char not in "0123456789abcdef" for char in expected_sha256) or \
            hashlib.sha256(raw).hexdigest() != expected_sha256:
        raise RunnerBoundaryError("runner profile pin mismatch")
    profile = _strict_profile_json(raw)
    base_keys = {"schema_version", "slot", "source_root", "cwd", "python",
        "source_pins", "data_pins", "runtime_pins", "read_files", "selectors",
        "environment", "hardlink_allowance"}
    slot = profile.get("slot") if type(profile) is dict else None
    root_key = "root_paths" if slot == "SQ0" else "root_receipt"
    if type(profile) is not dict or set(profile) != base_keys | {root_key} or \
            profile["schema_version"] != "pbdep.synthetic-runner-profile.v1":
        raise RunnerBoundaryError("runner profile schema or version drift")
    if slot not in ("SQ0", "SQ1", "SQ2", "SQ3", "SQ4"):
        raise RunnerBoundaryError("runner slot is not one of the five finite slots")
    if sys.flags.isolated != 1 or sys.flags.no_site != 1 or not sys.dont_write_bytecode:
        raise RunnerBoundaryError("runner requires isolated no-site no-bytecode startup")
    source_root = _canonical_absolute(profile["source_root"])
    cwd = _canonical_absolute(profile["cwd"])
    if str(cwd) != os.getcwd():
        raise RunnerBoundaryError("runner cwd differs from its explicit profile")
    if profile["environment"] != _FIXED_ENVIRONMENT or dict(os.environ) != _FIXED_ENVIRONMENT:
        raise RunnerBoundaryError("runner process environment is not the closed fixed profile")
    python = profile["python"]
    if type(python) is not dict or set(python) != {"path", "sha256", "version"} or \
            python["path"] != sys.executable or python["version"] != sys.version.split()[0]:
        raise RunnerBoundaryError("runner interpreter path or version is not exactly bound")

    def pins(name, relative_paths):
        value = profile[name]
        if type(value) is not list or len(value) != len(relative_paths):
            raise RunnerBoundaryError(name + " inventory count drift")
        result = []
        for row, relative in zip(value, relative_paths, strict=True):
            if type(row) is not dict or set(row) != {"path", "sha256"}:
                raise RunnerBoundaryError(name + " row schema drift")
            path = _canonical_absolute(row["path"])
            if path != source_root.joinpath(*PurePosixPath(relative).parts):
                raise RunnerBoundaryError(name + " path order or ownership drift")
            digest = row["sha256"]
            if type(digest) is not str or len(digest) != 64 or any(
                    char not in "0123456789abcdef" for char in digest):
                raise RunnerBoundaryError(name + " hash type drift")
            result.append((str(path), digest))
        return tuple(result)

    source_pins = pins("source_pins", _OWNER_SOURCE_RELATIVE)
    data_pins = pins("data_pins", _DATA_READ_RELATIVE)
    runtime_pins = profile["runtime_pins"]
    if type(runtime_pins) is not list or not runtime_pins:
        raise RunnerBoundaryError("runtime pin list is empty")
    runtime = []
    for row in runtime_pins:
        if type(row) is not dict or set(row) != {"path", "sha256"}:
            raise RunnerBoundaryError("runtime pin row schema drift")
        path = _canonical_absolute(row["path"])
        digest = row["sha256"]
        if type(digest) is not str or len(digest) != 64 or any(
                char not in "0123456789abcdef" for char in digest):
            raise RunnerBoundaryError("runtime pin hash type drift")
        if str(path) != python["path"] and not (
                _under(str(path), sys.base_prefix) or _under(str(path), sys.prefix)):
            raise RunnerBoundaryError("runtime pin is outside the selected Python installation")
        runtime.append((str(path), digest))
    if runtime != sorted(runtime):
        raise RunnerBoundaryError("runtime pins are not in deterministic path order")
    if len({path for path, _ in runtime}) != len(runtime) or \
            dict(runtime).get(python["path"]) != python["sha256"]:
        raise RunnerBoundaryError("runtime pins are duplicate or omit the interpreter")
    if set(path for path, _ in runtime) & set(path for path, _ in (*source_pins, *data_pins)):
        raise RunnerBoundaryError("runtime pins overlap source or data pins")
    expected_reads = {path for path, _ in (*source_pins, *data_pins, *runtime)}
    read_files = profile["read_files"]
    if type(read_files) is not list or read_files != sorted(expected_reads) or \
            len(read_files) != len(set(read_files)):
        raise RunnerBoundaryError("runner read allowlist differs from finite pinned inputs")
    selectors = profile["selectors"]
    if type(selectors) is not list or any(type(item) is not str for item in selectors):
        raise RunnerBoundaryError("selector list shape drift")
    if len(selectors) != len(set(selectors)) or any(
            item not in _EXPLICIT_SELECTORS for item in selectors):
        raise RunnerBoundaryError("selector is duplicate or not explicitly listed")
    order = {name: index for index, name in enumerate(_EXPLICIT_SELECTORS)}
    if selectors != sorted(selectors, key=order.__getitem__):
        raise RunnerBoundaryError("selector order differs from the explicit source inventory")
    if slot == "SQ0":
        if selectors:
            raise RunnerBoundaryError("SQ0 is bootstrap-only; its selector list must be empty")
    elif selectors != list(_SLOT_SELECTORS[slot]):
        raise RunnerBoundaryError("slot selector inventory differs from its frozen source partition")
    allowance = profile["hardlink_allowance"]
    if allowance is not None and (type(allowance) is not dict or
            set(allowance) != {"slot", "control_id", "max_calls"} or
            slot not in ("SQ3", "SQ4") or allowance != {
                "slot": slot, "control_id": "B05-D21-INODE01", "max_calls": 1} or
            "test_b05_d_fs01_synthetic_dependency_component_controls" not in selectors):
        raise RunnerBoundaryError("D21 allowance is not the exact per-slot source control")
    if slot in ("SQ0", "SQ1", "SQ2") and allowance is not None:
        raise RunnerBoundaryError("D21 hard links are closed in this runner slot")
    root_record = profile[root_key]
    if slot == "SQ0":
        _validate_root_paths(root_record)
    elif type(root_record) is not dict:
        raise RunnerBoundaryError("later slot requires the exact SQ0 root receipt")
    entrypoint = source_root.joinpath("scripts", "qualification", "pbdep", "tests", "checks.py")
    if not Path(__file__).is_absolute() or PurePosixPath(__file__) != entrypoint:
        raise RunnerBoundaryError("runner source file is outside its bound source root")
    return profile, tuple(read_files), tuple(source_pins), tuple(data_pins), tuple(runtime)


def _verify_profile_pins(pins):
    for path, expected in pins:
        raw = Path(path).read_bytes()
        if hashlib.sha256(raw).hexdigest() != expected:
            raise RunnerBoundaryError("pinned source or runtime bytes drifted")


def _load_accepted_owners():
    global D, C, R, S, W, QW, P, E, OWNER_IMPORTS_READY, _OWNER_PACKAGE
    if _AUDIT_BOUNDARY is None or _OWNER_PACKAGE is None:
        raise RunnerBoundaryError("guarded package bootstrap is not ready")
    D = importlib.import_module("..discovery", package=_OWNER_PACKAGE)
    C = importlib.import_module("..contract", package=_OWNER_PACKAGE)
    R = importlib.import_module("..runtime", package=_OWNER_PACKAGE)
    S = importlib.import_module("..supervise", package=_OWNER_PACKAGE)
    W = importlib.import_module("..whole_program", package=_OWNER_PACKAGE)
    P = importlib.import_module("..policy", package=_OWNER_PACKAGE)
    E = importlib.import_module("..entry", package=_OWNER_PACKAGE)
    QW = importlib.import_module("..qualify_whole_program", package=_OWNER_PACKAGE)
    QW.bind_temporary_roots(_TEMPORARY_ROOTS)
    OWNER_IMPORTS_READY = True


_RUNNER_TERMINAL_MAX_BYTES = 262144
_ROOT_CUSTODY_PLAN_MAX_BYTES = 16384

_CUSTODY_FRAME_MAX_BYTES = 49152
_CUSTODY_FRAME_MAX_COUNT = 35
_CUSTODY_STREAM_MAX_BYTES = 1720320
_CUSTODY_EVENTS = frozenset(("ROOT_PLAN", "PARENT_INTENT", "PARENT_OBSERVED",
    "MKDIR_INTENT", "MKDIR_RETURNED", "ROOT_IDENTITY_OBSERVED",
    "ROOT_VERIFIED", "ROOT_COMPLETE", "ROOT_FAILURE"))


def _new_custody_stream():
    return {"schema_version": "pbdep.root-custody-stream.v1", "state": "NOT_STARTED",
        "profile_sha256": None, "reserved_frames": 0, "reserved_bytes": 0,
        "local_flush_count": 0, "local_flush_bytes": 0, "head_sha256": None,
        "frames": [], "pending_frame": None, "delivery_gap": None,
        "external_capture": "UNBOUND",
        "limits": {"per_frame_bytes": _CUSTODY_FRAME_MAX_BYTES,
            "frame_count": _CUSTODY_FRAME_MAX_COUNT,
            "cumulative_bytes": _CUSTODY_STREAM_MAX_BYTES}}


def _emit_custody_frame(stream, event, index, observation):
    """One write+flush, no retry; return is required before the next mkdir."""
    if (stream["state"] != "READY" or stream["pending_frame"] is not None
            or stream["delivery_gap"] is not None):
        raise RunnerBoundaryError("custody stream is not ready; no next effect")
    if event not in _CUSTODY_EVENTS or (index is not None and (
            type(index) is not int or not 0 <= index < 7)):
        raise RunnerBoundaryError("custody event is outside the finite source protocol")
    body = {"schema_version": "pbdep.root-custody-frame.v1",
        "sequence": stream["reserved_frames"] + 1,
        "profile_sha256": stream["profile_sha256"],
        "previous_frame_sha256": stream["head_sha256"],
        "event": event, "index": index, "observation": observation,
        "delivery": "NOT_ACKNOWLEDGED_BY_THIS_FRAME"}
    body_raw = _runner_json(body).encode("ascii")
    text = _runner_json({"record_kind": "CUSTODY_FRAME", "body": body,
        "body_sha256": hashlib.sha256(body_raw).hexdigest()}) + "\n"
    raw = text.encode("ascii")
    meta = {"sequence": body["sequence"], "event": event, "index": index,
        "previous_frame_sha256": body["previous_frame_sha256"],
        "body_sha256": hashlib.sha256(body_raw).hexdigest(),
        "sha256": hashlib.sha256(raw).hexdigest(), "bytes": len(raw)}
    if (len(raw) > _CUSTODY_FRAME_MAX_BYTES
            or stream["reserved_frames"] >= _CUSTODY_FRAME_MAX_COUNT
            or stream["reserved_bytes"] + len(raw) > _CUSTODY_STREAM_MAX_BYTES):
        stream["state"] = "BOUND_REFUSAL"
        stream["delivery_gap"] = {"kind": "SOURCE_BOUND_REFUSAL_NO_WRITE",
                                  "prepared_frame": meta}
        raise RunnerBoundaryError("custody source bound refused; no next effect")
    # These counters charge prepared bytes, not observed delivery.
    phase = "RESERVATION_UNPROVEN"
    observed_count = None
    try:
        stream["pending_frame"] = meta
        stream["reserved_frames"] += 1
        stream["reserved_bytes"] += len(raw)
        phase = "INHERITED_STDOUT_CHECK"
        encoding = getattr(sys.stdout, "encoding", None)
        if type(encoding) is not str or encoding.lower() not in ("utf-8", "ascii", "us-ascii"):
            raise RunnerBoundaryError("custody stdout encoding is not ASCII-compatible")
        target = sys.stdout.buffer
        descriptor = target.fileno()
        if type(descriptor) is not int or descriptor != 1:
            raise RunnerBoundaryError("custody output is not inherited stdout")
        phase = "TEXT_PREFIX_FLUSH_UNPROVEN"
        sys.stdout.flush()
        phase = "WRITE_CALL_UNPROVEN"
        count = target.write(raw)
        observed_count = count if type(count) is int else None
        phase = "WRITE_RETURNED"
        if type(count) is not int or count != len(raw):
            raise RunnerBoundaryError("custody short write")
        phase = "FLUSH_CALL_UNPROVEN"
        target.flush()
        phase = "FLUSH_RETURNED"
        stream["frames"].append(meta)
        stream["local_flush_count"] += 1
        stream["local_flush_bytes"] += len(raw)
        stream["head_sha256"] = meta["sha256"]
        stream["pending_frame"] = None
    except BaseException as exc:
        stream["state"] = "DELIVERY_UNKNOWN"
        stream["delivery_gap"] = {"kind": "WRITE_FLUSH_OR_BOOKKEEPING_UNKNOWN",
            "prepared_frame": meta, "phase": phase,
            "write_return_count": observed_count, "exception_type": type(exc).__name__[:128]}
        raise RunnerBoundaryError("custody delivery UNKNOWN; no next effect") from exc


def _custody_stream_consistent(stream):
    frames = stream["frames"]
    if (type(frames) is not list or any(type(stream[key]) is not int or stream[key] < 0
            for key in ("reserved_frames", "reserved_bytes", "local_flush_count", "local_flush_bytes"))):
        return False
    if (stream["state"] not in ("NOT_STARTED", "READY")
            or stream["pending_frame"] is not None or stream["delivery_gap"] is not None
            or stream["reserved_frames"] != len(frames)
            or stream["local_flush_count"] != len(frames)
            or len(frames) > _CUSTODY_FRAME_MAX_COUNT):
        return False
    previous = None
    total = 0
    for index, frame in enumerate(frames, 1):
        if (type(frame["sequence"]) is not int or frame["sequence"] != index
                or frame["previous_frame_sha256"] != previous
                or type(frame["bytes"]) is not int
                or not 0 < frame["bytes"] <= _CUSTODY_FRAME_MAX_BYTES
                or any(type(frame[key]) is not str or len(frame[key]) != 64
                       or any(char not in "0123456789abcdef" for char in frame[key])
                       for key in ("sha256", "body_sha256"))):
            return False
        previous = frame["sha256"]
        total += frame["bytes"]
    return (stream["head_sha256"] == previous
            and stream["reserved_bytes"] == stream["local_flush_bytes"] == total
            and total <= _CUSTODY_STREAM_MAX_BYTES)


def _custody_creation_complete(stream):
    expected = (["ROOT_PLAN"] + ["PARENT_INTENT", "PARENT_OBSERVED"] * 2
        + ["MKDIR_INTENT", "MKDIR_RETURNED", "ROOT_IDENTITY_OBSERVED", "ROOT_VERIFIED"] * 7
        + ["ROOT_COMPLETE"])
    return (_custody_stream_consistent(stream)
            and [frame["event"] for frame in stream["frames"]] == expected)



def _runner_json(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=True, allow_nan=False)


def _new_runner_record(profile_raw, expected_sha256):
    raw_bound = type(profile_raw) is bytes and 0 < len(profile_raw) <= 262144
    expected_bound = (type(expected_sha256) is str and len(expected_sha256) == 64
                      and all(char in "0123456789abcdef" for char in expected_sha256))
    return {"schema_version": "pbdep.synthetic-runner-terminal.v2",
        "record_kind": "TERMINAL", "classification": "SYNTHETIC_CONSUMER_ONLY",
        "slot": None, "stage": "PROFILE_VALIDATION", "outcome": "STOP",
        "reason": "SETUP_NOT_COMPLETE",
        "profile": {"raw_sha256": hashlib.sha256(profile_raw).hexdigest() if raw_bound else None,
                    "expected_sha256": expected_sha256 if expected_bound else None,
                    "validated": False},
        "selectors_declared": None, "selectors_planned": None,
        "attempted_selectors": [], "returned_selectors": [],
        "unattempted_selectors": None, "selector_results": [],
        "framework_error": False, "exception": None, "runner_exception_after_callback": None,
        "root_custody": {"creation_state": "NOT_ATTEMPTED", "stage": "NOT_STARTED",
                         "paths": [], "parents": [], "stream": _new_custody_stream()},
        "root_receipt": None,
        "evidence_joins": {"pins_verified": False, "root_binding_verified": False,
                           "roots_verified_before_run": False, "owners_loaded": False},
        "delivery": "UNCONFIRMED_AT_SERIALIZATION",
        "native_terminal": {"manager_status": "NOT_OBSERVED_BY_RUNNER",
            "physical_exit": "UNKNOWN", "descendants": "UNKNOWN",
            "eof_capture": "UNKNOWN", "aggregate_resource_status": "UNKNOWN"},
        "product_acceptance": False,
        "limits": {"terminal_record_max_bytes": _RUNNER_TERMINAL_MAX_BYTES,
            "custody_frame_max_bytes": _CUSTODY_FRAME_MAX_BYTES,
            "custody_frame_max_count": _CUSTODY_FRAME_MAX_COUNT,
            "custody_stream_max_bytes": _CUSTODY_STREAM_MAX_BYTES,
            "root_custody_plan_max_bytes": _ROOT_CUSTODY_PLAN_MAX_BYTES,
            "maximum_selected_methods": len(_EXPLICIT_SELECTORS),
            "failfast": True, "retry": False,
            "stdout_custody": "EXTERNAL_COMPLETE_CAPTURE_REQUIRED",
            "guard_and_runtime_closure": "KNOWN_OPEN_FINDINGS_UNCHANGED"}}


class _RunnerResult(unittest.TextTestResult):
    """Track the fixed selectors, not model-derived coverage or native exit."""

    def __init__(self, stream, descriptions, verbosity, *, record):
        super().__init__(stream, descriptions, verbosity)
        self.record = record

    def _row(self, test):
        selector = getattr(test, "_testMethodName", None)
        if type(test) is not B05AcceptedB04Controls:
            self.record["framework_error"] = True
            self.stop()
            return None
        for row in self.record["selector_results"]:
            if row["selector"] == selector:
                return row
        self.record["framework_error"] = True
        self.stop()
        return None

    def _note(self, test, outcome):
        row = self._row(test)
        if row is None:
            return
        if not row["attempted"] or row["returned"]:
            self.record["framework_error"] = True
            self.stop()
            return
        if outcome not in row["events"]:
            row["events"].append(outcome)
        if row["outcome"] in ("RUNNING", "SUCCESS") or outcome != "SUCCESS":
            row["outcome"] = outcome

    def startTest(self, test):
        row = self._row(test)
        attempted = self.record["attempted_selectors"]
        planned = self.record["selectors_planned"]
        if (row is None or row["attempted"] or len(attempted) >= len(planned)
                or row["selector"] != planned[len(attempted)]):
            raise RunnerBoundaryError("runner selector start is not the next planned method")
        row["attempted"] = True
        row["outcome"] = "RUNNING"
        attempted.append(row["selector"])
        super().startTest(test)

    def stopTest(self, test):
        super().stopTest(test)
        row = self._row(test)
        if row is None or not row["attempted"] or row["returned"]:
            raise RunnerBoundaryError("runner selector return has no unique attempt")
        row["returned"] = True
        self.record["returned_selectors"].append(row["selector"])
        if row["outcome"] == "RUNNING":
            row["outcome"] = "UNKNOWN"
            self.stop()

    def addSuccess(self, test):
        self._note(test, "SUCCESS")
        super().addSuccess(test)

    def addFailure(self, test, err):
        self._note(test, "ASSERTION_FAILURE")
        self.stop()
        super().addFailure(test, err)

    def _note_abnormal_exception(self, err, stage):
        actual = err[1]
        if isinstance(actual, BaseException) and not isinstance(actual, Exception):
            if self.record["exception"] is None:
                self.record["exception"] = {"type": type(actual).__name__[:128], "stage": stage}
            self.record["reason"] = "NON_EXCEPTION_BASE_EXCEPTION"
            self.stop()

    def addError(self, test, err):
        self._note_abnormal_exception(err, "UNITTEST_ADD_ERROR")
        self._note(test, "ERROR")
        self.stop()
        super().addError(test, err)

    def addSubTest(self, test, subtest, err):
        if err is not None:
            self._note_abnormal_exception(err, "UNITTEST_ADD_SUBTEST")
            self._note(test, "SUBTEST_FAILURE")
            self.stop()
        super().addSubTest(test, subtest, err)

    def addSkip(self, test, reason):
        self._note(test, "SKIPPED")
        self.stop()
        super().addSkip(test, reason)

    def addExpectedFailure(self, test, err):
        self._note(test, "EXPECTED_FAILURE")
        self.stop()
        super().addExpectedFailure(test, err)

    def addUnexpectedSuccess(self, test):
        self._note(test, "UNEXPECTED_SUCCESS")
        self.stop()
        super().addUnexpectedSuccess(test)


def _finish_runner_record(record, completed, successful):
    planned = record["selectors_planned"]
    attempted = record["attempted_selectors"]
    returned = record["returned_selectors"]
    if planned is not None:
        record["unattempted_selectors"] = [name for name in planned if name not in attempted]
    coverage = {"plan_known": planned is not None,
        "planned_count": len(planned) if planned is not None else None,
        "attempted_count": len(attempted), "returned_count": len(returned),
        "unattempted_count": len(record["unattempted_selectors"]) if planned is not None else None,
        "scope": "BOOTSTRAP_ONLY" if record["slot"] == "SQ0" else "SELECTED_METHODS",
        "attempt_definition": "unittest.startTest entered before setUp",
        "return_definition": "unittest.stopTest completed; not a success or physical-exit claim"}
    complete = (planned is not None and attempted == planned and returned == planned
                and all(row["outcome"] == "SUCCESS" for row in record["selector_results"])
                and not record["framework_error"])
    coverage["all_selected_returned_successfully"] = complete if record["slot"] != "SQ0" else None
    record["coverage"] = coverage
    stream = record["root_custody"]["stream"]
    stream_ok = _custody_stream_consistent(stream)
    record["evidence_joins"]["custody_stream"] = {
        "scope": "THIS_RUN_ROOT_CREATION_ONLY",
        "profile_sha256": stream["profile_sha256"],
        "metadata_consistent": stream_ok,
        "local_flush_count": stream["local_flush_count"],
        "local_flush_bytes": stream["local_flush_bytes"],
        "reserved_frames": stream["reserved_frames"],
        "reserved_bytes": stream["reserved_bytes"],
        "head_sha256": stream["head_sha256"],
        "frame_index_sha256": hashlib.sha256(_runner_json(stream["frames"]).encode("ascii")).hexdigest(),
        "delivery_gap": stream["delivery_gap"], "external_capture": "UNBOUND"}
    if record["slot"] == "SQ0" and completed and successful:
        stream_ok = stream_ok and _custody_creation_complete(stream)
    partial_root = record["root_custody"]["creation_state"] not in (
        "NOT_ATTEMPTED", "COMPLETE_AT_CREATION")
    lifecycle_known = (returned == attempted and all(
        row["returned"] and row["outcome"] not in ("RUNNING", "UNKNOWN")
        for row in record["selector_results"] if row["attempted"]))
    if (record["exception"] is not None or record["framework_error"]
            or not lifecycle_known or not stream_ok):
        record["outcome"] = "STOP"
        if record["exception"] is None:
            record["reason"] = "FRAMEWORK_OR_SELECTOR_STATE_UNKNOWN"
    elif completed and successful and complete and not partial_root:
        record["outcome"] = "SUCCESS"
        record["reason"] = "BOOTSTRAP_COMPLETE" if record["slot"] == "SQ0" else "CONTROLS_COMPLETE"
    elif completed and not successful and not partial_root:
        record["outcome"] = "FAIL"
        record["reason"] = "SELECTED_CONTROL_FAILURE"
    else:
        record["outcome"] = "STOP"
        if completed:
            record["reason"] = "INCOMPLETE_OR_UNKNOWN_COVERAGE"
    return record


def _write_runner_terminal(record):
    """One bounded stdout record; a short write/flush failure propagates."""
    text = _runner_json(record) + "\n"
    if len(text.encode("ascii")) > _RUNNER_TERMINAL_MAX_BYTES:
        raise RunnerBoundaryError("terminal record over cap; delivery UNKNOWN")
    try:
        count = sys.stdout.write(text)
        if type(count) is not int or count != len(text):
            raise RunnerBoundaryError("terminal stdout short write; delivery UNKNOWN")
        sys.stdout.flush()
    except BaseException as exc:
        raise RunnerBoundaryError("terminal stdout delivery UNKNOWN") from exc
    # The serialized record never acknowledges its own delivery or physical exit.


def run_profile(profile_raw, expected_sha256):
    """Future explicit SQ0–SQ4 entry; this source pass does not call it."""
    global _OWNER_PACKAGE
    record = _new_runner_record(profile_raw, expected_sha256)
    completed = successful = False
    try:
        profile, read_files, source_pins, data_pins, runtime_pins = _validate_runner_profile(
            profile_raw, expected_sha256)
        record["profile"]["validated"] = True
        record["slot"] = profile["slot"]
        record["selectors_declared"] = list(profile["selectors"])
        planned = [] if profile["slot"] == "SQ0" else list(_SLOT_SELECTORS[profile["slot"]])
        record["selectors_planned"] = planned
        record["selector_results"] = [{"selector": name, "attempted": False,
            "returned": False, "outcome": "UNATTEMPTED", "events": []} for name in planned]
        for name, pins in (("source", source_pins), ("data", data_pins), ("runtime", runtime_pins)):
            record["evidence_joins"][name + "_pins"] = {
                "count": len(pins), "sha256": hashlib.sha256(
                    _runner_json(pins).encode("ascii")).hexdigest()}
        record["stage"] = "ROOT_SETUP"
        if profile["slot"] == "SQ0":
            root_receipt = _create_runner_roots(profile["root_paths"],
                                               custody=record["root_custody"],
                                               profile_sha256=record["profile"]["raw_sha256"])
        else:
            root_receipt = profile["root_receipt"]
            if len(_runner_json(root_receipt).encode("ascii")) > _ROOT_CUSTODY_PLAN_MAX_BYTES:
                raise RunnerBoundaryError("prior root receipt exceeds terminal retention reserve")
        record["root_receipt"] = root_receipt
        record["stage"] = "ROOT_BINDING"
        _bind_fixture_roots(root_receipt)
        record["evidence_joins"]["root_binding_verified"] = True
        record["stage"] = "ROOT_VERIFICATION"
        for root in (_BOUND_ROOTS["container"], _BOUND_ROOTS["fixture_root"],
                     _BOUND_ROOTS["ordinary_root"], *_BOUND_ROOTS["temporary_roots"]):
            _verify_bound_root(root)
        record["evidence_joins"]["roots_verified_before_run"] = True
        record["stage"] = "GUARD_INSTALL"
        _install_audit_guard(profile, read_files)
        record["stage"] = "GUARD_CANARIES"
        _guard_canaries(profile, read_files)
        record["stage"] = "PIN_VERIFICATION"
        _verify_profile_pins((*source_pins, *data_pins, *runtime_pins))
        record["evidence_joins"]["pins_verified"] = True
        if profile["slot"] == "SQ0":
            completed = successful = True
            record["stage"] = "BOOTSTRAP_COMPLETE"
        else:
            record["stage"] = "OWNER_IMPORT"
            source_root = str(_canonical_absolute(profile["source_root"]))
            if source_root not in sys.path:
                sys.path.insert(0, source_root)
            _OWNER_PACKAGE = "scripts.qualification.pbdep.tests"
            _load_accepted_owners()
            record["evidence_joins"]["owners_loaded"] = True
            record["stage"] = "SUITE_CONSTRUCTION"
            suite = unittest.TestSuite()
            for selector in planned:
                suite.addTest(B05AcceptedB04Controls(selector))

            def make_result(stream, descriptions, verbosity, *, durations=None):
                if durations is not None:
                    raise RunnerBoundaryError("runner durations option is not enabled")
                return _RunnerResult(stream, descriptions, verbosity, record=record)

            record["stage"] = "TEST_RUN"
            result = unittest.TextTestRunner(stream=sys.stdout, verbosity=2,
                failfast=True, resultclass=make_result).run(suite)
            completed = True
            successful = result.wasSuccessful()
            record["stage"] = "TEST_RETURN"
    except BaseException as exc:
        observed = {"type": type(exc).__name__[:128], "stage": record["stage"]}
        if record["exception"] is None:
            record["exception"] = observed
            record["reason"] = "RUNNER_SETUP_OR_EXECUTION_ERROR"
        else:
            record["runner_exception_after_callback"] = observed
            record["reason"] = "ABNORMAL_CALLBACK_AND_RUNNER_ERROR"
    _finish_runner_record(record, completed, successful)
    _write_runner_terminal(record)  # Outside recovery: failed delivery cannot return success.
    return 0 if record["outcome"] == "SUCCESS" else 1


def _main(argv):
    try:
        if len(argv) != 4 or argv[0] != "--profile-sha256" or argv[2] != "--profile-b64":
            raise RunnerBoundaryError("runner requires only --profile-sha256 and --profile-b64")
        if type(argv[3]) is not str or len(argv[3]) > 349528:
            raise RunnerBoundaryError("encoded runner profile exceeds its finite input bound")
        try:
            profile_raw = base64.b64decode(argv[3].encode("ascii"), validate=True)
        except (UnicodeEncodeError, ValueError) as exc:
            raise RunnerBoundaryError("encoded runner profile is not strict base64") from exc
    except BaseException as exc:
        record = _new_runner_record(None, None)
        record["stage"] = "CLI_DECODE"
        record["reason"] = "RUNNER_CLI_SETUP_ERROR"
        record["exception"] = {"type": type(exc).__name__[:128], "stage": "CLI_DECODE"}
        _finish_runner_record(record, False, False)
        _write_runner_terminal(record)
        return 1
    return run_profile(profile_raw, argv[1])


if __name__ == "__main__":
    raise SystemExit(_main(sys.argv[1:]))
