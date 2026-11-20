r"""Тесты tests/compat/windows_collect_logs.py (S8-12).

Запуск: python3 -I tests/compat/test_windows_collect_logs.py

Образы для тестов на macOS:

* образ диска полезной нагрузки (GPT + FAT32, метка OD8PAYLOAD) собирается нашим
  писателем od-storage командой `od-image-tool make-data-disk` (бинарь
  target/debug/od-image-tool или переменная OD8_IMAGE_TOOL; сборка:
  cargo build -p od-image-tool --offline). Тест писателя
  (test_collect_payload_from_od_image_tool) без бинаря ПРОПУСКАЕТСЯ с причиной; если
  бинарь есть, но make-data-disk упал, тест ПАДАЕТ (молчаливого перехода на hdiutil
  нет). Отдельный тест на образе, собранном hdiutil (GPT-обёртка с FAT: MS-DOS,
  FAT16/FAT32 по размеру), проверяет только чтение скриптом, а не наш писатель.
  Каждый такой тест печатает в stderr выбранный путь: writer=od-image-tool|hdiutil;
* образ целевого диска с деревом Windows\... собирается ТОЛЬКО hdiutil (GPT-обёртка
  с FAT, MS-DOS): писатель od-storage кладёт на диск лишь каталог OD8\ и
  установочный носитель из ISO, произвольного дерева Windows\ он не умеет.

Тесты на образе проверяют подключение, чтение, сбор, неизменность образа и
идемпотентность, но НЕ покрывают NTFS. Монтирование NTFS средствами macOS здесь
не проверяется: на хосте нет инструмента, создающего NTFS-образ (mkntfs
отсутствует), реального образа Windows у теста нет.
"""

import contextlib
import hashlib
import importlib.util
import io
import json
import os
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.normpath(os.path.join(HERE, "..", ".."))
SCRIPT = os.path.join(HERE, "windows_collect_logs.py")
spec = importlib.util.spec_from_file_location("windows_collect_logs", SCRIPT)
wcl = importlib.util.module_from_spec(spec)
spec.loader.exec_module(wcl)

SETUPERR = (
    "2026-11-20 10:00:00, Info                  SP     start\n"
    "2026-11-20 10:00:01, Error                 SP     Failed to apply driver 0x80070057\n"
    "2026-11-20 10:00:02, Error      [0x0606c8] IBS    something broke\n"
)


def write_file(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data)


