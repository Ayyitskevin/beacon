"""Bounded trusted-local observation/capture adapter.

No import-time observation or dispatch. Callers independently authenticate the
reference, observation bindings, fresh reservation and native execution grant.
This module supplies neither discovery, whole-program scheduling nor authority.
Physical descendant exit, capture EOF and aggregate resource state stay separate.
This is not hostile containment or a hermetic compiler-input closure.
"""
import errno
import fcntl
import hashlib
import json
import math
import os
from copy import deepcopy
from pathlib import Path
import re
import stat
import time

from . import contract, policy, supervise, whole_program

MIB = 1024 * 1024
GIB = 1024 * MIB
TOTAL_CAPTURE = 64 * MIB
TERMINAL_RESERVE = 4 * MIB
RECEIPT_CAP = 128 * 1024
DATA_RECEIPT_CAP = 2 * MIB
DATA_RECEIPT_NAMES = frozenset(('OPERATOR.json', 'PHASE-BINDING.json', 'RESULT.json'))
RAW_CAP = 16 * MIB
SOURCE_CAP = 16 * MIB
CONTROL_CAP = 2 * MIB
SCAN_ENTRIES = 500000
SCAN_NS = 5_000_000_000
HEX = re.compile(r'[0-9a-f]{64}\Z')


class Stop(ValueError):
    """Public text stays code-only; narrow dependency provenance is structured.

    No source bytes or raw exception text enter the optional refusal context.
    """
    def __init__(self, code, *, provenance='deliberate-contract', refusal_context=None):
        self.provenance = provenance
        self.refusal_context = refusal_context
        super().__init__(code)


class ResourcePathError(OSError):
    def __init__(self, pathname_class, number):
        super().__init__(number or errno.EIO, 'resource_path_fault')
        self.pathname_class = pathname_class


def need(ok, code):
    if not ok:
        raise Stop(code)


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def pairs(items):
    result = {}
    for key, value in items:
        need(key not in result, 'duplicate_key')
        result[key] = value
    return result


def bad_number(_value):
    raise Stop('json_number')


def decode(raw):
    need(type(raw) is bytes and 0 < len(raw) <= CONTROL_CAP, 'json_bound')
    try:
        return json.loads(raw, object_pairs_hook=pairs, parse_float=bad_number,
                          parse_constant=bad_number)
    except Stop:
        raise
    except (ValueError, UnicodeError, RecursionError):
        raise Stop('json_shape') from None


def encode(value):
    return (json.dumps(value, sort_keys=True, separators=(',', ':'),
                       allow_nan=False) + '\n').encode('utf-8')


def keys(value, expected):
    need(type(value) is dict and set(value) == set(expected), 'record_shape')
    return value


def path_check(value):
    need(type(value) is str and len(value.encode('utf-8')) <= 1024
         and value.startswith('/') and os.path.normpath(value) == value,
         'path_shape')
    path = Path(value)
    for part in reversed((path,) + tuple(path.parents)):
        need(not stat.S_ISLNK(part.lstat().st_mode), 'symlink')
    return path


def stamp(s):
    return (s.st_dev, s.st_ino, s.st_mode, s.st_nlink, s.st_uid,
            s.st_size, s.st_mtime_ns, s.st_ctime_ns)


def file_observation(path, cap, *, retain=False, executable=False, installed_tool=False, reference=None):
    need(type(installed_tool) is bool, 'installed_tool_class')
    if installed_tool:
        need(reference is not None, 'installed_reference_unbound')
        whole_program.validate_reference(reference)
    installed_paths = set() if not installed_tool else {
        r['path'] for r in reference['installed_multicall'].values()}
    path = path_check(str(path))
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        before = os.fstat(fd)
        need(stat.S_ISREG(before.st_mode)
             and (before.st_nlink == 1 or (installed_tool
                  and str(path) in installed_paths))
             and 0 <= before.st_size <= cap, 'regular_file_bound')
        if executable:
            need(before.st_mode & 0o111 != 0 and before.st_mode & 0o022 == 0,
                 'executable_mode')
        hasher = hashlib.sha256()
        chunks = []
        count = 0
        while True:
            chunk = os.read(fd, min(MIB, cap + 1 - count))
            if not chunk:
                break
            count += len(chunk)
            need(count <= cap, 'file_grew')
            hasher.update(chunk)
            if retain:
                chunks.append(chunk)
        after = os.fstat(fd)
        need(stamp(before) == stamp(after) == stamp(path.lstat()), 'file_drift')
        record = {'path': str(path), 'sha256': hasher.hexdigest(),
                  'dev': str(before.st_dev), 'ino': str(before.st_ino),
                  'size': str(count), 'mtime_ns': str(before.st_mtime_ns),
                  'mode': str(stat.S_IMODE(before.st_mode)),
                  'uid': str(before.st_uid), 'nlink': str(before.st_nlink)}
        if installed_tool:
            whole_program.validate_installed_tool(record, file_class='immutable-installed-tool', reference=reference)
        return record, b''.join(chunks) if retain else None
    finally:
        os.close(fd)


def pinned(spec, cap=CONTROL_CAP):
    keys(spec, ('path', 'sha256'))
    need(type(spec['sha256']) is str and HEX.fullmatch(spec['sha256']), 'pin_shape')
    observed, raw = file_observation(spec['path'], cap, retain=True)
    need(observed['sha256'] == spec['sha256'], 'pin_drift')
    return raw


def directory(spec, *, short=False):
    keys(spec, ('path', 'dev', 'ino', 'uid', 'mode'))
    path = path_check(spec['path'])
    s = path.lstat()
    need(stat.S_ISDIR(s.st_mode) and s.st_uid == os.getuid()
         and stat.S_IMODE(s.st_mode) == 0o700, 'private_directory')
    actual = {'path': str(path), 'dev': str(s.st_dev), 'ino': str(s.st_ino),
              'uid': str(s.st_uid), 'mode': str(stat.S_IMODE(s.st_mode))}
    need(actual == spec, 'directory_identity')
    if short:
        need(len(os.fsencode(path)) <= 9 and len(os.fsencode(path)) + 75 <= 107,
             'temporary_path_budget')
    return path


def own_directory(path):
    path.parent.mkdir(mode=0o700, parents=False, exist_ok=True)
    need(not path.exists(), 'directory_already_exists')
    path.mkdir(mode=0o700)
    sync_directory(path.parent)
    s = path.lstat()
    need(stat.S_IMODE(s.st_mode) == 0o700 and s.st_uid == os.getuid(), 'created_directory_mode')
    return {'path': str(path), 'dev': str(s.st_dev), 'ino': str(s.st_ino),
            'uid': str(s.st_uid), 'mode': str(stat.S_IMODE(s.st_mode))}


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def receipt_cap(filename):
    """Closed larger-data class, not a caller-supplied cap or path grant."""
    need(type(filename) is str and re.fullmatch(r'[A-Za-z0-9_-]+\.json', filename),
         'receipt_filename')
    return DATA_RECEIPT_CAP if filename in DATA_RECEIPT_NAMES else RECEIPT_CAP


def validate_receipt_bytes(filename, raw):
    need(type(raw) is bytes and len(raw) <= receipt_cap(filename), 'receipt_bound')
    return len(raw)


