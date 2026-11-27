# B04 discovery-to-freeze interface v2 — native prerequisite consumer

This source candidate includes the accepted Q/A2 read-only consumer design and
the chronology/custody repairs. Exact-source independent review/disposition is
pending. No producer, oracle instance, execution permission or runtime readiness
is supplied or inferred.

Source preparation only. All behavior, imports, controls and actual custody are
UNRUN. These modules issue no permission, reservation, human approval or product
PASS. An independently authenticated trusted local caller supplies admission;
hash/shape consistency (C8) is integrity only, never authentication. There is no
new cryptographic root, signature verifier, provider, dynamic loader or arbitrary
authority callback.

## Entry and effects

The sole supported entry is:

    entry.dispatch(mode, **independently_authenticated_pins)

Mode is exactly `unscored-discovery` or `scored-witness`. Unknown mode refuses
before reading clocks/files or launching. Argument names are exact, no defaults:

| Mode | Independently authenticated pin arguments |
|---|---|
| unscored-discovery | request_pin, grant_pin, reference_pin, reservation_pin, prior_pin |
| scored-witness | the same five, plus freeze_pin |

A pin is the accepted exact {path,sha256} shape, absolute normalized path and
lowercase SHA256. The caller must authenticate the genuine external admission,
its intended subject/effects/instrument, current program lineage, full prior
phase/attempt inventory and independently reviewed freeze *before calling*.
A boolean, issuer string, candidate-derived expected hash, or successful return
cannot fulfill this obligation. The code verifies the received independent
pins against actual raw files and rejects missing/empty admission inputs.
No active pins/allocations/clock or admission provider is supplied here.

Requests and the reservation must use runtime.encode's exact canonical JSON
(sorted keys, compact, retained ASCII escaping, no NaN, trailing LF). Reference,
prior and freeze bytes may have other valid strict JSON whitespace/order; their
raw FILE pins remain distinct from canonical record hashes. The raw freeze and
phase-envelope bytes are re-read, pinned and retained unchanged in witness.
No serialization is used to guess their expected raw-file digest.

Private entry helpers are orchestration, not additional permission surfaces.
There is no CLI, ambient argv/environment selection, shell command, fallback
root, inherited grant, import-time observation or executable module loader.
A trusted caller imports an independently reviewed pinned package through its
own admitted bootstrap; B04 does not authenticate arbitrary already-loaded code.

Only the following children are authored, never invoked by preparation:

- compile: (cargo, test, --locked, --offline, -p, beacond, --lib, --no-run,
  --message-format=json);
- list: (actually observed executable, --list, --format, terse);
- witness: (same independently frozen executable, accepted B02 PREFIX,
  --nocapture, --test-threads=1).

The executable must have the accepted measured Cargo grammar and
target_root/debug/deps/beacond-[16 lowercase hex] identity. Discovery uses
contract._compiler only as structural artifact decoding, not as an oracle.
B02 commands with required reference/raw binding must reproduce compile/list
and the single witness tuple before witness launch. No model-built command is
accepted. Compilation may execute compiler/build-script descendants and listing
executes the binary; these are trusted-local admitted effects, not containment.

## Closed request and evidence shapes

Every record below has *exactly* the named keys. Schema strings are literals.
All control JSON is strict duplicate-free UTF-8, no floats/nonfinite values,
at most2MiB. Raw supervisor JSON alone retains finite supervisor floats and is
bounded128KiB, then checked by the accepted normalize_capture predicate.
The literal key sets in entry.py/discovery.py and accepted B02 validators are
the machine contract; no extension keys or version fallback.

`pbdep.discovery-request.v1`:
schema_version, chain_id, grant, reference, reservation, prior, subject_head,
source_root, target_root, source_manifest, source_directory, target_directory,
tmp, state_directory, environment, python, cargo, rustfmt, immutable_files,
selection_inputs, module_paths, raw_cap.

chain_id is64hex; subject_head40hex. Source/target roots are distinct and
nonoverlapping. Directory identities are the accepted exact path/dev/ino/uid/mode
records (numeric decimal strings, current uid,0700, no symlink ancestors).
Source, target and temporary identities must be registered in the B02
reference.operator resource roots; state is separate and nonoverlapping.
tmp is a registered temporary root of the inherited short-path class.
module_paths exactly covers reference.operator.instrument_sha256.
All authority/instrument/tool/closure/selector pins refresh before each capture.
Immutable installed multicall exceptions remain confined to B02's literal four
roles; no wider hardlink exception. The source must be a disposable worktree
with a regular single-link .git marker in the immutable closure. A .git directory
is unsupported. The bounded source inventory excludes only source_root/target.

