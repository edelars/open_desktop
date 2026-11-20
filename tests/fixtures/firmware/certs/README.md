# Публичные данные для Secure Boot

Получены 2026-11-19 из Microsoft Secure Boot Objects, тег `v1.7.2` (выпуск 2026-11-02). UEFI Forum указывает этот репозиторий как источник `KEK`, `db` и `dbx`. Данные распространяются с исходным текстом лицензии. [Microsoft v1.7.2](https://github.com/microsoft/secureboot_objects/releases/tag/v1.7.2), [UEFI Forum](https://uefi.org/revocationlistfile), [лицензия](LICENSE-Microsoft-Secure-Boot-Objects.txt).

| Файл | Назначение | SHA-256 | Путь в источнике |
|---|---|---|---|
| `windows-production-pca-2011.der` | `db`, Microsoft Windows Production PCA 2011 | `e8e95f0733a55e8bad7be0a1413ee23c51fcea64b3c8fa6a786935fddcc71961` | [Microsoft DER](https://github.com/microsoft/secureboot_objects/blob/v1.7.2/PreSignedObjects/DB/Certificates/MicWinProPCA2011_2011-10-19.der) |
| `windows-uefi-ca-2023.der` | `db`, Windows UEFI CA 2023 | `076f1fea90ac29155ebf77c17682f75f1fdd1be196da302dc8461e350a9ae330` | [Microsoft DER](https://github.com/microsoft/secureboot_objects/blob/v1.7.2/PreSignedObjects/DB/Certificates/windows%20uefi%20ca%202023.der) |
| `microsoft-uefi-ca-2011.der` | `db`, Microsoft Corporation UEFI CA 2011 | `48e99b991f57fc52f76149599bff0a58c47154229b9f8d603ac40d3500248507` | [Microsoft DER](https://github.com/microsoft/secureboot_objects/blob/v1.7.2/PreSignedObjects/DB/Certificates/MicCorUEFCA2011_2011-06-27.der) |
| `microsoft-uefi-ca-2023.der` | `db`, Microsoft UEFI CA 2023 | `f6124e34125bee3fe6d79a574eaa7b91c0e7bd9d929c1a321178efd611dad901` | [Microsoft DER](https://github.com/microsoft/secureboot_objects/blob/v1.7.2/PreSignedObjects/DB/Certificates/microsoft%20uefi%20ca%202023.der) |
| `microsoft-kek-ca-2011.der` | `KEK`, Microsoft Corporation KEK CA 2011 | `a1117f516a32cefcba3f2d1ace10a87972fd6bbe8fe0d0b996e09e65d802a503` | [Microsoft DER](https://github.com/microsoft/secureboot_objects/blob/v1.7.2/PreSignedObjects/KEK/Certificates/MicCorKEKCA2011_2011-06-24.der) |
| `microsoft-kek-ca-2023.der` | `KEK`, Microsoft Corporation KEK 2K CA 2023 | `3cd3f0309edae228767a976dd40d9f4affc4fbd5218f2e8cc3c9dd97e8ac6f9d` | [Microsoft DER](https://github.com/microsoft/secureboot_objects/blob/v1.7.2/PreSignedObjects/KEK/Certificates/microsoft%20corporation%20kek%202k%20ca%202023.der) |
| `dbx-x64-v1.7.2.bin` | `dbx`, исходный x64 `EFI_SIGNATURE_LIST` | `7fbf186e4d045556ed92f449234606943006a105d11d078b9271a794225fd338` | `MicrosoftOnly/Firmware/DefaultDbx.bin` из [архива Microsoft](https://github.com/microsoft/secureboot_objects/releases/download/v1.7.2/edk2-x64-secureboot-binaries.tar.gz) |

SHA-256 архива `96d02ae9e691f0f34f1369cb266235cabdf8a6812ba462f723f961ea2c9719a7` совпал с опубликованным digest. Извлечённый `dbx` содержит 445 SHA-256 записей в 21 388 байтах. Все шесть DER прочитаны OpenSSL 4.1.0 и содержат открытые ключи RSA-2048. [Метаданные выпуска](https://api.github.com/repos/microsoft/secureboot_objects/releases/tags/v1.7.2).

`dbx-update-x64-v1.7.2-signed.efiauth2` — подписанное обновление `dbx_x64.efiauth2` из официального пакета 2023 KEK, получено 2026-11-19. SHA-256 файла `524c796ade42db0048b656f6cf3863832174ae7f58cad6986d1ccae1e3d3ecd1`, SHA-256 архива `e732e4a02ec0e75939093deea8c37486bafb8105d5492c237889ea3bb2a0881d`. Оно служит регрессионным входом для Auth2/PKCS#7; начальное значение `dbx` берётся из `dbx-x64-v1.7.2.bin`. [Подписанный архив Microsoft](https://github.com/microsoft/secureboot_objects/releases/download/v1.7.2-signed/edk2-2023-signed-secureboot-binaries.tar.gz), [метаданные выпуска](https://api.github.com/repos/microsoft/secureboot_objects/releases/tags/v1.7.2-signed).

## Тестовая подпись PE

Создана 2026-11-19 для собственного `hello.efi` из сборки `b938a83`. Независимый Python-код вычислил Authenticode SHA-256 `ca851c2acc753ad7772f5a366faaacadf7399a1ea7364ac4311a5625f487da39` и собрал DER `SpcIndirectDataContent` и атрибуты PKCS#7. OpenSSL 4.1.0 выпустил RSA-2048 CA с `pathlen:0` и конечный сертификат `codeSigning` на 2020-01-01–2021-01-01, подписал DER SET OF атрибутов через `openssl dgst -sha256 -sign` и подтвердил подпись через `openssl dgst -sha256 -verify`. В SignedData входит только конечный сертификат. Эти публичные данные используются в регрессионных тестах и не входят в набор ключей VM по умолчанию.

| Файл | Содержимое | SHA-256 |
|---|---|---|
| `authenticode-test-hello-sha256.p7b` | Authenticode SignedData с подписью и конечным сертификатом | `a543bf2e76ec2742229a6b0ade60fba95a7c7bd793feae1d39cbecb1c1cf1c8e` |
| `authenticode-test-ca.der` | Тестовый CA, устанавливаемый в проверке как доверенный сертификат | `b400a7303aed1ec32fe60b5ec539880354399e77b1fcdf46b55354d00b1d88a5` |