def retention_budget(current_bytes, additional_bytes, terminal):
    need(type(current_bytes) is int and current_bytes >= 0
         and type(additional_bytes) is int and additional_bytes >= 0
         and type(terminal) is bool, 'retention_budget_type')
    projected = current_bytes + additional_bytes
    need(projected <= (TOTAL_CAPTURE if terminal else TOTAL_CAPTURE - TERMINAL_RESERVE),
         'retention_budget')
    return projected


def terminal_receipt(path, *, state_root):
    need(type(path) is str and path.startswith('/') and os.path.normpath(path) == path,
         'terminal_receipt_path')
    return path == str(Path(state_root) / 'STOP.json')


def retained_bytes(state_root, *, entry_cap=4096, scan_ns=SCAN_NS):
    """Bounded state-only sample; no link/special/error/disappearance allowance."""
    need(type(entry_cap) is int and 0 <= entry_cap <= 4096
         and type(scan_ns) is int and 0 <= scan_ns <= SCAN_NS, 'retention_scan_bound')
    root = path_check(str(state_root))
    info = root.lstat()
    need(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid()
         and stat.S_IMODE(info.st_mode) == 0o700, 'retention_root')
    started = time.monotonic_ns()
    total = entries = pathname_bytes = 0
    todo = [root]
    while todo:
        need(time.monotonic_ns() - started < scan_ns, 'retention_scan_time')
        current = todo.pop()
        with os.scandir(current) as iterator:
            for item in iterator:
                entries += 1
                pathname_bytes += len(os.fsencode(item.path))
                need(entries <= entry_cap and pathname_bytes <= 16 * MIB
                     and len(os.fsencode(item.path)) <= 4096, 'retention_scan_bound')
                observed = item.stat(follow_symlinks=False)
                if stat.S_ISDIR(observed.st_mode):
                    todo.append(Path(item.path))
                else:
                    need(stat.S_ISREG(observed.st_mode) and observed.st_nlink == 1
                         and observed.st_size >= 0, 'retention_entry')
                    total += observed.st_size
                    need(total <= TOTAL_CAPTURE, 'retention_budget')
                need(time.monotonic_ns() - started <= scan_ns, 'retention_scan_time')
        need(time.monotonic_ns() - started <= scan_ns, 'retention_scan_time')
    return total


def persist_bytes(path, raw, *, state_root):
    """Create-only, fsync file+parent, read back. No self-hash field."""
    validate_receipt_bytes(path.name, raw)
    state_root = path_check(str(state_root))
    path = Path(path)
    need(state_root in path.parents, 'receipt_outside_state')
    retention_budget(retained_bytes(state_root), len(raw),
                     terminal_receipt(str(path), state_root=state_root))
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
                 | os.O_CLOEXEC, 0o600)
    try:
        with os.fdopen(fd, 'wb', closefd=False) as stream:
            stream.write(raw)
            stream.flush()
        os.fsync(fd)
    finally:
        os.close(fd)
    sync_directory(path.parent)
    _, readback = file_observation(path, receipt_cap(path.name), retain=True)
    need(readback == raw, 'receipt_readback')
    return raw


def persist(path, value, *, state_root):
    return persist_bytes(path, encode(value), state_root=state_root)


def boot_id():
    # Proc/sys observations are not regular retained-input files.
    with open('/proc/sys/kernel/random/boot_id', 'rb') as stream:
        raw = stream.read(65)
    need(re.fullmatch(rb'[0-9a-f-]{36}\n', raw) is not None, 'boot_identity')
    return raw.decode('ascii').strip()


def require_umask():
    with open('/proc/self/status', 'rb') as stream:
        raw = stream.read(65537)
    need(len(raw) <= 65536 and raw.count(b'Umask:\t0077\n') == 1, 'umask_not_0077')


def _checked_resource_error(record):
    keys(record, ('pathname_class', 'errno'))
    need(type(record['pathname_class']) is str
         and record['pathname_class'] in ('root-lstat', 'entry-stat', 'directory-scan')
         and type(record['errno']) is int and 1 <= record['errno'] <= 4095,
         'invalid_resource_path_error')
    return dict(record)


def resource_error_record(error):
    """Closed resource evidence only: never exception text or a raw pathname."""
    need(isinstance(error, ResourcePathError), 'invalid_resource_path_error')
    return _checked_resource_error({'pathname_class': getattr(error, 'pathname_class', None),
                                    'errno': error.errno})


def terminal_stop_record(phase, code, spent, recorded_monotonic_ns, *, resource_error=None):
    """Pure terminal serialization used by Program.stop and independent controls."""
    record = {'schema_version': 'phase-b.stop.v1',
        'status': 'STOP_NO_RETRY', 'phase': phase, 'reason': code,
        'spent': list(spent), 'physical_exit': 'SEE_RAW_RECEIPTS_OR_UNKNOWN',
        'resources': 'RETAINED_ENCUMBERED', 'retry_authorized': False,
        'recorded_monotonic_ns': recorded_monotonic_ns}
    if resource_error is not None:
        record['resource_error'] = _checked_resource_error(resource_error)
    return record


def verify_file_pins(specs):
    """Explicit finite read-only pin check; this result never grants dispatch."""
    need(type(specs) is list and 1 <= len(specs) <= 4096, 'pin_inventory')
    paths = []
    facts = {}
    for spec in specs:
        keys(spec, ('path', 'sha256'))
        need(spec['path'] not in paths, 'pin_duplicate')
        paths.append(spec['path'])
        facts[spec['path']] = digest(pinned(spec))
    return facts


def source_resource_join(phase, data, registered_roots):
    """Pure whole-root accounting join; A2's retained source is read-only."""
    def canonical(value):
        return (type(value) is str and value.startswith('/')
                and len(value.encode('utf-8')) <= 1024
                and os.path.normpath(value) == value)

    need(type(phase) is str and type(data) is dict
         and type(registered_roots) is list and len(registered_roots) <= 12
         and all(canonical(root) for root in registered_roots), 'source_resource_shape')
    need(len(registered_roots) == len(set(registered_roots)), 'source_resource_shape')
    need(phase in ('a2', 'format', 'base', 'candidate', 'integration', 'consumers', 'gate'),
         'source_resource_phase')
    if phase == 'format':
        roots = data.get('roots')
        need(type(roots) is list and len(roots) == 2
             and all(canonical(root) for root in roots), 'source_resource_shape')
        need(len(set(roots)) == 2, 'source_resource_shape')
        need(all(root in registered_roots for root in roots), 'source_resource_join')
        return {'source_roots': list(roots), 'target_exclusion': None}
    root = data.get('source_root')
    need(canonical(root), 'source_resource_shape')
    if phase == 'a2':
        return {'source_roots': [root], 'target_exclusion': None}
    need(root in registered_roots, 'source_resource_join')
    return {'source_roots': [root], 'target_exclusion': root + '/target'}


