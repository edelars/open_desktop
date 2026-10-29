# Собственные входные данные

`x86/` — короткие тестовые последовательности и их исходное описание. `control/` — детерминированные сценарии проверки существующего control protocol; его wire format нормативно задан в `protocols/control/`. `guest/` — небольшие собственные данные для гостевых сценариев. Для каждого набора фиксировать источник, способ генерации, ожидаемый контракт и seed при необходимости. Установочные ISO, большие диски и пользовательские материалы сюда не помещать.

The bounded S2-35.7 PSUBW example is documented in the
[Stage 2 fixture index](x86/stage2/README.md#s2-357-local-register-psubw-diagnostic-profile).

The local masked-status MAXSS fixture is documented in the
[Stage 2 fixture index](x86/stage2/README.md#s2-37f-local-masked-status-maxss-register-profile).

The local masked-status MINPS fixture is documented in the
[Stage 2 fixture index](x86/stage2/README.md#s2-37g-local-masked-status-minps-register-profile).
