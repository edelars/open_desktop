# S2-00: статическое исследование Linux kernel candidate

**Дата проверки:** 22 октября 2026 года. **Статус:** проверены метаданные,
config и setup header конкретного пакета; Linux boot и полный профиль C/D
не подтверждены. Исследование не меняет выбранный Ubuntu candidate и не
закрывает обязательные задачи этапа 2.

## Полученные данные и происхождение

Через HTTPS с официального `archive.ubuntu.com` прочитаны три небольших
пакета. Они разобраны в памяти исследовательского процесса стандартными
средствами Python (`urllib.request`, ar headers, `tarfile`, `hashlib`,
`struct`); установка, запуск kernel, изменение host/bootloader, SSH и
запись стороннего kernel code в проект не выполнялись. Пакеты и образ не
сохранены в guest staging и Git. Значения ниже измерены по полученным
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
Подпись APT `InRelease` и PE/Authenticode signature в этой проверке не
верифицировались. Слово signed — свойство пакета и его control metadata,
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

## Что остаётся для C/D

1. Подготовить локальный staging исследованного image/config с проверкой
   приведённых digest и APT provenance; зафиксировать initramfs/command
   line/memory map/device manifest. Содержимое не заменять движущейся
   зависимостью метапакета.
2. Сопоставить Ubuntu source version `7.0.0-38.38`, kernel build flags,
   alternatives и CPU checks с полным ISA/CPUID/MSR/exception/state
   inventory. Наличие capability не заменяет реализацию её семантики.
3. Проверить direct64 handoff, paging-level path и нужные mode transitions
   на точном image. Отдельно определить потребности initramfs/userspace;
   `IA32_EMULATION=y` не доказывает необходимость compat mode в выбранном
   минимальном сценарии и не разрешает заранее удалить его из этапа.
4. Зафиксировать каждое обязательное расширение и независимую проверку,
   включая то, что отсутствует у Mac mini. Наличие лишь SSE2 у тестового
   oracle или лишь целочисленного диагностического A не закрывает C/D.

Воспроизведение статической части: получить три exact URL из таблицы,
сверить размеры/SHA-256, разобрать ar и `control.tar.*`/`data.tar.*`,
прочитать указанные config/image members, пересчитать их SHA-256 и
прочитать поля header по v7.0 protocol offsets. Никакой kernel boot для
этой проверки не требуется. Ссылки и структуру проверяет
`python3 tools/check_layout.py`; CPU tests этот отчёт не подтверждают.
