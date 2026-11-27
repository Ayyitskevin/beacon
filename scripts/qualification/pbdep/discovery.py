"""Named B04 discovery custody primitives; no import-time effects.

Trusted callers authenticate pins before invoking entry. JSON joins are integrity
only. This is neither whole-program scheduling nor hostile containment.
"""
import ast
import errno
import fcntl
import hashlib
import json
import math
import re
import os
from pathlib import Path
import stat
import time
from copy import deepcopy

from . import contract as C, runtime as R, whole_program as W, policy as P, supervise

NANO = 1_000_000_000
ATTEMPTS = ('compile', 'list', 'witness')
CEILINGS = {'compile': 600, 'list': 30, 'witness': 180}
AXES = {'manager_status': 'NOT_OBSERVED_BY_ADAPTER',
        'aggregate_resource_status': 'UNKNOWN'}
REQUEST_KEYS = ('schema_version', 'chain_id', 'grant', 'reference', 'reservation',
    'prior', 'subject_head', 'source_root', 'target_root', 'source_manifest',
    'source_directory', 'target_directory', 'tmp', 'state_directory',
    'environment', 'python', 'cargo', 'rustfmt', 'immutable_files',
    'selection_inputs', 'module_paths', 'raw_cap')


def integer(value, maximum):
    R.need(type(value) is int and 0 <= value <= maximum, 'b04_integer')
    return value


def checked_pin(value):
    W.pin(value)
    return value


def read_json(pin):
    return R.decode(R.pinned(checked_pin(pin)))


def pin_of(path, cap=R.CONTROL_CAP):
    return {k: v for k, v in R.file_observation(path, cap)[0].items()
            if k in ('path', 'sha256')}


def binary(path):
    return {k: v for k, v in R.file_observation(path, R.GIB, executable=True)[0].items()
            if k in ('path', 'sha256', 'dev', 'ino', 'size', 'mtime_ns')}


def validate_history(value):
    R.keys(value, ('sample_count', 'peak_total_bytes', 'peak_temporary_bytes',
        'peak_capture_bytes', 'vanished_by_class', 'vanished', 'sampled_entry_types'))
    integer(value['sample_count'], 8192)
    for key, cap in (('peak_total_bytes', 96 * R.GIB),
                     ('peak_temporary_bytes', 8 * R.GIB),
                     ('peak_capture_bytes', R.TOTAL_CAPTURE - R.TERMINAL_RESERVE)):
        integer(value[key], cap)
    R.keys(value['vanished_by_class'], ('entry-stat', 'directory-scan'))
    R.need(type(value['vanished']) is list and len(value['vanished']) <= 128,
           'b04_history_cap')
    counts = {'entry-stat': 0, 'directory-scan': 0}
    for item in value['vanished']:
        R.keys(item, ('pathname_class', 'relative_path_sha256'))
        R.need(item['pathname_class'] in counts, 'b04_history_class')
        W.hash_value(item['relative_path_sha256'])
        counts[item['pathname_class']] += 1
    for name in counts:
        integer(value['vanished_by_class'][name], 128)
    R.need(counts == value['vanished_by_class'], 'b04_history_counts')
    R.keys(value['sampled_entry_types'], ('directory', 'regular', 'symlink', 'socket'))
    for count in value['sampled_entry_types'].values():
        integer(count, 8192 * R.SCAN_ENTRIES)
    if value['sample_count'] == 0:
        R.need(all(value[k] == 0 for k in ('peak_total_bytes',
            'peak_temporary_bytes', 'peak_capture_bytes')) and not value['vanished']
            and all(n == 0 for n in value['sampled_entry_types'].values()),
            'b04_history_genesis')
    return value


def history_extends(previous, current):
    validate_history(previous)
    validate_history(current)
    for key in ('sample_count', 'peak_total_bytes', 'peak_temporary_bytes',
                'peak_capture_bytes'):
        R.need(current[key] >= previous[key], 'b04_history_reset')
    R.need(current['vanished'][:len(previous['vanished'])] == previous['vanished'],
           'b04_history_reset')
    for field in ('vanished_by_class', 'sampled_entry_types'):
        for key, value in previous[field].items():
            R.need(current[field][key] >= value, 'b04_history_reset')


def retained_capture(spec):
    """Re-read full raw custody and log before parsing a successful capture."""
    R.keys(spec, ('start', 'raw', 'log', 'terminal'))
    start = read_json(spec['start'])
    raw_bytes = R.pinned(spec['raw'], R.RECEIPT_CAP)
    # Supervisor retains finite floats. Control JSON still forbids floats.
    try:
        raw = json.loads(raw_bytes, object_pairs_hook=R.pairs,
                         parse_constant=R.bad_number)
    except (ValueError, UnicodeError, RecursionError):
        raise R.Stop('b04_raw_json') from None
    R.keys(raw, ('schema_version', 'attempt', 'chain_id', 'program_sha256',
        'start_sha256', 'wrapper_before_run_monotonic_ns',
        'wrapper_after_run_monotonic_ns', 'stream', 'boundary_is_not_exact_fork',
        'manager_status', 'aggregate_resource_status', 'supervisor',
        'resource_samples', 'last_sampled_resources'))
    R.need(raw['schema_version'] == 'pbdep.b04-raw.v1'
        and raw['stream'] == 'merged-stdout-stderr'
        and raw['boundary_is_not_exact_fork'] is True
        and all(raw[k] == v for k, v in AXES.items())
        and raw['start_sha256'] == spec['start']['sha256'], 'b04_raw_join')
    R.keys(start, ('schema_version', 'attempt', 'chain_id', 'program_sha256',
        'allocation_id', 'reservation_sha256', 'request_sha256', 'argv', 'cwd',
        'environment_sha256', 'raw_cap', 'useful_seconds', 'exit_seconds',
        'recorded_monotonic_ns', 'slot_spent_even_if_fork_fails'))
    R.need(start['schema_version'] == 'pbdep.b04-start.v1'
        and start['slot_spent_even_if_fork_fails'] is True
        and start['attempt'] in ATTEMPTS
        and all(raw[k] == start[k] for k in ('attempt', 'chain_id', 'program_sha256')),
        'b04_start_join')
    for key in ('chain_id', 'program_sha256', 'allocation_id', 'reservation_sha256',
                'request_sha256', 'environment_sha256'):
        W.hash_value(start[key])
    R.need(type(start['useful_seconds']) is int
        and 0 < start['useful_seconds'] <= CEILINGS[start['attempt']]
        and type(start['exit_seconds']) is int and start['exit_seconds'] == 60,
        'b04_start_budget')
    R.need(type(start['argv']) is list and 1 <= len(start['argv']) <= 256
        and all(type(v) is str and 0 < len(v.encode()) <= 4096 and '\0' not in v
                for v in start['argv']), 'b04_start_argv')
    W.path(start['cwd'])
    integer(start['raw_cap'], R.RAW_CAP)
    R.need(start['raw_cap'] > 0, 'b04_raw_cap')
    log = R.pinned(spec['log'], start['raw_cap'])
    before = W.ns(raw['wrapper_before_run_monotonic_ns'])
    after = W.ns(raw['wrapper_after_run_monotonic_ns'])
    R.need(W.ns(start['recorded_monotonic_ns']) <= before, 'b04_capture_order')
    supervisor_float_scope(raw,wrapped=True)
    validate_supervisor_record(raw['supervisor'])
    capture = R.normalize_capture(raw['supervisor'], log, before, after, 0)
    validate_history(raw['resource_samples'])
    terminal = read_json(spec['terminal'])
    R.keys(terminal, ('schema_version', 'attempt', 'allocation_id', 'capture',
                      'artifacts', 'resources', *AXES))
    R.need(terminal['schema_version'] == 'pbdep.b04-terminal.v1'
        and terminal['attempt'] == start['attempt']
        and terminal['allocation_id'] == start['allocation_id']
        and terminal['capture'] == capture
        and terminal['artifacts'] == {k: spec[k] for k in ('start', 'raw', 'log')}
        and all(terminal[k] == v for k, v in AXES.items()), 'b04_terminal_join')
    W._same_json_data(terminal['capture'], capture)
    history_extends(raw['resource_samples'], terminal['resources'])
    return start, raw, log, capture, terminal


def artifact_cap(name):
    if name in ('Q-ORACLE.json','Q-RESULTS.json','Q-INPUT.json','CONTROL-MAP.json','CONTROL-RESULTS.json','PRIOR.json',
                'OBSERVATIONS-BEFORE.json','OBSERVATIONS-AFTER.json','SEMANTIC.json',
                'SOURCE-MANIFEST.json'):
        return R.CONTROL_CAP
    if name.endswith(('.py','.rs','.toml','.lock')):
        return R.CONTROL_CAP
    if name.endswith(('.txt','.hex','.blob')):
        return R.RECEIPT_CAP
    if name == 'DEPFILE.d':
        return 512 * 1024
    return R.RAW_CAP if name.endswith('.log') else R.receipt_cap(name)


def state_inventory(spec):
    """One nofollow state root; all retained bytes, no rollover or local counter."""
    root = R.directory(spec)
    started = time.monotonic_ns()
    todo, files, directories, count, path_bytes, total = [root], {}, {}, 0, 0, 0
    while todo:
        current = todo.pop()
        with os.scandir(current) as items:
            for item in items:
                count += 1
                path_bytes += len(os.fsencode(item.path))
                R.need(count <= 4096 and path_bytes <= 16 * R.MIB
                    and time.monotonic_ns() - started <= R.SCAN_NS, 'b04_state_bound')
                relative = str(Path(item.path).relative_to(root))
                info = item.stat(follow_symlinks=False)
                if stat.S_ISDIR(info.st_mode):
                    identity = {'path': item.path, 'dev': str(info.st_dev),
                        'ino': str(info.st_ino), 'uid': str(info.st_uid),
                        'mode': str(stat.S_IMODE(info.st_mode))}
                    R.directory(identity)
                    directories[relative] = identity
                    todo.append(Path(item.path))
                else:
                    R.need(stat.S_ISREG(info.st_mode) and info.st_nlink == 1
                        and info.st_uid == os.getuid() and stat.S_IMODE(info.st_mode) == 0o600,
                        'b04_state_file')
                    if relative == 'LOCK':
                        R.need(info.st_size == 0, 'b04_lock_content')
                        continue
                    cap = artifact_cap(item.name)
                    record, _ = R.file_observation(item.path, cap)
                    files[relative] = record
                    total += int(record['size'])
                    R.need(total <= R.TOTAL_CAPTURE - R.TERMINAL_RESERVE, 'b04_state_budget')
    R.need(R.retained_bytes(root) == total, 'b04_state_total')
    return {'state_directory': spec, 'files': files, 'directories': directories,
            'total_bytes': total}


def prior_capture(spec):
    """Accepted B03 RAW/START custody from independently reviewed prior phases."""
    R.keys(spec, ('start', 'raw', 'log'))
    start = read_json(spec['start'])
    R.keys(start, ('schema_version', 'phase', 'slot', 'argv', 'cwd',
        'environment_sha256', 'tools_sha256', 'sources_sha256', 'reference_sha256', 'binding_sha256', 'useful_seconds', 'exit_seconds',
        'raw_cap', 'recorded_monotonic_ns', 'slot_spent_even_if_fork_fails'))
    R.need(start['schema_version'] == 'pbdep.prior-slot-start.v2'
        and start['phase'] in P.ORDER
        and start['slot_spent_even_if_fork_fails'] is True, 'b04_prior_start')
    W.hash_value(start['reference_sha256'])
    W.hash_value(start['binding_sha256'])
    raw_bytes = R.pinned(spec['raw'], R.RECEIPT_CAP)
    try:
        raw = json.loads(raw_bytes, object_pairs_hook=R.pairs, parse_constant=R.bad_number)
    except (ValueError, UnicodeError, RecursionError):
        raise R.Stop('b04_prior_raw') from None
    R.keys(raw, ('schema_version', 'slot', 'wrapper_before_run_monotonic_ns',
        'wrapper_after_run_monotonic_ns', 'boundary_is_not_exact_fork', 'stream',
        'manager_status', 'supervisor', 'aggregate_resource_state_at_terminal_write',
        'resource_samples', 'last_sampled_resources'))
    R.need(raw['schema_version'] == 'pbdep.raw-custody.v1' and raw['slot'] == start['slot']
        and raw['boundary_is_not_exact_fork'] is True
        and raw['stream'] == 'merged-stdout-stderr'
        and raw['manager_status'] == 'NOT_OBSERVED_BY_ADAPTER'
        and raw['aggregate_resource_state_at_terminal_write'] == 'UNKNOWN', 'b04_prior_raw')
    R.need(0 < integer(start['raw_cap'], R.RAW_CAP), 'b04_prior_cap')
    log = R.pinned(spec['log'], start['raw_cap'])
    before, after = (W.ns(raw[k]) for k in
        ('wrapper_before_run_monotonic_ns', 'wrapper_after_run_monotonic_ns'))
    R.need(W.ns(start['recorded_monotonic_ns']) <= before, 'b04_prior_order')
    supervisor_float_scope(raw,wrapped=True)
    validate_supervisor_record(raw['supervisor'])
    expected_exit = 101 if start['phase'] == 'a2' and start['slot'] == '3' else 0
    R.normalize_capture(raw['supervisor'], log, before, after, expected_exit)
    validate_history(raw['resource_samples'])
    return start, raw


