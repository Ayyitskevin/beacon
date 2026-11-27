"""Retained independent synthetic public-seam controls, schema-adapted for PBDEP.

Fixtures derive from retained Cargo/libtest/gate text and the public contract.
The original owner read no candidate implementation. This successor author
changes only explicit schema fixture construction; all 354 IDs/expectations stay.
Import-time behavior supplies no I/O, clocks, processes, product imports or authority.
Selected future controls define synthetic filesystem observations, but this
source-only author pass does not execute them.
"""

from copy import deepcopy
from . import whole_program as w, runtime as R
from hashlib import sha256
from functools import wraps
from inspect import signature
import json
import re


EXPECTED_CONTROL_IDS = (
    "operator-valid", "operator-extra", "operator-missing", "operator-version",
    "operator-authority", "operator-disposition", "operator-attestation-bool",
    "operator-packet-join", "operator-instrument-missing", "operator-instrument-extra",
    "operator-phase-missing", "operator-phase-extra", "operator-root-mode",
    "operator-root-bool", "binding-bound", "binding-unbound", "binding-unknown",
    "binding-malformed", "continuation-valid", "continuation-packet",
    "continuation-acceptance", "continuation-instrument", "continuation-completed",
    "continuation-duplicate", "continuation-order", "continuation-unknown",
    "data-base", "data-candidate", "data-format", "data-integration",
    "data-consumers", "data-gate", "data-base-extra", "data-candidate-extra",
    "data-format-extra", "data-integration-extra", "data-consumers-extra",
    "data-gate-extra", "data-unknown", "data-qualification", "data-a2",
    "data-phase-mismatch", "data-bad-head", "data-root-traversal",
    "data-target-nested", "data-manifest-join", "data-case-missing",
    "data-case-duplicate", "data-format-foreign", "data-format-duplicate",
    "data-format-one-root", "data-format-non-rust", "data-consumer-missing",
    "data-consumer-duplicate", "data-integration-wrong", "data-target-extra",
    "argv-base-compile", "argv-base-full", "argv-candidate-full", "argv-format",
    "argv-integration", "argv-consumers", "argv-gate", "argv-unknown",
    "argv-tool-missing", "argv-tool-extra", "argv-tool-relative",
    "argv-executable-relative", "argv-executable-foreign", "argv-data-mismatch",
    "listing-valid", "listing-missing", "listing-duplicate", "listing-extra",
    "listing-foreign-same-count", "listing-trailer", "listing-no-lf",
    "listing-blank", "listing-lookalike", "listing-whitespace",
    "listing-unsorted", "listing-case-map", "listing-selected-missing",
    "listing-raw-type", "compile-base", "compile-candidate", "compile-json-duplicate",
    "compile-json-nonfinite", "compile-terminal-false", "compile-terminal-bool-int",
    "compile-terminal-missing", "compile-double-executable", "compile-message",
    "compile-source-root", "compile-target-root", "compile-profile-bool-int",
    "compile-binary-path", "compile-binary-ns-number", "compile-manifest-head",
    "compile-missing-source", "compile-dep-missing", "compile-dep-reorder",
    "compile-dep-env", "compile-dep-foreign", "compile-truncated",
    "pre-valid", "pre-admission-extra", "pre-admission-version",
    "pre-admission-late", "pre-inventory-before-compile", "pre-pre-before-admission",
    "pre-binary-drift", "pre-source-drift", "pre-depfile-drift",
    "pre-listing-drift", "pre-compiler-drift", "pre-capture-eof",
    "pre-capture-custody", "pre-capture-discard", "pre-capture-exit-bool",
    "pre-capture-time-number", "pre-admission-pin", "pre-data-pin",
    "selected-valid", "selected-missing", "selected-duplicate", "selected-extra",
    "selected-failed", "selected-ignored", "selected-count", "selected-filtered",
    "selected-measured", "selected-summary-missing", "selected-summary-duplicate",
    "selected-cargo-footer", "selected-no-lf", "selected-exit-bool",
    "selected-exit-nonzero", "selected-count-bool", "selected-nonfinite-time",
    "completion-valid", "completion-eof", "completion-custody",
    "completion-subreaper", "completion-discard", "completion-overflow",
    "completion-errors", "completion-event-errors", "completion-stop",
    "completion-late-pre", "completion-time-reversed", "completion-raw-drift",
    "completion-binary-drift", "completion-source-drift", "completion-depfile-drift",
    "completion-exit-bool", "completion-extra", "cargo-integration",
    "cargo-consumers", "cargo-singular-pluralized", "cargo-plural-singularized",
    "cargo-target-missing", "cargo-target-duplicate",
    "cargo-target-foreign", "cargo-header-source", "cargo-header-binary",
    "cargo-name-missing", "cargo-name-duplicate", "cargo-name-foreign",
    "cargo-aggregate-refused", "cargo-failed", "cargo-ignored", "cargo-filtered",
    "cargo-summary-missing", "cargo-exit-bool", "cargo-exit-nonzero",
    "gate-named", "gate-aggregate", "gate-zero-target", "gate-singular-pluralized",
    "gate-zero-singularized", "gate-step-missing",
    "gate-step-duplicate", "gate-step-order", "gate-step-failed", "gate-footer",
    "gate-trailing", "gate-header", "gate-target-missing", "gate-target-duplicate",
    "gate-target-foreign", "gate-partial-names", "gate-aggregate-count",
    "gate-exit-nonzero", "gate-exit-bool", "gate-summary-missing",
    "format-valid", "format-output", "format-whitespace", "format-exit-nonzero",
    "format-exit-bool", "format-exit-float", "format-raw-type",
    "cap-operator", "cap-phase", "cap-result", "cap-ordinary", "cap-case-sensitive",
    "cap-extra-suffix", "cap-invalid-empty", "cap-invalid-absolute", "cap-invalid-traversal",
    "cap-invalid-hidden", "cap-invalid-extension", "cap-invalid-unicode", "cap-invalid-type",
    "receipt-operator-exact", "receipt-phase-exact", "receipt-result-exact", "receipt-ordinary-exact",
    "receipt-operator-over", "receipt-phase-over", "receipt-result-over", "receipt-ordinary-over",
    "receipt-lowercase-over", "receipt-raw-type", "receipt-name-before-cap",
    "resource-directory", "resource-regular", "resource-zero-size",
    "resource-q-link", "resource-a-link", "resource-b-link", "resource-c-link",
    "resource-q-socket", "resource-a-socket", "resource-b-socket", "resource-c-socket",
    "resource-workspace-link", "resource-capture-link", "resource-workspace-socket",
    "resource-capture-socket", "resource-foreign-root", "resource-prefix-root",
    "resource-child-root", "resource-fifo", "resource-character", "resource-block",
    "resource-unknown-mode", "resource-mode-bool", "resource-size-bool",
    "resource-size-negative", "resource-mode-over", "resource-capture-int",
    "resource-kind-unknown", "resource-root-relative", "resource-root-traversal",
    "tool-ordinary", "tool-ordinary-installed", "tool-mkdir", "tool-wc", "tool-dirname", "tool-env",
    "tool-foreign-path", "tool-wrong-class", "tool-unknown-class", "tool-class-bool",
    "tool-extra", "tool-missing", "tool-uid", "tool-mode", "tool-nlink",
    "tool-hash", "tool-inode", "tool-device", "tool-size", "tool-mtime",
    "tool-metadata-int", "tool-metadata-bool", "tool-leading-zero", "tool-relative-path",
    "tool-ordinary-multilink",
    "source-inventory-valid", "source-inventory-missing", "source-inventory-extra",
    "source-inventory-config-extra", "source-inventory-foreign-same-count",
    "source-inventory-duplicate", "source-inventory-unsorted", "source-inventory-traversal",
    "source-inventory-absolute", "source-inventory-bool", "source-inventory-container",
    "source-inventory-map-type", "source-inventory-hash-type", "source-inventory-map-key",
    "selection-empty", "selection-valid", "selection-unordered", "selection-exact-caps",
    "selection-object-type", "selection-extra", "selection-missing", "selection-absence-container",
    "selection-alias-container", "selection-absence-cap", "selection-alias-cap",
    "selection-absence-duplicate", "selection-alias-duplicate", "selection-alias-path-duplicate",
    "selection-overlap", "selection-absence-bool", "selection-absence-relative",
    "selection-absence-traversal", "selection-absence-double-slash", "selection-absence-trailing-slash",
    "selection-alias-object-type", "selection-alias-extra", "selection-alias-missing",
    "selection-alias-path-bool", "selection-alias-path-relative", "selection-alias-path-traversal",
    "selection-resolved-type", "selection-resolved-relative", "selection-resolved-traversal",
    "retention-empty", "retention-normal-exact", "retention-terminal-exact",
    "retention-normal-current-exact", "retention-terminal-current-exact",
    "retention-normal-over", "retention-terminal-over", "retention-reserve-not-normal",
    "retention-current-bool", "retention-additional-bool", "retention-terminal-int",
    "retention-current-negative", "retention-additional-negative", "retention-current-float",
    "retention-additional-string", "retention-terminal-string", "retention-large-sum",
    "terminal-literal", "terminal-foreign", "terminal-other-receipt", "terminal-case",
    "terminal-child", "terminal-prefix", "terminal-relative", "terminal-traversal",
    "terminal-double-slash", "terminal-trailing-slash", "terminal-type", "terminal-empty",
)

_AUTHORITY = "a104aa0d7c4180d7ae7738c84580c03d047562cc83654b739474b3e7f400b48a"
_DISPOSITION = "19c176f4a8d0a5b263fd8f7bdd353f38e66b45880aaf540209171185d0ed5f7c"
_HEAD = "f00635bbd91590dbae6b7931072a973dd31e291b"
_GATE_SHA = "06a62145775baf28260c7f5d4ec6b5c7ea641dfb37a9cc4cdda54693076e3661"
_PHASES = ("qualification", "a2", "format", "base", "candidate", "integration", "consumers", "gate")
_TEMPORARY_ROOTS = ()
_MODULES = ("runner.py", "runtime.py", "policy.py", "supervise.py", "contract.py",
            "whole_program.py", "qualify.py", "qualify_contract.py", "qualify_runtime.py",
            "qualify_wrapper.py", "qualify_whole_program.py")
