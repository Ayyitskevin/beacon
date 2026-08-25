"""mosaic — the CLI.

Following the sibling convention in Janus: refusals are sentences, not
tracebacks. Every failure path here tells the reader what is wrong and what to
do instead, because both a tired human and an agent mid-task need that.

Command surface:

* ``mosaic render`` — render a ``desk.v1`` document from a file or stdin.
  Depends on nothing outside this package, so it is the command that proves
  the renderer without any module being reachable.
* ``mosaic desk`` — compose a live desk from the adapters, then render it.
* ``mosaic doctor`` — report what is reachable and which joins do not exist.
* ``mosaic version`` — print the version.

``desk`` and ``doctor`` are wired to modules owned by other lanes. Until those
land, they exit 69 (EX_UNAVAILABLE) with a sentence naming what is missing.
That is deliberate: a stub that returns plausible empty output would be
indistinguishable from a fleet where nothing is happening.
"""

from __future__ import annotations

import argparse
import json
import sys
from typing import Any

from . import __version__
from .render import DEFAULT_RENDERER, RENDERERS, text

EXIT_OK = 0
EXIT_ERROR = 1
EXIT_USAGE = 2
EXIT_UNAVAILABLE = 69  # EX_UNAVAILABLE: the service this needs is not there yet


class MosaicError(Exception):
    """An error whose message is meant to be read by a person."""


def _load_document(source: str) -> dict[str, Any]:
    """Load a desk document from a path, or from stdin when source is '-'."""
    try:
        if source == "-":
            raw = sys.stdin.read()
            origin = "stdin"
        else:
            with open(source, encoding="utf-8") as handle:
                raw = handle.read()
            origin = source
    except OSError as exc:
        raise MosaicError(f"could not read {source}: {exc.strerror}") from exc

    if not raw.strip():
        raise MosaicError(f"{origin} was empty — expected a desk.v1 JSON document")

    try:
        document = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise MosaicError(f"{origin} is not valid JSON: {exc.msg} (line {exc.lineno})") from exc

    if not isinstance(document, dict):
        raise MosaicError(
            f"{origin} holds a {type(document).__name__}, not an object — "
            "a desk document is a JSON object with an 'entries' list"
        )
    return document


def _require(module_path: str, attribute: str, lane: str, provides: str):
    """Import a symbol owned by another lane, or refuse in a full sentence."""
    try:
        module = __import__(module_path, fromlist=[attribute])
    except ImportError as exc:
        raise MosaicError(
            f"{provides} is not available yet: {module_path} does not exist.\n"
            f"It is owned by lane {lane}. Until it lands, `mosaic render` will "
            f"render a desk document you already have on disk."
        ) from exc
    try:
        return getattr(module, attribute)
    except AttributeError as exc:
        raise MosaicError(
            f"{provides} is not available yet: {module_path} exists but does "
            f"not define {attribute}(). It is owned by lane {lane}."
        ) from exc


def _render(document, args) -> None:
    if args.format == "text":
        print(text.render(document, worklist=getattr(args, "worklist", False)))
    else:
        print(RENDERERS[args.format](document))


def cmd_render(args) -> int:
    _render(_load_document(args.source), args)
    return EXIT_OK


def cmd_desk(args) -> int:
    compose = _require(
        "mosaic.compose",
        "compose_desk",
        lane="A (contracts and composition)",
        provides="Composing a live desk",
    )
    _render(compose(), args)
    return EXIT_OK


def cmd_doctor(args) -> int:
    run_doctor = _require(
        "mosaic.doctor",
        "run_doctor",
        lane="B (adapters and doctor)",
        provides="Diagnosing module reachability",
    )
    if args.format == "json":
        print(json.dumps(run_doctor(), indent=2, sort_keys=True, ensure_ascii=False))
        return EXIT_OK

    # The doctor's text rendering lives in doctor.py rather than render/,
    # because it describes adapter state rather than desk entries. It is
    # *required* here rather than falling back to JSON: "text mode quietly
    # printed JSON" is the kind of thing nobody notices until a script starts
    # depending on it.
    render_text = _require(
        "mosaic.doctor",
        "render_text",
        lane="B (adapters and doctor)",
        provides="Human-readable doctor output",
    )
    print(render_text(run_doctor()))
    return EXIT_OK


def cmd_version(_args) -> int:
    print(f"mosaic {__version__}")
    return EXIT_OK


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="mosaic",
        description=(
            "A read-only cross-module desk for AI Town. Mosaic composes what "
            "each module already publishes; it never writes, and it never "
            "collapses five kinds of approval into one."
        ),
    )
    parser.add_argument("--version", action="version", version=f"mosaic {__version__}")
    subparsers = parser.add_subparsers(dest="command")

    def add_format(sub):
        sub.add_argument(
            "--format",
            choices=sorted(RENDERERS),
            default=DEFAULT_RENDERER,
            help="output format (default: %(default)s)",
        )

    def add_worklist(sub):
        sub.add_argument(
            "--worklist",
            action="store_true",
            help=(
                "order by what you have to do (act, then decide, then nothing), "
                "most recent first within each. A view of the document, not a "
                "reordering of it, and not a ranking of importance."
            ),
        )

    p_render = subparsers.add_parser(
        "render",
        help="render a desk.v1 document from a file or stdin",
    )
    p_render.add_argument(
        "source",
        nargs="?",
        default="-",
        help="path to a desk.v1 JSON document, or '-' for stdin (default: -)",
    )
    add_format(p_render)
    add_worklist(p_render)
    p_render.set_defaults(func=cmd_render)

    p_desk = subparsers.add_parser("desk", help="compose a live desk and render it")
    add_format(p_desk)
    add_worklist(p_desk)
    p_desk.set_defaults(func=cmd_desk)

    p_doctor = subparsers.add_parser(
        "doctor",
        help="report what is reachable and which joins are missing",
    )
    add_format(p_doctor)
    p_doctor.set_defaults(func=cmd_doctor)

    p_version = subparsers.add_parser("version", help="print the version")
    p_version.set_defaults(func=cmd_version)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    if not getattr(args, "command", None):
        parser.print_help()
        return EXIT_USAGE

    try:
        return args.func(args)
    except MosaicError as exc:
        print(f"mosaic: {exc}", file=sys.stderr)
        # A missing lane is not the same failure as a malformed document, and
        # a caller scripting around this needs to tell them apart.
        unavailable = "is not available yet" in str(exc)
        return EXIT_UNAVAILABLE if unavailable else EXIT_ERROR
    except BrokenPipeError:
        return EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