# Native prerequisite consumers. No execution/producer is defined below.
Q_PROFILE = 'pbdep.current-qualification-consumer.v1'
Q_FAMILIES = ('wrapper', 'original-contract', 'whole-program', 'policy',
              'runtime-custody', 'pbdep-qd', 'pbdep-rd', 'successor')
Q_COUNTS = ((126,126),(169,169),(354,354),(272,272),(9,9),(32,36),(5,5))
Q_CASES = ('ordinary','detached','killed-detached','nonzero17','postfork-error',
           'outer-error','metadata0','log32','resource-error')
Q_STOPS = (None,None,'useful_time_cap',None,'instrument_error','instrument_error',
           'metadata_overflow','log_cap_prefix_only','instrument_error')
Q_KINDS = ('VALUE_EQUAL','CLOSED_REJECTION','EXCEPTION_CLASS','REFUSAL_TEXT',
           'REFUSAL_TRIPLE','ASSERTION_RETURN','RUNTIME_CASE')
Q_EXCEPTIONS = ('contract.Rejection','whole_program.Rejection','runtime.Stop',
    'policy.Refusal','bootstrap.BootstrapRefusal','runtime.ResourcePathError',
    'builtin.FileExistsError','builtin.FileNotFoundError','builtin.PermissionError')
Q_PRODUCERS = ('qualifier','wrapper-controls','original-contract-controls',
    'whole-program-controls','policy-controls','runtime-controls','pbdep-qd-controls',
    'pbdep-rd-controls','successor-controls','physical-predicates','expected-rows',
    'legacy-id-index','bootstrap-controls')
Q_FRAGMENT_ROLES = {'wrapper':'wrapper-controls','original-contract':'original-contract-controls',
    'whole-program':'whole-program-controls','policy':'policy-controls',
    'runtime-custody':'runtime-controls','pbdep-qd':'pbdep-qd-controls',
    'pbdep-rd':'pbdep-rd-controls','successor':'successor-controls'}
Q_FIXED_PINS = {
    'physical-predicates':'4f3b2ddadb9b8191aed2e8b876b5006204efaba8f3944ae8ab7cf8d7ea74db3e',
    'expected-rows':'a710ec4396e9480583a6cf50825d64761a7f101b580cb081ca01a253c7e803c1',
    'legacy-id-index':'4a3dd5442300becba6164b30f8bd61eba20a6470abcf32099be5a8d4e0fc5304'}
SUPERVISOR_KEYS = ('leader_pid','leader_returncode','leader_exit_monotonic',
    'whole_descendant_set_exit','echild_monotonic','pipe_eof','pipe_eof_monotonic',
    'subreaper_verified','wait_options','elapsed_seconds','stop','metadata_overflow',
    'event_counts','reaped','signals','errors','resource_peak_sampled','log_bytes',
    'discarded_log_bytes','log_sha256')
SUPERVISOR_TIMES = ('leader_exit_monotonic','echild_monotonic',
                    'pipe_eof_monotonic','elapsed_seconds')


def exact(value, expected):
    W._same_json_data(value, expected)


def evidence_bytes(pin, inventory, cap=None):
    retained_pin(pin, inventory)
    return R.pinned(pin, artifact_cap(Path(pin['path']).name) if cap is None else cap)


def evidence_json(pin, inventory):
    return R.decode(evidence_bytes(pin, inventory, R.CONTROL_CAP))


def ascii_id(value):
    R.need(type(value) is str and re.fullmatch(r'[A-Za-z0-9_.:-]{1,128}', value),
           'b04_semantic_id')
    return value


def pin_array(value, inventory, *, maximum=64):
    R.need(type(value) is list and 1 <= len(value) <= maximum, 'b04_semantic_pin_array')
    paths = []
    for pin in value:
        evidence_bytes(pin, inventory)
        paths.append(pin['path'])
    R.need(paths == sorted(set(paths)), 'b04_semantic_pin_order')
    return value


def source_closure(value, inventory):
    R.need(type(value) is list and 1 <= len(value) <= 256, 'b04_semantic_closure')
    roles, paths, result = [], [], {}
    for item in value:
        R.keys(item, ('role','path','sha256'))
        role = ascii_id(item['role'])
        pin = {'path':item['path'],'sha256':item['sha256']}
        result[role] = (pin, evidence_bytes(pin, inventory))
        roles.append(role); paths.append(pin['path'])
    R.need(roles == sorted(set(roles)) and len(paths) == len(set(paths)),
           'b04_semantic_closure_order')
    return result


ENVIRONMENT_FIXED = {'CARGO_BUILD_JOBS':'2', 'CARGO_NET_OFFLINE':'true',
    'CARGO_TERM_COLOR':'never', 'LANG':'C.UTF-8', 'LC_ALL':'C',
    'PYTHONDONTWRITEBYTECODE':'1', 'PYTHONNOUSERSITE':'1', 'RUST_TEST_THREADS':'1'}
ENVIRONMENT_PATHS = ('HOME','CARGO_HOME','RUSTUP_HOME','TMPDIR')
ENVIRONMENT_NO_TARGET = ('qualification','format')
ENVIRONMENT_WITH_TARGET = ('a2','base','candidate','integration','consumers','gate',
                           'discovery','witness')


def cleared_environment(value, binding, *, phase):
    """Closed operational map; lexical/binding checks do not prove safe values."""
    R.need(phase in ENVIRONMENT_NO_TARGET + ENVIRONMENT_WITH_TARGET,
           'b04_environment_phase')
    target = phase in ENVIRONMENT_WITH_TARGET
    R.keys(value, (*ENVIRONMENT_FIXED, *ENVIRONMENT_PATHS, 'PATH',
                   *(('CARGO_TARGET_DIR',) if target else ())))
    for text in value.values():
        R.need(type(text) is str and '\0' not in text
            and len(text.encode()) <= 8192, 'b04_environment')
    for key, expected in ENVIRONMENT_FIXED.items():
        R.need(value[key] == expected, 'b04_environment_literal')
    for key in ENVIRONMENT_PATHS:
        W.path(value[key])
    components = value['PATH'].split(':')
    R.need(1 <= len(components) <= 32 and len(set(components)) == len(components),
           'b04_environment_path')
    for component in components:
        W.path(component)
    if target:
        W.path(value['CARGO_TARGET_DIR'])
    R.need(value['TMPDIR'] == binding['tmp']['path'], 'b04_environment_tmp')
    if phase in ('discovery','witness'):
        R.need(value['CARGO_TARGET_DIR'] == binding['target_root'],
               'b04_environment_target')
    exact(value, binding['environment'])


def prerequisite_binding(record, reference, inventory):
    raw = evidence_bytes(record['phase_binding'], inventory)
    binding = R.decode(raw)
    common = ('schema_version','phase','authority_pins','cwd','environment','python',
        'immutable_files','tmp','selection_inputs','cargo','rustfmt','data',
        'step_zero_acceptance','source_freeze_review','operator_phase_attestation')
    R.keys(binding, common)
    phase = record['phase']
    R.need(binding['phase'] == phase and W.binding_pin(reference['operator'],phase,
        reference=reference) == record['phase_binding'], 'b04_prior_raw_binding')
    if phase in ('qualification','a2'):
        R.need(binding['schema_version'] == 'pbdep.prerequisite-phase-binding.v1',
               'b04_prerequisite_binding_version')
        W.validate_operator(reference['operator'], reference=reference)
        R.keys(binding['data'], ('profile','oracle','input') if phase=='qualification'
               else ('profile','original_reference'))
        profile = Q_PROFILE if phase == 'qualification' else 'pbdep.original-a2-consumer.v1'
        R.need(binding['data']['profile'] == profile,'b04_prerequisite_profile')
    else:
        R.validate_binding_links(reference['operator'],binding,raw,reference=reference)
    expected_authority = {name:reference['operator'][name]['sha256'] for name in
                         ('authority','packet','acceptance','review','disposition')}
    exact(binding['authority_pins'], expected_authority)
    for name in expected_authority:
        evidence_bytes(reference['operator'][name], inventory)
    for name in ('step_zero_acceptance','source_freeze_review'):
        evidence_bytes(binding[name],inventory)
    att = binding['operator_phase_attestation']
    R.keys(att, ('step_zero_read_by','freeze_read_by','issuer','step_zero_sha256',
                 'freeze_sha256','data_sha256','grants_authority'))
    for name in ('step_zero_read_by','freeze_read_by','issuer'):
        R.need(type(att[name]) is str and W.NAME.fullmatch(att[name])
            and len(att[name]) <= 128 and att[name] not in ('UNBOUND','UNKNOWN'),
            'b04_prior_attestation')
    R.need(att['step_zero_sha256'] == binding['step_zero_acceptance']['sha256']
        and att['freeze_sha256'] == binding['source_freeze_review']['sha256']
        and att['data_sha256'] == R.digest(R.encode(binding['data']))
        and att['grants_authority'] is False, 'b04_prior_attestation')
    W.path(binding['cwd'])
    R.path_check(binding['cwd'])
    cleared_environment(binding['environment'], binding, phase=phase)
    R.directory(binding['tmp'],short=True)
    roots=reference['operator']['resource_roots']
    R.need(any(item['directory']==binding['tmp'] and item['kind']=='temporary'
        for item in roots),'b04_prior_temporary_root')
    for item in roots: R.directory(item['directory'])
    if phase in ('qualification','a2'):
        R.need(binding['cwd'] in [item['directory']['path'] for item in roots]
            or binding['cwd']==record['state_directory']['path'],'b04_prior_cwd_root')
    else:
        R.source_resource_join(phase,binding['data'],[item['directory']['path'] for item in roots])
        if phase!='format':
            R.need(binding['cwd']==binding['data']['source_root']
                and binding['environment'].get('CARGO_TARGET_DIR')==binding['data']['target_root']
                and binding['data']['target_root'] in [item['directory']['path'] for item in roots],
                'b04_prior_target_root')
    immutable = binding['immutable_files']
    R.need(type(immutable) is list and 1 <= len(immutable) <= 4096, 'b04_prior_tools')
    paths = []
    for spec in immutable:
        actual, _ = R.file_observation(spec['path'], R.GIB, installed_tool=True, reference=reference)
        exact(actual,spec); paths.append(spec['path'])
    R.need(len(paths) == len(set(paths)), 'b04_prior_tools')
    for name in ('python','cargo','rustfmt'):
        R.need(binding[name] in paths, 'b04_prior_tool_role')
        R.file_observation(binding[name],R.GIB,executable=True,installed_tool=True,reference=reference)
    R.verify_selection_inputs(binding['selection_inputs'],immutable)
    return binding,raw


def retained_observation(pin, record, binding, reference, inventory):
    value = evidence_json(pin,inventory)
    R.keys(value, ('schema_version','program_sha256','phase','binding_sha256','observed_ns',
                  'cwd','environment','tools','sources','binary','depfile_sha256'))
    R.need(value['schema_version'] == 'pbdep.prior-observation.v1'
        and value['program_sha256'] == record['program_sha256']
        and value['phase'] == record['phase']
        and value['binding_sha256'] == record['phase_binding']['sha256']
        and value['cwd'] == binding['cwd'], 'b04_prior_observation')
    cleared_environment(value['environment'],binding,phase=record['phase'])
    R.keys(value['tools'], ('python','cargo','rustfmt'))
    for name, spec in value['tools'].items():
        R.need(spec['path'] == binding[name] and spec in binding['immutable_files'],
               'b04_prior_observed_tool')
        actual, _ = R.file_observation(spec['path'],R.GIB,executable=True,
                                       installed_tool=True,reference=reference)
        exact(actual,spec)
    sources = value['sources']
    R.need(type(sources) is dict and 1 <= len(sources) <= 2048, 'b04_prior_observed_sources')
    hashes = {}
    for name,pin_value in sources.items():
        W.path(name,relative=record['phase'] != 'format')
        hashes[name] = R.digest(evidence_bytes(pin_value,inventory))
    if value['binary'] is not None:
        W.binary(value['binary'])
        exact(binary(value['binary']['path']),value['binary'])
        W.hash_value(value['depfile_sha256'])
        R.need(R.file_observation(value['binary']['path']+'.d',512*1024)[0]['sha256']
            == value['depfile_sha256'],'b04_prior_depfile_identity')
    else:
        R.need(value['depfile_sha256'] is None,'b04_prior_binary_shape')
    return value,hashes


def strict_supervisor(pin, inventory):
    raw = evidence_bytes(pin,inventory,R.RECEIPT_CAP)
    def finite(text):
        number = float(text)
        R.need(math.isfinite(number),'b04_supervisor_nonfinite')
        return number
    try:
        value = json.loads(raw,object_pairs_hook=R.pairs,parse_float=finite,
                           parse_constant=R.bad_number)
    except (ValueError,UnicodeError,RecursionError):
        raise R.Stop('b04_supervisor_json') from None
    supervisor_float_scope(value,wrapped=False)
    return validate_supervisor_record(value)