_CONSUMERS = ("artifact", "artifact_occurrence_migration", "evaluation",
              "candidate_evaluation_packet", "evidence_span", "outcome",
              "outcome_collector", "outcome_prepare", "outcome_stores",
              "outcome_closure", "outcome_pdf", "local_transport")
_PILOT_NAMES = (
    "pilot_old_store_ladder_reopens_original_prohibitions_unchanged",
    "pilot_public_authentication_precedes_untrusted_decode_and_missing_schema",
    "pilot_public_resident_route_stays_fail_closed_no_caller_role_authority",
)
# Retained A3 framing, with declared synthetic external target and binary suffix;
# omitted Compiling line. The target path must join the fixture's own data.
_PILOT_RAW = (
    b"    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.86s\n"
    b"     Running tests/pilot_contract.rs (/fixture/targets/candidate/debug/deps/pilot_contract-0123456789abcdef)\n"
    b"\nrunning 3 tests\n"
    b"test pilot_old_store_ladder_reopens_original_prohibitions_unchanged ... ok\n"
    b"test pilot_public_authentication_precedes_untrusted_decode_and_missing_schema ... ok\n"
    b"test pilot_public_resident_route_stays_fail_closed_no_caller_role_authority ... ok\n"
    b"\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.46s\n\n"
)
# Synthetic names, not forecast actual Rust test names. IDs are the paper roster.
_CASE_PAIRS = (
    ("V00", "v00_legacy"), ("V01", "v01_baseline"), ("V02", "v02_occurrence"),
    ("V03", "v03_context"), ("V04", "v04_predecessor"), ("V05", "v05_other_occurrence"),
    ("C01", "c01_uuid_bits"), ("C02", "c02_truncation"), ("C03", "c03_lengths"),
    ("C04", "c04_tags"), ("C05", "c05_bounds"), ("C06", "c06_identity"),
    ("C07", "c07_versions"), ("C08", "c08_projection"),
    ("S01", "s01_equal_clock"), ("S02", "s02_nonadjacent"), ("S03", "s03_accountability"),
    ("S04", "s04_authorization"), ("S05", "s05_collision"), ("S06", "s06_atomicity"),
    ("S07", "s07_reopen"), ("S08", "s08_supersession"),
    ("K01", "k01_evaluation"), ("K02", "k02_non_author"), ("K03", "k03_fidelity"),
    ("K04", "k04_collection"), ("K05", "k05_publication"),
    ("M01", "m01_legacy"), ("M02", "m02_dependencies"), ("M03", "m03_rollback"),
    ("M04", "m04_restart"),
)
_PREFIX = "artifact::artifact_phase_b_contract::"
_TOOLS = {"cargo": "/fixture/tools/cargo", "rustfmt": "/fixture/tools/rustfmt"}
_STEPS = ("stage sidecar", "cargo fmt", "cargo clippy", "cargo test", "cargo build")


def bind_temporary_roots(roots):
    """Accept only the four roots explicitly bound by the outer runner."""
    global _TEMPORARY_ROOTS
    if type(roots) is not tuple or len(roots) != 4:
        raise ControlFailure("exactly four bound temporary roots are required")
    if any(type(path) is not str or re.fullmatch(r"/tmp/[A-Za-z0-9]{1,4}", path) is None
           for path in roots):
        raise ControlFailure("temporary roots must be canonical direct /tmp children")
    if any(len(path.encode("utf-8")) > 9 for path in roots):
        raise ControlFailure("temporary root exceeds the 9-byte path bound")
    if len(set(roots)) != 4:
        raise ControlFailure("temporary roots must be distinct")
    _TEMPORARY_ROOTS = roots


class ControlFailure(RuntimeError):
    """An oracle/setup/positive failure is never an expected checker refusal."""


def _h(raw):
    return sha256(raw).hexdigest()


def _j(value):
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False) + "\n").encode()


def _pin(label):
    return {"path": "/fixture/" + label, "sha256": _h(label.encode())}


def _operator():
    packet = _pin("packet.md")
    return {
        "schema_version": "pbdep.operator-binding.v3",
        "authority": {"path": "/fixture/authority.md", "sha256": _AUTHORITY},
        "packet": packet, "acceptance": _pin("acceptance.md"), "review": _pin("review.md"),
        "disposition": {"path": "/fixture/disposition.md", "sha256": _DISPOSITION},
        "operator_attestation": {
            "acceptance_read_by": "Astra", "issuer": "Muse",
            "decision": "ACCEPT_WHOLE_PROGRAM_INSTRUMENT",
            "accepted_packet_sha256": packet["sha256"],
            "approval_is_external_record_not_this_json": True,
        },
        "instrument_sha256": {name: _h(name.encode()) for name in _MODULES},
        "phase_bindings": {name: {"state": "BOUND", **_pin(name + ".json")}
                           for name in _PHASES},
        "resource_roots": [{"directory": {"path": "/fixture/output", "dev": "1",
                            "ino": "2", "uid": "1000", "mode": "448"}, "kind": "workspace"}],
    }


def _phase_binding_raw(phase, data):
    """Independent synthetic v2 envelope for one fixed phase-data case."""
    binding = {
        "schema_version": "phase-b.phase-binding.v2", "phase": phase,
        "authority_pins": {}, "cwd": "/fixture/cwd", "environment": {},
        "python": {}, "immutable_files": [], "tmp": {}, "selection_inputs": {},
        "cargo": {}, "rustfmt": {}, "data": deepcopy(data),
        "step_zero_acceptance": {}, "source_freeze_review": {},
        "operator_phase_attestation": {},
    }
    return _j(binding)


def _installed_multicall():
    return {
        role: {
            "path": "/fixture/tools/coreutils/" + role,
            "sha256": _h(("synthetic installed multicall:" + role).encode()),
            "dev": "7", "ino": str(100 + index), "size": "1024",
            "mtime_ns": "1", "mode": "493", "uid": "0", "nlink": "2",
        }
        for index, role in enumerate(("mkdir", "wc", "dirname", "env"))
    }


def _reference(phase=None, data=None, *, operator=None, phase_binding_raw=None):
    """Construct only test-owned pins; no value comes from a validator result."""
    op = deepcopy(_operator() if operator is None else operator)
    phase_hashes = {name: "UNBOUND" for name in _PHASES[2:]}
    if phase in _PHASES[2:] and data is not None:
        raw = _phase_binding_raw(phase, data) if phase_binding_raw is None else phase_binding_raw
        op["phase_bindings"][phase] = {
            "state": "BOUND", "path": "/fixture/phase-bindings/" + phase + ".json",
            "sha256": _h(raw),
        }
        phase_hashes[phase] = _h(_j(data))
    return {
        "schema_version": "pbdep.reference.v1", "evidence_contract": "pbdep.r2-evidence.v1",
        "operator": op, "phase_data_sha256": phase_hashes,
        "installed_multicall": _installed_multicall(),
    }


def _phase_context(phase, data):
    raw = _phase_binding_raw(phase, data)
    return raw, _reference(phase, data, phase_binding_raw=raw)


def _target(name, names=("case_one",)):
    return {"kind": "tests", "source": "tests/" + name + ".rs",
            "binary_prefix": name, "names": list(names)}


