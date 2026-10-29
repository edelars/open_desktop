# CPU fixture and result formats

## Version 2: long-mode integer states

`od_testkit::v2` implements schema `2` with profiles `s2-long-integer-v1` and
`s2-long-fs-base-v1`, `s2-long-gs-base-v1`, `s2-long-fs-selector-v1` and
`s2-long-gs-selector-v1`, `s2-long-verw-gdt-v1`, and local-only
`s2-long-fs-load-v1`, `s2-long-x87-state-v1`, `s2-long-x87-sse-state-v1`, and
`s2-long-x87-selectors-state-v1`, local-only `s2-long-sep-disabled-v1` and
`s2-long-rdtsc-v1`,
`s2-long-fxsave-rip-v1`, and
`s2-long-fxsave64-rax-disp8-v1`.
V1 readers, bytes and historical digests remain unchanged. Segment profiles are
profile-specific state extensions;
it does not reinterpret or rewrite existing integer-profile fixtures.

The local-only `s2-long-sep-disabled-v1` fixtures describe one exact bare
Long64 instruction: the handwritten cases each use one code mapping at the
initial RIP containing exactly `0F 34` or `0F 35`, with
`instruction_budget: 1`, `stop_rip: RIP + 2`, and `expected_exit: exception`
for vector 6 with no error code. The S2-26d execution contract is CPU-side #UD
at the initial RIP after complete fetch, with no retirement, architectural
state/definedness or memory change, and no data-memory access. Its unit and
runner tests assert those outcomes. These fixture examples do not add general
schema restrictions on other mappings or comparator guarantees for other
profiles.

