"""B04 trusted-caller entry with native-v2 prior semantic evidence consumption.

No CLI, ambient input, dynamic loader, authentication callback or import-time
effects. Calling this API is an external admission boundary, not an approval API.
"""
from pathlib import Path
import os
import time

from . import discovery as D, runtime as R, whole_program as W, contract as C

WITNESS_KEYS = ('schema_version', 'chain_id', 'grant', 'reference', 'reservation',
    'prior', 'discovery_request', 'discovery_result', 'freeze', 'phase_binding', 'phase')
FREEZE_KEYS = ('schema_version', 'chain_id', 'program_sha256', 'reservation',
    'prior', 'discovery_request', 'discovery_result', 'discovery_close',
    'state_directory', 'retained_inventory', 'resources', 'expected')
EXPECTED_KEYS = ('source_snapshot', 'binary', 'depfile', 'dependency_data',
    'dependency_observation', 'ordered_roster', 'case_map',
    'environment_sha256', 'immutable_files_sha256', 'terminal_captures')


def external_inputs(pin, *pins):
    """All expectations are independently authenticated by the caller, not here."""
    raw = R.pinned(D.checked_pin(pin))
    for expected in pins:
        R.need(len(R.pinned(D.checked_pin(expected))) > 0, 'b04_input_empty')
    value = R.decode(raw)
    R.need(raw == R.encode(value), 'b04_noncanonical_request')
    return value


def controls_outside_state(state_spec, pins):
    root = Path(state_spec['path'])
    for pin in pins:
        D.checked_pin(pin)
        path = Path(pin['path'])
        R.need(path != root and root not in path.parents, 'b04_control_inside_state')


def run_guard(context, action):
    try:
        context.enter()
        return action()
    except (Exception, KeyboardInterrupt):
        if context.armed:
            try:
                context.stop()
            except (Exception, KeyboardInterrupt):
                raise R.Stop('b04_terminal_unavailable') from None
        raise R.Stop('b04_operation_stopped_or_refused') from None
    finally:
        context.release()


