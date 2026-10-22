# Control Protocol v0

This file is the normative byte-level specification for the local launcher ↔ VMHost control channel. The transport is one connected Unix stream socket created with `socketpair`; the launcher passes the helper endpoint as file descriptor 3 when it starts VMHost. Each connection serves one fragment request and then closes.

## Encoding and frame

All integers are unsigned little-endian. A stream is a sequence of frames; readers and writers must handle partial system calls and must not assume one read or write equals one frame.

| Offset | Size | Field | Required value / meaning |
|---:|---:|---|---|
| 0 | 4 | Magic | ASCII `ODCP` |
| 4 | 2 | Protocol version | `0` |
| 6 | 2 | Kind | Message kind from the table below |
| 8 | 4 | Request ID | `0` for handshake; nonzero for a request and its responses |
| 12 | 4 | Payload length | Number of payload bytes; maximum `8192` |
| 16 | length | Payload | Message-specific data |

Reject an invalid magic, unsupported version, unknown kind, oversized payload, or truncated frame. The receiver closes the connection after a framing error. VMHost stops an active fragment if the launcher disconnects or the control channel fails. The launcher reports helper startup, handshake, transport, and helper-exit failures to its caller. On failure it sends SIGTERM, waits up to 500 ms, then sends SIGKILL if needed, and reaps the helper.

## Message kinds

| Value | Name | Direction | Payload |
|---:|---|---|---|
| 1 | HELLO | Launcher → VMHost | Empty; request ID `0` |
| 2 | READY | VMHost → Launcher | `u32` host ABI version; request ID `0` |
| 3 | RUN | Launcher → VMHost | Run request below |
| 4 | RUN_ACK | VMHost → Launcher | `u32` host ABI status |
| 5 | STOP | Launcher → VMHost | Empty; RUN request ID |
| 6 | STOP_ACK | VMHost → Launcher | `u32` host ABI status |
| 7 | RESULT | VMHost → Launcher | Result record below |
| 255 | ERROR | Either direction | `u32` status when available; the connection may close on a framing/version error |

The successful sequence begins with `HELLO(0)`, `READY(0)`, `RUN(n)`, `RUN_ACK(n)`, then `RESULT(n)`. The launcher may send `STOP(n)` immediately after `RUN_ACK(n)`; VMHost retains that request if execution has not entered the host ABI yet. If STOP is processed before natural completion, VMHost sends `STOP_ACK(n)` and then `RESULT(n)`. If natural completion wins the race, `RESULT(n)` may arrive before a STOP_ACK or without one; the launcher treats RESULT as final and must not report the completed run as failed. A peer must not reuse a request ID within one connection. This v0 implementation supports one RUN per connection.

## RUN payload

The fixed prefix is 60 bytes, followed by the code bytes. `code_length` must not exceed 4096 and total payload length must equal `60 + code_length`.

| Offset | Size | Field |
|---:|---:|---|
| 0 | 4 | Code length in bytes |
| 4 | 8 | Instruction budget |
| 12 | 8 | RAX |
| 20 | 8 | RCX |
| 28 | 8 | RDX |
| 36 | 8 | RBX |
| 44 | 8 | Initial RIP, byte offset into code |
| 52 | 8 | Initial RFLAGS |
| 60 | variable | x86 instruction bytes |

The fragment semantics and valid entry/branch boundaries are defined by [CPU_CONTRACT.md](../../docs/specs/CPU_CONTRACT.md). RUN_ACK reports whether the host accepted and started the request; it is not the execution result.

## RESULT payload

The result is exactly 64 bytes.

| Offset | Size | Field |
|---:|---:|---|
| 0 | 8 | RAX |
| 8 | 8 | RCX |
| 16 | 8 | RDX |
| 24 | 8 | RBX |
| 32 | 8 | RIP |
| 40 | 8 | RFLAGS |
| 48 | 4 | Exit reason: `0` code end, `1` instruction budget, `2` cooperative stop |
| 52 | 4 | Reserved; zero |
| 56 | 8 | Instructions executed |

STOP is cooperative and takes effect at a guest instruction boundary. A successful STOP_ACK means the stop flag was accepted; RESULT remains the completion record. RESULT can win a race with STOP_ACK when the fragment completes naturally. A nonzero status means the operation was rejected or could not be performed and does not promise a RESULT.

## Compatibility

Version `0` is the only version implemented by this smoke protocol. A peer must not guess at a different version or reinterpret unknown fields. A future incompatible frame or payload change requires a new protocol version. No retry, replay, multiplexing, persistent session, authentication, or remote transport is defined; the socketpair is local to the launcher and its child process.
