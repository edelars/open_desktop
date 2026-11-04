# S2-39k — Linux x86_64 acceptance, 2026-11-04

**Результат:** один запуск существующего `od-x86-runner --fxsave64-oracle`
выполнил A/B/A/B; все четыре наблюдения прошли независимый встроенный
компаратор, команда вернула `0`, stderr пустой. Подтверждены linked-binary
inspection, Linux runtime/ptrace и ограниченный successful CPL3 FXSAVE64
oracle. Это не закрывает родительский S2-39 или весь Stage 2.

`environment.execution_kind` в исходном evidence равен `unknown`.
CPUID не показывает hypervisor bit, `systemd-detect-virt` вернул `none`
(exit `1`), однако эти признаки не заменяют независимое подтверждение
физического стенда. Отчёт фиксирует успешное Linux x86_64 native исполнение,
не повышая его до отдельно подтверждённой physical Mac acceptance.

## Стенд и происхождение

- UTC: `2026-11-04T15:09:41.058533+00:00` —
  `2026-11-04T15:09:45.713942+00:00`.
- Host: `senya@192.168.1.39`, kernel hostname `mini`;
  checkout `/home/senya/projects/open_desktop`.
- CPU: Intel Core i5-3210M, vendor `GenuineIntel`, family `6`, model `58`,
  stepping `9`; affinity CPU `0`, FXSR/SSE присутствуют.
- Linux `7.0.0-38-generic`, x86_64; rustc
  `1.99.1 (58cf8f9c3 2026-10-13)`, cargo `1.99.1 (5f94df478 2026-08-27)`.
- Remote HEAD: `0a0640b5a7d54b13b3024146b5c9068f0f15bb5d`;
  рабочая копия имеет изменения `tools/x86-runner/src/native_fp.rs` и
  `tools/x86-runner/src/native_v2.rs`. Их полный diff и хеши сохранены в
  preflight; эти изменения не выдаются за чистую ревизию.
- Для запуска использован существующий ELF `target/debug/od-x86-runner`:
  debug, target `x86_64-unknown-linux-gnu`, embedded rustflags `none`.
  Новая сборка и правки исходников в рамках этой проверки не выполнялись.
  Полный исторический build log этого бинарника не устанавливался.
- Embedded SHA-256 harness/comparator/protocol совпали с remote preflight и
  локальными файлами при HEAD `216cf07b4a87eb48fc53d8c237719a9a4709a3eb`.
  SHA-256 бинарника до/после исполнения совпадает. Это связывает проверку
  с точным бинарником и указанными компонентами, а не доказывает совпадение
  всего dirty checkout с содержимым исторической сборки.

| Объект | SHA-256 |
|---|---|
| Бинарник | `e363693116a88d6c9e8bd752c7eefc7cc893f97ddea716d2d82b2d514ae28c93` |
| `tools/x86-runner/src/fxsave64.rs` | `e4e31e23589f070d4c56f964b8852a5507589f542ff143f327fa71095de965c6` |
| `crates/od-testkit/src/fxsave64.rs` | `f4f4b0a3a88213834ecfdae8ba6f86863d75b41bc16664ae3a45c63cd9f4d5e8` |
| Нормативный S2-39j protocol | `eb7be384828b960782f0753dbe96c7664b5facc82937ee978db0eada4f9a08c7` |
| Literal table | `284554b45ee3ae335a365c99133f848c8aa5664074efeaf22b32aa8bc2c8192c` |
| Исполняемый assembly block | `8d8704becd572c0aad691dba6a3f4b786deabf76a1aade324e7c014d615534ec` |
| Raw evidence JSON | `fb323ed37f8be6a10d593284e579c4db79aa2fa12fa3f5e5d70264e4ac1b0208` |

## Проверки и наблюдения

