"""Closed, pure Phase B later-phase contracts; no dispatch or authority.

Semantics reside in independently reviewed product assertions/vectors. These
checks establish exact input/command/receipt/roster and completion joins only.
Independent reference inputs are mandatory; self-consistent data grants nothing.
No I/O, environment, live clock, import of a runtime, or dispatch belongs here.
"""
import hashlib
import json
import posixpath
import re
from functools import wraps

from . import contract

PHASES = ('qualification', 'a2', 'format', 'base', 'candidate', 'integration', 'consumers', 'gate')
CONSUMERS = ('artifact', 'artifact_occurrence_migration', 'evaluation',
    'candidate_evaluation_packet', 'evidence_span', 'outcome', 'outcome_collector',
    'outcome_prepare', 'outcome_stores', 'outcome_closure', 'outcome_pdf', 'local_transport')
GROUPS = tuple('V%02d' % i for i in range(6)) + tuple('C%02d' % i for i in range(1, 9)) + tuple(
    'S%02d' % i for i in range(1, 9)) + tuple('K%02d' % i for i in range(1, 6)) + tuple(
    'M%02d' % i for i in range(1, 5))
PREFIX = 'artifact::artifact_phase_b_contract::'
CAP = 16 * 1024 * 1024
HEX = re.compile(r'[0-9a-f]{64}\Z')
NAME = re.compile(r'[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*\Z')
CHECKS = ('stage sidecar', 'cargo fmt', 'cargo clippy', 'cargo test', 'cargo build')
REFERENCE_VERSION = 'pbdep.reference.v1'
EVIDENCE_CONTRACT = 'pbdep.r2-evidence.v1'
INSTALLED_ROLES = ('mkdir', 'wc', 'dirname', 'env')


class Rejection(ValueError):
    def __init__(self, code, *, provenance='deliberate-contract'):
        self.code = code
        self.provenance = provenance
        super().__init__(code)


def closed_input(function):
    """Malformed public input is a typed refusal, never an incidental crash."""
    @wraps(function)
    def checked(*args, **kwargs):
        try:
            return function(*args, **kwargs)
        except Rejection:
            raise
        except (TypeError, KeyError, IndexError, AttributeError, UnicodeError,
                OverflowError, ValueError, RecursionError):
            raise Rejection('SHAPE', provenance='converted-exception') from None
    return checked


def need(value, code):
    if not value:
        raise Rejection(code)


def keys(value, fields):
    need(type(value) is dict and set(value) == set(fields), 'SHAPE')
    return value


def encoded(value):
    try:
        return (json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False) + '\n').encode()
    except (TypeError, ValueError, UnicodeError):
        raise Rejection('SHAPE', provenance='converted-exception') from None


def sha(raw):
    need(type(raw) is bytes, 'TYPE')
    return hashlib.sha256(raw).hexdigest()


def hash_value(value):
    need(type(value) is str and HEX.fullmatch(value) is not None, 'PIN')
    return value


def path(value, *, relative=False):
    need(type(value) is str and 0 < len(value) <= 1024
         and re.fullmatch(r'[A-Za-z0-9_./+-]+', value) is not None
         and posixpath.normpath(value) == value and '..' not in value.split('/'), 'PATH')
    need((not value.startswith('/') and value != '.') if relative else
         value.startswith('/') and not value.startswith('//') and value != '/', 'PATH')
    return value


def pin(value):
    keys(value, ('path', 'sha256'))
    path(value['path'])
    hash_value(value['sha256'])
    return value


def number(value, maximum=100000, minimum=0):
    need(type(value) is int and minimum <= value <= maximum, 'NUMBER')
    return value


def ns(value):
    need(type(value) is str and re.fullmatch(r'0|[1-9][0-9]{0,23}', value), 'NUMBER')
    return int(value)


def text(raw, cap=CAP):
    need(type(raw) is bytes and 0 < len(raw) <= cap, 'BOUND')
    try:
        value = raw.decode('utf-8')
    except UnicodeError:
        raise Rejection('TEXT') from None
    need('\r' not in value and '\0' not in value and value.endswith('\n'), 'TEXT')
    need(all(len(line.encode()) <= 65536 for line in value.splitlines()), 'BOUND')
    return value


def names(value, *, empty=False):
    need(type(value) is list and (empty or value) and len(value) <= 20000, 'ROSTER')
    need(all(type(n) is str and len(n) <= 512 and NAME.fullmatch(n) for n in value), 'ROSTER')
    need(value == sorted(set(value)), 'ROSTER')
    return value


def roster(raw):
    return names(text(raw, 1024 * 1024)[:-1].split('\n'))


def source_map(value):
    need(type(value) is dict and 1 <= len(value) <= 2048, 'SOURCE')
    for name, digest in value.items():
        path(name, relative=True)
        hash_value(digest)
    return value


def dependency_name(token):
    """Measured compiler token grammar; never weaken general path()."""
    need(type(token) is str and 0 < len(token) <= 1024
         and re.fullmatch(r'[A-Za-z0-9_./+-]+', token) is not None
         and not token.startswith('/'), 'DEP_PATH')
    stack = []
    for part in token.split('/'):
        need(part not in ('', '.'), 'DEP_PATH')
        if part == '..':
            need(bool(stack), 'DEP_ESCAPE')
            stack.pop()
        else:
            stack.append(part)
    name = '/'.join(stack)
    need(bool(name) and len(name) <= 512, 'DEP_PATH')
    return path(name, relative=True)


