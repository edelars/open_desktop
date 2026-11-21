#!/usr/bin/env python3
r"""Извлечение журналов Windows с raw-образов дисков виртуальной машины (S8-12).

Образы целевого диска и диска полезной нагрузки подключаются ТОЛЬКО ДЛЯ ЧТЕНИЯ
командой `hdiutil attach -readonly -nomount`; тома монтируются средствами macOS
(`diskutil mount readOnly`). Образ не изменяется, перед отключением образа
(detach в finally) проверяются размер и время изменения файла образа, а по
флагу --hash-images - и его SHA-256 до и после.

Что копируется в <out-root>/<run-id>/logs/ (пути зеркалируются относительно
корня тома; отсутствующее не ошибка, а запись в сводке):
  Windows\Panther\*                      (рекурсивно)
  $WINDOWS.~BT\Sources\Panther\*         (рекурсивно, если есть)
  Windows\INF\setupapi.*.log
  Windows\Minidump\*
  Windows\MEMORY.DMP                     (только размер и SHA-256, если > 64 MiB)
  Windows\System32\winevt\Logs\System.evtx
С диска полезной нагрузки (--payload-image) копируется каталог OD8\ (маркеры,
результаты, журналы, скопированные гостем) в logs/payload/.
Файлы больше 64 MiB не копируются: в сводке остаются размер и SHA-256.
Файлы "._*" (AppleDouble драйвера FAT в macOS) пропускаются. Симлинки внутри
тома (в файлах и в компонентах пути) не разыменовываются: такой путь пропускается
и отмечается в сводке. Файлы целевого тома попадают в сводку и тогда, когда на
этом томе нет каталога Windows (например, $WINDOWS.~BT на отдельном томе).
Если <out-root>/<run-id>/logs - симлинк, сбор отклоняется (ничего не удаляется).

Повторный запуск с тем же --run-id заменяет каталог logs/ целиком: результат тот
же, дублей нет (сводка не содержит времени запуска).

Коды выхода: 0 - собрано; 1 - ошибка инструментов/ввода-вывода; 2 - неверные
аргументы (в том числе --out-root внутри репозитория вне work/); 3 - каталога Windows не найдено и есть немонтированный NTFS-том
(типично: macOS не смонтировала NTFS), сводка предлагает запасной путь - и тогда,
когда файлы найдены на других томах; 4 - каталога Windows нет при смонтированных
томах и ничего не собрано (если файлы найдены, например $WINDOWS.~BT, - код 0).

Не меняйте образ и не запускайте машину на подключённом образе. Журналы Windows
не коммитятся: work/stage8/ игнорируется Git.
"""

import argparse
import fnmatch
import hashlib
import json
import os
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
import xml.parsers.expat

MAX_COPY_BYTES = 64 * 1024 * 1024
ERROR_LINES_LIMIT = 20
RUN_ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
EXIT_OK, EXIT_FAIL, EXIT_USAGE, EXIT_NOT_MOUNTED, EXIT_NO_WINDOWS = 0, 1, 2, 3, 4
REPO_ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
WORK_DIR = os.path.join(REPO_ROOT, "work")

# (категория, путь каталога от корня тома, шаблон имени, рекурсивно)
WINDOWS_RULES = [
    ("panther", ["Windows", "Panther"], "*", True),
    ("panther_bt", ["$WINDOWS.~BT", "Sources", "Panther"], "*", True),
    ("setupapi", ["Windows", "INF"], "setupapi.*.log", False),
    ("minidump", ["Windows", "Minidump"], "*", True),
    ("memory_dmp", ["Windows"], "MEMORY.DMP", False),
    ("system_evtx", ["Windows", "System32", "winevt", "Logs"], "System.evtx", False),
]
# Тома, которые macOS способна смонтировать без сторонних драйверов (NTFS - только чтение).
NTFS_KINDS = {"ntfs"}


class ToolError(Exception):
    pass