Environment uses a closed profile chosen by the consumer's phase, never by
evidence: qualification/format have exactly13 keys; a2/base/candidate/integration/
consumers/gate and B04 discovery/witness add only CARGO_TARGET_DIR (14 keys).
The common13 are CARGO_BUILD_JOBS,CARGO_HOME,CARGO_NET_OFFLINE,CARGO_TERM_COLOR,
HOME,LANG,LC_ALL,PATH,PYTHONDONTWRITEBYTECODE,PYTHONNOUSERSITE,RUSTUP_HOME,
RUST_TEST_THREADS,TMPDIR. The eight fixed literals are CARGO_BUILD_JOBS=2,
CARGO_NET_OFFLINE=true,CARGO_TERM_COLOR=never,LANG=C.UTF-8,LC_ALL=C,
PYTHONDONTWRITEBYTECODE=1,PYTHONNOUSERSITE=1,RUST_TEST_THREADS=1.
All values are strings, NUL-free and at most8192 UTF-8 bytes. HOME,CARGO_HOME,
RUSTUP_HOME,TMPDIR and any target use the existing pure W.path grammar:
normalized absolute non-root ASCII, no empty/dot/dotdot components, at most1024
characters. PATH has1..32 ordered distinct components, each W.path-valid, total
at most8192bytes. There is no normalization, ambient lookup or environment merge.
Missing/extra keys (including arbitrary credential names) and unknown phases refuse.

The full map equals its independently authenticated binding and corresponding
before/after observations and Q input. TMPDIR equals that phase's bound tmp.path.
Qualification/format omit target. A2 target equals native provenance target_root;
later Cargo phases use binding.data.target_root and registered roots; B04 uses
request.target_root plus the witness phase-binding equality. No later B04 target
is borrowed for prior A2. Existing source/target disjointness, directory identity,
temporary/resource-root, pinned tool/selector and timing checks still apply.

cleared_environment(value,binding,*,phase) is shared by initial check_request,
Custody.refresh and prerequisite binding/observation/Q-input consumption; witness
adoption checks its phase binding explicitly too. check_request refuses before
Custody construction or any B04 LOCK/stage/control/START write; refresh repeats
the check before the first write and later dispatch. Refusals do not echo values.
Already retained upstream bytes may be read for refusal; B04 cannot undo a
producer's earlier retention.

Only independently reviewed non-secret values may enter these maps. Future
producers must enforce that boundary before serializing bindings, observations,
Q input or captures. Key allowlisting and path-shaped strings establish neither
secrecy nor provenance; safe values and upstream pre-retention producer handling
remain UNBOUND B06 prerequisites. No secret scanning, secret-store integration,
new producer, tool lookup or root role is added. A genuine need for another key
stops for a separately reviewed profile. Python/cargo/rustfmt remain explicit
executable paths in the pinned closure. This is trusted-local, not hermetic.

`pbdep.witness-request.v1`:
schema_version, chain_id, grant, reference, reservation, prior,
discovery_request, discovery_result, freeze, phase_binding, phase.
All reference-like fields are raw-file pins; phase is base or candidate.
The required external pin arguments must equal their corresponding fields.

`pbdep.b04-freeze.v1`:
schema_version, chain_id, program_sha256, reservation, prior, discovery_request,
discovery_result, discovery_close, state_directory, retained_inventory,
resources, expected.

expected has exactly source_snapshot, binary, depfile, dependency_data,
dependency_observation, ordered_roster, case_map, environment_sha256,
immutable_files_sha256, terminal_captures. The first six equal the independently
reviewed discovery proposal; case_map is independently authored and must equal
the complete accepted B0231-group map, not a map generated by discovery.
The two hashes bind the original environment/tool closure. terminal_captures
equals the actual compile/list artifact sets, including START/RAW/log/terminal.
The entire freeze is an *external received expectation*, with no approval flag.

A source_snapshot has files (all full ordinary file-observation records),
directories (all even empty directory identities), and git (full .git record).
The independently pinned source-manifest is phase-b.source-manifest.v1, the
accepted source/hash map with actual raw digest, source root and subject head.
Source snapshots before compilation, around listing and before/after witness
must agree. No source edit is performed by B04.