@closed_input
def validate_dependency_bindings(tokens, schema, bindings, *, sources=None):
    need(schema == 'phase-b.dep-binding.v1' and type(schema) is str, 'DEP_VERSION')
    need(type(tokens) is list and 1 <= len(tokens) <= 2048
         and type(bindings) is list and len(bindings) == len(tokens), 'DEP_BINDING')
    canonical = []
    for token, record in zip(tokens, bindings):
        name = dependency_name(token)
        keys(record, ('raw_token', 'canonical_relative', 'source_sha256'))
        need(type(record['raw_token']) is str and record['raw_token'] == token
             and type(record['canonical_relative']) is str
             and record['canonical_relative'] == name, 'DEP_BINDING')
        hash_value(record['source_sha256'])
        canonical.append(name)
    need(len(set(tokens)) == len(tokens), 'DEP_DUPLICATE')
    need(len(set(canonical)) == len(canonical), 'DEP_ALIAS')
    if sources is not None:
        source_map(sources)
        for record in bindings:
            need(record['canonical_relative'] in sources, 'DEP_SOURCE')
            need(record['source_sha256'] == sources[record['canonical_relative']], 'DEP_DIGEST')
    return {'scope': 'supplied-dependency-map-only', 'dep_binding_schema': schema,
            'dep_bindings_sha256': sha(encoded(bindings)), 'canonical_names': canonical}


@closed_input
def validate_dependency_artifact(raw, *, source_root, executable, dep_tokens,
                                 dep_binding_schema, dep_bindings, source_hashes):
    """Exact retained grammar and supplied map only, never filesystem proof."""
    path(source_root)
    path(executable)
    mapping = validate_dependency_bindings(dep_tokens, dep_binding_schema, dep_bindings,
                                          sources=source_hashes)
    dependencies = ' '.join(dep_tokens)
    expected = (executable + '.d: ' + dependencies + '\n\n' + executable + ': ' + dependencies
        + '\n\n' + ''.join(t + ':\n' for t in dep_tokens)
        + '\n# env-dep:CARGO_MANIFEST_DIR=' + source_root + '/crates/beacond\n'
        + '# env-dep:CARGO_PKG_VERSION=0.1.0\n')
    need(text(raw, 512 * 1024) == expected, 'DEPFILE')
    return {**mapping, 'scope': 'dependency-artifact-and-supplied-map-only',
            'depfile_sha256': sha(raw)}


@closed_input
def validate_dependency_observation(value, *, data, sources):
    """Validate supplied observation joins; runtime owns actual observation."""
    keys(value, ('schema_version', 'source_root', 'subject_head', 'manifest_sha256',
                 'dep_binding_schema', 'dep_bindings_sha256', 'directories', 'files'))
    mapping = validate_dependency_bindings(data['dep_tokens'], data['dep_binding_schema'],
                                          data['dep_bindings'], sources=sources)
    need(value['schema_version'] == 'phase-b.dep-observation.v1'
         and value['subject_head'] == data['subject_head']
         and value['manifest_sha256'] == data['manifest_sha256']
         and value['dep_binding_schema'] == mapping['dep_binding_schema']
         and value['dep_bindings_sha256'] == mapping['dep_bindings_sha256'], 'DEP_OBSERVATION')
    root = value['source_root']
    keys(root, ('path', 'dev', 'ino', 'mode', 'uid'))
    need(root['path'] == data['source_root'], 'ROOT_JOIN')
    for field in ('dev', 'ino', 'mode', 'uid'):
        ns(root[field])
    need(ns(root['ino']) > 0, 'DEP_OBSERVATION')
    directory_names = set()
    for token in data['dep_tokens']:
        stack = []
        for part in token.split('/')[:-1]:
            if part == '..':
                stack.pop()
            else:
                stack.append(part)
                directory_names.add('/'.join(stack))
    keys(value['directories'], directory_names)
    for name, record in value['directories'].items():
        keys(record, ('path', 'dev', 'ino', 'mode', 'uid'))
        need(record['path'] == data['source_root'] + '/' + name, 'DEP_OBSERVATION')
        for field in ('dev', 'ino', 'mode', 'uid'):
            ns(record[field])
    keys(value['files'], mapping['canonical_names'])
    for name, record in value['files'].items():
        keys(record, ('path', 'sha256', 'dev', 'ino', 'size', 'mtime_ns', 'mode', 'uid', 'nlink'))
        need(record['path'] == data['source_root'] + '/' + name
             and record['sha256'] == sources[name] and record['nlink'] == '1', 'DEP_OBSERVATION')
        for field in ('dev', 'ino', 'size', 'mtime_ns', 'mode', 'uid', 'nlink'):
            ns(record[field])
    return value



@closed_input
def validate_reference(reference):
    """Shape of independent caller reference; never authenticates its origin."""
    keys(reference, ('schema_version', 'evidence_contract', 'operator',
                     'phase_data_sha256', 'installed_multicall'))
    need(reference['schema_version'] == REFERENCE_VERSION
         and reference['evidence_contract'] == EVIDENCE_CONTRACT, 'REFERENCE_VERSION')
    _operator_shape(reference['operator'])
    keys(reference['phase_data_sha256'], PHASES[2:])
    for value in reference['phase_data_sha256'].values():
        if value != 'UNBOUND':
            hash_value(value)
    keys(reference['installed_multicall'], INSTALLED_ROLES)
    paths = []
    for role, record in reference['installed_multicall'].items():
        keys(record, ('path', 'sha256', 'dev', 'ino', 'size', 'mtime_ns', 'mode', 'uid', 'nlink'))
        path(record['path'])
        need(posixpath.basename(record['path']) == role, 'INSTALLED_TOOL_IDENTITY')
        hash_value(record['sha256'])
        for field in ('dev', 'ino', 'size', 'mtime_ns', 'mode', 'uid', 'nlink'):
            ns(record[field])
        need(record['uid'] == '0' and record['mode'] == '493'
             and ns(record['ino']) > 0 and ns(record['size']) > 0
             and ns(record['nlink']) > 0, 'INSTALLED_TOOL_IDENTITY')
        paths.append(record['path'])
    need(len(set(paths)) == len(INSTALLED_ROLES), 'INSTALLED_TOOL_IDENTITY')
    return {'scope': 'independent-reference-shape-only',
            'reference_sha256': sha(encoded(reference))}