def run_cmd(argv, check=True):
    proc = subprocess.run(argv, capture_output=True, text=True, check=False)
    if check and proc.returncode != 0:
        raise ToolError(
            "%s: код %d: %s" % (" ".join(argv), proc.returncode, (proc.stderr or proc.stdout).strip())
        )
    return proc


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while True:
            chunk = f.read(1024 * 1024)
            if not chunk:
                break
            h.update(chunk)
    return h.hexdigest()


def macos_version():
    if sys.platform != "darwin":
        return None
    proc = run_cmd(["sw_vers"], check=False)
    info = {}
    for line in proc.stdout.splitlines():
        if ":" in line:
            k, v = line.split(":", 1)
            info[k.strip()] = v.strip()
    return "%s %s (%s)" % (
        info.get("ProductName", "macOS"),
        info.get("ProductVersion", "?"),
        info.get("BuildVersion", "?"),
    )


def image_fingerprint(path, with_hash):
    st = os.stat(path)
    fp = {"size": st.st_size, "mtime_ns": st.st_mtime_ns}
    if with_hash:
        fp["sha256"] = sha256_file(path)
    return fp


# ---------- hdiutil / diskutil ----------


def parse_plist(text, what):
    try:
        data = plistlib.loads(text.encode())
    except (plistlib.InvalidFileException, xml.parsers.expat.ExpatError, ValueError, UnicodeError) as e:
        raise ToolError("не удалось разобрать plist (%s): %s" % (what, e)) from e
    if not isinstance(data, dict):
        raise ToolError("plist (%s) не словарь" % what)
    return data


def attached_image_paths():
    proc = run_cmd(["hdiutil", "info", "-plist"])
    data = parse_plist(proc.stdout, "hdiutil info")
    paths = set()
    for img in data.get("images", []):
        p = img.get("image-path")
        if p:
            paths.add(os.path.realpath(p))
    return paths


def attach_readonly(image):
    """hdiutil attach -readonly -nomount; возвращает (корневое устройство, [тома])."""
    proc = run_cmd(
        [
            "hdiutil", "attach",
            "-readonly",
            "-nomount",
            "-noverify",
            "-noautofsck",
            "-imagekey", "diskimage-class=CRawDiskImage",
            "-plist",
            image,
        ]
    )
    # attach уже прошёл: образ подключён. Если дальше разбор вывода или поиск устройства
    # упадёт, образ нельзя оставлять подключённым - отключаем его по устройствам,
    # найденным прямо в тексте вывода, и только потом сообщаем об ошибке.
    try:
        return _parse_attach(proc.stdout, image)
    except BaseException:
        # Первичная ошибка (разбор вывода) важнее: сбой самого отключения её не скрывает,
        # а только дополняет сообщение в stderr; затем поднимается исходное исключение.
        try:
            _detach_from_output(proc.stdout)
        except Exception as detach_error:
            print("предупреждение: отключение образа после сбоя attach не удалось: %s" % detach_error, file=sys.stderr)
        raise


def _parse_attach(output, image):
    entities = parse_plist(output, "hdiutil attach").get("system-entities", [])
    whole = None
    volumes = []
    for e in entities:
        dev = e.get("dev-entry", "")
        if re.fullmatch(r"/dev/disk\d+", dev):
            whole = dev
        elif re.fullmatch(r"/dev/disk\d+s\d+", dev):
            volumes.append(
                {
                    "device": dev,
                    "content_hint": e.get("content-hint", ""),
                    "volume_kind": e.get("volume-kind", ""),
                    "potentially_mountable": bool(e.get("potentially-mountable")),
                }
            )
    if whole is None:
        # Образ без таблицы разделов: единственная запись и есть устройство.
        if volumes:
            whole = re.sub(r"s\d+$", "", volumes[0]["device"])
        else:
            raise ToolError("hdiutil attach не вернул устройств для " + image)
    return whole, volumes