def _discover(*, request_pin, grant_pin, reference_pin, reservation_pin, prior_pin):
    """Two fresh named attempts; returns an UNSCORED proposal pin only."""
    started = time.monotonic_ns()
    request = external_inputs(request_pin, grant_pin, reference_pin, reservation_pin, prior_pin)
    D.check_request(request, grant_pin=grant_pin, reference_pin=reference_pin,
                    reservation_pin=reservation_pin, prior_pin=prior_pin)
    controls_outside_state(request['state_directory'],
        (request_pin, grant_pin, reference_pin, reservation_pin, prior_pin, request['source_manifest']))
    reference, reservation = D.read_json(reference_pin), D.read_json(reservation_pin)
    R.need(R.pinned(reservation_pin) == R.encode(reservation), 'b04_reservation_encoding')
    W.validate_reference(reference)
    R.need(request['chain_id'] == reservation['chain_id'], 'b04_chain_join')
    prior = D.prior_history(prior_pin, reservation=reservation,
        state_spec=request['state_directory'], observed_entry_ns=started)
    if prior['phase_history']:
        last_reference = D.read_json(D.read_json(prior['phase_history'][-1]['record'])['reference'])
        W.validate_continuation(last_reference['operator'], reference['operator'],
            tuple(p['phase'] for p in prior['phase_history']),
            previous_reference=last_reference, reference=reference)
    context = D.Custody(request, reference, reservation, 'discovery', started,
        prior['resources'], request_pin, request_pin, grant_pin, reference_pin, prior['inventory'])

    def action():
        R.need(not (context.state / 'witness').exists(), 'b04_discovery_replay')
        with os.scandir(R.path_check(request['target_root'])) as entries:
            R.need(next(entries, None) is None, 'b04_target_not_fresh')
        manifest_raw, manifest, before = D.source_snapshot(request)

        def stable_source():
            context.refresh()
            R.need(D.source_snapshot(request)[2] == before, 'b04_source_changed')

        context.persist_raw('OPERATOR.json', R.pinned(request_pin))
        compiler_raw, compiled, compile_spec = context.capture(
            'compile', D.compile_command(request), stable_source)
        executable = C._compiler(compiler_raw, request['source_root'], request['target_root'])
        actual_binary = D.binary(executable)
        depfile, depraw = R.file_observation(executable + '.d', 512 * 1024, retain=True)
        retained_depfile = context.retain_depfile(depraw)  # Before depfile parsing.
        dependency_data = D.proposed_dependencies(depraw, executable, manifest, request['source_manifest'])
        observation = D.observe_proposed_dependencies(manifest, dependency_data)
        R.need(observation['source_root'] == request['source_directory'],
               'b04_dependency_root')
        for name, record in observation['files'].items():
            R.need(before['files'][name] == record, 'b04_dependency_preimage')
        for name, record in observation['directories'].items():
            R.need(before['directories'].get(name) == record, 'b04_directory_preimage')

        def stable_binary():
            stable_source()
            R.need(D.binary(executable) == actual_binary
                and R.file_observation(executable + '.d', 512 * 1024)[0] == depfile,
                'b04_binary_changed')
            D.observe_proposed_dependencies(manifest, dependency_data, expected=observation)

        listing_raw, listed, list_spec = context.capture(
            'list', (executable, '--list', '--format', 'terse'), stable_binary)
        roster = D.proposed_roster(listing_raw)  # Structural proposal, never an oracle.
        stable_binary()
        R.need(started <= W.ns(compiled['start_monotonic_ns'])
            <= W.ns(compiled['end_monotonic_ns']) <= W.ns(listed['start_monotonic_ns'])
            <= W.ns(listed['end_monotonic_ns']), 'b04_discovery_order')
        D.chronological_interval(started, W.ns(listed['end_monotonic_ns']),
            reservation=reservation, observed_entry_ns=context.clock.check()[0])
        context.verify_retained()
        proposed = {'source_snapshot': before, 'binary': actual_binary, 'depfile': depfile,
            'dependency_data': dependency_data, 'dependency_observation': observation,
            'ordered_roster': roster}
        result = {'schema_version': 'pbdep.discovery-result.v1', 'status': 'UNSCORED',
            'chain_id': request['chain_id'], 'program_sha256': reservation['program_sha256'],
            'reservation': reservation_pin, 'prior': prior_pin, 'request': request_pin,
            'reference': reference_pin, 'grant': grant_pin, 'state_directory': request['state_directory'],
            'captures': {'compile': compile_spec, 'list': list_spec},
            'retained_depfile': retained_depfile, 'proposed': proposed,
            'resources': context.history, **D.AXES}
        return context.finish(result)

    return run_guard(context, action)