def _fixture(phase="candidate"):
    root = "/fixture/" + phase
    target = "/fixture/targets/" + phase
    cases = {key: _PREFIX + name for key, name in _CASE_PAIRS}
    selected = sorted(cases.values())
    full = sorted(selected + ["alpha::unrelated", "beta::unrelated", "omega::unrelated"])
    selected_raw = ("\n".join(selected) + "\n").encode()
    full_raw = ("\n".join(full) + "\n").encode()
    listing = "".join(name + ": test\n" for name in full).encode()
    deps = ["crates/beacond/src/lib.rs", "crates/beacond/src/artifact.rs",
            "crates/beacond/src/artifact_phase_b_contract.rs"]
    if phase == "candidate":
        deps += ["crates/beacond/src/pilot_admission.rs", "crates/beacond/src/pilot_authority.rs",
                 "crates/beacond/src/pilot_store.rs"]
    files = {name: _h(("synthetic:" + name).encode()) for name in
             ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "crates/beacond/Cargo.toml", *deps]}
    manifest = {"schema_version": "phase-b.source-manifest.v1", "subject_head": _HEAD,
                "source_root": root, "files": files}
    manifest_sha = _h(_j(manifest))
    data = {"phase": phase, "subject_head": _HEAD, "source_root": root, "target_root": target,
            "source_manifest": {"path": "/fixture/" + phase + "-manifest.json", "sha256": manifest_sha},
            "manifest_sha256": manifest_sha,
            "full_roster": {"path": "/fixture/full.txt", "sha256": _h(full_raw)},
            "selected_roster": {"path": "/fixture/selected.txt", "sha256": _h(selected_raw)},
            "case_map": cases, "dep_tokens": deps,
            "dep_binding_schema": "phase-b.dep-binding.v1",
            "dep_bindings": [{"raw_token": name, "canonical_relative": name,
                              "source_sha256": files[name]} for name in deps]}
    phase_binding_raw, reference = _phase_context(phase, data)
    reference_sha = _h(_j(reference))
    dependency_observation = {
        "schema_version": "phase-b.dep-observation.v1",
        "source_root": {"path": root, "dev": "7", "ino": "2", "mode": "448", "uid": "1000"},
        "subject_head": _HEAD, "manifest_sha256": manifest_sha,
        "dep_binding_schema": "phase-b.dep-binding.v1",
        "dep_bindings_sha256": _h(_j(data["dep_bindings"])),
        "directories": {name: {"path": root + "/" + name, "dev": "7", "ino": str(index + 20),
                               "mode": "448", "uid": "1000"}
                        for index, name in enumerate(("crates", "crates/beacond", "crates/beacond/src"))},
        "files": {name: {"path": root + "/" + name, "sha256": files[name], "dev": "7",
                          "ino": str(index + 40), "size": "1", "mtime_ns": "1",
                          "mode": "384", "uid": "1000", "nlink": "1"}
                  for index, name in enumerate(deps)}}
    joins = {"source_root": root, "subject_head": _HEAD, "manifest_sha256": manifest_sha,
             "reference_sha256": reference_sha,
             "dep_binding_schema": "phase-b.dep-binding.v1",
             "dep_bindings_sha256": _h(_j(data["dep_bindings"])),
             "dependency_observation": dependency_observation}
    binary = {"path": target + "/debug/deps/beacond-0123456789abcdef",
              "sha256": _h(b"synthetic executable"), "dev": "7", "ino": "11", "size": "4096",
              "mtime_ns": "1792429317946943101"}
    record = {"reason": "compiler-artifact", "package_id": "path+file://" + root + "/crates/beacond#0.1.0",
              "manifest_path": root + "/crates/beacond/Cargo.toml",
              "target": {"kind": ["lib"], "crate_types": ["lib"], "name": "beacond",
                         "src_path": root + "/crates/beacond/src/lib.rs", "edition": "2024", "test": True},
              "profile": {"test": True}, "filenames": [binary["path"]],
              "executable": binary["path"], "fresh": False}
    compiler = _j(record) + b'{"reason":"build-finished","success":true}\n' + (
        b"    Finished `test` profile [unoptimized + debuginfo] target(s) in 1m 48s\n")
    dep_text = " ".join(deps)
    depfile = (binary["path"] + ".d: " + dep_text + "\n\n" + binary["path"] + ": " + dep_text + "\n\n"
               + "".join(name + ":\n" for name in deps)
               + "\n# env-dep:CARGO_MANIFEST_DIR=" + root + "/crates/beacond\n"
               + "# env-dep:CARGO_PKG_VERSION=0.1.0\n").encode()
    compilation = {"schema_version": "pbdep.repaired-compilation.v3", **joins,
                   "scope": "repaired-compilation-only", "binary": binary, "source_hashes": files,
                   "depfile_sha256": _h(depfile), "compiler_log_sha256": _h(compiler),
                   "manifest_sha256": manifest_sha, "subject_head": _HEAD}
    inventory = {"scope": "repaired-listing-only", "listing_sha256": _h(listing), "full_count": 34,
                 "selected_count": 31, "full_roster_sha256": _h(full_raw),
                 "selected_roster_sha256": _h(selected_raw), "case_map_sha256": _h(_j(cases))}
    admission = {"schema_version": "pbdep.repaired-admission.v3", **joins, "data_sha256": _h(_j(data)),
                 "manifest_sha256": manifest_sha, "binary": binary, "source_hashes": files,
                 "depfile_sha256": _h(depfile), "compiler_log_sha256": _h(compiler),
                 "listing_sha256": _h(listing), "compile": _capture(compiler, "1", "2"),
                 "inventory": _capture(listing, "3", "4"), "recorded_monotonic_ns": "5"}
    pre = {"schema_version": "pbdep.repaired-pre-witness.v3", **joins, "admission_sha256": _h(_j(admission)),
           "data_sha256": _h(_j(data)), "binary": binary, "source_hashes": files,
           "depfile_sha256": _h(depfile), "recorded_monotonic_ns": "6"}
    provenance = {"schema_version": "pbdep.repaired-provenance.v3",
                  "scope": "repaired-pre-dispatch-joins-only", "data_sha256": _h(_j(data)),
                  "admission_sha256": _h(_j(admission)), "pre_sha256": _h(_j(pre)),
                  "pre_monotonic_ns": pre["recorded_monotonic_ns"], "binary": binary,
                  "source_hashes": files, "depfile_sha256": _h(depfile),
                  "listing_sha256": _h(listing), **joins}
    selected_output = _direct(selected, 3)
    return {"data": data, "manifest": manifest, "binary": binary, "record": record,
            "phase_binding_raw": phase_binding_raw, "reference": reference,
            "compiler": compiler, "depfile": depfile, "full": full_raw, "selected": selected_raw,
            "listing": listing, "compilation": compilation, "inventory": inventory,
            "admission": admission, "pre": pre, "provenance": provenance,
            "selected_output": selected_output, "selected_names": selected,
            "capture": _capture(selected_output, "7", "8"),
            "dependency_observation": dependency_observation, "dependency_joins": joins}


def _capture(raw, start, end):
    return {"exit_code": 0, "whole_descendant_set_exit": "PROVEN_ECHILD", "pipe_eof": True,
            "subreaper_verified": True, "discarded_log_bytes": 0, "metadata_overflow": False,
            "errors": [], "event_error_count": 0, "stop": None, "log_sha256": _h(raw),
            "log_bytes": len(raw), "start_monotonic_ns": start, "end_monotonic_ns": end}


def _direct(names, filtered=0, observed=True):
    unit = " test\n" if len(names) == 1 else " tests\n"
    body = "\nrunning " + str(len(names)) + unit
    if observed:
        body += "".join("test " + name + " ... ok\n" for name in names)
    body += ("\ntest result: ok. " + str(len(names)) + " passed; 0 failed; 0 ignored; 0 measured; "
             + str(filtered) + " filtered out; finished in 1.46s\n\n")
    return body.encode()


def _later_data(phase):
    f = _fixture()
    data = {key: deepcopy(f["data"][key]) for key in
            ("subject_head", "source_root", "target_root", "source_manifest", "manifest_sha256")}
    data["phase"] = phase
    if phase == "integration":
        data["targets"] = [_target("pilot_contract", _PILOT_NAMES)]
    elif phase == "consumers":
        data["targets"] = [_target(name) for name in _CONSUMERS]
    elif phase == "gate":
        data["targets"] = [
            {"kind": "unittests", "source": "src/lib.rs", "binary_prefix": "beacon_desktop",
             "names": ["tests::app_data_directory_is_owner_only"]},
            {"kind": "unittests", "source": "src/main.rs", "binary_prefix": "beacon_desktop", "names": []},
            _target("pilot_contract", _PILOT_NAMES),
        ]
        data["gate_header"] = "gate: fixture/phase-b @ f00635bb"
        data["gate_sha256"] = _GATE_SHA
    else:
        raise ControlFailure("unknown fixture phase")
    return data


def _format_data():
    return {"phase": "format", "roots": ["/fixture/base", "/fixture/candidate"],
            "files": [{"root": root, "relative": "crates/beacond/src/artifact.rs",
                       "sha256": _h(root.encode())} for root in ("/fixture/base", "/fixture/candidate")],
            "allowlist": ["crates/beacond/src/artifact.rs"]}


def _cargo_body(targets, observed=True, target_root="/fixture/targets/candidate"):
    raw = b""
    for target in targets:
        label = "unittests " if target["kind"] == "unittests" else ""
        raw += ("     Running " + label + target["source"]
                + " (" + target_root + "/debug/deps/" + target["binary_prefix"] + "-0123456789abcdef)\n").encode()
        raw += _direct(target["names"], observed=observed)
    return raw


def _cargo(data, observed=True):
    return (b"    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.86s\n"
            + _cargo_body(data["targets"], observed, data["target_root"]))


def _gate(data, observed=True):
    return ((data["gate_header"] + "\n  ok    stage sidecar\n  ok    cargo fmt\n"
             + "  ok    cargo clippy\n").encode() + _cargo(data, observed)
            + b"  ok    cargo test\n    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.39s\n"
            + b"  ok    cargo build\ngate: all checks passed.\n")


def _target_facts(data, observed):
    return [{"kind": item["kind"], "source": item["source"], "binary_prefix": item["binary_prefix"],
             "count": len(item["names"]), "names_observed": list(item["names"]) if observed else []}
            for item in data["targets"]]


class _Suite:
    def __init__(self, module):
        self.module = module
        self.rows = []

    def _record(self, name):
        if name not in EXPECTED_CONTROL_IDS or any(row["id"] == name for row in self.rows):
            raise ControlFailure("control inventory drift: " + name)
        self.rows.append({"id": name, "status": "PASS"})

    def accept(self, name, function, expected, *args, **kwargs):
        actual = function(*args, **kwargs)
        if not _exact(actual, expected):
            raise ControlFailure("positive result mismatch: " + name)
        self._record(name)

    def reject(self, name, function, *args, **kwargs):
        try:
            function(*args, **kwargs)
        except self.module.Rejection as refusal:
            reason = str(refusal)
            if not re.fullmatch(r"[A-Z][A-Z0-9_]*", reason):
                raise ControlFailure("non-closed refusal: " + name) from refusal
            self._record(name)
        else:
            raise ControlFailure("negative accepted: " + name)

    def reject_type(self, name, exception, function, *args, **kwargs):
        # Only for the separately declared runtime.Stop seam; no guessed reason-code contract.
        try:
            function(*args, **kwargs)
        except exception:
            self._record(name)
        else:
            raise ControlFailure("negative accepted: " + name)


def _exact(actual, expected):
    if type(actual) is not type(expected):
        return False
    if type(expected) is dict:
        return (actual.keys() == expected.keys()
                and all(_exact(actual[key], expected[key]) for key in expected))
    if type(expected) in (list, tuple):
        return len(actual) == len(expected) and all(_exact(a, b) for a, b in zip(actual, expected))
    return actual == expected


def _changed(value, path, replacement):
    result = deepcopy(value)
    owner = result
    for key in path[:-1]:
        owner = owner[key]
    owner[path[-1]] = replacement
    return result


def _without(value, key):
    result = deepcopy(value)
    del result[key]
    return result