def _detach_from_output(output):
    """Отключает все устройства /dev/diskN, упомянутые в выводе attach (best effort)."""
    devices = set(re.findall(r"/dev/disk\d+\b", output))
    devices.update(re.sub(r"s\d+$", "", d) for d in re.findall(r"/dev/disk\d+s\d+\b", output))
    for dev in sorted(devices):
        detach(dev)


def detach(whole):
    proc = run_cmd(["hdiutil", "detach", whole], check=False)
    if proc.returncode != 0:
        proc = run_cmd(["hdiutil", "detach", "-force", whole], check=False)
    return proc.returncode == 0, (proc.stderr or proc.stdout).strip()


def mount_readonly(device, mountpoint, mounted=None):
    """Монтирует том только для чтения; (True, "") или (False, причина).

    Том вносится в список mounted СРАЗУ после успешного diskutil mount, до diskutil info:
    если info упадёт исключением, вызывающий в finally всё равно размонтирует том и
    удалит точку монтирования. При отказе (том записываемый) запись из списка убирается.
    """
    proc = run_cmd(
        ["diskutil", "mount", "readOnly", "-mountPoint", mountpoint, device], check=False
    )
    if proc.returncode != 0:
        return False, (proc.stderr or proc.stdout).strip()
    entry = (device, mountpoint)
    if mounted is not None:
        mounted.append(entry)
    info = parse_plist(run_cmd(["diskutil", "info", "-plist", device]).stdout, "diskutil info")
    if info.get("WritableVolume") or info.get("Writable"):
        run_cmd(["diskutil", "unmount", device], check=False)
        if mounted is not None:
            mounted.remove(entry)
        return False, "том смонтирован с возможностью записи - отказ"
    return True, ""


def unmount(device):
    proc = run_cmd(["diskutil", "unmount", device], check=False)
    if proc.returncode != 0:
        run_cmd(["diskutil", "unmount", "force", device], check=False)


# ---------- сбор ----------


def find_ci(directory, name):
    """Имя в каталоге без учёта регистра (NTFS/FAT регистронезависимы, но не тестовые деревья)."""
    try:
        names = os.listdir(directory)
    except OSError:
        return None
    if name in names:
        return name
    low = name.lower()
    for n in sorted(names):
        if n.lower() == low:
            return n
    return None


def resolve_dir(root, parts, skipped=None):
    """Каталог по цепочке имён внутри тома: (путь, [реальные имена]) или (None, None).

    Симлинки не следуем ни в одном компоненте пути: том берётся из чужого образа, и
    ссылка Windows -> / вывела бы чтение за пределы тома. Такой путь пропускается, а
    его имя добавляется в skipped (для сводки).
    """
    cur = root
    rel = []
    for part in parts:
        found = find_ci(cur, part)
        if found is None:
            return None, None
        cur = os.path.join(cur, found)
        rel.append(found)
        if os.path.islink(cur):
            if skipped is not None:
                skipped.append("\\".join(rel))
            return None, None
    if not os.path.isdir(cur):
        return None, None
    return cur, rel


def walk_files(directory, recursive):
    """Детерминированный обход: относительные пути файлов (без симлинков)."""
    out = []
    for name in sorted(os.listdir(directory)):
        full = os.path.join(directory, name)
        if os.path.islink(full) or name.startswith("._"):
            # Симлинки не следуем; "._*" - метаданные AppleDouble, которые драйвер FAT в macOS
            # показывает вместо расширенных атрибутов (в Windows таких файлов нет).
            continue
        if os.path.isdir(full):
            if recursive:
                out.extend(os.path.join(name, p) for p in walk_files(full, True))
        elif os.path.isfile(full):
            out.append(name)
    return out


