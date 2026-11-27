# PBDEP source-only verifier interface v2

This package is an offline, pure evidence interpreter. It reads no files, clocks,
environment, processes or providers and dispatches nothing. Supplied observations
and self-consistent hashes never establish filesystem custody, authenticity or
permission. This source has not been behaviorally qualified.

Python namespace package: scripts.qualification.pbdep. There is no entry point or
runtime shim. whole_program imports only its sibling contract plus stdlib;
policy and contract use only stdlib. Import/execution is not admitted by this
source-preparation task.

## Independent reference, required at the trust boundary

whole_program reference is an exact dict with these five keys:
- schema_version: literal pbdep.reference.v1.
- evidence_contract: literal pbdep.r2-evidence.v1.
- operator: the independently reviewed full pbdep.operator-binding.v3 record.
- phase_data_sha256: exact six keys format, base, candidate, integration, consumers,
  gate; each value is a lowercase SHA256 or explicit UNBOUND. A phase validator
  rejects UNBOUND. Digest is over encoded(data): sorted-key, compact JSON, no NaN,
  trailing LF, UTF-8 with json.dumps' retained ASCII escaping.
- installed_multicall: exact roles mkdir, wc, dirname, env, each an exact
  path/sha256/dev/ino/size/mtime_ns/mode/uid/nlink record. Absolute normalized path
  basename must match its role. All numeric metadata are canonical decimal
  strings; inode/size/nlink positive, uid0 and mode493 (0755). No old device,
  inode, digest, path or timestamp is supplied by this package.

Operator record retains exact authority, packet, acceptance, review, disposition
pins (each path and SHA256); operator_attestation; instrument_sha256;
phase_bindings; resource_roots; schema_version. Its schema is now
pbdep.operator-binding.v3. Attestation has acceptance_read_by, issuer, decision,
accepted_packet_sha256, approval_is_external_record_not_this_json. Names must
be bounded identifiers excluding UNKNOWN/UNBOUND; decision remains the closed
ACCEPT_WHOLE_PROGRAM_INSTRUMENT and the last boolean must literally be True.
Neither these labels nor the boolean attest that a human approved anything.

The full instrument map is a required finite map (1..64 basename .py keys and
SHA256 values). Comparing operator with the independent reference closes its
exact roster; no old eleven-module default or later-phase stub is installed. Phase
bindings retain all eight finite phase names and exact {state: UNBOUND} or
{state: BOUND, path, sha256} shapes. Resource roots retain1..12 supplied directory
identities, workspace/temporary kind and mode448 (0700). validate_operator
requires full equality with reference.operator, including those roots and pins.

The future binding caller must acquire this reference through an independent authenticated
review/admission path; verify its external grant, acceptance, subject, input bytes,
instrument identities and exact phase bindings; reject stale/reused sessions;
and freeze it separately from candidate evidence. Never derive expected digests
from the same untrusted evidence being scored. validate_reference proves shape
only. Its hash is a join, not a signature. This preparation supplies no active
reference or reservation and cannot authorize any future action.

## Repaired chain signatures

All arguments named after * are keyword-only and required unless explicitly noted.
Python missing-argument TypeError is a refusal, never a fallback. whole_program's
closed_input converts malformed content to Rejection; reason/provenance are
closed labels, never raw payloads.

    validate_compilation(raw, depfile, *, manifest_raw, data, observed_binary,
                         observed_dependencies, reference, phase_binding_raw)
    validate_inventory(raw, *, full_roster, selected_roster, case_map, data, reference,
                       phase_binding_raw)
    validate_admission(compiler_raw, listing_raw, admission, *, manifest_raw, data,
                       compilation, inventory, reference, phase_binding_raw)
    validate_pre_witness(compiler_raw, listing_raw, admission, pre, *, manifest_raw,
                         data, compilation, inventory, reference, phase_binding_raw)
    validate_completion(raw, capture, *, provenance, observed_binary, observed_sources,
                        observed_depfile_sha256, observed_dependencies, data,
                        manifest_raw, reference, phase_binding_raw)

