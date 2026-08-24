"""Validate native and wheel-packaged conda-ship builder/template pairs."""

from __future__ import annotations

import argparse
import hashlib
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path, PurePosixPath
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Sequence


CHECKSUM_LINE = re.compile(r"(?P<digest>[0-9a-fA-F]{64}) (?P<mode>[ *])(?P<path>.+)")


class ValidationError(Exception):
    """Report a malformed input or failed runtime-pair assertion."""


def executable_path(path: Path, target: str) -> Path:
    """Add the Windows executable suffix when the target requires it."""
    if "windows" in target and path.suffix.lower() != ".exe":
        return path.with_name(f"{path.name}.exe")
    return path


def require_file(path: Path, description: str) -> Path:
    """Return an absolute regular-file path or fail with a useful message."""
    resolved = path.resolve()
    if not resolved.is_file():
        raise ValidationError(f"{description} does not exist or is not a file: {path}")
    return resolved


def verify_checksum_manifest(manifest: Path) -> None:
    """Verify every GNU-style SHA-256 entry without an external checksum tool."""
    output_directory = manifest.parent.resolve()
    lines = manifest.read_text(encoding="utf-8").splitlines()
    entries = [line for line in lines if line]
    if not entries:
        raise ValidationError(f"checksum manifest contains no entries: {manifest}")

    for line_number, line in enumerate(entries, start=1):
        match = CHECKSUM_LINE.fullmatch(line)
        if match is None:
            raise ValidationError(f"invalid checksum entry at {manifest}:{line_number}: {line!r}")

        relative_path = Path(match.group("path"))
        artifact = (output_directory / relative_path).resolve()
        try:
            artifact.relative_to(output_directory)
        except ValueError as error:
            raise ValidationError(
                f"checksum entry escapes the output directory: {relative_path}"
            ) from error
        if not artifact.is_file():
            raise ValidationError(f"checksummed artifact does not exist: {relative_path}")

        digest = hashlib.sha256()
        with artifact.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
        if digest.hexdigest() != match.group("digest").lower():
            raise ValidationError(f"checksum mismatch for {relative_path}")
        print(f"{relative_path}: OK")


def validate_archive_member(name: str) -> None:
    """Reject wheel members that could extract outside the destination."""
    if not name or "\x00" in name or "\\" in name:
        raise ValidationError(f"wheel contains an unsafe member path: {name!r}")

    member = PurePosixPath(name)
    if member.is_absolute() or ".." in member.parts:
        raise ValidationError(f"wheel contains an unsafe member path: {name!r}")
    if member.parts and len(member.parts[0]) >= 2 and member.parts[0][1] == ":":
        raise ValidationError(f"wheel contains an unsafe member path: {name!r}")


def extract_wheel(wheel: Path, destination: Path) -> None:
    """Extract a wheel after validating every archive member path."""
    with zipfile.ZipFile(wheel) as archive:
        members = archive.infolist()
        names = [member.filename for member in members]
        if len(names) != len(set(names)):
            raise ValidationError(f"wheel contains duplicate member paths: {wheel}")
        for name in names:
            validate_archive_member(name)
        archive.extractall(destination)


def find_packaged_script(wheel_root: Path, filename: str) -> Path:
    """Find exactly one script with the wheel data-directory layout."""
    candidates = sorted(
        path
        for path in wheel_root.rglob(filename)
        if path.is_file() and path.parent.name == "scripts"
    )
    if len(candidates) != 1:
        raise ValidationError(f"expected one packaged scripts/{filename}, found {len(candidates)}")
    return candidates[0].resolve()


def make_executable(path: Path) -> None:
    """Restore execution bits that zip extraction does not preserve."""
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def run_builder(
    builder: Path,
    template: Path,
    fixture_root: Path,
    output_directory: Path,
    runtime_name: str,
    platform: str,
    target: str,
) -> None:
    """Build one runtime and verify its generated SHA-256 manifest."""
    command = [
        str(builder),
        "build",
        "--artifact-layout",
        "online",
        "--runtime-name",
        runtime_name,
        "--delegate-executable",
        "conda",
        "--platform",
        platform,
        "--target",
        target,
        "--target-label",
        target,
        "--template",
        str(template),
        "--root",
        str(fixture_root),
        "--out-dir",
        str(output_directory),
    ]
    subprocess.run(command, check=True)
    verify_checksum_manifest(output_directory / f"{runtime_name}-{target}.sha256")