def copy_entry(src, dst, size):
    """Копирует файл (если не больше лимита) и возвращает запись сводки."""
    sha = sha256_file(src)
    entry = {"size": size, "sha256": sha, "copied": False}
    if size <= MAX_COPY_BYTES:
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copyfile(src, dst)
        if sha256_file(dst) != sha:
            raise ToolError("SHA-256 копии не совпал с источником: " + src)
        entry["copied"] = True
    else:
        entry["note"] = "больше 64 MiB: только размер и SHA-256"
    return entry


def collect_windows(root, logs_dir):
    """Собирает журналы Windows с корня смонтированного тома."""
    files = []
    missing = []
    for category, dir_parts, pattern, recursive in WINDOWS_RULES:
        skipped = []
        directory, rel_parts = resolve_dir(root, dir_parts, skipped)
        label = "\\".join(dir_parts + ([pattern] if pattern != "*" else []))
        if directory is None:
            missing.append(label + (" (симлинк в пути: %s, пропущено)" % skipped[0] if skipped else ""))
            continue
        matched = [
            rel
            for rel in walk_files(directory, recursive)
            if fnmatch.fnmatch(os.path.basename(rel).lower(), pattern.lower())
        ]
        if not matched:
            missing.append(label)
            continue
        for rel in matched:
            src = os.path.join(directory, rel)
            out_rel = os.path.join(*rel_parts, rel)
            entry = copy_entry(src, os.path.join(logs_dir, out_rel), os.path.getsize(src))
            entry["category"] = category
            entry["path"] = out_rel.replace(os.sep, "/")
            files.append(entry)
    return files, missing


def collect_payload(root, logs_dir, prefix):
    skipped = []
    od8, rel_parts = resolve_dir(root, ["OD8"], skipped)
    if od8 is None:
        return [], ["OD8" + (" (симлинк, пропущено)" if skipped else "")]
    files = []
    for rel in walk_files(od8, True):
        src = os.path.join(od8, rel)
        out_rel = os.path.join(prefix, *rel_parts, rel)
        entry = copy_entry(src, os.path.join(logs_dir, out_rel), os.path.getsize(src))
        entry["category"] = "payload"
        entry["path"] = out_rel.replace(os.sep, "/")
        files.append(entry)
    return files, []


def decode_log(data):
    if data[:2] in (b"\xff\xfe", b"\xfe\xff"):
        return data.decode("utf-16", errors="replace")
    return data.decode("utf-8-sig", errors="replace")


def setuperr_errors(logs_dir, files):
    result = {}
    for e in files:
        if e["category"] in ("panther", "panther_bt") and e["copied"]:
            if os.path.basename(e["path"]).lower() == "setuperr.log":
                with open(os.path.join(logs_dir, *e["path"].split("/")), "rb") as f:
                    text = decode_log(f.read())
                lines = [ln.rstrip() for ln in text.splitlines() if "Error" in ln]
                result[e["path"]] = lines[:ERROR_LINES_LIMIT]
    return result


# ---------- образ ----------


def collect_target_volume(mp, staging_logs, v, rec, missing_sets):
    """Собирает журналы Windows с одного тома целевого образа и вносит их в запись rec.

    Файлы попадают в сводку с любого тома, где они найдены: например, $WINDOWS.~BT
    может лежать на отдельном томе без каталога Windows. has_windows ставится только
    тому, где есть каталог Windows.
    """
    found, missing = collect_windows(mp, staging_logs)
    has_windows = resolve_dir(mp, ["Windows"])[0] is not None
    if has_windows:
        v["has_windows"] = True
    if found:
        v["files_collected"] = len(found)
        rec["files"].extend(found)
    if has_windows or found:
        missing_sets.append(missing)


