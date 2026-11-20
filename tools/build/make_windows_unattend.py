#!/usr/bin/env python3
r"""Генератор постоянного ответ-файла автоматической установки Windows 11 (S8-22).

Результат - autounattend.xml без nonce и без ключа продукта. Nonce прогона лежит
на диске полезной нагрузки (OD8\nonce.txt), поэтому носитель с ответ-файлом
собирается один раз на версию шаблона, а не на каждый прогон. Только стандартная
библиотека Python. Генератор детерминирован: одинаковые аргументы и пароль дают
побайтно одинаковый файл. Пароль берётся из файла (по умолчанию work/stage8/secret),
в вывод и в отчёт он не попадает; печатается только SHA-256 готового файла.

Первоисточники Microsoft (все прочитаны 2026-11-20; релевантные страницы сверены
по тексту learn.microsoft.com, номер build Windows 11 не фиксировался):

* Windows Setup Automation Overview, раздел Implicit Answer File Search Order:
  https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/windows-setup-automation-overview
  Для фаз windowsPE и offlineServicing Setup ищет файл Autounattend.xml в каталоге
  \Sources дистрибутива Windows (пункт 6 таблицы); корни дисков проверяются только
  у съёмных носителей (пункты 4 и 5). Setup удаляет чувствительные данные из
  кешированной копии ответ-файла в конце каждой фазы.
* Unattended Windows Setup Reference (по компонентам, база URL
  https://learn.microsoft.com/en-us/windows-hardware/customize/desktop/unattend/):
  microsoft-windows-setup-runsynchronous, -diskconfiguration, -diskconfiguration-disk,
  -imageinstall-osimage, -imageinstall-osimage-installfrom-metadata,
  -imageinstall-osimage-installto, -userdata-accepteula;
  microsoft-windows-international-core-winpe, microsoft-windows-international-core;
  microsoft-windows-shell-setup-autologon, -oobe, -firstlogoncommands,
  -useraccounts-localaccounts-localaccount;
  microsoft-windows-securestartup-filterdriver-preventdeviceencryption.
* Split a Windows image file (.wim) to span across multiple DVDs:
  https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/split-a-windows-image--wim--file-to-span-across-multiple-dvds
  "If you're installing from Windows Setup, copy the set of .swm files into the
  Sources folder on the installation media and then run Windows Setup."
* wpeutil: https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/wpeutil-command-line-options
  (команда "Wpeutil Shutdown").

Принятые решения и записанные факты

1. Язык и раскладка: en-US (InputLocale 0409:00000409; формат значения подтверждён
   страницей InputLocale) в windowsPE (International-Core-WinPE) и oobeSystem
   (International-Core).
2. Образ: ImageInstall/OSImage/InstallFrom/MetaData Key=/IMAGE/INDEX, Value=6
   (индекс Pro по плану S8-22). Для набора SWM в справочнике Unattended Windows
   Setup Reference отдельных параметров нет: страница Path требует имя файла и
   называет install.wim именем по умолчанию, а страница про SWM предписывает
   положить install.swm, install2.swm... в \Sources и запускать Setup. Поэтому
   InstallFrom/Path в шаблон по умолчанию НЕ входит (поддержка Path=install.swm
   источником не подтверждена - источник не найден); аргумент --image-path
   добавляет Path только для проверки на S8-24, если Setup не найдёт набор сам.
3. Разметка: явная GPT, решение этого генератора (по умолчанию Setup не выбираем,
   потому что без DiskConfiguration при WillShowUI=Never Setup завершает установку
   с ошибкой - страница DiskConfiguration): раздел 1 EFI (260 МБ, FAT32, метка
   System), раздел 2 MSR (16 МБ; MSR не модифицируется - страница ModifyPartition),
   раздел 3 Primary (остаток диска, NTFS, метка Windows). Установка в DiskID/раздел 3.
   WillWipeDisk=true: разметка стирает целевой диск, поэтому проверка размера (п. 4)
   обязательна. Нумерация PartitionID (EFI=1, MSR=2, Windows=3) в справочнике
   явно не описана (страница MS про UEFI/GPT-разметку по прямому URL не
   открылась) - наблюдать в setupact.log на S8-24.
4. Проверка размера диска: RunSynchronous (фаза windowsPE) выполняет diskpart
   list disk и ищет строку "Disk <DiskID>  Online  <N> GB"; при несовпадении -
   wpeutil shutdown. Ограничение: diskpart показывает размер в округлённых единицах
   (GB = ГиБ), точный размер в байтах из списка не виден; нумерация diskpart
   принимается равной DiskID Setup (страница DiskID: 0 - первый диск, 1 - второй).
   ПОРЯДОК RunSynchronous И DiskConfiguration ВНУТРИ windowsPE В ДОКУМЕНТАЦИИ НЕ
   ОПИСАН (источник не найден): страница RunSynchronous говорит только о порядке
   внутри RunSynchronous и о том, что они идут раньше RunAsynchronous; схема
   windowsPE на странице windowspe (диаграмма) этот порядок не показывает.
   Поэтому проверку нужно подтвердить наблюдением на S8-24 по setupact.log (время
   записи вызова команды относительно разметки). Если окажется, что разметка
   идёт раньше проверки, защита от стирания не работает - тогда нужен другой
   механизм защиты (решение для архитектора), а не этот шаблон.
5. Пользователь и AutoLogon: локальная учётная запись в группе Administrators
   (UserAccounts/LocalAccounts), AutoLogon Enabled=true, LogonCount=1. Известная
   проблема из справочника (страница LogonCount): Windows прибавляет 1 к значению,
   большему 0, поэтому для ровно одного входа справочник предписывает первой
   командой FirstLogonCommands записать AutoLogonCount=0 (reg add ... /v
   AutoLogonCount /t REG_DWORD /d 0 /f) - так и сделано. Пароль пишется как
   PlainText=true (значение false справочник не позволяет задать напрямую);
   скрывает его только Setup в кешированной копии в конце фазы - проверка на S8-24.
6. OOBE: HideEULAPage, HideOnlineAccountScreens, HideWirelessSetupInOOBE = true,
   ProtectYourPC = 3 (Express settings выключены). Создание пользователя через
   UserAccounts по справочнику (страница AutoLogon) пропускает экран создания
   учётной записи. HideLocalAccountScreen относится только к Windows Server и не
   используется; NetworkLocation в Windows 10+ устарел и не используется.
   Что Windows 11 Pro без интернета достаточно этих параметров - наблюдать на S8-24.
7. FirstLogonCommands: команда 2 находит том с меткой диска полезной нагрузки
   (Get-Volume -FileSystemLabel, до 120 с ожидания) и запускает с него
   <том>:\OD8\firstlogon.ps1 вызовом
   powershell.exe -NoProfile -ExecutionPolicy Bypass -File <путь>.
   about_Execution_Policies: "The execution policy isn't a security boundary, it's
   defense in depth" (https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_execution_policies,
   2026-11-20), поэтому -ExecutionPolicy Bypass здесь не обход проверки безопасности.
   Справочник: команды FirstLogonCommands идут последовательно в заданном порядке.
8. Шифрование устройства: решение S8-05 не записано (задача не выполнена).
   Поэтому по умолчанию шифрование выключено: компонент
   Microsoft-Windows-SecureStartup-FilterDriver, PreventDeviceEncryption=true, фаза
   specialize. ВНИМАНИЕ: страница PreventDeviceEncryption подписана "These settings
   only apply to Windows 8" (пример для Windows 8.1) - действие на Windows 11 по
   источнику НЕ подтверждено (источник не найден), наблюдать на S8-25. Режим
   --device-encryption allow не выпускает параметр вообще. Генератор печатает
   предупреждение. DisableEncryptedDiskProvisioning относится к аппаратным
   eDrive (Opal) и к BitLocker device encryption не относится - не используется.
9. Ключ продукта в шаблон не входит (S8-05 п. 4). Нужен ли он для выбора Pro по
   индексу и для пропуска страницы ключа в Setup - неизвестно, наблюдать на
   S8-24; если нужен - вопрос о доступе владельцу, не вшивать ключ.
10. Не используются обходы LabConfig, bypass-ключи TPM и Secure Boot.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import sys
import tempfile
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_OUTPUT = ROOT / "work/stage8/autounattend.xml"
DEFAULT_PASSWORD_FILE = ROOT / "work/stage8/secret"
WORK_DIR = ROOT / "work"

UNATTEND_NS = "urn:schemas-microsoft-com:unattend"
WCM_NS = "http://schemas.microsoft.com/WMIConfig/2002/State"
XSI_NS = "http://www.w3.org/2001/XMLSchema-instance"
PUBLIC_KEY_TOKEN = "31bf3856ad364e35"

LOCALE = "en-US"
INPUT_LOCALE = "0409:00000409"

# Раздел Windows всегда третий: EFI=1, MSR=2, Windows=3 (см. docstring, п. 3).
WINDOWS_PARTITION_ID = 3
ESP_MB = 260
MSR_MB = 16

FIRSTLOGON_SCRIPT = "firstlogon.ps1"
PAYLOAD_WAIT_SECONDS = 120

DEVICE_ENCRYPTION_NOTE = (
    "решение S8-05 о шифровании устройства не записано: параметр выключен "
    "(PreventDeviceEncryption=true, фаза specialize); источник относит параметр "
    "только к Windows 8.x - действие на Windows 11 не подтверждено, наблюдать на S8-25"
)

ET.register_namespace("", UNATTEND_NS)
ET.register_namespace("wcm", WCM_NS)
ET.register_namespace("xsi", XSI_NS)


class UnattendError(Exception):
    pass


@dataclass(frozen=True)
class Options:
    password: str
    user: str = "od8"
    disk_id: int = 0
    disk_size_gib: int = 64
    image_index: int = 6
    image_path: Optional[str] = None
    payload_label: str = "OD8PAYLOAD"
    payload_dir: str = "OD8"
    device_encryption: str = "prevent"


def _q(tag: str) -> str:
    return f"{{{UNATTEND_NS}}}{tag}"


def _el(parent: Optional[ET.Element], tag: str, text: Optional[str] = None, add: bool = False) -> ET.Element:
    element = ET.Element(_q(tag)) if parent is None else ET.SubElement(parent, _q(tag))
    if text is not None:
        element.text = text
    if add:
        element.set(f"{{{WCM_NS}}}action", "add")
    return element


def _component(settings: ET.Element, name: str) -> ET.Element:
    component = _el(settings, "component")
    component.set("name", name)
    component.set("processorArchitecture", "amd64")
    component.set("publicKeyToken", PUBLIC_KEY_TOKEN)
    component.set("language", "neutral")
    component.set("versionScope", "nonSxS")
    return component


def _intl(component: ET.Element, winpe: bool) -> None:
    if winpe:
        setup_ui = _el(component, "SetupUILanguage")
        _el(setup_ui, "UILanguage", LOCALE)
    _el(component, "InputLocale", INPUT_LOCALE)
    _el(component, "SystemLocale", LOCALE)
    _el(component, "UILanguage", LOCALE)
    _el(component, "UserLocale", LOCALE)


def size_check_command(disk_id: int, disk_size_gib: int) -> str:
    """Команда фазы windowsPE: при несовпадении размера диска - wpeutil shutdown.

    findstr не поддерживает "+" в регулярных выражениях, поэтому пробелы заданы
    как "пробел, пробел*".
    """
    pattern = f"^ *Disk {disk_id}  *Online  *{disk_size_gib} GB "
    return (
        'cmd.exe /s /c "(echo list disk | diskpart) | '
        f'findstr /r /c:"{pattern}" >nul || wpeutil shutdown"'
    )


def payload_launch_command(label: str, directory: str, script: str) -> str:
    """Находит том с меткой label и запускает с него script через powershell.exe -File."""
    finder = (
        "$v=$null;"
        f"for($i=0;$i -lt {PAYLOAD_WAIT_SECONDS // 2} -and -not $v;$i++)"
        f"{{$v=Get-Volume -FileSystemLabel '{label}' -ErrorAction SilentlyContinue"
        "|Where-Object DriveLetter|Select-Object -First 1;"
        "if(-not $v){Start-Sleep -Seconds 2}};"
        "if(-not $v){exit 2};"
        "& powershell.exe -NoProfile -ExecutionPolicy Bypass -File "
        f"('{{0}}:\\{directory}\\{script}' -f $v.DriveLetter);"
        "exit $LASTEXITCODE"
    )
    return f'powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "{finder}"'


def _windows_pe_pass(root: ET.Element, opts: Options) -> None:
    settings = _el(root, "settings")
    settings.set("pass", "windowsPE")

    intl = _component(settings, "Microsoft-Windows-International-Core-WinPE")
    _intl(intl, winpe=True)

    setup = _component(settings, "Microsoft-Windows-Setup")

    run_sync = _el(setup, "RunSynchronous")
    command = _el(run_sync, "RunSynchronousCommand", add=True)
    _el(command, "Order", "1")
    _el(command, "Path", size_check_command(opts.disk_id, opts.disk_size_gib))
    _el(command, "Description", f"Verify disk {opts.disk_id} is {opts.disk_size_gib} GiB, else shut down")

    disk_config = _el(setup, "DiskConfiguration")
    disk = _el(disk_config, "Disk", add=True)
    _el(disk, "DiskID", str(opts.disk_id))
    _el(disk, "WillWipeDisk", "true")
    create = _el(disk, "CreatePartitions")
    for order, (ptype, size) in enumerate((("EFI", ESP_MB), ("MSR", MSR_MB), ("Primary", None)), start=1):
        part = _el(create, "CreatePartition", add=True)
        _el(part, "Order", str(order))
        _el(part, "Type", ptype)
        if size is None:
            _el(part, "Extend", "true")
        else:
            _el(part, "Size", str(size))
    modify = _el(disk, "ModifyPartitions")
    for order, (partition_id, fmt, label) in enumerate(((1, "FAT32", "System"), (WINDOWS_PARTITION_ID, "NTFS", "Windows")), start=1):
        part = _el(modify, "ModifyPartition", add=True)
        _el(part, "Order", str(order))
        _el(part, "PartitionID", str(partition_id))
        _el(part, "Format", fmt)
        _el(part, "Label", label)
    _el(disk_config, "WillShowUI", "OnError")

    image = _el(setup, "ImageInstall")
    os_image = _el(image, "OSImage")
    install_from = _el(os_image, "InstallFrom")
    if opts.image_path is not None:
        _el(install_from, "Path", opts.image_path)
    meta = _el(install_from, "MetaData", add=True)
    _el(meta, "Key", "/IMAGE/INDEX")
    _el(meta, "Value", str(opts.image_index))
    install_to = _el(os_image, "InstallTo")
    _el(install_to, "DiskID", str(opts.disk_id))
    _el(install_to, "PartitionID", str(WINDOWS_PARTITION_ID))
    _el(os_image, "InstallToAvailablePartition", "false")
    _el(os_image, "WillShowUI", "OnError")

    user_data = _el(setup, "UserData")
    _el(user_data, "AcceptEula", "true")


def _specialize_pass(root: ET.Element, opts: Options) -> None:
    if opts.device_encryption != "prevent":
        return
    settings = _el(root, "settings")
    settings.set("pass", "specialize")
    component = _component(settings, "Microsoft-Windows-SecureStartup-FilterDriver")
    _el(component, "PreventDeviceEncryption", "true")


def _oobe_pass(root: ET.Element, opts: Options) -> None:
    settings = _el(root, "settings")
    settings.set("pass", "oobeSystem")

    intl = _component(settings, "Microsoft-Windows-International-Core")
    _intl(intl, winpe=False)

    shell = _component(settings, "Microsoft-Windows-Shell-Setup")

    logon = _el(shell, "AutoLogon")
    password = _el(logon, "Password")
    _el(password, "Value", opts.password)
    _el(password, "PlainText", "true")
    _el(logon, "Enabled", "true")
    _el(logon, "LogonCount", "1")
    _el(logon, "Username", opts.user)

    accounts = _el(shell, "UserAccounts")
    local_accounts = _el(accounts, "LocalAccounts")
    account = _el(local_accounts, "LocalAccount", add=True)
    account_password = _el(account, "Password")
    _el(account_password, "Value", opts.password)
    _el(account_password, "PlainText", "true")
    _el(account, "Name", opts.user)
    _el(account, "Group", "Administrators")
    _el(account, "DisplayName", opts.user)

    oobe = _el(shell, "OOBE")
    _el(oobe, "HideEULAPage", "true")
    _el(oobe, "HideOnlineAccountScreens", "true")
    _el(oobe, "HideWirelessSetupInOOBE", "true")
    _el(oobe, "ProtectYourPC", "3")

    commands = _el(shell, "FirstLogonCommands")
    for order, (line, description) in enumerate(
        (
            (
                'reg add "HKLM\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Winlogon" '
                "/v AutoLogonCount /t REG_DWORD /d 0 /f",
                "Limit AutoLogon to one logon (LogonCount known issue)",
            ),
            (
                payload_launch_command(opts.payload_label, opts.payload_dir, FIRSTLOGON_SCRIPT),
                "Run firstlogon.ps1 from the payload disk",
            ),
        ),
        start=1,
    ):
        command = _el(commands, "SynchronousCommand", add=True)
        _el(command, "Order", str(order))
        _el(command, "CommandLine", line)
        _el(command, "Description", description)


def _is_xml10_text(text: str) -> bool:
    """True, если все символы допустимы в XML 1.0 (production Char); иначе XML не well-formed."""
    for ch in text:
        code = ord(ch)
        if not (code in (0x9, 0xA, 0xD) or 0x20 <= code <= 0xD7FF or 0xE000 <= code <= 0xFFFD or 0x10000 <= code <= 0x10FFFF):
            return False
    return True


def validate_options(opts: Options) -> None:
    if not 0 <= opts.disk_id <= 31:
        raise UnattendError("disk-id must be in 0..31")
    if not 1 <= opts.disk_size_gib <= 1023:
        raise UnattendError("disk-size-gib must be in 1..1023 (diskpart prints larger disks in TB)")
    if not 1 <= opts.image_index <= 99:
        raise UnattendError("image-index must be in 1..99")
    if opts.device_encryption not in ("prevent", "allow"):
        raise UnattendError("device-encryption must be prevent or allow")
    if not opts.password or any(ch in opts.password for ch in "\r\n\0"):
        raise UnattendError("password must be a non-empty single line")
    if not _is_xml10_text(opts.password):
        raise UnattendError(
            "password contains characters not allowed in XML 1.0 (control characters "
            "U+0000..U+001F other than tab, lone surrogates, U+FFFE, U+FFFF)"
        )
    if not opts.user or not all(ch.isalnum() or ch in "_-" for ch in opts.user) or len(opts.user) > 20:
        raise UnattendError("user must be 1..20 characters of letters, digits, _ and -")
    if not opts.payload_label or len(opts.payload_label) > 11 or not all(
        ch.isascii() and (ch.isalnum() or ch == "_") for ch in opts.payload_label
    ):
        raise UnattendError("payload-label must be 1..11 ASCII letters, digits or _ (FAT32 label)")
    if not opts.payload_dir or not all(ch.isascii() and (ch.isalnum() or ch == "_") for ch in opts.payload_dir):
        raise UnattendError("payload-dir must be ASCII letters, digits or _")
    if opts.image_path is not None and (not opts.image_path or '"' in opts.image_path or "\n" in opts.image_path):
        raise UnattendError("image-path must be a non-empty path without quotes")


def build_unattend(opts: Options) -> bytes:
    validate_options(opts)
    root = _el(None, "unattend")
    _windows_pe_pass(root, opts)
    _specialize_pass(root, opts)
    _oobe_pass(root, opts)
    tree = ET.ElementTree(root)
    ET.indent(tree, space="  ")
    return ET.tostring(root, encoding="utf-8", xml_declaration=True) + b"\n"


def read_password(path: Path) -> str:
    try:
        raw = path.read_bytes()
    except OSError as error:
        raise UnattendError(f"cannot read password file {path}: {error.strerror}") from error
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise UnattendError("password file must be UTF-8") from error
    if text.endswith("\r\n"):
        text = text[:-2]
    elif text.endswith("\n"):
        text = text[:-1]
    if not text:
        raise UnattendError("password file is empty")
    return text


def _is_inside(path: Path, directory: Path) -> bool:
    """path лежит в directory (или совпадает с ним).

    Сравнение по строкам регистрозависимо, а APFS по умолчанию регистронезависима:
    .../OPEN_DESKTOP/Work/x указывает в тот же каталог, но relative_to() его не
    узнает. Поэтому предки path сверяются с directory через os.path.samefile
    (st_dev/st_ino); строковое сравнение остаётся запасным для случая, когда
    directory ещё не существует.
    """
    for candidate in (path, *path.parents):
        try:
            if os.path.samefile(candidate, directory):
                return True
        except OSError:
            continue
    try:
        path.relative_to(directory)
    except ValueError:
        return False
    return True


def _check_not_in_git_tree(path: Path, what: str) -> None:
    resolved = path.resolve()
    if not _is_inside(resolved, ROOT):
        return
    if not _is_inside(resolved, WORK_DIR):
        raise UnattendError(f"{what} inside the repository must be under work/ (ignored by Git): {path}")


def write_atomic(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(dir=path.parent, prefix=".autounattend.", suffix=".tmp")
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(data)
        os.replace(tmp_name, path)
    except BaseException:
        try:
            os.unlink(tmp_name)
        except OSError:
            pass
        raise


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate the permanent autounattend.xml for the Stage 8 Windows install (S8-22). "
        "The password is read from a file and never printed; only the SHA-256 of the result is printed."
    )
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT, help="output file (default: work/stage8/autounattend.xml)")
    parser.add_argument("--password-file", type=Path, default=DEFAULT_PASSWORD_FILE, help="file with the local user password (default: work/stage8/secret)")
    parser.add_argument("--user", default="od8", help="local administrator user name (default: od8)")
    parser.add_argument("--disk-id", type=int, default=0, help="Setup DiskID of the target disk, by controller order (default: 0)")
    parser.add_argument("--disk-size-gib", type=int, default=64, help="expected target disk size in GiB (default: 64)")
    parser.add_argument("--image-index", type=int, default=6, help="install image index in the SWM set (default: 6, Pro)")
    parser.add_argument("--image-path", default=None, help="optional InstallFrom/Path (unconfirmed for SWM; for S8-24 diagnostics only)")
    parser.add_argument("--payload-label", default="OD8PAYLOAD", help="volume label of the payload disk (default: OD8PAYLOAD)")
    parser.add_argument("--payload-dir", default="OD8", help="directory with firstlogon.ps1 on the payload disk (default: OD8)")
    parser.add_argument("--device-encryption", choices=("prevent", "allow"), default="prevent", help="prevent: emit PreventDeviceEncryption=true (default until S8-05 records the decision); allow: emit nothing")
    return parser.parse_args(argv)


def main(argv: Optional[list[str]] = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    try:
        _check_not_in_git_tree(args.output, "output")
        _check_not_in_git_tree(args.password_file, "password file")
        opts = Options(
            password=read_password(args.password_file),
            user=args.user,
            disk_id=args.disk_id,
            disk_size_gib=args.disk_size_gib,
            image_index=args.image_index,
            image_path=args.image_path,
            payload_label=args.payload_label,
            payload_dir=args.payload_dir,
            device_encryption=args.device_encryption,
        )
        data = build_unattend(opts)
        write_atomic(args.output, data)
    except UnattendError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    print(f"output: {args.output}")
    print(f"sha256: {hashlib.sha256(data).hexdigest()}")
    print(
        f"parameters: disk_id={opts.disk_id} disk_size_gib={opts.disk_size_gib} image_index={opts.image_index} "
        f"payload_label={opts.payload_label} device_encryption={opts.device_encryption}"
    )
    if opts.device_encryption == "prevent":
        print(f"note: {DEVICE_ENCRYPTION_NOTE}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