Discovery proposes ordered raw dep_tokens and canonical in-root dep_bindings
using the accepted token grammar and exact measured depfile grammar.
dependency_data has source_root, subject_head, manifest_sha256, dep_tokens,
dep_binding_schema, dep_bindings; all values come from actual retained bytes and
pinned source membership, never fake later-phase defaults. Its component-only
pure dependency validators do not require a phase oracle. The B04 walk preserves
the accepted descriptor-relative O_NOFOLLOW traversal, held-directory/root/file
stability, cancelled-component checks, verified-missing-component provenance,
65,536-component/5s bound and ordered raw/canonical distinction. Actual dependency
files/directories must also match the complete precompile source snapshot.
The witness uses accepted runtime.observe_dependencies with the mandatory B02
reference and phase_binding_raw and matches that frozen observation.

The complete listing is structurally decoded to sorted unique measured test
names (at most20,000), preserving its exact required ordering. Discovery never
selects expected cases, asserts a semantic case result, or issues approval.

## One reservation, one master, three distinct attempts

`pbdep.b04-reservation.v1`:
schema_version, program_sha256, chain_id, allocation_ids, anchor,
master_start_utc_seconds, master_expiry_utc_seconds, exit_seconds, discovery,
witness, review_seconds, return_seconds, later_seconds.

allocation_ids has exact compile/list/witness keys, three distinct64hex
identities. They are explicit current semantic attempt identities, never aliases
for the historical13 policy slots. anchor is exact wall_ns/monotonic_ns/boot_id;
the two clock values use accepted canonical decimal ns strings. Program and
chain identities are64hex. All second-valued budgets are exact Python/JSON ints,
not booleans. Start/expiry are positive, bounded10^12, strictly ordered with
window at most36h. exit_seconds is60.

discovery has prep, close, total, compile, list; witness has prep, close, total,
witness. Each prep/close is1..150s; useful compile/list/witness are respectively
1..600/1..30/1..180s. Each stage total equals prep+close+sum(useful+60).
review_seconds is1..36h; return_seconds60..600; later_seconds0..36h is the
required explicit larger-program reserve. Sum of both stages, review, return
and later must fit the master window. These are source ceilings, no allocation.

Discovery and witness must use the *same exact reservation pin/raw bytes*,
program/chain, original anchor/expiry and state identity. A replacement reserve,
extension, root rollover or reset is unsupported. Boot identity, monotonic
nonregression and wall/monotonic relationship (at most2s difference) are checked
against that anchor. Both wall and anchored monotonic expiry constrain remaining
time; review never pauses it. Review wait is also bounded from the observed
pre-CLOSE-write instant. Expired review is terminal refusal for this lineage;
retrying the same immutable reservation cannot revive it.

Actual stage setup/inter-slot work is measured from entry, child wrappers are
measured separately, and closure has its own measured/reserved window. Live
admission checks occur before START and again after its persistence and all slow
fresh stability checks, then a second complete retained-inventory observation
including the newly persisted START, immediately before final clock admission and
supervisor invocation. This detects trusted-local drift, not hostile races. No fabricated
zero prep/close accounting. Resource callbacks subtract the ongoing child
interval from setup accounting, while master/stage/return limits keep running.
A slow final CLOSE write invalidates via STOP after its fsync; the recorded
close_ns_before_write is explicitly a lower observation, not an exact final
completion time or fork timestamp.

## Prior history and retained-root conservation

`pbdep.b04-prior.v2`:
schema_version, kind, program_sha256, chain_id, reservation_sha256,
state_directory, resources, terminal_captures, inventory, phase_history.

This is an independently authenticated complete predecessor inventory.
GENESIS requires no prior captures/phases/resources/files/subdirectories and
cannot be used to discard existing state. TERMINAL requires complete pinned
physical captures; UNKNOWN/nonterminal physical evidence refuses. Genesis
discovery alone does not bypass the outer phase prerequisites for a witness.

The internal read-only prior_history API requires observed_entry_ns explicitly:
initial discovery supplies its actual entry observation; witness adoption supplies
the independently frozen discovery stage-start observation, after checking that
the complete discovery interval precedes actual witness entry. There is no
missing-time default and no new clock anchor. Historical phases, START/capture
intervals and observed boundaries must lie between anchor.monotonic_ns and
anchor.monotonic_ns + expiry_ns - anchor.wall_ns. Prior phases cannot overlap,
their captures follow the declared phase/slot order without overlap, and every
prior close must precede discovery entry. Full adoption order is:
discovery.start <= compile.START <= compile.start <= compile.end <= list.START
<= list.start <= list.end <= discovery.close <= witness.entry.
All existing local duration, reservation, resource and UNKNOWN checks still apply.
These chronology checks remain separate from the native semantic evidence checks below.

