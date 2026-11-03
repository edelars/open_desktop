#!/usr/bin/env python3
"""Check control V1 RUN/STOP behavior and RUN-ID high-watermark in one VMHost."""

import argparse
import socket
import struct
import subprocess
import sys


MAGIC = b"ODCP"
VERSION = 1
HELLO, READY, RUN, RUN_ACK, STOP, STOP_ACK, RESULT, ERROR = 1, 2, 3, 4, 5, 6, 7, 255
STATUS_OK = 0
STATUS_BUSY = 3
STATUS_INVALID_REQUEST_ID = 7
SCOPE_REJECTED_COMMAND = 1
PROCESS_TIMEOUT_SECONDS = 5
IO_TIMEOUT_SECONDS = 3


def frame(kind, request_id, payload=b""):
    return MAGIC + struct.pack("<HHII", VERSION, kind, request_id, len(payload)) + payload


def read_exact(channel, length):
    data = bytearray()
    while len(data) < length:
        chunk = channel.recv(length - len(data))
        if not chunk:
            raise EOFError("VMHost closed the control channel during a frame")
        data.extend(chunk)
    return bytes(data)


def read_frame(channel):
    header = read_exact(channel, 16)
    magic, version, kind, request_id, payload_length = struct.unpack("<4sHHII", header)
    if magic != MAGIC or version != VERSION or payload_length > 8192:
        raise RuntimeError("invalid control V1 response frame")
    return kind, request_id, read_exact(channel, payload_length)


def run_payload(code, budget, registers, rip, rflags):
    prefix = struct.pack("<IQ6Q", len(code), budget, *registers, rip, rflags)
    if len(prefix) != 60:
        raise AssertionError("RUN prefix must be exactly 60 bytes")
    return prefix + code


def launch_vmhost(path):
    parent, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        process = subprocess.Popen(
            [path, "--socket-fd", str(child.fileno())],
            pass_fds=(child.fileno(),),
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except Exception:
        parent.close()
        child.close()
        raise
    child.close()
    parent.settimeout(IO_TIMEOUT_SECONDS)
    return process, parent


def expect_frame(channel, expected, label):
    actual = read_frame(channel)
    if actual != expected:
        raise RuntimeError(f"{label}: expected {expected!r}, received {actual!r}")


def send_run(channel, request_id, code, budget, registers, rflags):
    channel.sendall(frame(RUN, request_id, run_payload(code, budget, registers, 0, rflags)))


def expect_rejected_run(channel, request_id, status, label):
    payload = struct.pack("<III", status, SCOPE_REJECTED_COMMAND, RUN)
    expect_frame(channel, (ERROR, request_id, payload), label)


def check_stopped_result(payload, registers, rflags):
    if len(payload) != 64:
        raise RuntimeError(f"STOP RESULT must be exactly 64 bytes, got {len(payload)}")
    result = struct.unpack("<6QIIQ", payload)
    expected_prefix = (*registers, 0, rflags, 2, 0)
    if result[:8] != expected_prefix:
        raise RuntimeError(
            f"STOP RESULT architectural state/reason/reserved mismatch: {result[:8]!r}"
        )
    # Retired instructions are invocation-local but may be zero when STOP wins
    # before guest entry. Do not require a positive count after STOP.


def check_run_stop_id(vmhost):
    process, channel = launch_vmhost(vmhost)
    code = bytes.fromhex("75fe")  # JNZ -2; ZF clear keeps the invocation active.
    registers = (0x11, 0x22, 0x33, 0x44)
    rflags = 0x202
    try:
        channel.sendall(frame(HELLO, 0))
        expect_frame(channel, (READY, 0, struct.pack("<II", 1, 0)), "V1 READY")

        send_run(channel, 1, code, 0xFFFFFFFFFFFFFFFF, registers, rflags)
        expect_frame(channel, (RUN_ACK, 1, struct.pack("<I", STATUS_OK)), "RUN 1 ACK")

        # All commands are sent only after RUN_ACK, while the infinite loop is
        # active; the exact replies establish rejection precedence and ordering.
        channel.sendall(frame(STOP, 2))
        expect_frame(
            channel,
            (STOP_ACK, 2, struct.pack("<I", STATUS_INVALID_REQUEST_ID)),
            "wrong-ID STOP 2",
        )

        send_run(channel, 1, code, 1, registers, rflags)
        expect_rejected_run(channel, 1, STATUS_INVALID_REQUEST_ID, "duplicate RUN 1")

        send_run(channel, 3, code, 1, registers, rflags)
        expect_rejected_run(channel, 3, STATUS_BUSY, "active RUN 3 BUSY")

        send_run(channel, 2, code, 1, registers, rflags)
        expect_rejected_run(channel, 2, STATUS_INVALID_REQUEST_ID, "decreasing RUN 2")

        channel.sendall(frame(STOP, 1))
        expect_frame(channel, (STOP_ACK, 1, struct.pack("<I", STATUS_OK)), "STOP 1 ACK")
        terminal = read_frame(channel)
        if terminal[:2] != (RESULT, 1):
            raise RuntimeError(f"RUN 1 terminal record expected after STOP_ACK, got {terminal!r}")
        check_stopped_result(terminal[2], registers, rflags)

        # BUSY still consumed ID 3, so it is stale after the active run drains.
        send_run(channel, 3, code, 1, registers, rflags)
        expect_rejected_run(channel, 3, STATUS_INVALID_REQUEST_ID, "stale BUSY RUN 3")

        # ID 4 proves the session remains usable, with fresh state and budget.
        fresh_registers = (0xA1, 0xA2, 0xA3, 0xA4)
        fresh_flags = 0x203
        send_run(channel, 4, code, 1, fresh_registers, fresh_flags)
        expect_frame(channel, (RUN_ACK, 4, struct.pack("<I", STATUS_OK)), "RUN 4 ACK")
        expected_result = struct.pack("<6QIIQ", *fresh_registers, 0, fresh_flags, 1, 0, 1)
        expect_frame(channel, (RESULT, 4, expected_result), "RUN 4 RESULT")

        channel.close()
        channel = None
        if process.wait(timeout=PROCESS_TIMEOUT_SECONDS) != 0:
            raise RuntimeError("VMHost exited unsuccessfully after idle EOF")
    finally:
        if channel is not None:
            channel.close()
        if process.poll() is None:
            process.kill()
            process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("vmhost", help="path to the built VMHost binary")
    args = parser.parse_args()
    check_run_stop_id(args.vmhost)
    print("VMHost control V1 smoke passed: active STOP and RUN-ID high-watermark")


if __name__ == "__main__":
    try:
        main()
    except (EOFError, OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"vmhost_control_v1_run_stop_id_smoke: {error}", file=sys.stderr)
        sys.exit(1)