def supervisor_float_scope(value, *, wrapped, path=(), nodes=None):
    """Only six native supervisor time-path classes admit JSON floats."""
    if nodes is None:nodes=[0]
    nodes[0]+=1
    R.need(len(path)<=12 and nodes[0]<=65536,'b04_supervisor_json_bound')
    if type(value) is float:
        relative = path[1:] if wrapped and path[:1] == ('supervisor',) else (
            path if not wrapped else ())
        allowed = (len(relative) == 1 and relative[0] in SUPERVISOR_TIMES) or (
            len(relative) == 3 and relative[0] in ('reaped','signals')
            and type(relative[1]) is int and relative[2] == 'at')
        R.need(allowed and math.isfinite(value) and value>=0,'b04_supervisor_float_scope')
    elif type(value) is dict:
        for name,item in value.items():
            supervisor_float_scope(item,wrapped=wrapped,path=path+(name,),nodes=nodes)
    elif type(value) is list:
        for index,item in enumerate(value):
            supervisor_float_scope(item,wrapped=wrapped,path=path+(index,),nodes=nodes)


def supervisor_time(value, *, nullable=False):
    R.need((nullable and value is None) or (type(value) in (int,float)
        and math.isfinite(value) and value >= 0), 'b04_supervisor_time')


def validate_supervisor_record(value):
    """Native shape only: incomplete failure evidence is not successful custody."""
    R.keys(value,SUPERVISOR_KEYS)
    R.need((value['leader_pid'] is None or (type(value['leader_pid']) is int
        and value['leader_pid'] > 0)) and (value['leader_returncode'] is None
        or type(value['leader_returncode']) is int), 'b04_supervisor_leader')
    for name in SUPERVISOR_TIMES:
        supervisor_time(value[name],nullable=name != 'elapsed_seconds')
    for name in ('pipe_eof','subreaper_verified','metadata_overflow'):
        R.need(type(value[name]) is bool,'b04_supervisor_scalar')
    R.need(value['whole_descendant_set_exit'] in ('PROVEN_ECHILD','UNKNOWN')
        and value['subreaper_verified'] is True
        and value['wait_options'] == 'WNOHANG|__WALL'
        and value['stop'] in (None,'useful_time_cap','instrument_error','metadata_overflow',
            'log_cap_prefix_only','descendant_exit_or_capture_unknown'),'b04_supervisor_scalar')
    for name in ('log_bytes','discarded_log_bytes','resource_peak_sampled'):
        integer(value[name],2**63-1)
    W.hash_value(value['log_sha256'])
    R.keys(value['event_counts'],('reaped','signals','errors'))
    for name in ('reaped','signals','errors'):
        integer(value['event_counts'][name],2**63-1)
        R.need(type(value[name]) is list and len(value[name]) <= 128,'b04_supervisor_events')
    for row in value['reaped']:
        R.keys(row,('pid','status','exit_code','at'))
        R.need(type(row['pid']) is int and row['pid'] > 0
            and type(row['status']) is int and type(row['exit_code']) is int,
            'b04_supervisor_reaped')
        supervisor_time(row['at'])
    for row in value['signals']:
        R.keys(row,('pid','signal','at'))
        R.need(type(row['pid']) is int and row['pid'] > 0
            and type(row['signal']) is int,'b04_supervisor_signal')
        supervisor_time(row['at'])
    for row in value['errors']:
        R.need(type(row) is dict,'b04_supervisor_error')
        R.keys(row,('where','type','pathname_class','errno') if 'pathname_class' in row
               else ('where','type'))
        R.need(row['where'] in ('postfork_setup','reap','resources','capture','signal',
            'log_persist','outer_parent','emergency_cleanup','emergency_persist')
            and type(row['type']) is str,'b04_supervisor_error')
        if 'pathname_class' in row:
            R.need(row['pathname_class'] in ('root-lstat','entry-stat','directory-scan')
                and (row['errno'] is None or type(row['errno']) is int),
                'b04_supervisor_error')
    return value


def physical_case(case, supervisor_pin, log_pin, inventory):
    R.need(case in Q_CASES,'b04_runtime_case')
    value = strict_supervisor(supervisor_pin,inventory)
    raw = evidence_bytes(log_pin,inventory,4096)
    # Historical positive predicates remain separate from nullable native shape.
    R.need(type(value['leader_pid']) is int and value['leader_pid'] > 0
        and type(value['leader_returncode']) is int, 'b04_runtime_leader')
    for name in SUPERVISOR_TIMES:
        supervisor_time(value[name])
    expected_stop = Q_STOPS[Q_CASES.index(case)]
    R.need(value['whole_descendant_set_exit'] == 'PROVEN_ECHILD'
        and value['pipe_eof'] is True and value['subreaper_verified'] is True
        and value['wait_options'] == 'WNOHANG|__WALL' and value['stop'] == expected_stop,
        'b04_runtime_physical')
    R.need(value['echild_monotonic'] >= value['leader_exit_monotonic']
        and value['log_bytes'] == len(raw) and value['log_sha256'] == R.digest(raw)
        and value['metadata_overflow'] == (case == 'metadata0'),'b04_runtime_identity')
    counts = value['event_counts']
    for category in ('reaped','signals','errors'):
        if case != 'metadata0':
            R.need(counts[category] == len(value[category]) <= 128,'b04_runtime_counts')
    errors = value['errors']
    if case == 'postfork-error':
        exact(errors,[{'where':'postfork_setup','type':'OSError'}])
    elif case == 'outer-error':
        exact(errors,[{'where':'outer_parent','type':'KeyboardInterrupt'}])
    elif case == 'resource-error':
        R.need(1 <= len(errors) <= 128,'b04_runtime_errors')
        for row in errors:
            exact(row,{'where':'resources','type':'TraversalError',
                       'pathname_class':'directory-scan','errno':13})
    else:
        R.need(errors == [] and counts['errors'] == 0,'b04_runtime_errors')
    expected_count = 3 if case in ('detached','killed-detached') else 1
    R.need(counts['reaped'] == expected_count,'b04_runtime_reaped')
    leader,rc = value['leader_pid'],value['leader_returncode']
    if case == 'metadata0':
        R.need(value['reaped'] == [] and value['signals'] == [] and counts['signals'] == 0,
               'b04_runtime_metadata')
    else:
        pids = [row['pid'] for row in value['reaped']]
        R.need(len(set(pids)) == expected_count and pids.count(leader) == 1
            and next(row['exit_code'] for row in value['reaped'] if row['pid'] == leader) == rc,
            'b04_runtime_leader')
        R.need(all(row['pid'] in pids and row['signal'] in (15,9) for row in value['signals']),
               'b04_runtime_signal')
    if case in ('postfork-error','outer-error','resource-error'):
        R.need(rc in (-15,-9) and any(row['pid'] == leader and row['signal'] == -rc
            for row in value['signals']),'b04_runtime_cleanup')
    elif case == 'log32':
        R.need(rc in (0,-15,-9),'b04_runtime_overflow')
        if rc < 0:
            R.need(any(row['pid'] == leader and row['signal'] == -rc
                for row in value['signals']),'b04_runtime_cleanup')
    else:
        R.need(rc == (17 if case == 'nonzero17' else 0),'b04_runtime_exit')
    if case in ('ordinary','detached','nonzero17','metadata0'):
        R.need(value['signals'] == [] and counts['signals'] == 0,'b04_runtime_signal')
    if case in ('detached','killed-detached'):
        R.need(value['echild_monotonic'] > value['leader_exit_monotonic']
            and value['echild_monotonic'] > value['pipe_eof_monotonic'],'b04_runtime_detached')
        R.need(sorted(row['exit_code'] for row in value['reaped']) ==
            ([-9,0,0] if case == 'killed-detached' else [0,0,0]),'b04_runtime_detached')
        if case == 'killed-detached':
            victim = next(row['pid'] for row in value['reaped'] if row['exit_code'] == -9)
            R.need(victim != leader and {row['signal'] for row in value['signals']
                if row['pid'] == victim} == {15,9},'b04_runtime_detached')
    if case == 'log32':
        R.need(raw == b'x'*32 and 0 < value['discarded_log_bytes'] <= 8160,
               'b04_runtime_loss')
    else:
        R.need(value['discarded_log_bytes'] == 0
            and raw == (b'ordinary\n' if case == 'ordinary' else b''),'b04_runtime_log')
    R.need(value['resource_peak_sampled'] ==
        (0 if case in ('outer-error','resource-error') else 17),'b04_runtime_resource')


def tagged_value(value, inventory, depth=0, nodes=None):
    if nodes is None: nodes=[0]
    nodes[0] += 1
    R.need(depth <= 12 and nodes[0] <= 4096 and type(value) is dict,'b04_q_value_cap')
    tag=value.get('tag')
    if tag == 'none':
        R.keys(value,('tag',)); return ('none',)
    if tag == 'bool':
        R.keys(value,('tag','value')); R.need(type(value['value']) is bool,'b04_q_bool')
        return ('bool',value['value'])
    if tag == 'int':
        R.keys(value,('tag','value')); text=value['value']
        R.need(type(text) is str and re.fullmatch(r'(?:0|-?[1-9][0-9]{0,31})',text),
               'b04_q_int')
        return ('int',text)
    if tag == 'text':
        R.keys(value,('tag','value')); text=value['value']
        R.need(type(text) is str and '\0' not in text and len(text.encode()) <= 65536,'b04_q_text')
        return ('text',text)
    if tag == 'bytes':
        R.keys(value,('tag','pin','length')); integer(value['length'],65536)
        raw=evidence_bytes(value['pin'],inventory,65536)
        R.need(len(raw)==value['length'],'b04_q_bytes')
        return ('bytes',raw)
    if tag in ('list','tuple','map'):
        R.keys(value,('tag','items')); items=value['items']
        R.need(type(items) is list and len(items) <= 1024,'b04_q_items')
        if tag == 'map':
            keys=[]
            result=[]
            for pair in items:
                R.need(type(pair) is list and len(pair)==2 and type(pair[0]) is str
                    and '\0' not in pair[0] and len(pair[0].encode()) <= 65536,'b04_q_map')
                keys.append(pair[0]); result.append((pair[0],tagged_value(pair[1],inventory,depth+1,nodes)))
            R.need(keys==sorted(set(keys)),'b04_q_map')
            return ('map',tuple(result))
        return (tag,tuple(tagged_value(v,inventory,depth+1,nodes) for v in items))
    raise R.Stop('b04_q_unsupported_value')


def q_refusal_triple(expected, observed, row, context):
    for value in (expected,observed):
        R.keys(value,('outcome','code','provenance','context'))
        R.need(value['outcome'] in ('ACCEPT','REJECT_TYPED'),'b04_q_refusal')
        if value['outcome']=='ACCEPT':
            R.need(value['code'] is None and value['provenance'] is None
                and value['context'] is None,'b04_q_refusal')
        else:
            for key in ('code','provenance'):
                R.need(type(value[key]) is str and value[key].isascii()
                    and 1 <= len(value[key]) <= 128,'b04_q_refusal')
    exact({k:observed[k] for k in ('outcome','code','provenance')},
          {k:expected[k] for k in ('outcome','code','provenance')})
    if row['observation_id'] != 'D22.1':
        R.need(expected['context'] is None and observed['context'] is None,'b04_q_context')
        return
    exact(expected['context'],{'kind':'D22_MISSING','raw_token':'a/cancelled/../leaf.txt',
        'component':'cancelled','errno':2,'parent_suffix':'source/a'})
    value=observed['context']
    R.keys(value,('kind','raw_token','component','errno','parent_path','parent_dev',
                  'parent_ino','pre_observation'))
    for key in ('kind','raw_token','component','errno'):
        exact(value[key],expected['context'][key])
    fixture=context['fixture']
    R.need(value['parent_path']==fixture['directory']['path']+'/source/a','b04_q_parent_path')
    W.ns(value['parent_dev'])
    R.need(W.ns(value['parent_ino']) > 0,'b04_q_parent_identity')
    pre=evidence_json(value['pre_observation'],context['inventory'])
    R.keys(pre,('schema_version','program_sha256','binding_sha256','case_id','parent_path',
                'parent_dev','parent_ino','observed_ns','call_started_ns'))
    R.need(pre['schema_version']=='pbdep.q-parent-preimage.v1'
        and pre['program_sha256']==context['record']['program_sha256']
        and pre['binding_sha256']==context['record']['phase_binding']['sha256']
        and pre['case_id']=='D22.1','b04_q_parent_preimage')
    for key in ('parent_path','parent_dev','parent_ino'): exact(pre[key],value[key])
    R.need(context['capture_begin'] <= W.ns(pre['observed_ns']) <= W.ns(pre['call_started_ns'])
        <= context['capture_end'],'b04_q_parent_chronology')


