"""Contract tests for the standard-library Stage 6 litmus fixture generator."""

from __future__ import annotations

import json
import math
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import build_stage6_litmus as litmus


class Stage6LitmusTests(unittest.TestCase):
    def test_generator_is_byte_deterministic(self) -> None:
        with tempfile.TemporaryDirectory() as first, tempfile.TemporaryDirectory() as second:
            h1 = litmus.generate(Path(first))
            h2 = litmus.generate(Path(second))
            self.assertEqual(h1, h2)
            a = sorted(p.name for p in Path(first).glob("*.json"))
            b = sorted(p.name for p in Path(second).glob("*.json"))
            self.assertEqual(a, b)
            self.assertEqual([p.read_bytes() for p in sorted(Path(first).glob("*.json"))],
                             [p.read_bytes() for p in sorted(Path(second).glob("*.json"))])

    def test_corpus_contract_and_encodings(self) -> None:
        fixtures = {p.stem.upper(): json.loads(p.read_text()) for p in litmus.OUT.glob("*.json")}
        required = {"SB", "MP", "LB", "IRIW", "2+2W", "SB-MFENCE", "SB-LOCK",
                    "SB-XCHG", "LOCK-CMPXCHG-MUTEX", "XCHG-MUTEX", "LOCK-XADD",
                    "STORE-FORWARDING", "UNLOCKED-INC", "SB-LOCK-LOCAL"}
        self.assertTrue(required <= {d["id"] for d in fixtures.values()})
        for f in fixtures.values():
            if "outcomes" not in f:
                continue
            self.assertEqual(f["schema"], "od.smp-litmus.v1")
            self.assertTrue(f["sources"])
            self.assertIn("derived_by", f)
            self.assertTrue(f["memory"]["cells"])
            self.assertIn("excluded", f["observe"])
            self.assertEqual(set(f["outcomes"]), {"x86_tso", "sc", "interleaving_instruction_atomic"})
            self.assertEqual(f["schedule_metadata"]["oracle_schedules"], "exhaustive; no sampling")
            self.assertEqual(f["schedule_metadata"]["runtime_interleaving"]["owner"], "S6-19")
            self.assertEqual(f["schedule_metadata"]["runtime_interleaving"]["exhaustive_schedule_threshold"],1_000_000)
            self.assertEqual(f["schedule_metadata"]["runtime_interleaving"]["sampling_seed_range_inclusive"],[0,9_999])
            self.assertLessEqual(f["schedule_metadata"]["order_preserving_instruction_schedules"],1_000_000)
            counts=f["schedule_metadata"]["memory_touching_instructions_per_thread"]
            expected_schedule_count=math.factorial(sum(counts))//math.prod(math.factorial(n) for n in counts)
            self.assertEqual(f["schedule_metadata"]["order_preserving_instruction_schedules"],expected_schedule_count)
            for model in f["outcomes"].values():
                self.assertEqual(model["values"], sorted(model["values"]))
                self.assertEqual(model["derived_by"], litmus.DERIVED_BY)
                self.assertGreater(model["state_count"], 0)
                self.assertEqual(len(model["input_sha256"]), 64)
            for thread in f["threads"]:
                for instruction in thread["instructions"]:
                    litmus.validate_instruction(instruction)

    def test_sb_matches_plan_sets(self) -> None:
        f = next(x for x in litmus.corpus() if x["id"] == "SB")
        result = litmus.enrich(f)["outcomes"]
        # Plan acceptance: 00 belongs to TSO and is excluded from both SC models.
        self.assertIn([0, 0, 1, 1], result["x86_tso"]["values"])
        self.assertNotIn([0, 0, 1, 1], result["sc"]["values"])
        self.assertNotIn([0, 0, 1, 1], result["interleaving_instruction_atomic"]["values"])
        self.assertEqual(len(result["x86_tso"]["values"]), 4)
        self.assertEqual(len(result["sc"]["values"]), 3)

    def test_locked_operation_only_drains_issuing_cpu_store_buffer(self) -> None:
        f=next(x for x in litmus.corpus() if x["id"]=="SB-LOCK-LOCAL")
        values=litmus.enrich(f)["outcomes"]["x86_tso"]["values"]
        # Witness: T1 buffers W(y), then reads x=0. T0 buffers W(x),
        # drains only its queue at LOCK INC, then reads still-buffered y=0.
        # T1's W(y) flushes last; both loads read 0 while final memory is 1,1.
        self.assertIn([0,0,1,1,1],values)
        self.assertNotIn([0,0,1,1,1],litmus.enumerate_model(f,"sc")[0])
        # Execute the recorded witness transitions directly to ensure the
        # outcome uses the pending T1 queue at T0's locked instruction.
        memory=dict(f["memory"]["initial"]);queues=[[],[]];reads={}
        for step in f["witnesses"]["x86_tso"]:
            cpu=step["cpu"]
            if step["action"]=="flush_oldest":
                loc,value=queues[cpu].pop(0);memory[loc]=value;continue
            instruction=f["threads"][cpu]["instructions"][step["instruction_index"]]
            event=instruction["memory_events"][0]
            if event["kind"]=="write": queues[cpu].append((event["location"],event["value"]))
            elif event["kind"]=="read":
                value=next((v for l,v in reversed(queues[cpu]) if l==event["location"]),memory[event["location"]])
                reads[(cpu,event["result"])]=value
            elif event["kind"]=="atomic_rmw":
                self.assertFalse(queues[cpu])
                memory[event["location"]]+=event["argument"]
        self.assertEqual(reads,{(1,"ebx"):0,(0,"eax"):0})
        self.assertEqual((memory["x"],memory["y"]),(1,1))

    def test_mp_forbidden_message_outcome_is_absent_from_tso(self) -> None:
        f = next(x for x in litmus.corpus() if x["id"] == "MP")
        values = litmus.enrich(f)["outcomes"]["x86_tso"]["values"]
        # Reader sees publication y=1 but stale payload x=0.
        self.assertNotIn([1, 0, 1, 1], values)

    def test_iriw_forbidden_opposite_reader_orders_are_absent_from_tso(self) -> None:
        f = next(x for x in litmus.corpus() if x["id"] == "IRIW")
        values = litmus.enrich(f)["outcomes"]["x86_tso"]["values"]
        # The classic forbidden result: both readers read 1 then 0 in
        # opposite location orders. Register order is (r2.x, r2.y, r3.y, r3.x).
        self.assertNotIn([1, 0, 1, 0, 1, 1], values)

    def test_unlocked_inc_loss_and_instruction_atomic_distinction(self) -> None:
        f = litmus.enrich(next(x for x in litmus.corpus() if x["id"] == "UNLOCKED-INC"))["outcomes"]
        self.assertIn([1], f["x86_tso"]["values"])
        self.assertIn([1], f["sc"]["values"])
        self.assertNotIn([1], f["interleaving_instruction_atomic"]["values"])
        self.assertEqual(f["interleaving_instruction_atomic"]["values"], [[2]])

    def test_encoding_address_mismatch_is_rejected(self) -> None:
        instruction = litmus.mov_load("x", "eax")
        instruction["bytes"] = "8b 04 25 04 10 00 00"
        with self.assertRaisesRegex(ValueError, "byte/operand address mismatch"):
            litmus.validate_instruction(instruction)

    def test_event_encoding_operand_mutations_are_rejected(self) -> None:
        checks=[
            (litmus.locked("x","lock_inc"),lambda i:i["memory_events"][0].update(argument=2),"locked byte/op/register mismatch"),
            (litmus.locked("x","lock_inc"),lambda i:i["memory_events"][0].update(operation="sub"),"locked byte/op/register mismatch"),
            (litmus.mov_store("x",1),lambda i:i["memory_events"][0].update(value=2),"store event/immediate mismatch"),
            (litmus.mov_store("x",1),lambda i:i.update(bytes=" ".join(i["bytes"].split()[:7]+["02"]+i["bytes"].split()[8:])),"store byte/operand mismatch"),
            (litmus.mov_load("x","eax"),lambda i:i["memory_events"][0].update(result="ebx"),"load byte/register mismatch"),
            (litmus.mov_load("x","eax"),lambda i:i.update(bytes=i["bytes"].replace("8b 04 25","8b 1c 25")),"load byte/register mismatch"),
            (litmus.mov_load("x","eax"),lambda i:(i.update(bytes=i["bytes"]+" 90"),i.update(length=i["length"]+1)),"instruction byte length mismatch"),
            (litmus.mov_store("x",1),lambda i:(i.update(bytes=i["bytes"]+" 90"),i.update(length=i["length"]+1)),"instruction byte length mismatch"),
            (litmus.locked("x","lock_inc"),lambda i:i["memory_events"].append(dict(i["memory_events"][0])),"exactly one memory event"),
            (litmus.locked("x","xchg"),lambda i:i["operands"].update(implicit_lock=False),"XCHG abstract-operation mismatch"),
            (litmus.ins("rmw","inc dword ptr [0x00001000]","ff 04 25 00 10 00 00",[
                {"kind":"rmw_read","location":"x","result":"old"},{"kind":"rmw_write","location":"x","value":{"add":"old","delta":1}}],
                operands={"location":"x","address":"0x00001000","delta":1}),lambda i:i["operands"].update(delta=2),"unlocked INC abstract-operation mismatch"),
            (litmus.ins("rmw","inc dword ptr [0x00001000]","ff 04 25 00 10 00 00",[
                {"kind":"rmw_read","location":"x","result":"old"},{"kind":"rmw_write","location":"x","value":{"add":"old","delta":1}}],
                operands={"location":"x","address":"0x00001000","delta":1}),lambda i:i["memory_events"][1]["value"].update(delta=2),"unlocked INC abstract-operation mismatch"),
        ]
        for original,mutate,message in checks:
            instruction=json.loads(json.dumps(original));mutate(instruction)
            with self.subTest(message=message),self.assertRaisesRegex(ValueError,message):
                litmus.validate_instruction(instruction)

    def test_formula_counter_group_has_executable_loops_and_closed_form_oracles(self) -> None:
        group=json.loads((litmus.OUT/"counters-formula.json").read_text())
        self.assertEqual(group["kind"],"formula_counter_group")
        self.assertEqual({c["id"] for c in group["cases"]},{"LOCK-INC","UNLOCKED-INC","THREE-INSN-COUNTER"})
        for case in group["cases"]:
            self.assertNotIn("outcomes",case)
            oracle=case["formula_oracle"]["x86_tso"]
            self.assertTrue(all(spec["complete"] for spec in case["formula_oracle"].values()))
            self.assertIn("N*K",json.dumps(oracle))
            self.assertIn("x86_tso",case["formula_oracle"])
            template=case["thread_template"]
            self.assertEqual(template["initial_registers"]["rcx"],"K")
            instructions=template["instructions"]
            self.assertEqual(template["code_region"]["size_bytes"],sum(i["length"] for i in instructions))
            loop=instructions[-1]
            self.assertEqual(loop["bytes"][-2:],{"LOCK-INC":"f6","UNLOCKED-INC":"f7","THREE-INSN-COUNTER":"ed"}[case["id"]])
            offset=sum(i["length"] for i in instructions[:-1])
            displacement=int.from_bytes(bytes.fromhex(loop["bytes"])[1:2],"little",signed=True)
            self.assertEqual(offset+loop["length"]+displacement,0)
            self.assertEqual(case["schedule_metadata"]["oracle_schedules"],"exhaustive closed-form oracle; no sampling")
            self.assertEqual(case["schedule_metadata"]["order_preserving_instruction_schedules"]["unit"],"complete schedules")
        self.assertEqual(len(group["cases"][2]["thread_template"]["instructions"]),4)

    def test_counter_formulae_match_small_exhaustive_models(self) -> None:
        def plain_inc():
            return litmus.ins("rmw","inc dword ptr [0x00001000]","ff 04 25 00 10 00 00",[
                {"kind":"rmw_read","location":"x","result":"old"},
                {"kind":"rmw_write","location":"x","value":{"add":"old","delta":1}}],
                operands={"location":"x","address":"0x00001000","delta":1})
        load=litmus.mov_load("x","eax")
        add=litmus.ins("register_alu","add eax, 1","83 c0 01",[],operands={"destination":"eax","source":1})
        store=litmus.ins("register_store","mov dword ptr [0x00001000], eax","89 04 25 00 10 00 00",
            [{"kind":"write","location":"x","value":"$eax"}],operands={"source":"eax","location":"x","address":"0x00001000"})
        group=litmus.counter_group(); oracles={c["id"]:c["formula_oracle"] for c in group["cases"]}
        for n in (1,2,3):
            for k in (1,2,3):
                for case_id, body, expected_min in (
                    ("UNLOCKED-INC",[plain_inc()],n*k if n==1 else (1 if k==1 else 2)),
                    ("THREE-INSN-COUNTER",[load,add,store],n*k if n==1 else (1 if k==1 else 2))):
                    f=litmus.case("small-counter",{"x":0},[
                        litmus.thread(cpu,[instruction for _ in range(k) for instruction in body])
                        for cpu in range(n)], ["mem.x"])
                    for model in ("x86_tso","sc","interleaving_instruction_atomic"):
                        low=n*k if case_id=="UNLOCKED-INC" and model=="interleaving_instruction_atomic" else expected_min
                        expected=[list((v,)) for v in range(low,n*k+1)]
                        values=litmus.enumerate_model(f,model)[0]
                        self.assertEqual(values,expected,(case_id,n,k,model))
                        spec=oracles[case_id][model]
                        self.assertTrue(spec["complete"])
                        if spec["kind"]=="exact_singleton":
                            self.assertEqual(values,[[n*k]])
                        else:
                            self.assertEqual(spec["kind"],"exact_integer_range")
                            if n >= 2:
                                self.assertEqual(spec["minimum"],"1 if K == 1 else 2")
                            self.assertEqual(spec["maximum"],"N*K")
        f=litmus.case("n2k2",{"x":0},[litmus.thread(c,[plain_inc(),plain_inc()]) for c in range(2)],["mem.x"])
        self.assertEqual(litmus.enumerate_model(f,"x86_tso")[0],[[2],[3],[4]])
        self.assertEqual(litmus.enumerate_model(f,"sc")[0],[[2],[3],[4]])


if __name__ == "__main__":
    unittest.main()
