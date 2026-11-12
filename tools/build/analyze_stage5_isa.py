#!/usr/bin/env python3
"""Statically inventory selected ELF files from the accepted Stage 5 rootfs."""

from __future__ import annotations

import argparse
import hashlib
import json
import lzma
import re
import ssl
import subprocess
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LOCK_PATH = ROOT / "tools/build/stage5_isa_packages.json"
PROFILE_PATH = ROOT / "tools/build/stage5_isa_profile_forms.json"
BUILDER = "ubuntu@sha256:86eb1bc1059720a19a777ba5a109e9ee3690d5dfe3e319871a6d6beb86fb7f85"
IMAGE_SHA256 = "c2b2c94665ea256c6fa19c7eeebfc8451589028df3da00b2e1e05c7473dd2895"
IMAGE_SIZE = 2_147_483_648
STAGE5_E_STATIC_SHA256 = "da18de76182af4f986a076c8ec9796ae0db2af365104788929e22643143d8024"
STAGE5_E_SOURCE_SHA256 = "9341d7fa286e8355718483e28f05d001ea37275a9e610d5fdbe1944faf08f700"
BINARIES = (
    ("bash", "/usr/bin/bash"),
    ("dash", "/usr/bin/dash"),  # /bin/sh resolves to this ELF.
    ("tar", "/usr/bin/tar"),
    ("gcc-driver", "/usr/bin/x86_64-linux-gnu-gcc-15"),
    ("gcc-cc1", "/usr/libexec/gcc/x86_64-linux-gnu/15/cc1"),
    ("gcc-collect2", "/usr/libexec/gcc/x86_64-linux-gnu/15/collect2"),
    ("gcc-as", "/usr/bin/x86_64-linux-gnu-as"),
    ("gcc-ld-bfd", "/usr/bin/x86_64-linux-gnu-ld.bfd"),
)
ROW = re.compile(r"^\s*([0-9a-fA-F]+):")
BYTE = re.compile(r"^[0-9a-fA-F]{2}$")
PREFIXES = {"lock", "rep", "repe", "repz", "repne", "repnz", "bnd", "notrack"}
TEXT_PREFIXES = PREFIXES | {"data16", "addr16", "addr32", "cs", "ds", "es", "fs", "gs", "ss", "rex", "rex.w"}
REG = re.compile(r"\b(?:r[0-9]+(?:[bwd])?|[er]?(?:ax|bx|cx|dx|si|di|bp|sp)|[abcd][lh]|sil|dil|bpl|spl|xmm[0-9]+|ymm[0-9]+|zmm[0-9]+|mm[0-9]+|st(?:\([0-7]\))?|cr[0-9]+|dr[0-9]+|rip|eip|ip|rflags|eflags|cs|ds|es|fs|gs|ss)\b", re.I)
WIDTH = re.compile(r"\b(BYTE|WORD|DWORD|QWORD|TBYTE|XMMWORD|YMMWORD|ZMMWORD)\s+PTR\b", re.I)
WIDTH_BITS = {"BYTE": 8, "WORD": 16, "DWORD": 32, "QWORD": 64, "TBYTE": 80, "XMMWORD": 128, "YMMWORD": 256, "ZMMWORD": 512}
IMM = re.compile(r"(?<![A-Za-z_])(?:-?0x[0-9a-fA-F]+|-?[0-9]+)(?![A-Za-z_])")


def digest(path: Path, algorithm: str = "sha256") -> str:
    h = hashlib.new(algorithm)
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def parse_index(path: Path) -> dict[tuple[str, str, str], dict[str, str]]:
    text = lzma.decompress(path.read_bytes()).decode()
    result = {}
    for paragraph in text.split("\n\n"):
        fields = {}
        for line in paragraph.splitlines():
            if line[:1].isspace() or ": " not in line:
                continue
            key, value = line.split(": ", 1)
            fields[key] = value
        if all(k in fields for k in ("Package", "Version", "Architecture")):
            result[(fields["Package"], fields["Version"], fields["Architecture"])] = fields
    return result


def verify_cache(cache: Path, lock: dict) -> list[Path]:
    index = cache / "main-arm64-Packages.xz"
    if digest(index) != lock["signed_packages_xz_sha256"]:
        raise ValueError("ARM64 Packages.xz missing or hash differs from signed InRelease")
    records = parse_index(index)
    files = {index.name}
    packages = []
    for package in lock["packages"]:
        path = cache / package["filename"]
        files.add(path.name)
        if digest(path) != package["sha256"] or digest(path, "sha512") != package["sha512"]:
            raise ValueError(f"package hash mismatch: {path.name}")
        key = (package["package"], package["version"], package["architecture"])
        record = records.get(key)
        expected_filename = "pool/" + package["uri"].split("/pool/", 1)[1]
        if not record or record.get("Filename") != expected_filename:
            raise ValueError(f"package identity missing in authenticated index: {key}")
        if record.get("SHA256") != package["sha256"] or record.get("SHA512") != package["sha512"]:
            raise ValueError(f"package checksum differs from authenticated index: {package['package']}")
        packages.append(path)
    unexpected = {p.name for p in cache.iterdir() if p.is_file()} - files
    if unexpected:
        raise ValueError(f"unexpected files in tool cache: {sorted(unexpected)}")
    return packages


def prepare_cache(cache: Path, lock: dict) -> None:
    """Fetch missing tool packages through the pinned image and signed snapshot."""
    cache.mkdir(parents=True, exist_ok=True)
    expected = {"main-arm64-Packages.xz"} | {item["filename"] for item in lock["packages"]}
    for path in cache.iterdir():
        if path.is_file() and path.name not in expected:
            raise ValueError(f"unexpected file in tool cache: {path.name}")
    if all((cache / filename).is_file() for filename in expected):
        return
    ca_candidates = [Path("/etc/ssl/cert.pem")]
    system_ca = ssl.get_default_verify_paths().cafile
    if system_ca:
        ca_candidates.append(Path(system_ca))
    ca_file = next((p.resolve() for p in ca_candidates if p.is_file()), None)
    if ca_file is None:
        raise RuntimeError("system CA bundle missing; cannot fetch pinned tools over verified HTTPS")
    package_specs = " ".join(
        f"{item['package']}:{item['architecture']}={item['version']}" for item in lock["packages"]
    )
    script = f"""
set -eu
cat >/tmp/s5-21.sources <<'EOF'
Types: deb
URIs: {lock['apt_snapshot']}
Suites: {lock['suite']}
Components: {' '.join(lock['components'])}
Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg
EOF
apt-get -o Acquire::https::CaInfo=/host-ca.pem -o Dir::Etc::sourcelist=/tmp/s5-21.sources -o Dir::Etc::sourceparts=- -o APT::Get::List-Cleanup=0 -o Acquire::Check-Valid-Until=false update
inrelease=$(sha256sum /var/lib/apt/lists/*InRelease | awk '{{print $1}}')
test "$inrelease" = {lock['signed_inrelease_sha256']}
if [ ! -f /out/main-arm64-Packages.xz ]; then
  /usr/lib/apt/apt-helper -o Acquire::https::CaInfo=/host-ca.pem download-file {lock['apt_snapshot']}{lock['signed_packages_xz']} /out/main-arm64-Packages.xz SHA256:{lock['signed_packages_xz_sha256']}
fi
cd /out
apt-get -o Acquire::https::CaInfo=/host-ca.pem -o Dir::Etc::sourcelist=/tmp/s5-21.sources -o Dir::Etc::sourceparts=- download {package_specs}
"""
    subprocess.run([
        "docker", "run", "--rm", "--platform", "linux/arm64",
        "--mount", f"type=bind,src={ca_file},dst=/host-ca.pem,readonly",
        "--mount", f"type=bind,src={cache.resolve()},dst=/out",
        BUILDER, "/bin/sh", "-ec", script,
    ], check=True)


def reg_class(name: str) -> str:
    s = name.lower()
    if s.startswith("xmm"): return "xmm"
    if s.startswith("ymm"): return "ymm"
    if s.startswith("zmm"): return "zmm"
    if s.startswith("mm"): return "mm"
    if s.startswith("cr"): return "cr"
    if s.startswith("dr"): return "dr"
    if s.startswith("st"): return "st"
    if s in {"rip", "eip", "ip"}: return "ip"
    if s in {"rflags", "eflags"}: return "flags"
    if s in {"cs", "ds", "es", "fs", "gs", "ss"}: return s
    if s in {"al", "ah", "bl", "bh", "cl", "ch", "dl", "dh", "sil", "dil", "bpl", "spl"}: return "r8"
    if s in {"ax", "bx", "cx", "dx", "si", "di", "bp", "sp"}: return "r16"
    if s.startswith("e") and s[1:] in {"ax", "bx", "cx", "dx", "si", "di", "bp", "sp"}: return "r32"
    if s.startswith("r") and s[1:] in {"ax", "bx", "cx", "dx", "si", "di", "bp", "sp"}: return "r64"
    if re.fullmatch(r"r\d+", s): return "r64"
    m = re.fullmatch(r"r\d+([bwd])", s)
    if m: return {"b": "r8", "w": "r16", "d": "r32"}[m.group(1)]
    return "reg"


def operand_class(operand: str) -> str:
    text = re.sub(r"\s*<[^>]*>", "", operand).strip().lower()
    if not text: return "none"
    width = WIDTH.search(text)
    if "[" in text and "]" in text:
        return f"mem{WIDTH_BITS[width.group(1).upper()]}" if width else "mem"
    if IMM.fullmatch(text): return "imm"
    if REG.fullmatch(text): return reg_class(text)
    if width: return f"mem{WIDTH_BITS[width.group(1).upper()]}"
    return "other"