def copy_fixture(source: Path, temporary_root: Path) -> Path:
    """Copy the fixture so validation does not write generated state into the checkout."""
    destination = temporary_root / "fixture"
    shutil.copytree(source, destination)
    return destination


def validate_native(args: argparse.Namespace) -> None:
    """Validate binaries built directly by Cargo."""
    builder = require_file(executable_path(args.builder, args.target), "builder")
    template = require_file(executable_path(args.template, args.target), "runtime template")
    fixture_root = require_directory(args.fixture_root, "runtime fixture")
    scratch_root = require_directory(args.scratch_root, "scratch root")

    with tempfile.TemporaryDirectory(
        prefix=f"release-pair-{args.target}-", dir=scratch_root
    ) as temporary_directory:
        temporary_root = Path(temporary_directory)
        run_builder(
            builder=builder,
            template=template,
            fixture_root=copy_fixture(fixture_root, temporary_root),
            output_directory=temporary_root / "output",
            runtime_name="release-pair",
            platform=args.platform,
            target=args.target,
        )


def require_directory(path: Path, description: str) -> Path:
    """Return an absolute directory path or fail with a useful message."""
    resolved = path.resolve()
    if not resolved.is_dir():
        raise ValidationError(f"{description} does not exist or is not a directory: {path}")
    return resolved


def validate_wheel(args: argparse.Namespace) -> None:
    """Validate the builder and template packaged together in one wheel."""
    wheel_directory = require_directory(args.wheel_directory, "wheel directory")
    wheels = sorted(wheel_directory.glob("*.whl"))
    if len(wheels) != 1:
        raise ValidationError(f"expected one wheel, found {len(wheels)}")

    fixture_root = require_directory(args.fixture_root, "runtime fixture")
    scratch_root = require_directory(args.scratch_root, "scratch root")
    extension = ".exe" if "windows" in args.target else ""

    with tempfile.TemporaryDirectory(
        prefix=f"wheel-pair-{args.target}-", dir=scratch_root
    ) as temporary_directory:
        temporary_root = Path(temporary_directory)
        wheel_root = temporary_root / "wheel"
        extract_wheel(wheels[0], wheel_root)
        builder = find_packaged_script(wheel_root, f"cs{extension}")
        template = find_packaged_script(wheel_root, f"cs-template{extension}")
        make_executable(builder)
        make_executable(template)
        run_builder(
            builder=builder,
            template=template,
            fixture_root=copy_fixture(fixture_root, temporary_root),
            output_directory=temporary_root / "output",
            runtime_name="wheel-pair",
            platform=args.platform,
            target=args.target,
        )


def parser() -> argparse.ArgumentParser:
    """Construct the command-line parser."""
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--target", required=True, help="Rust target triple")
    common.add_argument("--platform", required=True, help="Conda platform identifier")
    common.add_argument(
        "--scratch-root",
        required=True,
        type=Path,
        help="existing directory for temporary validation files",
    )
    common.add_argument(
        "--fixture-root",
        type=Path,
        default=Path("tests/fixtures/structural-runtime"),
        help="structural runtime fixture root",
    )

    argument_parser = argparse.ArgumentParser(description=__doc__)
    subparsers = argument_parser.add_subparsers(dest="mode", required=True)

    native = subparsers.add_parser(
        "native", parents=[common], help="validate Cargo-built binaries"
    )
    native.add_argument("--builder", type=Path, default=Path("target/release/cs"))
    native.add_argument("--template", type=Path, default=Path("target/release/cs-template"))
    native.set_defaults(execute=validate_native)

    wheel = subparsers.add_parser(
        "wheel", parents=[common], help="validate binaries packaged in a wheel"
    )
    wheel.add_argument("--wheel-directory", type=Path, default=Path("dist-pypi"))
    wheel.set_defaults(execute=validate_wheel)
    return argument_parser


def main(argv: Sequence[str] | None = None) -> int:
    """Validate the selected runtime builder/template pair."""
    args = parser().parse_args(argv)
    try:
        args.execute(args)
    except subprocess.CalledProcessError as error:
        print(f"runtime builder exited with status {error.returncode}", file=sys.stderr)
        return 1
    except (OSError, ValidationError, zipfile.BadZipFile) as error:
        print(f"runtime-pair validation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
