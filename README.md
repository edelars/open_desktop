# Open Desktop

Собственная платформа для запуска x86-64 гостевых ОС на macOS/Apple Silicon. Декодер, интерпретатор, JIT ARM64, виртуальная машина и интеграция разрабатываются самостоятельно.

**Текущее состояние:** реализуются ограниченные вертикальные срезы CPU и системных компонентов Stage 2; это не означает готовность полной VM или поддержку полного x87. В частности, `od-isa` поддерживает bounded x87 compare/pop формы для canonical Normal binary80 и bounded memory conversions, включая `FLD m80fp` (`DB 28`) с signed-zero, binary80-subnormal, canonical signed-infinity, QuietNaN, masked canonical SignalingNaN и pseudo-denormal raw loads без новых status flags, `FLD m32fp` (`D9 00`) и `FLD m64fp` (`DD 00`) с signed-zero, masked nonzero-subnormal loads, signed infinities, canonical quiet NaNs с сохранением payload и masked canonical signaling NaNs с binary80 quieting и sticky IE; только эти две bounded binary32/binary64 формы сохраняют sticky IE/DE/OE/UE/PE по локальному статусному gate. Bounded `FRNDINT` (`D9 FC`) handles local Special-tagged unnormal and exact pseudo-infinity slices as masked invalid with a fixed negative binary80 indefinite, while preserving previously admitted sticky flags and clearing C1; the result re-enters as the existing QNaN identity. It also preserves seeded historical SF only when IE is present; SF without IE remains unsupported. These seeded states are local interpreter coverage, not native evidence. Bare `FLD ST(0)` (`D9 C0`) is bounded to canonical Normal and signed Zero raw-copy pushes into an Empty slot. Bare `FFREE ST(i)` (`DD C0..C7`) is bounded to tag-only freeing of one indexed physical cell under an exact x87 environment gate; neither slice enables general stack semantics. Native x87 execution remains closed. Фактические границы и проверки описаны в [плане этапа 2](docs/plans/STAGE_2.md), [CPU contract](docs/specs/CPU_CONTRACT.md) и [отчёте](docs/reports/STAGE_2_REPORT.md).

Bounded `FST m32fp`/`FSTP m32fp` (`D9 10`/`D9 18`) also store canonical signed binary80 zero as binary32 `+0` or `-0`; FSTP pops only after the atomic store succeeds.

The matching bounded `FST m64fp`/`FSTP m64fp` (`DD 10`/`DD 18`) forms store canonical signed binary80 zero as binary64 `+0` or `-0`, with the same post-write pop rule.

Those bounded m64 store forms also quiet canonical Special-tagged binary80 SignalingNaNs in the binary64 destination and set sticky IE only after a successful atomic write; the raw x87 source remains unchanged. Native/JIT admission and physical acceptance remain deferred.

## Начало работы

1. Прочитать [правила проекта](AGENTS.md) и [карту каталогов](docs/PROJECT_LAYOUT.md).
2. Ознакомиться с [архитектурой](docs/ARCHITECTURE.md) и [roadmap](docs/ROADMAP.md).
3. Выполнять [план этапа 0](docs/plans/STAGE_0.md) в пределах текущего поручения.

Проверки каркаса из корня проекта:

```sh
python3 tools/check_layout.py
cargo fmt --all -- --check
cargo check --workspace --all-targets --offline
```

Используются Cargo/Rust stable с edition 2024 и системные macOS SDK. Версии компилятора, SDK и macOS нужно записывать вместе с замерами. Точная фиксация toolchain и воспроизводимый app build входят в этап 0; Xcode project и подписанная VMHost-сборка ещё не созданы.

`work/` предназначен для промежуточных файлов, `outputs/` — для готовых deliverables. Долговечная документация хранится в `docs/`.


Bounded `FSTP m80fp` (`DB 38`) also preserves canonical signed binary80 zero
when the physical TOP tag is Zero, copying the raw ten bytes and popping only
after the atomic store succeeds. Native/JIT admission remains closed.


Bounded `FSTP m80fp` (`DB 38`) also copies canonical signed binary80 infinity
when tagged Special, preserving all ten bytes and popping only after the atomic
store succeeds. Native/JIT admission remains closed.

Bounded `FST/FSTP m64fp` (`DD 10`/`DD 18`) also stores canonical signed
binary80 infinity from a Special-tagged TOP as exact binary64 infinity. The
one-write/no-read and post-success-pop rules remain; native/JIT admission stays
closed.