def sha(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def windows_tree(root):
    """Дерево с разным регистром имён, как на NTFS/FAT после установки."""
    write_file(os.path.join(root, "Windows", "Panther", "setupact.log"), b"act\n")
    write_file(os.path.join(root, "Windows", "Panther", "setuperr.log"), SETUPERR.encode())
    write_file(os.path.join(root, "Windows", "Panther", "UnattendGC", "setupact.log"), b"gc\n")
    write_file(os.path.join(root, "$WINDOWS.~BT", "Sources", "Panther", "setupact.log"), b"bt\n")
    write_file(os.path.join(root, "Windows", "INF", "setupapi.dev.log"), b"dev\n")
    write_file(os.path.join(root, "Windows", "INF", "setupapi.setup.log"), b"setup\n")
    write_file(os.path.join(root, "Windows", "INF", "other.log"), b"not collected\n")
    write_file(os.path.join(root, "Windows", "Minidump", "112026-1.dmp"), b"MDMP" * 8)
    write_file(os.path.join(root, "Windows", "MEMORY.DMP"), b"PAGEDU64" * 16)
    write_file(os.path.join(root, "Windows", "System32", "winevt", "Logs", "System.evtx"), b"ElfFile\0")
    write_file(os.path.join(root, "Windows", "System32", "winevt", "Logs", "Other.evtx"), b"nope")


class CollectUnit(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="od8-unit-")
        self.addCleanup(shutil.rmtree, self.tmp, True)

    def test_collects_listed_files_with_hashes(self):
        root = os.path.join(self.tmp, "vol")
        windows_tree(root)
        logs = os.path.join(self.tmp, "logs")
        files, missing = wcl.collect_windows(root, logs)
        paths = {f["path"] for f in files}
        self.assertEqual(
            paths,
            {
                "Windows/Panther/setupact.log",
                "Windows/Panther/setuperr.log",
                "Windows/Panther/UnattendGC/setupact.log",
                "$WINDOWS.~BT/Sources/Panther/setupact.log",
                "Windows/INF/setupapi.dev.log",
                "Windows/INF/setupapi.setup.log",
                "Windows/Minidump/112026-1.dmp",
                "Windows/MEMORY.DMP",
                "Windows/System32/winevt/Logs/System.evtx",
            },
        )
        self.assertEqual(missing, [])
        for f in files:
            self.assertTrue(f["copied"])
            self.assertEqual(f["sha256"], sha(os.path.join(logs, *f["path"].split("/"))))
        errs = wcl.setuperr_errors(logs, files)
        self.assertEqual(list(errs), ["Windows/Panther/setuperr.log"])
        self.assertEqual(len(errs["Windows/Panther/setuperr.log"]), 2)
        self.assertTrue(all("Error" in ln for ln in errs["Windows/Panther/setuperr.log"]))

    def test_missing_paths_are_reported_not_fatal(self):
        root = os.path.join(self.tmp, "vol")
        write_file(os.path.join(root, "Windows", "Panther", "setupact.log"), b"x")
        files, missing = wcl.collect_windows(root, os.path.join(self.tmp, "logs"))
        self.assertEqual([f["path"] for f in files], ["Windows/Panther/setupact.log"])
        self.assertIn("$WINDOWS.~BT\\Sources\\Panther", missing)
        self.assertIn("Windows\\MEMORY.DMP", missing)
        self.assertIn("Windows\\Minidump", missing)

    def test_case_insensitive_names(self):
        root = os.path.join(self.tmp, "vol")
        write_file(os.path.join(root, "WINDOWS", "panther", "SETUPERR.LOG"), SETUPERR.encode())
        write_file(os.path.join(root, "WINDOWS", "Inf", "SetupAPI.App.LOG"), b"a")
        files, _ = wcl.collect_windows(root, os.path.join(self.tmp, "logs"))
        self.assertEqual(
            {f["path"] for f in files},
            {"WINDOWS/panther/SETUPERR.LOG", "WINDOWS/Inf/SetupAPI.App.LOG"},
        )

    def test_large_memory_dmp_is_hash_only(self):
        root = os.path.join(self.tmp, "vol")
        dmp = os.path.join(root, "Windows", "MEMORY.DMP")
        os.makedirs(os.path.dirname(dmp))
        with open(dmp, "wb") as f:
            f.truncate(wcl.MAX_COPY_BYTES + 1)
        logs = os.path.join(self.tmp, "logs")
        files, _ = wcl.collect_windows(root, logs)
        (entry,) = [f for f in files if f["category"] == "memory_dmp"]
        self.assertFalse(entry["copied"])
        self.assertEqual(entry["size"], wcl.MAX_COPY_BYTES + 1)
        self.assertEqual(entry["sha256"], sha(dmp))
        self.assertFalse(os.path.exists(os.path.join(logs, "Windows", "MEMORY.DMP")))

    def test_utf16_setuperr(self):
        root = os.path.join(self.tmp, "vol")
        write_file(os.path.join(root, "Windows", "Panther", "setuperr.log"), SETUPERR.encode("utf-16"))
        logs = os.path.join(self.tmp, "logs")
        files, _ = wcl.collect_windows(root, logs)
        errs = wcl.setuperr_errors(logs, files)
        self.assertEqual(len(errs["Windows/Panther/setuperr.log"]), 2)

    def test_run_id_and_usage(self):
        self.assertEqual(wcl.main(["--run-id", "../x", "--target-image", __file__]), wcl.EXIT_USAGE)
        self.assertEqual(
            wcl.main(["--run-id", "ok", "--target-image", os.path.join(self.tmp, "none.img")]),
            wcl.EXIT_USAGE,
        )


    def test_symlinked_path_components_are_not_followed(self):
        outside = os.path.join(self.tmp, "outside")
        write_file(os.path.join(outside, "Panther", "setupact.log"), b"secret outside the volume\n")
        write_file(os.path.join(outside, "setupapi.dev.log"), b"secret too\n")
        root = os.path.join(self.tmp, "vol")
        os.makedirs(root)
        os.symlink(outside, os.path.join(root, "Windows"))
        skipped = []
        self.assertEqual(wcl.resolve_dir(root, ["Windows", "Panther"], skipped), (None, None))
        self.assertEqual(skipped, ["Windows"])
        files, missing = wcl.collect_windows(root, os.path.join(self.tmp, "logs"))
        self.assertEqual(files, [])
        self.assertIn("Windows\\Panther (симлинк в пути: Windows, пропущено)", missing)
        self.assertFalse(os.path.exists(os.path.join(self.tmp, "logs")))
        # Симлинк на более глубоком компоненте.
        os.unlink(os.path.join(root, "Windows"))
        write_file(os.path.join(root, "Windows", "Minidump", "a.dmp"), b"MDMP")
        os.symlink(os.path.join(outside, "Panther"), os.path.join(root, "Windows", "Panther"))
        files, missing = wcl.collect_windows(root, os.path.join(self.tmp, "logs"))
        self.assertEqual([f["path"] for f in files], ["Windows/Minidump/a.dmp"])
        self.assertTrue(any("симлинк" in m and "Windows\\Panther" in m for m in missing), missing)
        # OD8 диска полезной нагрузки - симлинк.
        payload = os.path.join(self.tmp, "payload")
        os.makedirs(payload)
        os.symlink(outside, os.path.join(payload, "OD8"))
        pfiles, pmissing = wcl.collect_payload(payload, os.path.join(self.tmp, "plogs"), "payload/1")
        self.assertEqual(pfiles, [])
        self.assertEqual(pmissing, ["OD8 (симлинк, пропущено)"])

    def test_files_from_volume_without_windows_directory_are_reported(self):
        # $WINDOWS.~BT на отдельном томе: каталога Windows там нет, файлы всё равно в сводке.
        bt = os.path.join(self.tmp, "bt")
        write_file(os.path.join(bt, "$WINDOWS.~BT", "Sources", "Panther", "setupact.log"), b"bt\n")
        other = os.path.join(self.tmp, "esp")
        write_file(os.path.join(other, "EFI", "BOOT", "bootx64.efi"), b"MZ")
        rec = {"files": [], "missing": []}
        missing_sets = []
        v_bt, v_esp = {}, {}
        logs = os.path.join(self.tmp, "logs")
        wcl.collect_target_volume(bt, logs, v_bt, rec, missing_sets)
        wcl.collect_target_volume(other, logs, v_esp, rec, missing_sets)
        self.assertEqual([f["path"] for f in rec["files"]], ["$WINDOWS.~BT/Sources/Panther/setupact.log"])
        self.assertEqual(v_bt, {"files_collected": 1})
        self.assertNotIn("has_windows", v_bt)
        self.assertEqual(v_esp, {})
        self.assertEqual(len(missing_sets), 1)  # том без файлов и без Windows не влияет на "отсутствует"
        self.assertIn("Windows\\Panther", missing_sets[0])
        self.assertNotIn("$WINDOWS.~BT\\Sources\\Panther", missing_sets[0])

    def test_attach_detaches_when_output_cannot_be_parsed(self):
        calls = []

        def fake_run_cmd(argv, check=True):
            calls.append(argv)
            out = "<plist garbage /dev/disk9 /dev/disk9s1 /dev/disk9s2"  # не well-formed
            return subprocess.CompletedProcess(argv, 0, stdout=out, stderr="")

        saved = wcl.run_cmd
        wcl.run_cmd = fake_run_cmd
        try:
            with self.assertRaises(wcl.ToolError):
                wcl.attach_readonly("image.img")
            self.assertEqual([c[:2] for c in calls if c[1] == "detach"], [["hdiutil", "detach"]])
            self.assertEqual([c[2] for c in calls if c[1] == "detach"], ["/dev/disk9"])
            # attach прошёл, а устройств в выводе нет: отключать нечего, ошибка сохраняется.
            calls.clear()
            wcl.run_cmd = lambda argv, check=True: (
                calls.append(argv) or subprocess.CompletedProcess(argv, 0, stdout=wcl_plist([]), stderr="")
            )
            with self.assertRaises(wcl.ToolError):
                wcl.attach_readonly("image.img")
            self.assertEqual([c for c in calls if c[1] == "detach"], [])
        finally:
            wcl.run_cmd = saved

    def test_target_exit_code_and_ntfs_advice(self):
        def rec(volumes, files):
            return {"volumes": volumes, "files": files, "advice": []}

        bt = {"slice": "s1", "mounted": True, "volume_kind": "msdos", "content_hint": "", "files_collected": 1}
        ntfs = {"slice": "s2", "mounted": False, "volume_kind": "ntfs", "content_hint": "Windows_NTFS"}
        win = {"slice": "s3", "mounted": True, "volume_kind": "ntfs", "has_windows": True}
        fat = {"slice": "s1", "mounted": True, "volume_kind": "msdos", "content_hint": ""}
        one_file = [{"path": "$WINDOWS.~BT/Sources/Panther/setupact.log"}]
        # Файлы найдены на томе без Windows, NTFS-том не смонтирован: совет и код 3, а не молчаливый 0.
        r = rec([bt, ntfs], one_file)
        self.assertEqual(wcl.target_exit_code(r), wcl.EXIT_NOT_MOUNTED)
        self.assertEqual(r["advice"], [wcl.FALLBACK_TEXT])
        # Файлов нет вовсе - тоже 3.
        r = rec([fat, ntfs], [])
        self.assertEqual(wcl.target_exit_code(r), wcl.EXIT_NOT_MOUNTED)
        self.assertEqual(r["advice"], [wcl.FALLBACK_TEXT])
        # Каталог Windows найден: совета нет, даже если другой том не смонтирован.
        r = rec([win, ntfs], one_file)
        self.assertEqual(wcl.target_exit_code(r), wcl.EXIT_OK)
        self.assertEqual(r["advice"], [])
        # Тома смонтированы, Windows нет, файлов нет - 4.
        r = rec([fat], [])
        self.assertEqual(wcl.target_exit_code(r), wcl.EXIT_NO_WINDOWS)
        self.assertEqual(len(r["advice"]), 1)
        # Тома смонтированы, Windows нет, но файлы собраны ($WINDOWS.~BT) и NTFS-томов не осталось - 0.
        r = rec([bt], one_file)
        self.assertEqual(wcl.target_exit_code(r), wcl.EXIT_OK)
        self.assertEqual(r["advice"], [])

    def _payload_missing(self, od8_by_slice):
        """process_image для диска полезной нагрузки из двух томов (подмена hdiutil/diskutil)."""
        case = tempfile.mkdtemp(prefix="case-", dir=self.tmp)
        image = os.path.join(case, "payload.img")
        write_file(image, b"raw")
        mount_base = os.path.join(case, "mnt")
        os.makedirs(mount_base)
        staging = os.path.join(case, "staging")
        os.makedirs(staging)
        volumes = [
            {"device": "/dev/disk9s%d" % n, "content_hint": "", "volume_kind": "msdos", "potentially_mountable": True}
            for n in (1, 2)
        ]

        def fake_mount(device, mountpoint, mounted=None):
            mounted.append((device, mountpoint))
            if od8_by_slice[device[-1]]:
                write_file(os.path.join(mountpoint, "OD8", "markers.txt"), b"m")
            return True, ""

        with mock.patch.multiple(
            wcl,
            attached_image_paths=lambda: set(),
            attach_readonly=lambda img: ("/dev/disk9", volumes),
            mount_readonly=fake_mount,
            unmount=lambda dev: None,
            detach=lambda whole: (True, ""),
        ):
            return wcl.process_image(image, "payload", 1, staging, mount_base, False)

    def test_payload_missing_is_union_over_volumes(self):
        # OD8 на первом томе, второй том пуст: "отсутствует: OD8" - ложное.
        r = self._payload_missing({"1": True, "2": False})
        self.assertEqual(len(r["files"]), 1)
        self.assertEqual(r["missing"], [])
        # Порядок томов не важен.
        r = self._payload_missing({"1": False, "2": True})
        self.assertEqual(len(r["files"]), 1)
        self.assertEqual(r["missing"], [])
        # OD8 нет ни на одном томе: отсутствует.
        r = self._payload_missing({"1": False, "2": False})
        self.assertEqual(r["files"], [])
        self.assertEqual(r["missing"], ["OD8"])

    def test_mount_readonly_registers_volume_before_diskutil_info(self):
        calls = []

        def fake_run_cmd(argv, check=True):
            calls.append(argv)
            if argv[:2] == ["diskutil", "mount"]:
                return subprocess.CompletedProcess(argv, 0, stdout="", stderr="")
            if argv[:2] == ["diskutil", "info"]:
                raise wcl.ToolError("diskutil info упал")
            return subprocess.CompletedProcess(argv, 0, stdout="", stderr="")

        mounted = []
        with mock.patch.object(wcl, "run_cmd", fake_run_cmd):
            with self.assertRaises(wcl.ToolError):
                wcl.mount_readonly("/dev/disk9s1", "/mp", mounted)
        # Том уже смонтирован: он обязан быть в списке, чтобы finally его размонтировал.
        self.assertEqual(mounted, [("/dev/disk9s1", "/mp")])

        # Сквозной случай: process_image при падении diskutil info размонтирует том и отключает образ.
        calls.clear()
        image = os.path.join(self.tmp, "t.img")
        write_file(image, b"raw")
        mount_base = os.path.join(self.tmp, "mnt")
        os.makedirs(mount_base)
        vol = {"device": "/dev/disk9s1", "content_hint": "", "volume_kind": "msdos", "potentially_mountable": True}
        with mock.patch.multiple(
            wcl,
            run_cmd=fake_run_cmd,
            attached_image_paths=lambda: set(),
            attach_readonly=lambda img: ("/dev/disk9", [vol]),
        ):
            with self.assertRaises(wcl.ToolError):
                wcl.process_image(image, "target", 0, os.path.join(self.tmp, "st"), mount_base, False)
        self.assertIn(["diskutil", "unmount", "/dev/disk9s1"], calls)
        self.assertIn(["hdiutil", "detach", "/dev/disk9"], calls)
        self.assertEqual(os.listdir(mount_base), [])  # точка монтирования удалена

        # Записываемый том: отказ и запись из списка убрана (размонтирован внутри mount_readonly).
        def writable_run_cmd(argv, check=True):
            out = plistlib.dumps({"WritableVolume": True}).decode() if argv[:2] == ["diskutil", "info"] else ""
            return subprocess.CompletedProcess(argv, 0, stdout=out, stderr="")

        mounted = []
        with mock.patch.object(wcl, "run_cmd", writable_run_cmd):
            ok, _ = wcl.mount_readonly("/dev/disk9s1", "/mp", mounted)
        self.assertFalse(ok)
        self.assertEqual(mounted, [])

    def test_attach_failure_keeps_original_error_when_detach_fails(self):
        def fake_run_cmd(argv, check=True):
            if argv[:2] == ["hdiutil", "detach"]:
                raise OSError("detach boom")
            return subprocess.CompletedProcess(argv, 0, stdout="<plist garbage /dev/disk9 /dev/disk9s1", stderr="")

        err = io.StringIO()
        with mock.patch.object(wcl, "run_cmd", fake_run_cmd), contextlib.redirect_stderr(err):
            with self.assertRaises(wcl.ToolError) as cm:
                wcl.attach_readonly("image.img")
        self.assertIn("plist", str(cm.exception))  # исходная ошибка разбора, а не OSError из detach
        self.assertIn("detach boom", err.getvalue())

    def test_out_root_inside_repository_is_refused_except_work(self):
        with self.assertRaises(ValueError):
            wcl.check_out_root(os.path.join(REPO, "docs", "x"))
        with self.assertRaises(ValueError):
            wcl.check_out_root(REPO)
        wcl.check_out_root(os.path.join(REPO, "work", "stage8"))
        wcl.check_out_root(os.path.join(self.tmp, "anywhere"))
        # APFS регистронезависима: тот же каталог другим регистром не должен проскочить.
        upper = os.path.join(os.path.dirname(REPO), os.path.basename(REPO).upper(), "docs", "x")
        if os.path.exists(os.path.join(os.path.dirname(REPO), os.path.basename(REPO).upper())):
            with self.assertRaises(ValueError):
                wcl.check_out_root(upper)
            if os.path.isdir(os.path.join(REPO, "work")):
                wcl.check_out_root(os.path.join(os.path.dirname(REPO), os.path.basename(REPO).upper(), "WORK", "x"))
        else:
            self.skipTest("файловая система регистрозависима")

    def test_out_root_inside_repository_cli_exit_2(self):
        target = os.path.join(REPO, "tests", "compat", "zz_forbidden_out_root")
        r = subprocess.run(
            [sys.executable, "-I", SCRIPT, "--run-id", "ok", "--target-image", __file__, "--out-root", target],
            capture_output=True, text=True, check=False,
        )
        self.assertEqual(r.returncode, wcl.EXIT_USAGE, r.stderr)
        self.assertIn("work/", r.stderr)
        self.assertFalse(os.path.exists(target))

    @unittest.skipUnless(sys.platform == "darwin", "основной путь доступен только на macOS")
    def test_oserror_gives_clean_exit_1_without_traceback(self):
        not_a_dir = os.path.join(self.tmp, "file")
        write_file(not_a_dir, b"x")
        r = subprocess.run(
            [sys.executable, "-I", SCRIPT, "--run-id", "ok", "--target-image", __file__, "--out-root", not_a_dir],
            capture_output=True, text=True, check=False,
        )
        self.assertEqual(r.returncode, wcl.EXIT_FAIL, r.stderr)
        self.assertIn("ошибка ввода-вывода", r.stderr)
        self.assertNotIn("Traceback", r.stderr)

    @unittest.skipUnless(sys.platform == "darwin", "основной путь доступен только на macOS")
    def test_logs_symlink_is_refused_not_deleted(self):
        victim = os.path.join(self.tmp, "victim")
        write_file(os.path.join(victim, "keep.txt"), b"keep")
        run_dir = os.path.join(self.tmp, "out", "t")
        os.makedirs(run_dir)
        os.symlink(victim, os.path.join(run_dir, "logs"))
        r = subprocess.run(
            [sys.executable, "-I", SCRIPT, "--run-id", "t", "--target-image", __file__, "--out-root", os.path.join(self.tmp, "out")],
            capture_output=True, text=True, check=False,
        )
        self.assertEqual(r.returncode, wcl.EXIT_FAIL, r.stderr)
        self.assertIn("симлинк", r.stderr)
        self.assertNotIn("Traceback", r.stderr)
        self.assertTrue(os.path.islink(os.path.join(run_dir, "logs")))
        self.assertEqual(os.listdir(victim), ["keep.txt"])


def wcl_plist(entities):
    import plistlib

    return plistlib.dumps({"system-entities": entities}).decode()


def hdiutil(*args):
    return subprocess.run(["hdiutil", *args], capture_output=True, text=True, check=True)


PAYLOAD_NONCE = "0123456789abcdef0123456789abcdef"
PAYLOAD_FILE = b"payload file from the manifest\n"


def find_image_tool():
    for cand in (os.environ.get("OD8_IMAGE_TOOL"), os.path.join(REPO, "target", "debug", "od-image-tool")):
        if cand and os.path.isfile(cand) and os.access(cand, os.X_OK):
            return cand
    return None


def make_payload_with_writer(tool, base):
    """Диск полезной нагрузки нашим писателем (GPT + FAT32, OD8PAYLOAD); при сбое - RuntimeError."""
    work = base + "-src"
    os.makedirs(work)
    write_file(os.path.join(work, "hello.txt"), PAYLOAD_FILE)
    manifest = {
        "version": 1,
        "spare_mib": 16,
        "files": [{"path": "OD8/hello.txt", "source": "hello.txt", "sha256": hashlib.sha256(PAYLOAD_FILE).hexdigest()}],
    }
    with open(os.path.join(work, "m.json"), "w") as f:
        json.dump(manifest, f)
    raw = base + ".img"
    proc = subprocess.run(
        [tool, "make-data-disk", os.path.join(work, "m.json"), raw, "--nonce", PAYLOAD_NONCE],
        capture_output=True, text=True, check=False,
    )
    if proc.returncode != 0 or not os.path.isfile(raw):
        # Бинарь есть, но не отработал: это ошибка теста, а не повод переключиться на hdiutil.
        raise RuntimeError(
            "%s make-data-disk упал (код %s): %s" % (tool, proc.returncode, (proc.stderr or proc.stdout).strip())
        )
    return raw


def make_fat_image(base, populate):
    """GPT-обёртка hdiutil с FAT (MS-DOS) - только для теста; возвращает путь raw-образа."""
    dmg = base + ".dmg"
    hdiutil("create", "-size", "48m", "-layout", "GPTSPUD", "-fs", "MS-DOS", "-volname", "OD8TEST",
            "-type", "UDIF", "-ov", base)
    mnt = base + "-mnt"
    os.makedirs(mnt)
    hdiutil("attach", dmg, "-mountroot", mnt, "-nobrowse")
    try:
        (vol,) = os.listdir(mnt)
        populate(os.path.join(mnt, vol))
    finally:
        subprocess.run(["hdiutil", "detach", os.path.join(mnt, vol), "-force"], capture_output=True)
    hdiutil("convert", dmg, "-format", "UDTO", "-o", base)
    raw = base + ".img"
    os.rename(base + ".cdr", raw)
    os.remove(dmg)
    return raw


def tree_digest(root):
    out = {}
    for d, _, names in os.walk(root):
        for n in names:
            p = os.path.join(d, n)
            out[os.path.relpath(p, root)] = sha(p)
    return out


@unittest.skipUnless(sys.platform == "darwin", "нужны hdiutil/diskutil (macOS)")
class CollectImage(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.mkdtemp(prefix="od8-img-")
        try:
            cls.target = make_fat_image(os.path.join(cls.tmp, "target"), windows_tree)
            cls.payload_hdiutil = make_fat_image(
                os.path.join(cls.tmp, "payload-hdiutil"),
                lambda root: write_file(os.path.join(root, "OD8", "markers.txt"), b"firstlogon nonce=1\n"),
            )
            cls.empty = make_fat_image(
                os.path.join(cls.tmp, "empty"), lambda root: write_file(os.path.join(root, "x.txt"), b"x")
            )
            cls.tool = find_image_tool()
            # Бинарь есть, а make-data-disk упал: RuntimeError НЕ ловится ниже - класс падает.
            cls.payload_writer = (
                make_payload_with_writer(cls.tool, os.path.join(cls.tmp, "payload-writer")) if cls.tool else None
            )
        except (subprocess.CalledProcessError, OSError) as e:
            shutil.rmtree(cls.tmp, ignore_errors=True)
            raise unittest.SkipTest("не удалось собрать тестовый образ: %s" % e)
        except BaseException:
            shutil.rmtree(cls.tmp, ignore_errors=True)
            raise

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def run_script(self, *args):
        return subprocess.run(
            [sys.executable, "-I", SCRIPT, *args], capture_output=True, text=True, check=False
        )

    def test_collect_is_readonly_and_idempotent(self):
        # Образ полезной нагрузки собран hdiutil: проверяется только чтение скриптом.
        print("writer=hdiutil", file=sys.stderr)
        self._check_collect(self.payload_hdiutil, False, "t1")

    def test_collect_payload_from_od_image_tool(self):
        if self.payload_writer is None:
            self.skipTest(
                "od-image-tool не собран (target/debug/od-image-tool или OD8_IMAGE_TOOL): "
                "образ нашего писателя не проверен; cargo build -p od-image-tool --offline"
            )
        print("writer=od-image-tool", file=sys.stderr)
        self._check_collect(self.payload_writer, True, "t1w")

    def _check_collect(self, payload, payload_by_writer, run_id):
        # nonce.txt, пустой markers.txt и файл манифеста пишет наш писатель.
        payload_files = {"nonce.txt", "markers.txt", "hello.txt"} if payload_by_writer else {"markers.txt"}
        out_root = os.path.join(self.tmp, "out-" + run_id)
        before = {p: sha(p) for p in (self.target, payload)}
        args = ["--run-id", run_id, "--target-image", self.target, "--payload-image", payload,
                "--out-root", out_root, "--hash-images", "--json"]
        r1 = self.run_script(*args)
        self.assertEqual(r1.returncode, 0, r1.stderr + r1.stdout)
        logs = os.path.join(out_root, run_id, "logs")
        first = tree_digest(logs)
        summary = json.loads(r1.stdout)
        self.assertEqual(summary["file_count"], 9 + len(payload_files))
        self.assertEqual(first["Windows/Panther/setupact.log".replace("/", os.sep)], hashlib.sha256(b"act\n").hexdigest())
        payload_on_disk = {
            os.path.basename(p).lower() for p in first if p.startswith("payload" + os.sep)
        }
        self.assertEqual(payload_on_disk, payload_files)
        if payload_by_writer:
            hello = [p for p in first if p.lower().endswith("hello.txt")]
            self.assertEqual(first[hello[0]], hashlib.sha256(PAYLOAD_FILE).hexdigest())
            nonce = [p for p in first if p.lower().endswith("nonce.txt")]
            self.assertEqual(first[nonce[0]], hashlib.sha256(PAYLOAD_NONCE.encode()).hexdigest())
        self.assertEqual(len(summary["setuperr_errors"]["Windows/Panther/setuperr.log"]), 2)
        for rec in summary["images"]:
            self.assertTrue(rec["attached"])
            self.assertTrue(rec["detached"])
            self.assertTrue(rec["image_unchanged"])
            self.assertEqual(rec["before"], rec["after"])
        self.assertEqual(before, {p: sha(p) for p in before})
        # Образы не остались подключёнными.
        self.assertNotIn(os.path.realpath(self.target), wcl.attached_image_paths())
        # Повторный запуск: тот же результат, без дублей.
        r2 = self.run_script(*args)
        self.assertEqual(r2.returncode, 0, r2.stderr + r2.stdout)
        self.assertEqual(tree_digest(logs), first)
        self.assertEqual(json.loads(r2.stdout), summary)
        self.assertEqual(os.listdir(os.path.join(out_root, run_id)), ["logs"])
        self.assertEqual(before, {p: sha(p) for p in before})

    def test_text_summary(self):
        r = self.run_script("--run-id", "t2", "--target-image", self.target,
                            "--out-root", os.path.join(self.tmp, "out2"))
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn(hashlib.sha256(b"act\n").hexdigest(), r.stdout)
        self.assertIn("Failed to apply driver", r.stdout)
        self.assertIn("Windows/Panther/setuperr.log", r.stdout)

    def test_volume_without_windows_reports_and_detaches(self):
        before = sha(self.empty)
        r = self.run_script("--run-id", "t3", "--target-image", self.empty,
                            "--out-root", os.path.join(self.tmp, "out3"))
        self.assertEqual(r.returncode, wcl.EXIT_NO_WINDOWS, r.stderr + r.stdout)
        self.assertIn("не содержат каталога Windows", r.stdout)
        self.assertEqual(sha(self.empty), before)
        self.assertNotIn(os.path.realpath(self.empty), wcl.attached_image_paths())

    @unittest.skip(
        "NTFS не проверен: на хосте нет средства собрать NTFS-образ (mkntfs), реального образа Windows нет; "
        "проверка монтирования NTFS средствами macOS выполняется вручную на настоящем образе S7-32"
    )
    def test_ntfs_mount_by_macos(self):
        pass


if __name__ == "__main__":
    unittest.main()