def parse_row(line: str) -> dict | None:
    address_match = ROW.match(line)
    if not address_match: return None
    address, rest = line.strip().split(":", 1)
    tokens = rest.strip().split()
    raw_bytes = []
    while tokens and BYTE.fullmatch(tokens[0]): raw_bytes.append(tokens.pop(0).lower())
    if not raw_bytes or not tokens: return None
    text_prefixes = []
    while tokens and tokens[0].lower() in TEXT_PREFIXES:
        text_prefixes.append(tokens.pop(0).lower())
    if not tokens: return None
    mnemonic = tokens.pop(0).lower()
    semantic_prefix = next((item for item in reversed(text_prefixes) if item in PREFIXES), None)
    if semantic_prefix:
        semantic_prefix = {"repz": "repe", "repnz": "repne"}.get(semantic_prefix, semantic_prefix)
        mnemonic = semantic_prefix + " " + mnemonic
    operands = " ".join(tokens).split("#", 1)[0].strip()
    operand_list = [x.strip() for x in operands.split(",")] if operands else []
    classes = [operand_class(x) for x in operand_list]
    if (mnemonic in {"call", "jmp", "loop", "loope", "loopne", "jrcxz", "jecxz"} or mnemonic.startswith("j")) and classes and classes[0] in {"imm", "other"}: classes = ["rel"]
    signature = mnemonic + (" " + ",".join(classes) if classes else "")
    return {"address": address.lower(), "bytes": " ".join(raw_bytes), "byte_values": [int(value, 16) for value in raw_bytes], "mnemonic": mnemonic, "operands": operands, "signature": signature}


def opcode_view(row: dict) -> dict:
    """Return prefix/opcode/ModRM fields used only for bounded inventory matching."""
    data = row["byte_values"]
    index = 0
    prefixes = []
    while index < len(data):
        value = data[index]
        if value in {0xF0, 0xF2, 0xF3, 0x2E, 0x36, 0x3E, 0x26, 0x64, 0x65, 0x66, 0x67} or 0x40 <= value <= 0x4F:
            prefixes.append(value)
            index += 1
        else:
            break
    if index >= len(data):
        return {"prefixes": prefixes, "opcode": (), "modrm": None, "opcode_index": index}
    opcode = [data[index]]
    index += 1
    if opcode[0] == 0x0F and index < len(data):
        opcode.append(data[index])
        index += 1
        if opcode[1] in {0x38, 0x3A} and index < len(data):
            opcode.append(data[index])
            index += 1
    if len(opcode) > 1 and opcode[0] == 0x0F:
        no_modrm_0f = {0x05,0x06,0x07,0x08,0x09,0x0B,0x30,0x31,0x32,0x34,0x35,0x37,0x80,0x81,0x82,0x83,0x84,0x85,0x86,0x87,0x88,0x89,0x8A,0x8B,0x8C,0x8D,0x8E,0x8F,0xA0,0xA1,0xA2,0xA8,0xA9,0xAA,0xC8,0xC9,0xCA,0xCB,0xCC,0xCD,0xCE,0xCF}
        has_modrm = opcode[1] not in no_modrm_0f
    else:
        op = opcode[0]
        arithmetic_modrm = set(range(0x00,0x04)) | set(range(0x08,0x0C)) | set(range(0x10,0x14)) | set(range(0x18,0x1C)) | set(range(0x20,0x24)) | set(range(0x28,0x2C)) | set(range(0x30,0x34)) | set(range(0x38,0x3C))
        has_modrm = op in arithmetic_modrm or 0xD8 <= op <= 0xDF or op in {
            0x62,0x63,0x69,0x6B,0x80,0x81,0x82,0x83,0x84,0x85,0x86,0x87,
            0x88,0x89,0x8A,0x8B,0x8C,0x8D,0x8E,0x8F,0xC0,0xC1,0xC4,0xC5,
            0xC6,0xC7,0xD0,0xD1,0xD2,0xD3,0xF6,0xF7,0xFE,0xFF,
        }
    modrm = data[index] if has_modrm and index < len(data) else None
    return {"prefixes": prefixes, "opcode": tuple(opcode), "modrm": modrm, "opcode_index": index}


def instruction_dimensions(row: dict) -> dict:
    """Stable encoding key preserving prefix, opcode, ModRM, SIB and address form."""
    view = opcode_view(row)
    data = row["byte_values"]
    modrm = view["modrm"]
    modrm_fields = None
    sib = None
    if modrm is not None:
        modrm_fields = {"raw": f"{modrm:02X}", "mod": modrm >> 6, "reg": (modrm >> 3) & 7, "rm": modrm & 7}
        if (modrm >> 6) != 3 and (modrm & 7) == 4 and view["opcode_index"] + 1 < len(data):
            sib = f"{data[view['opcode_index'] + 1]:02X}"
    memory_text = " ".join(re.findall(r"(?:BYTE|WORD|DWORD|QWORD|TBYTE|XMMWORD|YMMWORD|ZMMWORD) PTR\s*[^,]+", row["operands"], re.I)).lower()
    if not memory_text and "[" in row["operands"]:
        memory_text = row["operands"].lower()
    has_memory_operand = "[" in memory_text or bool(WIDTH.search(memory_text))
    if not has_memory_operand:
        address = {"form": "none", "size": None, "segment": None}
    else:
        body = memory_text.split("[", 1)[1].split("]", 1)[0] if "[" in memory_text else ""
        regs = re.findall(r"\b(?:r\d+[bwd]?|e(?:ax|bx|cx|dx|si|di|bp|sp|ip)|(?:rax|rbx|rcx|rdx|rsi|rdi|rbp|rsp|rip)|[abcd][lh]|sil|dil|bpl|spl)\b", body, re.I)
        if re.search(r"\b(?:rip|eip|ip)\b", body, re.I): form = "rip_relative"
        elif "*" in body: form = "scaled_index"
        elif len(regs) >= 2: form = "base_index"
        elif len(regs) == 1: form = "base_only"
        else: form = "absolute"
        segment = next((name for name in ("fs", "gs", "cs", "ss", "ds", "es") if re.search(rf"\b{name}:", memory_text)), None)
        address = {"form": form, "size": 32 if 0x67 in view["prefixes"] else 64, "segment": segment}
    dimensions = {
        "prefix_bytes": [f"{value:02X}" for value in view["prefixes"]],
        "opcode_bytes": [f"{value:02X}" for value in view["opcode"]],
        "modrm": modrm_fields,
        "sib": sib,
        "operand_signature": row["signature"],
        "address": address,
        "instruction_length": len(data),
    }
    dimensions["encoding_key"] = json.dumps(dimensions, sort_keys=True, separators=(",", ":"))
    return dimensions


def _group(opcode: tuple[int, ...], modrm: int | None, allowed: set[int], extensions: set[int] = set(range(8))) -> bool:
    return bool(modrm is not None and len(opcode) == 1 and opcode[0] in allowed and ((modrm >> 3) & 7) in extensions)


def _widths_in_row(row: dict) -> set[int]:
    return {int(value) for value in re.findall(r"(?:r|mem)(8|16|32|64|128|256|512)\b", row["signature"])}


def _widths_allowed(task: str, row: dict) -> bool:
    widths = _widths_in_row(row)
    allowed = {
        "S2-08": {8,16,32,64}, "S2-10": {8,16,32,64}, "S2-11": {8,16,32,64},
        "S2-12": {8,16,32,64}, "S2-13": {8,16,32,64}, "S2-14": {8,16,32,64},
        "S2-15..16": {8,16,32,64}, "S2-16": {8,16,32,64}, "S2-17": {16,64},
        "S2-18..19": {8,16,32,64}, "S2-20-fault-only": set(), "S2-21": {64},
    }.get(task, set())
    if not widths.issubset(allowed):
        return False
    view = opcode_view(row)
    opcode = view["opcode"]
    if len(opcode) == 2 and opcode[0] == 0x0F and 0xC8 <= opcode[1] <= 0xCF and 16 in widths:
        return False  # S2-15b accepts BSWAP r32/r64 only.
    return True


PUNPCK_LOCAL_PROFILES = {
    0x60: ("punpcklbw", "S2-35.24-PUNPCKLBW"),
    0x68: ("punpckhbw", "S2-35.25-PUNPCKHBW"),
    0x61: ("punpcklwd", "S2-35.26-PUNPCKLWD"),
    0x69: ("punpckhwd", "S2-35.27-PUNPCKHWD"),
    0x62: ("punpckldq", "S2-35.28-PUNPCKLDQ"),
    0x6A: ("punpckhdq", "S2-35.29-PUNPCKHDQ"),
    0x6C: ("punpcklqdq", "S2-35.30-PUNPCKLQDQ"),
    0x6D: ("punpckhqdq", "S2-35.31-PUNPCKHQDQ"),
}
PUNPCK_LOCAL_PROFILES_BY_MNEMONIC = {
    mnemonic: (opcode, profile)
    for opcode, (mnemonic, profile) in PUNPCK_LOCAL_PROFILES.items()
}
PUNPCK_ALLOWED_PREFIXES = {
    (0x66,),
    (0x66, 0x40),
    (0x66, 0x41),
    (0x66, 0x44),
    (0x66, 0x45),
}
PSHUFD_ALLOWED_PREFIXES = PUNPCK_ALLOWED_PREFIXES
PSRLDQ_ALLOWED_PREFIXES = {(0x66,), (0x66, 0x41)}
PSLLD_ALLOWED_PREFIXES = {(0x66,), (0x66, 0x41)}
PSRLQ_ALLOWED_PREFIXES = {(0x66,), (0x66, 0x41)}
PSRAW_ALLOWED_PREFIXES = {(0x66,), (0x66, 0x41)}
PSRLD_ALLOWED_PREFIXES = {(0x66,), (0x66, 0x41)}
PSRAD_ALLOWED_PREFIXES = {(0x66,), (0x66, 0x41)}
PSUBW_ALLOWED_PREFIXES = {(0x66,), (0x66, 0x40), (0x66, 0x41), (0x66, 0x44), (0x66, 0x45)}
PADDB_ALLOWED_PREFIXES = PSUBW_ALLOWED_PREFIXES
PADDW_ALLOWED_PREFIXES = PSUBW_ALLOWED_PREFIXES
MOVSD_REGISTER_ALLOWED_PREFIXES = {(0xF2,), (0xF2, 0x40), (0xF2, 0x41), (0xF2, 0x44), (0xF2, 0x45)}
PCMPGTB_ALLOWED_PREFIXES = PUNPCK_ALLOWED_PREFIXES