def q_row(expected, observed, family, producer, context):
    R.keys(expected,('control_id','observation_id','kind','expected','inputs','oracle_source'))
    R.keys(observed,('control_id','observation_id','kind','inputs_sha256','oracle_source_sha256','observed'))
    for key in ('control_id','observation_id','kind'): exact(observed[key],expected[key])
    ascii_id(expected['control_id']); ascii_id(expected['observation_id'])
    kind=expected['kind']
    R.need(kind in Q_KINDS,'b04_q_unsupported_kind')
    if family=='whole-program':
        R.need(kind in ('VALUE_EQUAL','CLOSED_REJECTION','EXCEPTION_CLASS'),'b04_q_historical_kind')
    if family=='policy':
        R.need(kind in ('VALUE_EQUAL','REFUSAL_TEXT','EXCEPTION_CLASS'),'b04_q_historical_kind')
    if family=='runtime-custody':R.need(kind=='RUNTIME_CASE','b04_q_runtime_role')
    R.need(len(R.encode(expected))<=R.RECEIPT_CAP and len(R.encode(observed))<=R.RECEIPT_CAP,
           'b04_q_row_cap')
    pin_array(expected['inputs'],context['inventory'])
    R.need(observed['inputs_sha256']==R.digest(R.encode(expected['inputs']))
        and observed['oracle_source_sha256']==expected['oracle_source']['sha256'],
        'b04_q_row_pins')
    fragment=evidence_bytes(expected['oracle_source'],context['inventory'],R.RECEIPT_CAP)
    R.need(fragment and fragment in producer[Q_FRAGMENT_ROLES[family]][1],'b04_q_assertion_source')
    e,o=expected['expected'],observed['observed']
    if kind=='VALUE_EQUAL':
        R.keys(e,('value',));R.keys(o,('value',))
        nodes=[0]
        exact(tagged_value(e['value'],context['inventory'],nodes=nodes),
              tagged_value(o['value'],context['inventory'],nodes=nodes))
    elif kind=='CLOSED_REJECTION':
        exact(e,{'exception_role':'whole_program.Rejection'})
        R.keys(o,('exception_role','code'))
        R.need(o['exception_role']==e['exception_role'] and type(o['code']) is str
            and re.fullmatch(r'[A-Z][A-Z0-9_]{0,127}',o['code']),'b04_q_closed_rejection')
    elif kind=='EXCEPTION_CLASS':
        R.keys(e,('exception_role',)); R.keys(o,('exception_role',))
        R.need(e['exception_role'] in Q_EXCEPTIONS,'b04_q_exception_role')
        exact(o,e)
        role=e['exception_role'].split('.')[0]
        if role!='builtin':
            source = (producer['bootstrap-controls'] if role=='bootstrap'
                      else context['subject'].get(role+'.py'))
            R.need(source is not None,'b04_q_exception_source')
            tree=ast.parse(source[1])
            class_name=e['exception_role'].split('.')[1]
            R.need(any(isinstance(node,ast.ClassDef) and node.name==class_name
                for node in ast.walk(tree)),'b04_q_exception_source')
    elif kind=='REFUSAL_TEXT':
        R.keys(e,('exception_role','text')); R.keys(o,('exception_role','text'))
        R.need(e['exception_role']=='policy.Refusal' and type(e['text']) is str
            and 1<=len(e['text'].encode())<=256,'b04_q_refusal_text')
        exact(o,e)
    elif kind=='REFUSAL_TRIPLE':
        if expected['observation_id']=='D22.1':
            R.need(context['fixture_pin'] in expected['inputs'],'b04_q_fixture_role')
        q_refusal_triple(e,o,expected,context)
    elif kind=='ASSERTION_RETURN':
        R.need(family in ('wrapper','original-contract'),'b04_q_assertion_family')
        exact(e,{'assertion_sha256':expected['oracle_source']['sha256'],'return_kind':'none'})
        exact(o,e)
    else:
        R.keys(e,('case',)); R.keys(o,('case','supervisor','raw_log'))
        R.need(e['case'] in Q_CASES and o['case']==e['case']
            and (family!='runtime-custody' or e['case']==expected['control_id']),'b04_q_runtime_role')
        physical_case(e['case'],o['supervisor'],o['raw_log'],context['inventory'])


def verify_prior_commands(binding, captures, commands, before):
    R.need(len(commands)==len(captures),'b04_prior_command_count')
    for command,item in zip(commands,captures):
        start=item['start']
        R.need(type(start['argv']) is list and tuple(start['argv'])==command
            and start['cwd']==binding['cwd']
            and start['environment_sha256']==R.digest(R.encode(binding['environment']))
            and start['tools_sha256']==R.digest(R.encode(before['tools']))
            and start['sources_sha256']==R.digest(R.encode(before['_hashes'])),
            'b04_prior_command_semantics')


def consume_q(record, binding, evidence, captures, before, after, inventory):
    roles=evidence['roles']
    R.keys(roles,('input','oracle','results','fixture'))
    input_value=evidence_json(roles['input'],inventory)
    R.keys(input_value,('schema_version','profile','program_sha256','phase_binding_path','oracle',
                       'subject_source_closure','producer_source_closure','cwd','environment','tools','fixture'))
    R.need(input_value['schema_version']=='pbdep.q-input.v1' and input_value['profile']==Q_PROFILE
        and input_value['program_sha256']==record['program_sha256']
        and input_value['phase_binding_path']==record['phase_binding']['path']
        and binding['data']['oracle']==roles['oracle'] and binding['data']['input']==roles['input']
        and input_value['oracle']==roles['oracle'] and input_value['fixture']==roles['fixture']
        and input_value['cwd']==binding['cwd'],'b04_q_input')
    cleared_environment(input_value['environment'],binding,phase='qualification')
    exact(input_value['tools'],before['tools'])
    R.need(before['binary'] is None and after['binary'] is None,'b04_q_binary_shape')
    oracle=evidence_json(roles['oracle'],inventory)
    R.keys(oracle,('schema_version','profile','subject_source_closure','producer_source_closure',
                   'families','review_pin'))
    R.need(oracle['schema_version']=='pbdep.q-oracle.v1' and oracle['profile']==Q_PROFILE,
           'b04_q_oracle')
    evidence_bytes(oracle['review_pin'],inventory)
    for key in ('subject_source_closure','producer_source_closure'):
        exact(input_value[key],oracle[key])
    subject=source_closure(oracle['subject_source_closure'],inventory)
    producer=source_closure(oracle['producer_source_closure'],inventory)
    expected_modules=record['_reference']['operator']['instrument_sha256']
    R.keys(subject,expected_modules)
    for name,(pin,raw) in subject.items():
        R.need(Path(pin['path']).name==name and pin['sha256']==expected_modules[name]
            and any(spec['path']==pin['path'] and spec['sha256']==pin['sha256']
                    for spec in binding['immutable_files']),'b04_q_subject_role')
        R.need(before['_hashes'].get(name)==after['_hashes'].get(name)==R.digest(raw),
               'b04_q_subject_observation')
    exact(before['sources'],{name:pair[0] for name,pair in subject.items()})
    exact(after['sources'],before['sources'])
    R.keys(producer,Q_PRODUCERS)
    R.need(producer['qualifier'][0]['path'].endswith('/scripts/qualification/pbdep/qualify_whole_program.py'),
           'b04_q_qualifier_role')
    for role,digest in Q_FIXED_PINS.items():
        R.need(producer[role][0]['sha256']==digest,'b04_q_fixed_source')
    for role,(pin,raw) in producer.items():
        R.need(any(spec['path']==pin['path'] and spec['sha256']==R.digest(raw)
            for spec in binding['immutable_files']),'b04_q_producer_role')
    fixture=evidence_json(roles['fixture'],inventory)
    R.keys(fixture,('schema_version','role','directory'))
    R.need(fixture['schema_version']=='pbdep.q-fixture-root.v1' and fixture['role']=='D22',
           'b04_q_fixture_role')
    W.path(fixture['directory']['path'])
    R.directory(fixture['directory'])
    fixture_path=Path(fixture['directory']['path'])
    allowed_roots=[record['state_directory']['path']]+[
        item['directory']['path'] for item in record['_reference']['operator']['resource_roots']]
    R.need(any(fixture_path==Path(path) or Path(path) in fixture_path.parents
        for path in allowed_roots),'b04_q_fixture_root_accounting')
    command=(binding['python'],'-B','-I',producer['qualifier'][0]['path'],
             '--evidence-input',roles['input']['path'])
    verify_prior_commands(binding,captures,(command,),before)
    result=evidence_json(roles['results'],inventory)
    R.keys(result,('schema_version','profile','oracle_sha256','subject_source_closure_sha256',
        'producer_source_closure_sha256','binding_sha256','input_sha256','start_sha256','raw_sha256','families'))
    R.need(result['schema_version']=='pbdep.q-results.v1' and result['profile']==Q_PROFILE
        and result['oracle_sha256']==roles['oracle']['sha256']
        and result['binding_sha256']==record['phase_binding']['sha256']
        and result['input_sha256']==roles['input']['sha256']
        and result['start_sha256']==captures[0]['spec']['start']['sha256']
        and result['raw_sha256']==captures[0]['spec']['raw']['sha256'],'b04_q_result_join')
    for key in ('subject_source_closure','producer_source_closure'):
        R.need(result[key+'_sha256']==R.digest(R.encode(oracle[key])),'b04_q_closure_join')
    R.need(type(oracle['families']) is list and type(result['families']) is list
        and len(oracle['families'])==len(result['families'])==8,'b04_q_families')
    legacy=R.decode(producer['legacy-id-index'][1])
    projection=R.decode(producer['expected-rows'][1])
    context={'inventory':inventory,'record':record,'fixture':fixture,'fixture_pin':roles['fixture'],
        'capture_begin':W.ns(captures[0]['capture']['start_monotonic_ns']),
        'capture_end':W.ns(captures[0]['capture']['end_monotonic_ns']),'subject':subject}
    # Establish the complete family/row/kind identity chain before any predicate.
    pre_count=0
    for index,(ef,af) in enumerate(zip(oracle['families'],result['families'])):
        R.keys(ef,('family','control_ids','rows'));R.keys(af,('family','rows'))
        R.need(ef['family']==af['family']==Q_FAMILIES[index]
            and type(ef['rows']) is list and type(af['rows']) is list
            and len(ef['rows'])==len(af['rows']),'b04_q_registry')
        seen=set()
        for er,ar in zip(ef['rows'],af['rows']):
            R.keys(er,('control_id','observation_id','kind','expected','inputs','oracle_source'))
            R.keys(ar,('control_id','observation_id','kind','inputs_sha256','oracle_source_sha256','observed'))
            ascii_id(er['control_id']);ascii_id(er['observation_id'])
            R.need(er['kind'] in Q_KINDS,'b04_q_unsupported_kind')
            key=(er['control_id'],er['observation_id'])
            R.need(key not in seen,'b04_q_row_order');seen.add(key)
            for name in ('control_id','observation_id','kind'):exact(ar[name],er[name])
        pre_count+=len(ef['rows'])
    R.need(pre_count<=2048,'b04_q_total_rows')
    total=0; historical=set(); rows_for_fixed={}
    for index,(expected_family,actual_family) in enumerate(zip(oracle['families'],result['families'])):
        R.keys(expected_family,('family','control_ids','rows'));R.keys(actual_family,('family','rows'))
        family=Q_FAMILIES[index]
        R.need(expected_family['family']==actual_family['family']==family,'b04_q_family_order')
        ids=expected_family['control_ids']; expected_rows=expected_family['rows']; actual_rows=actual_family['rows']
        R.need(type(ids) is list and len(ids)==len(set(ids)) and ids
            and all(ascii_id(v) for v in ids) and type(expected_rows) is list
            and type(actual_rows) is list and len(expected_rows)==len(actual_rows),'b04_q_inventory')
        if index<7:
            id_count,row_count=Q_COUNTS[index]
            R.need(len(ids)==id_count and len(expected_rows)==row_count,'b04_q_inventory')
        else:
            R.need(1<=len(ids)<=len(expected_rows)<=1024
                and all(re.fullmatch(r'SUC[0-9]{3,6}',v) and v not in historical for v in ids),
                'b04_q_successor_inventory')
        if family=='wrapper': exact(ids,legacy['families']['wrapper']['id_order'])
        if family=='original-contract': R.need(set(ids)==set(legacy['families']['original_contract']['result_ids']),'b04_q_contract_ids')
        if family=='whole-program': exact(ids,legacy['families']['whole_program']['id_order'])
        if family=='runtime-custody': exact(ids,list(Q_CASES))
        if family=='pbdep-qd': exact(ids,['D%02d'%n for n in range(1,33)])
        if family=='pbdep-rd': exact(ids,['RD%02d'%n for n in range(1,6)])
        if family in ('wrapper','original-contract','whole-program','policy','runtime-custody'):
            exact([row['control_id'] for row in expected_rows],ids)
        seen=set();present=set()
        for expected_row,actual_row in zip(expected_rows,actual_rows):
            R.need(type(expected_row) is dict and type(actual_row) is dict,'b04_q_row')
            key=(expected_row.get('control_id'),expected_row.get('observation_id'))
            R.need(key not in seen and key[0] in ids,'b04_q_row_order')
            seen.add(key);present.add(key[0])
            q_row(expected_row,actual_row,family,producer,context)
        R.need(present==set(ids),'b04_q_inventory')
        historical.update(ids);total+=len(expected_rows)
        rows_for_fixed[family]=expected_rows
    R.need(total<=2048,'b04_q_total_rows')
    expected_qd=[(v['control_id'],o['observation_id'],o['expected']) for v in
                 projection['discovery_d_controls'] for o in v['observations']]
    expected_rd=[(v['observation_id'],v['observation_id'],v['expected']) for v in projection['replay_observations']]
    for family,fixed in (('pbdep-qd',expected_qd),('pbdep-rd',expected_rd)):
        actual=rows_for_fixed[family]
        R.need(len(actual)==len(fixed),'b04_q_fixed_inventory')
        for row,(cid,oid,triple) in zip(actual,fixed):
            R.need(row['control_id']==cid and row['observation_id']==oid
                and row['kind']=='REFUSAL_TRIPLE','b04_q_fixed_inventory')
            exact({k:row['expected'][k] for k in ('outcome','code','provenance')},triple)
    command=(binding['python'],'-B','-I',producer['qualifier'][0]['path'],
             '--evidence-input',roles['input']['path'])
    return (command,),{'scope':'current-synthetic-qualification-evidence-only','rows':total}


