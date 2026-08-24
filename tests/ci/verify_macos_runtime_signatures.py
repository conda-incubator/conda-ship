#!/usr/bin/env python3
"""Verify that macOS runtime metadata remains covered by code signing."""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import struct
import subprocess
from pathlib import Path

FOOTER_MAGIC = b"CONDA_SHIP_V0001"
FOOTER_LENGTH = 100
LC_CODE_SIGNATURE = 0x1D
MACHO_MAGIC_64_LE = 0xFEEDFACF
SUPERBLOB_MAGIC = 0xFADE0CC0


def code_signature_range(data: bytes | bytearray) -> tuple[int, int]:
    """Locate LC_CODE_SIGNATURE with an independent minimal Mach-O parser."""
    if len(data) < 32 or struct.unpack_from("<I", data)[0] != MACHO_MAGIC_64_LE:
        raise RuntimeError("expected a little-endian 64-bit Mach-O runtime")

    command_count = struct.unpack_from("<I", data, 16)[0]
    command_size = struct.unpack_from("<I", data, 20)[0]
    commands_end = 32 + command_size
    if commands_end > len(data):
        raise RuntimeError("Mach-O load commands are truncated")

    cursor = 32
    for _ in range(command_count):
        if cursor + 8 > commands_end:
            raise RuntimeError("Mach-O load command is truncated")
        command, size = struct.unpack_from("<II", data, cursor)
        if size < 8 or cursor + size > commands_end:
            raise RuntimeError("Mach-O load command has an invalid size")
        if command == LC_CODE_SIGNATURE:
            if size != 16:
                raise RuntimeError("LC_CODE_SIGNATURE has an invalid size")
            signature_offset, signature_size = struct.unpack_from("<II", data, cursor + 8)
            if signature_offset + signature_size > len(data):
                raise RuntimeError("LC_CODE_SIGNATURE is outside the Mach-O file")
            return signature_offset, signature_size
        cursor += size

    raise RuntimeError("LC_CODE_SIGNATURE was not found")


def tamper_with_runtime_footer(path: Path) -> None:
    data = bytearray(path.read_bytes())
    signature_offset, _ = code_signature_range(data)
    footer_magic = data.rfind(FOOTER_MAGIC, 0, signature_offset)
    if footer_magic < 0:
        raise RuntimeError("conda-ship runtime data footer was not found")
    data[footer_magic] ^= 1
    path.write_bytes(data)


def find_anchored_footer(data: bytearray, signature_offset: int) -> int:
    for padding in range(16):
        footer_end = signature_offset - padding
        footer_magic = footer_end - len(FOOTER_MAGIC)
        if footer_magic < 0 or data[footer_magic:footer_end] != FOOTER_MAGIC:
            continue
        if any(data[footer_end:signature_offset]):
            raise RuntimeError("runtime footer alignment padding is not zero")
        return footer_end
    raise RuntimeError("anchored conda-ship footer was not found")


def add_shadow_runtime_stamp(path: Path) -> None:
    data = bytearray(path.read_bytes())
    signature_offset, signature_size = code_signature_range(data)
    footer_end = find_anchored_footer(data, signature_offset)
    footer_start = footer_end - FOOTER_LENGTH
    if footer_start < 0:
        raise RuntimeError("conda-ship runtime data footer is truncated")

    header_length, bundle_length = struct.unpack_from("<QQ", data, footer_start)
    header_start = footer_start - header_length - bundle_length
    header_end = header_start + header_length
    if header_start < 0 or header_end > footer_start:
        raise RuntimeError("conda-ship runtime data header is outside the Mach-O file")

    header = json.loads(data[header_start:header_end])
    header["runtime_name"] = "forged"
    header["runtime_lock"] = ""
    header["runtime_config"] = {
        "channels": [],
        "packages": [],
        "freeze_base": False,
    }
    forged_header = json.dumps(header, separators=(",", ":"), ensure_ascii=False).encode()
    forged = b"".join(
        (
            forged_header,
            struct.pack("<QQ", len(forged_header), 0),
            hashlib.sha256(forged_header).digest(),
            hashlib.sha256(b"").digest(),
            struct.pack("<I", 1),
            FOOTER_MAGIC,
        )
    )

    if signature_size < 12:
        raise RuntimeError("code-signing SuperBlob is truncated")
    if struct.unpack_from(">I", data, signature_offset)[0] != SUPERBLOB_MAGIC:
        raise RuntimeError("code-signing SuperBlob was not found")
    superblob_length = struct.unpack_from(">I", data, signature_offset + 4)[0]
    forged_offset = signature_offset + superblob_length
    signature_end = signature_offset + signature_size
    if superblob_length < 12 or forged_offset > signature_end:
        raise RuntimeError("code-signing SuperBlob has an invalid size")
    if forged_offset + len(forged) > signature_end:
        raise RuntimeError("code-signature padding is too small for forged stamp")
    data[forged_offset : forged_offset + len(forged)] = forged
    path.write_bytes(data)