SIMD_LOGIC_NO_PREFIX_PROFILES = {
    0x57: ("xorps", "S2-34.7a-XORPS-register-register"),
    0x54: ("andps", "S2-34.7b-ANDPS-register-register"),
    0x56: ("orps", "S2-34.7c-ORPS-register-register"),
    0x55: ("andnps", "S2-34.7d-ANDNPS-register-register"),
}
SIMD_LOGIC_NO_PREFIX_BY_MNEMONIC = {
    mnemonic: (opcode, profile)
    for opcode, (mnemonic, profile) in SIMD_LOGIC_NO_PREFIX_PROFILES.items()
}
SIMD_LOGIC_66_PROFILES = {
    0xDF: ("pandn", "S2-35.50-PANDN-register-register"),
    0xEB: ("por", "S2-35.51-POR-register-register"),
}
SIMD_LOGIC_66_BY_MNEMONIC = {
    mnemonic: (opcode, profile)
    for opcode, (mnemonic, profile) in SIMD_LOGIC_66_PROFILES.items()
}
SIMD_LOGIC_NO_PREFIX_ALLOWED = {(), (0x40,), (0x41,), (0x44,), (0x45,)}
SIMD_LOGIC_66_ALLOWED = {(0x66,)} | {
    (0x66, rex) for rex in (0x40, 0x41, 0x44, 0x45)
}


def simd_logic_local_profile_candidate(row: dict) -> str | None:
    """Match reviewed local register logic profiles, excluding pending PAND."""
    no_prefix_entry = SIMD_LOGIC_NO_PREFIX_BY_MNEMONIC.get(row["mnemonic"])
    mandatory_66_entry = SIMD_LOGIC_66_BY_MNEMONIC.get(row["mnemonic"])
    if no_prefix_entry is None and mandatory_66_entry is None:
        return None
    if row["signature"] != f"{row['mnemonic']} xmm,xmm":
        return None
    opcode, profile = no_prefix_entry or mandatory_66_entry
    view = opcode_view(row)
    if view["opcode"] != (0x0F, opcode):
        return None
    prefixes = tuple(view["prefixes"])
    allowed_prefixes = SIMD_LOGIC_NO_PREFIX_ALLOWED if no_prefix_entry else SIMD_LOGIC_66_ALLOWED
    if prefixes not in allowed_prefixes:
        return None
    modrm = view["modrm"]
    if modrm is None or modrm >> 6 != 3:
        return None
    if len(row["byte_values"]) != len(prefixes) + 3:
        return None
    return profile


def punpck_local_profile_candidate(row: dict) -> str | None:
    """Match only the accepted local XMM register-register PUNPCK slices."""
    entry = PUNPCK_LOCAL_PROFILES_BY_MNEMONIC.get(row["mnemonic"])
    if entry is None:
        return None
    opcode, profile = entry
    if row["signature"] != f"{row['mnemonic']} xmm,xmm":
        return None
    view = opcode_view(row)
    if view["opcode"] != (0x0F, opcode):
        return None
    prefixes = tuple(view["prefixes"])
    if prefixes not in PUNPCK_ALLOWED_PREFIXES:
        return None
    modrm = view["modrm"]
    if modrm is None or modrm >> 6 != 3:
        return None
    if len(row["byte_values"]) != len(prefixes) + 3:
        return None
    return profile


def pshufd_local_profile_candidate(row: dict) -> str | None:
    """Match only accepted S2-35.35 legacy XMM register PSHUFD forms."""
    if row["mnemonic"] != "pshufd" or row["signature"] != "pshufd xmm,xmm,imm":
        return None
    view = opcode_view(row)
    if view["opcode"] != (0x0F, 0x70):
        return None
    prefixes = tuple(view["prefixes"])
    if prefixes not in PSHUFD_ALLOWED_PREFIXES:
        return None
    modrm = view["modrm"]
    if modrm is None or modrm >> 6 != 3:
        return None
    # Mandatory 66 + optional REX.R/B + 0F 70 + ModR/M + imm8.
    # Two opcode bytes, one ModR/M byte, and one imm8 follow the prefixes.
    if len(row["byte_values"]) != len(prefixes) + 4:
        return None
    return "S2-35.35-PSHUFD-XMM-register-register"


def psrldq_local_profile_candidate(row: dict) -> str | None:
    """Match only the accepted S2-35.47 XMM register immediate form."""
    if row["mnemonic"] != "psrldq" or row["signature"] != "psrldq xmm,imm":
        return None
    view = opcode_view(row)
    if view["opcode"] != (0x0F, 0x73):
        return None
    prefixes = tuple(view["prefixes"])
    if prefixes not in PSRLDQ_ALLOWED_PREFIXES:
        return None
    modrm = view["modrm"]
    if modrm is None or modrm >> 6 != 3 or ((modrm >> 3) & 7) != 3:
        return None
    # Mandatory 66 + optional REX.B + 0F 73 /3 + immediate byte.
    if len(row["byte_values"]) != len(prefixes) + 4:
        return None
    return "S2-35.47-PSRLDQ-XMM-immediate-register"


def pslld_local_profile_candidate(row: dict) -> str | None:
    """Match only the accepted S2-35.41 XMM register immediate form."""
    if row["mnemonic"] != "pslld" or row["signature"] != "pslld xmm,imm":
        return None
    view = opcode_view(row)
    if view["opcode"] != (0x0F, 0x72):
        return None
    prefixes = tuple(view["prefixes"])
    if prefixes not in PSLLD_ALLOWED_PREFIXES:
        return None
    modrm = view["modrm"]
    if modrm is None or modrm >> 6 != 3 or ((modrm >> 3) & 7) != 6:
        return None
    # Mandatory 66 + optional REX.B + 0F 72 /6 + immediate byte.
    if len(row["byte_values"]) != len(prefixes) + 4:
        return None
    return "S2-35.41-PSLLD-XMM-immediate-register"


def psrld_local_profile_candidate(row: dict) -> str | None:
    """Match only the accepted S2-35.42 XMM register immediate form."""
    if row["mnemonic"] != "psrld" or row["signature"] != "psrld xmm,imm":
        return None
    view = opcode_view(row)
    if view["opcode"] != (0x0F, 0x72):
        return None
    prefixes = tuple(view["prefixes"])
    if prefixes not in PSRLD_ALLOWED_PREFIXES:
        return None
    modrm = view["modrm"]
    if modrm is None or modrm >> 6 != 3 or ((modrm >> 3) & 7) != 2:
        return None
    # Mandatory 66 + optional REX.B + 0F 72 /2 + full imm8 byte.
    if len(row["byte_values"]) != len(prefixes) + 4:
        return None
    return "S2-35.42-PSRLD-XMM-immediate-register"


def psrad_local_profile_candidate(row: dict) -> str | None:
    """Match only the accepted S2-35.43 XMM register immediate form."""
    if row["mnemonic"] != "psrad" or row["signature"] != "psrad xmm,imm":
        return None
    view = opcode_view(row)
    if view["opcode"] != (0x0F, 0x72):
        return None
    prefixes = tuple(view["prefixes"])
    if prefixes not in PSRAD_ALLOWED_PREFIXES:
        return None
    modrm = view["modrm"]
    if modrm is None or modrm >> 6 != 3 or ((modrm >> 3) & 7) != 4:
        return None
    # Mandatory 66 + optional REX.B + 0F 72 /4 + the complete imm8.
    if len(row["byte_values"]) != len(prefixes) + 4:
        return None
    return "S2-35.43-PSRAD-XMM-immediate-register"


def psubw_local_profile_candidate(row: dict) -> str | None:
    """Match only the accepted S2-35.7 XMM register-register form."""
    if row["mnemonic"] != "psubw" or row["signature"] != "psubw xmm,xmm":
        return None
    view = opcode_view(row)
    if view["opcode"] != (0x0F, 0xF9):
        return None
    prefixes = tuple(view["prefixes"])
    if prefixes not in PSUBW_ALLOWED_PREFIXES:
        return None
    modrm = view["modrm"]
    if modrm is None or modrm >> 6 != 3:
        return None
    # Mandatory 66 + optional REX.R/B + 0F F9 + register ModR/M.
    if len(row["byte_values"]) != len(prefixes) + 3:
        return None
    return "S2-35.7-PSUBW-XMM-register-register"


def paddb_local_profile_candidate(row: dict) -> str | None:
    """Match only the accepted S2-35.2 XMM register-register form."""
    if row["mnemonic"] != "paddb" or row["signature"] != "paddb xmm,xmm":
        return None
    view = opcode_view(row)
    if view["opcode"] != (0x0F, 0xFC):
        return None
    prefixes = tuple(view["prefixes"])
    if prefixes not in PADDB_ALLOWED_PREFIXES:
        return None
    modrm = view["modrm"]
    if modrm is None or modrm >> 6 != 3:
        return None
    # Mandatory 66 + optional REX.R/B + 0F FC + register ModR/M.
    if len(row["byte_values"]) != len(prefixes) + 3:
        return None
    return "S2-35.2-PADDB-XMM-register-register"


def paddw_local_profile_candidate(row: dict) -> str | None:
    """Match only the accepted S2-35.6 XMM register-register form."""
    if row["mnemonic"] != "paddw" or row["signature"] != "paddw xmm,xmm":
        return None
    view = opcode_view(row)
    if view["opcode"] != (0x0F, 0xFD):
        return None
    prefixes = tuple(view["prefixes"])
    if prefixes not in PADDW_ALLOWED_PREFIXES:
        return None
    modrm = view["modrm"]
    if modrm is None or modrm >> 6 != 3:
        return None
    # Mandatory 66 + optional REX.R/B + 0F FD + register ModR/M.
    if len(row["byte_values"]) != len(prefixes) + 3:
        return None
    return "S2-35.6-PADDW-XMM-register-register"


