# Control protocol v0 overview

The S0 launcher and VMHost use a local inherited Unix `socketpair`. The channel supports a version handshake, one fragment execution request, cooperative stop, and a fixed result record. It is a single-request smoke protocol, not a persistent VM session or general RPC layer.

The normative frame and payload layout is maintained in [`protocols/control/CONTROL_PROTOCOL_V0.md`](../../protocols/control/CONTROL_PROTOCOL_V0.md). The cross-language execution and ownership boundary is described separately in [HOST_ABI_V0.md](HOST_ABI_V0.md). The current demo and reproduction steps are in [apps/vmhost/README.md](../../apps/vmhost/README.md).

The implementation has been exercised locally on the primary Apple M3 Max. Launcher integration checks confirmed unsupported-version rejection, invalid-code errors, disconnect before RESULT, cooperative stop, and two concurrently executing VMHost processes using independent channels; killing one active helper did not prevent the second from accepting STOP and returning its result. Full GUI lifecycle, sustained concurrent VM instances, and production signing remain unverified.