def verify_selection_inputs(value, immutable_files):
    """Named finite selectors only; no search, command or authority callback."""
    shape = whole_program.validate_selection_inputs(value)
    need(type(immutable_files) is list and len(immutable_files) <= 4096, 'closure_inventory')
    destinations = {spec['path'] for spec in immutable_files}
    for name in value['absent_paths']:
        absent = False
        candidate = Path(name)
        for part in reversed((candidate,) + tuple(candidate.parents)):
            try:
                observed = part.lstat()
            except OSError as exc:
                if exc.errno == errno.ENOENT:
                    absent = True
                    break
                raise Stop('selection_absence_io') from None
            need(not stat.S_ISLNK(observed.st_mode), 'selection_absence_symlink')
        need(absent, 'selection_absence_drift')
    for alias in value['aliases']:
        need(alias['resolved_path'] in destinations, 'selection_destination_not_pinned')
        need(os.path.realpath(alias['path'], strict=True) == alias['resolved_path'], 'selection_alias_drift')
    return shape


def validate_binding_links(operator, binding, binding_raw, *, reference):
    """Exact supplied joins, not approval or an actual-file observation."""
    whole_program.validate_operator(operator, reference=reference)
    keys(binding, ('schema_version', 'phase', 'authority_pins', 'cwd', 'environment',
                   'python', 'immutable_files', 'tmp', 'selection_inputs', 'cargo',
                   'rustfmt', 'data', 'step_zero_acceptance', 'source_freeze_review',
                   'operator_phase_attestation'))
    expected_pin = whole_program.binding_pin(operator, binding['phase'], reference=reference)
    need(type(binding_raw) is bytes and digest(binding_raw) == expected_pin['sha256'],
         'binding_pin')
    whole_program.validate_phase_data(binding['phase'], binding['data'], reference=reference,
                                      phase_binding_raw=binding_raw)
    whole_program._same_json_data(binding, decode(binding_raw))
    role_pins = {role: operator[role]['sha256']
                 for role in ('authority', 'packet', 'acceptance', 'review', 'disposition')}
    need(binding['authority_pins'] == role_pins, 'phase_authority_join')
    return {'scope': 'file-and-record-joins-not-authority',
            'binding_sha256': digest(binding_raw)}


def capture_budget(current_bytes):
    need(type(current_bytes) is int and 0 <= current_bytes <= TOTAL_CAPTURE - TERMINAL_RESERVE,
         'capture_reserve')
    cap = min(RAW_CAP, TOTAL_CAPTURE - TERMINAL_RESERVE - current_bytes - RECEIPT_CAP * 4)
    need(cap > 0, 'capture_no_room')
    return cap


def capacity_policy(total_bytes, temporary_bytes, capture_bytes, free_by_kind, *, initial):
    need(type(initial) is bool, 'resource_phase_type')
    for value in (total_bytes, temporary_bytes, capture_bytes):
        need(type(value) is int and value >= 0, 'resource_value')
    need(type(free_by_kind) is list and free_by_kind, 'resource_free_inventory')
    for kind, available in free_by_kind:
        need(kind in ('workspace', 'temporary') and type(available) is int and available >= 0,
             'resource_free_value')
        floor = (128 if initial else 32) if kind == 'workspace' else (32 if initial else 16)
        need(available >= floor * GIB, 'resource_free_floor')
    need(total_bytes <= 96 * GIB and temporary_bytes <= 8 * GIB, 'resource_output_cap')
    need(capture_bytes <= TOTAL_CAPTURE - TERMINAL_RESERVE, 'capture_reserve')
    return {'total_bytes': total_bytes, 'temporary_bytes': temporary_bytes,
            'capture_bytes': capture_bytes, 'sampled_not_quota': True}


def checked_temporary_roots(value):
    need(type(value) is tuple and len(value) <= 4
         and len(set(value)) == len(value), 'temporary_roots')
    for allowed in value:
        whole_program.path(allowed)
        need(len(os.fsencode(allowed)) <= 9, 'temporary_path_budget')
    return value


def resource_entry(root_path, kind, captures, mode, size, *, temporary_roots):
    """Resource metadata only; never opens or blesses an input/link target."""
    checked_temporary_roots(temporary_roots)
    need(type(root_path) is str and root_path.startswith('/')
         and os.path.normpath(root_path) == root_path
         and type(kind) is str and kind in ('workspace', 'temporary')
         and type(captures) is bool and type(mode) is int and 0 <= mode <= 0xffff
         and type(size) is int and size >= 0, 'resource_entry_shape')
    need(kind != 'temporary' or (root_path in temporary_roots and not captures),
         'temporary_root_class')
    if stat.S_ISDIR(mode):
        return {'entry_type': 'directory', 'descend': True, 'accounted_bytes': 0}
    if stat.S_ISREG(mode):
        return {'entry_type': 'regular', 'descend': False, 'accounted_bytes': size}
    need(kind == 'temporary' and not captures, 'resource_special_file')
    if stat.S_ISLNK(mode):
        return {'entry_type': 'symlink', 'descend': False, 'accounted_bytes': size}
    if stat.S_ISSOCK(mode):
        return {'entry_type': 'socket', 'descend': False, 'accounted_bytes': size}
    raise Stop('resource_special_file')


def scan_resources(specs, state_spec, *, initial, temporary_roots,
                   entry_cap=SCAN_ENTRIES, scan_ns=SCAN_NS):
    """Finite explicit directories. Raises pathname-classed I/O failures."""
    checked_temporary_roots(temporary_roots)
    need(type(specs) is list and 1 <= len(specs) <= 12, 'resource_inventory')
    need(type(entry_cap) is int and 0 <= entry_cap <= SCAN_ENTRIES
         and type(scan_ns) is int and 0 <= scan_ns <= SCAN_NS, 'resource_control_bound')
    roots = [(state_spec, 'workspace', True)]
    for spec in specs:
        keys(spec, ('directory', 'kind'))
        need(spec['kind'] in ('workspace', 'temporary'), 'resource_kind')
        need(spec['kind'] != 'temporary' or spec['directory']['path'] in temporary_roots,
             'temporary_root_class')
        roots.append((spec['directory'], spec['kind'], False))
    names = [Path(s['path']) for s, _, _ in roots]
    need(len(set(names)) == len(names), 'resource_roots_duplicate')
    for first in names:
        for second in names:
            need(first == second or first not in second.parents, 'resource_roots_overlap')
    started = time.monotonic_ns()
    entries = total = temporary = capture = pathname_bytes = 0
    free = []
    vanished = []
    vanished_counts = {'entry-stat': 0, 'directory-scan': 0}
    entry_types = {'directory': 0, 'regular': 0, 'symlink': 0, 'socket': 0}

    def missing(where, root_number, root, target):
        need(len(vanished) < 128, 'vanished_metadata_cap')
        relative = str(root_number) + ':' + os.path.relpath(target, root)
        vanished.append({'pathname_class': where,
                         'relative_path_sha256': digest(os.fsencode(relative))})
        vanished_counts[where] += 1

    for root_number, (spec, kind, captures) in enumerate(roots):
        need(time.monotonic_ns() - started < scan_ns, 'resource_scan_time')
        try:
            root = directory(spec)
        except OSError as exc:
            raise ResourcePathError('root-lstat', exc.errno) from None
        todo = [root]
        while todo:
            current = todo.pop()
            try:
                with os.scandir(current) as iterator:
                    for item in iterator:
                        entries += 1
                        pathname_bytes += len(os.fsencode(item.path))
                        need(entries <= entry_cap and pathname_bytes <= 16 * MIB,
                             'resource_metadata_cap')
                        need(time.monotonic_ns() - started <= scan_ns, 'resource_scan_time')
                        try:
                            observed = item.stat(follow_symlinks=False)
                        except OSError as exc:
                            if exc.errno == errno.ENOENT and not captures:
                                missing('entry-stat', root_number, root, item.path)
                                continue
                            raise ResourcePathError('entry-stat', exc.errno) from None
                        need(len(os.fsencode(item.path)) <= 4096, 'resource_path_length')
                        account = resource_entry(str(root), kind, captures,
                                                 observed.st_mode, observed.st_size, temporary_roots=temporary_roots)
                        entry_types[account['entry_type']] += 1
                        if account['descend']:
                            todo.append(Path(item.path))
                        else:
                            total += account['accounted_bytes']
                            temporary += account['accounted_bytes'] if kind == 'temporary' else 0
                            capture += account['accounted_bytes'] if captures else 0
            except ResourcePathError:
                raise
            except OSError as exc:
                if exc.errno == errno.ENOENT and current != root and not captures:
                    missing('directory-scan', root_number, root, current)
                    continue
                raise ResourcePathError('directory-scan', exc.errno) from None
            need(time.monotonic_ns() - started <= scan_ns, 'resource_scan_time')
        try:
            space = os.statvfs(root)
        except OSError as exc:
            raise ResourcePathError('root-lstat', exc.errno) from None
        free.append((kind, space.f_bavail * space.f_frsize))
    facts = capacity_policy(total, temporary, capture, free, initial=initial)
    facts.update({'vanished_count': len(vanished), 'vanished_by_class': vanished_counts,
                  'vanished': vanished, 'sampled_entry_types': entry_types})
    return facts


