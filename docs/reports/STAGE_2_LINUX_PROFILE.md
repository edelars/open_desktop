# S2-00: статическое исследование Linux kernel candidate

**Дата проверки:** 28 октября 2026 года. **Статус:** проверены подписи и
метаданные exact package/source snapshot, config, direct64 prefix и main-kernel
startup source/binary mapping. Linux boot и полный профиль C/D не подтверждены.
Исследование не меняет выбранный Ubuntu candidate и не закрывает обязательные
задачи этапа 2.

## Полученные данные и происхождение

Через HTTPS с официального `archive.ubuntu.com` прочитаны три небольших
пакета. Они разобраны в памяти исследовательского процесса стандартными
средствами Python (`urllib.request`, ar headers, `tarfile`, `hashlib`,
`struct`); установка, запуск kernel, изменение host/bootloader, SSH и
запись стороннего kernel code в проект не выполнялись. При первичном
исследовании пакеты и образ не сохранялись; точные deb и извлечённые members
позднее помещены в локальный ignored staging, описанный в конце отчёта. Значения ниже измерены по полученным
байтам; это не данные запущенного гостя.

| Артефакт | Размер, bytes | Измеренный SHA-256 |
|---|---:|---|
| [linux-image-generic-hwe-26.04_7.0.0-38.38_amd64.deb](https://archive.ubuntu.com/ubuntu/pool/main/l/linux-meta/linux-image-generic-hwe-26.04_7.0.0-38.38_amd64.deb) | 2460 | `560ae7c18d2ec4281e1d663d96851306a04030964d9a9ae6acdaaba1250c1f5c` |
| [linux-image-7.0.0-38-generic_7.0.0-38.38_amd64.deb](https://archive.ubuntu.com/ubuntu/pool/main/l/linux-signed/linux-image-7.0.0-38-generic_7.0.0-38.38_amd64.deb) | 17040232 | `f268d635efaa2e19a18e12813ffdbc2e32db2390cf67d4416abd59292975e7aa` |
| [linux-buildinfo-7.0.0-38-generic_7.0.0-38.38_amd64.deb](https://archive.ubuntu.com/ubuntu/pool/main/l/linux/linux-buildinfo-7.0.0-38-generic_7.0.0-38.38_amd64.deb) | 667928 | `cf640f42473d610969cc7c4ca6491f277e1eac47da06a4c347165728280fe4bd` |
| `boot/vmlinuz-7.0.0-38-generic` внутри signed image package | 17320328 | `9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e` |
| `usr/lib/linux/7.0.0-38-generic/config` внутри buildinfo package | 308501 | `6cb18717c6914d4947489eedf8cb69ac23db3faa8ef9b9f563ff02c346dcd32d` |

Измеренный digest signed image package совпал с опубликованным
[Ubuntu package download metadata](https://packages.ubuntu.com/resolute-updates/amd64/linux-image-7.0.0-38-generic/download).
При первоначальном исследовании подпись APT `InRelease` и PE/Authenticode
signature не верифицировались; последующая проверка APT описана ниже. Слово
signed — свойство пакета и его control metadata,
не заявление о выполненной криптографической проверке.

Из control metadata: metapackage имеет `Source: linux-meta`, version
`7.0.0-38.38`, architecture `amd64` и зависит от
`linux-image-7.0.0-38-generic` вместе с modules, ZFS modules, firmware и
microcode packages. Сам метапакет не является образом ядра. Image package
имеет `Source: linux-signed` и `Built-Using: linux (= 7.0.0-38.38)`;
buildinfo имеет `Source: linux` и ту же version/architecture. Это связывает
исследованный config с конкретной сборкой, а не с произвольным upstream
defconfig. Полный состав installed Ubuntu ISO и initramfs не исследован.

## Наблюдаемый setup header

Поля прочитаны little-endian по смещениям
[Linux v7.0 Boot Protocol](https://raw.githubusercontent.com/torvalds/linux/v7.0/Documentation/arch/x86/boot.rst).

| Поле | Значение |
|---|---|
| `boot_flag` / `header` | `0xaa55` / `HdrS` |
| `version` | `0x020f`, protocol 2.15 |
| `setup_sects` / `loadflags` | `39` / `0x01` |
| `xloadflags` | `0x007f`; bit 0 (`XLF_KERNEL_64`) установлен |
| `code32_start` | `0x00100000` |
| `kernel_alignment` / `min_alignment` | `0x00200000` / `21` |
| `relocatable_kernel` | `1` |
| `pref_address` | `0x01000000` |
| `init_size` | `0x044c1000` |

Version string образа: `7.0.0-38-generic`, `#38-Ubuntu SMP
PREEMPT_DYNAMIC Fri Sep 4 09:10:14 UTC 2026`.

По протоколу direct64 entry расположен на `+0x200` от начала загруженной
protected-mode части, а не на этом смещении от начала файла `vmlinuz`.
При входе нужны long mode с paging, identity mappings для диапазона
`init_size`, zero page и command line, корректная GDT/segment selectors,
выключенные interrupts и `%rsi = boot_params`. Эти условия описывают
handoff; они не перечисляют все инструкции kernel и не доказывают boot.

## Что config уточняет о CPU

Compiler metadata buildinfo: `GCC: (Ubuntu 15.2.0-16ubuntu1) 15.2.0`;
`CONFIG_CC_VERSION_TEXT` указывает `x86_64-linux-gnu-gcc` той же версии.
`CONFIG_64BIT=y`, `CONFIG_X86_64=y`, `CONFIG_SMP=y`,
`CONFIG_RELOCATABLE=y`, `CONFIG_RANDOMIZE_BASE=y`, `CONFIG_KERNEL_ZSTD=y`.

Набор установленных `CONFIG_X86_REQUIRED_FEATURE_*`:

```text
ALWAYS, NOPL, CX8, CMOV, CPUID, FPU, PAE, MSR, FXSR, XMM, XMM2, LM
```

Это названия Linux capabilities; `ALWAYS` — служебная capability, а
список нельзя механически объявить перечнем аппаратных CPUID bits.
Назначение required/disabled Kconfig features и генерации масок описано
в [upstream Linux v7.0 Kconfig.cpufeatures](https://raw.githubusercontent.com/torvalds/linux/v7.0/arch/x86/Kconfig.cpufeatures).
В частности, config уже даёт основание учитывать NOPL, CMPXCHG8B, CMOV,
CPUID, x87/FPU, PAE, MSR, FXSAVE/FXRSTOR, SSE/SSE2 и long mode в
исследовании Linux profile. Это нижняя граница исследования, не полный
набор обязательных opcode/forms, режимов и исключений.

`CONFIG_X86_NATIVE_CPU` выключен. В
[upstream Linux v7.0 Makefile](https://raw.githubusercontent.com/torvalds/linux/v7.0/arch/x86/Makefile)
64-bit generic ветвь использует `-march=x86-64 -mtune=generic`, а обычная
компиляция подавляет FP/SIMD code generation; специальные пути могут
использовать FP/SIMD отдельно. Поэтому отсутствие autovectorization в
основном kernel code не исключает FPU/SIMD state и его сохранение.
Ubuntu-specific patch/build flags и фактические инструкции всех путей
этим upstream Makefile не установлены.

[Ubuntu architecture policy](https://ubuntu.com/project/docs/how-ubuntu-is-made/concepts/supported-architectures/),
обновлённая 7 октября 2026 года, различает обычный `amd64` baseline и
вариант `amd64v3`, который допускает AVX2/FMA/BMI2. В исследованных control
metadata указан `amd64`; данных для автоматического назначения ему v3
нет. Это не исключает дополнительных требований выбранного initramfs,
userspace, драйверов или CPUID-зависимых kernel paths.

Config включает `CONFIG_IA32_EMULATION=y`, `CONFIG_EFI_MIXED=y`,
`CONFIG_X86_16BIT=y`, `CONFIG_X86_FRED=y`, `CONFIG_X86_CET=y`,
`CONFIG_X86_X2APIC=y`. Возможность скомпилирована — не значит, что CPU
обязан объявить её в CPUID или выбранный boot path её исполняет.
Например, [upstream v7.0 Kconfig](https://raw.githubusercontent.com/torvalds/linux/v7.0/arch/x86/Kconfig) описывает FRED как условный выбор на
поддерживающем CPU. Нельзя ни объявить эти features обязательными по
одному `=y`, ни исключить соответствующие проверки без анализа пути.

Особенно важен переход paging modes: upstream v7.0
[compressed/head_64.S](https://raw.githubusercontent.com/torvalds/linux/v7.0/arch/x86/boot/compressed/head_64.S)
вызывает `configure_5level_paging`; в
[pgtable_64.c](https://raw.githubusercontent.com/torvalds/linux/v7.0/arch/x86/boot/compressed/pgtable_64.c)
32-bit trampoline нужен при несовпадении текущего и требуемого LA57.
Если они совпадают, функция возвращается без trampoline. Следовательно,
direct64 сам по себе не доказывает отсутствие дальнейшего 32-bit пути;
нужны согласованные CPUID, CR4.LA57, page tables и command line. Это
анализ upstream источника, который ещё нужно сопоставить с точным Ubuntu
source package и фактическим выполнением выбранного образа.

## Решение по Generic amd64 baseline

**User decision:** целевой Ubuntu-кандидат — Generic `amd64`. Его нормативное
прочтение как ABI/ISA floor — baseline x86-64 psABI (x86-64-v1); уровни
x86-64-v2/v3/v4 не принимаются. Ubuntu описывает обычный `amd64` как baseline
без расширений сверх исходных 64-bit AMD/Intel CPU, отдельно выделяя
`amd64v3`; psABI задаёт baseline и кумулятивные уровни v2–v4. См. прямые
источники: [Ubuntu supported architectures and baselines](https://ubuntu.com/project/docs/how-ubuntu-is-made/concepts/supported-architectures/)
и [upstream x86-64 psABI, Micro-Architecture Levels](https://gitlab.com/x86-psABIs/x86-64-ABI/-/blob/master/x86-64-ABI/low-level-sys-info.tex).
Сам floor ограничен ABI/ISA. После exact-source/binary mapping выбран
synthetic reference CPU target `od-x86-64-v1-neutral-v0.1`, подробно заданный
в [CPU contract](../specs/CPU_CONTRACT.md#ubuntu-generic-amd64-od-x86-64-v1-neutral-v01).
Он закрепляет project vendor/family/model/stepping и CPUID policy для будущей
реализации; это не идентичность существующего физического CPU и не полный
runtime contract. Полный MSR allowlist, entry state, runtime alternatives,
initramfs и критерий загрузки остаются незакрытыми.

| Уровень доказательств | Текущий статус | Чего он не доказывает |
|---|---|---|
| Нормативный ABI/ISA floor | Generic amd64; baseline x86-64 psABI (v1), без v2/v3/v4 | Полный набор ISA форм и system semantics выбранной ОС |
| Kernel config requirements | Для exact buildinfo зафиксированы required capabilities из таблицы выше | Что каждая capability обязательно включена в CPUID mask либо исполняется каждым boot path |
| Upstream source reachability | Сопоставлены upstream v7.0 paths и Ubuntu source delta; см. audit ниже | Что конкретные ветви включены/достижимы в точном binary и выбранной конфигурации |
| Exact Ubuntu source/binary | Exact kernel/source files подтверждены подписанным snapshot; распакованный kernel ELF совпал по 38 allocated sections с ELF из dbgsym того же build ID. Direct64 prefix, `startup_64`, early `verify_cpu`, CPUID, FPU init/save/restore и alternatives сопоставлены с source paths/lines | Compiled alternatives не означают, что ветка выполнялась; выбранный target пока не реализован и его runtime ветви не подтверждены |
| Observed execution | Ubuntu kernel guest boot не наблюдался | Успешный boot, C/D или поддерживаемая ОС |

Exact image, buildinfo, historical metapackage и source package `linux`
7.0.0-38.38 подтверждены подписанным snapshot `resolute-updates` с timestamp
`20261001T200000Z`. `.dsc`, source tarball и Ubuntu delta скачаны и повторно
сверены с SHA-256 из подписанного `Sources.xz`. Main kernel paths сопоставлены
с байтами exact image через совпадение ELF sections, build ID и DWARF; ранний
compressed direct64 prefix сопоставлен с исходником и неизменённой Ubuntu delta.
До scenario freeze остаются initramfs, command line, полное начальное
CPU/MSR/CR/segment/table/FP state, memory/device manifest и измеримый success
milestone. Выбранный reference CPU target определяет CPUID и несколько ранних
boundary behaviors, но не является реализацией. S2-00 принят для диагностического
A, x86-64-v1 ABI/ISA floor и выбора synthetic target; реализация/проверка target,
profile C/D и observed boot остаются открытыми.

## S2-26d — граница SEP и локального SYSENTER/SYSEXIT решения

The selected Generic amd64 CPUID target has SEP clear, as shown by the
CPUID.1 EDX value in the [CPU contract](../specs/CPU_CONTRACT.md).
The S2-26d `SepDisabledV1` interpreter policy maps exact bare Long64 `0F 34`
and `0F 35` to CPU-side #UD after complete instruction fetch. This is local
policy evidence only: it does not exercise Linux exception delivery or safe
MSR fixup, demonstrate an observed kernel branch, or establish boot behavior.
Intel SDM revision 093, Vol. 2B, documents the SEP presence flag and the
SYSENTER/SYSEXIT opcodes ([official Intel Vol. 2B PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf)).
See the [Stage 2 report](STAGE_2_REPORT.md#s2-26d--local-sepdisabledv1-sysentersysexit-policy).

## S2-40a — матрица требований Ubuntu Generic amd64

**Решение владельца:** для Ubuntu 26.04.1 используется кандидат Generic
`amd64`, с нормативным floor x86-64-v1. Это ABI/ISA floor, а не полный контракт
kernel, initramfs или userspace. В имеющемся evidence пока не доказана
необходимость расширений сверх v1. При этом inventory условных путей и
расширений C/D не завершён, поэтому отсутствие доказанной необходимости не
равно выводу «не требуется» для всей системы или полного профиля Ubuntu.
Не добавлять CPUID bits и не объявлять профиль завершённым на основании этой
матрицы.

В столбце source/image указана статическая reachability (config, source и
точные байты выбранного образа), если отдельно не сказано об observed runtime.
Статическая достижимость не доказывает, что ветка или инструкция исполнялась.
Локальная реализация означает ограниченный interpreter/testkit slice; она не
является доказательством поведения физического CPU или загрузки Linux.

**S2-00 reconciliation, 2026-10-29:** состояние ниже сверено с committed
`8490ec8`, включая S2-38j MAXPD, S2-38k MINPS и S2-38l MAXPS. Это та же матрица принятого
Generic amd64/x86-64-v1 target, не новый нормативный scope или полный exact-form
manifest C/D. `local implemented` означает только named bounded slices;
`not_run` — проверка не выполнялась; `unresolved` — requirement/evidence ещё не
установлен. Эти статусы не равны parent acceptance. В этом docs-only обновлении
CPU tests, Mac mini/SSH и Stage 4 integration не запускались.

| Требование / сценарий | Source и image evidence | Локальная реализация и tests | Недостающая семантика / граница | Независимый oracle и тип проверки | Владелец покрытия |
|---|---|---|---|---|---|
| Generic `amd64` ABI/ISA floor: x86-64-v1, без выбранного v2/v3/v4 | Публичное Ubuntu baseline и psABI; exact package/config metadata идентифицирует `amd64`. Это не исчерпывающий inventory исполняемых kernel/userspace путей. См. [решение baseline](#решение-по-generic-amd64-baseline) | Synthetic target `od-x86-64-v1-neutral-v0.1` описан в CPU contract; это пока спецификация, не готовая capability | Полный feature policy, extension inventory, userspace/compat requirements и доказательство runtime branch selection | CPU-only literal/architectural oracle может проверять отдельные ISA semantics, но не достаточность профиля ОС; user-mode physical oracle — отдельный Mac mini batch | S2-00, S2-40; общий corpus по S2-42 |
| Baseline integer/control/memory and atomic semantics (including configured NOPL, CX8 and CMOV capabilities) | Exact config lists NOPL/CX8/CMOV/CPUID and other required capabilities; config is a lower bound, not an executed-instruction inventory | Bounded local forms are tracked in [S2-15](../plans/STAGE_2.md#s2-15-остальные-integer-формы-профиля), [S2-16](../plans/STAGE_2.md#s2-16-условия-и-переходы), [S2-17](../plans/STAGE_2.md#s2-17-стек-callret-и-косвенное-управление), [S2-20](../plans/STAGE_2.md#s2-20-точность-исключений-и-exit-state), and [S2-21](../plans/STAGE_2.md#s2-21-однопроцессорные-atomicfence-формы); parent requirements remain open pending broader acceptance | Exact required instruction forms and memory/fault/ordering cases must be enumerated from the selected kernel, initramfs and userspace; bounded slices do not imply full v1 or full Linux coverage | CPU-only literal fixtures for each admitted form; Linux user-mode physical comparison for safe CPL3 cases only; privileged/system cases need another oracle | S2-15–21, S2-40, S2-42; S2-41 for privileged/system cases |
| Early direct64 entry, CPU identity/CPUID, MSR и initial state | Exact Ubuntu 7.0.0-38.38 package/buildinfo, config, source delta и ELF/source mapping подтверждены статически; direct64 prefix, `verify_cpu` reachability и main-kernel `RDMSR IA32_EFER` at `ffffffff8139c1b4` are mapped. This is not an absolute first-access or runtime claim. Kernel boot не наблюдался. Config `CONFIG_X86_REQUIRED_FEATURE_*` задаёт нижнюю границу, не точный CPUID mask | S2-25e implements only a local exact read-only IA32_EFER profile over four synthetic values and explicit bounded CR0/CR4; S2-26 covers separate MSR slices. No Linux entry value/runtime is claimed. Отдельные setup/descriptor forms покрыты S2-27. Target implementation и системная последовательность не завершены | CPUID leaves/subleaves and masks for target, full MSR allowlist, actual entry EFER/CR/segment/table/FP snapshot, mode/exception semantics and chosen alternatives; compiled CR0 `0x80050033` is not the authenticated entry snapshot | CPU-only contract tests для определённых форм; Linux user-mode physical execution не проверяет CPL0 handoff; privileged/system oracle требует пригодного независимого harness/backend | S2-25d/e, S2-26, S2-27, S2-41; Stage 4 для полной system integration |
| x87/MMX/SSE/SSE2 arithmetic, conversions and observable FP/SIMD state | Exact config lists FPU/FXSR/XMM/XMM2; [target contract](../specs/CPU_CONTRACT.md#ubuntu-generic-amd64-od-x86-64-v1-neutral-v01) also selects MMX. Source/image evidence identifies availability and conditional save/restore routes, not all executed forms | S2-28–38 contain bounded state/helpers/instruction slices. Committed [S2-38j](STAGE_2_REPORT.md#s2-38j--exact-long64-maxpd-with-bounded-unmasked-imdm-exits) adds exact MAXPD IM/DM with 128+16 fixtures; [S2-38k](STAGE_2_REPORT.md#s2-38k--exact-long64-minps-with-bounded-unmasked-imdm-exits) adds exact MINPS IM/DM with a 2048-case ISA matrix and 32 fixtures; [S2-38l](STAGE_2_REPORT.md#s2-38l-exact-long64-maxps-unmasked-imdm-profile) adds exact MAXPS IM/DM with a 2048-case ISA matrix and 32 fixtures. These are bounded local interpreter-only profiles. ISA/runner/testkit checks and independent reviews passed as recorded in the linked slice reports; no physical claim | The normative [S2-37 SSE/SSE2 arithmetic, comparison and conversion inventory](../plans/STAGE_2.md#s2-37-required-legacy-ssesse2-arithmetic-comparison-and-conversion-inventory) now lists 62 required mnemonics and exact legacy encodings, including six MMX/XMM conversion forms with x87/MMX state effects. S2-37bi found zero decoded sites across the exact built-in kernel ELF code sections; runtime observation remains `not_run`, and modules/initramfs/userspace remain unscanned. S2-33 MMX/x87 shared-state acceptance, S2-34 feature/control/memory/alignment integration and S2-38 general MXCSR/x87 exception/fault ordering remain open. S2-38j/k/l do not close parent acceptance; DAZ/FTZ, other exception classes and general memory/fault behavior are not established by these register-only IM/DM profiles | Independent literal/bit-classification validators are CPU-only evidence. [S2-40b](#s2-40b--ограниченный-sse-only-native-xorpd-oracle) admits only exact XORPD; other FP/SIMD profiles are not thereby native-enabled. Prepare safe required oracle cases locally; run them only in the final batch | S2-28–38, S2-40, S2-42; S2-41/Stage 4 where memory/event/system integration is required. S2-37bi completed the built-in kernel static mapping without reducing the target-required inventory. Next: close S2-34/35 feature/control/memory gaps; physical acceptance stays in the combined final Mac mini substage; parents open |
| FXSAVE/FXRSTOR context image and kernel FP initialization/save/restore | Exact config lists FPU/FXSR/XMM/XMM2; committed source/image report maps `FNINIT`, MXCSR-mask `FXSAVE`, `FXSAVE64` and `FXRSTOR64`; S2-40c records the alternatives and the current artifact-reproduction gap. These are static/compile claims, not runtime observation | S2-39a is normative evidence; b–g add bounded selector representation, exact save/restore and gates; h/i add exact FP-init save/restore compositions; S2-38e consumes restored state through MINSS. S2-39k adds an independent Linux CPL3 harness; the 2026-11-04 run passed A/B/A/B (4/4), with raw `execution_kind=unknown`. Local literal-image, callback-failure/no-CPU-commit and composition checks are documented in [S2-39](../plans/STAGE_2.md#s2-39-сохранение-и-восстановление-fpsimd). Parent S2-39 remains open | The synthetic target mask is specified, not physically validated. Required remaining x87 environment/legacy formats, modes, alignment/reserved-bit checks, mixed x87/MMX/XMM lossless round-trip and invalid-image faults remain open. Do not add XSAVE/OSXSAVE/AVX: the accepted target excludes them | Literal images are independent of the production executor, not physical ISA evidence. S2-40b ptrace transport does not establish guest FXSAVE mask/layout/selectors. [S2-39j](../testing/S2_39J_FXSAVE64_ORACLE.md) specifies the successful CPL3 cases; [S2-39k](S2_39K_LINUX_ACCEPTANCE.md) records Linux A/B/A/B, not established physical-Mac acceptance. Raw host MXCSR_MASK is not synthetic target-mask validation; privileged context paths need S2-41 | S2-39; S2-40 for extension necessity; S2-41/Stage 4 for system integration; S2-42 corpus. Next: close the exact artifact reproduction gap in [S2-40c](#s2-40c--static-fpu-saverestore-alternatives-under-neutral-x86-64-v1); parent open |
| Conditional kernel paths / extensions beyond v1 (XSAVE/AVX, LA57, FRED, CET, etc.) | Config/source содержит условные сборки и alternatives; `CONFIG_*=y` и compiled bytes не доказывают требование CPUID bit или исполнение при выбранной policy | Для части соседних точных форм могут быть локальные interpreter slices; это не основание объявлять весь extension group закрытым | Полный source-to-binary reachability/branch audit, selected CPUID policy, per-group semantics and save/restore coverage. Пока ни необходимость сверх v1 не доказана, ни глобальное отсутствие необходимости не установлено | CPU-only suite по exact required forms; user-mode native hardware comparison только для доступных CPL3 cases; system paths остаются privileged/system validation | S2-40, S2-41, S2-42; расширения получают отдельные задачи после evidence |
| Linux boot protocol, privileged startup, paging, events and devices | Linux v7.0 protocol and exact image/header/source are defined; the selected kernel package, extracted `vmlinuz`/config and manifests are staged and statically examined. Guest-memory loading/boot execution, initramfs, command line and runtime logs remain absent | CPU instruction slices exist, but a verified guest boot path/board backend is not implemented | Boot handoff, paging/mode transitions, GDT/IDT/event delivery, memory/device/interrupt manifest and measurable success milestone | Privileged/system validation requires a suitable independent method; full machine integration and guest boot belong to Stage 4. A root Linux process is not a CPL0/real-mode oracle | S2-41 for oracle separation; Stage 4 for machine integration; S2-42 for harness |

### Оставшиеся gates S2-00 и общей приёмки

- **S2-00/C:** target уже выбран. Следующий шаг — versioned exact-form/branch
  manifest, required modes, MSR allowlist и entry control/segment/table/FP
  snapshot; неизвестное остаётся `unresolved`. [ISSUE-027](../KNOWN_ISSUES.md#issue-027--first-efer-access-and-startup-control-snapshot-are-unproven)
  не закрыта read-only S2-25e: EFER WRMSR/mode effects требуют owning contract
  и Stage 4 translation integration. Real mode S2-23 условен по выбранному пути;
  его исключение требует основания, а не возраста Mac mini.
- **S2-34/35/37/38/39:** использовать существующие exact-form inventories;
  наличие локального opcode не закрывает feature-wide state/gates/exceptions,
  memory/fault или save/restore acceptance. Для selected bits не объявлять
  capability до обязательной реализации и проверки. Текущий C/D inventory не
  позволяет объявить все remaining forms реализованными или ненужными.
- **S2-41:** [план](../plans/STAGE_2.md#s2-41-независимо-проверить-privilegedmode-сценарии)
  требует independent privileged/mode evidence и реального Stage 4 backend
  для обязательных paging/GDT/IDT/stack/events scenarios. Mock callbacks/root
  Linux process этого не заменяют. До этого возможна приёмка ограниченного
  A/B/D среза, но не всего Stage 2 с непроверенными обязательными C requirements.
- **S2-42/43:** S2-40b exact XORPD transport, [S2-42a corpus](../plans/STAGE_2.md#s2-42a-deterministic-corpus-для-native-xorpd)
  (1024 distinct cases) и [S2-42b reducer](../plans/STAGE_2.md#s2-42b-bounded-xmm-input-reducer-для-native-xorpd)
  реализованы локально; остальные profile-aware corpus/reducer/oracle и full
  diagnostic replay/state/ABI/JIT-boundary coverage остаются open/unresolved.
  S2-42 требует handwritten boundaries и минимум 1000 distinct valid generated
  cases на поддержанную группу по нескольким seeds, по два повтора
  interpreter/native; опасные/запрещённые combinations получают отдельно
  обоснованный bounded набор. Все новые Mac mini исполнения — только в общем
  финальном substage; обязательные skip/Unsupported/timeout не success.
- **S2-44:** итоговый [acceptance gate](../plans/STAGE_2.md#s2-44-оформить-приёмку)
  зависит от всех обязательных строк принятого профиля и не заменяет S2-41.
  Сначала оставшаяся локальная реализация и пригодный harness/integration,
  затем один общий hardware substage, затем report с revision/config/seeds,
  actual counts и replay evidence, обновление contracts/KNOWN_ISSUES по факту.
  Linux/Windows boot, JIT speed, SMP, AMD coverage и release signing не следуют
  из этой CPU-приёмки. Это обновление не закрывает ни один parent gate.

### Уровни проверки и отложенный hardware batch

- **CPU-only:** локальные interpreter/fixture checks устанавливают поведение
  ограниченного кода на заданном состоянии и backend; сами по себе они не
  являются независимым physical CPU oracle.
- **Linux user-mode physical oracle:** будущий native comparison безопасных
  CPL3 cases на Mac mini. Это не проверяет privileged startup или full-system
  boot и ещё не запускалось.
- **Privileged/system validation:** отдельные mode/privilege сценарии с
  независимым наблюдаемым результатом; Linux user-mode harness/root process не
  заменяет требуемые привилегии.
- **Stage 4 machine integration:** реальный machine backend, guest handoff,
  paging/event/device integration и Linux diagnostic boot. Mock callbacks не
  закрывают эти требования.

В системе пока нет реального machine backend, поэтому S2-41 не готов к
приёмке. Все Mac mini проверки выполняются одним общим hardware substage;
до него ни одна из строк ниже не запускалась:

| Mac mini hardware check | Статус | Граница |
|---|---|---|
| Снять текущие host CPU/OS/toolchain capabilities и зафиксировать revision перед прогоном | `not_run` | Инвентаризация стенда; не выбирает CPUID target автоматически |
| Native comparison доступных обязательных x86-64-v1 и SSE-capable CPL3 cases против независимых literal expected results | `not_run` | Узкий S2-40b transport/profile реализован и compile-checked; physical execution и остальные обязательные cases отложены до общего hardware substage |
| Снять raw host MXCSR_MASK и проверить определённые FXSAVE bytes; отдельно исследовать target mask, pointer layout и selectors | `not_run` на физическом Mac; Linux x86_64 S2-39k A/B/A/B прошёл 4/4 наблюдения с raw `execution_kind=unknown` | S2-39j задаёт протокол; [S2-39k acceptance](S2_39K_LINUX_ACCEPTANCE.md) фиксирует Linux harness/run. Это не physical-Mac result и не доказывает synthetic target equality. Pointer/selector cases и hardware batch остаются открыты; exact image/source/debug artifacts для повторной проверки mapping отсутствуют в текущем checkout — см. [ISSUE-036](../KNOWN_ISSUES.md#issue-036--ubuntu-fpu-source-image-mapping-artifacts-are-unavailable) |
| Независимые privileged/mode-transition/event checks | `not_run` | Не подменять root user-mode запуском; способ проверки должен быть определён в S2-41 |

Ubuntu guest boot и system-level serial success milestone: `not_run`; это
отдельная Stage 4 machine-integration проверка, а не Mac mini physical-oracle
check. Она требует реального machine backend, замороженных initramfs/command
line и board manifest.

Узкий user-mode SSE-only native-oracle contract и Linux x86_64 ptrace
transport определены в отдельном [S2-40b](#s2-40b--ограниченный-sse-only-native-xorpd-oracle).
Этот срез фиксирует MXCSR input `0x1f80`, но не определяет target MXCSR mask
для guest FXSAVE, 32-bit FXSAVE pointer layout, selector differences или полный
Linux profile. Простого включения существующих x87/SIMD fixtures для native
сравнения недостаточно.
Результат этого S2-40a среза не закрывает S2-26, S2-39, S2-40, S2-41 или
S2-42.

## Что остаётся для C/D

1. Реализовать и независимо проверить выбранный reference target
   `od-x86-64-v1-neutral-v0.1`: заданные CPUID leaves/subleaves и feature mask,
   widths и точное поведение уже перечисленных MSR probes. Дополнить полный
   MSR allowlist и начальные CR/EFER/segment/table/FP state; определить runtime
   ветви alternatives. Наличие capability в config не доказывает их исполнение.
2. Заморозить узкий diagnostic PID1 scenario: exact kernel, собственный
   воспроизводимый initramfs, command line, memory map, device/interrupt model
   и serial success marker. Не выдавать его за установленную Ubuntu 26.04.1.
3. Проверить direct64 handoff, paging-level path и нужные mode transitions
   для выбранного initial state. `IA32_EMULATION=y` не доказывает необходимость
   compat mode в этом сценарии и не позволяет объявить его необязательным
   для полного Ubuntu userspace.
4. Зафиксировать каждое обязательное расширение и независимую проверку,
   включая то, что отсутствует у Mac mini. Наличие лишь SSE2 у тестового
   oracle или лишь целочисленного диагностического A не закрывает C/D.

Воспроизведение статической части: получить три exact URL из таблицы,
сверить размеры/SHA-256, разобрать ar и `control.tar.*`/`data.tar.*`,
прочитать указанные config/image members, пересчитать их SHA-256 и
прочитать поля header по v7.0 protocol offsets. Никакой kernel boot для
этой проверки не требуется. Ссылки и структуру проверяет
`python3 tools/check_layout.py`; CPU tests этот отчёт не подтверждают.

## Дополнительная проверка Ubuntu source delta — 27 октября 2026

Для выбранного `linux` source version `7.0.0-38.38` дополнительно проверен
опубликованный Ubuntu source diff. Страница
[packages.ubuntu.com: linux-source-7.0.0](https://packages.ubuntu.com/resolute/linux-source-7.0.0)
ссылается на архивный
[linux_7.0.0-38.38.diff.gz](https://archive.ubuntu.com/ubuntu/pool/main/l/linux/linux_7.0.0-38.38.diff.gz).
Полученный diff имеет 5769 path entries и совпадает с опубликованным MD5
`130cb6bcf60f97cbe11c2a8b3fb0dadc` и SHA-256
`98e913d4a1c1f810e4dcfa573cbcf0f1a3a4427bdcd503722555298a794c52b2`.
В diff 98 x86 paths: 96 под `arch/x86/` и два в `tools/arch/x86/` либо
`tools/perf/arch/x86/`. Delta затрагивает, в частности,
`arch/x86/entry/entry_64.S`, CPU feature checks и FPU signal paths. Она не
меняет `arch/x86/boot/compressed/head_64.S`,
`arch/x86/boot/compressed/pgtable_64.c`, `arch/x86/kernel/head_64.S` или
`arch/x86/boot/header.S`.

Первоначальный подсчёт x86 paths был ошибочным: он пропустил пути,
начинающиеся с `arch/x86/`. Этот корректный перечень сужает вопрос об
изменениях именно четырёх ранних boot files, но показывает, что более поздние
entry, CPU и FPU paths имеют Ubuntu deltas. Он не связывает каждый собранный
byte с исходником и не доказывает выполнение конкретного kernel image.
Последующее сопоставление точного ELF и `startup_64` приведено в разделе
«Exact source-to-binary mapping» ниже. Поэтому локальные S2-22k fixtures
`MOV DS/ES, AX` не называются исполняемым startup prefix и не доказывают
фактический вход в Ubuntu artifact. Linux boot, machine handoff и C/D
inventory остаются открытыми.

## C/D reachability audit — 27 октября 2026

Этот аудит сопоставляет upstream v7.0 source paths с опубликованной Ubuntu
source delta; это уровень **source reachability**, а не disassembly точного
пакета или наблюдавшийся boot. Exact Ubuntu `.dsc` и delta доступны по
[linux 7.0.0-38.38 `.dsc`](https://archive.ubuntu.com/ubuntu/pool/main/l/linux/linux_7.0.0-38.38.dsc)
и [diff.gz](https://archive.ubuntu.com/ubuntu/pool/main/l/linux/linux_7.0.0-38.38.diff.gz).
Правильно разобранная delta содержит 98 x86 paths, включая
`arch/x86/entry/entry_64.S`, CPU и FPU paths; ранние boot files,
перечисленные выше, не меняются. Ubuntu delta дополнительно добавляет
Intel `IA32_PLATFORM_ID` probe в отдельных model/hypervisor conditions,
AMD-specific MSR writes и saferet alternatives; upstream-only ISA inventory
недостаточен. Binary/source identity и выполнение этих ветвей не проверены.

**C — системное состояние и переходы.** При условии direct64 handoff,
Long64, одного машинно описанного vCPU и согласованного 4-level paging
возможна узкая политика `CPUID.LA57=0`, `CR4.LA57=0` и `no5lvl`; тогда
upstream `configure_5level_paging` не требует 32-bit trampoline. Но это
условная policy, не установленное состояние точного пакета. Kernel path
достигает сегментных/descriptor операций (null DS/ES/SS/FS/GS, GDT/IDT/LTR,
LGDT/LIDT и CS reload), CR0/CR4/CR2/CR3, CPUID и model-dependent MSR,
debug registers, SYSCALL/IRET user↔kernel переходов и обработки #PF/#GP.
Путь также задаёт timer/interrupt requirements; «один vCPU» сам по себе не
исключает IRQ/APIC. Эти области отражены upstream в
[`head_64.S`](https://raw.githubusercontent.com/torvalds/linux/v7.0/arch/x86/boot/compressed/head_64.S),
[`kernel/head_64.S`](https://raw.githubusercontent.com/torvalds/linux/v7.0/arch/x86/kernel/head_64.S),
[`cpu/common.c`](https://raw.githubusercontent.com/torvalds/linux/v7.0/arch/x86/kernel/cpu/common.c),
[`entry_64.S`](https://raw.githubusercontent.com/torvalds/linux/v7.0/arch/x86/entry/entry_64.S)
и [`verify_cpu.S`](https://raw.githubusercontent.com/torvalds/linux/v7.0/arch/x86/kernel/verify_cpu.S).

**D — FPU/SIMD state.** По upstream path обязательны FPU initialization:
`FNINIT`, настройка CR0/CR4 FPU bits и `FXSAVE` для чтения MXCSR mask; при
XSAVE выключенном используется legacy `FXSAVE64`/`FXRSTOR64` для save/restore
перед userspace. Значит одного текущего x87 environment snapshot
недостаточно: требуются XMM/MXCSR state и точный FXSAVE layout/semantics.
XSAVE/AVX paths условны по CPUID/state; их нельзя выводить только из
`CONFIG_*`. RAID6/SSE2 modules могут исполнять SSE2 при загрузке, а
accelerated crypto branches зависят от CPU features; пока initramfs/module
manifest не закреплён, их нельзя объявить обязательными или исключить.
Обязательность общего MMX, отдельных x87 arithmetic/conversion/transcendental
forms и произвольных SSE2 instructions source path не доказал. Ссылки:
[`fpu/init.c`](https://raw.githubusercontent.com/torvalds/linux/v7.0/arch/x86/kernel/fpu/init.c),
[`fpu/core.c`](https://raw.githubusercontent.com/torvalds/linux/v7.0/arch/x86/kernel/fpu/core.c),
[`fpu/legacy.h`](https://raw.githubusercontent.com/torvalds/linux/v7.0/arch/x86/kernel/fpu/legacy.h),
[`fpu/xstate.c`](https://raw.githubusercontent.com/torvalds/linux/v7.0/arch/x86/kernel/fpu/xstate.c),
[`lib/raid6/algos.c`](https://raw.githubusercontent.com/torvalds/linux/v7.0/lib/raid6/algos.c)
и [`lib/raid6/sse2.c`](https://raw.githubusercontent.com/torvalds/linux/v7.0/lib/raid6/sse2.c).

**Открытые части профиля.** Target `od-x86-64-v1-neutral-v0.1` уже закрепляет
project CPU vendor/family/model/stepping, maximum CPUID leaves и feature mask;
он отключает hypervisor-present bit. До реализации следующего C/D blocker
нужно определить начальные CR/EFER/segment/table state;
mitigation policy; exact kernel/initramfs/ELF64 init/module hashes;
command line, page/memory map, UART, timer/interrupt controller и root path;
критерий успеха (PID1/serial marker). `S2-00` принят для диагностического A,
x86-64-v1 ABI/ISA floor и выбора synthetic target; реализация/проверка target
остаются открытыми. Profile C/D, S2-22/25/26/27, S2-23 necessity и source-to-
binary/observed-execution evidence остаются открытыми. Физический x86/Mac
mini acceptance будет единым финальным подэтапом, не заменой этому manifest.

## Exact package staging — 28 октября 2026

Три ранее исследованных пакета скачаны с точных HTTPS URL из
`archive.ubuntu.com` в игнорируемый каталог
`work/vms/s2-00-ubuntu-26.04.1/`. Для каждого совпали HTTP `Content-Length`,
измеренный размер и SHA-256 из таблицы выше; URL, `Last-Modified`, размер и
измеренный digest сохранены в локальном `download-manifest.json`. Пакеты не
добавлены в Git. Из signed image извлечён `boot/vmlinuz-7.0.0-38-generic`
(17,320,328 bytes, SHA-256
`9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e`); из
buildinfo повторно извлечён config (308,501 bytes, SHA-256
`6cb18717c6914d4947489eedf8cb69ac23db3faa8ef9b9f563ff02c346dcd32d`), а также
сохранены compiler и ABI metadata для последующего аудита.

Это подтверждает точное повторное получение и целостность скачанных bytes
относительно ранее зафиксированных digest. Подпись текущего `InRelease` pocket
`resolute-updates` проверена ключом с полным fingerprint
`F6ECB3762474EDA9D21B7022871920D1991BC93C`, опубликованным в [Ubuntu security
documentation](https://documentation.ubuntu.com/security/software-integrity/image-verification/).
Использованный экспортированный keyring хранится в том же ignored каталоге;
его размер 1,167 bytes, SHA-256
`5ebbeeb474034b1fa7e50abbe6f136e177fc826219e01f314b3623f7b3097e96`.
SHA-256 записи `main/binary-amd64/Packages.xz` совпал с digest индекса, затем
обе записи пакетов в индексе совпали с `.deb` bytes. `InRelease` имеет размер
137,401 bytes и SHA-256
`ebbcd81b1c46264144baa563be8d74971303ef8008ef291b8a01ec44f786f829`; индекс
`Packages.xz` — 792,980 bytes, SHA-256
`7684c1b9068894c293be6c8e911377bd9f36b47566e07480c8468ec02aa24a18`.
Также найдена историческая signed snapshot цепочка на
`https://snapshot.ubuntu.com/ubuntu/20261001T200000Z/dists/resolute-updates/`:
подпись `InRelease` прошла тем же fingerprint, а SHA-256/размеры `Packages.xz`
и `Sources.xz` совпали с подписанными entries. В `Packages.xz` найдены точные
image и metapackage 7.0.0-38.38 с совпадающими `.deb` hashes; `Sources.xz`
aутентифицирует source `linux` 7.0.0-38.38 и файлы `.dsc`, `.orig.tar.gz`,
`.diff.gz`. Все три source files скачаны; размеры и SHA-256 совпали с
подписанным source index. Ubuntu delta содержит 5,769 paths и меняет 98 x86
paths; среди затронутых startup/CPU paths — `arch/x86/entry/entry_64.S` и
`arch/x86/kernel/fpu/signal.c`. Source files для startup_64, CPUID/common.c,
alternative.c и FPU init/legacy ниже не изменены Ubuntu delta. Текущие индексы
уже обновились: base `resolute` содержит
metapackage 7.0.0-14.14, а `resolute-updates` — 7.0.0-42.42. Все локальные
index bytes, package records и source hashes записаны в `apt-provenance.json`.
PE/Authenticode signature по-прежнему не проверена. Локальные архивы и manifest находятся в игнорируемом `work/`; долговечная
фиксация результата — этот отчёт. Извлечение `vmlinuz` ещё не является
observed execution. Initramfs, command line, initial CPU/CPUID/MSR/state,
device/topology/memory manifest, success milestone и полный ISA/exception
matrix остаются открытыми; S2-00 по-прежнему принят только для диагностического
A и x86-64-v1 ABI/ISA floor, а synthetic CPU target выбран, но ещё не
реализован или проверен.

## Exact source-to-binary mapping — 28 октября 2026

Этот раздел уточняет прежний source-delta аудит выше: он связывает выбранные
startup bytes с ELF точного Ubuntu kernel package и его аутентифицированным
source package. Это всё ещё статическая проверка, не наблюдение выполнения.

Snapshot metadata in local `apt-provenance.json` authenticates source package
`linux` 7.0.0-38.38 and its three files. On-disk SHA-256 checks matched the
signed source index: `linux_7.0.0.orig.tar.gz` (254,937,830 bytes,
`c6343795c8c7e4feff55f5b309365c8e0eaf49cd6abcb412e32315313f149536`),
`linux_7.0.0-38.38.diff.gz` (3,455,370 bytes,
`98e913d4a1c1f810e4dcfa573cbcf0f1a3a4427bdcd503722555298a794c52b2`) and
`.dsc` (7,845 bytes,
`016d43577bd8a61629a02e140e3b88072b5ccdd1c90cb1ce2f3c8e5c28756e7e`).
The Ubuntu delta has 5,769 paths, including 98 x86 paths. It changes
`arch/x86/entry/entry_64.S` and `arch/x86/kernel/fpu/signal.c`; it does not
change the source files listed below for startup, CPUID, CPU alternatives or
FPU initialization/save/restore.

The signed image package contains `vmlinuz-7.0.0-38-generic` (17,320,328
bytes, SHA-256
`9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e`). Its
setup header gives a protected-mode base at file offset `0x5000`; direct64
entry is at `0x5200`. Those bytes decode as `CLD; CLI; XOR EAX,EAX;
MOV DS/ES/SS/FS/GS,AX`, matching `.org 0x200` and lines 278–299 of
`arch/x86/boot/compressed/head_64.S`. This ties the prefix to exact source and
image bytes, but does not prove a bootloader executed it.

The exact zstd payload starts at file offset `0x52cc`; after excluding its
four-byte uncompressed-size trailer it decompresses to a 71,005,544-byte ELF
with SHA-256
`ccaf590465e4e3bc6117efa8ac7cfe87ba0b348188e2608731961527dd05034f` and GNU
build ID `338e542e384c46228f7a378babc0f9bb2b6a5997`. A matching Ubuntu
debuginfod ELF was downloaded (506,419,160 bytes, SHA-256
`7e460f048c3de0f8ee85fd326f2f65d95a6ee448e57b6f5333e2be79b5be9d1f`); its
build ID matches and all 38 allocated file-backed sections are byte-identical
to the ELF extracted from the signed image package. Exact disassembly and
DWARF locations include:

| ELF virtual address | Binary evidence | Authenticated source location |
|---|---|---|
| `ffffffff83f253e0` | `startup_64`, 100 bytes; writes `MSR_GS_BASE` (`0xc0000101`), installs GDT/IDT and executes `LRETQ`; then calls `__pi_sme_enable`, `verify_cpu`, `__pi___startup_64`, `sev_verify_cbit`, and loads CR3 | `arch/x86/kernel/head_64.S:59–138` |
| `ffffffff813b3931` | `DB E3` — `FNINIT` | `arch/x86/kernel/fpu/init.c:44` |
| `ffffffff83f2b46e` | `0F AE 05 6B F9 17 00` — `FXSAVE` | `arch/x86/kernel/fpu/init.c:111` |
| `ffffffff813b48ae` | `48 0F AE 40 40` — `FXSAVE64` | `arch/x86/kernel/fpu/legacy.h:108` |
| `ffffffff813b4a59` | `48 0F AE 4B 40` — `FXRSTOR64` | `arch/x86/kernel/fpu/legacy.h:69` |
| `ffffffff81408f32` | `0F A2` — `CPUID` | `arch/x86/include/asm/cpuid/api.h:29` |
| `ffffffff813c0e90` | `get_cpu_cap` | `arch/x86/kernel/cpu/common.c:1017` |
| `ffffffff813adfd0` | `apply_alternatives` | `arch/x86/kernel/alternative.c:682` |

### IA32 EFER on the mapped main-kernel direct64 continuation

The authenticated `common_startup_64` bytes at ELF file offset `0x59c1a6`
map to `arch/x86/kernel/head_64.S:379–401`. At `ffffffff8139c1b4`, exact bytes
`0F 32` execute `RDMSR` after ECX is loaded with `0xc0000080` and the CPUID
extended-feature word is retained in EDI. The code preserves the input EFER
low dword, sets SCE (bit 0), tests `CPUID.80000001:EDX[20]`, and, when NX is
available, sets NXE (bit 11) and `early_pmd_flags.NX`. At
`ffffffff8139c1d5`, `0F 30` conditionally executes `WRMSR` with EDX zero only
if the low EFER dword changed. The selected synthetic project CPUID target
advertises NX, so its modeled feature response takes the NX branch; the write
still depends on the initial SCE/NXE bits. The earlier compressed 32-bit
entry's EFER LME read/write is bypassed by the selected direct64 prefix, but
`__pi_snp_init` and earlier compressed/decompression helpers have not received
an exhaustive transitive access-order audit. Therefore this is a mapped
main-kernel success-path access, **not a proven absolute first EFER access**.
No runtime execution or entry EFER snapshot was observed.

This site does not justify storage-only `WRMSR` semantics. In IA-32e Long64,
`LMA` is processor status derived from `CR0.PG` and `LME`; changing paging or
mode requires effects owned outside this CPU-only slice. The adjacent compiled
setup writes `CR0=0x80050033` at `ffffffff8139c1dc`, including AM, while the
bounded S2-25a FP-init profile uses synthetic base `0x80010033`. Neither value
is evidence of the actual unobserved entry snapshot. Intel SDM Rev. 093 Vol. 3A
defines the EFER fields and paging relationship; Vol. 2B defines RDMSR/WRMSR
privilege and fault behavior ([official SDM index](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html)).
See [S2-25d](STAGE_2_REPORT.md#s2-25d--main-kernel-ia32-efer-evidence-and-read-only-candidate) and
[ISSUE-027](../KNOWN_ISSUES.md#issue-027--first-efer-access-and-startup-control-snapshot-are-unproven).

### S2-25e local read-only IA32_EFER RDMSR

The local interpreter admits only bare Long64 `0F 32` at ECX
`0xc0000080` under an explicit value from `0x500`, `0x501`, `0xd00`, or
`0xd01`. Current CR0 and CR4 must be supplied from the existing S2-25a/S2-25b
domains, retaining PG=1 and PAE=1 for fixed LMA=1. CPL0 returns the selected
synthetic value in EDX:EAX; CPL1–3 gets local #GP(0) after complete fetch.
Strict fixtures constrain ECX and validate the exact #GP vector and error code.
This does not identify Ubuntu's runtime EFER or entry controls and does not
model WRMSR, NX enforcement, page-table/TLB effects, guest exception delivery,
or physical execution. JIT/native/C/fragment execution remains denied. See
the [S2-25e CPU contract](../specs/CPU_CONTRACT.md#s2-25e--exact-read-only-long64-rdmsr-ia32_efer),
[implementation report](STAGE_2_REPORT.md#s2-25e--exact-read-only-long64-rdmsr-ia32_efer),
[plan](../plans/STAGE_2.md#s2-25e--exact-read-only-long64-rdmsr-ia32_efer), and
[ISSUE-027](../KNOWN_ISSUES.md#issue-027--first-efer-access-and-startup-control-snapshot-are-unproven).

The configured direct64 flow has a compressed entry at `vmlinuz` offset
`0x5200`, then transfers from compressed-kernel setup/decompression to the
uncompressed main-kernel entry `startup_64`. Its exact binary call to
`verify_cpu` is at `ffffffff83f2540e`. Do not confuse it with the separate
`verify_cpu` call in
the compressed 32-bit startup path (`arch/x86/boot/compressed/head_64.S:133`),
which direct64 bypasses. The selected config has
`CONFIG_AMD_MEM_ENCRYPT=y`; the compiled `__pi_sme_enable` call is therefore
also on the main path, with activation behavior still dependent on boot
parameters and CPUID. The later `sev_verify_cbit` call is compiled into this
path as well.

This makes CPU identity and MSR behavior material to the exact startup
contract. `verify_cpu` performs generic CPUID/feature checks and vendor-specific
branches: Intel family/model combinations may read or update
`IA32_MISC_ENABLE`. The source contains an AMD recovery branch that may access
`MSR_K7_HWCR` when SSE/SSE2 are absent, but the earlier required-feature check
rejects CPUID leaf 1 without SSE and SSE2 (mask `0x07008161` at
`ffffffff8139c289`). Under a stable CPUID response, that AMD fallback is
unreachable on the successful startup path and is not a required MSR for it.
The authenticated source also contains later vendor-dependent MSR
initialization paths, including `MSR_CSTAR` on non-Intel CPUs; these source
paths are not all mapped to binary addresses in this audit, and their presence
does not show that each executes on a boot. The x86-64-v1 ABI/ISA floor does
not select those branches or establish their MSR behavior. The versioned
reference CPU contract now fixes project vendor identity, family/model/stepping
and the CPUID target. The profile remains incomplete until that target is
implemented and checked, and the complete relevant MSRs and entry state are
specified. The target's exact SYSENTER MSR probe policy is documented in the
CPU contract; static source evidence does not demonstrate execution of the
kernel's safe-MSR fixup. This static reachability finding does not claim that
every later initialization path executes on a successful boot.

The mapped `fpu__init_cpu_generic` path clears CR0.EM/TS before `FNINIT` and
sets CR4 mask `0x600` (OSFXSR/OSXMMEXCPT). Save and restore initially branch
through `.altinstr_aux`: bit mask `0x04` at `boot_cpu_data+0x43` selects
XSAVE/XRSTOR versus FXSAVE64/FXRSTOR64. A separate test at
`boot_cpu_data+0x88`, mask `0x40`, selects an FPU workaround path. These are
compiled alternatives; runtime selection still requires a frozen CPU and
policy manifest. Generic amd64 x86-64-v1 alone does not select the branch.

The source tarballs, full debug ELF, package artifacts, key and signed indexes
remain ignored under `work/vms/s2-00-ubuntu-26.04.1/`; no large artifacts are
in Git. The debug ELF came from Ubuntu debuginfod and was cross-checked against
all allocated sections of the signed image-derived ELF; the `.ddeb` package
itself was not separately authenticated by a signed ddebs index. PE/Authenticode
and runtime execution remain unchecked.

## S2-27a — exact Ubuntu LGDT candidate

The exact instruction form selected from the Ubuntu 7.0.0-38.38 kernel image
for this narrow slice is `0F 01 55 F6`, decoded as `LGDT m16&64 [RBP-10]`.
The static source-to-binary mapping is:

- After the `WRMSR` in `arch/x86/kernel/head_64.S:72–74`, control calls
  `__pi_startup_64_setup_gdt_idt`.
- `arch/x86/boot/startup/gdt_idt.c:49–60` builds the 10-byte pseudo-descriptor
  and calls `native_load_gdt`.
- In `vmlinux-7.0.0-38-generic.debug`, the mapped symbol begins at
  `ffffffff83f132e0`; bytes at `ffffffff83f132f9` are `0f 01 55 f6`.
- These source files are unchanged by Ubuntu delta `linux` 7.0.0-38.38.

This is static package/disassembly evidence only. It does not establish that
the instruction or surrounding path executes at runtime, and it is not a Linux
boot claim. The wider package and ELF provenance is described in the preceding
[exact source-to-binary mapping](#exact-source-to-binary-mapping--28-октября-2026).
The local implementation contract admits only bare Long64
`0F 01 55 ib` when `state.gdtr` is present, reads a 10-byte pseudo-descriptor,
and excludes `LIDT` and other operand forms. Local implementation checks are
recorded in the [Stage 2 report](STAGE_2_REPORT.md#s2-27a--exact-long64-lgdt-rbp-10).
Independent Sol medium review completed clean after the GDTR field-comment
clarification; no findings remain. Mac mini checks are deferred to the common
final substage.

Semantic references: Intel SDM Vol. 2A, `LGDT/LIDT` instruction entry
(Order No. 253666-093US, September 2026,
[official Intel PDF](https://cdrdv2.intel.com/v1/dl/getContent/671199));
Vol. 3A §§2.4.1, 3.5.2, 4.5.1 and 4.5.3 (Order No. 253668-093US, September
2026, [official Intel PDF](https://cdrdv2.intel.com/v1/dl/getContent/671190)).
See the [CPU contract](../specs/CPU_CONTRACT.md#s2-27a--exact-long64-lgdt-rbp-10),
[plan](../plans/STAGE_2.md#s2-27a--exact-long64-lgdt-rbp-10), and
[stage report](STAGE_2_REPORT.md#s2-27a--exact-long64-lgdt-rbp-10).

## S2-27b — exact Ubuntu DS load candidate

The Ubuntu 7.0.0-38.38 startup source contains a non-null GDT selector load
matching bare Long64 `MOV DS, AX` (`8E D8`). The static source-to-binary
mapping is:

- `arch/x86/boot/startup/gdt_idt.c:62–65` loads selector `0x18` into EAX and
  executes `mov ds, ax`.
- In `vmlinux-7.0.0-38-generic.debug`, the corresponding
  `__pi_startup_64_setup_gdt_idt` symbol begins at `ffffffff83f132e0`; the
  instruction bytes at `ffffffff83f13302` are `8e d8`.
- The corresponding descriptor bytes at `ffffffff8440f018` are
  `ff ff 00 00 00 93 cf 00` (base 0, effective limit `0xffffffff`, access
  byte `0x93` for a present DPL0 writable data descriptor with A=1, flags
  nibble `0xc`).
- These source files are unchanged by Ubuntu delta `linux` 7.0.0-38.38.

This is static package/disassembly evidence only. It does not establish that
the instruction or descriptor access occurs at runtime, and it is not a Linux
boot claim. The bounded local contract and its admission/exception limits are
described in the [CPU contract](../specs/CPU_CONTRACT.md#s2-27b--exact-long64-mov-ds-ax)
and [Stage 2 plan](../plans/STAGE_2.md#s2-27b--exact-long64-mov-ds-ax).
Local implementation checks and clean independent Sol medium review are
recorded in the [Stage 2 report](STAGE_2_REPORT.md#s2-27b--exact-long64-mov-ds-ax).
Mac mini/SSH checks remain deferred; no such checks or Linux boot are claimed.

Semantic reference: Intel SDM Vol. 2B, `MOV` entry (Order No. 253667-093US,
September 2026, [official Intel PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf));
Vol. 3A descriptor/privilege context (Order No. 253668-093US, September 2026,
[official Intel PDF](https://cdrdv2.intel.com/v1/dl/getContent/671190)). See the
[Stage 2 report](STAGE_2_REPORT.md#s2-27b--exact-long64-mov-ds-ax).

## S2-27c — exact Ubuntu ES load candidate

The Ubuntu 7.0.0-38.38 startup source includes an isolated instruction matching
bare Long64 `MOV ES, AX` (`8E C0`). Static source-to-binary evidence:

- `arch/x86/boot/startup/gdt_idt.c:62–65` contains the selector setup and
  segment-register loads; those source files are unchanged by Ubuntu delta
  `linux` 7.0.0-38.38.
- In `vmlinux-7.0.0-38-generic.debug`,
  `__pi_startup_64_setup_gdt_idt` begins at `ffffffff83f132e0`; the isolated
  `8e c0` bytes are at `ffffffff83f13306`.
- The selected GDT descriptor at `ffffffff8440f018` is
  `ff ff 00 00 00 93 cf 00` (base 0, effective limit `0xffffffff`, present
  DPL0 writable data descriptor, access byte `0x93`, flags nibble `0xc`).
- The preceding `MOV SS` candidate at `ffffffff83f13304` is explicitly outside
  this slice, remains unsupported, and was not executed. The ES evidence does
  not assert execution of a contiguous startup prefix.

This is static package/disassembly evidence only. It does not establish runtime
execution of the ES instruction, descriptor access, or a Linux boot. The local
contract is bounded to the exact bare ES load. Author-run implementation checks
are recorded in the [Stage 2 report](STAGE_2_REPORT.md#s2-27c--exact-long64-mov-es-ax).
Independent Sol medium code review completed clean after the Linux-target
conversion fix; no findings remain. This static mapping still establishes no
runtime execution or boot result.
Mac mini/SSH and physical x86 checks are not claimed.

Semantic reference: Intel SDM Vol. 2B, `MOV` entry (Order No. 253667-093US,
September 2026, [official Intel PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf));
Vol. 3A descriptor/privilege context (Order No. 253668-093US, September 2026,
[official Intel PDF](https://cdrdv2.intel.com/v1/dl/getContent/671190)). See the
[CPU contract](../specs/CPU_CONTRACT.md#s2-27c--exact-long64-mov-es-ax) and
[Stage 2 plan](../plans/STAGE_2.md#s2-27c--exact-long64-mov-es-ax).

## S2-27d — exact Ubuntu LIDT candidate

Static source-to-binary evidence from Ubuntu 7.0.0-38.38 identifies an isolated
Long64 `LIDT [RSP+6]` candidate:

- `arch/x86/boot/startup/gdt_idt.c:27–43` calls `native_load_idt`.
- In the matching ELF, `__pi_startup_64_load_idt` is at
  `ffffffff8133ed4e`; bytes there are `0f 01 5c 24 06`, decoded as
  `LIDT m16&64 [RSP+6]` (ModR/M `5c`, SIB `24`, disp8 `06`).
- The 10-byte pseudo-descriptor represents limit `0x01ff` and base
  `ffffffff83a04000`.
- `MOV SS` was skipped. This isolated instruction does not establish execution
  of a contiguous startup prefix.

This is static package/disassembly evidence only; it does not establish LIDT
runtime execution, IDT access, event delivery, Linux boot, or any physical
behavior. The local bounded contract requires `state.idtr == Some(_)` only as
a fail-closed profile gate. Local test and compile results are recorded in the
[Stage 2 report](STAGE_2_REPORT.md#s2-27d--exact-long64-lidt-rsp-6); independent
Sol medium code review completed clean after the LOCK-prefix wording
correction; no findings remain. No native
runtime, physical x86, Mac mini, or SSH checks are claimed.

Semantic references: Intel SDM revision 093, Vol. 2A, `LGDT/LIDT` entry, and
Vol. 3A IDTR/interrupt-descriptor-table sections; see the official sources
already cited in the [CPU contract](../specs/CPU_CONTRACT.md#s2-27d--exact-long64-lidt-rsp-6).
The [Stage 2 plan](../plans/STAGE_2.md#s2-27d--exact-long64-lidt-rsp-6)
records the bounded test matrix.

The proposed [S2-41b protocol](../testing/S2_41B_LIDT_ORACLE.md) prepares four
independent privileged cases for the exact bytes using synthetic inputs. It
requires SIDT before restoration and validated observation gates/stacks under
both old and new IDTs. Source review and harness implementation remain open;
all physical cases and integration are `not_run`. It adds no runtime evidence
to this static Ubuntu mapping and does not close S2-41.

## S2-39a — FXSAVE evidence-only

Static exact-image evidence above identifies three forms in Ubuntu
7.0.0-38.38: `0F AE 05 6B F9 17 00` is a plain Long64 `FXSAVE` with RIP+disp32
addressing in the MXCSR-mask initialization path; `48 0F AE 40 40` is
`FXSAVE64 [RAX+0x40]`; and `48 0F AE 4B 40` is `FXRSTOR64 [RBX+0x40]` in the
legacy save/restore path. These are disassembly/source-reachability facts, not
observed guest execution. The complete addresses and source locations are in
the exact package mapping table above.

Intel SDM Rev. 093 (September 2026), Vol. 2A pp. 3-409–3-419, Tables 3-45–3-49,
and Vol. 1 §§10.5.1–10.5.3, 11.6.5–11.6.6 define the 512-byte,
16-byte-aligned area. The two
Long64 formats have identical x87 environment/data, MXCSR, XMM0–15, and
reserved/unused ranges, but differ at bytes 8–23: REX.W=0 stores 32-bit FIP/FDP
and (for the selected CPUID leaf-7 zero result) FCS/FDS selectors; REX.W=1
stores full 64-bit FIP/FDP without selectors. The abridged FTW encodes empty
versus nonempty in physical-register order, while its eight raw 80-bit stack
values are laid out ST0–ST7 in stack order; TOP rotates those identities.
FXSAVE writes the defined architectural state fields, not every byte in the
area: it preserves software-available bytes 464–511, while bytes 416–463 are
reserved and their write behavior is not asserted here. The complete field
map and definedness policy are in the [CPU contract](../specs/CPU_CONTRACT.md#s2-39a--legacy-fxsavefxrstor-contract-evidence-only).

Intel defines the FXSR CPUID gate, CR0.EM/TS #NM gates, CR4.OSFXSR dependency
for deterministic SSE state, 16-byte alignment, and the 32-bit-versus-64-bit
pointer layouts. Intel specifies a fallback value when a saved MXCSR_MASK is
zero. The local selected target uses immutable `TARGET_FXSAVE_MXCSR_MASK`
policy `0x0000ffbf`; S2-39e ignores the incoming image field. This is not
testkit's diagnostic mask metadata, despite the current equal numeric value. A 512-byte
instruction operand can fault on address/alignment/page conditions; neither
this static evidence nor the cited instruction summary supports claiming a
whole-area atomic commit or arbitrary memory-backend rollback.

S2-39a closes only the normative evidence/contract subtask. S2-39b resolved
selector state representation without making the S2-34.1 snapshot an FXSAVE
memory image. The exact RIP-relative FXSAVE mask-probe was selected and is
implemented under S2-39c, including defined-field writes and preservation of
bytes 464–511. See the [S2-39b implementation report](STAGE_2_REPORT.md#s2-39b--selector-state-representation-only)
and [S2-39c local implementation report](STAGE_2_REPORT.md#s2-39c--plain-long64-rip-relative-fxsave).
No runtime boot, native oracle execution, C/D acceptance, or physical/Mac mini
result is claimed; physical checks remain in the combined final hardware
substage.


## S2-39b — selector state representation only

The missing FCS/FDS representation has been added as independent optional
16-bit values in core and strict schema-v2 snapshots; the older profiles omit
and explicitly reject these values. This is a lossless state-model update, not
an FXSAVE/FXRSTOR implementation or observation. The bounded fixtures and local verification
are recorded in the [Stage 2 report](STAGE_2_REPORT.md#s2-39b--selector-state-representation-only)
and [S2-39b plan](../plans/STAGE_2.md#s2-39b-lossless-selector-state-representation-no-fxsave-execution).

## S2-39c — plain Long64 RIP-relative FXSAVE

The observed Ubuntu bytes `0F AE 05 6B F9 17 00` now have a local
`s2-long-fxsave-rip-v1` interpreter profile. It admits only unprefixed
Long64 `0F AE 05 disp32`; the statically mapped `48 0F AE 40 40` FXSAVE64 and
`48 0F AE 4B 40` FXRSTOR64 remain outside the profile, as do other addressing
forms and neighboring selectors. This implements the image needed by the
statically identified MXCSR-mask probe but does not constitute observed
execution of that Linux path.

The implementation preflights its local aligned canonical 512-byte range and
issues exactly 13 ascending write-only callbacks for defined image slices.
Bytes 464–511 are preserved as Intel specifies; preservation of reserved
bytes is an explicit local policy. The immutable selected-target mask is
`0x0000_ffbf`; this is a synthetic Generic amd64 capability policy and is not
derived from testkit diagnostic metadata. The local fixture and direct tests
exercise another disp32, state serialization, sentinel preservation and the
callback-failure matrix. No #NM/#UD/#GP delivery, whole-area atomicity, MMIO,
shared-memory, DMA, SMP, hardware, boot, or physical result is asserted.

At the S2-39c reporting point, S2-39 remained open for FXRSTOR, other FXSAVE
forms, broader state validation, and round-trip requirements. The exact local
S2-39e restore is recorded below; broader and physical acceptance remain open. Details are in the [S2-39c plan](../plans/STAGE_2.md#s2-39c-exact-plain-long64-rip-relative-fxsave),
[CPU contract](../specs/CPU_CONTRACT.md#s2-39c--plain-long64-rip-relative-fxsave),
[Stage 2 report](STAGE_2_REPORT.md#s2-39c--plain-long64-rip-relative-fxsave),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-39c-plain-long64-rip-relative-fxsave).

## S2-39d — exact Long64 FXSAVE64 `[RAX+0x40]`

The statically selected Ubuntu legacy save encoding `48 0F AE 40 40` now has a
separate local `s2-long-fxsave64-rax-disp8-v1` interpreter profile. It accepts
only that five-byte instruction and only the 512-byte area at aligned,
canonical `RAX+0x40`; other REX/prefix, base, displacement, address, and restore
forms remain unsupported. It requires x87/SSE snapshots while allowing FCS
and FDS independently absent or present. The image writes full 64-bit FIP and
FDP at `[8,16)` and `[16,24)`, respectively.

The serializer shares all other fields and uses the same 13 ascending
write-only callbacks as S2-39c, with pointer slices `[6,16)` and `[16,24)`.
The handwritten literal fixture uses absent selectors and high-width pointers;
direct tests cover every TOP position, selector-independent execution and
per-callback failure without CPU-state retirement on error. JIT/native remain
closed. No hardware, boot, exception-delivery, FXRSTOR, or round-trip result is
claimed. Physical validation remains in the combined final Mac mini substage.
See the [S2-39d implementation report](STAGE_2_REPORT.md#s2-39d--exact-long64-fxsave64-rax0x40),
[CPU contract](../specs/CPU_CONTRACT.md#s2-39d--exact-long64-fxsave64-rax0x40),
[plan](../plans/STAGE_2.md#s2-39d-exact-long64-fxsave64-rax0x40), and
[fixture note](../../tests/fixtures/x86/stage2/README.md#s2-39d-long64-fxsave64-rax0x40).

## S2-39e — exact FXRSTOR64 evidence-only mapping plus local profile

The statically mapped Ubuntu bytes `48 0F AE 4B 40` correspond to the local
interpreter-only `s2-long-fxrstor64-rbx-disp8-v1` profile. That profile admits
only this exact form and reads a complete aligned canonical 512-byte image at
wrapping `RBX+0x40`. Its 13 callbacks read defined field slices only. The
image's `MXCSR_MASK` is ignored; loaded MXCSR is checked against the immutable
target capability mask `0x0000ffbf`. The profile's local tests do not establish
that the Ubuntu code executed, any exception delivery, Linux boot, physical
x86 behavior, or Mac mini acceptance. Those machine checks remain in the
combined final hardware substage. Details are in the [CPU contract](../specs/CPU_CONTRACT.md#s2-39e--exact-long64-fxrstor64-rbx0x40),
[plan](../plans/STAGE_2.md#s2-39e-exact-long64-fxrstor64-rbx0x40),
[report](STAGE_2_REPORT.md#s2-39e--exact-long64-fxrstor64-rbx0x40), and
[fixture note](../../tests/fixtures/x86/stage2/README.md#s2-39e-long64-fxrstor64-rbx0x40).

## S2-39h — local FP-init plus FXSAVE64 composition

The new local-only `s2-long-cr0-cr4-fp-init-fxsave64-v1` fixture composes the
already bounded S2-25c initialization bytes with the statically mapped final
`48 0F AE 40 40` form. This is a synthetic exact fixture and local interpreter
result; it does not show that Ubuntu executed the 33-byte composition or that
these bytes were observed at runtime. The sequence sets CR4.OSFXSR before the
save, so its initial OSFXSR-clear cases do not claim FXSAVE behavior with that
bit clear. The test image uses the existing synthetic target MXCSR mask and
independent literal fields; it is not a hardware image or an Ubuntu capture.

All 16 initial CR0.EM/TS × CR4.OSFXSR/OSXMMEXCPT combinations, nine retirements,
defined image fields, tail preservation, partial stop/budget/truncated-fetch
progress, all 13 callback failures, schema/result tampering, and JIT/native
denials are covered locally. The profile does not extend the Linux source
inventory, CPUID policy, or FXSAVE layout support. Hardware and physical
validation remain `not_run`. See the [S2-39h CPU contract](../specs/CPU_CONTRACT.md#s2-39h--exact-fp-init-plus-fxsave64-composition),
[plan](../plans/STAGE_2.md#s2-39h-exact-fp-initialization-composition-followed-by-fxsave64),
[report](STAGE_2_REPORT.md#s2-39h--exact-fp-init-plus-fxsave64-composition), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-39h-fp-init-plus-fxsave64-composition).

## S2-38e — local FP-init, FXRSTOR64 and MINSS composition

The `s2-long-cr0-cr4-fp-init-fxrstor64-minss-v1` profile is a synthetic
37-byte composition: the exact 33-byte S2-39i FP-init plus FXRSTOR64 prefix,
followed by `F3 0F 5D C1` (`MINSS XMM0,XMM1`). Local interpreter fixtures
retire ten instructions on completion and nine before #XM on an unmasked
invalid or denormal exception. Four independent literal images cover paired
cases differing only in image IM or DM, with MXCSR loaded by FXRSTOR64
controlling the existing S2-38d policy. Tests also cover sticky status,
restored XMM state, live CR0/CR4 gates, fixture/result rejection and JIT/native
denial. This is synthetic local evidence only: the composition was not
observed in the Ubuntu image or in a running guest; Ubuntu boot and physical
x86 validation remain `not_run`. See the [CPU contract](../specs/CPU_CONTRACT.md#s2-38e--exact-fp-init-fxrstor64-minss-composition),
[plan](../plans/STAGE_2.md#s2-38e-exact-fp-init-fxrstor64-and-image-controlled-minss-composition),
[implementation report](STAGE_2_REPORT.md#s2-38e--exact-fp-init-fxrstor64-and-image-controlled-minss-composition), and
[fixture note](../../tests/fixtures/x86/stage2/README.md#s2-38e-fp-init-fxrstor64-and-minss).

## S2-40b — ограниченный SSE-only native XORPD oracle

Выполнен программный срез с точным профилем
`s2-long-native-xorpd-xmm0-xmm1-v1`: только `66 0F 57 C1`, XMM0/XMM1,
один instruction retirement и stop RIP `entry+4`. Требуются все XMM0–15 и
MXCSR `0x1f80`; x87 state отсутствует и запрещён schema. Linux x86_64 runner
использует `PTRACE_GETFPREGS`/`PTRACE_SETFPREGS` как отдельный OS transport:
устанавливает только XMM/MXCSR, проверяет initial readback и снимает финальный
SSE результат отдельно от integer state. Это не guest FXSAVE serialization и
не использует подмену host `MXCSR_MASK`.

Два handwritten fixture с asymmetric inputs содержат independent literal
expected bytes в interpreter tests. Portable codec, strict admission, exact
initial MXCSR, missing SSE/x87-present rejection и preserved-state checks
покрыты тестами. Linux x86_64 код прошёл compile-only check с текущего Mac;
физического запуска не было. В hardware matrix строка остаётся `not_run` до
финального общего Mac mini substage. Старые x87/SSE и FXSAVE профили остаются
native-denied, CPUID unchanged; JIT и C adapter denied. S2-26, S2-39, S2-40,
S2-41 и S2-42 остаются открытыми.

Срез уточняет предшествовавший архитектурный блокер S2-40a для ограниченного
user-mode transport, но не определяет target MXCSR mask для guest FXSAVE,
32-bit layouts, selector behavior, privileged CPU contract или полный Linux
profile. См. [S2-40b plan](../plans/STAGE_2.md#s2-40b-узкий-sse-only-native-xorpd-oracle),
[CPU contract](../specs/CPU_CONTRACT.md#s2-40b--bounded-sse-only-native-xorpd-oracle),
и [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-40b-native-xorpd-xmm0-xmm1).

## S2-40c — static FPU save/restore alternatives under neutral x86-64-v1

This documentation slice inventories only the FPU save/restore routes already
described in the S2-00 image mapping. It does not add ISA features, CPUID
admissions, or runtime claims. The selected synthetic target
`od-x86-64-v1-neutral-v0.1` has CPUID.1:ECX zero, including XSAVE and OSXSAVE;
its contract therefore keeps CR4.OSXSAVE clear and excludes XSAVE/XRSTOR,
XGETBV/XSETBV, and AVX. Within that target policy, the legacy FXSAVE route is
the candidate route. The compiled alternatives and workaround below do not
prove what ran in a Linux boot.

| Recorded route | Previously mapped predicate/effect | Evidence boundary |
|---|---|---|
| FPU initialization | `fpu__init_cpu_generic` clears CR0.EM/TS before `FNINIT` and sets CR4 mask `0x600` (OSFXSR/OSXMMEXCPT); the MXCSR-mask probe is an unprefixed Long64 `FXSAVE` | The committed report records exact image address/bytes for `FNINIT` and the probe, but those bytes were not re-read in this checkout |
| Context save/restore alternative | A test of mask `0x04` at `boot_cpu_data+0x43` is recorded as selecting XSAVE/XRSTOR versus FXSAVE64/FXRSTOR64 | The predicate and replacement mapping are prior committed findings, not independently re-decoded here; no boot-time selection is observed |
| FPU workaround alternative | A separate test of mask `0x40` at `boot_cpu_data+0x88` is recorded as selecting an FPU workaround path | The current report does not establish the workaround's exact operation or target-policy result; do not infer either |

The committed mapping records package SHA-256
`f268d635efaa2e19a18e12813ffdbc2e32db2390cf67d4416abd59292975e7aa`, extracted
`vmlinuz` SHA-256
`9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e`,
image-derived ELF SHA-256
`ccaf590465e4e3bc6117efa8ac7cfe87ba0b348188e2608731961527dd05034f`, build ID
`338e542e384c46228f7a378babc0f9bb2b6a5997`, and matching debug-ELF SHA-256
`7e460f048c3de0f8ee85fd326f2f65d95a6ee448e57b6f5333e2be79b5be9d1f`. A
read-only check in the main checkout reverified the package, `vmlinuz`, ELF and
debug-ELF digests; the exact command and output are recorded in
[S2-40d](S2_40D_ARTIFACT_DIGEST_RECHECK.md). This confirms those four binary
artifact identities. The source archives, signed indexes and `apt-provenance.json`
were absent, so signature/source provenance, build-ID and 38 allocated-section
equality, exact bytes, DWARF line mappings and alternative predicates were not
reproduced. See [ISSUE-036](../KNOWN_ISSUES.md#issue-036--ubuntu-fpu-source-image-mapping-artifacts-are-unavailable).

No source/image/debug artifact was created or modified by this slice. Runtime
alternative selection, Linux boot, initramfs/module/userspace coverage, and
physical x86 behavior remain unobserved. This documentation-only result leaves
S2-39, S2-40, and the selected target implementation open.

### S2-27g local SWAPGS matrix entry

The S2-27g interpreter profile admits exact bare Long64 `0F 01 F8` only with
FRED disabled and canonical48 base values. It covers the architectural CPL0
exchange and CPL1–3 #GP(0) contract, plus a GS-load result at the exchanged
base and injected fault coverage for prior commit. This matrix entry is local CPU coverage only: it
makes no claim that a selected Ubuntu branch executed SWAPGS, that the profile
covers all Long64 configurations, or that Linux boot/native/physical behavior
was checked. Intel SDM Rev. 093 Vol. 2B identifies the exact operation, and
the Vol. 2B CR4.FRED restriction is excluded by the local assumption. See
the [CPU contract](../specs/CPU_CONTRACT.md#s2-27g--exact-long64-swapgs) and
[Stage 2 plan](../plans/STAGE_2.md#s2-27g--exact-long64-swapgs).

### S2-27h local CLI matrix entry

The interpreter-only `s2-long-cli-v1` fixture profile covers exact bare Long64
`FA` under CR0.PE=1, VM=0, CR4.PVI=0 and no VMX/SMM. Its 32 cases cross IF,
CPL and IOPL, with 20 local successful clears and 12 CPU-side #GP(0) results.
The coverage verifies the local CPL/IOPL eligibility boundary and subsequent
caller-supplied maskable-request API behavior; it does not observe Ubuntu
kernel execution, exception delivery, boot, or native/physical behavior. JIT,
native and C/fragment execution remain denied. The normative source is Intel
SDM Rev. 093 Vol. 2A, CLI pp. 3-148–3-149, Table 3-7. See the [CPU contract](../specs/CPU_CONTRACT.md#s2-27h--exact-long64-cli) and [Stage 2 plan](../plans/STAGE_2.md#s2-27h--exact-long64-cli).

### S2-26g local `IA32_KERNEL_GS_BASE`/SWAPGS matrix entry

The local interpreter profile admits only the exact MSR, SWAPGS, and GS-load
programs documented in the CPU contract. It verifies CPL0 access, CPL1–3
#GP(0), canonical48 checks, the low-half EDX:EAX contract, and the base exchange
observed by a following RDMSR or GS load. This is an isolated CPU interpreter
check, not evidence that an Ubuntu kernel branch executed these sequences or
that Linux boot/runtime behavior was observed. JIT/native/C execution is
denied; physical and SSH execution remain deferred to the final batch. See the
[CPU contract](../specs/CPU_CONTRACT.md#локальный-профиль-kernelgsbasev1-s2-26g)
and [Stage 2 plan](../plans/STAGE_2.md#s2-26g--bounded-kernelgsbasev1-msr-and-swapgs-composition).

### S2-27i local STI matrix entry

The interpreter-only `s2-long-sti-v1` profile covers exact bare Long64 `FB`
with explicit IF, CPL, IOPL and interrupt-shadow state. A successful STI sets
IF and inhibits maskable requests until one following allowed instruction
retires only when entry IF was clear; CPL>IOPL yields CPU-side #GP(0). Local
tests check the 32-case privilege/IF matrix, continuation and fault/boundary
preservation. This is not evidence of Ubuntu STI execution, event delivery,
Linux boot or native/physical behavior. JIT, native and C/fragment execution
remain denied. Source: Intel SDM Rev. 093, Vol. 2B, STI pp. 4-674–4-675,
Table 4-22. See the [CPU contract](../specs/CPU_CONTRACT.md#s2-27i--exact-long64-sti-and-maskable-interrupt-inhibition) and [Stage 2 plan](../plans/STAGE_2.md#s2-27i--exact-long64-sti-and-one-instruction-maskable-interrupt-shadow).

### S2-27j local HLT matrix entry

The interpreter-only `s2-long-hlt-v1` profile admits bare Long64 `F4` under
explicit Running/activity and STI-shadow state. Its sixteen literal cases
cross IF=0/1, CPL0–3 and IOPL=0/3: CPL0 halts and advances once, while CPL1–3
produce CPU-side #GP(0). Three CPL0 compound cases exercise HLT after CLI/STI
and confirm that HLT consumes an active STI shadow. Core tests verify halted
re-entry, non-waking request eligibility, and the existing synchronous
maskable delivery transition back to Running with post-HLT RIP in the frame.
This is local interpreter and API composition coverage, not Ubuntu HLT
execution, asynchronous wake behavior, Linux boot, or native/physical behavior.
JIT, native and C/fragment execution remain denied. Source: Intel SDM Rev. 093,
Vol. 2A, HLT p. 3-439. See the [CPU contract](../specs/CPU_CONTRACT.md#s2-27j--exact-long64-hlt-and-synchronous-maskable-wake)
and [Stage 2 plan](../plans/STAGE_2.md#s2-27j--exact-long64-hlt-and-persistent-activity).

### S2-27k local IRETQ matrix entry

The local interpreter profile admits only `48 CF` at Long64/CPL0 with a
validated same-CPL five-qword frame and exact accessed GDT descriptors. Four
fixtures cross restored IF=0/1 and IOPL=0/3; core composition tests cover the
existing STI/HLT and synchronous delivery APIs plus all 16 prior-RSP low-bit
alignments. This establishes local interpreter/API behavior only. It does not
show that Ubuntu executes this path, establish IRET fault delivery or priority,
or constitute Linux boot/native/physical evidence. JIT/native/C/fragment paths
remain denied. See the [CPU contract](../specs/CPU_CONTRACT.md#s2-27k--exact-long64-same-cpl0-iretq)
and [Stage 2 plan](../plans/STAGE_2.md#s2-27k--exact-long64-same-cpl0-iretq).

### S2-27l LRETQ evidence and local profile

The selected Ubuntu kernel's `arch/x86/kernel/head_64.S:59–138` startup path
is mapped to `startup_64` and records an `LRETQ` mnemonic after GDT/IDT setup.
This confirms source/symbol-level presence only. The exact instruction bytes
were not independently located in the image, so this evidence does not claim
opcode `48 CB` or runtime execution. The bounded local `48 CB` interpreter
profile is specified in the [CPU contract](../specs/CPU_CONTRACT.md#s2-27l--exact-long64-same-cpl0-lretq); Linux runtime, native/physical execution,
fixture runner integration and general far-return semantics remain
unestablished.

### S2-25a local CR0 EM/TS FP-init profile

The local `s2-long-cr0-fp-init-v1` interpreter fixtures exercise only the four
fixed CR0 base/EM/TS combinations and exact MOV-CR0/AND/FNINIT byte sequences.
The 32 handwritten cases cover initial EM/TS states, CPL0–3 read/write
privilege outcomes, FNINIT #NM gating, and write→FNINIT composition. This is
synthetic local interpreter coverage, not observation of Ubuntu instruction
bytes or execution. No Ubuntu/Linux runtime, native, hardware, SMP, MMU or
physical result is claimed. See the [CPU contract](../specs/CPU_CONTRACT.md#s2-25a--bounded-long64-cr0-emts-fp-initialization) and [plan](../plans/STAGE_2.md#s2-25a--bounded-long64-cr0emts-fp-initialization).

## S2-39i — local FP-init plus FXRSTOR64 composition

The local `s2-long-cr0-cr4-fp-init-fxrstor64-v1` profile appends exact
`48 0F AE 4B 40` to the 28-byte S2-25c program. It confirms local staged
FXRSTOR behavior, not Linux runtime execution or a Linux kernel dependency.
The restore image is a separately authored fixture input and remains unchanged;
all 16 starting CR0/CR4 pairs and callback failure/no-commit behavior are local
interpreter checks. This result does not establish feature advertisement, boot
reachability, hardware-image equivalence, physical x86 execution, or combined
fault priority. See the [plan](../plans/STAGE_2.md#s2-39i-exact-fp-initialization-composition-followed-by-fxrstor64),
[CPU contract](../specs/CPU_CONTRACT.md#s2-39i--exact-fp-init-plus-fxrstor64-composition),
[implementation report](STAGE_2_REPORT.md#s2-39i--exact-fp-init-plus-fxrstor64-composition),
and [fixture](../../tests/fixtures/x86/stage2/README.md#s2-39i-fp-init-plus-fxrstor64-composition).

## S2-37bi — Static mapping of S2-37 forms in the Ubuntu kernel ELF

Static mapping used the authenticated `linux-image-7.0.0-38-generic` 7.0.0-38.38 package and its exact built-in kernel ELF. The package SHA-256 is `f268d635efaa2e19a18e12813ffdbc2e32db2390cf67d4416abd59292975e7aa`; the extracted `vmlinuz` SHA-256 is `9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e`; the decompressed ELF SHA-256 is `ccaf590465e4e3bc6117efa8ac7cfe87ba0b348188e2608731961527dd05034f`, build ID `338e542e384c46228f7a378babc0f9bb2b6a5997`. The matching Ubuntu debug ELF SHA-256 is `7e460f048c3de0f8ee85fd326f2f65d95a6ee448e57b6f5333e2be79b5be9d1f` with the same build ID.

`xcrun llvm-objdump` from Apple LLVM 21.0.0 scanned `.text`, `.init.text`, `.altinstr_aux`, `.altinstr_replacement` and `.exit.text` with line tables enabled. All 62 required S2-37 inventory mnemonics and all 68 mandatory-prefix/opcode byte signatures had zero sites in those sections (A–L: 36/36; M–Z: 26/26). The byte-signature pass is independent of disassembler mnemonic aliases. The parser sanity check found `FNINIT` at `ffffffff813b3931`, bytes `DB E3`, function `fpu__init_cpu_generic`, source `arch/x86/kernel/fpu/init.c:44`. With no S2-37 sites, there are no source/DWARF locations or branch contexts to report for those forms.

This is static non-observation in the selected built-in kernel ELF only. It does not establish that any target-required form is unnecessary, absent from loadable modules, initramfs or userspace, or unreachable at runtime. Runtime observation and physical acceptance remain `not_run`. Exact per-mnemonic inventory status and hashes are preserved in the [versioned manifest](data/stage2/s2-37-linux-vmlinux-map-v1.json). The target inventory remains normative; the next bounded local candidate is `CVTPS2PI MM0,XMM1` (`0F 2D C1`), subject to the [Stage 2 plan](../plans/STAGE_2.md#s2-37bi--static-mapping-of-the-s2-37-inventory-in-the-selected-ubuntu-kernel).


## S2-40e — Static inventory of beyond-v1 instruction forms in the Ubuntu kernel ELF

This slice extends the S2-37bi method (same five code sections of the same built-in
kernel ELF, same hashes and build ID as above) from the S2-37 SSE/SSE2 list to every
instruction family outside the selected `od-x86-64-v1-neutral-v0.1` target. The
[versioned manifest](data/stage2/s2-40e-linux-beyond-v1-map-v1.json) records the
exact class-to-mnemonic lists, per-section and per-mnemonic site counts, distinct
function counts and the largest functions per class. `xcrun llvm-objdump` (Apple LLVM
21.0.0) disassembled `.text`, `.init.text`, `.altinstr_aux`, `.altinstr_replacement`
and `.exit.text` of the matching debug ELF (symbols for attribution): 8,885,773
instruction lines, 453 distinct mnemonics (counted on the stripped ELF). Per-class
site counts are identical in the stripped `vmlinux`; the `.text` line total differs
by 6 (8,629,609 versus 8,629,603), cause not investigated.

| Class | Sites | Where | Representative functions |
|---|---|---|---|
| AVX-512 (EVEX-only mnemonics, zmm/k operands) | 381 + 411 | `.text` | `chacha_*_avx512vl`, `poly1305_blocks_avx512`, `crc*_vpclmul_avx512` |
| AVX/AVX2 (VEX, ymm) | 2,148 | `.text` | `sha1_transform_avx2`, `chacha_*_avx2`, `poly1305_blocks_avx2` |
| AVX (VEX, xmm only) | 2,187 | `.text` | `sha512_transform_avx`, `poly1305_blocks_avx`, `sha256_transform_avx` |
| BMI2 (RORX/SHLX/SHRX/SARX/BZHI) | 1,192 | `.text` | `sha*_transform_rorx`, `HUF_*_bmi2`, `FSE_*_bmi2` |
| ANDN/TZCNT/LZCNT | 1,113 | `.text` 1,090, `.init.text` 23 | generic kernel code and ZSTD (TZCNT encoding is the `rep bsf` idiom), `*_bmi2` (LZCNT), `sha1_transform_avx2` (ANDN) |
| PCLMULQDQ/AES-NI/SHA | 351 | `.text` | `sha256_ni_*`, `sha1_ni_transform`, `crc*_pclmul_sse` |
| SSSE3 / SSE4.1 / SSE4.2 CRC32 | 168 / 14 / 27 | `.text` | `sha*_ssse3`, `chacha_*_ssse3`, `crc32c_x86_3way` |
| CMPXCHG16B | 10 | `.text` | `slab_update_freelist`, `update_dte256`, IRTE modify |
| POPCNT | 121 | `.altinstr_replacement` only | alternative replacements |
| XSAVE family, XGETBV/XSETBV | 33 | `.text` 20, `.altinstr_replacement` 11, `.init.text` 2 | `os_xsave`, `os_xrstor_safe`, `fpu__init_cpu_xstate` |
| RDRAND/RDSEED/FSGSBASE/PKU/CLFLUSHOPT/CLWB/WRMSRNS/MONITOR-MWAIT/etc. | 150 | `.text` 93, `.altinstr_replacement` 56, `.init.text` 1 | see manifest |
| VMCALL/VMMCALL/TDX/SGX/SEV-SNP/AMX | 84 | mostly `.text` | hypervisor and confidential-computing paths |
| FRED ERETS/ERETU | 3 | `.text` 2, `.altinstr_replacement` 1 | `asm_fred_*` |
| SMAP CLAC/STAC | 411 | `.altinstr_replacement` only | alternative replacements |

The byte `0xD6` (16 sites) inside the `__x86_indirect_its_thunk_*` padding is not decoded and
is invalid in 64-bit mode; it is not part of the target ISA. No `ENDBR`, `MOVBE`, `ADCX/ADOX`,
`LAHF/SAHF` or `CMPXCHG8B` sites were found.

What this shows: the selected kernel ELF contains code for every extension family above, so
the x86-64-v1 floor is not a statement about the image content. The code that is vector-ISA
specific sits in feature-suffixed functions (`_avx2`, `_ni`, `_bmi2`, `_ssse3`) and in
`.altinstr_replacement`, which is consistent with run-time dispatch, but no CPUID or alternative
predicate, call-site gate or branch context was reproduced; source and signature artifacts are
still unavailable (ISSUE-036). So this is static presence only. It does not establish that any
family is required, unreachable or selected at boot, and it does not change the CPUID policy
(S2-57). The `F3 0F BC` (TZCNT encoding) sites are expected to be `REP BSF` under the target,
which the interpreter already decodes as BSF without BMI1 (S4-38L4g); `F3 0F BD` (LZCNT
encoding) appears only in `*_bmi2` ZSTD/HUF/FSE functions and `REP BSR` is not admitted. Loadable modules,
initramfs and userspace were not scanned; no hardware, oracle or boot result is claimed.
S2-40 stays open.
