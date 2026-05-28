#!/usr/bin/env python3
"""Pack a Flutter AOT ELF (libapp.so) into an OSCortex .osx install bundle."""

from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path

MAGIC = b"OSCP"
VERSION = 1
HEADER_SIZE = 112


def pack_osx(
    aot_elf: bytes,
    name: str,
    version: str = "1.0.0",
    entry_offset: int = 0,
    stack_size: int = 0,
) -> bytes:
    name_b = name.encode("utf-8")[:63]
    version_b = version.encode("utf-8")[:15]
    header = bytearray(HEADER_SIZE)
    header[0:4] = MAGIC
    struct.pack_into("<I", header, 4, VERSION)
    header[8 : 8 + len(name_b)] = name_b
    header[72 : 72 + len(version_b)] = version_b
    struct.pack_into("<Q", header, 88, entry_offset)
    struct.pack_into("<Q", header, 96, stack_size)
    struct.pack_into("<I", header, 104, len(aot_elf))
    struct.pack_into("<I", header, 108, 0)
    return bytes(header) + aot_elf


def main() -> int:
    parser = argparse.ArgumentParser(description="Create an OSCortex .osx bundle")
    parser.add_argument("input", type=Path, help="AOT ELF (libapp.so) or raw AOT blob")
    parser.add_argument("output", type=Path, help="Output .osx path")
    parser.add_argument("--name", required=True, help="App display name")
    parser.add_argument("--version", default="1.0.0", help="App version string")
    args = parser.parse_args()

    data = args.input.read_bytes()
    if len(data) < 64:
        print(f"ERROR: input too small: {args.input}", file=sys.stderr)
        return 1

    bundle = pack_osx(data, args.name, args.version)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(bundle)
    print(f"Wrote {args.output} ({len(bundle)} bytes) name={args.name!r}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