def normalize_capture(receipt, raw, before_ns, after_ns, expected_exit):
    keys(receipt, ('leader_pid', 'leader_returncode', 'leader_exit_monotonic',
        'whole_descendant_set_exit', 'echild_monotonic', 'pipe_eof', 'pipe_eof_monotonic',
        'subreaper_verified', 'wait_options', 'elapsed_seconds', 'stop', 'metadata_overflow',
        'event_counts', 'reaped', 'signals', 'errors', 'resource_peak_sampled',
        'log_bytes', 'discarded_log_bytes', 'log_sha256'))
    keys(receipt['event_counts'], ('reaped', 'signals', 'errors'))
    need(type(receipt['leader_pid']) is int and receipt['leader_pid'] > 0
         and type(receipt['resource_peak_sampled']) is int and receipt['resource_peak_sampled'] >= 0
         and receipt['wait_options'] == 'WNOHANG|__WALL', 'capture_metadata')
    for key in ('leader_exit_monotonic', 'echild_monotonic', 'pipe_eof_monotonic', 'elapsed_seconds'):
        need(type(receipt[key]) in (int, float) and math.isfinite(receipt[key])
             and receipt[key] >= 0, 'capture_metadata')
    for name in ('reaped', 'signals', 'errors'):
        need(type(receipt[name]) is list and len(receipt[name]) <= 128
             and type(receipt['event_counts'][name]) is int
             and receipt['event_counts'][name] == len(receipt[name]), 'capture_metadata')
    need(type(before_ns) is int and type(after_ns) is int and 0 < before_ns <= after_ns,
         'capture_clock')
    need(type(raw) is bytes and len(raw) <= RAW_CAP, 'capture_bytes')
    need(receipt['whole_descendant_set_exit'] == 'PROVEN_ECHILD'
         and receipt['pipe_eof'] is True and receipt['subreaper_verified'] is True
         and receipt['metadata_overflow'] is False
         and type(receipt['discarded_log_bytes']) is int and receipt['discarded_log_bytes'] == 0
         and type(receipt['errors']) is list and receipt['errors'] == []
         and type(receipt['event_counts']['errors']) is int and receipt['event_counts']['errors'] == 0
         and receipt['stop'] is None and type(receipt['leader_returncode']) is int
         and receipt['leader_returncode'] == expected_exit, 'custody_or_unexpected_exit')
    need(type(expected_exit) is int and digest(raw) == receipt['log_sha256']
         and type(receipt['log_bytes']) is int and len(raw) == receipt['log_bytes'], 'capture_identity')
    return {'exit_code': expected_exit, 'whole_descendant_set_exit': 'PROVEN_ECHILD',
        'pipe_eof': True, 'subreaper_verified': True, 'discarded_log_bytes': 0,
        'metadata_overflow': False, 'errors': [], 'event_error_count': 0,
        'stop': None, 'log_sha256': digest(raw), 'log_bytes': len(raw),
        'start_monotonic_ns': str(before_ns), 'end_monotonic_ns': str(after_ns)}


def source_inventory(source_root, *, excluded_target=None, entry_cap=4096, scan_ns=SCAN_NS):
    """Exact bounded source names. Critical-input disappearance is never ignored."""
    need(type(source_root) is str and type(entry_cap) is int and 0 <= entry_cap <= 4096
         and type(scan_ns) is int and 0 <= scan_ns <= SCAN_NS, 'source_scan_bound')
    root = path_check(source_root)
    need(stat.S_ISDIR(root.lstat().st_mode), 'source_root_type')
    need(excluded_target is None or (type(excluded_target) is str
         and excluded_target == source_root + '/target'), 'source_exclusion')
    started = time.monotonic_ns()
    entries = path_bytes = 0
    files = []
    git_record = None
    todo = [root]
    while todo:
        need(time.monotonic_ns() - started < scan_ns, 'source_scan_time')
        current = todo.pop()
        with os.scandir(current) as iterator:
            for item in iterator:
                entries += 1
                path_bytes += len(os.fsencode(item.path))
                need(entries <= entry_cap and path_bytes <= 16 * MIB
                     and len(os.fsencode(item.path)) <= 4096, 'source_scan_bound')
                need(time.monotonic_ns() - started <= scan_ns, 'source_scan_time')
                observed = item.stat(follow_symlinks=False)
                need(not stat.S_ISLNK(observed.st_mode), 'source_symlink')
                relative = str(Path(item.path).relative_to(root))
                if relative == '.git':
                    need(stat.S_ISREG(observed.st_mode) and git_record is None, 'source_git_file')
                    git_record = file_observation(item.path, SOURCE_CAP)[0]
                elif excluded_target is not None and item.path == excluded_target:
                    need(stat.S_ISDIR(observed.st_mode), 'source_target_type')
                elif stat.S_ISDIR(observed.st_mode):
                    todo.append(Path(item.path))
                else:
                    need(stat.S_ISREG(observed.st_mode), 'source_special_file')
                    files.append(relative)
        need(time.monotonic_ns() - started <= scan_ns, 'source_scan_time')
    need(git_record is not None, 'source_git_missing')
    return {'files': sorted(files), 'git': git_record}