phase_history is at most8 ordered {phase,record} pins forming an exact prefix of
the accepted eight-phase vocabulary. The record is
`pbdep.b04-external-phase.v2` with schema_version, phase, program_sha256,
reservation_sha256, master_anchor, master_expiry_utc_seconds, state_directory,
predecessor_sha256, status, artifacts, resources, policy_reservation,
phase_started_monotonic_ns, phase_finished_monotonic_ns, elapsed_ns,
prep_used_ns, close_used_ns, child_wrapper_ns, spent, reference, phase_binding, semantic_evidence.
reference/phase_binding are actual pinned accepted predecessor inputs in the
retained root. Every prior START joins their canonical reference/raw binding
hashes; completed-phase continuation preserves those pins through discovery and
witness without rewriting completed phases. status is TERMINAL; predecessor links the previous record raw digest or program
digest for the first phase. Policy-reservation raw JSON uses accepted B02 shape
with JSON slot lists explicitly converted to tuples only for the pure policy
API. Its program/start/expiry must match the same master. Exact elapsed/child/
prep/close arithmetic and policy.close_phase are mandatory, including
qualification's zero prep/close ceilings. No executor/provider for qualification,
original A2 or larger scheduling is implemented here. Their authentic observed
records and oracle qualification are external prerequisites, not invented data.

terminal_captures is at most16 complete {start,raw,log} pin sets in the single
root, using native pbdep.prior-slot-start.v2 and the accepted B03 RAW transport. All prior phase slots are required
exactly once (qualificationQ, otherwise the accepted phase tuple), with actual
useful/exit bounds and ordered in-phase wrapper intervals. Original A2 slot3
expects exit101; other prior slots expect0. normalize_capture must prove retained
bytes, ECHILD, EOF, subreaper, correct leader exit, no truncation/error/stop.
Wrapper sums must equal each actual phase record's child_wrapper_ns. Every
capture pin belongs to that phase's artifact inventory; no missing child is
approved by a typed TERMINAL string. The external trusted caller authenticates the complete finite phase/child roster.
The consumer also checks its native semantic evidence, exact commands and raw
results; physical exit or the TERMINAL label alone cannot admit it.
Witness phase must be the next accepted phase after this prefix.

inventory/retained_inventory has exact state_directory, files, directories,
total_bytes. All retained ordinary files are fully observed by relative path;
all directories have private exact identities. No symlink, special or hardlinked
state member is admitted. LOCK is the sole excluded zero-byte regular0600 file,
and its identity/mode/uid are checked under flock. Root checks and byte scans
cover the entire one root (at most4096 entries/16MiB pathname bytes/5s).
All prior raw logs, START/terminal/control records remain present and hash-bound.
Extra/missing/changed files or directory identities refuse. Before *every*
later START, the complete observed inventory must equal the accumulated ledger.
Witness also revalidates original prior captures/accounting and frozen discovery
captures, never injects ObservationSession fields or counterfeits its STARTs.

resources is the accepted cumulative record: sample_count, peak_total_bytes,
peak_temporary_bytes, peak_capture_bytes, vanished_by_class, vanished,
sampled_entry_types. Counts are strict bounded ints; sample_count<=8192 and
combined disappearance entries<=128. Every entry retains class and path hash;
class counts must equal the actual list. Peaks obey inherited96GiB/8GiB/60MiB
bounds. Sampled type counts are bounded and nondecreasing. Each phase/raw/
terminal/freeze join preserves all earlier peaks, prefix disappearances and
counts. Adoption seeds from the verified discovery terminal history; no new
object, current small scan, or review boundary erases it. Last actual scan is
separately retained. No sampled resource record is reported as a hard quota
or aggregate terminal PASS.

## Output and failure custody

Discovery result: `pbdep.discovery-result.v1`, status UNSCORED; exact keys
schema_version,status,chain_id,program_sha256,reservation,prior,request,reference,
grant,state_directory,captures,retained_depfile,proposed,resources, plus both axes
below. proposed has the six discovery-only expected fields described above,
without case_map or expected success. Return value is the raw RESULT.json pin.

Witness result: `pbdep.witness-result.v1`, scope frozen-witness-evidence-only;
schema_version,scope,chain_id,program_sha256,request,freeze,reference,phase_binding,
reservation,prior,discovery_result,capture,completion,selected,resources, plus
both axes. Completion is the accepted repaired-post-witness-custody-only result;
selected is its separate31-case assertion framing result. Neither is a product
or whole-program acceptance. Return value is the raw RESULT.json pin.

