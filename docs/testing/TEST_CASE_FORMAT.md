# CPU fixture and result formats

## Version 2: long-mode integer states

`od_testkit::v2` implements schema `2` with profiles `s2-long-integer-v1` and
`s2-long-fs-base-v1`, `s2-long-gs-base-v1`, `s2-long-fs-selector-v1` and
`s2-long-gs-selector-v1`, `s2-long-verw-gdt-v1`, and local-only
`s2-long-fs-load-v1`.
V1 readers, bytes and historical digests remain unchanged. Segment profiles are
profile-specific state extensions;
it does not reinterpret or rewrite existing integer-profile fixtures.

Fixture fields: `schema_version`, `case_id`, `profile`, `provenance`, optional
HexU64 `seed`, `converted_from_v1_sha256`, `initial_state`, `memory`, optional
`stop_rip`, `instruction_budget`, `wall_timeout_ms`, `rflags_mask`, `expected_exit`.
For `s2-long-integer-v1`, state contains `mode: long64`, exactly 16 HexU64 `gpr`
values in x86 encoding order (RAX, RCX, RDX, RBX, RSP, RBP, RSI, RDI, R8…R15),
architectural u64 RIP and RFLAGS. This profile retains its exact field set.
`s2-long-fs-base-v1` uses the same values plus required HexU64 `fs_base` in
`initial_state`; it must be canonical48. Its result snapshot must expose
`fs_base` as an available architectural value, and comparison requires it to
match the initial value. This field is not inferred from an absent field, and
the older profile's result/digest shape is unchanged. RFLAGS allow integer
arithmetic flags, DF and required reserved bit 1.
`s2-long-gs-base-v1` similarly requires canonical48 HexU64 `gs_base` and exposes
it in result snapshots; comparison requires it to match the initial value. The
local-only `s2-long-fs-selector-v1` profile requires canonical48 `fs_base` and
16-bit HexU64 `fs_selector` in initial and result state; both are compared with
their initial values. The local-only `s2-long-gs-selector-v1` profile likewise
requires canonical48 `gs_base` and 16-bit HexU64 `gs_selector` in initial and
result state. Each selector profile exposes only its own selector and base;
older and opposite-segment profile field sets remain unchanged.
The VERW profile requires `gdtr_base` and `gdtr_limit` in initial and result
state; the base and complete configured range must be canonical48. Older
profiles omit both fields. It compares both GDTR values and does not infer them
from absent fields.
The FS-load profile also requires canonical48 `fs_base`, a 16-bit visible
`fs_selector`, GDTR fields, and `fs_cache_limit`, `fs_cache_access`, and
`fs_cache_flags` in initial and result state. It compares all of them. Initial
cached FS state is explicit and need not equal current GDT contents; the
instruction selector is read from AX and must be non-null with TI=0 for a
supported load. Older
profiles omit all FS cache fields.
Unknown fields/versions/profiles/modes, numeric overflow and incompatible state
are rejected. Unmapped/noncanonical entry RIP is allowed as input to fault tests.

Code and data use the same guest mappings. `memory` reuses `MemoryRegionSpec`
with unique IDs, nonoverlapping checked addresses, lengths, read/write/execute
permissions, initial bytes and per-byte comparison masks. Limits: 16 regions,
64 KiB total RAM, 512 KiB fixture JSON, 256 KiB result JSON. Budget is
1…10000000 and wall timeout is 1…30000 ms. `stop_rip` is a harness boundary
checked before fetch, not an x86 instruction or a universal end-of-RAM exit.

Result fields include exact input SHA-256, case/profile/backend, build/machine
metadata, `outcome`, observed instruction count and snapshot. Outcomes separate
stop address, budget, stop request, architectural exception, unsupported,
timeout, host signal and harness failure. An architectural exit requires a
snapshot: full state, `rflags_defined`, memory and observed exception metadata.
`exception: available(value: null)` means observed no exception; unavailable
is distinct. Fault metadata includes vector, faulting RIP and observations of
error code/address. The current fault profile requires saved RIP to equal the
faulting RIP; trap delivery is not implemented. Required architectural error
codes and the page-fault address cannot be unavailable.