phase_binding_raw is required by all nine phase-dependent pure public APIs:
validate_phase_data, commands, validate_inventory, validate_compilation,
validate_admission, validate_pre_witness, validate_completion, validate_cargo_tests,
validate_gate. Every internal phase-validation/admission call forwards it unchanged.
There is no default and no caller-supplied digest substituted for its preimage.

The corresponding reference.operator.phase_bindings[phase] must be BOUND.
Its sha256 binds the exact raw phase-binding FILE bytes, not encoded(data).
The raw input must be nonempty bytes of at most contract.MAX_JSON (2MiB), hash
exactly to that pin, and strict-parse as duplicate-free UTF-8 JSON without floats,
nonfinite constants or invalid JSON. The retained phase-b.phase-binding.v2
envelope has exactly these keys: schema_version, phase, authority_pins, cwd,
environment, python, immutable_files, tmp, selection_inputs, cargo, rustfmt,
data, step_zero_acceptance, source_freeze_review, operator_phase_attestation.
Its version and phase must match. The data member must match the supplied data
recursively by exact JSON type and value: bool/int, tuple/list and other Python
type substitutions do not alias.

Separately, reference.phase_data_sha256[phase] pins encoded(data), the canonical
representation defined above. Raw envelope bytes, parsed data and canonical data
bytes are three distinct subjects. Whitespace/key ordering in valid raw JSON may
differ from canonical serialization, provided the independently bound raw hash
matches and the typed data/canonical hash joins hold. No reserialization is used
to guess the raw FILE hash. Missing/UNBOUND phase, absent raw input, unknown version,
wrong raw bytes, wrong canonical expectation or typed data drift refuses.

This is the inherited envelope-shape/data join only. The observation/dispatch
adapter still owns observation of the pinned path and full interpretation of the
other envelope members: tool/environment/resource policy, external authority and
execution eligibility. Pure binding success proves no actual file was opened.

Raw arguments are retained bytes, maps are supplied plain dicts. data must be
the exact independently pinned phase data. manifest_raw must be the actual raw
phase-b.source-manifest.v1 bytes whose digest equals data.manifest_sha256 and
data.source_manifest.sha256. It is parsed by the duplicate-key/noninteger/UTF-8
rejecting contract parser, not reserialized to guess the raw manifest hash.
Its exact keys remain schema_version, subject_head, source_root, files.

Compilation preserves required source membership, measured Cargo JSON framing,
binary path/identity, exact depfile and dependency observations. It returns
pbdep.repaired-compilation.v3 with prior v2 fields plus reference_sha256.

Admission validates binary(compilation.binary) and the lowercase SHA256 type of
compilation.depfile_sha256 before any success, then the original repaired receipt
fields and capture ordering. Supplying the same malformed values in compilation
and admission cannot substitute for those standalone checks.
Its input schema is pbdep.repaired-admission.v3, with prior v2 fields plus
reference_sha256; it returns scope repaired-admission-joins-only and the
admission/reference hashes. It constructs no receipt and dispatches nothing.

Pre-witness input schema is pbdep.repaired-pre-witness.v3; prior v2 fields plus
reference_sha256. Output is pbdep.repaired-provenance.v3, scope
repaired-pre-dispatch-joins-only, with prior joins plus reference_sha256.
Compiler/listing/admission/pre bytes, exact source/binary/depfile identities and
monotonic ordering must agree. Completion revalidates the independently pinned
phase and raw manifest, the supplied dependency observation, reference/data/source
joins, post identity and capture. Output pbdep.repaired-completion.v3 retains
repaired-post-witness-custody-only scope and the same added reference join.
All repaired v2 inputs refuse at this v3 boundary; there is no migration fallback.