def movsd_register_local_profile_candidate(row: dict) -> str | None:
    """Match only the accepted S2-34.9a legacy MOVSD XMM register form."""
    if row["mnemonic"] != "movsd" or row["signature"] != "movsd xmm,xmm":
        return None
    view = opcode_view(row)
    if view["opcode"] != (0x0F, 0x10):
        return None
    prefixes = tuple(view["prefixes"])
    if prefixes not in MOVSD_REGISTER_ALLOWED_PREFIXES:
        return None
    modrm = view["modrm"]
    if modrm is None or modrm >> 6 != 3:
        return None
    # Mandatory F2 + optional REX.R/B + 0F 10 + register ModR/M.
    if len(row["byte_values"]) != len(prefixes) + 3:
        return None
    return "S2-34.9a-MOVSD-XMM-register-register"


def pcmpgtb_local_profile_candidate(row: dict) -> str | None:
    """Match only the accepted S2-35.21 XMM register-register form."""
    if row["mnemonic"] != "pcmpgtb" or row["signature"] != "pcmpgtb xmm,xmm":
        return None
    view = opcode_view(row)
    if view["opcode"] != (0x0F, 0x64):
        return None
    prefixes = tuple(view["prefixes"])
    if prefixes not in PCMPGTB_ALLOWED_PREFIXES:
        return None
    modrm = view["modrm"]
    if modrm is None or modrm >> 6 != 3:
        return None
    # Mandatory 66 + optional REX.R/B + 0F 64 + register ModR/M.
    if len(row["byte_values"]) != len(prefixes) + 3:
        return None
    return "S2-35.21-PCMPGTB-XMM-register-register"


def psrlq_local_profile_candidate(row: dict) -> str | None:
    """Match only the accepted S2-35.45 XMM register immediate form."""
    if row["mnemonic"] != "psrlq" or row["signature"] != "psrlq xmm,imm":
        return None
    view = opcode_view(row)
    if view["opcode"] != (0x0F, 0x73):
        return None
    prefixes = tuple(view["prefixes"])
    if prefixes not in PSRLQ_ALLOWED_PREFIXES:
        return None
    modrm = view["modrm"]
    if modrm is None or modrm >> 6 != 3 or ((modrm >> 3) & 7) != 2:
        return None
    # Mandatory 66 + optional REX.B + 0F 73 /2 + immediate byte.
    if len(row["byte_values"]) != len(prefixes) + 4:
        return None
    return "S2-35.45-PSRLQ-XMM-immediate-register"


def psraw_local_profile_candidate(row: dict) -> str | None:
    """Match only the accepted S2-35.39 XMM register immediate form."""
    if row["mnemonic"] != "psraw" or row["signature"] != "psraw xmm,imm":
        return None
    view = opcode_view(row)
    if view["opcode"] != (0x0F, 0x71):
        return None
    prefixes = tuple(view["prefixes"])
    if prefixes not in PSRAW_ALLOWED_PREFIXES:
        return None
    modrm = view["modrm"]
    if modrm is None or modrm >> 6 != 3 or ((modrm >> 3) & 7) != 4:
        return None
    # Mandatory 66 + optional REX.B + 0F 71 /4 + the full imm8 byte.
    if len(row["byte_values"]) != len(prefixes) + 4:
        return None
    return "S2-35.39-PSRAW-XMM-immediate-register"


def pxor_local_profile_candidate(row: dict) -> str | None:
    """Match accepted S2-35.1 XMM register-only PXOR, never MMX or memory."""
    if row["mnemonic"] != "pxor" or row["signature"] != "pxor xmm,xmm":
        return None
    view = opcode_view(row)
    if view["opcode"] != (0x0F, 0xEF):
        return None
    prefixes = view["prefixes"]
    if not prefixes or prefixes[0] != 0x66 or len(prefixes) > 2:
        return None
    if len(prefixes) == 2:
        rex = prefixes[1]
        if not 0x40 <= rex <= 0x4F or rex & 0x0A:
            return None
    modrm = view["modrm"]
    if modrm is None or modrm >> 6 != 3:
        return None
    if len(row["byte_values"]) != len(prefixes) + 3:
        return None
    return "S2-35.1-PXOR-XMM-register-register"


def movups_local_profile_candidate(row: dict) -> str | None:
    """Match only accepted S2-34.4/.5/.6a-c local MOVUPS memory forms."""
    if row["mnemonic"] != "movups" or row["signature"] not in {"movups xmm,mem128", "movups mem128,xmm"}:
        return None
    view = opcode_view(row)
    if view["opcode"] not in {(0x0F, 0x10), (0x0F, 0x11)}:
        return None
    prefixes = view["prefixes"]
    # These local contracts allow at most one effective REX byte. The individual
    # masks below keep REX.R/B/X/W permissions specific to each profile slice.
    if len(prefixes) > 1 or any(not 0x40 <= value <= 0x4F for value in prefixes):
        return None
    rex = prefixes[0] if prefixes else 0x40
    rex_bits = rex & 0x0F
    modrm = view["modrm"]
    if modrm is None:
        return None
    mod, rm = modrm >> 6, modrm & 7
    if mod == 3:
        return None
    data = row["byte_values"]
    sib_present = rm == 4
    disp_size = 0
    if mod == 1:
        disp_size = 1
    elif mod == 2:
        disp_size = 4
    elif mod == 0 and (rm == 5 or sib_present and view["opcode_index"] + 1 < len(data) and (data[view["opcode_index"] + 1] & 7) == 5):
        disp_size = 4
    expected_length = len(prefixes) + 3 + int(sib_present) + disp_size
    if len(data) != expected_length:
        return None

    if sib_present:
        # S2-34.6c accepts only a 64-bit SIB form and REX.R/B/X, never REX.W.
        if rex_bits & 0x08:
            return None
        return "S2-34.6c-MOVUPS-SIB"

    if mod == 0 and rm == 0 and not rex_bits & 0x01:
        # S2-34.4/.5 are the two exact [RAX] directions; only REX.R is allowed.
        if rex_bits & ~0x04:
            return None
        if view["opcode"] == (0x0F, 0x10) and row["signature"] == "movups xmm,mem128":
            return "S2-34.4-MOVUPS-memory-load-RAX"
        if view["opcode"] == (0x0F, 0x11) and row["signature"] == "movups mem128,xmm":
            return "S2-34.5-MOVUPS-memory-store-RAX"
        return None

    if mod == 0 and rm == 5:
        # S2-34.6b is RIP-relative disp32; REX.B/X/W are excluded.
        if rex_bits & ~0x04:
            return None
        return "S2-34.6b-MOVUPS-RIP-relative"

    # S2-34.6a is base+optional displacement without SIB, RIP-relative, or
    # legacy prefixes; it admits REX.R/B but rejects REX.X/W.
    if rex_bits & ~0x05:
        return None
    return "S2-34.6a-MOVUPS-base-displacement"


