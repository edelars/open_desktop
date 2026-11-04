"""Strict verifier for the literal S2-42q MOV addressing controls."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import re
from typing import Any

from stage2_corpus_support import CorpusError


ROOT = Path(__file__).resolve().parents[2]
CONTROL_DIR = Path("tests/fixtures/x86/stage2/s2_42q_mov_hand")
MANIFEST_PATH = Path("tests/fixtures/x86/stage2/s2_42q_mov_hand_controls.manifest.json")
PROFILE = "s2-long-integer-v1"
MANIFEST_VERSION = 1
CONTROL_COUNT = 56
WIDTHS = (("08", 8), ("16", 16), ("32", 32), ("64", 64))
DIRECTIONS = ("load", "store")
FORMS = ("base", "disp8", "disp32", "sib", "rip_relative", "address32", "rex_b")
CODE_LOAD_ADDRESS = 0x100000
CODE_STORE_ADDRESS = 0x300000
DATA_ADDRESS = 0x200000
DATA_BYTES = bytes.fromhex("1032547698badcfe0f2d4b6987a5c3e1") * 8
BASE_GPRS = (0x8877665544332211, 0xAABBCCDDEEFF0011, 0xDEADBEEFFEDCBA98, DATA_ADDRESS + 16,
             0x300080, 0x300080, 0x200040, 0x200050, 0x0123456789ABCDEF,
             0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
             0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF)

# The digest pins the canonical ordered identities and both immutable byte files
# for every control. Rehashing a modified fixture and manifest cannot change it.
CONTROL_SET_DIGEST = "6f1d9c2d1dcbd55b1ec05acf6ffd5aa41a1685385b1aa6672e8cb1a8ad60f1d2"


def _strict_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise CorpusError(f"duplicate JSON field: {key}")
        value[key] = item
    return value


def _read_json(path: Path) -> tuple[dict[str, Any], bytes]:
    raw = path.read_bytes()
    value = json.loads(raw, object_pairs_hook=_strict_object)
    if not isinstance(value, dict):
        raise CorpusError(f"JSON document must be an object: {path}")
    return value, raw


def _sha256(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def canonical_control_digest(items: list[dict[str, Any]]) -> str:
    canonical = "\n".join(json.dumps(item, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
                           for item in items)
    return _sha256(canonical.encode())


def _hx(value: int) -> str:
    return f"0x{value & 0xFFFFFFFFFFFFFFFF:016x}"


def _validate_control_contract(item: dict[str, Any], fixture: dict[str, Any],
                               expected: dict[str, Any]) -> None:
    width, direction, form = item["width"], item["direction"], item["form"]
    count = width // 8
    code_address = CODE_LOAD_ADDRESS if direction == "load" else CODE_STORE_ADDRESS
    offset = {"base": 0, "disp8": 1, "disp32": 128-count, "sib": 7,
              "rip_relative": 31, "address32": 128-count, "rex_b": 63}[form]
    displacement = {"base": 0, "disp8": 127 if direction == "load" else -128,
                    "disp32": -65536 if direction == "load" else 65536,
                    "sib": 17 if direction == "load" else -17,
                    "address32": 5 if direction == "load" else -5}.get(form, 0)
    if item["effective_address"] != _hx(DATA_ADDRESS + offset):
        raise CorpusError(f"S2-42q fixed effective address differs: {item['case_id']}")
    initial = fixture.get("initial_state")
    regions = fixture.get("memory")
    if not isinstance(initial, dict) or not isinstance(regions, list) or len(regions) != 2:
        raise CorpusError(f"S2-42q full fixture state is missing: {item['case_id']}")
    if (initial.get("mode") != "long64" or initial.get("rip") != _hx(code_address)
            or initial.get("rflags") != _hx(0x8D7) or fixture.get("rflags_mask") != _hx(0x0CD7)
            or fixture.get("instruction_budget") != 1 or fixture.get("expected_exit") != "stop_address"):
        raise CorpusError(f"S2-42q execution bounds or initial state differ: {item['case_id']}")
    gpr_hex = initial.get("gpr")
    if not isinstance(gpr_hex, list) or len(gpr_hex) != 16:
        raise CorpusError(f"S2-42q full GPR input is missing: {item['case_id']}")
    gpr = [int(value, 16) for value in gpr_hex]
    wanted_gpr = list(BASE_GPRS)
    if form in ("base", "disp8", "disp32", "sib", "address32"):
        wanted_gpr[3] = ({"base": DATA_ADDRESS, "disp8": DATA_ADDRESS + offset - displacement,
                          "disp32": DATA_ADDRESS + offset - displacement,
                          "sib": DATA_ADDRESS + offset - 12 - displacement,
                          "address32": 0xFFFFFFFF00000000 | ((DATA_ADDRESS + offset - 3 - displacement) & 0xFFFFFFFF)})[form]
    if form in ("sib", "address32"):
        wanted_gpr[1] = 3
    if form == "rex_b":
        wanted_gpr[8] = DATA_ADDRESS + offset
    if gpr != wanted_gpr:
        raise CorpusError(f"S2-42q fixed or effective-address GPR inputs differ: {item['case_id']}")

    prefix = bytearray()
    if form == "address32":
        prefix.append(0x67)
    if width == 16:
        prefix.append(0x66)
    if width == 64:
        prefix.append(0x48)
    if form == "rex_b":
        if width == 64:
            prefix[-1] |= 0x01
        else:
            prefix.append(0x41)
    opcode = 0x8A if direction == "load" and width == 8 else \
             0x8B if direction == "load" else 0x88 if width == 8 else 0x89
    suffix = {
        "base": bytes((0x03,)),
        "disp8": bytes((0x43, displacement & 0xFF)),
        "disp32": bytes((0x83,)) + (displacement & 0xFFFFFFFF).to_bytes(4, "little"),
        "sib": bytes((0x44, 0x8B, displacement & 0xFF)),
        "address32": bytes((0x44, 0x0B, displacement & 0xFF)),
        "rex_b": bytes((0x00,)),
    }.get(form)
    if form == "rip_relative":
        instruction_length = len(prefix) + 6
        rip_displacement = DATA_ADDRESS + offset - (code_address + instruction_length)
        suffix = bytes((0x05,)) + (rip_displacement & 0xFFFFFFFF).to_bytes(4, "little")
    if suffix is None:
        raise CorpusError(f"unsupported S2-42q addressing form: {item['case_id']}")
    instruction = bytes(prefix) + bytes((opcode,)) + suffix
    code_region, data_region = regions
    if (code_region.get("region_id") != "code" or code_region.get("guest_address") != _hx(code_address)
            or code_region.get("length") != len(instruction)
            or code_region.get("permissions") != {"read": False, "write": False, "execute": True}
            or code_region.get("initial_bytes_hex") != instruction.hex()
            or code_region.get("comparison_mask_hex") != "ff" * len(instruction)
            or data_region.get("region_id") != "data" or data_region.get("guest_address") != _hx(DATA_ADDRESS)
            or data_region.get("length") != 128
            or data_region.get("permissions") != {"read": True, "write": True, "execute": False}
            or data_region.get("initial_bytes_hex") != DATA_BYTES.hex()
            or data_region.get("comparison_mask_hex") != "ff" * 128
            or fixture.get("stop_rip") != _hx(code_address + len(instruction))):
        raise CorpusError(f"S2-42q mapping, permission, mask, or encoding differs: {item['case_id']}")
    if form == "base":
        effective = gpr[3]
    elif form in ("disp8", "disp32"):
        effective = gpr[3] + displacement
    elif form == "sib":
        effective = gpr[3] + gpr[1] * 4 + displacement
    elif form == "rip_relative":
        effective = code_address + len(instruction) + int.from_bytes(instruction[-4:], "little", signed=True)
    elif form == "address32":
        effective = (gpr[3] + gpr[1] + displacement) & 0xFFFFFFFF
    else:
        effective = gpr[8]
    if effective != DATA_ADDRESS + offset or offset + count > 128:
        raise CorpusError(f"S2-42q effective address is invalid or outside mapping: {item['case_id']}")
    if (expected.get("case_id") != item["case_id"] or expected.get("state", {}).get("rip") != fixture.get("stop_rip")
            or expected.get("state", {}).get("rflags") != _hx(0x8D7)
            or expected.get("state", {}).get("rflags_defined") != _hx(0x0CD7)
            or expected.get("instructions_executed") != {"status": "available", "value": _hx(1)}
            or expected.get("outcome") != {"kind": "stop_address"}):
        raise CorpusError(f"S2-42q literal expected snapshot identity or retirement differs: {item['case_id']}")


def expected_ids() -> tuple[str, ...]:
    return tuple(
        f"s2_42q_mov_{width}_{direction}_{form}"
        for width, _ in WIDTHS
        for direction in DIRECTIONS
        for form in FORMS
    )


def verify_controls() -> tuple[list[dict[str, Any]], dict[str, dict[str, Any]],
                               dict[str, dict[str, Any]], dict[str, Any], str]:
    """Return ordered (fixture, expected) controls after strict byte verification."""
    root = ROOT / CONTROL_DIR
    manifest_path = ROOT / MANIFEST_PATH
    if root.is_symlink() or not root.is_dir() or manifest_path.is_symlink() or not manifest_path.is_file():
        raise CorpusError("S2-42q control directory and manifest must be regular repository inputs")
    manifest, manifest_raw = _read_json(manifest_path)
    required = {"manifest_version", "profile", "control_count", "controls", "control_set_digest"}
    if set(manifest) != required:
        raise CorpusError("S2-42q control manifest has missing or extra fields")
    controls = manifest["controls"]
    if (type(manifest["manifest_version"]) is not int or manifest["manifest_version"] != MANIFEST_VERSION
            or manifest["profile"] != PROFILE or type(manifest["control_count"]) is not int
            or manifest["control_count"] != CONTROL_COUNT or not isinstance(controls, list)
            or len(controls) != CONTROL_COUNT):
        raise CorpusError("S2-42q control manifest has an unsupported version or shape")
    ids = expected_ids()
    actual_ids = tuple(item.get("case_id") if isinstance(item, dict) else None for item in controls)
    if actual_ids != ids:
        raise CorpusError("S2-42q controls are missing, extra, duplicated, or out of order")
    expected_fields = {"case_id", "source_id", "fixture_path", "fixture_sha256", "expected_path",
                       "expected_sha256", "width", "direction", "form", "effective_address", "boundary_tags"}
    fixtures_by_id: dict[str, dict[str, Any]] = {}
    expected_by_id: dict[str, dict[str, Any]] = {}
    expected_names: set[str] = set()
    for item in controls:
        if not isinstance(item, dict) or set(item) != expected_fields:
            raise CorpusError("S2-42q control entry has missing or extra fields")
        case_id = item["case_id"]
        match = re.fullmatch(r"s2_42q_mov_(08|16|32|64)_(load|store)_([a-z0-9_]+)", case_id)
        if match is None:
            raise CorpusError(f"invalid S2-42q control identity: {case_id!r}")
        width, direction, form = match.groups()
        if form not in FORMS:
            raise CorpusError(f"unsupported S2-42q control form: {case_id}")
        fixture_name = f"{case_id}.case.json"
        expected_name = f"{case_id}.expected.json"
        fixture_rel = f"{CONTROL_DIR.as_posix()}/{fixture_name}"
        expected_rel = f"{CONTROL_DIR.as_posix()}/{expected_name}"
        if (item["fixture_path"] != fixture_rel or item["expected_path"] != expected_rel
                or item["source_id"] != f"s2-42q:{case_id}"
                or item["width"] != int(width) or item["direction"] != direction or item["form"] != form
                or not isinstance(item["effective_address"], str)
                or not re.fullmatch(r"0x[0-9a-f]{16}", item["effective_address"])
                or not isinstance(item["boundary_tags"], list)
                or any(not isinstance(tag, str) for tag in item["boundary_tags"])):
            raise CorpusError(f"S2-42q control metadata differs from its fixed identity: {case_id}")
        for digest in (item["fixture_sha256"], item["expected_sha256"]):
            if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
                raise CorpusError(f"invalid S2-42q byte digest: {case_id}")
        fixture_path = ROOT / fixture_rel
        expected_path = ROOT / expected_rel
        for path in (fixture_path, expected_path):
            if path.is_symlink() or not path.is_file() or ROOT not in path.resolve().parents:
                raise CorpusError(f"S2-42q control path is missing, unsafe, or not a regular file: {path}")
        fixture, fixture_raw = _read_json(fixture_path)
        expected, expected_raw = _read_json(expected_path)
        if _sha256(fixture_raw) != item["fixture_sha256"] or _sha256(expected_raw) != item["expected_sha256"]:
            raise CorpusError(f"S2-42q control fixture or expected digest mismatch: {case_id}")
        if (fixture.get("schema_version") != 2 or fixture.get("case_id") != case_id
                or fixture.get("profile") != PROFILE or expected.get("case_id") != case_id):
            raise CorpusError(f"S2-42q fixture or expected identity mismatch: {case_id}")
        _validate_control_contract(item, fixture, expected)
        expected_names.update((fixture_name, expected_name))
        fixtures_by_id[case_id] = fixture
        expected_by_id[case_id] = expected
    entries = list(root.iterdir())
    if any(path.is_symlink() for path in entries) or {path.name for path in entries} != expected_names:
        raise CorpusError("S2-42q control directory has missing, extra, or symlink entries")
    digest = canonical_control_digest(controls)
    if manifest["control_set_digest"] != digest or digest != CONTROL_SET_DIGEST:
        raise CorpusError("S2-42q pinned control-set digest mismatch")
    return controls, fixtures_by_id, expected_by_id, manifest, _sha256(manifest_raw)


def expected_result_by_id() -> dict[str, dict[str, Any]]:
    _, _, expected, _, _ = verify_controls()
    return expected