def _manifest(raw, data):
    """Raw manifest digest join; caller still owns observation and custody."""
    need(sha(raw) == data['manifest_sha256'], 'SOURCE')
    try:
        value = contract._json(raw, contract.MAX_JSON)
    except contract.Rejection:
        raise Rejection('SOURCE') from None
    keys(value, ('schema_version', 'subject_head', 'source_root', 'files'))
    need(value['schema_version'] == 'phase-b.source-manifest.v1'
         and value['subject_head'] == data['subject_head']
         and value['source_root'] == data['source_root'], 'SOURCE')
    source_map(value['files'])
    return value


def binary(value):
    keys(value, ('path', 'sha256', 'dev', 'ino', 'size', 'mtime_ns'))
    path(value['path'])
    hash_value(value['sha256'])
    for name in ('dev', 'ino', 'size', 'mtime_ns'):
        need(ns(value[name]) > 0, 'BINARY')
    need(int(value['size']) <= 1024 ** 3, 'BINARY')
    return value


@closed_input
def validate_installed_tool(record, *, file_class, reference):
    """Supplied installed identity only, never a filesystem observation."""
    validate_reference(reference)
    installed = {r['path']: r for r in reference['installed_multicall'].values()}
    keys(record, ('path', 'sha256', 'dev', 'ino', 'size', 'mtime_ns', 'mode', 'uid', 'nlink'))
    path(record['path'])
    hash_value(record['sha256'])
    for field in ('dev', 'ino', 'size', 'mtime_ns', 'mode', 'uid', 'nlink'):
        ns(record[field])
    need(type(file_class) is str and file_class in ('ordinary-input', 'immutable-installed-tool'), 'FILE_CLASS')
    if record['nlink'] == '1' and not (file_class == 'immutable-installed-tool'
                                     and record['path'] in installed):
        return {'scope': 'file-link-policy-only', 'link_policy': 'ordinary-single-link', 'path': record['path']}
    need(file_class == 'immutable-installed-tool'
         and record['path'] in installed
         and record == installed[record['path']],
         'INSTALLED_TOOL_IDENTITY')
    return {'scope': 'file-link-policy-only', 'link_policy': 'literal-installed-multicall', 'path': record['path']}


@closed_input
def validate_source_inventory(actual_names, expected_hash_map):
    expected = source_map(expected_hash_map)
    need(type(actual_names) is list and 1 <= len(actual_names) <= 4096, 'SOURCE_INVENTORY')
    for name in actual_names:
        path(name, relative=True)
    need(actual_names == sorted(set(actual_names))
         and actual_names == sorted(expected), 'SOURCE_INVENTORY')
    return {'scope': 'source-inventory-only', 'file_count': len(actual_names),
            'inventory_sha256': sha(encoded(actual_names))}


@closed_input
def validate_selection_inputs(value):
    keys(value, ('absent_paths', 'aliases'))
    need(type(value['absent_paths']) is list and len(value['absent_paths']) <= 128
         and type(value['aliases']) is list and len(value['aliases']) <= 64, 'SELECTION')
    absent = [path(name) for name in value['absent_paths']]
    need(len(set(absent)) == len(absent), 'SELECTION')
    aliases = []
    for alias in value['aliases']:
        keys(alias, ('path', 'resolved_path'))
        aliases.append(path(alias['path']))
        path(alias['resolved_path'])
    need(len(set(aliases)) == len(aliases) and not set(aliases) & set(absent), 'SELECTION')
    return {'scope': 'selection-input-shape-only', 'selection_sha256': sha(encoded(value))}


def _capture(value, raw):
    try:
        return contract._capture(value, raw, 0)
    except contract.Rejection:
        raise Rejection('CUSTODY') from None


@closed_input
def _operator_shape(value):
    keys(value, ('schema_version', 'authority', 'packet', 'acceptance', 'review',
        'disposition', 'operator_attestation', 'instrument_sha256', 'phase_bindings', 'resource_roots'))
    need(value['schema_version'] == 'pbdep.operator-binding.v3', 'OPERATOR')
    for role in ('authority', 'packet', 'acceptance', 'review', 'disposition'):
        pin(value[role])
    attestation = value['operator_attestation']
    keys(attestation, ('acceptance_read_by', 'issuer', 'decision', 'accepted_packet_sha256',
                      'approval_is_external_record_not_this_json'))
    for field in ('acceptance_read_by', 'issuer'):
        need(type(attestation[field]) is str and 0 < len(attestation[field]) <= 128
             and NAME.fullmatch(attestation[field]) is not None
             and attestation[field] not in ('UNKNOWN', 'UNBOUND'), 'AUTHORITY')
    need(attestation['decision'] == 'ACCEPT_WHOLE_PROGRAM_INSTRUMENT'
         and attestation['accepted_packet_sha256'] == value['packet']['sha256']
         and attestation['approval_is_external_record_not_this_json'] is True, 'AUTHORITY')
    need(type(value['instrument_sha256']) is dict
         and 1 <= len(value['instrument_sha256']) <= 64
         and all(type(n) is str and re.fullmatch(r'[a-z_][a-z_0-9]*\.py', n)
                 for n in value['instrument_sha256']), 'BINDING')
    for digest in value['instrument_sha256'].values():
        hash_value(digest)
    keys(value['phase_bindings'], PHASES)
    for entry in value['phase_bindings'].values():
        need(type(entry) is dict, 'BINDING')
        if entry.get('state') == 'UNBOUND':
            keys(entry, ('state',))
        else:
            keys(entry, ('state', 'path', 'sha256'))
            need(entry['state'] == 'BOUND', 'BINDING')
            pin({'path': entry['path'], 'sha256': entry['sha256']})
    need(type(value['resource_roots']) is list and 1 <= len(value['resource_roots']) <= 12, 'RESOURCE')
    for root in value['resource_roots']:
        keys(root, ('directory', 'kind'))
        need(type(root['kind']) is str and root['kind'] in ('workspace', 'temporary'), 'RESOURCE')
        keys(root['directory'], ('path', 'dev', 'ino', 'uid', 'mode'))
        path(root['directory']['path'])
        for field in ('dev', 'ino', 'uid', 'mode'):
            ns(root['directory'][field])
        need(root['directory']['mode'] == '448', 'RESOURCE')
    return {'scope': 'operator-record-shape-only', 'operator_sha256': sha(encoded(value))}