Bounded `FST/FSTP m32fp` (`D9 10`/`D9 18`) also stores canonical signed
binary80 infinity from a Special-tagged TOP as exact binary32 infinity. The
one-write/no-read and post-success-pop rules remain; native/JIT admission stays
closed.

Bounded `FSTP m80fp` (`DB 38`) also preserves canonical binary80 QuietNaNs
byte-for-byte when physical TOP is Special-tagged. It adds no flags and pops
only after the atomic ten-byte write; native/JIT admission stays closed.

Exact `DB 38` also raw-stores canonical binary80 SignalingNaNs without
quieting or setting status flags in the bounded S2-31as interpreter slice.
General x87 exception delivery and physical acceptance remain deferred; see
the [CPU contract](docs/specs/CPU_CONTRACT.md#local-s2-31as-canonical-signalingnan-fstp-m80fp-rax-db-38).

Exact `DB 38` also preserves canonical binary80 subnormals byte-for-byte in
the bounded S2-31at interpreter slice, without normalization or new flags.
Physical acceptance remains deferred; see the [CPU contract](docs/specs/CPU_CONTRACT.md#local-s2-31at-canonical-subnormal-fstp-m80fp-rax-db-38).

Bounded `FST/FSTP m64fp` (`DD 10`/`DD 18`) also converts canonical
Special-tagged binary80 QuietNaNs to binary64 while preserving sign and the
upper 52 payload bits and truncating the lower 11. It adds no flags; native/JIT
admission remains closed.

Bounded `FST/FSTP m32fp` (`D9 10`/`D9 18`) also converts canonical
Special-tagged binary80 QuietNaNs to binary32, preserving sign and the upper 23
payload bits while truncating the lower 40. It adds no flags; native/JIT
admission remains closed.

Bounded Long64 REP MOVSB/W/D/Q (`F3 A4/A5`) is implemented in the local
interpreter with source-first, destination-second element transfers. RSI, RDI
and RCX commit only after both accesses succeed; STOP, errors and the local
1024-transfer limit preserve completed pairs. The new forms remain closed to
JIT/native execution and await the shared physical acceptance. Details:
[CPU contract](docs/specs/CPU_CONTRACT.md#local-s2-19d-bounded-rep-movsbwdq-contract),
[Stage 2 plan](docs/plans/STAGE_2.md#s2-19d--f3-a4a5-rep-movsbwdq-ограниченный-локальный-контракт).

The local-only S2-35.49 `PAND` diagnostic profile accepts only register-form
`66 [REX.R/B] 0F DB /r` and preserves unrelated CPU state. Its literal fixture
is interpreter-only; JIT/native and physical SSE2 execution are not claimed.

The local-only S2-37ae `CVTTSD2SI` profile admits canonical Long64 register
forms for r32/r64 and truncates raw binary64 toward zero independently of
MXCSR.RC. Its schema-v2 oracle and 512-form fixture matrix are in place;
generic/JIT/native execution is closed, and physical acceptance remains in the
combined final stage. See the [CPU contract](docs/specs/CPU_CONTRACT.md#s2-37ae--canonical-long64-cvttsd2si-register-space)
and [Stage 2 report](docs/reports/STAGE_2_REPORT.md#s2-37ae--canonical-long64-cvttsd2si-register-space).

The local-only S2-37av `DIVPD` profile admits canonical Long64 register-pair
forms and performs two binary64 lanes with MXCSR exception ordering. Its
independent raw-u64 oracle, literal fixture and 256-pair interpreter coverage
are in place; JIT/native and physical execution remain closed. See the [CPU
contract](docs/specs/CPU_CONTRACT.md#s2-37av--bounded-masked-range-long64-divpd-xmm-register-pairs)
and [Stage 2 report](docs/reports/STAGE_2_REPORT.md#s2-37av--divpd-register-pair-interpreter-profile).

The local-only S2-37aw `DIVPS` profile extends that bounded register-pair path
to four binary32 lanes with independently checked raw-rational rounding and
cross-lane exception ordering. It remains interpreter-only; memory forms and
physical acceptance are not claimed. See the [CPU
contract](docs/specs/CPU_CONTRACT.md#s2-37aw--bounded-masked-range-long64-divps-xmm-register-pairs)
and [Stage 2 report](docs/reports/STAGE_2_REPORT.md#s2-37aw--divps-register-pair-interpreter-profile).


The local-only S2-34.20 ANDNPD profile supports long-mode register `66 [REX.R/B] 0F 55 /r` as `(~destination) & source` across raw 128-bit XMM values; ANDNPS remains separate. JIT/native and physical execution remain outside the profile. See the [CPU contract](docs/specs/CPU_CONTRACT.md#local-profile-s2-3420-s2-long-andnpd-reg-v1).

The local-only S2-37ax `DIVSD` profile admits canonical Long64 register-pair
encodings, divides only the low binary64 lane, preserves destination.high64 and
ignores source.high64. Its independent exact-rational oracle, literal fixture
and interpreter coverage are recorded in the [CPU contract](docs/specs/CPU_CONTRACT.md#s2-37ax--bounded-masked-range-long64-divsd-xmm-register-pairs),
[Stage 2 plan](docs/plans/STAGE_2.md#s2-37ax--canonical-long64-divsd-xmm-register-pairs)
and [Stage 2 report](docs/reports/STAGE_2_REPORT.md#s2-37ax--divsd-register-pair-interpreter-profile).
Memory forms, JIT/native execution and physical acceptance remain outside this
profile.


The local-only S2-37ay `DIVSS` profile admits canonical Long64 register-pair
forms, divides only low32, preserves destination.high96 and ignores source.high96.
Its independent rational oracle and 256-pair interpreter matrix cover MXCSR RC,
WPR/FTZ rounding and SIMD exception ordering. The profile is SSE, interpreter-only,
and does not claim memory forms or physical acceptance. See the [CPU contract]
(docs/specs/CPU_CONTRACT.md#s2-37ay--canonical-long64-divss-xmm-register-pairs),
[Stage 2 plan](docs/plans/STAGE_2.md#s2-37ay--canonical-long64-divss-xmm-register-pairs)
and [report](docs/reports/STAGE_2_REPORT.md#s2-37ay--divss-register-pair-interpreter-profile).

The local-only S2-37bd `CVTPD2PS` profile narrows two binary64 lanes to the low
64 bits of an XMM destination and clears its upper 64 bits after success. Its
strict schema-v2 register profile covers MXCSR rounding/exception ordering and
all 256 XMM pairs; physical acceptance and memory forms remain outside scope.
See the [CPU contract](docs/specs/CPU_CONTRACT.md#s2-37bd--canonical-long64-cvtpd2ps-xmm-register-pairs)
and [Stage 2 report](docs/reports/STAGE_2_REPORT.md#s2-37bd--cvtpd2ps-register-pair-interpreter-profile).


The local-only S2-37be `CVTSD2SS` profile narrows source.low64 under MXCSR.RC,
writes destination.low32 and preserves its high96. The strict schema-v2 slice
covers all 256 XMM pairs, FTZ/rounding and atomic SIMD exceptions; JIT/native,
memory forms and physical acceptance remain closed. See the [CPU contract](docs/specs/CPU_CONTRACT.md#s2-37be--canonical-long64-cvtsd2ss-xmm-register-pairs),
[Stage 2 plan](docs/plans/STAGE_2.md#s2-37be--canonical-long64-cvtsd2ss-xmm-register-pairs)
and [Stage 2 report](docs/reports/STAGE_2_REPORT.md#s2-37be--cvtsd2ss-register-pair-interpreter-profile).

The local-only S2-37bf `CVTPI2PD` profile accepts only legacy `66 0F 2A 00`,
reads one m64 from canonical RAX and writes two exact signed-i32 binary64 lanes
to XMM0. It preserves MXCSR and x87 state, requires AC=0, and remains
interpreter-only; other address forms, JIT/native execution and physical
acceptance are outside the profile.

The local-only S2-37bg `CVTPI2PS` profile accepts only legacy `0F 2A 00`,
reads one m64 from canonical RAX and converts the two signed-i32 lanes to
binary32 under MXCSR.RC. It preserves XMM0.high64, sets sticky PE when either
lane is inexact, and on success transitions x87 TOP/tag_word to zero while
preserving the other x87 state. It requires AC=0, PM=1 and no pending x87
exception; other operand forms, JIT/native execution and physical acceptance
remain outside the profile. See the [CPU contract](docs/specs/CPU_CONTRACT.md#s2-37bg--canonical-long64-cvtpi2ps-memory-only-rax),
[Stage 2 plan](docs/plans/STAGE_2.md#s2-37bg--canonical-long64-cvtpi2ps-memory-only-rax)
and [Stage 2 report](docs/reports/STAGE_2_REPORT.md#s2-37bg--cvtpi2ps-memory-interpreter-profile).