Comparison checks exact case/digest/profile/memory layout, all GPR, RIP,
definedness, requested defined flags, masked memory bytes, outcome/expected exit
and exception metadata. Incomplete/unsupported results cannot compare equal.
The FS-base and GS-base profiles additionally compare their observed base; the
selector profiles compare the matching segment base and selector. Profiles
without those fields do not include or synthesize the observations. The VERW
profile additionally compares GDTR base and limit as architectural state. The
FS-load profile additionally compares the FS selector, base, limit and
attributes plus GDTR state.
Native retired count may be unavailable and is compared only if both sides
observe it. Synthetic comparator tests do not constitute CPU execution evidence.
S2-12 updates semantic `rflags_defined` from decoded `FlagEffects` only after
successful retirement. AND/OR/XOR/TEST leave AF undefined (`0xcc7` instead of
`0xcd7`); its value is excluded from comparison. NOT/MOV preserve definedness;
later arithmetic can define AF again. A fault or unsupported instruction does
not update the mask. The snapshot mask describes architecture semantics and is
separate from the fixture's requested value-comparison mask; it is not an
expected arithmetic/logic result evaluator.
For fault observations, comparison ignores explanatory text in unavailable
reasons, while retaining availability, available values and required-field
validation. Both results are validated against the case before CLI status
dispatch, including unsupported or failed results; such statuses do not bypass
input digest, profile or snapshot validation. An unexpected core exit from the
validated S0 adapter is a harness failure, not invalid fixture input.

Explicit conversion creates a new JSON/digest and records the v1 input digest:

```sh
cargo run --offline -p od-runner -- --convert-fixture-v1 tests/fixtures/x86/s0_09_mov_imm64.case.json work/tmp/converted-v2.case.json
```

