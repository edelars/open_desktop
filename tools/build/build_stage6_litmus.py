#!/usr/bin/env python3
"""Generate and exhaustively model the project-owned Stage 6 litmus corpus.

The input corpus below is an abstract-operation description. Encodings are
fixed fixture data; this tool does not invoke an assembler or execute x86.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import platform
import random
import resource
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "tests/fixtures/guest/stage6/litmus"
DERIVED_BY = "build_stage6_litmus.py/1"
PAPER = {
    "title": "x86-TSO: A Rigorous and Usable Programmer’s Model for x86 Multiprocessors",
    "author": "Sewell et al.", "edition": "CACM 2010",
    "section": "§1, §3, §3.2–3.3",
    "url": "https://www.cl.cam.ac.uk/~pes20/weakmemory/cacm.pdf",
}
SDM = {
    "title": "Intel 64 and IA-32 Architectures Software Developer’s Manual",
    "author": "Intel", "edition": "v093",
    "section": "Vol. 3A, §8.2; instruction entries for MFENCE, XCHG, XADD, CMPXCHG",
    "url": "https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html",
}


def ins(op: str, asm: str, encoding: str, events: list[dict[str, Any]], **kwargs: Any) -> dict[str, Any]:
    abstract_operation=kwargs.pop("abstract_operation",op)
    return {"op": op, "abstract_operation":abstract_operation,"asm": asm, "bytes": encoding, "length": len(bytes.fromhex(encoding)),
            "operands": kwargs.pop("operands", {}), "memory_events": events,
            "instruction_boundary": True, **kwargs}


def rd(loc: str, reg: str) -> dict[str, str]: return {"kind": "read", "location": loc, "result": reg}
def wr(loc: str, value: Any) -> dict[str, Any]: return {"kind": "write", "location": loc, "value": value}
def rmw(loc: str, operation: str, arg: Any = 1, result: str | None = None, locked: bool = False) -> dict[str, Any]:
    return {"kind": "atomic_rmw" if locked else "rmw_read", "location": loc,
            "operation": operation, "argument": arg, "result": result}


def mov_store(loc: str, value: int, imm: str | None = None) -> dict[str, Any]:
    address = {"x": 0x1000, "y": 0x1004, "z": 0x1008, "w": 0x100c, "lock": 0x1010, "cnt": 0x1014}[loc]
    disp = " ".join(f"{b:02x}" for b in address.to_bytes(4,"little"))
    return ins("store", f"mov dword ptr [0x{address:08x}], {value}", f"c7 04 25 {disp} {value:02x} 00 00 00",
               [wr(loc, value)], operands={"destination": f"[{loc}]", "address": f"0x{address:08x}", "value": value})


def mov_load(loc: str, reg: str) -> dict[str, Any]:
    # Canonical 32-bit absolute SIB encodings, disp32 little endian.
    opcode = {"eax": "8b 04 25", "ebx": "8b 1c 25", "ecx": "8b 0c 25", "edx": "8b 14 25"}[reg]
    addr = {"x": "00100000", "y": "04100000", "z": "08100000", "w": "0c100000", "lock": "10100000", "cnt": "14100000"}[loc]
    address = {"x": 0x1000, "y": 0x1004, "z": 0x1008, "w": 0x100c, "lock": 0x1010, "cnt": 0x1014}[loc]
    disp = " ".join(f"{b:02x}" for b in address.to_bytes(4,"little"))
    return ins("load", f"mov {reg}, dword ptr [0x{address:08x}]", opcode + " " + disp,
               [rd(loc, reg)], operands={"destination": reg, "source": f"[{loc}]", "address": f"0x{address:08x}"})


def barrier(op: str = "mfence") -> dict[str, Any]:
    return ins(op, op.upper(), "0f ae f0" if op == "mfence" else "f0 83 04 25 10 10 00 00 00", [],
               operands={"memory": "all"}, serializing=True)


def locked(loc: str, op: str, reg: str = "eax", imm: int = 1, implicit: bool = False) -> dict[str, Any]:
    address = {"x": 0x1000, "y": 0x1004, "z": 0x1008, "w": 0x100c, "lock": 0x1010, "cnt": 0x1014}[loc]
    addr = " ".join(f"{b:02x}" for b in address.to_bytes(4,"little"))
    reg_field={"eax":"04","ecx":"0c","edx":"14","ebx":"1c"}[reg]
    if op == "lock_inc":
        enc, asm, kind = "f0 ff 04 25 " + addr, f"lock inc dword ptr [{loc}]", "locked_rmw"
        ev = [rmw(loc, "add", 1, locked=True)]
    elif op == "xchg":
        enc, asm, kind = f"87 {reg_field} 25 " + addr, f"xchg dword ptr [{loc}], {reg}", "implicit_locked_rmw"
        ev = [{"kind": "atomic_xchg", "location": loc, "register": reg}]
    elif op == "lock_xadd":
        enc, asm, kind = f"f0 0f c1 {reg_field} 25 " + addr, f"lock xadd dword ptr [{loc}], {reg}", "locked_rmw"
        ev = [{"kind": "atomic_xadd", "location": loc, "register": reg}]
    elif op == "lock_cmpxchg":
        enc, asm, kind = f"f0 0f b1 {reg_field} 25 " + addr, f"lock cmpxchg dword ptr [{loc}], {reg}", "locked_rmw"
        ev = [{"kind": "atomic_cmpxchg", "location": loc, "expected": "eax", "replacement": reg}]
    else: raise ValueError(op)
    asm = asm.replace(f"[{loc}]",f"[0x{address:08x}]")
    operands={"location":f"[{loc}]","address":f"0x{address:08x}","implicit_lock":(op=="xchg" or implicit)}
    if op!="lock_inc": operands["register"]=reg
    return ins(kind, asm, enc, ev, operands=operands, atomic=True)


def thread(cpu: int, instructions: list[dict[str, Any]], **initial_registers: int) -> dict[str, Any]:
    regs={"eax": 0, "ebx": 0, "ecx": 0, "edx": 0}; regs.update(initial_registers)
    return {"cpu": cpu, "initial_registers": regs, "instructions": instructions}


def case(test_id: str, initial: dict[str, int], threads: list[dict[str, Any]], observe: list[str], source=PAPER) -> dict[str, Any]:
    memory_instruction_counts=[sum(bool(i["memory_events"]) for i in t["instructions"]) for t in threads]
    n=sum(memory_instruction_counts)
    schedule_count=math.factorial(n)
    for count in memory_instruction_counts: schedule_count//=math.factorial(count)
    return {"schema": "od.smp-litmus.v1", "id": test_id, "sources": [source],
            "derived_by": DERIVED_BY,
            "memory": {"width_bits": 32, "cells": [{"name": k, "size_bits": 32, "address": f"0x{ {'x':0x1000,'y':0x1004,'z':0x1008,'w':0x100c,'lock':0x1010,'cnt':0x1014}[k]:08x}",
                      "initial": v, "kind": "write_back_ram", "alias": None} for i, (k, v) in enumerate(initial.items())],
                       "initial": initial, "kind": "write_back_ram",
                       "code_regions": [{"cpu": t["cpu"], "start": f"0x{0x2000+t['cpu']*0x1000:08x}",
                           "size_bytes": sum(i["length"] for i in t["instructions"])} for t in threads],
                       "register_namespace": "per_cpu_register_files_disjoint_from_guest_memory"},
            "threads": threads, "observe": {"fields": observe, "excluded": ["all registers and memory cells not listed above"]},
            "schedule_metadata":{"memory_touching_instructions_per_thread":memory_instruction_counts,
                "order_preserving_instruction_schedules":schedule_count,"oracle_schedules":"exhaustive; no sampling",
                "runtime_interleaving":{"owner":"S6-19","status":"not_run_by_S6-05",
                    "exhaustive_schedule_threshold":1_000_000,"sampling_seed_range_inclusive":[0,9_999],
                    "seed_rule":"Python random.Random(seed) shuffles available instruction-step successors; deterministic by seed, not a confidence claim",
                    "policy_status":"measured_by_S6-05; runtime use and result recording belong to S6-19"}}}


def corpus() -> list[dict[str, Any]]:
    cases = []
    cases.append(case("SB", {"x":0,"y":0}, [thread(0,[mov_store("x",1),mov_load("y","eax")]),thread(1,[mov_store("y",1),mov_load("x","ebx")])], ["cpu0.eax","cpu1.ebx","mem.x","mem.y"]))
    cases.append(case("MP", {"x":0,"y":0}, [thread(0,[mov_store("x",1),mov_store("y",1)]),thread(1,[mov_load("y","eax"),mov_load("x","ebx")])], ["cpu1.eax","cpu1.ebx","mem.x","mem.y"]))
    cases.append(case("LB", {"x":0,"y":0}, [thread(0,[mov_load("y","eax"),mov_store("x",1)]),thread(1,[mov_load("x","ebx"),mov_store("y",1)])], ["cpu0.eax","cpu1.ebx","mem.x","mem.y"]))
    cases.append(case("IRIW", {"x":0,"y":0}, [thread(0,[mov_store("x",1)]),thread(1,[mov_store("y",1)]),thread(2,[mov_load("x","eax"),mov_load("y","ebx")]),thread(3,[mov_load("y","ecx"),mov_load("x","edx")])], ["cpu2.eax","cpu2.ebx","cpu3.ecx","cpu3.edx","mem.x","mem.y"]))
    cases.append(case("2+2W", {"x":0,"y":0}, [thread(0,[mov_store("x",1),mov_store("y",1)]),thread(1,[mov_store("y",2),mov_store("x",2)])], ["mem.x","mem.y"]))
    cases.append(case("SB-MFENCE", {"x":0,"y":0}, [thread(0,[mov_store("x",1),barrier(),mov_load("y","eax")]),thread(1,[mov_store("y",1),barrier(),mov_load("x","ebx")])], ["cpu0.eax","cpu1.ebx","mem.x","mem.y"], SDM))
    cases.append(case("SB-LOCK", {"x":0,"y":0,"lock":0}, [thread(0,[mov_store("x",1),locked("lock","lock_inc"),mov_load("y","eax")]),thread(1,[mov_store("y",1),locked("lock","lock_inc"),mov_load("x","ebx")])], ["cpu0.eax","cpu1.ebx","mem.x","mem.y","mem.lock"], SDM))
    cases.append(case("SB-LOCK-LOCAL", {"x":0,"y":0,"lock":0}, [thread(0,[mov_store("x",1),locked("lock","lock_inc"),mov_load("y","eax")]),thread(1,[mov_store("y",1),mov_load("x","ebx")])], ["cpu0.eax","cpu1.ebx","mem.x","mem.y","mem.lock"], SDM))
    cases[-1]["witnesses"]={"x86_tso":[
        {"action":"instruction","cpu":1,"instruction_index":0},
        {"action":"instruction","cpu":1,"instruction_index":1},
        {"action":"instruction","cpu":0,"instruction_index":0},
        {"action":"flush_oldest","cpu":0},
        {"action":"instruction","cpu":0,"instruction_index":1},
        {"action":"instruction","cpu":0,"instruction_index":2},
        {"action":"flush_oldest","cpu":1}]}
    cases.append(case("SB-XCHG", {"x":0,"y":0,"lock":0}, [thread(0,[mov_store("x",1),locked("lock","xchg"),mov_load("y","eax")]),thread(1,[mov_store("y",1),locked("lock","xchg"),mov_load("x","ebx")])], ["cpu0.eax","cpu1.ebx","mem.x","mem.y","mem.lock"], SDM))
    cases.append(case("LOCK-CMPXCHG-MUTEX", {"x":0}, [thread(0,[locked("x","lock_cmpxchg","ebx")],ebx=1),thread(1,[locked("x","lock_cmpxchg","ebx")],ebx=2)], ["mem.x","cpu0.eax","cpu1.eax"], SDM))
    cases.append(case("XCHG-MUTEX", {"x":0}, [thread(0,[locked("x","xchg","ebx",implicit=True)],ebx=1),thread(1,[locked("x","xchg","ebx",implicit=True)],ebx=2)], ["mem.x","cpu0.ebx","cpu1.ebx"], SDM))
    cases.append(case("LOCK-XADD", {"x":0}, [thread(0,[locked("x","lock_xadd","eax")],eax=1),thread(1,[locked("x","lock_xadd","ebx")],ebx=1)], ["mem.x","cpu0.eax","cpu1.ebx"], SDM))
    cases.append(case("STORE-FORWARDING", {"x":0}, [thread(0,[mov_store("x",1),mov_load("x","eax")]),thread(1,[mov_load("x","ebx")])], ["cpu0.eax","cpu1.ebx","mem.x"]))
    inc=lambda: ins("rmw", "inc dword ptr [0x00001000]", "ff 04 25 00 10 00 00", [
        {"kind":"rmw_read","location":"x","result":"tmp"}, {"kind":"rmw_write","location":"x","value":{"add":"tmp","delta":1}}], operands={"location":"x","address":"0x00001000","delta":1})
    cases.append(case("UNLOCKED-INC", {"x":0}, [thread(0,[inc()]),thread(1,[inc()])], ["mem.x"], SDM))
    cases.append(case("SB-LOCK-XADD", {"x":0,"y":0,"lock":0}, [thread(0,[mov_store("x",1),locked("lock","lock_xadd","ecx"),mov_load("y","eax")]),thread(1,[mov_store("y",1),locked("lock","lock_xadd","edx"),mov_load("x","ebx")])], ["cpu0.eax","cpu1.ebx","mem.x","mem.y","mem.lock"], SDM))
    return cases


def _eval(v: Any, regs: dict[str, int], temp: dict[str,int] | None = None) -> int:
    if isinstance(v, dict) and "add" in v: return (temp or {})[v["add"]]+v["delta"]
    return regs[v[1:]] if isinstance(v, str) and v.startswith("$") else int(v)


def _finish_instruction(insn: dict[str, Any], regs: dict[str, int], reads: dict[str, int]) -> None:
    for e in insn["memory_events"]:
        if e["kind"] in ("read", "rmw_read") and e.get("result"):
            regs[e["result"]] = reads[e["result"]]


def _execute_nonmemory(insn: dict[str, Any], regs: dict[str, int]) -> None:
    if insn["op"] == "register_alu":
        destination=insn["operands"]["destination"]
        regs[destination]=(regs[destination]+int(insn["operands"]["source"]))&0xffffffff


def _atomic(insn: dict[str, Any], mem: dict[str, int], regs: dict[str, int]) -> None:
    e = insn["memory_events"][0]; loc=e["location"]; old=mem[loc]
    if e["kind"] == "atomic_rmw": new=old + int(e["argument"])
    elif e["kind"] == "atomic_xchg": new=regs[e["register"]]; regs[e["register"]]=old
    elif e["kind"] == "atomic_xadd": new=old+regs[e["register"]]; regs[e["register"]]=old
    elif e["kind"] == "atomic_cmpxchg":
        reg=e["replacement"]; new=regs[reg] if regs["eax"] == old else old; regs["eax"]=old
    else: raise ValueError(e)
    mem[loc]=new


def enumerate_model(f: dict[str, Any], model: str, stop_after: int | None = None,
                   stats: dict[str, int] | None = None, *, deduplicate: bool = True,
                   stop_after_schedules: int | None = None, schedule_seed: int | None = None) -> tuple[list[list[int]], int]:
    """Explore model states; optionally preserve every path to count complete schedules."""
    threads=f["threads"]; initial=dict(f["memory"]["initial"]); n=len(threads)
    # State includes per-thread instruction/event cursor, registers/read temporaries,
    # committed memory, FIFO queues, and in-progress event results.
    start=(tuple((0,0) for _ in range(n)), tuple(tuple(sorted(t["initial_registers"].items())) for t in threads),
           tuple(sorted(initial.items())), tuple(() for _ in threads), tuple(() for _ in threads))
    seen={start}; todo=[start]; found=set(); states=0; terminal_states=0
    rng=random.Random(schedule_seed) if schedule_seed is not None else None
    while todo:
        if stop_after is not None and states >= stop_after: break
        state=todo.pop(); states+=1
        curs, regitems, memitems, qitems, temps=state
        regs=[dict(x) for x in regitems]; mem=dict(memitems); queues=[list(x) for x in qitems]; temp=[dict(x) for x in temps]
        if all(curs[i][0] >= len(threads[i]["instructions"]) for i in range(n)) and (model!="x86_tso" or not any(queues)):
            terminal_states+=1
            fields=[]
            for name in f["observe"]["fields"]:
                if name.startswith("cpu"): fields.append(regs[int(name[3:name.index(".")])][name[name.index(".")+1:]])
                else: fields.append(mem[name[4:]])
            found.add(tuple(fields))
            if stop_after_schedules is not None and terminal_states >= stop_after_schedules: break
            continue
        nxt=[]
        for ti,t in enumerate(threads):
            ii,ei=curs[ti]
            if ii>=len(t["instructions"]): continue
            insn=t["instructions"][ii]; events=insn["memory_events"]
            if model=="interleaving_instruction_atomic":
                mm=mem.copy(); rr=[r.copy() for r in regs]; tt=[x.copy() for x in temp]
                for e in events:
                    loc=e["location"]
                    if e["kind"] in ("write","rmw_write"): mm[loc]=_eval(e["value"],rr[ti],tt[ti])
                    elif e["kind"] in ("read","rmw_read"): tt[ti][e["result"]]=mm[loc]
                    elif e["kind"]=="atomic_rmw": _atomic(insn,mm,rr[ti])
                    elif e["kind"].startswith("atomic_"): _atomic(insn,mm,rr[ti])
                _execute_nonmemory(insn,rr[ti]);_finish_instruction(insn,rr[ti],tt[ti]); cc=list(curs);cc[ti]=(ii+1,0)
                nxt.append((tuple(cc),tuple(tuple(sorted(r.items())) for r in rr),tuple(sorted(mm.items())),tuple(tuple(q) for q in queues),tuple(tuple(sorted(x.items())) for x in tt)))
            elif ei < len(events):
                e=events[ei]; loc=e["location"]; mm=mem.copy(); qq=[q.copy() for q in queues]; rr=[r.copy() for r in regs]; tt=[x.copy() for x in temp]
                if e["kind"] in ("write","rmw_write"):
                    val=_eval(e["value"],rr[ti],tt[ti])
                    if model=="x86_tso": qq[ti].append((loc,val))
                    else: mm[loc]=val
                    ee=ei+1
                elif e["kind"] in ("read","rmw_read"):
                    value=next((v for l,v in reversed(qq[ti]) if l==loc),mm[loc]);tt[ti][e["result"]]=value;ee=ei+1
                elif e["kind"]=="atomic_rmw" or e["kind"].startswith("atomic_"):
                    # A locked operation drains the issuing CPU's FIFO buffer.
                    # Other CPUs may retain pending stores; the atomic event
                    # does not flush their queues.
                    if qq[ti]: continue
                    _atomic(insn,mm,rr[ti]);ee=ei+1
                else: raise ValueError(e)
                cc=list(curs)
                if ee==len(events): _finish_instruction(insn,rr[ti],tt[ti]);cc[ti]=(ii+1,0)
                else: cc[ti]=(ii,ee)
                nxt.append((tuple(cc),tuple(tuple(sorted(r.items())) for r in rr),tuple(sorted(mm.items())),tuple(tuple(q) for q in qq),tuple(tuple(sorted(x.items())) for x in tt)))
            elif insn["op"]=="mfence":
                if not queues[ti]:
                    cc=list(curs);cc[ti]=(ii+1,0);nxt.append((tuple(cc),regitems,memitems,qitems,temps))
            else:
                rr=[r.copy() for r in regs];_execute_nonmemory(insn,rr[ti])
                cc=list(curs);cc[ti]=(ii+1,0)
                nxt.append((tuple(cc),tuple(tuple(sorted(r.items())) for r in rr),memitems,qitems,temps))
        if model=="x86_tso":
            for ti,q in enumerate(queues):
                if q:
                    mm=mem.copy();qq=[x.copy() for x in queues];loc,val=qq[ti].pop(0);mm[loc]=val
                    nxt.append((curs,regitems,tuple(sorted(mm.items())),tuple(tuple(x) for x in qq),temps))
        if rng is not None: rng.shuffle(nxt)
        for ns in nxt:
            if deduplicate:
                if ns not in seen: seen.add(ns);todo.append(ns)
            else:
                seen.add(ns);todo.append(ns)
    if stats is not None:
        stats.update({"expanded_states":states,"unique_states_stored":len(seen),
                      "pending_frontier_states":len(todo),"completed_schedule_paths":terminal_states,
                      "terminal_outcome_states":terminal_states})
    return [list(x) for x in sorted(found)], len(seen)


def validate_instruction(insn: dict[str, Any]) -> None:
    raw=bytes.fromhex(insn["bytes"])
    if len(raw)!=insn["length"]: raise ValueError("encoding length mismatch")
    asm=insn["asm"].lower(); op=insn["op"]; operands=insn.get("operands",{}); events=insn["memory_events"]
    addrmap={"x":0x1000,"y":0x1004,"z":0x1008,"w":0x100c,"lock":0x1010,"cnt":0x1014}
    reg_code={"eax":0x04,"ecx":0x0c,"edx":0x14,"ebx":0x1c}
    def exact_length(size: int) -> None:
        if len(raw)!=size: raise ValueError("instruction byte length mismatch")
    def location(event: dict[str, Any]) -> str:
        loc=event.get("location")
        if loc not in addrmap or operands.get("location",loc) not in (loc,f"[{loc}]"):
            raise ValueError("abstract operand/event mismatch")
        address=f"0x{addrmap[loc]:08x}"
        if operands.get("address")!=address or address not in asm:
            raise ValueError("event/operand address mismatch")
        if raw.count(addrmap[loc].to_bytes(4,"little"))<1:
            raise ValueError("byte/operand address mismatch")
        return loc
    if op=="store":
        exact_length(11)
        if len(events)!=1 or events[0].get("kind")!="write": raise ValueError("store event/immediate mismatch")
        loc=location(events[0]); val=operands.get("value")
        if raw[:7]!=bytes.fromhex("c7 04 25")+addrmap[loc].to_bytes(4,"little") or int.from_bytes(raw[7:11],"little")!=val:
            raise ValueError("store byte/operand mismatch")
        if events[0].get("value")!=val or asm!=f"mov dword ptr [0x{addrmap[loc]:08x}], {val}":
            raise ValueError("store event/immediate mismatch")
    elif op=="load":
        exact_length(7)
        reg=operands.get("destination"); enc={"eax":0x04,"ecx":0x0c,"edx":0x14,"ebx":0x1c}.get(reg)
        if enc is None or len(events)!=1 or events[0].get("kind")!="read" or events[0].get("result")!=reg:
            raise ValueError("load byte/register mismatch")
        loc=location(events[0]); expected=bytes([0x8b,enc,0x25])+addrmap[loc].to_bytes(4,"little")
        if raw!=expected or operands.get("source")!=f"[{loc}]" or asm!=f"mov {reg}, dword ptr [0x{addrmap[loc]:08x}]":
            raise ValueError("load byte/register mismatch")
    elif op=="rmw":
        exact_length(7)
        if len(events)!=2 or events[0].get("kind")!="rmw_read" or events[1].get("kind")!="rmw_write":
            raise ValueError("unlocked INC abstract-operation mismatch")
        loc=location(events[0]);location(events[1])
        old=events[0].get("result")
        if raw!=bytes.fromhex("ff 04 25")+addrmap[loc].to_bytes(4,"little") or operands.get("delta")!=1 or \
           events[0].get("location")!=events[1].get("location") or events[1].get("value")!={"add":old,"delta":1} or \
           asm!=f"inc dword ptr [0x{addrmap[loc]:08x}]":
            raise ValueError("unlocked INC abstract-operation mismatch")
    elif op=="mfence":
        exact_length(3)
        if raw!=bytes.fromhex("0f ae f0") or events or asm!="mfence": raise ValueError("MFENCE bytes mismatch")
    elif op=="register_alu":
        exact_length(3)
        if raw!=bytes.fromhex("83 c0 01") or events or operands!={"destination":"eax","source":1} or asm!="add eax, 1":
            raise ValueError("register ALU byte/operand mismatch")
    elif op=="register_store":
        exact_length(7)
        if len(events)!=1 or events[0].get("kind")!="write" or events[0].get("value")!="$eax":
            raise ValueError("register store event/operand mismatch")
        loc=location(events[0]); expected=bytes.fromhex("89 04 25")+addrmap[loc].to_bytes(4,"little")
        if raw!=expected or operands.get("source")!="eax" or asm!=f"mov dword ptr [0x{addrmap[loc]:08x}], eax":
            raise ValueError("register store byte/operand mismatch")
    elif op=="loop":
        exact_length(2)
        if events or raw[0]!=0xe2 or raw[1]!=(int(operands.get("relative_displacement",-257))&0xff) or operands.get("counter")!="rcx" or operands.get("target")!=".loop" or \
           operands.get("decrement_then_branch")!="rcx != 0" or asm!="loop .loop":
            raise ValueError("loop byte/operand mismatch")
    elif op in ("locked_rmw","implicit_locked_rmw"):
        if len(events)!=1: raise ValueError("locked instruction must have exactly one memory event")
        event=events[0];loc=location(event)
        if op=="locked_rmw" and asm.startswith("lock inc"):
            exact_length(8)
            if raw!=bytes.fromhex("f0 ff 04 25")+addrmap[loc].to_bytes(4,"little") or event!={"kind":"atomic_rmw","location":loc,"operation":"add","argument":1,"result":None} or \
               asm!=f"lock inc dword ptr [0x{addrmap[loc]:08x}]": raise ValueError("locked byte/op/register mismatch")
        elif op=="locked_rmw":
            reg=operands.get("register"); code=reg_code.get(reg)
            if code is None: raise ValueError("locked byte/op/register mismatch")
            if "xadd" in asm:
                exact_length(9); expected=bytes.fromhex("f0 0f c1")+bytes([code,0x25])+addrmap[loc].to_bytes(4,"little")
                valid=event=={"kind":"atomic_xadd","location":loc,"register":reg}
            else:
                exact_length(9); expected=bytes.fromhex("f0 0f b1")+bytes([code,0x25])+addrmap[loc].to_bytes(4,"little")
                valid=event=={"kind":"atomic_cmpxchg","location":loc,"expected":"eax","replacement":reg}
            if raw!=expected or not valid or asm not in (f"lock xadd dword ptr [0x{addrmap[loc]:08x}], {reg}",f"lock cmpxchg dword ptr [0x{addrmap[loc]:08x}], {reg}"):
                raise ValueError("locked byte/op/register mismatch")
        else:
            exact_length(7);reg=operands.get("register");code=reg_code.get(reg)
            expected=bytes([0x87,code or 0,0x25])+addrmap[loc].to_bytes(4,"little")
            if code is None or operands.get("implicit_lock") is not True or raw!=expected or event!={"kind":"atomic_xchg","location":loc,"register":reg} or \
               asm!=f"xchg dword ptr [0x{addrmap[loc]:08x}], {reg}": raise ValueError("XCHG abstract-operation mismatch")
    else:
        raise ValueError(f"unsupported instruction op: {op}")


def enrich(f: dict[str, Any]) -> dict[str, Any]:
    for t in f["threads"]:
        for i in t["instructions"]: validate_instruction(i)
    data_ranges=[(int(c["address"],16),int(c["address"],16)+c["size_bits"]//8) for c in f["memory"]["cells"]]
    address_by_name={c["name"]:c["address"] for c in f["memory"]["cells"]}
    expected_addresses={"x":"0x00001000","y":"0x00001004","z":"0x00001008","w":"0x0000100c","lock":"0x00001010","cnt":"0x00001014"}
    if any(address_by_name.get(name)!=expected_addresses[name] for name in address_by_name):
        raise ValueError("memory cell address disagrees with encoded operand map")
    code_ranges=[(int(c["start"],16),int(c["start"],16)+c["size_bytes"]) for c in f["memory"]["code_regions"]]
    for a,b in code_ranges:
        if any(a<d and c<b for c,d in data_ranges): raise ValueError("guest code/data regions overlap")
    input_hash=hashlib.sha256(canonical_bytes(f)).hexdigest()
    derived={m:enumerate_model(f,m) for m in ("x86_tso","sc","interleaving_instruction_atomic")}
    sets={m:outcomes for m,(outcomes,_) in derived.items()}
    f["outcomes"]={k:{"values":v,"derived_by":DERIVED_BY,"state_count":derived[k][1],
                       "input_sha256":input_hash} for k,v in sets.items()}
    f["interesting"]=[o for o in sets["x86_tso"] if o not in sets["sc"]]
    return f


def canonical_bytes(obj: Any) -> bytes:
    return (json.dumps(obj,sort_keys=True,indent=2,ensure_ascii=False)+"\n").encode()


def generate(output: Path = OUT) -> str:
    output.mkdir(parents=True,exist_ok=True)
    digest=hashlib.sha256()
    for f in corpus():
        fixture=enrich(f); raw=canonical_bytes(fixture); (output/f"{fixture['id'].lower()}.json").write_bytes(raw); digest.update(raw)
    formula=counter_group()
    raw=canonical_bytes(formula);(output/"counters-formula.json").write_bytes(raw);digest.update(raw)
    return digest.hexdigest()


def counter_group() -> dict[str, Any]:
    """Executable loop templates with closed-form final-state oracles."""
    inc_locked=locked("x","lock_inc")
    inc_plain=ins("rmw","inc dword ptr [0x00001000]","ff 04 25 00 10 00 00",[
        {"kind":"rmw_read","location":"x","result":"old"},
        {"kind":"rmw_write","location":"x","value":{"add":"old","delta":1}}],
        operands={"location":"x","address":"0x00001000","delta":1})
    load=mov_load("x","eax")
    add=ins("register_alu","add eax, 1","83 c0 01",[],operands={"destination":"eax","source":1})
    store_reg=ins("register_store","mov dword ptr [0x00001000], eax","89 04 25 00 10 00 00",
        [{"kind":"write","location":"x","value":"$eax"}],
        operands={"source":"eax","location":"x","address":"0x00001000"})
    def loop(rel: str) -> dict[str,Any]:
        displacement=int.from_bytes(bytes.fromhex(rel),"little",signed=True)
        return ins("loop","loop .loop",f"e2 {rel}",[],operands={"counter":"rcx","target":".loop",
            "relative_displacement":displacement,"decrement_then_branch":"rcx != 0"})
    def make_case(case_id: str, body: list[dict[str,Any]], loop_rel: str, oracle: dict[str,Any]) -> dict[str,Any]:
        instructions=[dict(i) for i in body]+[loop(loop_rel)]
        instructions[0]["label"]=".loop"
        for instruction in instructions: validate_instruction(instruction)
        return {"id":case_id,"parameters":{"N":{"type":"integer","minimum":2},"K":{"type":"integer","minimum":1},
                   "constraint":"N*K <= 2^32-1; all arithmetic remains within the 32-bit counter width"},
            "initial_memory":{"width_bits":32,"cells":[{"name":"x","address":"0x00001000","initial":0,
                "kind":"write_back_ram","size_bits":32,"alias":None}]},
            "thread_template":{"cpu":"i where 0 <= i < N","code_base":"0x2000 + 0x1000*i",
                "initial_registers":{"rcx":"K","eax":0},"loop_label":".loop","loop_body_instruction_count":len(body),
                "code_region":{"start_formula":"0x2000 + 0x1000*i","size_bytes":sum(i["length"] for i in instructions)},
                "instructions":instructions,"loop_semantics":"Start at .loop; after the body, LOOP decrements RCX and branches to .loop iff RCX != 0. K >= 1."},
            "observe":{"fields":["mem.x"],"excluded":["all CPU registers after loop termination"]},
            "sources":[SDM],"derived_by":DERIVED_BY,
            "formula_oracle":oracle,"outcome_evaluation":"closed_form_only; no schedule or outcome enumeration",
            "schedule_metadata":{"memory_touching_instructions_per_thread":f"{sum(bool(i['memory_events']) for i in body)}*K",
                "order_preserving_instruction_schedules":{"unit":"complete schedules","formula":"multinomial(N*M*K; M*K, ..., M*K)"},
                "oracle_schedules":"exhaustive closed-form oracle; no sampling",
                "runtime_interleaving":{"owner":"S6-19","status":"not_run_by_S6-05",
                    "exhaustive_schedule_threshold":1_000_000,"sampling_seed_range_inclusive":[0,9_999],
                    "seed_rule":"Python random.Random(seed) shuffles available instruction-step successors; deterministic by seed, not a confidence claim",
                    "policy_status":"measured_by_S6-05; runtime use and result recording belong to S6-19"}}}
    exact={"kind":"exact_singleton","value":"N*K","complete":True}
    minimum="1 if K == 1 else 2"
    unlocked_range={"kind":"exact_integer_range","minimum":minimum,"maximum":"N*K","step":1,
                    "complete":True,"lost_updates_possible":True}
    split_range={"kind":"exact_integer_range","minimum":minimum,"maximum":"N*K","step":1,
                 "complete":True,"lost_updates_possible":True}
    return {"schema":"od.smp-litmus.v1","id":"COUNTERS-FORMULA","kind":"formula_counter_group",
        "sources":[SDM],"derived_by":DERIVED_BY,
        "description":"Parameterized finite x86 instruction loops; only the final counter outcome is evaluated by formula.",
        "cases":[
            make_case("LOCK-INC",[inc_locked],"f6",{
                "x86_tso":exact,"sc":exact,"interleaving_instruction_atomic":exact}),
            make_case("UNLOCKED-INC",[inc_plain],"f7",{
                "x86_tso":unlocked_range,"sc":unlocked_range,
                "interleaving_instruction_atomic":exact}),
            make_case("THREE-INSN-COUNTER",[load,add,store_reg],"ed",{
                "x86_tso":split_range,"sc":split_range,
                "interleaving_instruction_atomic":split_range})]}


def calibration_workload(threads_per_workload: int = 2, loads_per_thread: int = 12) -> dict[str, Any]:
    threads=[]
    for cpu in range(threads_per_workload):
        instructions=[]
        for _ in range(loads_per_thread):
            instructions.append({"op":"load","memory_events":[{"kind":"read","location":"x","result":"eax"}]})
        threads.append({"cpu":cpu,"initial_registers":{"eax":0},"instructions":instructions})
    return {"schema":"od.smp-calibration-workload.v1","id":"INTERLEAVING-STATE-CALIBRATION",
        "threads":threads,"memory":{"initial":{"x":0}},
        "observe":{"fields":[f"cpu{cpu}.eax" for cpu in range(threads_per_workload)]}}


def calibration_worker(limit: int) -> dict[str, Any]:
    workload=calibration_workload()
    stats={};start=time.perf_counter()
    outcomes,_=enumerate_model(workload,"interleaving_instruction_atomic",stats=stats,
                               deduplicate=False,stop_after_schedules=limit)
    elapsed=time.perf_counter()-start
    usage=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    rss_bytes=int(usage if platform.system()=="Darwin" else usage*1024)
    return {"elapsed_seconds":elapsed,**stats,"partial_outcome_count":len(outcomes),
        "peak_rss_bytes":rss_bytes,"input_sha256":hashlib.sha256(canonical_bytes(workload)).hexdigest()}


def seed_calibration_worker(seed_start: int, seed_end: int) -> dict[str, Any]:
    workload=calibration_workload();start=time.perf_counter();expanded=unique=pending=completed=0
    for seed in range(seed_start,seed_end+1):
        stats={}
        _,_=enumerate_model(workload,"interleaving_instruction_atomic",stats=stats,
                            deduplicate=False,stop_after_schedules=1,schedule_seed=seed)
        expanded+=stats["expanded_states"];unique+=stats["unique_states_stored"]
        pending+=stats["pending_frontier_states"];completed+=stats["completed_schedule_paths"]
    elapsed=time.perf_counter()-start
    usage=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    rss_bytes=int(usage if platform.system()=="Darwin" else usage*1024)
    return {"seed_start":seed_start,"seed_end":seed_end,"seeds":seed_end-seed_start+1,
            "elapsed_seconds":elapsed,"expanded_states":expanded,"unique_states_stored_sum":unique,
            "pending_frontier_states_sum":pending,"completed_schedule_paths":completed,
            "peak_rss_bytes":rss_bytes,"input_sha256":hashlib.sha256(canonical_bytes(workload)).hexdigest()}


def calibration(repeats: int = 3, limit: int = 1_000_000) -> dict[str, Any]:
    warm_workload=calibration_workload(threads_per_workload=2,loads_per_thread=5)
    warm_stats={};warm_start=time.perf_counter();enumerate_model(warm_workload,"interleaving_instruction_atomic",stats=warm_stats,deduplicate=False,stop_after_schedules=100)
    warm_elapsed=time.perf_counter()-warm_start
    worker_command=[sys.executable,str(Path(__file__).resolve()),"--schedule-worker",str(limit)]
    runs=[]
    for run in range(repeats):
        result=subprocess.run(worker_command,check=True,capture_output=True,text=True)
        row=json.loads(result.stdout);row["run"]=run+1;runs.append(row)
    times=[r["elapsed_seconds"] for r in runs];mean=sum(times)/len(times)
    max_rss=max(r["peak_rss_bytes"] for r in runs)
    practical=max(times)<=120 and max_rss<=2*1024**3
    seed_warm_command=[sys.executable,str(Path(__file__).resolve()),"--seed-worker","0","99"]
    seed_warm=subprocess.run(seed_warm_command,check=True,capture_output=True,text=True)
    seed_warm_result=json.loads(seed_warm.stdout)
    seed_runs=[]
    for run in range(repeats):
        result=subprocess.run([sys.executable,str(Path(__file__).resolve()),"--seed-worker","0","9999"],check=True,capture_output=True,text=True)
        row=json.loads(result.stdout);row["run"]=run+1;seed_runs.append(row)
    seed_times=[r["elapsed_seconds"] for r in seed_runs];seed_mean=sum(seed_times)/len(seed_times)
    seed_max_rss=max(r["peak_rss_bytes"] for r in seed_runs)
    seeds_practical=max(seed_times)<=120 and seed_max_rss<=2*1024**3 and all(r["completed_schedule_paths"]==10_000 for r in seed_runs)
    policy={"exhaustive_complete_schedule_threshold":limit if practical else None,
        "large_runtime_sampling_seed_range_inclusive":[0,9_999] if practical and seeds_practical else None,
        "seed_range_basis":"measured actual interleaver one complete path for every seed in three 0..9999 runs"}
    return {"schema":"od.stage6-litmus-calibration.v4","date":"2026-11-13",
      "candidate_gate":{"complete_order_preserving_schedules":limit,"repetitions":repeats},
      "workload":{"threads":2,"loads_per_thread":12,"distinct_write_back_cells":1,
        "order_preserving_instruction_schedules":math.comb(24,12),
        "method":"Actual enumerate_model(interleaving_instruction_atomic), deduplicate=False; execute each branch and count terminal complete schedule paths. The unique-state set and DFS frontier are retained for measured storage; no expected-outcome pruning."},
      "warmup":{"workload":"2x5 shared loads","completed_schedule_cap":100,
        "elapsed_seconds_excluded_from_runs":warm_elapsed,"state_metrics":warm_stats},
      "environment":{"platform":platform.platform(),"machine":platform.machine(),"processor":platform.processor(),
        "python":sys.version,"implementation":platform.python_implementation(),"memory_metric":"ru_maxrss converted to bytes"},
      "runs":runs,"summary":{"mean_seconds":mean,"min_seconds":min(times),"max_seconds":max(times),
        "relative_range":(max(times)-min(times))/mean if mean else 0,"median_seconds":sorted(times)[len(times)//2],
        "max_peak_rss_bytes":max_rss},
      "seed_sampling_calibration":{"candidate_seed_range_inclusive":[0,9_999],"repetitions":repeats,
        "warmup":{"range_inclusive":[0,99],"result":seed_warm_result,"elapsed_excluded":True},
        "method":"For each seed, run the actual instruction-atomic interleaver with deterministic seeded successor shuffling and stop at one completed schedule. Each worker retains real model states and executes instructions; one worker handles a whole seed range so ru_maxrss covers the full repeat.",
        "runs":seed_runs,"summary":{"mean_seconds":seed_mean,"min_seconds":min(seed_times),"max_seconds":max(seed_times),
          "relative_range":(max(seed_times)-min(seed_times))/seed_mean if seed_mean else 0,
          "max_peak_rss_bytes":seed_max_rss},"status":"measured_within_budget" if seeds_practical else "exceeds_budget_or_incomplete"},
      "proposed_policy":policy,"corpus_acceptance":"not_accepted_pending_independent_review",
      "conclusion":(f"The {limit:,}-complete-schedule gate completed within the recorded time and memory budget. Propose this as the exhaustive schedule threshold. The separate 10,000-seed calibration is required before accepting the seed range." if practical else f"The {limit:,}-complete-schedule gate exceeded the explicit 120-second/2-GiB practicality bound. Do not accept this threshold; run a lower measured complete-schedule candidate.")}


def main() -> int:
    parser=argparse.ArgumentParser();parser.add_argument("--output",type=Path,default=OUT);parser.add_argument("--hash",action="store_true");parser.add_argument("--calibrate",action="store_true");parser.add_argument("--calibration-limit",type=int,default=1_000_000);parser.add_argument("--schedule-worker",type=int);parser.add_argument("--seed-worker",nargs=2,type=int);args=parser.parse_args()
    if args.schedule_worker is not None:
        print(json.dumps(calibration_worker(args.schedule_worker),sort_keys=True));return 0
    if args.seed_worker is not None:
        print(json.dumps(seed_calibration_worker(*args.seed_worker),sort_keys=True));return 0
    if args.calibrate:
        print(json.dumps(calibration(limit=args.calibration_limit),sort_keys=True,indent=2));return 0
    print(generate(args.output));return 0

if __name__=="__main__": raise SystemExit(main())