def process_image(image, role, index, staging_logs, mount_base, with_hash):
    """Подключает образ, собирает файлы, отключает. Возвращает запись сводки."""
    rec = {
        "role": role,
        "image": os.path.abspath(image),
        "attached": False,
        "volumes": [],
        "files": [],
        "missing": [],
        "advice": [],
    }
    before = image_fingerprint(image, with_hash)
    rec["before"] = before
    if os.path.realpath(image) in attached_image_paths():
        raise ToolError("образ уже подключён (hdiutil info): отключите его перед сбором: " + image)
    whole = None
    mounted = []
    missing_sets = []
    try:
        whole, volumes = attach_readonly(image)
        rec["attached"] = True
        for vol in volumes:
            # Номер /dev/diskN меняется от запуска к запуску: в сводке только суффикс среза.
            v = {k: x for k, x in vol.items() if k != "device"}
            v["slice"] = re.sub(r"^/dev/disk\d+", "", vol["device"])
            v["mounted"] = False
            rec["volumes"].append(v)
            if not vol["potentially_mountable"]:
                v["note"] = "macOS не считает том монтируемым"
                continue
            mp = tempfile.mkdtemp(prefix="od8-logs-", dir=mount_base)
            ok, msg = mount_readonly(vol["device"], mp, mounted)
            if not ok:
                os.rmdir(mp)
                for name in {mp, os.path.realpath(mp)}:
                    msg = msg.replace(name, "<mountpoint>")
                v["note"] = re.sub(r"/dev/disk\d+", "/dev/diskN", msg)
                continue
            v["mounted"] = True
            if role == "target":
                collect_target_volume(mp, staging_logs, v, rec, missing_sets)
            else:
                found, missing = collect_payload(mp, staging_logs, "payload/%d" % index)
                if found:
                    v["has_payload"] = True
                    rec["files"].extend(found)
                # "Отсутствует" считается по объединению томов (как у целевого диска):
                # OD8 на одном томе не должен давать ложное "отсутствует" из-за пустого другого.
                missing_sets.append(missing)
    finally:
        for dev, mp in reversed(mounted):
            unmount(dev)
        for _, mp in mounted:
            try:
                os.rmdir(mp)
            except OSError:
                pass
        if whole is not None:
            ok, msg = detach(whole)
            rec["detached"] = ok
            if not ok:
                rec["detach_error"] = msg
    if missing_sets:
        # Отсутствует то, чего нет ни на одном томе, давшем файлы или каталог Windows.
        rec["missing"] = [m for m in missing_sets[0] if all(m in other for other in missing_sets[1:])]
    after = image_fingerprint(image, with_hash)
    rec["after"] = after
    rec["image_unchanged"] = before == after
    return rec


def ntfs_advice(rec):
    unmounted_ntfs = [
        v for v in rec["volumes"] if v.get("volume_kind") in NTFS_KINDS and not v["mounted"]
    ]
    unmounted_hint = [
        v
        for v in rec["volumes"]
        if not v["mounted"] and v.get("content_hint") in ("Windows_NTFS", "Microsoft Basic Data")
    ]
    return unmounted_ntfs or unmounted_hint


FALLBACK_TEXT = (
    "macOS не смонтировала NTFS-том этого образа. Запасной путь (S8-27): гость командой "
    "'xcopy'/'robocopy' копирует Windows\\Panther, Windows\\INF\\setupapi.*.log, "
    "Windows\\Minidump и System.evtx на FAT-диск полезной нагрузки (каталог OD8\\logs) "
    "ДО выключения; затем запустите скрипт с --payload-image, он заберёт OD8\\ с FAT. "
    "Сторонний драйвер NTFS и запись в образ не используются."
)


def target_exit_code(target):
    """Код выхода по целевому образу; при необходимости добавляет совет в target["advice"].

    Каталог Windows найден - 0. Иначе, если есть немонтированный NTFS-том, совет о
    запасном пути выдаётся ВСЕГДА (даже когда файлы найдены на других томах, например
    $WINDOWS.~BT) и код 3. Иначе при отсутствии файлов - 4; если файлы найдены, а
    каталога Windows нет и NTFS-томов не осталось - 0 (собрано, что было).
    """
    if any(v.get("has_windows") for v in target["volumes"]):
        return EXIT_OK
    if ntfs_advice(target):
        target["advice"].append(FALLBACK_TEXT)
        return EXIT_NOT_MOUNTED
    if not target["files"]:
        target["advice"].append("смонтированные тома не содержат каталога Windows")
        return EXIT_NO_WINDOWS
    return EXIT_OK