def local_profile_candidate(row: dict) -> str | None:
    """Match broad, source-backed local core families; never implies machine CPL."""
    psraw_candidate = psraw_local_profile_candidate(row)
    if psraw_candidate is not None:
        return psraw_candidate
    psrld_candidate = psrld_local_profile_candidate(row)
    if psrld_candidate is not None:
        return psrld_candidate
    psrad_candidate = psrad_local_profile_candidate(row)
    if psrad_candidate is not None:
        return psrad_candidate
    psubw_candidate = psubw_local_profile_candidate(row)
    if psubw_candidate is not None:
        return psubw_candidate
    paddb_candidate = paddb_local_profile_candidate(row)
    if paddb_candidate is not None:
        return paddb_candidate
    paddw_candidate = paddw_local_profile_candidate(row)
    if paddw_candidate is not None:
        return paddw_candidate
    movsd_register_candidate = movsd_register_local_profile_candidate(row)
    if movsd_register_candidate is not None:
        return movsd_register_candidate
    pcmpgtb_candidate = pcmpgtb_local_profile_candidate(row)
    if pcmpgtb_candidate is not None:
        return pcmpgtb_candidate
    psrlq_candidate = psrlq_local_profile_candidate(row)
    if psrlq_candidate is not None:
        return psrlq_candidate
    pslld_candidate = pslld_local_profile_candidate(row)
    if pslld_candidate is not None:
        return pslld_candidate
    psrldq_candidate = psrldq_local_profile_candidate(row)
    if psrldq_candidate is not None:
        return psrldq_candidate
    pshufd_candidate = pshufd_local_profile_candidate(row)
    if pshufd_candidate is not None:
        return pshufd_candidate
    simd_logic_candidate = simd_logic_local_profile_candidate(row)
    if simd_logic_candidate is not None:
        return simd_logic_candidate
    punpck_candidate = punpck_local_profile_candidate(row)
    if punpck_candidate is not None:
        return punpck_candidate
    pxor_candidate = pxor_local_profile_candidate(row)
    if pxor_candidate is not None:
        return pxor_candidate
    movups_candidate = movups_local_profile_candidate(row)
    if movups_candidate is not None:
        return movups_candidate
    view = opcode_view(row)
    prefixes = set(view["prefixes"])
    opcode, modrm, mnemonic = view["opcode"], view["modrm"], row["mnemonic"]
    if mnemonic.startswith("lock "):
        mnemonic = mnemonic[5:]
    op = opcode[0] if len(opcode) == 1 else None
    if prefixes.intersection({0x26,0x2E,0x36,0x3E,0x64,0x65}):
        # Segment-prefixed forms require their own exact bounded profile.
        return None
    if 0xF0 in prefixes and not (
        (len(opcode) == 2 and opcode in {(0x0F, 0xC1), (0x0F, 0xB1)} or opcode == (0x87,))
        and modrm is not None and ((modrm >> 6) & 3) != 3
    ):
        return None
    if any(value in prefixes for value in (0xF2, 0xF3)) and op not in {0xA4,0xA5,0xA6,0xA7,0xAA,0xAB,0xAC,0xAD,0xAE,0xAF}:
        return None
    two = opcode if len(opcode) > 1 else ()
    if (op is not None and (0xB0 <= op <= 0xBF or 0x88 <= op <= 0x8B)) and mnemonic in {"mov", "movabs"} and _widths_allowed("S2-08", row):
        return "S2-08"
    xchg_register = op in {0x86,0x87} and "mem" not in row["signature"] and 0xF0 not in prefixes
    if (op in {0xC6, 0xC7, 0xA0, 0xA1, 0xA2, 0xA3, 0x63, 0x8D} or two in {(0x0F, 0xBE), (0x0F, 0xBF), (0x0F, 0xB6), (0x0F, 0xB7)} or (op is not None and 0x90 <= op <= 0x97) or xchg_register) and mnemonic in {"mov", "movabs", "movsx", "movsxd", "movzx", "lea", "xchg", "nop"} and _widths_allowed("S2-10", row):
        return "S2-10"
    if op in {0x98, 0x99} and mnemonic in {"cbw", "cwde", "cdqe", "cwd", "cdq", "cqo"} and not prefixes.intersection({0xF0, 0xF2, 0xF3, 0x26, 0x2E, 0x36, 0x3E, 0x64, 0x65}):
        return "S2-15a"
    arithmetic = {0x00,0x01,0x02,0x03,0x04,0x05,0x10,0x11,0x12,0x13,0x14,0x15,0x18,0x19,0x1A,0x1B,0x1C,0x1D,0x28,0x29,0x2A,0x2B,0x2C,0x2D,0x38,0x39,0x3A,0x3B,0x3C,0x3D}
    if ((op in arithmetic) or _group(opcode, modrm, {0x80,0x81,0x83}, {0,2,3,5,7}) or _group(opcode, modrm, {0xFE,0xFF}, {0,1}) or _group(opcode, modrm, {0xF6,0xF7}, {3})) and mnemonic in {"add","adc","sub","sbb","cmp","inc","dec","neg"} and _widths_allowed("S2-11", row):
        return "S2-11"
    logic = {0x08,0x09,0x0A,0x0B,0x0C,0x0D,0x20,0x21,0x22,0x23,0x24,0x25,0x30,0x31,0x32,0x33,0x34,0x35,0xA8,0xA9}
    if ((op in logic) or _group(opcode, modrm, {0x80,0x81,0x83}, {1,4,6}) or _group(opcode, modrm, {0xF6,0xF7}, {0,2}) or op in {0x84,0x85}) and mnemonic in {"and","or","xor","test","not"} and _widths_allowed("S2-12", row):
        return "S2-12"
    if ((op in {0xC0,0xC1,0xD0,0xD1,0xD2,0xD3} and _group(opcode, modrm, {op}, {0,1,2,3,4,5,7})) or two in {(0x0F,0xA4),(0x0F,0xA5),(0x0F,0xAC),(0x0F,0xAD)}) and mnemonic in {"rol","ror","rcl","rcr","shl","sal","shr","sar","shld","shrd"} and _widths_allowed("S2-13", row):
        return "S2-13"
    if ((op in {0x69,0x6B}) or two == (0x0F,0xAF) or _group(opcode, modrm, {0xF6,0xF7}, {4,5,6,7})) and mnemonic in {"mul","imul","div","idiv"} and _widths_allowed("S2-14", row):
        return "S2-14"
    if (op is not None and 0x70 <= op <= 0x7F) or op in {0xE9,0xEB}:
        return "S2-16"
    if two and two[0] == 0x0F and (two[1] in {0x80,0x81,0x82,0x83,0x84,0x85,0x86,0x87,0x88,0x89,0x8A,0x8B,0x8C,0x8D,0x8E,0x8F,0xA3,0xAB,0xB3,0xBB,0xBC,0xBD,0xC8,0xC9,0xCA,0xCB,0xCC,0xCD,0xCE,0xCF,0x90,0x91,0x92,0x93,0x94,0x95,0x96,0x97,0x98,0x99,0x9A,0x9B,0x9C,0x9D,0x9E,0x9F,0x40,0x41,0x42,0x43,0x44,0x45,0x46,0x47,0x48,0x49,0x4A,0x4B,0x4C,0x4D,0x4E,0x4F} or (two[1] == 0xBA and modrm is not None and ((modrm >> 3) & 7) in {4,5,6,7} )) and _widths_allowed("S2-15..16", row):
        return "S2-15..16"
    if (op in {0x50,0x51,0x52,0x53,0x54,0x55,0x56,0x57,0x58,0x59,0x5A,0x5B,0x5C,0x5D,0x5E,0x5F,0x68,0x6A,0xC2,0xC3,0xE8,0xE9,0xEB,0x9C,0x9D} or _group(opcode, modrm, {0xFF,0x8F}, {0,2,4,6})) and mnemonic in {"push","pop","pushf","popf","call","ret","jmp"} and _widths_allowed("S2-17", row):
        return "S2-17"
    if op in {0xA4,0xA5,0xA6,0xA7,0xAA,0xAB,0xAC,0xAD,0xAE,0xAF,0xFC,0xFD} and mnemonic in {"movs","cmps","stos","lods","scas","cld","std","rep movs","rep stos","rep lods","repe cmps","repe scas","repne cmps","repne scas"} and _widths_allowed("S2-18..19", row):
        return "S2-18..19"
    if two == (0x0F,0x0B):
        return "S2-20-fault-only"
    if (op == 0x87 or two in {(0x0F,0xC1),(0x0F,0xB1)} or two == (0x0F,0xAE) and modrm == 0xF0) and mnemonic in {"xchg","xadd","cmpxchg","mfence"} and _widths_allowed("S2-21", row):
        return "S2-21"
    return None


def profile_matcher_maps(doc: dict) -> tuple[dict[tuple[int, ...], list[dict]], dict[tuple[int, ...], dict]]:
    exact = {}
    for profile in doc["profiles"]:
        for accepted in profile.get("accepted_bytes", []):
            key = tuple(int(part, 16) for part in accepted.split())
            exact.setdefault(key, []).append({"task": profile["task"], "evidence_scope": profile["evidence_scope"], "status": profile["status"], "gate": profile.get("gate", "see cited source")})
    excluded = {}
    for profile in doc.get("explicit_profile_exclusions", []):
        for raw in profile.get("excluded_bytes", []):
            excluded[tuple(int(part, 16) for part in raw.split())] = profile
    return exact, excluded


def exact_profile_candidates(row: dict, matchers: dict[tuple[int, ...], list[dict]]) -> list[dict]:
    return matchers.get(tuple(row["byte_values"]), [])


def excluded_profile_match(row: dict, exclusions: dict[tuple[int, ...], dict]) -> dict | None:
    profile = exclusions.get(tuple(row["byte_values"]))
    return profile if profile and row["signature"] == profile.get("signature") else None


def cpl0_encoding_candidate(row: dict) -> str | None:
    """Recognize selected CPL0 opcode candidates without claiming runtime gates."""
    v = opcode_view(row)
    op, two = (v["opcode"][0] if len(v["opcode"]) == 1 else None), v["opcode"]
    if op in {0xED,0xEF} or two in {(0x0F,0x20),(0x0F,0x22),(0x0F,0x30),(0x0F,0x32),(0x0F,0xA2),(0x0F,0x31)}:
        return "composed_cpl0_profile_candidate; control/state/port gates unresolved"
    return None