class _BoundWholeProgram:
    """Test adapter freezes baseline bindings and preflights accepted call shape."""
    Rejection = w.Rejection
    _DATA_APIS = frozenset(("validate_phase_data", "commands", "validate_inventory",
        "validate_compilation", "validate_admission", "validate_pre_witness",
        "validate_completion", "validate_cargo_tests", "validate_gate"))

    def __getattr__(self, name):
        function = getattr(w, name)
        if name not in self._DATA_APIS | {"validate_operator", "binding_pin",
                                          "validate_continuation", "validate_installed_tool"}:
            return function

        @wraps(function)
        def call(*args, **kwargs):
            if name in self._DATA_APIS:
                if name in ("validate_phase_data", "commands"):
                    phase, data = args[0], args[1]
                else:
                    data = kwargs["data"]
                    phase = data.get("phase") if type(data) is dict else None
                if phase in _PHASES[2:] and type(data) is dict:
                    context = getattr(self, "_baseline_contexts", {}).get(phase)
                    if context is not None:
                        raw, reference = context
                    else:
                        raw, reference = _phase_context(phase, data)
                    kwargs.setdefault("reference", reference)
                    kwargs.setdefault("phase_binding_raw", raw)
                else:
                    kwargs.setdefault("reference", _reference())
                    kwargs.setdefault("phase_binding_raw", b"no valid phase binding for this negative")
            elif name == "validate_operator":
                kwargs.setdefault("reference", _reference())
            elif name == "binding_pin":
                kwargs.setdefault("reference", _reference())
            elif name == "validate_continuation":
                kwargs.setdefault("previous_reference", _reference(operator=args[0]))
                kwargs.setdefault("reference", _reference(operator=args[1]))
            elif name == "validate_installed_tool":
                kwargs.setdefault("reference", _reference())
            try:
                signature(function).bind(*args, **kwargs)
            except TypeError as exc:
                raise ControlFailure("accepted API call shape drift: " + name) from exc
            return function(*args, **kwargs)

        return call

    def freeze_baselines(self):
        """Bind the positive phase inputs before any control mutates a copy."""
        datasets = {
            "format": _format_data(), "base": _fixture("base")["data"],
            "candidate": _fixture("candidate")["data"],
            "integration": _later_data("integration"),
            "consumers": _later_data("consumers"), "gate": _later_data("gate"),
        }
        self._baseline_contexts = {
            phase: _phase_context(phase, data) for phase, data in datasets.items()
        }
        return self


def _bound_phase(phase, data):
    """Construct a separate contract-bound malformed fixture, never a result."""
    raw, reference = _phase_context(phase, data)
    return {"phase_binding_raw": raw, "reference": reference}


def _compile_args(f):
    return {"manifest_raw": _j(f["manifest"]), "data": f["data"], "observed_binary": f["binary"],
            "observed_dependencies": f["dependency_observation"]}


def _pre_args(f):
    return {"manifest_raw": _j(f["manifest"]), "data": f["data"],
            "compilation": f["compilation"], "inventory": f["inventory"]}


def _completion_args(f):
    return {"provenance": f["provenance"], "observed_binary": f["binary"],
            "observed_sources": f["manifest"]["files"], "observed_depfile_sha256": _h(f["depfile"]),
            "observed_dependencies": f["dependency_observation"], "data": f["data"],
            "manifest_raw": _j(f["manifest"])}


def _manifest_case(data, manifest):
    """Pin an independently authored malformed manifest before its guard call."""
    rebound = deepcopy(data)
    raw = _j(manifest)
    digest = _h(raw)
    rebound["manifest_sha256"] = digest
    rebound["source_manifest"]["sha256"] = digest
    return rebound, raw


def _operator_controls(s, w):
    op = _operator()
    reference = _reference()
    s.accept("operator-valid", w.validate_operator,
             {"scope": "operator-record-shape-only", "operator_sha256": _h(_j(op)),
              "reference_sha256": _h(_j(reference))}, op, reference=reference)
    mutations = (
        ("operator-extra", {**op, "auto_approve": True}),
        ("operator-missing", _without(op, "acceptance")),
        ("operator-version", _changed(op, ("schema_version",), "phase-b.operator-binding.v1")),
        ("operator-authority", _changed(op, ("authority", "sha256"), "0" * 64)),
        ("operator-disposition", _changed(op, ("disposition", "sha256"), "0" * 64)),
        ("operator-attestation-bool", _changed(op, ("operator_attestation", "approval_is_external_record_not_this_json"), 1)),
        ("operator-packet-join", _changed(op, ("operator_attestation", "accepted_packet_sha256"), "0" * 64)),
        ("operator-instrument-missing", _changed(op, ("instrument_sha256",), _without(op["instrument_sha256"], "whole_program.py"))),
        ("operator-instrument-extra", _changed(op, ("instrument_sha256",), {**op["instrument_sha256"], "plugin.py": "0" * 64})),
        ("operator-phase-missing", _changed(op, ("phase_bindings",), _without(op["phase_bindings"], "gate"))),
        ("operator-phase-extra", _changed(op, ("phase_bindings",), {**op["phase_bindings"], "retry": {"state": "UNBOUND"}})),
        ("operator-root-mode", _changed(op, ("resource_roots", 0, "directory", "mode"), "511")),
        ("operator-root-bool", _changed(op, ("resource_roots", 0, "directory", "uid"), True)),
    )
    for name, value in mutations:
        s.reject(name, w.validate_operator, value)
    s.accept("binding-bound", w.binding_pin, _pin("candidate.json"), op, "candidate")
    unbound = _changed(op, ("phase_bindings", "candidate"), {"state": "UNBOUND"})
    s.reject("binding-unbound", w.binding_pin, unbound, "candidate",
             reference=_reference(operator=unbound))
    s.reject("binding-unknown", w.binding_pin, op, "retry")
    s.reject("binding-malformed", w.binding_pin,
             _changed(op, ("phase_bindings", "candidate", "sha256"), None), "candidate")
    previous = deepcopy(op)
    previous["phase_bindings"]["format"] = {"state": "UNBOUND"}
    completed = ("qualification", "a2")
    s.accept("continuation-valid", w.validate_continuation,
             {"scope": "stable-program-continuation-only", "completed": list(completed)}, previous, op, completed)
    changed_packet = _changed(op, ("packet",), _pin("other-packet.md"))
    changed_packet["operator_attestation"]["accepted_packet_sha256"] = changed_packet["packet"]["sha256"]
    for name, value in (
        ("continuation-packet", changed_packet),
        ("continuation-acceptance", _changed(op, ("acceptance",), _pin("other-acceptance.md"))),
        ("continuation-instrument", _changed(op, ("instrument_sha256", "whole_program.py"), "1" * 64)),
        ("continuation-completed", _changed(op, ("phase_bindings", "a2"), {"state": "BOUND", **_pin("other-a2.json")})),
    ):
        s.reject(name, w.validate_continuation, previous, value, completed)
    for name, value in (("continuation-duplicate", ("qualification", "qualification")),
                        ("continuation-order", ("a2", "qualification")),
                        ("continuation-unknown", ("qualification", "unknown"))):
        s.reject(name, w.validate_continuation, previous, op, value)


def _data_controls(s, w):
    datasets = {"base": _fixture("base")["data"], "candidate": _fixture()["data"],
                "format": _format_data(), "integration": _later_data("integration"),
                "consumers": _later_data("consumers"), "gate": _later_data("gate")}
    for phase, data in datasets.items():
        s.accept("data-" + phase, w.validate_phase_data,
                 {"scope": "phase-data-only", "phase": phase, "data_sha256": _h(_j(data))}, phase, data)
    for phase, data in datasets.items():
        malformed = {**data, "callback": "approve"}
        s.reject("data-" + phase + "-extra", w.validate_phase_data, phase, malformed,
                 **_bound_phase(phase, malformed))
    for name, phase in (("data-unknown", "retry"), ("data-qualification", "qualification"), ("data-a2", "a2")):
        s.reject(name, w.validate_phase_data, phase, datasets["candidate"])
    candidate = datasets["candidate"]
    mutations = (
        ("data-phase-mismatch", "candidate", _changed(candidate, ("phase",), "base")),
        ("data-bad-head", "candidate", _changed(candidate, ("subject_head",), "f00635b")),
        ("data-root-traversal", "candidate", _changed(candidate, ("source_root",), "/fixture/../candidate")),
        ("data-target-nested", "candidate", _changed(candidate, ("target_root",), "/fixture/candidate/target")),
        ("data-manifest-join", "candidate", _changed(candidate, ("manifest_sha256",), "0" * 64)),
        ("data-case-missing", "candidate", _changed(candidate, ("case_map",), _without(candidate["case_map"], "V00"))),
        ("data-case-duplicate", "candidate", _changed(candidate, ("case_map", "V00"), candidate["case_map"]["V01"])),
        ("data-format-foreign", "format", _changed(datasets["format"], ("files", 0, "relative"), "crates/beacond/src/foreign.rs")),
        ("data-format-duplicate", "format", _changed(datasets["format"], ("files",), datasets["format"]["files"] * 2)),
        ("data-format-one-root", "format", _changed(datasets["format"], ("files",), datasets["format"]["files"][:1])),
        ("data-format-non-rust", "format", _changed(datasets["format"], ("files", 0, "relative"), "Cargo.toml")),
        ("data-consumer-missing", "consumers", _changed(datasets["consumers"], ("targets",), datasets["consumers"]["targets"][:-1])),
        ("data-consumer-duplicate", "consumers", _changed(datasets["consumers"], ("targets", 1), datasets["consumers"]["targets"][0])),
        ("data-integration-wrong", "integration", _changed(datasets["integration"], ("targets", 0), _target("artifact"))),
        ("data-target-extra", "integration", _changed(datasets["integration"], ("targets", 0), {**datasets["integration"]["targets"][0], "allow_missing": True})),
    )
    for name, phase, value in mutations:
        s.reject(name, w.validate_phase_data, phase, value, **_bound_phase(phase, value))
    compile_argv = (_TOOLS["cargo"], "test", "--locked", "--offline", "-p", "beacond", "--lib", "--no-run", "--message-format=json")
    s.accept("argv-base-compile", w.commands, (compile_argv,), "base", datasets["base"], _TOOLS)
    for phase in ("base", "candidate"):
        executable = _fixture(phase)["binary"]["path"]
        expected = (compile_argv, (executable, "--list", "--format", "terse"),
                    (executable, "artifact::artifact_phase_b_contract::", "--nocapture", "--test-threads=1"))
        s.accept("argv-" + phase + "-full", w.commands, expected, phase, datasets[phase], _TOOLS, executable=executable)
    s.accept("argv-format", w.commands,
             ((_TOOLS["rustfmt"], "--edition", "2024", "--config", "skip_children=true",
               "/fixture/base/crates/beacond/src/artifact.rs", "/fixture/candidate/crates/beacond/src/artifact.rs"),),
             "format", datasets["format"], _TOOLS)
    prefix = (_TOOLS["cargo"], "test", "--locked", "--offline", "-p", "beacond")
    s.accept("argv-integration", w.commands,
             (prefix + ("--test", "pilot_contract", "--", "--test-threads=1"),),
             "integration", datasets["integration"], _TOOLS)
    # Closed source-derived order is literal, not copied from commands().
    consumer_args = ("--test", "artifact", "--test", "artifact_occurrence_migration",
                     "--test", "evaluation", "--test", "candidate_evaluation_packet",
                     "--test", "evidence_span", "--test", "outcome", "--test", "outcome_collector",
                     "--test", "outcome_prepare", "--test", "outcome_stores", "--test", "outcome_closure",
                     "--test", "outcome_pdf", "--test", "local_transport", "--", "--test-threads=1")
    s.accept("argv-consumers", w.commands, (prefix + consumer_args,), "consumers", datasets["consumers"], _TOOLS)
    s.accept("argv-gate", w.commands, (("/fixture/candidate/scripts/gate", "rust"),), "gate", datasets["gate"], _TOOLS)
    s.reject("argv-unknown", w.commands, "retry", candidate, _TOOLS)
    s.reject("argv-tool-missing", w.commands, "candidate", candidate, {"cargo": _TOOLS["cargo"]})
    s.reject("argv-tool-extra", w.commands, "candidate", candidate, {**_TOOLS, "shell": "/bin/sh"})
    s.reject("argv-tool-relative", w.commands, "candidate", candidate, {**_TOOLS, "cargo": "cargo"})
    s.reject("argv-executable-relative", w.commands, "candidate", candidate, _TOOLS, executable="target/test")
    s.reject("argv-executable-foreign", w.commands, "candidate", candidate, _TOOLS, executable="/foreign/beacond-0123456789abcdef")
    s.reject("argv-data-mismatch", w.commands, "base", candidate, _TOOLS,
             **_bound_phase("base", candidate))


