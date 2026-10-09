"""CLI adapter used by the ``conda ship`` plugin command."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import sysconfig
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING

from .project_metadata import ProjectMetadataError, runtime_version_args

if TYPE_CHECKING:
    from collections.abc import Sequence
    from typing import TextIO


EXIT_NOT_FOUND = 127
EXIT_NOT_EXECUTABLE = 126
EXIT_INTERRUPTED = 130
ERROR_FORMAT_ENV = "CONDA_SHIP_ERROR_FORMAT"


@dataclass(frozen=True)
class ResolvedExecutable:
    """Resolved ``cs`` executable path and how it was selected."""

    path: Path
    source: str


class AdapterError(Exception):
    """User-facing adapter error with the intended process exit code."""

    def __init__(self, message: str, exit_code: int) -> None:
        super().__init__(message)
        self.exit_code = exit_code


@dataclass(frozen=True)
class BuilderDiagnostic:
    """Structured diagnostic emitted by the ``cs`` executable."""

    kind: str
    message: str
    hint: str | None = None


def configure_parser(parser: argparse.ArgumentParser) -> None:
    """Configure the parser for ``conda ship``."""
    parser.add_argument(
        "ship_args",
        nargs=argparse.REMAINDER,
        metavar="ARGS",
        help="Arguments passed through to the cs executable.",
    )


def execute(args: argparse.Namespace) -> int:
    """Run ``cs`` and return its status code."""
    return run_cs(args.ship_args)


def main(argv: Sequence[str] | None = None) -> None:
    """Standalone debugging entry point for the plugin adapter."""
    raise SystemExit(run_cs(sys.argv[1:] if argv is None else argv))


def run_cs(argv: Sequence[str], *, executable: str | None = None) -> int:
    """Delegate to the canonical ``cs`` executable."""
    try:
        cs = resolve_cs(executable=executable)
    except AdapterError as error:
        print(f"conda-ship: {error}", file=sys.stderr)
        return error.exit_code

    ship_args = list(argv)
    if ship_args[:1] == ["--"]:
        ship_args = ship_args[1:]
    if not ship_args:
        ship_args = ["--help"]
    try:
        ship_args = runtime_version_args(ship_args)
    except ProjectMetadataError as error:
        print(f"conda-ship: {error}", file=sys.stderr)
        return 1

    env = os.environ.copy()
    env[ERROR_FORMAT_ENV] = "json"

    try:
        process = subprocess.Popen(
            [str(cs.path), *ship_args],
            env=env,
            stderr=subprocess.PIPE,
            text=True,
            errors="replace",
            bufsize=1,
        )
        pending_stderr = forward_stderr(process.stderr)
        status = process.wait()
    except FileNotFoundError:
        print(
            f"conda-ship: {cs.path} was resolved from {cs.source}, but it no longer exists.",
            file=sys.stderr,
        )
        return EXIT_NOT_FOUND
    except PermissionError:
        print(
            f"conda-ship: {cs.path} was resolved from {cs.source}, but it is not executable.",
            file=sys.stderr,
        )
        return EXIT_NOT_EXECUTABLE
    except KeyboardInterrupt:
        return EXIT_INTERRUPTED
    except OSError as error:
        print(f"conda-ship: could not execute {cs.path}: {error}", file=sys.stderr)
        return EXIT_NOT_EXECUTABLE

    if status != 0 and pending_stderr is not None:
        if diagnostic := parse_builder_diagnostic(pending_stderr):
            print(format_builder_diagnostic(diagnostic), file=sys.stderr)
        else:
            print(pending_stderr, end="", file=sys.stderr)
    elif pending_stderr is not None:
        print(pending_stderr, end="", file=sys.stderr)

    if status < 0:
        return 128 + abs(status)
    return status


def resolve_cs(*, executable: str | None = None) -> ResolvedExecutable:
    """Resolve the canonical ``cs`` executable for the adapter."""
    if executable is not None:
        return validate_executable(Path(executable), "explicit executable")

    env_value = os.environ.get("CONDA_SHIP_EXECUTABLE")
    if env_value is not None:
        if not env_value.strip():
            raise AdapterError(
                "CONDA_SHIP_EXECUTABLE is set but empty.",
                EXIT_NOT_FOUND,
            )
        return validate_executable(Path(env_value), "CONDA_SHIP_EXECUTABLE")

    installed = installed_cs()
    if installed is not None:
        return validate_executable(Path(installed), "current Python environment")

    raise AdapterError(
        "could not find `cs` in the current Python environment. "
        "Install conda-ship in this environment or set CONDA_SHIP_EXECUTABLE "
        "for a source checkout or custom package.",
        EXIT_NOT_FOUND,
    )


def validate_executable(path: Path, source: str) -> ResolvedExecutable:
    """Validate an executable path before spawning it."""
    if not path.exists():
        raise AdapterError(
            f"{source} points to missing executable: {path}",
            EXIT_NOT_FOUND,
        )
    if path.is_dir():
        raise AdapterError(
            f"{source} points to a directory, not an executable: {path}",
            EXIT_NOT_EXECUTABLE,
        )
    if not path.is_file():
        raise AdapterError(
            f"{source} does not point to a regular executable file: {path}",
            EXIT_NOT_EXECUTABLE,
        )
    if os.name != "nt" and not os.access(path, os.X_OK):
        raise AdapterError(
            f"{source} points to a file that is not executable: {path}",
            EXIT_NOT_EXECUTABLE,
        )
    return ResolvedExecutable(path=path, source=source)


def cs_binary_name() -> str:
    """Return the platform-specific ``cs`` executable name."""
    return "cs.exe" if os.name == "nt" else "cs"


def installed_cs() -> str | None:
    """Find the ``cs`` executable installed with the current Python env."""
    scripts_binary = Path(sysconfig.get_path("scripts")) / cs_binary_name()
    if scripts_binary.is_file():
        return str(scripts_binary)
    env_binary = Path(sys.executable).with_name(cs_binary_name())
    if env_binary.is_file():
        return str(env_binary)
    return None


def forward_stderr(stream: TextIO | None) -> str | None:
    """Forward stderr while keeping the final line available for parsing."""
    if stream is None:
        return None

    pending: str | None = None
    for line in stream:
        if pending is not None:
            print(pending, end="", file=sys.stderr, flush=True)
        pending = line
    return pending


def parse_builder_diagnostic(line: str) -> BuilderDiagnostic | None:
    """Parse a structured ``cs`` diagnostic line."""
    try:
        payload = json.loads(line)
    except json.JSONDecodeError:
        return None

    if not isinstance(payload, dict):
        return None
    if payload.get("schema_version") != 1 or payload.get("tool") != "cs":
        return None

    kind = payload.get("kind")
    message = payload.get("message")
    hint = payload.get("hint")
    if not isinstance(kind, str) or not isinstance(message, str):
        return None
    if hint is not None and not isinstance(hint, str):
        return None
    return BuilderDiagnostic(kind=kind, message=message, hint=hint)


def format_builder_diagnostic(diagnostic: BuilderDiagnostic) -> str:
    """Format a builder diagnostic for conda users."""
    message = f"conda-ship: {diagnostic.message}"
    if diagnostic.hint:
        message = f"{message}\nhint: {diagnostic.hint}"
    return message


if __name__ == "__main__":
    main()