def _adopt(*, request, request_pin, freeze_pin, reference, reservation, started):
    """Read-only verified adoption; no ObservationSession internals/START aliases."""
    freeze_raw = R.pinned(freeze_pin)  # Actual independently pinned raw bytes.
    freeze = R.decode(freeze_raw)
    R.keys(freeze, FREEZE_KEYS)
    R.need(freeze['schema_version'] == 'pbdep.b04-freeze.v1'
        and freeze['chain_id'] == request['chain_id'] == reservation['chain_id']
        and freeze['program_sha256'] == reservation['program_sha256']
        and freeze['reservation'] == request['reservation']
        and freeze['prior'] == request['prior']
        and freeze['discovery_request'] == request['discovery_request']
        and freeze['discovery_result'] == request['discovery_result'], 'b04_freeze_join')
    q = external_inputs(freeze['discovery_request'])
    D.check_request(q, grant_pin=q['grant'], reference_pin=q['reference'],
                    reservation_pin=request['reservation'], prior_pin=request['prior'])
    R.need(freeze['state_directory'] == q['state_directory'], 'b04_root_rollover')
    controls_outside_state(q['state_directory'], (request_pin, freeze_pin, request['grant'],
        request['reference'], request['reservation'], request['prior'], request['phase_binding']))
    old_reference = D.read_json(q['reference'])
    result = D.read_json(request['discovery_result'])
    R.keys(result, ('schema_version', 'status', 'chain_id', 'program_sha256',
        'reservation', 'prior', 'request', 'reference', 'grant', 'state_directory',
        'captures', 'retained_depfile', 'proposed', 'resources', *D.AXES))
    R.need(result['schema_version'] == 'pbdep.discovery-result.v1'
        and result['status'] == 'UNSCORED'
        and result['chain_id'] == request['chain_id']
        and result['program_sha256'] == reservation['program_sha256']
        and result['reservation'] == request['reservation']
        and result['prior'] == request['prior']
        and result['request'] == request['discovery_request']
        and result['reference'] == q['reference'] and result['grant'] == q['grant']
        and result['state_directory'] == q['state_directory']
        and all(result[k] == v for k, v in D.AXES.items()), 'b04_discovery_result_join')
    inventory = D.state_inventory(q['state_directory'])
    R.need(inventory == freeze['retained_inventory'], 'b04_retained_freeze_drift')
    for pin in (request['discovery_result'], freeze['discovery_close'], result['retained_depfile']):
        D.retained_pin(pin, inventory)
    close = D.read_json(freeze['discovery_close'])
    R.keys(close, ('schema_version', 'stage', 'chain_id', 'result', 'spent',
        'stage_started_monotonic_ns', 'observed_before_close_write_monotonic_ns',
        'prep_ns', 'child_wrapper_ns', 'close_ns_before_write',
        'close_write_is_inside_reserved_close_window'))
    R.need(close['schema_version'] == 'pbdep.b04-close.v1' and close['stage'] == 'discovery'
        and close['chain_id'] == request['chain_id']
        and close['result'] == request['discovery_result'] and close['spent'] == ['compile', 'list']
        and close['close_write_is_inside_reserved_close_window'] is True, 'b04_discovery_close')
    discovery_started = W.ns(close['stage_started_monotonic_ns'])
    close_tick = W.ns(close['observed_before_close_write_monotonic_ns'])
    D.chronological_interval(discovery_started, close_tick,
        reservation=reservation, observed_entry_ns=started)
    R.need(started >= close_tick
        and started - close_tick <= reservation['review_seconds'] * D.NANO, 'b04_review_expired')
    for key, cap in (('prep_ns', reservation['discovery']['prep']),
                     ('close_ns_before_write', reservation['discovery']['close'])):
        R.need(W.ns(close[key]) <= cap * D.NANO, 'b04_discovery_accounting')
    R.need(close_tick - W.ns(close['stage_started_monotonic_ns']) <=
           reservation['discovery']['total'] * D.NANO, 'b04_discovery_accounting')
    # Reconsume native-v2 Q/A2/later semantic evidence as well as the original
    # anchored history and retained bytes. No old-v1 relabeling or producer call.
    prior = D.prior_history(request['prior'], reservation=reservation,
        state_spec=q['state_directory'], observed_entry_ns=discovery_started, discovery_inventory=inventory)
    R.keys(prior, ('schema_version', 'kind', 'program_sha256', 'chain_id',
        'reservation_sha256', 'state_directory', 'resources', 'terminal_captures',
        'inventory', 'phase_history'))
    R.need(prior['program_sha256'] == reservation['program_sha256']
        and prior['chain_id'] == reservation['chain_id']
        and prior['reservation_sha256'] == request['reservation']['sha256']
        and prior['state_directory'] == q['state_directory'], 'b04_prior_adoption')
    completed = tuple(p['phase'] for p in prior['phase_history'])
    W.validate_continuation(old_reference['operator'], reference['operator'], completed,
                            previous_reference=old_reference, reference=reference)
    if completed:
        last_reference = D.read_json(D.read_json(prior['phase_history'][-1]['record'])['reference'])
        W.validate_continuation(last_reference['operator'], old_reference['operator'], completed,
                                previous_reference=last_reference, reference=old_reference)
    R.need(len(prior['phase_history']) < len(W.PHASES)
        and request['phase'] == W.PHASES[len(prior['phase_history'])], 'b04_prior_phase_order')
    for name, record in prior['inventory']['files'].items():
        R.need(inventory['files'].get(name) == record, 'b04_prior_artifact_drift')
    for name, record in prior['inventory']['directories'].items():
        R.need(inventory['directories'].get(name) == record, 'b04_prior_root_drift')
    R.keys(result['captures'], ('compile', 'list'))
    histories = prior['resources']
    normalized, logs, start_ticks = {}, {}, {}
    for attempt in ('compile', 'list'):
        spec = result['captures'][attempt]
        for pin in spec.values():
            D.retained_pin(pin, inventory)
        start, raw, log, capture, terminal = D.retained_capture(spec)
        R.need(start['attempt'] == attempt and start['chain_id'] == request['chain_id']
            and start['program_sha256'] == reservation['program_sha256']
            and start['reservation_sha256'] == request['reservation']['sha256']
            and start['request_sha256'] == request['discovery_request']['sha256']
            and start['allocation_id'] == reservation['allocation_ids'][attempt]
            and start['useful_seconds'] == reservation['discovery'][attempt]
            and start['exit_seconds'] == 60 and start['cwd'] == q['source_root']
            and start['environment_sha256'] == R.digest(R.encode(q['environment'])),
            'b04_discovery_start_join')
        D.history_extends(histories, raw['resource_samples'])
        histories = terminal['resources']
        normalized[attempt], logs[attempt] = capture, log
        start_ticks[attempt] = W.ns(start['recorded_monotonic_ns'])
    R.need(sum(W.ns(normalized[k]['end_monotonic_ns']) - W.ns(normalized[k]['start_monotonic_ns'])
                   for k in ('compile', 'list')) == W.ns(close['child_wrapper_ns']),
           'b04_discovery_child_accounting')
    R.need(W.ns(close['prep_ns']) + W.ns(close['child_wrapper_ns'])
        + W.ns(close['close_ns_before_write']) ==
        close_tick - W.ns(close['stage_started_monotonic_ns']), 'b04_discovery_accounting')
    R.need(discovery_started <= start_ticks['compile']
        <= W.ns(normalized['compile']['start_monotonic_ns'])
        <= W.ns(normalized['compile']['end_monotonic_ns']) <= start_ticks['list']
        <= W.ns(normalized['list']['start_monotonic_ns'])
        <= W.ns(normalized['list']['end_monotonic_ns']) <= close_tick <= started,
        'b04_discovery_order')
    R.need(histories == result['resources'] == freeze['resources'], 'b04_resource_freeze')
    R.keys(freeze['expected'], EXPECTED_KEYS)
    expected = freeze['expected']
    R.keys(result['proposed'], ('source_snapshot', 'binary', 'depfile', 'dependency_data',
                               'dependency_observation', 'ordered_roster'))
    R.need(all(expected[k] == v for k, v in result['proposed'].items())
        and expected['terminal_captures'] == result['captures']
        and expected['environment_sha256'] == R.digest(R.encode(q['environment']))
        and expected['immutable_files_sha256'] == R.digest(R.encode(q['immutable_files'])),
        'b04_independent_expected_join')
    phase_raw = R.pinned(request['phase_binding'])
    phase = R.decode(phase_raw)
    R.validate_binding_links(reference['operator'], phase, phase_raw, reference=reference)
    R.need(phase['phase'] == request['phase'] in ('base', 'candidate')
        and W.binding_pin(reference['operator'], request['phase'], reference=reference) == request['phase_binding'],
        'b04_phase_join')
    D.cleared_environment(phase['environment'], phase, phase=request['phase'])
    data = phase['data']
    R.need(data['subject_head'] == q['subject_head'] and data['source_root'] == q['source_root']
        and data['target_root'] == q['target_root'] and data['source_manifest'] == q['source_manifest']
        and data['manifest_sha256'] == q['source_manifest']['sha256'], 'b04_frozen_subject')
    for key in ('dep_tokens', 'dep_binding_schema', 'dep_bindings'):
        R.need(data[key] == expected['dependency_data'][key], 'b04_frozen_dependency')
    R.need(data['case_map'] == expected['case_map']
        and W.roster(R.pinned(data['full_roster'])) == expected['ordered_roster'],
        'b04_independent_roster')
    R.pinned(data['selected_roster'])
    for key in ('environment', 'python', 'cargo', 'rustfmt', 'immutable_files', 'tmp', 'selection_inputs'):
        R.need(phase[key] == q[key], 'b04_phase_tool_join')
    R.need(phase['cwd'] == q['source_root'], 'b04_phase_cwd')
    for role in ('step_zero_acceptance', 'source_freeze_review'):
        R.pinned(phase[role])
    attestation = phase['operator_phase_attestation']
    R.keys(attestation, ('step_zero_read_by', 'freeze_read_by', 'issuer', 'step_zero_sha256',
                        'freeze_sha256', 'data_sha256', 'grants_authority'))
    for name in ('step_zero_read_by', 'freeze_read_by', 'issuer'):
        R.need(type(attestation[name]) is str and 0 < len(attestation[name]) <= 128
            and W.NAME.fullmatch(attestation[name])
            and attestation[name] not in ('UNKNOWN', 'UNBOUND'), 'b04_attestation')
    R.need(attestation['step_zero_sha256'] == phase['step_zero_acceptance']['sha256']
        and attestation['freeze_sha256'] == phase['source_freeze_review']['sha256']
        and attestation['data_sha256'] == R.digest(R.encode(data))
        and attestation['grants_authority'] is False, 'b04_attestation')
    return q, result, expected, phase_raw, phase, logs, normalized, inventory, freeze_raw


