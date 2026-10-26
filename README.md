# Open Desktop

Собственная платформа для запуска x86-64 гостевых ОС на macOS/Apple Silicon. Декодер, интерпретатор, JIT ARM64, виртуальная машина и интеграция разрабатываются самостоятельно.

**Текущее состояние:** реализуются ограниченные вертикальные срезы CPU и системных компонентов Stage 2; это не означает готовность полной VM или поддержку полного x87. В частности, `od-isa` поддерживает bounded x87 compare/pop формы для canonical Normal binary80 и bounded memory conversions, включая `FLD m80fp` (`DB 28`) с signed-zero, binary80-subnormal, canonical signed-infinity, QuietNaN, masked canonical SignalingNaN и pseudo-denormal raw loads без новых status flags, `FLD m32fp` (`D9 00`) и `FLD m64fp` (`DD 00`) с signed-zero, masked nonzero-subnormal loads, signed infinities, canonical quiet NaNs с сохранением payload и masked canonical signaling NaNs с binary80 quieting и sticky IE; только эти две bounded binary32/binary64 формы сохраняют sticky IE/DE/OE/UE/PE по локальному статусному gate. Bounded `FRNDINT` (`D9 FC`) handles local Special-tagged unnormal and exact pseudo-infinity slices as masked invalid with a fixed negative binary80 indefinite, while preserving previously admitted sticky flags and clearing C1; the result re-enters as the existing QNaN identity. It also preserves seeded historical SF only when IE is present; SF without IE remains unsupported. These seeded states are local interpreter coverage, not native evidence. Native x87 execution остаётся закрытым. Фактические границы и проверки описаны в [плане этапа 2](docs/plans/STAGE_2.md), [CPU contract](docs/specs/CPU_CONTRACT.md) и [отчёте](docs/reports/STAGE_2_REPORT.md).

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