@closed_input
def validate_operator(value, *, reference):
    validate_reference(reference)
    _operator_shape(value)
    need(value == reference['operator'], 'REFERENCE_DRIFT')
    return {'scope': 'operator-record-shape-only', 'operator_sha256': sha(encoded(value)),
            'reference_sha256': sha(encoded(reference))}


@closed_input
def binding_pin(operator, phase, *, reference):
    validate_operator(operator, reference=reference)
    need(type(phase) is str and phase in PHASES, 'PHASE')
    entry = operator['phase_bindings'][phase]
    need(entry['state'] == 'BOUND', 'NOT_BOUND')
    return {'path': entry['path'], 'sha256': entry['sha256']}


@closed_input
def validate_continuation(previous, current, completed, *, previous_reference, reference):
    validate_operator(previous, reference=previous_reference)
    validate_operator(current, reference=reference)
    need(previous_reference['installed_multicall'] == reference['installed_multicall'], 'REFERENCE_DRIFT')
    need(type(completed) is tuple and completed == PHASES[:len(completed)], 'ORDER')
    for phase in completed:
        need(previous_reference['phase_data_sha256'].get(phase)
             == reference['phase_data_sha256'].get(phase), 'REFERENCE_DRIFT')
    need(type(completed) is tuple and completed == PHASES[:len(completed)], 'ORDER')
    for key in previous:
        if key != 'phase_bindings':
            need(previous[key] == current[key], 'STABLE_BINDING_DRIFT')
    for phase in completed:
        need(previous['phase_bindings'][phase] == current['phase_bindings'][phase]
             and current['phase_bindings'][phase]['state'] == 'BOUND', 'COMPLETED_BINDING_DRIFT')
    return {'scope': 'stable-program-continuation-only', 'completed': list(completed)}



def _same_json_data(value, expected):
    """Exact JSON value and type join; bool/int and tuple/list never alias."""
    need(type(value) is type(expected), 'PHASE_DATA')
    if type(expected) is dict:
        need(all(type(key) is str for key in value)
             and set(value) == set(expected), 'PHASE_DATA')
        for key in expected:
            _same_json_data(value[key], expected[key])
    elif type(expected) is list:
        need(len(value) == len(expected), 'PHASE_DATA')
        for item, expected_item in zip(value, expected):
            _same_json_data(item, expected_item)
    else:
        need(value == expected, 'PHASE_DATA')


@closed_input
def validate_phase_data(phase, data, *, reference, phase_binding_raw):
    validate_reference(reference)
    need(type(phase) is str and phase in PHASES[2:], 'PHASE')
    phase_pin = binding_pin(reference['operator'], phase, reference=reference)
    need(type(phase_binding_raw) is bytes
         and 0 < len(phase_binding_raw) <= contract.MAX_JSON, 'BOUND')
    need(sha(phase_binding_raw) == phase_pin['sha256'], 'PHASE_BINDING')
    try:
        binding = contract._json(phase_binding_raw, contract.MAX_JSON)
    except contract.Rejection:
        raise Rejection('PHASE_BINDING') from None
    # Retained later-phase envelope shape; execution policies remain caller-owned.
    keys(binding, ('schema_version', 'phase', 'authority_pins', 'cwd', 'environment',
                   'python', 'immutable_files', 'tmp', 'selection_inputs', 'cargo',
                   'rustfmt', 'data', 'step_zero_acceptance', 'source_freeze_review',
                   'operator_phase_attestation'))
    need(binding['schema_version'] == 'phase-b.phase-binding.v2'
         and binding['phase'] == phase, 'PHASE_BINDING')
    _same_json_data(data, binding['data'])
    expected = reference['phase_data_sha256'][phase]
    need(expected != 'UNBOUND' and sha(encoded(data)) == expected, 'REFERENCE_DRIFT')
    if phase == 'format':
        keys(data, ('phase', 'roots', 'files', 'allowlist'))
        need(data['phase'] == phase and type(data['roots']) is list
             and len(data['roots']) == len(set(data['roots'])) == 2, 'FORMAT')
        for root in data['roots']:
            path(root)
        need(type(data['allowlist']) is list and data['allowlist']
             and len(data['allowlist']) <= 128 and data['allowlist'] == sorted(set(data['allowlist'])), 'FORMAT')
        for relative in data['allowlist']:
            need(path(relative, relative=True).endswith('.rs'), 'FORMAT')
        need(type(data['files']) is list and 2 <= len(data['files']) <= 256, 'FORMAT')
        ordered = []
        for record in data['files']:
            keys(record, ('root', 'relative', 'sha256'))
            need(record['root'] in data['roots'] and record['relative'] in data['allowlist'], 'FORMAT')
            hash_value(record['sha256'])
            ordered.append(record['root'] + '/' + record['relative'])
        need(ordered == sorted(set(ordered)) and {r['root'] for r in data['files']} == set(data['roots']), 'FORMAT')
        return {'scope': 'phase-data-only', 'phase': phase, 'data_sha256': sha(encoded(data))}
    common = {'phase', 'subject_head', 'source_root', 'target_root', 'source_manifest', 'manifest_sha256'}
    extra = {'full_roster', 'selected_roster', 'case_map', 'dep_tokens',
             'dep_binding_schema', 'dep_bindings'} if phase in ('base', 'candidate') else {'targets'}
    if phase == 'gate':
        extra |= {'gate_header', 'gate_sha256'}
    keys(data, common | extra)
    need(data['phase'] == phase and type(data['subject_head']) is str
         and re.fullmatch(r'[0-9a-f]{40}', data['subject_head']), 'SUBJECT')
    source = path(data['source_root'])
    target = path(data['target_root'])
    need(source != target and not source.startswith(target + '/') and not target.startswith(source + '/'), 'ROOT_JOIN')
    pin(data['source_manifest'])
    need(hash_value(data['manifest_sha256']) == data['source_manifest']['sha256'], 'SOURCE')
    if phase in ('base', 'candidate'):
        pin(data['full_roster'])
        pin(data['selected_roster'])
        keys(data['case_map'], GROUPS)
        selected = list(data['case_map'].values())
        need(len(set(selected)) == 31 and all(type(n) is str and NAME.fullmatch(n)
             and n.startswith(PREFIX) for n in selected), 'CASE_MAP')
        validate_dependency_bindings(data['dep_tokens'], data['dep_binding_schema'], data['dep_bindings'])
    else:
        targets = data['targets']
        need(type(targets) is list and 1 <= len(targets) <= 256, 'TARGETS')
        identities = []
        for item in targets:
            keys(item, ('kind', 'source', 'binary_prefix', 'names'))
            need(item['kind'] in ('unittests', 'tests'), 'TARGETS')
            path(item['source'], relative=True)
            need(type(item['binary_prefix']) is str and re.fullmatch(r'[A-Za-z_][A-Za-z0-9_]*', item['binary_prefix']), 'TARGETS')
            names(item['names'], empty=phase == 'gate')
            identities.append((item['kind'], item['source'], item['binary_prefix']))
        need(len(set(identities)) == len(identities), 'TARGETS')
        if phase in ('integration', 'consumers'):
            expected = ('pilot_contract',) if phase == 'integration' else CONSUMERS
            need(set(identities) == {('tests', 'tests/' + n + '.rs', n) for n in expected}
                 and len(identities) == len(expected), 'TARGETS')
        if phase == 'gate':
            hash_value(data['gate_sha256'])
            need(type(data['gate_header']) is str, 'GATE')
            match = re.fullmatch(r'gate: [A-Za-z0-9_./+-]+ @ ([0-9a-f]{7,40})', data['gate_header'])
            need(match is not None and data['subject_head'].startswith(match.group(1)), 'GATE')
    return {'scope': 'phase-data-only', 'phase': phase, 'data_sha256': sha(encoded(data))}