The inherited dependency schemas remain phase-b.dep-binding.v1 and
phase-b.dep-observation.v1. Compilation/admission/pre/provenance/completion field
sets are closed by keys() in whole_program.py. Capture still requires physical
PROVEN_ECHILD, EOF, subreaper confirmation, correct actual exit, no discarded bytes,
no metadata overflow, no errors/stop and ordered monotonic instants. A supplied
capture record does not prove any of those observations happened.

## Other pure APIs and retained semantics

validate_phase_data(phase, data, *, reference, phase_binding_raw);
commands(phase, data, tools, *, reference, phase_binding_raw, executable=None);
validate_cargo_tests(raw, *, data, exit_code, reference, phase_binding_raw);
validate_gate(raw, *, data, exit_code, reference, phase_binding_raw);
validate_operator(value, *, reference); binding_pin(operator, phase, *, reference);
validate_installed_tool(record, *, file_class, reference).
All require the same reference; the four phase-dependent APIs above also require
the same raw binding bytes.
commands only constructs closed argv tuples; executable=None retains its previous
compile-only/other-phase meaning and never chooses a binary or runs it.

validate_continuation(previous, current, completed, *, previous_reference,
reference) validates each independently pinned operator, preserves stable operator
fields except phase_bindings, completed bound-phase identities, phase-data pins
and installed-tool expectations. It grants no phase extension or retry.

validate_dependency_bindings(tokens, schema, bindings, *, sources=None),
validate_dependency_artifact(raw, *, source_root, executable, dep_tokens,
dep_binding_schema, dep_bindings, source_hashes), and
validate_dependency_observation(value, *, data, sources) retain R2 signatures.
sources=None deliberately means supplied-map shape only, never source membership.
validate_source_inventory, validate_selection_inputs, validate_selected and
validate_format retain their narrow R2 scope and APIs; they do not authenticate
expectations. Caller must separately pin selection/listing/oracle evidence.

Ordered raw compiler tokens remain untouched. The token-local stack rejects
an attempted root escape immediately, including escape/re-entry; raw duplicates
and canonical aliases refuse. Only this compiler-token grammar admits a legitimate
parent spelling such as crates/beacond/src/../sample-project/Cargo.lock.
General path()/_relative() stay normalized and strict. Exact two depfile rules,
phony rules and the two measured environment-dependency lines are conserved,
including package version0.1.0. Unsupported continuations or raw normalization
forgeries refuse; includes in Rust/product source are never rewritten.

Phases remain qualification,a2,format,base,candidate,integration,consumers,gate.
The31 repaired semantic groups, prefixes, consumer target roster, closed gate
stages, command flags, byte/numeric limits, capture predicates and strict result
framing are retained. This is a finite R2 evidence contract, not a promise that
new Rust, gate, desktop or roster grammars are supported.

## Explicit reservation contract

policy accepts a required reservation in every admission/clock predicate:
validate_reservation(reservation);
deadline_from_anchor(wall_ns, mono_ns, *, reservation);
remaining_ns(*, anchor_wall_ns, anchor_mono_ns, now_wall_ns, now_mono_ns, reservation);
phase_reservation(phase, completed, *, reservation);
admit_phase(phase, completed, *, remaining_master_ns, reservation);
admit_slot(phase, slot, spent, *, elapsed_phase_ns, remaining_master_ns, prep_used_ns,
reservation);
close_phase(phase, *, elapsed_phase_ns, spent, prep_used_ns, close_used_ns, reservation).

Exact reservation keys: schema_version=pbdep.reservation.v1, program_sha256,
master_start_utc_seconds, master_expiry_utc_seconds, exit_seconds, slots, phases.
Start/expiry are positive Python ints, strictly ordered, maximum window36h.
Program digest must identify the externally verified current program; the caller
binds its reference and boot/clock domain to that program before supplying numbers.
No function reads a live clock or supplies an old anchor. Every clock argument is
an explicit nonnegative integer. The retained2s wall/monotonic relationship tolerance
never adds runtime. Both clocks constrain expiry.

