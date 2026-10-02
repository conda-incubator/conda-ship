#!/usr/bin/env python3
"""Bootstrap a fresh runtime and prove its conda and Python are native Windows ARM64."""

from __future__ import annotations

import argparse
import json
import os
import re
import struct
import subprocess
import sys
from pathlib import Path


class SmokeError(Exception):
    """Report a failed smoke assertion without exposing local paths."""


def verify_arm64_pe(path: Path, description: str) -> None:
    """Read the PE machine directly so x64 emulation cannot satisfy the smoke test."""
    try:
        with path.open("rb") as stream:
            header = stream.read(64)
            if len(header) != 64 or header[:2] != b"MZ":
                raise SmokeError(f"{description} has no DOS header")
            stream.seek(struct.unpack_from("<I", header, 60)[0])
            pe_header = stream.read(6)
    except OSError:
        raise SmokeError(f"cannot read {description}") from None
    if len(pe_header) != 6 or pe_header[:4] != b"PE\0\0":
        raise SmokeError(f"{description} has no PE header")
    machine = struct.unpack_from("<H", pe_header, 4)[0]
    if machine != 0xAA64:
        raise SmokeError(f"{description} is not native ARM64 (PE machine 0x{machine:04X})")


def run_json(command: list[str], env: dict[str, str], description: str, timeout: int):
    try:
        result = subprocess.run(
            command, env=env, check=True, capture_output=True, text=True, timeout=timeout
        )
    except subprocess.CalledProcessError as error:
        raise SmokeError(f"{description} failed with exit code {error.returncode}") from None
    except subprocess.TimeoutExpired:
        raise SmokeError(f"{description} exceeded {timeout} seconds") from None
    except (OSError, UnicodeError):
        raise SmokeError(f"could not execute {description}") from None
    try:
        return json.loads(result.stdout)
    except ValueError:
        raise SmokeError(f"{description} did not return JSON") from None


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--prefix", type=Path, required=True)
    args = parser.parse_args()
    if args.prefix.exists() or args.prefix.is_symlink():
        raise SmokeError("smoke prefix must not exist before bootstrap")
    runtime = args.runtime.resolve()
    prefix = args.prefix.resolve()
    verify_arm64_pe(runtime, "runtime")
    env = dict(os.environ, CONDA_SHIP_PREFIX=str(prefix))
    info = run_json([str(runtime), "info", "--json"], env, "runtime info", 600)
    if not isinstance(info, dict) or info.get("platform") != "win-arm64":
        raise SmokeError("conda info did not report win-arm64")
    if (
        not isinstance(info.get("root_prefix"), str)
        or Path(info["root_prefix"]).resolve() != prefix
    ):
        raise SmokeError("conda info did not report the fresh smoke prefix")
    version = re.match(r"^(\d+)\.(\d+)", str(info.get("conda_version", "")))
    if version is None or tuple(map(int, version.groups())) < (26, 9):
        raise SmokeError("native Windows ARM64 smoke requires conda 26.9 or newer")

    packages = run_json(
        [str(runtime), "list", "--json", "--prefix", str(prefix)], env, "runtime list", 120
    )
    if not isinstance(packages, list) or any(not isinstance(item, dict) for item in packages):
        raise SmokeError("conda list did not return package records")
    if not {"conda", "python"}.issubset(item.get("name") for item in packages):
        raise SmokeError("conda list did not include conda and Python")
    records = list((prefix / "conda-meta").glob("*.json"))
    names = set()
    for record in records:
        try:
            package = json.loads(record.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            raise SmokeError("cannot read an installed package record") from None
        if not isinstance(package, dict) or package.get("subdir") not in ("win-arm64", "noarch"):
            raise SmokeError("installed package record is not win-arm64 or noarch")
        names.add(package.get("name"))
    if not {"conda", "python"}.issubset(names):
        raise SmokeError("installed package records did not include conda and Python")

    python = prefix / "python.exe"
    verify_arm64_pe(python, "installed Python")
    verify_arm64_pe(prefix / "Scripts" / "conda.exe", "installed conda launcher")
    architecture = run_json(
        [
            str(python),
            "-I",
            "-c",
            "import json, platform, struct\n"
            "print(json.dumps({'machine': platform.machine(), 'bits': struct.calcsize('P') * 8}))",
        ],
        env,
        "installed Python architecture check",
        60,
    )
    if (
        not isinstance(architecture, dict)
        or str(architecture.get("machine", "")).lower() not in ("arm64", "aarch64")
        or architecture.get("bits") != 64
    ):
        raise SmokeError("installed Python did not execute as native 64-bit ARM64")
    print(
        f"Native Windows ARM64 verified: conda {info['conda_version']}, "
        f"{len(records)} win-arm64/noarch packages, 3 ARM64 PE files, 64-bit ARM64 Python"
    )


if __name__ == "__main__":
    try:
        main()
    except SmokeError as error:
        print(f"Windows ARM64 smoke failed: {error}", file=sys.stderr)
        sys.exit(1)
