#!/usr/bin/env python3
"""Check immutable-code rejection and RUN ID advancement in one VMHost process."""

import argparse
import socket
import struct
import subprocess
import sys


MAGIC = b"ODCP"
VERSION = 1
HELLO, READY, RUN, RUN_ACK, RESULT, ERROR = 1, 2, 3, 4, 7, 255
MAX_PAYLOAD = 8192
MAX_CODE = 4096
PROCESS_TIMEOUT_SECONDS = 5
TERMINATION_GRACE_SECONDS = 0.5
IO_TIMEOUT_SECONDS = 3
STATUS_INVALID_REQUEST_ID = 7
STATUS_CODE_MISMATCH = 8
ERROR_SCOPE_REJECTED_COMMAND = 1


def frame(kind, request_id, payload=b""):
    if len(payload) > MAX_PAYLOAD:
        raise ValueError("payload exceeds control V1 limit")
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
    if magic != MAGIC or version != VERSION or payload_length > MAX_PAYLOAD:
        raise RuntimeError("invalid control V1 response frame")
    return kind, request_id, read_exact(channel, payload_length)


def run_payload(code, budget, registers, rip, rflags):
    if len(code) > MAX_CODE:
        raise ValueError("code exceeds control V1 limit")
    prefix = struct.pack("<IQ6Q", len(code), budget, *registers, rip, rflags)
    if len(prefix) != 60:
        raise AssertionError("RUN prefix must be exactly 60 bytes")
    return prefix + code


def expected_result(registers, rip, rflags, reason, retired):
    result = struct.pack("<6QIIQ", *registers, rip, rflags, reason, 0, retired)
    if len(result) != 64:
        raise AssertionError("RESULT must be exactly 64 bytes")
    return result


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


def run_once(channel, request_id, code, budget, registers, rflags, expected):
    channel.sendall(frame(RUN, request_id, run_payload(code, budget, registers, 0, rflags)))
    expect_frame(channel, (RUN_ACK, request_id, struct.pack("<I", 0)), f"RUN {request_id} ACK")
    # Consume the terminal frame before sending another RUN, pinning wire order.
    expect_frame(channel, (RESULT, request_id, expected), f"RUN {request_id} RESULT")


def stop_vmhost(process):
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=TERMINATION_GRACE_SECONDS)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=PROCESS_TIMEOUT_SECONDS)


def check_code_mismatch(vmhost):
    process, channel = launch_vmhost(vmhost)
    original_code = bytes.fromhex("4803c3")  # add rax, rbx
    different_code = bytes.fromhex("482bc3")  # sub rax, rbx; same length
    try:
        channel.sendall(frame(HELLO, 0))
        expect_frame(channel, (READY, 0, struct.pack("<II", 1, 0)), "V1 READY")

        run_once(
            channel,
            1,
            original_code,
            1,
            (0x11, 0x22, 0x33, 0x44),
            0x202,
            expected_result((0x55, 0x22, 0x33, 0x44), 3, 0x206, 0, 1),
        )

        mismatch_error = struct.pack(
            "<III", STATUS_CODE_MISMATCH, ERROR_SCOPE_REJECTED_COMMAND, RUN
        )
        invalid_id_error = struct.pack(
            "<III", STATUS_INVALID_REQUEST_ID, ERROR_SCOPE_REJECTED_COMMAND, RUN
        )

        channel.sendall(
            frame(
                RUN,
                2,
                run_payload(different_code, 1, (0x51, 0x52, 0x53, 0x54), 0, 0x246),
            )
        )
        # ERROR is the sole response to this rejected command and precedes the
        # next response. Its exact payload identifies scope 1 for a RUN.
        expect_frame(channel, (ERROR, 2, mismatch_error), "RUN 2 CODE_MISMATCH")

        # ID 2 was consumed before semantic admission; a retry must be rejected
        # as stale even though this request carries the original code.
        channel.sendall(
            frame(RUN, 2, run_payload(original_code, 1, (0x61, 0x62, 0x63, 0x64), 0, 0x202))
        )
        expect_frame(channel, (ERROR, 2, invalid_id_error), "RUN 2 INVALID_REQUEST_ID")

        # A fresh-ID retry of the changed code must still mismatch, proving the
        # original code binding was not replaced by RUN 2.
        channel.sendall(
            frame(
                RUN,
                3,
                run_payload(different_code, 1, (0x71, 0x72, 0x73, 0x74), 0, 0x202),
            )
        )
        expect_frame(channel, (ERROR, 3, mismatch_error), "RUN 3 CODE_MISMATCH")

        # RUN 3's rejection also advances the mark, so repeating its ID fails.
        channel.sendall(
            frame(
                RUN,
                3,
                run_payload(different_code, 1, (0x81, 0x82, 0x83, 0x84), 0, 0x202),
            )
        )
        expect_frame(channel, (ERROR, 3, invalid_id_error), "RUN 3 INVALID_REQUEST_ID")

        # The original binding remains usable with fresh invocation state.
        run_once(
            channel,
            4,
            original_code,
            1,
            (0xA1, 0xA2, 0xA3, 0xA4),
            0x246,
            expected_result((0x145, 0xA2, 0xA3, 0xA4), 3, 0x202, 0, 1),
        )

        # EOF between frames while idle is the protocol's normal teardown path.
        channel.close()
        channel = None
        if process.wait(timeout=PROCESS_TIMEOUT_SECONDS) != 0:
            raise RuntimeError("VMHost exited unsuccessfully after idle EOF")
    finally:
        if channel is not None:
            channel.close()
        stop_vmhost(process)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("vmhost", help="path to the built VMHost binary")
    args = parser.parse_args()
    check_code_mismatch(args.vmhost)
    print(
        "VMHost control V1 smoke passed: immutable-code mismatch rejection "
        "preserves binding and RUN ID high-watermark"
    )


if __name__ == "__main__":
    try:
        main()
    except (EOFError, OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"vmhost_control_v1_code_mismatch_smoke: {error}", file=sys.stderr)
        sys.exit(1)
