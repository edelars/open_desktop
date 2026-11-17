# Stage6SmpProbeV2 (S6-16e, S6-16f)

The probe image is not stored here: it is hand-encoded and reproducible, so it is built on demand.

```
python3 tools/build/build_stage6_smp_probe.py --output-dir work/stage6/smp_probe --scenario exchange
```

`--scenario` is one of `normal` (S6-16e start of the AP), `exchange` (K Fixed IPI rounds, K by `--rounds`, 16 by default), `resipi` (a second SIPI to the running AP), `bad-ipi` (an ICR to a missing APIC ID), `nmi` (an NMI to the AP). `--variant` (`normal`, `no_long_mode`, `no_flag`) breaks the guest on purpose. The output directory holds `probe.bin` (0xB000 bytes for GPA `0x2000`), `build_manifest.json` (layout, protocol, instruction listing, expected UART transcript and memory cells derived from the protocol and the SDM) and `expected_uart_n1.txt` / `expected_uart_n2.txt` (`{nonce}` is replaced by the nonce of the run).

Run it:

```
od-runner --stage6-smp-probe work/stage6/smp_probe/probe.bin --nonce TOKEN --vcpus 2 --seed 7
```

Method, quantum limits and seed ranges: `docs/testing/SMP.md` ("probe v1 / v2"); contract: `docs/specs/MACHINE_CONTRACT.md` (S6-16e, S6-16f).
