"""Pure Phase B step-zero evidence checks. No I/O, dispatch or authorization.

Caller retains bounded raw inputs before parsing and supplies independently
observed hashes/identities. Returned facts do not attest to the caller's custody.
Original-contract oracle constants are usable only with the required version
phase-b.original-a2-evidence.v1. Repaired-chain validators live in whole_program.
No historical head, authority or source digest is an active default.
"""

from __future__ import annotations

import base64
import hashlib
import json
import posixpath
import re
from decimal import Decimal
from enum import Enum


class Code(str, Enum):
    TYPE = "INPUT_TYPE"
    LIMIT = "INPUT_LIMIT"
    TEXT = "TEXT_GRAMMAR"
    JSON = "JSON_SHAPE"
    NUMBER = "JSON_NUMBER"
    DUPLICATE = "DUPLICATE_KEY"
    ROSTER = "ROSTER_MISMATCH"
    PIN = "INPUT_PIN"
    COMPILE = "COMPILER_PROVENANCE"
    DEPFILE = "DEPFILE_BINDING"
    SOURCE = "SOURCE_MANIFEST"
    BINARY = "BINARY_IDENTITY"
    ADMISSION = "ADMISSION_JOIN"
    PRE_WITNESS = "PRE_WITNESS_JOIN"
    CUSTODY = "CAPTURE_OR_EXIT"
    FRAME = "WITNESS_FRAMING"
    WITNESS = "WITNESS_SHAPE"
    CANONICAL = "CANONICAL_V1"
    ROW_EVENT = "ROW_EVENT_JOIN"
    OUTCOME = "CASE_OUTCOME"
    SUMMARY = "SUMMARY"
    EXIT = "EXIT_STATUS"


class Rejection(ValueError):
    """Closed reason only; never include raw payloads in an exception."""

    def __init__(self, code: Code):
        self.code = code
        super().__init__(code.value)


MAX_JSON = 2 * 1024 * 1024
MAX_COMPILE = 16 * 1024 * 1024
MAX_DEPFILE = 512 * 1024
MAX_LISTING = 256 * 1024
MAX_WITNESS = 512 * 1024
MAX_LINE = 64 * 1024
MAX_FILES = 2048
MAX_BINARY = 1024 * 1024 * 1024
U64 = (1 << 64) - 1
HEX64 = re.compile(r"[0-9a-f]{64}")
IDENTIFIER = re.compile(r"[a-zA-Z_][a-zA-Z_0-9]*(?:::[a-zA-Z_][a-zA-Z_0-9]*)+")
UUID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
PREFIX = "artifact::artifact_identity_contract::"
PILOT_PREFIXES = ("pilot_admission::", "pilot_authority::", "pilot_store::")
ROSTER_PINS = (
    "80176813d5049c5db9ec2750ea6ec343603cfbc9d8ded6a3843fc2b74b28dc27",
    "464efb7b12934af12c91a537130a0303ed1a1bda73595a0ff1e2c0765e0caf9f",
    "be9e58b9677ab50ba2c954c3435f9d0f0d45d37674a01be5e929d6fe1d169641",
)
ORIGINAL_EVIDENCE_CONTRACT = "phase-b.original-a2-evidence.v1"
REQUIRED_SOURCE_NAMES = frozenset((
    "Cargo.lock", "crates/beacond/src/artifact.rs",
    "crates/beacond/src/artifact_identity_contract.rs",
    "crates/beacond/src/pilot_authority.rs", "crates/beacond/src/pilot_admission.rs",
    "crates/beacond/src/pilot_store.rs"))

T = 1788000000000
CONTENT = b"# Task Tracker outcome\n\nThe exporter landed.\n"
CONTENT_SHA = "3acdb041be74dbe9a5aefba38832121cd62dda3a88003ccd283b4801090043ac"
# Literal independent oracle mapping, not extracted through a product helper.
CASES = (
    ("different-accountability", "equal_stamp_different_accountability_records_twice", (T, T), 2, None, None),
    ("equal", "equal_stamp_identical_inputs_require_two_records", (T, T), 1, "PHASE_A_RED_EQUAL_STAMP_DISTINCTNESS", 372),
    ("increasing", "increasing_stamp_identical_inputs_record_twice", (T, T + 1), 2, None, None),
    ("nonadjacent", "nonadjacent_repeated_stamp_requires_three_records", (T, T + 1, T), 2, "PHASE_A_RED_NONADJACENT_DISTINCTNESS", 428),
    ("reopen", "verified_read_reopen_preserves_observed_cardinality", (T, T + 1, T), 2, None, None),
)
BACKTRACE_NOTE = "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace"


def _need(condition: bool, code: Code) -> None:
    if not condition:
        raise Rejection(code)