def cpl0_l7_candidates(row: dict) -> list[str]:
    """Return exact opcode/prefix/operand candidate IDs for bounded S4-38L7 forms."""
    v = opcode_view(row)
    p = v["prefixes"]
    pset = set(p)
    op = v["opcode"][0] if len(v["opcode"]) == 1 else None
    two = v["opcode"]
    modrm = v["modrm"]
    sig = row["signature"]
    memory = "mem" in sig and modrm is not None and (modrm >> 6) != 3
    segment_prefixes = [value for value in p if value in {0x26,0x2E,0x36,0x3E,0x64,0x65}]
    effective_segment = segment_prefixes[-1] if segment_prefixes else None
    segment = effective_segment in {0x64,0x65}
    rex_w_effective = bool(p and 0x40 <= p[-1] <= 0x4F and (p[-1] & 8))
    rex_prefix = lambda value: 0x40 <= value <= 0x4F
    lock_rex_prefix = p == [0xF0] or (len(p) == 2 and p[0] == 0xF0 and rex_prefix(p[1]))
    segment_rex_prefix = len(p) == 1 and p[0] in {0x64, 0x65} or len(p) == 2 and p[0] in {0x64, 0x65} and rex_prefix(p[1])
    optional_segment_nonw_rex = (
        not p
        or len(p) == 1 and (p[0] in {0x64, 0x65} or 0x40 <= p[0] <= 0x47)
        or len(p) == 2 and p[0] in {0x64, 0x65} and 0x40 <= p[1] <= 0x47
    )
    widths = _widths_in_row(row)
    candidates = []
    if two in {(0x0F,0x20),(0x0F,0x22)} and modrm is not None and ((modrm >> 3) & 7) == 4 and (modrm >> 6) == 3:
        candidates.append("S4-38L7b/c-CR4")
    if two == (0x0F,0x22) and modrm is not None and ((modrm >> 3) & 7) == 3 and (modrm >> 6) == 3:
        candidates.append("S4-38L7h-CR3-write")
    if two == (0x0F,0x30): candidates.append("S4-38L7ao-as-WRMSR-index-unobserved")
    if two == (0x0F,0x32): candidates.append("S4-38L7e-RDMSR-EFER")
    if segment and rex_w_effective and memory:
        if op == 0x89: candidates.append("S4-38L7g-FS-GS-qword-store")
        if op == 0x8B: candidates.append("S4-38L7i-FS-GS-qword-load")
        if op == 0x2B: candidates.append("S4-38L7j-FS-GS-qword-SUB")
    if 0xF0 in pset and memory and rex_w_effective and 64 in widths:
        if two == (0x0F,0xB1): candidates.append("S4-38L7k-LOCK-CMPXCHG-qword")
        if two == (0x0F,0xAB): candidates.append("S4-38L7n-LOCK-BTS-qword")
        if two == (0x0F,0xB3): candidates.append("S4-38L7t-LOCK-BTR-qword")
    if 0xF0 in pset and memory and op == 0xFF and modrm is not None:
        group = (modrm >> 3) & 7
        if group == 0 and widths.issubset({32,64}) and widths: candidates.append("S4-38L7l-LOCK-INC-memory")
    if segment and memory and op == 0xFF and modrm is not None:
        group = (modrm >> 3) & 7
        if group == 0 and widths.issubset({32,64}) and widths: candidates.append("S4-38L7p-FS-GS-INC-memory")
        if group == 1 and widths.issubset({32,64}) and widths: candidates.append("S4-38L7q-FS-GS-DEC-memory")
    if segment and memory and op == 0x8B and not rex_w_effective and 32 in widths:
        candidates.append("S4-38L7m-FS-GS-dword-MOV-load")
    if segment and memory and op == 0xF6 and modrm is not None and ((modrm >> 3) & 7) == 0 and 8 in widths:
        candidates.append("S4-38L7o-FS-GS-TEST8")
    if segment and memory and op == 0xF7 and 0x66 in pset and not rex_w_effective and modrm is not None and ((modrm >> 3) & 7) == 0 and 16 in widths:
        candidates.append("S4-38L7r-FS-GS-TEST16")
    if 0xF0 in pset and memory and op in {0x80,0x81,0x83} and modrm is not None and ((modrm >> 3) & 7) == 1 and widths.issubset({8,32,64}):
        candidates.append("S4-38L7s-LOCK-OR-memory")
    if segment and memory and op == 0x8C and widths.issubset({16,32,64}):
        candidates.append("S4-38L7u-FS-GS-segment-store")
    if op in {0xED,0xEF} and widths.intersection({16,32}) and all(value == 0x66 for value in p):
        candidates.append("S4-38L7w-PCI-config-port-word-dword")
    if op in {0xEC,0xEE} and not p:
        candidates.append("S4-38L7x-byte-PIO")
    l7y_prefix = p == [0xF0] or (len(p) == 2 and p[0] == 0xF0 and 0x40 <= p[1] <= 0x4F and not (p[1] & 0x08))
    if l7y_prefix and memory and two == (0x0F,0xB1) and 32 in widths:
        candidates.append("S4-38L7y-LOCK-CMPXCHG-dword")
    if lock_rex_prefix and memory and op in {0x80,0x81,0x83} and modrm is not None and ((modrm >> 3) & 7) == 0 and widths.intersection({8,32,64}):
        candidates.append("S4-38L7z-LOCK-ADD-memory-immediate")
    if segment_rex_prefix and memory and op == 0x39 and widths.intersection({32,64}):
        candidates.append("S4-38L7aa-FS-GS-CMP")
    if optional_segment_nonw_rex and two == (0x0F,0x01) and modrm is not None and ((modrm >> 3) & 7) == 7 and (modrm >> 6) != 3:
        candidates.append("S4-38L7ab-INVLPG-memory")
    l7ac_prefix = p in ([0x64], [0x65]) or (len(p) == 2 and p[0] in {0x64,0x65} and 0x40 <= p[1] <= 0x4F)
    if l7ac_prefix and segment and memory and op == 0xF7 and modrm is not None and ((modrm >> 3) & 7) == 0 and widths.intersection({32,64}) and 16 not in widths:
        candidates.append("S4-38L7ac-FS-GS-TEST32-64")
    if segment_rex_prefix and memory and two == (0x0F,0xB7) and 16 in widths and widths.intersection({32,64}):
        candidates.append("S4-38L7ad-FS-GS-MOVZX-word")
    if segment_rex_prefix and memory and op == 0xC6 and modrm is not None and ((modrm >> 3) & 7) == 0 and 8 in widths:
        candidates.append("S4-38L7ae-FS-GS-MOV-imm8")
    if op == 0xE8 and len(p) == 1 and p[0] in {0x2E,0x3E,0x26,0x36}:
        candidates.append("S4-38L7af-prefixed-CALL-rel32")
    if lock_rex_prefix and memory and op == 0x80 and modrm is not None and ((modrm >> 3) & 7) == 4 and 8 in widths:
        candidates.append("S4-38L7ag-LOCK-AND-imm8")
    if segment_rex_prefix and memory and op == 0x83 and modrm is not None and ((modrm >> 3) & 7) == 7 and widths.intersection({32,64}):
        candidates.append("S4-38L7ah-FS-GS-CMP-imm8")
    if lock_rex_prefix and memory and op == 0x01 and modrm is not None and widths.intersection({32,64}):
        candidates.append("S4-38L7ai-LOCK-ADD-register")
    if segment_rex_prefix and memory and op == 0xC7 and modrm is not None and ((modrm >> 3) & 7) == 0 and widths.intersection({32,64}):
        candidates.append("S4-38L7aj-FS-GS-MOV-immediate")
    if len(p) == 2 and p[0] in {0x64, 0x65} and rex_w_effective and op == 0x63 and memory and 32 in widths and 64 in widths:
        candidates.append("S4-38L7ak-FS-GS-MOVSXD")
    if optional_segment_nonw_rex and two == (0x0F,0x01) and modrm is not None and ((modrm >> 3) & 7) == 0 and memory:
        candidates.append("S4-38L7al-FS-GS-SGDT")
    ltr_rex_prefix = not p or (len(p) == 1 and 0x40 <= p[0] <= 0x47)
    if ltr_rex_prefix and two == (0x0F,0x00) and modrm is not None and ((modrm >> 3) & 7) == 3 and (modrm >> 6) == 3 and 16 in widths:
        candidates.append("S4-38L7am-LTR-register")
    l7an_stale_qword_rex = len(p) == 3 and rex_prefix(p[0]) and p[1] in {0x64, 0x65} and 0x48 <= p[2] <= 0x4F
    if memory and op == 0x89 and widths.intersection({32,64}) and (segment_rex_prefix or (64 in widths and l7an_stale_qword_rex)):
        candidates.append("S4-38L7an-FS-GS-MOV-store")
    return candidates


def cpl0_misc_candidates(row: dict) -> list[str]:
    v = opcode_view(row)
    op = v["opcode"][0] if len(v["opcode"]) == 1 else None
    two = v["opcode"]
    modrm = v["modrm"]
    candidates = []
    if two == (0x0F,0x1F) and modrm is not None and ((modrm >> 3) & 7) == 0 and (modrm >> 6) != 3:
        candidates.append("S4-38L4d-multi-byte-memory-NOP")
    if two == (0x0F,0xA2): candidates.append("S2-45-CPUID")
    if op == 0xFA: candidates.append("S2-45-CLI")
    if op == 0xF4: candidates.append("S2-45-HLT")
    if two == (0x0F,0x31): candidates.append("S2-45-RDTSC")
    if two == (0x0F,0x30): candidates.append("S2-45-WRMSR")
    if two == (0x0F,0x32): candidates.append("S2-45-RDMSR")
    if two in {(0x0F,0x20),(0x0F,0x22)}: candidates.append("S2-45-CR0-CR4-register-form")
    if two == (0x0F,0x22) and modrm is not None and ((modrm >> 3) & 7) == 3: candidates.append("S4-38L7h-CR3-write")
    if two == (0x0F,0x20) and modrm is not None and ((modrm >> 3) & 7) == 2: candidates.append("S4-38L5a-CR2-read")
    if two == (0x0F, 0x01) and modrm is not None and ((modrm >> 3) & 7) in {2,3} and (modrm >> 6) != 3:
        candidates.append("S2-47a/49-LGDT-LIDT")
    if op == 0x8E and modrm is not None and ((modrm >> 6) == 3):
        candidates.append("S4-38L6c-MOV-segment-register")
    return candidates


def scenario_summaries(binaries: list[dict]) -> dict:
    """Aggregate C/D tools and keep E's static ELF separate from generated code."""
    groups = {
        "C": ["bash", "dash", "tar"],
        "D": ["bash", "dash", "gcc-driver", "gcc-cc1", "gcc-collect2", "gcc-as", "gcc-ld-bfd"],
    }
    result = {}
    for scenario, names in groups.items():
        selected = [item for item in binaries if item["name"] in names]
        frequencies: Counter = Counter()
        classes: Counter = Counter()
        for item in selected:
            frequencies.update(item["form_frequencies"])
            classes.update(item["classification_frequencies"])
        result[scenario] = {
            "binaries": names,
            "instruction_count": sum(item["instruction_count"] for item in selected),
            "classification_frequencies": dict(sorted(classes.items())),
            "top_forms": [{"signature": key, "count": count} for key, count in frequencies.most_common(25)],
        }
    e_static = next((item for item in binaries if item["name"] == "stage5-e-static"), None)
    result["E"] = {
        "status": "static_executable_only" if e_static else "static_executable_not_built",
        "binaries": [] if e_static is None else ["stage5-e-static"],
        "static_instruction_count": None if e_static is None else e_static["instruction_count"],
        "static_classification_frequencies": None if e_static is None else e_static["classification_frequencies"],
        "runtime_generated_code": {
            "status": "not_inferred",
            "frequency": "not_inferred",
            "reason": "Static disassembly does not execute stage5_e.c or observe its runtime-generated code bytes.",
        },
    }
    return result


def binary_specs(stage5_e_static_elf: Path | None = None) -> list[tuple[str, str, str]]:
    specs = [(name, image_path, "rootfs") for name, image_path in BINARIES]
    if stage5_e_static_elf is not None:
        specs.append(("stage5-e-static", "external:stage5-e-static.elf", "external"))
    return specs


def parse_meta(line: str) -> tuple[int, dict]:
    fields = line.rstrip("\n").split("\t")
    if len(fields) != 8 or fields[0] != "S5META":
        raise ValueError(f"malformed S5META record: {line.rstrip()!r}")
    _, raw_index, name, image_path, raw_size, sha, machine, build_id = fields
    return int(raw_index), {
        "name": name,
        "image_path": image_path,
        "size_bytes": int(raw_size),
        "sha256": sha,
        "elf_machine": machine,
        "build_id": build_id,
    }


