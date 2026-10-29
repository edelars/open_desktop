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

| Требование / сценарий | Source и image evidence | Локальная реализация | Недостающая семантика / граница | Независимый oracle и тип проверки | Владелец покрытия |
|---|---|---|---|---|---|
| Generic `amd64` ABI/ISA floor: x86-64-v1, без выбранного v2/v3/v4 | Публичное Ubuntu baseline и psABI; exact package/config metadata идентифицирует `amd64`. Это не исчерпывающий inventory исполняемых kernel/userspace путей. См. [решение baseline](#решение-по-generic-amd64-baseline) | Synthetic target `od-x86-64-v1-neutral-v0.1` описан в CPU contract; это пока спецификация, не готовая capability | Полный feature policy, extension inventory, userspace/compat requirements и доказательство runtime branch selection | CPU-only literal/architectural oracle может проверять отдельные ISA semantics, но не достаточность профиля ОС; user-mode physical oracle — отдельный Mac mini batch | S2-00, S2-40; общий corpus по S2-42 |
| Baseline integer/control/memory and atomic semantics (including configured NOPL, CX8 and CMOV capabilities) | Exact config lists NOPL/CX8/CMOV/CPUID and other required capabilities; config is a lower bound, not an executed-instruction inventory | Bounded local forms are tracked in [S2-15](../plans/STAGE_2.md#s2-15-остальные-integer-формы-профиля), [S2-16](../plans/STAGE_2.md#s2-16-условия-и-переходы), [S2-17](../plans/STAGE_2.md#s2-17-стек-callret-и-косвенное-управление), [S2-20](../plans/STAGE_2.md#s2-20-точность-исключений-и-exit-state), and [S2-21](../plans/STAGE_2.md#s2-21-однопроцессорные-atomicfence-формы); parent requirements remain open pending broader acceptance | Exact required instruction forms and memory/fault/ordering cases must be enumerated from the selected kernel, initramfs and userspace; bounded slices do not imply full v1 or full Linux coverage | CPU-only literal fixtures for each admitted form; Linux user-mode physical comparison for safe CPL3 cases only; privileged/system cases need another oracle | S2-15–21, S2-40, S2-42; S2-41 for privileged/system cases |
| Early direct64 entry, CPU identity/CPUID, MSR и initial state | Exact Ubuntu 7.0.0-38.38 package/buildinfo, config, source delta и ELF/source mapping подтверждены статически; direct64 prefix и `verify_cpu` reachability известны. Kernel boot не наблюдался. Config `CONFIG_X86_REQUIRED_FEATURE_*` задаёт нижнюю границу, не точный CPUID mask | Ограниченные CPUID/MSR slices есть по S2-26; отдельные setup/descriptor forms покрыты S2-27. Target implementation и системная последовательность не завершены | CPUID leaves/subleaves и masks для target, полный MSR allowlist, CR/EFER/segment/table/FP entry state, mode/exception semantics и выбранные alternatives | CPU-only contract tests для определённых форм; Linux user-mode physical execution не проверяет CPL0 handoff; privileged/system oracle требует пригодного независимого harness/backend | S2-26, S2-27, S2-41; Stage 4 для полной system integration |
| x87/SSE/SSE2 arithmetic, conversions and observable FP/SIMD state | Exact config lists FPU/FXSR/XMM/XMM2; config and source/image evidence identify baseline availability and conditional save/restore routes, not all executed forms | Bounded execution slices are covered across [S2-30–38](../plans/STAGE_2.md#s2-30-основные-x87-операции); forms, state and exception behavior remain exact-profile slices | Complete required arithmetic/conversion inventory, MXCSR/x87 exception semantics and OS-managed context paths are not globally closed by the current bounded instruction set | CPU-only literal fixtures with independent expected bit patterns; native physical oracle can compare only safe user-mode-supported forms after the SSE-capable oracle contract is frozen | S2-30–38, S2-40, S2-42 |
| FXSAVE/FXRSTOR context image and kernel FP initialization/save/restore | Exact config lists FPU/FXSR/XMM/XMM2; source/image mapping confirms `FNINIT`, MXCSR-mask `FXSAVE`, `FXSAVE64` and `FXRSTOR64` forms. This is compile/source evidence, not runtime observation | Bounded S2-39a–e contracts/fixtures cover selected FXSAVE/FXRSTOR forms; JIT/native execution is denied and physical validation is `not_run` | Target MXCSR capability/policy, all required formats/branches, exception delivery and broader OS-managed context behavior remain. The selected synthetic mask does not establish a hardware target | CPU-only literal fixtures are independent of the production executor, but not of physical ISA behavior. Linux user-mode physical oracle may compare safe CPL3 forms with a ready SSE-capable harness; privileged FPU/system paths need separate validation | S2-39; S2-40 for extension necessity; S2-41 system oracle; S2-42 corpus |
| Conditional kernel paths / extensions beyond v1 (XSAVE/AVX, LA57, FRED, CET, etc.) | Config/source содержит условные сборки и alternatives; `CONFIG_*=y` и compiled bytes не доказывают требование CPUID bit или исполнение при выбранной policy | Для части соседних точных форм могут быть локальные interpreter slices; это не основание объявлять весь extension group закрытым | Полный source-to-binary reachability/branch audit, selected CPUID policy, per-group semantics and save/restore coverage. Пока ни необходимость сверх v1 не доказана, ни глобальное отсутствие необходимости не установлено | CPU-only suite по exact required forms; user-mode native hardware comparison только для доступных CPL3 cases; system paths остаются privileged/system validation | S2-40, S2-41, S2-42; расширения получают отдельные задачи после evidence |
| Linux boot protocol, privileged startup, paging, events and devices | Linux v7.0 protocol and exact image/header/source are defined; the selected kernel package, extracted `vmlinuz`/config and manifests are staged and statically examined. Guest-memory loading/boot execution, initramfs, command line and runtime logs remain absent | CPU instruction slices exist, but a verified guest boot path/board backend is not implemented | Boot handoff, paging/mode transitions, GDT/IDT/event delivery, memory/device/interrupt manifest and measurable success milestone | Privileged/system validation requires a suitable independent method; full machine integration and guest boot belong to Stage 4. A root Linux process is not a CPL0/real-mode oracle | S2-41 for oracle separation; Stage 4 for machine integration; S2-42 for harness |

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
| Native comparison доступных обязательных x86-64-v1 и SSE-capable CPL3 cases против независимых literal expected results | `not_run` | Linux user-mode physical oracle; native/backend harness и oracle contract ещё не определены |
| Проверить target MXCSR mask, FXSAVE pointer layout и selector behavior на подходящем SSE-capable oracle | `not_run` | Требует отдельного architecture contract для native oracle; текущие x87/SIMD fixtures сами по себе его не задают |
| Независимые privileged/mode-transition/event checks | `not_run` | Не подменять root user-mode запуском; способ проверки должен быть определён в S2-41 |

Ubuntu guest boot и system-level serial success milestone: `not_run`; это
отдельная Stage 4 machine-integration проверка, а не Mac mini physical-oracle
check. Она требует реального machine backend, замороженных initramfs/command
line и board manifest.

Следующая программная задача после S2-40a должна сначала определить
architecture contract для SSE-capable native oracle, включая target MXCSR
mask, 32/64-bit FXSAVE pointer layout и selector differences. Простого
включения существующих x87/SIMD fixtures для native сравнения недостаточно.
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