The finite new directories are state/discovery and state/witness, created once.
Per attempt: attempt-START.json, attempt.log, attempt-RAW.json,
attempt-NORMALIZED.json, attempt-TERMINAL.json. Discovery additionally retains
OPERATOR.json (actual request), DEPFILE.d (actual bytes before depfile parsing),
RESULT.json and CLOSE.json. Witness additionally retains OPERATOR.json (actual
freeze raw bytes), PHASE-BINDING.json (actual raw envelope), ADMISSION.json,
PRE-WITNESS.json, RESULT.json and CLOSE.json. Shared LOCK/STOP.json are the only
program-level outputs. All writes are create-only0600, under private directories,
with fsync, parent sync and readback. There is no retry after partial START/fork.

B04 START has schema_version=pbdep.b04-start.v1, attempt, chain_id, program_sha256,
allocation_id,reservation_sha256,request_sha256,argv,cwd,environment_sha256,raw_cap,
useful_seconds,exit_seconds,recorded_monotonic_ns,slot_spent_even_if_fork_fails=True.
RAW has schema_version=pbdep.b04-raw.v1, attempt, chain_id, program_sha256,
start_sha256, wrapper_before_run_monotonic_ns,wrapper_after_run_monotonic_ns,
stream=merged-stdout-stderr,boundary_is_not_exact_fork=True,supervisor,
resource_samples,last_sampled_resources, plus axes.
TERMINAL has schema_version=pbdep.b04-terminal.v1,attempt,allocation_id,capture,
artifacts={start,raw,log},resources, plus axes. Result capture sets add its
independently pinned terminal file; no self-hash field.
CLOSE has schema_version=pbdep.b04-close.v1,stage,chain_id,result,spent,
stage_started_monotonic_ns,observed_before_close_write_monotonic_ns,prep_ns,
child_wrapper_ns,close_ns_before_write,close_write_is_inside_reserved_close_window=True.

Caps are never enlarged: cumulative64MiB root with4MiB terminal reserve,
at most16MiB per merged log (request may require less), ordinary receipts128KiB,
the inherited OPERATOR/PHASE-BINDING/RESULT data class2MiB, DEPFILE.d512KiB,
source files16MiB. Overlarge joins/receipts refuse; no trimming or continuation
after discarded bytes. Existing runtime persistence and supervisor caps apply.

Complete merged stdout/stderr and raw supervisor receipt persist before any
normalization or compiler/listing/scoring parser. A raw failure remains evidence.
manager_status=NOT_OBSERVED_BY_ADAPTER and aggregate_resource_status=UNKNOWN
remain literal separate axes; physical descendant ECHILD, leader exit and pipe
EOF are independently verified. Unknown custody never becomes PASS.
Admitted operation failure creates STOP_NO_RETRY with SEE_RAW_OR_UNKNOWN and
retained history. Terminal write failure reports b04_terminal_unavailable.
Pre-admission/replay failures may have no new receipt; they never launch.

## Mapping and later gates

Accepted runtime public primitives supply pins, directories, exact observations,
source/resource scans, persistence, capture budgets, normalization and witnessed
dependency observation. Accepted supervisor supplies bounded subreaper/pipe/
descendant capture. B04 adds only named orchestration, state inventory, master/
stage accounting, freeze adoption and the pre-oracle dependency walk. B02 pure
compilation/inventory/admission/pre-witness/completion/selected/commands joins
receive every required reference/phase_binding_raw. ObservationSession and all
22 predecessor bodies remain unchanged.

The retained R2 instrument/entry.py filename was a helm typo: no such input is
required. R2 runtime.py Program/execute and earlier Phase-B r2/instrument/runner.py
are read-only provenance. No fixed historical root, authority, master, loader
body or old grant was imported. Generic names are used throughout.
The original oracle and synthetic fixtures remain untouched.

B05 independently owns positive/negative controls. B06 must supply reviewed
concrete values, actual current permissions/provider context, independently
qualified original-A2/qualification records, later scheduling/reserves and a
compatible source/roster/depfile subject. This code supplies no such records.
Unsupported oracle grammar, root layout, custody or missing prerequisite refuses.
All tests/imports/launches remain UNRUN; freezeON, T3CLOSED.

## Native prior semantic evidence: no v1 relabeling