def validate_stage5_e_static_elf(path: Path) -> None:
    try:
        path.relative_to((ROOT / "work/stage5").resolve())
    except ValueError as error:
        raise ValueError("Stage 5 E static ELF input must be under ignored work/stage5") from error
    if digest(path) != STAGE5_E_STATIC_SHA256:
        raise ValueError(f"Stage 5 E static ELF hash differs from paired pinned build: {path}")
    with path.open("rb") as source:
        header = source.read(20)
    if len(header) != 20 or header[:6] != b"\x7fELF\x02\x01" or int.from_bytes(header[18:20], "little") != 62:
        raise ValueError("Stage 5 E static input must be ELF64 little-endian x86-64")


def worker_command(specs: list[tuple[str, str, str]] | None = None) -> str:
    specs = specs or binary_specs()
    lines = [
        "set -eu",
        "dpkg -i /packages/libjansson4_2.14-2build4_arm64.deb /packages/libsframe3_2.46-3ubuntu2_arm64.deb /packages/binutils-common_2.46-3ubuntu2_arm64.deb /packages/binutils-x86-64-linux-gnu_2.46-3ubuntu2_arm64.deb >/dev/null",
        "test \"$(x86_64-linux-gnu-objdump --version | head -n 1)\" = 'GNU objdump (GNU Binutils for Ubuntu) 2.46'",
    ]
    for i, (name, image_path, source_kind) in enumerate(specs):
        extracted = f"/tmp/s5-21-{name}"
        if source_kind == "rootfs":
            lines.append(f"debugfs -R 'dump {image_path} {extracted}' /input.raw >/dev/null 2>&1")
        else:
            lines.append(f"cp /extra/stage5-e-static.elf {extracted}")
        lines += [
            f"printf 'S5META\\t{i}\\t{name}\\t{image_path}\\t%s\\t%s\\t%s\\t%s\\n' \"$(stat -c %s {extracted})\" \"$(sha256sum {extracted} | awk '{{print $1}}')\" \"$(x86_64-linux-gnu-readelf -h {extracted} | awk -F: '/Machine:/ {{gsub(/^ +/, \"\", $2); print $2}}')\" \"$(x86_64-linux-gnu-readelf -n {extracted} | sed -n 's/.*Build ID: //p' | head -n 1)\"",
            f"printf 'S5BEGIN\\t{i}\\n'",
            f"x86_64-linux-gnu-objdump -d --insn-width=16 -M intel {extracted}",
            f"printf 'S5END\\t{i}\\n'",
        ]
    return "\n".join(lines)


def cpl0_exact_probe_command() -> str:
    lines = [
        "set -eu",
        "dpkg -i /packages/libjansson4_2.14-2build4_arm64.deb /packages/libsframe3_2.46-3ubuntu2_arm64.deb /packages/binutils-common_2.46-3ubuntu2_arm64.deb /packages/binutils-x86-64-linux-gnu_2.46-3ubuntu2_arm64.deb >/dev/null",
        "test \"$(x86_64-linux-gnu-objdump --version | head -n 1)\" = 'GNU objdump (GNU Binutils for Ubuntu) 2.46'",
    ]
    for i, (name, image_path) in enumerate(BINARIES):
        extracted = f"/tmp/s5-21-probe-{name}"
        lines += [
            f"debugfs -R 'dump {image_path} {extracted}' /input.raw >/dev/null 2>&1",
            f"x86_64-linux-gnu-objdump -d --insn-width=16 -M intel {extracted} | awk -v name={name} 'tolower($0) ~ /^[[:space:]]*[0-9a-f]+:[[:space:]]+4c 89 17[[:space:]]/ {{store++}} tolower($0) ~ /^[[:space:]]*[0-9a-f]+:[[:space:]]+48 0f 07[[:space:]]/ {{sysret++}} END {{printf \"S5PROBE\\t%s\\t%d\\t%d\\n\", name, store+0, sysret+0}}'",
        ]
    return "\n".join(lines)


def probe_cpl0_exact(image: Path, cache: Path, lock: dict) -> dict:
    verify_cache(cache, lock)
    command = [
        "docker", "run", "--rm", "--platform", "linux/arm64",
        "--mount", f"type=bind,src={image.resolve()},dst=/input.raw,readonly",
        "--mount", f"type=bind,src={cache.resolve()},dst=/packages,readonly",
        BUILDER, "/bin/sh", "-ec", cpl0_exact_probe_command(),
    ]
    result = subprocess.run(command, check=True, capture_output=True, text=True)
    by_binary = {}
    for line in result.stdout.splitlines():
        if not line.startswith("S5PROBE\t"):
            continue
        _, name, store, sysret = line.split("\t")
        by_binary[name] = {"S4-20c-kernel-store-4C8917": int(store), "S4-20c-kernel-SYSRETQ-480F07": int(sysret)}
    if len(by_binary) != len(BINARIES):
        raise RuntimeError("focused CPL0 probe did not report every requested ELF")
    totals = Counter()
    for counts in by_binary.values():
        totals.update(counts)
    return {
        "source_image_sha256": digest(image),
        "source_image_size_bytes": image.stat().st_size,
        "tool": "x86_64-linux-gnu-objdump 2.46-3ubuntu2, static -d -M intel; guest ELFs not executed",
        "profile_scope": "exact encodings from accepted S4-20c CPL0 kernel tail; state/context gates remain conditional and this is not execution evidence",
        "counts_by_binary": by_binary,
        "counts_by_profile_form": dict(sorted(totals.items())),
    }


