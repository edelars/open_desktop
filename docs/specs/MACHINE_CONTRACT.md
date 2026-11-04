# Контракт машины: физическая память, срез S4-02—S4-04

**Статус:** локально принят ограниченный срез; это не контракт полной VM и не
подтверждение MMU, устройств, загрузки Linux или JIT-интеграции.

Нормативная карта платы и будущие boot reservations остаются в
[VIRTUAL_BOARD.md](VIRTUAL_BOARD.md) и в принятом manifest S4-00. Этот документ
фиксирует только уже реализованную границу production-owned guest physical RAM.

## Адреса и карта

- `GuestPhysicalAddress` — отдельный тип гостевого физического byte address; он
  не является host pointer, linear address или port address.
- `GuestPhysicalRange` — непустой полуоткрытый диапазон `[start, start + len)`.
  Внутренняя проверка endpoint выполняется в расширенном 65-битном домене, поэтому
  width 64 корректно представляет адрес `u64::MAX` с длиной 1.
- `MachineMemoryMap::new(width, regions)` сортирует regions, проверяет width
  1—64, checked bounds и запрещает overlap. Карта не меняется после создания.
- `MemoryRegionKind` различает `Ram`, `Rom`, `Reserved` и `Mmio`. Сейчас backing
  создаётся только для RAM; ROM/reserved/MMIO классифицируются и отклоняют доступ
  до появления соответствующих владельцев.
- Port I/O, E820, boot reservations, aliases и remapping пока не реализованы и
  не должны моделироваться перекрывающимися RAM regions.

## RAM ownership и доступ

`PhysicalMemory` — единственный владелец RAM backing в `od-machine`. RAM
создаётся zeroed, с явным `RamLimits::max_total_bytes`, проверяемым до первой
allocation. Наружу не выдаются mutable slices, raw pointers или долгоживущие
handles.

`read_bytes` и `write_bytes` выполняют один checked access целиком. Access,
пересекающий две regions, overflow или hole, отклоняется до изменения RAM или
буфера назначения. Пересечение guest 4 KiB границы внутри одной region
допускается; host page size не участвует в семантике. Scalar helpers используют
little-endian encoding.

Ошибки карты, allocation и physical access являются host-side результатами
этого слоя. Они не превращаются автоматически в guest `#PF`; linear translation
и fault delivery появятся на S4-06—S4-09.

## Committed writes

Все byte/scalar writes используют один `write_bytes` path. Callback получает
ровно один `GuestPhysicalRange` только после полной записи в RAM. Ошибка,
включая отказ region или cross-region access, не вызывает callback. Этот hook
является только локальным уведомлением о committed bytes; он ещё не подключён к
JIT code-dependency invalidation и не доказывает S4-15—S4-16.

## Синхронная MMIO/PIO bus

`MachineBus` переиспользует одну `MachineMemoryMap` и один `PhysicalMemory`;
второй backing или независимая карта физических regions не создаются. MMIO
binding может ссылаться только на полный `MemoryRegionKind::Mmio`, а PIO имеет
отдельную карту `PortRange` в пространстве `0..65536` с checked endpoint.

`BusDevice` получает один `DeviceAccess` с `IoSpace`, offset и `AccessWidth`.
Доступы `Byte/Word/Dword/Qword` маскируют значение до ширины; PIO Qword в этом
срезе запрещён. Шина проверяет width, overflow и полное попадание в одно окно до
вызова устройства. Она не разбивает смешанный или cross-window access и не
повторяет device operation.

Ошибки построения карты, доступа шины и самого устройства разделены. Ошибка
устройства должна быть возвращена до guest-visible изменения или внешнего
эффекта; это проверено тестовым устройством. MMIO/PIO writes не вызывают RAM
committed-write callback. CPU `IN/OUT`, I/O privilege checks, guest exceptions
и преобразование linear address остаются в `od-isa` и CPU bridge.

## Следующие зависимости

Следующий допустимый срез — MMIO/PIO bus и CPU bridge. `GuestMemory` из
`od-isa` принимает linear addresses и остаётся отдельным контрактом; прямое
реализование этого trait на `PhysicalMemory` до page walker запрещено.