def _inventory_compile_controls(s, w):
    f = _fixture()
    listing_args = {"full_roster": f["full"], "selected_roster": f["selected"],
                    "case_map": f["data"]["case_map"], "data": f["data"]}
    s.accept("listing-valid", w.validate_inventory, f["inventory"], f["listing"], **listing_args)
    lines = f["listing"].splitlines(keepends=True)
    foreign = sorted([b"zeta::foreign: test\n", *lines[1:]])
    original_names = [line[:-7].decode() for line in lines]
    foreign_names = [line[:-7].decode() for line in foreign]
    selected_set = set(f["selected_names"])
    # Independent mutation guard prevents syntax/count/subset failures masking full-set checking.
    if (len(lines) != 34 or len(foreign) != 34 or len(set(foreign)) != 34
            or foreign != sorted(foreign) or any(not re.fullmatch(rb"[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*: test\n", line) for line in foreign)
            or original_names[0] != "alpha::unrelated" or original_names[0] in selected_set
            or "zeta::foreign" in set(original_names) or not selected_set.issubset(foreign_names)
            or set(original_names) - set(foreign_names) != {"alpha::unrelated"}
            or set(foreign_names) - set(original_names) != {"zeta::foreign"}):
        raise ControlFailure("same-count foreign mutation is not isolated")
    raw_mutations = (
        ("listing-missing", b"".join(lines[1:])),
        ("listing-duplicate", b"".join([lines[0], *lines[:-1]])),
        ("listing-extra", f["listing"] + b"zeta::foreign: test\n"),
        ("listing-foreign-same-count", b"".join(foreign)),
        ("listing-trailer", f["listing"] + b"34 tests, 0 benchmarks\n"),
        ("listing-no-lf", f["listing"][:-1]),
        ("listing-blank", f["listing"] + b"\n"),
        ("listing-lookalike", f["listing"].replace(b"alpha", "alph\u0430".encode(), 1)),
        ("listing-whitespace", b" " + f["listing"]),
        ("listing-unsorted", b"".join(reversed(lines))),
    )
    for name, raw in raw_mutations:
        s.reject(name, w.validate_inventory, raw, **listing_args)
    s.reject("listing-case-map", w.validate_inventory, f["listing"],
             **{**listing_args, "case_map": _without(f["data"]["case_map"], "V00")})
    s.reject("listing-selected-missing", w.validate_inventory, f["listing"],
             **{**listing_args, "selected_roster": b"".join(f["selected"].splitlines(keepends=True)[1:])})
    s.reject("listing-raw-type", w.validate_inventory, f["listing"].decode(), **listing_args)
    for phase in ("base", "candidate"):
        item = _fixture(phase)
        s.accept("compile-" + phase, w.validate_compilation, item["compilation"],
                 item["compiler"], item["depfile"], **_compile_args(item))
    rest = f["compiler"].split(b"\n", 1)[1]
    record_raw = _j(f["record"])
    bad_streams = (
        ("compile-json-duplicate", record_raw[:-2] + b',"reason":"compiler-artifact"}\n' + rest),
        ("compile-json-nonfinite", record_raw[:-2] + b',"unexpected":NaN}\n' + rest),
        ("compile-terminal-false", f["compiler"].replace(b'"success":true', b'"success":false')),
        ("compile-terminal-bool-int", f["compiler"].replace(b'"success":true', b'"success":1')),
        ("compile-terminal-missing", record_raw + rest.split(b"\n", 1)[1]),
        ("compile-double-executable", record_raw + f["compiler"]),
        ("compile-message", b'{"reason":"compiler-message"}\n' + f["compiler"]),
        ("compile-source-root", _j(_changed(f["record"], ("manifest_path",), "/foreign/Cargo.toml")) + rest),
        ("compile-target-root", _j(_changed(f["record"], ("executable",), "/foreign/beacond-0123456789abcdef")) + rest),
        ("compile-profile-bool-int", _j(_changed(f["record"], ("profile", "test"), 1)) + rest),
    )
    for name, raw in bad_streams:
        s.reject(name, w.validate_compilation, raw, f["depfile"], **_compile_args(f))
    s.reject("compile-binary-path", w.validate_compilation, f["compiler"], f["depfile"],
             **{**_compile_args(f), "observed_binary": _changed(f["binary"], ("path",), "/foreign/test")})
    s.reject("compile-binary-ns-number", w.validate_compilation, f["compiler"], f["depfile"],
             **{**_compile_args(f), "observed_binary": _changed(f["binary"], ("mtime_ns",), 1792429317946943101)})
    bad_head_data, bad_head_manifest_raw = _manifest_case(
        f["data"], _changed(f["manifest"], ("subject_head",), "0" * 40))
    s.reject("compile-manifest-head", w.validate_compilation, f["compiler"], f["depfile"],
             **{**_compile_args(f), "data": bad_head_data, "manifest_raw": bad_head_manifest_raw,
                **_bound_phase("candidate", bad_head_data)})
    missing_manifest = deepcopy(f["manifest"])
    del missing_manifest["files"]["crates/beacond/src/artifact.rs"]
    missing_data, missing_manifest_raw = _manifest_case(f["data"], missing_manifest)
    s.reject("compile-missing-source", w.validate_compilation, f["compiler"], f["depfile"],
             **{**_compile_args(f), "data": missing_data, "manifest_raw": missing_manifest_raw,
                **_bound_phase("candidate", missing_data)})
    dep_mutations = (
        ("compile-dep-missing", f["depfile"].replace(b" crates/beacond/src/artifact_phase_b_contract.rs", b"", 1)),
        ("compile-dep-reorder", f["depfile"].replace(b"crates/beacond/src/lib.rs crates/beacond/src/artifact.rs",
                                                  b"crates/beacond/src/artifact.rs crates/beacond/src/lib.rs", 1)),
        ("compile-dep-env", f["depfile"].replace(b"CARGO_PKG_VERSION=0.1.0", b"CARGO_PKG_VERSION=0.2.0")),
        ("compile-dep-foreign", f["depfile"].replace(b"crates/beacond/src/lib.rs", b"../foreign.rs")),
    )
    for name, depfile in dep_mutations:
        s.reject(name, w.validate_compilation, f["compiler"], depfile, **_compile_args(f))
    s.reject("compile-truncated", w.validate_compilation, f["compiler"][:-1], f["depfile"], **_compile_args(f))


def _provenance_controls(s, w):
    f = _fixture()
    s.accept("pre-valid", w.validate_pre_witness, f["provenance"],
             f["compiler"], f["listing"], f["admission"], f["pre"], **_pre_args(f))
    changed_admissions = (
        ("pre-admission-extra", {**f["admission"], "approved": True}),
        ("pre-admission-version", _changed(f["admission"], ("schema_version",), "phase-b.pre-witness-admission.v1")),
        ("pre-admission-late", _changed(f["admission"], ("recorded_monotonic_ns",), "7")),
        ("pre-inventory-before-compile", _changed(f["admission"], ("inventory", "start_monotonic_ns"), "1")),
    )
    for name, admission in changed_admissions:
        # Rejoin the changed admission's identity so that a stale outer pin cannot mask the intended guard.
        pre = _changed(f["pre"], ("admission_sha256",), _h(_j(admission)))
        s.reject(name, w.validate_pre_witness, f["compiler"], f["listing"], admission, pre, **_pre_args(f))
    for name, pre in (
        ("pre-pre-before-admission", _changed(f["pre"], ("recorded_monotonic_ns",), "4")),
        ("pre-binary-drift", _changed(f["pre"], ("binary", "mtime_ns"), "1792429317946943000")),
        ("pre-source-drift", _changed(f["pre"], ("source_hashes", "Cargo.lock"), "0" * 64)),
        ("pre-depfile-drift", _changed(f["pre"], ("depfile_sha256",), "0" * 64)),
    ):
        s.reject(name, w.validate_pre_witness, f["compiler"], f["listing"], f["admission"], pre, **_pre_args(f))
    s.reject("pre-listing-drift", w.validate_pre_witness, f["compiler"],
             f["listing"].replace(b"alpha::unrelated", b"aardvark::unrelated"),
             f["admission"], f["pre"], **_pre_args(f))
    s.reject("pre-compiler-drift", w.validate_pre_witness,
             f["compiler"].replace(b"1m 48s", b"1m 49s"), f["listing"],
             f["admission"], f["pre"], **_pre_args(f))
    for name, path, value in (
        ("pre-capture-eof", ("compile", "pipe_eof"), False),
        ("pre-capture-custody", ("inventory", "whole_descendant_set_exit"), "UNKNOWN"),
        ("pre-capture-discard", ("inventory", "discarded_log_bytes"), 1),
        ("pre-capture-exit-bool", ("compile", "exit_code"), False),
        ("pre-capture-time-number", ("compile", "start_monotonic_ns"), 1),
    ):
        admission = _changed(f["admission"], path, value)
        pre = _changed(f["pre"], ("admission_sha256",), _h(_j(admission)))
        s.reject(name, w.validate_pre_witness, f["compiler"], f["listing"], admission, pre, **_pre_args(f))
    s.reject("pre-admission-pin", w.validate_pre_witness, f["compiler"], f["listing"], f["admission"],
             _changed(f["pre"], ("admission_sha256",), "0" * 64), **_pre_args(f))
    s.reject("pre-data-pin", w.validate_pre_witness, f["compiler"], f["listing"], f["admission"],
             _changed(f["pre"], ("data_sha256",), "0" * 64), **_pre_args(f))


