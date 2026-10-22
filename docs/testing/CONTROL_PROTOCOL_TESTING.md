# Проверка Control Protocol v0

Нормативный wire format и пределы находятся в [`CONTROL_PROTOCOL_V0.md`](../../protocols/control/CONTROL_PROTOCOL_V0.md). Этот документ описывает только воспроизведение smoke-сценариев; он не определяет второй вариант протокола.

## Реальный обработчик

Первый объект — существующий VMHost обработчик `HELLO → READY → RUN → RUN_ACK → STOP/RESULT`. Сценарии запускают настоящий `target/od-vmhost-smoke` через Unix `socketpair` из [`vmhost_control_race_smoke.py`](../../tests/system/vmhost_control_race_smoke.py). Парсер VMHost не копируется в тест. Исходные сценарии и seeds находятся в [`control_v0_scenarios.json`](../../tests/fixtures/control/control_v0_scenarios.json).

Для разбиения кадров используется фиксированный 32-bit LCG `state = (1664525 * state + 1013904223) mod 2^32`; каждый следующий размер chunk находится в диапазоне от 1 до 13 bytes (предел задаётся fixture). Seeds `0`, `1` и `24301` применяются и к HELLO, и к RUN. Это фиксирует последовательность отправки независимо от стандартного PRNG Python.

Smoke покрывает:

- 100 ранних STOP, гонку STOP с естественным RESULT, неполный header/payload и закрытие канала во время работы;
- ошибочный magic/version/kind и payload выше протокольного предела при handshake и обработке STOP;
- STOP с чужим request ID и непустым payload: VMHost возвращает ERROR, остаётся работоспособным и затем принимает корректный STOP;
- конечное завершение, допустимую последовательность STOP_ACK/RESULT и exit status дочернего VMHost.

Запуск после release-сборки использует аргументы:

```sh
python3 tests/system/vmhost_control_race_smoke.py \
  target/open-desktop-launcher-smoke target/od-vmhost-smoke
```

## Подключение проверок будущих устройств

Каждую новую группу сценариев начинать с reset устройства в известное состояние. Операции читать/писать через реальные callback-границы владельца устройства; до callback проверять объявленные диапазоны адресов и максимальный размер операции. Для каждого сценария фиксировать разрешённые состояния и переходы, seed и конечный timeout. Неверный адрес/размер должен возвращать предусмотренную ошибку и не менять состояние. Не вводить тестовую копию production parser или модель устройства, которой ещё нет в реализации.

В текущем этапе PCI, APIC, UART и другие guest-устройства не реализованы и этим smoke не покрываются.
