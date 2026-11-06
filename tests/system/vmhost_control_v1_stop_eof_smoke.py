#!/usr/bin/env python3
"""Check V1 STOP isolation and bounded VMHost teardown on active-run EOF."""

import argparse
import socket
import struct
import subprocess
import sys
from pathlib import Path


MAGIC = b"ODCP"
VERSION = 1
HELLO, READY, RUN, RUN_ACK, STOP, STOP_ACK, RESULT = 1, 2, 3, 4, 5, 6, 7
MAX_PAYLOAD = 8192
MAX_CODE = 4096
PROCESS_TIMEOUT_SECONDS = 5
IO_TIMEOUT_SECONDS = 3
LOOP_CODE = bytes.fromhex("75fe")


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


def start_v1(path):
    process, channel = launch_vmhost(path)
    try:
        channel.sendall(frame(HELLO, 0))
        expect_frame(channel, (READY, 0, struct.pack("<II", 1, 0)), "V1 READY")
    except Exception:
        channel.close()
        if process.poll() is None:
            process.kill()
            process.wait()
        raise
    return process, channel


def begin_run(channel, request_id, code, budget, registers, rflags):
    channel.sendall(frame(RUN, request_id, run_payload(code, budget, registers, 0, rflags)))
    expect_frame(channel, (RUN_ACK, request_id, struct.pack("<I", 0)), f"RUN {request_id} ACK")


def check_stop_is_invocation_local(vmhost):
    process, channel = start_v1(vmhost)
    initial_registers = (0x11, 0x22, 0x33, 0x44)
    try:
        # ZF is clear, so JNZ -2 is an admitted infinite self-loop.
        begin_run(channel, 1, LOOP_CODE, (1 << 64) - 1, initial_registers, 0x2)
        channel.sendall(frame(STOP, 1))
        expect_frame(channel, (STOP_ACK, 1, struct.pack("<I", 0)), "STOP 1 ACK")

        kind, request_id, result = read_frame(channel)
        if kind != RESULT or request_id != 1 or len(result) != 64:
            raise RuntimeError("stopped RUN 1 did not produce its 64-byte RESULT")
        rax, rcx, rdx, rbx, rip, rflags, reason, reserved, _retired = struct.unpack(
            "<6QIIQ", result
        )
        if (rax, rcx, rdx, rbx) != initial_registers:
            raise RuntimeError("stopped RUN 1 changed a GPR")
        if rip != 0 or rflags != 0x2 or reason != 2 or reserved != 0:
            raise RuntimeError("RUN 1 RESULT does not report terminal cooperative STOP semantics")

        # A fresh invocation has ZF set: the same JNZ falls through and retires once.
        second_registers = (0xA1, 0xA2, 0xA3, 0xA4)
        begin_run(channel, 2, LOOP_CODE, 1, second_registers, 0x42)
        kind, request_id, result = read_frame(channel)
        expected_result = struct.pack("<6QIIQ", *second_registers, 2, 0x42, 0, 0, 1)
        if (kind, request_id, result) != (RESULT, 2, expected_result):
            raise RuntimeError("RUN 2 did not complete normally with fresh state and budget after STOP")

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


def check_eof_during_run_tears_down(vmhost):
    process, channel = start_v1(vmhost)
    try:
        # RUN_ACK establishes admission before the peer closes during the loop.
        begin_run(channel, 1, LOOP_CODE, (1 << 64) - 1, (0x11, 0x22, 0x33, 0x44), 0x2)
        channel.close()
        channel = None
        try:
            return_code = process.wait(timeout=PROCESS_TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired as error:
            raise RuntimeError("VMHost did not tear down within the bounded EOF window") from error
        if return_code != 0:
            raise RuntimeError(f"VMHost exited unsuccessfully after active-run EOF: {return_code}")
    finally:
        if channel is not None:
            channel.close()
        if process.poll() is None:
            process.kill()
            process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "vmhost", nargs="?", default=str(Path(__file__).resolve().parents[2] / "apps" / ".build" / "debug" / "od-vmhost"),
        help="path to the built VMHost V1 binary (default: apps/.build/debug/od-vmhost)",
    )
    args = parser.parse_args()
    check_stop_is_invocation_local(args.vmhost)
    check_eof_during_run_tears_down(args.vmhost)
    print("VMHost control V1 smoke passed: STOP isolation and bounded active-run EOF teardown")


if __name__ == "__main__":
    try:
        main()
    except (EOFError, OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"vmhost_control_v1_stop_eof_smoke: {error}", file=sys.stderr)
        sys.exit(1)