@closed_input
def commands(phase, data, tools, *, reference, phase_binding_raw, executable=None):
    validate_phase_data(phase, data, reference=reference, phase_binding_raw=phase_binding_raw)
    keys(tools, ('cargo', 'rustfmt'))
    cargo, rustfmt = path(tools['cargo']), path(tools['rustfmt'])
    if phase == 'format':
        need(executable is None, 'COMMAND')
        return ((rustfmt, '--edition', '2024', '--config', 'skip_children=true',
                 *(r['root'] + '/' + r['relative'] for r in data['files'])),)
    if phase in ('base', 'candidate'):
        compile_argv = (cargo, 'test', '--locked', '--offline', '-p', 'beacond', '--lib', '--no-run', '--message-format=json')
        if executable is None:
            return (compile_argv,)
        path(executable)
        need(posixpath.dirname(executable) == data['target_root'] + '/debug/deps'
             and re.fullmatch(r'beacond-[0-9a-f]{16}', posixpath.basename(executable)), 'BINARY')
        return (compile_argv, (executable, '--list', '--format', 'terse'),
                (executable, PREFIX, '--nocapture', '--test-threads=1'))
    need(executable is None, 'COMMAND')
    if phase == 'gate':
        return ((data['source_root'] + '/scripts/gate', 'rust'),)
    selected = ('pilot_contract',) if phase == 'integration' else CONSUMERS
    argv = [cargo, 'test', '--locked', '--offline', '-p', 'beacond']
    for name in selected:
        argv += ['--test', name]
    return (tuple(argv + ['--', '--test-threads=1']),)


@closed_input
def validate_inventory(raw, *, full_roster, selected_roster, case_map, data, reference, phase_binding_raw):
    validate_phase_data(data.get('phase'), data, reference=reference, phase_binding_raw=phase_binding_raw)
    need(data['phase'] in ('base', 'candidate')
         and sha(full_roster) == data['full_roster']['sha256']
         and sha(selected_roster) == data['selected_roster']['sha256']
         and case_map == data['case_map'], 'REFERENCE_DRIFT')
    full, selected = roster(full_roster), roster(selected_roster)
    keys(case_map, GROUPS)
    need(set(case_map.values()) == set(selected) and len(selected) == 31
         and all(n.startswith(PREFIX) for n in selected), 'CASE_MAP')
    lines = text(raw, 1024 * 1024)[:-1].split('\n')
    need(all(line.endswith(': test') for line in lines), 'ROSTER')
    actual = names([line[:-6] for line in lines])
    need(actual == full and [n for n in full if n.startswith(PREFIX)] == selected, 'ROSTER')
    return {'scope': 'repaired-listing-only', 'listing_sha256': sha(raw),
        'full_count': len(full), 'selected_count': len(selected),
        'full_roster_sha256': sha(full_roster), 'selected_roster_sha256': sha(selected_roster),
        'case_map_sha256': sha(encoded(case_map))}


