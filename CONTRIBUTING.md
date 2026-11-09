# Contributing to Open Desktop

Thank you for your interest in the project. Open Desktop is an in-house platform for running x86-64 guest operating systems on macOS/Apple Silicon: the decoder, interpreter, ARM64 JIT, machine model, firmware and drivers are written in this project rather than taken from existing implementations. Because of that, the code provenance rules here are stricter than in a typical open source project. Please read this document before you start.

The binding project rules live in [AGENTS.md](AGENTS.md). This document describes the process for external contributors and does not replace them. If the two disagree, `AGENTS.md` and the documents it references take precedence.

## TL;DR

1. Open an issue for any non-trivial change and wait until the scope is agreed.
2. Do not copy or port code, tables or algorithms from other emulators, hypervisors, firmware or JITs (see [Code provenance](#code-provenance-clean-room)).
3. One change is one verifiable vertical slice, with tests and updated documentation.
4. Sign the [Contributor License Agreement](#license-and-cla) on your first pull request.
5. Run the [required checks](#checks-before-opening-a-pr) and state in the PR what was verified and what was not.

## Project status

The project is at an early stage and progresses through the stages defined in the [roadmap](docs/ROADMAP.md) and the [stage plans](docs/plans/). The existence of a directory or a mention of a subsystem does not mean the whole subsystem is open for implementation. The most welcome contributions are:

- fixes for confirmed defects listed in [KNOWN_ISSUES](docs/KNOWN_ISSUES.md);
- tests, oracle scenarios and fixtures for already implemented behavior;
- corrections and clarifications to documentation and specifications;
- slices explicitly described in the current stage plan, after agreement in an issue.

Large architectural changes, new top-level components and new subsystems are discussed in an issue first and recorded as an ADR in [docs/decisions/](docs/decisions/).

## License and CLA

Open Desktop is licensed under the [GNU Affero General Public License v3.0 only](LICENSE) (`AGPL-3.0-only`). The Project Owner also offers the project under separate commercial licenses, together with paid services for businesses; see [Licensing](README.md#licensing).

To make this dual licensing possible, every contributor must accept the [Contributor License Agreement](CLA.md) before a pull request can be merged. You keep the copyright in your contribution and grant the Project Owner a license that includes the right to distribute it under the AGPL and under commercial terms. To sign, comment on your first pull request:

```text
I have read the Open Desktop CLA (version 1.0) and I agree to its terms.
```

If you contribute on behalf of an employer, name it in the comment and confirm that you are authorized to sign for it. Pull requests without an accepted CLA are not merged.

New source files start with an SPDX header in the comment syntax of the language:

```rust
// SPDX-License-Identifier: AGPL-3.0-only
```

## Code provenance (clean room)

Core components are implemented in-house only. The list and its boundaries are defined in the [dependency policy](docs/policies/DEPENDENCIES.md) and in the self-implementation section of [AGENTS.md](AGENTS.md):

- x86 semantics and decoder, interpreter, IR, ARM64 emitter and JIT runtime;
- guest MMU/TLB, memory model and device models;
- firmware/UEFI and TPM protocol logic;
- our own guest drivers and graphics translation;
- our own disk and snapshot formats.

For these components it is **forbidden** to:

- copy, translate into another language or paraphrase line by line code, opcode tables, test vectors or data structures from third-party projects, including but not limited to QEMU, Bochs, Box64, FEX-Emu, DOSBox, Unicorn, Capstone, Intel XED, Zydis, EDK2/OVMF, SeaBIOS, swtpm/libtpms, Wine, DXVK, MoltenVK and any proprietary product;
- use disassembled or decompiled code of proprietary products, or code obtained under NDA or from leaks;
- pull such implementations in as a dependency, including transitively or in tests only.

**Allowed and encouraged:**

- relying on public specifications (Intel SDM, AMD APM, Arm ARM, UEFI, ACPI, TCG TPM 2.0, PCI, public Microsoft/Linux documentation), citing the document, version and section in a comment, specification or report;
- checking behavior against real hardware and the project's own oracle tests (see [docs/testing/CPU_ORACLE.md](docs/testing/CPU_ORACLE.md));
- using stock guest OS components inside the guest without copying them into the repository.

If you have previously worked in depth with the source of one of the projects above in the same area, say so in the PR. This does not bar you from contributing, but the maintainer may ask you to pick a different task or request an extra review. Tables, fixtures and generators must have clear provenance: state which specification section or which of your own experiments they come from.

## Dependencies

General-purpose libraries (logging, CLI, serialization, collections, testing, etc.) are allowed under the [dependency policy](docs/policies/DEPENDENCIES.md). A PR that adds a dependency must state its purpose, version, source, license, enabled features and significant transitive dependencies. Do not add packages "for later". `Cargo.toml` and `Cargo.lock` change together.

Because the project is distributed both under the AGPL and under commercial licenses, a dependency must use a permissive license that allows both: MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unlicense, Unicode-3.0 or an equivalent. Copyleft dependencies (GPL, LGPL, AGPL, MPL, EUPL and similar) require prior approval from the maintainer.

## Workflow

1. **Issue.** Describe the problem or slice, the expected behavior, the affected components and how it will be verified. For bugs, include reproduction steps, macOS/Rust versions and hardware configuration.
2. **Placement.** Find the right directory in the [project layout](docs/PROJECT_LAYOUT.md) and read every `AGENTS.md` from the root down to that directory. Do not create root-level `src/`, `lib/`, `common/`, `utils/` or similar directories.
3. **Branch.** Branch off an up-to-date `main` and name it `<type>/<slice-id-or-short-name>`, e.g. `feat/s4-33d-cross-page` or `fix/mmu-tlb-flush`.
4. **Implementation.** One vertical slice: code, tests and documentation together. Do not present a skeleton, stub or unsupported fallback as an implemented feature, and do not write tests that pass without the functionality they claim to check.
5. **Pull request.** Small and focused. Fill in the description using the [template](#pr-description).
6. **Review.** A maintainer approval is required. The reviewer checks the diff, affected contracts, code provenance and the stated checks. Push fixes as separate commits; rewrite history only when the reviewer asks.
7. **Merge.** Done by a maintainer.

## Code and documentation style

- Names, interfaces, source code, comments and contributor-facing documentation are written in English. Some existing internal design documents are in Russian; when editing one, follow the language of that file.
- Rust stable, edition 2024, formatted with `rustfmt` with no local overrides. Swift/SwiftUI/AppKit belongs in `apps/` only.
- SOLID, KISS and DRY apply to real system boundaries: no speculative abstractions, frameworks or generators.
- A host panic, ABI violation or memory corruption is not a guest exception. Do not hide them behind blanket error handling.
- `unsafe` is allowed only with a `// SAFETY:` comment explaining the invariants being upheld.

## Performance

Any speed-up claim in a performance-critical area (JIT, MMU, interpreter hot paths, etc.) must be backed by reproducible measurements following the performance section of [AGENTS.md](AGENTS.md) and [docs/testing/BENCHMARKS.md](docs/testing/BENCHMARKS.md). Report the scenario, hardware, software versions, build settings, number of runs and variance. Without data the change is a hypothesis. Speed-ups that break guest semantics are not accepted.

## Tests

- Unit tests live next to the implementation; cross-component scenarios and small in-house fixtures live in `tests/`.
- Subsystem testing guides are in [docs/testing/](docs/testing/), e.g. [ADDING_ISA_GROUP.md](docs/testing/ADDING_ISA_GROUP.md) for new instruction groups.
- Verify in proportion to the change: a documentation edit does not need CPU tests, a CPU semantics change does.

## Checks before opening a PR

```sh
python3 tools/check_layout.py
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked --offline
cargo test -p <affected-crate> --locked --offline
```

Add the subsystem-specific checks from `docs/testing/`. A successful build does not prove that a feature works.

## Commits

Format: `<type>: <what changed>` in English, no trailing period. Put the slice ID, if any, in parentheses at the end.

| Type | Purpose |
| --- | --- |
| `feat` | new verifiable functionality |
| `fix` | defect fix |
| `test` | tests, fixtures, oracles only |
| `docs` | documentation, plans, reports only |
| `proto` | changes to normative formats in `protocols/` |
| `chore` | tooling, build, maintenance |

Example: `feat: CR0.WP write rights and #PF repair/retry case on the own kernel (S4-33c)`.

## Documenting results

- Implemented behavior, its limits and how it was verified are recorded in the relevant specification under `docs/specs/` or a report under `docs/reports/`. A PR description is not a substitute for documentation.
- Problems found but not fixed are recorded in [docs/KNOWN_ISSUES.md](docs/KNOWN_ISSUES.md): component, description, impact, reproduction, status and next step. When an issue is fixed, update its entry with the verification result.
- Directory structure changes come with an update to [PROJECT_LAYOUT.md](docs/PROJECT_LAYOUT.md).

## What must not be committed

Guest OS images, large disk images, proprietary binaries (including Windows, third-party drivers and firmware), keys, signing certificates, secrets, user data and build outputs. Use `work/` (ignored by Git) for intermediate data.

## AI-assisted contributions

AI assistants may be used, but the author is responsible for the contribution:

- you understand every line of the change and can explain it during review;
- generated code is subject to the same provenance rules: the model must not reproduce code from forbidden projects;
- give agents [AGENTS.md](AGENTS.md) as context; the models and roles listed there describe the maintainer's internal process and are not required for external contributors;
- mention briefly in the PR description if the change was substantially prepared with AI assistance.

Bulk generated PRs without a prior issue are closed without review.

## PR description

```markdown
## What and why
<link to the issue / plan slice, summary of the change>

## Affected components and contracts
<crates/…, protocols/…, ABI/format changes>

## Checks
- [x] done: <commands and results>
- [ ] not verified / limitations: <what and why>

## Provenance
<specifications used (document, version, section); new dependencies and their licenses; AI assistance; CLA accepted: yes/no>

## Documentation
<updated specs/reports/KNOWN_ISSUES>
```

## Security

Report vulnerabilities (guest escape, host memory corruption, VMHost isolation bypass, vTPM issues) privately via [GitHub Security Advisories](https://github.com/edelars/open_desktop/security/advisories/new), not in a public issue.

## Conduct

Be respectful and keep discussions technical and on topic. The maintainer may hide comments, lock discussions and restrict participation when these norms are violated.
