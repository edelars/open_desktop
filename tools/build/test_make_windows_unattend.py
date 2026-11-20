#!/usr/bin/env python3
"""Host-side tests for the Stage 8 answer-file generator (S8-22).

Run: python3 -I tools/build/test_make_windows_unattend.py
Windows itself is not involved: the tests parse the generated XML and check that
the parameters land where the generator documents them. That Windows Setup accepts
the file is checked on S8-24, not here.
"""

from __future__ import annotations

import hashlib
import importlib.util
import re
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/build/make_windows_unattend.py"
FIRSTLOGON = ROOT / "guest/windows/tests/firstlogon.ps1"
NS = {"u": "urn:schemas-microsoft-com:unattend", "wcm": "http://schemas.microsoft.com/WMIConfig/2002/State"}
PASSWORD = "S3cret-Pass_for-test-only!"


def load_module():
    spec = importlib.util.spec_from_file_location("make_windows_unattend", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    sys.modules["make_windows_unattend"] = module
    spec.loader.exec_module(module)
    return module


gen = load_module()


def run_cli(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run([sys.executable, "-I", str(SCRIPT), *args], capture_output=True, text=True, check=False)


class GeneratorTestBase(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.tmp = Path(self._tmp.name)
        self.password_file = self.tmp / "secret"
        self.password_file.write_text(PASSWORD + "\n", encoding="utf-8")

    def generate(self, *extra: str, name: str = "out.xml") -> tuple[subprocess.CompletedProcess[str], Path]:
        out = self.tmp / name
        result = run_cli("--output", str(out), "--password-file", str(self.password_file), *extra)
        return result, out

    def parse(self, *extra: str) -> ET.Element:
        result, out = self.generate(*extra)
        self.assertEqual(result.returncode, 0, result.stderr)
        return ET.parse(out).getroot()


def settings(root: ET.Element, pass_name: str) -> ET.Element | None:
    for node in root.findall("u:settings", NS):
        if node.get("pass") == pass_name:
            return node
    return None


def component(root: ET.Element, pass_name: str, name: str) -> ET.Element:
    node = settings(root, pass_name)
    assert node is not None, pass_name
    for comp in node.findall("u:component", NS):
        if comp.get("name") == name:
            return comp
    raise AssertionError(f"component {name} not found in {pass_name}")


class StructureTest(GeneratorTestBase):
    def test_xml_parses_with_expected_passes_and_language(self) -> None:
        root = self.parse()
        self.assertEqual(root.tag, "{urn:schemas-microsoft-com:unattend}unattend")
        self.assertEqual([s.get("pass") for s in root.findall("u:settings", NS)], ["windowsPE", "specialize", "oobeSystem"])
        for pass_name, name in (
            ("windowsPE", "Microsoft-Windows-International-Core-WinPE"),
            ("oobeSystem", "Microsoft-Windows-International-Core"),
        ):
            intl = component(root, pass_name, name)
            self.assertEqual(intl.findtext("u:UILanguage", namespaces=NS), "en-US")
            self.assertEqual(intl.findtext("u:UserLocale", namespaces=NS), "en-US")
            self.assertEqual(intl.findtext("u:SystemLocale", namespaces=NS), "en-US")
            self.assertEqual(intl.findtext("u:InputLocale", namespaces=NS), "0409:00000409")

    def test_default_disk_index_and_partitions(self) -> None:
        root = self.parse()
        setup = component(root, "windowsPE", "Microsoft-Windows-Setup")
        self.assertEqual(setup.findtext("u:DiskConfiguration/u:Disk/u:DiskID", namespaces=NS), "0")
        self.assertEqual(setup.findtext("u:DiskConfiguration/u:Disk/u:WillWipeDisk", namespaces=NS), "true")
        types = [p.findtext("u:Type", namespaces=NS) for p in setup.findall(".//u:CreatePartition", NS)]
        self.assertEqual(types, ["EFI", "MSR", "Primary"])
        os_image = setup.find("u:ImageInstall/u:OSImage", NS)
        self.assertEqual(os_image.findtext("u:InstallFrom/u:MetaData/u:Key", namespaces=NS), "/IMAGE/INDEX")
        self.assertEqual(os_image.findtext("u:InstallFrom/u:MetaData/u:Value", namespaces=NS), "6")
        self.assertEqual(os_image.findtext("u:InstallTo/u:DiskID", namespaces=NS), "0")
        self.assertEqual(os_image.findtext("u:InstallTo/u:PartitionID", namespaces=NS), "3")
        self.assertIsNone(os_image.find("u:InstallFrom/u:Path", NS))
        self.assertEqual(setup.findtext("u:UserData/u:AcceptEula", namespaces=NS), "true")

    def test_disk_id_size_and_image_index_parameters(self) -> None:
        root = self.parse("--disk-id", "2", "--disk-size-gib", "80", "--image-index", "4", "--image-path", "install.swm")
        setup = component(root, "windowsPE", "Microsoft-Windows-Setup")
        self.assertEqual(setup.findtext("u:DiskConfiguration/u:Disk/u:DiskID", namespaces=NS), "2")
        self.assertEqual(setup.findtext("u:ImageInstall/u:OSImage/u:InstallTo/u:DiskID", namespaces=NS), "2")
        self.assertEqual(setup.findtext("u:ImageInstall/u:OSImage/u:InstallFrom/u:MetaData/u:Value", namespaces=NS), "4")
        self.assertEqual(setup.findtext("u:ImageInstall/u:OSImage/u:InstallFrom/u:Path", namespaces=NS), "install.swm")
        check = setup.findtext("u:RunSynchronous/u:RunSynchronousCommand/u:Path", namespaces=NS)
        self.assertIn("Disk 2 ", check)
        self.assertIn("80 GB", check)

    def test_size_check_runs_in_windowspe_and_shuts_down_on_mismatch(self) -> None:
        root = self.parse()
        setup = component(root, "windowsPE", "Microsoft-Windows-Setup")
        commands = setup.findall("u:RunSynchronous/u:RunSynchronousCommand", NS)
        self.assertEqual(len(commands), 1)
        self.assertEqual(commands[0].findtext("u:Order", namespaces=NS), "1")
        path = commands[0].findtext("u:Path", namespaces=NS)
        self.assertIn("diskpart", path)
        self.assertTrue(path.rstrip('"').endswith("|| wpeutil shutdown"), path)
        self.assertIn("Disk 0  *Online  *64 GB ", path)

    def test_size_check_pattern_matches_diskpart_output(self) -> None:
        # findstr has no "+": emulate its subset (. * literal) with re for the shipped pattern.
        path = gen.size_check_command(0, 64)
        pattern = re.search(r'/c:"([^"]+)"', path).group(1)
        regex = re.compile(pattern)
        self.assertTrue(regex.search("  Disk 0    Online           64 GB      0 B        *"))
        self.assertFalse(regex.search("  Disk 0    Online          164 GB      0 B        *"))
        self.assertFalse(regex.search("  Disk 0    Online           65 GB      0 B        *"))
        self.assertFalse(regex.search("  Disk 01   Online           64 GB      0 B        *"))
        self.assertFalse(regex.search("  Disk 1    Online           64 GB      0 B        *"))

    def test_autologon_and_firstlogon(self) -> None:
        root = self.parse("--payload-label", "OD8TEST")
        shell = component(root, "oobeSystem", "Microsoft-Windows-Shell-Setup")
        self.assertEqual(shell.findtext("u:AutoLogon/u:LogonCount", namespaces=NS), "1")
        self.assertEqual(shell.findtext("u:AutoLogon/u:Enabled", namespaces=NS), "true")
        self.assertEqual(shell.findtext("u:AutoLogon/u:Username", namespaces=NS), "od8")
        self.assertEqual(shell.findtext("u:AutoLogon/u:Password/u:Value", namespaces=NS), PASSWORD)
        self.assertEqual(shell.findtext("u:UserAccounts/u:LocalAccounts/u:LocalAccount/u:Group", namespaces=NS), "Administrators")
        commands = shell.findall("u:FirstLogonCommands/u:SynchronousCommand", NS)
        lines = [c.findtext("u:CommandLine", namespaces=NS) for c in commands]
        self.assertEqual([c.findtext("u:Order", namespaces=NS) for c in commands], ["1", "2"])
        self.assertIn("/v AutoLogonCount /t REG_DWORD /d 0", lines[0])
        self.assertIn("-FileSystemLabel 'OD8TEST'", lines[1])
        # DriveLetter is a Char: "$v.DriveLetter + ':\\...'" does not concatenate in PowerShell, so the path is built with -f.
        self.assertIn("powershell.exe -NoProfile -ExecutionPolicy Bypass -File ('{0}:\\OD8\\firstlogon.ps1' -f $v.DriveLetter)", lines[1])
        self.assertIsNone(re.search(r"DriveLetter\s*\+|\+\s*\$\w+\.DriveLetter", lines[1]), lines[1])
        for line in lines:
            self.assertNotIn(PASSWORD, line)
        for flag in ("HideEULAPage", "HideOnlineAccountScreens", "HideWirelessSetupInOOBE"):
            self.assertEqual(shell.findtext(f"u:OOBE/u:{flag}", namespaces=NS), "true")
        self.assertEqual(shell.findtext("u:OOBE/u:ProtectYourPC", namespaces=NS), "3")

    def test_device_encryption_modes(self) -> None:
        root = self.parse()
        node = component(root, "specialize", "Microsoft-Windows-SecureStartup-FilterDriver")
        self.assertEqual(node.findtext("u:PreventDeviceEncryption", namespaces=NS), "true")
        allowed = self.parse("--device-encryption", "allow")
        self.assertIsNone(settings(allowed, "specialize"))

    def test_no_product_key_or_bypass_in_output(self) -> None:
        result, out = self.generate()
        self.assertEqual(result.returncode, 0, result.stderr)
        text = out.read_text(encoding="utf-8")
        lowered = text.lower()
        for forbidden in ("ProductKey", "LabConfig", "BypassTPMCheck", "BypassSecureBootCheck", "BypassCPUCheck", "BypassNRO", "nonce",
                          "testsigning", "nointegritychecks"):
            self.assertNotIn(forbidden.lower(), lowered)


class DeterminismAndSecretTest(GeneratorTestBase):
    def test_same_inputs_same_bytes_and_printed_hash_matches(self) -> None:
        first, out1 = self.generate(name="one.xml")
        second, out2 = self.generate(name="two.xml")
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(out1.read_bytes(), out2.read_bytes())
        digest = hashlib.sha256(out1.read_bytes()).hexdigest()
        self.assertIn(f"sha256: {digest}", first.stdout)
        self.assertEqual(re.findall(r"sha256: ([0-9a-f]{64})", second.stdout), [digest])

    def test_different_password_changes_hash(self) -> None:
        _, out1 = self.generate(name="one.xml")
        self.password_file.write_text(PASSWORD + "x", encoding="utf-8")
        _, out2 = self.generate(name="two.xml")
        self.assertNotEqual(hashlib.sha256(out1.read_bytes()).hexdigest(), hashlib.sha256(out2.read_bytes()).hexdigest())

    def test_password_not_in_stdout_or_stderr(self) -> None:
        result, out = self.generate()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn(PASSWORD, result.stdout)
        self.assertNotIn(PASSWORD, result.stderr)
        self.assertIn(PASSWORD, out.read_text(encoding="utf-8"))
        self.assertIn("PreventDeviceEncryption", result.stdout)  # the encryption note is printed

    def test_trailing_newline_of_password_file_is_stripped(self) -> None:
        self.assertEqual(gen.read_password(self.password_file), PASSWORD)
        self.password_file.write_bytes((PASSWORD + "\r\n").encode())
        self.assertEqual(gen.read_password(self.password_file), PASSWORD)

    def test_xml_special_characters_in_password_are_escaped(self) -> None:
        self.password_file.write_text('a<b>&"c\'', encoding="utf-8")
        root = self.parse()
        shell = component(root, "oobeSystem", "Microsoft-Windows-Shell-Setup")
        self.assertEqual(shell.findtext("u:AutoLogon/u:Password/u:Value", namespaces=NS), 'a<b>&"c\'')


class ErrorTest(GeneratorTestBase):
    def test_password_with_xml_invalid_characters_is_rejected(self) -> None:
        for bad in ("Zq\x01Kw", "Zq\x1fKw", "Zq" + chr(0xFFFE) + "Kw", "Zq" + chr(0xFFFF) + "Kw"):
            self.password_file.write_bytes(bad.encode("utf-8"))
            result, out = self.generate()
            self.assertEqual(result.returncode, 2, repr(bad))
            self.assertFalse(out.exists(), repr(bad))
            self.assertIn("XML 1.0", result.stderr)
            self.assertNotIn("Zq", result.stderr + result.stdout)  # password is not echoed
            self.assertNotIn("Kw", result.stderr + result.stdout)
        with self.assertRaises(gen.UnattendError):
            gen.validate_options(gen.Options(password="ab" + chr(0xD800) + "cd"))
        # tab is a valid XML 1.0 character
        gen.validate_options(gen.Options(password="ab\tcd"))

    def test_missing_and_empty_password_file(self) -> None:
        missing = run_cli("--output", str(self.tmp / "o.xml"), "--password-file", str(self.tmp / "nope"))
        self.assertEqual(missing.returncode, 2)
        self.assertIn("error:", missing.stderr)
        self.password_file.write_text("\n", encoding="utf-8")
        result, out = self.generate()
        self.assertEqual(result.returncode, 2)
        self.assertFalse(out.exists())

    def test_invalid_parameters_are_rejected(self) -> None:
        for bad in (("--disk-id", "-1"), ("--disk-size-gib", "0"), ("--disk-size-gib", "2048"), ("--image-index", "0"),
                    ("--payload-label", "BAD LABEL"), ("--payload-label", "WAYTOOLONGLABEL"), ("--user", "a b")):
            result, out = self.generate(*bad)
            self.assertEqual(result.returncode, 2, bad)
            self.assertFalse(out.exists(), bad)

    def test_output_inside_repository_outside_work_is_refused(self) -> None:
        target = ROOT / "tools/build/should_not_exist_autounattend.xml"
        result = run_cli("--output", str(target), "--password-file", str(self.password_file))
        self.assertEqual(result.returncode, 2)
        self.assertFalse(target.exists())
        self.assertIn("work/", result.stderr)

    def test_case_variant_of_repository_path_is_still_inside_the_repository(self) -> None:
        # APFS is case-insensitive by default: <ROOT in other case>/x must not slip past the check.
        base = Path(self._tmp.name).resolve()
        repo = base / "repo"
        (repo / "work").mkdir(parents=True)
        (repo / "docs").mkdir()
        if not (base / "REPO").exists():
            self.skipTest("file system is case-sensitive")
        saved = (gen.ROOT, gen.WORK_DIR)
        gen.ROOT, gen.WORK_DIR = repo, repo / "work"
        try:
            for bad in (base / "REPO" / "docs" / "o.xml", base / "repo" / "DOCS" / "o.xml", base / "Repo" / "new_dir" / "o.xml"):
                with self.assertRaises(gen.UnattendError, msg=str(bad)):
                    gen._check_not_in_git_tree(bad, "output")
            for good in (base / "REPO" / "WORK" / "o.xml", base / "repo" / "Work" / "sub" / "o.xml", base / "elsewhere" / "o.xml"):
                gen._check_not_in_git_tree(good, "output")
        finally:
            gen.ROOT, gen.WORK_DIR = saved

    def test_help(self) -> None:
        result = run_cli("--help")
        self.assertEqual(result.returncode, 0)
        self.assertIn("--password-file", result.stdout)


class FirstLogonScriptStaticTest(unittest.TestCase):
    """Static checks only: no PowerShell is available on the host."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.text = FIRSTLOGON.read_text(encoding="utf-8")

    def test_marker_format_and_phases(self) -> None:
        self.assertIn("'OD8 {0} {1} {2} {3}'", self.text)
        for phase in ("'firstlogon'", "'boot'", "'shutdown'"):
            self.assertIn(phase, self.text)

    def test_task_and_shutdown(self) -> None:
        self.assertIn("$TaskName = 'OD8Boot'", self.text)
        self.assertIn("New-ScheduledTaskTrigger -AtStartup", self.text)
        self.assertIn("-UserId 'SYSTEM'", self.text)
        self.assertIn("shutdown.exe /s /t 0", self.text)
        self.assertIn("Flush($true)", self.text)

    def test_nonce_is_checked_strictly(self) -> None:
        # 128 bits as 32 lowercase hex digits; -cnotmatch (case-sensitive) and \z (no trailing LF).
        self.assertIn("-cnotmatch '^[0-9a-f]{32}\\z'", self.text)
        self.assertNotIn("\\S{1,256}", self.text)

    def test_drive_letter_is_never_concatenated(self) -> None:
        # DriveLetter is a Char: "+" with a string does not work in PowerShell; paths are built with -f.
        self.assertIsNone(re.search(r"DriveLetter\s*\+|\+\s*\$\w+\.DriveLetter", self.text))
        self.assertIn("('{0}:\\__DIR__\\__SCRIPT__' -f $v.DriveLetter)", self.text)

    def test_hash_uses_dotnet_cryptography(self) -> None:
        self.assertIn("[System.Security.Cryptography.SHA256]::Create()", self.text)
        for external in ("certutil", "Get-FileHash", "sha256sum"):
            self.assertNotIn(external, self.text)

    def test_no_secrets(self) -> None:
        lowered = self.text.lower()
        for word in ("password", "productkey", "product key", "labconfig"):
            self.assertNotIn(word, lowered)

    def test_braces_and_parentheses_balanced_outside_strings(self) -> None:
        text = re.sub(r"<#.*?#>", "", self.text, flags=re.S)
        text = re.sub(r"@'\n.*?\n'@", "", text, flags=re.S)
        text = re.sub(r'"(?:[^"\\\n]|\\.|\x60.)*"', '""', text)
        text = re.sub(r"'[^'\n]*'", "''", text)
        text = re.sub(r"#[^\n]*", "", text)
        for opener, closer in (("{", "}"), ("(", ")"), ("[", "]")):
            self.assertEqual(text.count(opener), text.count(closer), opener)


if __name__ == "__main__":
    unittest.main()
