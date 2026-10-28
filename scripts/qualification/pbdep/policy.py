"""Pure Phase B reservation predicates. No clock reads, I/O or dispatch.

Reservation inputs must come from a separately authenticated current program.
Fixed numbers below are reviewed ceilings and finite vocabulary, not live slots.
No default clock window, program identity or authorization is supplied.
"""

NANO = 1_000_000_000
EXIT_SECONDS = 60
# Useful slots are ledger identities, not a demand to format after testing.
SLOT_CEILINGS = {
    "1": 600, "2": 30, "3": 180,
    "4": 600, "5": 30, "6": 180,
    "7": 600, "8": 30, "9": 180,
    "10": 180, "11": 600, "12": 1800, "13": 90,
}
PHASE_CEILINGS = {
    "qualification": {"slots": (), "prep": 0, "close": 0, "total": 600},
    "a2": {"slots": ("1", "2", "3"), "prep": 150, "close": 150, "total": 1290},
    "format": {"slots": ("13",), "prep": 120, "close": 60, "total": 330},
    "base": {"slots": ("4", "5", "6"), "prep": 150, "close": 150, "total": 1290},
    "candidate": {"slots": ("7", "8", "9"), "prep": 150, "close": 150, "total": 1290},
    "integration": {"slots": ("10",), "prep": 150, "close": 90, "total": 480},
    "consumers": {"slots": ("11",), "prep": 150, "close": 150, "total": 960},
    "gate": {"slots": ("12",), "prep": 750, "close": 150, "total": 2760},
}
ORDER = tuple(PHASE_CEILINGS)


class Refusal(ValueError):
    """Closed admission failure; a return value never grants permission."""


def integer(value, label):
    if type(value) is not int or value < 0:
        raise Refusal(label)
    return value


def parse_ns(value):
    if type(value) is not str or not value.isascii() or not value.isdecimal():
        raise Refusal("ns_not_decimal_string")
    if len(value) > 24 or (len(value) > 1 and value[0] == "0"):
        raise Refusal("ns_noncanonical")
    return int(value)


def validate_reservation(reservation):
    """Closed pure shape and ceilings; caller verifies current external grant."""
    if type(reservation) is not dict or set(reservation) != {
            'schema_version', 'program_sha256', 'master_start_utc_seconds',
            'master_expiry_utc_seconds', 'exit_seconds', 'slots', 'phases'}:
        raise Refusal("reservation_shape")
    if reservation['schema_version'] != 'pbdep.reservation.v1':
        raise Refusal("reservation_version")
    program = reservation['program_sha256']
    if (type(program) is not str or len(program) != 64
            or any(c not in '0123456789abcdef' for c in program)):
        raise Refusal("reservation_program")
    start = integer(reservation['master_start_utc_seconds'], "start_type")
    expiry = integer(reservation['master_expiry_utc_seconds'], "expiry_type")
    if not 0 < start < expiry or expiry - start > 36 * 3600:
        raise Refusal("reservation_window")
    if type(reservation['exit_seconds']) is not int or reservation['exit_seconds'] != EXIT_SECONDS:
        raise Refusal("reservation_exit")
    slots, phases = reservation['slots'], reservation['phases']
    if type(slots) is not dict or set(slots) != set(SLOT_CEILINGS):
        raise Refusal("reservation_slots")
    for slot, ceiling in SLOT_CEILINGS.items():
        if not 0 < integer(slots[slot], "slot_type") <= ceiling:
            raise Refusal("slot_ceiling")
    if type(phases) is not dict or set(phases) != set(ORDER):
        raise Refusal("reservation_phases")
    for phase in ORDER:
        value, ceiling = phases[phase], PHASE_CEILINGS[phase]
        if type(value) is not dict or set(value) != {'slots', 'prep', 'close', 'total'}:
            raise Refusal("reservation_phase_shape")
        if type(value['slots']) is not tuple or value['slots'] != ceiling['slots']:
            raise Refusal("reservation_phase_slots")
        for key in ('prep', 'close', 'total'):
            if integer(value[key], "phase_type") > ceiling[key]:
                raise Refusal("phase_ceiling")
        expected = sum(slots[n] + EXIT_SECONDS for n in value['slots']) + value['prep'] + value['close']
        if (phase == 'qualification' and not 0 < value['total'] <= ceiling['total']
                or phase != 'qualification' and value['total'] != expected):
            raise Refusal("phase_reserve")
    return reservation


