"""CPU_CONTRACT status rules for the S4-38 L2 instruction inventory.

Statuses (hand-assessed against docs/specs/CPU_CONTRACT.md and, where the contract is silent, the
decoder `crates/od-isa/src/long_decode.rs`; nothing here is derived from execution):

* `exact`       - a section of the contract names this exact encoding (opcode, ModRM shape, prefixes)
                  as an admitted form; the profile preconditions are given in `evidence`.
* `family`      - the contract admits the opcode family broadly (ModRM/SIB/RIP-relative, widths
                  8/16/32/64), so this operand shape is inside the family. Shape-level checks
                  (register, displacement) were not repeated per form.
* `partial`     - an exact form exists, but only for another operand, value domain or privilege level
                  than the one on the boot path. Counts as "no usable form" for planning.
* `absent`      - no section of the contract and no decoder entry admits the form.
* `not_checked` - not assessed; a form without a matching rule ends here.

Rules are ordered; the first match wins. Matching is on the family string built by the inventory tool:
`[prefixes] [W] mnemonic operand-classes` (memory operands collapse to `m`).
"""

from __future__ import annotations

import re

CONTRACT = "docs/specs/CPU_CONTRACT.md"


def _ev(*refs: str) -> str:
    return "; ".join(refs)


# (regex on family, status, evidence)
RULES: list[tuple[str, str, str]] = [
    # --- exact / system forms -------------------------------------------------------------------
    (r"^cld $", "exact", _ev(f"{CONTRACT}:1082,11282 S2-18j bare CLD FC")),
    (r"^std $", "exact", _ev(f"{CONTRACT}:1082,11282 S2-18j bare STD FD")),
    (r"^cli $", "exact", _ev(f"{CONTRACT}:13909 S2-27h exact CLI")),
    (r"^sti $", "exact", _ev(f"{CONTRACT}:13941 S2-27i exact STI")),
    (r"^hlt $", "exact", _ev(f"{CONTRACT}:14059 S2-27j exact HLT")),
    (r"^cpuid $", "exact", _ev(f"{CONTRACT}:436-465 DiagnosticV1: bare 0F A2, answers for leaves 0,1,0x80000000..8 only")),
    (r"^W retfq $", "exact", _ev(f"{CONTRACT}:14015-14057 S2-27l exact 48 CB same-CPL0 with explicit GDTR and valid CS descriptor")),
    (r"^mov r64,creg$", "partial", _ev(f"{CONTRACT}:14103-14192 S2-25a/b/c: 0F 20 C0 (CR0) and 0F 20 E0 (CR4) only inside the FP-init profiles with a four-value CR4 domain")),
    (r"^mov creg,r64$", "partial", _ev(f"{CONTRACT}:14103-14192 S2-25a/b/c: 0F 22 C0 / 0F 22 E0 only inside the FP-init profiles; CR4 writes outside four values are Unsupported")),
    (r"^wrmsr $", "partial", _ev(f"{CONTRACT}:493-522 GsBaseV1: WRMSR only for ECX=0xC0000101; SysenterProbeV1 only 0x174..0x176; other MSRs Unsupported")),
    (r"^rdmsr $", "partial", _ev(f"{CONTRACT}:14228-14242 S2-25e: RDMSR only IA32_EFER with a four-value domain; GsBaseV1 only 0xC0000101")),
    (r"^popfq $", "partial", _ev(f"{CONTRACT}:4421-4436 S2-17o: bare 9D only at the runner default CPL3/IOPL0; other POPF privilege levels out of profile")),
    (r"^pushfq $", "exact", _ev(f"{CONTRACT}:4431 S2-17p bare 9C PUSHFQ")),
    (r"^fninit $", "exact", _ev(f"{CONTRACT}:2761 S2-30a bare FNINIT DB E3")),
    (r"^fnstcw m$", "partial", _ev(f"{CONTRACT}:6208 S2-31g FNSTCW [RAX] (D9 38) only")),
    (r"^fnstsw m$", "partial", _ev(f"{CONTRACT}:6244 S2-31h FNSTSW [RAX] (DD 38) only")),
    (r"^mfence $", "exact", _ev(f"{CONTRACT}:1083 S2-21g bare MFENCE on opted-in private synchronous RAM")),
    (r"^rdtsc $", "partial", _ev(f"{CONTRACT}:602-627 RdtscSourceV1 and S4-37a6a explicit time source; not a general RDTSC")),
    # --- absent ---------------------------------------------------------------------------------
    (r"^REP endbr64 $", "absent", _ev("no ENDBR64 / F3 0F 1E FA in the contract (grep) and no 0F 1E decoder entry in long_decode.rs")),
    (r"^REP pause $", "not_checked", _ev("no PAUSE in the contract; F3 90 handling in long_decode.rs (90..97 arm) not traced")),
    (r"^leave $", "absent", _ev("no LEAVE (C9) in the contract; C9 is not in the primary-opcode allowlist of long_decode.rs:2425")),
    (r"nop m$", "absent", _ev("multi-byte NOP 0F 1F /0 is not in the contract (grep) nor in long_decode.rs; typically alignment padding")),
    (r"^66 nop $", "not_checked", _ev("66 90 not assessed; contract row 1371 covers bare 90 only")),
    (r"^nop $", "family", _ev(f"{CONTRACT}:1371 NOP 90 preserves all GPR")),
    (r"^(REP W |REP )?(tzcnt|lzcnt)", "absent", _ev(f"{CONTRACT}:1080,2409-2410 F3-prefixed LZCNT/TZCNT remain outside the baseline")),
    (r"^(shlx|shrx|andn|bzhi)", "absent", _ev("BMI1/BMI2 VEX forms: no mention in the contract (grep SHLX/SHRX/ANDN/BZHI = 0)")),
    (r"^W rdrand", "absent", _ev("RDRAND: no mention in the contract (grep = 0)")),
    (r"^prefetch", "absent", _ev("PREFETCHh: no mention in the contract (grep = 0)")),
    (r"^clflush", "absent", _ev(f"{CONTRACT}:638 only lists CLFLUSH as a neutral-profile feature, no execution contract")),
    (r"^(REP vmmcall|REPNE pvalidate|REP rmpadjust|66 tdcall)", "absent", _ev("SEV/TDX instruction: no mention in the contract (grep = 0)")),
    (r"^xchg m,r8$", "absent", _ev(f"{CONTRACT}:1368 memory XCHG is only REX.W 87 /r qword (S2-21a); 86 /r memory byte form is not admitted")),
    # --- families -------------------------------------------------------------------------------
    (r"^(REP )?(W )?(movsb|movsd|movsq|stosb|stosd|stosq|cmpsb)", "family", _ev(f"{CONTRACT}:1082,10724-11514 S2-18/19 REP MOVS/STOS/CMPS (bounded 1024 iterations per invocation, DF honoured)")),
    (r"^(W |66 )?(mov|movzx|movsx|movsxd|movabs|lea) ", "family", _ev(f"{CONTRACT}:1076,1320-1389 S2-08/09/10 MOV 88-8B,B0-BF,C6/C7, MOVZX/MOVSX/MOVSXD, LEA")),
    (r"^(66 )?(W )?(add|adc|sbb|sub|cmp|inc|dec|neg) ", "family", _ev(f"{CONTRACT}:1077,2179-2216 S2-11 arithmetic 8-64, groups 80/81/83, INC/DEC FE/FF, NEG F6/F7")),
    (r"^(66 )?(W )?(and|or|xor|test|not) ", "family", _ev(f"{CONTRACT}:1077,2230-2266 S2-12 logic, TEST, NOT")),
    (r"^(66 )?(W )?(shl|shr|sar|rol|ror|rcl|rcr) ", "family", _ev(f"{CONTRACT}:1078,2270-2304 S2-13 shifts and rotates, imm/CL/1")),
    (r"^(W )?(mul|imul|div|idiv) ", "family", _ev(f"{CONTRACT}:1079,2308-2352 S2-14 MUL/IMUL/DIV/IDIV, IMUL 0F AF/69/6B")),
    (r"^W cdqe $", "family", _ev(f"{CONTRACT}:1079,2385 S2-15a sign-extension 98/99")),
    (r"^W (xchg) r64,r64$", "family", _ev(f"{CONTRACT}:1368 register XCHG 87 /r")),
    (r"^(W )?(bt|bts|btr|btc|bsr|bsf) ", "family", _ev(f"{CONTRACT}:1080,2383-2411 S2-15 BT-family register/memory/immediate, BSF/BSR register and memory sources")),
    (r"^j[a-z]+ rel$", "family", _ev(f"{CONTRACT}:1081 S2-16a all 16 short/near Jcc and EB/E9 JMP")),
    (r"^jmp (r64|m)$", "family", _ev(f"{CONTRACT}:1081 S2-16d/e FF /4 register and ordinary memory r/m64")),
    (r"^call rel$", "family", _ev(f"{CONTRACT}:1081 S2-17c near relative CALL")),
    (r"^call (r64|m)$", "family", _ev(f"{CONTRACT}:1081 S2-17d/e FF /2 register and ordinary memory r/m64")),
    (r"^ret $", "family", _ev(f"{CONTRACT}:1081 RET C3/C2 row; long_decode.rs:18147 bare C3 decodes to Return; S2-17f adds C2")),
    (r"^push (r64|imm|m)$", "family", _ev(f"{CONTRACT}:1081,1191-1199 S2-17a/b/i PUSH r64, imm8/imm32, qword memory FF /6")),
    (r"^pop r64$", "family", _ev(f"{CONTRACT}:1081,1191 S2-17a POP r64 (50-5F, REX.B)")),
    (r"^(W )?(set[a-z]+) r8$", "family", _ev(f"{CONTRACT}:1081 S2-16b all 16 SETcc byte forms")),
    (r"^(W )?(cmov[a-z]+) ", "family", _ev(f"{CONTRACT}:1081 S2-16c all 16 CMOVcc forms incl. memory source")),
]