def build_summary(args, recs, mac):
    all_files = [f for r in recs for f in r["files"]]
    return {
        "run_id": args.run_id,
        "host_macos": mac,
        "hdiutil_flags": "attach -readonly -nomount -noverify -noautofsck -imagekey diskimage-class=CRawDiskImage -plist",
        "images": recs,
        "file_count": len(all_files),
        "setuperr_errors": {},
    }


def format_text(summary):
    out = ["run-id: %s" % summary["run_id"], "host: %s" % summary["host_macos"]]
    for rec in summary["images"]:
        out.append(
            "образ [%s]: %s  (подключён: %s, отключён: %s, не изменён: %s)"
            % (
                rec["role"],
                rec["image"],
                rec["attached"],
                rec.get("detached"),
                rec.get("image_unchanged"),
            )
        )
        for v in rec["volumes"]:
            out.append(
                "  том %s hint=%r kind=%r смонтирован=%s%s"
                % (
                    v["slice"],
                    v["content_hint"],
                    v["volume_kind"],
                    v["mounted"],
                    ("  (" + v["note"] + ")") if v.get("note") else "",
                )
            )
        for f in rec["files"]:
            out.append(
                "  %s  %10d  %s%s"
                % (f["sha256"], f["size"], f["path"], "" if f["copied"] else "  [не скопирован]")
            )
        for m in rec["missing"]:
            out.append("  отсутствует: %s" % m)
        for a in rec["advice"]:
            out.append("  ВНИМАНИЕ: %s" % a)
    for path, lines in sorted(summary["setuperr_errors"].items()):
        out.append("Error из %s (первые %d):" % (path, ERROR_LINES_LIMIT))
        out.extend("  " + ln for ln in lines or ["(строк с Error нет)"])
    out.append("файлов: %d" % summary["file_count"])
    return "\n".join(out)


def parse_args(argv):
    p = argparse.ArgumentParser(
        description="Извлечь журналы Windows с raw-образов диска (только чтение, hdiutil/diskutil, macOS).",
        epilog="Коды выхода: 0 собрано; 1 ошибка; 2 аргументы; 3 Windows не найдена, есть немонтированный NTFS-том; 4 нет каталога Windows при смонтированных томах.",
    )
    p.add_argument("--run-id", required=True, help="идентификатор прогона: [A-Za-z0-9._-]+")
    p.add_argument("--target-image", required=True, help="raw-образ целевого диска Windows")
    p.add_argument("--payload-image", action="append", default=[], help="raw-образ диска полезной нагрузки (можно несколько)")
    default_root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "work", "stage8")
    p.add_argument("--out-root", default=os.path.normpath(default_root), help="корень результатов (по умолчанию work/stage8)")
    p.add_argument("--json", action="store_true", help="печатать сводку в JSON вместо текста")
    p.add_argument("--hash-images", action="store_true", help="считать SHA-256 образов до и после (медленно на больших образах)")
    return p.parse_args(argv)


def _is_inside(path, directory):
    """path лежит в directory или совпадает с ним.

    Строковое сравнение регистрозависимо, а APFS по умолчанию регистронезависима:
    предки path сверяются с directory через os.path.samefile (st_dev/st_ino),
    строковое сравнение остаётся запасным, пока directory ещё не существует.
    """
    path = os.path.realpath(path)
    directory = os.path.realpath(directory)
    cur = path
    while True:
        try:
            if os.path.samefile(cur, directory):
                return True
        except OSError:
            pass
        parent = os.path.dirname(cur)
        if parent == cur:
            break
        cur = parent
    return os.path.commonpath([path, directory]) == directory