def consume_a2(record,binding,evidence,captures,before,after,inventory):
    roles=evidence['roles']
    R.keys(roles,('original_reference','provenance_binding','full_roster','pilot_roster',
        'overlay_roster','manifest','depfile','admission','pre','completion','result'))
    R.need(binding['data']['original_reference']==roles['original_reference'],'b04_a2_reference_role')
    ref=evidence_json(roles['original_reference'],inventory)
    original_binding=evidence_bytes(roles['provenance_binding'],inventory)
    native=R.decode(original_binding)
    R.need(native['schema_version']=='pbdep.provenance-binding.v2','b04_a2_native_v2')
    for key,role in (('authority_sha256','authority'),('bound_packet_sha256','packet'),
                     ('acceptance_sha256','acceptance'),('review_sha256','review')):
        R.need(ref[key]==record['_reference']['operator'][role]['sha256'],'b04_a2_authority')
    R.need(native['source_root']==binding['cwd']
        and native['subject_head']==ref['subject_head']
        and native['source_manifest_sha256']==roles['manifest']['sha256']
        and binding['environment'].get('CARGO_TARGET_DIR')==native['target_root']
        and any(root['directory']['path']==native['target_root']
            for root in record['_reference']['operator']['resource_roots']),'b04_a2_source')
    R.need(before['binary'] is None and after['binary'] is not None,'b04_a2_binary')
    full=evidence_bytes(roles['full_roster'],inventory);pilot=evidence_bytes(roles['pilot_roster'],inventory)
    overlay=evidence_bytes(roles['overlay_roster'],inventory)
    R.need(R.digest(evidence_bytes(roles['depfile'],inventory,512*1024))==after['depfile_sha256'],
           'b04_a2_depfile')
    executable=after['binary']['path']
    commands=(compile_command(binding),(executable,'--list','--format','terse'),
        (executable,'artifact::artifact_identity_contract::','--nocapture','--test-threads=1'))
    verify_prior_commands(binding,captures,commands,before)
    C.validate_listing(captures[1]['log'],full_roster=full,pilot_roster=pilot,
        overlay_roster=overlay,roster_sha256=C.ROSTER_PINS,
        evidence_contract='phase-b.original-a2-evidence.v1')
    manifest=evidence_bytes(roles['manifest'],inventory);dep=evidence_bytes(roles['depfile'],inventory,512*1024)
    admission=evidence_bytes(roles['admission'],inventory);pre=evidence_bytes(roles['pre'],inventory)
    admission_value=R.decode(admission)
    exact(admission_value['compile'],captures[0]['capture'])
    exact(admission_value['inventory'],captures[1]['capture'])
    provenance=C.validate_provenance(captures[0]['log'],dep,captures[1]['log'],manifest,
        admission,pre,binding=original_binding,binding_sha256=roles['provenance_binding']['sha256'],
        observed_source_hashes=before['_hashes'],observed_binary=after['binary'],reference=ref)
    completion=evidence_bytes(roles['completion'],inventory)
    exact(R.decode(completion)['capture'],captures[2]['capture'])
    C.validate_witness_completion(captures[2]['log'],completion,provenance=provenance,
        observed_binary=after['binary'],observed_source_hashes=after['_hashes'],
        observed_depfile_sha256=after['depfile_sha256'],reference=ref)
    outcome=C.classify_a2(captures[2]['log'],exit_code=101,framing='direct',filtered_out=513,
        store_root=binding['tmp']['path'],evidence_contract='phase-b.original-a2-evidence.v1',
        historical_prefix=())
    exact(evidence_json(roles['result'],inventory),outcome)
    executable=after['binary']['path']
    commands=(compile_command(binding),(executable,'--list','--format','terse'),
        (executable,'artifact::artifact_identity_contract::','--nocapture','--test-threads=1'))
    return commands,outcome


def consume_later(record,binding,evidence,captures,before,after,inventory):
    phase=record['phase'];data=binding['data'];ref=record['_reference'];raw=record['_binding_raw']
    roles=evidence['roles']
    commands=W.commands(phase,data,{'cargo':binding['cargo'],'rustfmt':binding['rustfmt']},
        reference=ref,phase_binding_raw=raw,
        executable=after['binary']['path'] if phase in ('base','candidate') and after['binary'] is not None else None)
    verify_prior_commands(binding,captures,commands,before)
    if phase in ('base','candidate'):
        R.keys(roles,('manifest','depfile','admission','pre','completion','full_roster','selected_roster','result'))
        R.need(roles['manifest']==data['source_manifest'] and roles['full_roster']==data['full_roster']
            and roles['selected_roster']==data['selected_roster'],'b04_prior_source_roles')
        R.need(before['binary'] is None and after['binary'] is not None,'b04_prior_binary')
        manifest=evidence_bytes(roles['manifest'],inventory)
        dep=evidence_bytes(roles['depfile'],inventory,512*1024)
        R.need(R.digest(dep)==after['depfile_sha256'],'b04_prior_depfile_identity')
        expected=R.decode(manifest)
        exact(before['_hashes'],expected['files']);exact(after['_hashes'],expected['files'])
        obs_before=observe_proposed_dependencies(expected,data)
        obs_after=R.observe_dependencies(expected,data,reference=ref,phase_binding_raw=raw,expected=obs_before)
        compilation=W.validate_compilation(captures[0]['log'],dep,manifest_raw=manifest,data=data,
            observed_binary=after['binary'],observed_dependencies=obs_before,reference=ref,phase_binding_raw=raw)
        inventory_fact=W.validate_inventory(captures[1]['log'],full_roster=evidence_bytes(roles['full_roster'],inventory),
            selected_roster=evidence_bytes(roles['selected_roster'],inventory),case_map=data['case_map'],
            data=data,reference=ref,phase_binding_raw=raw)
        admission=evidence_json(roles['admission'],inventory);pre=evidence_json(roles['pre'],inventory)
        exact(admission['compile'],captures[0]['capture']);exact(admission['inventory'],captures[1]['capture'])
        W.validate_admission(captures[0]['log'],captures[1]['log'],admission,manifest_raw=manifest,data=data,
            compilation=compilation,inventory=inventory_fact,reference=ref,phase_binding_raw=raw)
        provenance=W.validate_pre_witness(captures[0]['log'],captures[1]['log'],admission,pre,
            manifest_raw=manifest,data=data,compilation=compilation,inventory=inventory_fact,
            reference=ref,phase_binding_raw=raw)
        outcome=W.validate_completion(captures[2]['log'],captures[2]['capture'],provenance=provenance,
            observed_binary=after['binary'],observed_sources=after['_hashes'],
            observed_depfile_sha256=after['depfile_sha256'],observed_dependencies=obs_after,
            data=data,manifest_raw=manifest,reference=ref,phase_binding_raw=raw)
        exact(evidence_json(roles['completion'],inventory),outcome)
        selected=W.validate_selected(captures[2]['log'],selected_roster=evidence_bytes(roles['selected_roster'],inventory),
                                     full_count=inventory_fact['full_count'],exit_code=0)
        exact(evidence_json(roles['result'],inventory),selected)
        commands=W.commands(phase,data,{'cargo':binding['cargo'],'rustfmt':binding['rustfmt']},
                           reference=ref,phase_binding_raw=raw,executable=after['binary']['path'])
        return commands,selected
    R.keys(roles,('result',))
    R.need(before['binary'] is None and after['binary'] is None,'b04_prior_binary_shape')
    if phase=='format':
        expected={item['root']+'/'+item['relative']:item['sha256'] for item in data['files']}
        exact(before['_hashes'],expected)
        R.need(set(after['_hashes'])==set(expected),'b04_prior_format_sources')
        outcome=W.validate_format(captures[0]['log'],exit_code=0)
    else:
        manifest=W._manifest(evidence_bytes(data['source_manifest'],inventory),data)
        if phase=='gate':
            R.need(manifest['files'].get('scripts/gate')==data['gate_sha256'],'b04_prior_gate_source')
        exact(before['_hashes'],manifest['files']);exact(after['_hashes'],manifest['files'])
        if phase=='gate':
            outcome=W.validate_gate(captures[0]['log'],data=data,exit_code=0,reference=ref,phase_binding_raw=raw)
        else:
            outcome=W.validate_cargo_tests(captures[0]['log'],data=data,exit_code=0,reference=ref,phase_binding_raw=raw)
    exact(evidence_json(roles['result'],inventory),outcome)
    commands=W.commands(phase,data,{'cargo':binding['cargo'],'rustfmt':binding['rustfmt']},
                        reference=ref,phase_binding_raw=raw)
    return commands,outcome


def consume_prior_phase(record, captures, inventory):
    reference=read_json(record['reference'])
    binding,raw=prerequisite_binding(record,reference,inventory)
    evidence=evidence_json(record['semantic_evidence'],inventory)
    R.keys(evidence,('schema_version','phase','program_sha256','binding_sha256','before','after','roles'))
    R.need(evidence['schema_version']=='pbdep.prior-semantic.v1'
        and evidence['phase']==record['phase'] and evidence['program_sha256']==record['program_sha256']
        and evidence['binding_sha256']==record['phase_binding']['sha256'],'b04_prior_semantic')
    before,hashes=retained_observation(evidence['before'],record,binding,reference,inventory);before['_hashes']=hashes
    after,hashes=retained_observation(evidence['after'],record,binding,reference,inventory);after['_hashes']=hashes
    if record['phase']=='qualification':
        R.need(W.ns(captures[0]['capture']['start_monotonic_ns'])
            <= W.ns(before['observed_ns']) <= W.ns(after['observed_ns'])
            <= W.ns(captures[0]['capture']['end_monotonic_ns']),'b04_prior_observation_time')
    else:
        R.need(W.ns(record['phase_started_monotonic_ns']) <= W.ns(before['observed_ns'])
            <= W.ns(captures[0]['start']['recorded_monotonic_ns'])
            and W.ns(captures[-1]['capture']['end_monotonic_ns']) <= W.ns(after['observed_ns'])
            <= W.ns(record['phase_finished_monotonic_ns']),'b04_prior_observation_time')
    record=deepcopy(record);record['_reference']=reference;record['_binding_raw']=raw
    if record['phase']=='qualification':
        commands,outcome=consume_q(record,binding,evidence,captures,before,after,inventory)
    elif record['phase']=='a2':
        commands,outcome=consume_a2(record,binding,evidence,captures,before,after,inventory)
    else:
        commands,outcome=consume_later(record,binding,evidence,captures,before,after,inventory)
    verify_prior_commands(binding,captures,commands,before)
    return outcome


def master_interval(reservation):
    """Pure anchored bounds; no new anchor, clock read or reservation."""
    anchor = reservation['anchor']
    R.keys(anchor, ('wall_ns', 'monotonic_ns', 'boot_id'))
    wall, mono = W.ns(anchor['wall_ns']), W.ns(anchor['monotonic_ns'])
    start = integer(reservation['master_start_utc_seconds'], 10**12)
    expiry = integer(reservation['master_expiry_utc_seconds'], 10**12)
    R.need(0 < start < expiry and expiry - start <= 36 * 3600
        and start * NANO <= wall < expiry * NANO, 'b04_history_master_anchor')
    return mono, mono + expiry * NANO - wall


def chronological_interval(begin, end, *, reservation, observed_entry_ns):
    """Historical interval must precede an explicit observed entry boundary."""
    lower, upper = master_interval(reservation)
    integer(observed_entry_ns, 10**24 - 1)
    integer(begin, 10**24 - 1)
    integer(end, 10**24 - 1)
    R.need(lower <= begin <= end <= observed_entry_ns <= upper,
           'b04_history_master_interval')


