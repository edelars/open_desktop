# Собственные schema v2 fixtures этапа 2

### S2-38aj — unmasked DIVSD m64 memory profile

Five handwritten literals cover exact `F2 0F 5E 00`: one success case and
unmasked IE, ZE, DE, and PE delivery. The separate schema-v2 profile requires
read-only m64 `[RAX]`, SSE2, `DAZ=FTZ=0`, and `OM=UM=1`, while admitting all RC,
IM/DM/ZM/PM masks, prior sticky histories, and OSXMMEXCPT routes. An in-process
interpreter matrix checks 163,840 executions against the independent integer
raw-u64 oracle using the actual memory RHS. Fixture tests cover events,
status/control tampering, and JIT/native denial. This is host-side interpreter
evidence, not physical x86 acceptance. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38aj--exact-divsd-memory-profile-with-unmasked-exceptions),
[plan](../../../../docs/plans/STAGE_2.md#s2-38aj--exact-divsd-memory-form-with-unmasked-exceptions),
and [report](../../../../docs/reports/S2_38AJ_DIVSD_MEMORY_UNMASKED.md).

### S2-38ah — unmasked MULSD m64 memory profile

`s2_38ah_mulsd_m64_memory_unmasked_rax.case.json` is the handwritten success
literal for exact `F2 0F 59 00` with an unaligned, read-only m64 source at
`[RAX]`. Three further literal event fixtures cover unmasked IE (`ie_snan_xm`),
DE (`de_subnormal_ud`), and PE (`pe_inexact_ud`).
Its separate schema-v2 profile permits every RC, IM/DM/ZM/PM and prior sticky
history, while requiring `DAZ=FTZ=0` and `OM=UM=1`. Runner checks use the
testkit's independent raw-u64 integer oracle, verify unmasked IE/DE before
computation and PE after computation, and keep the destination/RIP/retirement
unchanged on arithmetic events. Native/JIT routes remain denied. The fixture is
host-side interpreter evidence, not physical x86 acceptance. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38ah--exact-mulsd-memory-profile-with-unmasked-exceptions),
[plan](../../../../docs/plans/STAGE_2.md#s2-38ah--exact-mulsd-memory-form-with-unmasked-exceptions),
and [report](../../../../docs/reports/S2_38AH_MULSD_MEMORY_UNMASKED.md).

## S2-26e RDTSC with injected source

`s2_26e_rdtsc_source.case.json` and `s2_26e_rdtsc_tsd_fault.case.json` are
handwritten schema-v2 fixtures for exact bare Long64 `0F 31` under
`s2-long-rdtsc-v1`. Each fixes a deterministic 64-bit source value and an
expected source-call count. The first case uses `CR4.TSD=0` at the runner's
ring-3 CPL and checks zero-extended EAX/EDX output plus one call. The second
uses `CR4.TSD=1` and checks atomic `#GP(0)` with zero source calls. Core unit
tests cover TSD both ways at CPL0–3, legacy `Unsupported`, prefix rejection,
and stop/budget ordering. JIT/native runner admission is denied; the C ABI
fragment-create path rejects RDTSC because it has no explicit profile/source
inputs. No host clock, physical execution, or `#GP` delivery is modeled. Source: Intel SDM
revision 093, Vol. 2B, RDTSC, pp. 4-559–560
([official Intel PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf)).
See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#локальный-профиль-rdtscsourcev1-s2-26e),
[plan](../../../../docs/plans/STAGE_2.md#s2-26e--точный-rdtsc-с-явным-tsc-источником),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-26e--точный-rdtsc-с-явным-tsc-источником).

## S2-26d SEP-disabled SYSENTER/SYSEXIT

`s2_26d_sysenter_sep_disabled.case.json` and
`s2_26d_sysexit_sep_disabled.case.json` are handwritten literal schema-v2
fixtures for exact bare Long64 `0F 34` and `0F 35` under the explicit
`s2-long-sep-disabled-v1` profile. They expect a CPU-side #UD with no error
code, zero retirement, and unchanged GPR/RIP/RFLAGS/code memory. The profile
represents the selected synthetic target with SEP clear; it does not model
exception delivery, Linux fixup, or hardware observation. The runner checks
both fixtures on the interpreter and denies JIT. Local unit coverage checks
CPL0–3, fetch truncation and fetch-fault preservation, prefix/neighbour
fail-closed behavior, and stop/zero-budget priority. Intel SDM revision 093,
Vol. 2B, documents SYSENTER/SYSEXIT opcode forms and the SEP presence flag
([official Intel PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf)).
See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#локальная-политика-sepdisabledv1-s2-26d),
[plan](../../../../docs/plans/STAGE_2.md#s2-26d--local-sepdisabledv1-sysentersysexit-policy),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-26d--local-sepdisabledv1-sysentersysexit-policy).

### S2-37a local legacy RSQRTSS register profile

`s2_37a_rsqrtss_register.case.json` is a schema-v2 XMM8/XMM9 fixture for the
exact `F3 45 0F 52 C1` register encoding. Its XMM9 low dword contains `4.0`,
which produces the local raw-helper result `0.5`; the runner checks preservation
of the destination upper 96 bits and other state. The profile assumes SSE and
CR0/CR4 permission and performs no data-memory access. It does not model CPUID,
SIMD FP exception/MXCSR behavior or guest FP flags. JIT/native and physical
execution are closed; hardware approximation-envelope validation is deferred
to the combined final Mac mini substage. See the [S2-37a plan](../../../../docs/plans/STAGE_2.md#s2-37a--local-long64-register-only-legacy-rsqrtss).

### S2-37b local legacy RCPSS register profile

`s2_37b_rcpss_register.case.json` is a schema-v2 XMM8/XMM9 fixture for the
exact `F3 45 0F 53 C1` register encoding. XMM9 contains `2.0` and XMM8 receives
the deterministic local helper result `0.5`; the runner checks that only the
destination low dword changes. The helper rounds exact reciprocal nearest-ties-
even and flushes a subnormal rounded output. ISSUE-025 leaves the published
hardware tiny-output threshold ambiguous, so the fixture value is not a
universal hardware-bit assertion. The local profile assumes SSE and CR0/CR4
permission, does not access guest data memory, and keeps JIT/native closed.
Hardware checks validate the `3/8192` error envelope for non-flushed normal
outputs in the combined final Mac mini substage. See [S2-37b plan](../../../../docs/plans/STAGE_2.md#s2-37b--local-long64-register-only-legacy-rcpss) and [ISSUE-025](../../../../docs/KNOWN_ISSUES.md#issue-025--intel-rcpssrcpps-threshold-inequalities-overlap).

### S2-37c local legacy RSQRTPS register profile

`s2_37c_rsqrtps_register.case.json` is a schema-v2 XMM8/XMM9 fixture for the
exact `45 0F 52 C1` register encoding. Its four XMM9 lanes are `4.0`, `-0`,
`+∞`, and `-1`; literal local outputs are `0.5`, `-∞`, `+0`, and the negative
indefinite QNaN. The interpreter updates all four destination lanes, preserves
all other architectural state and makes no data-memory access. The profile
assumes SSE and CR0/CR4 permission; it does not model CPUID, #UD/#NM, MXCSR or
SIMD exception behavior. JIT/native remain denied. Final Mac mini checks use the
`3/8192` relative-error envelope for non-flushed normal outputs, not exact
approximate bits. See the [S2-37c plan](../../../../docs/plans/STAGE_2.md#s2-37c--local-long64-register-only-legacy-rsqrtps).

### S2-37d local legacy RCPPS register profile

`s2_37d_rcpps_register.case.json` is a schema-v2 XMM8/XMM9 fixture for exact
`45 0F 53 C1`. Its XMM9 lanes contain `2.0`, `-0`, `+∞`, and `-2`; independent
literal local results are `0.5`, `-∞`, `+0`, and `-0.5`. The interpreter
replaces all four destination lanes while preserving all other state and makes
no data-memory access. It assumes SSE and CR0/CR4 permission; CPUID, MXCSR
exception behavior, JIT and native execution remain outside the profile.
The deterministic helper policy does not claim exact hardware result bits or a
universal tiny-result cutoff. ISSUE-025 remains open. Final Mac mini checks use
the `3/8192` relative-error envelope for non-flushed normal outputs and handle
the tiny-result boundary separately. See the [S2-37d plan](../../../../docs/plans/STAGE_2.md#s2-37d--local-long64-register-only-legacy-rcpps) and [ISSUE-025](../../../../docs/KNOWN_ISSUES.md#issue-025--intel-rcpssrcpps-threshold-inequalities-overlap).

### S2-37dz local legacy RSQRTPS memory profile

`s2_37dz_rsqrtps_m128_memory_rhs.case.json` is a handwritten schema-v2 fixture
for exact legacy `0F 52 00`, sourcing all result lanes from one aligned m128
read at `[RAX]`. The literal memory values `4.0`, `-0`, `+∞`, and `-2` produce
local results `0.5`, `-∞`, `+0`, and the indefinite QNaN. Poisoned XMM0 checks
full replacement; other vector registers and guest memory remain unchanged.
A 4,096-run matrix covers both signs at every subnormal leading-bit position,
signed NaN payload handling, all four RC settings, and every prior sticky
history. The independent integer/rational validator enforces the `3/8192`
normal-result error envelope without asserting hardware bit equality. See the
[S2-37dz plan](../../../../docs/plans/STAGE_2.md#s2-37dz--local-long64-memory-legacy-rsqrtps-xmm0m128-rax),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-37dz-memory-rsqrtps-at-rax),
[standalone report](../../../../docs/reports/S2_37DZ_RSQRTPS_MEMORY.md), and
[ISSUE-025](../../../../docs/KNOWN_ISSUES.md#issue-025--intel-rcpssrcpps-threshold-inequalities-overlap).

### S2-37dy local legacy RCPPS memory profile

`s2_37dy_rcpps_m128_memory_rhs.case.json` is a handwritten schema-v2 fixture
for exact `0F 53 00`, sourcing all four result lanes from one aligned m128 read
at `[RAX]`. It uses memory lanes `2.0`, `-0`, `+∞`, and `-2`, with literal local
results `0.5`, `-∞`, `+0`, and `-0.5`; poisoned XMM0 and adjacent-memory bytes
check complete replacement and the read width. The deterministic S2-36ab
reference defines these values. ISSUE-025 remains open, so the fixture makes no
hardware-bit or universal tiny-result-boundary claim. See the [S2-37dy plan](../../../../docs/plans/STAGE_2.md#s2-37dy--local-long64-memory-legacy-rcpps-at-rax) and [ISSUE-025](../../../../docs/KNOWN_ISSUES.md#issue-025--intel-rcpssrcpps-threshold-inequalities-overlap).

### S2-37ea local legacy RCPSS memory profile

`s2_37ea_rcpss_m32_memory_rhs.case.json` is a handwritten schema-v2 fixture
for exact `F3 0F 53 00`. The four-byte source starts at unaligned `[RAX]` and
contains `2.0`, producing the local deterministic `0.5` result. Poisoned XMM0
checks that only the low dword changes; the high 96 bits and adjacent memory
canary remain intact. AC=1 is refused before the data callback; strict MXCSR
checks require all six exception masks with DAZ/FTZ clear while allowing RC and
prior sticky histories. ISSUE-025 remains open, so no exact hardware bits or
universal tiny-result cutoff are claimed. See the [S2-37ea plan](../../../../docs/plans/STAGE_2.md#s2-37ea--local-long64-memory-legacy-rcpss-at-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-37ea-memory-rcpss-at-rax), [Stage 2 report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ea--rcpss-xmm0m32-rax), and [ISSUE-025](../../../../docs/KNOWN_ISSUES.md#issue-025--intel-rcpssrcpps-threshold-inequalities-overlap).

### S2-37eb local legacy RSQRTSS memory profile

`s2_37eb_rsqrtss_m32_memory_rhs.case.json` is a handwritten schema-v2 fixture
for exact `F3 0F 52 00`. It reads one unaligned m32 at `[RAX]` when AC=0;
`4.0` produces the deterministic local `0.5` result. Poisoned XMM0 checks
low32-only update and high96 preservation, while adjacent bytes check the read
width. Strict MXCSR admits all RC/prior-sticky histories with all exception
masks set and DAZ/FTZ clear. AC=1 is refused before memory access, including
when RAX is noncanonical; AC=0 at that same address yields #GP(0). The
independent integer oracle checks the RSQRT `3/8192` envelope for positive
normal results plus defined special-value relations. JIT/native and physical
acceptance are not claimed. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37eb--local-long64-memory-legacy-rsqrtss-at-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-37eb-memory-rsqrtss-at-rax), and [report](../../../../docs/reports/S2_37EB_RSQRTSS_MEMORY.md).

Байты написаны вручную по Intel SDM revision 093: Vol.2B `MOV—Move`,
Vol.2A `ADD—Add`, Vol.2A §§2.1.5 и 2.2.1.2 (ModR/M/SIB и REX),
Vol.1 §3.4.1.1 (GPR/subregister aliases). Нормативная ссылка:
[Intel SDM](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
Это собственные короткие программы, а не вывод assembler/encoder или
копия чужой CPU-реализации. Provenance записан также в каждом fixture.

Базовые fixtures ниже используют strict schema v2 / `s2-long-integer-v1`;
последующие local-only segment profiles описаны в отдельных разделах ниже.
V1 fixtures не меняются.
Начальные GPR/RIP/flags, memory bytes, permissions, comparison masks и
stop address записаны lossless в `.case.json`. Digest вычисляется по
точным JSON bytes при чтении; исторические v1 файлы и `SHA256SUMS` не
изменены. Fixtures не являются результатами физического oracle.

| Файл | Программа и ручной ожидаемый результат |
|---|---|
| [mov_load_add_store.case.json](mov_load_add_store.case.json) | `MOV RAX,[RBX+1]; ADD RAX,RCX; MOV [RBX+1],RAX`. При RBX=`0xffd` qword по `0xffe` пересекает 4 KiB и два смежных RAM regions. Из `7 + 5` получаются RAX=`12`, data=`0c00000000000000`, RFLAGS=`0x406`, RIP=`0x400b`, retired=`3`; остальные GPR не меняются. |
| [mov_width_aliasing.case.json](mov_width_aliasing.case.json) | Loads/stores через AL/AH/AX/EAX. До EAX верхние биты RAX сохраняются; EAX zero-extends. Последовательные load values: RAX=`0x1122334455667711`, `0x1122334455662211`, `0x1122334455664433`, `0x0000000088776655`. Destination bytes `1122334455667788`, flags=`0x402`, RIP=`0x4019`, retired=`8`. |
| [mov_rex_rip_relative.case.json](mov_rex_rip_relative.case.json) | RIP-relative load в R8D; register MOV в R9D; store через R12+disp; immediate в R10B; `MOV SPL,AL` с REX. Результат: R8=R9=`0x44332211`, R10=`0x112233445566777f`, RSP=`0x7034`, destination bytes=`0011223344`, flags=`0x402`, RIP=`0x4015`, retired=`5`. |

Для каждого файла:

```sh
target/debug/od-runner --fixture tests/fixtures/x86/stage2/mov_load_add_store.case.json --engine interpreter --json
```

`tools/runner/tests/fixture_v2.rs` запускает настоящий CLI и проверяет
перечисленные ручные результаты, все 16 GPR, digest и final memory.
`crates/od-testkit/src/execution_tests.rs` отдельно проверяет промежуточные
alias values и injected read/store faults: faulting инструкция не
retire, предшествующие инструкции сохраняются, частичная запись в первый
region при ошибке второго не допускается. Это локальные CPU/fixture-RAM
проверки S2-08; физическая приёмка среза A требует S2-09.

## Отдельные входы для физического Linux oracle S2-09

`hw_*.case.json` — новые входы с новыми case IDs и digest. Они сохраняют
темы S2-08, но явно задают пригодный для Linux layout: RX/readable code
по `0x100000`, readable data от `0x200000`, RW stack по `0x300000`
длиной 256 bytes и начальный RSP=`0x3000f8`. Старые fixtures выше не
переписаны. Адреса и pointer-valued GPR/results сравниваются буквально;
normalization и замена входов после исполнения не допускаются.

| Новый fixture / case ID | Явные отличия layout и ручные ожидаемые значения |
|---|---|
| [hw_mov_load_add_store.case.json](hw_mov_load_add_store.case.json) / `s2_09_hw_mov_load_add_store` | RBX=`0x200ffd`, qword по `0x200ffe` пересекает 4 KiB и logical regions `data_left`/`data_right`. RAX=`12`, flags=`0x406`, RIP=`0x10000b`, retired=`3`; data bytes=`0c00000000000000`, stack не меняется. |
| [hw_mov_width_aliasing.case.json](hw_mov_width_aliasing.case.json) / `s2_09_hw_mov_width_aliasing` | RBX=`0x200000`; destination по `0x200008`. RAX=`0x88776655`, flags=`0x402`, RIP=`0x100019`, retired=`8`; destination bytes=`1122334455667788`. |
| [hw_mov_rex_rip_relative.case.json](hw_mov_rex_rip_relative.case.json) / `s2_09_hw_mov_rex_rip_relative` | RIP-relative displacement буквально `0x000ffff9` ведёт от следующего RIP=`0x100007` к source=`0x200000`; R12=`0x210000`. R8=R9=`0x44332211`, R10=`0x112233445566777f`, RSP после SPL write=`0x300034`, destination bytes=`0011223344`, flags=`0x402`, RIP=`0x100015`, retired=`5`. |

Native profile ограничивает mappings low canonical addresses, readable
regions и отсутствие W+X. Страничное округление не расширяет logical
fixture ranges; snapshot содержит только записанные region bytes.
Разные permissions у logical regions одной host page дают явный
unsupported. Полный 16-GPR snapshot включает настоящий гостевой RSP.

Для двух interpreter/native прогонов трёх новых cases:

```sh
python3 tests/cpu/stage2_oracle.py --host 192.168.1.14 --user senya \
  --remote-root /home/senya/projects/open_desktop-stage1 --repeats 2
```

Harness требует одинаковые source fingerprints локально и на oracle;
исходники сам не синхронизирует. Он замораживает точные JSON bytes до
сборки, собирает local interpreter и remote native runner, передаёт
только архив выбранных fixtures и сравнивает через существующий Rust
v2 comparator. Повторы сравниваются отдельно для обоих engines. Raw
stdout/stderr/return codes, batch payload, inputs/digests/seeds, build
metadata, machine metadata, hashes harness/helper и reproduction argv
сохраняются в `work/stage2-oracle/<run-id>/manifest.json` и соседних
файлах. Любой SSH/build/unsupported/missing-result/mismatch проваливает
выбранный suite с сохранённым failed manifest. JIT нового профиля не
запускается. Запуск этой команды требует разрешённой передачи новых
fixture payload на выбранный oracle.

Старый v1 corpus (1010 cases дважды) по-прежнему запускается существующим
stage1 harness. Его прошедший oracle manifest можно привязать через
`--stage1-manifest work/.../manifest.json`; обязательны тот же Rust source
fingerprint, ≥1010 уникальных inputs и все interpreter/JIT/native
сравнения минимум двух повторов. Без этой опции результат обозначает
только memory suite и явно записывает отсутствие legacy evidence;
полная приёмка S2-09 всё равно требует старый corpus. Наличие команды и
локальных unit tests не подтверждает физический прогон.

Проверки самой orchestration без SSH:

```sh
python3 -m unittest discover -s tests/cpu -p 'test_stage*_harness.py' -v
```

## Data-transfer fixtures S2-10

Пять новых `s2_10_*.case.json` — самостоятельные ручные входы с layout
code=`0x100000`, data/results от `0x200000`, stack=`0x300000`.
Intel SDM revision 093: Vol.2B MOV pp.4-28–4-31, MOVSX/MOVSXD
pp.4-120–4-123 и MOVZX pp.4-130–4-131; Vol.2A LEA
pp.3-547–3-548; Vol.2D XCHG pp.6-32–6-33. Точные ссылки на формы
есть в provenance каждого входа. Все инструкции сохраняют исходные
проверяемые flags=`0xcd7`. Ручные expected значения проверяются реальным
interpreter в `crates/od-testkit/src/execution_tests.rs`; сами fixtures
ещё не являются свидетельством физического прохождения.

| Fixture | Наблюдаемая проверка |
|---|---|
| [s2_10_immediate_moffs.case.json](s2_10_immediate_moffs.case.json) | 34 инструкции, stop=`0x1000dc`. C6/C7 memory stores 8/16/32/64, AH/word/dword/qword register immediates и C7 RIP-relative с immediate внутри next RIP. Moffs A0–A3: все widths 8/16/32/64 при address32 и address64. Каждый load сохраняет полный RAX в отдельный qword results, каждый store пишет отдельный 16-byte slot с нетронутыми canaries `5a`/`a5`, поэтому последующая инструкция не скрывает ошибочную ширину записи. Итог RAX=R9=`0xffffffff80000001`, R8=`0x80000000`, RCX=`0xaabbccddeeff8000`; RIP-relative destination содержит `78563412`. |
| [s2_10_extend.case.json](s2_10_extend.case.json) | 34 инструкции, stop=`0x100089`. MOVZX/MOVSX: source8→dest16/32/64 и source16→dest32/64; MOVSXD: 16→16, 32→32 и 32→64. Memory и register sources включают AH, REX SPL и R8B. После каждой формы полный destination сохраняется отдельным qword, включая верхние биты после narrow writes. |
| [s2_10_addr32_transfers.case.json](s2_10_addr32_transfers.case.json) | 38 инструкций, stop=`0x1000b3`. Все memory extension формы предыдущего входа и четыре C6/C7 widths используют `67` с RBX=`0x1122334400200000`, то есть усечение до EBX=`0x200000` необходимо для успеха. Отдельные 16-byte canary slots различают ширину каждого store; промежуточные результаты extension сохраняются. |
| [s2_10_lea_no_read.case.json](s2_10_lea_no_read.case.json) | 12 инструкций, stop=`0x10003b`. Все шесть destination16/32/64 × address32/64 сочетаний, SIB, signed displacement, RIP-relative unmapped result=`0x101020`. Noncanonical RBX=`0x0000800000000000` и отсутствие data mappings проверяют отсутствие чтения памяти. FS/GS-prefixed LEA оставляют R12=R15=`0x0000800000000000` без прибавления segment base. Каждый результат остаётся в отдельном GPR. |
| [s2_10_xchg_aliases.case.json](s2_10_xchg_aliases.case.json) | 18 инструкций, stop=`0x10002c`. Register XCHG byte/word/dword/qword, AL/AH simultaneous old values, REX SPL, extended registers; `90`, `40 90`, `48 90` сохраняют верх RAX (checkpoint R10=`0x1122334455667788`), `41 90` и `87 c0` дают dword zero-extension. Промежуточные обмены наблюдаемы через RSP/RDX/R9–R13. Memory XCHG в этот срез не входит. |

Для отдельного S2-10 suite (после разрешённой синхронизации исходников
и передачи этих пяти fixtures):

```sh
python3 tests/cpu/stage2_oracle.py --host 192.168.1.14 --user senya \
  --remote-root /home/senya/projects/open_desktop-stage1 --repeats 2 \
  tests/fixtures/x86/stage2/s2_10_*.case.json
```

Default suite S2-09 остаётся прежним: три `hw_*` входа. Старые fixture
bytes и исторический `SHA256SUMS` не изменяются. Для S2-10 нужно новое
физическое evidence на точных новых JSON bytes и актуальном source
fingerprint; прежний S2-09 manifest его не заменяет.

## Arithmetic fixtures S2-11

`s2_11_*.case.json` — 313 новых bounded входов. Каждый исполняет ровно
одну арифметическую инструкцию и останавливается на следующем RIP:
результат и все шесть arithmetic flags остаются наблюдаемы в final
snapshot. Начальные DF=1 и reserved bit1=1 также проверяются; прежние
OF/SF/ZF/AF/PF/CF намеренно задаются отличающимися от многих результатов.
Expected literal sidecar [s2_11_expected.json](s2_11_expected.json)
содержит полные 16 GPR, flags и точные изменяемые memory regions.
Он не является альтернативным fixture format и не подаётся в runner.

| Семья | Проверяемые формы и границы |
|---|---|
| ADD/ADC/SUB/SBB/CMP | widths8/16/32/64; оба ModR/M направления register/register, memory/register и register/memory; dedicated accumulator immediate; group80/81 register и memory; group83 register и memory для16/32/64. |
| Carry и flags | Signed overflow каждого width, unsigned carry/borrow, ADC/SBB с обоими входными CF, auxiliary carry отдельно от whole-width carry, zero/nonzero, odd/even low-byte parity, sign. Negative imm8 проверяет sign-extension для16/32/64, negative imm32 — group81 и accumulator64. |
| INC/DEC/NEG | FE/FF register и memory widths8/16/32/64, INC/DEC сохраняют оба входных CF, signed overflow и unsigned wrap/borrow. F6/F7 NEG проверяет zero, one и minimum signed operand каждого width. |
| Aliases и адреса | AL/AH общего RAX, REX SPL и R8D/R9D с dword zero-extension. Каждая семья/ширина дополнительно использует address32 с RBX=`0x1122334400200ffe`, реально усечённым до EBX=`0x200ffe`. Это ограниченная явная матрица форм, а не исчерпывающий cross-product всех16 GPR и addressing encodings. |
| Memory | Data access по `0x200ffe`, logical regions `data_left`/`data_right` стыкуются по `0x201000`: widths32/64 пересекают и logical границу, и host page. Вокруг operand находятся неизменяемые `a5` canaries. Memory sources и CMP destinations read-only; CMP не должен записывать даже прежнее значение. Full-byte masks покрывают весь logical region. |

Это ручные opcode/ModR/M templates и literal input/output/flags tables,
материализуемые [generate_stage2_arithmetic.py](../../../cpu/generate_stage2_arithmetic.py).
Генератор упаковывает числа и JSON, не вычисляет арифметику, flags или
CPU semantics и не использует assembler/encoder/emulator. Provenance
каждого входа содержит SHA256 генератора. Нормативные формы: Intel SDM
revision093, [Vol.2A](https://cdrdv2.intel.com/v1/dl/getContent/671199)
ADC pp.3-9–3-11, ADD pp.3-14–3-15, CMP pp.3-161–3-162,
DEC pp.3-262–3-263, INC pp.3-457–3-458;
[Vol.2B](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf)
NEG pp.4-158–4-159, SBB pp.4-613–4-615, SUB pp.4-685–4-686;
Vol.1 AppendixA TableA-2 определяет affected flags.

Локальная проверка точных materialized bytes и независимых expected:

```sh
python3 tests/cpu/generate_stage2_arithmetic.py --check
cargo test -p od-testkit --offline arithmetic_
```

Testkit исполняет каждый checked-in input настоящим interpreter и
сравнивает full state/RIP/count/flags/memory с literal sidecar. Отдельный
тест инъецирует architectural read/write #PF и backend failures во
второй logical region для ADD/ADC/SUB/SBB/INC/DEC/NEG: faulting instruction
не retire, GPR/flags/RIP и весь RAM остаются прежними. Это локальный
точный fault-state test. Native positive profile отклоняет недопустимые
ranges/permissions до физической инструкции, а host signal не считается
guest exception; физическое подтверждение guest #PF этим набором не
заявляется. LOCK/REP и FS/GS data arithmetic остаются Unsupported.

После отдельного разрешения передачи нового source/fixture payload:

```sh
python3 tests/cpu/stage2_oracle.py --host 192.168.1.14 --user senya \
  --remote-root /home/senya/projects/open_desktop-stage1 --repeats 2 \
  tests/fixtures/x86/stage2/s2_11_*.case.json
```

Runner использует реальные hardware GPR для admission адресов; arithmetic
expected evaluator в physical runner отсутствует. Последующий manifest
с точными digest/source fingerprint должен отдельно подтвердить
физический результат; наличие fixtures и локальный interpreter PASS
его не заменяют. Старые S2-08–10 inputs/default suite не изменяются.

## Logic fixtures S2-12

`s2_12_*.case.json` — 197 новых входов с отдельным literal sidecar
[s2_12_expected.json](s2_12_expected.json). Все граничные формы имеют
одну логическую инструкцию и stop на следующем RIP; два дополнительных
сценария проверяют последовательное состояние definedness.

| Семья | Матрица и наблюдаемый результат |
|---|---|
| AND/OR/XOR | Widths8/16/32/64, оба ModR/M направления для register/register и register/memory, memory destinations, accumulator immediate, group80/81 register/memory, group83 register/memory для16/32/64. Negative imm8 и64-bit negative imm32 проверяют sign-extension. Zero/nonzero, positive/negative и low-byte parity имеют независимые literal ответы; CF/OF из исходных1 сбрасываются в0. |
| TEST | Только реальные формы84/85, A8/A9 иF6/F7 /0; register/register, read-only memory, register/memory immediate, address32. Destinations, включая полные верхние32 бита RAX после TEST EAX, остаются прежними. Negative imm32 с minimum qword различает sign-extension от zero-extension. |
| NOT | F6/F7 /2 register/memory/address32 для8/16/32/64. Все flags сохраняются при исходных all-set modeled flags, CF0 и all-zero modeled flags (reserved bit1 остаётся1). 8/16-bit destination сохраняет верх GPR,32-bit destination zero-extends. |
| Aliases/layout | Явные AL/AH общего RAX, REX SPL и R8D/R9D. Address32 требует усечения noncanonical RBX до EBX=`0x200ffe`. Widths32/64 пересекают logical regions и4KiB; `a5` canaries и полные memory masks делают ошибочную ширину записи наблюдаемой. Memory source/TEST read-only, изменяемые destinations R+W. |
| Definedness | Для AND/OR/XOR/TEST AF архитектурно undefined: fixture value mask и final semantic `rflags_defined` равны `0xcc7` вместо `0xcd7`. Значение AF не является expected ответом. `XOR; MOV; NOT` сохраняет undefined AF; `XOR; ADD` вновь определяет AF и возвращает mask=`0xcd7`. NOT сохраняет mask. |

[generate_stage2_logic.py](../../../cpu/generate_stage2_logic.py)
упаковывает ручные byte templates и literal answers. Проверенные S2-11
входы используются только как явный layout baseline; SHA256 baseline и
самого генератора записаны в каждом новом provenance. Ни старые входы,
ни их generator/expected bytes не изменяются. Logic/reference evaluator,
assembler или сторонняя CPU реализация не используются. Нормативные
формы и flags: Intel SDM revision093, Vol.2 instruction entries
AND/OR/XOR/TEST/NOT и Vol.1 AppendixA TableA-2.

```sh
python3 tests/cpu/generate_stage2_logic.py --check
cargo test -p od-testkit --offline logic_
```

Локальные tests сравнивают все197 actual interpreter runs с полными
literal GPR/memory/RIP/count, semantic flags mask и только определёнными
flags values. Injected read/write architectural/backend faults для
AND/OR/XOR/NOT и read faults TEST подтверждают отсутствие retirement,
изменения состояния, partial RAM write или definedness при ошибке.
Native admission возвращает shared instruction flag effects без
вычисления результата; tracer применяет их только после подтверждённого
TRAP_TRACE. Linux native tests отдельно проверяют TEST readonly и
preservation/restoration definedness. Они требуют реального Linux x86
исполнения; host signals не заменяют guest exceptions.

После отдельного разрешения передачи конкретных новых файлов:

```sh
python3 tests/cpu/stage2_oracle.py --host 192.168.1.14 --user senya \
  --remote-root /home/senya/projects/open_desktop-stage1 --repeats 2 \
  tests/fixtures/x86/stage2/s2_12_*.case.json
```

Это ограниченная явная матрица форм, не cross-product всех GPR/addressing
encodings. Физическое evidence появляется только после реального
native/interpreter сравнения exact bytes/source fingerprint. Ограничение
positive native profile по проверке guest #PF остаётся тем же, что у
S2-11; локальный injected fault test не выдаётся за физическое #PF.

## Shift and rotate fixtures S2-13

`s2_13_*.case.json` — 144 однокомандных сценария. Литеральная таблица
[s2_13_answers.json](s2_13_answers.json) задаёт входы, результаты и defined
flags; [generate_stage2_shifts.py](../../../cpu/generate_stage2_shifts.py)
только упаковывает эти строки в schema-v2 fixtures и
[s2_13_expected.json](s2_13_expected.json), не вычисляя сдвиги, вращения или
flags. SHL/SAL, SHR, SAR, ROL, ROR, RCL и RCR покрывают widths8/16/32/64;
SHLD/SHRD — widths16/32/64. Каждая пара kind/width проверяет raw counts 0,
1, width и width+1, с immediate и CL формами, кроме SHLD/SHRD word count17.
При этом count17 после маскирования превышает ширину слова, а результат
назначения архитектурно неопределён; v2 сравнивает полный GPR и не может
исключить эти биты. Эта пара остаётся в локальной no-panic/definedness проверке
без сравнения destination. RCL/RCR count=1 проверяется с CF-in=0 и1 для каждой
ширины. Для32/64 raw width и width+1 после архитектурной маски могут стать
zero или one; эти случаи проверяют именно masked behavior.

Две дополнительные memory fixtures выполняют SHL qword и SHLD qword с
destination по `0x200ffe`, пересекая logical regions `data_left`/`data_right`.
Native admission unit checks подтверждают, что RMW отклоняется без write
permission или вне logical range. Это проверка границы harness, не результат
физического guest page fault. Count-zero dword cases начинают с ненулевой
верхней половиной RAX и проверяют, что запись в EAX zero-extends в RAX[63:32],
даже когда эффективный count равен нулю; flags при этом остаются прежними.

Нормативные формы и flags: Intel SDM revision093, Vol.2A SAL/SAR/SHL/SHR
pp.4-582–4-585, Vol.2B RCL/RCR/ROL/ROR pp.4-521–4-524, SHLD pp.4-640–4-641,
SHRD pp.4-643–4-644 и Vol.1 AppendixA TableA-2.

```sh
python3 tests/cpu/generate_stage2_shifts.py --check
cargo test -p od-testkit --offline shift_rotate_fixtures
```

Физическое evidence требует native/interpreter сравнения этих точных JSON
bytes и source fingerprint на Linux x86-64.

## Direct control flow fixtures S2-16a

`s2_16a_*.case.json` содержит 68 буквальных long-mode сценариев: все 16
legacy condition codes для short и near Jcc в taken/fallthrough вариантах,
плюс четыре direct JMP с forward/backward rel8/rel32. Generator
[`generate_stage2_control_flow.py`](../../../cpu/generate_stage2_control_flow.py)
упаковывает ручные opcode bytes, входные flags и literal full-state answers;
условия и targets не вычисляются общей библиотекой или assembler. Provenance
ссылается на Intel SDM revision 093 Vol.2A Jcc pp. 3-499–3-502 и JMP
pp. 3-504–3-505.

`od-isa` проверяет все 32 сочетания CF/PF/ZF/SF/OF против literal truth table,
signed displacement от next RIP, 64-bit RIP wrap, canonical target fault,
exact fetch boundaries и Unsupported prefixes. Testkit и CLI runner сверяют
полное состояние и sidecar. Native-v2 admission проверяет только выбранный
taken/fallthrough target. Физический mini-check отложен до общего финального
подэтапа.

```sh
python3 tests/cpu/generate_stage2_control_flow.py --check
cargo test -p od-isa -p od-testkit -p od-runner --offline direct_control_flow
```

## Memory stack `66` + `REX.W` fixtures S2-17m

Four `.case.json` files cover `PUSH 66 48 FF /6` and `POP 66 48 8F /0`.
`REX.W` selects qword width over the earlier `66` prefix. The inputs check
ordinary memory source/destination and `[RSP]` alias ordering: PUSH reads from
the pre-decrement address; POP computes its destination from post-increment
RSP. Literal full-state answers are in
[`s2_17m_memory_stack_rexw_expected.json`](s2_17m_memory_stack_rexw_expected.json)
and are materialized by
[`generate_stage2_stack_rexw_override.py`](../../../cpu/generate_stage2_stack_rexw_override.py).
The native runner admission test accepts all four fixtures and checks that
effective REX ordering is honored while LOCK and FS/GS-prefixed forms remain
outside this profile. The fixtures await the shared final physical mini-check.

```sh
python3 tests/cpu/generate_stage2_stack_rexw_override.py --check
cargo test -p od-isa -p od-testkit --offline memory_stack_rexw
cargo test -p od-x86-runner --offline rexw_overrides_operand_override
```

## Register stack `66` + REX.W fixtures S2-17n

Six literal fixtures cover register PUSH/POP with an operand-size override and
an effective REX prefix. Qword forms confirm that REX.W overrides `66`; word
forms place a REX prefix with W=1 before the final REX with W=0, confirming
that only the final REX immediately before the opcode is effective. Extended
R8/R9 forms cover REX.B, and POP fixtures check preservation of upper register
bits for word destinations. Literal full-state answers are stored in
[`s2_17n_register_stack_rexw_expected.json`](s2_17n_register_stack_rexw_expected.json)
and materialized by
[`generate_stage2_stack_rexw_register.py`](../../../cpu/generate_stage2_stack_rexw_register.py).
The native admission test validates all bounded stack reads and writes. These
fixtures await the shared final physical mini-check.

Normative source: Intel SDM revision 093, Vol.2A §2.2.1.2 pp.2-8–2-9 and
Vol.2B PUSH pp.4-522–4-523, POP pp.4-398–4-399.

```sh
python3 tests/cpu/generate_stage2_stack_rexw_register.py --check
cargo test -p od-isa -p od-testkit --offline register_stack_rexw
cargo test -p od-x86-runner --offline mixed_prefix_register_stack
```
## POPFQ fixtures S2-17o

Two literal fixtures cover bare `POPFQ` (`9D`) under the native runner's
default CPL3/IOPL0 context. Only CF/PF/AF/ZF/SF/DF/OF are writable in this
profile; bit 1 is preserved by the literal fixtures. IF/IOPL and other
out-of-mask preservation are asserted by unit tests. Inputs or initial state
with TF, NT, or AC set are outside this slice and return `Unsupported` without committing CPU state. A stack access
fault also leaves CPU state and RFLAGS definedness unchanged. The literal
answers are materialized by
[`generate_stage2_popfq.py`](../../../cpu/generate_stage2_popfq.py).

Normative source: Intel SDM revision 093, Vol.2B POPF/POPFQ pp.4-407–4-410.
The implemented CPL3/IOPL0 privilege contract is deliberately limited; other
IOPL/CPL combinations, other POPF forms, and other PUSHF width/prefix variants remain open.

## PUSHFQ fixture S2-17p

One literal fixture covers bare long-mode `PUSHFQ` (`9C`). The instruction
writes a qword `RFLAGS AND 00000000_00FCFFFFH` at `RSP - 8`, then commits RSP;
RFLAGS is unchanged. The local full-state answer is literal. The memory
comparison mask checks the profile status flags, DF, fixed bit 1, and cleared
RF/VM bits, while ignoring IF/IOPL and other bits that the CPL3 fixture harness
cannot set to arbitrary values. Unit tests separately exercise the exact mask
for arbitrary interpreter state and no-commit on a bounded write failure.

The answer is materialized by
[`generate_stage2_pushfq.py`](../../../cpu/generate_stage2_pushfq.py).
Normative source: Intel SDM revision 093, Vol.2B
PUSHF/PUSHFD/PUSHFQ pp.4-528–4-529.

```sh
python3 tests/cpu/generate_stage2_pushfq.py --check
cargo test -p od-isa -p od-testkit --offline pushfq
cargo test -p od-x86-runner --offline pushfq_fixture_admits_only_a_bounded_writable_stack_slot
```

```sh
python3 tests/cpu/generate_stage2_popfq.py --check
cargo test -p od-isa -p od-testkit --offline popfq
cargo test -p od-runner --offline popfq_cli_fixtures
cargo test -p od-x86-runner --offline popfq_fixtures_admit
```

## POPFW fixture S2-17r

One literal fixture covers exact long-mode `66 9D` POPFW at CPL3/IOPL0.
The word loads CF/PF/AF/ZF/SF/DF/OF, while IF, IOPL, bit 1 and other
unmodeled RFLAGS bits remain unchanged. A source/current TF bit or source/current NT
bit returns `Unsupported` before CPU commit. Nonzero current IOPL also returns
`Unsupported` before the stack read. NT=1 is kept outside the profile because Intel SDM revision 093 Vol.2B Table 4-15 and Vol.1 §7.3.13.2 p.7-22 conflict;
see [ISSUE-015](../../../../docs/KNOWN_ISSUES.md#issue-015--разногласие-sdm-по-nt-для-popf-word-на-cpl3iopl0).
The fixture uses TF=NT=0; its v2 literal comparison checks RFLAGS through mask
`0xCD7`. A separate local testkit assertion checks exact preservation of
unmasked IF, reserved, ID, and upper bits with current IOPL=0 and source IOPL=3.
Native admission accepts only the bounded readable word stack input and rejects
TF/NT profile violations and nonzero current IOPL.

The answer is materialized by
[`generate_stage2_popfw.py`](../../../cpu/generate_stage2_popfw.py).
Normative source: Intel SDM revision 093, Vol.2B Table 4-15 pp.4-395–4-396
and Vol.1 §7.3.13.2 p.7-22. The physical comparison remains deferred to common Mac mini
acceptance.

```sh
python3 tests/cpu/generate_stage2_popfw.py --check
cargo test -p od-isa -p od-testkit --offline popfw
cargo test -p od-x86-runner --offline popfw_fixtures_admit
```

## PUSHFW fixture S2-17q

One literal fixture covers the long-mode `66 9C` word-width form. It writes
the low 16 bits of RFLAGS at `RSP - 2`, leaves RFLAGS unchanged, and commits
the two-byte stack-pointer decrement only after the bounded write succeeds.
The full local answer checks every register and byte; the physical memory mask
checks the integer-profile flags and fixed bit 1. A misplaced REX before 66 is
ignored by the shared decoder, while an effective REX and other prefixes are
outside this slice. Unit tests cover a bounded write failure without CPU
commit, and native admission requires a writable two-byte stack slot.

The handwritten fixture and literal answer are materialized by
[`generate_stage2_pushfw.py`](../../../cpu/generate_stage2_pushfw.py).
Normative source: Intel SDM revision 093 (253667-093US), Vol.2B
PUSHF/PUSHFD/PUSHFQ pp.4-528–4-529.

```sh
python3 tests/cpu/generate_stage2_pushfw.py --check
cargo test -p od-isa -p od-testkit --offline pushfw
cargo test -p od-runner --offline pushfw_cli_fixture
cargo test -p od-x86-runner --offline pushf_fixtures_admit_only_a_bounded_writable_stack_slot
```


## PUSHFQ/POPFQ REX.W-over-66 fixture S2-17s

One literal fixture executes `66 48 9C` followed by `66 48 9D`. Effective REX.W selects qword PUSHFQ/POPFQ semantics despite 66H; with effective REX.W clear, the existing word interpretation remains. PUSHFQ writes `RFLAGS AND 00000000_00FCFFFFH`; POPFQ restores the modeled flags. Input TF, NT and IOPL are zero to avoid combining this decode case with separate POPFQ restrictions. The fixture checks full RSP/RIP/flags state and the entire 8-byte stack slot with a canary mask.

Generated by [`generate_stage2_stack_flags_rexw.py`](../../../cpu/generate_stage2_stack_flags_rexw.py). Normative source: Intel SDM revision 093, Vol.2A §2.2.1.2 pp.2-8–2-9 and Vol.2B PUSHF/PUSHFD/PUSHFQ, POPF/POPFD/POPFQ.

```sh
python3 tests/cpu/generate_stage2_stack_flags_rexw.py --check
cargo test -p od-isa --offline pushf
cargo test -p od-testkit --offline pushf_rexw
```

## Near RET with ignored operand-size prefix S2-17t

The two literal fixtures execute `66 C3` and `48 66 C3` in long mode. Intel
SDM revision 093 Vol.1 §6.4.7 pp.6-12–6-13 forces near-branch operation
size to 64 bits and silently ignores operand-size prefixes. Vol.2A §2.2.1
p.2-8 says REX is ignored unless it immediately precedes the opcode or 0F,
so the REX before 66 is ineffective and is removed by prefix normalization.
Vol.2B RET p.4-566 defines `C3` as near RET. Both fixtures check the qword
return target, eight-byte RSP increment, full GPR/RIP/RFLAGS state, and
unchanged code and stack memory. Effective REX, duplicate 66, and other
effective prefix states remain outside this slice. Physical acceptance remains
deferred.

Generated by [`generate_stage2_ret66.py`](../../../cpu/generate_stage2_ret66.py)
from handwritten opcode bytes and a literal expected result, without an
assembler or emulator.

```sh
python3 tests/cpu/generate_stage2_ret66.py --check
python3 -m unittest tests.cpu.test_stage2_ret66_generator
cargo test -p od-isa --offline direct_call_and_ret_reject_prefixes_and_truncated_immediates
cargo test -p od-runner --offline operand_size_prefixed_near_ret_keeps_qword_stack_semantics
```

## Single-iteration MOVSB fixtures S2-18a

Three literal positive fixtures execute only bare `A4` MOVSB in long mode.
They check RSI/RDI movement for DF=0 and DF=1, exact RSI=RDI source/destination
aliasing for a one-byte self-copy, full GPR/RFLAGS/RIP, and all bytes in the data region.
The interpreter reads `[RSI]`, writes `[RDI]`, and commits both indices only
after the write succeeds. Flat DS/ES bases are zero in the modeled long-mode
profile. REP, all prefixes, address-size changes, widths other than byte, and
other string instructions remain unsupported.

Two fault-only fixtures place the selected source or destination just beyond
the bounded data mapping. The local test injects architectural #PF at each
site and checks access order and the executor's current no-commit policy.
Intel SDM 093 does not establish the CPU's index/RIP snapshot after a
destination #PF; see [ISSUE-016](../../../../docs/KNOWN_ISSUES.md#issue-016--не-установлен-restart-state-movs-после-destination-fault).
Native admission accepts the three fully mapped positive cases and rejects
both fault-only cases. The deferred physical acceptance pool S2-15 onward
contains 563 cases at the S2-18a boundary: 558 prior fixtures, these 3
positive cases, and 2 local-only injected-fault cases. Other files in the
fixture directory were not reclassified here; native admission determines
the executable subset. No Mac mini execution has been performed for this
slice.

The handwritten bytes and expected state/memory constants are serialized by
[`generate_stage2_movsb.py`](../../../cpu/generate_stage2_movsb.py). The script
does not assemble instructions or calculate x86 results. Its SHA-256 is
`2adc5c9a3507d2ca1f2c88e75bc688c992884c69bd69e01597aadb7c76d2337a`.
Normative source: Intel SDM revision 093
(253667-093US), Vol.2B MOVS/MOVSB/MOVSW/MOVSD/MOVSQ pp.4-107–4-109.

```sh
python3 tests/cpu/generate_stage2_movsb.py --check
cargo test -p od-isa -p od-testkit --offline movsb
cargo test -p od-x86-runner --offline movsb_native_admission
```

## Single-iteration MOVSW fixtures S2-18b

Three literal positive fixtures execute `66 A5` MOVSW in long mode. They check
RSI/RDI movement by two for DF=0 and DF=1, exact RSI=RDI aliasing for a
two-byte self-copy, full GPR/RFLAGS/RIP, and the complete data region. The
interpreter reads the word at `[RSI]`, writes it to `[RDI]`, and commits both
indices only after the write succeeds. Three mapped raw fixtures pass native
admission; the prior fixture pool is not reclassified in this slice.

Two fault-only fixtures place the word source or destination outside the
bounded mapping. Local injected-#PF checks verify Fetch→Read(2 bytes), or
Fetch→Read→Write(2 bytes), and the executor's current no-commit policy. SDM
093 does not establish the destination-fault restart snapshot; see
[ISSUE-016](../../../../docs/KNOWN_ISSUES.md#issue-016--не-установлен-restart-state-movs-после-destination-fault).
Native admission rejects both fault-only inputs. S2-18a's deferred pool was
563 cases; this slice adds 5 cases (3 mapped positive, 2 local-only fault),
bringing that pool to 568. No Mac mini execution has been performed for this
slice.

The handwritten bytes and expected answers are serialized by
[`generate_stage2_movsw.py`](../../../cpu/generate_stage2_movsw.py). The
script does not assemble instructions or calculate x86 results. Its SHA-256 is
`b5f2702bbf3206870474e26f9814d42e7cf58069038035a1b6433c67d9eda4bc`.
Normative source: Intel SDM revision 093 (253667-093US), Vol.2B
MOVS/MOVSB/MOVSW/MOVSD/MOVSQ pp.4-107–4-109. The decoder normalizes away a
misplaced REX before `66`; a local regression verifies `48 66 A5` has the same
accepted word semantics as `66 A5`.

```sh
python3 tests/cpu/generate_stage2_movsw.py --check
cargo test -p od-isa -p od-testkit --offline movsw
cargo test -p od-x86-runner --offline movsw_native_admission
```

## Single-iteration MOVSD fixtures S2-18c

Three literal positive fixtures execute bare `A5` MOVSD in long mode. They
check four-byte RSI/RDI movement for DF=0 and DF=1, a four-byte overlapping
copy, full GPR/RFLAGS/RIP, and every byte in the data region. This slice admits
only the unprefixed dword form. Effective REX, REP, segment prefixes, and 67
address-size override remain unsupported. `66 A5` selects the separate MOVSW
slice. The decoder normalizes away a
REX byte followed by a legacy prefix; the surviving legacy prefix still keeps
such a sequence outside bare-A5 admission.

Two fault-only fixtures place the dword source or destination outside the
bounded mapping. Local injected-#PF checks verify Fetch→Read(4 bytes), or
Fetch→Read→Write(4 bytes), and the executor's current no-commit policy. SDM
093 does not establish the destination-fault restart snapshot; see
[ISSUE-016](../../../../docs/KNOWN_ISSUES.md#issue-016--не-установлен-restart-state-movs-после-destination-fault).
Native admission accepts the three mapped positive fixtures and rejects both
fault-only cases. S2-18b's deferred pool was 568 cases; these five bring it to
573. No Mac mini execution has been performed for this slice.

Handwritten bytes and literal expected answers are serialized by
[`generate_stage2_movsd.py`](../../../cpu/generate_stage2_movsd.py), which does
not assemble instructions or calculate x86 results. Normative source: Intel
SDM revision 093, Vol.2B MOVS/MOVSB/MOVSW/MOVSD/MOVSQ; bare `A5` is MOVSD in
long mode, default address size uses RSI/RDI, and `67` selects ESI/EDI.

```sh
python3 tests/cpu/generate_stage2_movsd.py --check
cargo test -p od-isa -p od-testkit --offline movsd
cargo test -p od-x86-runner --offline movsd_native_admission
```

## Single-iteration STOSB fixtures S2-18e

Four handwritten schema v2 cases use only bare `AA` STOSB in long mode. Three
mapped cases cover DF=0, DF=1, and RSI=RDI aliasing with canary bytes around the
single destination byte. The alias case also proves that STOSB performs no
source memory read and leaves RSI unchanged. A fourth case points RDI exactly
past the mapped data region; its injected write `#PF` is local-only and checks
the executor's current no-commit policy. No hardware restart snapshot is
asserted; see [ISSUE-016](../../../../docs/KNOWN_ISSUES.md#issue-016--не-установлен-restart-state-movs-после-destination-fault).

Native admission accepts the three mapped cases and rejects the fault-only
fixture because the implicit destination is not mapped. Prefixes, REP, and
other STOS widths remain outside this slice. Literal bytes, full state, flags,
and memory images are materialized by
[`generate_stage2_stosb.py`](../../../cpu/generate_stage2_stosb.py),
which contains no encoder or semantic expected-value calculation.

The deferred S2-15-through-S2-18 physical pool was 580 cases after MOVSQ.
These four cases bring it to 584: three native-admissible positive fixtures
and one local-only fault fixture. Physical execution remains deferred to the
single final Mac mini acceptance substage.

```sh
python3 tests/cpu/generate_stage2_stosb.py --check
cargo test -p od-isa -p od-testkit --offline stosb
cargo test -p od-x86-runner --offline stosb_native_admission
```

## Single-iteration STOSW/D/Q fixtures S2-18g

Twenty literal schema v2 fixtures cover six mapped width/direction cases, three
unmapped destination errors, three read-only destination errors, three
noncanonical destination #GP(0) results, three zero-budget cases, and the two
`48 66 AB` / `66 48 AB` normalization cases. Every expected answer contains all
GPRs, RFLAGS, definedness, RIP, and complete code/data images. Mapping and
permission failures are `harness_error`; they are never translated to guest
`#PF`. The fixture schema and profile remain unchanged.

[`generate_stage2_stos_widths.py`](../../../cpu/generate_stage2_stos_widths.py)
only serializes its handwritten encodings and complete literal answers. The
interpreter checks include the 6,144 success matrix, full access trace and
canaries, all REX.W forms, 35 prefix rejects, unaligned/page-crossing and region
end writes, RDI wrap/post-index noncanonicality, RSI values that must not be
read, injected architecture/backend faults, and fetch/budget no-commit. JIT and
native admission remain denied. Physical checks, if selected, remain in the
shared final acceptance substage.

```sh
python3 tests/cpu/generate_stage2_stos_widths.py --check
cargo test -p od-isa stos_ab --offline
cargo test -p od-testkit stos_ --offline
```

## Single-iteration SCAS B/W/D/Q fixtures S2-18h

Exactly 24 literal schema-v2 fixtures cover eight width/direction successes,
four unmapped source harness errors, four noncanonical source #GP(0) results,
four zero-budget cases, both `48 66 AF` / `66 48 AF` normalization forms, and
TEST→SCAS success/fault composition. Answers include all GPRs, RFLAGS,
definedness, RIP, and complete code/data images. SCAS reads the pre-state
`ES:[RDI]` once, compares the width-selected accumulator with CMP flags, and
commits flags and the DF-directed pointer change only after the read succeeds.
It preserves all RAX bits even for SCASD, and never inspects RSI. Unmapped
sources remain `harness_error`; noncanonical sources are local #GP(0). JIT and
native admission remain closed, and the fixture schema/profile is unchanged.

[`generate_stage2_scas.py`](../../../cpu/generate_stage2_scas.py) serializes
handwritten bytes and predetermined full-state answers only. Independent
interpreter coverage includes 6,144 states and flag comparisons, exact one-read
traces, 48 enumerated prefix rejects, REX.W/66 order variants, and faulting
current-instruction atomicity. Physical execution remains deferred to the
shared final acceptance substage; ISSUE-016/019 remain open.

```sh
python3 tests/cpu/generate_stage2_scas.py --check
python3 -m unittest tests/cpu/test_stage2_scas_generator.py
cargo test -p od-testkit --offline scas_
cargo test -p od-runner --test fixture_v2 --offline scas_cli_fixtures
```

## Single-iteration CMPS B/W/D/Q fixtures S2-18i

Exactly 33 literal schema-v2 fixtures cover eight width/direction positives,
eight unmapped outcomes split across RSI and RDI, eight canonical48 #GP(0)
cases split across both operands, four budget-zero boundaries, both
`48 66 A7` / `66 48 A7` normalizations, and three exact `TEST+CMPSB` streams
(success, first-read fault, second-read fault). Full GPR/flags/definedness/RIP
and complete code/data images are recorded. The operation always reads
pre-state DS:[RSI] first, then pre-state ES:[RDI], and computes memory[RSI] minus
memory[RDI]. A failure leaves CPU state uncommitted; on a second-read fault the
first read remains in the trace. RSI=RDI does not collapse the two reads.

[`generate_stage2_cmps.py`](../../../cpu/generate_stage2_cmps.py) serializes
the handwritten streams and predetermined full snapshots. Independent tests
cover 6,144 successful full-state cases, 64 fault injections, ordered read
traces, no writes, exact 48-prefix reject set, normalization, pointer wrapping,
alignment/page/canonical boundaries and both read fault sites. The selected
RSI-first ordering is a bounded local policy and does not establish hardware
priority when both operands are invalid. No MMIO rollback behavior is claimed.
The dual-invalid winner is recorded as ISSUE-020. JIT/native admission is
closed; Mac mini, SSH and physical validation remain in the shared final
acceptance substage.

```sh
python3 tests/cpu/generate_stage2_cmps.py --check
python3 -m unittest tests/cpu/test_stage2_cmps_generator.py
cargo test -p od-testkit --offline cmps_
cargo test -p od-runner --test fixture_v2 --offline cmps_cli_fixtures
```

## Single-iteration LODS B/W/D/Q fixtures S2-18f

Twenty-two literal schema v2 fixtures cover the bounded non-REP LODS slice:
eight positives (AC, 66 AD, AD, and 48 AD with DF=0/1), four genuinely
unmapped source reads, four noncanonical DS-source #GP cases, four zero-budget
cases, and `48 66 AD` / `66 48 AD` prefix-normalization cases. Positive cases
check the full GPR/RFLAGS/RIP snapshot, distinct source byte lanes, and complete
unchanged RAM. Fault and budget cases check the full unchanged snapshot and
RAM. The unmapped cases end as the existing `harness_error` result; this is not
converted to a guest #PF. `ExpectedExit::HarnessError` records that existing
backend outcome without changing the schema or profile shape.

[`generate_stage2_lods.py`](../../../cpu/generate_stage2_lods.py) serializes
the handwritten instruction bytes, input states, memory bytes, and predetermined
answers; it does not assemble instructions or calculate results from an
execution engine. These fixtures are local interpreter CLI checks only; native
and JIT admission remain closed. The 48 explicitly enumerated prefix rejects,
6,144-case full-state matrix, fault injection, fetch boundaries, address
residues, page crossing, exact readable end, and RSI wrapping are checked in
`od-isa` unit tests. Physical execution remains in the shared final acceptance
substage.

```sh
python3 tests/cpu/generate_stage2_lods.py --check
cargo test -p od-isa --offline lods_
cargo test -p od-runner --test fixture_v2 --offline lods_cli_fixtures
```

## Bounded REP STOSB fixtures S2-19a

Five handwritten schema v2 fixtures cover exact `F3 AA`: zero count without a
destination access, one forward store, three forward/backward stores, and an
injected middle write fault after one committed iteration. The fault fixture is
local-only and checks that the successful store remains visible while the
faulting iteration leaves RCX/RDI at the next byte and RIP at `F3 AA`; this is
the chosen executor contract and does not assert exception-priority behavior.
Focused interpreter tests also stop after two stores, resume from the saved
RCX/RDI state, and exercise the separate 1024-iteration work limit. That limit
does not consume the one-instruction retirement budget.

Native admission accepts the four complete mapped small-count fixtures,
including zero count, and rejects the local-only fault, counts above eight,
and extra prefixes. These are fixture-profile bounds, not interpreter
semantics. Literal answers are serialized by
[`generate_stage2_rep_stosb.py`](../../../cpu/generate_stage2_rep_stosb.py);
the script does not assemble instructions or calculate expected results.

The deferred S2-15-onward physical pool increases from 584 to 589 fixtures.
Across string-operation slices S2-18a–S2-19a it now contains 21 mapped cases
admitted by native profile and 10 local-only fault cases rejected by admission.
Physical execution stays deferred to the single final Mac mini acceptance
substage.

```sh
python3 tests/cpu/generate_stage2_rep_stosb.py --check
cargo test -p od-testkit --offline rep_stosb
cargo test -p od-x86-runner --offline rep_stosb_native_admission
```

## Tracked REP restart fixtures S2-19i

Six schema-v2 fixtures compose `TEST RAX,RAX` with REP STOSB, LODSB, and MOVSB
for DF=0/1 at count 1025. Each first run retires TEST and reaches the
1024-iteration limit with one REP iteration remaining. The unchanged fixture
schema records that partial outcome; `s2_19i_rep_restart_expected.json` stores
literal full-state and memory snapshots for both the partial run and the
explicitly resumed final iteration. The generator serializes fixed input and
expected values only. `run()` continues to mean a fresh entry; the runner test
passes the first result's flag-definedness mask to the explicit tracked-entry
API to verify the final snapshot. No hardware restart/fault-priority or
JIT/native/physical claim is made. ISSUE-016/020/021 remain open.

```sh
python3 tests/cpu/generate_stage2_rep_restart.py --check
python3 tests/cpu/test_stage2_rep_restart_generator.py
cargo test -p od-runner --test fixture_v2 test_rep_iteration_limit_fixtures_resume_with_literal_definedness_snapshots --offline
```

## Divide-error exit fixtures S2-20a

Two local-only schema v2 fixtures use literal `F6 F3` (`DIV BL`) bytes and
exercise its zero-divisor and byte-quotient-overflow `#DE` cases. The runner
CLI checks vector 0, unavailable error code and fault address, faulting RIP at
the instruction, zero retirements, and unchanged full GPR/RFLAGS/RIP and code
memory. They do not establish conflicting-fault priority or native exception
delivery. At S2-20a completion these two cases increased the deferred
S2-15-onward pool from 589 to 591; the native acceptance-selected subset was
unchanged. Across string-operation slices, native admission remains 21 mapped
fixtures and rejects 10 local-only fault fixtures. Physical runs stay in the
shared final Mac mini acceptance stage.

Literal values are serialized by
[`generate_stage2_divide_errors.py`](../../../cpu/generate_stage2_divide_errors.py).

```sh
python3 tests/cpu/generate_stage2_divide_errors.py --check
cargo test -p od-runner --test fixture_v2 divide_error_fixtures_preserve_state_and_report_exact_de_metadata --offline
```

## Complete UD2 exit fixture S2-20b

One local-only schema v2 fixture contains the complete `0F 0B` UD2 opcode. Its
CLI test checks vector 6, unavailable error code and fault address, faulting
RIP at the first byte, zero retired instructions, and unchanged full GPR,
RFLAGS, RIP and code memory. This does not assert priority against an
instruction-fetch fault, invalid-prefix behavior, native exception delivery,
or a physical hardware result. The case was not included in native acceptance
selection or execution. It brings the deferred S2-15-onward pool to 592 while
the native acceptance-selected subset remains unchanged.

The handwritten bytes and expected snapshot are serialized by
[`generate_stage2_ud2.py`](../../../cpu/generate_stage2_ud2.py), using Intel SDM
revision 093 Vol.2A UD2.

```sh
python3 tests/cpu/generate_stage2_ud2.py --check
python3 tests/cpu/test_stage2_ud2_generator.py
cargo test -p od-runner --test fixture_v2 complete_ud2_cli_fixture_reports_ud_without_state_commit --offline
```

## UD2 split-region fetch fixture S2-20d

The existing complete `0F 0B` fixture is supplemented by a literal schema-v2
case that puts `0F` and `0B` in adjacent executable regions. The CLI snapshot
checks vector 6, unavailable error code/address, faulting RIP, zero
retirements, full GPR/RFLAGS/definedness and unchanged bytes in both regions.
Callback tracing in `od-testkit` separately proves that only those two bytes
are fetched, that no data access occurs, and that truncated or injected fetch
errors preserve their original class/payload without committing CPU or RAM.
Fixture-v2 has no `HarnessError` expected-exit value, so these error cases are
not serialized as CLI fixtures. This records only the local FlatMemory and
callback contract and does not claim physical fetch or fault priority.

The split fixture is excluded from native acceptance. It increases the
deferred S2-15-onward pool from 593 to 594; native-selected membership is
unchanged. The complete and split literals are serialized by
[`generate_stage2_ud2.py`](../../../cpu/generate_stage2_ud2.py).

```sh
python3 tests/cpu/generate_stage2_ud2.py --check
python3 tests/cpu/test_stage2_ud2_generator.py
cargo test -p od-testkit --lib ud2_ --offline
cargo test -p od-runner --test fixture_v2 split_region_ud2_cli_fixture_reports_ud_without_state_or_ram_commit --offline
```

## Qword IDIV divide-error exit fixture S2-20c

One local-only schema v2 fixture contains literal `48 F7 F9` (`IDIV RCX`)
with `RDX:RAX` equal to signed 128-bit minimum and `RCX=-1`, producing a
signed quotient overflow. The CLI test checks vector 0, unavailable error code
and fault address, faulting RIP at the instruction, zero retirements, and
unchanged full GPR/RFLAGS/RIP and code memory. This is an isolated divide-error
case; it does not claim fault-priority or native exception semantics. It is
excluded from native acceptance selection and execution. The deferred
S2-15-onward pool increases from 592 to 593 while the native-selected subset
remains unchanged. Physical checks remain in the shared final acceptance stage.

[`generate_stage2_idiv_qword_error.py`](../../../cpu/generate_stage2_idiv_qword_error.py)
serializes the literal bytes and expected state from Intel SDM revision 093
Vol.2A IDIV.

```sh
python3 tests/cpu/generate_stage2_idiv_qword_error.py --check
python3 tests/cpu/test_stage2_idiv_qword_error_generator.py
cargo test -p od-runner --test fixture_v2 qword_idiv_overflow_cli_fixture_reports_de_without_state_commit --offline
```

## Bare qword memory XCHG fixture S2-21a

One local-only schema v2 case runs handwritten `48 87 00` (`XCHG [RAX], RAX`).
RAX is both the memory base and source value; its unaligned qword crosses two
adjacent data regions, whose exact post-state and canary bytes are checked by
the public CLI test. This checks executor behavior through one
`GuestMemory::atomic_exchange_integer` call and verifies that the old value is
committed to RAX only on success. The GuestMemory contract requires the call to
leave memory unchanged on error; this fixture does not claim SMP ordering,
production-backend guarantees, native selection, or physical atomicity.

The single case increases the deferred S2-15-onward pool from 593 to 594; the
native-selected subset is unchanged. Physical checks remain in the common final
Mac mini acceptance stage.

[`generate_stage2_xchg_qword_memory.py`](../../../cpu/generate_stage2_xchg_qword_memory.py)
serializes the handwritten bytes and literal initial memory with Intel SDM
revision 093 Vol.2D XCHG provenance.

```sh
python3 tests/cpu/generate_stage2_xchg_qword_memory.py --check
python3 tests/cpu/test_stage2_xchg_qword_memory_generator.py
cargo test -p od-runner --test fixture_v2 qword_memory_xchg_fixture_matches_literal_alias_and_split_memory_result --offline
```

## Qword register-register XADD fixture S2-21b

One local-only schema v2 fixture runs literal `48 0F C1 C0` (`XADD RAX, RAX`).
With initial RAX `0x7fff_ffff_ffff_ffff`, the result is
`0xffff_ffff_ffff_fffe`; the CLI test checks all GPRs, RIP, full arithmetic
flags and one instruction retirement. The same register in both ModR/M fields
checks alias behavior. The case does not cover memory XADD, LOCK, other widths,
CMPXCHG, native admission or physical execution.

The case increases the deferred S2-15-onward pool from 594 to 595; native
selection is unchanged. Physical checks remain in the common final Mac mini
acceptance stage.

[`generate_stage2_xadd_qword_register.py`](../../../cpu/generate_stage2_xadd_qword_register.py)
serializes handwritten bytes and literal state with provenance to Intel SDM
revision 093 Vol.2D XADD p.6-23.

```sh
python3 tests/cpu/generate_stage2_xadd_qword_register.py --check
python3 tests/cpu/test_stage2_xadd_qword_register_generator.py
cargo test -p od-runner --test fixture_v2 qword_register_xadd_fixture_matches_alias_sum_and_arithmetic_flags --offline
```

## Qword register-register CMPXCHG fixtures S2-21c

Two local-only schema-v2 fixtures cover bare `48 0F B1 /r` qword register
CMPXCHG: the equal case compares RAX with RCX and copies RBX to RCX; the
unequal case compares RAX with RCX while source and destination both name RCX,
then copies the old RCX to RAX. Both include complete expected register and
flag snapshots in the CLI oracle. Memory destinations, LOCK, other widths,
native admission and physical execution are outside this slice.

These cases increase the deferred S2-15-onward pool from 595 to 597. The native
selection remains unchanged; physical checks remain in the common final Mac
mini acceptance stage.

[`generate_stage2_cmpxchg_qword_register.py`](../../../cpu/generate_stage2_cmpxchg_qword_register.py)
serializes handwritten bytes and initial state with provenance to Intel SDM
revision 093 Vol.2A CMPXCHG.

```sh
python3 tests/cpu/generate_stage2_cmpxchg_qword_register.py --check
python3 tests/cpu/test_stage2_cmpxchg_qword_register_generator.py
cargo test -p od-runner --test fixture_v2 qword_register_cmpxchg_fixtures_match_both_compare_branches --offline
```

## Qword memory XADD fixture S2-21d

One local-only schema-v2 fixture executes literal `48 0F C1 00` (`XADD
qword [RAX], RAX`). RAX is both the address base and source value. The unaligned
qword spans two adjacent regions; adding it to `0xffffffffffffffff` wraps to
`0x00000000003000fa`. The expected state checks the old memory value in RAX,
arithmetic flags, RIP/retirement and every byte of both regions, including
canaries. The generator provenance identifies Intel SDM revision 093 Vol.2D
XADD p.6-23.

The interpreter makes one `GuestMemory::atomic_fetch_add_integer` call. Local
spy and fault tests prove one backend operation and no CPU/RAM commit on an
error. This fixture is excluded from native admission; no production backend,
LOCK/SMP, or hardware-atomicity claim is made. The deferred S2-15-onward pool
increases from 597 to 598; the native-selected subset is unchanged. Physical
validation remains in the shared final Mac mini acceptance stage.

[`generate_stage2_xadd_qword_memory.py`](../../../cpu/generate_stage2_xadd_qword_memory.py)
serializes literal bytes and expected state with Intel SDM provenance.

```sh
python3 tests/cpu/generate_stage2_xadd_qword_memory.py --check
python3 tests/cpu/test_stage2_xadd_qword_memory_generator.py
cargo test -p od-runner --test fixture_v2 qword_memory_xadd_fixture_matches_alias_sum_flags_and_split_canaries --offline
```

## Qword memory CMPXCHG fixtures S2-21e

Two local-only schema-v2 fixtures execute bare effective REX.W `0F B1 /r`
CMPXCHG with qword memory destinations. The equal and unequal cases both use an
unaligned qword split across adjacent regions and check the complete register,
flag, RIP, retirement, and region-byte snapshots. The unequal case uses RAX as
the source and effective-address base; it verifies that the observed memory
value replaces RAX while memory remains unchanged. The generator records Intel
SDM revision 093 Vol.2A CMPXCHG provenance.

The `GuestMemory::atomic_compare_exchange_integer` boundary is one operation,
validates write access even on mismatch, and does not mutate memory on error.
Spy/fault tests cover backend call count and commit behavior. Fixtures remain
excluded from native admission; LOCK/SMP/production backend/physical claims
are outside scope. Deferred S2-15-onward pool increases from 598 to 600; the
native-selected subset is unchanged. Physical validation remains in the shared
final Mac mini acceptance stage.

[`generate_stage2_cmpxchg_qword_memory.py`](../../../cpu/generate_stage2_cmpxchg_qword_memory.py)
serializes literal bytes and state with Intel SDM provenance.

```sh
python3 tests/cpu/generate_stage2_cmpxchg_qword_memory.py --check
python3 tests/cpu/test_stage2_cmpxchg_qword_memory_generator.py
cargo test -p od-runner --test fixture_v2 qword_memory_cmpxchg_fixtures_match_both_compare_branches_and_split_canaries --offline
```

## LOCK qword memory XADD fixture S2-21f

One local-only schema-v2 fixture executes literal `F0 48 0F C1 00` (`LOCK XADD
qword [RAX], RAX`). It checks the old destination in the aliased RAX source,
wrapping sum, arithmetic flags, RIP/retirement, and all bytes/canaries across an
unaligned qword spanning adjacent regions. Generator provenance references
Intel SDM revision 093 Vol.2D XADD and Vol.2A LOCK prefix rules.

This operation uses the existing `GuestMemory::atomic_fetch_add_integer` call.
Its use is restricted to serial execution over private synchronous ordinary
RAM with no concurrent readers/writers, DMA, shared aliases, delayed writes,
or MMIO. No full x86 LOCK ordering, SMP, native admission, or physical claim is
made. Other LOCK forms remain unsupported. Deferred S2-15-onward pool grows
from 600 to 601; the native-selected subset is unchanged. Physical validation
remains in the shared final Mac mini acceptance stage. The unaligned fixture
checks backend bytes only and does not establish hardware #AC or split-lock
behavior.

[`generate_stage2_lock_xadd_qword_memory.py`](../../../cpu/generate_stage2_lock_xadd_qword_memory.py)
serializes the literal code and state with Intel SDM provenance.

```sh
python3 tests/cpu/generate_stage2_lock_xadd_qword_memory.py --check
python3 tests/cpu/test_stage2_lock_xadd_qword_memory_generator.py
cargo test -p od-runner --test fixture_v2 locked_qword_memory_xadd_fixture_matches_alias_sum_flags_and_split_canaries --offline
```

## Bare MFENCE fixture S2-21g

The local-only schema-v2 fixture executes `48 89 18 0F AE F0 48 8B 08`
(`MOV [RAX], RBX; MFENCE; MOV RCX, [RAX]`). It verifies the store is committed
before the later load, with GPRs, flags, RIP, retirement and the data bytes
observed. `GuestMemory::mfence` is required and has no default no-op; only
private synchronous `FlatMemory` opts in. The contract does not cover shared
memory, asynchronous writes, MMIO, DMA, SMP or physical ordering. Alternate
`0F AE` encodings and prefixes remain unsupported by this profile, without an
architectural invalid-encoding claim. The native-selected subset is unchanged;
the deferred S2-15-onward pool grows from 601 to 602. Physical validation
remains in the shared final Mac mini acceptance stage.

[`generate_stage2_mfence.py`](../../../cpu/generate_stage2_mfence.py) serializes
the literal fixture with Intel SDM provenance.

```sh
python3 tests/cpu/generate_stage2_mfence.py --check
python3 tests/cpu/test_stage2_mfence_generator.py
cargo test -p od-runner --test fixture_v2 mfence_store_fence_load --offline
```

## FS selector MOV fixture S2-22c

The local-only `s2-long-fs-selector-v1` fixture runs exactly `48 8C E0`
(`MOV RAX, FS`). It checks zero-extension of `FS.selector=0xF123` into RAX,
three-byte RIP advance, one retirement, and preservation of FS.base, selector,
flags and all other GPRs. The selector profile includes both FS.base and
FS.selector in initial/result state so independence and preservation are
observable. This reads the selector only; it performs no descriptor lookup or
validation and makes no claim about privilege or segment-load behavior.
Native execution rejects the profile before mapping or tracee creation, and
legacy native admission rejects this decoded form. The shared deferred pool
increases by one; native-selected membership is unchanged, and physical
validation remains in the shared final acceptance stage.

[`generate_stage2_fs_selector.py`](../../../cpu/generate_stage2_fs_selector.py)
serializes literal bytes and expected state from Intel SDM revision 093 Vol.1
§3.4.2 and Vol.2B MOV pp.4-28–4-31 provenance.

```sh
python3 tests/cpu/generate_stage2_fs_selector.py --check
python3 tests/cpu/test_stage2_fs_selector_generator.py
cargo test -p od-runner --test fixture_v2 fs_selector_fixture_zero_extends_and_preserves_segment_state --offline
```

## GS selector MOV fixture S2-22d

The local-only `s2-long-gs-selector-v1` fixture runs exactly `48 8C E8`
(`MOV RAX, GS`). It checks zero-extension of `GS.selector=0xF123` into RAX,
three-byte RIP advance, one retirement, and preservation of GS.base, selector,
flags and all other GPRs. The profile exposes only GS.base and GS.selector;
descriptor lookup/load and privilege checks are outside the slice. Native
execution fails closed before mapping or tracee creation, and legacy native
admission rejects the decoded form. The shared deferred pool grows by one;
native-selected membership is unchanged, and physical validation remains in
the shared final acceptance stage.

[`generate_stage2_gs_selector.py`](../../../cpu/generate_stage2_gs_selector.py)
serializes literal bytes and expected state with Intel SDM revision 093 Vol.1
§3.4.2 and Vol.2B MOV pp.4-28–4-31 provenance.

```sh
python3 tests/cpu/generate_stage2_gs_selector.py --check
python3 tests/cpu/test_stage2_gs_selector_generator.py
cargo test -p od-runner --test fixture_v2 gs_selector_fixture_zero_extends_and_preserves_segment_state --offline
```

## CPL3 GDT-only VERW fixture S2-22e

The local-only `s2-long-verw-gdt-v1` profile runs exactly bare `0F 00 E8`
(`VERW AX`) at CPL3. Its GDTR base and 16-bit limit are explicit state. Literal
cases cover writable data, read-only data, code, system, DPL/RPL mismatch,
exact/short table bounds, null selector and RPL0 success. Each case checks the
complete GPR state, flags, RIP/retirement, GDTR and unchanged descriptor bytes.
The interpreter performs one read-only 8-byte GDT read for an in-bounds
non-null selector; null and out-of-bounds selectors do not read. Descriptor
table reads use supervisor page privilege while architectural CPL remains 3.
Backend faults propagate without retirement or architectural commit. LDT/TI,
other privilege levels, descriptor loading, hidden caches, native execution
and physical validation remain outside this slice; physical validation is
deferred to the shared final acceptance stage.

Sources: Intel SDM revision 093, Vol.2C (326018-093US), VERR/VERW
pp.5-162–5-163; Vol.3A (325384-093US), §§3.5.1, 6.10.2 and 6.11.5. The
generator records these official sources alongside each literal case.

```sh
python3 tests/cpu/generate_stage2_verw_gdt.py --check
python3 tests/cpu/test_stage2_verw_gdt_generator.py
cargo test -p od-runner --test fixture_v2 verw_gdt_fixtures_check_literal_rights_flags_state_and_descriptor_bytes --offline
```

## CPL3 GDT-only `MOV FS, AX` fixture S2-22f/g

The partial local-only `s2-long-fs-load-v1` profile runs only bare `8E E0`
(`MOV FS, AX`) at Long64/CPL3. Literal inputs cover accessed writable/read-only
data, readable conforming/nonconforming code, invalid system/execute-only and
privilege cases, P=0, a short GDTR limit, and writable/read-only data with A=0.
The runner checks the complete GPR array, RIP, FS base/selector/cache,
unchanged GDTR/RFLAGS, the access-byte-only A update and exact #GP/#NP
vectors/error codes. Unit coverage separately checks nonzero base32, G=0/G=1
limit expansion, A=1 with no write permission, exact read8/write1 trace and
descriptor context, write-fault no-commit behavior, validation priority,
repeated-selector write suppression, and a split adjacent GDT where only the
access byte is writable. These writes require private synchronous RAM and
serial interpreter execution with no external observer/writer, DMA, shared
alias, delayed write or MMIO. The byte-sized backend call specifies semantic
access width only, not physical bus transaction width. This is a local
interpreter profile; native and physical execution are not claimed.

Sources: Intel SDM revision 093, Vol.2B (253667-093US), MOV pp.4-28–4-31;
Vol.3A (325384-093US), §3.4.5.1 pp.3-12–3-13, §5.6.1 pp.5-33–5-35,
§5.7 pp.5-37–5-38, §11.1.2.1 p.11-3 Automatic Locking. The generator records
official PDF links and literal provenance with each case.

```sh
python3 tests/cpu/generate_stage2_fs_load.py --check
python3 tests/cpu/test_stage2_fs_load_generator.py
cargo test -p od-runner --test fixture_v2 fs_load_fixtures_check_literal_descriptor_results_fault_codes_and_full_state --offline
```

## CPL3 GDT-only `MOV GS, AX` fixture S2-22h

The partial local-only `s2-long-gs-load-v1` profile runs only bare `8E E8`
(`MOV GS, AX`) at Long64/CPL3. The 11 bounded literal fixtures cover accessed
writable/read-only data, readable nonconforming code and a DPL0 conforming
code descriptor loadable from CPL3, invalid
system/execute-only and privilege cases, P=0, a short GDTR limit, and writable
and read-only data with A=0. They compare full GPR/RFLAGS/RIP/GDTR and FS
canaries, GS selector/base/cache, descriptor bytes and exact #GP/#NP
error codes. Unit tests additionally cover upper base32 bits, limit expansion,
A=1 without write permission, descriptor access ordering and context, read and
write faults without CPU commit, repeated selectors, and split adjacent GDT
regions. A=0 writes are one semantic byte write at descriptor+5 after the
8-byte read; the backend width does not assert physical bus width. Like the FS
slice, writes require private synchronous RAM and serial execution without
external observers/writers, DMA, shared aliases, delayed writes or MMIO. This
is a local interpreter profile; native and physical execution are not claimed.

The generator takes the opcode bytes and selector/access/flags/base fields as
handwritten literals, then serializes descriptor fields into the architectural
eight-byte layout. It does not derive expected CPU results. Sources: Intel
SDM revision 093, Vol.2B (253667-093US), MOV pp.4-28–4-31;
Vol.3A (325384-093US), §3.4.3–§3.4.5.1, §5.6.1, §5.7, and §11.1.2.1
Automatic Locking, as referenced by the checked-in CPU contract. The generator
records the same official PDF links and literal field provenance.

```sh
python3 tests/cpu/generate_stage2_gs_load.py --check
python3 tests/cpu/test_stage2_gs_load_generator.py
cargo test -p od-runner --test fixture_v2 gs_load_fixtures_check_literal_descriptor_results_fault_codes_and_full_state --offline
```

## Loaded GS.base composition fixture S2-22i

The one local-only composition case runs exactly `8E E8 65 48 8B 00` under
`s2-long-gs-load-v1`: CPL3 loads GS from a literal GDT descriptor, then the
existing `MOV RAX, qword ptr GS:[RAX]` consumes that cached base. The starting
GS.base points at a contrasting canary; the loaded base `0x80000000` selects
the qword at `0x8000000b`. Its offset `0x0b` exceeds the cached limit 0 by
design, consistent with the existing Long64 GS memory contract. The fixture
checks the one A-bit byte update, full state, both memory regions and final
retirement count. Unit trace assertions prove descriptor-origin read8 and
optional write1 at CPL3/pageRing0, followed by the explicit CPL3 data read8
without a GDT reread. Separate unit cases verify later data-read fault
persistence and noncanonical-result #GP(0) with no data callback.

The profile admits the existing exact `8E E8` case and this exact
two-instruction byte sequence with fixed budget/stop boundary; other sequences
are rejected. Instruction bytes, initial state, descriptor/data/canary bytes
and harness fields are handwritten literals. The generator serializes those
fixture inputs; the expected post-state and memory answers are hard-coded in
Rust unit and runner tests, not generated into the fixture. Canonicality and
long-mode no-limit behavior reuse the checks already documented in
CPU_CONTRACT, with no independent manual verification claimed. S2-22 and
ISSUE-017/018 remain open.

```sh
python3 tests/cpu/generate_stage2_gs_load.py --check
python3 tests/cpu/test_stage2_gs_load_generator.py
cargo test -p od-testkit --offline s2_22i
cargo test -p od-runner --test fixture_v2 --offline s2_22i_gs_base_composition_runs_the_literal_two_instruction_profile
```

## Loaded FS.base composition fixture S2-22j

The one local-only composition case runs exactly `8E E0 64 48 8B 00` under
`s2-long-fs-load-v1`: CPL3 loads FS from a literal GDT descriptor, then the
existing `MOV RAX, qword ptr FS:[RAX]` consumes that cached base. The original
FS.base points to a contrasting canary; the descriptor-loaded base
`0x80000000` selects the qword at `0x8000000b`. The fixture checks the one
A-bit byte update, full state, both data regions and retirement count. Unit
trace assertions prove descriptor read8 and optional write1 at CPL3/pageRing0,
then an explicit CPL3 data read8 without a GDT reread. Separate cases verify
that a later data-read fault retains the first instruction and that a
noncanonical result raises #GP(0) before any data callback.

The profile admits the existing exact `8E E0` case and this exact two-
instruction program with fixed budget and stop boundary; other sequences are
rejected. Fixture bytes, initial state, descriptor/data/canary bytes and
harness fields are handwritten literals. The generator serializes inputs;
expected CPU state and memory answers are literals in Rust tests. The existing
CPU_CONTRACT entries support descriptor and canonicality semantics; no
independent manual verification is claimed. S2-22 and ISSUE-017/018 remain
open. This fixture is local-only and is excluded from native selection and
execution.

```sh
python3 tests/cpu/generate_stage2_gs_load.py --check
python3 tests/cpu/test_stage2_gs_load_generator.py
cargo test -p od-testkit --offline s2_22j
cargo test -p od-runner --test fixture_v2 --offline s2_22j_fs_base_composition_runs_the_literal_two_instruction_profile
```

## Null DS/ES loads S2-22k

The separate local-only `s2-long-null-ds-es-v1` profile admits bare `8E D8`
(`MOV DS, AX`) and `8E C0` (`MOV ES, AX`) at Long64/CPL0 only when AX is
zero. Four literal inputs cover each instruction, DS→ES composition and a
repeated DS load. Initial RAX has nonzero high bits and AX=0; DS/ES selectors
start nonzero and usable. The runner checks all GPRs, RFLAGS and their
definedness, both selectors/unusable bits, code bytes, RIP and retirement.
Unit traces also check that no descriptor/data callback occurs, and that
prefix neighbors, another Sreg, nonzero AX, CPL3 and truncated fetch leave
state uncommitted.

This profile does not require or read GDTR/GDT and does not infer a usable
descriptor cache from GDT[0]. It models no DS/ES base or memory access. The
bytes are local semantic fixtures only: they do not prove the order or
execution of an Ubuntu kernel startup prefix. Native/JIT acceptance and the
rest of S2-22 remain open.

```sh
python3 tests/cpu/generate_stage2_null_ds_es.py --check
python3 tests/cpu/test_stage2_null_ds_es_generator.py
cargo test -p od-isa --offline null_ds_es_loads
cargo test -p od-runner --test fixture_v2 --offline s2_22k_null_ds_es_fixtures_check_literal_full_state_and_retirement
```

### S2-30k x87 schema coverage

The schema test admits all eight bare `D8 E8+i` FSUBR encodings. Alias
`D8 E8` is represented with expected `Unsupported`, `stop_rip: null`, and an
instruction budget of one. Local execution tests supply literal success
vectors and compare the unchanged complete CPU/x87 snapshot on that alias.
These cases remain local-only; native x87 admission stays closed.

### S2-30l x87 schema coverage

The schema validator admits all eight bare `DC E0+i` FSUBR encodings.
`DC E0` is represented as expected `Unsupported` with `stop_rip: null` and
instruction budget one. Local literal execution checks ST(0)−ST(i) output,
physical destination placement and preservation of TOP/source state. Runtime
execution now also applies the bounded S2-30cw FCW precision profile; the
schema's existing exact-domain fixture and alias refusal remain unchanged.
These cases are local-only; native x87 admission stays closed.

### S2-30m x87 schema coverage

The schema validator admits all eight bare `DE E0+i` FSUBRP encodings.
`DE E0` is expected to remain Unsupported with `stop_rip: null` and instruction
budget one. Local execution and fixture-runner tests check old ST(i)
destination placement, retention of the popped payload under its Empty tag,
TOP increment and alias atomicity. Native x87 admission stays closed.

### S2-30dc bounded FSUBRP fixture coverage

The schema continues to admit all eight bare `DE E0+i` FSUBRP encodings, with
`DE E0` represented as Unsupported. The handwritten
`s2_30dc_fsubrp_pc24_midpoint_pop.case.json` is a full-state PC24 midpoint and
double-rounding oracle for `DE E2`: it distinguishes `ST(0)-ST(2)` order and
checks the old physical destination, retained payload under Empty tag, TOP
increment, PE/C1, FIP/RIP, exception observation and unchanged code memory.
Runner coverage adds negative directed rounding and JIT denial. The bounded
profile is local-only; native x87 admission and general exception delivery stay
closed.

### S2-30dn large-gap FSUBRP fixture coverage

`s2_30dn_fsubrp_gap65_pop.case.json` is a handwritten literal `DE E3` full-state
fixture. Its physical TOP slot contains 1.0 and old ST(3) contains the actual
`2^-65` operand. PC24 nearest rounding yields 1.0 with PE and incremented C1;
the result is written to old physical ST(3) before TOP=2 is emptied and
incremented. The case checks the popped payload, FIP/RIP, unchanged code bytes,
and an unavailable JIT result. It is local interpreter evidence only; it does
not claim physical x86 acceptance.

### S2-30n x87 schema coverage

The schema validator admits all eight bare `D8 C8+i` FMUL encodings. Literal
runner cases cover every TOP×i pair, including the valid i=0 square, differing
exponents, physical payload sentinels, C1 clearing and preservation of tags and
other state. Exact-only domain failures remain Unsupported atomically; native
x87 admission stays closed.

### S2-30o x87 schema coverage

The schema validator admits all eight bare `DC C8+i` FMUL encodings. Local
runner checks cover every TOP×i physical destination, the valid i=0 square,
different exponents, payload sentinels and atomic rejection of inexact/range
inputs. DC C0..C7 remain FADD, E0..E7 remain FSUBR, and E8..EF remain FSUB.
The forms are local-only; native x87 admission stays closed.

### S2-30p initial x87 schema coverage

The schema validator admits all eight bare `DE C8+i` FMULP encodings. Runner
cases check every TOP×i pair, including the valid i=0 square written into old
physical TOP before that tag is emptied, TOP wrap, C1 clearing and preservation
of other raw state. This original exact-only slice was extended to bounded FCW
precision by S2-30da below. `DE C9` is the supported no-operand
`FMULP ST(1), ST(0)` form. FADDP DE C0..C7, FSUBRP E0..E7 and FSUBP E8..EF
remain distinct; native x87 admission stays closed.

### S2-30cz x87 schema and fixture coverage

The LongX87StateV1 profile continues to admit exactly the eight bare
`DE C0..C7` FADDP encodings. The runner compares the full x87 snapshot for all
TOP/index pairs, including result payload placement before pop, the Empty old
TOP tag, TOP wrap, sticky PE and C1 rounding diagnostics. A gap-64 tie fixture
checks inexact PE with no increment; S2-30dh extends the same profile through
gap 32765 with a raw-integer oracle and a literal gap-65 fixture. The ISA
matrix covers every FCW and sign pair, while i=0 overflow refuses without
state commit. JIT/native admission remains closed; these are interpreter
fixtures, not physical x86 acceptance.

### S2-30dh large-gap FADDP fixture coverage

`s2_30dh_faddp_sti_st0_gap65_up.case.json` is a literal `DE C1` schema-v2 case
with ST(0)=+1 and actual Normal ST(1)=`0000000000000080be3f` (`2^-65`). Its
PC24 round-up result checks the rounded old physical destination, PE/C1, old
TOP emptying, TOP advancement and FIP. The runner checks complete state and
JIT denial; x86-runner source includes a native-admission denial check, which
is Linux-only and did not execute on the macOS validation host. ISA raw-integer
coverage spans gaps 65, 66 and 32765, both operand
placements/signs, all twelve masked FCWs, alias behavior and overflow rollback.
Gap≤64 behavior is retained; the `DC` reverse FADD neighbor and unsupported
overflow cases remain unchanged. At that stage `DE E8..EF` FSUBP still had its
gap-64 limit; S2-30dm extends that form separately. This is interpreter
evidence only, not physical x86 acceptance.

### S2-30df large-gap FADD fixture coverage

`s2_30df_fadd_st0_st1_gap65_up.case.json` is a literal `D8 C1` schema-v2
case with ST(0)=+1 and the actual Normal RHS `0000000000000080be3f`
(`2^-65`). With PC=24 and round-up it checks the inexact increment, sticky PE,
C1, TOP/register preservation and FIP. The runner unit test checks the full x87
snapshot and confirms the JIT backend still denies the instruction. The ISA
suite separately covers gap boundaries through 32765 and verifies `DC`/`DE`
gap-65 rejection. The fixture is interpreter evidence only, not physical x86
acceptance.

### S2-30di large-gap FSUB fixture coverage

`s2_30di_fsub_st0_st1_gap65_up.case.json` is a literal `D8 E1` schema-v2
case with ST(0)=+1 and actual Normal RHS `0000000000000080be3f` (`2^-65`).
PC=53 round-up yields +1 with sticky PE and C1. The runner checks the full x87
snapshot and confirms JIT denial; ISA coverage independently checks gaps 64,
65, 66 and 32765 and atomic DC/DE large-gap refusal. This is interpreter
evidence only, not physical x86 acceptance.

### S2-30dk large-gap reverse-destination FSUB fixture

`s2_30dk_fsub_sti_st0_gap65_up.case.json` is a literal `DC E9` schema-v2
case. It keeps actual `ST(0)=0000000000000080be3f` (`2^-65`) and subtracts it
from `ST(1)=+1` at PC=53, round-toward-positive. The rounded result is written
to old physical ST(1), with sticky PE and C1 set. The runner checks the full
x87 snapshot and confirms JIT denial. ISA oracle coverage separately checks
gaps 64, 65, 66 and 32765 and atomic `DC E0..E7`/`DE E8..EF` neighbor refusal.
This is interpreter evidence only, not physical x86 acceptance.

### S2-30dj large-gap FSUBR fixture coverage

`s2_30dj_fsubr_st0_st1_gap65_up.case.json` is a literal `D8 E9` schema-v2
case with old physical TOP=+1 and actual Normal `ST(1)=0000000000000080be3f`
(`2^-65`). PC=53 round-up computes `ST(1)-ST(0)` directly and produces the
negative predecessor of one, with sticky PE and C1 cleared for this rounding.
The runner checks the full
x87 snapshot and confirms JIT denial. ISA integer-oracle coverage checks gaps
64, 65, 66 and 32765 and verifies that DC/DE gap-65 neighbors still refuse.
This is interpreter evidence only, not physical x86 acceptance.

### S2-30dl large-gap reverse-destination FSUBR fixture

`s2_30dl_fsubr_sti_st0_gap65_up.case.json` is a literal `DC E1` schema-v2
case with actual `ST(0)=+1` and `ST(1)=2^-65`. At PC53 round-toward-positive,
`ST(0)-ST(1)` rounds to +1 and is written to old physical `ST(1)` without a
pop; sticky PE and C1 are checked alongside TOP, tags, FIP and the full state.
The runner also confirms JIT denial. ISA coverage independently checks gaps
64/65/66/32765 and confirms that `DC E8..EF` FSUB retains its prior admission
while `DE E0..E7` FSUBRP remains capped. This is interpreter evidence only,
not physical x86 acceptance.

### S2-30dg large-gap reverse FADD fixture coverage

`s2_30dg_fadd_sti_st0_gap65_up.case.json` is a literal `DC C1` schema-v2
case with ST(0)=+1 and actual Normal ST(1)=`0000000000000080be3f` (`2^-65`).
Its PC24 round-up result checks the old physical `ST(1)` destination, sticky
PE/C1, FIP, TOP/tags and full interpreter snapshot. The runner test also checks
JIT denial; the native admission test reads this same fixture and rejects the
x87 operation. ISA raw-integer coverage spans gaps 65, 66 and 32765, both
operand orders/signs and all twelve masked FCWs. `DE C1` FADDP continues to
refuse gap 65. This is interpreter evidence only, not physical x86 acceptance.

### S2-30da x87 schema and fixture coverage

The existing schema admits every bare `DE C8..CF` FMULP encoding. The
`fixture_v2` runner suite includes the static case and compares the complete
post-execution CPU state, exception observation and memory bytes. The runner
also checks inexact FCW rounding and the `i=0` square before popping the old
TOP. The static
[`PC64 inexact FMULP fixture`](s2_30da_fmulp_pc64_inexact_pop.case.json)
checks PE, rounded payload placement, old TOP emptying and TOP advancement.
The eight encodings remain interpreter-only; native/JIT admission stays
closed.

### S2-30db x87 schema and fixture coverage

The LongX87StateV1 profile admits all bare `DE E8..EF` FSUBP encodings. The
static [PC24 inexact fixture](s2_30db_fsubp_pc24_inexact_pop.case.json)
executes `FSUBP ST(1), ST(0)` with a midpoint
result, then checks the full CPU snapshot: rounded destination bytes are
written before the old TOP tag is emptied, TOP advances, PE becomes sticky,
and C1 records the increment. The runner also verifies exception observation,
unchanged memory and JIT denial. Direct ISA coverage spans all twelve FCWs,
every TOP/index pair, operand signs and prior PE/C1 histories. Native x87 and
general exception delivery remain unsupported; no physical x86 acceptance is
claimed.

### S2-30dm large-gap FSUBP fixture coverage

`s2_30dm_fsubp_sti_st0_gap65_down.case.json` is a literal `DE E9` schema-v2
case with ST(0)=+1 and actual Normal ST(1)=`0000000000000080be3f` (`2^-65`).
With PC24 and round-toward-negative, it checks the rounded negative result,
sticky PE/C1, old physical destination write, old TOP emptying, TOP advance
and FIP. The runner compares complete CPU state, exception observation and
unchanged code memory, then confirms JIT denial. Direct ISA coverage spans
gaps 64/65/66/32765, all twelve FCWs, both operand orders, all sign pairs,
every TOP/index placement and both prior PE/C1 histories. Alias cancellation
and a large-gap helper refusal are atomic. `DE E0..E7` FSUBRP and the `DC` and
`D8` neighbors remain unchanged; no physical x86 acceptance is claimed.

### S2-30q x87 schema coverage

The schema validator admits all eight bare `D8 F0+i` FDIV encodings. The
runner checks each TOP×i placement, exact `+1` alias behavior, C1 clearing,
tag/TOP preservation and atomic rejection of an inexact quotient. `D8 F1`
with no stop RIP represents the inexact Unsupported case. Other D8 division
directions, DC/DE forms and memory/integer forms remain unsupported; native
x87 admission stays closed.

### S2-30r x87 schema coverage

The schema validator admits all eight bare `DE F8+i` FDIVP encodings, including
the no-operand `DE F9` alias. Runner cases check every TOP×i destination,
ordered `15/3 = 5`, exact `+1` alias payload before the old TOP tag is emptied,
TOP wrap, C1 clearing and full x87 snapshot preservation. The bounded S2-30dd
extension below adds the twelve FCWs, all RC modes, PE/C1 rounding diagnostics,
and valid inexact quotients; the old exact-only inexact-rejection expectation no
longer applies. Invalid profile inputs remain atomically Unsupported. The
distinct `DE F0..F7` FDIVRP encoding retains its exact-only profile; native x87
admission stays closed.

### S2-30dd bounded FDIVP fixture coverage

The schema-v2 runner executes a handwritten `DE FA` PC24, round-toward-negative
fixture for `ST(2)=-1` and `ST(0)=3`, checking the full state after negative
`1/3` rounding and popping old TOP. The static fixture also verifies unchanged
memory and JIT denial. The runner matrix exercises every admitted FCW, while
direct ISA tests cover all FCW×TOP×index×sign×PE/C1 combinations and a gap-65
case. Invalid controls, status, tags and operands refuse without committing
state. `DE F0..F7` remains the distinct reverse quotient FDIVRP encoding.

### S2-30de bounded FDIVRP fixture coverage

The schema-v2 runner executes a handwritten `DE F2` PC24, round-toward-negative
fixture for old `ST(0)=-1` divided by old `ST(2)=3`, checks the full state
after rounding to `-1/3` and popping old TOP, confirms unchanged memory, and
denies JIT execution. Direct ISA coverage spans all FCW×TOP×index×sign×PE/C1
combinations and gap-65 operands. Exact and alias cases verify the ordered
quotient and destination-before-pop rules. The DE F1 stop-null schema allowance
covers result-range preflight refusals such as overflow/underflow. Selector
refusals are covered by direct ISA tests because schema-v2 does not admit x87
selectors with DE F1; valid inexact quotients are admitted.

### S2-30s x87 schema coverage

The schema validator admits all eight bare `DE F0+i` FDIVRP encodings,
including the no-operand `DE F1` alias. Runner cases check every TOP×i
destination, ordered `15/3 = 5`, exact `+1` alias payload before the old TOP
tag is emptied, TOP wrap, C1 clearing and unchanged unrelated state. Under
S2-30de, valid inexact `DE F1` quotients round and retire; out-of-domain
results such as overflow refuse atomically with `stop_rip: null` and budget one.
`DE F8..FF` FDIVP keeps its ordered quotient and pop semantics, with bounded
rounding extended by S2-30dd; native x87 admission stays closed.


### S2-30t x87 schema coverage

The schema validator admits all eight bare `D8 F8+i` FDIVR encodings;
`D8 F9` is `FDIVR ST(0), ST(1)`. Runner cases check every TOP×i
destination, ordered `15/3 = 5`, exact `+1` alias behavior, C1 clearing and
preservation of tags/TOP and unrelated raw state. The formerly inexact `D8 F9`
case is now accepted by the local bounded-FCW interpreter profile and checks
rounded p64 `1/3`, sticky PE and C1 replacement. The profile requires absent
FCS/FDS, one of twelve all-masked PC24/53/64 FCWs, clean required FSW bits, and
canonical nonzero Normal operands; the self-index validates only TOP. Other
invalid inputs still refuse atomically. `D8 F0..F7` FDIV remains distinct;
native x87 admission stays closed. See [S2-30cs](../../../../docs/reports/S2_30CS_X87_FDIVR_PRECISION.md).

### S2-30u x87 schema coverage

The instruction schema accepts the eight bare `DC F0+i` forms in the local
interpreter profile. The runner tests every TOP×i destination, `ST(0)=6` over
`ST(i)=3`, the exact `i=0` +1 alias, sign combinations, normal-range edges and
`15/3=5` to verify operand order. An inexact `DC F1` case is Unsupported with
no stop RIP and budget one, and compares the complete state. Exhaustive DC
ModR/M and prefix checks preserve existing neighboring groups. Native x87
admission explicitly rejects all eight encodings.

### S2-30v x87 schema coverage

The instruction schema accepts the eight bare `DC F8+i` FDIV forms in the
local interpreter profile. The runner tests every TOP×i destination, ordered
`15/3=5`, the exact `i=0` +1 alias, and payload/TOP/tag preservation. Its
inexact `DC F9` case is Unsupported with no stop RIP and budget one, and
compares the complete state. `od-isa` literal vectors additionally cover sign
combinations and Normal-range edges. Exhaustive DC ModR/M and prefix checks
preserve `DC F0..F7` FDIVR and adjacent groups. Native x87 admission explicitly
rejects all eight new encodings.


### S2-30cy x87 reverse division coverage

The schema admits all eight bare `DC F0..F7` FDIVR encodings. `od-isa`
execution tests cover every TOP/index placement, all twelve all-masked FCWs,
rounding/sign and PE/C1 histories, exact p64 results with differing exponents,
and a gap greater than 64. The testkit runner checks the exact `i=0` +1 alias
and changes the former inexact `DC F1` rejection into a literal rounded p64
1/3 success, checking PE/C1, FIP/RIP, one retirement and full-state equality.
Invalid profiles still refuse atomically. Other DC directions remain distinct
and native x87 admission stays closed. See [S2-30cy report](../../../../docs/reports/S2_30CY_X87_FDIVR_REVERSE_PRECISION.md).

### S2-30w x87 schema and fixture coverage

The schema admits all eight bare `DD D8+i` FSTP ST(i) encodings in the local
interpreter profile. `s2_30w_fstp_invalid_source.case.json` is a literal
`DD D9` rejection case with canonical-Normal ST(0) tagged Empty; it uses
`stop_rip: null` and budget one and checks zero retirement plus
full-state preservation through the CLI runner. Interpreter unit vectors cover
all TOP×i pairs, each old destination tag state for `i!=0`, exact ten-byte
source copies including signed and Normal-edge payloads, `i=0` and `i=1`, and
atomic environment/source rejection. All DD ModR/M bytes and prefix/truncation
cases are checked. Native x87 admission explicitly denies all eight forms.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating-Point
Value,” pp.3-386–3-387 ([official PDF, Vol.2A p.3-386](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504)).

### S2-30aa x87 schema and fixture coverage

The schema admits exactly the D9 E0 FCHS, D9 E1 FABS and bare D9 E4 FTST
forms. Two literal FTST success fixtures cover positive and negative
canonical-Normal ST(0), including a condition-code field initially set to all
ones. The Empty-tag rejection fixture has `stop_rip: null` and instruction
budget one; the CLI verifies full state preservation and zero retirement.
Local interpreter vectors additionally cover every TOP, source sign and
Normal endpoints, all initial condition-code combinations, unchanged raw
payload/tag sentinels, FTST followed by FNSTSW AX, and fetch/truncation/STOP/
budget atomicity. Native x87 admission explicitly denies D9 E4.

Normative source: Intel SDM revision 093, Vol.2A, “FTST—Test,” pp.3-400–3-401
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-30ab x87 schema and fixture coverage

The schema admits bare `D8 D1` FCOM ST(1) and the exact four-byte
`D8 D1 DF E0` sequence used to observe its status through FNSTSW AX. Three
literal success fixtures cover ST(0)>ST(1), equality, and ST(0)<ST(1), with
physical register/tag sentinels. `s2_30ab_fcom_st1_invalid_rhs_tag.case.json`
has canonical-Normal ST(1) bytes but an Empty tag, `stop_rip: null`, and budget
one; the CLI confirms unchanged full state and zero retirement. Interpreter
vectors cover all TOP values and condition-code inputs, sign/exponent/all-bit
significand ordering, both operands' independent invalid tags/classes,
environment rejection, and fetch/truncation/STOP/budget atomicity. Exhaustive
D8 ModR/M checks keep arithmetic neighbors intact; native admission explicitly
denies `D8 D1`.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-30ac x87 schema and fixture coverage

The schema admits exactly the eight bare `D8 D0+i` FCOM register forms and
`D8 D0+i DF E0` for status observation. The S2-30ab three-relation fixtures
remain unchanged. `s2_30ac_fcom_st7_equal.case.json` adds a literal index-7
success case; `s2_30ac_fcom_st7_invalid_rhs_tag.case.json` has canonical-Normal
ST(7) bytes with an Empty tag, `stop_rip: null`, and budget one, and the CLI
checks full-state atomic rejection. Additional runner vectors execute every
source index. Interpreter vectors cover every TOP×i pair, including i=0 alias
and TOP wrap. The schema rejects D8 D8..DF and every prefixed or extended form.
Native admission explicitly denies all eight D8 D0..D7 encodings.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-30ad x87 schema and fixture coverage

The schema admits bare `D8 D9` FCOMP ST(1) and the exact four-byte
`D8 D9 DF E0` sequence for status observation. The literal success fixture
compares equal canonical-Normal ST(0)/ST(1), empties the old physical TOP tag,
and advances TOP without changing register payload bytes. The literal reject
fixture has an Empty ST(1) tag, `stop_rip: null`, and budget one; the CLI checks
full-state atomic rejection. Interpreter vectors cover every TOP including
TOP=7 wrap, all initial condition-code combinations, relation/sign/exponent/
low-significand ordering, independent lhs/rhs tag and class rejection, and
fetch/truncation/STOP/budget atomicity. The schema rejects D8 D8 and D8 DA..DF
along with prefixes, REX and WAIT. Native admission explicitly denies D8 D9.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-30x x87 schema and fixture coverage

The schema admits all eight bare `DD D0+i` FST ST(i) encodings. The literal
`s2_30x_fst_invalid_source.case.json` uses `DD D1` with a canonical-Normal
ST(0) tagged Empty, `stop_rip: null`, and budget one to verify atomic source-tag
rejection through the CLI runner. Runner checks cover each destination index
and verify raw payload placement, Valid destination tagging, unchanged TOP and
FSW, and full x87 state. Local vectors additionally exercise every TOP×i pair
and all old destination tags for i!=0. All DD ModR/M values and prefix,
REX, WAIT and truncation behavior are checked; `DD D8..DF` remains FSTP.
Native x87 admission explicitly denies all eight FST encodings.

### S2-30y x87 schema and fixture coverage

The schema admits the bare `D9 E0` FCHS success fixture with canonical-Normal
ST(0), C1 set, and a stop address after one instruction. The CLI verifies that
only the raw ST(0) sign bit changes, C1 clears, FIP/RIP update, and the rest of
the snapshot stays intact. `s2_30y_fchs_invalid_source.case.json` contains a
canonical-Normal payload with an Empty source tag; it has `stop_rip: null` and
budget one, so the CLI confirms a real zero-retirement atomic rejection.
Interpreter vectors separately cover all TOPs, both signs, Normal range edges,
double FCHS, all D9 second bytes, prefixes and execution boundaries. Native x87
admission explicitly denies `D9 E0`.

### S2-30ao signed-zero FCHS

The `s2_30ao_fchs_signed_zero.case.json` Zero-tagged positive-zero fixture and
the `s2_30ao_fchs_zero_wrong_tag.case.json` no-stop, budget-one fixture with
canonical zero bytes and a Valid tag exercise `D9 E0` through the interpreter CLI.
Unit coverage checks
both zero signs × every TOP × all 16 prior condition-code combinations, full
state, double-operation restoration, malformed payloads and atomic execution
boundaries. FLD m32/m64 → FCHS → FST → FNSTSW composition is checked locally;
the `s2_30ao_fld_fchs_fst_fnstsw.case.json` project fixture runs the same
sequence from binary32 negative zero and checks its full state and memory.
these project fixtures are not native or physical evidence. Native/JIT remains
closed under ISSUE-019.

Normative source: Intel SDM revision 093, Vol.2A, “FCHS—Change Sign,” p.3-318 ([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=436)).

### S2-30ap signed-zero FABS

The `s2_30ap_fld_fabs_fst_fnstsw.case.json` handwritten project fixture uses
exactly `d900d9e1d910dfe0`: FLD m32fp loads negative zero, FABS clears its sign,
FST writes positive zero, and FNSTSW observes the final TOP. It retires four
instructions and checks full state and memory through the interpreter CLI.
Local vectors cover both zero signs × all TOP × all prior condition-code
states, repeated-FABS idempotence, malformed Zero/wrong-tag/environment inputs,
and execution boundaries; Normal and FCHS behavior remain covered by their
existing cases. At the time, FST/FSTP ST(i) remained Normal-only; S2-30aq later
extends FTST, and S2-30ar/as later extend bare FST/FSTP respectively. This is project-fixture
evidence, not native or physical evidence; ISSUE-019 remains open.

### S2-30aq signed-zero FTST

The handwritten `s2_30aq_fld_ftst_fnstsw.case.json` fixture uses exactly
`d900d9e4dfe0`: FLD m32fp loads negative zero, FTST sets C3 for equality, and FNSTSW
observes the final status. It retires three instructions and checks full state and memory through the
interpreter CLI. Unit coverage includes m32/m64 FLD compositions for both zero
signs, both signs × every TOP and initial condition-code combination, repeated
FTST, malformed Zero/tag/environment inputs, the Normal-only FST/FSTP ST(i)
guard as of S2-30aq, and decode/fetch/STOP/budget boundaries.
Schema admission is limited to this exact composition; one-op FTST native
admission and JIT execution remain closed. No physical x87 behavior is claimed.

### S2-30at canonical signed-infinity FTST

The handwritten `s2_30at_fld_ftst_positive_infinity_fnstsw.case.json` and
`s2_30at_fld_ftst_negative_infinity_fnstsw.case.json` both use the exact
existing `d900d9e4dfe0` sequence. FLD m32fp loads binary32 positive or negative
infinity, FTST compares against zero, and FNSTSW AX observes status `0x3800`
for positive infinity or `0x3900` for negative infinity. Each retires three
instructions and preserves the Special-tagged binary80 payload. Fixtures are
project evidence, not native evidence; the schema is unchanged and JIT/native
admission remain closed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-30at-canonical-signed-infinity-ftst-d9-e4),
[plan](../../../../docs/plans/STAGE_2.md#s2-30at-admit-canonical-signed-infinity-to-bounded-ftst-d9-e4),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-30at-canonical-signed-infinity-through-bounded-ftst).

### S2-30ar signed-zero FST ST(i)

The handwritten project fixture `s2_30ar_fld_fst_sti_fnstsw.case.json` uses
exactly `d900ddd1dfe0`: FLD m32fp loads negative zero, FST ST(1) copies the
original ten bytes and Zero tag, and FNSTSW observes TOP. It retires three
instructions and checks full state and memory through the interpreter CLI.
Local vectors cover both zero signs × all TOP × all eight indices × all 16
condition-code combinations, old destination payload/tag sentinels, alias and
idempotence, malformed Zero/wrong-tag/environment inputs, the then-current FSTP zero
rejection (historical; superseded by S2-30as) and execution boundaries. Existing
Normal FST/FSTP cases remain. This is project-fixture evidence, not native or
physical evidence; ISSUE-019 remains open.

### S2-30au canonical signed-infinity FST/FSTP ST(i)

Four handwritten `project_fixture` cases use the existing exact byte patterns
`d900ddd1dfe0` and `d900ddd9dfe0` with positive and negative binary32 infinity.
Each runs FLD m32fp, FST or FSTP ST(1), then FNSTSW. The fixtures check raw
canonical binary80 payload copying, Special tags, the distinct no-pop/pop TOP
and tag results, full state, memory, and the canary. Unit coverage also composes
m32 and m64 infinity loads with both store forms for both signs. The fixture
schema remains unchanged; JIT/native and physical behavior are not established,
so ISSUE-019 remains open. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-30au-canonical-signed-infinity-through-fstfstp-sti),
[plan](../../../../docs/plans/STAGE_2.md#s2-30au-admit-canonical-signed-infinity-to-bounded-fstfstp-sti),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-30au--canonical-signed-infinity-through-fstfstp-sti).

<a id="s2-30az-canonical-qnan-fst-fstp-sti"></a>

### S2-30az canonical binary80 QuietNaN FST/FSTP ST(i)

Four handwritten schema-v2 `project_fixture` cases use the unchanged
`d900ddd1dfe0` and `d900ddd9dfe0` streams with positive and negative
payload-bearing binary32 QNaNs. Each runs FLD m32fp, FST or FSTP ST(1), then
FNSTSW. They compare the exact extended bytes/sign, Special tags, full state,
memory and canary; the pop and no-pop cases have distinct TOP/tag results.
Direct interpreter compositions also cover m32 and m64 loads for both signs
and both stores. The schema and JIT/native admissions are unchanged. Local
evidence does not establish physical x87 behavior; ISSUE-019 remains open. See
the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-30az-canonical-qnan-fst-fstp-sti),
[plan](../../../../docs/plans/STAGE_2.md#s2-30az-admit-canonical-binary80-quietnan-to-bounded-fstfstp-sti),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-30az--canonical-binary80-quietnan-through-bounded-fstfstp-sti).

<a id="s2-30ba-canonical-snan-fst-fstp-sti"></a>

### S2-30ba canonical binary80 SignalingNaN FST/FSTP ST(i)

Four handwritten schema-v2 fixtures seed canonical SNaN raw bytes and Special
tags, then execute bare `ddd1` or `ddd9` with budget one and stop RIP two bytes
later. Both signs are covered. The wrong-tag `ddd1` fixture uses the same
canonical SNaN bytes with a Valid tag and expects unsupported execution without
retirement or RIP movement. Runner checks compare complete state and raw bytes
and deny JIT for successful fixtures. The wire schema is unchanged; the narrow
`od-testkit` profile allowlist now recognizes only bare DD D0+i / DD D8+i
register-store single-op streams. Unit compositions load raw m80fp values,
copy to ST(1), and observe status with FNSTSW while asserting Q remains clear
and IE remains absent. No m32/m64 SNaN load composition or physical x86 claim is
included. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-30ba-canonical-snan-fst-fstp-sti),
[plan](../../../../docs/plans/STAGE_2.md#s2-30ba-admit-canonical-binary80-signalingnan-to-bounded-fstfstp-sti),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-30ba--canonical-binary80-signalingnan-through-bounded-fstfstp-sti).

<a id="s2-30bb-canonical-binary80-subnormal-fst-fstp-sti"></a>

### S2-30bb canonical binary80 subnormal FST/FSTP ST(i)

Four handwritten schema-v2 fixtures seed canonical nonzero binary80
subnormals with the Special tag, then execute bare `ddd1` or `ddd9` with budget
one and stop RIP two bytes later. Both signs are covered. The wrong-tag `ddd1`
fixture uses a nonzero subnormal with a Valid tag and checks atomic rejection.
Runner checks compare complete state and raw bytes and deny JIT for successful
fixtures. The wire schema and `od-testkit` profile are unchanged; the bare
register-store streams were already admitted for S2-30ba. Local checks do not
establish physical x87 behavior; ISSUE-019 remains open. See the [CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-30bb-canonical-subnormal-fst-fstp-sti),
[plan](../../../../docs/plans/STAGE_2.md#s2-30bb-admit-canonical-binary80-subnormal-to-bounded-fstfstp-sti),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-30bb-canonical-binary80-subnormal-through-bounded-fstfstp-sti).


### S2-30av canonical signed-infinity FCHS/FABS

The handwritten `s2_30av_fld_fchs_fst_fnstsw.case.json` and
`s2_30av_fld_fabs_fst_fnstsw.case.json` fixtures use exactly
`d900d9e0d910dfe0` and `d900d9e1d910dfe0`. FLD m32fp loads negative infinity,
the sign operation produces positive infinity, FST writes it back, and FNSTSW
observes TOP. Each fixture retires four instructions. A no-stop, budget-one
fixture carries canonical positive-infinity bytes with a Valid tag and checks
atomic FCHS rejection. Interpreter vectors cover both opcodes, signs, all TOPs
and prior condition-code states, full-state canaries, repeated operations,
malformed neighboring classes, wrong tags, invalid environments and execution
boundaries. m32/m64 compositions cover both signs. The schema is unchanged and
JIT/native admission remains closed; local fixture evidence does not establish
physical x86 behavior, so ISSUE-019 remains open. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-30av-canonical-signed-infinity-fchs-fabs),
[plan](../../../../docs/plans/STAGE_2.md#s2-30av-admit-canonical-signed-infinity-to-bounded-fchsfabs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-30av--canonical-signed-infinity-through-bounded-fchsfabs).

### S2-30aw canonical binary80 QuietNaN FCHS/FABS

Four handwritten schema-v2 fixtures use existing m32 payload-bearing QNaN
sequences through FLD → FCHS or FABS → FST → FNSTSW, covering both input signs
and operations. Direct interpreter coverage adds the same compositions for
m64. They check extended QNaN payload/sign bits, Special tag and status word;
CLI fixtures are denied by JIT admission. The separate no-stop, budget-one
FCHS fixture has canonical QNaN bytes with a Valid tag and checks atomic
rejection. Fixture schema is unchanged; project fixtures are not native
evidence. ISSUE-019 remains open. See the [CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-30aw-canonical-qnan-fchs-fabs),
[plan](../../../../docs/plans/STAGE_2.md#s2-30aw-admit-canonical-binary80-quietnan-to-bounded-fchsfabs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-30aw--canonical-binary80-quietnan-through-bounded-fchsfabs).

### S2-30ax canonical binary80 SignalingNaN FCHS/FABS

Four handwritten schema-v2 fixtures seed canonical Q=0 binary80 SNaNs with
nonzero low-62 payloads directly in ST(0), then run bare `D9 E0` or `D9 E1`
with budget one. They cross both signs and operations; the CLI runner checks
full state and JIT denial. A separate wrong-tag fixture confirms zero
retirement and unchanged state. An od-isa unit composition inspects raw bytes
immediately after `FLD m80fp` and after the sign operation, stores with
`FSTP m80fp`, and observes status with `FNSTSW AX`, without converting through
m32/m64 FLD (which quiets an SNaN and raises IE). Local fixtures are not
physical x86 evidence; ISSUE-019 remains open. See the [CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-30ax-canonical-snan-fchs-fabs),
[plan](../../../../docs/plans/STAGE_2.md#s2-30ax-admit-canonical-binary80-signalingnan-to-bounded-fchsfabs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-30ax--canonical-binary80-signalingnan-through-bounded-fchsfabs).

### S2-30ay canonical binary80 subnormal FCHS/FABS

Four handwritten schema-v2 fixtures seed raw canonical binary80 subnormals
(E=0, J=0, nonzero fraction) directly in ST(0), then run one bare `D9 E0`
or `D9 E1` with budget one. They cover both source signs and operations; the
runner checks exact fraction/sign bytes, full state and JIT denial. A separate
wrong-tag fixture confirms zero retirement and unchanged state. An od-isa raw
`FLD m80fp` → sign operation → `FSTP m80fp` → `FNSTSW AX` unit composition
checks bytes after load, operation and store without m32/m64 conversion and
confirms no DE. Local fixtures are not physical x86 evidence; ISSUE-019 remains
open. See the [CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-30ay-canonical-subnormal-fchs-fabs),
[plan](../../../../docs/plans/STAGE_2.md#s2-30ay-admit-canonical-binary80-subnormal-to-bounded-fchsfabs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-30ay--canonical-binary80-subnormal-through-bounded-fchsfabs).

### S2-30z x87 schema and fixture coverage

The schema admits only bare `D9 E1` for the FABS slice. The positive literal
fixture starts with a negative canonical-Normal ST(0), C1 set and C0/C2/C3 set;
the CLI checks that only the raw sign bit clears, C1 clears, FIP/RIP update,
and all other snapshot fields remain unchanged. The invalid-source fixture has
the same Normal bytes with the physical TOP tag Empty, `stop_rip: null`, and
budget one, so the CLI checks a genuine zero-retirement atomic rejection.
Interpreter vectors cover all TOP values, both source signs, Normal boundaries,
idempotence, undefined-condition-bit retention, exhaustive D9 second-byte
decoding, prefixes and execution boundaries. Native x87 admission explicitly
denies `D9 E1`.

Normative source: Intel SDM revision 093, Vol.2A, “FABS—Absolute Value,” p.3-309 ([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=427)).

### S2-31a FRNDINT schema and fixture coverage

The schema admits bare `D9 FC` and the exact four-byte composition
`D9 FC DF E0` (FNSTSW AX). The success fixture rounds a positive 1.5 tie to
2.0 under nearest-even, setting sticky PE and C1 while preserving
C0/C2/C3; the composition fixture makes the resulting FSW observable in AX.
The no-stop, budget-one fixture has an Empty old physical TOP tag and confirms
atomic Unsupported. Interpreter vectors cover all TOP×RC×sign×exact/tie/
below/above combinations, prior sticky PE, condition-bit retention, both signed
2^63 results, rejected exponent endpoints and raw classes, invalid environment,
and fetch/truncation/STOP/budget boundaries. RC values and pre-existing PE are
accepted only by the FRNDINT interpreter gate; native admission explicitly
denies D9 FC.

Normative source: Intel SDM revision 093, Vol.2A, “FRNDINT—Round to Integer,”
p.3-372 ([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=490)).

### S2-31dc FRNDINT signed-zero composition

The literal `s2_31dc_frndint_negative_zero_fnstsw.case.json` runs bare `D9 FC`
on physical TOP=5 containing canonical negative binary80 zero with the Zero
tag, then executes `DF E0` to expose the resulting FSW in AX. C1 clears while
sticky PE and C0/C2/C3 remain set; the ten-byte negative-zero payload, TOP,
tag word, FDP/FOP and every unrelated physical register remain unchanged. The
interpreter separately covers both signs and all sign×FCW×TOP×PE×C1 cases. The
existing S2-31a positive tie fixture remains as the Normal-rounding CLI
regression. Native/JIT denial remains active. See the [S2-31dc CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31dc-canonical-signed-zero-frndint-d9-fc), [plan](../../../../docs/plans/STAGE_2.md#s2-31dc-admit-signed-zero-to-bounded-frndint-d9-fc), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31dc--canonical-signed-zero-through-bounded-frndint).

### S2-31dd FRNDINT exact-integer Normal composition

The literal `s2_31dd_frndint_large_exact_integer_fnstsw.case.json` runs bare
`D9 FC` followed by `DF E0` on a canonical-Normal binary80 value with
unbiased exponent 63 and a mixed significand. It confirms byte-for-byte
identity, sticky PE/C0/C2/C3 retention, C1 clearing, and FNSTSW visibility.
Interpreter coverage adds the exponent/significand/sign/TOP/FCW/PE/C1 matrix
and the e=62 fractional boundary. Native/JIT denial remains active. See the
[S2-31dd CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31dd-exact-integer-normal-frndint-d9-fc), [plan](../../../../docs/plans/STAGE_2.md#s2-31dd-admit-exact-integer-normal-values-to-bounded-frndint-d9-fc), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31dd--exact-integer-normal-values-through-bounded-frndint).

### S2-31de FRNDINT canonical signed-infinity composition

The literal `s2_31de_frndint_negative_infinity_fnstsw.case.json` executes
`FLD m80fp`, bare `D9 FC`, then `DF E0` on negative canonical binary80
infinity. The test checks the raw ten-byte payload, Special tag, resulting TOP,
FIP/FDP/FOP, unchanged unrelated register bytes, and `FNSTSW AX` observation.
The interpreter separately checks all sign×FCW×TOP×PE×C1 cases and rejects
pseudo-infinity, NaNs, malformed fractions/exponents and tag mismatches. The
JIT remains denied and native admission stays fail-closed. See the [S2-31de
CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31de-canonical-signed-infinity-frndint-d9-fc),
[plan](../../../../docs/plans/STAGE_2.md#s2-31de-admit-canonical-signed-infinity-to-bounded-frndint-d9-fc),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31de--canonical-signed-infinity-through-bounded-frndint).

### S2-31df FRNDINT canonical quiet-NaN composition

The literal `s2_31df_frndint_quiet_nan_fnstsw.case.json` executes `FLD m80fp`,
bare `D9 FC`, and `DF E0` on a Special-tagged canonical QNaN with a nonzero
payload. It checks the exact ten-byte payload, Special tag, TOP, status and
FIP/FDP/FOP after `FNSTSW AX`. Interpreter coverage spans four significands,
both signs, every FCW/TOP/PE/old-C1 combination, while rejects cover SNaN,
pseudo-NaN, wrong tags and invalid controls/status. Direct `D9 FC` JIT
compilation is denied and native x87 admission remains fail-closed. See the
[S2-31df CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31df-canonical-quiet-nan-frndint-d9-fc),
[plan](../../../../docs/plans/STAGE_2.md#s2-31df-admit-canonical-quiet-nan-to-bounded-frndint-d9-fc),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31df--canonical-quiet-nans-through-bounded-frndint).

### S2-31dg FRNDINT signaling-NaN quieting and exception composition

The literal `s2_31dg_frndint_signaling_nan_fnclex.case.json` executes
`FLD m80fp`, `D9 FC`, a second `D9 FC` while IE is still sticky, `DF E0`,
`DB E2`, and a final `D9 FC` on a canonical Special-tagged SNaN. It covers
successful quiet-NaN re-entry with historical IE, IE observation, FNCLEX
clearing, and another successful execution. The independent 1,024-case matrix checks
only the quiet bit changes, IE is added without PE, old PE and condition bits
are preserved, C1 clears, and all non-result state stays unchanged. Explicit
boundary controls cover the existing zero-payload infinity and QNaN identity;
reject cases cover J=0, wrong tag, bad
FCW/FSW, prefix, fetch, truncation, STOP and budget. JIT/native admission stays
closed. See the [S2-31dg CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31dg-canonical-signaling-nan-frndint-d9-fc), [plan](../../../../docs/plans/STAGE_2.md#s2-31dg-quiet-canonical-signaling-nan-in-bounded-frndint-d9-fc), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31dg--canonical-signaling-nans-through-bounded-frndint).

### S2-31dh FRNDINT historical IE admission

The bare `D9 FC` gate admits a pre-existing sticky IE while preserving
independent current-source behavior. Interpreter coverage crosses every
admitted source class with sign, FCW, TOP, IE/PE and prior C1. A second
FRNDINT on the QNaN produced by SNaN quieting succeeds without FNCLEX; the
literal `FLD m80fp`→`FRNDINT`→`FRNDINT`→`FNSTSW AX`→`FNCLEX`→`FRNDINT` fixture
verifies CLI status observation, quiet-NaN re-entry and clearing. JIT/native admission
remains denied. See the [S2-31dh CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31dh-historical-sticky-ie-in-bounded-frndint-d9-fc), [plan](../../../../docs/plans/STAGE_2.md#s2-31dh-admit-historical-sticky-ie-to-bounded-frndint-d9-fc), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31dh--historical-sticky-ie-through-bounded-frndint).

### S2-31dj FRNDINT historical DE admission

The seeded raw `s2_31dj_frndint_seeded_de_fnstsw.case.json` runs `D9 FC DF E0`
with canonical-Normal input and IE/DE/PE already sticky under FCW `0x037f`.
It verifies that DE survives and the prior C1 clears as the rounded signed
zero and `FNSTSW AX` result are observed. The literal
`s2_31dj_frndint_fld_m32_subnormal_fnstsw.case.json` and
`s2_31dj_frndint_fld_m64_subnormal_fnstsw.case.json` execute minimum-subnormal
`FLD m32fp`/`FLD m64fp`, bare `D9 FC`, and `DF E0`. Each load produces a
canonical-Normal binary80 value and sticky DE; FRNDINT rounds that value to
positive zero, adds sticky PE, preserves DE and exposes the resulting FSW
(`0x7d22`) in AX. The interpreter matrix combines source classes and Normal
exponent representatives with sign, all FCWs/TOPs, IE/DE/PE combinations and
prior C1. ZE/OE/UE/SF/ES/B remain rejected with any admitted sticky bits. The
all-masked FCW gate and JIT/native denial remain unchanged. Physical and Mac
mini acceptance are deferred to the combined final S2-31 substage under
ISSUE-019. See the [S2-31dj CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31dj-historical-sticky-de-in-bounded-frndint-d9-fc), [plan](../../../../docs/plans/STAGE_2.md#s2-31dj-admit-historical-sticky-de-to-bounded-frndint-d9-fc), and report.

### S2-31dk FRNDINT historical UE admission

The seeded `s2_31dk_frndint_seeded_ue_fnstsw.case.json` starts with exact
binary80 `1.0`, UE only, all-masked FCW `0x037f`, and literal `D9 FC DF E0`;
FRNDINT preserves UE without generating PE and FNSTSW observes it. The
`s2_31dk_fst_m32_underflow_frndint_fnstsw.case.json` fixture composes literal
`D9 10 D9 FC DF E0` over 1.5 times the minimum binary32 subnormal. FST m32
creates UE+PE, FRNDINT rounds the unchanged binary80 source to zero while
preserving both flags, and FNSTSW observes `0x0030`. The interpreter matrix
also covers UE+inexact PE and UE+SNaN IE over IE/DE/PE, C1, RC and TOP. Other
forbidden status bits remain atomic rejects; JIT/native stay closed. Physical
x86 and Mac mini acceptance remain part of the combined final S2-31 acceptance
substage under ISSUE-019. See the [S2-31dk CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31dk-historical-sticky-ue-in-bounded-frndint-d9-fc), [plan](../../../../docs/plans/STAGE_2.md#s2-31dk-admit-historical-sticky-ue-to-bounded-frndint-d9-fc), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31dk--historical-sticky-ue-through-bounded-frndint).

### S2-31dl FRNDINT historical OE admission

The seeded `s2_31dl_frndint_seeded_oe_fnstsw.case.json` starts with exact
binary80 `1.0`, OE only, all-masked FCW `0x037f`, and literal `D9 FC DF E0`;
FRNDINT preserves OE without generating PE. The
`s2_31dl_fst_m32_overflow_frndint_fnstsw.case.json` fixture composes literal
`D9 10 D9 FC DF E0` over canonical binary80 `(2 − 2^-63) × 2^127`, starting
with FSW=0. FST m32 rounds to binary32 infinity and sets OE+PE+C1; FRNDINT
preserves OE+PE and clears C1, then FNSTSW observes the final status. The
fixture reuses the exact sequence already admitted by the existing schema.
Native/JIT admission stays closed, and
physical x86/Mac mini acceptance remains part of the combined final S2-31
substage under ISSUE-019. See the [S2-31dl CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31dl-historical-sticky-oe-in-bounded-frndint-d9-fc), [plan](../../../../docs/plans/STAGE_2.md#s2-31dl-admit-historical-sticky-oe-to-bounded-frndint-d9-fc), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31dl--historical-sticky-oe-through-bounded-frndint).

### S2-31dm FRNDINT historical ZE admission

The literal `s2_31dm_frndint_seeded_ze_oe_fnstsw.case.json` begins with an
explicitly seeded canonical-Normal `1.0`, sticky ZE+OE (`FSW=0x000c`), and
all-masked `FCW=0x037f`. The guest bytes are exactly `D9 FC DF E0`; no guest
instruction produces ZE. FRNDINT preserves ZE+OE without creating PE, and
FNSTSW AX is expected to return exactly `0x000c`. The interpreter cross-matrix
combines ZE with every IE/DE/OE/UE/PE subset over source class, sign, RC, TOP,
and prior C1; SF/ES/B remain atomic rejects. JIT/native admissions remain
closed. Physical x86/Mac mini acceptance is deferred to the combined final
S2-31 substage under ISSUE-019. See the [S2-31dm CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31dm-historical-sticky-ze-in-bounded-frndint-d9-fc), [plan](../../../../docs/plans/STAGE_2.md#s2-31dm-admit-historical-sticky-ze-to-bounded-frndint-d9-fc), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31dm--historical-sticky-ze-through-bounded-frndint).

### S2-31dn FRNDINT historical SF+IE admission

The literal `s2_31dn_frndint_seeded_sf_ie_fnstsw.case.json` runs exactly
`D9 FC DF E0` with canonical binary80 `1.0`, FCW `0x037f`, and initial FSW
`0x0241` (SF+IE+C1). FRNDINT preserves SF+IE and recomputes C1 to zero;
FNSTSW AX observes `0x0041`. Provenance is `project_fixture`: IE and SF are
seeded in the initial snapshot, not produced by guest code. The fixture is not
native evidence. The independent interpreter matrix combines SF+IE with every
DE/ZE/OE/UE/PE subset across all supported source classes/signs/FCWs/TOPs/prior
C1; SF-only (including current SNaN), ES and B remain rejected. Re-entry and
FNCLEX are covered by the existing signaling-NaN sequence. Physical x86/Mac
mini acceptance remains deferred to the combined final S2-31 substage under
ISSUE-019. See the [S2-31dn CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31dn-historical-sfie-in-bounded-frndint-d9-fc), [plan](../../../../docs/plans/STAGE_2.md), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31dn--historical-sticky-sf-with-ie-through-bounded-frndint).

### S2-31di FRNDINT small Normal values

The literal `s2_31di_frndint_min_normal_to_negative_zero_fnstsw.case.json`
starts at canonical negative binary80 minimum Normal, runs `D9 FC` and `DF E0`,
and observes signed-zero output, the Zero tag, sticky IE/PE, cleared C1 and
preserved condition codes in AX/FSW. Interpreter vectors cover both signs,
nearest half and neighbors, near-one values, directed rounding, selected
negative exponents down to -16382, all FCWs/TOPs and sticky IE/PE/C1; a second
FRNDINT confirms PE sticks while C1 clears for the Zero identity. Native/JIT
denial remains active. See the [S2-31di CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31di-small-canonical-normal-frndint-d9-fc), [plan](../../../../docs/plans/STAGE_2.md), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31di--small-canonical-normal-values-through-bounded-frndint).

### S2-31b FISTP m32int schema and literal coverage

The schema admits only bare `DB 18` and exact `DB 18 DF E0` (FNSTSW AX).
Literal cases cover a successful unaligned Dword write split across adjacent
fixture regions, the exact FNSTSW composition, an Empty old physical TOP tag,
and a signed-i32 overflow. The in-range success cases preserve all raw x87
payloads, record the destination linear address in FDP, pop exactly one
physical slot, and make the stored bytes observable. The exponent-32 finite
overflow fixture is now a successful integer-indefinite/IE case under S2-31cj;
the invalid-tag no-stop case still preserves its full CPU and memory snapshots.
Injected store permission and architectural faults are exercised by
interpreter tests. Native admission explicitly rejects FISTP; flat fixture RAM
does not establish MMU, page-walk, MMIO, SMP, or physical x86 behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer”
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-31d FISTP m16int schema and literal coverage

The schema admits only bare `DF 18` and exact `DF 18 DF E0` (FNSTSW AX).
Handwritten fixtures cover positive and negative half-way inputs under all four
RC modes, exact signed-word limits, rounded values at both limits, and the
FNSTSW composition. The former signed-i16 overflow and exponent-16 fixtures
now execute as integer-indefinite with IE under S2-31ck; rejection fixtures cover invalid old
physical TOP tag, invalid FSW and unsupported FCW. Interpreter unit tests retain
binary80 subnormal rejection coverage.
The CLI verifies one Word mutation across the split fixture regions, unchanged
raw x87 payloads, pop/status/FIP/FDP state, and zero-retirement failures with
unchanged state and memory. Interpreter tests additionally exercise the exact
one-write/no-read contract, permission and injected #PF atomicity, and full
canonical48-range rejection. Native x87 admission explicitly denies FISTP.
Flat fixture RAM does not establish MMU translation, page walks, MMIO, SMP, or
physical x86 behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer,”
pp.3-347–3-349 ([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-31g FNSTCW m2byte schema and literal coverage

The schema admits exact bare `D9 38` and exact `D9 38 DF E0` (FNSTSW AX);
prefixed and neighboring D9 forms remain unsupported. Literal fixtures cover
the four standard FCWs, arbitrary schema-valid FCW `0xa55a` with `FSW=0xffff`,
Empty tags, payload and FPU pointer sentinels, an unaligned split-region Word
store with canaries, and canonical48 crossing rejection. The composition case
observes unchanged FSW through AX. Interpreter tests check that the store makes
one Word write and no data read, preserves FCW/FSW/tags/TOP/payloads/FIP/FDP/FOP,
GPRs and RFLAGS, and does not commit on second-byte write failure, fetch
truncation, STOP or budget exhaustion. Native admission explicitly denies
FNSTCW. The local profile preserves undefined C0-C3 deterministically but does
not claim native equality; it does not model general x87 exception delivery,
MMU translation, MMIO, SMP or physical x86 behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FSTCW/FNSTCW—Store x87 FPU
Control Word,” pp.3-388–3-389 (`D9 /7`); Vol.1 §§8.1.8, 8.1.9 and §8.6 for
x87 state and FIP/FDP context ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=506),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).

### S2-31h FNSTSW m2byte schema and literal coverage

The schema admits exact bare `DD 38` and `DD 38 DF E0` (FNSTSW AX), while
prefixes—including waiting `9B DD 38`—and neighboring DD forms remain
unsupported. Literal fixtures cover FSW `0000`, mixed `45a5`, `ffff`, arbitrary
FCW/status with Empty tags and payload/pointer sentinels, an unaligned
split-region Word store with canaries, and canonical48 crossing rejection.
The composition case observes the same FSW through AX after storing it. Local
interpreter checks cover exactly one Word write with no data read, full x87/GPR/
RFLAGS preservation, write permission/backend failure and fetch/stop/budget
boundaries. Native x87 admission explicitly denies `DD 38`. C0-C3 preservation
is local deterministic policy without native-equivalence evidence; the slice
does not establish `#MF`, `#NM`/CR0, MMU, MMIO, SMP, or physical x86 behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FSTSW/FNSTSW—Store x87 FPU
Status Word,” pp.3-392–3-393 (`DD /7`); Vol.1 §§8.1.8, 8.1.9 and 8.3.12
([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=510),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).

### S2-31i FLDCW m2byte schema and literal coverage

The schema admits exact bare `D9 28` and the composition
`D9 28 D9 FC DF E0`; prefixes, including waiting `9B D9 28`, and other D9
forms remain unsupported. The literal composition fixture loads `0x0b7f`
across the existing stack region, rounds 1.5 upward through FRNDINT, and
observes resulting FSW `0x4720` through FNSTSW AX. Interpreter vectors cover
all 16 old/new standard FCW pairs and each loaded rounding-control setting.
Invalid old FCW/FSW rejects before data access; invalid loaded FCW performs
one Word read and rejects without committing. Missing x87 state, second-byte
permission failure, canonical48 crossing, fetch/truncation/STOP/budget and
exhaustive D9 ModR/M/prefix decoding are covered. Native x87 admission denies
FLDCW. The bounded implementation does not deliver pending/unmasked x87
exceptions, `#MF`, or `#NM`/CR0 and makes no physical execution claim.

Normative source: Intel SDM revision 093, Vol.2A, “FLDCW—Load x87 FPU Control
Word,” pp.3-356–3-357 (`D9 /5`); Vol.1 §§8.1.8, 8.1.9 and §8.6 ([official
Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=474),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).

### S2-31j FSTP m32fp schema and literal CLI coverage

The schema admits exact bare `D9 18` and exact `D9 18 DF E0` (FNSTSW AX);
prefixes, WAIT and neighboring D9 bytes remain unsupported. The two handwritten
fixtures use an aligned, mapped Dword destination, a +1.5 binary80 source,
preserved payload sentinels and an observable post-pop status. The CLI test
checks the stored binary32 bits `0x3fc00000`, TOP/tag update, preserved payloads, FIP/FDP,
and FNSTSW AX composition. Interpreter vectors independently cover all TOPs,
even/odd ties, sign-sensitive directed rounding, minimum/maximum normal
exponents, carry from exponent 126, rejected exponents -127/127, C1/PE state,
canaries, one atomic write with no data read, canonical crossing, store fault,
and rejection before data access. Native x87 admission remains closed. These
fixtures and flat-memory checks do not establish hardware exception priority,
general x87 exception delivery, MMU, MMIO, SMP, or physical x86 behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`D9 /2`/`D9 /3`); see also the local bounded
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31j-bounded-fstp-m32fp-rax-d9-18).

### S2-31k FST m32fp schema and literal CLI coverage

The schema admits exact bare `D9 10` and exact `D9 10 DF E0` (FNSTSW AX);
prefixes, WAIT and neighboring D9 bytes remain unsupported. These two
handwritten fixtures reuse the +1.5 binary80 source and mapped Dword destination
from S2-31j. The CLI verifies stored `0x3fc00000`, unchanged TOP/tag/payload
state, retained C0/C2/C3, exact-result C1 clearing, FIP/FDP and status-word
composition through AX. Independent interpreter tests cover all TOPs with tag
and payload sentinels, RC/tie/sign behavior, normal exponent endpoints and
carry rejection, canaries, one-write/no-read, canonical range, backend store
failure and architectural #PF atomicity. Native x87 admission stays closed.
No hardware exception-priority, MMU, MMIO, SMP or physical x86 behavior is
claimed.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`D9 /2`/`D9 /3`); see also the local bounded
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31k-bounded-fst-m32fp-rax-d9-10).

### S2-31l FSTP m64fp schema and literal CLI coverage

The schema admits exact bare `DD 18` and exact `DD 18 DF E0` (FNSTSW AX);
other FSTP m64fp ModR/M/addressing forms, prefixes, REX and WAIT remain
unsupported for this operation; existing DD register/status forms retain
their current behavior. Two
handwritten schema-v2 fixtures use a canonical-Normal 1.5 source and a Qword
destination. The CLI verifies stored binary64 bits `0x3ff8000000000000`, the
old TOP tag becoming Empty, TOP advance, preserved payloads, FIP/FDP and the
FNSTSW-observed status. Independent interpreter vectors cover both signs and
all RC modes, ties and discarded-bit boundaries, exponent endpoints and
carry rejection, all TOP values including wrap, PE/C1, canaries, one atomic
Qword write with no read, canonical range checks, backend errors and
architectural `#PF` atomicity. Native x87 admission remains closed. These
local checks do not establish hardware exception priority, MMU, MMIO, SMP or
physical x86 behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`DD /2`/`DD /3`)
([official PDF, Vol.2A p.3-386](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504));
see the local bounded [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31l-bounded-fstp-m64fp-rax-dd-18).

### S2-31m FST m64fp schema and literal CLI coverage

The schema admits exact bare `DD 10` and exact `DD 10 DF E0` (FNSTSW AX);
other DD ModR/M/addressing forms, prefixes, REX and WAIT remain unsupported
for this operation, while existing DD register/status forms retain their
behavior. Two handwritten schema-v2 fixtures use a canonical-Normal 1.5
source and Qword destination. CLI checks verify binary64 bytes
`0x3ff8000000000000`, unchanged TOP/tag word/raw registers and the
FNSTSW-observed status. Interpreter checks use independent rounding literals
for both signs and four RC modes, exact/tie-adjacent/discarded-bit values,
exponent endpoints and carry rejection. They also cover every TOP including
TOP 7, no-pop source preservation after inexact conversion, canaries, one
Qword write/no read, separate noncanonical-start and canonical-crossing
ranges, backend and architectural store failures, invalid source/environment,
and fetch/truncation/STOP/budget boundaries. Native x87 admission explicitly
denies this operation. These local tests do not establish hardware exception
priority, MMU, MMIO, SMP or physical x86 behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`DD /2` FST, `DD /3` FSTP)
([official PDF, Vol.2A p.3-386](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504),
[official PDF, Vol.2A p.3-387](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=505));
see the local bounded [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31m-bounded-fst-m64fp-rax-dd-10).

### S2-31n FILD m32int schema and literal CLI coverage

The schema admits exact bare `DB 00` and exact `DB 00 DF E0` (FNSTSW AX),
while exhaustive decoder checks admit only `DB 00` for FILD and reject its other
ModR/M values, prefixes, REX and WAIT. Existing DB instruction forms remain
supported. Two handwritten fixtures load signed integer 1 into physical
register 7 from an unaligned Dword crossing adjacent regions; the composition
fixture observes the pushed status through AX. CLI/interpreter checks compare
the literal binary80 payload/tag, TOP, FIP/FDP, status and preserved CPU/x87
state. Unit vectors independently cover zero, ±1/2/3, `2^24±1`, i32 maximum
and minimum, every TOP including wrap, and all four RC values. Failure checks
cover nonempty destination before memory, missing x87 state, FCW/FSW gates,
canonical start/crossing, architectural read fault, backend error, fetch and
truncation, STOP and zero budget. Native x87 admission remains closed.

Normative source: Intel SDM revision 093, Vol.2A, “FILD—Load Integer,”
pp.3-342–3-343 ([official PDF, Vol.2A p.3-342](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=460),
[official PDF, Vol.2A p.3-343](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=461));
see the local bounded [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31n-bounded-fild-m32int-rax-db-00).

### S2-31o FILD m16int schema and literal CLI coverage

The schema admits exact bare `DF 00` and exact `DF 00 DF E0` (FNSTSW AX).
Exhaustive decoder checks add only `DF 00`, while retaining `DF 10`, `DF 18`,
`DF 38`, `DF E0`, `DF E9`, `DF F1`, and the existing DB forms; all prefixes
remain rejected. Two handwritten fixtures read signed Word value 1 from an
unaligned address crossing adjacent regions and observe the pushed status in
the composition case. Runner checks compare binary80 payload/tag, TOP, FIP/FDP,
status, and preserved state. Interpreter vectors cover zero, ±1/2/3,
32767/-32768, all TOP×RC combinations, sign extension, stale Empty payload,
unrelated tags/payloads, C1/PE policy, exact two-byte reads with canaries,
canonical range/read faults, and fetch/truncation/STOP/budget boundaries.
Native x87 admission remains closed.

Normative source: Intel SDM revision 093, Vol.2A, “FILD—Load Integer,”
pp.3-342–3-343 ([official PDF, Vol.2A p.3-342](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=460),
[official PDF, Vol.2A p.3-343](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=461));
see the local bounded [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31o-bounded-fild-m16int-rax-df-00).

### S2-31f FISTP m64int schema and literal coverage

The schema admits exact `DF 38` and `DF 38 DF E0` (FNSTSW AX), with prefixes
and neighboring encodings rejected. Literal CLI fixtures cover an unaligned
Qword crossing adjacent regions, exact signed-i64 maximum and minimum, TOP
pop, the FNSTSW-observed status, and positive 2^63 rejection before any data
write. Interpreter vectors separately cover positive/negative ties under all
four RC modes, negative 2^63 under every RC, `2^63 - 0.5` under all four RC
modes, negative `2^63+1`, exponent 64, canaries, canonical48 rejection,
permission failure and injected #PF atomicity. Native x87 admission explicitly
denies `DF 38`. Overflow uses this bounded profile's Unsupported result rather
than architectural indefinite integer encoding; x87 exception delivery,
MMU/page walks, MMIO, SMP and physical execution remain unmodeled.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer,”
pp.3-347–3-349 ([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-31e FIST m16int schema and literal coverage

The schema admits only bare `DF 10` and exact `DF 10 DF E0` (FNSTSW AX).
Handwritten cases cover a positive tie, FNSTSW composition, exact signed-word
limits; the former signed-i16 overflow and exponent-16 fixtures are now
successful under S2-31ck. Invalid old TOP tag, invalid FSW and unsupported FCW remain rejected;
binary80 subnormal rejection is covered by interpreter tests. The CLI verifies
one Word mutation and complete
no-pop state preservation, including TOP, tags and raw payloads; rejection
cases preserve CPU and memory with zero retirement. Interpreter unit tests also
cover every rounding control and sign, rounding at the signed limits, complete
canonical48 range validation, the one-write/no-read contract, permission
failure and injected architectural-fault atomicity. Native x87 admission
explicitly denies FIST. Flat fixture RAM does not establish MMU translation,
page walks, MMIO, SMP or physical x86 behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer,”
pp.3-347–3-349 ([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-31c FIST m32int schema and literal coverage

The schema admits only bare `DB 10` and exact `DB 10 DF E0` (FNSTSW AX).
Literal cases execute the same unaligned split-region Dword write as S2-31b
and then optionally observe FSW through AX. They verify that FIST retains the
old TOP, tag word and raw payloads while recording FIP/FDP and setting sticky
PE/C1. The invalid-tag fixture remains rejected; the former exponent-32
overflow fixture now commits integer-indefinite with IE under S2-31cj.
Interpreter checks cover all four rounding controls, signed values, exact
round-before-range overflow, roundback into range, the one-write/no-read
contract, and store fault atomicity. Native x87 admission explicitly denies
FIST. Flat fixture RAM
does not establish MMU, page-walk, MMIO, SMP, or physical x86 behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer”
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating-Point
Value,” pp.3-386–3-387 ([official PDF, Vol.2A p.3-386](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504)).

### S2-30ae FCOMP ST(i) schema and literal CLI coverage

The schema admits every bare `D8 D8+i` form and each exact four-byte
`D8 D8+i DF E0` status-observation sequence with instruction budgets one and
two. Eight handwritten equality fixtures execute each FCOMP index through the
CLI and verify that the old TOP tag becomes Empty, TOP advances once, the
condition field reports equality, and all physical register bytes remain
unchanged. The existing invalid-ST(1)-tag case rejects atomically with
`stop_rip: null` and budget one. Interpreter vectors independently cover every
TOP×i pair and lhs/rhs tag/class failures. Native admission explicitly denies
all eight encodings.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326.

### S2-30af FCOMPP schema and literal CLI coverage

The schema admits exactly bare `DE D9` and the exact `DE D9 DF E0` FNSTSW AX
observation sequence. The literal success fixture checks the compare status and
two-entry physical TOP pop through the CLI. A separate `DE D9` fixture has an
invalid actual old ST(1) tag, no stop RIP and budget one, proving atomic
rejection. Schema validation ties that rejection rule to the physical tag
selected by the input TOP. Interpreter vectors cover every TOP, relation and
initial condition field; native admission denies `DE D9`.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326.

### S2-30ag FUCOM schema and literal CLI coverage

[s2_30ag_fucom_equal_fnstsw.case.json](s2_30ag_fucom_equal_fnstsw.case.json)
runs bare `DD E1` followed by `DF E0` with equal canonical-Normal operands;
FUCOM replaces the condition codes with equality and FNSTSW copies that value
to AX. TOP, tags and raw register payloads remain unchanged. The paired
[s2_30ag_fucom_invalid_rhs_tag.case.json](s2_30ag_fucom_invalid_rhs_tag.case.json)
has a Valid lhs and Empty old physical ST(1), no stop RIP, and budget one; it
must return Unsupported without retiring. Schema and interpreter-runner tests
load both literal files. The unique QNaN comparison behavior of FUCOM is
outside this Normal-only slice.

Normative source: Intel SDM revision 093, Vol.2A,
“FUCOM/FUCOMP/FUCOMPP—Unordered Compare Floating-Point Values,” pp.3-402–3-404
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-30aj FCOMI schema and literal CLI coverage

[`s2_30aj_fcomi_equal_fnstsw.case.json`](s2_30aj_fcomi_equal_fnstsw.case.json)
uses exact `dbf1dfe0`: equal canonical-Normal operands set ZF, clear the other
five affected RFLAGS bits, clear only FSW.C1, and let FNSTSW copy `0x4500` to
AX. The fixture compares the six defined FCOMI flag bits. The paired
[`s2_30aj_fcomi_invalid_rhs_tag.case.json`](s2_30aj_fcomi_invalid_rhs_tag.case.json)
uses exact `dbf1`, no stop RIP and budget one; the actual old physical ST(1)
tag is Empty, so the instruction must reject atomically without retirement.
Schema loading and interpreter CLI coverage assert both exact byte strings.
Native admission explicitly denies `Operation::Fcomi` and DB F1.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329.

### S2-30ak FCOMIP schema and literal CLI coverage

[`s2_30ak_fcomip_equal_fnstsw.case.json`](s2_30ak_fcomip_equal_fnstsw.case.json)
uses exact `dff1dfe0`: equal canonical-Normal operands set ZF, clear the other
five affected RFLAGS bits, clear only FSW.C1, empty physical old TOP, advance
TOP once, and let FNSTSW copy `0x4d00` to AX. Physical old ST(1) remains Valid
and its ten-byte payload is preserved. The paired
[`s2_30ak_fcomip_invalid_rhs_tag.case.json`](s2_30ak_fcomip_invalid_rhs_tag.case.json)
uses exact `dff1`, no stop RIP and budget one; the actual old physical ST(1)
tag is Empty, so FCOMIP rejects atomically without retirement. Schema loading
and interpreter CLI coverage assert the exact byte strings and resulting
state. Native admission explicitly denies `Operation::Fcomip` and DF F1.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-30al FUCOMI schema and literal CLI coverage

[`s2_30al_fucomi_equal_fnstsw.case.json`](s2_30al_fucomi_equal_fnstsw.case.json)
uses exact `dbe9dfe0`: ordered equal canonical-Normal operands set ZF, clear
the other five affected RFLAGS bits and only FSW.C1, then FNSTSW copies
`0x4500` to AX. FUCOMI leaves TOP, tags and payloads unchanged. The paired
[`s2_30al_fucomi_invalid_rhs_tag.case.json`](s2_30al_fucomi_invalid_rhs_tag.case.json)
uses exact `dbe9`, no stop RIP and budget one; the actual old physical ST(1)
tag is Empty, so execution rejects without retirement or state mutation.
Schema/CLI coverage rejects `dbf8dfe0`. Native admission explicitly denies
`Operation::Fucomi` and DB E9.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329.

### S2-30ah FUCOMP schema and literal CLI coverage

[s2_30ah_fucomp_equal_fnstsw.case.json](s2_30ah_fucomp_equal_fnstsw.case.json)
runs bare `DD E9` followed by `DF E0` with equal canonical-Normal operands.
FUCOMP reports equality, empties only old physical TOP, advances TOP once, and
FNSTSW copies the resulting status to AX. Raw physical register payloads and
the old ST(1) Valid tag remain unchanged.
[s2_30ah_fucomp_invalid_rhs_tag.case.json](s2_30ah_fucomp_invalid_rhs_tag.case.json)
has a Valid lhs and Empty old physical ST(1), no stop RIP, and budget one; the
instruction must reject without retiring or changing state. Schema and
interpreter CLI tests load both literal files. FUCOMP's QNaN behavior remains
outside the canonical-Normal-only slice.

Normative source: Intel SDM revision 093, Vol.2A,
“FUCOM/FUCOMP/FUCOMPP—Unordered Compare Floating-Point Values,” pp.3-402–3-404
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-30ai FUCOMPP schema and literal CLI coverage

[s2_30ai_fucompp_equal_fnstsw.case.json](s2_30ai_fucompp_equal_fnstsw.case.json)
runs bare `DA E9` then `DF E0` with equal canonical-Normal operands. It observes
FSW/AX `0x5000`, both old physical tags Empty, the TOP advance by two and two
retired instructions; physical payloads remain unchanged.
[s2_30ai_fucompp_invalid_rhs_tag.case.json](s2_30ai_fucompp_invalid_rhs_tag.case.json)
uses a Valid lhs and invalid actual old physical ST(1) tag with no stop RIP and
budget one, so execution must reject without retiring or changing state. Schema
tests admit only exact `dae9` and `dae9dfe0` for this FUCOMPP slice, and decoder
tests enumerate all DA ModR/M values and prefix forms. Native admission denies
DA E9. C1=0 is an explicit local policy. Intel SDM revision 093 Vol.2A
pp.3-402–3-404's opcode table/prose says two entries are popped while p.3-403
pseudocode depicts one; the local contract follows the explicit table/prose.

Normative source: Intel SDM revision 093, Vol.2A,
“FUCOM/FUCOMP/FUCOMPP—Unordered Compare Floating-Point Values,” pp.3-402–3-404
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-30am FUCOMIP schema and literal CLI coverage

[s2_30am_fucomip_equal_fnstsw.case.json](s2_30am_fucomip_equal_fnstsw.case.json)
uses exact `dfe9dfe0`: equal canonical-Normal operands set ZF, clear the other
five affected flags and C1, empty old physical TOP, advance TOP once, and let
FNSTSW copy `0x4d00` to AX. All raw payloads and the old physical ST(1) Valid
tag remain preserved. The paired
[s2_30am_fucomip_invalid_rhs_tag.case.json](s2_30am_fucomip_invalid_rhs_tag.case.json)
uses exact `dfe9`, TOP=5, and Empty at actual physical `(TOP+1)&7`; with no stop
RIP and budget one, it rejects without retirement or mutation. Schema/CLI
reject `dff8dfe0` and 66/48/9B-prefixed variants. Native admission explicitly
denies `Operation::Fucomip` and DF E9.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-30an indexed FCOMI-family schema and literal CLI coverage

The schema accepts the exact 32 bare forms `DB E8..EF` FUCOMI, `DB F0..F7`
FCOMI, `DF E8..EF` FUCOMIP and `DF F0..F7` FCOMIP, either alone or followed
by `DF E0` FNSTSW AX. Exhaustive schema tests cover all 64 strings and reject
neighboring bytes, prefixes, memory forms and extra trailing bytes.

Four literal success fixtures cover `DB F0` self-compare at TOP=3 with only
old TOP Valid; `DF E8` self-compare/pop at TOP=7 with wrap and only old TOP
Valid; `DB EF` source ST(7) at TOP=7 using physical slot 6 while old ST(1) is
Empty; and `DF F6` source ST(6) at TOP=6 using physical slot 4, then popping
only old TOP. Standalone budget-one rejects cover `i=0` with a Valid old TOP
but all-zero payload and `i=7` with Empty actual physical `(TOP+7)&7` while the
unrelated old ST(1) remains Valid. Every reject has no stop RIP and preserves
full state. The existing `i=1` fixtures remain unchanged. Native admission
explicitly denies all 32 indexed instruction encodings and all four operation
families.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-31p FILD m64int schema and literal CLI coverage

The schema and CLI execute only the exact bare bytes `DF 28` and exact
composition `DF 28 DF E0` (FNSTSW AX). Each handwritten fixture places signed
Qword value 1 at an unaligned address that crosses two adjacent RAM regions;
the composition case observes the pushed status through AX. The CLI checks
the binary80 payload/tag, TOP, FIP/FDP, status and preserved state. Decoder
coverage exhausts every DF ModR/M byte and all tested legacy and REX prefixes,
while retaining DF 00/10/18/38/E0/E9/F1 and all DB forms.

Interpreter literals cover zero, ±1/2/3, i64 extrema, values around `2^53`
and signed values around `2^62` across every TOP and RC. The shared i64 packer
is also exercised by the existing m16/m32 FILD suites. An unaligned Qword
read checks canaries and one exact eight-byte access. Invalid x87 gates,
noncanonical/crossing/overflow ranges, architectural read #PF, backend errors,
fetch/truncation/STOP/zero-budget boundaries and full state atomicity are
covered. Native x87 admission explicitly denies the instruction. This remains
a local partial slice; general exception delivery, MMU/MMIO/SMP and physical
x86 behavior are not established.

Normative source: Intel SDM revision 093, Vol.2A, “FILD—Load Integer,”
pp.3-342–3-343 ([official PDF, Vol.2A p.3-342](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=460),
[official PDF, Vol.2A p.3-343](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=461));
see the local bounded [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31p-bounded-fild-m64int-rax-df-28).

### S2-31q FSTP m80fp schema and literal CLI coverage

The schema admits exact bare `DB 38` and exact `DB 38 DF E0` (FNSTSW AX).
Fixtures contain independent literal code bytes, raw binary80 ST(0) state and
the ten-byte destination; CLI checks verify all ten bytes, TOP/tag retirement,
preserved payloads and FNSTSW-visible C1 clearing. Interpreter tests add all TOP
values, minimum/maximum Normal raw exponents, accepted FCW variants, sticky PE,
condition retention, unaligned canaries and a nonzero byte 11, one write/no
reads, full canonical48 range/overflow, denied and injected backend/#PF stores,
and source/environment/fetch/execution-boundary atomicity. Native admission
explicitly denies the decoded operation. This local profile does not establish
physical x86 behavior or general x87 exception delivery.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`DB /7`), official PDF p.504
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP.


### S2-31r FLD m80fp schema and literal CLI coverage

Fixtures contain exact literal `DB 28` and `DB 28 DF E0` bytes, a raw
canonical-Normal ten-byte read-only memory region, full x87 state, and a
non-readable adjacent byte-11 canary; independent code lengths/stop addresses
keep both boundaries explicit. The status-composition case observes the pushed TOP and
C1 policy through FNSTSW AX. A third fixture supplies binary80 zero, expects
Unsupported, and verifies the CPU snapshot remains uncommitted after the single
source read. CLI results check the expected exit and one/two retired
instructions. Interpreter tests cover the full TOP × FCW matrix, literal raw
sign/fraction/exponent vectors, unaligned exact-width access/no writes, invalid
classes, canonical ranges, #PF/backend errors, and execution boundaries.
DB ModR/M/prefix tests preserve all existing forms and reject DB 30. Native
x87 admission remains closed.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF p.470
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).

### S2-31t FLD m64fp schema and literal CLI coverage

The original S2-31t fixtures contained exact literal `DD 00` and
`DD 00 DF E0` instruction bytes, an unaligned eight-byte binary64 source,
complete x87 state and an adjacent read-only canary. The composition fixture
observes pushed TOP and C1 clearing through FNSTSW AX. At that stage the third
fixture used zero as an unsupported source. The historical S2-31t report
records that Normal-only behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF p.470
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).

### S2-31u FLD m64fp signed-zero fixture coverage (historical)

The exact `DD 00` and `DD 00 DF E0` fixtures now use a literal positive-zero
binary64 source and expect a successful push with the Zero tag; the composition
observes the updated TOP and C1 through FNSTSW AX. The invalid-class fixture
supplied positive minimum subnormal in the S2-31u slice; S2-31z reclassifies it
to positive infinity while preserving the S2-31u report as historical
evidence. Interpreter literals additionally cover negative zero and,
currently, negative minimum subnormal. Native admission continues to deny all
three fixtures. See the [S2-31u CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31u-bounded-fld-m64fp-rax-signed-zero-extension), [plan](../../../../docs/plans/STAGE_2.md#s2-31u-extend-bounded-fld-m64fp-rax-dd-00-to-signed-zero), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31u--bounded-fld-m64fp-signed-zero-extension).

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF pp.470–471
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).

### S2-31w FLD m80fp signed-zero fixture coverage

The current `DB 28` success fixture supplies literal binary80 +0 and asserts
the pushed Zero tag and ten zero bytes. The `DB 28 DF E0` composition fixture
supplies -0 and checks the preserved sign byte (`0x80`) and Zero tag while
FNSTSW observes TOP/C1. The invalid-class fixture now supplies positive
minimum nonzero binary80 subnormal; schema admission is limited to nonzero
binary80 subnormal values for exact DB 28 with budget one and no stop RIP.
Both signs of minimum subnormal remain Unsupported after the one ten-byte read
with no CPU commit. The S2-31r report and fixture history above document its
original Normal-only state.

Intel SDM revision 093, Vol.2A FLD m80fp (pp.3-352–3-353, official PDF p.470)
does not define #D for this source format. Binary32/binary64 denormal policy is
not carried over; broader denormal/unmasked handling requires a dedicated
semantics slice. See the [S2-31w CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31w-bounded-fld-m80fp-rax-db-28-signed-zero-extension),
[plan](../../../../docs/plans/STAGE_2.md#s2-31w-extend-bounded-fld-m80fp-rax-db-28-to-signed-zero),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31w--bounded-fld-m80fp-signed-zero-extension).

### S2-31x FLD m80fp canonical-subnormal coverage

The S2-31x exact `DB 28` fixture supplies positive minimum binary80 subnormal;
the `DB 28 DF E0` composition fixture supplies negative largest subnormal and
checks that FNSTSW observes the pushed TOP/C1 result. Both assert preserved
raw ten-byte payloads and the Special tag. Interpreter literals cover both
signs of minimum, largest and interior subnormals for every TOP and accepted
FCW, with one exact ten-byte read and preserved unrelated state. The invalid
class fixture used positive pseudo-denormal at the S2-31x stage; S2-31aa
replaces it with positive canonical QNaN for the narrow exact-DB28,
budget-one, no-stop schema exception. Negative pseudo-denormal and other
special encodings remain rejected. Intel SDM
revision 093, Vol.2A FLD m80fp (pp.3-352–3-353, official PDF p.470) defines no
`#D` for this source format. See the [S2-31x CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31x-bounded-fld-m80fp-rax-db-28-canonical-subnormal-extension),
[plan](../../../../docs/plans/STAGE_2.md#s2-31x-extend-bounded-fld-m80fp-rax-db-28-to-canonical-subnormals),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31x--bounded-fld-m80fp-canonical-subnormal-extension).

### S2-31aa FLD m80fp canonical-infinity coverage

The positive bare `DB 28` fixture supplies canonical +infinity and expects the
unchanged ten-byte payload with the Special tag. The negative infinity fixture
executes `DB 28 DF E0`, preserving the sign and observing TOP/C1 through
FNSTSW. Interpreter literals cover both signs across every TOP and supported
FCW. Canonical NaNs, pseudo-infinities of both signs, pseudo-denormals,
pseudo-NaNs and unnormals reject after one ten-byte read without CPU commit;
memory preflight, read faults and execution-boundary atomicity remain covered.
At the S2-31aa stage the narrow Unsupported fixture supplied positive
canonical QNaN; S2-31ab replaces it with positive canonical SNaN. JIT/native admission
continues to deny the operation. Intel SDM revision 093 Vol.1 Table 4-3 gives
the canonical extended infinity encoding, while Table 8-3 distinguishes
pseudo-infinity and unsupported encodings; Vol.2A FLD m80fp is the instruction
reference. See the [S2-31aa CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31aa-bounded-fld-m80fp-rax-db-28-canonical-infinities),
[plan](../../../../docs/plans/STAGE_2.md#s2-31aa-extend-bounded-fld-m80fp-rax-db-28-to-canonical-infinities),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31aa--bounded-fld-m80fp-canonical-infinity-extension).

### S2-31ab FLD m80fp canonical-QuietNaN coverage

The positive bare `DB 28` fixture supplies a canonical QNaN with an independent
payload; the negative-indefinite fixture executes `DB 28 DF E0`, preserves its
raw bytes and observes the resulting TOP through FNSTSW. Interpreter literals
cover both signs/payloads across every TOP and supported FCW, retaining sticky
PE without setting IE. The exact `DB 28`, budget-one, no-stop Unsupported
fixture supplied canonical positive SNaN at that stage; S2-31ac replaces it
with positive pseudo-infinity. Schema/CLI neighbors reject other classes from
that narrow exception. The JIT/native profile continues to deny `DB 28`. See
the [S2-31ab CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ab-bounded-fld-m80fp-rax-db-28-canonical-quietnans),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ab-extend-bounded-fld-m80fp-rax-db-28-to-canonical-qnans),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ab--bounded-fld-m80fp-canonical-quietnan-extension).

### S2-31ac FLD m80fp canonical-SNaN coverage

The positive bare `DB 28` fixture supplies a minimum-payload canonical SNaN;
the negative maximum-payload fixture executes `DB 28 DF E0`, preserves all ten
raw bytes including the clear quiet bit and observes the pushed TOP through
FNSTSW. Interpreter literals cover minimum, middle and maximum payloads of
both signs across every TOP and supported FCW, preserving PE and C0/C2/C3 with
IE clear. The exact `DB 28`, budget-one, no-stop Unsupported fixture now uses
positive pseudo-infinity. Canonical SNaNs, pseudo-SNaNs and other neighboring
classes do not match that exception. Native and JIT profiles continue to deny
`DB 28`. See the [S2-31ac CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ac-bounded-fld-m80fp-rax-db-28-canonical-signalingnans),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ac-extend-bounded-fld-m80fp-rax-db-28-to-canonical-snans),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ac--bounded-fld-m80fp-canonical-signalingnan-extension).

### S2-31s FLD m32fp schema and literal CLI coverage

Fixtures contain exact literal `D9 00` and `D9 00 DF E0` instruction bytes,
an unaligned four-byte binary32 source, full x87 state and an adjacent
read-only canary. The composition case observes the pushed TOP and C1 clearing
through FNSTSW AX. At the original S2-31s slice, the third fixture supplied
zero and expected Unsupported; S2-31v reclassifies it to positive minimum
subnormal. The native admission
tests explicitly deny all three fixtures. Interpreter checks cover the full
TOP × FCW matrix using independent binary80 literals for signed one, adjacent
fractions, minimum/maximum Normal values and exponent 127; they also cover
special-class rejection, access width, canonical range failures, read
#PF/backend errors, invalid push gates and execution boundaries. Exhaustive D9
decode checks retain the existing forms and reject all prefix variants.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF p.470
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).

### S2-31v FLD m32fp signed-zero fixture coverage

The exact `D9 00` and `D9 00 DF E0` fixtures load positive and negative
binary32 zero respectively. CLI assertions check the pushed Zero tag and raw
signed binary80 payload; the composition observes TOP and C1 through FNSTSW
AX. In the original S2-31v result, the invalid-class fixture supplied positive
minimum subnormal and expected Unsupported. S2-31y supersedes that current
fixture/profile while keeping the S2-31v report historical. Native admission
continues to deny all fixtures. See the [S2-31v CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31v-bounded-fld-m32fp-rax-d9-00-signed-zero-extension), [plan](../../../../docs/plans/STAGE_2.md#s2-31v-extend-bounded-fld-m32fp-rax-d9-00-to-signed-zero), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31v--bounded-fld-m32fp-signed-zero-extension).

### S2-31y FLD m32fp masked-subnormal fixture coverage

The new exact `D9 00` fixture supplies positive minimum binary32 subnormal;
the `D9 00 DF E0` composition fixture supplies negative minimum subnormal and
observes sticky DE through FNSTSW AX. CLI checks verify canonical Normal
binary80 payloads, Valid tags, and the sticky DE status bit. Interpreter
literal vectors cover positive/negative minimum, interior and largest
subnormals for every TOP and accepted FCW, plus sticky PE/C1 and repeated-load
behavior. At the S2-31y stage, the invalid fixture supplied positive infinity;
S2-31af now supplies canonical positive QNaN because signed infinities are
supported. The current schema exception admits only QNaN for exact `D9 00`,
budget one and no stop RIP. Native admission remains closed. See the [S2-31y CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31y-bounded-fld-m32fp-rax-d9-00-masked-subnormal-extension), [plan](../../../../docs/plans/STAGE_2.md#s2-31y-extend-bounded-fld-m32fp-rax-d9-00-to-masked-subnormals), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31y--bounded-fld-m32fp-masked-subnormal-extension).

### S2-31z FLD m64fp masked-subnormal fixture coverage

The positive exact `DD 00` fixture supplies the minimum binary64 subnormal;
the negative `DD 00 DF E0` fixture supplies its negative and observes sticky
DE through FNSTSW AX. CLI checks verify canonical Normal binary80 payloads,
Valid tags and DE. Interpreter literals cover signed minimum, interior and
largest subnormals for every TOP and accepted FCW, plus sticky PE/C1 and
repeated-load behavior. At the S2-31z stage, the invalid-class fixture supplied positive infinity;
S2-31ae changed it to canonical positive QNaN because infinities became
supported. S2-31ag supersedes that predicate by accepting QNaNs; the current
schema exception requires canonical binary64 SNaN with exact `DD 00`, budget
one and no stop RIP. Native admission remains closed. See the [S2-31z CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31z-bounded-fld-m64fp-rax-dd-00-masked-subnormal-extension), [plan](../../../../docs/plans/STAGE_2.md#s2-31z-extend-bounded-fld-m64fp-rax-dd-00-to-masked-subnormals), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31z--bounded-fld-m64fp-masked-subnormal-extension).

### S2-31ad FLD m80fp pseudo-denormal fixture coverage

The positive bare `DB 28` fixture supplies a pseudo-denormal with zero
fraction; the negative `DB 28 DF E0` fixture supplies maximum fraction and
observes TOP/status through FNSTSW. Both preserve all ten bytes, including the
zero exponent, and push Special without new status flags. Interpreter literal
coverage includes both signs, zero/minimum/maximum fraction, all TOP×FCW pairs,
and equivalent exponent-one Normal controls. The unsupported schema fixture
remains positive pseudo-infinity; pseudo-denormals do not match its exact
DB28/budget-one/no-stop predicate. Arithmetic consumption, unmasked delivery,
native/JIT admission and physical behavior remain outside the slice. See the
[S2-31ad CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ad-bounded-fld-m80fp-rax-db-28-pseudo-denormals),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ad-extend-bounded-fld-m80fp-rax-db-28-to-pseudo-denormals),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ad--bounded-fld-m80fp-pseudo-denormal-extension).

### S2-31ae FLD m64fp signed-infinity fixture coverage

The positive bare `DD 00` fixture supplies +infinity; the negative
`DD 00 DF E0` fixture supplies -infinity and observes the pushed state through
FNSTSW. Both produce the exact canonical binary80 infinity and Special tag,
with no new flags. Interpreter literals cover both signs across every TOP and
accepted FCW; a minimum-subnormal load followed by infinity verifies sticky DE
and PE. Signed QNaN/SNaN neighbors remained Unsupported at this stage after
one Qword read. Its narrow schema exception was exact DD00, budget one, no
stop RIP and QNaN only; S2-31ag supersedes it. Native/JIT admission remains
closed. See the [S2-31ae CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ae-bounded-fld-m64fp-rax-dd-00-signed-infinities), [plan](../../../../docs/plans/STAGE_2.md#s2-31ae-extend-bounded-fld-m64fp-rax-dd-00-to-infinities), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ae--bounded-fld-m64fp-signed-infinity-extension).


### S2-31af FLD m32fp signed-infinity fixture coverage

The positive bare `D9 00` fixture supplies +infinity; the negative
`D9 00 DF E0` fixture supplies -infinity and observes the pushed state through
FNSTSW. Both produce canonical binary80 infinity and Special tag without new
flags. Interpreter literals cover both signs across all TOP×FCW pairs; a
minimum-subnormal load followed by infinity verifies sticky DE and PE. Signed
QNaN/SNaN neighbors remain Unsupported after one Dword read. The narrow
schema exception is exact D900, budget one, no stop RIP and canonical QNaN
only. Native/JIT admission remains closed. See the [S2-31af CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31af-bounded-fld-m32fp-rax-d9-00-signed-infinities), [plan](../../../../docs/plans/STAGE_2.md#s2-31af-extend-bounded-fld-m32fp-rax-d9-00-to-infinities), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31af--bounded-fld-m32fp-signed-infinity-extension).


### S2-31ag FLD m64fp canonical QNaN fixture coverage

The positive bare `DD 00` fixture supplies a quiet-only positive QNaN; the
negative `DD 00 DF E0` fixture supplies a payload-bearing QNaN and observes
the pushed state through FNSTSW. Both preserve sign and payload in canonical
binary80 with Special tag and add no status flags. Interpreter literals cover
quiet-only, low, middle and maximum payloads of both signs across TOP×FCW; a
minimum-subnormal load followed by QNaN verifies sticky DE/PE. The current
schema exception is exact DD00, budget one, no stop RIP and canonical SNaN
only. SNaNs remain post-read Unsupported after one Qword read. JIT/native
admission remains closed. See the [S2-31ag CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ag-bounded-fld-m64fp-rax-dd-00-canonical-qnans), [plan](../../../../docs/plans/STAGE_2.md#s2-31ag-extend-bounded-fld-m64fp-rax-dd-00-to-canonical-qnans), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ag--bounded-fld-m64fp-canonical-qnan-extension).

### S2-31ah FLD m32fp canonical QNaN fixture coverage

The positive bare `D9 00` fixture supplies a quiet-only QNaN; the negative
`D9 00 DF E0` fixture supplies a payload-bearing QNaN and observes the pushed
state through FNSTSW. Both preserve sign/payload in canonical binary80 with
Special tag and add no exception flags. Interpreter literals cover
quiet-only, low, middle and maximum payloads of both signs across TOP×FCW; a
minimum-subnormal load followed by QNaN verifies sticky DE/PE. The current
At S2-31ah, the schema exception was keyed to canonical SNaN and SNaNs
remained post-read Unsupported; S2-31aj accepts them and now uses an invalid-
environment fixture. JIT/native admission remains closed. See the [S2-31ah CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ah-bounded-fld-m32fp-rax-d9-00-canonical-qnans), [plan](../../../../docs/plans/STAGE_2.md#s2-31ah-extend-bounded-fld-m32fp-rax-d9-00-to-canonical-qnans), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ah--bounded-fld-m32fp-canonical-qnan-extension).

### S2-31ai FLD m64fp canonical SNaN fixture coverage

The positive bare `DD 00` fixture loads a low-payload SNaN; the negative
`DD 00 DF E0` fixture loads a maximum-payload negative SNaN and observes IE
through FNSTSW. Both quiet the source by integer bit mapping, preserve sign and
payload, produce binary80 Special, and set sticky IE only. Interpreter vectors
cover signed low/middle/maximum payloads across TOP×FCW, sticky DE/PE, exact
FSW, one-read fault atomicity, pre-read IM=0/pre-existing-IE rejection, and
FNCLEX recovery. Pre-existing IE blocks another DD00 until FNCLEX. The exact
schema exception is DD00, budget one, no stop RIP, and an invalid FCW or
`FSW & 0x80dd != 0`, independent of operand value. Native/JIT admission
remains closed. See the [S2-31ai CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ai-bounded-fld-m64fp-rax-dd-00-canonical-snans), [plan](../../../../docs/plans/STAGE_2.md#s2-31ai-extend-bounded-fld-m64fp-rax-dd-00-to-canonical-snans), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ai--bounded-fld-m64fp-canonical-snan-extension).

### S2-31aj FLD m32fp canonical SNaN fixture coverage

The positive bare `D9 00` fixture loads a minimum-payload canonical binary32
SNaN. The negative payload-bearing `D9 00 DF E0` fixture observes IE through
FNSTSW. Integer conversion sets binary80's quiet bit, preserves sign/payload,
and assigns Special. Interpreter literal vectors cover signed low, middle and
maximum payloads across TOP×FCW, sticky DE/PE, exact FSW, and FNCLEX recovery.
The latter clears exception flags while preserving TOP, tags and loaded bytes,
then permits another D900 load. Memory errors, unmasked/pre-existing-IE
pre-read gates and subsequent-load rejection are covered. The exact
D900/budget-one/no-stop Unsupported schema fixture is environment-driven
(invalid FCW or `FSW & 0x80dd != 0`) and uses an ordinary Normal source to show
operand class is irrelevant. Native/JIT admission remains closed. See the
[S2-31aj CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31aj-bounded-fld-m32fp-rax-d9-00-canonical-snans), [plan](../../../../docs/plans/STAGE_2.md#s2-31aj-extend-bounded-fld-m32fp-rax-d9-00-to-canonical-snans), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31aj--bounded-fld-m32fp-canonical-snan-extension).

### S2-31ak bounded m32/m64 FLD sticky-status fixtures

The prior D900 environment fixture and DD00 rejected-class fixture now carry
forbidden ZE status and use exact `D900`/`DD00`, budget-one, no-stop predicates.
The schema no longer treats IE, DE or PE as an automatic rejection for these
loads. The existing bare and DFE0/FNSTSW signaling-NaN fixtures verify the
sticky IE result; interpreter tests cover repeated and cross-width sequences,
all sticky IE/DE/PE combinations, and preserve the separate FILD/m80 gates.
Native/JIT admission remains denied. See [S2-31ak CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ak-sticky-exception-flags-for-bounded-m32m64-fld), [plan](../../../../docs/plans/STAGE_2.md#s2-31ak-preserve-sticky-ie-de-pe-across-exact-m32m64-fld-loads), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ak--sticky-ie-de-pe-across-bounded-m32m64-fld).

### S2-31al signed-zero m32 store fixtures

Four literal fixtures cover positive and negative binary80 Zero through exact
`D9 10` and `D9 18`; the negative cases compose with `DF E0` so FNSTSW exposes
the final status. The source physical TOP tag is Zero, the ten-byte payload is
canonical zero with the requested sign, and the destination bytes are literal
`00000000` or `00000080`. Runner tests verify FST stack preservation and FSTP
pop/tag updates after the write. Interpreter coverage spans all TOP×FCW values,
incoming PE/C1 combinations, FLD-zero compositions, bad tag/payload pairs,
store faults and canonical range rejection. JIT/native admission remains
closed. See [S2-31al CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31al-signed-zero-fst-m32fp-and-fstp-m32fp), [plan](../../../../docs/plans/STAGE_2.md#s2-31al-store-signed-binary80-zero-through-bounded-m32-fstfstp), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31al--signed-zero-stores-for-bounded-m32-fstfstp).

### S2-31am signed-zero m64 store fixtures

Four literal fixtures cover positive and negative binary80 Zero through exact
`DD 10` and `DD 18`; negative cases compose with `DF E0` so FNSTSW exposes
the final status. The raw ten-byte source has the Zero tag and canonical zero
payload with the requested sign. Destination bytes are literal binary64
`0000000000000000` or `0000000000000080`. Runner/schema tests check FST
preservation and FSTP pop/tag changes only after the Qword write. Interpreter
tests span TOP×FCW×sign×PE/C1, cross-width FLD-zero roundtrips, invalid source
and status gates, write faults and canonical boundaries. JIT/native admission
remains closed. See [S2-31am CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31am-signed-zero-fst-m64fp-and-fstp-m64fp), [plan](../../../../docs/plans/STAGE_2.md#s2-31am-store-signed-binary80-zero-through-bounded-m64-fstfstp), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31am--signed-zero-stores-for-bounded-m64-fstfstp).


### S2-31an signed-zero m80 FSTP fixtures

Exact `DB 38` accepts either a Valid-tagged canonical Normal source or a
Zero-tagged canonical binary80 zero. The positive bare fixture stores ten zero
bytes; the negative `DB 38 DF E0` fixture stores the sign byte in the final
position and observes the post-pop FSW through FNSTSW. Both are schema/CLI
reachable and included in native-denial coverage. Tag/payload mismatches remain
pre-access rejects, the ten-byte store remains atomic, and native/JIT admission
remains closed. See [S2-31an CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31an-signed-zero-fstp-m80fp-rax-db-38), [plan](../../../../docs/plans/STAGE_2.md#s2-31an-store-canonical-signed-binary80-zero-through-bounded-fstp-m80fp), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31an--signed-zero-binary80-stores-through-bounded-fstp-m80fp).


### S2-31ao signed-infinity m80 FSTP fixtures

Exact `DB 38` accepts a Special-tagged canonical infinity only when the raw
binary80 exponent is all ones, explicit integer bit is set, and fraction is
zero. Both positive bare and negative `DB 38 DF E0`/FNSTSW fixtures preserve
the ten literal bytes. Valid+Normal and Zero+Zero remain accepted. Pseudo-infinity,
NaNs, and tag/class mismatches reject before the data write; fixture schema/CLI
and native-denial checks include the signed-infinity cases. JIT/native execution
remains closed. See [S2-31ao CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ao-signed-infinity-fstp-m80fp-rax-db-38), [plan](../../../../docs/plans/STAGE_2.md#s2-31ao-store-canonical-signed-binary80-infinity-through-bounded-fstp-m80fp), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ao--signed-infinity-binary80-store-through-bounded-fstp-m80fp).


### S2-31ap signed-infinity m64 FST/FSTP fixtures

Exact `DD 10`/`DD 18` accept canonical binary80 infinity only with Special
tag, emitting `+∞` as `000000000000f07f` and `-∞` as
`000000000000f0ff`. The positive bare FST and negative FSTP+FNSTSW fixtures
exercise both operations through schema and CLI; both are included in native
denial. Existing Normal and signed-zero behavior remains. Noncanonical
specials, mismatched tags and forbidden status reject before the write. See the
[S2-31ap CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ap-signed-infinity-fstfstp-m64fp-rax),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ap-store-canonical-signed-binary80-infinity-through-bounded-m64-fstfstp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ap--signed-infinity-stores-for-bounded-m64-fstfstp).


### S2-31aq signed-infinity m32 FST/FSTP fixtures

Exact D910/D918 accept a Special-tagged canonical binary80 infinity and emit
literal binary32 `+∞` (`0000807f`) or `-∞` (`000080ff`). The positive bare FST
and negative FSTP+FNSTSW fixtures are schema/CLI reachable and included in
native denial. Noncanonical neighbors, mismatched tags, and forbidden status
reject before access. See the [S2-31aq CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31aq-signed-infinity-fstfstp-m32fp-rax),
[plan](../../../../docs/plans/STAGE_2.md#s2-31aq-store-canonical-signed-binary80-infinity-through-bounded-m32-fstfstp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31aq--signed-infinity-stores-for-bounded-m32-fstfstp).


### S2-31ar canonical QuietNaN DB38 fixtures

Exact DB38 preserves all ten bytes for a Special-tagged canonical binary80
QuietNaN. The positive bare payload fixture and negative-indefinite DFE0/FNSTSW
fixture exercise schema/CLI reachability, with native denial and JIT-denial
coverage. SNaNs, pseudo-QNaNs and mismatched tag/class pairs reject before
access. See the [S2-31ar CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ar-canonical-quietnan-fstp-m80fp-rax-db-38),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ar-store-canonical-binary80-quietnan-through-bounded-fstp-m80fp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ar--canonical-quietnan-binary80-store-through-bounded-fstp-m80fp).

### S2-31as canonical SignalingNaN DB38 fixtures

The positive bare `DB 38` fixture stores a minimum-payload canonical binary80
SignalingNaN. The negative `DB 38 DF E0` fixture stores a negative middle-
payload SNaN and observes FNSTSW. Both preserve all ten bytes, including the
clear quiet bit, and add no flags. See the [S2-31as CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31as-canonical-signalingnan-fstp-m80fp-rax-db-38),
[plan](../../../../docs/plans/STAGE_2.md#s2-31as-store-canonical-binary80-signalingnan-through-bounded-fstp-m80fp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31as--canonical-signalingnan-binary80-store-through-bounded-fstp-m80fp).

### S2-31at canonical-subnormal DB38 fixtures

The positive bare `DB 38` fixture stores the minimum canonical binary80
subnormal. The negative `DB 38 DF E0` fixture stores the maximum-payload
negative subnormal and observes FNSTSW. Both preserve all ten bytes and add no
flags. See the [S2-31at CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31at-canonical-subnormal-fstp-m80fp-rax-db-38),
[plan](../../../../docs/plans/STAGE_2.md#s2-31at-store-canonical-binary80-subnormal-through-bounded-fstp-m80fp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31at--canonical-subnormal-binary80-store-through-bounded-fstp-m80fp).


### S2-31au canonical QuietNaN m64 FST/FSTP fixtures

Exact DD10/DD18 accepts a Special-tagged canonical binary80 QuietNaN and emits
binary64 with the same sign and upper 52 fraction bits, truncating the lower
11. Positive bare FST and negative DFE0/FNSTSW FSTP fixtures exercise both
operations through schema/CLI; both are explicitly denied by native and JIT.
SNaNs and malformed/tag-mismatched classes remain rejection neighbors. See the
[S2-31au CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31au-canonical-quietnan-fstfstp-m64fp-rax-dd-10-dd-18),
[plan](../../../../docs/plans/STAGE_2.md#s2-31au-store-canonical-binary80-quietnan-through-bounded-fstfstp-m64fp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31au--canonical-quietnan-binary80-to-binary64-stores).


### S2-31av canonical QuietNaN m32 FST/FSTP fixtures

Exact D910/D918 accepts a Special-tagged canonical binary80 QuietNaN and emits
binary32 with the same sign and upper 23 fraction bits, truncating the lower
40. Positive bare FST and negative DFE0/FNSTSW FSTP fixtures exercise both
operations through schema/CLI; both are explicitly denied by native and JIT.
SNaNs and malformed/tag-mismatched classes remain rejection neighbors. See the
[S2-31av CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31av-canonical-quietnan-fstfstp-m32fp-rax-d9-10-d9-18),
[plan](../../../../docs/plans/STAGE_2.md#s2-31av-store-canonical-binary80-quietnan-through-bounded-fstfstp-m32fp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31av--canonical-quietnan-binary80-to-binary32-stores).


### S2-31aw canonical SignalingNaN m64 FST/FSTP fixtures

The positive bare DD10 fixture quiets a minimum-payload binary80 SNaN in the
binary64 destination and observes sticky IE. The negative DD18/DFE0 fixture
preserves a middle payload's upper 51 bits and observes IE through FNSTSW. Both
retain the source bytes, are schema/CLI reachable, and are included in native
and JIT denial. See the [S2-31aw CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31aw-canonical-signalingnan-fstfstp-m64fp-rax-dd-10-dd-18),
[plan](../../../../docs/plans/STAGE_2.md#s2-31aw-store-canonical-binary80-signalingnan-through-bounded-fstfstp-m64fp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31aw--canonical-signalingnan-binary80-to-binary64-stores).


### S2-31ax canonical SignalingNaN m32 FST/FSTP fixtures

The positive bare D910 fixture quiets a minimum-payload binary80 SNaN in the
binary32 destination and observes sticky IE. The negative D918/DFE0 fixture
preserves the upper 22 payload bits and observes IE through FNSTSW after pop.
Both retain the source bytes, execute through schema/CLI, and are included in
native and JIT denial. See the [S2-31ax CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ax-canonical-signalingnan-fstfstp-m32fp-rax-d9-10-d9-18),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ax-store-canonical-binary80-signalingnan-through-bounded-fstfstp-m32fp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ax--canonical-signalingnan-binary80-to-binary32-stores).


### S2-31ay sticky IE m32/m64 store fixtures

The m32 D910 fixture starts with masked IE already set and stores a positive
binary80 SNaN as binary32 QNaN while preserving IE. The m64 D918/DFE0 fixture
starts with IE set, stores a negative SNaN as binary64 QNaN, pops after the
write, and observes the sticky status through FNSTSW. Both are schema/CLI
reachable and included in JIT denial checks; native execution remains outside
the current runner. See the [S2-31ay CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ay-admit-sticky-ie-fstfstp-m32-m64),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ay-admit-sticky-ie-in-bounded-m32-m64-fst-fstp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ay--sticky-ie-m32-m64-stores).


### S2-31az sticky IE/DE/PE m32/m64 store compositions

The positive D900/DD10 fixture loads minimum binary32 subnormal and stores the
exact binary64 normal bytes `000000000000a036`, retaining DE. The negative
largest-subnormal D900/DD18/DFE0 fixture stores `000000c0ffff0fb8`, pops only
after the write, and observes retained DE with FNSTSW. Both exact programs are
schema/CLI reachable and explicitly denied by JIT without fallback. Native
admission denial is covered by the x86 runner's profile test; that test
cross-compiles here but native runtime was not executed on this arm64 host.
Physical acceptance remains deferred under ISSUE-019. See the
[S2-31az CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31az-admit-sticky-de-m32-m64-fstfstp),
[plan](../../../../docs/plans/STAGE_2.md#s2-31az-admit-sticky-de-in-bounded-m32-m64-fst-fstp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31az--sticky-de-in-bounded-m32-m64-stores).


### S2-31ba sticky IE/DE raw DB38 fixtures

The positive bare DB38 fixture and negative DB38/DFE0/FNSTSW fixture start
with masked IE, DE and PE set. Both preserve the exact raw ten-byte binary80
source through an atomic store and pop, retain all three sticky flags, and
observe status `0x4d23` after the pop. Interpreter/schema reachability and
explicit JIT denial are tested; the native-admission unit test is compiled for
the x86 runner target. Native runtime and physical behavior remain deferred
under ISSUE-019. See the [S2-31ba CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ba-sticky-ie-de-raw-fstp-m80fp-db-38),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ba-admit-sticky-ie-de-to-raw-binary80-fstp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ba--sticky-ie-de-on-raw-binary80-fstp-m80fp).


### S2-31bb maximum-finite binary32 FLD/store fixtures

The positive bare D900 fixture loads binary32 maximum finite. A positive D910
fixture stores the equivalent canonical binary80 value back to identical
binary32 bytes, and a negative D918/DFE0 fixture does the same through the
popping form and observes status after pop. Each fixture exercises exponent
127 through the schema/CLI interpreter path and remains explicitly denied by
JIT and native admission. See the
[S2-31bb CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bb-finite-exponent-127-bounded-m32-stores),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bb-finite-exponent-127-bounded-m32-stores),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bb--finite-exponent-127-bounded-m32-stores).


### S2-31bc maximum-finite binary64 FLD/store fixtures

The positive bare DD00 fixture loads binary64 maximum finite. A positive DD10
fixture stores the equivalent canonical binary80 value back to identical
binary64 bytes, and a negative DD18/DFE0 fixture does the same through the
popping form and observes status after pop. Each fixture exercises exponent
1023 through the schema/CLI interpreter path and remains explicitly denied by
JIT and native admission. See the
[S2-31bc CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bc-finite-exponent-1023-bounded-m64-stores),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bc-finite-exponent-1023-bounded-m64-stores),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bc--finite-exponent-1023-bounded-m64-stores).


### S2-31bd exact binary32 subnormal store fixtures

The positive bare D910 fixture stores the minimum binary32 subnormal from its
canonical binary80 source while retaining DE. The negative D918/DFE0 fixture
stores the signed minimum subnormal and observes sticky DE after the pop.
Both run through the schema/CLI interpreter path and are explicitly denied by
JIT and native admission. A direct ISA composition test covers D900 minimum-
subnormal FLD followed by D910, proving DE is retained without PE. See the
[S2-31bd CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bd-exact-binary32-subnormal-results),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bd-exact-binary32-subnormal-results-through-m32-stores),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bd--exact-binary32-subnormal-results-through-m32-stores).


### S2-31be exact binary64 subnormal store fixtures

The positive bare DD10 fixture stores the minimum binary64 subnormal from a
canonical binary80 source while retaining DE. The negative DD18/DFE0 fixture
stores the signed minimum subnormal and observes sticky DE after the pop. Both
run through the schema/CLI interpreter path and are explicitly denied by JIT
and native admission. A direct DD00 FLD→DD10/DD18 composition test verifies
exact recovery and DE retention without UE or PE. See the
[S2-31be CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31be-exact-binary64-subnormal-results),
[plan](../../../../docs/plans/STAGE_2.md#s2-31be-exact-binary64-subnormal-results-through-m64-stores),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31be--exact-binary64-subnormal-results-through-m64-stores).


### S2-31bf masked inexact binary32 subnormal store fixtures

The positive D910/FNSTSW and negative D918/DFE0/FNSTSW fixtures store rounded
1.5-minimum-subnormal values and expose sticky UE+PE; the negative FSTP case
also observes the pop-updated TOP. Direct ISA coverage verifies FNCLEX clears
the underflow and precision flags. Both fixtures run through the schema/CLI
interpreter path and remain explicitly denied by JIT and native admission.
See the [S2-31bf CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bf-masked-inexact-binary32-subnormal-results),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bf-masked-inexact-binary32-subnormal-results-through-m32-stores),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bf--masked-inexact-binary32-subnormal-results-through-m32-stores).


### S2-31bg masked inexact binary64 subnormal store fixtures

The positive DD10 fixture stores rounded 1.5 times the minimum binary64
subnormal. The negative DD18/DFE0 fixture rounds the same magnitude toward
negative and exposes sticky UE+PE and updated TOP with FNSTSW. Both are
interpreter CLI fixtures and remain explicitly denied by JIT and native
admission. Direct ISA coverage checks FNCLEX clearing and failed-write/range
atomicity. See the [S2-31bg CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bg-masked-inexact-binary64-subnormal-results),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bg-masked-inexact-binary64-subnormal-results-through-m64-stores),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bg--masked-inexact-binary64-subnormal-results-through-m64-stores).


### S2-31bh sticky UE re-entry fixture

The literal D910/DFE0 fixture starts with UE already set, performs a masked
inexact m32 store without FNCLEX, and observes retained UE+PE through FNSTSW.
ISA coverage exercises repeated stores without FNCLEX. FNCLEX remains available
to clear exception flags, but consecutive bounded stores no longer require it
when only admitted sticky flags are set. The fixture runs through the
interpreter CLI and remains denied by JIT and native admission.


### S2-31bi raw m80 store with sticky UE

The literal DB38/DFE0 fixture starts with UE alone and stores a canonical
negative binary80 signaling NaN unchanged. FNSTSW observes the preserved UE
without adding IE or PE. Direct ISA coverage also composes m32/m64 underflow
stores with raw m80 FSTP and checks source-byte preservation. The fixture runs
through the interpreter CLI and remains denied by JIT and native admission.


### S2-31bs raw m80 store with sticky OE

The literal DB38/DFE0 fixture starts with OE only and a canonical negative
binary80 signaling NaN. It stores all ten bytes unchanged; FNSTSW observes
`0x0808` after TOP advances, with IE and PE still clear. Direct ISA coverage
includes all 32 sticky combinations, cross-width overflow followed by DB38
without FNCLEX, and incoming-OE fault/range atomicity. The interpreter CLI
accepts the fixture; JIT and native admission deny it. See the [S2-31bs CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bs-sticky-oe-on-raw-binary80-fstp-m80fp),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bs-admit-sticky-oe-to-raw-binary80-fstp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bs--sticky-oe-on-raw-binary80-fstp-m80fp).


### S2-31bj extreme binary32 underflow fixture

The positive D910/DFE0 fixture uses a canonical source above half of the minimum
binary32 subnormal at exponent -150. Nearest rounding stores the minimum
subnormal and FNSTSW exposes UE+PE and C1. The fixture runs through the
interpreter CLI and is explicitly denied by JIT and native admission. Direct ISA
coverage includes both sides of the tie, signed-zero outcomes, the lowest source
exponent, and m32-underflow followed by raw m80 FSTP. See the
[S2-31bj CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bj-extreme-binary32-underflow-to-zero-or-minimum-subnormal),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bj-round-extreme-underflow-through-binary32-stores),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bj--extreme-binary32-underflow-to-zero-or-minimum-subnormal).


### S2-31bk extreme binary64 underflow fixture

The positive DD10/DFE0 fixture uses a canonical source above half of the minimum
binary64 subnormal at exponent -1075. Nearest rounding stores the minimum
subnormal and FNSTSW exposes UE+PE and C1. The fixture runs through the
interpreter CLI and is explicitly denied by JIT and native admission. Direct ISA
coverage includes both sides of the tie, signed-zero outcomes, the lowest source
exponent, and m64-underflow followed by raw m80 FSTP. See the
[S2-31bk CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bk-extreme-binary64-underflow-to-zero-or-minimum-subnormal),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bk-round-extreme-underflow-through-binary64-stores),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bk--extreme-binary64-underflow-to-zero-or-minimum-subnormal).

### S2-31bl binary80 subnormal to binary32 underflow fixture

The literal D910/DFE0 fixture stores a negative Special-tagged canonical
binary80 subnormal under round-toward-negative, producing the negative minimum
binary32 subnormal. FNSTSW exposes UE+PE and C1. The fixture runs through the
interpreter CLI and remains denied by JIT and native admission. Direct ISA
coverage checks all four directed-rounding/sign outcomes, payload extremes,
all TOPs/sticky flags, unchanged raw source and atomic failures. See the
[S2-31bl CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bl-binary80-subnormal-to-binary32-underflow-rounding),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bl-round-binary80-subnormals-through-binary32-stores),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bl--binary80-subnormal-to-binary32-underflow-rounding).

### S2-31bm binary80 subnormal to binary64 store fixtures

The positive DD10/DFE0 fixture rounds a Special-tagged binary80 subnormal toward
positive to the minimum binary64 subnormal and observes UE+PE and C1 through
FNSTSW. The negative DD18/DFE0 fixture rounds toward zero, stores negative zero,
and observes the popped TOP and UE+PE. Both are interpreter fixtures and remain
denied by JIT and native admission. Direct ISA coverage verifies all fraction,
sign, RC, TOP, sticky-flag and atomicity cases, including FNCLEX and raw-m80
source preservation. See the [S2-31bm CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bm-binary80-subnormal-to-binary64-underflow-rounding),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bm-round-binary80-subnormals-through-binary64-stores),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bm--binary80-subnormal-to-binary64-underflow-rounding).

### S2-31bn binary80 exponent -127 to binary32 minimum-normal fixtures

The positive D910/DFE0 fixture starts at the exact nearest-even boundary where
direct binary32 rounding selects minimum normal; FNSTSW observes UE+PE and C1.
The negative D918/DFE0 fixture rounds toward negative to minimum normal, pops
TOP, and observes PE+C1 without a new UE because unbounded 24-bit rounding
carries. Both fixtures use Valid-tagged canonical Normal sources, run only in
the interpreter, and remain denied by JIT/native admission. See the [S2-31bn
CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bn-binary80-exponent--127-to-binary32-minimum-normal),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bn-round-exponent--127-binary80-normals-directly-to-binary32-minimum-normal),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bn--binary80-normal-to-binary32-minimum-normal-rounding).

### S2-31bo binary80 exponent -1023 to binary64 minimum-normal fixtures

The positive DD10/DFE0 fixture begins at the nearest-even binary64
minimum-normal threshold; FNSTSW exposes UE+PE+C1. The negative DD18/DFE0
fixture rounds toward negative to minimum normal, pops TOP and observes PE+C1
without new UE because unbounded 53-bit rounding carries. Both use
Valid-tagged canonical Normal sources, run in the interpreter and remain denied
by JIT/native admission. Direct ISA coverage checks both destination thresholds,
the independent UE threshold, signed RC neighbors, TOP/sticky-flag matrices,
and write/range atomicity. See the [S2-31bo CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bo-binary80-exponent--1023-to-binary64-minimum-normal),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bo-round-exponent--1023-binary80-normals-directly-to-binary64-minimum-normal),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bo--binary80-normal-to-binary64-minimum-normal-rounding).

### S2-31bp exponent-127 binary80 overflow to binary32 infinity fixtures

The positive D910/DFE0 fixture uses the nearest-even halfway value from maximum
finite binary32 to positive infinity; FNSTSW observes OE+PE and C1. The negative
D918/DFE0 fixture rounds toward negative to negative infinity, pops TOP, and
observes OE+PE+C1. Both use Valid-tagged canonical Normal binary80 exponent-127
sources, run only in the interpreter, and remain denied by JIT/native admission.
The ISA matrix covers the nearest and directed thresholds, signs, RCs, TOPs,
sticky flags, exponent-128 rejection, and atomic failures. See the [S2-31bp CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bp-binary80-exponent-127-overflow-to-binary32-infinity),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bp-round-exponent-127-binary80-normals-to-binary32-infinity-on-overflow),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bp--binary80-normal-to-binary32-infinity-on-overflow).


### S2-31bq exponent-1023 binary80 overflow to binary64 infinity fixtures

Positive nearest FST and negative toward-negative FSTP fixtures store signed
binary64 infinity and expose OE+PE+C1 through FNSTSW. The interpreter accepts
them; JIT/native admission denies them. The ISA matrix covers exact maximum
finite through the overflow midpoint and maximum significand, both signs, all
RCs, stack positions, sticky flags, exponent-1024 rejection, and atomic write
faults. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bq-binary80-exponent-1023-overflow-to-binary64-infinity),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bq-round-exponent-1023-binary80-normals-to-binary64-infinity-on-overflow),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bq--binary80-normal-to-binary64-infinity-on-overflow).

### S2-31br sticky status admission fixture

The OE-only fixture executes an exact binary80 1.0 m64 FST with OE already
sticky, then FNSTSW. The store is admitted, OE remains set, and PE/C1 remain
clear. CLI validates the resulting status and memory; JIT and native admission
remain closed. ISA tests separately cover all 32 sticky IE/DE/OE/UE/PE
combinations, ZE/SF/ES/B rejection, conversion-specific flags and C1, overflow
re-entry without FNCLEX, and atomic faults. See the [S2-31br CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31br-sticky-status-admission-for-bounded-m32m64-fstfstp),
[plan](../../../../docs/plans/STAGE_2.md#s2-31br-admit-sticky-status-on-bounded-m32m64-stores),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31br--sticky-status-admission-for-bounded-m32m64-stores).

### S2-31dx historical ZE in bounded m32/m64 FST/FSTP

Four literal fixtures cover exact bare `D9 10`, `D9 18`, `DD 10`, and `DD 18`
stores with historical ZE already set. They verify result bytes, retained ZE,
source/tag/TOP/RIP state, and FNSTSW AX after FSTP. ISA tests cover other
historical sticky-flag combinations, current conversion flags, and write/range
rollback. The schema accepts the newly admitted status while JIT remains
denied. Native runtime and physical x86 acceptance remain deferred under
ISSUE-019. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-31dx--historical-ze-admission-for-bounded-m32m64-fstfstp),
[plan](../../../../docs/plans/STAGE_2.md#s2-31dx-admit-historical-ze-on-bounded-m32m64-fstfstp-stores),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31dx--historical-ze-for-bounded-m32m64-fstfstp).

### S2-31dy historical ZE on raw FSTP m80fp

The literal DB38/FNSTSW fixture starts with ZE+OE and a Special-tagged SNaN.
It verifies that the ten raw source bytes, including the signaling payload,
are stored unchanged; ZE/OE remain sticky, C1 clears, and the FSTP pop commits
after the write. Schema and ISA tests cover all accepted source classes and
other sticky histories, ZE-set malformed source/tag rejection, repeated
stores, rollback and FNCLEX. JIT/native execution is denied; no physical x86
acceptance is claimed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-31dy--historical-ze-admission-for-raw-fstp-m80fp),
[plan](../../../../docs/plans/STAGE_2.md#s2-31dy-admit-historical-ze-on-exact-raw-db38-fstp-m80fp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31dy--historical-ze-on-raw-fstp-m80fp).

### S2-31bt exact signed-zero FIST/FISTP m32int fixtures

The negative-zero `DB 10` and positive-zero `DB 18` fixtures store the literal
Dword zero and compose with FNSTSW to observe preserved PE, cleared C1, and
FIST's unchanged versus FISTP's advanced TOP/tag. ISA coverage includes both
signs, all RCs/TOPs, source/tag/control rejection, and fault/range atomicity.
JIT and native admission remain closed. See the [S2-31bt CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bt-exact-signed-zero-fist-m32int-rax-db-10-db-18),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bt-exact-signed-zero-fist-m32int),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bt--exact-signed-zero-fist-m32int).

### S2-31bu exact signed-zero FIST/FISTP m16int fixture

The negative-zero `DF 18` fixture stores the literal Word zero and composes
with FNSTSW to observe preserved PE, cleared C1, and the popped TOP/tag. ISA
coverage includes both signs, all RCs/TOPs, FIST and FISTP, payload/tag/control
and prefix rejection, and write/range atomicity. JIT and native admission
remain closed. See the [S2-31bu CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bu-exact-signed-zero-fist-m16int-rax-df-10-df-18),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bu-exact-signed-zero-fist-m16int),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bu--exact-signed-zero-fist-m16int).

### S2-31bv exact signed-zero FISTP m64int fixture

The negative-zero `DF 38` fixture stores the literal Qword zero and composes
with FNSTSW to observe preserved PE, cleared C1, and the popped TOP/tag. ISA
coverage includes both signs, all RCs/TOPs, canonical/tag/class/control and
prefix rejection, and write/range atomicity. JIT and native admission remain
closed. See the [S2-31bv CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bv-exact-signed-zero-fistp-m64int-rax-df-38),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bv-exact-signed-zero-fistp-m64int),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bv--exact-signed-zero-fistp-m64int).

### S2-31bw fractional FISTP m32int fixture

The negative `-0.25` `DB 18` fixture uses round-toward-negative, stores Dword
`0xffffffff`, and composes with FNSTSW to observe PE+C1 and the popped TOP/tag.
The ISA matrix separately checks both signs, four RC modes, both opcodes, all
TOP values, the half and adjacent encodings, minimum Normal and extreme
exponents, source/environment rejection, and fault/range atomicity. JIT and
native admission remain closed. See the [S2-31bw CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bw-round-normal-binary80-fractions-m32-fist),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bw-round-binary80-fractions-through-bounded-m32-fist),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bw--round-binary80-fractions-through-m32-fistfstp).

### S2-31bx fractional FISTP m16int fixture

The negative `-0.25` `DF 18` fixture uses round-toward-negative, stores Word
`0xffff`, and composes with FNSTSW to observe PE+C1 and the popped TOP/tag. The
ISA matrix separately checks both signs, four RC modes, both opcodes, all TOP
values, half neighbors, minimum Normal and extreme exponents, source/environment
rejection, and fault/range atomicity. JIT and native admission remain closed.
See the [S2-31bx CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bx-round-normal-binary80-fractions-m16-fist),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bx-round-binary80-fractions-through-bounded-m16-fist),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bx--round-binary80-fractions-through-m16-fistfstp).

### S2-31by fractional FISTP m64int fixture

The negative `-0.25` `DF 38` fixture uses round-toward-negative, stores the
Qword `0xffffffffffffffff`, and composes with FNSTSW to observe PE+C1 and the
popped TOP/tag. The ISA matrix checks both signs, all four RCs and TOPs,
incoming PE/C1, the half threshold and neighbors, largest fraction below one,
minimum Normal and extreme exponents, rejection classes/gates, and Qword
write/range atomicity. JIT and native admission remain closed. See the [S2-31by
CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31by-round-normal-binary80-fractions-m64-fistp),
[plan](../../../../docs/plans/STAGE_2.md#s2-31by-round-normal-binary80-fractions-through-bounded-m64-fistp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31by--round-binary80-fractions-through-m64-fistp).

### S2-31bz masked infinity FISTP m32int fixture

The negative-infinity `DB 18` fixture stores integer indefinite `0x80000000`
under masked exceptions and composes with FNSTSW to observe IE and the popped
TOP/tag. A direct ISA sequence composes FISTP with FNSTSW and FNCLEX to verify
that IE is visible before it is cleared. The unit matrix covers both signs,
all admitted FCWs, both opcodes, every TOP, incoming PE/C1, malformed/tag/class
and environment rejection, finite -2^31 distinction, and write/range
atomicity. JIT and native admission remain closed. See the [S2-31bz CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31bz-masked-canonical-infinity-fist-m32int),
[plan](../../../../docs/plans/STAGE_2.md#s2-31bz-masked-binary80-infinity-through-bounded-m32-fist),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31bz--masked-binary80-infinity-through-m32-fistfstp).

### S2-31ca masked infinity FISTP m16int fixture

The negative-infinity `DF 18` fixture stores Word integer indefinite `0x8000`
under masked exceptions and composes with FNSTSW to observe IE and the popped
TOP/tag. A direct ISA sequence composes FISTP with FNSTSW and FNCLEX to verify
that IE is observed before clearing. The unit matrix covers both signs, all
admitted FCWs, both opcodes, every TOP, incoming PE/C1, malformed/tag/class
and environment rejection, finite -32768 distinction, and write/range
atomicity. JIT and native admission remain closed. See the [S2-31ca CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ca-masked-canonical-infinity-fist-m16int),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ca-masked-binary80-infinity-through-bounded-m16-fist),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ca--masked-binary80-infinity-through-m16-fistfstp).

### S2-31cb masked infinity FISTP m64int fixture

The negative-infinity `DF 38` fixture stores Qword integer indefinite
`0x8000000000000000` under masked exceptions and composes with FNSTSW to
observe IE and the popped TOP/tag. A direct ISA sequence composes FISTP with
FNSTSW and FNCLEX to verify the flag is observed before clearing. ISA tests
cross signs, all admitted FCWs, every TOP, and incoming PE/C1; distinguish
infinity from exact -2^63; reject malformed classes/tags and controls; and
check Qword write/range atomicity. JIT and native admission remain closed. See
the [S2-31cb CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cb-masked-canonical-infinity-fistp-m64int),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cb-masked-binary80-infinity-through-bounded-m64-fistp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cb--masked-binary80-infinity-through-m64-fistp).


### S2-31cc masked quiet-NaN FISTP m32int fixture

The negative canonical quiet-NaN `DB 18` fixture stores Dword integer
indefinite `0x80000000` under masked exceptions and composes with FNSTSW to
observe IE and the popped TOP/tag. ISA coverage crosses payload shapes, both
signs, all admitted FCWs, both opcodes, all TOPs, and incoming PE/C1; it rejects
J=0 pseudo-NaNs, SNaN, mismatched tags, and checks write/range atomicity. JIT and
native admission remain closed. See the [S2-31cc CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cc-masked-canonical-qnan-fist-m32int),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cc-masked-binary80-qnan-through-bounded-m32-fist),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cc--masked-binary80-qnan-through-m32-fistfstp).


### S2-31cd masked quiet-NaN FISTP m16int fixture

The negative canonical quiet-NaN `DF 18` fixture stores Word integer
indefinite `0x8000` under masked exceptions and composes with FNSTSW to observe
IE and the popped TOP/tag. ISA coverage crosses payload shapes, both signs, all
admitted FCWs, both opcodes, every TOP, and incoming PE/C1; it rejects J=0
pseudo-NaNs, SNaN, mismatched tags and checks Word write/range atomicity. JIT and
native admission remain closed. See the [S2-31cd CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cd-masked-canonical-qnan-fist-m16int),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cd-masked-binary80-qnan-through-bounded-m16-fist),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cd--masked-binary80-qnan-through-m16-fistfstp).


### S2-31ce masked quiet-NaN FISTP m64int fixture

The negative canonical quiet-NaN `DF 38` fixture stores Qword integer
indefinite `0x8000000000000000` under masked exceptions and composes with FNSTSW
to observe IE and the popped TOP/tag. ISA coverage crosses payload shapes, both
signs, all admitted FCWs, every TOP, and incoming PE/C1; it rejects J=0
pseudo-NaNs, SNaN, mismatched tags and checks Qword write/range atomicity. JIT
and native admission remain closed. See the [S2-31ce CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ce-masked-canonical-qnan-fistp-m64int),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ce-masked-binary80-qnan-through-bounded-m64-fistp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ce--masked-binary80-qnan-through-m64-fistp).


### S2-31cf masked signaling-NaN FISTP m32int fixture

The negative canonical signaling-NaN `DB 18` fixture stores Dword integer
indefinite `0x80000000` under masked exceptions and composes with FNSTSW to
observe IE and the popped TOP/tag. The raw SNaN payload remains unchanged.
ISA coverage crosses four payload shapes, both signs, all admitted FCWs, both
opcodes, every TOP and incoming PE/C1 (2,048 cases); it rejects J=0
pseudo-NaNs, mismatched tags, and checks Dword write/range atomicity. JIT and
native admission remain closed. See the [S2-31cf CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cf-masked-canonical-snan-fist-m32int),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cf-masked-binary80-snan-through-bounded-m32-fist),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cf--masked-binary80-snan-through-m32-fistfstp).


### S2-31cg masked signaling-NaN FISTP m16int fixture

The negative canonical signaling-NaN `DF 18` fixture stores Word integer
indefinite `0x8000` under masked exceptions and composes with FNSTSW to observe
IE and the popped TOP/tag. The raw SNaN payload remains unchanged. ISA coverage
crosses four payload shapes, both signs, all admitted FCWs, both opcodes,
every TOP and incoming PE/C1 (2,048 cases); it rejects J=0 pseudo-NaNs,
mismatched tags, and checks Word write/range atomicity. JIT and native admission
remain closed. The exact m64 `DF 38` form is handled separately in S2-31ch.
See the [S2-31cg CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cg-masked-canonical-snan-fist-m16int),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cg-masked-binary80-snan-through-bounded-m16-fist),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cg--masked-binary80-snan-through-m16-fistfstp).


### S2-31ch masked signaling-NaN FISTP m64int fixture

The negative canonical signaling-NaN `DF 38` fixture stores Qword integer
indefinite `0x8000000000000000` under masked exceptions and composes with
FNSTSW to observe IE and the popped TOP/tag. The raw SNaN source payload stays
unchanged after FISTP. ISA coverage crosses four payload shapes, both signs,
all admitted FCWs, every TOP and incoming PE/C1 (1,024 cases); it rejects J=0
pseudo-NaNs and mismatched tags, and checks Qword write/range atomicity. JIT
and native admission remain closed. See the [S2-31ch CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ch-masked-canonical-snan-fistp-m64int),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ch-masked-binary80-snan-through-bounded-m64-fistp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ch--masked-binary80-snan-through-m64-fistp).


### S2-31ci sticky IE FIST/FISTP fixtures

The SNaN DB 10 fixture observes masked-invalid IE with FNSTSW AX, without
adding PE or changing the raw signaling source. The exact finite DB 18 fixture
starts with IE set and PE clear, stores integer 1 rather than
integer-indefinite, and observes retained IE with FNSTSW. A separate
interpreter composition test runs masked-invalid FIST, SNaN FISTP and exact
finite FISTP in sequence without FNCLEX, then observes and clears exception
flags. ISA tests cover all five integer-store forms, both signs, all RCs/TOPs,
four IE/PE input combinations, both prior C1 states, and IE+PE write/range
atomicity plus IE combined with each forbidden FSW bit. JIT/native admission
remains closed. See the [S2-31ci CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ci-admit-sticky-ie-bounded-integer-stores),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ci-admit-sticky-ie-on-bounded-integer-stores),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ci--sticky-ie-on-bounded-integer-stores).


### S2-31cj finite overflow and roundback m32 integer-store fixtures

The DB 18 fixture uses exact `+2147483647.5` under nearest-even, stores Dword
integer-indefinite and observes IE with PE clear through FNSTSW. The DB 10
fixture uses `+2147483647.25` under toward-negative rounding; it rounds back to
`INT32_MAX`, stores that finite result, and observes PE without IE. ISA tests
cover both opcodes and signs, exact signed limits, adjacent binary80 values
around both overflow ties, exponents 32/63/64/16383, all RC/TOP/sticky IE/PE
states and prior C1; overflow write and canonical-range failures preserve full
state and memory. JIT/native admission remains closed. See the
[S2-31cj CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cj-masked-finite-overflow-m32-fist),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cj-masked-finite-overflow-to-m32-fistfstp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cj--masked-finite-overflow-to-m32-fistfstp).

### S2-31ck finite overflow and roundback m16 integer-store fixtures

The DF 18 fixture uses `+32767.5` under nearest-even, stores Word
integer-indefinite, and observes IE with PE clear through FNSTSW. The DF 10
fixture uses `+32767.25` under toward-negative rounding; it rounds back to
`INT16_MAX`, stores that finite result, and observes PE without IE. The earlier
S2-31d/S2-31e `+32768` and exponent-16 fixtures now execute successfully with
IE as well. ISA
tests cover signed limits, adjacent binary80 values around both half-integer
thresholds, large exponents, all RC/TOP/sticky IE/PE/prior-C1 states, and
denied-write/canonical-range atomicity for both opcodes. JIT/native admission
remains closed. See the
[S2-31ck CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ck-masked-finite-overflow-m16-fist),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ck-masked-finite-overflow-to-m16-fistfstp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ck--masked-finite-overflow-to-m16-fistfstp).


### S2-31cl finite overflow and roundback m64 integer-store fixture

The existing `s2_31f_fistp_m64int_positive_overflow` literal fixture is now
admitted by S2-31cl. With `DF 38` and `DF E0`, positive `+2^63` stores Qword
integer-indefinite, adds IE without PE, pops only after the write, and exposes
FSW `0x4d01` in AX via FNSTSW. Its split memory regions verify the Qword and
surrounding canaries; the source binary80 payload remains unchanged. ISA
coverage uses 26 signed literal inputs across all RC/TOP/IE/PE/C1 combinations
(6,656 executions), including half-integer roundback, exact `i64::MIN`, and
large exponents. Write-denial and canonical-range faults preserve full state
and memory. JIT and native admission remain closed. See the
[S2-31cl CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cl-masked-finite-overflow-m64-fistp),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cl-masked-finite-overflow-to-m64-fistp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cl--masked-finite-overflow-to-m64-fistp).


### S2-31cm sticky DE integer-store fixtures

The m32 `DB 18` and m64 `DF 38` fixtures start with the canonical binary80
Normal values produced by FLD of the negative largest source-width subnormal,
plus incoming DE. Directed-round-down stores integer `-1`, adds PE and C1,
retains DE, and composes with FNSTSW. The fixture schema accepts this exact
FISTP/FNSTSW pair; the complete FLD→store sequence across minimum/maximum
subnormals and all five integer forms is covered by the 40-case ISA composition
test. JIT and native admission remain closed. See the
[S2-31cm CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cm-admit-sticky-de-bounded-integer-stores),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cm-admit-sticky-de-on-bounded-integer-stores),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cm--sticky-de-on-bounded-integer-stores).


### S2-31cn sticky UE integer-store fixtures

The m32 `DB 18` and m64 `DF 38` fixtures seed the canonical binary80 Normal
value 1.5 times the minimum binary32 subnormal and incoming UE. Nearest
FISTP rounds to integer zero, adds PE, retains UE, pops after the write, and
exposes FSW `0x4d30` with FNSTSW. JIT and native admission remain closed. The
ISA composition test also executes FST m32 first, producing UE|PE, then checks
all five bounded integer forms across signs and FCWs. See the
[S2-31cn CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cn-admit-sticky-ue-bounded-integer-stores),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cn-admit-sticky-ue-on-bounded-integer-stores),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cn--sticky-ue-on-bounded-integer-stores).


### S2-31co sticky OE bounded integer-store fixtures

The m32 and m64 `FISTP`/`FNSTSW` fixtures carry incoming OE with the exact
Normal binary80 operand produced by the preceding subnormal-load fixtures.
They verify that OE remains sticky through finite integer conversion, PE, the
pop, and status-word observation. JIT and native admission remain denied.
See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31co-admit-sticky-oe-bounded-integer-stores),
[plan](../../../../docs/plans/STAGE_2.md#s2-31co-admit-sticky-oe-on-bounded-integer-stores),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31co--sticky-oe-on-bounded-integer-stores).


### S2-31cp historical ZE bounded integer-store fixtures

The m32 and m64 `FISTP`/`FNSTSW` fixtures seed ZE alone with a canonical
binary80 Normal operand and verify that the historical flag survives the
finite conversion, pop and status-word observation. These fixtures describe
only seeded ZE; they do not execute division or produce ZE. JIT and native
admission remain denied. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cp-admit-historical-sticky-ze-bounded-integer-stores),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cp-admit-historical-sticky-ze-on-bounded-integer-stores),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cp--historical-sticky-ze-on-bounded-integer-stores).


### S2-31cq canonical-subnormal m32 FISTP fixture

`s2_31cq_fistp_m32int_negative_subnormal_fnstsw.case.json` seeds a
Special-tagged negative canonical binary80 subnormal under
round-toward-negative, then executes literal `DB 18` and `DF E0`. It exercises
the Dword store and sticky PE/C1 observation. The instruction implementation
also has a unit composition that loads the raw ten-byte subnormal with FLD m80,
executes FIST and FISTP, observes with FNSTSW, and clears flags with FNCLEX. JIT/native
admission remains denied. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cq-canonical-subnormal-m32-fist),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cq-canonical-subnormal-m32-fist),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cq--canonical-subnormal-m32-fist).


### S2-31cr canonical-subnormal m16 FISTP fixture

`s2_31cr_fistp_m16int_negative_subnormal_fnstsw.case.json` seeds a
Special-tagged negative canonical binary80 subnormal under round-toward-
negative, then executes literal `DF 18` and `DF E0`. The Word store is checked
across adjacent canary regions; FNSTSW observes sticky PE and C1 after FISTP.
The ISA unit composition loads the ten-byte source with FLD m80, executes both
m16 stores, observes the status, and clears exception flags with FNCLEX. JIT
and native admission remain denied. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cr-canonical-subnormal-m16-fist),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cr-canonical-subnormal-m16-fist),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cr--canonical-subnormal-m16-fist).


### S2-31cs canonical-subnormal m64 FISTP fixture

`s2_31cs_fld_fistp_m64int_negative_subnormal_fnstsw_fnclex.case.json` loads a
literal negative binary80 subnormal from unaligned m80 memory, changes RAX to
an unaligned Qword destination, then executes `DF 38`, `DF E0`, and `DB E2`.
It checks all-ones Qword storage between sentinels, status observation of PE/C1,
FNCLEX clearing, TOP wrap/pop and preservation of the popped source's raw ten
bytes. JIT and native admission remain denied. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cs-canonical-subnormal-m64-fistp),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cs-canonical-subnormal-m64-fistp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cs--canonical-subnormal-m64-fistp).


### S2-31ct Special-tagged pseudo-infinity m32 FISTP fixture

`s2_31ct_fistp_m32int_negative_pseudo_infinity_fnstsw.case.json` seeds a
Special-tagged J=0 negative pseudo-infinity with raw sign/exponent `0xffff`,
then executes literal `DB 18` and `DF E0`. It checks Dword integer-indefinite,
sticky IE observed by FNSTSW, source payload preservation, and surrounding
canaries. Unit tests cover both signs, FIST/FISTP, FCW/TOP/sticky/C1 matrices,
FNCLEX, malformed classes/tags, excluded widths and atomic write/range faults.
JIT/native admission remains denied; FLD composition is excluded because FLD
does not accept pseudo-infinity. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ct-pseudo-infinity-m32-fist),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ct-pseudo-infinity-m32-fist),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ct--special-tagged-pseudo-infinity-through-m32-fist).


### S2-31cu Special-tagged pseudo-infinity m16 FISTP fixture

`s2_31cu_fistp_m16int_negative_pseudo_infinity_fnstsw.case.json` seeds a
Special-tagged J=0 negative pseudo-infinity with raw sign/exponent `0xffff`,
then executes literal `DF 18` and `DF E0`. It checks Word integer-indefinite,
sticky IE observed by FNSTSW, source payload preservation, and the byte canaries
around the two-byte write. ISA tests cover the complete FCW/form/TOP/sticky/C1
matrix, FNCLEX, malformed classes/tags, m64 exclusion and atomic Word-write/
range faults. JIT/native admission remains denied; FLD composition is excluded
because FLD does not accept pseudo-infinity. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cu-pseudo-infinity-m16-fist),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cu-pseudo-infinity-m16-fist),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cu--special-tagged-pseudo-infinity-through-m16-fist).


### S2-31cv Special-tagged pseudo-infinity m64 FISTP fixture

`s2_31cv_fistp_m64int_negative_pseudo_infinity_fnstsw.case.json` seeds a
Special-tagged negative J=0 pseudo-infinity with raw sign/exponent `0xffff`,
then executes literal `DF 38`, `DF E0`, and `DB E2`. It checks Qword
integer-indefinite between canaries, IE observed by FNSTSW and cleared by
FNCLEX, raw source retention, and the popped TOP/tag. ISA tests cover both
signs, the 8,192-case FCW/TOP/sticky/C1 matrix, malformed classes/tags,
canonical zero/infinity controls, and Qword write/range atomicity. JIT/native
admission remains denied; FLD composition is excluded because FLD rejects
pseudo-infinity. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cv-pseudo-infinity-m64-fistp),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cv-pseudo-infinity-m64-fistp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cv--special-tagged-pseudo-infinity-through-m64-fistp).


### S2-31cw Special-tagged pseudo-NaN m32 FISTP fixture

`s2_31cw_fistp_m32int_negative_pseudo_nan_fnstsw.case.json` seeds a
Special-tagged negative J=0 pseudo-quiet-NaN with raw significand
`0x4000000000000001`, then executes literal `DB 18` and `DF E0`. It checks
Dword integer-indefinite between byte canaries, IE observed by FNSTSW, the raw
source payload, and the popped TOP/tag. An ISA composition check also verifies
FNCLEX clears IE. ISA coverage spans 65,536 FCW/form/TOP/sticky/C1 combinations
and confirms the existing pseudo-infinity and canonical J=1 QNaN/SNaN behavior
remains distinct; it rejects non-all-one exponents, wrong tags and m16/m64
pseudo-NaNs. Write/range faults preserve state and memory. JIT/native admission
remains denied. At the S2-31cw boundary, m16/m64 pseudo-NaNs were rejected;
S2-31cx below later admits only the bounded m16 DF10/DF18 cases, while m64
remains unsupported. FLD composition is excluded because FLD rejects pseudo-NaNs.
See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cw-special-pseudo-nan-m32-fist),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cw-special-pseudo-nan-m32-fist),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cw--special-tagged-pseudo-nans-through-m32-fist).


### S2-31cx Special-tagged pseudo-NaN m16 FISTP fixture

`s2_31cx_fistp_m16int_negative_pseudo_nan_fnstsw.case.json` seeds a
Special-tagged negative J=0 pseudo-quiet-NaN with significand
`0x4000000000000001`, then executes literal `DF 18` and `DF E0`. It checks Word
integer-indefinite at the data region boundary with a preceding byte and
following bytes preserved, IE observed by FNSTSW, raw source
preservation, and FISTP TOP/tag behavior. ISA coverage spans 65,536 FCW/form/
TOP/sticky/C1 combinations, checks canonical pseudo-infinity/QNaN/SNaN controls,
and rejects wrong tags/classes, other exponents, m64 pseudo-NaNs and forbidden
controls. Word write and canonical-range faults preserve state and memory. JIT/
native admission remains denied; FLD and unmasked delivery are outside the
slice. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cx-special-pseudo-nan-m16-fist),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cx-special-pseudo-nan-m16-fist),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cx--special-tagged-pseudo-nans-through-m16-fist).


### S2-31cy Special-tagged pseudo-NaN m64 FISTP fixture

`s2_31cy_fistp_m64int_negative_pseudo_nan_fnstsw.case.json` seeds a
Special-tagged negative J=0 pseudo-quiet-NaN with raw significand
`0x4000000000000001`, then executes literal `DF 38`, `DF E0`, and `DB E2`.
It checks Qword integer-indefinite between canaries, IE observed by FNSTSW and
cleared by FNCLEX, raw source retention, and the popped TOP/tag. ISA coverage
crosses 32,768 FCW/TOP/sticky/C1 combinations, both bit-62 variants and signs;
it also covers zero/pseudo-infinity and canonical J=1 controls, neighboring
classes/tags, denied-write/canonical-range atomicity, and unchanged m16/m32
behavior. JIT/native admission remains denied; FLD and unmasked delivery remain
outside the slice. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cy-special-pseudo-nan-m64-fistp),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cy-special-pseudo-nan-m64-fistp),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cy--special-tagged-pseudo-nan-through-m64-fistp).


### S2-31cz Special-tagged unnormal m32 FISTP fixture

`s2_31cz_fistp_m32int_negative_unnormal.case.json` seeds a
Special-tagged negative binary80 unnormal (exponent `0x3fff`, J=0, fraction
`0x4000000000000000`), then executes literal `DB 18`. It checks Dword
integer-indefinite between canaries, sticky IE, raw source retention, and the
popped TOP/tag. Separate ISA composition coverage observes IE with FNSTSW and
clears it with FNCLEX. ISA coverage
crosses three finite nonzero exponents, four fractions, both signs, four FCWs,
both DB forms, every TOP, all 64 sticky combinations and both prior C1 values.
Wrong tags, exponent boundaries, m16/m64, forbidden controls/status, and
denied-write/canonical-range atomicity are covered. JIT/native admission stays
denied; FLD composition and unmasked delivery are excluded. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31cz-special-unnormals-m32-fist),
[plan](../../../../docs/plans/STAGE_2.md#s2-31cz-special-unnormals-m32-fist),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31cz--special-tagged-unnormals-through-m32-fist).

### S2-31da Special-tagged unnormal m16 FISTP fixture

`s2_31da_fistp_m16int_negative_unnormal.case.json` seeds a Special-tagged
negative binary80 unnormal (exponent `0x3fff`, J=0, fraction
`0x4000000000000000`) and executes literal `DF 18`. It checks Word
integer-indefinite between canaries, sticky IE, unchanged raw source, and the
popped TOP/tag. The ISA matrix covers three finite exponents, four fractions,
both signs, four FCWs, both DF forms, every TOP, all 64 sticky combinations and
both prior C1 values. Exponent boundaries, J=1, wrong tags, unchanged m32 and
excluded m64 forms, forbidden controls/status, and denied-write/canonical-range
atomicity are also checked. FNSTSW/FNCLEX observes and clears IE; JIT/native
admission remains denied. FLD composition and unmasked delivery are excluded. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31da-special-unnormals-m16-fist),
[plan](../../../../docs/plans/STAGE_2.md#s2-31da-special-unnormals-m16-fist), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31da--special-tagged-unnormals-through-m16-fist).

### S2-31db Special-tagged unnormal m64 FISTP fixture

`s2_31db_fistp_m64int_negative_unnormal.case.json` seeds a Special-tagged
negative binary80 unnormal (exponent `0x3fff`, J=0, fraction
`0x4000000000000000`) and executes literal `DF 38`. It checks Qword
integer-indefinite between canaries, sticky IE, unchanged raw source, and the
popped TOP/tag. The ISA matrix covers three finite exponents, four fractions,
both signs, four FCWs, every TOP, all 64 sticky combinations and both prior C1
values. Exponent boundaries, J=1, wrong tags, excluded forms, forbidden
controls/status, and write/range atomicity are checked separately.
FNSTSW/FNCLEX observes and clears IE; JIT/native admission remains denied. FLD
composition and unmasked delivery are excluded. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31db-special-unnormals-m64-fistp),
[plan](../../../../docs/plans/STAGE_2.md#s2-31db-special-unnormals-m64-fistp), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31db--special-tagged-unnormals-through-m64-fistp).
## S2-31do FRNDINT canonical subnormal fixture

The seeded literal `s2_31do_frndint_seeded_subnormal_fnstsw.case.json` runs
`D9 FC DF E0` with Special-tagged canonical positive binary80 subnormal
fraction `0x123` and sticky IE/DE/PE in the initial project fixture state.
FRNDINT produces positive zero with Zero tag, retains DE/PE and the other
seeded status, and FNSTSW AX exposes `0x6d23`. Provenance is explicitly
`project_fixture`: these initial flags and bytes are handwritten fixture
state, not evidence from native x86 execution. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31do-canonical-binary80-subnormal-frndint-d9-fc),
[plan](../../../../docs/plans/STAGE_2.md), and [report](../../../../docs/reports/STAGE_2_REPORT.md).

## S2-31dp FRNDINT pseudo-denormal fixture

The literal `s2_31dp_frndint_fld_pseudo_denormal_fnstsw.case.json` runs
`DB 28 D9 FC DF E0` over positive binary80 pseudo-denormal bytes with E=0,
J=1 and zero fraction. The project fixture starts with clean x87 exception
status. FLD preserves the ten raw bytes and Special tag without setting DE;
FRNDINT rounds to positive zero, changes the tag to Zero, and sets DE+PE, which
FNSTSW exposes as `0x3822` with TOP=7. Provenance is `project_fixture`; it is
not evidence from native x86 execution. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31dp-special-tagged-binary80-pseudo-denormal-frndint-d9-fc),
[plan](../../../../docs/plans/STAGE_2.md#s2-31dp-special-tagged-binary80-pseudo-denormal-through-bounded-frndint), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31dp--special-tagged-binary80-pseudo-denormal-through-bounded-frndint).

## S2-31dq FRNDINT unnormal fixture

The literal `s2_31dq_frndint_seeded_unnormal_fnstsw_fnclex.case.json` starts
with a raw negative Special-tagged binary80 unnormal seeded directly in the
project x87 snapshot; it does not load it through FLD. The bytes execute
`D9 FC DF E0 DB E2`: masked FRNDINT replaces the source with fixed negative
indefinite `00000000000000c0ffff`, adds IE and clears C1; FNSTSW AX observes
`0x4501`; FNCLEX clears IE while preserving the QNaN indefinite. The provenance
is `project_fixture`, not native x86 evidence. JIT/native admission stays
closed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31dq-special-unnormal-frndint-d9-fc),
[plan](../../../../docs/plans/STAGE_2.md#s2-31dq-special-tagged-binary80-unnormal-through-bounded-frndint), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31dq--special-tagged-binary80-unnormal-through-bounded-frndint).

## S2-31dr FRNDINT pseudo-infinity fixture

The literal `s2_31dr_frndint_seeded_pseudo_infinity_fnstsw_fnclex.case.json`
starts with raw negative binary80 pseudo-infinity bytes `0000000000000000ffff`
and Special tag seeded directly in the project x87 snapshot. `D9 FC` replaces
it with fixed negative indefinite `00000000000000c0ffff` and sets IE; `DF E0`
observes FSW `0x4501`, and `DB E2` clears exception status while retaining the
indefinite. Provenance is `project_fixture`, not native x86 evidence. JIT/native
admission remains closed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31dr-special-pseudo-infinity-frndint-d9-fc),
[plan](../../../../docs/plans/STAGE_2.md#s2-31dr-special-tagged-binary80-pseudo-infinity-through-bounded-frndint), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31dr--special-tagged-binary80-pseudo-infinity-through-bounded-frndint).

## S2-31ds FRNDINT pseudo-NaN fixtures

The literal `s2_31ds_frndint_seeded_pseudo_nan_q0_fnstsw_fnclex.case.json` and
`s2_31ds_frndint_seeded_pseudo_nan_q1_fnstsw_fnclex.case.json` seed Special-
tagged E=`0x7fff`, J=0 fractions with Q bit clear/set respectively. Both run
`D9 FC DF E0 DB E2`: FRNDINT replaces the source with the fixed negative
indefinite `00000000000000c0ffff`, FNSTSW observes `0x4501`, and FNCLEX clears
IE while preserving the result. The raw sign/payload are not preserved and
canonical SNaN quieting is not applied. Provenance is `project_fixture`; these
fixtures are not native x86 evidence. JIT/native admission remains closed. See
Intel SDM revision 093 Vol.2A FRNDINT p.3-372 and Vol.1 §8.2.2 Tables 8-3/
8-10 and Table 4-3 for the normative basis. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ds-special-pseudo-nan-frndint-d9-fc),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ds-special-tagged-binary80-pseudo-nan-through-bounded-frndint),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ds--special-tagged-binary80-pseudo-nans-through-bounded-frndint).

## S2-31dt historical UE in bounded m32/m64 FLD

`s2_31dt_fld_m32fp_seeded_ue_fnstsw.case.json` and
`s2_31dt_fld_m64fp_seeded_ue_fnstsw.case.json` seed only historical UE, load
exact binary32/binary64 `1.0` with `D9 00`/`DD 00`, and observe FSW using
`DF E0`. The resulting status remains UE-only before observation; the fixture
provenance identifies the initial status as directly seeded project state, not
as a guest-produced underflow. Separate interpreter matrices cover current
subnormal DE, SNaN IE/quieting, source classes, all FCWs/TOP and prior
IE/DE/UE/PE combinations. The environment-reject fixtures use SF after
S2-31dv admitted ZE. See [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31dt-historical-sticky-ue-for-bounded-m32m64-fld), [plan](../../../../docs/plans/STAGE_2.md#s2-31dt-admit-historical-sticky-ue-through-exact-binary32binary64-fld), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31dt--historical-sticky-ue-for-bounded-m32m64-fld).

## S2-31du historical OE in bounded m32/m64 FLD

The `s2_31du_fld_m32fp_seeded_oe_fnstsw.case.json` and
`s2_31du_fld_m64fp_seeded_oe_fnstsw.case.json` fixtures seed only historical
OE, load exact `1.0` with `D9 00`/`DD 00`, then observe FSW with `DF E0`. The
normal loads preserve OE and create no IE/DE/PE. The four
`s2_31du_fst_fld_m{32,64}fp_overflow_infinity_{nearest,upward}_fnstsw_fnclex`
fixtures use actual same-width stores that overflow to positive infinity,
reload it at the same width, observe sticky OE/PE, then clear flags with
FNCLEX. Producers use nearest and toward-positive-infinity RC; no overflow-to-
infinity producer is claimed for toward-zero-directed RC. These are
`project_fixture` results, not native x86 evidence. Exact bare, budget-one/no-
stop Normal Unsupported inputs are schema-tested with OE-only mutation and
remain rejected; SF replaces ZE in the environment-reject fixtures. See [CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31du-historical-sticky-oe-for-bounded-m32m64-fld), [plan](../../../../docs/plans/STAGE_2.md#s2-31du-admit-historical-sticky-oe-through-exact-binary32binary64-fld), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31du--historical-sticky-oe-for-bounded-m32m64-fld).

## S2-31dv historical ZE in bounded m32/m64 FLD

The bounded bare `D9 00`/`DD 00` gates admit and preserve supplied historical
ZE with the other five sticky flags. The 65,536-case interpreter matrix spans
both widths, eight source classes, four FCWs, all TOP values, all 64 sticky
histories and prior C1. A separate composition checks ZE through FLD and
FNSTSW, then confirms FNCLEX clears it. S2-31ak's environment-reject fixtures
seed SF, and a schema regression rejects ZE-only Unsupported fixtures because
ZE is now admitted. No ZE producer or physical x86 claim is made; native/JIT
admission remains closed and ISSUE-019 remains open. See [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31dv-historical-sticky-ze-for-exact-binary32binary64-fld), [plan](../../../../docs/plans/STAGE_2.md#s2-31dv-admit-historical-sticky-ze-through-exact-binary32binary64-fld), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31dv--historical-sticky-ze-for-bounded-m32m64-fld).

## S2-31dw historical SF with IE in bounded m32/m64 FLD

The exact bare `D9 00`/`DD 00` FLD gates admit historical SF only when IE is
also set. SF-only, ES and B remain pre-read rejects; the ISA test includes an
SNaN operand to prove current IE cannot make SF-only admissible. Its 57,344
full-state cases cover both widths and both signs of zero, minimum-normal,
interior-Normal, infinity, QNaN, SNaN and minimum-subnormal vectors, all 32
DE/ZE/OE/UE/PE combinations with SF+IE, the four FCWs, every TOP and prior C1,
including payload/tag/metadata and memory canaries.
FLD→FNSTSW observes SF+IE and FNCLEX clears both flags. The schema-v2 regression
keeps the exact bare budget-one/no-stop Unsupported case from being justified
by SF+IE. Native/JIT and physical x86 acceptance remain closed; no SF producer
or exception-delivery claim is made. See [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31dw-historical-sf-with-ie-for-exact-binary32binary64-fld), [plan](../../../../docs/plans/STAGE_2.md#s2-31dw-admit-historical-sfie-through-exact-binary32binary64-fld), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31dw--historical-sf-with-ie-for-bounded-m32m64-fld).


### S2-30as signed-zero FSTP ST(i)

The handwritten fixture `s2_30as_fld_fstp_sti_fnstsw.case.json` uses exactly
`d900ddd9dfe0`: FLD m32fp loads negative zero, FSTP ST(1) copies the bytes and
Zero tag, empties the old physical TOP, and advances TOP; FNSTSW observes the
new TOP. The six-byte stream retires three instructions and checks full state
and memory through the interpreter CLI. Unit coverage checks both zero signs ×
all TOP positions × all destinations × all 16 condition-code states, destination
sentinels, `i=0`, `i=1`, TOP wrap, Normal regressions, malformed/wrong-tag and
bad-environment rejection, and atomic execution boundaries. Local m32/m64
compositions cover both signs. Native/JIT and physical acceptance remain
unverified; ISSUE-019 remains open.

### S2-30bc historical IE through bounded FCHS/FABS

Four handwritten schema-v2 producer fixtures load positive/negative binary32
SNaNs, run FCHS or FABS, store at the same width and execute FNSTSW. The
interpreter assertions inspect the quieted binary80 QNaN+IE immediately after
FLD and verify sign/payload and IE after the operation/store. Bare Normal-source
fixtures separately check IE-only success and IE+SF atomic rejection. The
6,144-case unit matrix spans the six accepted source classes, both operations,
signs, TOP, prior CC and IE state; forbidden exception/summary combinations
remain rejected. Native/JIT and physical behavior remain outside this slice;
ISSUE-019 is open. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-30bc-historical-ie-fchs-fabs),
[plan](../../../../docs/plans/STAGE_2.md#s2-30bc-admit-historical-sticky-ie-to-bounded-fchsfabs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-30bc--historical-sticky-ie-through-bounded-fchs-fabs).

### S2-30bd historical DE through bounded FCHS/FABS

Four handwritten schema-v2 m32 producers load positive/negative subnormals,
apply FCHS or FABS, store at the same width and observe FSW with FNSTSW. CLI
checks verify that FLD's DE remains set through the sign instruction and store,
and that the output has the expected sign. Seeded DE-only Normal succeeds;
DE+ZE and IE+DE+SF reject atomically, and JIT denies the profile. Unit
compositions cover m32/m64 minimum, interior and largest subnormals, both signs,
both operations and initial IE clear/set. Local evidence does not establish
native or physical behavior; ISSUE-019 remains open. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-30bd-historical-de-fchs-fabs),
[plan](../../../../docs/plans/STAGE_2.md#s2-30bd-admit-historical-sticky-de-to-bounded-fchsfabs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-30bd--historical-sticky-de-through-bounded-fchsfabs).


### S2-30be historical PE through bounded FCHS/FABS

Two exact seeded schema-v2 cases run `D9 FC D9 E0 DF E0` and
`D9 FC D9 E1 DF E0` from positive and negative 1.5 under FCW `0x037f`.
FRNDINT produces PE; FCHS/FABS clear C1 and preserve PE before FNSTSW observes
it. Bare Normal-source fixtures admit PE-only and IE|DE|PE, while PE|OE and
IE|DE|PE|SF rejected atomically at that stage. S2-30bg later admits OE; the
former PE|OE case was renamed `s2_30be_fchs_normal_pe_sf` and now rejects PE|SF
(`0x0060`) after S2-30bh admits historical ZE. All six S2-30be fixtures exercise interpreter CLI
behavior; JIT remains denied. These are project fixtures, not native or
physical evidence. A local interpreter composition additionally checks
canonical subnormal→FRNDINT→FCHS→FABS, preserving signed zero and DE|PE through
both sign operations; it asserts the Zero tag, complete binary80 raw bytes and
full FSW `0x0022` at each intermediate result before FNSTSW/FNCLEX. ISSUE-019
remains open. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-30be-historical-pe-fchs-fabs), [plan](../../../../docs/plans/STAGE_2.md#s2-30be-admit-historical-sticky-pe-to-bounded-fchsfabs), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-30be--historical-sticky-pe-through-bounded-fchsfabs).


### S2-30bf historical UE through bounded FCHS/FABS

At this stage the bounded source filter admitted all IE/DE/UE/PE history
combinations while rejecting ZE/OE/SF/ES/B; S2-30bg later admits OE. The former
UE|OE case was renamed `s2_30bf_fchs_normal_ue_es` and now rejects ZE|UE|ES
(`0x0094`) after S2-30bh admits historical ZE. Unit producer compositions use non-popping FST m32/m64
under FCW `0x037f` for ±2^-150 / ±3×2^-151 and ±2^-1075 / ±3×2^-1076; they
assert signed-zero or minimum-subnormal bytes, UE|PE, C1 clear/set, and the exact
unchanged Normal+Valid binary80 source before applying both FCHS and FABS. The
sign operation's following assertion starts from the recorded producer status,
so it proves preservation rather than flag generation. CLI coverage uses exact
streams `d910d9e0dfe0` and `dd10d9e1dfe0`, with positive history and negative
status combinations, and JIT denial. These are local project checks only;
ISSUE-019 remains open. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-30bf-historical-ue-fchs-fabs),
[plan](../../../../docs/plans/STAGE_2.md#s2-30bf-admit-historical-sticky-ue-to-bounded-fchsfabs), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-30bf--historical-sticky-ue-through-bounded-fchs-fabs).


### S2-30bg historical OE through bounded FCHS/FABS

The bounded source filter admits all IE/DE/OE/UE/PE histories while rejecting
ZE/SF/ES/B. Its full-state matrix covers 98,304 cases; forbidden-status checks
cover all four disallowed bits crossed with every history. Under FCW `0x037f`,
non-popping FST m32/m64 overflow producers test midpoint−1, midpoint,
midpoint+1 and maximum significand for both signs. The boundary and above
produce signed infinity with OE|PE and C1=1; midpoint−1 adds no OE. Full-state
and canary memory checks confirm the source survives FST unchanged, FCHS/FABS
change only its sign, clear C1, and preserve the already-produced flags. The
unit harness covers all producer/opcode/sign combinations and failure atomicity;
CLI fixtures use exact streams `d910d9e0dfe0` and `dd10d9e1dfe0`, seeded
OE-only success and ZE rejects, with JIT denial. ISSUE-019 remains open; no
Mac mini, SSH, native runtime or physical x86 checks were performed. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-30bg-historical-oe-through-bounded-fchsfabs),
[plan](../../../../docs/plans/STAGE_2.md#s2-30bg-admit-historical-sticky-oe-to-bounded-fchsfabs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-30bg--historical-sticky-oe-through-bounded-fchsfabs).


### S2-30bh historical ZE through bounded FCHS/FABS

The exact sign-operation gate admits all 64 IE/DE/ZE/OE/UE/PE histories while
SF, ES and B continue to reject atomically. Seeded CLI fixtures cover ZE-only
and all-six-flags success, ZE|SF rejection and all-six-flags|ES rejection; ZE
is set explicitly in the initial x87 state, and no guest instruction here
produces divide-by-zero. Obsolete DE|ZE and PE|ZE rejects were renamed with
SF/ES status combinations. The FRNDINT compositions preserve seeded ZE while
FRNDINT adds PE for inexact inputs or IE while quieting an SNaN; FCHS/FABS then
preserve those histories and clear C1. These project fixtures are not native
evidence. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-30bh-historical-ze-through-bounded-fchsfabs),
[plan](../../../../docs/plans/STAGE_2.md#s2-30bh-admit-historical-sticky-ze-to-bounded-fchsfabs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-30bh--historical-sticky-ze-through-bounded-fchsfabs).

### S2-30bi historical SF|IE through bounded FCHS/FABS

Four schema-v2 fixtures exercise seeded status histories. The bare FCHS case
starts with canonical +1 and FSW `0x0241` (IE|SF|C1), then accepts the operation
and clears C1 to `0x0041`. The bare FABS case starts with -1 and the same status.
Two exact streams, `d9fcd9e0dfe0` and `d9fcd9e1dfe0`, seed ±1.5 with IE|SF;
FRNDINT adds PE/C1, FCHS/FABS clears C1, and FNSTSW observes `0x0061` at TOP=0.
Fixtures seed IE|SF directly in the initial project snapshot; no guest
stack-fault producer exists in this scope, and FRNDINT is only composition.
Former IE|SF rejection fixtures now test ES in addition to IE|SF and use
`s2_30bd_fchs_normal_ie_de_es`, `s2_30be_fchs_normal_ie_de_pe_es`, and
`s2_30bf_fchs_normal_ie_de_ue_pe_es` IDs. SF-only and SF|DE without IE remain
atomic rejects in the interpreter matrix. These are
local project fixtures, not native evidence. Separate ISA unit coverage checks
full state after each seeded IE|SF FRNDINT, FCHS/FABS, FNSTSW and FNCLEX step,
including FRNDINT quieting an SNaN to QNaN and FNCLEX clearing SF and exception
bits. ISSUE-019 remains open. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-30bi-historical-sf-ie-fchs-fabs),
[plan](../../../../docs/plans/STAGE_2.md#s2-30bi-admit-historical-sf-only-with-ie-to-bounded-fchsfabs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-30bi--historical-sf-only-with-ie-through-bounded-fchsfabs).

Normative source: [Intel SDM Vol.1 revision 081 §8.1.3.4 p.8-6 and §8.5.1.1
p.8-26](https://cdrdv2-public.intel.com/789574/253665-sdm-vol-1.pdf#page=208).

### S2-30bj narrow FXCH ST(1)

The local `s2-long-x87-state-v1` schema admits only bare `D9 C9`,
`D9 C9 DF E0` (status observation), and `D9 C9 D9 C9 DF E0` (double-exchange
roundtrip and observation). Seeded TOP=7/C1 success preserves the physical tag
word and exchanges the two wrapped entries' full binary80 bytes; the roundtrip
restores their bytes while C1 remains clear. Empty/wrong-tag unsupported cases
retire zero instructions and preserve the complete initial snapshot. This is
local project-fixture evidence only, not native or physical x86 evidence.

### S2-30bk bounded FXCH ST(i)

The `s2-long-x87-state-v1` schema now admits one exact bare `D9 C8+i` for each
`i=0..7`, optionally followed by `DF E0`, or the same indexed exchange twice
followed by `DF E0`. Literal runner fixtures exercise self-exchange, `i=7`
physical wrap, nonadjacent same-index roundtrip, wrong participating tag, and
JIT denial. These fixtures establish only local interpreter and schema
behavior; they do not establish native or physical x86 acceptance.

### S2-30bl canonical signed-zero FXCH

FXCH keeps the S2-30bk exact instruction and environment gates, and accepts
either canonical-Normal Valid entries or exact signed-zero Zero entries. A
nonzero index exchanges both raw ten-byte payloads and the two physical tags;
self-exchange checks TOP only. Unit coverage adds 32,256 mixed indexed states
and 256 self-Zero states while retaining the prior 15,360 Normal-only states.
The runner fixture checks a Zero/Normal `i=7` wrap with raw bytes and the full
tag word, opposite signed-zero exchange followed by a same-index roundtrip,
untouched state and JIT denial. Malformed Zero and Empty/Special rejection are
checked atomically. These are local interpreter/schema checks, not native or
physical x86 evidence.

### S2-30bm bounded `FINCSTP` / `FDECSTP`

The local schema admits only `d9f7`, `d9f6`, `d9f7dfe0`, `d9f6dfe0`,
`d9f7d9f6dfe0`, and `d9f6d9f7dfe0`. The literal FINCSTP/FNSTSW and
FDECSTP/FNSTSW fixtures seed TOP=7 and C1, then observe the post-adjustment
status. Two inverse-pair fixtures observe TOP=7 after FINCSTP/FDECSTP and
FDECSTP/FINCSTP. All eight physical payloads use distinct opaque canaries, and the tag
word includes mixed tags and Empty entries. Unit coverage additionally checks
eight repeats, both inverse orders, TOP wrap, all nine rejected FSW bits,
atomic instruction boundaries, and FXCH using the adjusted TOP. Every step
preserves all raw cells and the full tag word. These are local interpreter and
schema results only; JIT/native and physical x86 acceptance remain deferred
under ISSUE-019. Undefined C0/C2/C3 preservation is a deterministic local
policy, not a hardware-equality claim.

### S2-30bn: FLD1 and FLDZ constant pushes

Four handwritten schema-v2 success fixtures cover the exact allowed byte strings `d9e8`, `d9ee`, `d9e8dfe0`, and `d9eedfe0`. The first pair executes one constant push; the latter pair observes execution through FNSTSW AX. A fifth fixture reuses bare `d9e8` with an occupied destination and checks Unsupported with zero retirement. Local full-state unit coverage additionally checks all TOP/condition-code/snapshot combinations, opaque malformed bytes under Empty, all occupied tag rejects, eight successful pushes followed by atomic ninth-push rejection, and FLD1→FLDZ→FXCH→FSTP m80fp byte/tag order. Fixtures and interpreter tests are not native or physical evidence; ISSUE-019 remains open.


### S2-30bo bounded `FLD ST(0)`

Four exact schema-v2 streams are admitted: `d9c0`, `d9c0dfe0`, `d9e8d9c0dfe0`, and `d9eed9c0dfe0`. The bare fixture seeds negative zero with TOP=0 to check physical wrap to slot 7 and raw/tag transfer; the status-observation fixture seeds Normal at TOP=7 to copy into physical slot 6. Two composition fixtures verify FLD1/FLDZ producer values can be duplicated and observed with FNSTSW. Separate budget-one fixtures reject an Empty source and an occupied destination. All are local interpreter/schema evidence. JIT/native and physical x86 behavior remain unverified under ISSUE-019.


### S2-30bp bounded `FLD ST(i)`

Six handwritten schema-v2 success fixtures cover each newly admitted encoding `d9c1` through `d9c6`. The C1 fixture selects physical source `(TOP+1)&7` with TOP=7; the C6 fixture selects a nonadjacent source while logical ST(0) is Empty. An additional C1 fixture copies negative zero from ST(1) through TOP=0 to wrapped physical slot 7 while ST(0) is Empty. Two budget-one fixtures reject a wrong source tag and an occupied push destination. Runner assertions compare copied raw value/tag, retained source, new TOP, C1 clearing, retirement and JIT denial. These are local interpreter/schema results only; native and physical x86 acceptance remains deferred under ISSUE-019.


### S2-30bq bounded `FFREE ST(0)`

S2-30br generalizes the tag-only behavior below from `DD C0` to all indexed
forms. The original 3,072-case ST(0) matrix and its literal fixtures remain as
regression coverage.

### S2-30br bounded `FFREE ST(i)`

The schema admits exactly nineteen streams: each `ddc0` through `ddc7` alone
and each followed by `dfe0`; `ddc0d9c0dfe0`; `ddc0d9f7d9e8dfe0`; and
`ddc7d9e8dfe0`. Seven indexed literal fixtures use TOP=7 and C1=1 to cover
physical index wrap, mixed tags, opaque SNaN/malformed payloads and
`FFREE ST(7)→FLD1` reuse. The reuse fixture starts physical slot 6 with a
Special tag and opaque payload; FFREE must empty it before FLD1 can commit.
An additional indexed forbidden-FSW fixture checks
atomic rejection. The 24,576-case interpreter matrix covers all TOPs, indices,
CC patterns, prior tags and opaque payloads. The operation changes only the
selected physical tag and preserves payloads, all condition-code bits and TOP.
CLI snapshots and explicit JIT denial are local interpreter/schema results,
not native or physical x86 evidence. ISSUE-019 remains open.

### S2-30bs bounded FNOP

Six handwritten schema-v2 fixtures cover the four exact success streams
`d9d0`, `d9d0dfe0`, `d9d0d9d0dfe0`, and `ddc7d9d0d9e8dfe0`, plus bare
`d9d0` at budget one with forbidden ES|B and invalid FCW. Success fixtures
include TOP=7/C1=1 with opaque mixed cells, all-Empty raw canaries, repeated
FNOP, and `FFREE ST(7)→FNOP→FLD1→FNSTSW`. They are local interpreter inputs;
the runner checks snapshots and JIT denial. No native or physical x86 evidence
is claimed, and ISSUE-019 remains open.

### S2-30bt bounded FXAM

Seven exact schema-v2 success streams cover bare FXAM, FNSTSW, repeated FXAM,
FLD1/FLDZ producers, and signed results after FCHS. The direct fixture uses
negative Normal at TOP=7; producer fixtures assert FSW `0x3c00` (FLD1),
`0x7800` (FLDZ), `0x3e00` (negative Normal after FCHS), and `0x7a00`
(negative zero after FCHS). Five literal rejects cover wrong tag, Empty,
Special, ES|B, and invalid FCW. Local runner assertions check full snapshots,
FIP and JIT denial. These are local results only; native/physical x86 behavior
remains open under ISSUE-019.

### S2-30bu bounded `FCMOVB ST(0), ST(1)`

The schema accepts exactly `dac1`, `dac1dfe0`, `dac1dac1dfe0`,
`d9e8d9e0dbf1dac1dfe0`, and `d9e8d9e0d9e8dbf1dac1dfe0` with budgets
1/2/3/6/6. Ten handwritten local fixtures cover taken and untaken moves with
distinct participant tags, exact negative-zero copying, TOP=7 wrap, repeat,
real `FLD1/FCHS/FCOMI` producers for the two relation directions, invalid
source with CF=0, invalid destination, incoming C1=1 and bad FCW. The runner
checks local snapshots and JIT denial. These fixtures do not establish native
or physical x86 behavior; ISSUE-019 remains open.

The `ST(1)`-only register extent above is historical; S2-30bv supersedes it for
the indexed `DA C0..C7` forms.

### S2-30bv bounded indexed `FCMOVB ST(0), ST(i)`

The schema accepts the eight exact bare streams `dac0` through `dac7`, each
with budget 1, plus `dac0dfe0` through `dac7dfe0`, each with budget 2. It also
retains the S2-30bu `dac1dac1dfe0` repeat and two exact producer streams, for
19 streams total. Eight new literal success fixtures exercise every source
index with TOP=7 and FNSTSW observation; source data/tag combinations include
the signed-zero source and canonical Normals, and every indexed fixture is
checked by the runner under interpreter and JIT-denial modes. Three indexed
reject fixtures cover an invalid source with CF=0, C1=1, and bad FCW. Unit
matrices additionally cover both CF outcomes, every TOP, full supported status
patterns, `i=0` with Special/malformed non-TOP canaries and no incidental tag
reads, and the complete invalid class/tag pair matrix. The CLI self-index
fixture keeps physical ST(0..6) Special-tagged with opaque malformed bytes;
the C1 reject fixture gives its indexed ST(7) a valid canonical Normal so the
status bit is the rejecting condition. All evidence is local;
native/JIT execution and physical x86 behavior remain outside ISSUE-019.

### S2-30bw bounded `FCMOVNB ST(0), ST(1)`

The exact schema admits `dbc1`, `dbc1dfe0`, `dbc1dbc1dfe0`,
`d9e8d9e8d9e0dbf1dbc1dfe0`, and `d9e8d9e0d9e8dbf1dbc1dfe0` with budgets
1/2/3/6/6. Ten literal fixtures cover taken and untaken paths, distinct tags,
negative zero, TOP=7 wrap, repeat, both real producer streams, invalid source
under CF=1, invalid destination, C1, and bad FCW. Runner snapshots check
interpreter behavior and JIT denial. All evidence is local; ISSUE-019 remains
open.

### S2-30bx bounded indexed `FCMOVNB ST(0), ST(i)`

The schema admits exactly `dbc0..dbc7` (budget 1) and each corresponding
`dbc0dfe0..dbc7dfe0` status-observation stream (budget 2), plus the three
existing repeat/producer streams, for 19 total. Eight new literal fixtures
cover indexed self-moves under both CF values and source indices 2 through 7;
the existing DB C1 fixture completes index coverage. They include TOP wrap,
Normal-to-Zero and Zero-to-Normal copies, signed zero and malformed unrelated
slots for self-index. Runner snapshots verify each index and deny JIT. The
local interpreter also checks invalid class/tag pairs with CF=1, per-index
environment and execution boundaries, and the FCMOVNB/FCMOVB CF-complement
property. This is local evidence only; ISSUE-019 remains open.

### S2-30bz bounded indexed `FCMOVE ST(0), ST(i)`

The local schema admits exactly `dac8..dacf`, each followed by `dfe0`, plus the
existing repeat and two producer streams (19 total, budgets 1/2/3/6/6). Eight
new success fixtures cover self-index with both ZF outcomes and indices 2..7;
existing `DA C9` fixtures cover index 1. Together they exercise TOP wrap,
distinct tags, Valid/Zero transfer and signed zero. New rejects cover an
invalid index-7 source with ZF=0 and an index-7 environment gate. The runner
checks complete x87 snapshots for every success fixture and checks the repeat
after both steps. It also verifies all prior ST(1) success payload/tag/state
results and denies JIT. ISA matrices cover 363,520 successes and 161,280
atomic class/tag rejects across all indices, per-index environment and
execution boundaries. This is local evidence only; ISSUE-019 remains open.

### S2-30ca bounded bare `FCMOVNE ST(0), ST(1)`

Historical ST(1)-only fixture slice, superseded by indexed S2-30cb below.

Two handwritten literal fixtures cover ZF=0 raw payload/tag copying and ZF=1
operand preservation for exact `DB C9`, including signed zero and distinct
tags. A third fixture has an invalid source with ZF=1 and checks atomic
rejection. They are local interpreter evidence and do not claim native
execution.

### S2-30cb indexed bare `FCMOVNE ST(0), ST(i)`

Eight literal DB C8..CF success fixtures cover the self-index forms with both
ZF outcomes and source indices 2..7, including TOP wrap, exact Normal/Zero
values and raw negative-zero/tag transfer. Additional fixtures check a
two-transfer stream, both FCOMI producer sequences, an invalid index-7 source
with ZF=1, and an index-7 forbidden environment. The schema admits exactly 19
streams: each indexed instruction alone and followed by FNSTSW, the repeated
C9 stream, and the two producers. Runner checks full x87 snapshots, RFLAGS,
FIP/RIP and retirement; every fixture denies JIT. The local interpreter
matrices cover 363,520 successes, 161,280 invalid class/tag rejects, and
per-index environment/status/boundary atomicity. This is local evidence only;
ISSUE-019 remains open and no native or physical x86 execution is claimed.

### S2-30cc historical bare `FCMOVBE ST(0), ST(1)` (`DA D1`)

Ten handwritten fixtures cover the four integer `CF || ZF` truth-table rows,
including raw signed-zero/tag transfer and untaken preservation, TOP=7 wrap,
FNSTSW observation, a two-transfer repeat, and three real FCOMI producer cases
(greater, equal followed by FCHS, and less). Two reject fixtures cover an
invalid source with both condition flags set and a forbidden status
environment. The schema admits exactly six streams; these fixtures provide
complete local interpreter snapshots and deny JIT. They are not native or
physical x86 evidence.

### S2-30cd indexed bare `FCMOVBE ST(0), ST(i)` (`DA D0..D7`)

The exact schema contains twenty streams: each `DA D0..D7` bare form and the
same form followed by `DF E0` FNSTSW AX, the retained `DA D1` repeat, and the
three retained FCOMI producer streams. New literals cover indices 0 and 2..7,
all four CF/ZF self-index rows, indexed negative-zero/tag copy, TOP wrap,
invalid operands with both untaken and taken conditions, and an indexed bad
environment. The ST(1) fixtures and producers remain as compatibility coverage.
The runner compares complete state and memory, including RFLAGS, RIP, FIP and
retirement, on each accepted index and on both repeated transfers; it also
checks JIT denial. These are local interpreter fixtures only and establish no
native or physical x86 behavior.

### S2-30ce bare `FCMOVNBE ST(0), ST(1)` (`DB D1`)

Eight handwritten success fixtures cover all four integer CF/ZF rows, signed
negative-zero raw/tag transfer with TOP wrap, a two-transfer stream, and the
three FCOMI producer cases: greater is taken; equal-then-FCHS and less are
untaken. Two reject fixtures cover an invalid source under an untaken
condition and a forbidden FCW environment. The exact schema allowlist is six
streams: `dbd1`, `dbd1dfe0`, `dbd1dbd1dfe0`,
`d9e8d9e0d9e8dbf1dbd1dfe0`, `d9e8d9e8dbf1d9e0dbd1dfe0`, and
`d9e8d9e8d9e0dbf1dbd1dfe0` with budgets 1/2/3/6/6/6. Runner snapshots
compare full machine state and memory and explicitly deny JIT. These fixtures
are local interpreter evidence only; ISSUE-019 and physical x86 behavior
remain open.

### S2-30cf indexed bare `FCMOVNBE ST(0), ST(i)` (`DB D0..D7`)

The exact 20-stream schema admits each DB D0..D7 form alone and followed by
FNSTSW AX, the retained DB D1 repeat, and the three retained FCOMI producers.
New literals cover all four self-index CF/ZF rows and representative indices
2..7 with and without FNSTSW, including wrap, canonical Normal/Zero and raw
negative-zero/tag copy. Invalid source fixtures exercise taken and untaken
conditions; existing environment, boundary, status observation, repeat, and
producer fixtures remain covered. Runner assertions compare complete machine
state and memory, including RFLAGS, RIP, FIP and retirement, and deny JIT for
every indexed form. This is local interpreter evidence only.


### S2-30cg historical bare `FCMOVU ST(0), ST(1)` (`DA D9`)

Two handwritten fixtures run the exact five-instruction FLD1/FLDZ/TEST/FCMOVU/FNSTSW
producer stream. RAX=3 makes `TEST RAX,RAX` set PF with ZF clear, so FCMOVU
copies ST(1); RAX=1 clears PF while ZF also stays clear, so it preserves ST(0).
The schema rejects other streams and encodings. Historical interpreter
coverage was 512 successes and 768 invalid class/tag snapshots. The two
fixtures remain as compatibility coverage and do not establish indexed
behavior; unordered x87 values and physical x86 execution are not covered.

### S2-30ch indexed bare `FCMOVU ST(0), ST(i)` (`DA D8..DF`)

The exact schema admits 18 streams: the eight bare DA D8..DF encodings at
budget 1, each followed by FNSTSW AX at budget 2, the DA D9 repeat plus
FNSTSW at budget 3, and the retained FLD1/FLDZ/TEST/DA D9/FNSTSW producer at
budget 5. Nine new literal bare fixtures cover self-index PF=0/1 and indices
1..7; a repeat fixture and both earlier producer fixtures remain. Rejection
literals cover invalid source with PF=0/1, invalid destination, C1, ES, B and
bad FCW. The runner compares complete state and memory and checks JIT denial
for accepted and rejected programs. Unit interpreter coverage is 363,520
successes and 161,280 invalid class/tag snapshots; all evidence is local only.
No unordered/NaN semantics, native behavior or physical x86 execution is
claimed.

### S2-30ci historical exact bare `FCMOVNU ST(0), ST(1)` (`DB D9`)

Five handwritten literal fixtures cover taken PF=0 raw transfer, invalid
source while PF=1, repeat plus FNSTSW, and both TEST producers. The producer
program is exactly `D9 E8 D9 EE 48 85 C0 DB D9 DF E0` (11 bytes): RAX=1 gives
PF=0/ZF=0 and takes the move; RAX=3 gives PF=1/ZF=0 and preserves ST(0).
The schema allows those four stream shapes only. Local unit tests compare each
producer intermediate state and full-state success/rejection matrices; these
fixtures do not establish native or physical behavior. The one-index scope is
superseded by S2-30cj; these fixtures remain compatibility coverage.

### S2-30cj indexed bare `FCMOVNU ST(0), ST(i)` (`DB D8..DF`)

Nine bare success fixtures cover indices 1..7 plus self-index PF=0 and PF=1.
Additional literals exercise TOP=7 wraparound, Normal-to-Zero and Zero-to-
Normal raw tag transfer, invalid source under PF=0/1, invalid destination,
and the C1, ES, B and FCW gates. The historical repeat and both TEST producer
fixtures remain. Testkit literal interpretation compares complete state and
memory; the CLI runner separately checks JIT denial. These are local evidence. Unordered/NaN values, x87
exception delivery, native execution and physical x86 behavior remain outside
this bounded profile.

## Bounded REP STOSW/D/Q fixtures S2-19b

Twenty-eight literal schema-v2 fixtures cover the three new `F3 AB` widths: for
each width, zero count with a noncanonical RDI, DF=0/1 count-three success, an
unmapped first store, a missing mapping after one committed store, noncanonical
first destination, zero instruction budget, and the 1024-store limit with
count 1025. Two further fixtures exercise `F3 48 66 AB` word and `F3 66 48 AB`
qword normalization; two `TEST`→`REP STOSD` programs cover completion and a
partial second-store backend failure. Full GPR, RFLAGS/definedness, RIP, and
complete memory answers are serialized in the expected sidecar. The fixture
generator serializes predetermined constants and does not compute CPU results.
The profile/schema are unchanged. Native/JIT remain fail-closed; no physical
execution is claimed.

The interpreter checks include 12,288 full completion states, 96 injected
write failures, 16 STOP/replay cases, 24 work-limit boundaries, STOP at 1024,
`RCX=u64::MAX` bounded progress, prefix rejection, memory edges, and malformed
fetch/budget behavior. The fixture pool and all physical/native checks remain
reserved for the shared final acceptance.

```sh
python3 tests/cpu/generate_stage2_rep_stos_wdq.py --check
cargo test -p od-testkit --offline rep_stos_wdq
cargo test -p od-runner --test fixture_v2 --offline rep_stos_wdq
```

## Bounded REP STOS address-size-32 fixtures S2-19k

Eleven literal schema-v2 fixtures cover `F3 67 AA` and the word/dword/qword
`F3 67 AB` forms: successful cases for each width, a word DF=1 case,
qword DF=1, qword EDI wrap from `0xfffffffc`, zero ECX with noncanonical
high-half RCX/RDI, a byte store failure after one completed iteration,
zero instruction budget, initial STOP, and the two accepted word/qword
normalization cases. The full-state sidecar records GPR, RFLAGS/definedness,
RIP and complete data memory, including partial ECX/EDI progress on failure.
The generator serializes fixed hand-authored inputs and expected values only;
it does not execute or derive instruction results. The profile and schema are
unchanged. Native/JIT and physical acceptance remain deferred.

```sh
python3 tests/cpu/generate_stage2_rep_stos_addr32.py --check
cargo test -p od-testkit --offline rep_stos_address32_literal_fixtures_match_full_expected_answers
```

## Bounded REP LODSB/W/D/Q fixtures S2-19c

Thirty-six literal schema-v2 fixtures cover all four `F3 AC/AD` widths: eight
per width for zero count with invalid RSI, DF=0/1 count-three completion,
unmapped first read, missing mapping after one committed read, noncanonical
first RSI, zero instruction budget and count-1025 work-limit exhaustion. Two
more fixtures exercise `F3 48 66 AD` word and `F3 66 48 AD` qword
normalization. Two `TEST`→`REP LODSD` programs cover completion and a partial
second-read harness error. The sidecar records complete GPR, flags and
in-run definedness, RIP and unchanged memory, including partial RAX/RSI/RCX
and retirement state. The generator only serializes predetermined literals;
profile and fixture schema remain unchanged.

The interpreter checks include 12,288 complete machine states, 96 injected
read errors, 16 STOP/replay cases plus initial STOP and STOP at the limit boundary, 24 work-limit boundaries,
`RCX=u64::MAX`, no-access zero count, zero instruction budget, page/alignment/
short-range/wrap/noncanonical-next-pointer edges and TEST definedness. Native/JIT
remain fail-closed and no native or physical execution is claimed. ISSUE-016
and ISSUE-020 remain open.

```sh
python3 tests/cpu/generate_stage2_rep_lods.py --check
cargo test -p od-testkit --offline rep_lods_
cargo test -p od-runner --test fixture_v2 --offline rep_lods
```

## Bounded REP LODS address-size-32 fixtures S2-19j

Nine literal schema-v2 cases cover address-size-32 REP LODS: byte/word/dword/
qword successes, a qword read at `ESI=0xfffffffc` whose post-read pointer wraps
to 4, zero ECX with high-half RCX and a noncanonical RSI, a zero instruction
budget, an initial STOP, and a qword read failure after one completed element.
The full-state sidecar records ECX decrement/zero-extension into RCX, preserved
high-half RCX when ECX is initially zero, zero-extended ESI, RAX progress, RIP,
definedness, and unchanged data memory. The generator
serializes hand-authored inputs and expected values; it does not execute or
derive instruction results. The fixtures use the existing profile and schema.

```sh
python3 tests/cpu/generate_stage2_rep_lods_addr32.py --check
cargo test -p od-testkit --offline rep_lods_address32
```

## Bounded REP MOVSB/W/D/Q fixtures S2-19d

Fifty-six literal schema-v2 fixtures cover exact REP MOVSB/W/D/Q admission and
progress: 44 base cases (11 per width), eight overlapping source/destination
cases, two 66/REX normalization cases, and two TEST→REP compositions. Per width,
the base rows cover zero count with invalid RSI/RDI, DF=0/1 completion, source
and destination failures before and after one transfer, noncanonical source
and destination, zero instruction budget, and the 1025-count local work limit.
Overlap snapshots use a single region and record sequential source-first
transfer results. The TEST composition checks successful retirement and a
destination error after the first completed transfer. Expected sidecars retain
complete register, flags/definedness, RIP and memory state; the generator only
serializes predetermined values.

The interpreter suite also checks 12,288 full machine states with exact
alternating access logs, 192 injected read/write failures, 16 STOP/replay
cases, 24 limit boundaries, 36 overlap cases and 48 prefix rejects. JIT is
denied for all 56 fixtures without fallback. Native/physical execution is
deferred; ISSUE-021 tracks the unresolved hardware winner for dual-invalid
source/destination addresses. ISSUE-016 and ISSUE-020 remain open.

```sh
python3 tests/cpu/generate_stage2_rep_movs.py --check
cargo test -p od-testkit --offline rep_movs
cargo test -p od-runner --test fixture_v2 --offline rep_movs_cli_fixtures_check_full_state_and_jit_denial
```

## Bounded REP MOVSB/W/D/Q address-size-32 fixtures S2-19l

Twelve literal schema-v2 cases cover all four address-size-32 widths, DF=1,
zero-extended ESI/EDI high-half aliases, ECX=0 preserving full RCX/RSI/RDI,
qword pointer wrap, source and destination faults after one committed element,
zero instruction budget, initial STOP, and normalized 66/REX order. Expected
sidecars include all GPRs, flags and definedness, RIP, and every memory region.
The generator serializes fixed row values and never derives expected results
from instruction semantics. Fixture schema/profile are unchanged.

The interpreter checks 12,288 address-size-32 full states with exact alternating
source-read/destination-write traces; combined with S2-19d this is 24,576 state
cases. Separate coverage includes 192 injected address-size-32 read/write
faults with retry, 36 overlapping transfers whose source observes prior writes,
ECX values 0/1/1023/1024/1025/`0xffffffff`, partial-progress limit/STOP tracked
resume, TEST-produced undefined AF preservation, and malformed-prefix rejects.
The runner checks every full fixture snapshot and explicitly denies JIT; the
native runner test rejects all twelve before starting a tracee on its supported
Linux x86-64 test target. ISSUE-021 remains open for unverified hardware
dual-invalid priority.

```sh
python3 tests/cpu/generate_stage2_rep_movs_addr32.py --check
python3 tests/cpu/test_stage2_rep_movs_addr32_generator.py
cargo test -p od-testkit --offline rep_movs_addr32
cargo test -p od-runner --test fixture_v2 --offline rep_movs_addr32
```

## Bounded REPE SCASB address-size-32 fixtures S2-19m

Thirteen literal schema-v2 fixtures cover both exact three-byte encodings,
`F3 67 AE` and `67 F3 AE`; zero ECX with nonzero RCX high halves and invalid
EDI; nonzero ECX/EDI high-half aliases; mismatch under DF; an unmapped first
read and a fault after one committed equal read; the 1024-success limit; zero
instruction budget; CMP→REP success; TEST→REP undefined-AF rejection; bad
entry flags at zero count; and extra-prefix/addr32 `A7` rejects. The expected sidecar
records complete GPR, RFLAGS, definedness, RIP and memory state. The generator
serializes a static input/expected table only. Fixture schema/profile are
unchanged.

Interpreter coverage includes all 131,072 AL×memory-byte×DF combinations
against an independent CMP model, gate rejection for all invalid value and
definedness combinations at ECX zero and nonzero, partial-fault atomicity,
address-size-32 count/index semantics, and completion→STOP→work-limit ordering.
The CLI compares full snapshots and confirms JIT remains denied.

```sh
python3 tests/cpu/generate_stage2_rep_scasb_addr32.py --check
python3 tests/cpu/test_stage2_rep_scasb_addr32_generator.py
cargo test -p od-testkit --offline rep_scasb_addr32_literal_fixtures_match_full_expected_answers
cargo test -p od-runner --test fixture_v2 --offline rep_scasb_addr32_cli_fixtures_check_full_state_and_jit_denial
```

## Bounded REPE SCASW/D/Q address-size-32 fixtures S2-19o

Fourteen literal schema-v2 fixtures extend the addr32 REPE SCAS profile to
`F3 67 66 AF` SCASW, `F3 67 AF` SCASD, and effective-REX.W SCASQ. Cases cover
zero count and preserved high halves, mismatch under DF, completion, an
unmapped first access and fault after one successful element, the 1024-read
limit, zero instruction budget, a width-sized read crossing 4 GiB, CMP→SCAS
success, TEST→SCAS undefined-AF rejection, a bad zero-count gate, extra-prefix
rejection, and duplicate-66 CMPSD rejection (addr32 CMPSD is added by S2-19p).
Snapshots include all GPRs,
RFLAGS/definedness, RIP, and complete fixture memory. The generator serializes
hand-authored input/answer constants; it does not compute instruction results.

The CLI compares full interpreter snapshots and confirms JIT remains denied
without fallback. No native or physical x86 execution is claimed; ISSUE-020
remains open.

```sh
python3 tests/cpu/generate_stage2_rep_scas_wdq_addr32.py --check
python3 tests/cpu/test_stage2_rep_scas_wdq_addr32_generator.py
cargo test -p od-testkit --offline rep_scas_wdq_addr32_literal_fixtures_match_full_expected_answers
cargo test -p od-runner --test fixture_v2 rep_scas_wdq_addr32_cli_fixtures_check_full_state_and_jit_denial
```

## Bounded REPE CMPSB address-size-32 fixtures S2-19n

Thirteen literal schema-v2 fixtures cover both exact encodings (`F3 67 A6`,
`67 F3 A6`), zero-count high-half preservation, high-half address zero-extension,
equal completion, mismatch at comparison two, source and destination unmapped reads,
the 1024-success limit, zero instruction budget, CMP→REP success,
TEST→REP undefined-AF rejection, bad entry values at ECX=0, and duplicate-66
CMPSD rejection (S2-19p adds the bounded wide forms). Expected sidecars contain
full GPR/RFLAGS/definedness/RIP/memory
snapshots. The generator serializes a fixed input and expected-state table; it
does not calculate or execute instruction semantics.

The independent interpreter model covers all 256×256 source/destination byte
pairs for both DF values on both addr32 spellings, plus the retained address64
form. Focused cases check ECX and index wrap/truncation, source-first dual-invalid
trace, second-read no-commit and replay with modified source data, STOP,
zero-budget, 1024 limit, and tracked continuation. ISSUE-020 remains open; the
local first-read result does not establish hardware dual-fault priority or MMIO
rollback. Runner CLI snapshots pass and JIT stays denied. No native, physical
x86, Mac mini, or SSH execution is claimed.

```sh
python3 tests/cpu/generate_stage2_rep_cmpsb_addr32.py --check
python3 tests/cpu/test_stage2_rep_cmpsb_addr32_generator.py
cargo test -p od-testkit --offline rep_cmpsb_addr32_literal_fixtures_match_full_expected_answers
cargo test -p od-runner --test fixture_v2 --offline rep_cmpsb_addr32_cli_fixtures_check_full_state_and_jit_denial
```

## Bounded REPE SCASB fixtures S2-19e

Exactly 24 literal schema-v2 fixtures cover exact `F3 AE` progress: zero count
for DF=0/1; count-one equality and mismatch for each DF; mismatch positions
1/2/3 for each DF; all-equal count-three for each DF; unmapped read after 0/1/2
equal elements; noncanonical first access; count 1025 for both DF values;
zero instruction budget; CMP RAX,RAX→REP success/fault after one equal read;
and TEST RAX,RAX→REP rejection. The TEST case has concrete PF|ZF but AF is
undefined, so it fails the local admission-definedness gate before data access.
The generator serializes only the hand-authored bytes and literal complete
snapshots; it does not calculate CPU results or invoke an execution engine.

The interpreter matrix separately covers 131,072 AL×memory×DF comparisons
against an independent CMP model, 160 count/mismatch termination paths, 16,380
arithmetic value/definedness/count/DF admission rejects, 24 injected memory
failures, STOP/replay, limit boundaries, mismatch at iteration 1024, and
termination priority. This entry gate is a local profile restriction and does
not claim that x86 REPE SCAS requires PF|ZF at instruction entry. JIT/native
execution remains denied. ISSUE-016/019/020/021 remain open; physical acceptance
is deferred to the shared final gate.

```sh
python3 tests/cpu/generate_stage2_rep_scasb.py --check
cargo test -p od-isa --offline rep_scasb
cargo test -p od-testkit --offline rep_scasb_literal_fixtures_match_full_expected_answers
```

## Bounded REPE CMPSB fixtures S2-19f

Exactly 31 schema-v2 fixtures record literal full-state outcomes for exact
`F3 A6`: zero count with DF=0/1 and invalid pointers; count-one equal/mismatch
for both directions; mismatch at positions 1/2/3 and all-equal count three for
both directions; unmapped source and destination after 0/1/2 equal iterations;
noncanonical first source/destination; count 1025 for both directions;
instruction budget zero; CMP RAX,RAX composition success and first/second
read failure; TEST RAX,RAX rejection with concrete PF|ZF but undefined AF; and
same/overlap aliases. The generator serializes only handwritten bytes and
predetermined full register, flags/definedness, RIP, and memory snapshots.

Interpreter coverage independently checks 131,072 source×destination×DF flag
results, 160 termination paths, all 16,380 entry-gate rejection combinations,
48 injected failures, access ordering, no current-iteration commit, aliases,
STOP/replay, work-limit boundaries, RCX maximum and pointer boundaries. The
RSI-first ordering is local only and does not identify hardware dual-fault
priority; MMIO rollback is not claimed. JIT fixtures are required to remain
Unsupported with no interpreter fallback. No native, Mini, SSH, or physical
execution is claimed; ISSUE-020 remains open.

```sh
python3 tests/cpu/generate_stage2_rep_cmpsb.py --check
cargo test -p od-testkit --offline rep_cmpsb_literal_fixtures_match_full_expected_answers
cargo test -p od-runner --test fixture_v2 rep_cmpsb_cli_fixtures_check_full_state_and_jit_denial
```

## Bounded REPE SCASW/D/Q fixtures S2-19g

Exactly 59 literal schema-v2 fixtures extend bounded `F3 AE` SCASB with
`F3 66 AF` SCASW, `F3 AF` SCASD, and `F3 effective-REX.W AF` SCASQ. Each
width has 16 cases: zero count, count-one equal/mismatch, middle mismatch and
all-equal count three for both DF values; first-read and after-one mapping
errors; noncanonical first read; budget zero; and count 1025 for either DF.
Two additional cases cover the `F3 48 66 AF` word and `F3 66 48 AF` qword
prefix orders. Nine compositions cover CMP success/fault-after-one and TEST
rejection with undefined AF for each width. Full expected snapshots include
all GPRs, flags and definedness, RIP, and memory.

The generator serializes fixed fixture inputs and expected snapshots; it does
not invoke an assembler, oracle, or execution engine. All 59 fixtures assert
that JIT remains Unsupported with no interpreter fallback, and native
admission has an explicit rejection test for each width and both normalized
prefix orders. Schema/profile remain unchanged. No native execution, Mini,
SSH, or physical acceptance is claimed.

```sh
python3 tests/cpu/generate_stage2_rep_scas_wdq.py --check
cargo test -p od-testkit --offline rep_scas_wdq_literal_fixtures_match_full_expected_answers
cargo test -p od-runner --test fixture_v2 rep_scas_wdq_cli_fixtures_check_full_state_and_jit_denial
```

## Bounded REPE CMPSW/D/Q fixtures S2-19h

Exactly 71 literal schema-v2 fixtures extend S2-19f `F3 A6` CMPSB with
`F3 66 A7` CMPSW, `F3 A7` CMPSD, and effective-REX.W `F3 A7` CMPSQ. Each
width has 19 cases: zero count for both DF values; one equal and mismatch
comparison for both DF values; middle mismatch and all-equal count three for
both DF values; source/destination unmapped at the first read and after one
complete equal element; noncanonical RSI and RDI; count 1025 for both DF
values; and zero instruction budget. Two additional cases check
`F3 48 66 A7` as word and `F3 66 48 A7` as qword. Twelve compositions check
CMP RAX,RAX followed by successful REPE or each ordered read fault, plus TEST
RAX,RAX rejection for each width with undefined AF. Snapshots cover all GPRs,
flags/definedness, RIP, and fixture memory.

The generator serializes fixed bytes and expected state only; it does not use
an assembler, oracle, or execution engine. The runner fixture test compares
complete interpreter state and checks explicit JIT Unsupported/no-fallback for
all 71 cases. A Linux-only native-admission test requires all 71 fixtures to
remain denied; it was not executed on this macOS host. No native execution,
Mini, SSH, or physical acceptance is claimed. Existing CMPSB fixtures and the
schema/profile remain unchanged.

```sh
python3 tests/cpu/generate_stage2_rep_cmps_wdq.py --check
python3 -m unittest tests/cpu/test_stage2_rep_cmps_wdq_generator.py
cargo test -p od-runner --test fixture_v2 rep_cmps_wdq_cli_fixtures_check_full_state_and_jit_denial --offline
```

## Non-REP address-size-32 LODS B/W/D/Q fixtures S2-18k

Thirty-nine literal schema-v2 cases cover the four non-REP LODS widths with
nonzero upper RSI halves, unmapped source errors, zero instruction budget,
32-bit RSI increment/decrement wrap, contiguous reads crossing the 4-GiB
boundary, two prefix-normalization cases, TEST composition with success and
unmapped read, and CLD/STD composition for every width. The upper half of RSI
is discarded before forming the starting address; reads themselves remain
contiguous and do not wrap individual bytes at 4 GiB. Successful pointer
updates are zero-extended 32-bit results. Expected snapshots include all GPRs,
flags and their definedness, RIP, and fixture memory.

The generator serializes fixed bytes, state, memory, and expected snapshots;
it does not invoke an assembler, interpreter, or external oracle. The runner
CLI test compares every complete interpreter snapshot and checks that JIT
execution is denied without fallback. These are local interpreter fixtures;
native, Mac mini, SSH, and physical acceptance remain part of the shared final
acceptance stage.

```sh
python3 tests/cpu/generate_stage2_lods_addr32.py --check
python3 -m unittest tests/cpu/test_stage2_lods_addr32_generator.py
cargo test -p od-runner --test fixture_v2 addr32_lods_fixtures_match_literal_full_state_answers_and_fail_closed --offline
```

### S2-18l non-REP STOS B/W/D/Q, address-size 32

The 43 literal schema-v2 fixtures cover the four `67` STOS widths, both DF
directions, write failures, zero budget, 32-bit RDI wrap, continuous stores
across the 4-GiB boundary, prefix normalization, TEST composition and CLD/STD
composition. Each expected final RDI and each complete expected data byte
string is fixed fixture data; the generator only serializes those constants.
The start address is zero-extended EDI, while the successful store itself is
contiguous and does not wrap byte-by-byte at 4 GiB. The full RAX source is
preserved. Error cases encode the local ISSUE-016 no-commit policy, not a
hardware post-fault guarantee.

```sh
python3 tests/cpu/generate_stage2_stos_addr32.py --check
python3 -m unittest tests.cpu.test_stage2_stos_addr32_generator
cargo test -p od-runner --test fixture_v2 stos_addr32_cli_fixtures_match_literal_full_state_and_keep_jit_closed --offline
```

### S2-18m non-REP SCAS B/W/D/Q, address-size 32

These 39 literal schema-v2 fixtures cover the four non-REP `67` SCAS widths,
both DF directions, unmapped reads, zero budget, 32-bit RDI wrap, continuous
reads across 4 GiB, prefix normalization, TEST and CLD/STD compositions. Each
expected RDI, arithmetic flags/definedness, memory image, RIP, and retirement
count is literal input; the generator serializes rows and hashes fixtures but
does not use the interpreter as an oracle. The initial address is
zero-extended EDI; the complete SCAS operand remains a contiguous read.

The runner compares complete CPU/RAM snapshots, exact read HarnessError
details, retirement, and JIT fail-closed behavior. A Linux-only native
admission test rejects these forms before tracee execution. Neither that test
on this macOS host nor a physical oracle run is claimed here.

```sh
python3 tests/cpu/generate_stage2_scas_addr32.py --check
python3 -m unittest tests.cpu.test_stage2_scas_addr32_generator
cargo test -p od-runner --test fixture_v2 scas_addr32_cli_fixtures_match_literal_full_state_and_keep_jit_closed --offline
```

### S2-18o non-REP MOVS with address-size 32

Exactly 73 literal schema-v2 fixtures cover non-REP `67 A4` MOVSB,
`67 66 A5` MOVSW, `67 A5` MOVSD, and `67 48 A5` MOVSQ. The category split is
8 successes, 8 unmapped source/destination errors, 4 read-only destination
errors, 4 zero-budget cases, 16 pointer wraps, 6 continuous cross-4-GiB spans,
2 prefix normalizations, 3 TEST compositions, 8 CLD/STD compositions,
4 aliases, 4 dual-invalid cases, and 6 partial overlaps. One success uses a
read-only source with a write-only destination.

Full register, RFLAGS/definedness, RIP, memory, retirement, and local
HarnessError snapshots are in `s2_18o_movs_addr32_expected.json`; the committed
`s2_18o_movs_addr32_manifest.json` is authoritative for hashes and exact file
inventory. `generate_stage2_movs_addr32.py` serializes fixed input constants
and its literal `EXPECTED` table; it does not compute expected transitions or
run the interpreter to produce answers. The source-first dual-invalid cases
record local policy only and establish neither hardware exception priority nor
MMIO rollback. Native/JIT and physical acceptance remain deferred to the
shared final acceptance substage.

```sh
python3 tests/cpu/generate_stage2_movs_addr32.py --check
python3 -m unittest tests.cpu.test_stage2_movs_addr32_generator
cargo test -p od-runner --test fixture_v2 movs_addr32_cli_fixtures_match_literal_snapshots_and_keep_jit_closed --offline
cargo test -p od-runner --test fixture_v2 movs_addr32_rejects_73_prefix_variants_as_unsupported_end_to_end --offline
```

## Bounded REPE CMPSW/D/Q address-size-32 fixtures S2-19p

Fourteen literal schema-v2 fixtures cover zero-count high-half preservation,
legacy prefix order, a dword mismatch with DF=1, a Qword access beginning at
`0xffffffff`, source and destination mapping failures, zero instruction budget,
the 1024-success limit, CMP→REP success, TEST→REP undefined-AF rejection,
bad zero-count gate, duplicate address prefix, and both REX/66 width-order
cases. Full snapshots include every GPR, RFLAGS, definedness, RIP and memory
region. The generator serializes authored constants only.

Executor tests independently compare W/D/Q results across operand values and
both DF values, exercise all normalized addr32 widths, zero-extension,
modulo-32 pointer wrap, separate source/destination accesses, destination
failure retry with changed source data, dual-invalid source-first trace, and
the full 4-GiB-crossing width read. Decoder and executor reject the same
negative prefix matrix. Source-first/dual-invalid behavior remains local
ISSUE-020 policy and does not establish hardware priority or MMIO rollback.
The CLI checks full snapshots and JIT denial. Native/JIT execution, physical
x86, Mac mini and SSH acceptance were not performed.

```sh
python3 tests/cpu/generate_stage2_rep_cmps_wdq_addr32.py --check
python3 -m unittest tests.cpu.test_stage2_rep_cmps_wdq_addr32_generator
cargo test -p od-testkit --offline rep_cmps_wdq_addr32_literal_fixtures_match_full_expected_answers
cargo test -p od-runner --test fixture_v2 --offline rep_cmps_wdq_addr32_cli_fixtures_check_full_state_and_jit_denial
```


### S2-18j bare CLD/STD

Bare `FC` and `FD` are covered in the local interpreter profile with DF as their
only defined flag effect. Prefix variants are rejected as profile admission
failures. `D9 FC` continues to identify the x87 FRNDINT encoding. The 32 local cases
cover initial DF, zero budget, TEST-definedness, five non-REP consumers, three
REP consumers, CMP→REPE SCAS/CMPS, and unmapped consumer
faults. Literal expected CPU/definedness/memory answers are in
`s2_18j_cld_std_expected.json`; the generator serializes constants only. JIT
is checked fail-closed by the CLI test. Native admission has a dedicated
test but is not executed on this macOS host; no physical acceptance is claimed.

```sh
python3 tests/cpu/generate_stage2_cld_std.py --check
python3 -m unittest tests/cpu/test_stage2_cld_std_generator.py
cargo test -p od-runner --test fixture_v2 cld_std_cli_fixtures_match_literal_full_state_and_jit_denial --offline
```

### S2-18n non-REP CMPS with address-size 32

Exactly 63 literal schema-v2 fixtures cover `67 A6` CMPSB, `67 66 A7` CMPSW,
`67 A7` CMPSD, and `67 48 A7` CMPSQ: eight successes, eight single-operand
unmapped outcomes, four zero-budget cases, sixteen ESI/EDI wrap cases, six
continuous reads across 4 GiB, two prefix normalizations, three TEST
compositions, eight CLD/STD compositions, four aliases, and four dual-unmapped
cases. Full GPR, flags/definedness, RIP, memory and retirement expectations are
recorded in `s2_18n_cmps_addr32_expected.json`; the fixture SHA-256 list is in
`s2_18n_cmps_addr32_manifest.json`, with the explicit file inventory at
`work/s2-18n-files.json`.

The two ordering cases are `48 67 A7` (earlier REX.W is discarded, CMPSD) and
`67 66 48 A7` (effective REX.W, CMPSQ). The dual-invalid local reproduction is
`s2_18n_dual_unmapped_b.case.json`; `_w`, `_d`, and `_q` cover the remaining
widths.

`generate_stage2_cmps_addr32.py` serializes handwritten expected-state inputs;
it does not run the interpreter to produce expected results. Operands are read
as contiguous width-sized spans after zero-extending ESI/EDI. Dual-unmapped
fixtures verify only the bounded local ESI-first error outcome. They do not
establish physical exception priority or MMIO rollback. JIT/native, Mac mini
and physical x86 acceptance remain deferred; ISSUE-020 is open.

### Bounded REPNE SCASB address-size-32 fixtures S2-19q

Schema-v2 one-shot fixtures use the explicit continuation API with a fresh
`None` token. They cover zero-count arbitrary flag values and successful byte
comparisons using literal full-state expected answers. The token is intentionally
not serialized by schema v2: callers retain it in memory between calls to
resume a partial REP instruction. JIT/native remain denied; no physical, Mac
mini or SSH execution is claimed.

### Bounded REPNE CMPSB address-size-32 fixtures S2-19r

Two schema-v2 one-shot cases cover zero count with arbitrary concrete flags
and mismatch followed by equality, with literal full-state answers for both
normalized prefix orders. The generator only serializes authored input and
expected values. Each fixture begins with a fresh `None` continuation token;
the token is not part of schema v2. Testkit separately carries it in memory
through a STOP/resume sidecar. Runner CLI checks the complete snapshots and
keeps JIT denied. ISSUE-020 remains open for the local ESI-first dual-fault
policy; no hardware priority, MMIO rollback, native, physical, Mac mini or SSH
claim is made.


### Bounded REPNE SCASW/D/Q address-size-32 fixtures S2-19s

Three schema-v2 snapshots cover W equality, D mismatch and Q zero count for
the bounded Long64 F2+67 AF profile. The generator serializes handwritten
bytes and complete expected states only. One-shot fixtures start with a fresh
`None` continuation; callers retain tokens outside schema v2 when execution
needs a resume. Runner/native/JIT support is not implied by these interpreter
fixtures.

### Bounded REPNE CMPSW/D/Q address-size-32 fixtures S2-19t

Three schema-v2 snapshots cover W equality, D mismatch and Q zero count. Their
bytes and complete expected states are handwritten; the generator only
serializes them. Each fixture starts a fresh explicit continuation, which is
not serialized in schema v2. A separate testkit sidecar carries a caller-owned
token through STOP and multiple resumes. JIT/native admission remains closed;
dual-invalid source-first behavior remains local ISSUE-020 policy.

### S2-34.1 local x87/SSE raw-state snapshot

`s2_34_1_x87_sse_raw_state_snapshot.case.json` is a handwritten literal with
exactly one `90` NOP, budget 1 and stop RIP `0x100001`. It includes the existing
x87 snapshot format plus all 16 raw 16-byte XMM registers and raw MXCSR. The
fixture diagnostic metadata fixes `diagnostic_mxcsr_mask` to `0x0000ffbf` for
this bounded profile (default without optional DAZ); it is neither a universal
hardware CPUID mask nor guest `#GP` semantics. The local interpreter result
preserves both state objects. This is snapshot/state preservation only, not SSE
execution or physical verification; native/JIT admission remains closed.

### S2-34.2 local register MOVUPS

`s2_34_2_movups_register.case.json` is a handwritten literal using the separate
`s2-long-movups-reg-v1` profile. It executes `45 0F 10 C1` (`MOVUPS XMM8,XMM9`)
once with budget 1 and stop RIP immediately afterward. The expected result
copies all 16 raw bytes from XMM9 to XMM8 and preserves x87, MXCSR, GPR and
RFLAGS. Decoder tests exercise every XMM source/destination pair, bare and
REX.R/B encodings, and fail-closed prefix, memory-ModR/M and truncation cases.
The profile assumes SSE available, CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1; these
are explicit assumptions, not modeled control state. It excludes legacy
prefixes, REX.W/X, duplicate prefixes, `0F 11`, SSE2 alternatives, VEX/EVEX,
memory/alignment semantics, SSE faults, native/JIT and physical verification.
The S2-34.1 state-only profile remains NOP-only; S2-34/D/System remain open.

### S2-34.3 local reverse-direction register MOVUPS

`s2_34_3_movups_store_register.case.json` is a handwritten literal using the
existing `s2-long-movups-reg-v1` profile. It executes `0F 11 C1` once, copying
all 16 raw bytes from XMM0 to XMM1, with budget 1 and stop RIP immediately
afterward. The expected full snapshot preserves all x87 state, every other XMM
register, MXCSR, GPR and RFLAGS. Runner coverage also checks `0F 11 C9`
self-copy. Decoder tests cover all 256 source/destination pairs for both MOVUPS
directions and REX.R/B, including `45 0F 11 C1` (XMM8→XMM9); executor tests
assert the register form performs no data-memory reads or writes. Native
admission rejects the fixture. The profile keeps its explicit SSE, CR0.EM=0,
CR0.TS=0 and CR4.OSFXSR=1 preconditions. This local slice does not close
S2-34/D/System, and S2-34.1 remains NOP-only.

### S2-34.4 local MOVUPS memory load from `[RAX]`

`s2_34_4_movups_memory_load_rax.case.json` is a handwritten literal in its
separate `s2-long-movups-mem-load-v1` profile. It executes `44 0F 10 00`
(`MOVUPS XMM8,[RAX]`) once. RAX points one byte into a read-only data region,
so the load is deliberately unaligned; the 16 loaded bytes are `00` through
`0f`, surrounded by canaries. The expected snapshot updates only XMM8 and RIP;
all x87 state, the other XMM registers, MXCSR, GPR, RFLAGS and source memory
remain unchanged.

The separate profile does not change the NOP-only S2-34.1 profile or
register-only S2-34.2/.3 profile. Decoder/schema checks accept bare
`0F 10 /r` for low XMM destinations and effective REX.R for high destinations,
only with ModR/M.mod=00, r/m=000 (`[RAX]`). They reject REX.B/X/W, SIB,
displacement, other bases, `0F 11`, legacy prefixes, duplicate prefixes,
VEX and EVEX. Executor checks cover all 16 destinations, one contiguous
16-byte read, raw byte order, unaligned access, canonical/range boundaries,
memory canaries, injected fetch/read failures and no architectural commit on
failure. The native runner explicitly denies this profile. This is local
diagnostic evidence for one x86-64 form; it does not validate a full Generic
amd64 CPU profile or claim physical execution. SSE/CR0/CR4 prerequisites remain
profile assumptions, not implemented gates.

### S2-34.5 local MOVUPS memory store to `[RAX]`

`s2_34_5_movups_memory_store_rax.case.json` is a handwritten full-state literal
in its separate `s2-long-movups-mem-store-v1` profile. It executes
`44 0F 11 00` (`MOVUPS [RAX],XMM8`) once, with unaligned RAX one byte into a
writable data region. The expected state preserves all XMM/x87/MXCSR/GPR/RFLAGS
values and advances only RIP; memory changes only in the exact 16-byte target
range, preserving surrounding canaries.

Decoder and schema tests admit only the `[RAX]` legacy memory-destination form,
with optional REX.R source extension. Executor tests cover all 16 source byte
patterns, one exact 16-byte write and no read, canonical/overflow boundaries,
read-only and unmapped ranges, injected backend write failure, no partial
mutation, and no CPU commit on errors. Literal runner coverage checks the full
state and memory snapshot, stop/budget behavior, and JIT/native rejection. The
FlatMemory all-or-error behavior is limited to private synchronous fixture RAM;
it makes no hardware atomicity, MMIO, or SMP claim. ISSUE-012's upper-u64
boundary restriction remains open. The profile's SSE and CR0/CR4 assumptions
are not implemented control gates, and the chosen x86-64-v1 baseline is only an
ABI floor rather than a complete generic CPU.

### S2-34.6a local MOVUPS base+displacement memory forms

`s2_34_6_movups_memory_base.case.json` is a handwritten schema-v2 literal in
the new `s2-long-movups-mem-base-v1` profile. It executes
`45 0F 11 40 F9` (`MOVUPS [R8-7],XMM8`), writes the 16 raw source bytes to the
unaligned target at `0x200001`, preserves surrounding canaries and advances
only RIP. The profile supports both `0F 10` load and `0F 11` store with
ModR/M modes 00/01/10 and base plus absent/signed disp8/signed disp32. REX.R
extends XMM and REX.B extends the GPR base. Existing exact `[RAX]` and register
profiles keep their prior shapes. SIB, RIP-relative, REX.W/X, address-size or
segment overrides, other legacy prefixes, VEX/EVEX remain rejected. Executor
tests cover both directions and all XMM registers, displacement extrema, exact
single callback, full-state no-commit on backend error, canonical-range faults,
STOP/budget and forbidden/truncated encodings. SIB is excluded, so RSP and R12
cannot be encoded as bases; shared `MemoryOperand` keeps the preexisting SS/#SS
attribution for RBP and data/#GP attribution for R13. FlatMemory's fixture-RAM
all-or-error rule does not claim hardware store atomicity. ISSUE-012 remains.
Native/JIT and physical execution are not established.

### S2-34.6b local MOVUPS RIP-relative memory forms

`s2_34_6b_movups_rip_load.case.json` and
`s2_34_6b_movups_rip_store.case.json` are handwritten literals in the existing
`s2-long-movups-mem-base-v1` profile. Each uses `44 0F 10/11 05 F9 FF 0F 00`:
REX.R selects XMM8, the seven-byte base instruction plus REX makes an
eight-byte instruction, and signed disp32 `+0x000ffff9` resolves from next RIP
`0x100008` to the unaligned data address `0x200001`. Load copies the 16 data
bytes to XMM8; store copies XMM8 to the data range. Both fixtures retain
canaries, full-state observations and the established one-instruction stop.
Executor tests additionally distinguish next RIP from current RIP and signed
from unsigned displacement, and cover zero/negative displacement, all disp32
fetch truncation boundaries, canonical crossing and unsupported REX.B/SIB/
prefix forms. The profile rejects REX.B for r/m=101; Intel SDM 093 Vol. 2A
§2.2.1.6 specifies that it remains RIP-relative and cannot select R13.

### S2-34.6c local MOVUPS SIB-addressed memory forms

`s2_34_6c_movups_sib_load.case.json` is a handwritten literal for
`44 0F 10 44 4C F9` (`MOVUPS XMM8,[RSP+RCX*2-7]`), which loads one unaligned
16-byte range using a stack-class base and scaled index. The store fixture
`s2_34_6c_movups_sib_store.case.json` uses
`47 0F 11 04 E5 01 00 20 00` (`MOVUPS [R12*8+0x200001],XMM8`): REX.X selects
R12 from SIB index=4, while `mod=00/base=5` has no base regardless of REX.B.
Both cases retain canaries, full-state observations and one-instruction stops.

The same profile uses the shared `rm_operand`/`MemoryOperand` path for scale
1/2/4/8, absent/normal/R12 indexes, RSP/RBP/R12/R13 bases and no-base
displacement forms. Intel SDM revision 093 Vol. 2A §§2.1.3–2.1.5,
2.2.1.1–2.2.1.3 defines SIB/REX/displacements; Vol. 1 §§3.3.7.1, 3.7.4.1,
3.7.5.1 defines #SS/#GP and segment defaults; Vol. 2B defines MOVUPS:
[official SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html),
[Vol. 2A PDF](https://cdrdv2.intel.com/v1/dl/getContent/671199),
[Vol. 2B PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf),
[Vol. 1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436). Execution tests
cover both directions, all XMMs, one exact callback, canaries, errors without
commit, canonical attribution, truncation, prefix rejection and STOP/budget.
The fixture runner confirms interpreter behavior and native/JIT rejection.
S2-34 remains open; x86-64-v1 is only the ABI floor.

### S2-34.7a local register XORPS

`s2_34_7a_xorps_register.case.json` is a handwritten literal fixture for the
separate `s2-long-xorps-reg-v1` diagnostic profile. It executes
`45 0F 57 C1` (`XORPS XMM8,XMM9`) once, XORing all 128 raw bits while preserving
MXCSR, x87, GPR, flags and unrelated XMM registers. This profile does not
extend any MOVUPS profile. SSE availability and CR0/CR4 prerequisites are
assumed; memory forms, VEX/EVEX, exception delivery and native/JIT support are
outside this local slice. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-profile-s2-347a-s2-long-xorps-reg-v1)
and [Stage 2 plan](../../../../docs/plans/STAGE_2.md#s2-347a--local-legacy-xorps-register-to-register).

### S2-34.7b local register ANDPS

`s2_34_7b_andps_register.case.json` is a handwritten literal fixture for the
separate `s2-long-andps-reg-v1` diagnostic profile. It executes
`45 0F 54 C1` (`ANDPS XMM8,XMM9`) once, ANDing all 128 raw bits while
preserving MXCSR, x87, GPR, flags and unrelated XMM registers. Intel SDM
revision 093 Vol. 2A lists legacy `NP 0F 54 /r`, Op/En A, with ModRM.reg as
destination and ModRM.r/m as source, and defines the 128-bit raw AND operation
([official Volume 2A PDF](https://cdrdv2.intel.com/v1/dl/getContent/671199),
pages 3-72–3-74). This profile does not extend any MOVUPS/XORPS profile. SSE
availability and CR0/CR4 prerequisites are assumed; memory forms, VEX/EVEX,
exception delivery and native/JIT support are outside this local slice. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-profile-s2-347b-s2-long-andps-reg-v1)
and [Stage 2 plan](../../../../docs/plans/STAGE_2.md#s2-347b--local-legacy-andps-register-to-register).

### S2-34.7c local register ORPS

`s2_34_7c_orps_register.case.json` is a handwritten literal fixture for the
separate `s2-long-orps-reg-v1` diagnostic profile. It executes
`45 0F 56 C1` (`ORPS XMM8,XMM9`) once, ORing all 128 raw bits while preserving
MXCSR, x87, GPR, flags and unrelated XMM registers. Intel SDM revision 093
Vol.  2B lists legacy `NP 0F 56 /r`, Op/En RM, ModRM.reg as destination and
ModRM.r/m as source, and defines the raw OR operation ([official Volume 2B
PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf), pages 4-171–4-172).
This profile does not extend any MOVUPS/XORPS/ANDPS profile. SSE availability
and CR0/CR4 prerequisites are assumed; memory forms, VEX/EVEX, exception
delivery and native/JIT support are outside this local slice. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-profile-s2-347c-s2-long-orps-reg-v1)
and [Stage 2 plan](../../../../docs/plans/STAGE_2.md#s2-347c--local-legacy-orps-register-to-register).

### S2-34.7d local register ANDNPS

`s2_34_7d_andnps_register.case.json` is a handwritten literal fixture for the
separate `s2-long-andnps-reg-v1` diagnostic profile. It executes
`45 0F 55 C1` (`ANDNPS XMM8,XMM9`) once and computes `(~old XMM8) & XMM9`
bytewise over all 128 raw bits. The chosen XMM8/XMM9 inputs deliberately make
the literal XMM8 result `10` in every byte, distinguishing inversion of the
destination from inversion of the source. MXCSR, x87, GPR, flags and unrelated
XMM registers are preserved. Intel SDM revision 093 Vol. 2A lists `NP 0F 55 /r`,
Op/En RM (`ModRM.reg` read/write, `ModRM.r/m` read), and defines the legacy
operation as NOT DEST then AND SRC ([official Volume 2A PDF]
(https://cdrdv2.intel.com/v1/dl/getContent/671199), pages 3-73–3-75). SSE and
CR0/CR4 prerequisites are assumed; memory/VEX/EVEX forms, exception delivery
and native/JIT support are outside this local slice. See the [CPU contract]
(../../../../docs/specs/CPU_CONTRACT.md#local-profile-s2-347d-s2-long-andnps-reg-v1)
and [Stage 2 plan]
(../../../../docs/plans/STAGE_2.md#s2-347d--local-legacy-andnps-register-to-register).

### S2-34.8a local register MOVSS

[`s2_34_8a_movss_register.case.json`](s2_34_8a_movss_register.case.json) is a
handwritten literal `F3 45 0F 10 C1` (`MOVSS XMM8,XMM9`) one-instruction case.
It checks that low dword `90919293` replaces XMM8's low dword while its upper 96
bits stay `8485868788898a8b8c8d8e8f`. RCX is nonzero and DF is set to detect
accidental REP string behavior. The profile admits only the mandatory F3
register-source form with ModR/M.mod=3 and optional REX.R/B after F3; no data
memory callback is made. Intel SDM revision 093 Vol. 2B distinguishes this from
the memory-load form, which clears destination bits 127:32. VEX/EVEX, memory,
F2/66, `F3 0F 11`, prefix permutations, CPU gates, exception delivery, and
native/JIT are outside scope. See the [S2-34.8a plan](../../../../docs/plans/STAGE_2.md#s2-348a--local-legacy-movss-register-to-register).

### S2-34.8b local reverse-direction register MOVSS

[`s2_34_8b_movss_store_register.case.json`](s2_34_8b_movss_store_register.case.json)
is a handwritten literal in the distinct exact profile
`s2-long-movss-store-reg-v1`. It executes `F3 45 0F 11 C1`
(`MOVSS XMM9,XMM8`): ModR/M.reg selects source XMM8 and ModR/M.r/m selects
destination XMM9. The low dword becomes `80818283`; destination upper bytes
remain `9495969798999a9b9c9d9e9f`. RCX is nonzero and DF is set to make any
REP-style behavior visible. The profile's allowlist covers only ModR/M.mod=3
and one effective REX.R/B after F3; REX.W/X, memory forms, the `0F 10`
direction, legacy prefix permutations, F2/66, VEX and EVEX are rejected. The
operation makes no data-memory callback and preserves every other state field.
Native/JIT, feature gates, exception delivery and physical execution remain
outside scope. See the [S2-34.8b plan](../../../../docs/plans/STAGE_2.md#s2-348b--local-legacy-reverse-direction-movss-register-to-register).

### S2-34.8c local MOVSS m32 load from `[RAX]`

[`s2_34_8c_movss_memory_load_rax.case.json`](s2_34_8c_movss_memory_load_rax.case.json)
is a handwritten literal `F3 44 0F 10 00` (`MOVSS XMM8,[RAX]`) case in exact
profile `s2-long-movss-mem-load-v1`. RAX points one byte into a readable
read-only data region; the interpreter reads exactly four unaligned bytes
`00 01 02 03`, loads XMM8 low dword and clears its upper 96 bits. The fixture
keeps nonzero RCX/DF and seeded state to expose unrelated state changes. The
profile also accepts all destination XMM indices and optional REX.R, while
rejecting REX.B/X/W, other bases, SIB and displacement forms. Native/JIT,
feature gates, #UD/#NM delivery, fault priority, and physical execution are
outside scope. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-legacy-movss-m32-load-profile--s2-348c)
and [plan](../../../../docs/plans/STAGE_2.md#s2-348c--local-legacy-movss-m32-load-from-rax).

### S2-34.8d local MOVSS m32 store to `[RAX]`

[`s2_34_8d_movss_memory_store_rax.case.json`](s2_34_8d_movss_memory_store_rax.case.json)
is a handwritten literal `F3 44 0F 11 00` (`MOVSS [RAX],XMM8`) case for the
exact profile `s2-long-movss-mem-store-v1`. RAX points one byte into writable
data. The low dword `80818283` from XMM8 replaces exactly four data bytes;
surrounding canaries and all XMM state remain unchanged. Nonzero RCX and seeded
state expose unrelated changes. The allowlist accepts all source XMM indices
and optional REX.R, while rejecting REX.B/X/W, SIB, displacement, other bases,
register forms, and other prefix encodings. Native/JIT are denied; feature
gates, exception delivery, alignment/fault behavior, and physical execution are
outside scope. See the [CPU contract]
(../../../../docs/specs/CPU_CONTRACT.md#local-legacy-movss-m32-store-profile--s2-348d)
and [plan](../../../../docs/plans/STAGE_2.md#s2-348d--local-legacy-movss-m32-store-to-rax).

### S2-34.9a local scalar-double MOVSD register move

[`s2_34_9a_movsd_register.case.json`](s2_34_9a_movsd_register.case.json) is
the literal legacy `F2 45 0F 10 C1` (`MOVSD XMM8,XMM9`) case in exact profile
`s2-long-movsd-reg-v1`. It copies XMM9's low 64 raw bits into XMM8 and keeps
XMM8's upper 64 sentinel bits. The seeded state includes RCX=0, DF=1,
x87 state, MXCSR and distinct contents in all XMM registers. The separate
profile admits only `F2 0F 10 /r`, ModR/M.mod=3, and optional REX.R/B; it rejects
memory/reverse forms, other mandatory prefixes, W/X, VEX/EVEX, and string MOVSD
`A5`. SSE2 state and architectural preconditions are assumed locally; no CPUID
claim is made. Native/JIT, exception delivery and physical execution are not
covered. See the [CPU contract]
(../../../../docs/specs/CPU_CONTRACT.md#local-legacy-scalar-double-movsd-register-profile--s2-349a),
[interpreter notes](../../../../docs/testing/INTERPRETER.md#local-legacy-scalar-double-movsd-register-slice--s2-349a),
and [plan](../../../../docs/plans/STAGE_2.md#s2-349a--local-legacy-scalar-double-movsd-register-to-register).

### S2-34.9b reverse-direction scalar-double MOVSD register move

[`s2_34_9b_movsd_store_register.case.json`](s2_34_9b_movsd_store_register.case.json)
is a handwritten literal legacy `F2 45 0F 11 C1` (`MOVSD XMM9,XMM8`) case in
the separate exact profile `s2-long-movsd-store-reg-v1`. ModR/M.reg selects
source XMM8 and r/m selects destination XMM9. XMM8's raw low qword is copied to
XMM9 while its distinct upper-qword sentinel remains. The fixture includes
RCX=0, DF=1, x87/MXCSR and full XMM state so REP/string confusion and unrelated
state changes are observable. The profile rejects memory operands, F3/66/other
prefixes, REX.W/X, prefix permutations, VEX/EVEX and string MOVSD `A5`; F2 is
the mandatory opcode prefix. Native/JIT and physical execution are outside
scope. Intel SDM revision 093 Vol. 2B MOVSD `F2 0F 11 /r`, Op/En C, defines
`xmm2/m64,xmm1`, with ModR/M.reg source and r/m destination. See the [CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-reverse-direction-scalar-double-movsd-register-profile--s2-349b),
[interpreter notes](../../../../docs/testing/INTERPRETER.md#local-reverse-direction-scalar-double-movsd-register-slice--s2-349b),
and [plan](../../../../docs/plans/STAGE_2.md#s2-349b--local-legacy-reverse-direction-scalar-double-movsd-register).

### S2-34.9c legacy scalar-double MOVSD memory load

[`s2_34_9c_movsd_memory_load_rax.case.json`](s2_34_9c_movsd_memory_load_rax.case.json)
is a handwritten literal `F2 44 0F 10 00` (`MOVSD XMM8,[RAX]`) in the separate
exact profile `s2-long-movsd-mem-load-v1`. The source address is deliberately
unaligned. It reads the eight bytes beginning at RAX into XMM8[63:0] and clears
XMM8[127:64]; the full XMM/x87/MXCSR/GPR/flags snapshot and surrounding memory
bytes make unrelated writes visible. The profile accepts only `[RAX]`,
mandatory F2 and optional REX.R, and rejects other addressing, prefixes,
register/store directions, VEX/EVEX and string MOVSD `A5`. F2 is the mandatory
opcode prefix rather than REPNE. Intel SDM revision 093, Vol. 2B, MOVSD
`F2 0F 10 /r`, Op/En A, specifies ModR/M.reg destination and m64 source and
clears the legacy memory-load destination upper qword. The executor validates
the complete canonical48 byte range and performs one 8-byte read before XMM/RIP
commit. Native/JIT and physical execution are outside scope. See the [CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-legacy-scalar-double-movsd-memory-load-profile--s2-349c),
[interpreter notes](../../../../docs/testing/INTERPRETER.md#local-scalar-double-movsd-memory-load-slice--s2-349c),
and [plan](../../../../docs/plans/STAGE_2.md#s2-349c--local-legacy-scalar-double-movsd-m64-load-from-rax).

### S2-34.9d legacy scalar-double MOVSD memory store

[`s2_34_9d_movsd_memory_store_rax.case.json`](s2_34_9d_movsd_memory_store_rax.case.json)
is a handwritten literal `F2 44 0F 11 00` (`MOVSD [RAX],XMM8`) case in the
separate exact profile `s2-long-movsd-mem-store-v1`. RAX points one byte into a
writable data region; XMM8's low qword replaces exactly eight bytes while
surrounding canaries and all XMM state remain unchanged. The profile admits all
source XMM indices with optional REX.R and rejects other address forms, prefixes,
REX bits, register reverse-direction forms, loads, VEX/EVEX and string MOVSD
`A5`. Intel SDM revision 093 Vol. 2B `F2 0F 11 /r`, Op/En C, maps ModR/M.reg to
the XMM source and ModR/M.r/m to the m64 destination. The interpreter performs
one eight-byte write and commits RIP only on success. Native/JIT, fault priority
or delivery, SSE gates, CPUID and physical execution are outside this local
slice. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-legacy-scalar-double-movsd-memory-store-profile--s2-349d),
[interpreter notes](../../../../docs/testing/INTERPRETER.md#local-scalar-double-movsd-memory-store-slice--s2-349d),
and [plan](../../../../docs/plans/STAGE_2.md#s2-349d--local-legacy-scalar-double-movsd-m64-store-to-rax).

### S2-35.1 local register PXOR diagnostic profile

The exact profile `s2-long-pxor-reg-v1` admits `66 [REX.R/B] 0F EF /r` only
when ModR/M.mod=3. The mandatory 66 selects the XMM/SSE2 form; bare `0F EF`
is MMX and rejected. Register fields independently select XMM0–XMM15. The
operation XORs raw 128-bit values and preserves MXCSR, x87, GPR, RFLAGS and
other XMM state. Intel SDM revision 093 Vol. 2B PXOR entry is the encoding and
feature basis. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumed without a
CPUID claim. Memory, other prefixes, REX.W/X, VEX/EVEX and native/JIT are
outside the slice. S2-34 and S2-35 remain open.

### S2-35.2 local register PADDB diagnostic profile

`s2_35_2_paddb_register.case.json` is a literal XMM8,XMM9 example for exact
profile `s2-long-paddb-reg-v1`. It admits `66 [one optional REX.R/B] 0F FC /r`
with ModR/M.mod=3; mandatory `66` distinguishes XMM/SSE2 from MMX. The local
executor adds each of the 16 byte lanes modulo 256 and preserves all unrelated
state without data-memory callbacks. Intel SDM revision 093 Vol. 2B PADDB is
the encoding/semantics source. SSE2, CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1 are
assumptions. Memory forms, other packed-add widths and saturating operations,
other prefixes, REX.W/X, VEX/EVEX, native/JIT and physical execution remain
outside this diagnostic slice. S2-34, S2-35 and `S2-35 → S2-34` remain open.


### S2-35.3 local register PSUBB diagnostic profile

`s2_35_3_psubb_register.case.json` is a literal XMM8,XMM9 example of
`66 [one optional REX.R/B] 0F F8 /r`, ModR/M.mod=3. Each byte computes
`(DEST - SRC) mod 256` independently. The mandatory 66 distinguishes XMM/SSE2
from MMX; wider/saturating forms, memory, other prefixes, REX.W/X, VEX/EVEX,
native/JIT and physical execution are excluded. SSE2, CR0.EM=0, CR0.TS=0 and
CR4.OSFXSR=1 are assumptions only, with no CPUID claim.

### S2-35.35 local register PSHUFD diagnostic profile

`s2_35_35_pshufd_register.case.json` is a literal XMM8,XMM9 example for exact
profile `s2-long-pshufd-reg-v1`. It admits only
`66 [one optional REX.R/B] 0F 70 /r ib`, ModR/M.mod=3. Each immediate control
field selects one dword from the original source value into the corresponding
destination lane, including when source and destination alias. Tests cover all
256 controls and all 16×16 register pairs. The profile assumes SSE2 and
CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1; native/JIT and physical execution are excluded.

### S2-35.4 local register PADDUSB diagnostic profile

`s2_35_4_paddusb_register.case.json` is a literal XMM8,XMM9 example for exact
profile `s2-long-paddusb-reg-v1`. It admits only
`66 [one optional REX.R/B] 0F DC /r`, ModR/M.mod=3. Each unsigned byte lane
clamps its sum at 255 independently: `0x80+0x80` becomes `0xff`, while
`0x7f+0x01` becomes `0x80`. Mandatory 66 distinguishes XMM/SSE2 from bare
MMX. Memory, PADDUSW/other widths, signed saturation, other prefixes, REX.W/X,
VEX/EVEX, native/JIT and physical execution are excluded. SSE2 and
CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only; there is no CPUID
claim. Source: Intel SDM revision 093 Vol. 2B, PADDUSB,
[official PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf) and
[official Intel SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).

### S2-35.5 local register PSUBUSB diagnostic profile

`s2_35_5_psubusb_register.case.json` is a literal XMM8,XMM9 example for exact profile `s2-long-psubusb-reg-v1`. Only `66 [one optional REX.R/B] 0F D8 /r` with ModR/M.mod=3 is admitted. Every unsigned byte lane computes `max(DEST - SRC, 0)` independently; the mandatory 66 selects the XMM/SSE2 form rather than bare MMX. Memory, other packed widths, signed saturation, other prefixes, REX.W/X, VEX/EVEX, native/JIT and physical execution are excluded. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only, with no CPUID claim. Source: Intel SDM revision 093, Vol. 2B, PSUBUSB, [official instruction reference PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf) and [Intel SDM index](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).

### S2-35.6 local register PADDW diagnostic profile

`s2_35_6_paddw_register.case.json` is a handwritten XMM8,XMM9 fixture for exact profile `s2-long-paddw-reg-v1`. Only `66 [one optional REX.R/B] 0F FD /r` with ModR/M.mod=3 is admitted. Eight little-endian 16-bit lanes add independently modulo 65536. Bare MMX, other packed widths, saturation, memory, other prefixes, REX.W/X, VEX/EVEX, native/JIT and physical execution are excluded. SSE2 and CR0/CR4 conditions are assumptions only; there is no CPUID claim. Source: Intel SDM revision 093, Volume 2B, PADDW, pp. 4-201–203, [official Intel instruction reference PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf) and [Intel SDM index](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).

### S2-35.7 local register PSUBW diagnostic profile

`s2_35_7_psubw_register.case.json` is a literal XMM8,XMM9 case for exact
profile `s2-long-psubw-reg-v1`. It admits only `66 [one optional REX.R/B] 0F
F9 /r`, ModR/M.mod=3. Each little-endian u16 lane computes `(DEST - SRC) mod
65536` independently. Bare MMX, PSUBD/Q, saturation, memory, other prefixes,
REX.W/X, VEX/EVEX, JIT/native and physical execution are excluded. SSE2 and
CR0/CR4 are assumptions only, without a CPUID claim. The encoding and lane
semantics follow Intel SDM revision 093 Vol. 2B, PSUBW.


### S2-35.8 local register PADDUSW diagnostic profile

`s2_35_8_paddusw_register.case.json` is a literal XMM8,XMM9 case for exact
`66 [one optional REX.R/B] 0F DD /r` register-only execution in the local
`s2-long-paddusw-reg-v1` profile. It captures all XMM, MXCSR and x87 state. The
case documents the PADDUSW encoding and unsigned word saturation from Intel SDM
revision 093, Volume 2B. This is not native/JIT or physical evidence.

### S2-35.9 local register PSUBUSW diagnostic profile

`s2_35_9_psubusw_register.case.json` is a literal XMM8,XMM9 fixture for exact
profile `s2-long-psubusw-reg-v1`. It admits only
`66 [one optional REX.R/B] 0F D9 /r` with ModR/M.mod=3. Eight little-endian
unsigned word lanes independently compute `max(DEST - SRC, 0)`; mandatory 66
selects the XMM/SSE2 form rather than bare MMX. The literal lanes include
`0000-0001=0000`, `0002-0001=0001`, `0001-0002=0000`, `ffff-0001=fffe`,
`8000-7fff=0001`, `7fff-8000=0000`, and `0100-0001=00ff`. Memory, other widths,
signed saturation, other prefixes, REX.W/X, VEX/EVEX, JIT/native and physical
execution are excluded. SSE2, CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1 are assumptions
only; no CPUID claim is made. Source: Intel SDM revision 093, Vol. 2B, PSUBUSW,
[official instruction reference](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf)
and [Intel SDM index](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).

### S2-35.10 local register PADDSB diagnostic profile

`s2_35_10_paddsb_register.case.json` is a handwritten literal XMM8,XMM9 case for
`s2-long-paddsb-reg-v1`. It uses signed-byte saturation, preserves all state
outside XMM8 and RIP, and has no data-memory accesses. The encoding and
semantics follow Intel SDM revision 093, Vol. 2B, PADDSB. SSE2 and CR0/CR4
preconditions are assumptions; JIT, native and physical execution are excluded.

### S2-35.11 local register PSUBSB diagnostic profile

`s2_35_11_psubsb_register.case.json` is a handwritten literal XMM8,XMM9 case for
`s2-long-psubsb-reg-v1`. It computes signed-byte DEST minus SRC with saturation
per lane, preserves all state outside XMM8 and RIP, and makes no data-memory
accesses. Its lanes include `00-80=7f`, `80-01=80`, `7f-ff=7f`, and
`ff-01=00`, showing signed interpretation, direction and both saturation ends.
The encoding and semantics follow Intel SDM revision 093, Vol. 2B, PSUBSB. SSE2
and CR0/CR4 preconditions are assumptions; JIT, native and physical execution
are excluded.

### S2-35.12 local register PADDSW diagnostic profile

`s2_35_12_paddsw_register.case.json` is a handwritten literal XMM8,XMM9 case
for `s2-long-paddsw-reg-v1`. It admits only legacy
`66 [one optional REX.R/B] 0F ED /r`, ModR/M.mod=3, and exercises signed-word
saturation, including positive and negative overflow. Eight little-endian
word lanes use widened signed sums clamped to `[-32768, 32767]`. The encoding
and semantics follow Intel SDM revision 093, Vol. 2B, PADDSW. SSE2 and CR0/CR4
conditions are assumptions; native/JIT and physical execution are excluded.

### S2-35.13 local register PSUBSW diagnostic profile

`s2_35_13_psubsw_register.case.json` is a handwritten literal XMM8,XMM9 case
for `s2-long-psubsw-reg-v1`. It admits only legacy
`66 [one optional REX.R/B] 0F E9 /r` with ModR/M.mod=3. Each little-endian
signed word lane subtracts the source from the destination in widened i32 and
clamps to `[-32768, 32767]`. The encoding and semantics follow Intel SDM
revision 093, Vol. 2B, PSUBSW. SSE2 and CR0/CR4 conditions are assumptions;
native/JIT and physical execution are excluded.

### S2-35.14 local register PADDD diagnostic profile

`s2_35_14_paddd_register.case.json` is a handwritten literal XMM8,XMM9 case
for `s2-long-paddd-reg-v1`. It admits only legacy
`66 [one optional REX.R/B] 0F FE /r` with ModR/M.mod=3. Four little-endian
u32 lanes add modulo 2^32 independently; its operands exercise wraparound,
carry-chain behavior and lane isolation. SSE2 and CR0/CR4 conditions are
assumptions; native/JIT and physical execution are excluded.

### S2-35.15 local register PSUBD diagnostic profile

`s2_35_15_psubd_register.case.json` is a handwritten literal XMM8,XMM9 fixture
for strict `s2-long-psubd-reg-v1`: `66 45 0F FA C1`. It subtracts four
independent little-endian u32 lanes modulo 2^32. Expected XMM8 is
`fefffffffffffffffeffff7f00000000`; other XMM registers, x87 state, MXCSR,
GPRs, flags, and memory are unchanged. JIT and native x86 execution remain
unsupported. This fixture is local interpreter evidence only.

### S2-35.16 local register PADDQ diagnostic profile

`s2_35_16_paddq_register.case.json` is a handwritten literal XMM8,XMM9 fixture
for strict `s2-long-paddq-reg-v1`: `66 45 0F D4 C1`. Its two little-endian
u64 lanes add independently modulo 2^64; lane 0 wraps to zero and lane 1
carries through bit 63. Other XMM registers, x87 state, MXCSR, GPRs, flags, and
memory are unchanged. JIT and native x86 execution remain unsupported. This is
local interpreter evidence only.

### S2-35.17 local register PSUBQ diagnostic profile

`s2_35_17_psubq_register.case.json` is a handwritten literal XMM8,XMM9 fixture
for strict `s2-long-psubq-reg-v1`: `66 45 0F FB C1`. Intel SDM Rev. 093,
Vol. 2B describes the legacy XMM form as subtracting the source (`ModRM:r/m`)
from the destination (`ModRM:reg`); the operation is therefore
`DEST - SRC` independently in each little-endian u64 lane, modulo 2^64. The
fixture uses asymmetric operands to make the direction observable. Other XMM
registers, x87 state, MXCSR, GPRs, flags, and memory are unchanged. JIT and
native x86 execution remain unsupported. This is local interpreter evidence
only.

### S2-35.18 local register PCMPEQB diagnostic profile

`s2_35_18_pcmpeqb_register.case.json` is a handwritten literal XMM8,XMM9
fixture for strict `s2-long-pcmpeqb-reg-v1`: `66 45 0F 74 C1`. Each of the
sixteen byte lanes compares destination against source and writes `FF` for
equality or `00` otherwise. This remains the local interpreter profile only;
JIT and native x86 execution are denied.


### S2-35.19 local register PCMPEQW diagnostic profile

`s2_35_19_pcmpeqw_register.case.json` is a handwritten literal XMM8,XMM9
fixture for strict `s2-long-pcmpeqw-reg-v1`: `66 45 0F 75 C1`. It compares
eight independent little-endian words and emits `FFFF` for equal words and
`0000` otherwise. This local-only fixture demonstrates byte order and mixed
equality masks; interpreter execution is supported while JIT/native execution
is denied. SSE2 and CR0/CR4 preconditions are assumptions; there is no CPUID or
exception-delivery claim.

### S2-35.20 local register PCMPEQD diagnostic profile

`s2_35_20_pcmpeqd_register.case.json` is a handwritten literal XMM8,XMM9
fixture for strict `s2-long-pcmpeqd-reg-v1`: `66 45 0F 76 C1`. It compares
four independent little-endian dwords and emits `FFFFFFFF` only for exact
whole-dword equality, otherwise `00000000`. Interpreter execution is supported;
JIT/native execution remains denied. SSE2 and CR0/CR4 preconditions are
assumptions only, with no CPUID or exception-delivery claim.


### S2-35.21 local register PCMPGTB diagnostic profile

`s2_35_21_pcmpgtb_register.case.json` is a handwritten literal XMM8,XMM9
fixture for strict `s2-long-pcmpgtb-reg-v1`: `66 45 0F 64 C1`. Sixteen signed
byte lanes emit `FF` iff the destination byte is greater than the source byte,
otherwise `00`; alternating sentinels make operand direction and signedness
visible. Interpreter execution is supported; JIT/native execution is denied.
PCMPGTQ needs SSE4.2 and is excluded; there is no feature-gate or
exception-delivery claim.

### S2-35.22 local register PCMPGTW diagnostic profile

`s2_35_22_pcmpgtw_register.case.json` is a handwritten literal XMM8,XMM9
fixture for strict `s2-long-pcmpgtw-reg-v1`: `66 45 0F 65 C1`. Eight signed
little-endian word lanes emit `FFFF` iff the destination word is greater than
the source word, otherwise `0000`; mixed sentinels distinguish word-lane
comparison from byte comparison and show operand direction. Interpreter
execution is supported; JIT/native execution is denied. SSE2 and CR0/CR4
preconditions are assumptions only; no feature-gate or exception-delivery
claim is made.

### S2-35.23 local register PCMPGTD diagnostic profile

`s2_35_23_pcmpgtd_register.case.json` is a handwritten literal XMM8,XMM9
fixture for strict `s2-long-pcmpgtd-reg-v1`: `66 45 0F 66 C1`. The first
`66` is the mandatory prefix; the final `66` is PCMPGTD's opcode byte. Four
signed little-endian dword lanes emit `FFFFFFFF` iff destination is greater than
source, otherwise `00000000`; sentinels expose operand direction, signedness,
and dword lane width. Interpreter execution is supported;
JIT/native execution is denied. SSE2 and CR0/CR4 preconditions are assumptions
only; no feature-gate or exception-delivery claim is made.

### S2-35.29 local register PUNPCKHDQ diagnostic profile

`s2_35_29_punpckhdq_register.case.json` is a handwritten literal XMM8,XMM9
fixture for `s2-long-punpckhdq-reg-v1`: `66 45 0F 6A C1`. The four dwords
become `[old_DEST[2], SRC[2], old_DEST[3], SRC[3]]` in little-endian order.
Interpreter execution is supported while JIT/native execution remains denied;
SSE2 and CR0/CR4 preconditions are assumptions only.

### S2-35.30 local register PUNPCKLQDQ diagnostic profile

`s2_35_30_punpcklqdq_register.case.json` is a handwritten literal XMM8,XMM9
fixture for `s2-long-punpcklqdq-reg-v1`: `66 45 0F 6C C1`. The two little-endian
qwords become `[old_DEST[0], SRC[0]]`; the original high qwords are ignored and
all 128 destination bits are replaced. PUNPCKLQDQ has no MMX form, so bare
`0F 6C /r` without mandatory `66` is rejected. Interpreter execution is
supported while JIT/native execution remains denied; SSE2 and CR0/CR4
preconditions are assumptions only.


### S2-35.31 local register PUNPCKHQDQ diagnostic profile

`s2_35_31_punpckhqdq_register.case.json` is a handwritten literal XMM8,XMM9
fixture for `s2-long-punpckhqdq-reg-v1`: `66 45 0F 6D C1`. The destination
receives its original high qword followed by the source high qword, and its low
qword is ignored. PUNPCKHQDQ has no MMX form, so bare `0F 6D /r` without the
mandatory `66` is rejected. Interpreter execution is supported; JIT/native
execution remains denied.


### S2-35.32 local register PACKSSWB diagnostic profile

`s2_35_32_packsswb_register.case.json` is a handwritten literal XMM8,XMM9
fixture for `s2-long-packsswb-reg-v1`, encoded as `66 45 0F 63 C1`. Its signed
word inputs include the saturation boundaries and `0100`/`ff00`; destination
bytes 0–7 contain the eight saturated original destination words, followed by
the eight saturated original source words in bytes 8–15. The local profile
assumes SSE2 architectural preconditions and denies JIT and native execution.


s2_35_33_packuswb_register.case.json is a handwritten literal XMM8,XMM9
fixture for s2-long-packuswb-reg-v1, encoded as 66 45 0F 67 C1. Each input
word is interpreted as signed i16, clamped to [0,255], and narrowed; ffff,
8000, and ff00 therefore produce 00, while 0001, 007f, 0080, 00ff, 0100, and
7fff exercise the low and high boundaries. The result contains eight
destination lanes followed by eight source lanes. The local profile assumes
SSE2 architectural preconditions and denies JIT and native execution.

### S2-35.34 local register PACKSSDW diagnostic profile

`s2_35_34_packssdw_register.case.json` is a handwritten literal XMM8,XMM9
fixture for `s2-long-packssdw-reg-v1`, encoded as `66 45 0F 6B C1`. Each
signed dword is clamped to [-32768,32767] and narrowed; destination lanes come
first, followed by source lanes. The local profile assumes SSE2 architectural
preconditions and denies JIT and native execution.

### S2-35.37 local register PSLLW immediate diagnostic profile

`s2_35_37_psllw_imm_register.case.json` is a handwritten literal XMM9 fixture
for `s2-long-psllw-imm-reg-v1`, encoded as `66 41 0F 71 F1 01`. `/6` is the
ModR/M selector and `r/m` selects XMM9. Eight little-endian u16 lanes shift
left by the unmasked unsigned imm8 count; each result is truncated to 16 bits.
The local profile assumes SSE2 architectural preconditions and denies JIT and
native execution.


### S2-35.38 local register PSRLW immediate diagnostic profile

`s2_35_38_psrlw_imm_register.case.json` is a handwritten literal XMM9 fixture
for `s2-long-psrlw-imm-reg-v1`, encoded as `66 41 0F 71 D1 01`. `/2` is the
ModR/M selector and `r/m` selects XMM9; `/6` is rejected as the sibling PSLLW
selector. Eight little-endian u16 lanes shift logically right by the unmasked
unsigned imm8 count; counts >=16 clear every lane. The local profile assumes
SSE2 architectural preconditions and denies JIT and native execution.

### S2-35.39 local register PSRAW immediate diagnostic profile

`s2_35_39_psraw_imm_register.case.json` is a handwritten literal XMM9 fixture
for `s2-long-psraw-imm-reg-v1`, encoded as `66 41 0F 71 E1 01`. `/4` selects
PSRAW and `r/m` selects XMM9; `/2` PSRLW and `/6` PSLLW are rejected by this
profile. The eight signed little-endian i16 lanes arithmetic-shift right by
unmasked unsigned imm8; counts >=16 sign-fill negative lanes and zero
nonnegative lanes. The local profile assumes SSE2 preconditions and denies JIT
and native execution.


### S2-35.41 local register PSLLD immediate diagnostic profile

`s2_35_41_pslld_imm_register.case.json` is a handwritten literal XMM9 fixture
for `s2-long-pslld-imm-reg-v1`, encoded as `66 41 0F 72 F1 01`. `/6` selects
PSLLD and `r/m` selects XMM9. Four little-endian u32 lanes shift logically
left by the unmasked unsigned imm8; counts >=32 clear every lane. The local
profile assumes SSE2 architectural preconditions and denies JIT and native
execution.

### S2-35.45 local register PSRLQ immediate diagnostic profile

`s2_35_45_psrlq_imm_register.case.json` is a handwritten literal XMM9 fixture
for `s2-long-psrlq-imm-reg-v1`, encoded as `66 41 0F 73 D1 01`. `/2` selects
PSRLQ and `r/m` selects XMM9; `/6` PSLLQ and `/3` PSRLDQ are distinct selectors.
Two independent little-endian u64 lanes shift logically right by unmasked
unsigned imm8; counts >=64 clear both lanes. The local profile assumes SSE2
preconditions and denies JIT and native execution.

### S2-35.44 local register PSLLQ immediate diagnostic profile

`s2_35_44_psllq_imm_register.case.json` is a handwritten literal XMM9 fixture
for `s2-long-psllq-imm-reg-v1`, encoded as `66 41 0F 73 F1 01`. `/6` selects
PSLLQ and `r/m` selects XMM9; `/7` PSLLDQ is rejected. Two little-endian u64
lanes shift logically left by unmasked unsigned imm8; counts >=64 clear both
lanes. The local profile assumes SSE2 preconditions and denies JIT and native
execution.

### S2-35.46 local register PSLLDQ immediate diagnostic profile

`s2_35_46_pslldq_imm_register.case.json` is a handwritten literal XMM9 fixture
for `s2-long-pslldq-imm-reg-v1`, encoded as `66 41 0F 73 F9 01`. `/7` selects
PSLLDQ and `r/m` selects XMM9; `/6` PSLLQ and `/3` PSRLDQ remain distinct.
The unsigned, unmasked byte count shifts the whole XMM value toward higher
byte indices with zero fill; counts >=16 clear the entire register. The local
profile assumes SSE2 architectural preconditions and denies JIT and native
execution.

### S2-35.42 local register PSRLD immediate diagnostic profile

`s2_35_42_psrld_imm_register.case.json` is a handwritten literal XMM9 fixture
for `s2-long-psrld-imm-reg-v1`, encoded as `66 41 0F 72 D1 01`. `/2` selects
PSRLD and `r/m` selects XMM9; `/6` selects sibling PSLLD and is rejected by
this fixture profile. Four little-endian u32 lanes shift logically right by
the unmasked unsigned imm8; counts >=32 clear every lane. The local profile
assumes SSE2 architectural preconditions and denies JIT and native execution.

### S2-35.47 local register PSRLDQ immediate diagnostic profile

`s2_35_47_psrldq_imm_register.case.json` is a handwritten literal XMM9 fixture
for `s2-long-psrldq-imm-reg-v1`, encoded as `66 41 0F 73 D9 01`. `/3` selects
PSRLDQ and `r/m` selects XMM9; `/2` PSRLQ and `/7` PSLLDQ remain distinct.
The unsigned, unmasked byte count shifts the whole XMM value toward lower byte
indices with zero fill; counts >=16 clear the entire register. The local
profile assumes SSE2 architectural preconditions and denies JIT/native.

### S2-35.43 local register PSRAD immediate diagnostic profile

`s2_35_43_psrad_imm_register.case.json` is a handwritten literal XMM9 fixture
for `s2-long-psrad-imm-reg-v1`, encoded as `66 41 0F 72 E1 01`. `/4` selects
PSRAD and `r/m` selects XMM9; `/2` PSRLD and `/6` PSLLD are rejected by this
fixture profile. Four signed little-endian i32 lanes shift arithmetically
right by the unmasked unsigned imm8; counts >=32 sign-fill negative lanes and
zero nonnegative lanes. The local profile assumes SSE2 preconditions and
denies JIT and native execution.

### S2-35.49 local register PAND diagnostic profile

`s2_35_49_pand_register.case.json` is a handwritten high-XMM fixture for
`s2-long-pand-reg-v1`, encoded as `66 45 0F DB C1` (`PAND XMM8, XMM9`). The
mandatory `66` selects XMM/SSE2 rather than MMX; `REX.R` and `REX.B` select
XMM8 and XMM9 independently. The result is the raw 128-bit AND. This local
profile assumes SSE2 preconditions and denies JIT and native execution.

### S2-35.51 local register POR diagnostic profile

`s2_35_51_por_register.case.json` is a handwritten high-XMM fixture for
`s2-long-por-reg-v1`, encoded as `66 45 0F EB C1` (`POR XMM8, XMM9`). The
mandatory `66` selects XMM/SSE2, and REX.R/REX.B independently select XMM8 and
XMM9. The interpreter computes the raw 128-bit `old_DEST | old_SRC`; its
literal XMM8 result is `909192939495969798999a9b9c9d9e9f`. This local profile
assumes SSE2 preconditions and denies JIT and native execution.

### S2-35.52 local register PAVGB diagnostic profile

`s2_35_52_pavgb_register.case.json` is a handwritten high-XMM fixture for
`s2-long-pavgb-reg-v1`, encoded as `66 45 0F E0 C1` (`PAVGB XMM8, XMM9`).
REX.R/REX.B independently select XMM8/XMM9. The unsigned rounded byte average
`(old_DEST + old_SRC + 1) >> 1` produces literal XMM8 bytes
`88898a8b8c8d8e8f9091929394959697`. SSE2 preconditions are assumed; JIT and
native execution are denied. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-register-only-pavgb-profile--s2-3552), [plan](../../../../docs/plans/STAGE_2.md#s2-3552--local-only-bounded-register-pavgb), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-3552--local-pavgb-register-profile).

### S2-35.50 local register PANDN diagnostic profile

`s2_35_50_pandn_register.case.json` is a handwritten high-XMM fixture for
`s2-long-pandn-reg-v1`, encoded as `66 45 0F DF C1` (`PANDN XMM8, XMM9`). The
mandatory `66` selects XMM/SSE2; REX.R and REX.B independently select XMM8 and
XMM9. The destination computes `~old_DEST & SRC` over all 128 bits. This local
profile assumes SSE2 preconditions and denies JIT and native execution.


### S2-34.11 local register MOVAPS diagnostic profile

`s2_34_11_movaps_register.case.json` is a handwritten schema-v2 XMM8/XMM9 fixture for `s2-long-movaps-reg-v1`, encoded as `45 0F 28 C1` (`MOVAPS XMM8, XMM9`). Its literal XMM9 source is `909192939495969798999a9b9c9d9e9f`; the interpreter copies those 16 raw bytes to XMM8. The local profile assumes SSE preconditions and is denied by JIT and native admission. This fixture remains separate from the MOVAPD profile and does not represent parent S2-34 acceptance.

### S2-34.12 local reverse-direction register MOVAPS diagnostic profile

`s2_34_12_movaps_reverse_register.case.json` is a handwritten schema-v2 XMM8-to-XMM9 fixture for the separate `s2-long-movaps-reg-reverse-v1` profile, encoded as `45 0F 29 C1`. ModR/M.reg plus REX.R selects source XMM8; ModR/M.r/m plus REX.B selects destination XMM9. The literal source bytes are `808182838485868788898a8b8c8d8e8f`; the interpreter copies all 16 bytes to XMM9 and preserves XMM8 and the remaining state. JIT and native admission deny this profile. The register form makes no aligned-memory behavior claim and does not represent parent S2-34 acceptance.

### S2-34.13 local register MOVAPD diagnostic profile

`s2_34_13_movapd_register.case.json` is a handwritten schema-v2 XMM8/XMM9 fixture for `s2-long-movapd-reg-v1`, encoded as `66 45 0F 28 C1` (`MOVAPD XMM8, XMM9`). Mandatory `66` distinguishes this SSE2 form from MOVAPS. Its literal XMM9 source is `909192939495969798999a9b9c9d9e9f`; the interpreter copies those 16 raw bytes to XMM8. The local profile assumes SSE2 preconditions and is denied by JIT and native admission. It covers only the register form and makes no aligned-memory behavior claim or parent S2-34 acceptance claim.

### S2-34.14 local reverse-direction register MOVAPD diagnostic profile

`s2_34_14_movapd_reverse_register.case.json` is a handwritten schema-v2 XMM8-to-XMM9 fixture for `s2-long-movapd-reg-reverse-v1`, encoded as `66 45 0F 29 C1`. Mandatory `66` selects SSE2 MOVAPD; ModR/M.reg plus REX.R selects source XMM8, while ModR/M.r/m plus REX.B selects destination XMM9. The literal source bytes are `808182838485868788898a8b8c8d8e8f`; the interpreter copies all 16 bytes to XMM9 and preserves XMM8 and the remaining state. JIT and native admission deny this profile. The register form makes no aligned-memory behavior claim and does not represent parent S2-34 acceptance.

### S2-34.15 local register MOVUPD diagnostic profile

`s2_34_15_movupd_register.case.json` is a handwritten schema-v2 XMM8/XMM9 fixture for `s2-long-movupd-reg-v1`, encoded as `66 45 0F 10 C1` (`MOVUPD XMM8, XMM9`). Mandatory `66` distinguishes the packed SSE2 move from prefix-free MOVUPS and F2/F3 scalar MOVSD/MOVSS. Its literal XMM9 source contains all 16 distinct bytes `909192939495969798999a9b9c9d9e9f`, including a distinguishable high 64-bit half; execution copies all 128 raw bits into XMM8. The profile assumes SSE2 preconditions and is denied by JIT and native admission. It covers only the register form and makes no aligned-memory behavior claim or parent S2-34 acceptance claim.

### S2-34.16 local reverse-direction register MOVUPD diagnostic profile

`s2_34_16_movupd_reverse_register.case.json` is a handwritten schema-v2 XMM8-to-XMM9 fixture for `s2-long-movupd-reg-reverse-v1`, encoded as `66 45 0F 11 C1`. Mandatory `66` selects packed SSE2 MOVUPD; ModR/M.reg plus REX.R selects source XMM8, while ModR/M.r/m plus REX.B selects destination XMM9. The literal source contains all 16 distinct bytes `808182838485868788898a8b8c8d8e8f`, including a distinguishable high half. The interpreter copies all 128 bits and preserves source and other CPU state. JIT/native admission deny the profile. It covers only register form, with no alignment or parent S2-34 acceptance claim.


### S2-34.17 local register XORPD diagnostic profile

`s2_34_17_xorpd_register.case.json` is a handwritten schema-v2 XMM8/XMM9 fixture for `s2-long-xorpd-reg-v1`, encoded as `66 45 0F 57 C1` (`XORPD XMM8,XMM9`). Mandatory `66` selects packed SSE2 XORPD; REX.R/B select high destination/source registers. The interpreter XORs all 128 raw bits, preserves MXCSR, x87, GPR, flags and unrelated XMM state, and performs no data-memory access. Native and JIT admission deny this local profile. It makes no physical execution or parent S2-34 acceptance claim. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-profile-s2-3417-s2-long-xorpd-reg-v1) and [Stage 2 plan](../../../../docs/plans/STAGE_2.md#s2-3417--local-only-register-xorpd).


### S2-34.18 local register ANDPD diagnostic profile

`s2_34_18_andpd_register.case.json` is a handwritten schema-v2 XMM8/XMM9 fixture for `s2-long-andpd-reg-v1`, encoded as `66 45 0F 54 C1` (`ANDPD XMM8,XMM9`). Mandatory `66` selects packed SSE2 ANDPD; REX.R/B select high destination/source registers. The interpreter ANDs all 128 raw bits, including alias/self forms in the unit matrix, preserves MXCSR, x87, GPR, flags and unrelated XMM state, and performs no data-memory access. Native and JIT admission deny this local profile. The fixture asserts no parent S2-34 acceptance or physical execution; physical remains `not_run`. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-profile-s2-3418-s2-long-andpd-reg-v1) and [Stage 2 plan](../../../../docs/plans/STAGE_2.md#s2-3418--local-only-register-andpd).

### S2-34.19 local register ORPD diagnostic profile

`s2_34_19_orpd_register.case.json` is a handwritten schema-v2 XMM8/XMM9 fixture for `s2-long-orpd-reg-v1`, encoded as `66 45 0F 56 C1` (`ORPD XMM8,XMM9`). Mandatory `66` selects packed SSE2 ORPD; REX.R/B select high destination/source registers. The interpreter ORs all 128 raw bits, including alias/self forms in the unit matrix, preserves MXCSR, x87, GPR, flags and unrelated XMM state, and performs no data-memory access. ORPS remains in its separate legacy profile. Native and JIT admission deny this local profile. The fixture asserts no parent S2-34 acceptance or physical execution; physical remains `not_run`. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-profile-s2-3419-s2-long-orpd-reg-v1) and [Stage 2 plan](../../../../docs/plans/STAGE_2.md#s2-3419--local-only-register-orpd).

### S2-34.20 local register ANDNPD diagnostic profile

`s2_34_20_andnpd_register.case.json` is a handwritten schema-v2 XMM8/XMM9 fixture for `s2-long-andnpd-reg-v1`, encoded as `66 45 0F 55 C1` (`ANDNPD XMM8,XMM9`). Mandatory `66` selects packed SSE2 ANDNPD; REX.R/B select high destination/source registers. Execution computes `(~old XMM8) & XMM9` across all 128 raw bits. The asymmetric literal operands distinguish this from `XMM8 & ~XMM9`; alias/self ANDNPD yields zero in the unit matrix. The interpreter preserves MXCSR, x87, GPR, flags and unrelated XMM state, and performs no data-memory access. Native and JIT admission deny this local profile. Parent S2-34 remains open; physical remains `not_run`. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-profile-s2-3420-s2-long-andnpd-reg-v1) and [Stage 2 plan](../../../../docs/plans/STAGE_2.md#s2-3420--local-only-register-andnpd).
## S2-34.21 — local MOVUPD `[RAX]` load diagnostic profile

`s2_34_21_movupd_memory_load_rax.case.json` is a handwritten schema-v2 fixture
for exact `66 44 0F 10 00` (`MOVUPD XMM8,[RAX]`). The source is unaligned and
contains distinct bytes; the interpreter copies the complete 16-byte value to
XMM8. The local diagnostic profile assumes SSE2/CR0/CR4 preconditions, permits
only the specified `[RAX]` encoding, and does not claim hardware alignment or
`#AC` behavior. JIT/native execution and physical validation are outside scope.

### S2-34.22 local MOVUPD `[RAX],XMM` store diagnostic profile

`s2_34_22_movupd_memory_store_rax.case.json` is a handwritten schema-v2
XMM8-to-`[RAX]` case encoded as `66 44 0F 11 00`. It verifies all 16 raw source
bytes are written to unaligned writable data, with exactly one data write and no
data reads. Interpreter runs the fixture; JIT and native profiles deny it. This
local profile makes no hardware atomicity, MMIO/SMP, `#AC`, fault-priority, or
arbitrary-backend rollback claim. Parent S2-34 remains open and physical status
is `not_run`.

### S2-34.23 local MOVUPD memory-base diagnostic profile

`s2_34_23_movupd_memory_base_load_r9_disp32.case.json` loads 16 unaligned raw
bytes from `[R9+1]` using `66 45 0F 10 81 01 00 00 00`; REX.R selects XMM8
and REX.B selects R9. `s2_34_23_movupd_memory_base_store_r8_disp8.case.json`
writes XMM8 to `[R8-7]` using `66 45 0F 11 40 F9`. Both use the distinct
`s2-long-movupd-mem-base-v1` profile and the shared memory operand/range path.
The runner checks raw state and canary bytes; JIT/native admission is denied.
This local slice does not establish hardware alignment/fault behavior or
arbitrary backend rollback. Parent S2-34 remains open; physical status is
`not_run`.

### S2-34.24 local MOVUPD RIP-relative diagnostic profile

`s2_34_24_movupd_rip_load.case.json` and
`s2_34_24_movupd_rip_store.case.json` encode the separate
`s2-long-movupd-mem-rip-v1` profile. They use mandatory `66`, REX.R to select
XMM8, ModRM.mod=00/rm=101 and a signed disp32 targeting mapped data. The decoder
uses the full instruction's next RIP; execution shares the existing 16-byte
memory path. JIT and native execution are denied. Physical behavior remains
unverified (`not_run`).


### S2-34.25 local MOVUPD SIB memory diagnostic profile

The separate `s2-long-movupd-mem-sib-v1` profile admits mandatory-66 MOVUPD
load/store with ModR/M.mod=00/01/10 and r/m=100, followed by SIB. Four
handwritten fixtures cover indexed XMM8 load `[R9+R10*2-1]`, indexed XMM8
store `[RAX+R12*8+1]`, and load/store with ModR/M.mod=00/SIB.base=5 signed
disp32 no-base addressing. The no-base encodings also set REX.B and demonstrate
that it is ignored; SIB.index=4 means no index when REX.X=0 and R12 when REX.X=1.
Raw payloads and store canaries are checked by the CLI runner. Testkit checks
exact encoded SIB classification and rejects the same bytes when assigned to
older `[RAX]`, base, or RIP-relative MOVUPD profiles. MOVUPS retains its own
SIB profile in S2-34.6c; this mandatory-66 MOVUPD profile does not alter it.
JIT/native admission is denied; physical status remains `not_run`.

### S2-35.53 local register PAVGW diagnostic profile

`s2_35_53_pavgw_register.case.json` is a handwritten high-XMM fixture for `s2-long-pavgw-reg-v1`, encoded as `66 45 0F E3 C1` (`PAVGW XMM8, XMM9`). It checks rounded unsigned 16-bit lane averages with boundary values and an unrelated-state preservation oracle. SSE2 and CR0/CR4 are assumed; it is local-only with no JIT/native/physical admission.

### S2-35.54 local register PMAXUB diagnostic profile

`s2_35_54_pmaxub_register.case.json` is a handwritten high-XMM fixture for `s2-long-pmaxub-reg-v1`, encoded as `66 45 0F DE C1` (`PMAXUB XMM8, XMM9`). It checks unsigned byte maxima across sign-boundary values and preserves unrelated state. SSE2 and CR0/CR4 are assumed; JIT/native/physical admission remain closed.

### S2-35.55 local register PMINUB diagnostic profile

`s2_35_55_pminub_register.case.json` is a handwritten high-XMM fixture for `s2-long-pminub-reg-v1`, encoded as `66 45 0F DA C1` (`PMINUB XMM8, XMM9`). It checks unsigned byte minima across sign-boundary values and preserves unrelated state. SSE2 and CR0/CR4 are assumed; JIT/native/physical admission remain closed.

### S2-35.56 local register PMAXSW diagnostic profile

`s2_35_56_pmaxsw_register.case.json` is a handwritten high-XMM fixture for `s2-long-pmaxsw-reg-v1`, encoded as `66 45 0F EE C1` (`PMAXSW XMM8, XMM9`). It checks signed 16-bit maxima across positive/negative boundary values and preserves unrelated state. SSE2 and CR0/CR4 are assumed; JIT/native/physical admission remain closed.

### S2-35.57 local register PMINSW diagnostic profile

`s2_35_57_pminsw_register.case.json` is a handwritten high-XMM fixture for `s2-long-pminsw-reg-v1`, encoded as `66 45 0F EA C1` (`PMINSW XMM8, XMM9`). It checks signed 16-bit minima across positive/negative boundary values and preserves unrelated state. SSE2 and CR0/CR4 are assumed; JIT/native/physical admission remain closed.

### S2-35.58 local register PMULLW diagnostic profile

`s2_35_58_pmullw_register.case.json` is a handwritten high-XMM fixture for `s2-long-pmullw-reg-v1`, encoded as `66 45 0F D5 C1` (`PMULLW XMM8, XMM9`). It checks signed word products with low-16-bit truncation and preserves unrelated state. SSE2 and CR0/CR4 are assumed; JIT/native/physical admission remain closed.

### S2-35.59 local register PMULHUW diagnostic profile

`s2_35_59_pmulhuw_register.case.json` is a handwritten high-XMM fixture for `s2-long-pmulhuw-reg-v1`, encoded as `66 45 0F E4 C1` (`PMULHUW XMM8, XMM9`). It checks high-half unsigned word products, including wrap boundaries, and preserves unrelated state. SSE2 and CR0/CR4 are assumed; JIT/native/physical admission remain closed.

### S2-35.60 local register PSADBW diagnostic profile

`s2_35_60_psadbw_register.case.json` is a handwritten high-XMM fixture for `s2-long-psadbw-reg-v1`, encoded as `66 45 0F F6 C1` (`PSADBW XMM8, XMM9`). It checks separate 64-bit byte-distance sums and zeroed upper bits in each half while preserving unrelated state. SSE2 and CR0/CR4 are assumed; JIT/native/physical admission remain closed.

### S2-35.61 local XMM-source PMOVMSKB diagnostic profile

`s2_35_61_pmovmskb_xmm_gpr.case.json` is a handwritten fixture for `s2-long-pmovmskb-xmm-gpr-v1`, encoded as `66 45 0F D7 C1` (`PMOVMSKB R8D, XMM9`). It checks byte-order sign-mask extraction, 32-bit zero extension, and unrelated-state preservation. The local subset permits REX.R/B, rejects REX.W/X and memory forms, and keeps JIT/native/physical admission closed.

### S2-35.62 local register PMULUDQ diagnostic profile

`s2_35_62_pmuludq_register.case.json` is a handwritten high-XMM fixture for `s2-long-pmuludq-reg-v1`, encoded as `66 45 0F F4 C1` (`PMULUDQ XMM8, XMM9`). It uses independent odd-dword sentinels to verify that only lanes 0 and 2 are multiplied and that both complete qword products are stored. SSE2 and CR0/CR4 are assumed; JIT/native/physical admission remain closed.

### S2-35.63 local register PMADDWD diagnostic profile

`s2_35_63_pmaddwd_register.case.json` is a handwritten high-XMM fixture for `s2-long-pmaddwd-reg-v1`, encoded as `66 45 0F F5 C1` (`PMADDWD XMM8, XMM9`). The inputs cover signed products, adjacent pair addition, the two-product signed overflow corner, and independent high-register extension. It verifies 32-bit wrapping and unrelated-state preservation. The local profile accepts only register form with optional REX.R/B; SSE2 and CR0/CR4 are assumed, while JIT/native/physical admission remains closed.

### S2-37e local masked-status MINSS register profile

`s2_37e_minss_masked_register.case.json` is a handwritten schema-v2 XMM8←XMM9
fixture for `F3 45 0F 5D C1`. The source low lanes are QNaN and the smallest
positive subnormal. MINSS selects operand 2 and sets IE; invalid has priority
over denormal in this scalar operation, so MXCSR becomes `0x1f81`. The upper destination bits and
other state are preserved. The local contract requires all six exception masks,
DAZ/FTZ clear and reserved MXCSR bits clear. Normative references are Intel SDM
Rev. 093 Vol. 1 §§4.9.2, 11.5, Appendix D Tables D-9/D-15 and Vol. 2B MINSS. This fixture
does not claim unmasked exception or fault-ordering behavior; JIT/native remain
closed. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37e--local-long64-register-only-legacy-minss-with-masked-status).

### S2-37f local masked-status MAXSS register profile

`s2_37f_maxss_masked_register.case.json` is a handwritten schema-v2 XMM8←XMM9
fixture for `F3 45 0F 5F C1`. The source low lanes are 1.0 and 2.0; MAXSS
selects operand 2 while MXCSR and all other state remain unchanged. The local
contract requires all six MXCSR exception masks, DAZ/FTZ
clear and reserved MXCSR bits clear. Normative references are Intel SDM Rev.
093 Vol. 1 §11.5, Appendix D Tables D-9/D-13/D-15 and Vol. 2B MAXSS. Unmasked
exception and fault-ordering behavior is not claimed; JIT/native remain closed.
See the [plan](../../../../docs/plans/STAGE_2.md#s2-37f--local-long64-register-only-legacy-maxss-with-masked-status).

### S2-37g local masked-status MINPS register profile

`s2_37g_minps_masked_register.case.json` is a handwritten schema-v2 XMM8←XMM9
fixture for `45 0F 5D C1`. It mixes finite, subnormal-without-NaN,
QNaN/subnormal and infinity/SNaN lanes. Invalid suppresses new DE within its
lane; the separate subnormal lane contributes DE, so the fixture expects MXCSR
IE|DE (`0x1f80` to `0x1f83`). Dedicated unit tests verify the same-lane
priority directly. Expected result bytes preserve raw RHS NaN bits. The interpreter
test also checks full state preservation and the profile remains JIT/native
closed. MXCSR preconditions are all exception masks set, DAZ/FTZ clear and
reserved bits clear. Normative references are Intel SDM Rev. 093 Vol. 1 §§4.9.2, 11.5.2,
11.5.3.1, Appendix D Tables D-9/D-13/D-15, and Vol. 2B MINPS. This is masked-status local
coverage only; unmasked #XM delivery and fault ordering are not claimed. See
the [plan](../../../../docs/plans/STAGE_2.md#s2-37g--local-long64-register-only-legacy-minps-with-masked-status).

### S2-37h local masked-status MAXPS register profile

`s2_37h_maxps_masked_register.case.json` is a handwritten schema-v2 XMM8←XMM9
fixture for `45 0F 5F C1`. It mixes finite, subnormal-without-NaN,
QNaN/subnormal and infinity/SNaN lanes. Invalid suppresses new DE within its
lane; the separate subnormal lane contributes DE, so the fixture expects MXCSR
IE|DE (`0x1f80` to `0x1f83`). Dedicated unit tests verify the same-lane
priority directly. Expected result bytes preserve raw RHS NaN bits. The interpreter
test also checks full state preservation and the profile remains JIT/native
closed. MXCSR preconditions are all exception masks set, DAZ/FTZ clear and
reserved bits clear. Normative references are Intel SDM Rev. 093 Vol. 1 §§4.9.2, 11.5.2,
11.5.3.1, Appendix D Tables D-9/D-13/D-15, and Vol. 2B MAXPS. This is masked-status local
coverage only; unmasked #XM delivery and fault ordering are not claimed. See
the [plan](../../../../docs/plans/STAGE_2.md#s2-37h--local-long64-register-only-legacy-maxps-with-masked-status).


### S2-37i local masked-status MINSD register profile

`s2_37i_minsd_masked_register.case.json` is a handwritten schema-v2 XMM8←XMM9
fixture for `F2 45 0F 5D C1`. The low qword destination is QNaN and the low
qword source is the minimum positive subnormal; the fixture expects raw RHS in
the low qword, preserves destination upper-qword SNaN sentinel, and checks
MXCSR IE only (`0x1f80` to `0x1f81`) because same-operation invalid has
priority over denormal. A source upper-qword subnormal sentinel
must not affect the result or flags. The local profile requires all MXCSR
exception masks set, DAZ/FTZ clear and reserved bits clear. It is not a claim
about unmasked exception delivery, memory forms, JIT/native or physical CPU
acceptance. Normative references are Intel SDM Rev. 093 Vol. 1 §§4.9.2, 11.5, Appendix
D Tables D-9/D-15, and Vol. 2B MINSD. See the
[plan](../../../../docs/plans/STAGE_2.md#s2-37i--local-long64-register-only-legacy-minsd-with-masked-status).


### S2-37j local masked-status MAXSD register profile

`s2_37j_maxsd_masked_register.case.json` is a handwritten schema-v2 XMM8←XMM9
fixture for `F2 45 0F 5F C1`. It pairs a low-qword QNaN destination with a
minimum positive subnormal source, requiring raw RHS selection and IE-only
status because same-operation invalid has priority over denormal. Upper-qword
SNaN/subnormal sentinels are ignored and preserved. All MXCSR exception masks
are set, DAZ/FTZ and reserved bits are clear. JIT/native and physical execution
are not claimed; physical acceptance is deferred to the combined final Mac mini
substage. Normative references: Intel SDM Rev. 093 Vol. 1 §§4.9.2, 11.5.2 and
Appendix D Tables D-9/D-13/D-15, and Vol. 2B MAXSD. See the
[plan](../../../../docs/plans/STAGE_2.md#s2-37j--local-long64-register-only-legacy-maxsd-with-masked-status).


### S2-37k local masked-status MINPD register profile

`s2_37k_minpd_masked_register.case.json` is a handwritten schema-v2 XMM8←XMM9
fixture for `66 45 0F 5D C1`. Lane 0 pairs a destination QNaN with the
minimum positive subnormal and sets IE without DE. Lane 1 pairs 1.0 with a
minimum positive subnormal and sets DE; aggregate sticky status is IE|DE.
Both results select raw operand 2.
The fixture uses all MXCSR exception masks,
DAZ/FTZ clear and reserved bits clear, with RC 0. JIT/native and physical
execution are not claimed; physical acceptance is deferred to the combined
final Mac mini substage. Normative references: Intel SDM Rev. 093 Vol. 1
§§4.9.2, 11.5.2 and Appendix D Tables D-9/D-13/D-15, and Vol. 2B MINPD. See
the [plan](../../../../docs/plans/STAGE_2.md#s2-37k--local-long64-register-only-legacy-minpd-with-masked-status).

### S2-37l local masked-status MAXPD register profile

`s2_37l_maxpd_masked_register.case.json` is a handwritten schema-v2 XMM8←XMM9
fixture for `66 45 0F 5F C1`. Lane 0 pairs a destination QNaN with the
minimum positive subnormal and sets IE without DE. Lane 1 pairs 1.0 with a
minimum positive subnormal and sets DE; aggregate sticky status is IE|DE.
Lane 0 selects raw operand 2; lane 1 retains the larger destination operand.
The fixture uses all MXCSR exception masks,
DAZ/FTZ clear and reserved bits clear, with RC 0. JIT/native and physical
execution are not claimed; physical acceptance is deferred to the combined
final Mac mini substage. Normative references: Intel SDM Rev. 093 Vol. 1
Appendix D Tables D-9/D-13/D-15 and Vol. 2B MAXPD. See the
[plan](../../../../docs/plans/STAGE_2.md#s2-37l--local-long64-register-only-legacy-maxpd-with-masked-status).


### S2-39b selector state snapshot

`s2_39b_x87_selector_state_snapshot.case.json` is a handwritten schema-v2
fixture for `s2-long-x87-selectors-state-v1`. It transfers one NOP while
retaining distinct nonzero instruction/data selectors, full-width FIP/FDP
sentinels, raw x87 registers/tags/opcode, and all XMM/MXCSR state. It proves
local representation and snapshot preservation only; no FXSAVE/FXRSTOR
execution, native observation, or physical/Mac mini behavior is claimed. The
older profiles continue to omit selector fields and reject them if supplied.
See the [S2-39b plan](../../../../docs/plans/STAGE_2.md#s2-39b-lossless-selector-state-representation-no-fxsave-execution),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-39b--selector-state-representation-only),
and [Stage 2 report](../../../../docs/reports/STAGE_2_REPORT.md#s2-39b--selector-state-representation-only).

### S2-39c plain Long64 RIP-relative FXSAVE

`s2_39c_fxsave_rip_relative_literal.case.json` is a handwritten schema-v2
fixture for `s2-long-fxsave-rip-v1`. It carries the observed Linux bytes
`0F AE 05 6B F9 17 00`, both known selectors, raw x87/SSE state, and an aligned
512-byte destination prefilled with `A5` sentinels. The profile writes only the
13 defined-field slices; holes and reserved/software bytes retain their input
sentinels. The local interpreter test checks the literal output and unchanged
CPU state apart from normal RIP retirement. A separate core test executes a
different disp32, covers all TOP positions, exact callbacks and injected write
failures. This is local interpreter/profile evidence only; no physical CPU,
Mac mini, guest exception delivery, FXRSTOR, or JIT/native result is claimed.
See the [S2-39c plan](../../../../docs/plans/STAGE_2.md#s2-39c-exact-plain-long64-rip-relative-fxsave),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-39c--plain-long64-rip-relative-fxsave),
and [Stage 2 report](../../../../docs/reports/STAGE_2_REPORT.md#s2-39c--plain-long64-rip-relative-fxsave).

### S2-39d Long64 FXSAVE64 `[RAX+0x40]`

`s2_39d_fxsave64_rax_disp8_literal.case.json` is a handwritten schema-v2
fixture for `s2-long-fxsave64-rax-disp8-v1`. It contains the exact bytes
`48 0F AE 40 40`, raw x87/SSE state with selectors absent, high-width FIP/FDP,
and an aligned 512-byte destination prefilled with `A5` sentinels. The local
profile writes the full pointers at `[8,16)` and `[16,24)` using the same
13-callback contract as S2-39c; reserved and software-available bytes remain
preserved. Core tests also cover known selectors, all TOP values and callback
failures. This is local interpreter evidence only: it does not claim physical
execution, native/JIT support, FXRSTOR, or round-trip behavior. See the
[S2-39d plan](../../../../docs/plans/STAGE_2.md#s2-39d-exact-long64-fxsave64-rax0x40),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-39d--exact-long64-fxsave64-rax0x40),
and [Stage 2 report](../../../../docs/reports/STAGE_2_REPORT.md#s2-39d--exact-long64-fxsave64-rax0x40).

### S2-39e Long64 FXRSTOR64 `[RBX+0x40]`

`s2_39e_fxrstor64_rbx_disp8_literal.case.json` is a handwritten schema-v2
fixture for `s2-long-fxrstor64-rbx-disp8-v1`. It carries the exact bytes
`48 0F AE 4B 40`, a literal read-only 512-byte image, full 64-bit pointers,
abridged physical FTW, TOP-ordered raw x87 slots, nonzero ignored/reserved
bytes, and all sixteen XMM images. It does not derive the input from the local
FXSAVE serializer. Local interpreter tests cover all TOP values, independent
optional selectors, x87 tag classes and raw payloads, target MXCSR validation
with forged-mask invariance, 13 ascending read-only callbacks, every callback
failure, range preflight, wrapping RBX arithmetic, and save/mutate/restore for
representable state. JIT/native and physical execution are not claimed. See
[S2-39e plan](../../../../docs/plans/STAGE_2.md#s2-39e-exact-long64-fxrstor64-rbx0x40),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-39e--exact-long64-fxrstor64-rbx0x40),
and [Stage 2 report](../../../../docs/reports/STAGE_2_REPORT.md#s2-39e--exact-long64-fxrstor64-rbx0x40).

### S2-39f Long64 FXSAVE64 CR0 EM and TS gates

`s2_39f_fxsave64_cr0_em_nm.case.json` and
`s2_39f_fxsave64_cr0_clear_success.case.json` are hand-authored schema-v2
fixtures for `s2-long-fxsave64-cr0-gates-v1`, both using exact bytes
`48 0F AE 40 40` and a valid aligned canonical writable 512-byte private RAM
mapping. The EM fixture expects CPU-side vector 7 with no error code, zero
retirement and unchanged complete CPU/RAM snapshots. The success fixture seeds
an unmasked pending x87 invalid exception (`FSW.ES/IE`, `FCW.IE=0`) and MXCSR
sticky flags; its output is compared byte-for-byte to the independent literal
512-byte image `s2_39f_fxsave64_expected_image.hex`, including preserved holes.
Tests cover all four EM/TS combinations at CPL0–3, exact fault state and zero
data callbacks, full instruction fetch before gates, STOP/zero-budget precedence,
strict control schema/result comparison and continued S2-39d behavior. Intel
SDM Rev. 093 Vol. 2A FXSAVE 64-Bit Mode Exceptions pp. 3-418–3-419 lists #NM
for either CR0 bit; Vol. 3A Event 7 pp. 7-33–7-34 specifies no error code and
no program-state change for this fault. Sources: [Vol. 2A PDF](https://cdrdv2.intel.com/v1/dl/getContent/671199),
[Vol. 3A PDF](https://cdrdv2.intel.com/v1/dl/getContent/671190). These fixtures
do not test simultaneous target data mapping faults with CR0 gates, FXSR=0,
OSFXSR=0, hardware equivalence, guest exception delivery, JIT/native/C adapter,
or physical execution. Parent S2-39 and Generic amd64 acceptance remain open.
See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-39f--bounded-fxsave64-cr0-em-and-ts-nm-gates),
[plan](../../../../docs/plans/STAGE_2.md#s2-39f-bounded-fxsave64-cr0-em-and-ts-nm-gates),
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-39f--bounded-fxsave64-cr0-em-and-ts-nm-gates),
and [fixture format](../../../../docs/testing/TEST_CASE_FORMAT.md#s2-39f-fxsave64-cr0-gate-profile).

### S2-39g Long64 FXRSTOR64 CR0 EM and TS gates

`s2_39g_fxrstor64_cr0_em_nm.case.json` and
`s2_39g_fxrstor64_cr0_clear_success.case.json` are hand-authored schema-v2
fixtures for `s2-long-fxrstor64-cr0-gates-v1`, with exact bytes
`48 0F AE 4B 40` and explicit immutable `cr0_em`/`cr0_ts` controls. Both use
the same independently authored literal 512-byte readable private image with
valid MXCSR, ignored forged MXCSR_MASK and nonzero padding. The initial x87/SSE
state in the success fixture differs from the image, including a pending x87
exception flag and MXCSR sticky flags. At either CR0 gate the expected result is
vector 7 with no error code, zero retirement and complete unchanged CPU/RAM
snapshots. With both bits clear, the existing S2-39e staged restore semantics
apply. Core tests cover all four EM/TS combinations at CPL0–3 and verify zero
read callbacks on #NM; clear controls verify the 13 reads and committed pending
status as data. Fixture validation restricts the profile to aligned canonical48
readable private memory and valid MXCSR, so combined gate plus malformed-image
or mapping-fault precedence is not claimed. Fetch order and STOP/budget order
are covered. JIT/native/C adapter remain denied; physical execution is deferred.
See the [S2-39g plan](../../../../docs/plans/STAGE_2.md#s2-39g-bounded-fxrstor64-cr0-em-and-ts-nm-gates),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-39g--bounded-fxrstor64-cr0-em-and-ts-nm-gates),
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-39g--bounded-fxrstor64-cr0-em-and-ts-nm-gates),
and [fixture format](../../../../docs/testing/TEST_CASE_FORMAT.md#s2-39g-fxrstor64-cr0-gate-profile).

### S2-39h FP-init plus FXSAVE64 composition

`s2_39h_cr0_cr4_fxsave64.case.json` is a handwritten schema-v2 fixture for
`s2-long-cr0-cr4-fp-init-fxsave64-v1`. It admits exactly the S2-25c 28-byte
CR0/FNINIT/CR4/XORPD program followed by `48 0F AE 40 40`, for nine total
retirements. The initial state fixes RAX to `0x620`; aligned private writable
RAM at `0x660..0x85f` starts with `0xa5` sentinels. The runner test exercises
all 16 initial CR0.EM/TS × CR4.OSFXSR/OSXMMEXCPT values. OSFXSR clear is
accepted initially because the exact earlier sequence sets it before FXSAVE;
the profile says nothing about OSFXSR clear at the FXSAVE itself.

`s2_39h_fxsave64_expected_image.hex` is independently authored from the
fixture's expected post-FNINIT/XORPD state. Tests compare defined image fields
and verify bytes 464–511 are unchanged, while skipping reserved/undefined
ranges. Execution tests cover STOP, zero budget, partial retirement before a
truncated final instruction, and injected failure at each of the serializer's
13 callbacks. Earlier private RAM writes can remain after callback failure;
this does not claim whole-memory rollback. Interpreter only; JIT/native/C/
fragment and physical execution are not claimed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-39h--exact-fp-init-plus-fxsave64-composition),
[plan](../../../../docs/plans/STAGE_2.md#s2-39h-exact-fp-initialization-composition-followed-by-fxsave64),
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-39h--exact-fp-init-plus-fxsave64-composition),
and [fixture format](../../../../docs/testing/TEST_CASE_FORMAT.md#s2-39h-fp-init-plus-fxsave64-profile).

### S2-39i FP-init plus FXRSTOR64 composition

`s2_39i_cr0_cr4_fxrstor64.case.json` is a handwritten schema-v2 fixture for
`s2-long-cr0-cr4-fp-init-fxrstor64-v1`. It appends exact `48 0F AE 4B 40`
to S2-25c for 33 bytes and nine retirements. RBX=`0x620` addresses a private,
aligned readable 512-byte literal image at `0x660`; the image is independently
authored and does not come from the preceding FP-init sequence. Its varied
raw x87 payloads, nonzero TOP, MXCSR, and XMM values replace the state left by
FNINIT/XORPD, including XMM0. Nonzero ignored MXCSR_MASK and reserved/padding/
software bytes exercise defined-image handling.

Runner tests cover all 16 initial CR0/CR4 pairs, restored full pointers, TOP and
abridged-tag reconstruction, selectors, all XMM registers, target MXCSR
validation, ignored fields, unchanged image, and strict result/schema tampering.
Core executor tests inject failure at each of 13 read callbacks and verify the
final instruction does not retire or commit CPU state while the preceding eight
instructions remain retired. STOP, budget exhaustion and truncated fetch do
not read the image. Hardware and physical execution are not claimed. See the
[plan](../../../../docs/plans/STAGE_2.md#s2-39i-exact-fp-initialization-composition-followed-by-fxrstor64),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-39i--exact-fp-init-plus-fxrstor64-composition),
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-39i--exact-fp-init-plus-fxrstor64-composition),
and [fixture format](../../../../docs/testing/TEST_CASE_FORMAT.md#s2-39i-fp-init-plus-fxrstor64-profile).

### S2-40b native XORPD XMM0 XMM1

`s2_40b_xorpd_xmm0_xmm1_asymmetric_a.case.json` and
`s2_40b_xorpd_xmm0_xmm1_asymmetric_b.case.json` are handwritten one-instruction
fixtures for `s2-long-native-xorpd-xmm0-xmm1-v1`. Each contains exact bytes
`66 0F 57 C1`, asymmetric byte patterns in XMM0/XMM1, unique sentinels in
XMM2–XMM15, full GPR state, defined integer flags, and MXCSR `0x1f80`. No x87
state is serialized. `tools/runner/tests/fixture_v2.rs` independently records
the expected XMM0 byte strings and checks preservation of the remaining 240
XMM bytes, MXCSR, GPRs, flags and RIP against the interpreter result. The
native transport preserves the existing supervised child and deadline model,
installs/readbacks only XMM and MXCSR with Linux ptrace FP-register requests,
and keeps integer and SSE observations separate. No physical result is claimed;
hardware status is `not_run`. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-40b--bounded-sse-only-native-xorpd-oracle),
[plan](../../../../docs/plans/STAGE_2.md#s2-40b-узкий-sse-only-native-xorpd-oracle),
and [Linux profile report](../../../../docs/reports/STAGE_2_LINUX_PROFILE.md#s2-40b--ограниченный-sse-only-native-xorpd-oracle).

### S2-38b local XORPD prerequisite gates

`s2_38b_xorpd_sse2_absent.case.json` is a handwritten schema-v2 CPU fault
fixture for the exact `66 0F 57 C1` form and profile
`s2-long-xorpd-prerequisites-v1`. Its explicit control snapshot selects
SSE2-unavailable and CR0.TS=1 together, demonstrating #UD priority over #NM.
The fixture keeps complete x87 and SSE state sentinels so the CPU-side fault
can be compared without partial-state assumptions. Core unit coverage runs all
16 combinations of SSE2 availability, CR0.EM, CR0.TS and CR4.OSFXSR at CPL0–3;
it checks #UD/#NM priority, the successful bytewise XOR, zero retirement and
full state preservation on faults, and zero data callbacks. Neighboring
encodings, truncation, missing SSE snapshot, STOP and zero-budget ordering are
covered locally. JIT/native remain denied and no physical execution is claimed.
The v2 result echoes all four controls in `snapshot.xorpd_prerequisites`; the
comparator rejects differing oracle controls and checks them against the input
fixture. Older profiles omit this optional result field.
See [S2-38b plan](../../../../docs/plans/STAGE_2.md#s2-38b-exact-xorpd-sse2-prerequisite-gates),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38b--точечные-sse2-prerequisite-gates-для-xorpd),
and [Stage 2 report](../../../../docs/reports/STAGE_2_REPORT.md#s2-38b--точечные-sse2-prerequisite-gates-для-xorpd).

### S2-38c local MINSS unmasked invalid operation

`s2_38c_minss_unmasked_invalid.case.json` is a handwritten schema-v2 fixture
for exact bytes `F3 0F 5D C1` and profile
`s2-long-minss-invalid-controls-v1`. XMM0 contains a QNaN, XMM1 a positive
subnormal, IM is clear and OSXMMEXCPT is clear. The expected arithmetic #UD
sets MXCSR.IE while preserving the complete XMM0 destination, RIP and
retirement count; invalid takes priority over denormal. Controls are explicit
and echoed by the result snapshot. Local unit coverage checks prerequisite
gates at CPL0–3 and both arithmetic #UD/#XM routes. JIT/native remain denied.
Physical/Mac mini remains `not_run` until the combined final hardware stage.
See the [S2-38c plan](../../../../docs/plans/STAGE_2.md#s2-38c-exact-long64-minss-xmm0xmm1-unmasked-invalid-operation-exit),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38c--exact-minss-unmasked-invalid-operation-exit),
and [Stage 2 report](../../../../docs/reports/STAGE_2_REPORT.md#s2-38c--exact-minss-unmasked-invalid-operation-exit).

### S2-38d local MINSS unmasked denormal operand

The new versioned profile `s2-long-minss-unmasked-denormal-v1` adds only
unmasked denormal handling for exact Long64 bytes `F3 0F 5D C1` and XMM0/XMM1.
Its explicit controls and fixed CR0.MP=1 are the same as S2-38c, while
S2-38c's fixture bytes and MXCSR policy are unchanged. The profile permits IM
and DM independently, any RC and prior sticky bits, but requires ZE/OE/UE/PE
masks set, DAZ/FTZ clear and reserved bits zero.

Four handwritten cases cover unmasked DE through #UD with preserved old IE/ZE
flags (`s2_38d_minss_denormal_unmasked_ud`, final MXCSR `0x1e07`), unmasked DE
through #XM with prior OE/UE (`s2_38d_minss_denormal_unmasked_xm`, final MXCSR
`0x1e1a`), masked QNaN-invalid followed by unmasked DE (`s2_38d_minss_masked_invalid_unmasked_denormal`,
final MXCSR `0x1e83`), and an exception-free finite result. Arithmetic faults
preserve the entire XMM0 state, RIP and zero retirement. Local unit coverage
crosses QNaN/SNaN, operand order, IM, DM and OSXMMEXCPT. It records IE|DE when
either mask is open and only IE when both are masked, following Intel SDM
Rev. 093 Vol. 1 §§4.9.2, 11.5.3.2–3 and Vol. 3A Table 7-8. The selected
unmasked pre-computation exception produces one #UD/#XM exit; no second
delivery is modeled.

The profile remains interpreter-only. JIT, native, C adapter, MAXSS, packed
forms, memory operands, DAZ/FTZ and other register pairs remain closed.
Physical/Mac mini status is `not_run` until the combined final hardware stage.
See the [S2-38d plan](../../../../docs/plans/STAGE_2.md#s2-38d-exact-long64-minss-xmm0xmm1-unmasked-denormal-operand-exit),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38d--exact-minss-unmasked-denormal-operand-exit),
and [Stage 2 report](../../../../docs/reports/STAGE_2_REPORT.md#s2-38d--exact-minss-unmasked-denormal-operand-handling).

### S2-38e FP-init, FXRSTOR64, and MINSS

The new exact local profile appends `F3 0F 5D C1` (`MINSS XMM0,XMM1`) to the
S2-39i 33-byte FP-init plus FXRSTOR64 composition. The resulting program is 37
bytes and completes with ten retirements. The handwritten image is preserved
as [s2_38e_fp_init_fxrstor64_minss_image.hex](s2_38e_fp_init_fxrstor64_minss_image.hex);
restored XMM0 and XMM1 operands differ from pre-restore state, and image MXCSR
supplies IM/DM behavior.

The schema admits the S2-38d MXCSR domain: IM/DM vary, other masks are set,
DAZ/FTZ and reserved bits are clear, while RC and old sticky flags are
arbitrary. Literal fixture pairs
`s2_38e_fp_init_fxrstor64_minss`/`s2_38e_minss_masked_qnan` and
`s2_38e_unmasked_subnormal`/`s2_38e_masked_subnormal` differ only in the
corresponding image IM or DM bit; each pair represents masked completion and
#XM. Coverage includes
sticky-status combinations and all 16 starting CR0.EM/TS ×
CR4.OSFXSR/OSXMMEXCPT combinations on representative success/exception cases.
Completion preserves XMM0 upper 96 bits; #XM preserves the full restored XMM0
and commits only applicable IE/DE. The complete literal image stays unchanged.
Boundary STOP/budget after restore, final fetch failure and staged restore
errors prevent MINSS from executing. Schema/result tampering is rejected and
the native backend denies this profile. MINSS prerequisite gates use the live
post-init CR0/CR4 state without redundant explicit gate fields. This is a
synthetic local composition; no Linux/Ubuntu runtime or physical hardware
observation was made. See the
[plan](../../../../docs/plans/STAGE_2.md#s2-38e-exact-fp-init-fxrstor64-and-image-controlled-minss-composition),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38e--exact-fp-init-fxrstor64-minss-composition),
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-38e--exact-fp-init-fxrstor64-and-image-controlled-minss-composition),
and [format](../../../../docs/testing/TEST_CASE_FORMAT.md#s2-38e-s2-long-cr0-cr4-fp-init-fxrstor64-minss-v1).

### S2-38f local MAXSS unmasked invalid and denormal

The separate profile `s2-long-maxss-unmasked-denormal-v1` admits only exact
Long64 `F3 0F 5F C1` and uses its own five-field immutable
`maxss_unmasked_controls` object. Three fault fixtures cover qNaN plus positive
minimum subnormal in both operand positions and #UD/#XM selection; they expect
IE|DE (`MXCSR 0x1e83`) and preserve complete XMM0/RIP with zero retirement. A
fourth literal fixture masks IM and DM: MAXSS completes, returns raw operand 2
(the subnormal), sets IE only (`MXCSR 0x1f81`) and preserves XMM0 upper 96 bits.

The review is specific to MAXSS. Intel SDM Rev. 093 Vol. 2B pp.4-13–4-14
explicitly lists invalid including QNaN and denormal, and describes operand-2
selection for NaNs, equal values and signed-zero ties, including raw SNaN
return. Vol.1 §§4.9.2 and 11.5.3.2–3 give invalid priority and mixed-mask
sticky behavior: both masks set on NaN+subnormal completes with IE only; any
open mask commits IE|DE and exits once. Unit tests cross QNaN/SNaN, both operand
positions, IM×DM×OSXMMEXCPT, prerequisite gates, CPL0–3, fetch/STOP/budget,
truncation, exact-form rejection, full preservation and raw MAX completion
values. JIT/native/C adapters remain denied; physical/Mac mini validation is
deferred to the combined final hardware substage. See the [plan](../../../../docs/plans/STAGE_2.md#s2-38f-exact-long64-maxss-xmm0xmm1-unmasked-invalid-and-denormal-exit),
[contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38f--exact-maxss-unmasked-invalid-and-denormal-exit),
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-38f--exact-long64-maxss-with-bounded-unmasked-imdm-exits),
and [format](../../../../docs/testing/TEST_CASE_FORMAT.md#s2-38f-exact-maxss-unmasked-imdm-profile).


### S2-42a native XORPD generated corpus

The existing `s2-long-native-xorpd-xmm0-xmm1-v1` profile now has a local,
versioned deterministic corpus of 1024 generated cases (4 fixed seeds × 256
sequences). Generated cases are written only under ignored
`work/stage2/s2-42a-xorpd/corpus/`; the ordered manifest and local result report
are stored beside that directory. `od-runner --generate-native-xorpd-corpus`
creates them, and `od-runner --verify-native-xorpd-corpus` checks strict schema,
digests, missing/duplicate inputs, two interpreter runs per case, the
independent raw-byte XOR result, full XMM and integer snapshots, MXCSR, code
immutability, and exit/retirement. A saved case is replayed with the normal
`od-runner --fixture CASE.json --engine interpreter --json` command.

Handwritten runner checks cover zero/ones, alternating, equal operands and
preservation of XMM1–XMM15, integer state and code bytes. These are deterministic
local interpreter checks; native/hardware output remains `not_run` until the
combined final hardware batch. See the [S2-42a plan](../../../../docs/plans/STAGE_2.md#s2-42a-deterministic-corpus-для-native-xorpd),
[test format](../../../../docs/testing/TEST_CASE_FORMAT.md#s2-42a-native-xorpd-generated-corpus),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-42a--generated-corpus-для-native-xorpd).

### S2-42b native XORPD XMM-input reducer

`od-runner --reduce-native-xorpd CASE.json [--limit N] [--output-dir
work/stage2/s2-42b-xorpd/NAME]` accepts one fixture from the existing
`s2-long-native-xorpd-xmm0-xmm1-v1` profile. It preserves the source and writes
an isolated, exclusive result folder under ignored `work/stage2/`. The report
contains original/reduced hashes, repeated interpreter results, expected
bytewise-XOR oracle results, mismatch byte-position fingerprint, run counts,
bit costs, provenance, revision/toolchain and replay commands. If the local
interpreter initially matches its independent oracle, the report says
`no_initial_mismatch` and no reduced fixture is written. `native=not_run`; the
synthetic callbacks used by unit tests are algorithm checks only, not evidence
of a physical CPU bug. The one-pass reducer is not globally minimal. See the
[S2-42b plan](../../../../docs/plans/STAGE_2.md#s2-42b-bounded-xmm-input-reducer),
[format and replay instructions](../../../../docs/testing/TEST_CASE_FORMAT.md#s2-42b-bounded-native-xorpd-xmm-input-reducer),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-42b--bounded-native-xorpd-xmm-input-reducer).

## S2-26f `FsBaseV1` MSR fixture

`s2_26f_fs_base_rdmsr.case.json` and
`s2_26f_fs_base_wrmsr_fs_load.case.json` are handwritten schema-v2 fixtures.
The first checks a canonical high-half RDMSR, zero-extension of EAX/EDX, and
ignored high RCX bits in index selection. The second covers exact
`0F 30 64 48 8B 00`, budget 2. Its first instruction writes
`IA32_FS_BASE` from low EDX:EAX; the second reads the eight-byte value at
`fs_base + RAX`. The literal expected value comes from the mapped data bytes.
The schema admits only exact bare `0F 32` or `0F 30` at budget 1, or this exact
write-then-load composition at budget 2. No native, JIT, C ABI, Linux runtime or
physical execution is claimed.

## S2-26g `KernelGsBaseV1` MSR/SWAPGS fixtures

The four handwritten `s2_26g_kernel_gs_base_*.case.json` cases use the
explicit `s2-long-msr-kernel-gs-base-v1` profile. It requires present
canonical48 `gs_base` and `kernel_gs_base` values and admits only bare
`0F 32`/`0F 30`, `0F 30 0F 01 F8 0F 32`, and
`0F 30 0F 01 F8 65 48 8B 00`, with budgets 1/1/3/3. RDMSR proves low-half
zero extension; WRMSR uses dirty upper GPR halves; the composition writes
`0x1000` to kernel GS base and swaps it into active GS. The RDMSR case then
reads former active GS base `0x3000`. The GS-load composition keeps distinct
literal qwords at `0x2000` and `0x4000`, so the load at active base `0x1000`
plus RAX `0x1000` identifies the exchanged base. CPL, canonical edges,
noncanonical writes, absent state, bounded fetch/stop/fault behavior and
late-fault commit are covered by local tests. JIT, native, and C/fragment
admission remain closed; no physical/Linux runtime result is claimed. Sources:
Intel SDM Rev. 093 `RDMSR`/`WRMSR`, Vol. 3A §§2.8.10/4.5.3, and `SWAPGS`,
Vol. 2B pp. 4-697–4-698, including FRED restrictions. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#локальный-профиль-kernelgsbasev1-s2-26g).

## S2-27g exact Long64 SWAPGS fixture profile

`s2-long-swapgs-v1` is an explicit bounded interpreter profile for bare
`0F 01 F8`, with Long64, FRED disabled, canonical48 bases, and no VMX/SMM.
Its only admitted programs are `0F 01 F8` (budget 1), two repetitions (budget
2), and `0F 01 F8 65 48 8B 00` (budget 2). The final program loads through the
exchanged GS base. Distinct qwords at old/new bases identify the active base.
An injected second-instruction page fault separately checks that the SWAPGS
exchange remains committed, retirement is 1, and RIP points at instruction 2.
Fixture schema stores `kernel_gs_base` as an
optional state field; a `Some(0)` value is serialized, while old profiles omit
the field and retain their previous wire shape.

CPL0 executes the exchange; CPL1–3 produce #GP(0) before state/data access.
Missing profile or base is unsupported. Exact LOCK gets #UD after complete
fetch; other prefixes/neighbors are unsupported and truncation keeps fetch
priority. JIT, native runner, and fragment/C adapters deny SWAPGS. This fixture
profile does not establish unconditional Long64 support or physical/Linux
runtime behavior. Source: Intel SDM Rev. 093 Vol. 2B, SWAPGS pp. 4-697–4-698,
including CR4.FRED behavior. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-27g--exact-long64-swapgs).

## S2-27h exact Long64 CLI fixture profile

The 32 handwritten `s2_27h_cli_cpl*_iopl*_if*.case.json` cases use the
`s2-long-cli-v1` profile and exact bare `FA`, with instruction budget 1.
Each carries a numeric CPL input restricted to 0–3; other profiles cannot
carry that field. The fixed profile assumes Long64, CR0.PE=1, VM=0, CR4.PVI=0,
and no VMX/SMM. It reads IOPL from initial RFLAGS and compares the concrete
`INTEGER_FLAGS|IF|IOPL` mask. The arithmetic `rflags_defined` mask remains
`INTEGER_FLAGS`; IF and IOPL do not become arithmetic-defined flags.

Across IF=0/1 × CPL0–3 × IOPL0–3, 20 cases clear only IF, advance RIP and
retire; 12 cases produce #GP(0) with unchanged state and no retirement. Tests
also cover exact LOCK #UD after full fetch, unsupported prefixes/neighbors,
fetch/STOP/budget order, legacy/JIT/native/C denial, concrete IF/IOPL comparison,
and the caller-resubmitted maskable-request boundary after CLI. Intel SDM
Rev. 093 Vol. 2A, CLI pp. 3-148–3-149, Table 3-7 is the source. This is local
interpreter coverage, not event delivery or physical/Linux validation.

## S2-27i exact Long64 STI fixture profile

The `s2-long-sti-v1` profile carries the bounded `StiShadowSnapshot` value in
each CPU snapshot (`clear` or `blocking`) and requires it to be present. It
compares `INTEGER_FLAGS|IF|IOPL`; IF and IOPL remain concrete system bits and
do not expand arithmetic `rflags_defined`. Numeric CPL remains restricted to
0–3 through the profile-specific CPL field. Literal `s2_27i_sti_*.case.json`
fixtures use bare `FB`, IF=0 and Clear shadow, with privilege/IOPL cases
covering successful transitions and #GP(0) denial. Local API tests additionally
cover `FB 90`, `FB FB`, `FB FA`, and `FA FB 90`, plus continuation, faults,
STOP/budget, request eligibility and shadow preservation. JIT/native/C/fragment
paths remain closed. The source is Intel SDM Rev. 093, Vol. 2B, STI
pp. 4-674–4-675, Table 4-22. NMI/SMI delivery, scheduling, HLT, MOV SS and REP
boundaries are excluded.

## S2-27j exact Long64 HLT fixture profile

The `s2-long-hlt-v1` profile carries both `CpuActivitySnapshot` and
`StiShadowSnapshot`; initial fixtures require `running` plus `clear`, and the
post-HLT result records `halted` plus `clear`. It compares
`INTEGER_FLAGS|IF|IOPL`, while arithmetic `rflags_defined` stays unchanged.
The sixteen literal bare-F4 cases cross CPL0–3, IF=0/1 and IOPL=0/3: four CPL0
cases halt and advance one byte; twelve CPL1–3 cases produce atomic #GP(0).
Three additional CPL0 cases cover `FB F4`, `FA F4`, and `FA FB F4`, including
successful HLT consumption of the STI Blocking shadow. Halted outcomes have no
`stop_rip`.

Core API checks cover exact `F4`, `F0 F4` full-fetch #UD, unsupported prefixes,
zero-fetch run/step/REP re-entry, IF=1 request eligibility without wake, IF=0
remaining halted, and successful delivery saving post-HLT RIP before returning
to Running. JIT/native/C/fragment adapters remain denied. The local slice assumes
one vCPU and private synchronous RAM, with VMX/SMM/debug/NMI/MOV-SS inhibition
absent. Source: Intel SDM Rev. 093 Vol. 2A, HLT p. 3-439. This does not add
asynchronous wake sources or establish Linux/physical execution.

## S2-27k exact same-CPL0 IRETQ fixture profile

The four `s2_27k_iretq_if*_iopl*.case.json` cases use
`s2-long-iretq-cpl0-valid-v1`, exact code `48 CF`, budget 1, and stop at the
RIP saved in the 40-byte frame. Code, frame and GDT are separate regions. The
matrix crosses restored IF=0/1 and IOPL=0/3; one case sets high 48 bits in the
CS/SS frame qwords to show that only their low selectors are consumed. The
restored RSP is intentionally unaligned.

The profile compares RIP/RSP/RFLAGS, CS/SS selectors and complete hidden-cache
fields, Running activity, Clear STI shadow, and arithmetic `rflags_defined`.
The descriptors are exact bounded ring-0 Long64 code/data entries with
accessed bits already set. Frame and GDT remain byte-for-byte unchanged.
Selector/cache fields are omitted from older profile wire forms and rejected
there. Interpreter-only; JIT/native/C/fragment execution is denied. This is
local bounded coverage, not general IRET exception behavior or Linux/physical
execution. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-27k--exact-long64-same-cpl0-iretq).

## S2-27l exact same-CPL0 LRETQ profile

The local interpreter profile `s2-long-lretq-cpl0-valid-v1` admits only exact
`48 CB`, budget 1, and stop RIP equal to the target RIP stored in an exact
16-byte frame. Code, frame and current GDT are separate regions; only target
RIP and CS are consumed. The target descriptor is reread from the current GDT
even when it matches the old CS selector. Result state compares CS selector and
cache while preserving SS/cache, flags and every other modeled field. Four
literal fixtures cross IF=0/1 × IOPL=0/3 and runner checks preserve the frame
and GDT. This local fixture evidence does not claim Linux runtime coverage.
Ubuntu's selected kernel source names an
`LRETQ` in `startup_64`, but these bytes have not been independently recovered.

## S2-25a bounded CR0 EM/TS FP-init profile

The 32 `s2_25a_*.case.json` cases use `s2-long-cr0-fp-init-v1` and a single
read/execute code mapping. They cover all four fixed CR0 base+EM/TS values in
the full read/AND/write/FNINIT sequence, the 16 FNINIT gate combinations over
EM/TS and CPL0–3, separate CPL read and write cases, and four write→FNINIT
cases. The strict profile permits only exact `0F20C0`, `0F22C0`, `DBE3`,
`0F22C0DBE3`, and `0F20C04883E0F30F22C0DBE3` bytes with fixed budget and stop
RIP. Results compare CR0, integer state, x87, SSE, activity, and STI shadow.
The fixture source is handwritten and synthetic; it is not an Ubuntu byte or
runtime claim. JIT/native/C/fragment execution remains denied. See the [CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#s2-25a--bounded-long64-cr0-emts-fp-initialization).

## S2-25b bounded mutable CR4 OSFXSR/OSXMMEXCPT FP-init profile

Six `s2_25b_*.case.json` fixtures use `s2-long-cr4-fp-init-v1`. They cover the
four fixed CR4 PAE-base values (only OSFXSR/OSXMMEXCPT vary), EM and OSFXSR
`#UD`, `#UD` priority over TS, TS `#NM`, a successful XORPD with OSXMMEXCPT
clear, and the exact 18-byte `FNINIT; MOV RAX,CR4; OR RAX,0x600; MOV CR4,RAX;
XORPD` composition, whose initial CR0 is constrained to the profile base so
FNINIT succeeds. The current mutable CR0/CR4 values are the only sources for
dynamic gates; the fixtures do not carry duplicate immutable prerequisites.
Testkit checks exact bytes, state, budget, stop, results and tampering. These
are local synthetic fixtures; JIT/native/C/fragment and physical execution
remain denied/not run. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-25b--mutable-cr4-osfxsrosxmmexcpt-in-bounded-fp-initialization).

## S2-25c exact CR0/CR4 FP-init composition

Sixteen `s2_25c_clear_emts_set_cr4_*.case.json` fixtures use
`s2-long-cr0-cr4-fp-init-v1`. Each contains exactly
`0F20C0 4883E0F3 0F22C0 DBE3 0F20E0 480D00060000 0F22E0 660F57C1`, budget 8,
and stop RIP at byte 28. They cross all four initial CR0.EM/TS values with all
four CR4.OSFXSR/OSXMMEXCPT values. Every case expects eight retirements and
checks the cleared CR0 controls, enabled CR4 bits, FNINIT state and XORPD
result. The interpreter executes each instruction independently; interruption
or failure after a retirement preserves that earlier state. JIT/native/C/
fragment execution remains denied, and these handwritten synthetic fixtures
make no hardware or Ubuntu byte claim. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-25c--exact-cr0-clear-fninit-cr4-enable-and-xorpd-composition).

## S2-25e IA32_EFER RDMSR

Four literal `s2_25e_efer_rdmsr_*.case.json` inputs use the strict
`s2-long-efer-rdmsr-v1` profile and exact bare `0F 32`. They independently
exercise synthetic EFER values `0x500`, `0x501`, `0xd00`, and `0xd01`; all
require ECX=`0xc0000080`, the current S2-25a CR0 domain, and the current
S2-25b CR4 domain. CPL0 results place the exact EFER value in EDX:EAX, with
zero extension, and preserve other state. Runner integration also clones
inputs for CPL1–3 and checks atomic #GP(0), then tampers with outputs and
exception vector/error code to verify strict result validation and comparator
rejection. Invalid EFER values and wrong ECX are rejected by schema validation.
These synthetic fixtures do not claim Ubuntu entry state, WRMSR or transition
behavior, NX enforcement, MMU/paging effects, or hardware execution. JIT,
native x86, C and fragment execution remain denied. See the [S2-25e contract](../../../../docs/specs/CPU_CONTRACT.md#s2-25e--exact-read-only-long64-rdmsr-ia32_efer),
[plan](../../../../docs/plans/STAGE_2.md#s2-25e--exact-read-only-long64-rdmsr-ia32_efer), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-25e--exact-read-only-long64-rdmsr-ia32_efer).

### S2-38g local MINSD unmasked invalid and denormal

The separate schema-v2 profile `s2-long-minsd-unmasked-denormal-v1` admits
only Long64 `F2 0F 5D C1` and carries its own immutable SSE2/CR0/CR4 controls.
Eight handwritten fixtures include NaN-only, subnormal-only, mixed NaN/subnormal,
both operand positions, masked completion, and #UD/#XM selection. Mixed inputs
with both masks closed expect IE only; a mixed input with IM masked and DM open
expects IE|DE. The masked mixed completion selects the raw source-2 minimum
subnormal (`0100000000000000` in little-endian bytes), preserves XMM0's upper
qword, and records IE only. Separate masked QNaN/SNaN source-2 fixtures check the
exact sign and payload bytes (`fecaefbeaddef8ff` and
`fecaefbeaddef0ff`). Fault cases preserve full XMM0 and RIP with zero retirement.

The independent result validator classifies raw operands and uses a separate
binary64 ordering rule to check raw output bits, sticky status, full state,
exception, retirement and immutable control echo. Unit tests cross the complete
operand-position × IM × DM × OSXMMEXCPT matrix for NaN-only, subnormal-only and
mixed conditions, plus prerequisite/fetch/budget/form boundaries. JIT and native
execution deny the profile. Physical/Mac mini acceptance remains deferred. See
the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38g--exact-minsd-unmasked-invalid-and-denormal-exit),
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-38g--exact-long64-minsd-with-bounded-unmasked-imdm-exits),
[plan](../../../../docs/plans/STAGE_2.md#s2-38g-exact-long64-minsd-xmm0xmm1-unmasked-invalid-and-denormal-exit),
and [format](../../../../docs/testing/TEST_CASE_FORMAT.md#s2-38g-exact-minsd-unmasked-imdm-profile). Normative sources: Intel SDM Rev. 093 Vol. 2B MINSD pp. 4-22–4-23 and Vol. 1 §§4.9.2, 11.5.2, 11.5.3.2–3.


### S2-38h local MAXSD unmasked invalid and denormal

The separate schema-v2 profile `s2-long-maxsd-unmasked-denormal-v1` admits
only Long64 `F2 0F 5F C1` and carries a MAXSD-specific immutable SSE2/CR0/CR4
control object. Eighty fixed literal inputs cover QNaN, SNaN, subnormal and
mixed NaN/subnormal classes in both operand positions crossed with IM, DM and
OSXMMEXCPT. Seven more selector cases cover finite maxima, equal values,
both signed-zero ties and infinities; one pre-existing IE/DE-only case proves
old sticky flags alone do not cause a fault. The result validator independently
checks each actual interpreter snapshot, raw payload/sign preservation, upper
qword and unrelated state, IE/DE status, retirement and exception metadata.
Fault cases preserve XMM0/RIP with zero retirement. JIT/native deny the profile;
C/fragment and hardware execution remain deferred. The selector and exception
list cite Intel SDM Rev. 093 Vol. 2B MAXSD pp. 4-11–4-12 (official PDF SHA-256
`a261998ace8e07f624bf3e2486bb6cfe950fb8be8d0dc9631bcecb4c5ec953e4`), not a
MINSD-only inference; Vol. 1 §§4.9.2, 11.5.2.1–2, 11.5.3.2–3 and Vol. 2A
Table 2-20 establish the exception/control behavior.


### S2-38i local MINPD unmasked invalid and denormal

The separate schema-v2 profile `s2-long-minpd-unmasked-denormal-v1` accepts
only Long64 `66 0F 5D C1` and carries immutable SSE2/CR0/CR4 controls. The 128
fixed inputs cross lane0/lane1 event classes `{none, I, D, I+D}` with IM, DM
and OSXMMEXCPT. The independent validator checks raw per-lane conditions,
all-lane flag collection on an unmasked exit, same-lane invalid priority for a
masked completion, atomic full-XMM state, retirement and fault metadata. JIT
and native deny the profile; C/fragment and physical hardware remain deferred.
Normative sources: Intel SDM Rev. 093 Vol. 2B MINPD pp. 4-16–4-18 and Vol. 1
§§4.9.2, 11.5.3.1–3.

### S2-38j local MAXPD unmasked invalid and denormal

The separate schema-v2 profile `s2-long-maxpd-unmasked-denormal-v1` accepts
only Long64 `66 0F 5F C1` and carries immutable SSE2/CR0/CR4 controls. Its 128
fixed inputs cross both lane classes `{none, I, D, I+D}` with IM, DM and
OSXMMEXCPT; 16 additional literal results distinguish MAXPD selection, NaN
payload/sign forwarding, SNaN non-quieting, zero ties, equal values, infinities
and prior sticky status. The validator checks both lanes independently, full
XMM fault suppression, exception metadata and retirement. JIT/native reject the
profile; C/fragment and physical hardware remain deferred. Normative sources:
Intel SDM Rev. 093 Vol. 2B MAXPD pp. 4-5–4-7 and Vol. 1 §§4.9.2,
11.5.3.1–3. See [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38j--exact-maxpd-unmasked-invalid-and-denormal-exit),
[plan](../../../../docs/plans/STAGE_2.md#s2-38j--exact-long64-maxpd-xmm0xmm1-unmasked-invalid-and-denormal-exit),
and [fixture format](../../../../docs/testing/TEST_CASE_FORMAT.md#s2-38j-exact-maxpd-unmasked-imdm-profile).

### S2-38k local MINPS unmasked invalid and denormal

The exact Long64 `0F 5D C1` profile uses 32 literal fixtures: sixteen four-lane
event/control anchors and sixteen selector/status anchors. The independent
schema-v2 validator covers all four lanes, unmasked aggregate status, masked
same-lane IE-over-DE filtering, atomic XMM fault suppression, payload/sign
forwarding, zero ties, sticky-only status and exact SSE control echoes. JIT and
native reject the profile; no physical run is claimed. Sources and scope are
listed in the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38k--exact-minps-unmasked-invalid-and-denormal-exit),
[plan](../../../../docs/plans/STAGE_2.md#s2-38k--exact-long64-minps-xmm0xmm1-unmasked-invalid-and-denormal-exit),
and [fixture format](../../../../docs/testing/TEST_CASE_FORMAT.md#s2-38k-exact-minps-unmasked-imdm-profile).

### S2-38l local MAXPS unmasked invalid and denormal

The 32 literal `s2_38l_maxps_*.case.json` inputs use exact `0f5fc1`, Long64,
budget one, a single executable mapping and stop RIP `entry+3`. The sixteen
lane/control anchors exercise four binary32 lanes with event classes `{none,
I,D,I+D}` and IM/DM/OSXMMEXCPT combinations; the sixteen selector/status anchors
cover QNaN/SNaN on either operand, raw source-2 payload/sign, signed-zero and
equal ties, infinities, finite MAX-vs-MIN selection, lane-local IE priority,
split IE/DE and prior sticky flags. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38l--exact-maxps-unmasked-invalid-and-denormal-exit).

### S2-37m local CVTDQ2PD

The six `s2_37m_cvtdq2pd_*.case.json` files are handwritten Long64
`F3 0F E6 C1` fixtures with one executable mapping, budget one and stop RIP
`entry+4`. They cover zero, ±1, i32 endpoints, asymmetric lane pairs, values
around 2^24, and signed integers whose bit patterns resemble NaNs. XMM1 high64
varies as an ignored source; MXCSR values vary within supported mask
`0x0000ffbf`. The schema-v2 validator derives expected binary64 encodings from
integer bits, independently of the production conversion helper. SSE2/CR0/CR4
gate inputs are explicit and echoed in the result. JIT/native reject the
profile; no C/fragment or physical hardware acceptance is claimed. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37m--exact-long64-cvtdq2pd-xmm0xmm1)
and [plan](../../../../docs/plans/STAGE_2.md#s2-37m--exact-long64-cvtdq2pd-xmm0xmm1).

### S2-37p local CVTDQ2PD register pairs

`s2_37p_cvtdq2pd_xmm15_alias.case.json` is a handwritten canonical
`F3 45 0F E6 FF` XMM15,XMM15 self-alias fixture with one executable mapping,
budget one and stop RIP `entry+5`. It checks that both signed dword lanes come
from the initial XMM15 value and that its high qword is ignored. The runner
test also generates all 256 canonical register pairs and validates the
independent schema oracle, state preservation, and RIP. Malformed/redundant
prefixes, REX.W/X, memory forms, trailing/truncated bytes, VEX/EVEX, and exact
profile mismatches are rejected as local-profile refusals. JIT/native remain
closed. This fixture documents canonical register-pair coverage only; memory
m64 and physical acceptance are not claimed. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37p--canonical-long64-cvtdq2pd-xmm-register-pairs)
and [plan](../../../../docs/plans/STAGE_2.md#s2-37p--canonical-long64-cvtdq2pd-xmm-register-pairs).

### S2-37n local CVTSI2SD r32

Eight literal fixtures accept only Long64 `F2 0F 2A C0` with a single
executable mapping, budget one and stop RIP `entry+4`. They cover zero, ±1,
i32 endpoints and values immediately below/above 2^24. Two `EAX=1` fixtures
use different RAX high halves to prove the source width is exactly 32 bits.
Each fixture seeds a sentinel XMM0 high qword; the independent schema oracle
computes the expected low-qword binary64 bits directly from signed i32 input.
Control echoes include OSXMMEXCPT to verify both values have no effect on this
exactly representable input. Profile refusals do not generalize architectural
#UD claims to excluded forms. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37n--exact-long64-cvtsi2sd-xmm0eax)
and [plan](../../../../docs/plans/STAGE_2.md#s2-37n--exact-long64-cvtsi2sd-xmm0eax).

### S2-37o local CVTSI2SD r64

Sixteen literal fixtures accept only Long64 `F2 48 0F 2A C0`, one executable
mapping, budget one and stop RIP `entry+5`. They cover zero, ±1, i64 endpoints,
values around 2^53, halfway cases and carry at a power boundary. MXCSR inputs
select exact and inexact results, RC, masked and unmasked Precision, old PE and
both OSXMMEXCPT values. Unmasked inexact cases check sticky PE with unchanged
destination, RIP and retirement; OSXMMEXCPT selects #XM or SIMD-generated #UD.
The schema oracle independently rounds signed magnitude using explicit
remainder bits. Runner checks preserve full RAX and XMM0 high64, reject result
and control tampering, and deny JIT/native backends. Other encodings are local
profile refusals, not architectural #UD claims. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37o--exact-long64-cvtsi2sd-xmm0rax)
and [plan](../../../../docs/plans/STAGE_2.md#s2-37o--exact-long64-cvtsi2sd-xmm0rax).

### S2-37q canonical CVTSI2SD register space

The schema-v2 profile `s2-long-cvtsi2sd-registers-v1` admits canonical
Long64 `CVTSI2SD` r32/r64 register forms across all 16 XMM destinations and 16
GPR sources (512 forms). The runner test constructs each byte stream from its
selectors and validates the resulting state against the independent schema
oracle. r32 reads signed low32; r64 follows the existing MXCSR.RC and sticky PE
rules. Destination XMM high64 and all other GPR/XMM state are preserved, and
RSP is treated as an ordinary source without stack access. The profile refuses
noncanonical REX/prefixes, memory, trailing bytes, VEX and EVEX. The S2-37n/o
fixed profiles keep their prior restrictions. JIT/native and physical
acceptance remain closed; m32/m64 memory forms and broader CPU support remain
open. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37q--canonical-long64-cvtsi2sd-register-operand-space)
and [plan](../../../../docs/plans/STAGE_2.md#s2-37q--canonical-long64-cvtsi2sd-register-operands).

### S2-37r canonical CVTSI2SS register space

The schema-v2 profile `s2-long-cvtsi2ss-registers-v1` admits canonical
Long64 r32/r64 register forms across XMM0–15 × GPR0–15 (512 forms). The runner
constructs every byte sequence and validates it against an independent
integer-only binary32 oracle. Literal oracle anchors cover signed 2^24
thresholds, i32/i64 endpoints, halfway neighbors, exponent carry and direct
rounding cases that distinguish conversion from double rounding. Separate
interpreter and runner literal cases execute signed thresholds/endpoints,
ties, carry and direct-vs-double-rounding values across RC for both widths,
and verify exact completion with old PE and PM=0. The selector loop
checks GPR source preservation, XMM upper96 preservation, RSP as a normal source,
MXCSR.RC and Precision mask/OSXMMEXCPT/old-PE combinations. Full fetch and
prerequisite gates are checked at all CPL values across REX classes; no data
memory callback occurs. Malformed/truncated prefixes and forms are rejected
as local profile refusals. Result/control tampering is rejected, and JIT,
native, C and fragment paths remain closed. m32/m64, other modes/prefixes and
physical acceptance remain open. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37r--canonical-long64-cvtsi2ss-register-operand-space)
and [plan](../../../../docs/plans/STAGE_2.md#s2-37r--canonical-long64-cvtsi2ss-register-operand-space).

### S2-37s canonical CVTDQ2PS register pairs

`s2_37s_cvtdq2ps_asymmetric_register_pair.case.json` is a handwritten
schema-v2 `s2-long-cvtdq2ps-registers-v1` fixture. It records a mixed-sign four-lane
source and checks direct signed-i32 to binary32 conversion, sticky PE aggregation,
full destination replacement and unrelated-state preservation. The independent
validator derives canonical bytes, XMM indices and length; runner coverage
constructs all 256 register pairs and covers the 512 lane-inexactness/RC/PM/
OSXMMEXCPT/old-PE cases, including lane-3-only inexactness for atomic
suppression. Literal vectors include signed 2^24 boundaries, ties, directed
rounding, i32 endpoints and zero. Prerequisite gates, CPL0–3 and fetch ordering
make no data-memory callbacks. Prefix/memory/trailing/VEX/EVEX forms are profile
refusals only. JIT/native/C/fragment/generic CPUID paths remain closed. Memory
m128 and its alignment/address/page faults, other modes, physical acceptance,
and parent S2-37/S2-38 requirements remain open. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37s--canonical-long64-cvtdq2ps-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37s--canonical-long64-cvtdq2ps-xmm-register-pairs), and
[stage report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37s--canonical-long64-cvtdq2ps-xmm-register-pairs).

### S2-37t canonical CVTSS2SD register pairs

`s2_37t_cvtss2sd_asymmetric_register_pair.case.json` is the handwritten
schema-v2 `s2-long-cvtss2sd-registers-v1` anchor. It uses canonical
`F3 0F 5A C1`, with low32 source SNaN and high96 hostile patterns. The runner
covers all 256 register pairs, self-aliases, signed zero/normal/infinity edges,
QNaN/SNaN payloads, all 23 subnormal leading-bit positions, high96 ignore,
destination high64 preservation, and IE/DE exception controls. JIT and native
requests remain unsupported. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37t--canonical-long64-cvtss2sd-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37t--canonical-long64-cvtss2sd-xmm-register-pairs), and
[stage report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37t--canonical-long64-cvtss2sd-xmm-register-pairs).


### S2-37u canonical CVTPS2PD register pairs

`s2_37u_cvtps2pd_asymmetric_register_pair.case.json` is the handwritten
schema-v2 `s2-long-cvtps2pd-registers-v1` fixture. It uses canonical
`0F 5A C1`, converts both low binary32 lanes, and places hostile SNaN/subnormal
patterns in ignored source high64. Runner coverage checks all 256 register
pairs and aliases, raw NaN/subnormal widening, full destination replacement,
and all 288 lane-event/mask/OSXMMEXCPT/old-sticky combinations, including
second-lane-only unmasked suppression. Gate/fetch/CPL/no-callback, malformed
encoding, tampering, JIT and native denial are covered. See the [CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37u--canonical-long64-cvtps2pd-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37u--canonical-long64-cvtps2pd-xmm-register-pairs),
and [stage report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37u--canonical-long64-cvtps2pd-xmm-register-pairs).

### S2-37v canonical COMISS and UCOMISS register pairs

`s2_37v_comiss_ucomiss_baseline.case.json` is a handwritten schema-v2
`s2-long-comiss-ucomiss-registers-v1` anchor using canonical `COMISS XMM0,XMM1`.
Its XMM state includes unrelated high96 bit patterns. The runner constructs both
opcodes across all 256 register pairs and checks raw binary32 ordering, signed
zero, infinities, QNaN/SNaN behavior, NaN-before-denormal priority, sticky
status and exception masks. Gate/fetch/CPL/no-callback, malformed encoding,
result tampering and JIT/native denial are covered. This local profile does not
claim memory forms, generic CPUID admission, physical acceptance, or closure
of parent S2-37/S2-38. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37v--canonical-long64-comiss-and-ucomiss-xmm-register-pairs), [plan](../../../../docs/plans/STAGE_2.md#s2-37v--canonical-long64-comiss-and-ucomiss-xmm-register-pairs), and [stage report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37v--canonical-long64-comiss-and-ucomiss-xmm-register-pairs).

### S2-37w canonical COMISD and UCOMISD register pairs

`s2_37w_comisd_ucomisd_baseline.case.json` is the handwritten schema-v2
`s2-long-comisd-ucomisd-registers-v1` anchor with canonical `66 0F 2F C1`.
The independent fixture oracle classifies raw binary64 values and computes
signed-magnitude ordering without calling `Binary64::compare_raw`. Runner
coverage builds both opcodes for all 256 XMM pairs, checks QNaN/SNaN, subnormal,
infinity, signed-zero and adjacent-normal relations, and rejects malformed
prefix, memory, trailing-byte, VEX and EVEX forms. The interpreter-only profile
keeps JIT/native admission closed; memory forms, generic CPUID admission,
physical acceptance and parent S2-37/S2-38 remain out of scope. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37w--canonical-long64-comisd-and-ucomisd-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37w--canonical-long64-comisd-and-ucomisd-xmm-register-pairs),
and [stage report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37w--canonical-long64-comisd-and-ucomisd-xmm-register-pairs).

### S2-37x canonical CMPSS register pairs

`s2_37x_cmpss_registers.case.json` is the handwritten schema-v2
`s2-long-cmpss-registers-v1` anchor for canonical `F3 0F C2 C1 01`.
The runner constructs all 2048 XMM-pair/predicate encodings and checks operand
aliases, all legacy predicate outcomes, raw binary32 classes, signed zero,
NaN-before-denormal priority, signaling-predicate IE, IE/DE masks and sticky
bits, destination high96, fault atomicity, gate/fetch/CPL behavior, malformed
forms, tampering and JIT/native denial. The testkit has an independent raw-bit
comparator and predicate table. Immediate high bits are local profile refusals;
this profile does not claim architectural #UD for them. Memory forms, generic
CPUID admission, physical acceptance, and parent S2-37/S2-38 remain open.
See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37x--canonical-long64-cmpss-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37x--canonical-long64-cmpss-xmm-register-pairs),
and [stage report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37x--canonical-long64-cmpss-xmm-register-pairs).

### S2-37y canonical CMPSD register pairs

`s2_37y_cmpsd_registers.case.json` is the handwritten schema-v2
`s2-long-cmpsd-registers-v1` anchor for canonical `F2 0F C2 C1 01`. The
independent fixture oracle classifies raw binary64 values and computes
signed-magnitude ordering without calling production comparison code. Runner
coverage builds all 2048 XMM-pair/predicate encodings and checks truth classes,
QNaN/SNaN signaling, subnormal priority, exception masks/sticky bits, high64
preservation, fault atomicity, gate/fetch/CPL behavior, malformed forms,
tampering and JIT/native denial. The interpreter-only profile does not claim
memory forms, generic CPUID admission or physical acceptance. Parent S2-37 and
S2-38 remain open; hardware acceptance is deferred to the final combined
stage. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37y--canonical-long64-cmpsd-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37y--canonical-long64-cmpsd-xmm-register-pairs),
and [stage report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37y--canonical-long64-cmpsd-xmm-register-pairs).


### S2-37z canonical CMPPD register pairs

`s2_37z_cmppd_registers.case.json` is the handwritten schema-v2
`s2-long-cmppd-registers-v1` anchor for canonical `66 0F C2 C1 01`. The
independent fixture oracle classifies and orders each raw binary64 lane itself.
Runner coverage builds all 2,048 XMM-pair/predicate encodings and checks truth
classes, QNaN/SNaN signaling, independent lane exception accumulation in both
orders, exception masks/sticky bits, full-destination atomicity, gate/fetch/CPL
behavior, malformed forms, tampering and JIT/native denial. The profile does not
claim memory forms, generic CPUID admission or physical acceptance. Parent S2-37
and S2-38 remain open; hardware acceptance is deferred to the final combined
stage. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37z--canonical-long64-cmppd-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37z--canonical-long64-cmppd-xmm-register-pairs),
and [stage report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37z--canonical-long64-cmppd-xmm-register-pairs).


### S2-37aa canonical CMPPS register pairs

`s2_37aa_cmpps_registers.case.json` is the handwritten schema-v2
`s2-long-cmpps-registers-v1` anchor for canonical `0F C2 C1 01`. The independent
fixture oracle classifies and orders each raw binary32 lane itself. Runner
coverage constructs all 2,048 XMM-pair/predicate encodings and checks raw class
combinations in every lane, predicates, independent cross-lane exception
aggregation, sticky/mask behavior, full-destination atomicity, fetch/gates/CPL,
malformed forms, control tampering and JIT/native denial. The bounded profile
does not claim memory forms, generic CPUID admission or physical acceptance.
Parent S2-37/S2-38 remain open; hardware acceptance is deferred to the final
combined stage. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37aa--canonical-long64-cmpps-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37aa--canonical-long64-cmpps-xmm-register-pairs),
and [stage report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37aa--canonical-long64-cmpps-xmm-register-pairs).

### S2-37ab canonical CVTSS2SI register space

The schema-v2 profile `s2-long-cvtss2si-registers-v1` parses all 512 canonical
r32/r64 GPR destination × XMM source pairs. The oracle independently reads raw
binary32 fields, rounds integer magnitudes for MXCSR.RC and classifies invalid
range without using production conversions or host floating point. Runner tests
cover selector mappings, destination zero-extension, preservation, IE/PE masks,
invalid-over-precision priority, exact signed minima versus indefinite,
subnormal input with DM unmasked, fault atomicity and backend denial. No DE is
reported for subnormals. Full fetch and SSE/EM/OSFXSR then TS ordering are
checked by the interpreter profile. Memory forms, other modes/prefixes and
physical acceptance remain open. Sources: Intel SDM Rev. 093 Vol. 2A pp.
3-242–3-243; Vol. 1 §11.5.2.2 ([official SDM](https://cdrdv2.intel.com/v1/dl/getContent/671199)).
See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ab--canonical-long64-cvtss2si-register-space)
and [plan](../../../../docs/plans/STAGE_2.md#s2-37ab--canonical-long64-cvtss2si-register-space).

### S2-37ac canonical CVTSD2SI register space

The schema-v2 profile `s2-long-cvtsd2si-registers-v1` accepts all 512
canonical r32/r64 GPR destination × XMM source combinations. Its oracle parses
raw binary64 fields and performs integer rounding and range checks; it does not
call production conversion helpers or host floating point. Coverage includes
all selector pairs, MXCSR.RC, halfway values, signed i32/i64 boundaries, exact
signed minima versus integer indefinite, QNaN/SNaN/infinity, subnormal without
DE, sticky IE/PE and unmasked exception atomicity. The bounded profile admits
only the interpreter; memory sources and physical acceptance remain outside
scope. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ac--canonical-long64-cvtsd2si-register-space),
[plan](../../../../docs/plans/STAGE_2.md#s2-37ac--canonical-long64-cvtsd2si-register-space),
and [stage report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ac--canonical-long64-cvtsd2si-register-space).

### S2-37ad canonical CVTTSS2SI register space

The separate profile `s2-long-cvttss2si-registers-v1` recognizes all 512
canonical Long64 r32/r64 GPR × XMM register forms. The schema oracle parses the
F3 0F 2C opcode and uses an independent raw-binary32 integer oracle with
TowardZero, regardless of MXCSR.RC. Runtime coverage includes source and
destination selectors, zero-extension, fractional values, width boundaries,
Invalid/Precision priority, sticky state, DM-unmasked subnormals without DE,
and unmasked exception atomicity. Gate/fetch ordering and backend denial are
profile requirements; physical acceptance remains in the combined final stage.
Source: Intel SDM Rev. 093 Vol. 2A `CVTTSS2SI` pp. 3-255–3-256 and Vol. 1
§11.5.2.2 ([official SDM](https://cdrdv2.intel.com/v1/dl/getContent/671199)).

### S2-37af canonical CVTPS2DQ register pairs

`s2_37af_cvtps2dq_ie_lane0_pe_lane3.case.json` is a literal canonical
`66 0F 5B /r` XMM0,XMM1 fixture. IM is clear. Lane 0 contains +2^31 and
generates unmasked IE; lane 3 contains 1.5 and would generate PE. The fixture
expects the IE fault with sticky IE only, unchanged XMM/RIP and zero retirement,
proving that IE commits before PE from another valid lane. A separate runner
case masks IE and unmasks PE to prove IE|PE aggregation. The schema-v2 runner
test generates all 256 pairs and the complete four-lane `{none,IE,PE}` event
cross-product.
Only register forms are in scope. Source: Intel SDM Rev. 093 Vol. 2A
CVTPS2DQ pp. 3-225–3-227 and Vol. 1 §§11.5.3.2–3
([official SDM](https://cdrdv2.intel.com/v1/dl/getContent/671199)).

### S2-37ag canonical CVTTPS2DQ register pairs

`s2_37ag_cvttps2dq_ie_lane0_pe_lane3.case.json` is a literal canonical
`F3 0F 5B /r` XMM0,XMM1 fixture. Lane 0 contains +2^31 and raises unmasked IE;
lane 3 contains 1.5 and would raise PE. It checks that IE commits before PE
from another valid lane, while XMM/RIP and retirement remain unchanged on the
fault. The schema-v2 runner covers all 256 register pairs and the complete
four-lane `{none,IE,PE}` event/control matrix, including RC invariance and
TowardZero results. Only register forms are in scope. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ag--canonical-long64-cvttps2dq-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37ag--canonical-long64-cvttps2dq-xmm-register-pairs)
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ag--canonical-long64-cvttps2dq-xmm-register-pairs).
Source: Intel SDM Rev. 093 Vol. 2A CVTTPS2DQ pp. 3-249–3-251; Vol. 1
§§11.5.3.2–3 ([official SDM](https://cdrdv2.intel.com/v1/dl/getContent/671199)).


### S2-37ah canonical CVTPD2DQ register pairs

`s2_37ah_cvtpd2dq_ie_lane0_pe_lane1.case.json` is a literal canonical
`F2 0F E6 /r` XMM0,XMM1 fixture. Lane 0 contains +2^31 and raises unmasked
IE; lane 1 contains 1.5 and would raise PE. It verifies IE-only sticky status
and unchanged XMM/RIP with zero retirement on the fault. The runner covers all
256 register pairs and 1,152 two-lane IE/PE event, rounding, mask,
OSXMMEXCPT and prior-sticky combinations, including masked-IE PE aggregation,
alias snapshot, boundaries, profile cross-denial and JIT/native denial. Only
register forms are in scope. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ah--canonical-long64-cvtpd2dq-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37ah--canonical-long64-cvtpd2dq-xmm-register-pairs)
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ah--cvtpd2dq-register-pair-interpreter-profile-2026-10-30).
Source: Intel SDM Rev. 093 Vol. 2A CVTPD2DQ pp. 3-214–3-217 and Vol. 1
§§11.5.3.2–3 ([official SDM](https://cdrdv2.intel.com/v1/dl/getContent/671199)).


### S2-37ai canonical CVTTPD2DQ register pairs

`s2_37ai_cvttpd2dq_ie_lane0_pe_lane1.case.json` is a literal canonical
`66 0F E6 /r` XMM0,XMM1 fixture. Lane 0 contains +2^31 and raises unmasked
IE; lane 1 contains 1.5 and would raise PE. It verifies IE-only sticky status
and unchanged XMM/RIP with zero retirement on the fault. The runner covers all
256 register pairs and 1,152 two-lane IE/PE/RC, control and prior-sticky
combinations, including RC invariance, masked-IE PE aggregation, alias snapshot,
boundaries, profile cross-denial and JIT/native denial. Only register forms
are in scope. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ai--canonical-long64-cvttpd2dq-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37ai--canonical-long64-cvttpd2dq-xmm-register-pairs)
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ai--cvttpd2dq-register-pair-interpreter-profile-2026-10-30).
Source: Intel SDM Rev. 093 Vol. 2A CVTTPD2DQ pp. 3-244–3-247 and Vol. 1
§§11.5.3.2–3 ([official SDM](https://cdrdv2.intel.com/v1/dl/getContent/671199)).


### S2-37aj canonical SQRTSS register-pair profile

`s2_37aj_sqrtss_xmm0_xmm1.case.json` is a literal `F3 0F 51 /r` Long64 register fixture. The schema-v2 interpreter-only profile accepts exactly canonical four-byte low-register and five-byte extended-register encodings for all 256 pairs. It snapshots source low32, uses MXCSR.RC, preserves destination high96 and unrelated state, and aggregates masked IE/DE/PE while delivering unmasked priority outcomes atomically. The fixture contains an exact square-root success and high-bit sentinels; runner coverage exercises register routing, input classes, controls, sticky status and JIT/native denial. DAZ/reserved MXCSR values, memory forms, other modes and physical acceptance remain outside scope. NaN payload behavior is a local raw-helper policy, not a hardware payload claim. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37aj--canonical-long64-sqrtss-xmm-register-pairs), [plan](../../../../docs/plans/STAGE_2.md#s2-37aj--canonical-long64-sqrtss-xmm-register-pairs) and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37aj--sqrtss-register-pair-interpreter-profile-2026-10-30). Sources: Intel SDM Rev. 093 Vol. 2B SQRTSS pp. 4-669–4-670; Vol. 1 §4.9.2, §§11.5.2.1–.2 and 11.5.3.1; Vol. 3A Table 7-8; Floating-Point Reference Sheet v2.13.


### S2-37ak canonical SQRTPS register-pair profile

`s2_37ak_sqrtps_xmm0_xmm1.case.json` is a literal `0F 51 /r` Long64 XMM0,XMM1 fixture with four source lanes (4.0, 2.0, 1.0, 0.25). It exercises exact and inexact lane results, MXCSR.PE, full 128-bit destination replacement, and interpreter-only backend admission. The strict schema-v2 profile covers all 256 register pairs and canonical three-/four-byte forms; DAZ/reserved MXCSR values, memory forms, other modes and physical acceptance are out of scope. Runner coverage exercises all lane positions and IE/DE/PE permutations across IM/DM/PM/OSXMMEXCPT, all four RC values, FTZ, prior sticky flags, NaN/zero/infinity/subnormal/invalid inputs, gate/fetch/atomicity behavior and JIT/native denial. When a pre-event in any lane is unmasked, all generated IE/DE flags commit before fault and no PE is added. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ak--canonical-long64-sqrtps-xmm-register-pairs), [plan](../../../../docs/plans/STAGE_2.md#s2-37ak--canonical-long64-sqrtps-xmm-register-pairs) and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ak--sqrtps-register-pair-interpreter-profile-2026-10-30). Sources: Intel SDM Rev. 093 Vol. 2B SQRTPS pp. 4-664–4-666; Vol. 1 §§11.5.2.1–.2 and 11.5.3.1–.3; Vol. 3A Event 19/Table 7-8; Floating-Point Reference Sheet v2.13.

### S2-37al canonical SQRTPD register-pair profile

`s2_37al_sqrtpd_xmm0_xmm1.case.json` is a literal `66 0F 51 /r` Long64
XMM0,XMM1 fixture. Its two binary64 lanes are 2.0 and 4.0, exercising an
inexact and exact square root, MXCSR.PE, full 128-bit destination replacement,
and interpreter-only execution. The strict schema-v2 profile covers all 256
register pairs and canonical four-/five-byte encodings. DAZ/reserved MXCSR,
memory forms, other modes and physical acceptance are out of scope. Local tests
exercise lane order, aggregate IE/DE before PE, mask/OSXMMEXCPT behavior, RC,
FTZ, prior sticky flags, MXCSR rejection, gate/fetch ordering and JIT/native
denial. Sources: Intel SDM Rev. 093 Vol. 2B SQRTPD pp. 4-661–4-663 (Type 2,
Table 2-19); Vol. 1 §§11.5.3.1–.3; Vol. 3A Event 19/Table 7-8; Floating-Point
Reference Sheet v2.13 pp. 1–2. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37al--canonical-long64-sqrtpd-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37al--canonical-long64-sqrtpd-xmm-register-pairs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37al--sqrtpd-register-pair-interpreter-profile-2026-10-30).

### S2-37am canonical SQRTSD register-pair profile

`s2_37am_sqrtsd_xmm0_xmm1.case.json` is a literal `F2 0F 51 /r` Long64
XMM0,XMM1 fixture. Source low64 is 2.0; source high64 is a negative QNaN poison
that must not be examined; destination high64 is a sentinel that must survive.
The result checks low64 binary64 square root and MXCSR.PE while preserving that
high half. The strict schema-v2 profile covers all 256 pairs and canonical
four-/five-byte forms. DAZ/reserved MXCSR, memory forms, other modes and
physical acceptance are out of scope. Local tests exercise scalar IE/DE/PE
ordering, high-half preservation and aliasing, min-subnormal DE-only versus
DE+PE, RC/FTZ, masks/stickies, gate/fetch behavior and JIT/native denial.
Sources: Intel SDM Rev. 093 Vol. 2B SQRTSD pp. 4-667–4-668 (Type 3, Table 2-20);
Vol. 1 §§11.5.3.1–.3; Vol. 3A Event 19/Table 7-8; Floating-Point Reference
Sheet v2.13 pp. 1–2. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37am--canonical-long64-sqrtsd-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37am--canonical-long64-sqrtsd-xmm-register-pairs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37am--sqrtsd-register-pair-interpreter-profile-2026-10-30).

### S2-37ao bounded masked-range SUBPS register-pair profile

`s2_37ao_subps_xmm0_xmm1.case.json` is a literal `0F 5C /r` Long64
XMM0,XMM1 fixture with four binary32 lanes. The strict schema-v2 profile covers
all 256 register pairs using canonical three-/four-byte encodings and requires
MXCSR.OM=1 and MXCSR.UM=1. DAZ/reserved bits are rejected; RC, FTZ, IM/DM/PM
and prior sticky flags are admitted. The independent oracle uses integer raw-u32
subtraction and rounding; the oracle covers all four RC settings. Production
runner cases execute the halfway vector at each RC and check result bits and PE.
Local checks also cover lane routing and aliases, combined SNaN/denormal/overflow/
tiny events, IE/DE-before-OE/UE/PE ordering, FTZ, PM=0 atomicity, old sticky
flags with clear masks, result-tampering rejection, schema/runtime gates and
JIT/native denial. This is not physical
acceptance. Memory operands, other modes and full unmasked OE/UE behavior remain
outside the slice. Sources: Intel SDM Rev. 093 Vol. 2B SUBPS pp. 4-690–4-692;
Vol. 1 §§4.9.1.4–.6, 11.5.3 and DAZ/FTZ; Vol. 3A Event 19/Table 7-8; Intel
Floating-Point Reference Sheet v2.13. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ao--bounded-masked-range-long64-subps-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37ao--bounded-masked-range-long64-subps-xmm-register-pairs)
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ao--subps-register-pair-interpreter-profile-2026-10-30).

### S2-37an bounded masked-range SUBPD register-pair profile

`s2_37an_subpd_xmm0_xmm1.case.json` is a literal `66 0F 5C /r` Long64
XMM0,XMM1 fixture with two binary64 lanes. The strict schema-v2 profile covers
all 256 pairs and canonical four-/five-byte register forms. Admission requires
MXCSR.OM=1 and MXCSR.UM=1, rejects DAZ/reserved bits, and accepts RC, FTZ,
IM/DM/PM and prior sticky flags. It therefore covers masked OE/UE only, not the
full unmasked overflow/underflow behavior. Local runner tests cover aliasing,
all pair routes, NaN/infinity/subnormal classes, cross-lane IE/DE priority,
directed rounding/signed zero, masked overflow, FTZ tiny results, atomic
exceptions, gates/fetch/CPL and backend denial. These results are not physical
acceptance. Memory forms and other modes remain outside this profile. Sources:
Intel SDM Rev. 093 Vol. 2B SUBPD pp. 4-687–4-689 and Vol. 2A §2.5.2,
Table 2-19 (p. 2-27); Vol. 1 §§4.9.1.4–.6, 11.5.3 and DAZ/FTZ; Vol. 3A
Event 19/Table 7-8; Intel
Floating-Point Reference Sheet v2.13. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37an--bounded-masked-range-long64-subpd-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37an--bounded-masked-range-long64-subpd-xmm-register-pairs)
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37an--subpd-register-pair-interpreter-profile-2026-10-30).

### S2-37aq bounded masked-range SUBSS register-pair profile

`s2_37aq_subss_xmm0_xmm1.case.json` is a handwritten literal `F3 0F 5C /r`
Long64 XMM0,XMM1 scalar binary32 subtraction. The low32 computes 3.0−1.0;
destination high96 contains a byte-pattern canary, and source high96 contains
poison data that must be ignored. The strict schema-v2 profile covers all 256
register pairs, requires MXCSR.OM=1 and MXCSR.UM=1, rejects DAZ/reserved bits,
and accepts every RC, FTZ, IM/DM/PM and old sticky setting. Its independent
oracle uses raw-u32 integer arithmetic without host floating point or
production helpers. Local checks cover aliases, high96 preservation/ignoring,
all RC settings, NaN ordering and quieting, infinity-invalid, subnormal DE,
FTZ, masked overflow, DE-before-PE atomicity, gates/fetch/CPL, result tampering
and interpreter-only backend denial. This is local evidence, not physical
acceptance. Memory forms, other modes and full unmasked OE/UE behavior remain
outside the slice. Sources: Intel SDM Rev. 093 Vol. 2A Type 3/Table 2-20 and
Vol. 2B SUBSS pp. 4-695–4-696; Vol. 1 §§4.9.1.4–.6 and 11.5.3; Vol. 3A Event
19/Table 7-8. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37aq--bounded-masked-range-long64-subss-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37aq--bounded-masked-range-long64-subss-xmm-register-pairs)
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37aq--subss-register-pair-interpreter-profile-2026-10-30).

### S2-37ap bounded masked-range SUBSD register-pair profile

`s2_37ap_subsd_xmm0_xmm1.case.json` is a literal `F2 0F 5C /r` Long64
XMM0,XMM1 scalar binary64 subtraction. Its low64 computes 3.0−1.0; the
destination high64 remains 1.0 and the source high64 contains a signaling-NaN
poison that must be ignored. The strict schema-v2 profile covers all 256
canonical register pairs and requires MXCSR.OM=1 and MXCSR.UM=1; DAZ/reserved
bits are rejected while RC, FTZ, IM/DM/PM and previous sticky flags are accepted.
The independent oracle uses raw-u64 integer subtraction and rounding. Local
checks cover all register pairs and aliases, high-half preservation/ignoring,
all production RC modes, NaNs, infinity-invalid, subnormal, FTZ tiny output,
masked overflow, unmasked PM/DM atomicity, fetch/gates/CPL, result tampering and
interpreter-only backend denial. This is local evidence, not physical acceptance.
Memory operands, other modes and full unmasked OE/UE behavior remain outside the
slice. Sources: Intel SDM Rev. 093 Vol. 2A Type 3, Table 2-20; Vol. 2B SUBSD
instruction entry; Vol. 1 §§4.9.1.4–.6, 11.5.3; Vol. 3A Event 19/Table 7-8.
See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ap--bounded-masked-range-long64-subsd-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37ap--bounded-masked-range-long64-subsd-xmm-register-pairs)
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ap--subsd-register-pair-interpreter-profile-2026-10-30).

### S2-37ar bounded masked-range MULPD register-pair profile

`s2_37ar_mulpd_xmm0_xmm1.case.json` is a literal canonical `66 0F 59 /r`
Long64 packed binary64 multiply. The fixture computes `3.0×1.0` exactly in
lane 0 and `(1+2^-52)×1.5` with one final rounding in lane 1. The strict
schema-v2 profile covers all 256 register pairs, requires MXCSR.OM=1 and
MXCSR.UM=1, rejects DAZ/reserved bits, and accepts all RC modes, FTZ, IM/DM/PM
and old sticky flags. Its independent raw-u64 integer oracle has no host FP or
production arithmetic helper. Local tests cover register aliases, halfway
rounding, the separate `minimum-normal×(1−2^-53)` midpoint and the
`0x0010000000000001×0x3feffffffffffffe` WPR tininess boundary for every RC and
FTZ setting, atomic full-XMM replacement, and interpreter-only backend denial.
The latter classifies underflow from the precision-rounded unbounded-exponent
result: RN/RU keep minimum-normal with PE only; RD/RZ return maximum-subnormal
with UE|PE or signed zero under FTZ. This is local evidence, not physical
acceptance; memory operands, other
modes and full unmasked OE/UE behavior remain outside the slice. Sources: Intel
SDM Rev. 093 Vol. 2B MULPD pp. 4-142–4-143; Vol. 2A Type 2/Table 2-19;
Vol. 1 §§4.9.1.4–.6, 11.5.3 and DAZ/FTZ; Vol. 3A Event 19/Table 7-8; Intel
Floating-Point Reference Sheet v2.13. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ar--bounded-masked-range-long64-mulpd-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37ar--bounded-masked-range-long64-mulpd-xmm-register-pairs)
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ar--mulpd-register-pair-interpreter-profile-2026-10-30).

### S2-37as bounded masked-range MULPS register-pair profile

`s2_37as_mulps_xmm0_xmm1.case.json` is a literal canonical `0F 59 /r` Long64
packed binary32 multiply. It checks exact `3.0×1.0`, one inexact halfway lane,
and positive/negative products. The strict schema-v2 profile covers all 256
XMM pairs, requires MXCSR.OM=1 and MXCSR.UM=1, rejects DAZ/reserved bits, and
accepts all RC, FTZ, IM/DM/PM and old sticky bits. The independent oracle uses
raw integer significands, without host FP or production arithmetic helpers.
Runner checks cover every alias, malformed encodings, SSE/CR0/CR4 gates at
CPL0–3, both precision-rounded tiny boundaries over all RC/FTZ settings,
halfway rounding, signed overflow, NaN/SNaN ordering, atomic pre/post faults,
and interpreter-only backend admission. Local evidence does not establish
physical acceptance; memory operands, other modes and full unmasked OE/UE
behavior remain outside the slice. Sources: Intel SDM Rev. 093 Vol. 2B MULPS
pp. 4-145–4-147; Vol. 2A Type 2/Table 2-19; Vol. 1 §§4.9.1.4–.6, 11.5.3 and
DAZ/FTZ; Vol. 3A Event 19/Table 7-8; Intel Floating-Point Reference Sheet v2.13.
See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37as--bounded-masked-range-long64-mulps-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37as--bounded-masked-range-long64-mulps-xmm-register-pairs), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37as--mulps-register-pair-interpreter-profile).


### S2-37at bounded masked-range MULSD register-pair profile

`s2_37at_mulsd_xmm0_xmm1.case.json` is a literal canonical `F2 0F 59 /r`
Long64 scalar binary64 multiply. It checks `3.0×1.0`, preserves the old
destination high64 and ignores the source high64. The strict schema-v2 profile
covers all 256 XMM pairs, requires OM=UM=1, rejects DAZ/reserved MXCSR bits, and
accepts all RC, FTZ, IM/DM/PM and sticky flags. Its independent raw-u64 oracle
uses integer significands and a scalar wrapper, without host FP or production
arithmetic helpers. Runner checks cover aliases, both WPR minimum-normal
boundaries under all RC/FTZ combinations, halfway rounding, signed overflow,
NaN/SNaN, invalid and subnormal cases, unmasked pre/post event atomicity, and
JIT/native denial; direct ISA tests cover malformed encodings, full fetch,
SSE2/EM/OSFXSR/TS gates at CPL0–3 and no data callbacks. Local checks do not
establish physical acceptance; memory operands, other modes and full unmasked
OE/UE behavior remain outside this slice. Sources: Intel SDM Rev. 093 Vol. 2B
MULSD pp. 4-148–4-149; Vol. 2A Type 3/Table 2-20; Vol. 1 §§4.9.1.4–.6,
11.5.3 and DAZ/FTZ; Vol. 3A Event 19/Table 7-8; Intel Floating-Point
Reference Sheet v2.13. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37at--bounded-masked-range-long64-mulsd-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37at--bounded-masked-range-long64-mulsd-xmm-register-pairs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37at--mulsd-register-pair-interpreter-profile).

### S2-37au bounded masked-range MULSS register-pair profile

`s2_37au_mulss_xmm0_xmm1.case.json` is a literal canonical `F3 0F 59 /r`
Long64 scalar binary32 multiply. It checks `3.0×1.0`, preserves destination
high96 and ignores source high96. The schema-v2 profile covers all 256 XMM
pairs, requires OM=UM=1, rejects DAZ/reserved MXCSR bits, and accepts all RC,
FTZ, IM/DM/PM and old sticky flags. Its independent raw-u32 integer oracle
reuses the independent finite-lane arithmetic and has a scalar wrapper; it uses
no host FP or production arithmetic helper. Focused tests cover aliases,
source/destination high-half poison, both WPR minimum-normal boundaries under
all RC×FTZ combinations, positive and negative halfway rounding, exact tiny and
FTZ, NaN/SNaN selection, QNaN DE suppression, zero×infinity, unmasked IE/DE
pre-events, unmasked PM atomicity, gates and fetch priority at CPL0–3, no data
callbacks, malformed encodings, result tampering and JIT/native denial. Local
checks do not establish physical acceptance; memory operands, other modes and
full unmasked OE/UE delivery remain outside this slice. Sources: Intel SDM
Rev. 093 Vol. 2B MULSS pp. 4-150–4-151; Vol. 2A Type 3/Table 2-20; Vol. 1
§§4.9.1.4–.6 and 11.5.3; Vol. 3A Event 19/Table 7-8; Intel Floating-Point
Reference Sheet v2.13. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37au--bounded-masked-range-long64-mulss-xmm-register-pairs),
[plan](../../../../docs/plans/STAGE_2.md#s2-37au--bounded-masked-range-long64-mulss-xmm-register-pairs), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37au--mulss-register-pair-interpreter-profile).

### S2-37av bounded masked-range DIVPD register-pair profile

`s2_37av_divpd_xmm0_xmm1.case.json` is a literal canonical `66 0F 5E /r`
Long64 packed binary64 divide. Lane 0 checks 1/3; lane 1 checks minimum-normal
/ 2^53, which is a half-minsub tie. The strict schema-v2 oracle uses raw-u64
integer quotient/remainder arithmetic with no host FP or production helper. It
covers all 256 register pairs, masks OM/UM on, rejects DAZ/reserved MXCSR bits,
and accepts RC, FTZ, IM/DM/ZM/PM and old stickies. Runtime checks include
NaN/invalid/divide-by-zero/subnormal ordering, cross-lane pre-event aggregation,
WPR tininess, FTZ, exact and inexact subnormal results, gates/fetch/CPL0–3,
no data callbacks, malformed encodings, result tampering and backend denial.

`maxsub/2` is exactly halfway between two subnormal encodings and raises UE|PE;
minimum-normal/2 is an exact subnormal unless FTZ flushes it. A subnormal
numerator divided by zero with ZM=1 and DM=0 raises ZE only. The profile remains
interpreter-only and does not claim memory forms or physical acceptance. Sources
and limits are recorded in the [plan](../../../../docs/plans/STAGE_2.md#s2-37av--bounded-masked-range-long64-divpd-xmm-register-pairs),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37av--bounded-masked-range-long64-divpd-xmm-register-pairs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37av--divpd-register-pair-interpreter-profile).

### S2-37aw bounded masked-range DIVPS register-pair profile

`s2_37aw_divps_xmm0_xmm1.case.json` is a literal canonical `0F 5E /r`
Long64 packed binary32 divide. Its four lanes cover 1/3, the minnormal/2^24
half-minsub tie, the `00ffffff/40000000` midpoint below minnormal, and
minnormal/nextafter(1,+). The strict schema-v2
oracle uses raw-u32 exact-rational integer arithmetic without host FP or
production helpers. It covers all 256 XMM pairs, requires OM/UM=1, rejects DAZ
and reserved MXCSR bits, and accepts RC, FTZ, IM/DM/ZM/PM and prior stickies.
Runtime checks cover NaN payload and SNaN handling, invalid/divide-by-zero/
denormal priority, cross-lane IE/ZE/DE pre-event aggregation, four-lane WPR
underflow/overflow and FTZ, pre/post atomicity, gates/fetch/CPL0–3, no data
callbacks, malformed encodings, result tampering and backend denial.

The profile is interpreter-only and makes no memory-form, memory-alignment or
physical-acceptance claim. The p. 3-272 Type 2/Table 2-19 cross-reference in
Intel SDM Rev. 093 is for VEX forms; the legacy SSE rows are the source for this
encoding. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37aw--bounded-masked-range-long64-divps-xmm-register-pairs),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37aw--bounded-masked-range-long64-divps-xmm-register-pairs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37aw--divps-register-pair-interpreter-profile).

### S2-37ax bounded masked-range DIVSD register-pair profile

`s2_37ax_divsd_xmm0_xmm1.case.json` is a literal canonical `F2 0F 5E /r`
Long64 register fixture. Its low qwords exercise 1.0/3.0; the destination high
qword is an SNaN canary and the source high qword is a subnormal canary. They
must not contribute exceptions, and the destination high qword must be
preserved. The local schema independently checks exact-rational rounding and
pre/post exception atomicity. Sources and scope limits are recorded in the
[plan](../../../../docs/plans/STAGE_2.md#s2-37ax--canonical-long64-divsd-xmm-register-pairs),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ax--bounded-masked-range-long64-divsd-xmm-register-pairs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ax--divsd-register-pair-interpreter-profile).


### S2-37ay bounded masked-range DIVSS register-pair profile

`s2_37ay_divss_xmm0_xmm1.case.json` is a literal canonical `F3 0F 5E /r`
Long64 scalar binary32 divide. Its low lanes exercise 1/3, while destination
high96 contains signaling-NaN/integer canaries and source high96 contains
zero/infinity canaries; those upper lanes do not participate. The strict
schema-v2 profile admits all 256 XMM register pairs, requires OM/UM=1 and
rejects DAZ/reserved MXCSR bits. It tests RC/FTZ rounding, WPR tininess, NaN
quieting, IE/ZE/DE priorities, pre/post atomicity, gates/fetch/CPL, invalid
encodings, result tampering and JIT/native denial. Subnormal/zero with ZM=1 and
DM=0 raises ZE only; infinity/zero remains nonfaulting with ZM unmasked.

DIVSS is SSE. The Type 3 note on Intel SDM Rev. 093 Vol. 2A p. 3-276 refers to
VEX forms, not legacy DIVSS; the profile cites legacy SSE rows in Table 2-20.
Memory forms and physical acceptance are not claimed. See the [plan]
(../../../../docs/plans/STAGE_2.md#s2-37ay--canonical-long64-divss-xmm-register-pairs),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ay--canonical-long64-divss-xmm-register-pairs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ay--divss-register-pair-interpreter-profile).

### S2-37az ADDPD register-pair profile

`s2_37az_addpd_xmm0_xmm1.case.json` is a handwritten canonical Long64
`66 0F 58 /r` literal. Lane 0 adds 2^-53 to 1.0 and rounds to even with PE;
lane 1 adds negative zero to 1.0 exactly. The schema-v2 profile admits all
256 XMM pairs, requires OM=UM=1, rejects DAZ/reserved MXCSR bits, and accepts
RC, FTZ, IM/DM/ZM/PM and sticky flags. An independent raw-u64 integer oracle
checks NaN selection/quieting, IE/DE pre-event aggregation, addition rounding,
overflow midpoint versus directed overflow, WPR tininess and FTZ without host
FP or production arithmetic helpers. Runtime coverage includes all pairs and
aliases, large exponent gaps with masked/unmasked DE, old sticky flags,
NaN/DE suppression, cross-lane IE/DE suppression of post-flags,
gates/fetch/CPL0–3/no callbacks, malformed inputs and JIT/native denial. The
handwritten literal itself contains only the halfway and signed-zero vectors;
the remaining cases are generated in the oracle and runner matrix.

Sources: Intel SDM Rev. 093 Vol. 2A ADDPD pp. 3-16–3-18 and legacy SSE rows in
Table 2-27. The p. 3-18 Type 2/Table 2-19 reference applies to VEX forms, not
legacy ADDPD. No memory alignment or physical-acceptance claim is made. See the
[plan](../../../../docs/plans/STAGE_2.md#s2-37az--canonical-long64-addpd-xmm-register-pairs),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37az--canonical-long64-addpd-xmm-register-pairs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37az--addpd-register-pair-interpreter-profile).

### S2-37du ADDPD m128 memory profile

`s2_37du_addpd_m128_memory_source_two_binary64_lanes.case.json` is a
handwritten exact legacy `66 0F 58 00` fixture. With old XMM0 lanes 1.0 and
2.0, its memory lanes 2.0 and 5.0 produce 3.0 and 7.0. The strict masked-only
profile requires SSE2, aligned canonical m128 `[RAX]`, and exactly one read16;
its independent validator reads the RHS from these memory bytes. The runner
also verifies #UD/#GP atomicity, control/result tampering and JIT/native denial.
See the [plan](../../../../docs/plans/STAGE_2.md#s2-37du--masked-only-legacy-long64-addpd-xmm0m128-rax),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37du--masked-only-long64-addpd-xmm0m128-rax),
and [standalone report](../../../../docs/reports/S2_37DU_ADDPD_MEMORY.md).


### S2-37ba ADDPS register-pair profile

`s2_37ba_addps_xmm0_xmm1.case.json` is a handwritten canonical Long64
`0F 58 /r` literal. Its four lanes cover positive and negative halfway ties,
an exact minsub result, and the positive overflow midpoint. The strict
schema-v2 profile admits all 256 XMM pairs, requires OM=UM=1, rejects DAZ and
reserved MXCSR bits, and accepts RC, FTZ, IM/DM/ZM/PM and old stickies. The
independent raw-u32 integer oracle checks four-lane NaN selection/quieting,
IE/DE pre-event aggregation, exact versus FTZ subnormal, directed rounding,
overflow midpoint versus larger overflow, and large-gap DE|PE without host FP
or production helpers. Runtime checks cover four-lane XMM atomicity, all RC and
FTZ controls, pre-event and PM exits, full-fetch gate priority, CPL0–3, aliasing,
malformed encodings, invalid MXCSR and backend denial.

ADDPS is SSE, not SSE2. Intel SDM Rev. 093 Vol. 2A pp. 3-19–3-21 supplies the
instruction text; the legacy SSE rows in Table 2-27 supply the encoding and
feature classification. The p. 3-21 Type 2/Table 2-19 note refers to VEX forms.
This local interpreter profile claims no memory alignment or physical
acceptance behavior. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37ba--canonical-long64-addps-xmm-register-pairs),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ba--canonical-long64-addps-xmm-register-pairs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ba--addps-register-pair-interpreter-profile).

### S2-37bb ADDSD register-pair profile

`s2_37bb_addsd_xmm0_xmm1.case.json` is a handwritten canonical Long64
`F2 0F 58 /r` literal. Low64 adds 2^-53 to 1.0 and rounds to even with PE;
the destination high64 contains an SNaN canary and is preserved, while the
source high64 contains a subnormal canary that scalar execution ignores. The
strict schema-v2 profile admits all 256 XMM pairs, requires OM=UM=1, rejects
DAZ/reserved MXCSR bits, and accepts RC, FTZ, IM/DM/ZM/PM and old stickies. Its
independent raw-u64 scalar wrapper checks four RC halfway modes, signed zero,
exact and FTZ tiny sums, large-gap DE|PE, overflow midpoint versus larger
overflow, first-NaN selection and quieting, opposite infinities, and infinity
plus subnormal without host FP or production helpers. Runtime checks cover
all pairs and aliases, destination-high64 preservation, pre/post event
atomicity, full-fetch gate ordering, CPL0–3, no data callbacks, malformed
encodings, tampering, and JIT/native denial.

Sources: Intel SDM Rev. 093 Vol. 2A ADDSD pp. 3-22–3-23 and legacy SSE rows on
p. 2-28. The p. 3-23 Type 3/Table 2-20 reference concerns VEX forms, not
legacy ADDSD. No memory alignment or physical-acceptance claim is made. See the
[plan](../../../../docs/plans/STAGE_2.md#s2-37bb--canonical-long64-addsd-xmm-register-pairs),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bb--canonical-long64-addsd-xmm-register-pairs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bb--addsd-register-pair-interpreter-profile).

### S2-37bc ADDSS register-pair profile

`s2_37bc_addss_xmm0_xmm1.case.json` is a handwritten canonical Long64
`F3 0F 58 /r` literal. Low32 adds 2^-24 to 1.0 and rounds to even with PE;
destination.high96 contains SNaN/integer canaries and remains unchanged, while
source.high96 contains poison ignored by scalar execution. The strict schema-v2
profile admits all 256 XMM pairs, requires OM=UM=1, rejects DAZ/reserved MXCSR
bits, and accepts RC, FTZ, IM/DM/ZM/PM and old stickies. Its independent raw-u32
scalar wrapper checks positive and negative halfway vectors, exact and FTZ tiny
sums, directed overflow midpoint and larger overflow, large-gap DE|PE, NaN
selection/quieting, opposite infinities, and infinity plus subnormal without
host FP or production helpers. Runtime checks cover all pairs and aliases,
low32-only writes, pre/post event atomicity, full-fetch gate ordering, CPL0–3,
no data callbacks, malformed encodings, tampering, and JIT/native denial.

Sources: Intel SDM Rev. 093 Vol. 2A ADDSS pp. 3-24–3-25 and legacy SSE rows on
p. 2-28. The p. 3-25 Type 3/Table 2-20 reference concerns VEX forms, not
legacy ADDSS. The apparent “double precision result” and “64-bit memory” text
errors are not used; the opcode row and low32 Operation semantics define the
profile. No memory alignment or physical-acceptance claim is made. See the
[plan](../../../../docs/plans/STAGE_2.md#s2-37bc--canonical-long64-addss-xmm-register-pairs),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bc--canonical-long64-addss-xmm-register-pairs),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bc--addss-register-pair-interpreter-profile).

### S2-37bd: bounded masked-range CVTPD2PS register pair

`s2_37bd_cvtpd2ps_xmm0_xmm1.case.json` is a schema-v2 Long64 interpreter-only
fixture for canonical `66 0F 5A /r` register pairs. It converts two source
binary64 lanes to destination.low64, zeros destination.high64 on success, and
uses poisoned destination bytes to detect accidental arithmetic input or
partial upper-state retention. The independent result validator uses integer
bit construction and rounding. All 256 register encodings, aliases, exceptions,
rounding boundaries, FTZ, NaN payloads, and backend denial are covered by local
unit/integration scenarios. This is not physical acceptance or memory-form
support.


### S2-37be canonical CVTSD2SS register pairs

`s2_37be_cvtsd2ss_asymmetric_register_pair.case.json` is the literal schema-v2 anchor for canonical legacy `F2 0F 5A C1`. It converts the source low64 value 1+2^-24, puts a signaling-NaN-shaped pattern in source.high64, and uses destination high96 canaries. The independent raw-integer oracle and runner matrix cover all 256 register pairs, aliases, RC/tie and normal/subnormal boundaries, exact/inexact tiny values, FTZ, overflow, signed QNaN/SNaN payloads, DE/IE/PE mask and sticky behavior, atomic faults, gates/fetch/CPL, malformed encodings and JIT/native denial. Memory forms and physical acceptance are not claimed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37be--canonical-long64-cvtsd2ss-xmm-register-pairs), [plan](../../../../docs/plans/STAGE_2.md#s2-37be--canonical-long64-cvtsd2ss-xmm-register-pairs), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37be--cvtsd2ss-register-pair-interpreter-profile).

### S2-37bf CVTPI2PD memory-only RAX

`s2_37bf_cvtpi2pd_memory_rax.case.json` is the schema-v2 anchor for only legacy `66 0F 2A 00`. It places asymmetric signed i32 extrema at unaligned canonical RAX with AC clear. The interpreter reads one eight-byte span, converts both values exactly to XMM0 binary64 lanes, and preserves MXCSR and x87 state. The independent integer oracle and runner matrix cover signed boundaries, 2^24 precision boundaries, NaN-shaped integers, RC/sticky preservation, gates, malformed/tampered fixtures and JIT/native denial. Local tests cover canonical overflow, access count, canaries and rollback. No alignment-check or guest #PF claim is made; physical acceptance remains open. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bf--canonical-long64-cvtpi2pd-memory-only-rax), [plan](../../../../docs/plans/STAGE_2.md#s2-37bf--canonical-long64-cvtpi2pd-memory-only-rax), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bf--cvtpi2pd-memory-interpreter-profile).


### S2-37bg CVTPI2PS memory-only RAX

`s2_37bg_cvtpi2ps_memory_rax.case.json` is the schema-v2 anchor for only legacy `0F 2A 00`. Its asymmetric signed i32 lanes sit at unaligned canonical RAX with AC clear; XMM0.high64 and x87 payload/pointer canaries detect unintended writes. The expected transition writes two binary32 values under MXCSR.RC, ORs PE, and sets x87 TOP/tag_word to zero while preserving the rest of x87 state. The independent integer oracle and runner matrix cover four rounding modes, positive/negative ties, extrema, 2^24 boundaries, gates, pending-x87/PM=0 refusal, malformed/trailing bytes, result tampering and JIT/native denial. Physical acceptance and other operand forms remain out of scope. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bg--canonical-long64-cvtpi2ps-memory-only-rax), [plan](../../../../docs/plans/STAGE_2.md#s2-37bg--canonical-long64-cvtpi2ps-memory-only-rax), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bg--cvtpi2ps-memory-interpreter-profile).

### S2-37bh CVTTPS2PI MM0,XMM1 masked register profile

`s2_37bh_cvttps2pi_mm0_xmm1_masked.case.json` is the literal schema-v2 anchor for exact `0F 2C C1`. XMM1.low64 supplies a fractional binary32 lane and QNaN; XMM1.high64 is poison. The expected masked transition aggregates IE and PE, stores the integer pair in physical x87 slot 0 with exponent word `ffff`, and sets TOP=0/all-valid tags. The independent integer oracle and runner matrix cover signed zeros/fractions, i32 boundaries, QNaN/SNaN/infinity/subnormal values, every MXCSR.RC, mixed flags, prior sticky state, TOP/tag variation, gates, malformed encodings, tampering and JIT/native denial. Physical x86 acceptance and other operand forms remain outside this slice. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bh--bounded-long64-cvttps2pi-mm0xmm1-masked-register-profile), [plan](../../../../docs/plans/STAGE_2.md#s2-37bh--bounded-long64-cvttps2pi-mm0xmm1-masked-register-profile), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bh--cvttps2pi-mm0xmm1-masked-register-profile).


### S2-37bj CVTPS2PI MM0,XMM1 masked register profile

`s2_37bj_cvtps2pi_mm0_xmm1_masked.case.json` is the literal schema-v2 anchor for exact `0F 2D C1`. XMM1.low64 carries a binary32 fraction and QNaN; high64 is poison. The expected MXCSR.RC rounded transition aggregates masked IE/PE and writes the integer pair to physical x87 slot 0 with exponent word `ffff`, TOP=0 and all-valid tags. The independent integer oracle and runner matrix exercise all rounding modes, ties, signed limits, invalids/subnormals, sticky flags, all incoming TOP/tag encodings, gates, malformed forms, tampering and JIT/native denial. Physical x86 acceptance and broader exception behavior are not claimed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bj--bounded-long64-cvtps2pi-mm0xmm1-masked-register-profile), [plan](../../../../docs/plans/STAGE_2.md#s2-37bj--bounded-long64-cvtps2pi-mm0xmm1-masked-register-profile), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bj--cvtps2pi-mm0xmm1-masked-register-profile).

### S2-37bk CVTPD2PI MM0,XMM1 masked register profile

`s2_37bk_cvtpd2pi_mm0_xmm1_masked.case.json` is the literal schema-v2 anchor
for exactly `66 0F 2D C1`. XMM1 contains two binary64 lanes, 1.5 and QNaN. The
expected transition rounds both lanes under MXCSR.RC, aggregates masked IE/PE,
stores the signed pair into physical x87 slot 0 with exponent word `ffff`, and
sets TOP=0/all-valid tags. The independent raw-u64 oracle and runner matrix
cover all RC modes, signed ties/limits, QNaN/SNaN/infinity/subnormal inputs,
mixed and old sticky flags, all TOP/tag states, clear non-IM/PM masks, gate
priority, CPL0–3, malformed encodings, tampering and JIT/native denial. Full
instruction fetch precedes the selected SSE2/EM/OSFXSR #UD then TS #NM profile
order. Pending x87 states are refused; #MF delivery and physical x86 acceptance
are not claimed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bk--bounded-long64-cvtpd2pi-mm0xmm1-masked-register-profile),
[plan](../../../../docs/plans/STAGE_2.md#s2-37bk--bounded-long64-cvtpd2pi-mm0xmm1-masked-register-profile),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bk--cvtpd2pi-mm0xmm1-masked-register-profile).

### S2-37bl CVTTPD2PI MM0,XMM1 masked register profile

`s2_37bl_cvttpd2pi_mm0_xmm1_masked.case.json` is the literal schema-v2 anchor
for exactly `66 0F 2C C1`. Its XMM1 lanes are binary64 1.5 and QNaN, and MXCSR
selects round-up to make the CVTT mode independence observable. The expected
transition truncates the first lane to 1, maps the invalid second lane to
`0x80000000`, sets both sticky IE and PE, and writes the pair to physical x87
slot 0 with exponent word `ffff`, TOP=0 and all-valid tags. The independent
raw-u64 oracle and runner matrix cover all RC modes, signed values and limits,
QNaN/SNaN/infinity/subnormal, mixed IE/PE, prior sticky state, TOP/tag
variation, SSE2/EM/OSFXSR/TS gates, CPL0–3, malformed encodings, tampering and
JIT/native denial. Pending x87 states are refused; #MF delivery and physical
x86 acceptance are not claimed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bl--bounded-long64-cvttpd2pi-mm0xmm1-masked-register-profile),
[plan](../../../../docs/plans/STAGE_2.md#s2-37bl--bounded-long64-cvttpd2pi-mm0xmm1-masked-register-profile),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bl--cvttpd2pi-mm0xmm1-masked-register-profile).

### S2-37bm CVTPI2PD XMM0,MM1 register profile

`s2_37bm_cvtpi2pd_xmm0_mm1_register.case.json` is the literal schema-v2 anchor
for exact `66 0F 2A C1`. With TOP=3, physical x87 slot 1/MM1 contains signed
i32 extrema while logical ST(1) points at a different slot; the fixture also
sets MXCSR to `0x000000000000df80` (round-up with FTZ) to demonstrate these controls do not gate or change exact conversion.
The expected state writes XMM0, clears TOP, marks every tag valid, preserves
all x87 payloads and exponent words, preserves MXCSR and other XMM registers,
and makes no memory access. Its independent oracle and runner matrix cover all
TOP values, representative tag words, varied valid MXCSR, gates, malformed
encodings, pending-x87 refusal and backend denial. No physical x86 acceptance
is claimed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bm--bounded-long64-cvtpi2pd-xmm0mm1-register-profile),
[plan](../../../../docs/plans/STAGE_2.md#s2-37bm--bounded-long64-cvtpi2pd-xmm0mm1-register-profile),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bm--cvtpi2pd-xmm0mm1-register-profile).

### S2-37bn CVTPI2PS XMM0,MM1 register profile

`s2_37bn_cvtpi2ps_xmm0_mm1_register.case.json` is the literal schema-v2 anchor
for exact `0F 2A C1`. Physical x87 slot 1/MM1 holds signed i32 extrema while
TOP=3 and tags are nontrivial; logical ST(1) points elsewhere. MXCSR uses
`0x000000000000df80` (round-up and FTZ) to exercise rounding and preservation.
The expected state rounds both lanes under RC, writes XMM0.low64 while
preserving high64, aggregates sticky PE for valid inexact lanes, clears TOP and
sets all tags valid, and preserves all x87 payloads/exponents and MXCSR except
PE. The independent integer oracle and runner matrix cover RC, signed and
2^24 boundaries, TOP 0–7, representative tag words, gates, malformed
encodings, PM/pending-x87 rejection, output tampering and backend denial. No
physical x86 acceptance is claimed. See the [CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bn--bounded-long64-cvtpi2ps-xmm0mm1-register-profile),
[plan](../../../../docs/plans/STAGE_2.md#s2-37bn--bounded-long64-cvtpi2ps-xmm0mm1-register-profile),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bn--cvtpi2ps-xmm0mm1-register-profile).

### S2-37bo CVTTPS2PI MM0,[RAX] memory profile

`s2_37bo_cvttps2pi_mm0_memory_rax.case.json` is the literal schema-v2 anchor
for exact `0F 2C 00`. It reads one unaligned 8-byte span from canonical RAX
with AC=0. The lanes exercise valid inexact conversion and masked invalid
conversion from clear incoming sticky flags; expected state aggregates PE/IE
and stores the pair in physical x87 slot 0 with exponent word `ffff`, TOP=0
and all-valid tags. The independent raw-bit oracle and runner matrix cover RC
independence, explicit IE|PE generation, exact-lane preservation of incoming
IE/PE histories, gates, strict encoding and backend denial. Direct ISA runtime
tests cover noncanonical and overflow pre-read refusals, AC/pending-x87/IM/PM
refusals with zero reads and unchanged CPU state, full-fetch ordering, a
successful unaligned canonical read, and one attempted read whose injected
failure leaves CPU state unchanged. The runner covers the highest valid
canonical span. Physical x86 acceptance is not claimed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bo--bounded-long64-cvttps2pi-mm0rax-memory-profile),
[plan](../../../../docs/plans/STAGE_2.md#s2-37bo--bounded-long64-cvttps2pi-mm0rax-memory-profile),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bo--cvttps2pi-mm0rax-memory-profile).

### S2-37bp CVTPS2PI MM0,[RAX] memory profile

`s2_37bp_cvtps2pi_mm0_memory_rax.case.json` is the literal schema-v2 anchor
for exact `0F 2D 00`. Its single unaligned m64 contains 1.25 and QNaN under
round-up MXCSR with clear incoming status flags, demonstrating RC rounding and
mixed PE/IE generation. Success writes physical MM0 with exponent `ffff`,
TOP=0 and all tags valid. The SDM operand table identifies xmm/m64 and the
Operation consumes `SRC[63:0]`, despite the prose's “128-bit memory location”
phrase; the fixture therefore provides exactly eight source bytes. The
independent raw-bit oracle and runner matrix exercise all RC modes, invalid
and fractional-valid lanes, old sticky histories, TOP/tag variation, gates,
malformed inputs, output tampering and backend denial. ISA execution tests
provide the one-read and read-failure atomicity evidence, along with span and
no-read refusal coverage. No physical x86 acceptance is claimed. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bp--bounded-long64-cvtps2pi-mm0rax-memory-profile),
[plan](../../../../docs/plans/STAGE_2.md#s2-37bp--bounded-long64-cvtps2pi-mm0rax-memory-profile),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bp--cvtps2pi-mm0rax-memory-profile).

### S2-37bq CVTTPD2PI MM0,[RAX] aligned m128 memory profile

`s2_37bq_cvttpd2pi_mm0_memory_rax.case.json` is the literal schema-v2 fixture
for exact `66 0F 2C 00`. It reads one aligned 16-byte source at RAX; the lanes
are 1.5 and QNaN with MXCSR round-up and clear incoming sticky flags, so the
expected result proves RC-independent truncation and generates PE|IE. Success
writes physical x87 slot 0/MM0 with exponent word `ffff`, sets TOP=0/all tags
valid, preserves remaining x87/XMM state and preserves AC=1 in RFLAGS. The
runner matrix covers all four CPLs and AC values for aligned success and all
15 misaligned offsets at representative CPL/AC; direct ISA tests cover those
offsets across the full CPL×AC cross product. It also covers binary64
boundaries, all RC modes, exact-lane sticky histories and noncanonical/
canonical-boundary probes, strict gates, tampered output and backend refusal.
Direct ISA tests establish overflow rejection, zero callback
and unchanged state for pre-read refusals, exactly one 16-byte read for a valid
source, and rollback after an injected read failure; runner execution does not
record callback counts. Misaligned canonical-end/overflow probes also violate
alignment, so they do not establish independent fault priority. No physical
x86 acceptance is claimed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bq--bounded-long64-cvttpd2pi-mm0rax-aligned-m128-memory-profile),
[plan](../../../../docs/plans/STAGE_2.md#s2-37bq--bounded-long64-cvttpd2pi-mm0rax-aligned-m128-memory-profile),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bq--cvttpd2pi-mm0rax-aligned-m128-memory-profile).

### S2-37br CVTPD2PI MM0,[RAX] aligned m128 memory profile

`s2_37br_cvtpd2pi_mm0_memory_rax.case.json` is the literal schema-v2 fixture
for exact `66 0F 2D 00`. It uses the aligned 16-byte source lanes +1.5 and QNaN
with MXCSR round-up, so the valid lane rounds to 2 and contributes PE while the
invalid lane contributes `0x80000000` and IE. Incoming sticky flags are clear,
AC is set on the aligned access, and x87 payload canaries exercise the MMX
transition. The fixture is evidence for rounding-control use, mixed IE|PE and
preservation; it does not claim physical x86 acceptance.

The runner matrix checks each RC mode, rounded endpoint/range behavior,
clear-flag generation and sticky preservation, aligned CPL×AC success, all 15
misalignment offsets at representative CPL/AC, TOP/tag state, malformed and
tampered results, and backend denial. Direct ISA tests prove the full
misalignment×CPL×AC matrix has #GP(0), zero reads and unchanged CPU state, plus
one exact 16-byte read on success and rollback after an injected read error.
They also cover fetch/gates and pre-read span refusals. No physical x86
acceptance is claimed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37br--bounded-long64-cvtpd2pi-mm0rax-aligned-m128-memory-profile),
[plan](../../../../docs/plans/STAGE_2.md#s2-37br--bounded-long64-cvtpd2pi-mm0rax-aligned-m128-memory-profile),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37br--cvtpd2pi-mm0rax-aligned-m128-memory-profile).

### S2-37bs CVTDQ2PD XMM0,[RAX] m64 memory profile

The seven `s2_37bs_cvtdq2pd_*.case.json` files are handwritten schema-v2
fixtures for exact legacy `F3 0F E6 00`. They source exactly two signed i32
values from an unaligned m64 at RAX with AC=0 and replace all of XMM0 with the
exact binary64 results. Inputs cover zero/one, ±1, signed extrema, asymmetric
values around 2^24, integer words that resemble binary NaNs, and a canonical-hole span fault. MXCSR spans
all four RC values plus varied status/control histories, and fixtures include
pending x87 status and tag/payload canaries. The interpreter preserves MXCSR
and all x87 state. Native/JIT/C ABI remain closed, and physical x86 acceptance
was not run. Provenance cites Intel SDM Rev. 093 Vol. 2A CVTDQ2PD pp. 3-208–3-210,
Vol. 3A §16.1.3 Table 16-1 p. 16-3, and Vol. 1 Table 14-21 (unaligned access
with AC=0) plus Appendix C §C.4/Table C-4 p. C-5 (no SIMD numeric exception
conditions). See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bs--bounded-long64-cvtdq2pd-xmm0rax-m64-memory-profile), [plan](../../../../docs/plans/STAGE_2.md#s2-37bs--bounded-long64-cvtdq2pd-xmm0rax-m64-memory-profile), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bs--cvtdq2pd-xmm0rax-m64-memory-profile).

### S2-37bt CVTDQ2PS XMM0,[RAX] aligned m128 memory profile

The six `s2_37bt_cvtdq2ps_*.case.json` files are literal schema-v2 fixtures for
exact legacy `0F 5B 00`. Success fixtures read one aligned m128 from RAX and
replace XMM0 with four MXCSR.RC-rounded binary32 lanes. They use PM=1 while
allowing IM and the other exception masks to be clear; one fixture checks the
local PM=0 refusal. Inputs include signed boundaries, rounding carry, old PE,
misalignment and pending-x87 canaries. The instruction preserves every x87
field and only raises sticky PE for valid inexact lanes. Native/JIT/C ABI stay
closed; the fixtures do not claim physical x86 acceptance.

The runner matrix covers all four rounding modes × all 16 inexact lane masks
from clear flags, exact-lane sticky PE histories, aligned AC=0/1, every
misalignment offset at representative CPL/AC, state/memory preservation,
malformed and tampered results, foreign controls and backend denial. Direct
ISA tests establish the full misalignment offset × CPL0–3 × AC0/1 matrix as
pre-read #GP(0) with no callbacks and unchanged state, as well as aligned
success, one exact 16-byte read, fetch/gates, span/PM refusals and read-failure
rollback. Runner output itself does not record callback counts.

Provenance: Intel SDM Rev. 093 Vol. 2A CVTDQ2PS pp. 3-211–3-213; Vol. 1
§§11.5.2.6–11.5.3.2 for Precision masking and Appendix C §C.4/Table C-4
p. C-5 for SIMD exception-condition classification ([official Vol. 1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436)); Vol. 3A §16.1.3 Table 16-1
p. 16-3 for legacy SSE2 gates ([official Vol. 3A PDF](https://cdrdv2.intel.com/v1/dl/getContent/671190)); Vol. 1 Table 14-21 for alignment behavior. The VEX-only Type 2/Table 2-19 entry is not used. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bt--bounded-long64-cvtdq2ps-xmm0rax-aligned-m128-memory-profile),
[plan](../../../../docs/plans/STAGE_2.md#s2-37bt--bounded-long64-cvtdq2ps-xmm0rax-aligned-m128-memory-profile),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bt--cvtdq2ps-xmm0rax-aligned-m128-memory-profile).


### S2-37bu CVTTPS2DQ XMM0,[RAX] aligned m128 memory profile

The handwritten literal `s2_37bu_cvttps2dq_m128_raw_boundaries.case.json` uses
exact `F3 0F 5B 00` and one aligned 16-byte source. Its lanes are exact
`INT32_MIN`, the largest in-range positive binary32 integer, invalid `+2^31`,
and the smallest positive subnormal. The expected output demonstrates the
indefinite integer bit pattern can also represent valid `INT32_MIN`; IE
distinguishes them, while the subnormal lane contributes PE. A pending x87 state
is carried through unchanged.

The runner matrix varies all MXCSR.RC values while proving truncation, tests
invalid-lane combinations and existing IE/PE histories, aligned AC=0/1 at
Ring0 and representative misalignment cases, strict schema controls, tampering,
and JIT/native denial. Direct ISA tests supply full offset × CPL × AC and read-callback
atomicity evidence; runner snapshots alone do not establish callback counts.
This is local interpreter evidence only, not physical x86 acceptance.

Provenance: Intel SDM Rev. 093 Vol. 2A CVTTPS2DQ pp. 3-249–3-251; Vol. 1
§11.5.2.2 and Table 14-21; Vol. 3A §16.1.3 Table 16-1 p. 16-3 ([official
Vol. 3A PDF](https://cdrdv2.intel.com/v1/dl/getContent/671190)).

### S2-37bv CVTPS2DQ XMM0,[RAX] aligned m128 rounding profile

Literal input: `s2_37bv_cvtps2dq_m128_rounding_boundaries.case.json`. It uses
exact legacy `66 0F 5B 00`, one aligned 16-byte source, and MXCSR with masked
Invalid/Precision and DAZ clear. The raw lanes exercise ±ties, a positive
subnormal, and an exact half-way value. The profile rounds according to RC before
the signed-i32 range check; invalid lanes produce the indefinite integer and IE,
valid inexact lanes contribute PE, and prior sticky status is preserved.

The focused runner varies all RC values, ties and signed subnormals, representable
and invalid range edges, mixed IE+PE, sticky histories, aligned AC at Ring0,
representative misalignment, prerequisite/MXCSR refusal, malformed/foreign
controls, tampering, and JIT/native Unsupported. Direct ISA tests cover the full
CPL×AC alignment matrix, pre-read #GP behavior for all 15 offsets, single-span
access and atomic read failure. No physical x86 acceptance was run.

Provenance: Intel SDM Rev. 093 Vol. 2A CVTPS2DQ pp. 3-225–3-227; Vol. 3A
§16.1.3 Table 16-1 p. 16-3 for legacy SSE2 gates; Vol. 1 Table 14-21 for
aligned m128 access. The VEX-only Type 2/Table 2-19 entry is not used here.

Related documentation: [Stage 2 plan](../../../../docs/plans/STAGE_2.md#s2-37bv--bounded-long64-cvtps2dq-xmm0rax-aligned-m128-memory-profile), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bv--bounded-long64-cvtps2dq-xmm0rax-aligned-m128-profile), and [stage report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bv--cvtps2dq-xmm0rax-aligned-m128-memory-profile).

### S2-37bw CVTTPD2DQ XMM0,[RAX] aligned m128 truncation profile

`s2_37bw_cvttpd2dq_m128_truncation_and_flags.case.json` is the literal exact
legacy `66 0F E6 00` schema-v2 fixture. Two raw binary64 lanes include
`2147483647.75` and a quiet NaN: successful output is INT32_MAX and the masked
indefinite value, with both IE and PE sticky; XMM0 bits 127:64 are zeroed. The
fixture deliberately carries pending x87 canaries, which this pure XMM
operation preserves without a pending-x87 gate.

The interpreter profile fetches four bytes, then applies local SSE2/EM/OSFXSR
#UD and TS #NM checks, and performs one aligned 16-byte RAX read after checked
canonical48 validation. Misalignment is #GP(0) before callback at any CPL/AC;
aligned AC=0/1 is allowed. Toward-zero conversion ignores MXCSR.RC; range is
checked after truncation. Invalid lanes add IE only; valid inexact lanes add
PE. DAZ=0 subnormals become zero+PE, with no DE. The local mask is `0xffbf`,
IM=PM=1, DAZ=0. JIT/native/C ABI remain closed and physical x86 acceptance is
not claimed. Sources: Intel SDM Rev. 093 Vol. 2A CVTTPD2DQ pp. 3-244–3-247
(legacy operation p. 3-246); Vol. 1 §§11.5.2.2, 11.5.2.6, 11.5.3, 11.5.5
and Table 14-21; Vol. 3A §§16.1.3–16.1.4, Table 16-1.

Related docs: [Stage 2 plan](../../../../docs/plans/STAGE_2.md#s2-37bw--bounded-long64-cvttpd2dq-xmm0rax-aligned-m128-memory-profile), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bw--bounded-long64-cvttpd2dq-xmm0rax-aligned-m128-profile), and [stage report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bw--cvttpd2dq-xmm0rax-aligned-m128-memory-profile).

### S2-37bx CVTPD2DQ XMM0,[RAX] aligned m128 rounding profile

`s2_37bx_cvtpd2dq_m128_rounding_and_flags.case.json` is the literal exact legacy
`F2 0F E6 00` schema-v2 case. Its two raw binary64 lanes are just above the
positive signed-i32 boundary and a QNaN, producing masked indefinite results
with IE; the initial XMM0 high64 canary is zeroed on success. The runner matrix
covers RC-sensitive signed boundaries, ties, mixed IE/PE, subnormals and old
sticky-status histories.

The profile reads one aligned 16-byte RAX span after checked canonical48 and
pre-callback alignment checks. Misalignment is #GP(0) at any CPL and AC setting;
aligned AC=0/1 is accepted. Conversion rounds by MXCSR.RC before checking range.
DAZ=0, supported mask `0xffbf`, IM=PM=1; invalid adds IE only, valid inexact
adds PE. x87 is preserved with no gate. JIT/native/C ABI remain closed; physical
x86 acceptance is not claimed. Provenance: Intel SDM Rev. 093 Vol. 2A CVTPD2DQ
pp. 3-214–3-217 (legacy operation p. 3-216); Vol. 1 §§11.5.2.2, 11.5.2.6,
11.5.3, 11.5.5, Table 14-21; Vol. 3A §§16.1.3–16.1.4, Table 16-1.

Related documentation: [Stage 2 plan](../../../../docs/plans/STAGE_2.md#s2-37bx--bounded-long64-cvtpd2dq-xmm0rax-aligned-m128-memory-profile), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bx--bounded-long64-cvtpd2dq-xmm0rax-aligned-m128-profile), and [stage report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bx--cvtpd2dq-xmm0rax-aligned-m128-memory-profile).

### S2-37by — CVTPS2PD XMM0,[RAX] m64 memory profile

`s2_37by_cvtps2pd_m64_snan_subnormal.case.json` is the literal exact legacy
`0F 5A 00` case. Its unaligned m64 contains SNaN `0x7f800001` and the smallest
negative subnormal `0x80000001`; MXCSR has FTZ set, round-up selected and PM
clear. The output demonstrates SNaN quieting with IE, exact subnormal widening
with DE, and preserved pending-x87 state. The runner matrix covers all four RC
values, FTZ on/off, representative NaN/zero/infinity/max-finite values and all
23 subnormal leading-bit positions. x87 canaries are preserved with no gate.

The schema-v2 oracle uses raw binary32 integer fields; it does not use host FP
or the production conversion helper. Direct ISA tests cover exact fetch, SSE2
gates, unaligned single-read behavior and read-failure rollback. JIT/native/C
ABI remain closed; physical x86 acceptance is not claimed. Provenance cites
Intel SDM Rev. 093 Vol. 2A CVTPS2PD pp. 3-228–3-230; Vol. 1 §§11.5.2.1–
11.5.2.2, 11.5.3, 11.5.5, Tables D-11 and 14-21; Vol. 3A §§16.1.3–16.1.4,
Table 16-1. See [plan](../../../../docs/plans/STAGE_2.md#s2-37by--bounded-long64-cvtps2pd-xmm0rax-m64-memory-profile), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37by--bounded-long64-cvtps2pd-xmm0rax-m64-memory-profile), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37by--cvtps2pd-xmm0rax-m64-memory-profile).


### S2-37bz — CVTSS2SD XMM0,[RAX] m32 memory profile

`s2_37bz_cvtss2sd_m32_snan_subnormal.case.json` is the literal exact legacy
`F3 0F 5A 00` case. Its unaligned 4-byte source is an SNaN; the fixture records
quieting/IE, preserves the XMM high qword and pending x87 canaries, and uses a
MXCSR setting with IM=DM=1 and PM clear. The runner matrix covers NaN classes,
subnormals, zeros, infinities, normal values, all RC/FTZ settings, AC refusal,
foreign controls and backend denial. The oracle uses raw integer bit construction
without host FP or production helpers. No physical x86 acceptance is claimed.

Normative sources: Intel SDM Rev. 093 Vol. 2A CVTSS2SD pp. 3-240–3-241; Vol. 1
§§11.5.2, 11.5.3, 11.5.5, Table D-11 p. D-8, Table 14-21; Vol. 3A
§§16.1.3–16.1.4, Table 16-1. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37bz--bounded-long64-cvtss2sd-xmm0rax-m32-memory-profile), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37bz--bounded-long64-cvtss2sd-xmm0rax-m32-memory-profile), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37bz--cvtss2sd-xmm0rax-m32-memory-profile).

### S2-37ca — CVTSD2SS XMM0,[RAX] m64 memory profile

`s2_37ca_cvtsd2ss_m64_wpr_tiny_ftz_rc_up.case.json` is the literal exact legacy
`F2 0F 5A 00` case. It supplies an unaligned binary64 just below the binary32
minimum-normal boundary with round-up and FTZ enabled. The source remains
WPR-tiny even if final binary32 boundary rounding with FTZ disabled produces
minimum normal; this fixture flushes the WPR-tiny value to signed zero with UE|PE. Runner cases cover all RC/FTZ settings, both signed WPR
thresholds, overflow boundaries, NaNs, infinities, signed zero, subnormals,
sticky flags, and strict refusal/schema/tamper/backend paths. The raw-u64 oracle
uses integer operations only. Direct ISA coverage establishes one 8-byte read,
unaligned success, AC refusal, gate priority, missing-SSE refusal, and read
rollback. Native/JIT/C ABI are closed; no physical x86 acceptance is claimed.

Sources: Intel SDM Rev. 093 Vol. 2A CVTSD2SS pp. 3-234–3-235; Vol. 1
§§4.9.1.4–4.9.1.6, Table 4-12, §10.2.3.3, §§11.5.2–11.5.3, 11.5.5, Table
D-12, Table 14-21; Vol. 3A §§16.1.3–16.1.4, Table 16-1; Intel Floating-Point
Reference Sheet v2.13 p. 2. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37ca--bounded-long64-cvtsd2ss-xmm0rax-m64-memory-profile), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ca--bounded-long64-cvtsd2ss-xmm0rax-m64-memory-profile), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ca--cvtsd2ss-xmm0rax-m64-memory-profile).


### S2-37cb — CVTPD2PS XMM0,[RAX] aligned m128 memory profile

`s2_37cb_cvtpd2ps_m128_snan_subnormal_rc_up.case.json` is the literal exact
legacy `66 0F 5A 00` case. Its aligned lanes contain SNaN and the smallest
binary64 subnormal; the fixture verifies independent lane exception aggregation,
including IE|DE|UE|PE. The focused runner covers both lane orders for mixed
invalid/overflow/denormal combinations, QNaN+normal, schema-only DAZ and PM
rejection, strict foreign controls, tamper detection, and JIT/native denial. Direct ISA tests cover CPL×AC
alignment behavior, one aligned 16-byte read, and read rollback; runner tests do
not expose callback counts. The integer raw-u64 oracle uses no host FP or
production conversion helper. No physical x86 acceptance is claimed.

Sources: Intel SDM Rev. 093 Vol. 2A CVTPD2PS pp. 3-219–3-222; Vol. 1
§§4.9.1.4–4.9.1.6, Table 4-12, §10.2.3.3, §§11.5.2–11.5.3, §11.5.3.1,
Table D-12, Table 14-21; Vol. 3A Table 16-1; Intel Floating-Point Reference
Sheet v2.13 p. 2. Type 2/Table 2-19 is VEX-only and is not used for legacy
alignment. See [plan](../../../../docs/plans/STAGE_2.md#s2-37cb--bounded-long64-cvtpd2ps-xmm0rax-aligned-m128-memory-profile),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cb--bounded-long64-cvtpd2ps-xmm0rax-aligned-m128-memory-profile), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cb--cvtpd2ps-xmm0rax-aligned-m128-memory-profile).


### S2-37cc — CVTSI2SD XMM0,dword ptr [RAX] m32 memory profile

`s2_37cc_cvtsi2sd_m32_i32_min.case.json` is the literal exact legacy
`F2 0F 2A 00` case. Its unaligned m32 source is `INT32_MIN`; the expected
binary64 result is exact. The profile preserves all MXCSR bits and XMM0.high64,
with no x87/MMX transition. Runner coverage spans signed extrema and values near
2^24 across RC settings, including MXCSR=0 and PM=0. The runner test also checks
pending-x87 canary preservation, gate priority and AC refusal, schema
foreign-field rejection, output tampering, and JIT/native denial. Direct ISA
coverage checks missing-SSE and read-failure atomicity. The focused runner passed
1/1; full local suites passed (`od-isa` 1633, `od-testkit` 446, `od-runner`
112 unit + 459 fixture, and `od-x86-runner` 8 library + 4 binary tests).
Workspace all-target check passed. No physical x86 acceptance is claimed.

Sources: Intel SDM Rev. 093 Vol. 2A CVTSI2SD pp. 3-236–3-237; Vol. 1
§11.5.2.2 and Table 14-21; Vol. 3A §16.1/Table 16-1. The CVTSI2SD
instruction entry explicitly scopes its Type 3/Table 2-20 and Type 5/Table
2-22 references to VEX-encoded forms. Both tables also contain legacy SSE
conditions; this profile uses Vol. 3A Table 16-1 and Vol. 1 Table 14-21 for
legacy gates and alignment. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37cc--exact-legacy-long64-cvtsi2sd-xmm0dword-ptr-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cc--bounded-long64-cvtsi2sd-xmm0dword-ptr-rax-m32-memory-profile), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cc--cvtsi2sd-xmm0dword-ptr-rax-m32-memory-profile).


### S2-37cd — CVTSI2SD XMM0,qword ptr [RAX] m64 memory profile

`s2_37cd_cvtsi2sd_m64_i64_min.case.json` is the literal exact legacy
`F2 48 0F 2A 00` case. Its unaligned source contains `INT64_MIN`, which converts
exactly to binary64. The profile writes only XMM0.low64, preserves the full
MXCSR and XMM0.high64, and leaves x87 untouched. Strict fixture admission requires PM=1 and DAZ=0; malformed PM=0/DAZ/reserved-bit inputs are schema-only rejection cases. Runner coverage checks signed boundaries, 2^53 rounding ties, four RC modes,
PM-only versus all-mask MXCSR, sticky PE histories, pending-x87 canaries, foreign
fields, tampering, and JIT/native denial. Direct ISA tests cover CPL/gates, AC
refusal, span validation, and read rollback. Full suites passed: od-isa 1,634;
od-testkit 446; od-runner 112 unit + 460 fixture; od-x86-runner 8 library + 4
binary tests. Workspace all-target check, formatting, diff, layout, anchor, and
Graphify checks passed; independent code review is complete. Physical x86
acceptance was not run. See [plan](../../../../docs/plans/STAGE_2.md#s2-37cd--exact-legacy-long64-cvtsi2sd-xmm0qword-ptr-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cd--bounded-long64-cvtsi2sd-xmm0qword-ptr-rax-m64-memory-profile), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cd--cvtsi2sd-xmm0qword-ptr-rax-m64-memory-profile).

Sources: Intel SDM Rev. 093 Vol. 2A CVTSI2SD pp. 3-236–3-237; Vol. 1
§4.8.4, §11.5.2.6, Table 14-21; Vol. 3A Table 16-1. The Type 3/Table 2-20
note is VEX-only and is not cited as legacy evidence.


### S2-37ce — CVTSI2SS XMM0,dword ptr [RAX] m32 memory profile

`s2_37ce_cvtsi2ss_m32_i32_max_nearest.case.json` is the literal exact legacy
`F3 0F 2A 00` case. Its unaligned m32 source is `INT32_MAX`; nearest conversion
produces `+2^31` with sticky PE, which is a valid rounded result. The fixture
uses PM-only MXCSR (`0x1000`) and XMM0 high96/x87 canaries. Runner coverage
checks signed boundaries, positive and negative inexact values across all RC
modes, existing PE history, high96 and pending-x87 preservation, schema and
foreign-field rejection, output tampering, and JIT/native denial. Direct ISA
tests cover SSE-bit-25 gates, all CPLs, AC refusal, span/read atomicity, and
runtime MXCSR refusal. The AC=1 runner case is valid and expects local
`Unsupported`; PM=0, DAZ=1, and reserved MXCSR controls are rejected by strict
schema admission. Full local suites passed: od-isa 1,637; od-testkit 446;
od-runner 112 unit + 461 fixture; od-x86-runner 8 library + 4 main tests.
Workspace all-target check passed. The portable native classifier marks the
profile local-only; no physical x86 execution is claimed. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37ce--exact-legacy-long64-cvtsi2ss-xmm0dword-ptr-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ce--bounded-long64-cvtsi2ss-xmm0dword-ptr-rax-m32-memory-profile), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ce--cvtsi2ss-xmm0dword-ptr-rax-m32-memory-profile).

Sources: Intel SDM Rev. 093 Vol. 2A CVTSI2SS pp. 3-238–3-239; Vol. 1
§11.5.2.6, §§11.5.3.1–3, and Table 14-21; Vol. 3A Table 16-1. The CVTSI2SS
instruction entry explicitly scopes its Type 3/Table 2-20 reference to
VEX-encoded forms; Table 2-20 itself also contains legacy SSE conditions.

### S2-37cf — CVTSI2SS XMM0,qword ptr [RAX] m64 memory profile

`s2_37cf_cvtsi2ss_m64_double_round_guard.case.json` is the literal exact legacy
`F3 48 0F 2A 00` profile. Its unaligned signed-i64 source is
`0x0040000040000001`; direct nearest-even conversion yields binary32
`0x5a800001` with PE, distinguishing it from a binary64 intermediate. XMM0
high96 and an arbitrary pending x87 environment are canaries. The runner matrix
covers seven signed boundaries/ties across all RCs, PM-only and full-mask
settings, and both old-PE histories (112 conversions), plus backend denial,
AC refusal, gates, malformed and foreign controls, output tampering, and
cross-profile result rejection. Direct ISA tests cover eight source offsets,
all CPLs, gates and no-SSE ordering, PM/DAZ/reserved-control refusals, canonical
span failures, exact one-read width, and rollback. Strict schema rejects PM=0,
DAZ=1, and reserved MXCSR bits; AC=1 is valid runtime refusal coverage. Full
local suites passed (od-isa 1,639; od-testkit 446; od-runner 112+462;
od-x86-runner 8+4) and the workspace all-target check passed. Native closure
and classifier tests do not claim physical x86 execution. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37cf--exact-legacy-long64-cvtsi2ss-xmm0qword-ptr-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cf--bounded-long64-cvtsi2ss-xmm0qword-ptr-rax-m64-memory-profile), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cf--cvtsi2ss-xmm0qword-ptr-rax-m64-memory-profile).

Sources: Intel SDM Rev. 093 Vol. 2A CVTSI2SS pp. 3-238–3-239; Vol. 1
§4.8.4, §11.5.2.6, §§11.5.3.1–3, and Table 14-21; Vol. 3A Table 16-1. The
CVTSI2SS instruction entry explicitly scopes its Type 3/Table 2-20 reference
to VEX-encoded forms; Table 2-20 itself also contains legacy SSE conditions.

### S2-37cg — CVTSD2SI EAX,qword ptr [RAX] m64-to-r32 memory profile

`s2_37cg_cvtsd2si_m64_i32_min.case.json` is a literal exact legacy
`F2 0F 2D 00` case with an unaligned m64 source containing exact -2^31. The
result bits are `0x80000000`, but the lane is valid and does not set IE. The
profile captures the original RAX source address, reads exactly eight bytes,
then writes EAX and zero-extends RAX. Its independent oracle reads raw memory
bits and rounds according to MXCSR.RC before checking i32 range.

The runner matrix covers positive/negative halfway range boundaries, exact
INT32_MIN versus invalid indefinite, ±minimum subnormals, an inexact finite
value, all four RC settings, old IE/PE histories, unaligned m64 access, RAX
aliasing, and optional pending x87 canaries. Direct ISA tests cover CPL0–3 gate
priority, missing SSE state, unsupported MXCSR controls, AC refusal including
AC plus noncanonical source, canonical-span failures, read rollback and exact
one-read width. Runner validation checks gate/exception metadata, source/result
state, memory echo, retirement, foreign controls, tampering, and JIT/native
closure. Strict schema rejects PM=0, IM=0, DAZ=1, and reserved MXCSR bits; AC=1
is a valid runtime `Unsupported` fixture. Native classifier and backend denial
are closure evidence only, not physical x86 execution. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37cg--exact-legacy-long64-cvtsd2si-eaxqword-ptr-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cg--bounded-long64-cvtsd2si-eaxqword-ptr-rax-m64-to-r32-memory-profile), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cg--cvtsd2si-eaxqword-ptr-rax-m64-to-r32-memory-profile).

Sources: Intel SDM Rev. 093 Vol. 2A `CVTSD2SI` pp. 3-232–3-233; Vol. 1
§3.4.1.1, §11.5.2.2, Table D-13 and Table 14-21; Vol. 3A §16.1.3 and Table 16-1
([official SDM](https://cdrdv2.intel.com/v1/dl/getContent/671199)).

### S2-37ch — CVTSS2SI EAX,dword ptr [RAX] m32-to-r32 memory profile

The literal fixture checks exact legacy `F3 0F 2D 00` and an unaligned four-byte
binary32 source at `[RAX]`. The runner matrix covers seventeen raw values across
all four RC modes, including signed range edges, rounding ties, subnormals,
NaN/infinity, sticky IE/PE, EAX zero-extension, optional pending-x87 preservation,
strict controls, tampering and JIT/native denial. Direct ISA tests cover gates,
CPLs, full fetch, address spans and read rollback. PM=0, IM=0, DAZ=1 and reserved
MXCSR values are malformed schema inputs; AC=1 is an admitted runtime
`Unsupported` fixture. Backend denial does not demonstrate physical x86
execution.

Sources: Intel SDM Rev. 093 Vol. 2A `CVTSS2SI` pp. 3-242–3-243; Vol. 1
§3.4.1.1, §11.5.2.2, Table D-13, and Table 14-21; Vol. 3A §16.1.3 and Table 16-1. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37ch--exact-legacy-long64-cvtss2si-eaxdword-ptr-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ch--bounded-long64-cvtss2si-eaxdword-ptr-rax-m32-to-r32-memory-profile), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ch--cvtss2si-eaxdword-ptr-rax-m32-to-r32-memory-profile).

### S2-37ci — CVTTSS2SI EAX,m32

`s2_37ci_cvttss2si_m32_i32_rc_independence.case.json` is the exact legacy
`F3 0F 2C 00` fixture. Its source is unaligned `+1.5` at a high32 RAX address,
while MXCSR selects round-up; the instruction still truncates to EAX=1 and sets
PE. The runner matrix checks seventeen raw binary32 values across four RC
settings, signed boundaries, NaNs/infinities, subnormals without DE, sticky
flags, RAX zero-extension, optional pending-x87 canaries, strict schema,
tampering, and JIT/native denial. Direct ISA coverage checks full fetch, gates,
CPL0–3, noncanonical/cross-boundary/overflow spans, AC refusal, exact read
width/count, and atomic read failure. Native closure does not establish
physical x86 execution. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37ci--exact-legacy-long64-cvttss2si-eaxdword-ptr-rax),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ci--bounded-long64-cvttss2si-eaxdword-ptr-rax-m32-to-r32-memory-profile), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ci--cvttss2si-eaxdword-ptr-rax-m32-to-r32-memory-profile).

### S2-37cj — CVTTSD2SI EAX,m64

`s2_37cj_cvttsd2si_m64_i32_rc_independence.case.json` is a literal exact legacy
`F2 0F 2C 00` fixture. It supplies unaligned +1.5 at a high32 RAX address while
MXCSR selects round-up; CVTT still writes EAX=1 and sets PE. The runner matrix
covers truncation boundaries, exact and invalid i32 minima, NaN/infinity,
signed zero and subnormals, all RC settings, sticky histories, RAX
zero-extension, pending-x87 preservation, schema/foreign-field validation,
tampering and backend denial. The independent oracle uses raw binary64 bits
and integer operations only. Focused ISA/decoder, testkit oracle, runner, and
native-local checks passed (3/3, 1/1, 1/1, 1/1). Full suites passed: od-isa
1,650; od-testkit 448; od-runner 112 unit + 466 fixture; od-x86-runner 8+4.
Workspace all-target check, formatting, diff-check, layout and Graphify passed;
the graph has 10,454 nodes, 22,374 edges and 313 communities. Independent code
and documentation review is complete; 13 CJ cross-document/component links
resolve. Native/JIT/C ABI remain closed; no physical x86 acceptance is claimed.

Sources: Intel SDM Rev. 093 Vol. 2A CVTTSD2SI pp. 3-253–3-254; Vol. 1
§11.5.2.2 and Table 14-21; Vol. 3A §16.1.3/Table 16-1. The instruction entry
scopes its Type 3/Table 2-20 reference to VEX-encoded forms; the table also
contains legacy SSE conditions. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37cj--exact-legacy-long64-cvttsd2si-eaxqword-ptr-rax),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cj--bounded-long64-cvttsd2si-eaxqword-ptr-rax-m64-to-r32-memory-profile), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cj--cvttsd2si-eaxqword-ptr-rax-m64-to-r32-memory-profile).

### S2-37ck — CVTSD2SI RAX,m64

`s2_37ck_cvtsd2si_m64_i64_rc_rounding.case.json` is the literal exact legacy
`F2 48 0F 2D 00` profile. It reads eight bytes at the original RAX and writes
the 64-bit result back to RAX. The literal contains −1.5 with round-up selected
(`MXCSR=0x5080`), so the full-width result is `RAX=0xffffffffffffffff` with PE.
This distinguishes RAX output from an EAX write with zero-extension. The matrix
also checks the other RC modes, sticky IE/PE histories, signed i64 limits,
NaN/infinity/subnormal inputs and pending-x87 preservation. The integer-only
oracle consumes raw binary64 bits, without host FP or a production conversion
helper. Focused decoder/direct ISA, runner and native-local classifier checks
passed (1/1, 2/2, 1/1, 1/1); the runner exercised the independent testkit
oracle for 288 RC/sticky-history combinations. Full suites passed: od-isa
1,653; od-testkit 448; od-runner 112 unit + 467 fixture; od-x86-runner 8+4.
Workspace all-target check, formatting, layout, diff and 16 CK-relative links
passed. Graphify contains 10,469 nodes, 22,415 edges and 310 communities.
Independent code/schema and docs/evidence reviews are clean. Native/JIT/C ABI
stay closed; no physical x86 acceptance is claimed.

Sources: Intel SDM Rev. 093 Vol. 2A CVTSD2SI pp. 3-232–3-233; Vol. 1 §4.8.4,
§11.5.2.2 and Table 14-21; Vol. 3A §16.1.3/Table 16-1. The instruction entry
scopes its Type 3/Table 2-20 reference to VEX forms; Table 2-20 itself also
contains legacy SSE conditions. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37ck--exact-legacy-long64-cvtsd2si-raxqword-ptr-rax),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ck--bounded-long64-cvtsd2si-raxqword-ptr-rax-m64-to-r64-memory-profile), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ck--cvtsd2si-raxqword-ptr-rax-m64-to-r64-memory-profile).


### S2-37cl — CVTSS2SI RAX,m32

`s2_37cl_cvtss2si_m32_i64_rc_rounding.case.json` is the literal exact legacy
`F3 48 0F 2D 00` form. The source address is the original RAX and the fixture
performs one four-byte read; successful execution writes the full signed-i64
result to RAX. The literal uses binary32 −1.5 and round-up (`MXCSR=0x5080`),
expecting `RAX=0xffffffffffffffff` plus PE. It distinguishes the full RAX
result from a 32-bit destination write. The runner matrix covers RC modes,
signed i64 boundaries, exact/inexact values, invalid inputs, sticky histories,
pending-x87 preservation, tampering, and backend denial. The independent oracle
uses raw binary32 bits and integer logic only. Runner numerical comparisons and
direct ISA gate/span/read-failure tests exercise Ring0; they are not physical-x86
acceptance. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37cl--exact-legacy-long64-cvtss2si-raxdword-ptr-rax),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cl--bounded-long64-cvtss2si-raxdword-ptr-rax-m32-to-r64-memory-profile), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cl--cvtss2si-raxdword-ptr-rax-m32-to-r64-memory-profile).

### S2-37cm — CVTTSS2SI RAX,m32

`s2_37cm_cvttss2si_m32_i64_truncation.case.json` is the literal exact legacy
`F3 48 0F 2C 00` profile. It reads four bytes from the captured original RAX
and writes the signed-i64 result to full RAX. The binary32 source is -1.5 with
MXCSR round-down (`0x3080`); the result remains -1 with PE, distinguishing
TowardZero from ordinary CVT rounding and an EAX write. The runner matrix
covers signed boundaries, fractions, subnormals, NaN/infinity, RC independence,
sticky histories, pending-x87 preservation, tampering, and backend denial. Its
numerical comparisons and direct ISA gate/span/read-failure tests exercise
Ring0 only; no physical-x86 acceptance is claimed. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37cm--exact-legacy-long64-cvttss2si-raxdword-ptr-rax),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cm--bounded-long64-cvttss2si-raxdword-ptr-rax-m32-to-r64-memory-profile), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cm--cvttss2si-raxdword-ptr-rax-m32-to-r64-memory-profile).

### S2-37cn — CVTTSD2SI RAX,m64

`s2_37cn_cvttsd2si_m64_i64_truncation.case.json` is the literal exact legacy
`F2 48 0F 2C 00` schema-v2 profile. Its binary64 source is -1.5 with MXCSR
round-down (`0x3080`); truncation still writes full RAX=`0xffffffffffffffff`
and sets PE. This distinguishes TowardZero from ordinary CVT rounding and
confirms the 64-bit destination width. The runner checks raw values across all
RC settings and sticky IE/PE histories (304 comparisons), plus x87 preservation,
tampering, and backend denial. Direct ISA tests cover gates, canonical spans,
and read-failure atomicity. The runner matrix uses Ring0; direct numerical and
gate matrices cover CPL0–3, while span/read-failure probes have no CPL loop.
No physical x86 acceptance is claimed. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37cn--exact-legacy-long64-cvttsd2si-raxqword-ptr-rax),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cn--bounded-long64-cvttsd2si-raxqword-ptr-rax-m64-to-r64-memory-profile), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cn--cvttsd2si-raxqword-ptr-rax-m64-to-r64-memory-profile).

### S2-37co — COMISD XMM0,m64

`s2_37co_comisd_m64_qnan_subnormal_priority.case.json` is the literal exact
legacy `66 0F 2F 00` profile. It compares a QNaN in XMM0.low64 against the
minimum positive binary64 subnormal at `[RAX]`. The expected unordered flags
are CF|PF|ZF; sticky IE is added while no new DE is added, demonstrating NaN
priority. A pending-x87 canary and an RFLAGS canary exercise state preservation.

The runner matrix covers six comparison/class vectors across all four RC
values, both FTZ settings, and four incoming IE/DE histories (192 independent
oracle comparisons). The direct comparison-relation loop covers CPL0–3;
direct AC/refusal, canonical-span, fetch/gate, and read-failure probes use
Ring0. The runner fixture is host-side Ring0 evidence. Native
local-only classification is checked, but no physical x86 acceptance is
claimed. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37co--exact-legacy-long64-comisd-xmm0qword-ptr-rax),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37co--bounded-long64-comisd-xmm0qword-ptr-rax-m64-memory-profile), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37co--comisd-xmm0m64).

### S2-37cp — UCOMISD XMM0,m64

`s2_37cp_ucomisd_m64_qnan_subnormal_priority.case.json` is the literal exact
legacy `66 0F 2E 00` profile. XMM0.low64 holds a QNaN and `[RAX]` holds the
minimum positive binary64 subnormal. UCOMISD returns unordered flags CF|PF|ZF
without adding IE or DE; the fixture includes pending-x87 and RFLAGS canaries.

The runner matrix covers seven relation/class pairs across four RC settings,
both FTZ settings, and four incoming IE/DE histories (224 raw-bit oracle
comparisons), including QNaN/subnormal in both operand orders and the SNaN
contrast. Direct numeric checks cover CPL0–3; direct gate/refusal/span/read-
failure checks use Ring0. The runner checks foreign-field rejection and JIT
denial; it does not add CP-specific AC or malformed-MXCSR negatives. Native
local-only classification is checked, with no physical x86 claim. See the
[plan](../../../../docs/plans/STAGE_2.md#s2-37cp--exact-legacy-long64-ucomisd-xmm0qword-ptr-rax),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cp--bounded-long64-ucomisd-xmm0qword-ptr-rax-m64-memory-profile), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cp--ucomisd-xmm0m64).

### S2-37cq — COMISS XMM0,m32

`s2_37cq_comiss_m32_qnan_subnormal_priority.case.json` is the literal exact
legacy `0F 2F 00` profile. XMM0.low32 is a QNaN and `[RAX]` contains the
minimum positive binary32 subnormal. COMISS sets unordered flags CF|PF|ZF and
adds sticky IE, while NaN priority suppresses new DE. The literal includes
pending-x87 and unrelated-RFLAGS canaries.

The runner matrix covers seven relation/class cases across four RC values,
two FTZ settings, and four prior IE/DE histories (224 raw-bit comparisons).
Shared compare-path regressions passed: direct ISA 4/4, COMISD runner 1/1
(192), and UCOMISD runner 1/1 (224).
The direct relation test covers CPL0–3; direct gate/refusal/span/read-failure
probes and the runner fixture use Ring0. Native-local-only classification is
checked; full suites passed: ISA 1,671; testkit 452; runner 112+473;
x86-runner 8+4; workspace all-target check passed. The profile has no native
execution or physical x86 acceptance claim. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37cq--exact-legacy-long64-comiss-xmm0dword-ptr-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cq--bounded-long64-comiss-xmm0dword-ptr-rax-m32-memory-profile), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cq--comiss-xmm0m32).

### S2-37cr — UCOMISS XMM0,m32

`s2_37cr_ucomiss_m32_qnan_subnormal_priority.case.json` is the literal exact
legacy `0F 2E 00` profile. XMM0.low32 is a QNaN and `[RAX]` holds the minimum
positive binary32 subnormal. UCOMISS sets unordered CF|PF|ZF, adds neither IE
nor DE for this pair, and preserves the incoming MXCSR control/status value
`0x180`. This differs from COMISS, which sets IE for QNaN. The fixture includes
a pending-x87 canary to prove the profile preserves that optional state.

The runner checks nine relation/class cases across RC, FTZ, and four incoming
IE/DE histories (288 raw-bit comparisons), plus foreign-control and backend
denial. Direct numerical cases cover CPL0–3; direct gate/refusal/span/read-
failure probes and runner cases use Ring0. The native-local-only classifier
passed three tests. Full suites passed: ISA 1,674, testkit 453, runner 112+474,
and x86-runner 8+4; workspace all-target check, formatting, layout, Graphify
(10,595/22,730/311), diff check, and 13 anchors passed. Independent code/schema
and final docs/evidence reviews passed; native execution and physical x86
acceptance are not claimed. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37cr--exact-legacy-long64-ucomiss-xmm0dword-ptr-rax),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cr--bounded-long64-ucomiss-xmm0dword-ptr-rax-m32-memory-profile), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cr--ucomiss-xmm0m32).

### S2-37cs — CMPSS XMM0,m32,imm8

`s2_37cs_cmpss_m32_qnan_subnormal_priority.case.json` is the literal exact
legacy `F3 0F C2 00 03` profile. XMM0.low32 is a QNaN and `[RAX]` contains the
minimum positive binary32 subnormal. Predicate 3 returns all ones for
unordered, preserves RFLAGS and the pending-x87 canary, and raises neither IE
nor DE because a NaN suppresses DE. Only XMM0.low32 is replaced; its high96
canary remains unchanged.

The runner matrix covers all eight predicates against greater, less, equal,
and unordered relations, with four RC values, both FTZ settings, and four
incoming IE/DE histories (1,024 independent raw-bit oracle comparisons). Its
48 additional priority cases exercise QNaN/SNaN and subnormal combinations.
Direct relation checks cover CPL0–3; direct fetch/gate/refusal/span/read-error
probes and runner cases use Ring0. The full-fetch pp8+gate local-refusal
priority cases are checked by both direct ISA and runner. Native-local-only
classification passed 1/1. Full suites passed: od-isa 1,678, od-testkit 453,
od-runner 112+475, od-x86-runner 8+4; workspace all-target check passed.
Formatting, layout, Graphify (10,615/22,791/311), diff, and 13 anchors passed.
Independent code/schema and docs/evidence review passed. No physical x86
acceptance is claimed. See the
[plan](../../../../docs/plans/STAGE_2.md#s2-37cs--exact-legacy-long64-cmpss-xmm0m32-raximm8),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cs--bounded-long64-cmpss-xmm0m32-raximm8-memory-profile), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cs--cmpss-xmm0m32imm8).

### S2-37ct — CMPSD XMM0,m64,imm8

`s2_37ct_cmpsd_m64_qnan_subnormal_priority.case.json` is the literal exact
legacy `F2 0F C2 00 03` profile. XMM0.low64 contains a QNaN and `[RAX]` holds
the minimum positive binary64 subnormal; the high64 canary is an SNaN. The
unordered predicate writes all ones to low64, preserves high64, pending x87,
RFLAGS, and all unrelated state, and adds neither IE nor DE because a NaN
suppresses denormal status.

The runner checks all eight predicates against greater, less, equal, and
unordered relations across RC, FTZ, and four incoming IE/DE histories: 1,024
raw-bit oracle comparisons. Its 56 priority comparisons cover QNaN/SNaN and
subnormal combinations in both operand orders where relevant. Direct ISA
predicate/relation coverage spans CPL0–3; its gate/refusal/fetch/span/read
probes and the runner use Ring0. Direct tests prove full five-byte fetch
before pp>7 local refusal and before valid-predicate prerequisite gates. The
memory oracle runs through the fixture runner; there is no separate CMPSD
memory oracle unit test. Focused decoder, direct ISA, runner, testkit
test-target compile, and native-local-only classifier passed. Full suites
passed: od-isa 1,680, od-testkit 453, od-runner 112+476, and od-x86-runner
8+4. Workspace all-target check, formatting, layout, Graphify
(10,632/22,836/310), diff, and 13 anchors passed. Independent code/schema and
final docs/source/evidence reviews passed. No physical x86 acceptance is
claimed. See the
[plan](../../../../docs/plans/STAGE_2.md#s2-37ct--exact-legacy-long64-cmpsd-xmm0m64-raximm8),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ct--bounded-long64-cmpsd-xmm0m64-raximm8-memory-profile), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ct--cmpsd-xmm0m64imm8).

### S2-37cu — CMPPD XMM0,m128,imm8

`s2_37cu_cmppd_m128_qnan_subnormal_independent_lanes.case.json` is the
literal legacy `66 0F C2 00 03` profile. XMM0 lane 0 is a QNaN and lane 1 is
the minimum positive binary64 subnormal; the memory lanes are 1.0 and zero.
The unordered predicate produces all ones in lane 0 and zero in lane 1. The
QNaN raises no IE for predicate 3, while the subnormal in the other lane adds
DE. A pending x87 canary and unrelated RFLAGS are preserved; both XMM0 lanes
are replaced by the instruction result.

The runner matrix checks all eight predicates, four relation pairs, four RC
values, both FTZ settings, and four incoming IE/DE histories (1,024 raw-bit
oracle comparisons). Sixteen additional comparisons put a QNaN and subnormal
in opposite lanes in both orders, checking independent DE and QNaN IE
predicate policy. It also checks aligned AC=1 success, misaligned AC=1
`#GP(0)`, pp8 local refusal, foreign-control rejection, output tampering, and
JIT/native denial. Runner evidence uses Ring0. Direct predicate/relation and
alignment checks cover CPL0–3; direct gate/span/read-failure probes use
Ring0. Full suites passed: od-isa 1,686, od-testkit 453, od-runner 112+477, and od-x86-runner 8+4; workspace all-target check passed. Graphify update (10,660/22,910/311), formatting, layout, diff check, and 13 anchors passed. Independent code/schema, docs/source, and final evidence reviews are complete. No physical x86 result is claimed. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37cu--exact-legacy-long64-cmppd-xmm0m128-raximm8),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cu--bounded-long64-cmppd-xmm0m128-raximm8-memory-profile), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cu--cmppd-xmm0m128imm8).

### S2-37cv — CMPPS XMM0,m128,imm8

`s2_37cv_cmpps_m128_lane_ie_de_aggregate.case.json` is a literal exact
legacy `0F C2 00 03` case. XMM0
contains a QNaN, a normal value, an SNaN, and the minimum positive subnormal;
the aligned m128 source makes the QNaN/SNaN lanes unordered, one lane equal,
and the subnormal lane ordered against zero. The result masks are
`ffffffff, 00000000, ffffffff, 00000000`; IE and DE both become sticky.
RFLAGS, pending x87, and unrelated XMM/GPR state are preserved.

The fixture runner checks 448 raw-bit oracle comparisons: 256 cases across
eight predicates, four RC values, two FTZ settings, and four incoming IE/DE
histories; plus 192 QNaN/SNaN and subnormal lane-priority cases in both lane
orders and across all predicates/histories. It also checks aligned AC=1,
pp8 local refusal, foreign-control rejection, output tampering, and JIT/native
denial. The runner uses Ring0; direct predicate/relation and alignment tests
cover CPL0–3, while direct gate, fetch, span, and read-failure probes use
Ring0. Focused decoder/direct ISA passed 3/3 and the runner passed 1/1;
testkit test-target check and native-local-only classifier 1/1 passed. Full
suites passed: od-isa 1,692, od-testkit 453, od-runner 112+478, and
od-x86-runner 8+4; workspace all-target check passed. Graphify update
(10,684/22,976/303), fmt, layout, diff check, and 13 links passed. Independent
code/schema and final docs/evidence reviews are complete. No physical x86
acceptance is claimed. See
the [plan](../../../../docs/plans/STAGE_2.md#s2-37cv--exact-legacy-long64-cmpps-xmm0m128-raximm8),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cv--bounded-long64-cmpps-xmm0m128-raximm8-memory-profile), and
[report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cv--cmpps-xmm0m128imm8).


### S2-37cw — MINSD XMM0,m64 [RAX]

The literal fixture `s2_37cw_minsd_m64_qnan_selects_rhs.case.json` uses `F2 0F 5D 00`. XMM0.low64 is a QNaN and the m64 source is 1.0, so the successful result selects the raw 1.0 operand and sets sticky IE; a signaling-NaN memory operand is also returned without quieting. The high64 SNaN canary and pending x87 state remain unchanged. The runner exercises 144 raw-u64 oracle comparisons across NaN, subnormal, signed-zero/tie and ordinary comparisons, four RC encodings, and four prior IE/DE histories, plus aligned AC=1 refusal, AC-before-noncanonical refusal, tamper, foreign-field, and backend-denial probes. Direct ISA tests cover decoder, gate priorities, AC/span and read-failure rollback. Focused decoder/direct ISA is 3/3; runner is 1/1; od-testkit test-target check and native-local-only classifier 1/1 passed. Full suites passed: od-isa 1,697; od-testkit 453; od-runner 112 unit + 479 fixtures; od-x86-runner 8 library + 4 binary tests. Workspace all-target check, fmt, layout, diff check, and 16 links passed. Graphify refreshed to 10,711 nodes / 23,033 edges / 308 communities. Independent code/schema and final docs/evidence reviews are complete. There is no physical x86 acceptance claim. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37cw--exact-legacy-long64-minsd-xmm0m64-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cw--bounded-long64-minsd-xmm0m64-rax-memory-profile), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cw--minsd-xmm0m64-rax).


### S2-37cx — MAXSD XMM0,m64 [RAX]

The literal fixture `s2_37cx_maxsd_m64_qnan_selects_rhs.case.json` uses `F2 0F 5F 00`: XMM0.low64 is QNaN and the m64 source is 1.0, so the successful result selects raw 1.0 and sets sticky IE. The ignored XMM0 high64 SNaN canary and pending x87 state remain unchanged. The runner matrix has 160 independent raw-u64 comparisons across strict-greater selection, ties, signed-zero order, Q/SNaNs, subnormals, four RC settings, and four prior IE/DE histories. It also checks AC=1 refusal before noncanonical-span refusal, result tampering, foreign controls, and JIT/native denial. Direct ISA tests cover exact decoding, gate priority, MXCSR/AC admission, canonical span, and read rollback at Ring0. Focused decoder/direct ISA passed 3/3 and runner passed 1/1; `cargo check -p od-testkit --tests --offline` and native-local-only classifier 1/1 passed. Full suites passed: od-isa 1,702; od-testkit 453; od-runner 112 unit + 480 fixtures; od-x86-runner 8 library + 4 binary tests. Workspace all-target check passed. Graphify refreshed to 10,738 nodes / 23,093 edges / 311 communities; formatting, layout, diff, and 12 links passed. Independent code/schema and final docs/evidence reviews are complete and clean. No physical x86 acceptance is claimed. See [plan](../../../../docs/plans/STAGE_2.md#s2-37cx--exact-legacy-long64-maxsd-xmm0m64-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cx--bounded-long64-maxsd-xmm0m64-rax-memory-profile), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cx--maxsd-xmm0m64-rax).


### S2-37cz — MAXPD XMM0,m128 [RAX]

The literal fixture `s2_37cz_maxpd_m128_qnan_and_other_lane_denormal.case.json` uses `66 0F 5F 00`. Lane 0 has a QNaN input and selects the raw subnormal RHS (`0x1`), raising IE; lane 1 selects 1.0 and independently raises DE for its subnormal input. This verifies lane-local DE aggregation alongside raw RHS preservation. The bounded interpreter-only profile requires SSE2 (CPUID.01H:EDX[26]), full-fetch-before-gates, present XMM/MXCSR state, MXCSR supported mask `0xffbf`, all six exception masks, DAZ/FTZ clear and reserved bits clear. RC and previous sticky flags may vary. RAX must be 16-byte aligned independent of AC; both AC=0 and AC=1 are allowed for aligned operands. Misalignment, noncanonicality, or overflow of canonical48 `[RAX,RAX+15]` raises #GP(0) without a memory callback; success performs exactly one `read16`. Read failure preserves CPU state and RIP and does not retire. Each lane selects raw RHS for ties or NaNs; IE aggregates across lanes, while DE is raised only for a subnormal operand in that same lane without a NaN. Native/JIT/C ABI/generic/fragment paths remain closed.

The runner checks the literal two-lane raw-u64 oracle matrix, NaN/SNaN payloads, signed-zero ties and status history, aligned/misaligned AC=0/1 behavior, and backend/schema denial. Direct ISA tests cover gate order, canonical complete-span validation, one-read budget, and read rollback. Focused decoder/direct ISA passed 3/3. Full suites passed: od-isa 1717; od-testkit 453; fixture_v2 482; od-x86-runner 8 library + 4 binary tests; workspace all-target check and formatting. Graphify refreshed to 10,843/23,187/327; layout passed in main checkout; independent code/schema and docs/evidence reviews are clean. No physical x86 acceptance is claimed. Intel SDM Rev. 093 sources: Vol. 2B `MAXPD` pp. 4-5–4-7; Vol. 2A Type 2/Table 2-19 p. 2-27; Vol. 1 §4.9.2, §§11.5.3.1–3 and Appendix D D-9/D-13/D-15; Vol. 3A §16.1.3/Table 16-1. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37cz--exact-legacy-long64-maxpd-xmm0m128-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cz--bounded-long64-maxpd-xmm0m128-rax-memory-profile), [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cz--maxpd-xmm0m128-rax), and [standalone implementation report](../../../../docs/reports/S2_37CZ_MAXPD_MEMORY.md).


### S2-37da — MAXPS XMM0,m128 [RAX]

The schema-v2 interpreter profile accepts only exact `0F 5F 00` for XMM0 and m128 `[RAX]`; SSE is CPUID.01H:EDX[25], not SSE2. It requires full fetch before #UD/#NM gates, present XMM/MXCSR state, supported MXCSR mask `0xffbf`, all six exception masks, DAZ/FTZ clear and reserved bits clear. RC and prior sticky history are allowed. RAX must be 16-byte aligned regardless of AC. The full canonical48 span `[RAX,RAX+15]` is checked before exactly one `read16`; misalignment, noncanonicality or endpoint overflow raises #GP(0) without a callback. AC=0 and AC=1 both work on aligned addresses. Failed reads preserve state and do not retire.

The four binary32 lanes select raw RHS bits for ties and NaNs, including SNaN payloads. IE aggregates across lanes; DE is lane-local, so a NaN only suppresses DE in its own lane. Success replaces all XMM0 bits and advances RIP by three. JIT, native, C ABI, generic and fragment paths remain closed. Tests cover sole QNaN/SNaN in either operand position, both signed-zero ties, infinities, subnormals, negative ordering, MXCSR rounding/sticky combinations, gates, fetch-before-gates, memory access count, rollback and schema tampering. Focused decoder/direct ISA passed 3/3; schema tampering passed 1/1. Full suites passed: od-isa 1723, od-testkit 454, runner 112 unit + 482 fixture tests; workspace check, fmt/diff, Graphify and independent Sol/high review passed. The Linux-only native-denial assertion was not run on macOS, and no physical x86 acceptance is claimed. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37da--exact-legacy-long64-maxps-xmm0m128-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37da--bounded-long64-maxps-xmm0m128-rax-memory-profile), [shared report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37da--maxps-xmm0m128-rax), and [standalone implementation report](../../../../docs/reports/S2_37DA_MAXPS_MEMORY.md).


### S2-37db — MINPD XMM0,m128 [RAX]

The literal fixture `s2_37db_minpd_m128_qnan_and_other_lane_denormal.case.json` exercises exact `66 0F 5D 00`. The schema-v2 interpreter profile fetches all four bytes before SSE2 (`CPUID.01H:EDX[26]`)/EM/OSFXSR #UD and TS #NM gates, then admits only present XMM/MXCSR state with mask `0xffbf`, all six exception masks, DAZ/FTZ clear and reserved bits clear; RC and old sticky flags may vary. RAX must be 16-byte aligned regardless of AC; both AC values are allowed for aligned data. The complete canonical48 `[RAX,RAX+15]` span is validated before exactly one `read16`. Misalignment, noncanonicality or endpoint overflow faults with #GP(0) before the callback; read failure leaves CPU state, MXCSR and RIP unchanged and does not retire.

Both binary64 lanes use raw RHS on ties (including signed zeros) or any NaN, preserving SNaN payload bits. IE aggregates across lanes; DE is lane-local and is raised only for a subnormal operand in a lane without NaN. The fixture combines QNaN in one lane with a subnormal in the other, proving cross-lane IE+DE. Independent raw-u64 oracle coverage includes QNaNs on both sides, SNaN as runner LHS and as direct ISA memory RHS, ties, infinities, subnormals, RC and sticky histories. Decoder-neighbor, gate ordering, canonical/address faults, one-read, rollback/atomicity, schema control echo/tamper denial and backend-closure checks are included. Native/JIT/C ABI/generic/fragment paths remain closed.

Focused decoder/direct ISA checks passed 3/3; runner profile passed 1/1. Full suites passed: od-isa 1730, od-testkit 454, runner 112 unit + 483 fixture tests, od-x86-runner 8 library + 4 binary tests; workspace all-target check, formatting, diff check and implementation Graphify report (10,859/23,366/321), post-docs refresh (10,870/23,388/317) passed. Sol/high review corrections are closed: SNaN memory-RHS coverage is attributed to the direct ISA oracle, not the runner matrix; the high `u64::MAX-7` case faults on alignment before endpoint arithmetic and is not called an overflow-branch test. No physical x86 acceptance is claimed. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37db--exact-legacy-long64-minpd-xmm0m128-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37db--bounded-long64-minpd-xmm0m128-rax-memory-profile), [shared report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37db--minpd-xmm0m128-rax), and [standalone implementation report](../../../../docs/reports/S2_37DB_MINPD_MEMORY.md).


### S2-37dc — MINPS XMM0,m128 [RAX]

The literal fixture `s2_37dc_minps_m128_qnan_and_other_lane_denormal.case.json` exercises exact `0F 5D 00`. The schema-v2 interpreter profile requires SSE (`CPUID.01H:EDX[25]`), not SSE2; its control test explicitly uses SSE=1/SSE2=0. Full three-byte fetch precedes #UD for missing SSE, `CR0.EM=1`, or `CR4.OSFXSR=0`, then #NM for `CR0.TS=1`. MXCSR admission requires supported mask `0xffbf`, all six exception masks, DAZ/FTZ clear and reserved bits clear, while allowing any RC and all prior sticky histories.

Aligned 16-byte operands work with AC=0 or AC=1. Misalignment and invalid canonical48 `[RAX,RAX+15]` spans fault before a callback; the successful path performs one `read16`, while read failure is atomic. Four raw binary32 lanes select the RHS on ties and NaNs; IE aggregates and DE stays lane-local. Oracle coverage includes both signed-zero tie orders, Q/SNaN behavior from both source sides, same-lane NaN+subnormal suppression in both orders, pure DE, and a lane0×lane1 4×4 `{none,I,D,I+D}` class product with lanes 2–3 fixed to normal values, plus eight standalone vectors. Together, 24 vectors are paired with all four RC values and 64 old sticky histories for 6,144 comparisons. Schema tampering and JIT/native denial are checked; native-denial source is Linux/x86-64-only and was not executed on macOS. No physical x86 acceptance is claimed. The task result is documented in the [Stage 2 report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37dc--minps-xmm0m128-rax) and [standalone implementation report](../../../../docs/reports/S2_37DC_MINPS_MEMORY.md).

### S2-37dd — SQRTPS XMM0,m128 [RAX]

The literal fixture `s2_37dd_sqrtps_m128_memory_source_lanes_rounding.case.json` encodes exact `0F 51 00`; its prior XMM0 is poisoned, and output must come only from the four m128 source lanes. The strict schema-v2 interpreter profile requires SSE (`CPUID.01H:EDX[25]`), not SSE2; full fetch precedes #UD for missing SSE/EM/OSFXSR and then #NM for TS. Masked-only MXCSR admission requires supported mask `0xffbf`, reserved/DAZ/FTZ clear, and all six masks set, while all RC modes and old sticky histories are allowed. Unmasked/#XM and all non-interpreter backends remain closed.

The independent `independent_sqrtps` raw-bit integer oracle checks the memory lanes without production code or host floating point. Coverage includes ±0, +∞, normals/subnormals, negative normal/subnormal, QNaN/SNaN, exact roots, and sqrt(2) across RN/RD/RZ (`3fb504f3`) and RU (`3fb504f4`). The mixed `[-1, min-positive-subnormal, 2, SNaN]` vector aggregates IE|DE|PE; the previous XMM0 contents are verified irrelevant. Schema/result/control tampering and backend denial are covered. Full fetch/gate precedence and absence of callbacks before gate rejection are checked; successful execution performs one read16, while misalignment/noncanonical spans fault before reads and read errors roll back state. Aligned addresses are accepted with AC=0 and AC=1. Focused ISA passed 3/3; full od-isa 1736, od-testkit 454, runner 112 unit + 485 fixture; workspace all-target check, fmt/layout/Graphify and independent Sol/high review passed. Initial workspace tests hit `No space left on device`; component suites passed after disposable worktree target cleanup. No physical x86 acceptance is claimed. See [plan](../../../../docs/plans/STAGE_2.md#s2-37dd--exact-legacy-long64-sqrtps-xmm0m128-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37dd--masked-only-long64-sqrtps-xmm0m128-rax), [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37dd--sqrtps-xmm0m128-rax), and [standalone implementation report](../../../../docs/reports/S2_37DD_SQRTPS_MEMORY.md).

### S2-37de — SQRTPD XMM0,m128 [RAX]

The literal fixture `s2_37de_sqrtpd_m128_memory_source_lanes_rounding.case.json` uses exact `66 0F 51 00`. It poisons XMM0 and supplies memory lanes `[4,2]`, so the results must derive from memory. The local schema-v2 interpreter profile requires SSE2 (`CPUID.01H:EDX[26]`), complete four-byte fetch before #UD/#NM gates, XMM/MXCSR state, and masked-only MXCSR (`0xffbf`, all masks set, DAZ/FTZ/reserved clear). Every RC and prior sticky combination is covered. The profile allows only aligned m128 `[RAX]` independent of AC, validates canonical48 endpoints before one read16, and preserves state/retirement on read failure. Unmasked/#XM and native/JIT/C ABI/generic/fragment/physical x86 support remain closed.

The independent `independent_sqrtpd` integer oracle covers 256 combinations of RC and old sticky flags plus 19 raw-bit lane vectors, including sqrt(2) rounding, signed zeros, ±infinities, Q/SNaN signs and payloads in both lanes, negative values, subnormals, and both `[positive-subnormal 3,SNaN]` lane orders with IE|DE|PE. Tests verify exact-profile refusal, schema/control/result/retirement tampering, gate/fetch order, one read16 and rollback, and backend denial. Full suites passed: od-isa 1738, od-testkit 454, runner 112 unit + 486 fixture tests; workspace check, fmt/diff, Graphify update, main layout check and independent Sol/high review passed. No physical x86 acceptance is claimed. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37de--exact-legacy-long64-sqrtpd-xmm0m128-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37de--masked-only-long64-sqrtpd-xmm0m128-rax), [shared report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37de--sqrtpd-xmm0m128-rax), and [standalone implementation report](../../../../docs/reports/S2_37DE_SQRTPD_MEMORY.md).

### S2-37cy — MAXSS XMM0,m32 [RAX]

The literal fixture `s2_37cy_maxss_m32_qnan_selects_rhs.case.json` uses `F3 0F 5F 00`: XMM0.low32 is QNaN and the m32 source supplies the raw selected bits; the operation sets sticky IE and preserves XMM0.high96. The bounded interpreter-only profile uses SSE (CPUID.01H:EDX[25]), one four-byte read, and canonical48 span `[RAX,RAX+3]`. AC=1 refuses locally before span validation or a memory callback; AC=0 admits unaligned addresses. MXCSR admission requires all six exception masks, DAZ/FTZ and reserved bits clear, while RC and prior stickies are allowed. The profile keeps native/JIT/C ABI/generic/fragment paths closed. Focused decoder/direct ISA checks passed 2/2, MAXSS runner checks passed 3/3, the full `fixture_v2` suite passed 481/481, `od-testkit` passed 453/453, and workspace all-target check and formatting passed. Independent code/schema review is clean; physical x86 acceptance is not claimed. Intel SDM Rev. 093 evidence covers Vol. 2B `MAXSS` pp. 4-13–4-14; Vol. 1 §4.9.2, §§11.5.2.1–2 and Appendix D; Vol. 2A Table 2-20; and Vol. 3A §16.1.3. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37cy--exact-legacy-long64-maxss-xmm0m32-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37cy--bounded-long64-maxss-xmm0m32-rax-memory-profile), [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37cy--maxss-xmm0m32-rax), and [standalone implementation report](../../../../docs/reports/S2_37CY_MAXSS_MEMORY.md).

### S2-37dg — SQRTSS XMM0,m32 [RAX]

The literal fixture `s2_37dg_sqrtss_m32_unaligned_inexact.case.json` encodes exact `F3 0F 51 00`, reads binary32 `2.0` from unaligned `[RAX]` with AC=0, rounds under MXCSR.RC, sets sticky PE, and preserves XMM0.high96. The schema-v2 interpreter profile requires SSE (CPUID.01H:EDX[25]), full four-byte fetch before #UD/#NM gates, and masked-only MXCSR (`0xffbf`, all masks set, DAZ/FTZ/reserved clear). It accepts all RC modes and previous sticky flags. AC=1 refuses before address validation/data reads; AC=0 permits unaligned m32. Canonical48 `[RAX,RAX+3]` is validated before exactly one read4. The profile denies JIT/native/C ABI/generic/fragment execution. Focused decoder/ISA checks passed 2/2; the schema-v2 runner fixture passed and verified the independent raw-u32 integer oracle, high96 preservation, PE sticky, unchanged memory, and backend denial. Independent review is pending; no physical x86 acceptance is claimed. Sources: Intel SDM Rev. 093 Vol. 2B SQRTSS pp. 4-669–4-670; Vol. 1 §4.9.2 and §§11.5.2.1–.2, 11.5.3.1–.2; Vol. 3A Table 7-8; Intel Floating-Point Reference Sheet v2.13. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37dg--exact-legacy-long64-sqrtss-xmm0m32-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37dg--masked-only-long64-sqrtss-xmm0m32-rax), and [standalone report](../../../../docs/reports/S2_37DG_SQRTSS_MEMORY.md).

### S2-37dh — MINSS XMM0,m32 [RAX]

The literal fixture `s2_37dh_minss_m32_qnan_selects_rhs.case.json` encodes exact `F3 0F 5D 00`: XMM0.low32 contains a QNaN and the m32 memory source supplies the raw selected bits. The result sets sticky IE and preserves XMM0.high96. The schema-v2 interpreter-only profile requires SSE (CPUID.01H:EDX[25]), complete fetch before #UD/#NM gates, and masked MXCSR (`0xffbf`, all six masks set, DAZ/FTZ/reserved clear); RC and all prior sticky flags are admitted. AC=1 refuses locally before address-span checks or callbacks; AC=0 allows unaligned m32. The full canonical48 `[RAX,RAX+3]` span is checked before exactly one read4. Local tests also cover QNaN/SNaN RHS selection, both signed-zero tie directions, denormal IE/DE behavior, all four RC modes × 64 old sticky histories, poisoned XMM0 versus memory data, read failure rollback, and schema/result tampering. JIT/native/C ABI/generic/fragment paths remain closed. No physical x86 acceptance is claimed. Sources: Intel SDM Rev. 093 Vol. 2B `MINSS` pp. 4-24–4-25; Vol. 1 §4.9.2, §§11.5.2.1–.2 and Appendix D; Vol. 2A Type 2/Table 2-19; Vol. 3A §16.1.3/Table 16-1. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37dh--exact-legacy-long64-minss-xmm0m32-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37dh--bounded-long64-minss-xmm0m32-rax-memory-profile), [shared report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37dh--minss-xmm0m32-rax), and [standalone implementation report](../../../../docs/reports/S2_37DH_MINSS_MEMORY.md).

### S2-38m — MINSS XMM0,m32 [RAX] with unmasked IM/DM

The literal `s2_38m_minss_memory_unmasked_qnan.case.json` admits only exact Long64 `F3 0F 5D 00`, reads the RHS from m32 `[RAX]`, raises IE for the QNaN and exits through #XM because IM=0 and OSXMMEXCPT=1. The new strict schema-v2 profile has immutable SSE/CR0/CR4 controls; the S2-37dh masked profile remains unchanged. MXCSR allows IM/DM independently with ZM/OM/UM/PM set, DAZ/FTZ and reserved bits clear, all RC modes and previous sticky histories. Full fetch and #UD-before-#NM gates precede MXCSR/state admission; AC=1 refuses before canonical-span validation/read, while AC=0 allows unaligned data. After one successful read4, S2-38d mixed IM/DM priority commits new IE/DE and suppresses all of XMM0 on an unmasked arithmetic exit; both masks set reports IE only for NaN+subnormal. Success writes only XMM0.low32 and selects raw RHS on ties, signed zero and NaNs including raw SNaN payloads. Direct tests use a separate raw-bit oracle across IM×DM×OSXMMEXCPT, all RC × 64 prior sticky histories, both NaN/subnormal source orders, address/read failure precedence, callback count and state rollback. JIT/native/C ABI/generic/fragment execution is denied. This records only local schema behavior; no complete SSE/MMX or physical x86 claim is made. ISSUE-025 remains open and concerns the unrelated RCPSS/RCPPS threshold. See [plan](../../../../docs/plans/STAGE_2.md#s2-38m-exact-long64-minss-xmm0m32-rax-memory-profile-with-mixed-imdm-exit), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38m--exact-minss-unmasked-memory-exit-for-m32-rax), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-38m--exact-minss-unmasked-memory-profile-at-m32-rax).


### S2-38n — MAXSS XMM0,m32 [RAX] with unmasked IM/DM

The handwritten `s2_38n_maxss_memory_unmasked_qnan.case.json` admits only
Long64 `F3 0F 5F 00` and reads the RHS from one m32 at `[RAX]`. Its QNaN
XMM0 operand selects the raw memory source for MAXSS, sets IE, and exits via
#XM with XMM0, RIP and retirement suppressed. The strict profile echoes SSE,
CR0.EM/TS/MP and CR4.OSFXSR/OSXMMEXCPT; CR0.MP is fixed to one. It keeps the
S2-37cy masked memory and S2-38f register profiles unchanged. Feature gates
follow full fetch and exact-form selection; SSE/EM/OSFXSR #UD precedes TS #NM.
MXCSR supports IM/DM independently while requiring ZM/OM/UM/PM set, DAZ/FTZ
and reserved bits clear. AC=1 refuses before span checks and callbacks; AC=0
permits unaligned m32. Canonical48 `[RAX,RAX+3]` precedes exactly one read4.
Address/read failure precedes FP events and preserves CPU state. The raw-bit
oracle and direct tests cover QNaN/SNaN/subnormal source orders, IM×DM×OSXMMEXCPT,
RC×64 old sticky histories, mixed flags, prior sticky histories, complete
XMM suppression, exact-form-before-disabled-gates, callback/rollback behavior,
schema/result tampering and JIT/native denial. No physical x86 acceptance is
claimed; ISSUE-025 remains unrelated. See the [plan](../../../../docs/plans/STAGE_2.md#s2-38n-exact-long64-maxss-xmm0m32-rax-memory-profile-with-mixed-imdm-exit),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38n--exact-maxss-unmasked-memory-exit-for-m32-rax),
and [standalone report](../../../../docs/reports/S2_38N_MAXSS_MEMORY_UNMASKED.md).

### S2-37di — ADDSS XMM0,m32 [RAX]

The literal fixture `s2_37di_addss_m32_halfway.case.json` encodes exact `F3 0F 58 00`: XMM0.low32 is `1.0`, memory m32 is `2^-24`, and XMM1 is poisoned. Under round-to-nearest-even, the result stays at `1.0` with sticky PE; XMM0.high96 remains canaries and memory is unchanged. The interpreter-only schema-v2 profile requires SSE (CPUID.01H:EDX[25]), full fetch before #UD/#NM gates, and masked MXCSR (`0xffbf`, all six masks set, DAZ/FTZ/reserved clear), while admitting all RC and previous sticky values. AC=1 refuses locally before address validation/read; AC=0 admits unaligned m32. Canonical48 `[RAX,RAX+3]` is checked before exactly one read4. Direct tests cover RC × sticky histories over scalar arithmetic boundaries and failure rollback. JIT/native/C ABI/generic/fragment and unmasked/#XM remain closed. No physical x86 acceptance is claimed. Sources: Intel SDM Rev. 093 Vol. 2A ADDSS pp. 3-24–3-25 and legacy SSE rows in Table 2-27; Vol. 1 §§4.9.1.4–.6, 11.5.2–11.5.3 and DAZ/FTZ; Vol. 3A Event 19/Table 7-8. See [plan](../../../../docs/plans/STAGE_2.md#s2-37di--masked-only-long64-addss-xmm0m32-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37di--masked-only-long64-addss-xmm0m32-rax), [shared report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37di--addss-xmm0m32-rax), and [standalone report](../../../../docs/reports/S2_37DI_ADDSS_MEMORY.md).


### S2-37dj — `ADDSD` XMM0,m64 [RAX]

The literal fixture `s2_37dj_addsd_m64_halfway.case.json` encodes `F2 0F 58 00`: `XMM0.low64` is 1.0, memory supplies 2^-53, and `XMM1` is poisoned. Round-to-nearest-even leaves the result at 1.0 and sets sticky PE; `XMM0.high64` remains its canary and memory is unchanged. The strict local schema-v2 profile requires `Long64`/`SSE2` (`CPUID.01H:EDX[26]`), full fetch before `#UD`/`#NM` gates, and masked `MXCSR` (`0xffbf`, all masks set, DAZ/FTZ/reserved clear). `AC=1` refuses before span checks and callbacks; `AC=0` admits unaligned m64. Canonical48 `[RAX,RAX+7]` is checked before one `read8`. Direct ISA checks cover all RC × 64 sticky histories over midpoint/adjacent/tiny/subnormal/overflow/NaN/infinity/zero inputs. The runner matrix asserts `StopAddress`, available snapshot and retired=1 before independent `independent_addsd` validation. Read rollback, malformed schema/provenance, register regression and JIT/native denial are checked. A generic-runner regression verifies Unsupported without state changes or data reads. The generic profile gate is also closed by default; fragment and C ABI stay outside this schema-v2 route through their separate API/decoder boundaries, without dedicated S2-37dj tests. No physical x86 acceptance is claimed. Sources: Intel SDM Rev. 093 Vol. 2A `ADDSD` pp. 3-22–3-23 and legacy `SSE2` rows in Table 2-27; Vol. 1 §§4.9.1.4–.6, 11.5.2–11.5.3; Vol. 3A Event 19/Table 7-8. See [plan](../../../../docs/plans/STAGE_2.md#s2-37dj--masked-only-legacy-long64-addsd-xmm0m64-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37dj--masked-only-long64-addsd-xmm0m64-rax), [shared report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37dj--addsd-xmm0m64-rax), and [standalone report](../../../../docs/reports/S2_37DJ_ADDSD_MEMORY.md).

### S2-37dk — SUBSS XMM0,m32 [RAX]

`s2_37dk_subss_m32_halfway.case.json` is a handwritten literal encoding of exact legacy `F3 0F 5C 00`. XMM0.low32 is 1.0 and memory RHS is 2^-25, the halfway value below 1.0; nearest-even leaves 1.0 and raises sticky PE. XMM1 and XMM0.high96 are canaries, and memory is unchanged. The strict interpreter-only schema-v2 profile requires Long64/SSE (CPUID.01H:EDX[25]), fetches the complete instruction before #UD/#NM gates, and admits only supported MXCSR bits with all six masks set, DAZ=FTZ=0 and reserved bits clear. RC and prior stickies are unrestricted. AC=1 is local Unsupported before span checks or reads; AC=0 allows unaligned m32. Check canonical48 `[RAX,RAX+3]` before one read4; failed reads preserve state and retirement. Original NaNs are classified and selected before arithmetic treatment of the memory RHS sign. The independent raw-u32 oracle receives memory RHS. JIT/native/C ABI/generic/fragment and unmasked/#XM execution remain closed. No physical x86 acceptance is claimed. Sources: Intel SDM Rev. 093 Vol. 2B SUBSS pp. 4-695–4-696; Vol. 1 §§4.9.1.4–.6, 11.5.2–11.5.3 and DAZ/FTZ; Vol. 2A Table 2-20; Vol. 3A Event 19/Table 7-8. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37dk--masked-only-long64-subss-xmm0m32-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37dk--masked-only-long64-subss-xmm0m32-rax), and [standalone report](../../../../docs/reports/S2_37DK_SUBSS_MEMORY.md).


### S2-37dl — SUBSD XMM0,m64 [RAX]

The literal fixture `s2_37dl_subsd_m64_halfway.case.json` encodes `F2 0F 5C 00`: XMM0.low64 is 1.0, memory supplies 2^-54, and nearest-even leaves 1.0 with sticky PE. XMM0.high64, XMM1 and x87 state are canaries; RAM is unchanged. The strict local interpreter schema requires SSE2 and full fetch before #UD/#NM gates, plus masked MXCSR (`0xffbf`, all masks set, DAZ/FTZ/reserved clear). It permits all RC and old sticky histories. AC=1 refuses before span validation/read; AC=0 allows unaligned m64; canonical48 `[RAX,RAX+7]` validation precedes exactly one read8. The independent `independent_subsd` raw-u64 oracle receives memory RHS. Runner coverage includes all 65,536 operand×RC×sticky executions, schema/provenance/result tampering, backend denial, gates, endpoint/overflow, rollback and SUBSD/SUBSS/ADDSD register regressions. No physical x86 acceptance is claimed. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37dl--masked-only-legacy-long64-subsd-xmm0m64-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37dl--masked-only-long64-subsd-xmm0m64-rax), [shared report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37dl--subsd-xmm0m64-rax), and [standalone report](../../../../docs/reports/S2_37DL_SUBSD_MEMORY.md).


### S2-37dm — MULSS XMM0,m32 [RAX]

The literal fixture `s2_37dm_mulss_m32_memory_rax.case.json` encodes exact legacy `F3 0F 59 00`; XMM0.low32 is 2.0 and unaligned memory at RAX supplies 0.5. XMM0.high96 and poisoned XMM1 are canaries; memory is unchanged. The local interpreter-only schema requires Long64/SSE, complete fetch before #UD/#NM gates, and MXCSR supported mask `0xffbf`, reserved bits clear, all exception masks set, DAZ=FTZ=0. All RC modes and prior sticky histories are allowed. AC=1 refuses before span/read; AC=0 admits unaligned m32. Checked `RAX+3` and canonical48 endpoints precede exactly one read4, with rollback on read failure.

The independent raw-u32 `independent_mulss` oracle receives the actual memory RHS. Runner coverage crosses 25 raw operand pairs × 4 RC modes × 64 prior sticky histories (6,400 executions), asserting StopAddress, snapshot and retirement=1 before oracle comparison. Direct/schema tests cover runtime MXCSR denial with no state commit/read/retirement, truncated-fetch precedence over #UD/#NM, gates, AC/address order, read rollback, provenance/result tampering, backend denial, and register/neighbor regressions. JIT/native/C ABI/generic/fragment and unmasked/#XM paths remain closed; no physical x86 acceptance is claimed. Sources: Intel SDM Rev. 093 Vol. 2B MULSS pp. 4-150–4-151; Vol. 2A §2.5.3/Table 2-20 p. 2-28; Vol. 1 §§11.5.2–11.5.3 and 11.6.6. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37dm--masked-only-legacy-long64-mulss-xmm0m32-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37dm--masked-only-long64-mulss-xmm0m32-rax), [shared report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37dm--mulss-xmm0m32-rax), and [standalone report](../../../../docs/reports/S2_37DM_MULSS_MEMORY.md).

### S2-37dn — MULSD XMM0,m64 [RAX]

The literal fixture `s2_37dn_mulsd_m64_memory_rax.case.json` encodes exact legacy `F2 0F 59 00`; XMM0.low64 is 2.0 and unaligned memory at RAX supplies 0.5. XMM0.high64 and poisoned XMM1 are canaries; RAM is unchanged. The local interpreter-only schema requires Long64/SSE2, complete fetch before #UD/#NM gates, and MXCSR supported mask `0xffbf`, reserved bits clear, all six exception masks set, DAZ=FTZ=0. Every RC mode and prior sticky history is allowed. AC=1 refuses before span/read; AC=0 admits unaligned m64. Checked `RAX+7` and canonical48 endpoints precede exactly one read8, with rollback on failure.

The independent raw-u64 `independent_mulsd` oracle receives the actual memory RHS. Runner coverage crosses 25 operand pairs × 4 RC modes × 64 prior sticky histories (6,400 executions), asserting outcome, snapshot and retirement before oracle comparison. Direct/schema coverage includes runtime MXCSR denial without read/commit, truncated-fetch precedence, feature/#NM gates, AC/span/read rollback, provenance/result tampering and backend denial. JIT/native/C ABI/generic/fragment and unmasked/#XM remain closed; no physical x86 acceptance is claimed. Sources: Intel SDM Rev. 093 Vol. 2B MULSD pp. 4-148–4-149; Vol. 2A §2.5.3/Table 2-20; Vol. 1 §§4.9.1.4–.6, 11.5.2–11.5.3 and 11.6.6. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37dn--masked-only-legacy-long64-mulsd-xmm0m64-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37dn--masked-only-long64-mulsd-xmm0m64-rax), and [shared report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37dn--mulsd-xmm0m64-rax).


### S2-37do — DIVSS XMM0,m32 [RAX]

The literal fixture `s2_37do_divss_m32_memory_rax.case.json` encodes exact legacy `F3 0F 5E 00`; XMM0.low32 is 1.0 and unaligned memory at RAX supplies 3.0. XMM0.high96 and poisoned XMM1 are canaries. The interpreter-only schema admits Long64/SSE, complete fetch before #UD/#NM gates, and masked MXCSR (`0xffbf`, all masks set, DAZ/FTZ/reserved clear), with all RC and old sticky histories. AC=1 refuses before address-span/read work; AC=0 admits unaligned m32. Checked `RAX+3` and canonical48 endpoints precede exactly one read4; read failure rolls back.

The independent `independent_divss` raw-u32 oracle receives the actual memory RHS. Runner coverage crosses 20 operand pairs × 4 RC modes × 64 sticky histories (5,120 runs), checking StopAddress, snapshot and retirement before oracle comparison. Cases cover NaN orders, 0/0, infinity/infinity, ZE/DE priority, infinity/zero, WPR tiny/underflow, exact tiny, overflow and rounding. Direct tests cover runtime MXCSR denial, fetch/gates, AC/span/read rollback, result/input-digest tampering and JIT denial. The Intel SDM Type 3 note is VEX-specific; the exact form here is legacy SSE. No physical x86 acceptance is claimed. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37do--masked-only-legacy-long64-divss-xmm0m32-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37do--masked-only-long64-divss-xmm0m32-rax), [shared report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37do--divss-xmm0m32-rax), and [standalone report](../../../../docs/reports/S2_37DO_DIVSS_MEMORY.md).


### S2-37dp — DIVSD XMM0,m64 [RAX]

The literal fixture `s2_37dp_divsd_m64_memory_rax.case.json` encodes exact legacy
`F2 0F 5E 00`; XMM0.low64 is 2.0 and unaligned memory at RAX supplies 0.5. The
profile requires Long64/SSE2, complete fetch before #UD/#NM, supported MXCSR mask
`0xffbf`, all six exception masks set and DAZ=FTZ=0. Every RC and sticky history
is admitted. AC=1 refuses before span/read; checked `RAX+7` and canonical48
endpoints precede one exact read8. The raw-u64 `independent_divsd` oracle uses the
actual memory RHS. Coverage crosses 24 operand pairs × 4 RC × 64 sticky values
(6,144 executions) including NaN priority, invalid/ZE/DE distinctions, 1/3, tiny,
WPR and overflow boundaries. JIT/native/C ABI/generic/fragment and unmasked/#XM
remain closed; no physical x86 acceptance is claimed. Source: Intel SDM Rev. 093
Vol. 2A DIVSD pp. 3-273–3-274; Type 3 is VEX-only. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37dp--masked-only-legacy-long64-divsd-xmm0m64-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37dp--masked-only-long64-divsd-xmm0m64-rax), and [shared report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37dp--divsd-xmm0m64-rax).


### S2-37dr — SUBPS XMM0,m128 [RAX]

The handwritten fixture `s2_37dr_subps_m128_memory_source_and_destination_lanes.case.json`
encodes exact legacy `0F 5C 00`; old XMM0 and the aligned m128 memory lanes
provide independent operands. The schema-v2 profile requires SSE bit 25,
complete fetch before #UD/#NM, and masked-only MXCSR (`0xffbf`, six exception
masks set, DAZ/FTZ/reserved clear), while accepting all RC and sticky histories.
It checks 16-byte alignment and canonical48 span regardless of AC before one
`read16`; failed checks and reads leave architectural state unchanged.

The independent integer-only SUBPS oracle receives the actual memory RHS and
validates all four output lanes and sticky status. Runner coverage crosses RC
settings and prior sticky histories, verifies backend denial and result
tampering, and checks the SSE gate. Additional memory-specific vectors cover
NaN sign/payload and SNaN-vs-QNaN priority, denormal priority, opposite
infinities, RC-sensitive ties, signed zero, tiny boundary, overflow, and
mixed-lane IE/DE/OE/PE, RHS-only negative QNaN payload/sign preservation,
NaN suppression of same-lane DE, and RC-dependent cancellation zero. Finite
binary32 subtraction cannot newly raise UE: all finite operands and exact
results are integer multiples of `2^-149`; any exact result below `2^-126`
has coefficient magnitude below `2^23`, so it is exactly representable as a
subnormal or zero. This proof also covers tiny results outside Sterbenz's
factor-of-two range. The sticky matrix verifies incoming UE preservation. The
sequential fixture suite passed 498/498. No physical x86 acceptance is claimed. See
the [plan](../../../../docs/plans/STAGE_2.md#s2-37dr--masked-only-legacy-long64-subps-xmm0m128-rax),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37dr--masked-only-long64-subps-xmm0m128-rax),
and [Stage 2 report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37dr--subps-xmm0m128-rax).

### S2-37ds — MULPS XMM0,m128 [RAX]

The handwritten fixture `s2_37ds_mulps_m128_memory_source_and_destination_lanes.case.json`
encodes exact legacy `0F 59 00`; old XMM0 and the aligned m128 memory lanes
provide independent operands. The schema-v2 profile requires SSE bit 25,
complete fetch before #UD/#NM, and masked-only MXCSR (`0xffbf`, all six masks
set, DAZ/FTZ/reserved clear), accepting all RC and sticky histories. It checks
16-byte alignment and canonical48 span regardless of AC before one `read16`;
failed gates, checks and reads leave architectural state unchanged.

The independent integer-only MULPS oracle receives the actual memory RHS and
validates all four output lanes and aggregate sticky status. Runner coverage
crosses every RC and 64 prior sticky patterns for memory-specific NaN
payload/priority, same-lane NaN+subnormal suppression, 0×∞ invalid, signed
zeros/infinities, tiny/WPR/rounding ties, overflow and mixed IE/DE/OE/UE/PE.
It checks strict MXCSR validation, control/result tampering and JIT/native
denial. Direct ISA checks cover exact decode, full fetch and #UD-before-#NM,
single read16, misalignment rollback and read-error rollback. No physical x86
acceptance is claimed. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37ds--masked-only-legacy-long64-mulps-xmm0m128-rax),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37ds--masked-only-legacy-long64-mulps-xmm0m128-rax),
and [Stage 2 report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37ds--mulps-xmm0m128-rax).

### S2-37dq — ADDPS XMM0,m128 [RAX]

The handwritten fixture `s2_37dq_addps_m128_memory_source_and_destination_lanes.case.json` encodes exact `0F 58 00`; it uses old XMM0 and memory as separate lane sources and sets AC=1 while the aligned m128 is accepted. The profile requires SSE bit 25 (not SSE2), complete fetch before #UD/#NM, and masked-only MXCSR (`0xffbf`, six masks set, DAZ/FTZ/reserved clear), with all RC and prior sticky histories admitted. It validates mandatory 16-byte alignment and canonical48 span regardless of AC before one `read16`; faults and read errors roll back.

The independent raw-bit ADDPS oracle covers all four RC modes and all 64 prior sticky patterns, with a runner case for a halfway tie and a mixed SNaN/subnormal, opposite-infinity, and overflow vector. Direct ISA checks cover full fetch/gate ordering, SSE/EM/OSFXSR/TS, AC=0/1 alignment rejection, noncanonical span, and read-error atomicity. Successful interpreter results relabeled JIT or NativeX86 are rejected by schema validation; direct JIT/native routes return Unsupported. No physical x86 acceptance is claimed. See [plan](../../../../docs/plans/STAGE_2.md#s2-37dq--exact-legacy-long64-addps-xmm0m128-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37dq--masked-only-long64-addps-xmm0m128-rax), and [Stage 2 report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37dq--addps-xmm0m128-rax).

### S2-37dt — DIVPS XMM0,m128 [RAX]

The handwritten fixture `s2_37dt_divps_m128_memory_source_and_destination_lanes.case.json` encodes exact `0F 5E 00` and uses old XMM0 lanes over four independent memory denominators. The strict `s2-long-divps-memory-rax-v1` schema requires SSE bit 25 (not SSE2), full fetch before #UD/#NM, masked-only MXCSR (`0xffbf`, all six masks set, DAZ/FTZ/reserved clear), and admits all RC/prior sticky values. The executor checks mandatory 16-byte alignment regardless of AC, then canonical48 span and one read16; any access failure is atomic.

Result validation uses the independent raw-u32 rational `independent_divps` oracle with memory as the RHS. Runner coverage crosses 1/3 with four RC modes and all 64 sticky histories, plus NaN/zero/infinity/subnormal, overflow and mixed lane cases. Focused ISA coverage checks exact decode, one read16, state preservation and read rollback. Schema/result tampering, malformed encodings, alignment/canonical/gate failures and JIT/NativeX86 relabeling are rejected. No physical x86 acceptance is claimed. See [plan](../../../../docs/plans/STAGE_2.md#s2-37dt--masked-only-legacy-long64-divps-xmm0m128-rax), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37dt--masked-only-long64-divps-xmm0m128-rax), and [Stage 2 report](../../../../docs/reports/STAGE_2_REPORT.md#s2-37dt--divps-xmm0m128-rax).

### S2-37dw MULPD m128 memory profile

`s2_37dw_mulpd_m128_memory_source_two_binary64_lanes.case.json` is a
handwritten exact legacy `66 0F 59 00` fixture. Old XMM0 lanes 1.0 and 2.0,
with memory RHS lanes 2.0 and 5.0, produce 2.0 and 10.0; the output differs
from multiplying the register-space canaries, confirming actual memory-source
routing. The strict masked-only profile requires SSE2, supported MXCSR bits,
all exception masks, DAZ=FTZ=0, aligned canonical m128 `[RAX]`, and one read16.
The independent result validator reads both RHS lanes from fixture data.
Runner coverage crosses all RC modes with 64 sticky histories and checks NaN,
zero×infinity, signed zero, subnormal, tie, overflow/tiny cases, gates,
alignment, tampering, and JIT/native denial. No physical x86 acceptance is
claimed. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37dw--masked-only-legacy-long64-mulpd-xmm0m128-rax),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37dw--masked-only-long64-mulpd-xmm0m128-rax),
and [standalone report](../../../../docs/reports/S2_37DW_MULPD_MEMORY.md).

### S2-37dv SUBPD m128 memory profile

`s2_37dv_subpd_m128_memory_source_two_binary64_lanes.case.json` is a
handwritten exact legacy `66 0F 5C 00` fixture. Old XMM0 lanes 1.0 and 2.0,
with memory RHS lanes 2.0 and 5.0, produce -1.0 and -3.0. The strict masked-only
profile requires SSE2, aligned canonical m128 `[RAX]`, and exactly one read16;
its independent validator reads the RHS from these memory bytes. Runner checks
cover RC × 64 sticky histories, lane events, #UD/#GP atomicity,
control/result tampering and JIT/native denial. No physical x86 acceptance is
claimed. See the [plan](../../../../docs/plans/STAGE_2.md#s2-37dv--masked-only-legacy-long64-subpd-xmm0m128-rax),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37dv--masked-only-long64-subpd-xmm0m128-rax),
and [standalone report](../../../../docs/reports/S2_37DV_SUBPD_MEMORY.md).

### S2-37dx — DIVPD XMM0,m128 [RAX]

`s2_37dx_divpd_m128_memory_source_and_destination_lanes.case.json` is a
handwritten legacy `66 0F 5E 00` fixture with old XMM0 lanes 1.0 and 2.0,
memory denominators 3.0 and 4.0, and a poisoned XMM1. The strict
`s2-long-divpd-memory-rax-v1` profile requires SSE2, complete fetch before
#UD/#NM gates, supported MXCSR bits `0xffbf`, all six masks set, and DAZ/FTZ
clear. It requires aligned canonical m128 `[RAX]` and exactly one read16; an
address or memory error leaves state and retirement unchanged. The independent
raw-u64 rational oracle consumes the fixture memory RHS and checks two packed
lanes. Runner coverage crosses representative NaN/invalid/ZE/DE/tiny/overflow
cases with all four RC modes and 64 sticky histories, and denies JIT/native
backends. No physical x86 acceptance is claimed. Sources and limits are in
the [plan](../../../../docs/plans/STAGE_2.md#s2-37dx--masked-only-legacy-long64-divpd-xmm0m128-rax),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-37dx--masked-only-legacy-long64-divpd-xmm0m128-rax),
and [standalone report](../../../../docs/reports/S2_37DX_DIVPD_MEMORY.md).

### S2-38o MINSD m64 unmasked invalid and denormal

The separate `s2-long-minsd-memory-unmasked-rax-v1` fixture profile admits
only `F2 0F 5D 00` and consumes its binary64 RHS from the single 8-byte data
mapping at RAX. Its immutable control echo is the S2-38g MINSD control object;
the memory profile fixes CR0.MP=1. The literal masked mixed fixture combines
XMM0 QNaN with a memory minimum subnormal and checks source-2 raw selection,
IE priority, XMM0 high64 preservation, and one retirement. A separate masked
SNaN memory fixture checks exact source-2 payload preservation. The unmasked
QNaN fixture opens IM and checks IE commit, #XM delivery, full XMM0/RIP
suppression, and zero retirement.

The strict result oracle classifies raw u64 values and compares signs and
magnitudes with integer logic. Runner coverage includes the IM×DM×OSXMMEXCPT
event matrix, all four RC encodings crossed with 64 prior sticky combinations,
fetch/gate ordering, AC-before-span refusal, canonical span/read rollback,
control echo, tamper rejection, and explicit JIT/native denial. These fixtures
document the local interpreter profile only; no physical x86 acceptance is
claimed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38o--exact-minsd-unmasked-memory-exit-for-m64-rax), [plan](../../../../docs/plans/STAGE_2.md#s2-38o-exact-long64-minsd-xmm0m64-rax-memory-profile-with-mixed-imdm-exit), and [report](../../../../docs/reports/S2_38O_MINSD_MEMORY_UNMASKED.md).

### S2-38p MAXSD m64 unmasked invalid and denormal

`s2_38p_maxsd_qnan_im_unmasked_xm.case.json` is a handwritten literal for the
separate `s2-long-maxsd-memory-unmasked-rax-v1` schema profile and exact
`F2 0F 5F 00`. It combines a QNaN destination with a normal memory RHS, opens
IM, and checks IE sticky commit, #XM selection, preserved XMM0, and no
retirement. The profile has its own strict MAXSD memory selector and
`maxsd_unmasked_controls` echo; JIT and native execution are denied. No
physical x86 acceptance is claimed. See the [plan](../../../../docs/plans/STAGE_2.md#s2-38p-exact-long64-maxsd-xmm0m64-rax-memory-profile-with-mixed-imdm-exit), [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38p--exact-maxsd-unmasked-memory-exit-for-m64-rax), and [standalone report](../../../../docs/reports/S2_38P_MAXSD_MEMORY_UNMASKED.md).

### S2-38q MINPD m128 unmasked invalid and denormal

The handwritten `s2_38q_minpd_m128_qnan_unmasked_im.case.json` fixture uses
the dedicated unmasked MINPD memory controls for exact `66 0F 5D 00`. A QNaN
in one lane raises unmasked IE and #XM while a subnormal in the other lane
contributes DE; the entire XMM destination and retirement remain suppressed.
The schema rejects foreign masked-memory controls and JIT/native result labels.
Physical x86 acceptance is not claimed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38q--exact-minpd-memory-unmasked-imdm),
[plan](../../../../docs/plans/STAGE_2.md#s2-38q--exact-long64-minpd-xmm0m128-rax-with-unmasked-imdm-exits),
and [standalone report](../../../../docs/reports/S2_38Q_MINPD_MEMORY_UNMASKED.md).

### S2-38r MAXPD m128 unmasked invalid and denormal

The named #XM and arithmetic-#UD fixtures use exact `66 0F 5F 00` and the
dedicated unmasked MAXPD memory controls. Generated cases cover the cross-lane
event and IM/DM/OSXMMEXCPT matrix, AC=0/1, and prior sticky histories. Special
vectors distinguish MAX from MIN, exercise actual memory RHS instead of XMM1,
and preserve memory-source SNaN payloads. The schema rejects forged backend
results; physical x86 acceptance is not claimed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38r--exact-maxpd-memory-unmasked-imdm),
[plan](../../../../docs/plans/STAGE_2.md#s2-38r--exact-long64-maxpd-xmm0m128-rax-with-unmasked-imdm-exits),
and [standalone report](../../../../docs/reports/S2_38R_MAXPD_MEMORY_UNMASKED.md).

### S2-31dz historical SF with pre-existing IE on bounded m32/m64 stores

The four literal success fixtures cover `FST/FSTP m32fp/m64fp` with SF+IE;
the refusal fixture uses SF-only status with an SNaN source, proving that the
current conversion cannot make the old status eligible. ISA checks cover
73,728 full-state SF+IE cases across nine source classes. Runner checks cover
the literal success/refusal fixtures, ES/B and malformed source/tag/control
rejection, failed writes and range rollback, repeated
store/FNSTSW/FNCLEX/retry, schema-v2 Unsupported validation and JIT/native
denial. The slice is limited to the interpreter contract; physical x86
acceptance is not claimed. See the [plan](../../../../docs/plans/STAGE_2.md#s2-31dz-admit-historical-sf-only-with-pre-existing-ie-on-bounded-m32m64-stores),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-31dz--historical-sf-with-pre-existing-ie-for-bounded-m32m64-stores),
and [standalone report](../../../../docs/reports/S2_31DZ_FST_M32_M64_HISTORICAL_SF.md).

### S2-38s MINPS m128 unmasked invalid and denormal

Three literal fixtures check an upper-lane IE+DE #XM with whole-XMM
suppression, raw memory-RHS NaN/zero ties, and arithmetic #UD for an SNaN RHS.
The independent raw-bit oracle reads the actual memory bytes. The executor
matrix covers all 2,048 `{none,I,D,I+D}⁴ × IM × DM × OSXMMEXCPT` combinations;
additional checks cover SSE without SSE2, exact three-byte fetch, aligned AC
states, #GP(0) misalignment/noncanonical spans, callback counts, schema/backend
tampering and JIT/native denial. No physical x86 acceptance is claimed. See the
[plan](../../../../docs/plans/STAGE_2.md#s2-38s--exact-long64-minps-xmm0m128-rax-with-unmasked-imdm-exits),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38s--exact-minps-memory-unmasked-imdm),
and [standalone report](../../../../docs/reports/S2_38S_MINPS_MEMORY_UNMASKED.md).

### S2-31ea historical SF on raw FSTP m80fp

The S2-31ea interpreter matrix covers 24,576 full-state SF+IE cases across
accepted source/tag pairs, signs, FCWs, TOP, C1 and DE/ZE/OE/UE/PE histories.
Runner fixtures preserve raw ten-byte values and exercise SF-only, ES/B and
malformed source/tag/control refusals, write/span rollback, repeated store,
FNSTSW/FNCLEX/retry, schema-v2 validation and backend denial. No physical x86
acceptance is claimed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-31ea--historical-sf-with-pre-existing-ie-for-raw-fstp-m80fp),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ea--historical-sf-with-pre-existing-ie-for-raw-fstp-m80fp),
and [standalone report](../../../../docs/reports/S2_31EA_FSTP_M80FP_HISTORICAL_SF.md).

### S2-38t MAXPS m128 unmasked invalid and denormal

The 2,316 fixture cases comprise the full 2,048 lane-event/IM/DM/OSXMMEXCPT
product, 256 RC/prior-sticky histories, and 12 special and priority cases.
The independent integer oracle reads actual memory RHS bytes; fault cases cover
fetch/gates, #GP(0) alignment/span, failed reads and suppression of arithmetic
sticky writes. Literal fixtures, schema/result tampering and backend denial are
included. No physical x86 acceptance is claimed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38t--exact-maxps-memory-unmasked-imdm),
[plan](../../../../docs/plans/STAGE_2.md#s2-38t--exact-long64-maxps-xmm0m128-rax-with-unmasked-imdm-exits),
and [standalone report](../../../../docs/reports/S2_38T_MAXPS_MEMORY_UNMASKED.md).

<a id="s2-31eb-historical-ie-de-on-raw-fld-m80fp"></a>

### S2-31eb historical IE and DE on raw FLD m80fp

The two literal fixtures cover an accepted DB 28 raw load with IE+DE+PE and
an ES-set pre-read refusal. ISA coverage expands all 34 admitted binary80 raw
class literals across 8 TOPs, 4 FCWs, and 8 IE/DE/PE histories, comparing the
complete CPU state and surrounding memory; rollback tests start with all three
historical flags set. The repeated DB28/FNSTSW/FNCLEX/DB28 sequence checks
status observation, clearing, and retry. The slice is interpreter-only; no
physical x86 acceptance is claimed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-31eb--historical-ie-and-de-for-raw-fld-m80fp),
[plan](../../../../docs/plans/STAGE_2.md#s2-31eb--historical-ie-and-de-for-raw-fld-m80fp),
and [standalone report](../../../../docs/reports/S2_31EB_FLD_M80FP_HISTORICAL_IE_DE.md).

<a id="s2-31ec-historical-zeoeue-for-raw-fld-m80fp"></a>

### S2-31ec historical ZE/OE/UE on raw FLD m80fp

The literal fixture admits all six historical exception flags together while
retaining the exact DB 28 raw load, status snapshot and push result. The ISA
matrix compares complete state across 64 sticky histories, 34 admitted raw
class literals, 8 TOP values and 4 FCWs. Rollback starts with `FSW=0x043f`;
the repeated FLD/FNSTSW/FNCLEX/retry sequence checks status observation and
clearing. SF/ES/B remain rejected, and native/JIT stay closed. No physical x86
acceptance is claimed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-31ec--historical-zeoeue-for-raw-fld-m80fp),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ec--historical-zeoeue-for-raw-fld-m80fp),
and [standalone report](../../../../docs/reports/S2_31EC_FLD_M80FP_HISTORICAL_FLAGS.md).

### S2-31ed historical SF with prior IE on raw FLD m80fp

The literal fixture adds a DB 28 raw SNaN load with SF+IE and the other five
historical exception flags already set. The complete-state ISA matrix adds
69,632 SF+IE combinations across DE/ZE/OE/UE/PE, raw classes, TOP, FCW and both
C1 states while retaining the S2-31ec 69,632 SF=0 cases. SF-only, ES and B
refusals are checked before read, including each ES/B bit over accepted SF+IE;
read/span rollback, FILD's then-current SF+IE rejection, and
FNSTSW/FNCLEX/retry are covered. S2-31ef later supersedes that FILD-only
rejection for SF+IE; SF-only remains rejected. Native/JIT remain closed. No physical x86 acceptance is claimed. See
the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-31ed--historical-sf-with-pre-existing-ie-for-raw-fld-m80fp),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ed), and
[standalone report](../../../../docs/reports/S2_31ED_FLD_M80FP_HISTORICAL_SF.md).

### S2-31ee historical sticky flags on bounded FILD

Three handwritten schema-v2 fixtures cover exact DF 00, DB 00 and DF 28
FILD with all six sticky flags set, SF/ES/B clear and old C1 set. The m32
fixture composes FILD with FNSTSW AX to observe the preserved flags and cleared
C1. Runner checks literal binary80 results and denies both JIT and native
backends. The ISA matrix covers every sticky combination, TOP, FCW and prior
C1 state for all three widths; rejection and memory rollback remain covered by
local unit tests. No physical x86 acceptance is claimed. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-31ee--historical-sticky-flags-on-exact-bounded-fild-memory-forms),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ee--exact-fild-historical-sticky-flag-admission),
and [standalone report](../../../../docs/reports/S2_31EE_FILD_HISTORICAL_STICKY_FLAGS.md).

### S2-31ef historical SF+IE on bounded FILD

Four additional handwritten schema-v2 fixtures cover exact DF 00, DB 00 and
DF 28 FILD with historical SF+IE and all five other sticky flags; the fourth
composes DB 00 with FNSTSW AX. Runner checks literal binary80 results, retained
SF and cleared C1, plus JIT/native denial. The independent ISA matrix checks
all 30,720 SF+IE states and retains S2-31ee's separate SF=0 matrix. SF-only,
ES and B stay pre-read refusals. No physical x86 acceptance is claimed. See
the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-31ef--historical-sf-with-prior-ie-on-exact-bounded-fild-forms),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ef--historical-sf-with-prior-ie-on-bounded-fild),
and [standalone report](../../../../docs/reports/S2_31EF_FILD_HISTORICAL_SF.md).

<a id="s2-38u-comiss-memory-invalid-only-profile"></a>

### S2-38u COMISS memory invalid-only profile

Five literal fixtures cover finite comparison, post-read subnormal refusal,
masked QNaN, unmasked QNaN/#XM and unmasked SNaN/#UD. The independent runner
oracle expands NaNs across both operand sides, IM, OSXMMEXCPT and all 64 old
sticky-status values (1,024 cases), and also checks finite/infinity/zero
relations, gate priority, AC, canonical span, schema/result tampering and
interpreter-only backend enforcement. ISA unit tests cover CPL and read-error
rollback. No physical x86 acceptance is claimed. ISSUE-033 remains an audit of the existing mixed
NaN/denormal profiles. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38u--exact-comiss-memory-invalid-only-profile),
[plan](../../../../docs/plans/STAGE_2.md#s2-38u--exact-comiss-memory-invalid-only-profile),
and [standalone report](../../../../docs/reports/S2_38U_COMISS_MEMORY_INVALID_ONLY.md).

<a id="s2-38v-ucomiss-memory-invalid-only-profile"></a>

### S2-38v UCOMISS memory invalid-only profile

Six literal fixtures cover finite comparison, QNaN with prior IE, masked
SNaN, unmasked SNaN through #XM and arithmetic #UD, and post-read
subnormal/SNaN refusal. The independent runner oracle expands NaNs across
both operand sides, IM, OSXMMEXCPT and all 64 prior sticky-status values
(1,024 cases), and checks finite/infinity/signed-zero relations, result
tampering and interpreter-only backend enforcement. It also checks QNaN+SNaN.
The direct ISA tests cover both QNaN+subnormal operand orders, CPL0–3,
fetch/form and prerequisite order, AC refusal, canonical-span and read-error
rollback, and subnormal atomic refusal. The schema rejects writable data
mappings and forged result/backend fields. No physical x86 acceptance is
claimed. ISSUE-033 remains open. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38v--exact-ucomiss-memory-invalid-only-profile),
[plan](../../../../docs/plans/STAGE_2.md#s2-38v--exact-ucomiss-memory-invalid-only-profile),
and [standalone report](../../../../docs/reports/S2_38V_UCOMISS_MEMORY_INVALID_ONLY.md).

<a id="s2-38w-comisd-memory-invalid-only-profile"></a>

### S2-38w COMISD memory invalid-only profile

Six literal fixtures cover a finite comparison, masked QNaN/SNaN, unmasked
SNaN `#XM`, unmasked QNaN arithmetic `#UD`, and QNaN plus subnormal refusal.
The independent raw-u64 runner oracle covers both NaN classes and operand
sides × IM × OSXMMEXCPT × all 64 prior sticky states (1,024 cases); the RC
field cycles through all four encodings. It also checks QNaN+SNaN in both
orders, finite/infinity/signed-zero and normal-range boundaries, prior IE alone,
and atomic NaN/subnormal refusal. Direct ISA tests cover CPL0–3, prerequisite
order including combined EM+TS precedence, exact form/fetch, successful
unaligned access with AC=0, AC=1 refusal, canonical span, read failure and
state rollback.
The schema enforces a read-only eight-byte mapping and rejects foreign control
echo/result changes. JIT/native remain unsupported. No physical x86 acceptance
is claimed; ISSUE-033 remains an audit of older COMISS mixed-event profiles.
See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38w--exact-comisd-memory-invalid-only-profile),
[plan](../../../../docs/plans/STAGE_2.md), and
[standalone report](../../../../docs/reports/S2_38W_COMISD_MEMORY_INVALID_ONLY.md).

<a id="s2-38x-ucomisd-memory-invalid-only-profile"></a>

### S2-38x UCOMISD memory invalid-only profile

Six literal fixtures cover finite greater, QNaN with IM=0 (quiet success),
masked SNaN, unmasked SNaN `#XM`, unmasked SNaN arithmetic `#UD`, and atomic
NaN/subnormal refusal. Runner checks exercise 1,024 NaN side/class × IM ×
OSXMMEXCPT × prior-sticky combinations, all RC values, both QNaN+SNaN orders,
prior-only IE, binary64 finite/infinity/signed-zero boundaries, schema/result
tampering and backend denial. Direct ISA coverage spans CPL0–3, exact reads,
AC behavior, NaN invalid handling and atomic refusal. No physical x86 acceptance
is claimed; ISSUE-033 remains open. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38x--exact-ucomisd-memory-invalid-only-profile),
[plan](../../../../docs/plans/STAGE_2.md#s2-38x--exact-ucomisd-memory-invalid-only-profile),
and [standalone report](../../../../docs/reports/S2_38X_UCOMISD_MEMORY_INVALID_ONLY.md).

<a id="s2-38y-sqrtss-memory-unmasked"></a>

### S2-38y SQRTSS m32 memory unmasked exceptions

Seven literal fixtures cover a masked inexact result and unmasked IE, DE, and
PE under both `#XM` and arithmetic `#UD`. The dedicated schema-v2 profile
`s2-long-sqrtss-memory-rax-unmasked-v1` echoes
`sqrtss_memory_rax_unmasked_controls`; the S2-37dg masked profile remains
separate. The runner matrix executes all 65,536 raw source × IM/DM/PM ×
OSXMMEXCPT × RC × sticky-history combinations against the independent raw-u32
SQRTSS oracle. Fixtures and checks verify terminal IE, QNaN without IE,
positive-subnormal DE ordering, atomic postcompute PE suppression/commit,
preserved destination/high96/RIP on exceptions, exact memory preservation,
schema/control tampering, and JIT/native refusal. Direct ISA coverage checks
CPL0–3, exact four-byte fetch/read ordering, #UD before #NM, AC refusal, and
state rollback. No native, JIT, C ABI, or physical x86 acceptance is claimed.
The source set includes the exact-square positive subnormal `0x00000002`, both
signed QNaNs, and a positive SNaN; a focused runner test covers a negative SNaN.
See the [S2-38y report](../../../../docs/reports/S2_38Y_SQRTSS_MEMORY_UNMASKED.md)
and [stage plan](../../../../docs/plans/STAGE_2.md).

### S2-31eg historical SF with prior IE on bounded integer stores

Schema-v2 fixtures cover exact bare bounded Long64 FIST/FISTP `DF 10`, `DF 18`,
`DB 10`, `DB 18`, and `DF 38` at `[RAX]`. Admit prior SF only when IE is also
set; preserve SF and the six sticky flags, and retain the conversion-defined
C1 result. The SF+IE admission matrix compares full state and memory across 13
accepted source classes in 266,240 cases; the adjacent SF=0 regression keeps
its original seven-class coverage. There are also 20 span/write rollback cases
for all forms and both old C1 values. SF-only/ES/B remain no-write refusals; the
FNSTSW/FNCLEX/retry sequence and JIT/native denial are covered. Existing FLD/FST
gates are unchanged. See the [S2-31eg report](../../../../docs/reports/S2_31EG_FIST_HISTORICAL_SF.md),
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-31eg--historical-sf-with-prior-ie-on-bounded-integer-stores),
and [stage plan](../../../../docs/plans/STAGE_2.md).

### S2-38z SQRTSD m64 memory unmasked exceptions

The immutable schema-v2 interpreter-only profile
`s2-long-sqrtsd-memory-rax-unmasked-v1` uses exact `F2 0F 51 00`, SSE2 bit 26,
one read-only m64 mapping at `[RAX]`, and its own
`sqrtsd_memory_rax_unmasked_controls` echo. Seven fixtures cover masked success
and unmasked IE/DE/PE under both `#XM` and arithmetic `#UD`. The runner matrix
covers all 65,536 raw-u64/control combinations with an independent integer
oracle. Focused checks cover exact positive-minimum-subnormal root `0x1` with
DE/no PE, inexact `0x2`/`0x3`, negative-subnormal/SNaN terminal IE, full-state
suppression, one-read ordering, CPL/gate/AC/span/read behavior, schema tampering,
writable source refusal, and backend denial. Validator mutation checks reject
XMM0 low64/high64, RIP, IE/DE/PE sticky flags, retirement, exception vector,
and `#GP(0)` error-code changes. Gate checks exercise missing SSE2, EM, OSFXSR,
TS priority, full/truncated fetch, no-data callbacks on refusal, and CPL0–3.
A regression confirms MINSD still requires FTZ=0. The matrix passes in 27.79 seconds using direct in-process
execution; an initial subprocess-per-case harness was stopped after roughly two
minutes due to overhead and had reported no assertion failure. S2-37df masked
memory and S2-37am register behavior are preserved. No physical x86 acceptance
is claimed. See the [S2-38z CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38z--exact-sqrtsd-memory-profile-with-unmasked-exceptions),
[implementation report](../../../../docs/reports/S2_38Z_SQRTSD_MEMORY_UNMASKED.md),
and [stage plan](../../../../docs/plans/STAGE_2.md).

### S2-38aa SQRTPS m128 memory unmasked exceptions

The immutable schema-v2 profile uses exact `0F 51 00`, SSE bit 25, read-only
m128 `[RAX]`, one read16 and mandatory alignment16. Seven literal fixtures cover
success and unmasked IE/DE/PE under both `#XM` and arithmetic `#UD`. Direct ISA
coverage executes 625 four-lane event products across all IM/DM/PM and
OSXMMEXCPT combinations (10,000 cases), plus all four RC modes and 64 prior
sticky combinations. The independent raw-u32 oracle checks the same products
and RC/sticky histories. Focused coverage also checks mixed-lane IE|DE|PE
aggregation, whole-XMM/RIP/retirement suppression, gate priority at CPL0–3,
full/truncated fetch, alignment with AC clear/set, one-read16 and read-fault
rollback, result/control tampering, and interpreter-only backend denial.
S2-37dd masked memory and S2-37ak register profiles remain separate. No
physical x86 acceptance is claimed. See the [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38aa--exact-sqrtps-memory-profile-with-unmasked-exceptions),
[implementation report](../../../../docs/reports/S2_38AA_SQRTPS_MEMORY_UNMASKED.md),
and [stage plan](../../../../docs/plans/STAGE_2.md).

### S2-38ab SQRTPD m128 memory unmasked exceptions

The immutable schema-v2 interpreter-only profile uses exact `66 0F 51 00`,
SSE2 bit 26, read-only m128 `[RAX]`, one read16 and mandatory alignment16.
Three literal fixtures cover masked success (`s2_38ab_sqrtpd_unmasked_success`),
unmasked IE `#XM`, and unmasked PE arithmetic `#UD`. The runner's independent
raw-u64 oracle covers 25 two-lane event-category products × all IM/DM/PM and
OSXMMEXCPT combinations × all RC modes × 64 old sticky histories (102,400
cases), plus 40 signed-NaN cases for ±QNaN/±SNaN in either or both lanes, with
IM masked/unmasked and both OSXMMEXCPT values. These confirm QNaN does not set
IE, SNaN sets terminal IE, and an unmasked SNaN suppresses the whole XMM
destination while selecting `#XM` or arithmetic `#UD`. The matrix checks
mixed-lane pre-event aggregation and whole-XMM suppression.
The exact positive minimum subnormal `0x1` has root `2^-537` and DE only;
`0x2`/`0x3` produce DE+PE. Focused tests cover gate/fetch/CPL, alignment with
AC clear/set, canonical span, one-read/rollback, result/status/retirement/
exception tampering, read-only schema and JIT/native denial. S2-37de masked
memory and S2-37al register profiles remain separate. No physical x86
acceptance is claimed. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38ab--exact-sqrtpd-memory-profile-with-unmasked-exceptions),
[implementation report](../../../../docs/reports/S2_38AB_SQRTPD_MEMORY_UNMASKED.md),
and [stage plan](../../../../docs/plans/STAGE_2.md).

### S2-38ac ADDSS m32 memory unmasked exceptions

The exact Long64 `F3 0F 58 00` profile requires SSE bit 25 and performs one
read-only read4 from `[RAX]`. Three literal fixtures cover success, unmasked IE
to `#XM`, and unmasked PE to arithmetic `#UD`. The runner's independent raw-u32
oracle executes eight ADDSS event classes × RC ×64 prior sticky histories ×
all IM/DM/PM mask combinations × both OSXMMEXCPT mappings (32,768 cases).
Focused ISA tests verify pre-event IE/DE status, post-event PE status, complete
XMM/RIP/retirement suppression, high96 preservation, full-fetch/SSE/TS priority,
AC-before-span refusal, #GP(0) span, unaligned access with AC clear, one read
and rollback. Schema/result/control tampering and interpreter-only backend
denial are checked. S2-37di/S2-37bc remain separate; unmasked OE/UE and
physical x86 acceptance are outside this profile. See the
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#s2-38ac--exact-addss-memory-profile-with-unmasked-exceptions),
[implementation report](../../../../docs/reports/S2_38AC_ADDSS_MEMORY_UNMASKED.md),
and [stage plan](../../../../docs/plans/STAGE_2.md).

### S2-38ad ADDSD m64 memory unmasked exceptions

The exact Long64 `F2 0F 58 00` profile requires SSE2 bit 26 and performs
one read-only read8 from `[RAX]`. The independent raw-u64 oracle covers the
existing ADDSD operand/result classes across RC × 64 prior sticky histories ×
IM/DM/PM × OSXMMEXCPT. Verify pre-event IE/DE, post-event PE, signed NaNs,
atomic XMM0/RIP/retirement suppression, high64 preservation on success, and
literal success/exception fixtures. Also cover full fetch/gate/CPL ordering,
AC/local rejection, unaligned reads with AC clear, canonical span/read rollback,
one callback, read-only schema, tampering and interpreter-only backend denial.
S2-37dj/S2-37bb remain separate; unmasked OE/UE and physical x86 acceptance
are outside this profile.

### S2-38ae SUBSS m32 memory unmasked exceptions

The exact Long64 `F3 0F 5C 00` profile requires SSE bit 25 and performs one
read-only read4 from `[RAX]`. The independent raw-u32 oracle covers subtraction
with the actual memory RHS, original-operand NaN selection, signed QNaN/SNaN in
either position, NaN+subnormal, same-sign invalid and opposite-sign valid
infinities, signed zero, rounding and exact tiny results. Cross RC × 64 prior
sticky histories × IM/DM/PM × OSXMMEXCPT. Verify pre-event IE/DE, post-event PE,
atomic XMM0/RIP/retirement suppression, high96 preservation on success, and
literal success/exception fixtures. Also cover fetch/gate/CPL/AC ordering,
unaligned access with AC clear, canonical span/read rollback, one callback,
strict read-only schema, tampering and interpreter-only backend denial. Keep
S2-37dk/S2-37aq separate; unmasked OE/UE and physical x86 acceptance are outside
this profile.

Literal cases: `s2_38ae_subss_success.case.json`,
`s2_38ae_subss_ie_xm.case.json`, `s2_38ae_subss_ie_ud.case.json`,
`s2_38ae_subss_de_xm.case.json`, and `s2_38ae_subss_pe_ud.case.json`.

### S2-38af SUBSD m64 memory unmasked exceptions

Implemented as the immutable interpreter-only profile
`s2-long-subsd-memory-unmasked-rax-v1`. It requires SSE2 bit 26 and performs
one read-only read8 from `[RAX]`; four literal fixtures cover success and IE,
DE and PE outcomes. A 180,224-case direct interpreter matrix crosses 22 raw
operand vectors with all RC values, 64 old sticky histories, IM/DM/ZM/PM mask
combinations and both OSXMMEXCPT mappings. Vectors include signed QNaN/SNaN in
either operand position, NaN+subnormal, same-sign invalid and opposite-sign
valid infinities, signed zero, finite rounding, exact tiny and overflow values.
The independent raw-u64 result validator uses the actual memory RHS and checks
pre-event IE/DE, post-event PE, status commit, XMM0/RIP/retirement suppression,
and high64 preservation. Focused tests cover gate order, AC-before-read,
canonical span/read rollback, read-only schema, literal result/control/error
code tampering and JIT/native denial. Keep S2-37dl/S2-37ap separate; unmasked
OE/UE and physical x86 acceptance are outside this profile.

### S2-38ag MULSS m32 memory unmasked exceptions

Exact Long64 `F3 0F 59 00`, SSE bit 25, and one read-only read4 from `[RAX]`.
The independent raw-u32 oracle uses the memory RHS and covers signed SNaN/QNaN
in both operands, NaN+subnormal, zero×infinity, signed zero/infinity,
exact/inexact products, overflow, tiny results and WPR rounding. Cross RC × 64
prior sticky histories × IM/DM/ZM/PM × OSXMMEXCPT. Verify pre-event IE/DE,
post-event PE, atomic XMM0/RIP/retirement suppression, high96 preservation, and
literal success/exception fixtures. Also cover exact fetch/gate/CPL/AC order,
unaligned access with AC clear, canonical span/read rollback, one callback,
read-only schema, result/control tampering and interpreter-only backend denial.
Keep S2-37dm/S2-37au separate; native/JIT/C ABI/generic/fragment and physical
x86 acceptance remain outside this profile.

### S2-38ah MULSD m64 memory unmasked exceptions

Exact Long64 `F2 0F 59 00`, SSE2 bit 26, and one read-only read8 from `[RAX]`.
The independent raw-u64 oracle uses the memory RHS and covers signed SNaN/QNaN
in both operands, NaN+subnormal, subnormal-input DE, exact/inexact tiny,
zero×infinity, signed zero/infinity, exact/inexact products, overflow and WPR
rounding. Cross RC × 64 prior sticky histories × IM/DM/ZM/PM × OSXMMEXCPT
across 20 vectors (163,840 runtime cases), including DE with inexact-tiny
continuation. Verify pre-event IE/DE,
post-event PE, atomic XMM0/RIP/retirement suppression, high64 preservation, and
literal success/exception fixtures. Also cover exact fetch/gate/CPL/AC order,
unaligned access with AC clear, canonical span/read rollback, one callback,
read-only schema, result/control tampering and interpreter-only backend denial.
Keep S2-37dn/S2-37at separate; native/JIT/C ABI/generic/fragment and physical
x86 acceptance remain outside this profile.