Only prior.v2, external-phase.v2 and prior-slot-start.v2 are admitted at this
boundary. Old v1 files are unsupported; nothing rewrites versions/acceptance
keys or manufactures a timestamp. Existing B04 request/freeze pins now name
these native prerequisite records. All current chronology, actual retained-root,
master/reservation/phase usage, physical exit/EOF, resource and UNKNOWN gates
remain mandatory. This consumer reads retained evidence; it never starts a
qualification/A2/later child, produces evidence, or grants authority.

External-phase.v2 adds semantic_evidence (a retained pin). Its exact object is
pbdep.prior-semantic.v1 with schema_version,phase,program_sha256,binding_sha256,
before,after,roles. before/after pin pbdep.prior-observation.v1 records with exact
schema_version,program_sha256,phase,binding_sha256,observed_ns,cwd,environment,
tools,sources,binary,depfile_sha256. sources is a nonempty finite map of original
source name to retained raw-source pin; hashes are computed from those bytes,
never accepted as self-consistent labels. Format source names are absolute
root/relative names; other names are normalized relative names. tools has exactly
python,cargo,rustfmt full actual metadata records, matching binding immutable
closure and fresh read-only file observations. binary is null for Q/format/
integration/consumers/gate; before compilation it is null for A2/base/candidate,
whose after observation contains the exact measured six-field binary identity
and depfile digest. Actual current immutable binary/depfile identities are
reobserved; missing or changed retained inputs refuse.

Q source observations bracket its controls inside the outer Q capture; A2/later
observations bracket their phase captures. Times join to the recorded phase and
the unchanged anchored chronology. Qualification prep/close ceilings stay zero:
the consumer accepts only genuine observed policy-compatible accounting and
never writes zero to make an input fit. Actual compatible producer/wrapper
availability is a B06 prerequisite, not an assertion here.

Native prior-slot-start.v2 has all prior B03 START fields (schema_version,phase,
slot,argv,cwd,reference_sha256,binding_sha256,useful_seconds,exit_seconds,raw_cap,
recorded_monotonic_ns,slot_spent_even_if_fork_fails) plus environment_sha256,
tools_sha256,sources_sha256. The extra digests bind the exact cleared environment,
before.tools and computed before-source hash map through runtime.encode. START
command/cwd/digests and physical raw capture are checked before result predicates.
Ordinary B03 RAW transport version stays pbdep.raw-custody.v1; its contents must
satisfy the current closed supervisor schema and float scope, not merely its tag.

Later phase raw envelopes remain the accepted phase-b.phase-binding.v2 and pass
runtime.validate_binding_links plus B02's required reference/phase_binding_raw.
Q/A2 instead require pbdep.prerequisite-phase-binding.v1 with the same exact15
envelope keys as the accepted envelope, but a separate closed data profile:
- Q data: profile=pbdep.current-qualification-consumer.v1,oracle,input (two pins).
- A2 data: profile=pbdep.original-a2-consumer.v1,original_reference (a pin).

The raw envelope is pinned by the independently received current reference.
Authority/packet/acceptance/review/disposition, attestation, source review, tools,
cleared env, cwd, temporary/resource roots and selectors are actually re-read
and joined. No fake later-phase data is supplied for Q/A2. Independent expected
pins are known before predictions; the later semantic descriptor/results are
pinned by the external phase record. The raw binding never contains the hash
of its own later semantic descriptor or results.

### Fixed Q input, closures and registry

The authoritative predicate semantics are Q-PROTOCOL-R1 (1f56502f), accepted
under the current delegated source-design disposition. One profile and all eight
ordered families are mandatory: wrapper126, original-contract169, whole-program354,
policy272, runtime-custody9, PBDEP-QD32 IDs/36 rows, PBDEP-RD5, successor nonempty
at most1024. Total rows at most2048. Historical930 is the first five families;
first seven current families total971 rows. The386-ID/390-QD/five-RD partition
remains distinct:354 carried rows appear once, not twice. Successor IDs use the
separate SUC[3..6 decimal digits] namespace and cannot reuse historical IDs.

pbdep.q-input.v1 has exact schema_version,profile,program_sha256,
phase_binding_path,oracle,subject_source_closure,producer_source_closure,cwd,
environment,tools,fixture. Its raw pin and independent oracle pin are in the
raw Q envelope; phase_binding_path equals that envelope's bound path.
It intentionally contains the binding *path*, not its digest: input and binding
therefore have no circular hash dependency. Results later bind the actual raw
binding digest, START and RAW receipt. No result or producer selects the oracle.

Oracle/results exactly implement pbdep.q-oracle.v1 and pbdep.q-results.v1 key
sets from the accepted protocol. Exact ordered family and every row ID/kind
identity are checked before any predicate; missing/extra/duplicate/unsupported/
UNBOUND members refuse. Each row binds actual retained input bytes, oracle
fragment, raw results, independent map, source closures and fixed command.