@closed_input
def validate_compilation(raw, depfile, *, manifest_raw, data, observed_binary,
                         observed_dependencies, reference, phase_binding_raw):
    validate_phase_data(data.get('phase'), data, reference=reference, phase_binding_raw=phase_binding_raw)
    manifest = _manifest(manifest_raw, data)
    need(data['phase'] in ('base', 'candidate'), 'PHASE')
    keys(manifest, ('schema_version', 'subject_head', 'source_root', 'files'))
    need(manifest['schema_version'] == 'phase-b.source-manifest.v1'
         and manifest['subject_head'] == data['subject_head']
         and manifest['source_root'] == data['source_root'], 'SOURCE')
    sources = source_map(manifest['files'])
    # Manifest pin concerns actual raw file bytes; runtime verifies that pin.
    required = {'Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', 'crates/beacond/Cargo.toml',
        'crates/beacond/src/lib.rs', 'crates/beacond/src/artifact.rs',
        'crates/beacond/src/artifact_phase_b_contract.rs'}
    if data['phase'] == 'candidate':
        required |= {'crates/beacond/src/' + n + '.rs' for n in ('pilot_authority', 'pilot_admission', 'pilot_store')}
    mapping = validate_dependency_bindings(data['dep_tokens'], data['dep_binding_schema'],
                                          data['dep_bindings'], sources=sources)
    need(required <= set(sources) and required - {'Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml',
         'crates/beacond/Cargo.toml'} <= set(mapping['canonical_names']), 'DEPFILE')
    try:
        executable = contract._compiler(raw, data['source_root'], data['target_root'])
    except contract.Rejection:
        raise Rejection('COMPILATION') from None
    actual = binary(observed_binary)
    need(actual['path'] == executable, 'BINARY')
    validate_dependency_artifact(depfile, source_root=data['source_root'], executable=executable,
        dep_tokens=data['dep_tokens'], dep_binding_schema=data['dep_binding_schema'],
        dep_bindings=data['dep_bindings'], source_hashes=sources)
    observation = validate_dependency_observation(observed_dependencies, data=data, sources=sources)
    return {'schema_version': 'pbdep.repaired-compilation.v3',
        'scope': 'repaired-compilation-only', 'binary': dict(actual), 'source_hashes': dict(sources),
        'depfile_sha256': sha(depfile), 'compiler_log_sha256': sha(raw),
        'manifest_sha256': data['manifest_sha256'], 'subject_head': data['subject_head'],
        'source_root': data['source_root'], 'reference_sha256': sha(encoded(reference)),
        'dep_binding_schema': data['dep_binding_schema'],
        'dep_bindings_sha256': mapping['dep_bindings_sha256'], 'dependency_observation': observation}


@closed_input
def validate_admission(compiler_raw, listing_raw, admission, *, manifest_raw, data,
                       compilation, inventory, reference, phase_binding_raw):
    validate_phase_data(data.get('phase'), data, reference=reference, phase_binding_raw=phase_binding_raw)
    need(data['phase'] in ('base', 'candidate'), 'PHASE')
    manifest = _manifest(manifest_raw, data)
    keys(manifest, ('schema_version', 'subject_head', 'source_root', 'files'))
    joins = ('manifest_sha256', 'subject_head', 'source_root', 'dep_binding_schema',
             'dep_bindings_sha256', 'dependency_observation', 'reference_sha256')
    keys(compilation, ('schema_version', 'scope', 'binary', 'source_hashes', 'depfile_sha256',
                       'compiler_log_sha256', *joins))
    binary(compilation['binary'])
    hash_value(compilation['depfile_sha256'])
    keys(inventory, ('scope', 'listing_sha256', 'full_count', 'selected_count',
                     'full_roster_sha256', 'selected_roster_sha256', 'case_map_sha256'))
    number(inventory['full_count'], 20000, 31)
    mapping = validate_dependency_bindings(data['dep_tokens'], data['dep_binding_schema'],
                                          data['dep_bindings'], sources=manifest['files'])
    validate_dependency_observation(compilation['dependency_observation'], data=data, sources=manifest['files'])
    need(compilation['schema_version'] == 'pbdep.repaired-compilation.v3'
         and compilation['scope'] == 'repaired-compilation-only'
         and compilation['reference_sha256'] == sha(encoded(reference))
         and compilation['source_root'] == data['source_root'] == manifest['source_root']
         and compilation['subject_head'] == manifest['subject_head']
         and manifest['schema_version'] == 'phase-b.source-manifest.v1'
         and compilation['dep_binding_schema'] == data['dep_binding_schema']
         and compilation['dep_bindings_sha256'] == mapping['dep_bindings_sha256']
         and inventory['scope'] == 'repaired-listing-only'
         and compilation['subject_head'] == data['subject_head']
         and compilation['manifest_sha256'] == data['manifest_sha256']
         and compilation['source_hashes'] == source_map(manifest['files'])
         and inventory['full_roster_sha256'] == data['full_roster']['sha256']
         and inventory['selected_roster_sha256'] == data['selected_roster']['sha256']
         and inventory['case_map_sha256'] == sha(encoded(data['case_map']))
         and inventory['selected_count'] == 31 and type(inventory['selected_count']) is int, 'PROVENANCE')
    keys(admission, ('schema_version', 'data_sha256', 'binary', 'source_hashes',
        'depfile_sha256', 'compiler_log_sha256', 'listing_sha256', 'compile', 'inventory',
        'recorded_monotonic_ns', *joins))
    need(admission['schema_version'] == 'pbdep.repaired-admission.v3'
         and admission['data_sha256'] == sha(encoded(data))
         and admission['manifest_sha256'] == data['manifest_sha256'], 'PROVENANCE')
    for field in ('binary', 'source_hashes', 'depfile_sha256', 'compiler_log_sha256', *joins):
        need(admission[field] == compilation[field], 'PROVENANCE')
    need(admission['listing_sha256'] == inventory['listing_sha256'] == sha(listing_raw)
         and admission['compiler_log_sha256'] == sha(compiler_raw), 'PROVENANCE')
    compiled, listed = _capture(admission['compile'], compiler_raw), _capture(admission['inventory'], listing_raw)
    need(ns(compiled['end_monotonic_ns']) <= ns(listed['start_monotonic_ns'])
         <= ns(listed['end_monotonic_ns']) <= ns(admission['recorded_monotonic_ns']), 'ORDER')
    return {'scope': 'repaired-admission-joins-only',
            'admission_sha256': sha(encoded(admission)),
            'reference_sha256': sha(encoded(reference))}


