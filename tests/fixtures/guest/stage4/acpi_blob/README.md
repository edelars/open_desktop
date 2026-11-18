# Stage 4 ACPI blob fixture (S4-31d, S4-31g)

`stage4_acpi_blob.bin` is the ACPI table blob of the stage-4 board (`pci_ecam`, `acpi_pm`,
`acpi_tables`; blob version 2, 1104 bytes: RSDP rev 2, DSDT with `CPU0` and `\_S5`, MADT, FADT, MCFG, XSDT and the FACS segment). It is not hand-written: it is the `.acpi_blob`
section of the freestanding `od-acpi-blob` binary (`firmware/uefi`, target
`x86_64-unknown-none`), cut out by `tools/build/build_stage4_acpi_blob.py`. Format:
`firmware/uefi/src/acpi/blob.rs`.

Current fixture SHA-256: `6fc25b091348d8ad314bdd0f4f579ec9dd3969c961020a7a61d2daeb8dc26d4d`. The
2026-11-08 [S4-31g evidence record](../../../../../docs/reports/data/stage4/s4-31g-full-blob.json)
preserves the earlier 1072-byte pre-CPU0 blob; the current 1104-byte replacement is documented
in the [S4-31b CPU namespace follow-up](../../../../../docs/reports/STAGE_4_S4-31B_2026-11-08.md).

Three checks keep it honest:

- `firmware/uefi` test `fixture_equals_the_runtime_blob` compares it with the run-time
  `build_blob(STAGE4_DEVICES)`;
- `python3 tools/build/build_stage4_acpi_blob.py --compare tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin`
  rebuilds the ELF and compares the extracted section with it;
- `od-machine` test `acpi_blob::tests::stage4_blob_is_built_for_this_boards_manifest` validates it
  against the manifest hash of the real board configuration.

When a serializer, the manifest or the layout changes, regenerate it:

```sh
python3 tools/build/build_stage4_acpi_blob.py
cp work/stage4/acpi_blob/stage4_acpi_blob.bin tests/fixtures/guest/stage4/acpi_blob/
```