Subject closure roles are exactly the current independently pinned instrument
basenames; archive basenames, raw source hashes, immutable-file metadata and
both source observations must agree with reference.operator.instrument_sha256.
Producer roles are exactly qualifier,wrapper-controls,original-contract-controls,
whole-program-controls,policy-controls,runtime-controls,pbdep-qd-controls,
pbdep-rd-controls,successor-controls,physical-predicates,expected-rows,
legacy-id-index,bootstrap-controls. All are actual retained raw pins and immutable
closure entries, not hash labels. The qualifier path must end with
scripts/qualification/pbdep/qualify_whole_program.py. The fixed three specification
sidecars require their accepted physical4f3b2dda, expected-rowsa710ec43 and legacy
index4a3dd544 hashes; their historical paths are never opened or defaulted.
Wrapper/whole-program ID vectors and original-contract ID membership are
conserved from those reviewed source inventories. Observed-only legacy policy
IDs are never used as a current expected oracle.

Each row's nonempty oracle_source fragment must be actual bytes present in its
family's pinned producer/control source. Assertion-return is restricted to
wrapper/original-contract rows and the exact fragment digest/none return; it
claims only that this independently mapped assertion returned normally, not
rederived values, exact refusal codes or branch sensitivity. EXCEPTION_CLASS
uses the fixed role enum and actual pinned module class declarations, parsed
as text AST without loading/importing them. Whole-program kinds retain typed
equality/closed rejection/exception-class strength; policy retains typed
equality/refusal text/exception class. Runtime's fixed nine rows use its physical
predicate. Successor rows cannot use the source-assertion shortcut.

All seven closed kinds are implemented: VALUE_EQUAL tagged none/bool/int/text/
bytes/list/tuple/map with type-preserving comparison; CLOSED_REJECTION fixed
whole_program.Rejection and literal uppercase-code grammar; EXCEPTION_CLASS
closed roles; REFUSAL_TEXT exact policy text; REFUSAL_TRIPLE independent outcome/
code/provenance/context; ASSERTION_RETURN as limited above; RUNTIME_CASE fixed
case predicates. Tagged values enforce depth12, combined expected/observed4096
nodes,1024 items,32-digit canonical ints,64KiB text/bytes and128KiB encoded rows.
Top oracle/results still reject every JSON float. Unsupported types never vanish.

D/QD and RD triples/row order are checked against independently supplied pinned
EXPECTED-ROWS bytes. D22.1 additionally requires the protocol's exact missing
component literals and a retained pbdep.q-parent-preimage.v1 with matching
program/binding/case/path/device/inode. Actual pre-observation time must precede
the checker call within outer Q capture. Its parent path joins the fixed
fixture-root role in pbdep.q-fixture-root.v1 (schema_version,role=D22,directory).
The fixture pin is in the independent row inputs; its directory identity is
actually reobserved. No inode or expected verdict is learned from the refusal.

The Q roles object is exactly input,oracle,results,fixture. Q command verification
is solely (bound_python,-B,-I,bound_qualifier,--evidence-input,bound_input).
No Q executor, callback, DSL, old runner import or producer is implemented.

### Native A2 and later semantic roles

A2 roles are exactly original_reference,provenance_binding,full_roster,pilot_roster,
overlay_roster,manifest,depfile,admission,pre,completion,result. Original reference
must be native pbdep.original-reference.v2; provenance binding native
pbdep.provenance-binding.v2; admission native pbdep.pre-witness-admission.v2.
The four accepted contract APIs validate listing, provenance, completion and
behavior from raw retained bytes. Authority/packet/acceptance/review/subject/
manifest/root/target/tool/env/source/binary/depfile and actual capture records
join them to the current native phase. Exactly three retained commands are
verified: fixed Cargo compile, measured executable list, then original
artifact::artifact_identity_contract:: witness prefix. Direct framing,518/68/5
literal rosters,513 filtered,exit101,2 RED/3 GREEN and empty historical prefix
are preserved. No old v1 conversion or historical-cargo-A1 admission occurs.

Later base/candidate roles: manifest,depfile,admission,pre,completion,full_roster,
selected_roster,result. They use B02 compilation/inventory/admission/pre-witness/
completion/selected APIs and accepted command derivation with required raw
binding/reference, actual source archive hashes and fresh nofollow dependency
observation. Prior result and completion roles must equal the separately derived
narrow results, not an aggregate PASS. Other later roles are exactly result;
raw source/format/gate dedicated pins and the accepted format/cargo/gate validators
plus commands apply. Formatter scope remains formatter-exit-only, as accepted;
no stronger Rust formatting proof is inferred.

