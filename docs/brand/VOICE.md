# Voice and writing

Open Desktop writes like an engineer filing a test report: short sentences, exact words, evidence next to every claim. Product copy, the site and the app are in English. Project documents (plans, reports, specs) are in Russian and follow the same rules.

The figures in the examples below show the form of a sentence. They are not measurements; take real numbers from the stage reports.

## Rules

- State what is true now, with its condition. "Boots Debian 12 to a shell in 3.1 s on M3 Pro, median of 12 runs", not "boots fast".
- Use the roadmap's status words exactly: Accepted, In progress, Partial, Early work, Planned. A skeleton or a passing build is not "done".
- Name the check. "Checked instruction by instruction against a physical x86 machine", not "highly accurate".
- "We" is the project. "You" is the person using the app. Never "the user" in product copy.
- Sentence case for headings, buttons and menu items. Capitals only in the `label` style: status words, eyebrows and table heads.
- Numbers as digits with units and a space between thousands: 14 812 fixtures, 8 GiB, 2.41 s.
- Architecture names as written: x86-64, ARM64, Apple Silicon. Not "x64" in our own copy (only when quoting Microsoft), not "M-series".
- No superlatives and no hype words: blazing, seamless, magic, revolutionary, effortless.
- No exclamation marks and no emoji.

## Errors

Say what stopped, where, and what the person can do next.

| Write | Not |
| --- | --- |
| Windows stopped: triple fault at RIP 0xFFFFFFF0. The log has the last 64 instructions. | Oops! Something went wrong. |
| Disk image is read-only. Copy it to a writable folder, then start again. | Error 0x17. |
| This guest needs 2 vCPUs. Stage 6 (multiple CPUs) is planned, so the VM starts with 1. | Multi-core coming soon! |

## Status and progress

| Write | Not |
| --- | --- |
| Stage 1: Accepted, register-only profile. | Stage 1 is basically done. |
| Stage 2: In progress. The interpreter passes the register-only fixtures; memory fixtures are next. | The interpreter is nearly ready. |
| Not done until it works and has been verified. | Shipping soon. |

## Taglines

Use one per surface, in this order of preference:

- x86-64 ≡ ARM64. Checked instruction by instruction.
- Every instruction, accounted for.
- Run x86-64 Windows and Linux on Apple Silicon Macs.

The last one is the plain description and fits anywhere the first two would need explaining.