def analyze(image: Path, cache: Path, lock: dict, stage5_e_static_elf: Path | None = None) -> dict:
    verify_cache(cache, lock)
    specs = binary_specs(stage5_e_static_elf)
    if stage5_e_static_elf is not None:
        validate_stage5_e_static_elf(stage5_e_static_elf)
    registry = json.loads(PROFILE_PATH.read_text())
    exact_matchers, exclusion_matchers = profile_matcher_maps(registry)
    command = [
        "docker", "run", "--rm", "--platform", "linux/arm64",
        "--mount", f"type=bind,src={image.resolve()},dst=/input.raw,readonly",
        "--mount", f"type=bind,src={cache.resolve()},dst=/packages,readonly",
    ]
    if stage5_e_static_elf is not None:
        command += ["--mount", f"type=bind,src={stage5_e_static_elf.resolve()},dst=/extra/stage5-e-static.elf,readonly"]
    command += [
        BUILDER, "/bin/sh", "-ec", worker_command(specs),
    ]
    meta: dict[int, dict] = {}
    per_binary: dict[int, dict] = {}
    forms: dict[str, dict] = {}
    encoding_forms: dict[str, dict] = {}
    mnemonics: Counter = Counter()
    conditional_candidate_unique_rows = 0
    conditional_candidate_supported_overlap_rows = 0
    current = None
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    assert process.stdout is not None
    for line in process.stdout:
        if line.startswith("S5META\t"):
            i, record = parse_meta(line)
            if record["name"] == "stage5-e-static" and record["sha256"] != STAGE5_E_STATIC_SHA256:
                raise RuntimeError("copied Stage 5 E ELF hash differs from the pinned static input")
            meta[i] = record
            per_binary[i] = {"count": 0, "nondecoded_byte_rows": 0, "nondecoded_samples": [], "forms": Counter(), "mnemonics": Counter(), "classifications": Counter(), "profile_matches": Counter(), "conditional_candidates": Counter()}
        elif line.startswith("S5BEGIN\t"):
            current = int(line.split("\t", 1)[1])
        elif line.startswith("S5END\t"):
            current = None
        elif current is not None:
            row = parse_row(line)
            if row is None:
                if ROW.match(line):
                    per_binary[current]["nondecoded_byte_rows"] += 1
                    if len(per_binary[current]["nondecoded_samples"]) < 5:
                        per_binary[current]["nondecoded_samples"].append(line.rstrip())
                continue
            entry = per_binary[current]
            entry["count"] += 1
            entry["forms"][row["signature"]] += 1
            entry["mnemonics"][row["mnemonic"]] += 1
            mnemonics[row["mnemonic"]] += 1
            local_candidate = local_profile_candidate(row)
            exact_candidates = exact_profile_candidates(row, exact_matchers)
            excluded = excluded_profile_match(row, exclusion_matchers)
            cpl0_candidate = cpl0_encoding_candidate(row)
            l7_candidates = cpl0_l7_candidates(row)
            misc_cpl0_candidates = cpl0_misc_candidates(row)
            dimensions = instruction_dimensions(row)
            encoding_key = dimensions["encoding_key"]
            machine_matches = [item for item in exact_candidates if item["evidence_scope"] == "machine_cpl3_profile"]
            local_exact = [item for item in exact_candidates if item["evidence_scope"] == "local_interpreter_profile"]
            composed_exact = [item for item in exact_candidates if item["evidence_scope"] == "composed_cpl0_profile"]
            if excluded:
                classification = "proven_missing"
                evidence_scope = excluded["evidence_scope"]
                reason = excluded["reason"]
            elif machine_matches:
                classification = "proven_supported"
                evidence_scope = "machine_cpl3_profile"
                reason = "exact bytes match an accepted bounded CPL3 machine profile; runtime state gates are not observable in static disassembly"
            elif local_candidate or local_exact:
                classification = "proven_supported"
                evidence_scope = "local_interpreter_profile"
                reason = "exact opcode-family candidate in accepted local profile; this does not prove machine CPL3 entry"
            else:
                classification = "unclassified"
                evidence_scope = None
                reason = "no exact accepted profile match"
            has_conditional_candidate = bool(cpl0_candidate or l7_candidates or misc_cpl0_candidates or composed_exact)
            if has_conditional_candidate:
                conditional_candidate_unique_rows += 1
                if classification == "proven_supported":
                    conditional_candidate_supported_overlap_rows += 1
            form = forms.setdefault(row["signature"], {"count": 0, "binaries": Counter(), "samples": [], "classification_counts": Counter(), "evidence_scopes": Counter(), "profile_matches": Counter(), "conditional_candidates": Counter(), "reasons": Counter()})
            form["count"] += 1
            form["binaries"][meta[current]["name"]] += 1
            form["classification_counts"][classification] += 1
            if evidence_scope:
                form["evidence_scopes"][evidence_scope] += 1
            form["reasons"][reason] += 1
            entry["classifications"][classification] += 1
            if evidence_scope:
                entry["profile_matches"][evidence_scope] += 1
            for match in machine_matches + local_exact:
                form["profile_matches"][match["task"]] += 1
                entry["profile_matches"][match["task"]] += 1
            if local_candidate:
                form["profile_matches"][local_candidate] += 1
                entry["profile_matches"][local_candidate] += 1
            if cpl0_candidate:
                form["conditional_candidates"][cpl0_candidate] += 1
                entry["conditional_candidates"][cpl0_candidate] += 1
            for candidate in l7_candidates + misc_cpl0_candidates:
                form["conditional_candidates"][candidate] += 1
                entry["conditional_candidates"][candidate] += 1
            for match in composed_exact:
                candidate = f"{match['task']} (CPL0 gate conditional)"
                form["conditional_candidates"][candidate] += 1
                entry["conditional_candidates"][candidate] += 1
                form["profile_matches"][match["task"]] += 1
                entry["profile_matches"][match["task"]] += 1
            encoded = encoding_forms.setdefault(encoding_key, {"dimensions": dimensions, "count": 0, "binaries": Counter(), "classification_counts": Counter(), "evidence_scopes": Counter(), "profile_matches": Counter(), "conditional_candidates": Counter(), "samples": []})
            encoded["count"] += 1
            encoded["binaries"][meta[current]["name"]] += 1
            encoded["classification_counts"][classification] += 1
            if evidence_scope:
                encoded["evidence_scopes"][evidence_scope] += 1
            for match in machine_matches + local_exact + composed_exact:
                encoded["profile_matches"][match["task"]] += 1
            if local_candidate:
                encoded["profile_matches"][local_candidate] += 1
            if cpl0_candidate:
                encoded["conditional_candidates"][cpl0_candidate] += 1
            for candidate in l7_candidates + misc_cpl0_candidates:
                encoded["conditional_candidates"][candidate] += 1
            for match in composed_exact:
                encoded["conditional_candidates"][f"{match['task']} (CPL0 gate conditional)"] += 1
            if len(encoded["samples"]) < 3:
                encoded["samples"].append({"binary": meta[current]["name"], "address": row["address"], "bytes": row["bytes"], "assembly": row["mnemonic"] + (" " + row["operands"] if row["operands"] else "")})
            if len(form["samples"]) < 4:
                form["samples"].append({"binary": meta[current]["name"], "address": row["address"], "bytes": row["bytes"], "assembly": row["mnemonic"] + (" " + row["operands"] if row["operands"] else "")})
    rc = process.wait()
    if rc: raise RuntimeError(f"pinned disassembly container exited {rc}")
    if len(meta) != len(specs): raise RuntimeError("container did not report every requested ELF")
    all_forms = []
    for signature, data in sorted(forms.items(), key=lambda p: (-p[1]["count"], p[0])):
        counts = dict(data["classification_counts"])
        if len(counts) == 1:
            summary_class = next(iter(counts))
        else:
            summary_class = "mixed"
        all_forms.append({"signature": signature, "count": data["count"], "binary_frequencies": dict(sorted(data["binaries"].items())), "classification": summary_class, "classification_frequencies": counts, "evidence_scope_frequencies": dict(sorted(data["evidence_scopes"].items())), "profile_match_frequencies": dict(sorted(data["profile_matches"].items())), "conditional_candidate_frequencies": dict(sorted(data["conditional_candidates"].items())), "classification_reasons": dict(data["reasons"]), "sample_raw_rows": data["samples"]})
    all_encoding_forms = []
    for key, data in sorted(encoding_forms.items(), key=lambda pair: (-pair[1]["count"], pair[0])):
        all_encoding_forms.append({
            "encoding_key": key,
            "dimensions": data["dimensions"],
            "count": data["count"],
            "binary_frequencies": dict(sorted(data["binaries"].items())),
            "classification_frequencies": dict(sorted(data["classification_counts"].items())),
            "evidence_scope_frequencies": dict(sorted(data["evidence_scopes"].items())),
            "profile_match_frequencies": dict(sorted(data["profile_matches"].items())),
            "conditional_candidate_frequencies": dict(sorted(data["conditional_candidates"].items())),
            "sample_raw_rows": data["samples"],
        })
    binaries = []
    for i in range(len(specs)):
        item = meta[i]
        item["instruction_count"] = per_binary[i]["count"]
        item["nondecoded_byte_rows"] = per_binary[i]["nondecoded_byte_rows"]
        item["nondecoded_byte_row_samples"] = per_binary[i]["nondecoded_samples"]
        item["unique_mnemonics"] = len(per_binary[i]["mnemonics"])
        item["unique_form_signatures"] = len(per_binary[i]["forms"])
        item["mnemonic_frequencies"] = dict(sorted(per_binary[i]["mnemonics"].items(), key=lambda p: (-p[1], p[0])))
        item["form_frequencies"] = dict(sorted(per_binary[i]["forms"].items(), key=lambda p: (-p[1], p[0])))
        item["classification_frequencies"] = dict(sorted(per_binary[i]["classifications"].items()))
        item["profile_match_frequencies"] = dict(sorted(per_binary[i]["profile_matches"].items()))
        item["conditional_candidate_frequencies"] = dict(sorted(per_binary[i]["conditional_candidates"].items()))
        binaries.append(item)
    count = sum(item["instruction_count"] for item in binaries)
    classification_totals = Counter()
    evidence_scope_totals = Counter()
    conditional_totals = Counter()
    for data in forms.values():
        classification_totals.update(data["classification_counts"])
        evidence_scope_totals.update(data["evidence_scopes"])
        conditional_totals.update(data["conditional_candidates"])
    return {
        "schema_version": 2,
        "analysis": "GNU objdump disassembles each target executable statically; this analyzer does not execute any target ELF.",
        "source_image": {"sha256": digest(image), "size_bytes": image.stat().st_size, "filesystem": "ext4", "root_device": "/dev/nvme0n1"},
        "external_inputs": [] if stage5_e_static_elf is None else [{"name": "stage5-e-static", "path": stage5_e_static_elf.relative_to(ROOT).as_posix(), "sha256": digest(stage5_e_static_elf), "source_c_path": "tests/fixtures/guest/stage5/stage5_e.c", "source_c_sha256": STAGE5_E_SOURCE_SHA256, "build": "pinned Stage 5 guest rootfs GCC 15.2.0-16ubuntu1; ELF is statically disassembled only, not executed"}],
        "disassembler": {"binary": "x86_64-linux-gnu-objdump", "version": lock["tool_version"], "host": "linux/arm64", "builder_image": BUILDER, "command": "x86_64-linux-gnu-objdump -d --insn-width=16 -M intel <debugfs-dumped-ELF-or-mounted-external-ELF>"},
        "profile_comparison": {"accepted_profile_file": str(PROFILE_PATH.relative_to(ROOT)), "registry_complete": registry["registry_complete"], "policy": registry["classification_policy"], "scopes": ["local_interpreter_profile", "composed_cpl0_profile", "machine_cpl3_profile"], "scope_labels_are_nonexclusive": True, "conditional_candidates_can_overlap_supported_rows": True, "limitation": registry["known_limit"]},
        "conditional_candidates": {"unique_instruction_rows": conditional_candidate_unique_rows, "supported_overlap_rows": conditional_candidate_supported_overlap_rows, "hit_count": sum(conditional_totals.values()), "candidate_labels_are_nonexclusive": True},
        "binaries": binaries,
        "scenario_inventory": scenario_summaries(binaries),
        "inventory": {"instruction_count": count, "unique_mnemonics": len(mnemonics), "unique_form_signatures": len(forms), "unique_encoding_forms": len(encoding_forms), "classification_frequencies": dict(sorted(classification_totals.items())), "evidence_scope_frequencies": dict(sorted(evidence_scope_totals.items())), "conditional_cpl0_candidate_frequencies": dict(sorted(conditional_totals.items())), "mnemonic_frequencies": dict(sorted(mnemonics.items(), key=lambda p: (-p[1], p[0]))), "forms": all_forms, "encoding_forms": all_encoding_forms},
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--image", type=Path, required=True)
    ap.add_argument("--tool-cache", type=Path, default=ROOT / "work/stage5/isa-tool-cache")
    ap.add_argument("--output", type=Path)
    ap.add_argument("--stage5-e-static-elf", type=Path, help="include the pinned, separately built Stage 5 E static ELF; never executes it")
    ap.add_argument("--probe-cpl0-exact", action="store_true", help="Count only exact S4-20c CPL0 kernel-tail byte rows; do not run the full inventory")
    args = ap.parse_args()
    image, cache = args.image.resolve(), args.tool_cache.resolve()
    if not image.is_file() or image.stat().st_size != IMAGE_SIZE or digest(image) != IMAGE_SHA256:
        raise ValueError(f"image must match the accepted S5-05 SHA256 {IMAGE_SHA256} and size {IMAGE_SIZE}")
    lock = json.loads(LOCK_PATH.read_text())
    prepare_cache(cache, lock)
    if args.probe_cpl0_exact:
        probe = probe_cpl0_exact(image, cache, lock)
        print(json.dumps(probe, indent=2, sort_keys=True))
        return 0
    if args.output is None:
        raise ValueError("--output is required for the full inventory")
    output = args.output.resolve()
    if output.exists(): raise ValueError(f"refusing to overwrite {output}")
    report = analyze(image, cache, lock, args.stage5_e_static_elf.resolve() if args.stage5_e_static_elf else None)
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("x") as stream: json.dump(report, stream, indent=2, sort_keys=True); stream.write("\n")
    inv = report["inventory"]
    print(f"image_sha256={report['source_image']['sha256']}")
    print(f"binaries={len(report['binaries'])} instructions={inv['instruction_count']} unique_mnemonics={inv['unique_mnemonics']} unique_forms={inv['unique_form_signatures']}")
    print(f"classification_frequencies={inv['classification_frequencies']} conditional_cpl0_candidates={sum(inv['conditional_cpl0_candidate_frequencies'].values())}")
    print(f"output={output}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.CalledProcessError, ValueError, KeyError) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