def _hash(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def _bytes(raw: bytes, cap: int) -> bytes:
    _need(type(raw) is bytes, Code.TYPE)
    _need(0 < len(raw) <= cap, Code.LIMIT)
    return raw


def _text(raw: bytes, cap: int) -> str:
    _bytes(raw, cap)
    try:
        text = raw.decode("utf-8", "strict")
    except UnicodeError:
        raise Rejection(Code.TEXT) from None
    _need("\r" not in text and "\x00" not in text, Code.TEXT)
    return text


def _utf8(text: str) -> bytes:
    try:
        return text.encode("utf-8", "strict")
    except UnicodeError:
        raise Rejection(Code.TEXT) from None


def _keys(value: object, keys: set[str], code: Code = Code.JSON) -> dict:
    _need(type(value) is dict and set(value) == keys, code)
    return value


def _pairs(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        _need(key not in result, Code.DUPLICATE)
        result[key] = value
    return result


def _no_float(_: str) -> None:
    raise Rejection(Code.NUMBER)


def _json(raw: bytes, cap: int = MAX_JSON) -> object:
    text = _text(raw, cap)
    try:
        return json.loads(text, object_pairs_hook=_pairs,
                          parse_float=_no_float, parse_constant=_no_float)
    except (json.JSONDecodeError, RecursionError, ValueError) as exc:
        if isinstance(exc, Rejection):
            raise
        raise Rejection(Code.JSON) from None


def _uint(value: object, *, positive: bool = False) -> int:
    _need(type(value) is int and (1 if positive else 0) <= value <= U64, Code.NUMBER)
    return value


def _decimal(value: object, *, positive: bool = False) -> int:
    _need(type(value) is str and re.fullmatch(r"0|[1-9][0-9]{0,19}", value) is not None, Code.NUMBER)
    number = int(value)
    _need((1 if positive else 0) <= number <= U64, Code.NUMBER)
    return number


def _sha(value: object) -> str:
    _need(type(value) is str and HEX64.fullmatch(value) is not None, Code.PIN)
    return value


def _absolute(value: object) -> str:
    _need(type(value) is str and 1 < len(value) <= 1024, Code.TEXT)
    _need(re.fullmatch(r"/[A-Za-z0-9_./+-]+", value) is not None, Code.TEXT)
    _need(posixpath.normpath(value) == value and not value.startswith("//"), Code.TEXT)
    return value


def _relative(value: object) -> str:
    _need(type(value) is str and 0 < len(value) <= 512, Code.TEXT)
    _need(re.fullmatch(r"[A-Za-z0-9_./+-]+", value) is not None, Code.TEXT)
    _need(not value.startswith("/") and posixpath.normpath(value) == value, Code.TEXT)
    _need(value != ".." and not value.startswith("../"), Code.TEXT)
    return value


def _source_map(value: object) -> dict[str, str]:
    _need(type(value) is dict and 0 < len(value) <= MAX_FILES, Code.SOURCE)
    for name, digest in value.items():
        _relative(name)
        _sha(digest)
    return value


def _roster(raw: bytes, count: int) -> tuple[str, ...]:
    text = _text(raw, MAX_LISTING)
    _need(text.endswith("\n"), Code.ROSTER)
    names = tuple(text[:-1].split("\n"))
    _need(len(names) == count and len(set(names)) == count, Code.ROSTER)
    _need(all(IDENTIFIER.fullmatch(n) is not None for n in names), Code.ROSTER)
    _need(tuple(sorted(names)) == names, Code.ROSTER)
    return names


def _listing(raw: bytes) -> tuple[str, ...]:
    text = _text(raw, MAX_LISTING)
    _need(text.endswith("\n"), Code.ROSTER)
    lines = text[:-1].split("\n")
    _need(all(line.endswith(": test") for line in lines), Code.ROSTER)
    names = tuple(line[:-6] for line in lines)
    _need(len(names) == 518 and len(set(names)) == 518, Code.ROSTER)
    _need(all(IDENTIFIER.fullmatch(n) is not None for n in names), Code.ROSTER)
    _need(tuple(sorted(names)) == names, Code.ROSTER)
    return names


def _names_hash(names: tuple[str, ...]) -> str:
    return _hash(("\n".join(names) + "\n").encode("ascii"))


def _listing_pins(raw: bytes) -> tuple[str, ...]:
    names = _listing(raw)
    pilot = tuple(n for n in names if n.startswith(PILOT_PREFIXES))
    overlay = tuple(n for n in names if n.startswith(PREFIX))
    _need(len(pilot) == 68 and len(overlay) == 5, Code.ROSTER)
    _need(tuple(_names_hash(v) for v in (names, pilot, overlay)) == ROSTER_PINS, Code.ROSTER)
    return names


def validate_listing(raw: bytes, *, full_roster: bytes, pilot_roster: bytes,
                     overlay_roster: bytes,
                     roster_sha256: tuple[str, str, str], evidence_contract: str) -> dict:
    """Version-scoped literal518/68/5 equality; no behavior or current roster proof."""
    _need(type(evidence_contract) is str and evidence_contract == ORIGINAL_EVIDENCE_CONTRACT, Code.PIN)
    _need(type(roster_sha256) is tuple and roster_sha256 == ROSTER_PINS, Code.PIN)
    literals = (full_roster, pilot_roster, overlay_roster)
    for raw_roster, expected in zip(literals, ROSTER_PINS, strict=True):
        _bytes(raw_roster, MAX_LISTING)
        _need(_hash(raw_roster) == expected, Code.PIN)
    full, pilot, overlay = tuple(_roster(raw_roster, count) for raw_roster, count
                                 in zip(literals, (518, 68, 5), strict=True))
    names = _listing_pins(raw)
    _need(names == full, Code.ROSTER)
    _need(tuple(n for n in names if n.startswith(PILOT_PREFIXES)) == pilot, Code.ROSTER)
    _need(tuple(n for n in names if n.startswith(PREFIX)) == overlay, Code.ROSTER)
    return {"scope": "listing-only", "listing_sha256": _hash(raw),
            "full_count": 518, "pilot_count": 68, "overlay_count": 5,
            "roster_sha256": list(ROSTER_PINS)}


def _binary(value: object) -> dict[str, str]:
    item = _keys(value, {"path", "sha256", "dev", "ino", "size", "mtime_ns"}, Code.BINARY)
    _absolute(item["path"])
    _sha(item["sha256"])
    for key in ("dev", "ino", "size", "mtime_ns"):
        _decimal(item[key], positive=True)
    _need(int(item["size"]) <= MAX_BINARY, Code.LIMIT)
    return item


def _capture(value: object, raw: bytes, expected_exit: int) -> dict:
    item = _keys(value, {"exit_code", "whole_descendant_set_exit", "pipe_eof",
                        "subreaper_verified", "discarded_log_bytes", "metadata_overflow",
                        "errors", "event_error_count", "stop", "log_sha256", "log_bytes",
                        "start_monotonic_ns", "end_monotonic_ns"}, Code.CUSTODY)
    _need(type(item["exit_code"]) is int and item["exit_code"] == expected_exit, Code.EXIT)
    _need(item["whole_descendant_set_exit"] == "PROVEN_ECHILD"
          and item["pipe_eof"] is True and item["subreaper_verified"] is True
          and item["metadata_overflow"] is False and item["stop"] is None, Code.CUSTODY)
    _need(item["errors"] == [] and type(item["errors"]) is list, Code.CUSTODY)
    _need(_uint(item["discarded_log_bytes"]) == 0
          and _uint(item["event_error_count"]) == 0, Code.CUSTODY)
    _need(_uint(item["log_bytes"]) == len(raw)
          and _sha(item["log_sha256"]) == _hash(raw), Code.CUSTODY)
    _need(_decimal(item["start_monotonic_ns"], positive=True)
          <= _decimal(item["end_monotonic_ns"], positive=True), Code.CUSTODY)
    return item


def _compiler(raw: bytes, source_root: str, target_root: str) -> str:
    text = _text(raw, MAX_COMPILE)
    _need(text.endswith("\n"), Code.COMPILE)
    lines = text[:-1].split("\n")
    _need(0 < len(lines) <= 20000, Code.COMPILE)
    executables = []
    finished = 0
    status_finished = 0
    for line in lines:
        _need(len(line.encode("utf-8")) <= MAX_LINE, Code.LIMIT)
        if line.startswith("{"):
            obj = _json(line.encode(), MAX_LINE)
            _need(type(obj) is dict and type(obj.get("reason")) is str, Code.COMPILE)
            reason = obj["reason"]
            _need(reason in ("compiler-artifact", "build-script-executed", "build-finished"), Code.COMPILE)
            _need(finished == 0, Code.COMPILE)
            if reason == "build-finished":
                _need(set(obj) == {"reason", "success"} and obj["success"] is True, Code.COMPILE)
                finished += 1
            elif reason == "compiler-artifact" and obj.get("executable") is not None:
                target = obj.get("target")
                profile = obj.get("profile")
                _need(type(target) is dict and type(profile) is dict, Code.COMPILE)
                _need(obj.get("fresh") is False and target.get("name") == "beacond"
                      and target.get("kind") == ["lib"] and target.get("crate_types") == ["lib"]
                      and target.get("edition") == "2024" and target.get("test") is True
                      and profile.get("test") is True, Code.COMPILE)
                _need(obj.get("manifest_path") == source_root + "/crates/beacond/Cargo.toml"
                      and target.get("src_path") == source_root + "/crates/beacond/src/lib.rs",
                      Code.COMPILE)
                _need(obj.get("package_id") == "path+file://" + source_root + "/crates/beacond#0.1.0", Code.COMPILE)
                exe = _absolute(obj["executable"])
                _need(posixpath.dirname(exe) == target_root + "/debug/deps"
                      and re.fullmatch(r"beacond-[0-9a-f]{16}", posixpath.basename(exe)) is not None,
                      Code.COMPILE)
                _need(obj.get("filenames") == [exe], Code.COMPILE)
                executables.append(exe)
        elif re.fullmatch(r"   Compiling [A-Za-z0-9_-]+ v[0-9A-Za-z.+-]+(?: \(/[A-Za-z0-9_./+-]+\))?", line):
            _need(finished == 0, Code.COMPILE)
        elif re.fullmatch(r"    Finished `test` profile \[unoptimized \+ debuginfo\] target\(s\) in (?:[0-9]+m )?[0-9]+(?:\.[0-9]+)?s", line):
            _need(finished == 1, Code.COMPILE)
            status_finished += 1
        else:
            raise Rejection(Code.COMPILE)
    _need(len(executables) == 1 and finished == 1 and status_finished == 1, Code.COMPILE)
    return executables[0]


def _depfile(raw: bytes, *, source_root: str, executable: str,
             tokens: object, source_names: set[str]) -> None:
    _need(type(tokens) is list and 0 < len(tokens) <= MAX_FILES, Code.DEPFILE)
    normalized = []
    for token in tokens:
        _need(type(token) is str and 0 < len(token) <= 1024
              and re.fullmatch(r"[A-Za-z0-9_./+-]+", token) is not None
              and not token.startswith("/"), Code.DEPFILE)
        # Preserve compiler spelling. Reject an escape at the moment it occurs,
        # including escape/re-entry; general _relative() stays unchanged.
        stack = []
        for part in token.split("/"):
            _need(part not in ("", "."), Code.DEPFILE)
            if part == "..":
                _need(bool(stack), Code.DEPFILE)
                stack.pop()
            else:
                stack.append(part)
        name = "/".join(stack)
        _relative(name)
        normalized.append(name)
    _need(len(set(tokens)) == len(tokens) and len(set(normalized)) == len(normalized), Code.DEPFILE)
    _need(set(normalized) <= source_names, Code.DEPFILE)
    required = {"crates/beacond/src/" + name for name in (
        "lib.rs", "artifact.rs", "artifact_identity_contract.rs", "pilot_admission.rs",
        "pilot_authority.rs", "pilot_store.rs")}
    _need(required <= set(normalized), Code.DEPFILE)
    # Supported measured Rust 1.97.1 grammar only: two rules, then exact phony
    # rules, then these two environment dependencies. No escape/continuation guess.
    dependencies = " ".join(tokens)
    expected = (executable + ".d: " + dependencies + "\n\n"
                + executable + ": " + dependencies + "\n\n"
                + "".join(token + ":\n" for token in tokens)
                + "\n# env-dep:CARGO_MANIFEST_DIR=" + source_root + "/crates/beacond\n"
                + "# env-dep:CARGO_PKG_VERSION=0.1.0\n")
    _need(_text(raw, MAX_DEPFILE) == expected, Code.DEPFILE)



def _original_reference(reference: object) -> dict:
    r = _keys(reference, {"schema_version", "evidence_contract", "binding_sha256",
        "subject_head", "authority_sha256", "bound_packet_sha256",
        "acceptance_sha256", "review_sha256", "source_manifest_sha256",
        "required_source_pins"}, Code.PIN)
    _need(r["schema_version"] == "pbdep.original-reference.v2"
          and r["evidence_contract"] == ORIGINAL_EVIDENCE_CONTRACT, Code.PIN)
    _need(type(r["subject_head"]) is str
          and re.fullmatch(r"[0-9a-f]{40}", r["subject_head"]) is not None, Code.PIN)
    for field in ("binding_sha256", "authority_sha256", "bound_packet_sha256",
                  "acceptance_sha256", "review_sha256", "source_manifest_sha256"):
        _sha(r[field])
    _need(set(_source_map(r["required_source_pins"])) == REQUIRED_SOURCE_NAMES, Code.SOURCE)
    return r


def validate_provenance(compiler_log: bytes, depfile: bytes, listing: bytes,
                        source_manifest: bytes, admission_receipt: bytes,
                        pre_witness_receipt: bytes, *, binding: bytes,
                        binding_sha256: str, observed_source_hashes: dict[str, str],
                        observed_binary: dict[str, str], reference: dict) -> dict:
    """Validate pre-dispatch joins, not the caller's filesystem observations.

    Call validate_witness_completion after dispatch to prove temporal ordering,
    final identity and capture. Neither function grants dispatch authority.
    """
    expected = _original_reference(reference)
    _need(binding_sha256 == expected["binding_sha256"], Code.PIN)
    _need(_hash(_bytes(binding, MAX_JSON)) == _sha(binding_sha256), Code.PIN)
    b = _keys(_json(binding), {"schema_version", "subject_head", "source_root", "target_root",
        "authority_sha256", "bound_packet_sha256", "acceptance_sha256", "review_sha256",
        "source_manifest_sha256", "roster_sha256", "dep_tokens"}, Code.ADMISSION)
    _need(b["schema_version"] == "pbdep.provenance-binding.v2", Code.ADMISSION)
    _need(b["subject_head"] == expected["subject_head"], Code.ADMISSION)
    source_root = _absolute(b["source_root"])
    target_root = _absolute(b["target_root"])
    _need(source_root != target_root, Code.ADMISSION)
    for key in ("authority_sha256", "bound_packet_sha256", "acceptance_sha256",
                "review_sha256", "source_manifest_sha256"):
        _sha(b[key])
    for key in ("authority_sha256", "bound_packet_sha256", "acceptance_sha256",
                "review_sha256", "source_manifest_sha256"):
        _need(b[key] == expected[key], Code.ADMISSION)
    _need(b["roster_sha256"] == list(ROSTER_PINS), Code.PIN)
    _need(_hash(_bytes(source_manifest, MAX_JSON)) == b["source_manifest_sha256"], Code.SOURCE)
    manifest = _keys(_json(source_manifest), {"schema_version", "subject_head", "source_root", "files"}, Code.SOURCE)
    _need(manifest["schema_version"] == "phase-b.source-manifest.v1"
          and manifest["subject_head"] == b["subject_head"]
          and manifest["source_root"] == source_root, Code.SOURCE)
    sources = _source_map(manifest["files"])
    _need(all(sources.get(path) == digest for path, digest in expected["required_source_pins"].items()), Code.SOURCE)
    _need(_source_map(observed_source_hashes) == sources, Code.SOURCE)
    _need({"Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "crates/beacond/Cargo.toml"} <= set(sources), Code.SOURCE)
    exe = _compiler(compiler_log, source_root, target_root)
    _depfile(depfile, source_root=source_root, executable=exe,
             tokens=b["dep_tokens"], source_names=set(sources))
    _listing_pins(listing)
    binary = _binary(observed_binary)
    _need(binary["path"] == exe, Code.BINARY)
    admission = _keys(_json(admission_receipt), {"schema_version", "binding_sha256",
        "authority_sha256", "bound_packet_sha256", "acceptance_sha256", "review_sha256",
        "subject_head", "source_manifest_sha256", "source_hashes", "binary", "depfile_sha256",
        "compiler_log_sha256", "listing_sha256", "roster_sha256", "compile", "inventory",
        "recorded_monotonic_ns"}, Code.ADMISSION)
    _need(admission["schema_version"] == "pbdep.pre-witness-admission.v2"
          and admission["binding_sha256"] == binding_sha256, Code.ADMISSION)
    for key in ("authority_sha256", "bound_packet_sha256", "acceptance_sha256",
                "review_sha256", "subject_head", "source_manifest_sha256", "roster_sha256"):
        _need(admission[key] == b[key], Code.ADMISSION)
    _need(_source_map(admission["source_hashes"]) == sources, Code.SOURCE)
    _need(_binary(admission["binary"]) == binary, Code.BINARY)
    for key, raw in (("depfile_sha256", depfile), ("compiler_log_sha256", compiler_log),
                     ("listing_sha256", listing)):
        _need(_sha(admission[key]) == _hash(raw), Code.PIN)
    compile_fact = _capture(admission["compile"], compiler_log, 0)
    list_fact = _capture(admission["inventory"], listing, 0)
    admitted_ns = _decimal(admission["recorded_monotonic_ns"], positive=True)
    _need(int(compile_fact["end_monotonic_ns"]) <= int(list_fact["start_monotonic_ns"])
          <= int(list_fact["end_monotonic_ns"]) <= admitted_ns, Code.ADMISSION)
    pre = _keys(_json(pre_witness_receipt), {"schema_version", "admission_sha256", "binding_sha256",
        "binary", "source_hashes", "depfile_sha256", "recorded_monotonic_ns"}, Code.PRE_WITNESS)
    _need(pre["schema_version"] == "phase-b.pre-witness.v1"
          and pre["admission_sha256"] == _hash(admission_receipt)
          and pre["binding_sha256"] == binding_sha256, Code.PRE_WITNESS)
    _need(_binary(pre["binary"]) == binary and _source_map(pre["source_hashes"]) == sources,
          Code.PRE_WITNESS)
    _need(_sha(pre["depfile_sha256"]) == _hash(depfile)
          and admitted_ns <= _decimal(pre["recorded_monotonic_ns"], positive=True), Code.PRE_WITNESS)
    return {"scope": "pre-dispatch-provenance-joins-only", "binding_sha256": binding_sha256,
            "admission_sha256": _hash(admission_receipt), "pre_witness_sha256": _hash(pre_witness_receipt),
            "pre_witness_monotonic_ns": pre["recorded_monotonic_ns"],
            "binary": dict(binary), "source_hashes": dict(sources),
            "depfile_sha256": _hash(depfile), "listing_sha256": _hash(listing)}


def validate_witness_completion(raw: bytes, completion_receipt: bytes, *,
                                provenance: dict, observed_binary: dict[str, str],
                                observed_source_hashes: dict[str, str],
                                observed_depfile_sha256: str, reference: dict) -> dict:
    """Post-dispatch custody, time and identity join. Not behavioral acceptance."""
    expected = _original_reference(reference)
    _bytes(raw, MAX_WITNESS)
    p = _keys(provenance, {"scope", "binding_sha256", "admission_sha256", "pre_witness_sha256",
        "pre_witness_monotonic_ns", "binary", "source_hashes", "depfile_sha256", "listing_sha256"}, Code.PRE_WITNESS)
    _need(p["scope"] == "pre-dispatch-provenance-joins-only"
          and p["binding_sha256"] == expected["binding_sha256"], Code.PRE_WITNESS)
    observed = _source_map(observed_source_hashes)
    _need(all(observed.get(name) == digest
              for name, digest in expected["required_source_pins"].items()), Code.SOURCE)
    record = _keys(_json(completion_receipt), {"schema_version", "binding_sha256",
        "admission_sha256", "pre_witness_sha256", "capture"}, Code.CUSTODY)
    _need(record["schema_version"] == "phase-b.witness-completion.v1", Code.CUSTODY)
    for key in ("binding_sha256", "admission_sha256", "pre_witness_sha256"):
        _need(_sha(record[key]) == _sha(p[key]), Code.CUSTODY)
    capture = _capture(record["capture"], raw, 101)
    _need(_decimal(p["pre_witness_monotonic_ns"], positive=True)
          <= int(capture["start_monotonic_ns"]), Code.PRE_WITNESS)
    _need(_binary(observed_binary) == _binary(p["binary"]), Code.BINARY)
    _need(_source_map(observed_source_hashes) == _source_map(p["source_hashes"]), Code.SOURCE)
    _need(_sha(observed_depfile_sha256) == _sha(p["depfile_sha256"]), Code.DEPFILE)
    return {"scope": "post-witness-custody-only", "raw_sha256": _hash(raw),
            "completion_sha256": _hash(completion_receipt), "pre_witness_sha256": p["pre_witness_sha256"]}


def _uuid(value: object) -> bytes:
    _need(type(value) is str and UUID.fullmatch(value) is not None, Code.WITNESS)
    return bytes.fromhex(value.replace("-", ""))


def _hex(value: object, size: int) -> bytes:
    _need(type(value) is str and len(value) == size * 2
          and re.fullmatch(r"[0-9a-f]+", value) is not None, Code.CANONICAL)
    return bytes.fromhex(value)


def _envelope(value: object, canonicalizer: str, digest: str) -> None:
    item = _keys(value, {"algorithm", "canonicalizer", "value"}, Code.WITNESS)
    _need(item == {"algorithm": "sha256", "canonicalizer": canonicalizer, "value": digest}, Code.CANONICAL)


def _canonical(returned: dict, workspace: str, stamp: int) -> bytes:
    record = _keys(returned, {"digest", "canonical_hex", "recorded_at_ms", "creation_event_seq",
                             "content_hex", "resident", "configuration"}, Code.WITNESS)
    _need(_uint(record["recorded_at_ms"]) == stamp, Code.CANONICAL)
    _uint(record["creation_event_seq"], positive=True)
    _need(_hex(record["content_hex"], 45) == CONTENT, Code.CANONICAL)
    # Independent fixed byte table from SOURCE-ORACLES, not Beacon's codec.
    expected = (b"beacon.artifact.v1\0" + _uuid(workspace) + b"\x01"
                + (28).to_bytes(8, "big") + b"text/markdown; charset=utf-8"
                + (6).to_bytes(8, "big") + b"sha256"
                + (13).to_bytes(8, "big") + b"beacon.raw.v1"
                + bytes.fromhex(CONTENT_SHA) + (45).to_bytes(8, "big")
                + _uuid(record["resident"]) + _uuid(record["configuration"])
                + b"\0\0" + stamp.to_bytes(8, "big"))
    actual = _hex(record["canonical_hex"], 189)
    _need(actual == expected, Code.CANONICAL)
    _envelope(record["digest"], "beacon.artifact.v1", _hash(actual))
    return actual


def _witness(value: object, case: tuple, store_root: str) -> dict:
    label, _, stamps, distinct, _, _ = case
    w = _keys(value, {"schema_version", "case", "store", "workspace", "fixture_project",
        "fixture_mission", "timestamps", "digests", "canonical_hex", "reopen_read_result",
        "returned", "row_count", "event_count", "rows", "events"}, Code.WITNESS)
    _need(w["schema_version"] == "artifact.identity.phase_a.v1" and w["case"] == label, Code.WITNESS)
    for key in ("workspace", "fixture_project", "fixture_mission"):
        _uuid(w[key])
    _need(len({w["workspace"], w["fixture_project"], w["fixture_mission"]}) == 3, Code.WITNESS)
    path = _absolute(w["store"])
    _need(posixpath.dirname(path) == store_root
          and re.fullmatch(r"pa-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.sqlite3",
                           posixpath.basename(path)) is not None, Code.WITNESS)
    _need(type(w["timestamps"]) is list and len(w["timestamps"]) == len(stamps), Code.WITNESS)
    _need(tuple(_uint(v) for v in w["timestamps"]) == stamps, Code.WITNESS)
    for key in ("returned", "digests", "canonical_hex"):
        _need(type(w[key]) is list and len(w[key]) == len(stamps), Code.WITNESS)
    records = w["returned"]
    for record, stamp in zip(records, stamps, strict=True):
        _canonical(record, w["workspace"], stamp)
    digests = [r["digest"]["value"] for r in records]
    _need(w["digests"] == digests and w["canonical_hex"] == [r["canonical_hex"] for r in records], Code.WITNESS)
    _need(len(set(digests)) == distinct, Code.OUTCOME)
    identities = [(r["resident"], r["configuration"]) for r in records]
    if label == "different-accountability":
        _need(identities[0][0] != identities[1][0] and identities[0][1] != identities[1][1], Code.OUTCOME)
    else:
        _need(len(set(identities)) == 1, Code.OUTCOME)
    if label == "equal":
        _need(records[0] == records[1], Code.OUTCOME)
    if label in ("nonadjacent", "reopen"):
        _need(records[0] == records[2] and digests[0] != digests[1], Code.OUTCOME)
    _need(_uint(w["row_count"]) == distinct and _uint(w["event_count"]) == distinct, Code.ROW_EVENT)
    _need(type(w["rows"]) is list and type(w["events"]) is list
          and len(w["rows"]) == distinct and len(w["events"]) == distinct, Code.ROW_EVENT)
    unique = list(dict.fromkeys(digests))
    by_digest = {r["digest"]["value"]: r for r in records}
    sequences = []
    for digest, row, event in zip(unique, w["rows"], w["events"], strict=True):
        record = by_digest[digest]
        _keys(row, {"digest", "canonical_hex", "recorded_at_ms", "creation_event_seq"}, Code.ROW_EVENT)
        _uint(row["recorded_at_ms"])
        sequence = _uint(row["creation_event_seq"], positive=True)
        _need(row == {"digest": digest, "canonical_hex": record["canonical_hex"],
                      "recorded_at_ms": record["recorded_at_ms"],
                      "creation_event_seq": record["creation_event_seq"]}, Code.ROW_EVENT)
        sequences.append(sequence)
        _keys(event, {"sequence", "payload_json", "occurred_at_ms"}, Code.ROW_EVENT)
        _need(_uint(event["sequence"], positive=True) == sequence
              and _uint(event["occurred_at_ms"]) == record["recorded_at_ms"], Code.ROW_EVENT)
        _need(type(event["payload_json"]) is str, Code.ROW_EVENT)
        payload_bytes = _utf8(event["payload_json"])
        payload = _json(payload_bytes, MAX_LINE)
        expected_payload = {"artifact": {
            "digest": {"algorithm": "sha256", "canonicalizer": "beacon.artifact.v1", "value": digest},
            "canonicalBytesBase64": base64.b64encode(bytes.fromhex(record["canonical_hex"])).decode("ascii"),
            "contentDigest": {"algorithm": "sha256", "canonicalizer": "beacon.raw.v1", "value": CONTENT_SHA},
            "contentBytes": 45, "actor": "owner", "recordedAtMs": record["recorded_at_ms"]}}
        _keys(payload, {"artifact"}, Code.ROW_EVENT)
        artifact = _keys(payload["artifact"], set(expected_payload["artifact"]), Code.ROW_EVENT)
        _uint(artifact["contentBytes"])
        _uint(artifact["recordedAtMs"])
        _need(payload == expected_payload, Code.ROW_EVENT)
        # JSON object order/spacing is not identity; exact typed content and joins are.
    _need(sequences == sorted(set(sequences)), Code.ROW_EVENT)
    if label == "reopen":
        reopen = _keys(w["reopen_read_result"], {"state", "row_count", "event_count", "verified_digests",
            "records_equal", "contract_expected_independent_records", "observed_distinct_records",
            "observation_is_not_contract_acceptance"}, Code.WITNESS)
        for key, number in (("row_count", 2), ("event_count", 2),
                            ("contract_expected_independent_records", 3), ("observed_distinct_records", 2)):
            _need(_uint(reopen[key]) == number, Code.OUTCOME)
        _need(reopen["state"] == "observed" and reopen["verified_digests"] == digests
              and reopen["records_equal"] is True
              and reopen["observation_is_not_contract_acceptance"] is True, Code.OUTCOME)
    else:
        _need(w["reopen_read_result"] == {"state": "not_requested"}, Code.WITNESS)
    return {"case": label, "store": path, "workspace": w["workspace"],
            "digests": digests, "row_count": distinct, "event_count": distinct}


class _Lines:
    def __init__(self, lines: list[str]):
        self.lines = lines
        self.index = 0

    def take(self) -> str:
        _need(self.index < len(self.lines), Code.FRAME)
        value = self.lines[self.index]
        self.index += 1
        return value

    def exact(self, value: str) -> None:
        _need(self.take() == value, Code.FRAME)


def classify_a2(raw: bytes, *, exit_code: int, framing: str,
                filtered_out: int, store_root: str, evidence_contract: str,
                historical_prefix: tuple[str, ...]) -> dict:
    """Behavior-only check. 'historical-cargo-A1' cannot become a live A2 receipt.

    Direct controls derived from A1 must be labeled by their caller. This function
    cannot distinguish synthetic byte strings from genuinely retained output.
    """
    _need(type(evidence_contract) is str and evidence_contract == ORIGINAL_EVIDENCE_CONTRACT, Code.PIN)
    _need(type(historical_prefix) is tuple, Code.FRAME)
    _need(type(exit_code) is int and exit_code == 101, Code.EXIT)
    _need(type(framing) is str and framing in ("direct", "historical-cargo-A1"), Code.FRAME)
    expected_filtered = 513 if framing == "direct" else 445
    _need(type(filtered_out) is int and filtered_out == expected_filtered, Code.SUMMARY)
    _absolute(store_root)
    text = _text(raw, MAX_WITNESS)
    _need(text.endswith("\n"), Code.FRAME)
    lines = text[:-1].split("\n")
    _need(all(len(line.encode("utf-8")) <= MAX_LINE for line in lines), Code.LIMIT)
    c = _Lines(lines)
    if framing == "historical-cargo-A1":
        _need(len(historical_prefix) == 3
              and all(type(line) is str and 0 < len(line) <= MAX_LINE
                      and "\\n" not in line and "\\r" not in line for line in historical_prefix),
              Code.FRAME)
        for line in historical_prefix:
            c.exact(line)
    else:
        _need(historical_prefix == (), Code.FRAME)
    c.exact("")
    c.exact("running 5 tests")
    checked = []
    for case in CASES:
        label, suffix, _, _, assertion, line_number = case
        full_name = PREFIX + suffix
        header = c.take()
        prefix = "test " + full_name + " ... PHASE_A_FIXTURE "
        _need(header.startswith(prefix), Code.FRAME)
        fixture = _keys(_json(header[len(prefix):].encode(), MAX_LINE), {"case", "retained"}, Code.FRAME)
        _need(fixture["case"] == label and fixture["retained"] is True, Code.FRAME)
        witness = c.take()
        _need(witness.startswith("PHASEA_WITNESS "), Code.FRAME)
        fact = _witness(_json(witness[len("PHASEA_WITNESS "):].encode(), MAX_LINE), case, store_root)
        if assertion is None:
            c.exact("ok")
        else:
            c.exact("")
            panic = c.take()
            pattern = (r"thread '" + re.escape(full_name) + r"' \(([1-9][0-9]*)\) panicked at "
                       + r"crates/beacond/src/artifact_identity_contract\.rs:" + str(line_number) + r":5:")
            match = re.fullmatch(pattern, panic)
            _need(match is not None, Code.OUTCOME)
            _need(int(match.group(1)) <= U64, Code.OUTCOME)
            c.exact("assertion `left != right` failed: " + assertion)
            digest = fact["digests"][0]
            c.exact('  left: "' + digest + '"')
            c.exact(' right: "' + digest + '"')
            if label == "equal":
                c.exact(BACKTRACE_NOTE)
            c.exact("FAILED")
        checked.append(fact)
    _need(len({v["store"] for v in checked}) == 5
          and len({v["workspace"] for v in checked}) == 5, Code.WITNESS)
    for line in ("", "failures:", "", "failures:",
                 "    " + PREFIX + CASES[1][1], "    " + PREFIX + CASES[3][1], ""):
        c.exact(line)
    summary = c.take()
    pattern = (r"test result: FAILED\. 3 passed; 2 failed; 0 ignored; 0 measured; "
               + str(expected_filtered) + r" filtered out; finished in ([0-9]{1,4}\.[0-9]{2})s")
    match = re.fullmatch(pattern, summary)
    _need(match is not None, Code.SUMMARY)
    _need(Decimal(match.group(1)) <= Decimal("180"), Code.SUMMARY)
    c.exact("")
    if framing == "historical-cargo-A1":
        c.exact("error: test failed, to rerun pass `-p beacond --lib`")
    _need(c.index == len(lines), Code.FRAME)
    return {"scope": "behavior-only", "classification": "EXPECTED_ORIGINAL_CONTRACT_RED",
            "framing": framing, "raw_sha256": _hash(raw), "passed": 3, "failed": 2,
            "filtered_out": expected_filtered, "cases": checked,
            "not_product_acceptance": True, "provenance_checked": False}