### Exact physical parser and evidence bounds

The accepted repair2 corrigendum supplements the original protocol's incomplete
four-time-field/exception wording; no old source/receipt is relabeled or projected.
validate_supervisor_record checks native shape separately from positive custody.
All20 inherited top-level fields remain; normal and emergency reaped rows are
exactly {pid,status,exit_code,at}, signal rows {pid,signal,at}; every field is kept.
Error rows are exactly {where,type} or {where,type,pathname_class,errno}, with
the classified pair together. where is one of postfork_setup,reap,resources,
capture,signal,log_persist,outer_parent,emergency_cleanup,emergency_persist;
type is a class-name string. pathname_class is root-lstat,entry-stat or
directory-scan; errno is integer or null. Errors have no timestamp.
PID is positive int (leader_pid may be null); leader_returncode is int or null;
status/exit_code/signal are native ints without new ranges or decoding. Counts,
resource_peak_sampled,log_bytes,discarded_log_bytes are nonnegative bounded ints;
bool never aliases int. Callback-provided noninteger peaks/errno refuse without
coercion. Lists remain bounded128; event counters may exceed retained lengths on
overflow. Shape parsing does not require successful count equality.
whole_descendant_set_exit is PROVEN_ECHILD or UNKNOWN; pipe_eof and overflow are
bool, subreaper_verified is true, wait_options is WNOHANG|__WALL. stop is null,
useful_time_cap,instrument_error,metadata_overflow,log_cap_prefix_only or
descendant_exit_or_capture_unknown.

Only six path classes admit finite nonnegative int-or-float time values:
leader_exit_monotonic,echild_monotonic,pipe_eof_monotonic,elapsed_seconds,
reaped[i].at,signals[i].at; RAW wrappers prefix these with supervisor.
Only the first three top-level observation times may be null when unobserved.
Event times and elapsed_seconds are required numbers; bool/negative/nonfinite
times refuse. supervisor_float_scope rejects floats everywhere else, including
errors/status/PID/count/resource/errno and wrapper decimal-ns fields. Controls,
oracle/results and START ns still reject floats. Complete native failure bytes
remain parseable where appropriate; parsing UNKNOWN/null does not admit success.

Standalone strict_supervisor/physical_case, retained_capture, prior_capture and
the own-capture post-retention/pre-normalization seam use this same shape/scope.
The existing production normalizer still rejects null/UNKNOWN/unproved exit,
EOF loss, overflow/errors or wrong exit. Fixed nine-case physical predicates
retain their original leader/time requirements separately, including metadata0/
log32 intentional failures. Native status/event at are retained/type-checked,
not decoded, ordered or joined to exit status. No historical predicate is
strengthened; negative fixture success is not usable production custody.
PHYSICAL-CHECKS-SOURCE remains a pinned specification, never imported/executed.

The closed exception union now names runtime.ResourcePathError (declared in
runtime.py), replacing only the impossible supervise.ResourcePathError.
Source-closure/class-declaration verification remains; no arbitrary exception
fallback, dynamic import or class remap. Physical fixture TraversalError remains
a separate expected error string.

No inherited byte/resource ceiling increases. The consumer recognizes finite
new2MiB control-data names Q-ORACLE,Q-RESULTS,Q-INPUT,CONTROL-MAP,CONTROL-RESULTS,
PRIOR,OBSERVATIONS-BEFORE,OBSERVATIONS-AFTER,SEMANTIC,SOURCE-MANIFEST (.json).
Retained .py/.rs/.toml/.lock source copies are conservatively capped2MiB;
.txt/.hex/.blob copies128KiB; ordinary receipts remain128KiB, logs16MiB,
depfile512KiB, whole single-root64MiB with4MiB terminal reserve. These read-only
artifact classes create no output permission. CONTROL-RESULTS.json remains
a future documentary seam, requiring B06-bound actual paths/effects.

Current wrapper/original-contract/physical/bootstrap producer sources, the full
independent native oracle (including policy/successor rows), native result/
observation/START records, native-v2 A2 package, actual master/program/grant/
provider and output bindings remain UNBOUND. They are B06 prerequisites, not an
implicit expansion of B05's five source files. Source review can assess this
consumer contract; it cannot establish those producers, runtime facts or
execution readiness.