def _completion_controls(s, w):
    f = _fixture()
    raw = f["selected_output"]
    first_line = ("test " + f["selected_names"][0] + " ... ok\n").encode()
    expected = {"scope": "selected-assertion-completion-only", "raw_sha256": _h(raw),
                "passed": 31, "filtered_out": 3, "names": f["selected_names"]}
    args = {"selected_roster": f["selected"], "full_count": 34, "exit_code": 0}
    s.accept("selected-valid", w.validate_selected, expected, raw, **args)
    changes = (
        ("selected-missing", raw.replace(first_line, b"", 1)),
        ("selected-duplicate", raw.replace(first_line, first_line * 2, 1)),
        ("selected-extra", raw.replace(first_line, first_line + b"test foreign::extra ... ok\n", 1)),
        ("selected-failed", raw.replace(b" ... ok\n", b" ... FAILED\n", 1)),
        ("selected-ignored", raw.replace(b" ... ok\n", b" ... ignored\n", 1)),
        ("selected-count", raw.replace(b"31 passed;", b"30 passed;")),
        ("selected-filtered", raw.replace(b"3 filtered out;", b"2 filtered out;")),
        ("selected-measured", raw.replace(b"0 measured;", b"1 measured;")),
        ("selected-summary-missing", raw[:raw.index(b"test result:")]),
        ("selected-summary-duplicate", raw + raw[raw.index(b"test result:"):]),
        ("selected-cargo-footer", raw + b"error: test failed, to rerun pass `-p beacond --lib`\n"),
        ("selected-no-lf", raw.rstrip(b"\n")),
    )
    for name, value in changes:
        s.reject(name, w.validate_selected, value, **args)
    s.reject("selected-exit-bool", w.validate_selected, raw, **{**args, "exit_code": False})
    s.reject("selected-exit-nonzero", w.validate_selected, raw, **{**args, "exit_code": 101})
    s.reject("selected-count-bool", w.validate_selected, raw, **{**args, "full_count": True})
    s.reject("selected-nonfinite-time", w.validate_selected, raw.replace(b"1.46s", b"NaNs"), **args)
    completion = {"schema_version": "pbdep.repaired-completion.v3", **f["dependency_joins"],
                  "depfile_sha256": _h(f["depfile"]),
                  "scope": "repaired-post-witness-custody-only", "raw_sha256": _h(raw),
                  "pre_sha256": f["provenance"]["pre_sha256"]}
    s.accept("completion-valid", w.validate_completion, completion, raw, f["capture"], **_completion_args(f))
    for name, key, value in (
        ("completion-eof", "pipe_eof", False),
        ("completion-custody", "whole_descendant_set_exit", "LEADER_EXITED"),
        ("completion-subreaper", "subreaper_verified", False),
        ("completion-discard", "discarded_log_bytes", 1),
        ("completion-overflow", "metadata_overflow", True),
        ("completion-errors", "errors", ["synthetic custody error"]),
        ("completion-event-errors", "event_error_count", 1),
        ("completion-stop", "stop", "DEADLINE"),
    ):
        s.reject(name, w.validate_completion, raw, _changed(f["capture"], (key,), value), **_completion_args(f))
    s.reject("completion-late-pre", w.validate_completion, raw, f["capture"],
             **{**_completion_args(f), "provenance": _changed(f["provenance"], ("pre_monotonic_ns",), "9")})
    s.reject("completion-time-reversed", w.validate_completion, raw,
             _changed(f["capture"], ("end_monotonic_ns",), "6"), **_completion_args(f))
    s.reject("completion-raw-drift", w.validate_completion, raw + b"\n", f["capture"], **_completion_args(f))
    s.reject("completion-binary-drift", w.validate_completion, raw, f["capture"],
             **{**_completion_args(f), "observed_binary": _changed(f["binary"], ("ino",), "12")})
    s.reject("completion-source-drift", w.validate_completion, raw, f["capture"],
             **{**_completion_args(f), "observed_sources": _changed(f["manifest"]["files"], ("Cargo.lock",), "1" * 64)})
    s.reject("completion-depfile-drift", w.validate_completion, raw, f["capture"],
             **{**_completion_args(f), "observed_depfile_sha256": "1" * 64})
    s.reject("completion-exit-bool", w.validate_completion, raw,
             _changed(f["capture"], ("exit_code",), False), **_completion_args(f))
    s.reject("completion-extra", w.validate_completion, raw, {**f["capture"], "approval": True}, **_completion_args(f))


def _cargo_gate_controls(s, w):
    integration = _later_data("integration")
    consumers = _later_data("consumers")
    gate = _later_data("gate")
    if _cargo(integration) != _PILOT_RAW:
        raise ControlFailure("fixture diverged from retained A3 external-target grammar")
    # Actual pristine-sample.log line 5 supplies the singular shape independently.
    if (b"\nrunning 1 test\n" not in _cargo(consumers)
            or b"\nrunning 1 tests\n" in _cargo(consumers)):
        raise ControlFailure("fixture diverged from retained one-test libtest grammar")
    for name, data in (("cargo-integration", integration), ("cargo-consumers", consumers)):
        raw = _cargo(data)
        s.accept(name, w.validate_cargo_tests,
                 {"scope": "cargo-target-completion-only", "raw_sha256": _h(raw),
                  "targets": _target_facts(data, True),
                  "test_count": sum(len(item["names"]) for item in data["targets"])},
                 raw, data=data, exit_code=0)
    s.reject("cargo-singular-pluralized", w.validate_cargo_tests,
             _cargo(consumers).replace(b"\nrunning 1 test\n", b"\nrunning 1 tests\n", 1),
             data=consumers, exit_code=0)
    s.reject("cargo-plural-singularized", w.validate_cargo_tests,
             _cargo(integration).replace(b"\nrunning 3 tests\n", b"\nrunning 3 test\n", 1),
             data=integration, exit_code=0)
    raw = _cargo(integration)
    target_block = _cargo_body(integration["targets"])
    first_line = ("test " + _PILOT_NAMES[0] + " ... ok\n").encode()
    mutations = (
        ("cargo-target-missing", raw.replace(target_block, b"")),
        ("cargo-target-duplicate", raw + target_block),
        ("cargo-target-foreign", raw + _cargo_body([_target("foreign")])) ,
        ("cargo-header-source", raw.replace(b"tests/pilot_contract.rs", b"tests/foreign.rs")),
        ("cargo-header-binary", raw.replace(b"deps/pilot_contract-", b"deps/foreign-")),
        ("cargo-name-missing", raw.replace(first_line, b"")),
        ("cargo-name-duplicate", raw.replace(first_line, first_line * 2)),
        ("cargo-name-foreign", raw.replace(_PILOT_NAMES[0].encode(), b"foreign_case", 1)),
        ("cargo-aggregate-refused", _cargo(integration, observed=False)),
        ("cargo-failed", raw.replace(b" ... ok\n", b" ... FAILED\n", 1)),
        ("cargo-ignored", raw.replace(b"0 ignored;", b"1 ignored;")),
        ("cargo-filtered", raw.replace(b"0 filtered out;", b"1 filtered out;")),
        ("cargo-summary-missing", raw[:raw.index(b"test result:")]),
    )
    for name, value in mutations:
        s.reject(name, w.validate_cargo_tests, value, data=integration, exit_code=0)
    s.reject("cargo-exit-bool", w.validate_cargo_tests, raw, data=integration, exit_code=False)
    s.reject("cargo-exit-nonzero", w.validate_cargo_tests, raw, data=integration, exit_code=101)
    for name, observed, data in (("gate-named", True, gate), ("gate-aggregate", False, gate)):
        gate_raw = _gate(data, observed)
        s.accept(name, w.validate_gate,
                 {"scope": "full-rust-gate-only", "raw_sha256": _h(gate_raw),
                  "targets": _target_facts(data, observed),
                  "test_count": sum(len(item["names"]) for item in data["targets"]), "checks": list(_STEPS)},
                 gate_raw, data=data, exit_code=0)
    # This distinct target-set positive owns its own raw envelope and canonical
    # reference.  The frozen general adapter remains bound to the unmutated
    # full-target baseline for every other case.
    zero_target = {**gate, "targets": [gate["targets"][1]]}
    zero_target_raw, zero_target_reference = _phase_context("gate", zero_target)
    gate_raw = _gate(zero_target)
    s.accept("gate-zero-target", w.validate_gate,
             {"scope": "full-rust-gate-only", "raw_sha256": _h(gate_raw),
              "targets": _target_facts(zero_target, False),
              "test_count": sum(len(item["names"]) for item in zero_target["targets"]),
              "checks": list(_STEPS)},
             gate_raw, data=zero_target, reference=zero_target_reference,
             phase_binding_raw=zero_target_raw, exit_code=0)
    s.reject("gate-singular-pluralized", w.validate_gate,
             _gate(gate).replace(b"\nrunning 1 test\n", b"\nrunning 1 tests\n", 1),
             data=gate, exit_code=0)
    s.reject("gate-zero-singularized", w.validate_gate,
             _gate(gate).replace(b"\nrunning 0 tests\n", b"\nrunning 0 test\n", 1),
             data=gate, exit_code=0)
    raw = _gate(gate)
    pilot_line = ("test " + _PILOT_NAMES[0] + " ... ok\n").encode()
    all_pilot_lines = b"".join(("test " + name + " ... ok\n").encode() for name in _PILOT_NAMES)
    first_block = _cargo_body([gate["targets"][0]])
    mutations = (
        ("gate-step-missing", raw.replace(b"  ok    cargo fmt\n", b"")),
        ("gate-step-duplicate", raw.replace(b"  ok    cargo fmt\n", b"  ok    cargo fmt\n" * 2)),
        ("gate-step-order", raw.replace(b"  ok    stage sidecar\n  ok    cargo fmt\n",
                                       b"  ok    cargo fmt\n  ok    stage sidecar\n")),
        ("gate-step-failed", raw.replace(b"  ok    cargo clippy\n", b"  FAIL  cargo clippy\n")),
        ("gate-footer", raw.replace(b"gate: all checks passed.\n", b"gate: FAILED -- not safe to commit.\n")),
        ("gate-trailing", raw + b"unexpected trailing output\n"),
        ("gate-header", raw.replace(gate["gate_header"].encode(), b"gate: foreign @ 00000000", 1)),
        ("gate-target-missing", raw.replace(first_block, b"", 1)),
        ("gate-target-duplicate", raw.replace(first_block, first_block * 2, 1)),
        ("gate-target-foreign", raw.replace(first_block, first_block + _cargo_body([_target("foreign")]), 1)),
        ("gate-partial-names", raw.replace(pilot_line, b"", 1)),
        ("gate-aggregate-count", raw.replace(all_pilot_lines, b"").replace(b"3 passed;", b"2 passed;", 1)),
    )
    for name, value in mutations:
        s.reject(name, w.validate_gate, value, data=gate, exit_code=0)
    s.reject("gate-exit-nonzero", w.validate_gate, raw, data=gate, exit_code=1)
    s.reject("gate-exit-bool", w.validate_gate, raw, data=gate, exit_code=False)
    missing_summary = raw.replace(b"test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.46s\n", b"", 1)
    s.reject("gate-summary-missing", w.validate_gate, missing_summary, data=gate, exit_code=0)
    s.accept("format-valid", w.validate_format, {"scope": "formatter-exit-only", "raw_sha256": _h(b"")}, b"", exit_code=0)
    s.reject("format-output", w.validate_format, b"changed file\n", exit_code=0)
    s.reject("format-whitespace", w.validate_format, b"\n", exit_code=0)
    s.reject("format-exit-nonzero", w.validate_format, b"", exit_code=1)
    s.reject("format-exit-bool", w.validate_format, b"", exit_code=False)
    s.reject("format-exit-float", w.validate_format, b"", exit_code=0.0)
    s.reject("format-raw-type", w.validate_format, "", exit_code=0)