def prior_history(pin, *, reservation, state_spec, observed_entry_ns, discovery_inventory=None):
    """Authenticated complete lineage; strict joins do not grant its authority."""
    value = read_json(pin)
    R.keys(value, ('schema_version', 'kind', 'program_sha256', 'chain_id',
        'reservation_sha256', 'state_directory', 'resources', 'terminal_captures',
        'inventory', 'phase_history'))
    R.need(value['schema_version'] == 'pbdep.b04-prior.v2'
        and value['program_sha256'] == reservation['program_sha256']
        and value['chain_id'] == reservation['chain_id']
        and value['reservation_sha256'] == R.digest(R.encode(reservation))
        and value['state_directory'] == state_spec, 'b04_prior_join')
    chronological_interval(observed_entry_ns, observed_entry_ns,
        reservation=reservation, observed_entry_ns=observed_entry_ns)
    previous_phase_end = master_interval(reservation)[0]
    resources = validate_history(value['resources'])
    live = state_inventory(state_spec)
    if discovery_inventory is None:
        R.need(live == value['inventory'], 'b04_prior_inventory')
    else:
        R.need(live == discovery_inventory and value['inventory']['state_directory'] == state_spec
            and all(live['files'].get(k) == v for k, v in value['inventory']['files'].items())
            and all(live['directories'].get(k) == v for k, v in value['inventory']['directories'].items()),
            'b04_prior_inventory')
    captures = value['terminal_captures']
    R.need(type(captures) is list and len(captures) <= 16, 'b04_prior_inventory')
    phases = value['phase_history']
    R.need(type(phases) is list and len(phases) <= 8, 'b04_prior_phases')
    phase_resources = None
    phase_policies = {}
    phase_records = {}
    for index, item in enumerate(phases):
        R.keys(item, ('phase', 'record'))
        R.need(item['phase'] == tuple(W.PHASES)[index], 'b04_prior_phase_order')
        retained_pin(item['record'], value['inventory'])
        record = read_json(item['record'])
        R.keys(record, ('schema_version', 'phase', 'program_sha256', 'reservation_sha256',
            'master_anchor', 'master_expiry_utc_seconds', 'state_directory',
            'predecessor_sha256', 'status', 'artifacts', 'resources',
            'policy_reservation', 'phase_started_monotonic_ns', 'phase_finished_monotonic_ns',
            'elapsed_ns', 'prep_used_ns', 'close_used_ns', 'child_wrapper_ns', 'spent', 'reference', 'phase_binding', 'semantic_evidence'))
        R.need(record['schema_version'] == 'pbdep.b04-external-phase.v2'
            and record['phase'] == item['phase'] and record['status'] == 'TERMINAL'
            and record['program_sha256'] == reservation['program_sha256']
            and record['reservation_sha256'] == value['reservation_sha256']
            and record['master_anchor'] == reservation['anchor']
            and record['master_expiry_utc_seconds'] == reservation['master_expiry_utc_seconds']
            and record['state_directory'] == state_spec
            and record['predecessor_sha256'] == (phases[index-1]['record']['sha256']
                                               if index else reservation['program_sha256']),
            'b04_prior_phase_join')
        phase_begin = W.ns(record['phase_started_monotonic_ns'])
        phase_end = W.ns(record['phase_finished_monotonic_ns'])
        chronological_interval(phase_begin, phase_end,
            reservation=reservation, observed_entry_ns=observed_entry_ns)
        R.need(previous_phase_end <= phase_begin, 'b04_prior_phase_overlap')
        previous_phase_end = phase_end
        for control in ('reference', 'phase_binding', 'semantic_evidence'):
            retained_pin(record[control], value['inventory'])
        old_reference = read_json(record['reference'])
        W.validate_reference(old_reference)
        R.need(W.binding_pin(old_reference['operator'], item['phase'], reference=old_reference)
            == record['phase_binding'], 'b04_prior_binding')
        if index:
            preceding = phase_records[phases[index-1]['phase']]
            previous_reference = read_json(preceding['reference'])
            W.validate_continuation(previous_reference['operator'], old_reference['operator'],
                tuple(p['phase'] for p in phases[:index]), previous_reference=previous_reference,
                reference=old_reference)
        retained_pin(record['policy_reservation'], value['inventory'])
        policy_raw = read_json(record['policy_reservation'])
        R.keys(policy_raw, ('schema_version', 'program_sha256', 'master_start_utc_seconds',
            'master_expiry_utc_seconds', 'exit_seconds', 'slots', 'phases'))
        policy_reservation = deepcopy(policy_raw)
        R.keys(policy_raw['phases'], P.ORDER)
        for name in P.ORDER:
            R.keys(policy_raw['phases'][name], ('slots', 'prep', 'close', 'total'))
            slots = policy_raw['phases'][name]['slots']
            R.need(type(slots) is list, 'b04_prior_policy_encoding')
            policy_reservation['phases'][name]['slots'] = tuple(slots)
        P.validate_reservation(policy_reservation)
        R.need(policy_reservation['program_sha256'] == reservation['program_sha256']
            and policy_reservation['master_start_utc_seconds'] == reservation['master_start_utc_seconds']
            and policy_reservation['master_expiry_utc_seconds'] == reservation['master_expiry_utc_seconds'],
            'b04_prior_policy_master')
        elapsed = W.ns(record['elapsed_ns'])
        R.need(type(record['spent']) is list
            and W.ns(record['phase_finished_monotonic_ns']) - W.ns(record['phase_started_monotonic_ns']) == elapsed
            and sum(W.ns(record[k]) for k in ('prep_used_ns', 'close_used_ns', 'child_wrapper_ns')) == elapsed,
            'b04_prior_accounting')
        phase_policies[item['phase']] = policy_reservation
        phase_records[item['phase']] = record
        if phase_resources is not None:
            history_extends(phase_resources, record['resources'])
        phase_resources = record['resources']
        P.close_phase(item['phase'], elapsed_phase_ns=elapsed, spent=tuple(record['spent']),
            prep_used_ns=W.ns(record['prep_used_ns']), close_used_ns=W.ns(record['close_used_ns']),
            reservation=policy_reservation)
        R.need(type(record['artifacts']) is list and 1 <= len(record['artifacts']) <= 256,
               'b04_prior_phase_inventory')
        for artifact in record['artifacts']:
            retained_pin(artifact, value['inventory'])
        history_extends(record['resources'], resources)
    if value['kind'] == 'GENESIS':
        R.need(captures == [] and phases == [] and resources['sample_count'] == 0
            and value['inventory']['files'] == {} and value['inventory']['directories'] == {},
            'b04_prior_genesis')
    else:
        R.need(value['kind'] == 'TERMINAL' and captures, 'b04_prior_nonterminal')
        previous = None
        identities = set()
        phase_captures = {name: [] for name in phase_policies}
        wrapper_totals = {name: 0 for name in phase_policies}
        declared_order = [(phase, slot) for phase, phase_policy in phase_policies.items()
            for slot in (('Q',) if phase == 'qualification'
                         else phase_policy['phases'][phase]['slots'])]
        R.need(len(captures) == len(declared_order), 'b04_prior_capture_inventory')
        previous_capture_end = master_interval(reservation)[0]
        for capture_index, spec in enumerate(captures):
            for artifact in spec.values():
                retained_pin(artifact, value['inventory'])
            start, raw = prior_capture(spec)
            identity = (start['phase'], start['slot'])
            R.need(identity == declared_order[capture_index], 'b04_prior_capture_sequence')
            R.need(identity not in identities
                and start['phase'] in [p['phase'] for p in phases], 'b04_prior_attempt_join')
            identities.add(identity)
            policy_reservation = phase_policies[start['phase']]
            phase = start['phase']
            declared = ('Q',) if phase == 'qualification' else policy_reservation['phases'][phase]['slots']
            limit = (policy_reservation['phases'][phase]['total'] - 60 if phase == 'qualification'
                     else policy_reservation['slots'].get(start['slot'], 0))
            R.need(start['slot'] in declared and type(start['useful_seconds']) is int
                and 0 < start['useful_seconds'] <= limit
                and type(start['exit_seconds']) is int and start['exit_seconds'] == 60,
                'b04_prior_slot_join')
            record = phase_records[phase]
            prior_reference = read_json(record['reference'])
            R.need(start['reference_sha256'] == R.digest(R.encode(prior_reference))
                and start['binding_sha256'] == record['phase_binding']['sha256'],
                'b04_prior_capture_binding')
            history_extends(raw['resource_samples'], record['resources'])
            before = W.ns(raw['wrapper_before_run_monotonic_ns'])
            after = W.ns(raw['wrapper_after_run_monotonic_ns'])
            start_tick = W.ns(start['recorded_monotonic_ns'])
            chronological_interval(start_tick, after,
                reservation=reservation, observed_entry_ns=observed_entry_ns)
            R.need(W.ns(record['phase_started_monotonic_ns']) <= start_tick <= before <= after
                <= W.ns(record['phase_finished_monotonic_ns']), 'b04_prior_capture_order')
            R.need(previous_capture_end <= start_tick, 'b04_prior_capture_overlap')
            previous_capture_end = after
            log = evidence_bytes(spec['log'],value['inventory'],start['raw_cap'])
            capture = R.normalize_capture(raw['supervisor'],log,before,after,
                101 if phase=='a2' and start['slot']=='3' else 0)
            phase_captures[phase].append({'spec':spec,'start':start,'raw':raw,'log':log,'capture':capture})
            wrapper_totals[phase] += after - before
            for artifact in spec.values():
                R.need(artifact in record['artifacts'], 'b04_prior_phase_capture_join')
            if previous is not None:
                history_extends(previous, raw['resource_samples'])
            previous = raw['resource_samples']
        expected = {(phase, slot) for phase, reservation_value in phase_policies.items()
            for slot in (('Q',) if phase == 'qualification'
                         else reservation_value['phases'][phase]['slots'])}
        R.need(identities == expected, 'b04_prior_capture_inventory')
        for phase, total in wrapper_totals.items():
            R.need(total == W.ns(phase_records[phase]['child_wrapper_ns']), 'b04_prior_child_accounting')
        history_extends(previous, resources)
        for phase in phase_records:
            consume_prior_phase(phase_records[phase],phase_captures[phase],value['inventory'])
    return value


def retained_pin(pin, inventory):
    checked_pin(pin)
    root = Path(inventory['state_directory']['path'])
    path = Path(pin['path'])
    R.need(root in path.parents, 'b04_retained_outside_state')
    relative = str(path.relative_to(root))
    R.need(relative in inventory['files']
        and inventory['files'][relative]['sha256'] == pin['sha256'], 'b04_retained_pin')
    cap = artifact_cap(path.name)
    R.pinned(pin, cap)


class Clock:
    """Fresh observations against one explicit, non-pausing master anchor."""
    def __init__(self, reservation, stage, started):
        R.keys(reservation, ('schema_version', 'program_sha256', 'chain_id',
            'allocation_ids', 'anchor', 'master_start_utc_seconds',
            'master_expiry_utc_seconds', 'exit_seconds', 'discovery', 'witness',
            'review_seconds', 'return_seconds', 'later_seconds'))
        R.need(reservation['schema_version'] == 'pbdep.b04-reservation.v1'
            and stage in ('discovery', 'witness'), 'b04_reservation_version')
        for key in ('program_sha256', 'chain_id'):
            W.hash_value(reservation[key])
        ids = reservation['allocation_ids']
        R.keys(ids, ATTEMPTS)
        for value in ids.values():
            W.hash_value(value)
        R.need(len(set(ids.values())) == 3, 'b04_allocation_alias')
        start = integer(reservation['master_start_utc_seconds'], 10**12)
        expiry = integer(reservation['master_expiry_utc_seconds'], 10**12)
        R.need(0 < start < expiry and expiry - start <= 36 * 3600,
               'b04_master_window')
        R.need(type(reservation['exit_seconds']) is int
            and reservation['exit_seconds'] == 60, 'b04_exit_reserve')
        for name, attempts in (('discovery', ('compile', 'list')), ('witness', ('witness',))):
            part = reservation[name]
            R.keys(part, ('prep', 'close', 'total', *attempts))
            for key in ('prep', 'close'):
                R.need(0 < integer(part[key], 150), 'b04_stage_budget')
            for key in attempts:
                R.need(0 < integer(part[key], CEILINGS[key]), 'b04_useful_budget')
            R.need(type(part['total']) is int and part['total'] ==
                part['prep'] + part['close'] + sum(part[k] + 60 for k in attempts),
                'b04_stage_total')
        R.need(0 < integer(reservation['review_seconds'], 36 * 3600)
            and 60 <= integer(reservation['return_seconds'], 600), 'b04_return_reserve')
        required = sum(reservation[k]['total'] for k in ('discovery', 'witness'))
        integer(reservation['later_seconds'], 36 * 3600)
        required += reservation['review_seconds'] + reservation['return_seconds'] + reservation['later_seconds']
        R.need(required <= expiry - start, 'b04_master_reserve')
        anchor = reservation['anchor']
        R.keys(anchor, ('wall_ns', 'monotonic_ns', 'boot_id'))
        aw, am = W.ns(anchor['wall_ns']), W.ns(anchor['monotonic_ns'])
        R.need(type(anchor['boot_id']) is str and anchor['boot_id'] == R.boot_id()
            and start * NANO <= aw < expiry * NANO, 'b04_anchor')
        self.reservation = deepcopy(reservation)
        self.stage, self.started, self.child_ns = stage, started, 0
        self.close_started = None
        self.running_since = None
        self.aw, self.am = aw, am
        self.remaining()

    def remaining(self):
        wall, mono = time.time_ns(), time.monotonic_ns()
        R.need(R.boot_id() == self.reservation['anchor']['boot_id']
            and mono >= self.am
            and abs(wall - self.aw - (mono - self.am)) <= 2 * NANO, 'b04_clock_drift')
        expiry = self.reservation['master_expiry_utc_seconds'] * NANO
        left = min(expiry - wall, self.am + expiry - self.aw - mono)
        R.need(left > 0, 'b04_master_expired')
        return left

    def check(self):
        now = time.monotonic_ns()
        part = self.reservation[self.stage]
        elapsed = now - self.started
        R.need(0 <= elapsed <= part['total'] * NANO, 'b04_stage_expired')
        prep_end = now if self.close_started is None else self.close_started
        running = 0 if self.running_since is None else now - self.running_since
        prep = prep_end - self.started - self.child_ns - running
        R.need(0 <= prep <= part['prep'] * NANO, 'b04_prep_expired')
        if self.close_started is not None:
            R.need(now - self.close_started <= part['close'] * NANO, 'b04_close_expired')
        later = self.reservation['return_seconds'] + self.reservation['later_seconds']
        if self.stage == 'discovery':
            later += self.reservation['review_seconds'] + self.reservation['witness']['total']
        R.need(self.remaining() >= later * NANO, 'b04_return_exhausted')
        return now, prep

    def admit(self, attempt, spent):
        expected = ('compile', 'list') if self.stage == 'discovery' else ('witness',)
        R.need(type(spent) is tuple and spent == expected[:len(spent)]
            and len(spent) < len(expected) and attempt == expected[len(spent)],
            'b04_attempt_replay')
        now, prep = self.check()
        part = self.reservation[self.stage]
        required = sum(part[k] + 60 for k in expected[len(spent):])
        required += part['prep'] + part['close']
        required_ns = required * NANO - prep
        R.need(part['total'] * NANO - (now - self.started) >= required_ns,
               'b04_stage_reserves')
        later = self.reservation['return_seconds'] + self.reservation['later_seconds']
        if self.stage == 'discovery':
            later += self.reservation['review_seconds'] + self.reservation['witness']['total']
        R.need(self.remaining() >= required_ns + later * NANO, 'b04_master_reserves')
        return part[attempt], 60


