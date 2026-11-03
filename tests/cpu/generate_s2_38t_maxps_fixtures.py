#!/usr/bin/env python3
"""Materialize exact MAXPS m128 unmasked IE/DE event and RC fixtures."""
from __future__ import annotations
import copy
import itertools
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2] / "tests/fixtures/x86/stage2"
PREFIX = "s2_38t_maxps_m128_"
# Each class is (lhs, memory RHS); MAXPS operates on those raw binary32 bits.
CLASSES = {
    "n": (0x3F800000, 0x40000000),
    "i": (0x7FC00001, 0x40000000),
    "d": (0x00000001, 0x40000000),
    "id": (0x7FC00001, 0x00000001),
}
SPECIALS = {
    "snan_memory_rhs_lane3": ((0x3F800000, 0x3F800000, 0x3F800000, 0x3F800000), (0x40000000, 0x40000000, 0x40000000, 0x7F800042)),
    "qnan_memory_rhs_lane2": ((0x3F800000, 0x3F800000, 0x3F800000, 0x3F800000), (0x40000000, 0x40000000, 0x7FC01234, 0x40000000)),
    "subnormal_lane3": ((0x3F800000, 0x3F800000, 0x3F800000, 1), (0x40000000, 0x40000000, 0x40000000, 0x40000000)),
    "max_vs_min": ((0xBF800000, 0xBF800000, 0xBF800000, 0xBF800000), (0x3F800000, 0x3F800000, 0x3F800000, 0x3F800000)),
    "zero_tie_rhs": ((0, 0x3F800000, 0, 0x3F800000), (0x80000000, 0x3F800000, 0, 0x3F800000)),
}

def nan(v: int) -> bool: return v & 0x7F800000 == 0x7F800000 and v & 0x7FFFFF != 0
def sub(v: int) -> bool: return v & 0x7F800000 == 0 and v & 0x7FFFFF != 0

def rewrite(template: dict, name: str, left: tuple[int, ...], right: tuple[int, ...], im: int, dm: int, osx: int, rc: int=0, sticky: int=0) -> dict:
    value=copy.deepcopy(template); value["case_id"]=PREFIX+name
    value["profile"]="s2-long-maxps-memory-unmasked-rax-v1"
    value["provenance"]["source"]=f"Project-generated literal S2-38t input {name}."
    value["provenance"]["specification_version"]="Intel SDM Rev. 093; bounded local S2-38t."
    value["provenance"]["specification"]="Intel SDM Rev. 093 Vol. 1 §§4.9.2, 11.5.3.1–3; Vol. 2B MAXPS pp. 4-8–4-10."
    value["memory"][0]["length"]=3; value["memory"][0]["initial_bytes_hex"]="0f5f00"; value["memory"][0]["comparison_mask_hex"]="ffffff"
    value["stop_rip"]="0x0000000000100003"
    sse=value["initial_state"]["sse"]
    sse["mxcsr"]=f"0x{(0x1E00|(im<<7)|(dm<<8)|(rc<<13)|sticky):016x}"
    for lane,bits in enumerate(left):
        xmm=bytearray.fromhex(sse["xmm"][0]); xmm[lane*4:lane*4+4]=bits.to_bytes(4,"little"); sse["xmm"][0]=xmm.hex()
    sse["xmm"][1]="4200007f010000800000c07f00000080"
    value["memory"][1]["initial_bytes_hex"]=b"".join(v.to_bytes(4,"little") for v in right).hex()
    old=int(value["initial_state"]["rflags"],16); value["initial_state"]["rflags"]=f"0x{old|(1<<18):016x}"
    value["maxpd_memory_rax_controls"]=None
    value["maxpd_unmasked_controls"]=None
    value["maxps_memory_rax_controls"]=None
    value["maxps_unmasked_controls"]={"sse_available":True,"cr0_em":False,"cr0_ts":False,"cr4_osfxsr":True,"cr4_osxmmexcpt":bool(osx)}
    event=any((nan(l) and not im) or (nan(r) and not im) or (sub(l) and not dm) or (sub(r) and not dm) for l,r in zip(left,right))
    value["expected_exit"]="exception" if event else "stop_address"
    return value

def write(v: dict) -> None: (ROOT/f"{v['case_id']}.case.json").write_text(json.dumps(v,indent=2)+"\n",encoding="utf-8")

def main() -> None:
    template=json.loads((ROOT/"s2_38r_maxpd_m128_rc0_sticky_events03.case.json").read_text())
    for classes,masks in itertools.product(itertools.product(CLASSES, repeat=4),itertools.product((0,1),repeat=3)):
        im,dm,osx=masks; left=tuple(CLASSES[c][0] for c in classes); right=tuple(CLASSES[c][1] for c in classes)
        write(rewrite(template,"matrix_"+"_".join(classes)+f"_im{im}_dm{dm}_os{osx}",left,right,im,dm,osx))
    for name,(left,right) in SPECIALS.items(): write(rewrite(template,"special_"+name,left,right,1,1,1,rc=3))
    ac0 = rewrite(template,"special_ac0_aligned",(0x3F800000,)*4,(0x40000000,)*4,1,1,1,rc=2)
    ac0["initial_state"]["rflags"] = f"0x{(int(ac0['initial_state']['rflags'],16) & ~(1<<18)):016x}"
    write(ac0)
    unaligned = rewrite(template,"fault_unaligned_ac0",(0x3F800000,)*4,(0x40000000,)*4,0,0,1)
    unaligned["initial_state"]["rflags"] = f"0x{(int(unaligned['initial_state']['rflags'],16) & ~(1<<18)):016x}"
    unaligned["initial_state"]["gpr"][0] = "0x0000000000002001"
    unaligned["memory"][1]["guest_address"] = "0x0000000000002001"
    unaligned["expected_exit"] = "exception"
    write(unaligned)
    noncanonical = rewrite(template,"fault_noncanonical_span",(0x3F800000,)*4,(0x40000000,)*4,0,0,1)
    noncanonical["initial_state"]["gpr"][0] = "0x0000800000000000"
    noncanonical["memory"][1]["guest_address"] = "0x0000800000000000"
    noncanonical["expected_exit"] = "exception"
    write(noncanonical)
    # A raw event in lane 3 with IM/DM independently unmasked anchors #XM and arithmetic #UD.
    left=(0x3F800000,0x3F800000,1,0x7FC00001); right=(0x40000000,)*4
    for osx in (0,1): write(rewrite(template,f"literal_lane23_ie_de_os{osx}",left,right,0,0,osx))
    gp_event = rewrite(template,"fault_unaligned_ie_de",(0x3F800000,0x3F800000,1,0x7FC00001),(0x40000000,)*4,0,0,1)
    gp_event["initial_state"]["gpr"][0] = "0x0000000000002001"
    gp_event["memory"][1]["guest_address"] = "0x0000000000002001"
    gp_event["expected_exit"] = "exception"
    write(gp_event)
    gate_event = rewrite(template,"gate_nm_ie_de",(0x3F800000,0x3F800000,1,0x7FC00001),(0x40000000,)*4,0,0,1)
    gate_event["maxps_unmasked_controls"]["cr0_ts"] = True
    gate_event["expected_exit"] = "exception"
    write(gate_event)
    left=(0x7FC00001,1,0x3F800000,0x3F800000); right=(0x40000000,)*4
    for rc,sticky in itertools.product(range(4),range(64)):
        write(rewrite(template,f"rc{rc}_sticky_events{sticky:02x}",left,right,1,1,1,rc,sticky))

if __name__=="__main__": main()
