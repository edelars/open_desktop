"""Hardware-free checks for the S4-34 system KVM evidence validator."""

from __future__ import annotations

import struct
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import build_stage4_kvm_system as m


def runner(case: str, words: list[int], events: int | None = None, **extra):
    spec = m.CASES[case]
    block = bytearray(m.RESULT_SIZE)
    struct.pack_into("<QIIIIII", block, 0, m.MAGIC, 1, spec["id"], m.COMPLETE,
                     len(words), spec["events"] if events is None else events, 0)
    if words:
        struct.pack_into(f"<{len(words)}Q", block, 32, *words)
    cpuid = [
        {"function": 0, "index": 0, "flags": 0, "eax": 1, "ebx": 0x756E6547, "ecx": 0x6C65746E, "edx": 0x49656E69},
        {"function": 1, "index": 0, "flags": 0, "eax": 1, "ebx": 0, "ecx": 0, "edx": 1},
    ]
    final = {"gpr": {"rsp": 0x17000, "r15": 0x18000},
             "segments": {"cs": {"selector": 8, "l": 1, "db": 0, "dpl": 0}, "ss": {"selector": 0x10}},
             "tables": {"gdtr_base": 0x5000, "idtr_base": 0x9000},
             "control": {"cr0": 0x80000033, "cr2": 0, "cr3": 0x2000, "cr4": 0x20, "efer": 0x500},
             "rip": 0x1234, "rflags": 2}
    value = {"exit": "hlt", "stop_reason": "guest_hlt", "error": None,
             "final_state": final, "entry": dict(m.base.EXPECTED_ENTRY),
             "result_block_hex": block.hex(), "kvm_api": 12,
             "exits": [{"kind": "hlt"}, {"kind": "hlt"}] if case == "kvm_injected_interrupt" else [{"kind": "hlt"}],
             "exits_dropped": 0, "uart_hex": "", "uart_text": "",
             "guest_cpuid_model": cpuid, "hostname": "mini", "uname": "Linux",
             "cpu_model": "Intel", "host_microcode": "0x21", "vmx": True,
             "injection": None}
    value.update(extra)
    return value


class ResultBlockTests(unittest.TestCase):
    def test_round_trip_and_event_count(self):
        parsed = m.parse_result_block(runner("control", [0] * 13)["result_block_hex"], "control")
        self.assertEqual(parsed["case_id"], 201)
        self.assertEqual(parsed["word_count"], 13)
        self.assertEqual(parsed["observations"], [0] * 13)

    def test_rejects_wrong_case_and_incomplete_marker(self):
        for field_offset, bad in ((12, 0x22), (16, 0)):
            raw = bytearray.fromhex(runner("control", [0] * 13)["result_block_hex"])
            struct.pack_into("<I", raw, field_offset, bad)
            with self.assertRaises(m.BuildError):
                m.parse_result_block(raw.hex(), "control")

    def test_fault_fields_are_checked(self):
        words = [0, 0x8000000000, 0x1234, 1, 1, 1, 0, 0x11223344]
        accepted = m.validate_case("paging_103", runner("paging_103", words), {"s4_34c_fault_not_present": 0x1234})
        self.assertEqual(accepted["status"], "accepted")
        words[0] = 1
        with self.assertRaises(m.BuildError):
            m.validate_case("paging_103", runner("paging_103", words), {"s4_34c_fault_not_present": 0x1234})

    def test_nx_observations_use_fault_payload_slots(self):
        words = [1, 1 << 20, 0x11, 0x8000000000, 0x8000000000,
                 1, 1, 1,
                 0x14003 | (1 << 63), 0x500]
        self.assertEqual(m.validate_case("paging_106", runner("paging_106", words))["status"], "accepted")
        words[2] = 1
        with self.assertRaises(m.BuildError):
            m.validate_case("paging_106", runner("paging_106", words))
        words[2] = 0x11
        words[8] = 1 << 63
        with self.assertRaises(m.BuildError):
            m.validate_case("paging_106", runner("paging_106", words))

    def test_tables_roundtrip_extracts_split_tss_fields(self):
        words = [0x37, 0x9000, 0x40F, 0xC000, 0x28] + [0] * 20
        words[6:10] = [0x00AF9B000000FFFF, 0x00CF93000000FFFF,
                       0x00AFFB000000FFFF, 0x00CFF3000000FFFF]
        words[10] = 0x00008900A0000067 | (1 << 41)
        words[12] = 0xB000 << 32
        words[24] = 104 << 48
        self.assertEqual(m.validate_case("tables_roundtrip", runner("tables_roundtrip", words))["status"], "accepted")
        words[12] = 0
        with self.assertRaises(m.BuildError):
            m.validate_case("tables_roundtrip", runner("tables_roundtrip", words))

    def test_same_cpl_iret_frame_restores_stack(self):
        words = [0x2, 0x17000, 0x10, 0x1234, 0x8, 0x202, 0x17000, 0x10, 2, 0x17000, 0x10, 1]
        self.assertEqual(m.validate_case("int_iret_cpl0", runner("int_iret_cpl0", words))["status"], "accepted")
        words[6] += 8
        with self.assertRaises(m.BuildError):
            m.validate_case("int_iret_cpl0", runner("int_iret_cpl0", words))

    def test_interrupt_requires_runner_delivery_evidence(self):
        words = [0x41, 0x1000, 0x8, 0x202, 2, 0x48414E444C455231, 0x53434B504F535431]
        with self.assertRaises(m.BuildError):
            m.validate_case("kvm_injected_interrupt", runner("kvm_injected_interrupt", words))
        valid = runner("kvm_injected_interrupt", words, injection={
            "vector": 0x41, "delivery_observed": True, "attempts": 1, "ioctl_result": 0,
            "event_count_before": 0, "event_count_after": 1,
            "pre_state": {"rip": 0x1000, "rflags": 0x202, "cr0": 0x80000033,
                          "cr3": 0x2000, "cr4": 0x20, "efer": 0x500,
                          "cs_selector": 8, "cs_l": 1, "cs_db": 0, "cs_dpl": 0, "ss_selector": 0x10},
        })
        self.assertEqual(m.validate_case("kvm_injected_interrupt", valid, {"after_first_hlt": 0x1000})["status"], "accepted")

    def test_cpl3_code_and_stack_selectors_are_distinct(self):
        words = [0x1234, 0x1B, 0x202, 0x16000, 0x23, 0x1250, 1]
        self.assertEqual(m.validate_case("cpl3_roundtrip", runner("cpl3_roundtrip", words), {"cpl3_roundtrip_after_int": 0x1234})["status"], "accepted")
        words[1] = 0x23
        with self.assertRaises(m.BuildError):
            m.validate_case("cpl3_roundtrip", runner("cpl3_roundtrip", words))

    def test_privilege_gp_return_rip_is_after_int_instruction(self):
        words = [0x80000033, 0, 0x1234, 0x1B, 0x202, 0x80000033, 0x5678, 1]
        symbols = {"cpl3_privileged_user": 0x1234, "cpl3_privileged_after_int": 0x5678}
        self.assertEqual(m.validate_case("cpl3_privileged_gp", runner("cpl3_privileged_gp", words), symbols)["status"], "accepted")
        symbols["cpl3_privileged_after_int"] = 0x5676
        with self.assertRaises(m.BuildError):
            m.validate_case("cpl3_privileged_gp", runner("cpl3_privileged_gp", words), symbols)


if __name__ == "__main__":
    unittest.main()