def _receipt_cap_controls(s, runtime):
    # Literal byte capacities independently fixed by the public runtime seam, not read from its constants.
    for name, filename, expected in (
        ("cap-operator", "OPERATOR.json", 2097152),
        ("cap-phase", "PHASE-BINDING.json", 2097152),
        ("cap-result", "RESULT.json", 2097152),
        ("cap-ordinary", "CAPTURE.json", 131072),
        ("cap-case-sensitive", "operator.json", 131072),
        ("cap-extra-suffix", "OPERATOR_COPY.json", 131072),
    ):
        s.accept(name, runtime.receipt_cap, expected, filename)
    for name, filename in (
        ("cap-invalid-empty", ""), ("cap-invalid-absolute", "/OPERATOR.json"),
        ("cap-invalid-traversal", "../OPERATOR.json"), ("cap-invalid-hidden", ".OPERATOR.json"),
        ("cap-invalid-extension", "OPERATOR.JSON"), ("cap-invalid-unicode", "OPERAT\u041eR.json"),
        ("cap-invalid-type", True),
    ):
        s.reject_type(name, runtime.Stop, runtime.receipt_cap, filename)
    large = b"x" * 2097152
    ordinary = b"x" * 131072
    for name, filename, raw in (
        ("receipt-operator-exact", "OPERATOR.json", large),
        ("receipt-phase-exact", "PHASE-BINDING.json", large),
        ("receipt-result-exact", "RESULT.json", large),
        ("receipt-ordinary-exact", "CAPTURE.json", ordinary),
    ):
        s.accept(name, runtime.validate_receipt_bytes, len(raw), filename, raw)
    for name, filename, raw in (
        ("receipt-operator-over", "OPERATOR.json", large + b"x"),
        ("receipt-phase-over", "PHASE-BINDING.json", large + b"x"),
        ("receipt-result-over", "RESULT.json", large + b"x"),
        ("receipt-ordinary-over", "CAPTURE.json", ordinary + b"x"),
        ("receipt-lowercase-over", "operator.json", ordinary + b"x"),
    ):
        s.reject_type(name, runtime.Stop, runtime.validate_receipt_bytes, filename, raw)
    s.reject_type("receipt-raw-type", runtime.Stop, runtime.validate_receipt_bytes, "RESULT.json", "")
    s.reject_type("receipt-name-before-cap", runtime.Stop, runtime.validate_receipt_bytes, "../RESULT.json", b"")


def _resource_controls(s, runtime):
    if len(_TEMPORARY_ROOTS) != 4:
        raise ControlFailure("runner did not bind all four temporary roots")
    s.accept("resource-directory", runtime.resource_entry,
             {"entry_type": "directory", "descend": True, "accounted_bytes": 0},
             "/fixture/output", "workspace", False, 0o40700, 4096,
             temporary_roots=_TEMPORARY_ROOTS)
    s.accept("resource-regular", runtime.resource_entry,
             {"entry_type": "regular", "descend": False, "accounted_bytes": 17},
             "/fixture/output", "workspace", False, 0o100600, 17,
             temporary_roots=_TEMPORARY_ROOTS)
    s.accept("resource-zero-size", runtime.resource_entry,
             {"entry_type": "regular", "descend": False, "accounted_bytes": 0},
             "/fixture/output", "workspace", True, 0o100600, 0,
             temporary_roots=_TEMPORARY_ROOTS)
    temporary = dict(zip(("q", "a", "b", "c"), _TEMPORARY_ROOTS, strict=True))
    for kind, mode in (("link", 0o120777), ("socket", 0o140600)):
        for suffix in ("q", "a", "b", "c"):
            s.accept("resource-" + suffix + "-" + kind, runtime.resource_entry,
                     {"entry_type": "symlink" if kind == "link" else "socket",
                      "descend": False, "accounted_bytes": 23},
                     temporary[suffix], "temporary", False, mode, 23,
                     temporary_roots=_TEMPORARY_ROOTS)
    mutations = (
        ("resource-workspace-link", _TEMPORARY_ROOTS[0], "workspace", False, 0o120777, 23),
        ("resource-capture-link", _TEMPORARY_ROOTS[0], "temporary", True, 0o120777, 23),
        ("resource-workspace-socket", _TEMPORARY_ROOTS[0], "workspace", False, 0o140600, 23),
        ("resource-capture-socket", _TEMPORARY_ROOTS[0], "temporary", True, 0o140600, 23),
        ("resource-foreign-root", "/fixture/foreign-temporary", "temporary", False, 0o120777, 23),
        ("resource-prefix-root", "/tmp/q-extra", "temporary", False, 0o120777, 23),
        ("resource-child-root", _TEMPORARY_ROOTS[0] + "/child", "temporary", False, 0o120777, 23),
        ("resource-fifo", _TEMPORARY_ROOTS[0], "temporary", False, 0o10600, 23),
        ("resource-character", _TEMPORARY_ROOTS[0], "temporary", False, 0o20600, 23),
        ("resource-block", _TEMPORARY_ROOTS[0], "temporary", False, 0o60600, 23),
        ("resource-unknown-mode", _TEMPORARY_ROOTS[0], "temporary", False, 0, 23),
        ("resource-mode-bool", _TEMPORARY_ROOTS[0], "temporary", False, True, 23),
        ("resource-size-bool", _TEMPORARY_ROOTS[0], "temporary", False, 0o100600, True),
        ("resource-size-negative", _TEMPORARY_ROOTS[0], "temporary", False, 0o100600, -1),
        ("resource-mode-over", _TEMPORARY_ROOTS[0], "temporary", False, 65536, 23),
        ("resource-capture-int", _TEMPORARY_ROOTS[0], "temporary", 0, 0o100600, 23),
        ("resource-kind-unknown", _TEMPORARY_ROOTS[0], "state", False, 0o100600, 23),
        ("resource-root-relative", "tmp/b2q", "temporary", False, 0o100600, 23),
        ("resource-root-traversal", _TEMPORARY_ROOTS[0] + "/../" +
            _TEMPORARY_ROOTS[0].rsplit("/", 1)[-1], "temporary", False, 0o100600, 23),
    )
    for name, root, kind, captures, mode, size in mutations:
        s.reject_type(name, runtime.Stop, runtime.resource_entry, root, kind, captures, mode, size,
                      temporary_roots=_TEMPORARY_ROOTS)


def _tool_controls(s, w):
    ordinary = {"path": "/fixture/tools/cargo", "sha256": _h(b"ordinary synthetic tool"),
                "dev": "7", "ino": "11", "size": "4096", "mtime_ns": "1792429317946943101",
                "mode": "493", "uid": "1000", "nlink": "1"}
    ordinary_expected = {"scope": "file-link-policy-only", "link_policy": "ordinary-single-link",
                         "path": ordinary["path"]}
    s.accept("tool-ordinary", w.validate_installed_tool, ordinary_expected,
             ordinary, file_class="ordinary-input")
    s.accept("tool-ordinary-installed", w.validate_installed_tool, ordinary_expected,
             ordinary, file_class="immutable-installed-tool")
    literal = _installed_multicall()["mkdir"]
    for name in ("mkdir", "wc", "dirname", "env"):
        record = _installed_multicall()[name]
        s.accept("tool-" + name, w.validate_installed_tool,
                 {"scope": "file-link-policy-only", "link_policy": "literal-installed-multicall",
                  "path": record["path"]}, record, file_class="immutable-installed-tool")
    s.reject("tool-foreign-path", w.validate_installed_tool,
             {**literal, "path": "/fixture/tools/coreutils/cat"}, file_class="immutable-installed-tool")
    s.reject("tool-wrong-class", w.validate_installed_tool, literal, file_class="ordinary-input")
    s.reject("tool-unknown-class", w.validate_installed_tool, literal, file_class="trusted")
    s.reject("tool-class-bool", w.validate_installed_tool, literal, file_class=True)
    s.reject("tool-extra", w.validate_installed_tool, {**literal, "allow_hardlinks": True}, file_class="immutable-installed-tool")
    s.reject("tool-missing", w.validate_installed_tool, _without(literal, "uid"), file_class="immutable-installed-tool")
    for name, key, value in (
        ("tool-uid", "uid", "65534"), ("tool-mode", "mode", "511"),
        ("tool-nlink", "nlink", "115"), ("tool-hash", "sha256", "0" * 64),
        ("tool-inode", "ino", "93457178"), ("tool-device", "dev", "66307"),
        ("tool-size", "size", "11807105"), ("tool-mtime", "mtime_ns", "1788422510000000001"),
        ("tool-metadata-int", "nlink", 116), ("tool-metadata-bool", "uid", False),
        ("tool-leading-zero", "uid", "00"), ("tool-relative-path", "path", "coreutils/mkdir"),
    ):
        s.reject(name, w.validate_installed_tool, {**literal, key: value}, file_class="immutable-installed-tool")
    s.reject("tool-ordinary-multilink", w.validate_installed_tool, {**ordinary, "nlink": "2"}, file_class="ordinary-input")


