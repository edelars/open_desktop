# CPU fixture and result format v1

**Status:** implemented in `od-testkit` and used by the Stage 1 local and physical-x86 runners; see [CPU_ORACLE.md](CPU_ORACLE.md).

## JSON rules and limits

- UTF-8 JSON, schema version `1`; unknown object fields are rejected.
- Tagged observations and comparison scopes follow the same strict rule. `unsupported` cannot carry ignored regions, and an `available` observation cannot also carry an `unavailable` reason.
- A fixture is limited to 65,536 input bytes and a result to 262,144 output bytes. Each limit is checked before JSON parsing.
- A program is limited to 4,096 bytes and stored as contiguous lowercase hexadecimal in `program.bytes_hex`.
- `program.sha256` is SHA-256 over the decoded program bytes. The run result `input_sha256` is SHA-256 over the exact fixture JSON bytes that were loaded, including whitespace. Thus results from differently encoded input files cannot be silently compared as the same run.
- The eight bytes of each architectural value are represented by exactly `0x` and sixteen lowercase hexadecimal digits. JSON numbers and floating point values are invalid for architectural state. Seed, instruction budget, timeout and other nonarchitectural counts use JSON integers.
- Case IDs contain 1–128 ASCII letters, digits, `_` or `-`. Provenance text is required and bounded.
- Instruction budget is 1–10,000,000; wall timeout is 1–30,000 ms. They are independent limits. A timeout or process signal is a harness outcome, not a guest architectural exit.

## Fixture

Required fields:

| Field | Meaning |
|---|---|
| `schema_version` | Must be `1`. |
| `case_id` | Stable identifier for this case. |
| `profile` | `s0_09_register_subset_v1` for the first implemented profile. |
| `provenance` | `kind`, source path, project specification and its version. |
| `seed` | Optional unsigned 64-bit generator seed. |
| `program` | Lowercase `bytes_hex` and SHA-256 of the decoded bytes. |
| `initial_state` | RAX/RCX/RDX/RBX, byte-offset RIP and initial RFLAGS. |
| `instruction_budget` | Guest instruction bound for project-owned engines. |
| `wall_timeout_ms` | Host-side bound for an isolated native child. |
| `comparison` | Registers, RIP requirement, and exact mask of RFLAGS bits to compare. |
| `expected_exit` | `reached_code_end` or `instruction_budget_exhausted`. |

The initial profile requires nonempty code, an entry RIP inside the program, reserved RFLAGS bit 1 set, and arithmetic flags clear at entry. The comparison flag mask may include only CF/PF/AF/ZF/SF/OF. Register names are unique. Memory and exceptions are explicitly encoded as `{"status":"unsupported"}` for this profile; an empty region list or missing value must not be interpreted as a successful memory/exception test.

## Result

A result records `schema_version`, `case_id`, exact `input_sha256`, `profile`, backend (`interpreter`, `jit` or `native_x86`), completion status, expected and observed exit reason, instruction count observation, architectural snapshot, build metadata and machine metadata. Completion statuses distinguish completed, unsupported, timed out, signaled, harness error and invalid input.

Each snapshot field is an explicit observation:

```json
{"status":"available","value":"0x0000000000000000"}
{"status":"unavailable","reason":"not exposed by this backend"}
```

Absence is never a zero value. A completed result must have GPR, RIP, RFLAGS and exit reason available. Interpreter and JIT results also require an instruction count. Native x86 may report that count as unavailable when the hardware run is not instrumented; comparison includes the count only when both results provide it. Memory snapshots have a limit of 16 regions and 65,536 total bytes. Unknown architectural data stays unavailable; host signals do not imply a guest exception. Exception metadata has separate observations for error code and fault address.

The v1 loader validates schema version, IDs, program digest, numeric formats, profile restrictions and declared size limits before a fixture is accepted. `od-testkit` exposes strict load/save methods and the comparator; CLI execution is documented in [od-runner](../../tools/runner/README.md).

## Dependency record

`od-testkit` uses general-purpose libraries only for test infrastructure:

| Package | Version | Purpose | License | Features |
|---|---:|---|---|---|
| `serde` | 1.0.229 | Derive strict data model serialization | MIT OR Apache-2.0 | `derive` |
| `serde_json` | 1.0.151 | Bounded JSON format parsing and writing | MIT OR Apache-2.0 | default features (`std`) |
| `sha2` | 0.10.9 | SHA-256 for fixture program bytes and exact input files | MIT OR Apache-2.0 | default features (`std`) |

The dependencies are confined to `od-testkit`; `od-isa`, JIT, machine and runtime crates do not depend on them. Source is crates.io and resolved packages are pinned in [`Cargo.lock`](../../Cargo.lock). The resolved transitive graph contains only serialization/proc-macro and digest infrastructure: `serde_core`, `serde_derive`, `syn`, `proc-macro2`, `quote`, `unicode-ident`, `digest`, `crypto-common`, `block-buffer`, `generic-array`, `typenum`, `version_check`, `cpufeatures`, `libc`, `cfg-if`, `itoa`, `memchr` and `zmij`. Cargo metadata reports MIT, Apache-2.0, Unicode-3.0, or Unlicense terms for these packages. No transitive package provides a CPU decoder, emulator, JIT, or guest/device semantics.
