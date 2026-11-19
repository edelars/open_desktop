# Поддержанный профиль UEFI

## Начальная карта памяти

После установки собственных CR3 и RSP прошивка преобразует отсортированные
регионы handoff в массив `EFI_MEMORY_DESCRIPTOR` версии 1. Размер записи
на x64 — 40 байт, выравнивание — 8 байт. Поля PhysicalStart, VirtualStart,
NumberOfPages и Attribute начинаются со смещений 8, 16, 24 и 32.
VirtualStart и четырёхбайтовый отступ после Type равны нулю. Начальный MapKey
равен 1. [UEFI 2.11, §7.2.3](https://uefi.org/specs/UEFI/2.11/07_Services_Boot_Services.html#efi-boot-services-getmemorymap).

Проекция типов задана контрактом handoff. Старые таблицы машины становятся
`EfiConventionalMemory` после переключения CR3. Новые таблицы и оба стека
занимают `EfiBootServicesData`, guard занимает одну страницу
`EfiReservedMemoryType`. Код shadow получает `EfiBootServicesCode`, а
`.rodata` и данные — `EfiBootServicesData` с разрезом по границам линкера.
Дыры между регионами не описываются. [FIRMWARE_BOOT, §6.2](FIRMWARE_BOOT.md#62-таблица-регионов-запись-32-байта).

| Область | Attribute |
|---|---|
| RAM, включая занятые области прошивки, handoff и ACPI | `EFI_MEMORY_WB` |
| ROM, MMIO и framebuffer | `EFI_MEMORY_UC` |
| Окно NVRAM | `EFI_MEMORY_UC` и `EFI_MEMORY_RUNTIME` |
| Guard и прочие reserved-области | 0 |

Атрибуты в записи описывают возможности региона. Текущие права R/W и NX
задаются таблицами страниц. Номер экрана из handoff не используется как
EFI-атрибут. Буфер записей находится на новом стеке, который уже объявлен
занятым прошивкой. При нехватке буфера построитель сообщает требуемое
число записей до записи в буфер.

ROM выводит в COM1 MapKey, версию и число записей, затем Type, PhysicalStart,
NumberOfPages и Attribute каждой записи. Публикация указателя Boot Services
`GetMemoryMap` относится к S7-05. [Проверки этапа 7](../reports/STAGE_7_REPORT.md#s7-04--начальная-карта-памяти-uefi).