До запуска дизассемблирован именно итоговый ELF. `od_fxsave64_sequence`
содержит 693 байта; точные `48 0F AE 40 40` находятся на offset `425`,
непосредственно за ними — `CC`. Проверены полные MOVDQU XMM0–15 и STMXCSR
сохранения caller state, literal setup, pre/post observations и restore;
последовательность заканчивается `RET`, не содержит `CALL`, переходов или
FXRSTOR. Offsets согласованы с `repr(C, align(16))` Context, SysV arguments
используют RDI/RSI, callee-saved GPR и RSP не меняются.
После запуска все 693 байта из raw evidence побайтно совпали с linked
disassembly; embedded source/protocol/comparator hashes тоже проверены.

| Порядок | Case | MXCSR | Маркер | Exit | Результат |
|---|---|---|---|---|---|
| 0 | A | `0x1f80` | SIGTRAP code `128`, RIP `E+6` | `0` | passed |
| 1 | B | `0x7f80` | SIGTRAP code `128`, RIP `E+6` | `0` | passed |
| 2 | A | `0x1f80` | SIGTRAP code `128`, RIP `E+6` | `0` | passed |
| 3 | B | `0x7f80` | SIGTRAP code `128`, RIP `E+6` | `0` | passed |

Во всех четырёх записях `attempted=true`, `completion_marker=true`,
`execution_error=null`, `raw_wait_status=0`, `signal=null`. Нормализованная
граница равна `E+5`, поправка на INT3 — один байт; hardware retirement counter
не заявляется. RAX+64 указывает на 16-byte-aligned 512-byte image.

Встроенный независимый `od-testkit::fxsave64` comparator проверил нормативную
evidence schema и все обязательные поля. Дополнительно без повторного native
запуска проверены literal XMM bytes `00..FF` для A и `FF..00` для B во всех
pre/post observations и image `[160,416)`, точный MXCSR `[24,28)`, неизменный
software tail `[464,512)` и обе 32-byte guards. Raw MXCSR_MASK во всех случаях
составляет bytes `FF FF 00 00`; он сохранён без нормализации и без сравнения
с синтетическим target mask. x87, reserved/unused ranges остаются raw/excluded
согласно [S2-39j](../testing/S2_39J_FXSAVE64_ORACLE.md).

## Evidence и воспроизведение

Все raw outputs остаются в ignored `work/`. Remote оригинал:
`/home/senya/projects/open_desktop/work/stage2/s2-39k-fxsave64/fxsave64-1793804985707874526.json`.
Локальная отдельная копия:

- [Raw evidence JSON](../../work/stage2/s2-39k-acceptance-20261104-01/raw-evidence.json),
  сохранённый без изменения байтов.
- [Run envelope](../../work/stage2/s2-39k-acceptance-20261104-01/run.json):
  точные argv/cwd/UTC, stdout/stderr, exit code и бинарные хеши до/после.
- [Preflight](../../work/stage2/s2-39k-acceptance-20261104-01/preflight.json):
  source hashes, dirty diff, ОС/CPU/toolchain и virtualization observations.
- [Linked disassembly](../../work/stage2/s2-39k-acceptance-20261104-01/linked-sequence.objdump.txt)
  и [offline verification](../../work/stage2/s2-39k-acceptance-20261104-01/verification.json).

Точная команда внутри remote checkout, с `/home/senya/.cargo/bin` в PATH:

```sh
timeout --signal=TERM --kill-after=5s 60s /home/senya/projects/open_desktop/target/debug/od-x86-runner --fxsave64-oracle
```

При повторении runner создаст новый timestamped JSON в штатном
`work/stage2/s2-39k-fxsave64/`; исходное evidence не перезаписывается.
Сначала нужно повторить проверку исходников/бинарника и emitted-code/ABI
предпосылок [S2-39k](../testing/S2_39K_FXSAVE64_ORACLE.md). Будущий запуск
из другого бинарника является отдельным evidence.

Пять S2-42 suites не запускались. Interpreter integration, x87 source cases,
FXRSTOR, control-gate/fault coverage, другие адресные формы, privileged/machine
integration и отдельно подтверждённая physical Mac acceptance остаются за
границами этого среза. Новых дефектов не обнаружено.

Авторская проверка evidence и независимое ревью отчёта завершены. Замечание
ревью к согласованности статусных обновлений устранено в подготовленном patch.