def write_shadow_info(source: Path, destination: Path) -> None:
    with source.open(encoding="utf-8") as file:
        info = json.load(file)
    info["update"] = {
        "channel": "https://packages.example.test/runtime",
        "package": "demo-runtime",
        "build-number": 0,
    }
    with destination.open("w", encoding="utf-8") as file:
        json.dump(info, file)


def verify_codesign(path: Path, *, verbose: bool = False) -> subprocess.CompletedProcess[str]:
    command = ["codesign", "--verify", "--deep", "--strict"]
    if verbose:
        command.append("--verbose=4")
    command.append(str(path))
    return subprocess.run(command, check=False, text=True)


def verify_runtime(*, cs: Path, dist_dir: Path, target: str, temp_dir: Path, name: str) -> None:
    binary = dist_dir / f"{name}-{target}"
    subprocess.run(
        ["codesign", "--verify", "--deep", "--strict", "--verbose=4", str(binary)],
        check=True,
    )

    resigned = temp_dir / f"{name}-{target}-resigned"
    shutil.copy2(binary, resigned)
    subprocess.run(["codesign", "--force", "--sign", "-", str(resigned)], check=True)
    if verify_codesign(resigned, verbose=True).returncode != 0:
        raise RuntimeError(f"codesign rejected re-signed runtime {resigned}")

    tampered = temp_dir / f"{name}-{target}-tampered"
    shutil.copy2(resigned, tampered)
    tamper_with_runtime_footer(tampered)
    if verify_codesign(tampered).returncode == 0:
        raise RuntimeError(f"codesign accepted modified runtime data in {tampered}")

    shadow = temp_dir / f"{name}-{target}-shadow"
    shadow_info = temp_dir / f"{name}-{target}-shadow.info.json"
    shutil.copy2(resigned, shadow)
    add_shadow_runtime_stamp(shadow)
    write_shadow_info(dist_dir / f"{name}-{target}.info.json", shadow_info)
    if verify_codesign(shadow, verbose=True).returncode != 0:
        raise RuntimeError(f"codesign rejected shadow-stamp runtime {shadow}")

    package_dir = temp_dir / f"{name}-{target}-shadow-package"
    package_dir.mkdir()
    package = subprocess.run(
        [
            str(cs),
            "package-update",
            "--info",
            str(shadow_info),
            "--binary",
            str(shadow),
            "--out-dir",
            str(package_dir),
        ],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    expected = "runtime executable update configuration does not match artifact info"
    if package.returncode == 0 or expected not in package.stdout:
        if package.stdout:
            print(package.stdout, end="")
        raise RuntimeError("reader did not retain the authenticated runtime stamp")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cs", type=Path, required=True, help="Path to the cs binary")
    parser.add_argument(
        "--dist-dir", type=Path, required=True, help="Directory containing artifacts"
    )
    parser.add_argument("--target", required=True, help="Rust target triple")
    parser.add_argument(
        "--temp-dir", type=Path, required=True, help="Directory for mutated copies"
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    for name in ("demo", "demoz"):
        verify_runtime(
            cs=args.cs,
            dist_dir=args.dist_dir,
            target=args.target,
            temp_dir=args.temp_dir,
            name=name,
        )


if __name__ == "__main__":
    main()