The output must not exist; conversion never overwrites it. Existing
`--compare-results A.json B.json --case CASE.json --json` dispatches by case
schema and uses strict v2 readers. Exit codes remain 0/2/3/4/5 for
success/mismatch/invalid/unsupported/harness failure. The locally accepted S2-08 slice adds v2
`--fixture` execution with the interpreter: full integer state, memory and
architectural fault snapshots. V2 JIT returns Unsupported/exit 4 explicitly.
Mapping/backend errors return Harness/exit 5; architectural faults remain a
separate outcome. A cooperative instruction-boundary wall deadline returns
TimedOut/exit 5, distinct from a stop request. See
[INTERPRETER](INTERPRETER.md#mov-и-v2-execution--s2-08) for exact MOV forms.

## Version 1

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

## S2-30aj literal x87 fixture profile

The S2-30aj x87-state fixtures preserve the project-local bounded instruction
contract in their literal programs: success observes only `dbf1dfe0` (FCOMI
followed by FNSTSW AX), and atomic rejection uses `dbf1` with budget one and no
stop RIP. The rejection fixture must encode an Empty tag at the actual old
physical `(TOP+1)&7` register while retaining a Valid old TOP. FCOMI defines
all six affected RFLAGS bits, so the top-level `rflags_mask` includes `0x8d5`;
the generic JSON schema validates the mask and fixture structure, while the
instruction decoder and fixture tests enforce the exact byte boundary.

## S2-30ak literal FCOMIP fixture profile

The S2-30ak x87-state fixtures preserve the local bounded instruction contract
in their literal programs: success observes only `dff1dfe0` (FCOMIP followed
by FNSTSW AX), and atomic rejection uses `dff1` with budget one and no stop
RIP. The rejection fixture encodes an Empty tag at actual old physical
`(TOP+1)&7` while retaining a Valid old TOP. FCOMIP defines all six affected
RFLAGS bits, so `rflags_mask` includes `0x8d5`; the schema validates fixture
structure and decoder/runner tests enforce the exact byte boundary and the
one-entry physical TOP pop.

## S2-30al literal FUCOMI fixture profile

The S2-30al x87-state fixtures preserve exact local instruction bytes:
success observes only `dbe9dfe0` (FUCOMI followed by FNSTSW AX), and atomic
rejection uses `dbe9` with budget one and no stop RIP. The rejection fixture
must encode an Empty tag at the actual old physical `(TOP+1)&7` while retaining
a Valid old TOP. FUCOMI defines all six affected RFLAGS bits, so
`rflags_mask` includes `0x8d5`. Schema/CLI tests reject `dbf8dfe0`.

## S2-30am literal FUCOMIP fixture profile

The S2-30am x87-state fixtures use exact local instruction bytes: success
observes `dfe9dfe0` (FUCOMIP followed by FNSTSW AX), and atomic rejection uses
`dfe9` with budget one and no stop RIP. The rejection fixture sets TOP=5 and
encodes Empty at the actual physical `(TOP+1)&7` tag while retaining a Valid
old TOP. FUCOMIP defines all six affected RFLAGS bits, so `rflags_mask`
includes `0x8d5`; successful execution also empties old TOP and advances TOP
once. Schema/CLI reject `dff8dfe0` and 66/48/9B-prefixed variants.

## S2-30an literal indexed FCOMI-family profile

The x87 state profile accepts the exact 32 bare comparison encodings
`DB E8..EF`, `DB F0..F7`, `DF E8..EF`, and `DF F0..F7`, either alone or
followed by exact `DF E0` FNSTSW AX. This is a closed set of 64 program strings;
prefixes, memory forms, other instructions, and extra trailing bytes are
rejected. Successful comparisons define all six affected RFLAGS bits, so
`rflags_mask` includes `0x8d5`.

A no-stop, budget-one Unsupported case may exercise only the encoded physical
source `(TOP+i)&7` with a clean x87 environment and a Valid old TOP. For `i=0`,
the deterministic class-reject vector uses a Valid old TOP with its exact
all-zero payload; the unrelated ST(1) can remain Valid. For nonzero indices,
the actual source tag is invalid; when ST(1) is unrelated it stays Valid.
Existing `i=1` invalid-RHS fixtures remain valid. Missing x87 state, arbitrary
special values, unrelated invalid tags and non-exact code strings do not form
part of this reject profile.

## S2-31u literal FLD m64fp fixture profile (historical)

The S2-31u `s2-long-x87-state-v1` fixtures use only exact `dd00` or
`dd00dfe0` programs. A successful fixture may supply binary64 signed zero or a
Normal source; signed zero is a successful input and the runner checks that its
result has the corresponding Zero tag. A no-stop, budget-one rejection fixture
for `dd00` admitted nonzero binary64 subnormal sources in the S2-31u slice.
S2-31z supersedes that predicate: current Unsupported fixtures are limited to
binary64 infinity/NaN, while masked subnormals are success inputs. Neighboring
source classes, code bytes and instruction boundaries remain subject to the
exact x87 profile.

## S2-31y literal FLD m32fp fixture profile

The S2-31y `s2-long-x87-state-v1` fixtures use only exact `d900` or
`d900dfe0` programs. Successful fixtures may supply signed zero, Normal, or
nonzero binary32 subnormal; subnormals produce canonical Normal binary80 with
Valid tag and sticky DE under the all-masked FCW profile. A no-stop,
budget-one Unsupported fixture for exact `d900` is admitted only when its
four-byte source is binary32 infinity or NaN (exponent field 255). Zero,
Normal and subnormal sources are excluded from this exception. Other x87
state, code bytes and instruction boundaries remain subject to the existing
exact x87 profile. This does not admit unmasked exception behavior.

## S2-31z literal FLD m64fp fixture profile

The S2-31z `s2-long-x87-state-v1` fixtures use only exact `dd00` or
`dd00dfe0` programs. Successful fixtures may supply signed zero, Normal, or
nonzero binary64 subnormal; subnormals produce canonical Normal binary80 with
Valid tag and sticky DE under the all-masked FCW profile. A no-stop,
budget-one Unsupported fixture for exact `dd00` is admitted only when its
eight-byte source has exponent field 2047 (infinity or NaN). Zero, Normal and
subnormal sources are excluded from this exception. Other x87 state, code
bytes and instruction boundaries remain subject to the existing exact x87
profile. This does not admit unmasked exception behavior.

## S2-31aa literal FLD m80fp fixture profile (historical)

The S2-31aa `s2-long-x87-state-v1` fixtures use exact `db28` or
`db28dfe0`. Successful cases may provide canonical signed binary80 infinity
(biased exponent `0x7fff`, explicit integer bit set, fraction zero); execution
preserves the ten raw bytes and assigns Special. At that stage the no-stop,
budget-one Unsupported exception admitted canonical QNaN for exact `db28`.
S2-31ab changes the current narrow exception to canonical positive SNaN; see
the current rule below.

## S2-31ab literal FLD m80fp fixture profile

The S2-31ab `s2-long-x87-state-v1` fixtures use exact `db28` or
`db28dfe0`. Successful cases may supply canonical signed binary80 QuietNaN
(biased exponent `0x7fff`, explicit integer bit set, quiet bit set, nonzero
fraction); execution preserves all ten raw bytes and assigns Special. The
no-stop, budget-one Unsupported exception admits canonical positive SNaN for
exact `db28` only. It excludes either QNaN sign, negative SNaN, pseudo-NaNs,
pseudo-infinity, pseudo-denormal, unnormal and other encodings. This narrow
schema rule does not admit other m80 forms, unmasked exception behavior, or
native execution.