Slots retain exact labels1..13 and ceiling seconds600,30,180,600,30,180,600,30,180,
180,600,1800,90. All actual slot seconds are required positive integers at or below
these ceilings. exit_seconds must be60. Phases retain exact slot tuples and
prep/close/total ceilings from PHASE_CEILINGS. Each nonqualification total must
equal sum(slot+exit)+prep+close; qualification total is positive and at most600.
No default allocations, omitted usage, unknown fields, phases or versions pass.
close_phase requires explicit preparation/closure usage and enforces the declared
prep/close ceilings for every phase, including qualification's zero ceilings.
There is no qualification exception. Slot labels and
arithmetic do not issue slots. Old spent slots or clocks are unusable.

## Version-scoped original oracle

contract retains the original five-case A2 independent content/byte/timestamp,
518/68/5 roster, panic-line, result and exit101 expectations only under the
explicit required evidence_contract=phase-b.original-a2-evidence.v1.
These constants are fixture evidence, not wall clocks or generalized acceptance.

validate_listing adds required evidence_contract.
classify_a2 adds required evidence_contract and historical_prefix (tuple of three
independently pinned literal header lines for historical-cargo-A1, empty tuple for
direct framing). Removed private historical paths are not active defaults.

validate_provenance and validate_witness_completion add required reference:
exact keys schema_version=pbdep.original-reference.v2, evidence_contract above,
binding_sha256, subject_head, authority_sha256, bound_packet_sha256,
acceptance_sha256, review_sha256, source_manifest_sha256,
required_source_pins. The required source map retains exactly six original module/
lock names but all expected digests and subject/authority values are now supplied
independently. There is no old-head or authority fallback. The legacy _depfile
uses the repaired stack grammar too; general path grammar is unchanged. This
legacy chain is not the repaired v3 API and cannot stand in for its observations.
The prior internal acceptance key has been replaced by acceptance_sha256 in every
original-chain reference/binding/admission field set and join. Affected schemas
are pbdep.original-reference.v2, pbdep.provenance-binding.v2 and
pbdep.pre-witness-admission.v2. Old fields/versions are rejected, with no alias.
The version-scoped synthetic vectors, fixture timestamps, content and roster
hashes are retained exactly; they are not classified as secrets or relocated.

## Conservation and later owners

Portable evidence-bundle member references:
predecessor/instrument/{whole_program,policy,contract}.py;
predecessor/RETURN.md and predecessor/RETURN-SEALED.sha256;
design/PRODUCT-SUCCESSOR-JOIN.md sections1–3;
design/CONTROL-GATE-MAP.md (354 predecessor IDs + D01–D32,
386 IDs/390 QD observations; five supplemental RD observations).
These are logical, caller-resolved members of an independently pinned evidence
bundle, not installed paths or implicit private report locations. This package
performs no lookup or fallback resolution.

Parent-spelling positive, retained raw replay, raw reorder/missing/extra,
normalization forgery, duplicate raw, canonical alias, escape/re-entry, absolute/
unsupported token, map/schema/type/size/digest/source drift and complete-chain
controls remain obligations. No old PASS transfers to these changed sources.
Independent evidence review maps cases; the control owner owns the port and required-input
negative coverage; no controls or runtime stubs are included here.

The observation adapter owns actual filesystem observation, nofollow walks including cancelled
components, identities, source/binary stability, EOF/descendant/resource evidence.
The discovery adapter owns discovery and dispatch entry plus independently reconciled freeze.
The binding owner owns the complete frozen source/control/reference/command binding and external
review/admission. New qualification, native execution and product acceptance remain
separate, UNRUN. No native feature, migration, entry stub, approval, landing or
push follows from this interface. Adoption and execution need separate external approval.
