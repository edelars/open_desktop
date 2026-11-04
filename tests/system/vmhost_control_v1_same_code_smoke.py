#!/usr/bin/env python3
"""Check two sequential control V1 RUNs in one VMHost process."""

import argparse
import socket
import struct
import subprocess
import sys


MAGIC = b"ODCP"
VERSION = 1
HELLO, READY, RUN, RUN_ACK, RESULT = 1, 2, 3, 4, 7
MAX_PAYLOAD = 8192
MAX_CODE = 4096
PROCESS_TIMEOUT_SECONDS = 5
IO_TIMEOUT_SECONDS = 3


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
    # Reading the next frame here pins terminal ordering before the next RUN.
    expect_frame(channel, (RESULT, request_id, expected), f"RUN {request_id} RESULT")


def check_repeated_same_code(vmhost):
    process, channel = launch_vmhost(vmhost)
    code = bytes.fromhex("4803c3 4803c3")
    try:
        channel.sendall(frame(HELLO, 0))
        expect_frame(channel, (READY, 0, struct.pack("<II", 1, 0)), "V1 READY")

        # One ADD retires before the first invocation's distinct one-instruction
        # budget expires; the second invocation executes both ADDs to code end.
        run_once(
            channel,
            1,
            code,
            1,
            (0x11, 0x22, 0x33, 0x44),
            0x202,
            expected_result((0x55, 0x22, 0x33, 0x44), 3, 0x206, 1, 1),
        )
        run_once(
            channel,
            2,
            code,
            2,
            (0xA1, 0xA2, 0xA3, 0xA4),
            0x246,
            expected_result((0x1E9, 0xA2, 0xA3, 0xA4), 6, 0x202, 0, 2),
        )

        # EOF between frames while idle is the protocol's normal teardown path.
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
    check_repeated_same_code(args.vmhost)
    print("VMHost control V1 smoke passed: same-code sequential RUNs with fresh state and budgets")


if __name__ == "__main__":
    try:
        main()
    except (EOFError, OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"vmhost_control_v1_same_code_smoke: {error}", file=sys.stderr)
        sys.exit(1)
