# Stage 0 guest workload inputs

These are small project-owned workload definitions for the scenarios in [`SUPPORT_MATRIX.md`](../../../../docs/specs/SUPPORT_MATRIX.md). They contain no guest OS image or third-party test data.

## 7-Zip archive scenarios

Generate a deterministic corpus outside the checkout: a 50,000-row CSV plus 1 MiB each of repeated text and deterministic pseudorandom bytes (about 3.1 MiB total):

```sh
python3 tests/fixtures/guest/stage0/generate_archive_corpus.py --output work/tmp/stage0-archive-input-run1
```

Use a new output directory for each run; the generator refuses to write into a nonempty directory.

Run the single-thread and two-thread scenarios against the same generated directory using 7-Zip 26.05 x64. Fix archive format to 7z, compression method to LZMA2, dictionary to 32 MiB, and thread count to 1 or 2. Record elapsed time, peak process memory, archive size, and the generated manifest. Validate each archive with `7z t`, extract it to a clean directory, and compare the extracted manifest with the input. These workloads test correctness and comparative execution; no speed target is claimed.

## Node.js workload

Run `node tests/fixtures/guest/stage0/node_workload.mjs` with Node.js 24.22.1 x64. The program uses a fixed seed and integer-only arithmetic and prints one JSON result. The checked result is stored in `node_workload.expected.json`:

```json
{"count":5000000,"digest":3355974656,"seed":1831565813}
```

Record cold process startup and steady workload time separately; check exact JSON output.

## LibreOffice Writer scenario

Open `writer-source.txt` in Writer 26.8.1 x64, append the exact line `Revision: S0-WRITER-001`, and save a copy as ODT. Validate that the saved document extracts the original sentinel and revision line. Record open-to-save time and peak process memory. The input file hash is listed in `SHA256SUMS`.

## Blender CPU scene

Run `blender-scene.py` with Blender 5.2.2 LTS x64 in background mode and render with Cycles CPU at 640×360 and 16 samples. Record wall time, peak memory, and output image hash. The scene is generated from the script so it does not depend on external assets. The image hash is a per-host reference; cross-host pixel equality has not been established.

## Provenance and verification

All inputs are authored for this repository and use only generated content. `SHA256SUMS` fixes the committed workload definitions. The archive corpus generator emits a separate `SHA256SUMS` for generated data. Application binaries, Windows VM, final memory/time budgets, and cross-host output tolerances remain to be recorded during S0-02/S0-06 after the Windows VM launch gate.