def _witness(*, request_pin, grant_pin, reference_pin, reservation_pin, prior_pin, freeze_pin):
    """One fixed scored witness, joined to independently accepted actual evidence."""
    started = time.monotonic_ns()
    request = external_inputs(request_pin, grant_pin, reference_pin, reservation_pin, prior_pin, freeze_pin)
    R.keys(request, WITNESS_KEYS)
    R.need(request['schema_version'] == 'pbdep.witness-request.v1'
        and request['phase'] in ('base', 'candidate'), 'b04_witness_version')
    for key, pin in (('grant', grant_pin), ('reference', reference_pin), ('reservation', reservation_pin),
                     ('prior', prior_pin), ('freeze', freeze_pin)):
        R.need(request[key] == pin, 'b04_independent_pin_join')
    reference, reservation = D.read_json(reference_pin), D.read_json(reservation_pin)
    R.need(R.pinned(reservation_pin) == R.encode(reservation), 'b04_reservation_encoding')
    W.validate_reference(reference)
    # Validate the original master before any potentially slow adoption work.
    D.Clock(reservation, 'witness', started)
    q, result, expected, phase_raw, phase, logs, captures, inventory, freeze_raw = _adopt(
        request=request, request_pin=request_pin, freeze_pin=freeze_pin,
        reference=reference, reservation=reservation, started=started)
    context = D.Custody(q, reference, reservation, 'witness', started, result['resources'],
        request_pin, request['discovery_request'], grant_pin, reference_pin, inventory)
    data = phase['data']
    prior_value_at_adoption = D.read_json(request['prior'])

    def current():
        context.refresh()
        R.need(D.read_json(request['prior']) == prior_value_at_adoption, 'b04_prior_changed')
        R.need(R.pinned(freeze_pin) == freeze_raw
            and R.pinned(request['phase_binding']) == phase_raw, 'b04_raw_binding_drift')
        R.pinned(data['full_roster'])
        R.pinned(data['selected_roster'])
        R.need(W.roster(R.pinned(data['full_roster'])) == expected['ordered_roster'],
               'b04_roster_drift')
        manifest_raw, manifest, snapshot = D.source_snapshot(q)
        R.need(snapshot == expected['source_snapshot'], 'b04_source_changed')
        actual = D.binary(expected['binary']['path'])
        depfile, depraw = R.file_observation(actual['path'] + '.d', 512 * 1024, retain=True)
        R.need(actual == expected['binary'] and depfile == expected['depfile']
            and depraw == R.pinned(result['retained_depfile'], 512 * 1024), 'b04_frozen_binary_drift')
        observed = R.observe_dependencies(manifest, data, reference=reference,
            phase_binding_raw=phase_raw, expected=expected['dependency_observation'])
        return manifest_raw, manifest, actual, depfile, depraw, observed

    def action():
        context.persist_raw('OPERATOR.json', freeze_raw)
        context.persist_raw('PHASE-BINDING.json', phase_raw)
        manifest_raw, manifest, actual, depfile, depraw, observed = current()
        compilation = W.validate_compilation(logs['compile'], depraw, manifest_raw=manifest_raw,
            data=data, observed_binary=actual, observed_dependencies=observed,
            reference=reference, phase_binding_raw=phase_raw)
        inventory_fact = W.validate_inventory(logs['list'], full_roster=R.pinned(data['full_roster']),
            selected_roster=R.pinned(data['selected_roster']), case_map=data['case_map'],
            data=data, reference=reference, phase_binding_raw=phase_raw)
        joins = {k: compilation[k] for k in ('manifest_sha256', 'subject_head', 'source_root',
            'dep_binding_schema', 'dep_bindings_sha256', 'dependency_observation', 'reference_sha256')}
        admission = {'schema_version': 'pbdep.repaired-admission.v3', **joins,
            'data_sha256': R.digest(R.encode(data)), 'binary': actual,
            'source_hashes': manifest['files'], 'depfile_sha256': depfile['sha256'],
            'compiler_log_sha256': R.digest(logs['compile']), 'listing_sha256': R.digest(logs['list']),
            'compile': captures['compile'], 'inventory': captures['list'],
            'recorded_monotonic_ns': str(time.monotonic_ns())}
        context.persist('ADMISSION.json', admission)
        W.validate_admission(logs['compile'], logs['list'], admission, manifest_raw=manifest_raw,
            data=data, compilation=compilation, inventory=inventory_fact,
            reference=reference, phase_binding_raw=phase_raw)
        current()
        pre = {'schema_version': 'pbdep.repaired-pre-witness.v3', **joins,
            'admission_sha256': R.digest(R.encode(admission)), 'data_sha256': R.digest(R.encode(data)),
            'binary': actual, 'source_hashes': manifest['files'], 'depfile_sha256': depfile['sha256'],
            'recorded_monotonic_ns': str(time.monotonic_ns())}
        context.persist('PRE-WITNESS.json', pre)
        provenance = W.validate_pre_witness(logs['compile'], logs['list'], admission, pre,
            manifest_raw=manifest_raw, data=data, compilation=compilation, inventory=inventory_fact,
            reference=reference, phase_binding_raw=phase_raw)
        commands = W.commands(request['phase'], data, {'cargo': q['cargo'], 'rustfmt': q['rustfmt']},
            reference=reference, phase_binding_raw=phase_raw, executable=actual['path'])
        R.need(commands[:2] == (D.compile_command(q), (actual['path'], '--list', '--format', 'terse')),
               'b04_command_contract')
        for attempt, command in zip(('compile', 'list'), commands[:2]):
            start = D.read_json(result['captures'][attempt]['start'])
            R.need(tuple(start['argv']) == command, 'b04_adopted_command')
        raw, capture, spec = context.capture('witness', commands[2], current)
        manifest_raw, manifest, actual, depfile, _, observed = current()
        completion = W.validate_completion(raw, capture, provenance=provenance,
            observed_binary=actual, observed_sources=manifest['files'],
            observed_depfile_sha256=depfile['sha256'], observed_dependencies=observed,
            data=data, manifest_raw=manifest_raw, reference=reference, phase_binding_raw=phase_raw)
        selected = W.validate_selected(raw, selected_roster=R.pinned(data['selected_roster']),
                                      full_count=inventory_fact['full_count'], exit_code=capture['exit_code'])
        context.verify_retained()
        return context.finish({'schema_version': 'pbdep.witness-result.v1',
            'scope': 'frozen-witness-evidence-only', 'chain_id': q['chain_id'],
            'program_sha256': reservation['program_sha256'], 'request': request_pin,
            'freeze': freeze_pin, 'reference': reference_pin, 'phase_binding': request['phase_binding'],
            'reservation': reservation_pin, 'prior': prior_pin, 'discovery_result': request['discovery_result'],
            'capture': spec, 'completion': completion, 'selected': selected,
            'resources': context.history, **D.AXES})

    return run_guard(context, action)


def dispatch(mode, **independently_authenticated_pins):
    """Only named operations; unsupported names/arguments have no launch fallback."""
    if mode == 'unscored-discovery':
        operation = _discover
    elif mode == 'scored-witness':
        operation = _witness
    else:
        raise R.Stop('b04_unknown_mode')
    try:
        return operation(**independently_authenticated_pins)
    except R.Stop:
        raise
    except (Exception, KeyboardInterrupt):
        raise R.Stop('b04_input_refused') from None