The local-only `s2-long-rdtsc-v1` fixtures describe exactly one bare Long64
`0F 31`, budget 1, and stop RIP at entry+2. They require `rdtsc_controls` with
immutable `cr4_tsd`, a literal `source_value` and `expected_source_calls`. The
current runner CPL is ring 3: `cr4_tsd: false` expects one source call, EAX/EDX
zero-extension and `stop_address`; `cr4_tsd: true` expects `#GP(0)`, no
retirement and zero calls. The observed call count is recorded as
`snapshot.rdtsc_source_calls` and result validation checks it against the
fixture. Unit tests separately cover CPL0–3. This profile injects no host clock,
does not model exception delivery or physical TSC behavior, and is admitted
only by the interpreter; JIT/native are denied and the C ABI fragment-create
path rejects RDTSC because it has no explicit profile/source inputs. See the [fixture
notes](../../tests/fixtures/x86/stage2/README.md#s2-26e-rdtsc-with-injected-source),
[CPU contract](../specs/CPU_CONTRACT.md#локальный-профиль-rdtscsourcev1-s2-26e)
and [plan](../plans/STAGE_2.md#s2-26e--точный-rdtsc-с-явным-tsc-источником).

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
0…10000000 and wall timeout is 1…30000 ms. `stop_rip` is a harness boundary
checked before fetch, not an x86 instruction or a universal end-of-RAM exit.

`expected_exit` accepts the existing architectural stops plus `harness_error`
for a bounded-memory/backend failure. A harness error remains distinct from a
guest exception: the interpreter does not synthesize #PF from an absent mapping.
Harness errors can be recorded in CLI fixtures, but `compare` only compares
completed architectural observations and rejects harness-error results.

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
result has the corresponding Zero tag. A no-stop, budget-one rejection fixture for `dd00` admitted nonzero binary64
subnormal sources in the S2-31u slice. S2-31z superseded that predicate with
binary64 infinity/NaN while masked subnormals became success inputs; S2-31ae
then enabled infinities and narrowed the current Unsupported fixture to
canonical QNaNs. Neighboring source classes, code bytes and instruction
boundaries remain subject to the exact x87 profile.

## S2-31y literal FLD m32fp fixture profile

The S2-31y `s2-long-x87-state-v1` fixtures use only exact `d900` or
`d900dfe0` programs. Successful fixtures may supply signed zero, Normal, or
nonzero binary32 subnormal; subnormals produce canonical Normal binary80 with
Valid tag and sticky DE under the all-masked FCW profile. At the S2-31y stage, a no-stop, budget-one Unsupported fixture for exact
`d900` admitted binary32 infinity or NaN sources (exponent field 255). S2-31af
supersedes this: signed infinities are now successful sources, and the current
Unsupported exception admits canonical QNaN only. Zero, Normal and subnormal
sources remain excluded. Other x87 state, code bytes and instruction
boundaries remain subject to the existing exact x87 profile. Unmasked behavior
is not admitted.

## S2-31z literal FLD m64fp fixture profile

The S2-31z `s2-long-x87-state-v1` fixtures use only exact `dd00` or
`dd00dfe0` programs. Successful fixtures may supply signed zero, Normal, or
nonzero binary64 subnormal; subnormals produce canonical Normal binary80 with
Valid tag and sticky DE under the all-masked FCW profile. At the S2-31z stage, a no-stop, budget-one Unsupported fixture for exact
`dd00` was admitted when its eight-byte source had exponent field 2047
(infinity or NaN). S2-31ae supersedes it: current rejection fixtures require a
canonical QNaN source. S2-31ag supersedes that predicate by accepting canonical
QNaNs; the current rejection fixture requires canonical SNaN. Zero, Normal,
subnormal and infinity sources remain excluded from the current exception.
Other x87 state, code bytes and instruction boundaries remain subject to the
existing exact x87 profile. This does not admit unmasked exception behavior.

## S2-31aa literal FLD m80fp fixture profile (historical)

The S2-31aa `s2-long-x87-state-v1` fixtures use exact `db28` or
`db28dfe0`. Successful cases may provide canonical signed binary80 infinity
(biased exponent `0x7fff`, explicit integer bit set, fraction zero); execution
preserves the ten raw bytes and assigns Special. At that stage the no-stop,
budget-one Unsupported exception admitted canonical QNaN for exact `db28`.
S2-31ab changed the narrow exception to canonical positive SNaN; S2-31ac
replaces it with positive pseudo-infinity in the current rule below.

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

## S2-31ac literal FLD m80fp fixture profile

The S2-31ac `s2-long-x87-state-v1` profile admits canonical binary80
SignalingNaN sources for exact `db28` and `db28dfe0`: biased exponent `0x7fff`,
explicit integer bit set, quiet bit clear and nonzero remaining fraction.
Success preserves the raw bytes and assigns Special under the unchanged
all-masked FCW/`FSW & 0x80df`/Empty/ten-byte/read-once gates. The exact `db28`,
budget-one, no-stop Unsupported exception now admits positive pseudo-infinity
only. It excludes either sign of canonical SNaN or QNaN, pseudo-NaNs,
pseudo-denormals, unnormals, other special classes and other opcodes.

## S2-31ad `FLD m80fp` pseudo-denormal fixtures

Exact `db28` and `db28dfe0` success fixtures may supply binary80 pseudo-denormals
(exponent zero, explicit integer bit set, either sign and any fraction). The
interpreter preserves the raw ten-byte source, assigns Special and adds no DE
or other status bits. The exact DB28/budget-one/no-stop Unsupported exception
continues to admit only positive pseudo-infinity; pseudo-denormals and adjacent
classes are not admitted by that predicate.


## S2-31ae literal FLD m64fp signed-infinity fixtures

The `s2-long-x87-state-v1` profile accepts signed binary64 infinity sources
for exact `dd00` and `dd00dfe0`, yielding canonical signed binary80 infinity
with Special tag. At the S2-31ae stage, the no-stop, budget-one Unsupported
exception admitted canonical QNaN for exact `dd00`; S2-31ag supersedes that
rule by accepting QNaN loads and narrowing current rejection to canonical
SNaN. The all-masked FCW set, `FSW & 0x80dd` gate, Empty destination and
eight-byte single-read constraints remain in force. Native/JIT profiles
continue to deny the load.


## S2-31af literal FLD m32fp signed-infinity fixtures

The `s2-long-x87-state-v1` profile accepts signed binary32 infinity sources
for exact `d900` and `d900dfe0`, yielding canonical signed binary80 infinity
with Special tag. The no-stop, budget-one Unsupported exception admits
canonical QNaN only for exact `d900`; infinities and SNaNs are excluded. The
all-masked FCW set, `FSW & 0x80dd` gate, Empty destination and four-byte
single-read constraints remain in force. Native/JIT profiles continue to deny
the load.


## S2-31ag literal FLD m64fp canonical QNaN fixtures

The `s2-long-x87-state-v1` profile accepts canonical binary64 QNaNs for exact
`dd00` and `dd00dfe0`. The quiet bit is set, lower payload bits are arbitrary,
and sign/payload map exactly into canonical binary80 with Special tag. The
four all-masked FCWs, `FSW & 0x80dd` gate, Empty destination, canonical48
eight-byte preflight and one-Qword-read rules remain unchanged. The exact
DD00, budget-one, no-stop Unsupported exception now admits canonical SNaN
only; QNaN, infinity, zero, Normal and subnormal sources do not match. The
positive bare and negative payload-bearing DD00/DFE0+FNSTSW fixtures run
through CLI. JIT/native profiles continue to deny the load.

## S2-31ah literal FLD m32fp canonical QNaN fixtures

The `s2-long-x87-state-v1` profile accepts canonical binary32 QNaNs for exact
`d900` and `d900dfe0`. Fraction bit 22 is set, lower payload bits are
arbitrary, and sign/payload map into canonical binary80 with Special tag. The
four all-masked FCWs, `FSW & 0x80dd` gate, Empty destination, canonical48
four-byte preflight and one-Dword-read rules remain unchanged. At S2-31ah, the
exact D900/budget-one/no-stop exception was keyed to SNaN; S2-31aj now accepts
SNaNs and changes this exception to an invalid-environment predicate. The positive
bare and negative payload-bearing D900/DFE0+FNSTSW fixtures run through CLI.
JIT/native profiles continue to deny the load.

## S2-31ai literal FLD m64fp canonical SNaN fixtures

The current `s2-long-x87-state-v1` profile accepts canonical binary64 SNaNs
for exact `dd00` successful fixtures. They are converted from integer fields
to canonical binary80 Special with preserved sign/payload and a set quiet bit;
IE is sticky, and `FSW & 0x80dd` is the preflight gate. Pre-existing IE remains
an Unsupported environment case before operand access. Therefore, the exact
DD00, one-instruction, no-stop Unsupported schema exception is environment-
driven (invalid FCW or a set masked gate bit), independent of the operand
class. The old SNaN-class rejection exception is superseded. Bare and
`dd00dfe0`/FNSTSW fixtures assert output and IE. See the [S2-31ai CPU
contract](../specs/CPU_CONTRACT.md#local-s2-31ai-bounded-fld-m64fp-rax-dd-00-canonical-snans), [plan](../plans/STAGE_2.md#s2-31ai-extend-bounded-fld-m64fp-rax-dd-00-to-canonical-snans), and [report](../reports/STAGE_2_REPORT.md#s2-31ai--bounded-fld-m64fp-canonical-snan-extension).

## S2-31aj literal FLD m32fp canonical SNaN fixtures

The positive bare `D9 00` fixture loads a minimum-payload canonical binary32
SNaN; the negative `D9 00 DF E0` fixture loads a middle-payload SNaN and
observes sticky IE through FNSTSW. Conversion sets the binary80 quiet bit,
preserves payload and sign, and assigns Special. Independent low/middle/max
payload literals of both signs cover every TOP×FCW pair. Tests also cover
sticky DE/PE, SNaN→quieted-QNaN bytes, one-Dword-read faults, unmasked/pre-
existing-IE rejection before access, subsequent-load rejection, and FNCLEX
clearing flags while preserving TOP/tags/payloads and permitting another load.
The exact D900/budget-one/no-stop Unsupported schema case is environment-
driven (invalid FCW or `FSW & 0x80dd != 0`) and does not depend on operand
class. JIT/native profiles continue to deny D900 loads. See the [S2-31aj CPU
contract](../specs/CPU_CONTRACT.md#local-s2-31aj-bounded-fld-m32fp-rax-d9-00-canonical-snans), [plan](../plans/STAGE_2.md#s2-31aj-extend-bounded-fld-m32fp-rax-d9-00-to-canonical-snans), and [report](../reports/STAGE_2_REPORT.md#s2-31aj--bounded-fld-m32fp-canonical-snan-extension).

## S2-31ak m32/m64 FLD status predicates

Exact D900/DD00 loads now admit sticky IE, DE, OE, UE and PE because their
local preflight is `FSW & 0x80c4 == 0`; those flags alone must not be used to
make an Unsupported fixture valid. Rejected-environment fixtures should set
ZE, SF, ES or B (or use an invalid FCW/boundary), independent of operand class.
This predicate is local to D900/DD00; FILD and m80 retain their own x87 gates.

The `s2_31dt_fld_m32fp_seeded_ue_fnstsw.case.json` and
`s2_31dt_fld_m64fp_seeded_ue_fnstsw.case.json` fixtures seed UE only, execute
the exact load followed by `FNSTSW AX`, and assert that the load creates no
additional IE/DE/PE. Current subnormal and SNaN behavior remains covered by
the neighboring literal fixtures and interpreter matrices.

## S2-31dk FRNDINT historical UE fixtures

The `s2_31dk_frndint_seeded_ue_fnstsw.case.json` fixture exercises exact
`D9 FC DF E0` with UE set before FRNDINT; exact UE-only must remain UE-only.
The `s2_31dk_fst_m32_underflow_frndint_fnstsw.case.json` fixture uses the
literal composition `D9 10 D9 FC DF E0`: FST m32 underflow creates UE+PE,
FRNDINT on the same inexact value preserves both, and FNSTSW exposes the final
status. This extends the local x87-state profile by one exact instruction
sequence; it does not change the general fixture schema. Keep the `0x037f`
FCW and matching physical TOP tag/payload, and preserve a separate literal
fixture for the exact UE-only case so UE cannot mask an accidental PE producer.

## S2-31dl FRNDINT historical OE fixtures

The `s2_31dl_frndint_seeded_oe_fnstsw.case.json` fixture exercises literal
`D9 FC DF E0` with exact `1.0` and OE only; FRNDINT must preserve OE without
creating PE. The `s2_31dl_fst_m32_overflow_frndint_fnstsw.case.json` fixture
uses literal `D9 10 D9 FC DF E0` over canonical binary80 `(2 − 2^-63) × 2^127`, with
initial FSW=0. FST m32 produces OE+PE+C1; FRNDINT preserves OE+PE and clears
C1, and FNSTSW exposes the final status. This reuses the exact composition
sequence already admitted by the local profile and does not widen the fixture
schema.

## S2-31al signed-zero m32 stores

The bounded `D9 10`/`D9 18` store profile admits a binary80 source tagged Zero
only when its raw exponent and 64-bit significand are both zero. The sign is
observable in the stored binary32 Dword. Fixtures should use the matching Zero
tag; a zero payload under another tag or a Zero tag over a nonzero/pseudo
payload is an interpreter rejection before the data write. The existing
all-masked FCWs and `FSW & 0x80df` status predicate remain in force.

## S2-31am signed-zero m64 stores

Exact `DD 10`/`DD 18` use the same strict Zero-tag plus canonical raw binary80
zero predicate, storing the sign in a binary64 Qword. An invalid source or
forbidden status gate must not become a valid Unsupported fixture by relying
on a special-value source; use an environment/boundary predicate. Positive
bare and negative `DF E0` fixtures cover FST and FSTP, and must be included in
CLI/schema and native-denial tests. Native/JIT admission remains denied. See
[S2-31am CPU contract](../specs/CPU_CONTRACT.md#local-s2-31am-signed-zero-fst-m64fp-and-fstp-m64fp), [plan](../plans/STAGE_2.md#s2-31am-store-signed-binary80-zero-through-bounded-m64-fstfstp), and [report](../reports/STAGE_2_REPORT.md#s2-31am--signed-zero-stores-for-bounded-m64-fstfstp).


## S2-31an signed-zero m80 FSTP

The exact bounded `DB 38` profile accepts canonical binary80 zero only with a
Zero tag (or the existing canonical Normal with Valid tag). Use a matching
`Zero`/all-zero-exponent-and-significand pair to test signed-zero storage. The
raw ten bytes, including sign, are copied unchanged. Invalid pairs are
interpreter rejects before data access; invalid-environment fixtures must use a
forbidden status bit, invalid FCW, or boundary condition rather than relying on
a value-class rejection. See the [S2-31an contract](../specs/CPU_CONTRACT.md#local-s2-31an-signed-zero-fstp-m80fp-rax-db-38), [plan](../plans/STAGE_2.md#s2-31an-store-canonical-signed-binary80-zero-through-bounded-fstp-m80fp), and [report](../reports/STAGE_2_REPORT.md#s2-31an--signed-zero-binary80-stores-through-bounded-fstp-m80fp).


## S2-31ao signed-infinity m80 FSTP

For exact `DB 38`, canonical binary80 infinity uses a Special tag, exponent
`0x7fff`, integer bit one, and zero fraction. The output must preserve all ten
source bytes and sign. Retain the S2-31an signed-zero and Normal cases.
Unsupported test cases must use actual noncanonical classes or mismatched tags,
not treat canonical infinity as rejected. See the [S2-31ao contract](../specs/CPU_CONTRACT.md#local-s2-31ao-signed-infinity-fstp-m80fp-rax-db-38), [plan](../plans/STAGE_2.md#s2-31ao-store-canonical-signed-binary80-infinity-through-bounded-fstp-m80fp), and [report](../reports/STAGE_2_REPORT.md#s2-31ao--signed-infinity-binary80-store-through-bounded-fstp-m80fp).


## S2-31ap signed-infinity m64 stores

Exact `DD 10`/`DD 18` fixtures may use a Special-tagged canonical binary80
infinity source and literal binary64 infinity output. Do not classify canonical
infinity as an unsupported source; rejection fixtures must use pseudo-infinity,
NaNs, finite specials, mismatched tag/class pairs, or a forbidden environment.
Positive bare and negative `DD 18 DF E0` fixtures are schema/CLI reachable and
observe the FSTP status with FNSTSW. See the [S2-31ap contract](../specs/CPU_CONTRACT.md#local-s2-31ap-signed-infinity-fstfstp-m64fp-rax),
[plan](../plans/STAGE_2.md#s2-31ap-store-canonical-signed-binary80-infinity-through-bounded-m64-fstfstp),
and [report](../reports/STAGE_2_REPORT.md#s2-31ap--signed-infinity-stores-for-bounded-m64-fstfstp).

The exact existing project-fixture streams `D9 00 DD D1 DF E0` and
`D9 00 DD D9 DF E0` also admit canonical binary32 positive/negative infinity
through bounded FST/FSTP ST(1), followed by FNSTSW. These sequences exercise
FLD-created Special-tagged infinity, raw ten-byte copying, and the FSTP pop;
their fixture schema profile is unchanged. Pseudo-infinity, NaNs, malformed
encodings, and mismatched source tags remain unsupported. See the [S2-30au
contract](../specs/CPU_CONTRACT.md#local-s2-30au-canonical-signed-infinity-through-fstfstp-sti),
[plan](../plans/STAGE_2.md#s2-30au-admit-canonical-signed-infinity-to-bounded-fstfstp-sti),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30au-canonical-signed-infinity-fstfstp-sti).


### S2-31aq signed-infinity m32 FST/FSTP fixtures

Exact `D9 10`/`D9 18` accept a Special-tagged canonical binary80 infinity and
expect literal binary32 `+∞` (`0000807f`) or `-∞` (`000080ff`). Positive bare
FST and negative FSTP+FNSTSW fixtures exercise both instructions through
schema and CLI; both are included in native denial. Pseudo-infinity, NaNs,
other Special classes, mismatched tags, and forbidden status reject before the
write. See the [S2-31aq contract](../specs/CPU_CONTRACT.md#local-s2-31aq-signed-infinity-fstfstp-m32fp-rax),
[plan](../plans/STAGE_2.md#s2-31aq-store-canonical-signed-binary80-infinity-through-bounded-m32-fstfstp),
and [report](../reports/STAGE_2_REPORT.md#s2-31aq--signed-infinity-stores-for-bounded-m32-fstfstp).


### S2-31ar canonical QuietNaN DB38 fixtures

Exact DB38 fixtures may use a Special-tagged canonical binary80 QuietNaN and
expect all ten raw bytes to remain unchanged. Positive bare payload-bearing
QNaN and negative-indefinite DFE0/FNSTSW fixtures are schema/CLI reachable and
included in native denial; JIT denial is checked through both fixtures. SNaN,
pseudo-QNaN, other unsupported encodings, and mismatched tags are rejection
neighbors. See the [S2-31ar contract](../specs/CPU_CONTRACT.md#local-s2-31ar-canonical-quietnan-fstp-m80fp-rax-db-38),
[plan](../plans/STAGE_2.md#s2-31ar-store-canonical-binary80-quietnan-through-bounded-fstp-m80fp),
and [report](../reports/STAGE_2_REPORT.md#s2-31ar--canonical-quietnan-binary80-store-through-bounded-fstp-m80fp).

### S2-31as canonical SignalingNaN DB38 fixtures

The positive bare `DB 38` fixture stores a canonical binary80 SignalingNaN
unchanged. The negative `DB 38 DF E0` fixture stores a signed payload-bearing
SignalingNaN and checks FNSTSW; both have exact opcode budgets, memory images,
and stop predicates. See the [S2-31as contract](../specs/CPU_CONTRACT.md#local-s2-31as-canonical-signalingnan-fstp-m80fp-rax-db-38),
[plan](../plans/STAGE_2.md#s2-31as-store-canonical-binary80-signalingnan-through-bounded-fstp-m80fp),
and [report](../reports/STAGE_2_REPORT.md#s2-31as--canonical-signalingnan-binary80-store-through-bounded-fstp-m80fp).

### S2-31at canonical-subnormal DB38 fixtures

The positive bare `DB 38` fixture stores a minimum binary80 subnormal
unchanged. The negative `DB 38 DF E0` fixture stores a maximum-payload
subnormal and checks FNSTSW; both have exact opcode budgets, memory images,
and stop predicates. See the [S2-31at contract](../specs/CPU_CONTRACT.md#local-s2-31at-canonical-subnormal-fstp-m80fp-rax-db-38),
[plan](../plans/STAGE_2.md#s2-31at-store-canonical-binary80-subnormal-through-bounded-fstp-m80fp),
and [report](../reports/STAGE_2_REPORT.md#s2-31at--canonical-subnormal-binary80-store-through-bounded-fstp-m80fp).


## S2-31au canonical QuietNaN m64 stores

The positive bare DD10 fixture stores a positive QuietNaN; the negative DD18
DFE0 fixture stores a payload-bearing negative QuietNaN and observes FSW with
FNSTSW. Both are schema/CLI reachable and included in native/JIT denial. See
the [S2-31au contract](../specs/CPU_CONTRACT.md#local-s2-31au-canonical-quietnan-fstfstp-m64fp-rax-dd-10-dd-18),
[plan](../plans/STAGE_2.md#s2-31au-store-canonical-binary80-quietnan-through-bounded-fstfstp-m64fp),
and [report](../reports/STAGE_2_REPORT.md#s2-31au--canonical-quietnan-binary80-to-binary64-stores).


## S2-31av canonical QuietNaN m32 stores

The positive bare D910 fixture stores a positive QuietNaN; the negative D918
DFE0 fixture stores a payload-bearing negative QuietNaN and observes FSW with
FNSTSW. Both are schema/CLI reachable and included in native/JIT denial. See
the [S2-31av contract](../specs/CPU_CONTRACT.md#local-s2-31av-canonical-quietnan-fstfstp-m32fp-rax-d9-10-d9-18),
[plan](../plans/STAGE_2.md#s2-31av-store-canonical-binary80-quietnan-through-bounded-fstfstp-m32fp),
and [report](../reports/STAGE_2_REPORT.md#s2-31av--canonical-quietnan-binary80-to-binary32-stores).


## S2-31aw canonical SignalingNaN m64 stores

The positive bare DD10 fixture and negative DD18/DFE0/FNSTSW fixture use
Special-tagged canonical binary80 SNaNs. Expected binary64 values preserve sign,
quiet the NaN, and retain the upper 51 payload bits; sticky IE is observed only
after the Qword write, while PE and the raw source payload are preserved. Both
fixtures are schema/CLI reachable and explicitly denied by JIT/native. See the
[S2-31aw contract](../specs/CPU_CONTRACT.md#local-s2-31aw-canonical-signalingnan-fstfstp-m64fp-rax-dd-10-dd-18),
[plan](../plans/STAGE_2.md#s2-31aw-store-canonical-binary80-signalingnan-through-bounded-fstfstp-m64fp),
and [report](../reports/STAGE_2_REPORT.md#s2-31aw--canonical-signalingnan-binary80-to-binary64-stores).


## S2-31ax canonical SignalingNaN m32 stores

The positive bare D910 fixture stores a quieted positive binary32 NaN and
observes sticky IE. The negative D918/DFE0 fixture preserves the upper 22
payload bits, pops only after writing, and observes IE through FNSTSW. Both
fixtures retain their binary80 source bytes, execute through the interpreter,
and are denied by JIT/native admission. See the
[S2-31ax contract](../specs/CPU_CONTRACT.md#local-s2-31ax-canonical-signalingnan-fstfstp-m32fp-rax-d9-10-d9-18),
[plan](../plans/STAGE_2.md#s2-31ax-store-canonical-binary80-signalingnan-through-bounded-fstfstp-m32fp),
and [report](../reports/STAGE_2_REPORT.md#s2-31ax--canonical-signalingnan-binary80-to-binary32-stores).


## S2-31ay sticky IE m32/m64 stores

The S2-31ay CLI fixtures start with masked IE already set and exercise exact
m32 FST plus m64 FSTP/FNSTSW. They are parsed and executed by the interpreter,
and included in explicit JIT-denial checks. Full-width ISA matrices cover all
four store opcodes, FCWs, TOP wrap, sticky IE/PE, C1 policy, and admitted source
classes; native runtime and physical acceptance remain deferred under
ISSUE-019. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31ay-admit-sticky-ie-fstfstp-m32-m64),
[plan](../plans/STAGE_2.md#s2-31ay-admit-sticky-ie-in-bounded-m32-m64-fst-fstp),
and [report](../reports/STAGE_2_REPORT.md#s2-31ay--sticky-ie-m32-m64-stores).


## S2-31az sticky IE/DE/PE m32/m64 store fixtures

The two S2-31az schema-v2 fixtures exercise exact D900 followed by DD10 or
DD18, with FNSTSW in the pop case. They preserve the DE set by a masked
binary32 subnormal load, then verify exact binary64-normal output; the negative
largest-subnormal case also checks pop and status observation. Fixture schema
admission is limited to these two exact composition programs. The standalone
S2-31y load fixtures and unrelated x87 gates are unchanged. See the
[S2-31az contract](../specs/CPU_CONTRACT.md#local-s2-31az-admit-sticky-de-m32-m64-fstfstp),
[plan](../plans/STAGE_2.md#s2-31az-admit-sticky-de-in-bounded-m32-m64-fst-fstp),
and [report](../reports/STAGE_2_REPORT.md#s2-31az--sticky-de-in-bounded-m32-m64-stores).

## S2-30ch literal indexed FCMOVU profile

The S2-30ch x87 fixtures use the exact unprefixed Long64 DA D8..DF bytes.
The schema admits eight bare indexed forms and their exact FNSTSW AX suffixes,
the repeated DA D9 stream, and the retained TEST producer. Literal successes
cover self-index PF=0/1 and indices 1..7; rejects use a bare indexed instruction
with one-step budget and no stop RIP. This is a local bounded fixture contract;
JIT/native execution, unordered/NaN operands and physical acceptance are not
implied. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30ch-bounded-indexed-fcmovu-st0-sti-da-d8df).

## S2-30ci literal exact bare FCMOVNU profile

The schema admits exactly the unprefixed Long64 streams `dbd9`, `dbd9dfe0`,
`dbd9dbd9dfe0`, and `d9e8d9ee4885c0dbd9dfe0`, with budgets 1, 2, 3 and 5.
Rejection fixtures use only the bare `dbd9` instruction with one-step budget
and no stop RIP. Two TEST producer fixtures use RAX=1 and RAX=3 and preserve
full input state and memory for runner comparison. JIT remains denied. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-30ci-exact-bare-fcmovnu-st0-st1-db-d9).

## Stage 2 interpreter budget boundaries

Schema-v2 fixtures allow `instruction_budget` zero through the existing upper
bound. Zero budget stops before fetch and leaves CPU/memory unchanged; it is
used to distinguish the outer instruction budget from a bounded REP instruction
that may perform up to 1024 successful iterations while consuming one retired
instruction only on complete success. `rep_iteration_budget_exhausted` and
`harness_error` remain distinct from architectural exceptions. A missing bounded
mapping does not imply `#PF`; partial store progress is observable only when
the fixture runner returns a snapshot, while injected/unit checks may inspect
local memory after harness failure.

## S2-19i tracked REP continuation snapshots

Tracked continuation adds no schema-v2 fixture fields and does not change the
fixture digest contract. The six `s2_19i_rep_restart_*` cases use the existing
`rep_iteration_budget_exhausted` expected exit for the first run. Their
`s2_19i_rep_restart_expected.json` sidecar stores complete literal snapshots
for that partial run and the final explicitly resumed run. Resume is a runner
test action: it supplies the prior `RunResult.rflags_defined` to
`od_isa::execute::run_with_rflags_defined`. Ordinary fixtures and
`od_isa::execute::run()` continue to start as fresh executions.

## Local RCPSS diagnostic profile — S2-37b

`s2-long-rcpss-reg-v1` reuses schema v2 and the existing full long-mode state
shape, including required raw x87 and SSE snapshots. Its executable region must
contain exactly one `F3 [optional adjacent REX.R/B] 0F 53 /r` instruction with
ModR/M.mod=3 and no trailing bytes; ModR/M.reg is the destination and r/m is
the source. The fixture's stop RIP is the instruction end. The profile adds no
JSON fields and does not change older fixture bytes or digests. JIT/native
execution is unsupported. Expected output is the deterministic local helper
value, not an exact CPU oracle around the unresolved RCPSS tiny-result
threshold; see [ISSUE-025](../KNOWN_ISSUES.md#issue-025--intel-rcpssrcpps-threshold-inequalities-overlap).

## Local RSQRTPS diagnostic profile — S2-37c

`s2-long-rsqrtps-reg-v1` reuses schema v2 and requires the existing full
long-mode state, including raw x87 and SSE snapshots. Its executable region
must contain exactly one bare `0F 52 /r` (three bytes) or one adjacent REX.R/B
byte followed by `0F 52 /r` (four bytes), with ModR/M.mod=3 and no trailing
bytes; ModR/M.reg selects destination and r/m selects source. The fixture stop
RIP is the instruction end. Other prefixes, REX.W/X, memory, VEX/EVEX, scalar
RSQRTSS and all neighboring forms are rejected. The profile adds no JSON fields
and does not change existing fixture bytes or digests. JIT/native execution is
unsupported. Expected outputs are literal local reference values; hardware
validation compares the `3/8192` error envelope for non-flushed normal outputs
without requiring exact approximate bits. See [the S2-37c fixture](../../tests/fixtures/x86/stage2/s2_37c_rsqrtps_register.case.json).

## Local RCPPS diagnostic profile — S2-37d

`s2-long-rcpps-reg-v1` reuses schema v2 and requires the existing full
long-mode state, including raw x87 and SSE snapshots. Its executable region
must contain exactly one bare `0F 53 /r` (three bytes) or one adjacent REX.R/B
byte followed by `0F 53 /r` (four bytes), with ModR/M.mod=3 and no trailing
bytes; ModR/M.reg selects destination and r/m selects source. The profile
rejects RCPSS (`F3 0F 53 /r`) and other prefixes/forms locally, while common
decoder profiles continue to admit separately implemented instructions.
JIT/native execution is unsupported. Expected output is the deterministic
local `Binary32::reciprocal_sse_raw()` value, not an exact CPU oracle near the
unresolved ISSUE-025 tiny-result threshold. Hardware validation checks the
`3/8192` error envelope for non-flushed normal outputs without exact approximate
bit equality. See [the S2-37d fixture](../../tests/fixtures/x86/stage2/s2_37d_rcpps_register.case.json)
and [ISSUE-025](../KNOWN_ISSUES.md#issue-025--intel-rcpssrcpps-threshold-inequalities-overlap).

## Local MINSS masked-status profile — S2-37e

The schema-v2 profile `s2-long-minss-reg-masked-v1` requires the existing full
Long64 state, including raw x87 and SSE snapshots. Its code region contains
exactly one `F3 [optional adjacent REX.R/B] 0F 5D /r` instruction with
ModR/M.mod=3, no memory operand and no trailing bytes; ModR/M.reg is the
destination and r/m is the source. Unsupported mandatory/legacy prefixes,
REX.W/X, repeated/nonadjacent REX, truncation, VEX/EVEX and neighboring forms
are rejected by the profile. MXCSR must have all six exception masks set,
DAZ/FTZ clear and reserved bits 16–31 clear; RC and pre-set sticky bits are
allowed. No fixture fields are added. The expected low dword and sticky bits
are literal. Invalid status has priority over denormal for the same operation:
a same-operation NaN/subnormal case adds
IE but not new DE, while prior sticky DE is preserved. The profile is local-only
and JIT/native remain denied. See
[the S2-37e fixture](../../tests/fixtures/x86/stage2/s2_37e_minss_masked_register.case.json)
and [the bounded plan](../plans/STAGE_2.md#s2-37e--local-long64-register-only-legacy-minss-with-masked-status).

## Local MAXSS masked-status profile — S2-37f

The schema-v2 profile `s2-long-maxss-reg-masked-v1` requires the full Long64
state and exactly one `F3 [optional adjacent REX.R/B] 0F 5F /r` instruction,
ModR/M.mod=3, no memory operand or trailing bytes. Destination/source register
extensions follow REX.R/B. The profile rejects all extra prefixes, REX.W/X,
memory, truncation, VEX/EVEX and neighboring opcodes. MXCSR must have all six
exception masks set, DAZ/FTZ clear and reserved bits 16–31 clear; any RC and
prior sticky flags are permitted. Fixture expectations are literal; JIT and
native admission remain denied. See [the S2-37f fixture](../../tests/fixtures/x86/stage2/s2_37f_maxss_masked_register.case.json)
and [the bounded plan](../plans/STAGE_2.md#s2-37f--local-long64-register-only-legacy-maxss-with-masked-status).

## Local MINPS masked-status profile — S2-37g

The schema-v2 profile `s2-long-minps-reg-masked-v1` requires exactly one bare
or one-adjacent-REX Long64 `0F 5D /r` instruction, ModR/M.mod=3, no memory
operand or trailing bytes. Destination/source extension uses REX.R/B; F3
MINSS is a separate instruction and is rejected by the MINPS profile. The
profile requires the full Long64 snapshot, all six MXCSR exception masks set,
DAZ/FTZ clear and reserved bits 16–31 clear. Any RC and old sticky flags are
allowed. Literal fixture values check the four-lane result and IE/DE
aggregation from distinct lanes. Focused unit tests check per-lane exception
priority without another subnormal lane that could set DE. JIT/native remain
denied; unmasked exception delivery and fault ordering are outside this profile. See [the S2-37g
fixture](../../tests/fixtures/x86/stage2/s2_37g_minps_masked_register.case.json)
and [the bounded plan](../plans/STAGE_2.md#s2-37g--local-long64-register-only-legacy-minps-with-masked-status).

## Local MAXPS masked-status profile — S2-37h

The schema-v2 profile `s2-long-maxps-reg-masked-v1` requires exactly one bare
or one-adjacent-REX Long64 `0F 5F /r` instruction, ModR/M.mod=3, no memory
operand or trailing bytes. Destination/source extension uses REX.R/B; F3
MAXSS is a separate instruction and is rejected by the MAXPS profile. The
profile requires the full Long64 snapshot, all six MXCSR exception masks set,
DAZ/FTZ clear and reserved bits 16–31 clear. Any RC and old sticky flags are
allowed. The literal fixture checks the four-lane result and IE/DE
aggregation from distinct lanes. Focused unit tests check per-lane exception
priority without another subnormal lane that could set DE. JIT/native remain
denied; unmasked exception delivery and fault ordering are outside this profile. See [the S2-37h
fixture](../../tests/fixtures/x86/stage2/s2_37h_maxps_masked_register.case.json)
and [the bounded plan](../plans/STAGE_2.md#s2-37h--local-long64-register-only-legacy-maxps-with-masked-status).


## Local MINSD masked-status profile — S2-37i

The schema-v2 profile `s2-long-minsd-reg-masked-v1` accepts exactly one
Long64 `F2 [optional adjacent REX.R/B] 0F 5D /r` instruction with ModR/M.mod=3.
F3 MINSS and bare MINPS remain distinct decoder operations and are rejected by
this profile. The existing state schema is unchanged; MXCSR admission requires
all exception masks, DAZ/FTZ clear and reserved bits 16–31 clear. A literal
fixture checks low-qword RHS selection and masked IE priority over
same-operation DE, with upper-qword SNaN/subnormal sentinels. The profile is
local-only, with JIT/native denied. See [the S2-37i fixture](../../tests/fixtures/x86/stage2/s2_37i_minsd_masked_register.case.json)
and [the bounded plan](../plans/STAGE_2.md#s2-37i--local-long64-register-only-legacy-minsd-with-masked-status).


## Local MAXSD masked-status profile — S2-37j

The schema-v2 profile `s2-long-maxsd-reg-masked-v1` accepts exactly one
Long64 `F2 [optional adjacent REX.R/B] 0F 5F /r` instruction with ModR/M.mod=3.
F3 MAXSS and bare MAXPS remain distinct decoder operations and are rejected by
this profile. MXCSR requires all exception masks, DAZ/FTZ clear and reserved
bits 16–31 clear. See [fixture](../../tests/fixtures/x86/stage2/s2_37j_maxsd_masked_register.case.json),
[plan](../plans/STAGE_2.md#s2-37j--local-long64-register-only-legacy-maxsd-with-masked-status),
and [CPU contract](../specs/CPU_CONTRACT.md#local-s2-37j-register-only-maxsd-with-masked-status).

## Local MINPD masked-status profile — S2-37k

The schema-v2 profile `s2-long-minpd-reg-masked-v1` accepts exactly one
Long64 `66 [optional adjacent REX.R/B] 0F 5D /r` instruction with
ModR/M.mod=3 and XMM0–15. Bare MINPS, F3 MINSS and F2 MINSD remain distinct
and are rejected by this profile. MXCSR requires all exception masks,
DAZ/FTZ clear and reserved bits 16–31 clear; any RC and old sticky bits are
allowed. The handwritten high-register fixture checks independent lane
selection, same-lane IE-over-DE and cross-lane IE|DE aggregation. Missing SSE
or unsupported MXCSR refuses before mutation. JIT/native/CPUID, unmasked
delivery, memory forms and fault ordering remain out of scope. See
[fixture](../../tests/fixtures/x86/stage2/s2_37k_minpd_masked_register.case.json),
[plan](../plans/STAGE_2.md#s2-37k--local-long64-register-only-legacy-minpd-with-masked-status),
and [CPU contract](../specs/CPU_CONTRACT.md#local-s2-37k-register-only-minpd-with-masked-status).

The local MAXPD profile `s2-long-maxpd-reg-masked-v1` admits only the legacy
Long64 register form `66 [optional adjacent REX.R/B] 0F 5F /r`. It snapshots
both full XMM operands, selects raw RHS for NaNs and ties, applies same-lane
IE-over-DE priority and aggregates effective IE/DE across both lanes. Missing
SSE and unsupported MXCSR refuse atomically; memory and unmasked-delivery paths
remain closed. See [fixture](../../tests/fixtures/x86/stage2/s2_37l_maxpd_masked_register.case.json),
[plan](../plans/STAGE_2.md#s2-37l--local-long64-register-only-legacy-maxpd-with-masked-status),
and [CPU contract](../specs/CPU_CONTRACT.md#local-s2-37l-register-only-maxpd-with-masked-status).

## S2-39b selector-state snapshot profile

Schema-v2 `X87Snapshot` may contain `instruction_selector` and
`data_selector` as 16-bit HexU64 values. Each field is independently optional:
absence (`None`) means unavailable/unrepresented, while `0x0000000000000000`
is a known zero selector. Values above `0xffff`, `null`, malformed HexU64
values, and unknown fields are rejected. FIP/FDP remain full-width 64-bit HexU64
values.

The existing x87 state, x87/SSE snapshot, and SSE instruction profiles omit
selector fields when serializing and reject them when present. The new
`s2-long-x87-selectors-state-v1` profile requires x87 state, SSE state, and both
selectors. It admits only the exact one-byte NOP `90`, instruction budget 1,
and stop RIP immediately after the NOP. The handwritten selector fixture uses
distinct nonzero selectors and high-width FIP/FDP sentinels; its mixed physical
tag-word and raw register payloads are compared in full. This profile verifies
local state transfer only. It does not execute FXSAVE/FXRSTOR, and the C
fragment adapter, JIT, and native runner remain closed to it. See the
[fixture note](../../tests/fixtures/x86/stage2/README.md#s2-39b-selector-state-snapshot),
[CPU contract](../specs/CPU_CONTRACT.md#s2-39b--selector-state-representation-only),
and [plan](../plans/STAGE_2.md#s2-39b-lossless-selector-state-representation-no-fxsave-execution).

## S2-39c plain FXSAVE profile

The local-only `s2-long-fxsave-rip-v1` profile requires raw x87/SSE state and
both known 16-bit selectors. It accepts exactly seven code bytes in the shape
`0F AE 05 disp32`, one instruction, budget 1, and a stop RIP immediately after
the instruction. The signed displacement must resolve from next RIP to a
16-byte-aligned canonical48 data region of exactly 512 writable, non-executable
bytes. CPUID FXSR and CR0/CR4 SSE-state conditions are preconditions of the
selected target; these fields are not runtime gates here.

The fixture's `initial_bytes_hex` is the pre-instruction destination, including
sentinels in holes. Comparison masks describe fixture byte fields where
provided; execution coverage also checks the exact saved image and preserved
sentinels. The observed literal fixture is `0F AE 05 6B F9 17 00`; additional
disp32 execution is covered independently. The profile keeps the interpreter
local and does not open FXRSTOR, alternate FXSAVE forms, JIT, native execution,
CPUID execution, or physical acceptance. See the [fixture note](../../tests/fixtures/x86/stage2/README.md#s2-39c-plain-long64-rip-relative-fxsave),
[CPU contract](../specs/CPU_CONTRACT.md#s2-39c--plain-long64-rip-relative-fxsave),
and [plan](../plans/STAGE_2.md#s2-39c-exact-plain-long64-rip-relative-fxsave).

## S2-39d FXSAVE64 profile

The local-only `s2-long-fxsave64-rax-disp8-v1` profile admits exactly the five
bytes `48 0F AE 40 40`, with a 512-byte aligned canonical writable data region
at `RAX+0x40`, one-instruction budget, and stop RIP immediately after the
instruction. It requires raw x87/SSE state. FCS and FDS are independent optional
fields and do not affect this 64-bit image. The image stores full FIP/FDP at
offsets `[8,16)` and `[16,24)`. JIT, native execution, FXRSTOR and other
addressing forms remain closed. See the [fixture note](../../tests/fixtures/x86/stage2/README.md#s2-39d-long64-fxsave64-rax0x40),
[CPU contract](../specs/CPU_CONTRACT.md#s2-39d--exact-long64-fxsave64-rax0x40),
and [plan](../plans/STAGE_2.md#s2-39d-exact-long64-fxsave64-rax0x40).

## S2-39e FXRSTOR64 profile

The local-only `s2-long-fxrstor64-rbx-disp8-v1` profile admits exactly
`48 0F AE 4B 40`, one instruction, budget 1, and stop RIP immediately after
that instruction. It requires x87/SSE snapshots and a readable, non-executable
512-byte image at the wrapping architectural address `RBX+0x40`; the entire
range must be 16-byte aligned, canonical48 and non-overflowing. Selectors are
independently optional and preserved by the restore. The handwritten input
image uses nonzero bytes in ignored/reserved ranges and an arbitrary
`MXCSR_MASK`; only the immutable target mask `0x0000_ffbf` validates loaded
MXCSR. Image memory is read-only in the fixture. Interpreter execution is
allowed; JIT/native are denied. See the [fixture note](../../tests/fixtures/x86/stage2/README.md#s2-39e-long64-fxrstor64-rbx0x40),
[CPU contract](../specs/CPU_CONTRACT.md#s2-39e--exact-long64-fxrstor64-rbx0x40),
and [plan](../plans/STAGE_2.md#s2-39e-exact-long64-fxrstor64-rbx0x40).

## S2-39f FXSAVE64 CR0 gate profile

The local-only `s2-long-fxsave64-cr0-gates-v1` profile accepts exactly
`48 0F AE 40 40`, one instruction, budget 1, and stop RIP immediately after
the five-byte instruction. It requires x87/SSE snapshots and the strict
`fxsave64_cr0_controls` object with exactly the immutable boolean fields
`cr0_em` and `cr0_ts`. Both false uses the S2-39d save serializer; either true
expects vector 7 `#NM`, no error code and zero retirement. The fixture has an
aligned canonical48 writable private 512-byte destination. The fault case
compares the full CPU and RAM snapshots; the success case is checked against
the independent literal image in the fixture directory, including pending
x87 exception and MXCSR sticky flags. FXSR=1 and CR4.OSFXSR=1 are fixed
prerequisites. Combined target data-fault/gate priority, absent FXSR/OSFXSR,
other image layouts, `#MF/#XM` delivery, hardware equivalence and JIT/native/C
adapter execution remain outside scope. See the [fixture note](../../tests/fixtures/x86/stage2/README.md#s2-39f-long64-fxsave64-cr0-em-and-ts-gates),
[CPU contract](../specs/CPU_CONTRACT.md#s2-39f--bounded-fxsave64-cr0-em-and-ts-nm-gates),
and [plan](../plans/STAGE_2.md#s2-39f-bounded-fxsave64-cr0-em-and-ts-nm-gates).

## S2-39g FXRSTOR64 CR0 gate profile

The local-only `s2-long-fxrstor64-cr0-gates-v1` profile accepts exactly
`48 0F AE 4B 40`, one instruction, budget 1 and stop RIP immediately after the
five-byte instruction. It requires complete x87/SSE state and the strict
`fxrstor64_cr0_controls` object containing only immutable booleans `cr0_em` and
`cr0_ts`. Either bit true expects vector 7 `#NM`, no error code, zero retirement
and an unchanged full CPU/RAM snapshot; both false execute the S2-39e staged
restore. The readable, private 512-byte image must be aligned, canonical48,
and have valid MXCSR under the immutable target mask. It may contain ignored
MXCSR_MASK and padding values. The schema does not combine a gate with invalid
image/address/page-fault cases and makes no precedence claim for those cases.
STOP/zero-budget order and complete fetch before gate remain observable.

The result repeats controls as `snapshot.fxrstor64_cr0_controls`; result
validation requires the field for this profile, and comparison checks both
observations against each other and the input fixture. Older result schemas
omit the optional field. Interpreter is the only admitted backend. JIT/native/C
adapter, alternate layouts, FXSR=0 and CR4.OSFXSR=0 remain unsupported. See the
[fixture note](../../tests/fixtures/x86/stage2/README.md#s2-39g-long64-fxrstor64-cr0-em-and-ts-gates),
[CPU contract](../specs/CPU_CONTRACT.md#s2-39g--bounded-fxrstor64-cr0-em-and-ts-nm-gates),
and [plan](../plans/STAGE_2.md#s2-39g-bounded-fxrstor64-cr0-em-and-ts-nm-gates).

## S2-40b native SSE-only XORPD profile

The `s2-long-native-xorpd-xmm0-xmm1-v1` schema-v2 profile requires a complete
`state.sse` object for XMM0–XMM15 and exact MXCSR `0x1f80`, with no `state.x87`
member. It admits only the four code bytes `66 0F 57 C1`, one instruction,
`stop_rip = initial_state.rip + 4`, `instruction_budget = 1`, and
`expected_exit = stop_address`. The code mapping is the only fixture mapping.
`rflags_mask` selects the defined integer flags to check. Fixture expected
XMM0 bytes are literal test expectations written independently from the
interpreter; native/interpreter comparison adds a second oracle check. See the
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-40b-native-xorpd-xmm0-xmm1)
and [CPU contract](../specs/CPU_CONTRACT.md#s2-40b--bounded-sse-only-native-xorpd-oracle).

## S2-38c exact MINSS invalid-controls profile

The schema-v2 profile `s2-long-minss-invalid-controls-v1` requires all five
explicit booleans in `minss_invalid_controls`: `sse_available`, `cr0_em`,
`cr0_ts`, `cr4_osfxsr`, and `cr4_osxmmexcpt`. They are immutable inputs and
are repeated in `snapshot.minss_invalid_controls`; validation requires the
result field for both versioned MINSS control profiles, and comparison checks
both outputs against each other and the bound input fixture. Older profiles
omit this optional field.
Unknown or missing controls fail closed.

Only exact bytes `f30f5dc1`, one executable code mapping, budget 1, and
`stop_rip = entry+4` are admitted. The fixture requires complete x87/SSE
snapshots. Its MXCSR has reserved bits clear, DE/ZE/OE/UE/PE masks set and
DAZ/FTZ clear; IM may be set or clear and any RC is accepted. Expected exit is
`exception` for an entry gate or arithmetic invalid exit, or `stop_address`
for completion. This fixture profile does not broaden the decoder's other
forms or permit memory access. JIT and native execution are denied.

## S2-38d exact MINSS unmasked-denormal profile

The schema-v2 profile `s2-long-minss-unmasked-denormal-v1` reuses the strict
one-instruction `F3 0F 5D C1` shape, full x87/SSE state and the same five
explicit immutable controls as S2-38c. It is separately versioned so the
S2-38c `s2-long-minss-invalid-controls-v1` fixture and MXCSR mask policy remain
unchanged. The new profile requires reserved bits zero, ZE/OE/UE/PE masks set,
and DAZ/FTZ clear; it allows IM and DM independently, any RC, and arbitrary old
sticky flags. The existing control object is echoed in the result and remains
strict against missing, extra or changed values.

New effective DE with DM=0 commits DE, suppresses XMM0, RIP and retirement, and
returns `exception` with #UD/#XM selected by OSXMMEXCPT. A prior DE bit alone is
not a new event. For NaN+subnormal operand combinations, the unit coverage
checks the Rev. 093 priority/mask matrix: both masks set reports IE only and
completes the masked result; any open mask records IE|DE and faults once.
QNaN/SNaN and operand order are crossed with IM×DM×OSXMMEXCPT. Four handwritten
fixtures record unmasked DE #UD, unmasked DE #XM, masked invalid with unmasked
DE, and an exception-free result. No JSON fields are added. JIT/native/C-adapter
remain denied; physical acceptance is not claimed. See the [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-38d-local-minss-unmasked-denormal-operand),
[CPU contract](../specs/CPU_CONTRACT.md#s2-38d--exact-minss-unmasked-denormal-operand-exit),
and [plan](../plans/STAGE_2.md#s2-38d-exact-long64-minss-xmm0xmm1-unmasked-denormal-operand-exit).

## S2-38b exact XORPD prerequisite profile

The local-only `s2-long-xorpd-prerequisites-v1` schema-v2 profile requires
the complete x87/SSE snapshots plus a top-level `xorpd_prerequisites` object
with all four explicit booleans: `sse2_available`, `cr0_em`, `cr0_ts`, and
`cr4_osfxsr`. Missing object/fields are invalid; the fields are never inferred
or defaulted. The code mapping must contain only `66 0F 57 C1`, be executable,
and use budget 1 and `stop_rip = initial_state.rip + 4`. The expected outcome may
be `exception` for a gate fault or `stop_address` for the successful path.
Interpreter-only; JIT and native execution are denied. See the [fixture note](../../tests/fixtures/x86/stage2/README.md#s2-38b-local-xorpd-prerequisite-gates),
[CPU contract](../specs/CPU_CONTRACT.md#s2-38b--точечные-sse2-prerequisite-gates-для-xorpd),
and [plan](../plans/STAGE_2.md#s2-38b-exact-xorpd-sse2-prerequisite-gates).
The v2 result repeats the same object at `snapshot.xorpd_prerequisites`; result
validation requires it for this profile, and the comparator checks left and
right values against each other and the bound fixture. Results for older
profiles omit the field.

## S2-42a native XORPD generated corpus

Generated cases use the existing schema-v2 profile
`s2-long-native-xorpd-xmm0-xmm1-v1`; fixture shape and admission do not change.
The deterministic generator version is `od-testkit-native-xorpd-v1`, with four
fixed seeds and sequences `0..256` per seed. Each case randomizes all 16 XMM
registers and retains exact bytes `66 0F 57 C1`, budget 1, stop RIP `entry+4`,
one code mapping, MXCSR `0x1f80`, and no x87 snapshot. The committed, handwritten
cases remain the source for small boundary patterns.

`od-runner --generate-native-xorpd-corpus` creates 1024 case files plus a
versioned ordered manifest in ignored `work/stage2/s2-42a-xorpd/`. The manifest
records generator version, seeds and sequence, profile, each exact JSON
SHA-256, and a corpus digest computed over the ordered digest list.
`od-runner --verify-native-xorpd-corpus` enforces strict schema, manifest/file
digest integrity, full case presence and distinct XMM input contents. It runs
the interpreter twice per case and checks the complete result against a
separate test-harness oracle that XORs operand bytes directly. Unsupported
mandatory interpreter execution or any mismatch is a nonzero failure. A single
case can be replayed with the ordinary `od-runner --fixture CASE.json --engine
interpreter --json` command. Native execution is `not_run` for this local
acceptance; it remains scheduled for the combined final hardware batch.

## S2-42b bounded native XORPD XMM-input reducer

This reducer consumes one existing, strict schema-v2 fixture for
`s2-long-native-xorpd-xmm0-xmm1-v1`. It does not add fixture fields or mutate
the source file. Each candidate is serialized and parsed again; only
`initial_state.sse.xmm[0..15]` may differ from the original. The full profile,
instruction bytes and mapping, GPR/RFLAGS/RIP, MXCSR, x87 absence, budget and
stop boundary remain schema-validated. Initial callback results use the exact
raw input digest, including accepted whitespace and object-key order; generated
candidates use their own canonical serialized digest.

`od-runner --reduce-native-xorpd CASE.json [--limit N]
[--output-dir work/stage2/s2-42b-xorpd/NAME]` runs one deterministic pass in
register order XMM0–XMM15. For each register it proposes zeroing the complete
register, then aligned 8-, 4-, 2- and 1-byte blocks, then each set bit in
byte0..15/bit7..0 order. A proposal is skipped if it is a no-op or does not
lower the total set-bit count across all 256 XMM bytes. It is accepted only
after two semantically identical callback results preserve the original
fingerprint. The fingerprint contains the exact XMM register/byte positions
that differ from the bytewise-XOR oracle; it deliberately omits wrong values.
All non-XMM state, defined flags, retirement, successful stop exit, no exception
and unchanged code mapping must match the oracle. Build/machine metadata and
timestamps/durations do not decide semantic repeatability.

The callback budget counts actual runs, defaults to 5120, permits 4 through
5120, and reserves two initial plus two final confirmations. Each candidate
uses two actual runs. The reducer makes one pass and makes no global-minimum
claim. If there is no initial XMM mismatch it writes the original, two results,
expected oracle result and report, returns `no_initial_mismatch`, and writes no
reduced fixture. Fault, Unsupported, invalid input and timeout outcomes do not
preserve a mismatch; callback infrastructure errors stop with an incomplete
report. A reduced fixture is written only after final repeated confirmation.

Output is exclusive under ignored `work/stage2/s2-42b-xorpd/` and includes the
original and SHA-256, two initial results, two final results when applicable,
expected oracle results, fingerprint, proposal/run/accept counts, initial/final
Hamming weights, source provenance, revision, toolchain, exact replay commands,
and `native=not_run`. Synthetic callbacks in tests validate the algorithm only;
they are not real CPU bug or hardware evidence. The generic S2-42 instruction
deletion, branches, memory/REP, FP faults and physical comparison requirements
remain open. See [plan](../plans/STAGE_2.md#s2-42b-bounded-xmm-input-reducer),
[report](../reports/STAGE_2_REPORT.md#s2-42b--bounded-native-xorpd-xmm-input-reducer),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-42b-native-xorpd-xmm-input-reducer).
