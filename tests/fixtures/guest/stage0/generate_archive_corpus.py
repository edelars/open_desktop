#!/usr/bin/env python3
"""Generate a reproducible 7-Zip input corpus using only Python's stdlib."""

import argparse
import hashlib
from pathlib import Path


def write_file(path, data):
    path.write_bytes(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    output = parser.parse_args().output
    output.mkdir(parents=True, exist_ok=True)
    if any(output.iterdir()):
        raise SystemExit(f"output directory must be empty: {output}")

    records = ["id,name,value\n"]
    records.extend(f"{index:06d},item-{index % 257:03d},{(index * 7919) % 1000003}\n" for index in range(50_000))
    write_file(output / "records.csv", "".join(records).encode("utf-8"))

    repeated = (b"Open Desktop Stage 0 generated archive workload\n" * 21_846)[:1_048_576]
    write_file(output / "repeated.txt", repeated)

    noise = bytearray()
    counter = 0
    while len(noise) < 1_048_576:
        noise.extend(hashlib.sha256(b"open-desktop-stage0-noise-v1" + counter.to_bytes(8, "little")).digest())
        counter += 1
    write_file(output / "deterministic.bin", noise[:1_048_576])

    entries = []
    for path in sorted(output.iterdir()):
        if path.is_file() and path.name != "SHA256SUMS":
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            entries.append(f"{digest}  {path.name}")
    (output / "SHA256SUMS").write_text("\n".join(entries) + "\n", encoding="ascii")
    print(f"generated {len(entries)} files under {output}")


if __name__ == "__main__":
    main()