def source_snapshot(request):
    raw = R.pinned(request['source_manifest'])
    manifest = R.decode(raw)
    R.keys(manifest, ('schema_version', 'subject_head', 'source_root', 'files'))
    R.need(manifest['schema_version'] == 'phase-b.source-manifest.v1'
        and manifest['subject_head'] == request['subject_head']
        and manifest['source_root'] == request['source_root'], 'b04_manifest_join')
    W.source_map(manifest['files'])
    root = R.path_check(request['source_root'])
    before = R.source_inventory(str(root), excluded_target=str(root / 'target'))
    W.validate_source_inventory(before['files'], manifest['files'])
    files = {name: R.file_observation(root / name, R.SOURCE_CAP)[0]
             for name in manifest['files']}
    R.need(all(files[k]['sha256'] == v for k, v in manifest['files'].items()),
           'b04_source_drift')
    # Record even empty/cancelled-component directories before compilation.
    started = time.monotonic_ns()
    todo, directories, count, path_bytes = [root], {}, 0, 0
    while todo:
        current = todo.pop()
        with os.scandir(current) as items:
            for item in items:
                count += 1
                path_bytes += len(os.fsencode(item.path))
                R.need(count <= 4096 and path_bytes <= 16 * R.MIB
                    and time.monotonic_ns() - started <= R.SCAN_NS, 'b04_source_bound')
                info = item.stat(follow_symlinks=False)
                R.need(not stat.S_ISLNK(info.st_mode), 'b04_source_symlink')
                if stat.S_ISDIR(info.st_mode) and item.path != str(root / 'target'):
                    path = Path(item.path)
                    directories[str(path.relative_to(root))] = {
                        'path': str(path), 'dev': str(info.st_dev), 'ino': str(info.st_ino),
                        'mode': str(stat.S_IMODE(info.st_mode)), 'uid': str(info.st_uid)}
                    todo.append(path)
    R.need(R.source_inventory(str(root), excluded_target=str(root / 'target')) == before,
           'b04_source_inventory_drift')
    R.need(before['git'] in request['immutable_files'], 'b04_git_closure')
    return raw, manifest, {'files': files, 'directories': directories, 'git': before['git']}


def proposed_dependencies(depraw, executable, manifest, manifest_pin):
    text = W.text(depraw, 512 * 1024)
    prefix = executable + '.d: '
    R.need(text.startswith(prefix) and '\n' in text, 'b04_depfile')
    tokens = text.split('\n', 1)[0][len(prefix):].split(' ')
    R.need(1 <= len(tokens) <= 2048, 'b04_dependency_count')
    bindings = []
    for token in tokens:
        name = W.dependency_name(token)
        R.need(name in manifest['files'], 'b04_dependency_membership')
        bindings.append({'raw_token': token, 'canonical_relative': name,
                         'source_sha256': manifest['files'][name]})
    data = {'source_root': manifest['source_root'], 'subject_head': manifest['subject_head'],
        'manifest_sha256': manifest_pin['sha256'], 'dep_tokens': tokens,
        'dep_binding_schema': 'phase-b.dep-binding.v1', 'dep_bindings': bindings}
    W.validate_dependency_artifact(depraw, source_root=manifest['source_root'],
        executable=executable, dep_tokens=tokens, dep_binding_schema=data['dep_binding_schema'],
        dep_bindings=bindings, source_hashes=manifest['files'])
    return data


def proposed_roster(raw):
    lines = W.text(raw, 1024 * 1024)
    R.need(lines.endswith('\n'), 'b04_listing_frame')
    lines = lines[:-1].split('\n')
    R.need(all(line.endswith(': test') for line in lines), 'b04_listing_frame')
    # Complete measured ordering; no expected count/map/selected verdict.
    return W.names([line[:-6] for line in lines])


def observe_proposed_dependencies(manifest, data, *, expected=None):
    """Bounded no-follow raw component walk, including cancelled directories.

    Descriptor/path stability and repeated observations are trusted-local
    evidence, not hostile-host or atomic filesystem containment.
    """
    R.keys(manifest, ('schema_version', 'source_root', 'subject_head', 'files'))
    R.need(manifest['schema_version'] == 'phase-b.source-manifest.v1'
         and manifest['source_root'] == data['source_root']
         and manifest['subject_head'] == data['subject_head'], 'dependency_source_join')
    mapping = W.validate_dependency_bindings(data['dep_tokens'], data['dep_binding_schema'],
        data['dep_bindings'], sources=manifest['files'])
    root = R.path_check(data['source_root'])
    opened = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    started = time.monotonic_ns()
    directories = {}
    files = {}
    work = 0

    def directory_record(pathname, observed):
        R.need(stat.S_ISDIR(observed.st_mode), 'dependency_directory')
        return {'path': pathname, 'dev': str(observed.st_dev), 'ino': str(observed.st_ino),
                'mode': str(stat.S_IMODE(observed.st_mode)), 'uid': str(observed.st_uid)}

    try:
        root_before = os.fstat(opened)
        R.need(R.stamp(root_before) == R.stamp(root.lstat()), 'dependency_root_drift')
        root_record = directory_record(str(root), root_before)
        for token, binding in zip(data['dep_tokens'], data['dep_bindings']):
            descriptors = [os.dup(opened)]
            stack = []
            held = []
            try:
                parts = token.split('/')
                # A final '..' denotes a directory, not a regular source input.
                R.need(parts[-1] != '..', 'dependency_terminal')
                for part in parts[:-1]:
                    work += 1
                    R.need(work <= 65536 and time.monotonic_ns() - started < R.SCAN_NS,
                         'dependency_walk_bound')
                    if part == '..':
                        R.need(len(descriptors) > 1, 'dependency_escape')
                        os.close(descriptors.pop())
                        stack.pop()
                        continue
                    try:
                        before = os.stat(part, dir_fd=descriptors[-1], follow_symlinks=False)
                    except FileNotFoundError as error:
                        # Only the expected intermediate lookup's ENOENT is a
                        # missing-component contract refusal, not arbitrary IO.
                        if error.errno != errno.ENOENT:
                            raise
                        parent_name = str(root.joinpath(*stack))
                        parent_info = os.fstat(descriptors[-1])
                        R.need(error.filename == part and stat.S_ISDIR(parent_info.st_mode)
                             and R.stamp(parent_info) == R.stamp(Path(parent_name).lstat()),
                             'dependency_missing_site')
                        raise R.Stop('dependency_missing_component',
                            provenance='verified-missing-component', refusal_context={
                                'raw_token': token, 'component': part, 'parent_path': parent_name,
                                'parent_dev': str(parent_info.st_dev), 'parent_ino': str(parent_info.st_ino),
                                'errno': errno.ENOENT}) from None
                    R.need(stat.S_ISDIR(before.st_mode), 'dependency_directory')
                    fd = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC,
                                 dir_fd=descriptors[-1])
                    try:
                        R.need(R.stamp(os.fstat(fd)) == R.stamp(before), 'dependency_directory_drift')
                        held.append((os.dup(descriptors[-1]), part, before))
                        stack.append(part)
                        name = '/'.join(stack)
                        record = directory_record(str(root) + '/' + name, before)
                        R.need(name not in directories or directories[name] == record,
                             'dependency_directory_drift')
                        directories[name] = record
                        descriptors.append(fd)
                    except BaseException:
                        os.close(fd)
                        raise
                before = os.stat(parts[-1], dir_fd=descriptors[-1], follow_symlinks=False)
                R.need(stat.S_ISREG(before.st_mode) and before.st_nlink == 1
                     and 0 <= before.st_size <= R.SOURCE_CAP, 'dependency_terminal')
                fd = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC,
                             dir_fd=descriptors[-1])
                try:
                    R.need(R.stamp(os.fstat(fd)) == R.stamp(before), 'dependency_file_drift')
                    hasher = hashlib.sha256()
                    count = 0
                    while True:
                        R.need(time.monotonic_ns() - started < R.SCAN_NS, 'dependency_walk_bound')
                        chunk = os.read(fd, min(R.MIB, R.SOURCE_CAP + 1 - count))
                        if not chunk:
                            break
                        count += len(chunk)
                        R.need(count <= R.SOURCE_CAP, 'dependency_file_bound')
                        hasher.update(chunk)
                    R.need(R.stamp(before) == R.stamp(os.fstat(fd))
                         == R.stamp(os.stat(parts[-1], dir_fd=descriptors[-1], follow_symlinks=False)),
                         'dependency_file_drift')
                    canonical = root / binding['canonical_relative']
                    canonical_record = R.file_observation(canonical, R.SOURCE_CAP)[0]
                    R.need(R.stamp(before) == R.stamp(canonical.lstat())
                         and hasher.hexdigest() == canonical_record['sha256'] == binding['source_sha256'],
                         'dependency_canonical_join')
                    files[binding['canonical_relative']] = canonical_record
                finally:
                    os.close(fd)
                for parent_fd, name, before in held:
                    R.need(R.stamp(before) == R.stamp(os.stat(name, dir_fd=parent_fd, follow_symlinks=False)),
                         'dependency_directory_drift')
            finally:
                for fd in descriptors:
                    os.close(fd)
                for fd, _, _ in held:
                    os.close(fd)
        R.need(R.stamp(root_before) == R.stamp(os.fstat(opened)) == R.stamp(root.lstat()), 'dependency_root_drift')
        result = {'schema_version': 'phase-b.dep-observation.v1', 'source_root': root_record,
            'subject_head': data['subject_head'], 'manifest_sha256': data['manifest_sha256'],
            'dep_binding_schema': data['dep_binding_schema'],
            'dep_bindings_sha256': mapping['dep_bindings_sha256'], 'directories': directories, 'files': files}
        W.validate_dependency_observation(result, data=data, sources=manifest['files'])
        R.need(expected is None or result == expected, 'dependency_observation_drift')
        return result
    except OSError:
        raise R.Stop('dependency_walk_io', provenance='io-exception') from None
    finally:
        os.close(opened)


