#!/usr/bin/env python3
"""Check repository placement against docs/PROJECT_LAYOUT.md (Python 3.9+)."""

from pathlib import Path, PurePosixPath
import re
import sys


ROOT = Path(__file__).resolve().parents[1]
LAYOUT = ROOT / "docs" / "PROJECT_LAYOUT.md"


def read_inventory(document, name):
    pattern = (
        rf"<!-- {re.escape(name)}:start -->\s*"
        r"```text\n(.*?)\n```\s*"
        rf"<!-- {re.escape(name)}:end -->"
    )
    matches = re.findall(pattern, document, re.DOTALL)
    if len(matches) != 1:
        raise ValueError(f"Expected exactly one inventory: {name}")
    entries = [line.strip() for line in matches[0].splitlines() if line.strip()]
    if len(set(entries)) != len(entries):
        raise ValueError(f"Duplicate entries in inventory: {name}")
    for entry in entries:
        path = PurePosixPath(entry)
        if path.is_absolute() or ".." in path.parts or str(path) != entry:
            raise ValueError(f"Invalid project path: {entry}")
    return entries


def check():
    document = LAYOUT.read_text(encoding="utf-8")
    directories = read_inventory(document, "required-directories")
    allowed_root_directories = read_inventory(document, "allowed-root-directories")
    root_files = read_inventory(document, "root-files")
    errors = []

    for directory in directories:
        path = ROOT / directory
        if not path.is_dir():
            errors.append(f"Missing directory: {directory}")
        elif path.resolve() != path.absolute():
            errors.append(f"Required directory must not use a symlink: {directory}")

    for name in root_files:
        if len(PurePosixPath(name).parts) != 1:
            errors.append(f"Root file inventory contains a nested path: {name}")
        elif not (ROOT / name).is_file():
            errors.append(f"Missing root file: {name}")

    components = {PurePosixPath(path).parts[0] for path in directories}
    allowed_directories = components | set(allowed_root_directories) | {".git", "target", "build", "dist"}
    for path in sorted(ROOT.iterdir()):
        if path.is_dir() and path.name not in allowed_directories:
            errors.append(f"Unmapped root directory: {path.name}")
        elif path.is_file() and path.name != ".DS_Store" and path.name not in root_files:
            errors.append(f"Unmapped root file: {path.name}")

    scopes = sorted(components | {"guest/windows", "guest/linux"})
    for scope in scopes:
        if not (ROOT / scope / "AGENTS.md").is_file():
            errors.append(f"Missing scoped instructions: {scope}/AGENTS.md")

    crate_names = {
        PurePosixPath(path).parts[1]
        for path in directories
        if PurePosixPath(path).parts[0] == "crates"
    }
    for name in sorted(crate_names):
        for suffix in ["Cargo.toml", "README.md", "src/lib.rs"]:
            if not (ROOT / "crates" / name / suffix).is_file():
                errors.append(f"Missing crate scaffold: crates/{name}/{suffix}")
    crates_root = ROOT / "crates"
    if crates_root.is_dir():
        for path in sorted(crates_root.iterdir()):
            if path.is_dir() and path.name not in crate_names:
                errors.append(f"Unmapped crate directory: crates/{path.name}")

    # Deliberately limited to local Markdown paths; not a Markdown or HTTP linter.
    markdown_files = [ROOT / "README.md", ROOT / "AGENTS.md"]
    for component in sorted(components - {"work", "outputs"}):
        markdown_files.extend((ROOT / component).rglob("*.md"))
    for path in markdown_files:
        if not path.is_file():
            continue
        for target in re.findall(r"\]\(([^)]+)\)", path.read_text(encoding="utf-8")):
            if ":" in target or target.startswith("#"):
                continue
            local = target.split("#", 1)[0]
            if not local:
                continue
            destination = (path.parent / local).resolve()
            if not destination.is_relative_to(ROOT):
                errors.append(f"Local link escapes repository: {path.relative_to(ROOT)} -> {target}")
            elif not destination.exists():
                errors.append(f"Broken local link: {path.relative_to(ROOT)} -> {target}")

    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print(
        f"Layout OK: {len(directories)} required directories, "
        f"{len(crate_names)} crate scaffolds, {len(scopes) + 1} instruction scopes."
    )
    return 0


if __name__ == "__main__":
    try:
        sys.exit(check())
    except (OSError, ValueError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        sys.exit(1)
