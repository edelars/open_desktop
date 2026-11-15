"""Fingerprint of a `cargo fmt --check` log for the S4-44 local set.

Usage: python3 -I fmt_fingerprint.py <root> <log>

Prints one line per file with a rustfmt diff, sorted by path:
    <path relative to root> <diff blocks> <changed +/- lines> <sha256[:16] of the changed lines>

The local set compares these lines with its expected list, so a NEW divergence
inside an already known file (more blocks, more or different changed lines) is
not mistaken for the known one. Paths are made relative to <root>.
"""

import hashlib
import re
import sys

root, log = sys.argv[1].rstrip("/") + "/", sys.argv[2]
ansi = re.compile(r"\x1b\[[0-9;]*m|\x0f")
current, files = None, {}
for raw in open(log, encoding="utf-8", errors="replace"):
    line = ansi.sub("", raw.rstrip("\n"))
    match = re.match(r"^Diff in (.*):\d+:?$", line)
    if match:
        name = match.group(1)
        name = name[len(root):] if name.startswith(root) else name
        name = name.replace("/tests/../tests/", "/tests/")
        current = files.setdefault(name, {"blocks": 0, "lines": []})
        current["blocks"] += 1
    elif current is not None and line[:1] in ("+", "-"):
        current["lines"].append(line)
for name in sorted(files):
    entry = files[name]
    digest = hashlib.sha256("\n".join(entry["lines"]).encode()).hexdigest()[:16]
    print(f"{name} {entry['blocks']} {len(entry['lines'])} {digest}")