def observe_dependencies(manifest, data, *, reference, phase_binding_raw, expected=None):
    """Bounded no-follow raw component walk, including cancelled directories.

    Descriptor/path stability and repeated observations are trusted-local
    evidence, not hostile-host or atomic filesystem containment.
    """
    whole_program.validate_phase_data(data.get('phase'), data, reference=reference,
                                      phase_binding_raw=phase_binding_raw)
    keys(manifest, ('schema_version', 'source_root', 'subject_head', 'files'))
    need(manifest['schema_version'] == 'phase-b.source-manifest.v1'
         and manifest['source_root'] == data['source_root']
         and manifest['subject_head'] == data['subject_head'], 'dependency_source_join')
    mapping = whole_program.validate_dependency_bindings(data['dep_tokens'], data['dep_binding_schema'],
        data['dep_bindings'], sources=manifest['files'])
    root = path_check(data['source_root'])
    opened = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    started = time.monotonic_ns()
    directories = {}
    files = {}
    work = 0

    def directory_record(pathname, observed):
        need(stat.S_ISDIR(observed.st_mode), 'dependency_directory')
        return {'path': pathname, 'dev': str(observed.st_dev), 'ino': str(observed.st_ino),
                'mode': str(stat.S_IMODE(observed.st_mode)), 'uid': str(observed.st_uid)}

    try:
        root_before = os.fstat(opened)
        need(stamp(root_before) == stamp(root.lstat()), 'dependency_root_drift')
        root_record = directory_record(str(root), root_before)
        for token, binding in zip(data['dep_tokens'], data['dep_bindings']):
            descriptors = [os.dup(opened)]
            stack = []
            held = []
            try:
                parts = token.split('/')
                # A final '..' denotes a directory, not a regular source input.
                need(parts[-1] != '..', 'dependency_terminal')
                for part in parts[:-1]:
                    work += 1
                    need(work <= 65536 and time.monotonic_ns() - started < SCAN_NS,
                         'dependency_walk_bound')
                    if part == '..':
                        need(len(descriptors) > 1, 'dependency_escape')
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
                        need(error.filename == part and stat.S_ISDIR(parent_info.st_mode)
                             and stamp(parent_info) == stamp(Path(parent_name).lstat()),
                             'dependency_missing_site')
                        raise Stop('dependency_missing_component',
                            provenance='verified-missing-component', refusal_context={
                                'raw_token': token, 'component': part, 'parent_path': parent_name,
                                'parent_dev': str(parent_info.st_dev), 'parent_ino': str(parent_info.st_ino),
                                'errno': errno.ENOENT}) from None
                    need(stat.S_ISDIR(before.st_mode), 'dependency_directory')
                    fd = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC,
                                 dir_fd=descriptors[-1])
                    try:
                        need(stamp(os.fstat(fd)) == stamp(before), 'dependency_directory_drift')
                        held.append((os.dup(descriptors[-1]), part, before))
                        stack.append(part)
                        name = '/'.join(stack)
                        record = directory_record(str(root) + '/' + name, before)
                        need(name not in directories or directories[name] == record,
                             'dependency_directory_drift')
                        directories[name] = record
                        descriptors.append(fd)
                    except BaseException:
                        os.close(fd)
                        raise
                before = os.stat(parts[-1], dir_fd=descriptors[-1], follow_symlinks=False)
                need(stat.S_ISREG(before.st_mode) and before.st_nlink == 1
                     and 0 <= before.st_size <= SOURCE_CAP, 'dependency_terminal')
                fd = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC,
                             dir_fd=descriptors[-1])
                try:
                    need(stamp(os.fstat(fd)) == stamp(before), 'dependency_file_drift')
                    hasher = hashlib.sha256()
                    count = 0
                    while True:
                        need(time.monotonic_ns() - started < SCAN_NS, 'dependency_walk_bound')
                        chunk = os.read(fd, min(MIB, SOURCE_CAP + 1 - count))
                        if not chunk:
                            break
                        count += len(chunk)
                        need(count <= SOURCE_CAP, 'dependency_file_bound')
                        hasher.update(chunk)
                    need(stamp(before) == stamp(os.fstat(fd))
                         == stamp(os.stat(parts[-1], dir_fd=descriptors[-1], follow_symlinks=False)),
                         'dependency_file_drift')
                    canonical = root / binding['canonical_relative']
                    canonical_record = file_observation(canonical, SOURCE_CAP)[0]
                    need(stamp(before) == stamp(canonical.lstat())
                         and hasher.hexdigest() == canonical_record['sha256'] == binding['source_sha256'],
                         'dependency_canonical_join')
                    files[binding['canonical_relative']] = canonical_record
                finally:
                    os.close(fd)
                for parent_fd, name, before in held:
                    need(stamp(before) == stamp(os.stat(name, dir_fd=parent_fd, follow_symlinks=False)),
                         'dependency_directory_drift')
            finally:
                for fd in descriptors:
                    os.close(fd)
                for fd, _, _ in held:
                    os.close(fd)
        need(stamp(root_before) == stamp(os.fstat(opened)) == stamp(root.lstat()), 'dependency_root_drift')
        result = {'schema_version': 'phase-b.dep-observation.v1', 'source_root': root_record,
            'subject_head': data['subject_head'], 'manifest_sha256': data['manifest_sha256'],
            'dep_binding_schema': data['dep_binding_schema'],
            'dep_bindings_sha256': mapping['dep_bindings_sha256'], 'directories': directories, 'files': files}
        whole_program.validate_dependency_observation(result, data=data, sources=manifest['files'])
        need(expected is None or result == expected, 'dependency_observation_drift')
        return result
    except OSError:
        raise Stop('dependency_walk_io', provenance='io-exception') from None
    finally:
        os.close(opened)


def consume_slot(phase_root, phase, slot, record, *, reservation, state_root):
    policy.validate_reservation(reservation)
    phases = reservation['phases']
    """Create-only START. No dispatch, arbitrary commands or resume authority."""
    need(phase in phases, 'unknown_phase')
    declared = ('Q',) if phase == 'qualification' else phases[phase]['slots']
    need(slot in declared and type(record) is dict and record.get('slot') == slot
         and record.get('phase') == phase and record.get('slot_spent_even_if_fork_fails') is True,
         'slot_record')
    root = path_check(str(phase_root))
    need(root == Path(state_root) / phase, 'phase_root_join')
    actual = set()
    with os.scandir(root) as entries:
        for index, entry in enumerate(entries):
            need(index < 64, 'phase_file_inventory')
            if entry.name.endswith('-START.json'):
                actual.add(entry.name)
    expected = {s + '-START.json' for s in declared[:declared.index(slot)]}
    need(actual == expected, 'slot_order_or_replay')
    return persist(root / (slot + '-START.json'), record, state_root=state_root)