def deadline_from_anchor(wall_ns, mono_ns, *, reservation):
    """Map the required current expiry; no clock is read or authorized here."""
    validate_reservation(reservation)
    integer(wall_ns, "wall_type")
    integer(mono_ns, "mono_type")
    if not reservation['master_start_utc_seconds'] * NANO <= wall_ns < reservation['master_expiry_utc_seconds'] * NANO:
        raise Refusal("outside_master_window")
    return mono_ns + reservation['master_expiry_utc_seconds'] * NANO - wall_ns


def remaining_ns(*, anchor_wall_ns, anchor_mono_ns, now_wall_ns, now_mono_ns, reservation):
    """Both clocks constrain; movement of wall time can never extend the expiry."""
    validate_reservation(reservation)
    for value in (anchor_wall_ns, anchor_mono_ns, now_wall_ns, now_mono_ns):
        integer(value, "clock_type")
    if now_mono_ns < anchor_mono_ns:
        raise Refusal("monotonic_regressed")
    elapsed = now_mono_ns - anchor_mono_ns
    # This tolerance detects a material wall adjustment; it is not extra runtime.
    if abs((now_wall_ns - anchor_wall_ns) - elapsed) > 2 * NANO:
        raise Refusal("clock_relationship_drift")
    deadline = deadline_from_anchor(anchor_wall_ns, anchor_mono_ns, reservation=reservation)
    remaining = min(deadline - now_mono_ns, reservation['master_expiry_utc_seconds'] * NANO - now_wall_ns)
    if remaining <= 0:
        raise Refusal("master_expired")
    return remaining


def phase_reservation(phase, completed, *, reservation):
    validate_reservation(reservation)
    phases = reservation['phases']
    if type(phase) is not str or phase not in phases:
        raise Refusal("unknown_phase")
    if type(completed) is not tuple or completed != ORDER[:ORDER.index(phase)]:
        raise Refusal("phase_order_or_replay")
    return sum(phases[name]["total"] for name in ORDER[ORDER.index(phase):])


def admit_phase(phase, completed, *, remaining_master_ns, reservation):
    validate_reservation(reservation)
    phases = reservation['phases']
    integer(remaining_master_ns, "remaining_type")
    reserve = phase_reservation(phase, completed, reservation=reservation)
    if remaining_master_ns < reserve * NANO:
        raise Refusal("master_reserves_do_not_fit")
    return phases[phase]["total"] * NANO


def admit_slot(phase, slot, spent, *, elapsed_phase_ns, remaining_master_ns, prep_used_ns, reservation):
    validate_reservation(reservation)
    phases, slots = reservation['phases'], reservation['slots']
    if type(phase) is not str or phase not in phases or phase == "qualification":
        raise Refusal("wrong_product_phase")
    declared = phases[phase]["slots"]
    if type(spent) is not tuple or spent != declared[:len(spent)]:
        raise Refusal("slot_order")
    if len(spent) >= len(declared) or slot != declared[len(spent)]:
        raise Refusal("slot_replay_or_unknown")
    integer(elapsed_phase_ns, "phase_time_type")
    integer(remaining_master_ns, "remaining_type")
    integer(prep_used_ns, "preparation_time_type")
    phase_data = phases[phase]
    # All setup/inter-slot validation belongs to the same non-replenishing cap.
    if prep_used_ns > phase_data["prep"] * NANO:
        raise Refusal("phase_preparation_exhausted")
    remaining_slots = declared[len(spent):]
    own_required_ns = ((sum(slots[s] + EXIT_SECONDS for s in remaining_slots)
                        + phase_data["close"] + phase_data["prep"]) * NANO
                       - prep_used_ns)
    if phase_data["total"] * NANO - elapsed_phase_ns < own_required_ns:
        raise Refusal("phase_reserves_do_not_fit")
    later = sum(phases[n]["total"] for n in ORDER[ORDER.index(phase) + 1:])
    if remaining_master_ns < own_required_ns + later * NANO:
        raise Refusal("later_reserves_do_not_fit")
    return slots[slot], EXIT_SECONDS


def close_phase(phase, *, elapsed_phase_ns, spent, prep_used_ns, close_used_ns, reservation):
    validate_reservation(reservation)
    phases = reservation['phases']
    if type(phase) is not str or phase not in phases:
        raise Refusal("unknown_phase")
    integer(elapsed_phase_ns, "phase_time_type")
    integer(prep_used_ns, "preparation_time_type")
    integer(close_used_ns, "close_time_type")
    if type(spent) is not tuple or spent != phases[phase]["slots"]:
        raise Refusal("incomplete_phase")
    if elapsed_phase_ns > phases[phase]["total"] * NANO:
        raise Refusal("phase_expired")
    if (prep_used_ns > phases[phase]["prep"] * NANO
            or close_used_ns > phases[phase]["close"] * NANO):
        raise Refusal("suballocation_exhausted")
    return "WINDOW_COMPLETE"
