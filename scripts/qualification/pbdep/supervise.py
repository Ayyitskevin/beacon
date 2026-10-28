"""Bounded trusted Linux lineage witness; not a containment/security boundary."""
import ctypes
import hashlib
import os
from pathlib import Path
import select
import signal
import time

MAX_EVENTS = 128

def subreaper():
    if len(list(Path('/proc/self/task').iterdir())) != 1:
        raise RuntimeError('single thread required')
    signal.signal(signal.SIGCHLD, signal.SIG_DFL)
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), 'subreaper')
    observed = ctypes.c_int()
    if libc.prctl(37, ctypes.byref(observed), 0, 0, 0) or observed.value != 1:
        raise RuntimeError('subreaper not verified')
    try:
        os.waitpid(-1, os.WNOHANG | 0x40000000)
    except ChildProcessError:
        return
    raise RuntimeError('pre-existing child; not exclusive')

def run(argv, cwd, env, log_path, *, useful=1800, grace=60, kill_after=10,
        log_cap, resource_check, setup_fault=False, outer_fault=False, event_cap=MAX_EVENTS):
    if (type(argv) not in (list, tuple) or not 1 <= len(argv) <= 256
            or any(type(v) is not str or not v or len(v.encode()) > 4096 or '\0' in v for v in argv)
            or type(cwd) is not str or not cwd.startswith('/')
            or os.path.normpath(cwd) != cwd
            or type(env) is not dict or len(env) > 64
            or any(type(k) is not str or type(v) is not str or not k or '=' in k
                   or '\0' in k or '\0' in v or len(k.encode()) > 64 or len(v.encode()) > 8192
                   for k, v in env.items())
            or type(log_path) is not str or not log_path.startswith('/')
            or os.path.normpath(log_path) != log_path
            or type(useful) is not int or not 0 < useful <= 1800
            or type(grace) is not int or not 0 < grace <= 60
            or type(kill_after) is not int or not 0 < kill_after <= min(10, grace)
            or type(log_cap) is not int or not 0 < log_cap <= 16 * 1024 * 1024
            or type(event_cap) is not int or not 0 <= event_cap <= MAX_EVENTS
            or type(setup_fault) is not bool or type(outer_fault) is not bool
            or not callable(resource_check)):
        raise ValueError('supervisor_input_bound')
    subreaper()
    started = time.monotonic()
    log = Path(log_path).open('xb')
    read_fd, write_fd = os.pipe2(os.O_CLOEXEC)
    leader = None
    code = leader_at = empty_at = eof_at = None
    eof = False
    stop = stop_at = None
    retained = discarded = peak = 0
    digest = hashlib.sha256()
    events = {'reaped':[], 'signals':[], 'errors':[]}
    counts = {k:0 for k in events}
    overflow = False
    emergency_sigint = None
    def add(kind, item):
        nonlocal overflow, stop
        counts[kind] += 1
        if len(events[kind]) < event_cap:
            events[kind].append(item)
        else:
            overflow = True
            stop = stop or 'metadata_overflow'
    def error(where, exc):
        nonlocal stop
        stop = stop or 'instrument_error'
        item={'where':where,'type':type(exc).__name__}
        pathname_class=getattr(exc,'pathname_class',None)
        if pathname_class in ('root-lstat','entry-stat','directory-scan'):
            item['pathname_class']=pathname_class
            item['errno']=exc.errno
        add('errors', item)
    try:
        leader = os.fork()
        if leader == 0:
            try:
                os.close(read_fd)
                os.setsid()
                os.chdir(cwd)
                null = os.open('/dev/null', os.O_RDONLY)
                os.dup2(null, 0)
                os.dup2(write_fd, 1)
                os.dup2(write_fd, 2)
                os.close(null)
                os.close(write_fd)
                os.execve(argv[0], argv, env)
            except BaseException:
                os._exit(127)
        # The entire parent post-fork lifetime participates in stop/reap.
        try:
            os.close(write_fd)
            write_fd = None
            os.set_blocking(read_fd, False)
            if setup_fault:
                raise OSError('qualification: post-fork setup failure')
        except BaseException as exc:
            error('postfork_setup', exc)
        last_resource = started - 2
        last_signal = 0
        while True:
            if outer_fault:
                raise KeyboardInterrupt('qualification: outside inner guards')
            tick = time.monotonic()
            try:
                while True:
                    try:
                        pid, status = os.waitpid(-1, os.WNOHANG | 0x40000000)
                    except ChildProcessError:
                        empty_at = time.monotonic()
                        break
                    if pid == 0:
                        break
                    rc = os.waitstatus_to_exitcode(status)
                    add('reaped', {'pid':pid,'status':status,'exit_code':rc,'at':time.monotonic()})
                    if pid == leader:
                        code, leader_at = rc, time.monotonic()
            except BaseException as exc:
                error('reap', exc)
            if tick-started >= useful:
                stop = stop or 'useful_time_cap'
            if tick-last_resource >= 2 or empty_at is not None:
                try:
                    peak = max(peak, resource_check() or 0)
                except BaseException as exc:
                    error('resources', exc)
                last_resource = tick
            try:
                if not eof and select.select([read_fd], [], [], .05)[0]:
                    block = os.read(read_fd, 65536)
                    if not block:
                        eof, eof_at = True, time.monotonic()
                    else:
                        kept = block[:max(0,log_cap-retained)]
                        log.write(kept)
                        log.flush()
                        digest.update(kept)
                        retained += len(kept)
                        discarded += len(block)-len(kept)
                        if discarded:
                            stop = stop or 'log_cap_prefix_only'
                elif eof:
                    time.sleep(.02)
            except BaseException as exc:
                error('capture', exc)
                time.sleep(.02)
            if empty_at is not None and eof:
                break
            if stop:
                stop_at = stop_at if stop_at is not None else time.monotonic()
                if tick-last_signal >= .1:
                    try:
                        # Direct/adopted children cannot be PID-reused before reaping.
                        pids = Path(f'/proc/self/task/{os.getpid()}/children').read_text().split()
                        sig = signal.SIGKILL if tick-stop_at >= kill_after else signal.SIGTERM
                        for value in pids:
                            pid = int(value)
                            fd = None
                            try:
                                fd = os.pidfd_open(pid)
                                signal.pidfd_send_signal(fd, sig)
                                add('signals', {'pid':pid,'signal':int(sig),'at':time.monotonic()})
                            except ProcessLookupError:
                                pass
                            finally:
                                if fd is not None:
                                    os.close(fd)
                    except BaseException as exc:
                        error('signal', exc)
                    last_signal = tick
                if time.monotonic()-stop_at >= grace:
                    break
        try:
            log.flush()
            os.fsync(log.fileno())
        except BaseException as exc:
            error('log_persist', exc)
    except BaseException as exc:
        error('outer_parent', exc)
        if leader is not None:
            # Last-resort bounded cleanup covers ANY catchable parent unwind,
            # not just the anticipated setup/capture operations above. This
            # does not claim recovery from supervisor death/SIGKILL.
            # A second keyboard interrupt must not abandon the bounded cleanup.
            # Restore the caller disposition after the cleanup attempt terminates.
            emergency_sigint=signal.signal(signal.SIGINT,signal.SIG_IGN)
            stop_at = stop_at if stop_at is not None else time.monotonic()
            emergency_deadline = stop_at + grace
            while time.monotonic() < emergency_deadline:
                try:
                    if write_fd is not None:
                        os.close(write_fd)
                        write_fd = None
                    os.set_blocking(read_fd, False)
                    while True:
                        try:
                            pid,status=os.waitpid(-1,os.WNOHANG|0x40000000)
                        except ChildProcessError:
                            empty_at=time.monotonic()
                            break
                        if pid==0:
                            break
                        rc=os.waitstatus_to_exitcode(status)
                        add('reaped',{'pid':pid,'status':status,'exit_code':rc,'at':time.monotonic()})
                        if pid==leader:
                            code,leader_at=rc,time.monotonic()
                    if not eof and select.select([read_fd],[],[],.02)[0]:
                        block=os.read(read_fd,65536)
                        if not block:
                            eof,eof_at=True,time.monotonic()
                        else:
                            kept=block[:max(0,log_cap-retained)]
                            log.write(kept)
                            log.flush()
                            digest.update(kept)
                            retained+=len(kept)
                            discarded+=len(block)-len(kept)
                    if empty_at is not None and eof:
                        break
                    sig=signal.SIGKILL if time.monotonic()-stop_at>=kill_after else signal.SIGTERM
                    pids=Path(f'/proc/self/task/{os.getpid()}/children').read_text().split()
                    for value in pids:
                        fd=None
                        try:
                            fd=os.pidfd_open(int(value))
                            signal.pidfd_send_signal(fd,sig)
                            add('signals',{'pid':int(value),'signal':int(sig),'at':time.monotonic()})
                        except ProcessLookupError:
                            pass
                        finally:
                            if fd is not None:
                                os.close(fd)
                    time.sleep(.05)
                except BaseException as cleanup_error:
                    error('emergency_cleanup',cleanup_error)
                    # No clock reset; repeated failures end UNKNOWN at the same
                    # bounded deadline instead of manufacturing completion.
            try:
                log.flush()
                os.fsync(log.fileno())
            except BaseException as persist_error:
                error('emergency_persist',persist_error)
    finally:
        if emergency_sigint is not None:
            signal.signal(signal.SIGINT,emergency_sigint)
        for fd in (read_fd, write_fd):
            if fd is not None:
                try:
                    os.close(fd)
                except OSError:
                    pass
        log.close()
    if empty_at is None or code is None or not eof:
        stop = stop or 'descendant_exit_or_capture_unknown'
    return {'leader_pid':leader,'leader_returncode':code,'leader_exit_monotonic':leader_at,
        'whole_descendant_set_exit':'PROVEN_ECHILD' if empty_at is not None else 'UNKNOWN',
        'echild_monotonic':empty_at,'pipe_eof':eof,'pipe_eof_monotonic':eof_at,
        'subreaper_verified':True,'wait_options':'WNOHANG|__WALL',
        'elapsed_seconds':time.monotonic()-started,'stop':stop,'metadata_overflow':overflow,
        'event_counts':counts,**events,'resource_peak_sampled':peak,
        'log_bytes':retained,'discarded_log_bytes':discarded,'log_sha256':digest.hexdigest()}
