#!/usr/bin/env python3
"""ADR-0023 slice C2a-1: the corpus of the legacy CPL3 paths (plan work/s7-32/c1/plan-c1-c2a.md, C2a-1).

Two steps around the Rust test `c2a_cpl3_legacy_forms` (the cut into instructions, the signature
and the "the old CPL3 entry accepts" flag are computed by Rust on the code of slice C1; this script
only collects the candidates and de-duplicates):

  candidates <sweep.txt> <out>   (a) the reduced sweep of C1-0 (`dump_sweep`), (b) the byte literals
                                 of the CPL3 tests, (c) the executable sections of the user ELFs of
                                 the Linux recipes; writes `insn|stream <source> <hex>` lines.
  corpus <classified.txt> <candidates> <out>
                                 reads `hex accepted signature sources` lines of
                                 `classify_legacy_candidates` and writes the corpus kept in Git
                                 (the size rule of plan C2a-1 item 2, decision of the slice C2a):
                                 (b) every accepted encoding; (c) per signature at most 8 accepted
                                 (distinct displacement/immediate bytes) and 2 refused when the
                                 old allow-list depends on the values (the candidates of the
                                 signature hold both accepted and refused ones), else 1 accepted
                                 or 2 refused; (a) is not listed: the golden folds the sweep of
                                 C1-0 by its groups.
  corpus-unlimited <classified.txt> <candidates> <out>
                                 the rule of the plan without the size limit: (a) at most 4 per
                                 signature, (b) every accepted, (c) at most 8 accepted and 2 refused
                                 per signature (the unreduced corpus of `work/`).

Python 3 standard library only. Paths are relative to the repository root.
"""

import gzip
import hashlib
import re
import sys
from collections import defaultdict

TEST_FILES = [
    "crates/od-isa/tests/cpl3_profile_set.rs",
    "crates/od-isa/tests/s2_39_cpl3_sse.rs",
    "crates/od-isa/tests/cpl3_syscall.rs",
    "crates/od-isa/tests/user_mode_commit.rs",
]
# (c): /init of the S6-21 recipe and the initramfs of S6-22 (`run-s6-22-prebuilt-raw.sh`).
INITRAMFS = [
    ("work/stage6/smp-init/initramfs.cpio", False),
    ("work/stage6/s6-22-rootfs/prebuilt-package-stage/main-package.sZB7fK/prebuilt.cpio.gz", True),
]
LIMIT_BYTES = 400 * 1024