_COMPILED = [(re.compile(p), s, e) for p, s, e in RULES]


def _exact_form_status(form: dict) -> dict | None:
    """Byte-level rules for instructions whose contract slices admit one exact encoding."""
    mnemonic, raw = form["mnemonic"], form["example_bytes"]
    if mnemonic == "lgdt":
        if re.fullmatch(r"0f0155[0-9a-f]{2}", raw):
            return {"status": "exact", "evidence": f"{CONTRACT}:1792-1819 S2-27a exact 0F 01 55 ib = LGDT [RBP+disp8]"}
        return {
            "status": "partial",
            "evidence": f"{CONTRACT}:1792-1819 S2-27a admits only 0F 01 55 ib (LGDT [RBP+disp8]); this ModRM shape is not admitted",
        }
    if mnemonic == "lidt":
        if raw == "0f015c2406":
            return {"status": "exact", "evidence": f"{CONTRACT}:1945 S2-27d exact LIDT [RSP+6] (0F 01 5C 24 06)"}
        return {
            "status": "partial",
            "evidence": f"{CONTRACT}:1945 S2-27d admits only LIDT [RSP+6]; this shape is not admitted",
        }
    if mnemonic == "wrmsr" and form["region"] == "kernel_startup_64_to_cpuid1" and form.get("first_function") == "startup_64":
        return {
            "status": "exact",
            "evidence": f"{CONTRACT}:493-522 GsBaseV1: bare 0F 30 with ECX=0xC0000101 (MSR_GS_BASE) and EAX=EDX=0 at kernel startup_64 (head_64.S:69-72), CPL0, canonical value",
        }
    if form["operands"][:1] == ["sreg"]:
        if raw in ("8ed8", "8ec0"):
            return {
                "status": "exact",
                "evidence": f"{CONTRACT}:2147-2167 S2-22k exact bare 8E D8 (DS) / 8E C0 (ES), only with AX == 0",
            }
        if raw == "8ed0":
            return {"status": "absent", "evidence": "MOV SS,AX (8E D0): no profile in the contract (S2-22k covers DS/ES only)"}
        if raw in ("8ee0", "8ee8"):
            return {
                "status": "partial",
                "evidence": f"{CONTRACT}:1665-1790 S2-22f/h are CPL3 GDT-only FS/GS loads; CPL0 null FS/GS (8E E0 / 8E E8) not admitted",
            }
    return None


def status_for(form: dict) -> dict:
    exact = _exact_form_status(form)
    if exact is not None:
        return exact
    family = form["family"]
    for pattern, status, evidence in _COMPILED:
        # Rules for operand-less forms end in a space ("cld "), the others in the operand classes.
        if pattern.search(family) or pattern.search(family + " "):
            note = None
            if status == "absent" and family.endswith("nop m"):
                note = "alignment padding reached by fallthrough after a noreturn call; likely not executed"
            result = {"status": status, "evidence": evidence}
            if note:
                result["note"] = note
            return result
    return {"status": "not_checked", "evidence": None}
