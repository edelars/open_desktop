# S2-37dk — masked-only SUBSS XMM0,m32 [RAX]

## Scope

Added the strict schema-v2 interpreter-only profile `s2-long-subss-memory-rax-v1` for exact legacy `F3 0F 5C 00`. It fetches four bytes, executes one instruction, and stops at entry+4. The profile requires Long64, SSE bit 25, CR0.EM=0, CR4.OSFXSR=1, CR0.TS=0 and XMM/MXCSR state. Full fetch precedes gates, with #UD before #NM.

Admission requires MXCSR reserved bits clear, all six exception masks set, DAZ=FTZ=0, and only supported bits (`0xffbf`). Any RC and old sticky IE/DE/ZE/OE/UE/PE are accepted. Unmasked exceptions and #XM remain closed. AC=1 returns local Unsupported before address-span validation and data read; AC=0 permits unaligned m32. Checked RAX+3 and canonical48 endpoints are checked before one read4. Read failure is atomic for CPU/SSE/MXCSR/RIP and retirement.

The result is old XMM0.low32 minus actual memory RHS. XMM0.high96, XMM1 and unrelated state are preserved; generated IE/DE/OE/UE/PE are ORed into prior stickies. Original NaNs are classified and selected before arithmetic handling of subtraction's RHS sign; SNaNs are quieted and raise IE, while a QNaN alone does not. Same-sign infinity subtraction returns masked indefinite and raises IE. JIT/native/C ABI/generic/fragment and unmasked/#XM routes are denied.

## Evidence

The literal handwritten fixture poisons XMM1 and XMM0.high96. The independent `independent_subss` oracle consumes memory RHS using raw-u32 integer arithmetic without host floating point or production helpers. The end-to-end matrix executes 12 operand pairs × four RC modes × all 64 old sticky histories (3,072 executions), including both SNaN/QNaN orders, zeros, subnormal/tiny values, infinities and overflow. Direct ISA checks cover exact decode, gate/fetch priority, AC refusal, unaligned one-read access, canonical endpoint and overflow, MXCSR refusal, and failed-read rollback. Runner fixture checks cover strict profile/schema, output/result validation and JIT/native denial; the existing SUBSS register fixture remains exercised separately.

Focused exact-decoder, direct gate/read-rollback, fixture/backend-denial and oracle-matrix tests passed. Full component suites passed: `od-isa` 1,798; `od-testkit` 455; `od-runner` 114 unit + 492 fixture tests; `od-x86-runner` 8 library + 4 binary tests. `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`, and `git diff --check` passed. `python3 tools/check_layout.py` found no S2-37dk path/link problems after correcting its fixture-note link, but continues to report the repository-root `.git` mapping and two pre-existing links to absent Stage 1/Stage 2 oracle manifests. Graphify AST refresh passed (11,279 nodes, 24,237 edges, 347 communities). Independent Sol/high review is pending. No physical x86 acceptance is claimed.

Sources: Intel SDM Rev. 093 Vol. 2B `SUBSS` pp. 4-695–4-696 and Vol. 2A Table 2-20; Vol. 1 §§4.9.1.4–.6, 11.5.2–11.5.3 and DAZ/FTZ; Vol. 3A Event 19/Table 7-8. [Intel SDM downloads](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html), [Rev. 093 Vol. 2B PDF](https://cdrdv.intel.com/v1/dl/getContent/671199).