class ObservationSession:
    """Later-phase observation/capture seam, not an entry or approval provider.

    Caller authenticates all required expected inputs, owns fresh program/clock/
    phase admission and independently retains the returned evidence. Qualification
    and original-oracle entry orchestration are not implemented by this adapter.
    """

    def __init__(self, phase, *, reference, phase_binding_raw, observation_bindings, reservation):
        whole_program.validate_reference(reference)
        policy.validate_reservation(reservation)
        need(type(phase) is str and phase in policy.ORDER[2:], 'adapter_phase_unbound')
        binding = decode(phase_binding_raw)
        validate_binding_links(reference['operator'], binding, phase_binding_raw, reference=reference)
        need(binding['phase'] == phase, 'phase_join')
        keys(observation_bindings, ('schema_version', 'state_directory', 'module_paths'))
        need(observation_bindings['schema_version'] == 'pbdep.observation-bindings.v1',
             'observation_version')
        keys(observation_bindings['state_directory'], ('path', 'dev', 'ino', 'uid', 'mode'))
        whole_program.path(observation_bindings['state_directory']['path'])
        for field in ('dev', 'ino', 'uid', 'mode'):
            whole_program.ns(observation_bindings['state_directory'][field])
        need(observation_bindings['state_directory']['mode'] == '448', 'state_mode')
        keys(observation_bindings['module_paths'], reference['operator']['instrument_sha256'])
        for value in observation_bindings['module_paths'].values():
            whole_program.path(value)
        self.phase = phase
        self.reference = decode(encode(reference))
        self.binding_raw = phase_binding_raw
        self.binding = decode(phase_binding_raw)
        self.data = self.binding['data']
        self.observation_bindings = decode(encode(observation_bindings))
        self.reservation = deepcopy(reservation)
        self.state_spec = self.observation_bindings['state_directory']
        self.state_root = Path(self.state_spec['path'])
        self.phase_root = self.state_root / phase
        self.temporary_roots = tuple(r['directory']['path']
            for r in self.reference['operator']['resource_roots'] if r['kind'] == 'temporary')
        need(len(self.temporary_roots) <= 4, 'temporary_roots')
        for value in self.temporary_roots:
            whole_program.path(value)
            need(len(os.fsencode(value)) <= 9, 'temporary_path_budget')
        self.manifest_raw = self.manifest = None
        self.compilation_fact = self.inventory_fact = self.provenance = None
        self.binary_fact = self.depfile_fact = self.dependency_snapshot = None
        self.format_completed = False
        self.spent = []
        self.captures = {}
        self.resource_facts = {'sample_count': 0, 'peak_total_bytes': 0,
            'peak_temporary_bytes': 0, 'peak_capture_bytes': 0,
            'vanished_by_class': {'entry-stat': 0, 'directory-scan': 0}, 'vanished': [],
            'sampled_entry_types': {'directory': 0, 'regular': 0, 'symlink': 0, 'socket': 0}}
        self.last_resource_scan = None

    def verify_inputs(self):
        """Fresh actual pins; their external approval is still caller-owned."""
        require_umask()
        validate_binding_links(self.reference['operator'], self.binding, self.binding_raw,
                               reference=self.reference)
        need(pinned(whole_program.binding_pin(self.reference['operator'], self.phase,
                    reference=self.reference)) == self.binding_raw, 'phase_binding_drift')
        directory(self.state_spec)
        root = path_check(str(self.phase_root))
        info = root.lstat()
        need(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid()
             and stat.S_IMODE(info.st_mode) == 0o700, 'phase_directory')
        need(not (self.state_root / 'STOP.json').exists(), 'program_stopped')
        operator = self.reference['operator']
        verify_file_pins([operator[r] for r in ('authority', 'packet', 'acceptance', 'review', 'disposition')])
        verify_file_pins([{'path': self.observation_bindings['module_paths'][name], 'sha256': value}
                          for name, value in operator['instrument_sha256'].items()])
        for role in ('step_zero_acceptance', 'source_freeze_review'):
            pinned(self.binding[role])
        attestation = self.binding['operator_phase_attestation']
        keys(attestation, ('step_zero_read_by', 'freeze_read_by', 'issuer', 'step_zero_sha256',
                           'freeze_sha256', 'data_sha256', 'grants_authority'))
        for field in ('step_zero_read_by', 'freeze_read_by', 'issuer'):
            need(type(attestation[field]) is str and 0 < len(attestation[field]) <= 128
                 and whole_program.NAME.fullmatch(attestation[field]) is not None
                 and attestation[field] not in ('UNKNOWN', 'UNBOUND'), 'attestation_shape')
        need(attestation['step_zero_sha256'] == self.binding['step_zero_acceptance']['sha256']
             and attestation['freeze_sha256'] == self.binding['source_freeze_review']['sha256']
             and attestation['data_sha256'] == digest(encode(self.data))
             and attestation['grants_authority'] is False, 'attestation_join')
        if self.phase == 'format' and not self.format_completed:
            for record in self.data['files']:
                need(file_observation(Path(record['root']) / record['relative'], SOURCE_CAP)[0]['sha256']
                     == record['sha256'], 'format_source_drift')
        if self.phase in ('base', 'candidate'):
            pinned(self.data['full_roster'])
            pinned(self.data['selected_roster'])
        elif self.phase == 'gate':
            need(file_observation(Path(self.data['source_root']) / 'scripts/gate', SOURCE_CAP)[0]['sha256']
                 == self.data['gate_sha256'], 'gate_script_drift')
        immutable = self.binding['immutable_files']
        need(type(immutable) is list and 1 <= len(immutable) <= 4096, 'closure_inventory')
        paths = []
        for record in immutable:
            keys(record, ('path', 'sha256', 'dev', 'ino', 'size', 'mtime_ns', 'mode', 'uid', 'nlink'))
            observed, _ = file_observation(record['path'], GIB, installed_tool=True,
                                            reference=self.reference)
            need(observed == record, 'closure_or_tool_drift')
            paths.append(record['path'])
        need(len(set(paths)) == len(paths), 'closure_duplicates')
        for role in ('python', 'cargo', 'rustfmt'):
            need(self.binding[role] in paths, 'tool_not_in_closure')
            file_observation(self.binding[role], GIB, executable=True, installed_tool=True,
                             reference=self.reference)
        verify_selection_inputs(self.binding['selection_inputs'], immutable)
        environment = self.binding['environment']
        need(type(environment) is dict and 1 <= len(environment) <= 64, 'environment_shape')
        for name, value in environment.items():
            need(type(name) is str and re.fullmatch(r'[A-Z][A-Z0-9_]{0,63}', name)
                 and type(value) is str and '\0' not in value and len(value.encode()) <= 8192,
                 'environment_value')
        need(environment.get('PYTHONDONTWRITEBYTECODE') == '1'
             and environment.get('PYTHONNOUSERSITE') == '1'
             and environment.get('CARGO_NET_OFFLINE') == 'true'
             and environment.get('LC_ALL') == 'C'
             and not {'LD_PRELOAD', 'LD_AUDIT', 'PYTHONPATH', 'PYTHONHOME'} & set(environment),
             'environment_policy')
        temporary = directory(self.binding['tmp'], short=True)
        need(environment.get('TMPDIR') == str(temporary), 'temporary_environment_join')
        roots = [directory(r['directory']) for r in operator['resource_roots']]
        need(temporary in roots and str(temporary) in self.temporary_roots, 'temporary_not_accounted')
        all_roots = [self.state_root] + roots
        need(len(set(all_roots)) == len(all_roots), 'resource_roots_duplicate')
        for first in all_roots:
            for second in all_roots:
                need(first == second or first not in second.parents, 'resource_roots_overlap')
        source_resource_join(self.phase, self.data, [str(p) for p in roots])
        path_check(self.binding['cwd'])
        if self.phase != 'format':
            need(self.binding['cwd'] == self.data['source_root'], 'source_root_join')
            target = Path(self.data['target_root'])
            need(target in roots and environment.get('CARGO_TARGET_DIR') == str(target),
                 'target_resource_join')
        return {'scope': 'observed-input-pins-only', 'reference_sha256': digest(encode(self.reference)),
                'phase_binding_sha256': digest(self.binding_raw)}

    def resources(self, *, initial=False):
        facts = scan_resources(self.reference['operator']['resource_roots'],
            self.state_spec, initial=initial, temporary_roots=self.temporary_roots)
        r = self.resource_facts
        r['sample_count'] += 1
        need(r['sample_count'] <= 8192, 'resource_sample_metadata_cap')
        for field in ('total_bytes', 'temporary_bytes', 'capture_bytes'):
            r['peak_' + field] = max(r['peak_' + field], facts[field])
        need(len(r['vanished']) + facts['vanished_count'] <= 128, 'vanished_metadata_cap')
        r['vanished'].extend(facts['vanished'])
        for name in r['vanished_by_class']:
            r['vanished_by_class'][name] += facts['vanished_by_class'][name]
        for name in r['sampled_entry_types']:
            r['sampled_entry_types'][name] += facts['sampled_entry_types'][name]
        self.last_resource_scan = facts
        return facts['total_bytes']

    def persist(self, name, value):
        need(type(name) is str and re.fullmatch(r'[A-Za-z0-9_-]+\.json', name), 'receipt_filename')
        return persist(self.phase_root / name, value, state_root=self.state_root)

    def sources(self):
        self.verify_inputs()
        need(self.phase != 'format', 'source_manifest_unbound')
        raw = pinned(self.data['source_manifest'])
        manifest = whole_program._manifest(raw, self.data)
        root = path_check(manifest['source_root'])
        need(root == Path(self.binding['cwd']), 'source_root_join')
        excluded = source_resource_join(self.phase, self.data,
            [r['directory']['path'] for r in self.reference['operator']['resource_roots']])['target_exclusion']
        before = source_inventory(str(root), excluded_target=excluded)
        whole_program.validate_source_inventory(before['files'], manifest['files'])
        git_pins = [r for r in self.binding['immutable_files'] if r['path'] == str(root / '.git')]
        need(len(git_pins) == 1 and git_pins[0] == before['git'], 'source_git_pin')
        hashes = {}
        for relative, expected in manifest['files'].items():
            whole_program.path(relative, relative=True)
            hashes[relative] = file_observation(root / relative, SOURCE_CAP)[0]['sha256']
            need(hashes[relative] == expected, 'source_hash_drift')
        need(source_inventory(str(root), excluded_target=excluded) == before, 'source_inventory_drift')
        self.manifest_raw, self.manifest = raw, manifest
        return hashes

    def dependencies(self, *, expected):
        need(self.manifest is not None and self.phase in ('base', 'candidate'), 'dependency_source_unbound')
        return observe_dependencies(self.manifest, self.data, reference=self.reference,
                                    phase_binding_raw=self.binding_raw, expected=expected)

    def binary(self, path):
        full, _ = file_observation(path, GIB, executable=True)
        return {k: full[k] for k in ('path', 'sha256', 'dev', 'ino', 'size', 'mtime_ns')}

    def stable(self):
        self.sources()
        if self.binary_fact is not None:
            need(self.binary(self.binary_fact['path']) == self.binary_fact, 'binary_drift')
            need(file_observation(self.binary_fact['path'] + '.d', 512 * 1024)[0] == self.depfile_fact,
                 'depfile_drift')
        if self.dependency_snapshot is not None:
            self.dependencies(expected=self.dependency_snapshot)

    def captured(self, slot, raw):
        need(slot in self.captures and type(raw) is bytes
             and digest(raw) == self.captures[slot]['log_sha256']
             and len(raw) == self.captures[slot]['log_bytes'], 'capture_input_unbound')
        return self.captures[slot]

    def commands(self):
        return whole_program.commands(self.phase, self.data,
            {'cargo': self.binding['cargo'], 'rustfmt': self.binding['rustfmt']},
            reference=self.reference, phase_binding_raw=self.binding_raw,
            executable=None if self.binary_fact is None else self.binary_fact['path'])

    def capture(self, slot, argv, *, useful_seconds, exit_seconds):
        """Exclusive capture; failure encumbers the shared state without retry."""
        require_umask()
        directory(self.state_spec)
        fd = os.open(self.state_root / 'LOCK', os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW
                     | os.O_CLOEXEC, 0o600)
        try:
            info = os.fstat(fd)
            need(stat.S_ISREG(info.st_mode) and info.st_nlink == 1
                 and info.st_uid == os.getuid() and stat.S_IMODE(info.st_mode) == 0o600,
                 'lock_identity')
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            try:
                return self._capture(slot, argv, useful_seconds=useful_seconds,
                                     exit_seconds=exit_seconds)
            except (Exception, KeyboardInterrupt) as error:
                code = str(error) if isinstance(error, Stop) else 'adapter_capture_failure'
                if not re.fullmatch(r'[a-z0-9_]{1,80}', code):
                    code = 'closed_failure'
                resource_error = resource_error_record(error) if isinstance(error, ResourcePathError) else None
                try:
                    persist(self.state_root / 'STOP.json',
                        terminal_stop_record(self.phase, code, self.spent,
                            str(time.monotonic_ns()), resource_error=resource_error),
                        state_root=self.state_root)
                except (Exception, KeyboardInterrupt):
                    raise Stop('terminal_receipt_unavailable') from None
                if isinstance(error, Stop):
                    raise
                raise Stop(code) from None
        finally:
            os.close(fd)

    def _capture(self, slot, argv, *, useful_seconds, exit_seconds):
        """One durable START then bounded lineage/capture; caller owns admission."""
        self.verify_inputs()
        slots = self.reservation['phases'][self.phase]['slots']
        need(type(slot) is str and slot in slots, 'slot_unbound')
        index = slots.index(slot)
        if self.phase in ('base', 'candidate'):
            need(index == 0 or self.binary_fact is not None, 'binary_unbound')
            need(index != 2 or self.provenance is not None, 'pre_witness_unbound')
        expected_commands = self.commands()
        command_index = index if self.phase in ('base', 'candidate') else 0
        need(command_index < len(expected_commands) and type(argv) in (list, tuple)
             and tuple(argv) == expected_commands[command_index], 'command_join')
        need(type(useful_seconds) is int and 0 < useful_seconds <= self.reservation['slots'][slot]
             and type(exit_seconds) is int and exit_seconds == self.reservation['exit_seconds'],
             'slot_bound')
        self.resources(initial=True)
        if self.phase != 'format':
            self.stable()
        if self.phase in ('base', 'candidate') and index == 0:
            self.dependency_snapshot = self.dependencies(expected=None)
            with os.scandir(path_check(self.data['target_root'])) as items:
                need(next(items, None) is None, 'target_not_fresh')
        cap = capture_budget(retained_bytes(self.state_root))
        start = {'schema_version': 'pbdep.slot-start.v1', 'phase': self.phase, 'slot': slot,
            'argv': list(argv), 'cwd': self.binding['cwd'],
            'reference_sha256': digest(encode(self.reference)), 'binding_sha256': digest(self.binding_raw),
            'useful_seconds': useful_seconds, 'exit_seconds': exit_seconds, 'raw_cap': cap,
            'recorded_monotonic_ns': str(time.monotonic_ns()), 'slot_spent_even_if_fork_fails': True}
        self.spent.append(slot)
        consume_slot(self.phase_root, self.phase, slot, start,
                     reservation=self.reservation, state_root=self.state_root)
        if self.phase != 'format':
            self.stable()
        if self.phase == 'format':
            self.persist('FORMAT-BEFORE.json', {
                record['root'] + '/' + record['relative']: file_observation(
                    Path(record['root']) / record['relative'], SOURCE_CAP)[0]
                for record in self.data['files']})
        before = time.monotonic_ns()
        raw_receipt = supervise.run(argv, self.binding['cwd'], self.binding['environment'],
            str(self.phase_root / (slot + '.log')), useful=useful_seconds, grace=exit_seconds,
            kill_after=10, log_cap=cap, resource_check=self.resources)
        after = time.monotonic_ns()
        sync_directory(self.phase_root)
        self.persist(slot + '-RAW.json', {'schema_version': 'pbdep.raw-custody.v1',
            'slot': slot, 'wrapper_before_run_monotonic_ns': str(before),
            'wrapper_after_run_monotonic_ns': str(after), 'boundary_is_not_exact_fork': True,
            'stream': 'merged-stdout-stderr', 'manager_status': 'NOT_OBSERVED_BY_ADAPTER',
            'supervisor': raw_receipt, 'aggregate_resource_state_at_terminal_write': 'UNKNOWN',
            'resource_samples': self.resource_facts,
            'last_sampled_resources': self.last_resource_scan})
        _, raw = file_observation(self.phase_root / (slot + '.log'), cap, retain=True)
        normalized = normalize_capture(raw_receipt, raw, before, after, 0)
        self.persist(slot + '-NORMALIZED.json', normalized)
        self.captures[slot] = normalized
        if self.phase == 'format':
            self.format_completed = True
            self.persist('FORMAT-AFTER.json', {
                record['root'] + '/' + record['relative']: file_observation(
                    Path(record['root']) / record['relative'], SOURCE_CAP)[0]
                for record in self.data['files']})
        if self.phase != 'format':
            self.stable()
        return {'raw': raw, 'capture': normalized, 'supervisor': raw_receipt,
                'manager_status': 'NOT_OBSERVED_BY_ADAPTER',
                'aggregate_resource_state_at_terminal_write': 'UNKNOWN'}

    def compilation(self, compiler_raw):
        need(self.phase in ('base', 'candidate'), 'compilation_phase')
        self.captured(self.reservation['phases'][self.phase]['slots'][0], compiler_raw)
        self.sources()
        executable = contract._compiler(compiler_raw, self.data['source_root'], self.data['target_root'])
        actual = self.binary(executable)
        depfile, depraw = file_observation(executable + '.d', 512 * 1024, retain=True)
        need(self.dependency_snapshot is not None, 'dependency_preimage_unbound')
        observed = self.dependencies(expected=self.dependency_snapshot)
        fact = whole_program.validate_compilation(compiler_raw, depraw, manifest_raw=self.manifest_raw,
            data=self.data, observed_binary=actual, observed_dependencies=observed,
            reference=self.reference, phase_binding_raw=self.binding_raw)
        self.compilation_fact, self.binary_fact = fact, actual
        self.depfile_fact, self.dependency_snapshot = depfile, observed
        self.stable()
        return fact

    def inventory(self, listing_raw):
        need(self.compilation_fact is not None, 'compilation_unbound')
        self.captured(self.reservation['phases'][self.phase]['slots'][1], listing_raw)
        self.stable()
        fact = whole_program.validate_inventory(listing_raw,
            full_roster=pinned(self.data['full_roster']), selected_roster=pinned(self.data['selected_roster']),
            case_map=self.data['case_map'], data=self.data, reference=self.reference,
            phase_binding_raw=self.binding_raw)
        self.inventory_fact = fact
        return fact

    def pre_witness(self, compiler_raw, listing_raw):
        need(self.compilation_fact is not None and self.inventory_fact is not None, 'inventory_unbound')
        slots = self.reservation['phases'][self.phase]['slots']
        compiled, listed = self.captured(slots[0], compiler_raw), self.captured(slots[1], listing_raw)
        self.stable()
        joins = {k: self.compilation_fact[k] for k in ('manifest_sha256', 'subject_head', 'source_root',
                 'dep_binding_schema', 'dep_bindings_sha256', 'dependency_observation', 'reference_sha256')}
        admission = {'schema_version': 'pbdep.repaired-admission.v3', **joins,
            'data_sha256': digest(encode(self.data)), 'binary': self.binary_fact,
            'source_hashes': self.sources(), 'depfile_sha256': self.depfile_fact['sha256'],
            'compiler_log_sha256': digest(compiler_raw), 'listing_sha256': digest(listing_raw),
            'compile': compiled, 'inventory': listed, 'recorded_monotonic_ns': str(time.monotonic_ns())}
        self.persist('ADMISSION.json', admission)
        whole_program.validate_admission(compiler_raw, listing_raw, admission,
            manifest_raw=self.manifest_raw, data=self.data, compilation=self.compilation_fact,
            inventory=self.inventory_fact, reference=self.reference, phase_binding_raw=self.binding_raw)
        self.stable()
        pre = {'schema_version': 'pbdep.repaired-pre-witness.v3', **joins,
            'admission_sha256': digest(encode(admission)), 'data_sha256': digest(encode(self.data)),
            'binary': self.binary_fact, 'source_hashes': self.sources(),
            'depfile_sha256': self.depfile_fact['sha256'], 'recorded_monotonic_ns': str(time.monotonic_ns())}
        self.persist('PRE-WITNESS.json', pre)
        fact = whole_program.validate_pre_witness(compiler_raw, listing_raw, admission, pre,
            manifest_raw=self.manifest_raw, data=self.data, compilation=self.compilation_fact,
            inventory=self.inventory_fact, reference=self.reference, phase_binding_raw=self.binding_raw)
        self.provenance = fact
        return fact

    def completion(self, witness_raw):
        need(self.provenance is not None, 'pre_witness_unbound')
        capture = self.captured(self.reservation['phases'][self.phase]['slots'][2], witness_raw)
        self.stable()
        return whole_program.validate_completion(witness_raw, capture, provenance=self.provenance,
            observed_binary=self.binary(self.binary_fact['path']), observed_sources=self.sources(),
            observed_depfile_sha256=file_observation(self.binary_fact['path'] + '.d', 512 * 1024)[0]['sha256'],
            observed_dependencies=self.dependencies(expected=self.dependency_snapshot), data=self.data,
            manifest_raw=self.manifest_raw, reference=self.reference, phase_binding_raw=self.binding_raw)

    def result(self, raw):
        """Narrow parser result; never a program completion or resource PASS."""
        need(self.phase in ('format', 'integration', 'consumers', 'gate'), 'result_phase')
        self.captured(self.reservation['phases'][self.phase]['slots'][0], raw)
        self.verify_inputs()
        if self.phase == 'format':
            return whole_program.validate_format(raw, exit_code=0)
        self.stable()
        if self.phase == 'gate':
            return whole_program.validate_gate(raw, data=self.data, exit_code=0,
                reference=self.reference, phase_binding_raw=self.binding_raw)
        return whole_program.validate_cargo_tests(raw, data=self.data, exit_code=0,
            reference=self.reference, phase_binding_raw=self.binding_raw)
