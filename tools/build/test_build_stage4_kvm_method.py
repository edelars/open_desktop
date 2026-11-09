#!/usr/bin/env python3
"""Hardware-free tests for the S4-34a KVM method builder."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import build_stage4_kvm_method as m  # noqa: E402

SHA = "a" * 64


GOOD_EXITS = [
    {"kind": "io", "port": "0x3f8", "size": 1, "dir": "out", "data": "0x4f"},
    {"kind": "io", "port": "0x3f8", "size": 1, "dir": "out", "data": "0x4b"},
    {"kind": "io", "port": "0x3f8", "size": 1, "dir": "out", "data": "0xa"},
    {"kind": "hlt"},
]


def good_result(**overrides):
    result = {
        "exit": "hlt",
        "uart_hex": "4f4b0a",
        "uart_text": "OK\n",
        "exits": [dict(e) for e in GOOD_EXITS],
        "exits_dropped": 0,
        "entry": {
            "rip": 0x1000,
            "rsp": 0x8000,
            "cr0": 0x80000033,
            "cr3": 0x2000,
            "cr4": 0x20,
            "efer": 0x500,
            "cs_selector": 8,
            "cs_l": 1,
            "cs_db": 0,
            "cs_dpl": 0,
            "ss_selector": 0x10,
        },
        "kvm_api": 12,
        "run_count": 4,
        "intr_count": 0,
        "hostname": "h",
        "uname": "u",
        "cpu_model": "c",
        "vmx": True,
    }
    result.update(overrides)
    return result


def classify(name="main", sha=SHA, **overrides):
    return m.classify_case(name, good_result(**overrides), local_image_sha=SHA, remote_image_sha=sha)


class EvaluationTests(unittest.TestCase):
    def test_main_accepted(self):
        verdict = classify()
        self.assertEqual(verdict["status"], "accepted")
        self.assertTrue(all(c["ok"] for c in verdict["checks"]))

    def test_each_check_rejects(self):
        cases = {
            "exit": {"exit": "budget"},
            "uart_text": {"uart_text": "NO\n"},
            "api": {"kvm_api": 11},
            "rip": {"entry": dict(good_result()["entry"], rip=0x2000)},
            "cr3": {"entry": dict(good_result()["entry"], cr3=0x3000)},
            "cr4": {"entry": dict(good_result()["entry"], cr4=0x0)},
            "efer": {"entry": dict(good_result()["entry"], efer=0x100)},
            "cs": {"entry": dict(good_result()["entry"], cs_selector=0x10)},
            "cr0_no_pe_pg": {"entry": dict(good_result()["entry"], cr0=0x32)},
            "cr0_no_pg": {"entry": dict(good_result()["entry"], cr0=0x33)},
            "cr0_extra_wp": {"entry": dict(good_result()["entry"], cr0=0x80010033)},
            "cs_l0": {"entry": dict(good_result()["entry"], cs_l=0)},
            "cs_db1": {"entry": dict(good_result()["entry"], cs_db=1)},
            "cs_dpl3": {"entry": dict(good_result()["entry"], cs_dpl=3)},
            "rsp": {"entry": dict(good_result()["entry"], rsp=0x7000)},
            "ss": {"entry": dict(good_result()["entry"], ss_selector=0x0)},
            "missing_cs_l": {"entry": {k: v for k, v in good_result()["entry"].items() if k != "cs_l"}},
            "exits_other_port": {"exits": [dict(GOOD_EXITS[0], port="0x3f9")] + GOOD_EXITS[1:]},
            "exits_wrong_data": {"exits": GOOD_EXITS[:2] + [dict(GOOD_EXITS[2], data="0x0d")] + GOOD_EXITS[3:]},
            "exits_wrong_size": {"exits": [dict(GOOD_EXITS[0], size=2)] + GOOD_EXITS[1:]},
            "exits_in_dir": {"exits": [dict(GOOD_EXITS[0], dir="in")] + GOOD_EXITS[1:]},
            "exits_extra": {"exits": GOOD_EXITS[:1] + [GOOD_EXITS[0]] + GOOD_EXITS[1:]},
            "exits_missing": {"exits": GOOD_EXITS[:2] + GOOD_EXITS[3:]},
            "exits_no_hlt": {"exits": GOOD_EXITS[:3]},
            "exits_empty": {"exits": []},
            "exits_none": {"exits": None},
            "exits_dropped": {"exits_dropped": 1},
        }
        for label, overrides in cases.items():
            with self.subTest(label):
                self.assertEqual(classify(**overrides)["status"], "rejected")

    def test_exit_data_zero_padding_is_equivalent(self):
        exits = GOOD_EXITS[:2] + [dict(GOOD_EXITS[2], data="0x0a")] + GOOD_EXITS[3:]
        self.assertEqual(classify(exits=exits)["status"], "accepted")

    def test_sha_mismatch_rejects(self):
        self.assertEqual(classify(sha="b" * 64)["status"], "rejected")

    def test_missing_entry_rejects(self):
        self.assertEqual(classify(entry=None)["status"], "rejected")

    def test_bool_is_not_a_number(self):
        entry = dict(good_result()["entry"], rip=True)
        self.assertEqual(classify(entry=entry)["status"], "rejected")

    def test_no_hlt_injection_distinguished(self):
        verdict = classify("no_hlt", exit="budget")
        self.assertEqual(verdict["status"], "rejected")
        self.assertTrue(verdict["distinguished"])

    def test_no_hlt_injection_not_distinguished_when_it_halts(self):
        verdict = classify("no_hlt", exit="hlt")
        self.assertFalse(verdict["distinguished"])

    def test_wrong_string_injection_distinguished(self):
        verdict = classify("wrong_str", uart_text="NO\n")
        self.assertEqual(verdict["status"], "rejected")
        self.assertTrue(verdict["distinguished"])

    def test_wrong_string_injection_not_distinguished_if_ok(self):
        self.assertFalse(classify("wrong_str")["distinguished"])

    def test_injection_results_differ_from_each_other(self):
        a = classify("no_hlt", exit="budget")
        b = classify("wrong_str", uart_text="NO\n")
        self.assertNotEqual(
            [c["actual"] for c in a["checks"]],
            [c["actual"] for c in b["checks"]],
        )

    def test_overall_status(self):
        ok = {
            "main": classify(),
            "no_hlt": classify("no_hlt", exit="budget"),
            "wrong_str": classify("wrong_str", uart_text="NO\n"),
        }
        self.assertEqual(m.overall_status(ok), "accepted")
        bad_main = dict(ok, main=classify(exit="budget"))
        self.assertEqual(m.overall_status(bad_main), "rejected")
        undistinguished = dict(ok, no_hlt=classify("no_hlt", exit="hlt"))
        self.assertEqual(m.overall_status(undistinguished), "rejected")
        self.assertEqual(m.overall_status({}), "rejected")


class ExitSequenceTests(unittest.TestCase):
    def test_expected_exit_sequence_is_strict(self):
        self.assertEqual(m.EXPECTED_EXITS[-1], {"kind": "hlt"})
        self.assertEqual([int(e["data"], 16) for e in m.EXPECTED_EXITS[:3]], [0x4f, 0x4b, 0x0a])
        self.assertTrue(all(e["port"] == "0x3f8" and e["size"] == 1 and e["dir"] == "out" for e in m.EXPECTED_EXITS[:3]))


class ParseTests(unittest.TestCase):
    def test_single_json_line(self):
        self.assertEqual(m.parse_runner_stdout('{"a":1}\n')["a"], 1)

    def test_extra_output_rejected(self):
        with self.assertRaises(m.BuildError):
            m.parse_runner_stdout('noise\n{"a":1}\n')

    def test_not_json_rejected(self):
        with self.assertRaises(m.BuildError):
            m.parse_runner_stdout("hello\n")

    def test_not_object_rejected(self):
        with self.assertRaises(m.BuildError):
            m.parse_runner_stdout("[1]\n")


class FakeHost:
    """Records commands; pretends to be the mini."""

    def __init__(self, run_results):
        self.commands = []
        self.run_results = run_results
        self.shas = {}

    def __call__(self, command, timeout):
        self.commands.append(command)
        text = " ".join(command)
        out, rc = "", 0
        if command[0] == "scp":
            local = Path(command[-2])
            self.shas[Path(command[-1].split(":", 1)[1]).name] = m.sha256_file(local)
        elif "sha256sum" in text:
            name = Path(command[-1].split()[-1]).name
            out = f"{self.shas.get(name, '0' * 64)}  {name}\n"
        elif "./kvm_runner" in text:
            image = text.split("./kvm_runner ")[1].split()[0]
            out = json.dumps(self.run_results[image]) + "\n"
        elif "gcc --version" in text:
            out = "gcc (fake) 15\n"
        return subprocess.CompletedProcess(command, rc, out, "")


class OrchestrationTests(unittest.TestCase):
    def fake_builder(self, case, workdir):
        return f"image-{case}".encode(), {
            "clang": {"path": "c", "version": "v"},
            "ld": {"path": "l", "version": "v"},
            "objcopy": {"path": "o", "version": "v"},
        }

    def test_full_flow_with_fake_host(self):
        results = {
            "kvm_method_main.bin": good_result(),
            "kvm_method_no_hlt.bin": good_result(exit="budget", run_count=204),
            "kvm_method_wrong_str.bin": good_result(uart_text="NO\n", uart_hex="4e4f0a"),
        }
        host = FakeHost(results)
        remote = m.Remote("user@host", "/remote/dir", runner=host)
        with tempfile.TemporaryDirectory() as tmp:
            body = m.orchestrate(remote, ["main", "no_hlt", "wrong_str"], Path(tmp), image_builder=self.fake_builder)
        self.assertEqual(body["status"], "accepted")
        self.assertEqual(body["runs"]["main"]["verdict"]["status"], "accepted")
        self.assertEqual(body["runs"]["no_hlt"]["exit"], "budget")
        self.assertEqual(body["runs"]["wrong_str"]["uart_text"], "NO\n")
        self.assertEqual(body["identity"]["hostname"], "h")
        self.assertEqual(body["runs"]["main"]["intr_count"], 0)
        self.assertEqual(body["tools"]["clang"]["version"], "v")
        text = [" ".join(c) for c in host.commands]
        self.assertTrue(any("gcc -O2 -Wall -Wextra -Werror -std=c11" in t for t in text))
        self.assertTrue(all(c[0] in ("ssh", "scp") for c in host.commands))
        self.assertTrue(all("BatchMode=yes" in c for c in host.commands))
        self.assertTrue(any(t.startswith("scp") and t.endswith("user@host:/remote/dir/kvm_runner.c") for t in text))

    def test_remote_hash_mismatch_is_rejected(self):
        results = {"kvm_method_main.bin": good_result()}
        host = FakeHost(results)
        original = host.__call__

        def tampering(command, timeout):
            res = original(command, timeout)
            if "sha256sum" in " ".join(command) and "kvm_method_main.bin" in " ".join(command):
                return subprocess.CompletedProcess(command, 0, "f" * 64 + "  x\n", "")
            return res

        remote = m.Remote("user@host", "/remote/dir", runner=tampering)
        with tempfile.TemporaryDirectory() as tmp:
            body = m.orchestrate(remote, ["main"], Path(tmp), image_builder=self.fake_builder)
        self.assertEqual(body["status"], "rejected")
        self.assertEqual(body["runs"]["main"]["verdict"]["status"], "rejected")

    def test_ssh_failure_is_build_error(self):
        def failing(command, timeout):
            return subprocess.CompletedProcess(command, 255, "", "no route")

        remote = m.Remote("user@host", "/remote/dir", runner=failing)
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(m.BuildError):
                m.orchestrate(remote, ["main"], Path(tmp), image_builder=self.fake_builder)

    def test_runner_nonzero_exit_is_build_error(self):
        def runner(command, timeout):
            rc = 2 if "./kvm_runner " in " ".join(command) else 0
            return subprocess.CompletedProcess(command, rc, "{}\n" if rc == 0 else "", "err")

        remote = m.Remote("user@host", "/remote/dir", runner=runner)
        with self.assertRaises(m.BuildError):
            remote.run_image("x.bin")


class SourceTests(unittest.TestCase):
    def test_fixture_and_runner_sources_exist(self):
        for path in (m.FIXTURE_DIR / "entry.S", m.FIXTURE_DIR / "layout.ld", m.RUNNER_SOURCE):
            self.assertTrue(path.is_file(), path)

    def test_cases_declare_distinguishable_expectations(self):
        keys = {(c["expect_exit"], c["expect_uart"]) for c in m.CASES.values()}
        self.assertEqual(len(keys), len(m.CASES))

    def test_entry_defines_injections(self):
        text = (m.FIXTURE_DIR / "entry.S").read_text()
        self.assertIn("INJECT_NO_HLT", text)
        self.assertIn("INJECT_WRONG_STR", text)

    def test_image_build_if_toolchain_present(self):
        try:
            m.find_tool("STAGE4_CLANG", ["clang"], [])
            m.find_tool("STAGE4_LD_LLD", ["ld.lld"], ["gcc-ld/ld.lld"])
            m.find_tool("STAGE4_OBJCOPY", ["llvm-objcopy"], ["llvm-objcopy", "rust-objcopy"])
        except m.BuildError:
            self.skipTest("clang/ld.lld/objcopy not available")
        with tempfile.TemporaryDirectory() as tmp:
            images = {c: m.build_image(c, Path(tmp))[0] for c in m.CASES}
        for case, image in images.items():
            self.assertLessEqual(len(image), 0x1000, case)
        self.assertTrue(images["main"].startswith(bytes.fromhex("66baf803b04feeb04bee")))
        self.assertIn(b"\xf4", images["main"])
        self.assertNotIn(b"\xf4", images["no_hlt"])
        self.assertIn(bytes.fromhex("b04e"), images["wrong_str"])
        self.assertEqual(len({m.sha256(i) for i in images.values()}), 3)


if __name__ == "__main__":
    unittest.main()