def check_out_root(out_root):
    """Журналы Windows не должны оказаться в дереве Git: внутри репозитория - только work/."""
    if _is_inside(out_root, REPO_ROOT) and not _is_inside(out_root, WORK_DIR):
        raise ValueError(
            "--out-root внутри репозитория допустим только под work/ (игнорируется Git): %s" % out_root
        )


def check_logs_target(run_dir):
    """Отказ, если logs - симлинк или не каталог: replace-логика не должна идти по ссылке."""
    final_logs = os.path.join(run_dir, "logs")
    if os.path.islink(final_logs):
        raise ToolError("%s - симлинк: отказ (удалять и заменять по ссылке нельзя)" % final_logs)
    if os.path.lexists(final_logs) and not os.path.isdir(final_logs):
        raise ToolError("%s существует и не каталог: отказ" % final_logs)
    return final_logs


def main(argv=None):
    args = parse_args(sys.argv[1:] if argv is None else argv)
    if not RUN_ID_RE.match(args.run_id):
        print("неверный --run-id: %r" % args.run_id, file=sys.stderr)
        return EXIT_USAGE
    images = [("target", args.target_image)] + [("payload", p) for p in args.payload_image]
    for _, path in images:
        if not os.path.isfile(path):
            print("образ не найден или не файл: %s" % path, file=sys.stderr)
            return EXIT_USAGE
    try:
        check_out_root(args.out_root)
    except ValueError as e:
        print("ошибка: %s" % e, file=sys.stderr)
        return EXIT_USAGE
    if sys.platform != "darwin":
        print("скрипт использует hdiutil/diskutil и работает только на macOS", file=sys.stderr)
        return EXIT_FAIL

    run_dir = os.path.join(args.out_root, args.run_id)
    staging = None
    mount_base = None
    try:
        os.makedirs(run_dir, exist_ok=True)
        check_logs_target(run_dir)
        staging = tempfile.mkdtemp(prefix=".logs-staging-", dir=run_dir)
        staging_logs = os.path.join(staging, "logs")
        os.makedirs(staging_logs)
        mount_base = tempfile.mkdtemp(prefix="od8-mnt-")
        mac = macos_version()
        recs = []
        for index, (role, path) in enumerate(images):
            recs.append(process_image(path, role, index, staging_logs, mount_base, args.hash_images))
        summary = build_summary(args, recs, mac)
        target = recs[0]
        code = EXIT_OK
        code = target_exit_code(target)
        for rec in recs:
            if not rec["image_unchanged"]:
                raise ToolError("образ изменился во время сбора: " + rec["image"])
            if rec["attached"] and not rec.get("detached"):
                raise ToolError("образ не отключён: %s (%s)" % (rec["image"], rec.get("detach_error")))
        all_files = [f for r in recs for f in r["files"]]
        summary["setuperr_errors"] = setuperr_errors(staging_logs, all_files)
        with open(os.path.join(staging_logs, "summary.json"), "w", encoding="utf-8") as f:
            json.dump(summary, f, ensure_ascii=False, indent=2, sort_keys=True)
            f.write("\n")
        final_logs = check_logs_target(run_dir)
        if os.path.lexists(final_logs):
            shutil.rmtree(final_logs)
        os.rename(staging_logs, final_logs)
        if args.json:
            print(json.dumps(summary, ensure_ascii=False, indent=2, sort_keys=True))
        else:
            print(format_text(summary))
        return code
    except ToolError as e:
        print("ошибка: %s" % e, file=sys.stderr)
        return EXIT_FAIL
    except OSError as e:
        print("ошибка ввода-вывода: %s" % e, file=sys.stderr)
        return EXIT_FAIL
    finally:
        if staging is not None:
            shutil.rmtree(staging, ignore_errors=True)
        if mount_base is not None:
            shutil.rmtree(mount_base, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
