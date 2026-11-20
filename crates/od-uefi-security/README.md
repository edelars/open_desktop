# UEFI Secure Boot formats and verification

This crate contains the types and validation shared by Open Desktop firmware and the host NVRAM boundary. It has no device I/O and does not own guest or persisted state.

The base library decodes Auth2, EFI signature lists and detached PKCS#7 with borrowed data. Enabling `verify` adds RSA-2048/SHA-256 verification and certificate path checks for Auth2, with certificate dates excluded per ADR-0011. The feature uses `alloc`. The caller selects the current PK and KEK anchors allowed for the update and owns timestamp and transaction enforcement.

Run `cargo test -p od-uefi-security --features verify --offline` to test the formats and the detached signature from Microsoft Secure Boot Objects v1.7.2. [ADR-0011](../../docs/decisions/ADR-0011-secure-boot-keys.md), [fixture provenance](../../tests/fixtures/firmware/certs/README.md).