@closed_input
def validate_pre_witness(compiler_raw, listing_raw, admission, pre, *, manifest_raw, data,
                         compilation, inventory, reference, phase_binding_raw):
    validate_admission(compiler_raw, listing_raw, admission, manifest_raw=manifest_raw,
                       data=data, compilation=compilation, inventory=inventory, reference=reference,
                       phase_binding_raw=phase_binding_raw)
    joins = ('manifest_sha256', 'subject_head', 'source_root', 'dep_binding_schema',
             'dep_bindings_sha256', 'dependency_observation', 'reference_sha256')
    keys(pre, ('schema_version', 'admission_sha256', 'data_sha256', 'binary', 'source_hashes',
               'depfile_sha256', 'recorded_monotonic_ns', *joins))
    need(pre['schema_version'] == 'pbdep.repaired-pre-witness.v3'
         and pre['admission_sha256'] == sha(encoded(admission))
         and pre['data_sha256'] == sha(encoded(data))
         and ns(pre['recorded_monotonic_ns']) >= ns(admission['recorded_monotonic_ns']), 'PROVENANCE')
    for field in ('binary', 'source_hashes', 'depfile_sha256', *joins):
        need(pre[field] == admission[field], 'PROVENANCE')
    return {'schema_version': 'pbdep.repaired-provenance.v3',
        'scope': 'repaired-pre-dispatch-joins-only', 'data_sha256': sha(encoded(data)),
        'admission_sha256': sha(encoded(admission)), 'pre_sha256': sha(encoded(pre)),
        'pre_monotonic_ns': pre['recorded_monotonic_ns'], 'binary': binary(pre['binary']),
        'source_hashes': source_map(pre['source_hashes']), 'depfile_sha256': hash_value(pre['depfile_sha256']),
        'listing_sha256': sha(listing_raw), **{field: pre[field] for field in joins}}


@closed_input
def validate_completion(raw, capture, *, provenance, observed_binary, observed_sources,
                        observed_depfile_sha256, observed_dependencies, data, manifest_raw, reference,
                        phase_binding_raw):
    validate_phase_data(data.get('phase'), data, reference=reference, phase_binding_raw=phase_binding_raw)
    need(data['phase'] in ('base', 'candidate'), 'PHASE')
    manifest = _manifest(manifest_raw, data)
    need(type(raw) is bytes and 0 < len(raw) <= CAP, 'BOUND')
    validate_dependency_observation(observed_dependencies, data=data, sources=manifest['files'])
    joins = ('manifest_sha256', 'subject_head', 'source_root', 'dep_binding_schema',
             'dep_bindings_sha256', 'dependency_observation', 'reference_sha256')
    keys(provenance, ('schema_version', 'scope', 'data_sha256', 'admission_sha256', 'pre_sha256',
                      'pre_monotonic_ns', 'binary', 'source_hashes', 'depfile_sha256', 'listing_sha256', *joins))
    need(provenance['schema_version'] == 'pbdep.repaired-provenance.v3'
         and provenance['scope'] == 'repaired-pre-dispatch-joins-only'
         and provenance['reference_sha256'] == sha(encoded(reference))
         and provenance['data_sha256'] == sha(encoded(data))
         and provenance['manifest_sha256'] == data['manifest_sha256']
         and provenance['subject_head'] == data['subject_head']
         and provenance['source_root'] == data['source_root']
         and provenance['source_hashes'] == manifest['files'], 'PROVENANCE')
    keys(observed_dependencies, ('schema_version', 'source_root', 'subject_head', 'manifest_sha256',
                                'dep_binding_schema', 'dep_bindings_sha256', 'directories', 'files'))
    need(observed_dependencies['schema_version'] == 'phase-b.dep-observation.v1'
         and observed_dependencies['dep_binding_schema'] == 'phase-b.dep-binding.v1', 'DEP_VERSION')
    hash_value(observed_dependencies['manifest_sha256'])
    hash_value(observed_dependencies['dep_bindings_sha256'])
    root = observed_dependencies['source_root']
    keys(root, ('path', 'dev', 'ino', 'mode', 'uid'))
    path(root['path'])
    for field in ('dev', 'ino', 'mode', 'uid'):
        ns(root[field])
    need(type(observed_dependencies['files']) is dict
         and 1 <= len(observed_dependencies['files']) <= 2048
         and type(observed_dependencies['directories']) is dict, 'DEP_OBSERVATION')
    for name, record in observed_dependencies['files'].items():
        path(name, relative=True)
        keys(record, ('path', 'sha256', 'dev', 'ino', 'size', 'mtime_ns', 'mode', 'uid', 'nlink'))
        need(record['path'] == root['path'] + '/' + name and name in observed_sources
             and record['sha256'] == observed_sources[name] and record['nlink'] == '1', 'DEP_OBSERVATION')
        for field in ('dev', 'ino', 'size', 'mtime_ns', 'mode', 'uid', 'nlink'):
            ns(record[field])
    for name, record in observed_dependencies['directories'].items():
        path(name, relative=True)
        keys(record, ('path', 'dev', 'ino', 'mode', 'uid'))
        need(record['path'] == root['path'] + '/' + name, 'DEP_OBSERVATION')
        for field in ('dev', 'ino', 'mode', 'uid'):
            ns(record[field])
    need(observed_dependencies == provenance['dependency_observation']
         and observed_dependencies['dep_bindings_sha256'] == provenance['dep_bindings_sha256']
         and observed_dependencies['dep_binding_schema'] == provenance['dep_binding_schema']
         and observed_dependencies['source_root']['path'] == provenance['source_root']
         and observed_dependencies['subject_head'] == provenance['subject_head']
         and observed_dependencies['manifest_sha256'] == provenance['manifest_sha256'], 'DEP_COMPLETION')
    captured = _capture(capture, raw)
    need(ns(provenance['pre_monotonic_ns']) <= ns(captured['start_monotonic_ns'])
         and binary(observed_binary) == provenance['binary']
         and source_map(observed_sources) == provenance['source_hashes']
         and hash_value(observed_depfile_sha256) == provenance['depfile_sha256'], 'COMPLETION')
    return {'schema_version': 'pbdep.repaired-completion.v3',
            'scope': 'repaired-post-witness-custody-only', 'raw_sha256': sha(raw),
            'pre_sha256': hash_value(provenance['pre_sha256']),
            **{field: provenance[field] for field in joins}, 'depfile_sha256': observed_depfile_sha256}