def _source_inventory_controls(s, w):
    expected = _fixture()["manifest"]["files"]
    names = sorted(expected)
    s.accept("source-inventory-valid", w.validate_source_inventory,
             {"scope": "source-inventory-only", "file_count": len(names), "inventory_sha256": _h(_j(names))},
             names, expected)
    foreign = sorted([name for name in names if name != "Cargo.lock"] + ["build.rs"])
    if ("Cargo.lock" not in expected or "build.rs" in expected or ".cargo/config.toml" in expected
            or len(foreign) != len(names) or len(set(foreign)) != len(names)
            or set(names) - set(foreign) != {"Cargo.lock"}
            or set(foreign) - set(names) != {"build.rs"}):
        raise ControlFailure("source-inventory replacement mutation is not isolated")
    for name, actual in (
        ("source-inventory-missing", names[1:]),
        ("source-inventory-extra", sorted([*names, "build.rs"])),
        ("source-inventory-config-extra", sorted([*names, ".cargo/config.toml"])),
        ("source-inventory-foreign-same-count", foreign),
        ("source-inventory-duplicate", sorted([*names, names[0]])),
        ("source-inventory-unsorted", list(reversed(names))),
        ("source-inventory-traversal", ["../Cargo.lock", *names[1:]]),
        ("source-inventory-absolute", ["/Cargo.lock", *names[1:]]),
        ("source-inventory-bool", [True, *names[1:]]),
        ("source-inventory-container", tuple(names)),
    ):
        s.reject(name, w.validate_source_inventory, actual, expected)
    s.reject("source-inventory-map-type", w.validate_source_inventory, names, list(expected.items()))
    s.reject("source-inventory-hash-type", w.validate_source_inventory, names,
             {**expected, "Cargo.lock": True})
    s.reject("source-inventory-map-key", w.validate_source_inventory, names,
             {**expected, False: "0" * 64})


def _selection_controls(s, w):
    value = {"absent_paths": ["/fixture/absent-a", "/fixture/absent-b"],
             "aliases": [{"path": "/fixture/alias-a", "resolved_path": "/fixture/tool-a"},
                         {"path": "/fixture/alias-b", "resolved_path": "/fixture/tool-b"}]}
    unordered = {"absent_paths": list(reversed(value["absent_paths"])),
                 "aliases": list(reversed(value["aliases"]))}
    cap_value = {"absent_paths": ["/fixture/absent-" + str(n) for n in range(128)],
                 "aliases": [{"path": "/fixture/alias-" + str(n),
                              "resolved_path": "/fixture/tool-" + str(n)} for n in range(64)]}
    for name, item in (("selection-empty", {"absent_paths": [], "aliases": []}),
                       ("selection-valid", value), ("selection-unordered", unordered),
                       ("selection-exact-caps", cap_value)):
        s.accept(name, w.validate_selection_inputs,
                 {"scope": "selection-input-shape-only", "selection_sha256": _h(_j(item))}, item)
    s.reject("selection-object-type", w.validate_selection_inputs, True)
    s.reject("selection-extra", w.validate_selection_inputs, {**value, "discover": True})
    s.reject("selection-missing", w.validate_selection_inputs, _without(value, "aliases"))
    s.reject("selection-absence-container", w.validate_selection_inputs,
             {**value, "absent_paths": tuple(value["absent_paths"])})
    s.reject("selection-alias-container", w.validate_selection_inputs,
             {**value, "aliases": tuple(value["aliases"])})
    s.reject("selection-absence-cap", w.validate_selection_inputs,
             {**cap_value, "absent_paths": [*cap_value["absent_paths"], "/fixture/absent-128"]})
    s.reject("selection-alias-cap", w.validate_selection_inputs,
             {**cap_value, "aliases": [*cap_value["aliases"],
                                       {"path": "/fixture/alias-64", "resolved_path": "/fixture/tool-64"}]})
    s.reject("selection-absence-duplicate", w.validate_selection_inputs,
             {**value, "absent_paths": [*value["absent_paths"], value["absent_paths"][0]]})
    s.reject("selection-alias-duplicate", w.validate_selection_inputs,
             {**value, "aliases": [*value["aliases"], deepcopy(value["aliases"][0])]})
    s.reject("selection-alias-path-duplicate", w.validate_selection_inputs,
             {**value, "aliases": [*value["aliases"],
                                   {"path": "/fixture/alias-a", "resolved_path": "/fixture/tool-c"}]})
    s.reject("selection-overlap", w.validate_selection_inputs,
             {**value, "absent_paths": [*value["absent_paths"], "/fixture/alias-a"]})
    for name, path in (
        ("selection-absence-bool", True), ("selection-absence-relative", "fixture/absent"),
        ("selection-absence-traversal", "/fixture/../absent"),
        ("selection-absence-double-slash", "/fixture//absent"),
        ("selection-absence-trailing-slash", "/fixture/absent/"),
    ):
        s.reject(name, w.validate_selection_inputs, {**value, "absent_paths": [path]})
    alias = value["aliases"][0]
    for name, item in (
        ("selection-alias-object-type", True),
        ("selection-alias-extra", {**alias, "follow": True}),
        ("selection-alias-missing", _without(alias, "resolved_path")),
        ("selection-alias-path-bool", {**alias, "path": False}),
        ("selection-alias-path-relative", {**alias, "path": "fixture/alias"}),
        ("selection-alias-path-traversal", {**alias, "path": "/fixture/./alias"}),
        ("selection-resolved-type", {**alias, "resolved_path": False}),
        ("selection-resolved-relative", {**alias, "resolved_path": "fixture/tool"}),
        ("selection-resolved-traversal", {**alias, "resolved_path": "/fixture/../tool"}),
    ):
        s.reject(name, w.validate_selection_inputs, {**value, "aliases": [item]})


def _retention_controls(s, runtime):
    # Literal 60/64 MiB arithmetic is independent of runtime's constants and tally.
    for name, current, additional, terminal, projected in (
        ("retention-empty", 0, 0, False, 0),
        ("retention-normal-exact", 62914559, 1, False, 62914560),
        ("retention-terminal-exact", 67108863, 1, True, 67108864),
        ("retention-normal-current-exact", 62914560, 0, False, 62914560),
        ("retention-terminal-current-exact", 67108864, 0, True, 67108864),
    ):
        s.accept(name, runtime.retention_budget, projected, current, additional, terminal)
    for name, current, additional, terminal in (
        ("retention-normal-over", 62914560, 1, False),
        ("retention-terminal-over", 67108864, 1, True),
        ("retention-reserve-not-normal", 62914560, 4194304, False),
        ("retention-current-bool", False, 0, False),
        ("retention-additional-bool", 0, False, False),
        ("retention-terminal-int", 0, 0, 1),
        ("retention-current-negative", -1, 1, False),
        ("retention-additional-negative", 1, -1, False),
        ("retention-current-float", 0.0, 0, False),
        ("retention-additional-string", 0, "0", False),
        ("retention-terminal-string", 0, 0, "true"),
        ("retention-large-sum", 1 << 128, 1 << 128, True),
    ):
        s.reject_type(name, runtime.Stop, runtime.retention_budget, current, additional, terminal)
    state = "/fixture/legacy-runtime-state"
    terminal = state + "/STOP.json"
    for name, path, expected in (
        ("terminal-literal", terminal, True),
        ("terminal-foreign", "/fixture/legacy/STOP.json", False),
        ("terminal-other-receipt", state + "/RESULT.json", False),
        ("terminal-case", state + "/stop.json", False),
        ("terminal-child", state + "/child/STOP.json", False),
        ("terminal-prefix", state + "-other/STOP.json", False),
    ):
        s.accept(name, runtime.terminal_receipt, expected, path, state_root=state)
    for name, path in (
        ("terminal-relative", "runtime-state/STOP.json"),
        ("terminal-traversal", state + "/child/../STOP.json"),
        ("terminal-double-slash", state + "//STOP.json"),
        ("terminal-trailing-slash", terminal + "/"),
        ("terminal-type", True), ("terminal-empty", ""),
    ):
        s.reject_type(name, runtime.Stop, runtime.terminal_receipt, path, state_root=state)


def run_whole_program_controls():
    """Run only the finite source controls; no process, provider, or authority."""
    api = _BoundWholeProgram().freeze_baselines()
    suite = _Suite(api)
    _operator_controls(suite, api)
    _data_controls(suite, api)
    _inventory_compile_controls(suite, api)
    _provenance_controls(suite, api)
    _completion_controls(suite, api)
    _cargo_gate_controls(suite, api)
    _receipt_cap_controls(suite, R)
    _resource_controls(suite, R)
    _tool_controls(suite, api)
    _source_inventory_controls(suite, api)
    _selection_controls(suite, api)
    _retention_controls(suite, R)
    if [row["id"] for row in suite.rows] != list(EXPECTED_CONTROL_IDS):
        raise ControlFailure("historical control order/count drift")
    return suite.rows