class Custody:
    """Only named B04 attempts, one state lock and cumulative sampled evidence."""
    def __init__(self, request, reference, reservation, stage, started, history,
                 request_pin, common_pin, grant_pin, reference_pin, inventory):
        self.request, self.reference = deepcopy(request), deepcopy(reference)
        self.clock = Clock(reservation, stage, started)
        self.state = R.directory(request['state_directory'])
        self.stage = stage
        self.root = self.state / stage
        self.request_pin = deepcopy(request_pin)
        self.request_raw = R.pinned(request_pin)
        self.common_pin = deepcopy(common_pin)
        self.grant_pin = deepcopy(grant_pin)
        self.reference_pin = deepcopy(reference_pin)
        self.inventory = deepcopy(inventory)
        self.armed = False
        self.reservation_pin = deepcopy(request['reservation'])
        self.history = deepcopy(validate_history(history))
        self.last_scan = None
        self.spent = []
        self.lock = None

    def refresh(self):
        q, ref = self.request, self.reference
        R.require_umask()
        R.need(not (self.state / 'STOP.json').exists(), 'b04_stopped')
        R.need(R.pinned(self.common_pin) == R.encode(q)
            and R.pinned(self.request_pin) == self.request_raw, 'b04_request_changed')
        # Entry requires canonical request bytes; freeze bytes need not be canonical.
        R.need(len(R.pinned(q['grant'])) > 0 and len(R.pinned(self.grant_pin)) > 0,
               'b04_grant_empty')
        for control in ('reference', 'reservation', 'prior', 'source_manifest'):
            R.pinned(q[control])
        R.need(R.decode(R.pinned(self.reference_pin)) == ref, 'b04_reference_drift')
        R.need(R.decode(R.pinned(q['reservation'])) == self.clock.reservation,
               'b04_reservation_drift')
        W.validate_reference(ref)
        R.verify_file_pins([ref['operator'][r] for r in
                           ('authority', 'packet', 'acceptance', 'review', 'disposition')])
        R.keys(q['module_paths'], ref['operator']['instrument_sha256'])
        R.verify_file_pins([{'path': q['module_paths'][k], 'sha256': v}
                           for k, v in ref['operator']['instrument_sha256'].items()])
        R.need(type(q['immutable_files']) is list and 1 <= len(q['immutable_files']) <= 4096,
               'b04_closure')
        paths = []
        for spec in q['immutable_files']:
            actual, _ = R.file_observation(spec['path'], R.GIB, installed_tool=True, reference=ref)
            R.need(actual == spec, 'b04_tool_drift')
            paths.append(spec['path'])
        R.need(len(paths) == len(set(paths)), 'b04_tool_alias')
        for role in ('python', 'cargo', 'rustfmt'):
            R.need(q[role] in paths, 'b04_tool_unbound')
            R.file_observation(q[role], R.GIB, executable=True, installed_tool=True, reference=ref)
        R.verify_selection_inputs(q['selection_inputs'], q['immutable_files'])
        env = q['environment']
        cleared_environment(env, q, phase=self.stage)
        roots = ref['operator']['resource_roots']
        for key in ('source_directory', 'target_directory', 'tmp'):
            R.need(q[key] in [r['directory'] for r in roots], 'b04_resource_join')
            R.directory(q[key], short=key == 'tmp')
        R.need(q['source_directory']['path'] == q['source_root']
            and q['target_directory']['path'] == q['target_root']
            and env.get('CARGO_TARGET_DIR') == q['target_root']
            and env.get('TMPDIR') == q['tmp']['path'], 'b04_root_join')
        R.source_resource_join('base', q, [r['directory']['path'] for r in roots])
        temporary = tuple(r['directory']['path'] for r in roots if r['kind'] == 'temporary')
        R.checked_temporary_roots(temporary)
        R.need(q['tmp']['path'] in temporary, 'b04_temporary_join')
        R.directory(q['state_directory'])
        self.clock.check()

    def verify_retained(self):
        actual = state_inventory(self.request['state_directory'])
        R.need(actual == self.inventory, 'b04_retained_inventory_drift')

    def remember(self, path):
        relative = str(path.relative_to(self.state))
        self.inventory['files'][relative] = R.file_observation(
            path, artifact_cap(path.name))[0]
        self.inventory['total_bytes'] = sum(int(v['size']) for v in self.inventory['files'].values())

    def enter(self):
        self.refresh()
        self.clock.check()
        fd = os.open(self.state / 'LOCK', os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
        try:
            info = os.fstat(fd)
            R.need(stat.S_ISREG(info.st_mode) and info.st_nlink == 1
                and info.st_uid == os.getuid() and stat.S_IMODE(info.st_mode) == 0o600,
                'b04_lock_identity')
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.lock = fd
            R.need(not self.root.exists(), 'b04_stage_replay')
            self.verify_retained()
            self.resources(initial=True)
            self.clock.check()
            identity = R.own_directory(self.root)
            self.inventory['directories'][self.stage] = identity
            self.armed = True
        except BaseException:
            if self.lock is None:
                os.close(fd)
            raise

    def release(self):
        if self.lock is not None:
            os.close(self.lock)
            self.lock = None

    def stop(self):
        # Emergency terminal evidence is permitted after expiry; never retries.
        R.persist(self.state / 'STOP.json', {
            'schema_version': 'pbdep.b04-stop.v1', 'status': 'STOP_NO_RETRY',
            'chain_id': self.request['chain_id'], 'stage': self.stage,
            'spent': self.spent, 'reason': 'b04_admitted_operation_failed',
            'physical_state': 'SEE_RAW_OR_UNKNOWN', **AXES,
            'resources': self.history, 'retry_authorized': False}, state_root=self.state)

    def persist(self, name, value):
        self.clock.check()
        raw = R.persist(self.root / name, value, state_root=self.state)
        self.remember(self.root / name)
        return raw

    def retain_depfile(self, raw):
        R.need(type(raw) is bytes and 0 < len(raw) <= 512 * 1024, 'b04_depfile_cap')
        self.clock.check()
        R.retention_budget(R.retained_bytes(self.state), len(raw), False)
        path = self.root / 'DEPFILE.d'
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
        try:
            with os.fdopen(fd, 'wb', closefd=False) as stream:
                stream.write(raw)
                stream.flush()
            os.fsync(fd)
        finally:
            os.close(fd)
        R.sync_directory(path.parent)
        R.need(R.file_observation(path, 512 * 1024, retain=True)[1] == raw, 'b04_depfile_readback')
        self.remember(path)
        return pin_of(path, 512 * 1024)

    def persist_raw(self, name, raw):
        R.need(name in ('OPERATOR.json', 'PHASE-BINDING.json'), 'b04_control_name')
        self.clock.check()
        R.persist_bytes(self.root / name, raw, state_root=self.state)
        self.remember(self.root / name)

    def resources(self, *, initial=False):
        roots = self.reference['operator']['resource_roots']
        temporary = tuple(r['directory']['path'] for r in roots if r['kind'] == 'temporary')
        facts = R.scan_resources(roots, self.request['state_directory'],
                                initial=initial, temporary_roots=temporary)
        r = self.history
        r['sample_count'] += 1
        R.need(r['sample_count'] <= 8192, 'b04_history_cap')
        for name in ('total_bytes', 'temporary_bytes', 'capture_bytes'):
            r['peak_' + name] = max(r['peak_' + name], facts[name])
        R.need(len(r['vanished']) + facts['vanished_count'] <= 128, 'b04_history_cap')
        r['vanished'].extend(facts['vanished'])
        for name in r['vanished_by_class']:
            r['vanished_by_class'][name] += facts['vanished_by_class'][name]
        for name in r['sampled_entry_types']:
            r['sampled_entry_types'][name] += facts['sampled_entry_types'][name]
        self.last_scan = facts
        self.clock.check()
        return facts['total_bytes']

    def capture(self, attempt, argv, stable):
        """stable is an internal fixed B04 observer, never a caller/provider callback."""
        R.need(self.lock is not None, 'b04_lock_unbound')
        self.refresh()
        stable()
        self.resources(initial=True)
        useful, grace = self.clock.admit(attempt, tuple(self.spent))
        expected = compile_command(self.request) if attempt == 'compile' else None
        if expected is not None:
            R.need(tuple(argv) == expected, 'b04_command')
        elif attempt == 'list':
            R.need(len(argv) == 4 and tuple(argv[1:]) == ('--list', '--format', 'terse'),
                   'b04_command')
        else:
            R.need(attempt == 'witness' and len(argv) == 4
                and tuple(argv[1:]) == (W.PREFIX, '--nocapture', '--test-threads=1'),
                'b04_command')
        cap = min(self.request['raw_cap'], R.capture_budget(R.retained_bytes(self.state)))
        start = {'schema_version': 'pbdep.b04-start.v1', 'attempt': attempt,
            'chain_id': self.request['chain_id'], 'program_sha256': self.clock.reservation['program_sha256'],
            'allocation_id': self.clock.reservation['allocation_ids'][attempt],
            'reservation_sha256': self.reservation_pin['sha256'],
            'request_sha256': self.request_pin['sha256'], 'argv': list(argv),
            'cwd': self.request['source_root'], 'environment_sha256': R.digest(R.encode(self.request['environment'])),
            'raw_cap': cap, 'useful_seconds': useful, 'exit_seconds': grace,
            'recorded_monotonic_ns': str(time.monotonic_ns()), 'slot_spent_even_if_fork_fails': True}
        spent_before = tuple(self.spent)
        self.verify_retained()
        self.clock.admit(attempt, spent_before)
        self.spent.append(attempt)  # A failed START/fork is spent; stage cannot retry.
        R.persist(self.root / (attempt + '-START.json'), start, state_root=self.state)
        self.remember(self.root / (attempt + '-START.json'))
        self.refresh()
        stable()
        # Re-observe the entire lineage, including START, after slow checks.
        self.verify_retained()
        # Fresh admission follows the final complete inventory observation.
        self.clock.admit(attempt, spent_before)
        before = time.monotonic_ns()
        self.clock.running_since = before
        receipt = supervise.run(argv, self.request['source_root'], self.request['environment'],
            str(self.root / (attempt + '.log')), useful=useful, grace=grace, kill_after=10,
            log_cap=cap, resource_check=self.resources)
        after = time.monotonic_ns()
        self.clock.child_ns += after - before
        self.clock.running_since = None
        R.sync_directory(self.root)
        # Retention of complete merged stream + supervisor precedes parsing/checks.
        raw_record = {'schema_version': 'pbdep.b04-raw.v1', 'attempt': attempt,
            'chain_id': self.request['chain_id'], 'program_sha256': self.clock.reservation['program_sha256'],
            'start_sha256': R.digest(R.encode(start)),
            'wrapper_before_run_monotonic_ns': str(before), 'wrapper_after_run_monotonic_ns': str(after),
            'stream': 'merged-stdout-stderr', 'boundary_is_not_exact_fork': True, **AXES,
            'supervisor': receipt, 'resource_samples': self.history,
            'last_sampled_resources': self.last_scan}
        R.persist(self.root / (attempt + '-RAW.json'), raw_record, state_root=self.state)
        self.remember(self.root / (attempt + '-RAW.json'))
        self.remember(self.root / (attempt + '.log'))
        self.clock.check()
        _, raw = R.file_observation(self.root / (attempt + '.log'), cap, retain=True)
        supervisor_float_scope(raw_record,wrapped=True)
        validate_supervisor_record(receipt)
        normalized = R.normalize_capture(receipt, raw, before, after, 0)
        self.persist(attempt + '-NORMALIZED.json', normalized)
        self.refresh()
        stable()
        self.resources()
        spec = {'start': pin_of(self.root / (attempt + '-START.json')),
                'raw': pin_of(self.root / (attempt + '-RAW.json')),
                'log': pin_of(self.root / (attempt + '.log'), cap)}
        self.persist(attempt + '-TERMINAL.json', {'schema_version': 'pbdep.b04-terminal.v1',
            'attempt': attempt, 'allocation_id': start['allocation_id'], 'capture': normalized,
            'artifacts': spec, 'resources': self.history, **AXES})
        spec['terminal'] = pin_of(self.root / (attempt + '-TERMINAL.json'))
        return raw, normalized, spec

    def finish(self, result):
        self.clock.close_started = time.monotonic_ns()
        self.clock.check()
        self.persist('RESULT.json', result)
        result_pin = pin_of(self.root / 'RESULT.json')
        now, prep = self.clock.check()
        self.persist('CLOSE.json', {'schema_version': 'pbdep.b04-close.v1',
            'stage': self.stage, 'chain_id': self.request['chain_id'],
            'result': result_pin, 'spent': self.spent,
            'stage_started_monotonic_ns': str(self.clock.started),
            'observed_before_close_write_monotonic_ns': str(now),
            'prep_ns': str(prep), 'child_wrapper_ns': str(self.clock.child_ns),
            'close_ns_before_write': str(now - self.clock.close_started),
            'close_write_is_inside_reserved_close_window': True})
        # A slow final write invalidates result through STOP, not a fabricated zero.
        self.clock.check()
        self.verify_retained()
        return result_pin


def compile_command(request):
    return (request['cargo'], 'test', '--locked', '--offline', '-p', 'beacond',
            '--lib', '--no-run', '--message-format=json')


def check_request(request, *, grant_pin, reference_pin, reservation_pin, prior_pin):
    R.keys(request, REQUEST_KEYS)
    R.need(request['schema_version'] == 'pbdep.discovery-request.v1', 'b04_request_version')
    W.hash_value(request['chain_id'])
    R.need(type(request['subject_head']) is str
        and re.fullmatch(r'[0-9a-f]{40}', request['subject_head']), 'b04_subject')
    for key, expected in (('grant', grant_pin), ('reference', reference_pin),
                          ('reservation', reservation_pin), ('prior', prior_pin)):
        checked_pin(expected)
        R.need(request[key] == expected, 'b04_independent_pin_join')
    for key in ('source_root', 'target_root', 'python', 'cargo', 'rustfmt'):
        W.path(request[key])
    source, target = request['source_root'], request['target_root']
    R.need(source != target and not source.startswith(target + '/')
        and not target.startswith(source + '/'), 'b04_root_overlap')
    # Refuse an unsafe initial map before Custody or any B04-owned retention.
    cleared_environment(request['environment'], request, phase='discovery')
    checked_pin(request['source_manifest'])
    R.need(0 < integer(request['raw_cap'], R.RAW_CAP), 'b04_raw_cap')
    return request