def _test_block(lines, start, expected, *, filtered, aggregate=False):
    label = ' test' if len(expected) == 1 else ' tests'
    need(start < len(lines) and lines[start] == 'running ' + str(len(expected)) + label, 'TEST_COUNT')
    index = start + 1
    observed = []
    while index < len(lines) and lines[index].startswith('test ') and not lines[index].startswith('test result:'):
        match = re.fullmatch(r'test ([A-Za-z_][A-Za-z0-9_:]*) \.\.\. ok', lines[index])
        need(match is not None, 'TEST_RESULT')
        observed.append(match.group(1))
        index += 1
    need(len(observed) == len(set(observed)), 'ROSTER')
    need((aggregate and not observed) or set(observed) == set(expected), 'ROSTER')
    need(index < len(lines) and lines[index] == '', 'TEST_FRAME')
    index += 1
    need(index < len(lines), 'TEST_FRAME')
    pattern = (r'test result: ok\. ' + str(len(expected)) + r' passed; 0 failed; 0 ignored; '
        r'0 measured; ' + str(filtered) + r' filtered out; finished in [0-9]{1,5}\.[0-9]{2}s')
    need(re.fullmatch(pattern, lines[index]) is not None, 'TEST_SUMMARY')
    index += 1
    need(index < len(lines) and lines[index] == '', 'TEST_FRAME')
    return index + 1, observed


@closed_input
def validate_selected(raw, *, selected_roster, full_count, exit_code):
    need(type(exit_code) is int and exit_code == 0, 'EXIT')
    selected = roster(selected_roster)
    need(len(selected) == 31 and all(n.startswith(PREFIX) for n in selected), 'ROSTER')
    number(full_count, 20000, len(selected))
    lines = text(raw)[:-1].split('\n')
    need(lines[0] == '', 'TEST_FRAME')
    end, observed = _test_block(lines, 1, selected, filtered=full_count - len(selected))
    need(end == len(lines), 'TEST_FRAME')
    return {'scope': 'selected-assertion-completion-only', 'raw_sha256': sha(raw),
        'passed': len(observed), 'filtered_out': full_count - len(selected), 'names': observed}


def _cargo_noise(line):
    return bool(re.fullmatch(r'   (?:Compiling|Checking) [A-Za-z0-9_-]+ v[0-9A-Za-z.+-]+(?: \(/[A-Za-z0-9_./+-]+\))?', line)
        or re.fullmatch(r'    Finished `(?:test|dev|release)` profile \[(?:unoptimized \+ debuginfo|optimized)\] target\(s\) in (?:[0-9]+m )?[0-9]+(?:\.[0-9]+)?s', line))


def _cargo_blocks(lines, data, *, aggregate):
    index = 0
    observed = []
    while index < len(lines):
        line = lines[index]
        if not line or _cargo_noise(line):
            index += 1
            continue
        match = re.fullmatch(r'     Running (unittests )?([A-Za-z0-9_./+-]+) \(([A-Za-z0-9_./+-]+)\)', line)
        need(match is not None and len(observed) < len(data['targets']), 'CARGO_FRAME')
        target = data['targets'][len(observed)]
        unit_prefix, source, executable = match.groups()
        kind = 'unittests' if unit_prefix else 'tests'
        need(bool(unit_prefix) or source.startswith('tests/'), 'CARGO_FRAME')
        if not executable.startswith('/'):
            executable = posixpath.normpath(data['source_root'] + '/' + executable)
        need(kind == target['kind'] and source == target['source']
             and posixpath.dirname(executable) == data['target_root'] + '/debug/deps'
             and re.fullmatch(re.escape(target['binary_prefix']) + r'-[0-9a-f]{16}', posixpath.basename(executable)), 'CARGO_TARGET')
        index += 1
        need(index < len(lines) and lines[index] == '', 'TEST_FRAME')
        index, test_names = _test_block(lines, index + 1, target['names'], filtered=0, aggregate=aggregate)
        observed.append({'kind': kind, 'source': source, 'binary_prefix': target['binary_prefix'],
                         'count': len(target['names']), 'names_observed': test_names})
    need(len(observed) == len(data['targets']), 'TARGETS')
    return observed


@closed_input
def validate_cargo_tests(raw, *, data, exit_code, reference, phase_binding_raw):
    validate_phase_data(data.get('phase'), data, reference=reference, phase_binding_raw=phase_binding_raw)
    need(data['phase'] in ('integration', 'consumers') and type(exit_code) is int and exit_code == 0, 'EXIT')
    targets = _cargo_blocks(text(raw)[:-1].split('\n'), data, aggregate=False)
    return {'scope': 'cargo-target-completion-only', 'raw_sha256': sha(raw), 'targets': targets,
            'test_count': sum(t['count'] for t in targets)}


@closed_input
def validate_gate(raw, *, data, exit_code, reference, phase_binding_raw):
    validate_phase_data('gate', data, reference=reference, phase_binding_raw=phase_binding_raw)
    need(type(exit_code) is int and exit_code == 0, 'EXIT')
    lines = text(raw)[:-1].split('\n')
    need(lines[0] == data['gate_header'] and lines[-1] == 'gate: all checks passed.', 'GATE_FRAME')
    positions = []
    for step in CHECKS:
        mark = '  ok    ' + step
        need(lines.count(mark) == 1, 'GATE_CHECK')
        positions.append(lines.index(mark))
    need(positions == sorted(positions), 'GATE_ORDER')
    need(not any('  FAIL  ' in line or line.startswith('  skip ') for line in lines), 'GATE_CHECK')
    prior = 1
    targets = None
    for index, position in enumerate(positions):
        segment = lines[prior:position]
        if CHECKS[index] == 'cargo test':
            targets = _cargo_blocks(segment, data, aggregate=True)
        else:
            need(all(not line or _cargo_noise(line) for line in segment), 'GATE_OUTPUT')
        prior = position + 1
    need(prior == len(lines) - 1 and targets is not None, 'GATE_FRAME')
    return {'scope': 'full-rust-gate-only', 'raw_sha256': sha(raw), 'targets': targets,
            'test_count': sum(t['count'] for t in targets), 'checks': list(CHECKS)}


@closed_input
def validate_format(raw, *, exit_code):
    need(type(raw) is bytes and raw == b'' and type(exit_code) is int and exit_code == 0, 'FORMAT_RESULT')
    return {'scope': 'formatter-exit-only', 'raw_sha256': sha(raw)}
