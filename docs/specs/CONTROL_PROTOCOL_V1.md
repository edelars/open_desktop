# Control protocol v1 overview

The proposed opt-in S3-38f channel retains one immutable S0-09 session in one
VMHost for sequential same-code RUN requests, with fresh invocation state and
cooperative STOP. The normative wire format, statuses and lifecycle are defined
only in [protocols/control/CONTROL_PROTOCOL_V1.md](../../protocols/control/CONTROL_PROTOCOL_V1.md).

[ADR-0005](../decisions/ADR-0005-persistent-same-code-session.md) fixes ownership
and compatibility boundaries; [ADR-0003](../decisions/ADR-0003-event-driven-vmhost-completion.md)
requires event-driven completion. The separate
[session C ABI contract](HOST_SESSION_ABI_V1.md) defines the owner proxy and
per-invocation admission/completion boundary. [Control v0](CONTROL_PROTOCOL_V0.md) remains
the existing single-request path. This document records a protocol design,
not implemented transport or runtime acceptance evidence.
