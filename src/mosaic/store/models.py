"""The canonical table -> model map, and the columns each model implies.

Lane C's column reconciler (totality check 2) takes this map rather than
discovering models itself -- deliberately, so their layer holds no copy of
this layer's structure. This module is the single place that map lives.

**Why `expected_columns` computes rather than lists.** A hand-written column
list per table would be the same defect the enum lookup tables were built to
remove: a second copy of a structure, drifting silently in the direction
nothing detects. So columns are derived from the dataclass fields, and the
*only* hand-declared thing is the flattening -- which genuinely cannot be
derived, because the DDL's choice to inline a 1:1 nested record is a schema
decision rather than a property of the type.

Check 2 exists because check 1 cannot see it: a round-trip against a type's
field list catches a field added to the type, and is completely blind to a
column added to the schema that no type models.
"""

from __future__ import annotations

from dataclasses import Field, fields, is_dataclass

from mosaic.store.envelope import Provenance, Record, Reference, Relation

#: table name -> the dataclass that defines its shape. **Lane C reconciles
#: the database's columns against this.** A table absent from this map has no
#: canonical model, which is the state the rule exists to prevent -- so the
#: reconciler should treat an unmapped non-enum table as a failure rather than
#: skipping it.
CANONICAL_MODELS: dict[str, type] = {
    "record": Record,
    "reference": Reference,
    "relation": Relation,
}

#: field name -> (nested dataclass, column prefix). The DDL inlines these 1:1
#: rather than giving them a table, so their columns are `prefix_fieldname`.
#: The ONLY hand-declared mapping in this module, and it is declared because
#: it is a schema decision that no amount of reading the type can reveal.
FLATTENED: dict[str, tuple[type, str]] = {
    "provenance": (Provenance, "provenance_"),
}

#: Columns the DDL carries that no model field produces, with the reason.
#: Declared rather than tolerated: an unexplained extra column is exactly what
#: check 2 is looking for, so each one must be justified here or it is a
#: finding.
DDL_ONLY_COLUMNS: dict[str, dict[str, str]] = {
    "record": {
        "authority_state": (
            "explicit required/not_required, because 'no approval was sought' "
            "and 'an approval is missing' are different facts and a nullable "
            "FK would make them one"
        ),
        "authority_id": "'' when not_required; never NULL",
    },
    "reference": {
        "id": "store-assigned surrogate; the model carries the (capability, ref, origin) triple"
    },
    "relation": {"to_reference": "FK to reference.id; the model holds the Reference itself"},
}

#: Model fields with no column of their own, with the reason. The mirror of
#: DDL_ONLY_COLUMNS, and needed for the same reason: a silent asymmetry in
#: either direction is what the reconciler exists to catch.
MODEL_ONLY_FIELDS: dict[str, dict[str, str]] = {
    "relation": {"to": "stored as the to_reference foreign key"},
}


def _field_names(model: type) -> list[str]:
    if not is_dataclass(model):
        raise TypeError(f"{model!r} is not a dataclass and cannot define a table")
    out: list[str] = []
    fs: tuple[Field, ...] = fields(model)
    for f in fs:
        if f.name in FLATTENED:
            nested, prefix = FLATTENED[f.name]
            out.extend(f"{prefix}{n.name}" for n in fields(nested))
        else:
            out.append(f.name)
    return out


def expected_columns(table: str) -> set[str]:
    """The columns `table` should have, derived from its canonical model.

    Includes DDL-only columns, because they are declared above with reasons.
    Excludes model-only fields, for the same reason. Anything outside both
    sets is a genuine divergence rather than a known asymmetry.
    """
    model = CANONICAL_MODELS[table]
    names = set(_field_names(model))
    names -= set(MODEL_ONLY_FIELDS.get(table, {}))
    names |= set(DDL_ONLY_COLUMNS.get(table, {}))
    return names


def column_divergences(conn) -> dict[str, dict[str, list[str]]]:
    """Where the database's columns and the canonical models disagree.

    Both directions, for the reason the enum reconciler needs both: a column
    the models do not know about is the silent direction -- nothing fails,
    the schema simply carries a field no code maintains, and it accumulates.
    """
    out: dict[str, dict[str, list[str]]] = {}
    for table in CANONICAL_MODELS:
        actual = {r[1] for r in conn.execute(f"PRAGMA table_info({table})")}
        wanted = expected_columns(table)
        missing = sorted(wanted - actual)
        extra = sorted(actual - wanted)
        if missing or extra:
            out[table] = {"in_model_not_db": missing, "in_db_not_model": extra}
    return out
