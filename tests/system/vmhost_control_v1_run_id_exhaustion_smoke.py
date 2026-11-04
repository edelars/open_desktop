#!/usr/bin/env python3
"""Check V1 RUN-ID exhaustion and fresh-connection high-watermark reset."""

import argparse
import socket
import struct
import subprocess
import sys


MAGIC = b"ODCP"
VERSION = 1
HELLO, READY, RUN, RUN_ACK, RESULT, ERROR = 1, 2, 3, 4, 7, 255
MAX_REQUEST_ID = 0xFFFFFFFF
STATUS_OK = 0
STATUS_INVALID_REQUEST_ID = 7
ERROR_SCOPE_REJECTED_COMMAND = 1
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


def run_payload(code):
    # One decoder-supported ADD RAX,RBX reaches code_length and completes.
    prefix = struct.pack("<IQ6Q", len(code), 1, 0x11, 0x22, 0x33, 0x44, 0, 0x202)
    if len(prefix) != 60:
        raise AssertionError("RUN prefix must be exactly 60 bytes")
    return prefix + code


def accepted_run(channel, request_id, label):
    code = bytes.fromhex("4803c3")
    channel.sendall(frame(RUN, request_id, run_payload(code)))
    expect_frame(
        channel,
        (RUN_ACK, request_id, struct.pack("<I", STATUS_OK)),
        f"{label} RUN_ACK",
    )
    result = struct.pack("<6QIIQ", 0x55, 0x22, 0x33, 0x44, 3, 0x206, 0, 0, 1)
    expect_frame(channel, (RESULT, request_id, result), f"{label} RESULT")


def stop_vmhost(process):
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=PROCESS_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=PROCESS_TIMEOUT_SECONDS)


def check_exhaustion_and_fresh_connection(vmhost):
    process, channel = launch_vmhost(vmhost)
    try:
        channel.sendall(frame(HELLO, 0))
        expect_frame(channel, (READY, 0, struct.pack("<II", 1, 0)), "first V1 READY")

        accepted_run(channel, MAX_REQUEST_ID, "maximum RUN ID")

        # The maximum ID itself is now stale. Reading its exact ERROR before
        # sending the next RUN also catches any extra response to the accepted
        # maximum-ID invocation.
        exhausted_error = struct.pack(
            "<III", STATUS_INVALID_REQUEST_ID, ERROR_SCOPE_REJECTED_COMMAND, RUN
        )
        channel.sendall(frame(RUN, MAX_REQUEST_ID, run_payload(bytes.fromhex("4803c3"))))
        expect_frame(
            channel,
            (ERROR, MAX_REQUEST_ID, exhausted_error),
            "duplicate maximum RUN ID",
        )

        # RUN IDs cannot wrap on this connection. ID 1 is stale too; its exact
        # ERROR is the next frame and catches any extra response to max-ID.
        channel.sendall(frame(RUN, 1, run_payload(bytes.fromhex("4803c3"))))
        expect_frame(channel, (ERROR, 1, exhausted_error), "post-exhaustion RUN ID 1")

        channel.close()
        channel = None
        if process.wait(timeout=PROCESS_TIMEOUT_SECONDS) != 0:
            raise RuntimeError("VMHost exited unsuccessfully after idle EOF")
    finally:
        if channel is not None:
            channel.close()
        stop_vmhost(process)

    # The protocol scopes the mark to a helper/connection. A newly launched
    # VMHost with a fresh socket pair therefore starts from request ID 0.
    process, channel = launch_vmhost(vmhost)
    try:
        channel.sendall(frame(HELLO, 0))
        expect_frame(channel, (READY, 0, struct.pack("<II", 1, 0)), "fresh V1 READY")
        accepted_run(channel, 1, "fresh-connection RUN ID 1")

        channel.close()
        channel = None
        if process.wait(timeout=PROCESS_TIMEOUT_SECONDS) != 0:
            raise RuntimeError("fresh VMHost exited unsuccessfully after idle EOF")
    finally:
        if channel is not None:
            channel.close()
        stop_vmhost(process)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("vmhost", help="path to the built VMHost binary")
    args = parser.parse_args()
    check_exhaustion_and_fresh_connection(args.vmhost)
    print(
        "VMHost control V1 smoke passed: RUN-ID exhaustion does not wrap and "
        "fresh connection accepts ID 1"
    )


if __name__ == "__main__":
    try:
        main()
    except (EOFError, OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"vmhost_control_v1_run_id_exhaustion_smoke: {error}", file=sys.stderr)
        sys.exit(1)
