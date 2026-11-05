# Stage 4 stack probe fixture

This freestanding ELF fixture preserves the fixed S4-32b board and image
layout. The raw image is loaded at GPA `0x1000`; it contains the six page-table
pages, code at GPA `0x7000`/GVA `0x400000`, and one payload qword at GPA
`0x9000`/GVA `0x800000`. The page-mapped stack page is GPA `0xa000..0xafff`
and is outside the raw image.

The exact entry bytes are `51 58 ee f4`: `PUSH RCX; POP RAX; OUT DX,AL; HLT`.
The loader obtains the RCX value from the payload qword and begins with
`RAX=0`, `RSP=0xa01000`, Long64/CPL0 and the existing fixed CR3/EFER handoff.
PUSH writes the qword at GPA `0xaff8`; POP reads it back and restores RSP.

This fixture exercises only the existing bounded paged interpreter quantum.
It does not test exception delivery/repair, IRQ/APIC, HLT wakeup, JIT, Linux,
physical hardware, or full S4-32 acceptance. ISSUE-054 remains open.