def sha256(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def cpio_entries(data):
    """The regular files of a newc cpio archive: (name, mode, bytes)."""
    offset = 0
    while offset + 110 <= len(data):
        header = data[offset : offset + 110]
        if header[:6] not in (b"070701", b"070702"):
            raise SystemExit(f"cpio: bad magic at {offset}")
        fields = [int(header[6 + 8 * i : 14 + 8 * i], 16) for i in range(13)]
        mode, size, name_size = fields[1], fields[6], fields[11]
        name = data[offset + 110 : offset + 110 + name_size - 1].decode()
        body_at = (offset + 110 + name_size + 3) & ~3
        body = data[body_at : body_at + size]
        offset = (body_at + size + 3) & ~3
        if name == "TRAILER!!!":
            break
        yield name, mode, body


def elf_executable_sections(data):
    """The SHF_EXECINSTR PROGBITS sections of a 64-bit little-endian ELF: (name, bytes)."""
    if data[:4] != b"\x7fELF" or data[4] != 2 or data[5] != 1:
        return []
    u16 = lambda at: int.from_bytes(data[at : at + 2], "little")
    u32 = lambda at: int.from_bytes(data[at : at + 4], "little")
    u64 = lambda at: int.from_bytes(data[at : at + 8], "little")
    if u16(16) not in (2, 3):  # ET_EXEC, ET_DYN (not ET_REL kernel modules)
        return []
    shoff, shentsize, shnum, shstrndx = u64(0x28), u16(0x3A), u16(0x3C), u16(0x3E)
    headers = [shoff + i * shentsize for i in range(shnum)]
    strtab = headers[shstrndx]
    names_at = u64(strtab + 0x18)
    out = []
    for header in headers:
        kind, flags = u32(header + 4), u64(header + 8)
        if kind == 1 and flags & 0x4:
            name_at = names_at + u32(header)
            name = data[name_at : data.index(b"\0", name_at)].decode()
            at, size = u64(header + 0x18), u64(header + 0x20)
            out.append((name, data[at : at + size]))
    return out


def test_literals(path):
    """The byte-array literals of a Rust test file (comments removed)."""
    text = open(path).read()
    text = re.sub(r"//[^\n]*", "", text)
    pattern = re.compile(
        r"\[\s*(0x[0-9a-fA-F]{1,2}(?:u8)?(?:\s*,\s*0x[0-9a-fA-F]{1,2}(?:u8)?)*)\s*,?\s*\]"
    )
    for match in pattern.finditer(text):
        values = [int(item.strip().removesuffix("u8"), 16) for item in match.group(1).split(",")]
        yield bytes(values)


def candidates(sweep_path, out_path):
    inputs = []
    lines = []
    inputs.append((sweep_path, sha256(sweep_path)))
    for line in open(sweep_path):
        line = line.strip()
        if line:
            lines.append(f"insn a {line}")
    for path in TEST_FILES:
        inputs.append((path, sha256(path)))
        for index, literal in enumerate(test_literals(path)):
            lines.append(f"stream b:{path.rsplit('/', 1)[1]}#{index} {literal.hex()}")
    for path, compressed in INITRAMFS:
        inputs.append((path, sha256(path)))
        data = open(path, "rb").read()
        if compressed:
            data = gzip.decompress(data)
        for name, mode, body in cpio_entries(data):
            if mode & 0o170000 != 0o100000:
                continue
            for section, code in elf_executable_sections(body):
                # One stream per 64 KiB chunk (the cut restarts at a chunk start).
                for chunk in range(0, len(code), 0x10000):
                    lines.append(
                        f"stream c:{name}:{section}+{chunk:#x} {code[chunk:chunk + 0x10000].hex()}"
                    )
    with open(out_path, "w") as out:
        for path, digest in inputs:
            out.write(f"# input {path} sha256 {digest}\n")
        out.write("\n".join(lines) + "\n")


# The opcodes whose ModRM.reg selects the instruction (SDM Vol. 2 Table A-6, as `is_group` of
# `tools/runner/src/decode_coverage.rs`).
GROUP_OPCODES = {f"{byte:02x}" for byte in [*range(0x80, 0x84), 0x8F, 0xC0, 0xC1, 0xC6, 0xC7,
                                             *range(0xD0, 0xD4), *range(0xD8, 0xE0), 0xF6, 0xF7, 0xFE, 0xFF]}
GROUP_OPCODES |= {f"0f{byte:02x}" for byte in [0x00, 0x01, 0x0D, 0x18, 0x71, 0x72, 0x73, 0xAE, 0xB9, 0xBA, 0xC7]}


def group(signature):
    """The group of a signature: prefixes, opcode, the register/memory form and, for the group
    opcodes, ModRM.reg (the opcode extension)."""
    parts = signature.split("|")
    modrm = [part for part in parts if part.startswith("m")]
    if not modrm:
        return "|".join(parts[:2] + ["none"])
    form = "reg" if modrm[0].startswith("m3") else "mem"
    extension = [modrm[0][2:4]] if parts[1][1:] in GROUP_OPCODES else []
    return "|".join(parts[:2] + [form] + extension)


def select_unlimited(rows):
    chosen = {}
    per_signature = defaultdict(lambda: [0, 0, 0])  # (a) count, (c) accepted, (c) refused
    for hex_bytes, accepted, signature, sources in rows:
        reasons = []
        if "a" in sources and per_signature[signature][0] < 4:
            per_signature[signature][0] += 1
            reasons.append("a")
        if "b" in sources and accepted:
            reasons.append("b")
        if "c" in sources:
            if accepted and per_signature[signature][1] < 8:
                per_signature[signature][1] += 1
                reasons.append("c")
            elif not accepted and per_signature[signature][2] < 2:
                per_signature[signature][2] += 1
                reasons.append("c")
        if reasons:
            chosen[hex_bytes] = ("".join(reasons), accepted)
    return chosen


def select_reduced(rows):
    by_signature = defaultdict(lambda: [0, 0])
    for _, accepted, signature, sources in rows:
        if "c" in sources:
            by_signature[signature][accepted] += 1
    depends = {signature for signature, (refused, accepted) in by_signature.items() if refused and accepted}
    chosen = {}
    taken = defaultdict(lambda: [0, 0])  # (c) refused, accepted
    for hex_bytes, accepted, signature, sources in rows:
        reasons = []
        if "b" in sources and accepted:
            reasons.append("b")
        if "c" in sources:
            limit = (2, 8) if signature in depends else (2, 1)
            if taken[signature][accepted] < limit[accepted]:
                taken[signature][accepted] += 1
                reasons.append("c")
        if reasons:
            chosen[hex_bytes] = ("".join(reasons), accepted, signature)
    # (a) is not kept line by line: the golden folds the deterministic sweep of C1-0
    # (`cpl3_sweep::encodings`) by its groups (coordinator decision on review F5).
    print(f"(c) signatures {len(by_signature)}, value-dependent {len(depends)}")
    return {key: value[:2] for key, value in chosen.items()}


def corpus(classified_path, candidates_path, out_path, limit=True):
    rows = []
    for line in open(classified_path):
        if line.startswith("#") or not line.strip():
            continue
        hex_bytes, accepted, signature, sources = line.split()
        rows.append((hex_bytes, accepted == "1", signature, set(sources.split(","))))
    rows.sort()
    chosen = select_reduced(rows) if limit else select_unlimited(rows)
    header = [
        "# ADR-0023 slice C2a-1: the corpus of the legacy CPL3 paths (plan C2a-1).",
        "# Generated by c2a_cpl3_legacy_corpus_gen.py from the inputs below; one encoding per line:",
        "# <hex> <sources: a sweep, b test literals, c Linux ELF> <accepted by the C1 CPL3 entry 0/1>.",
    ]
    for line in open(candidates_path):
        if line.startswith("# input"):
            header.append(line.rstrip("\n"))
    header.append(f"# input {classified_path} sha256 {sha256(classified_path)}")
    body = [f"{hex_bytes} {reasons} {int(accepted)}" for hex_bytes, (reasons, accepted) in sorted(chosen.items())]
    text = "\n".join(header + body) + "\n"
    if limit and len(text.encode()) > LIMIT_BYTES:
        raise SystemExit(f"corpus {len(text.encode())} bytes > {LIMIT_BYTES}")
    with open(out_path, "w") as out:
        out.write(text)
    counts = defaultdict(int)
    for reasons, accepted in chosen.values():
        for reason in reasons:
            counts[(reason, accepted)] += 1
    print(f"corpus: {len(chosen)} encodings, {len(text.encode())} bytes, {dict(counts)}")


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "candidates":
        candidates(sys.argv[2], sys.argv[3])
    elif len(sys.argv) == 5 and sys.argv[1] in ("corpus", "corpus-unlimited"):
        corpus(sys.argv[2], sys.argv[3], sys.argv[4], limit=sys.argv[1] == "corpus")
    else:
        raise SystemExit(__doc__)
