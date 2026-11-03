#!/usr/bin/env python3
"""Prove V1 VMHost process isolation after one helper receives SIGSEGV."""

import argparse
import signal
import socket
import struct
import subprocess
import sys


MAGIC = b"ODCP"
VERSION = 1
HELLO, READY, RUN, RUN_ACK, STOP, STOP_ACK, RESULT = 1, 2, 3, 4, 5, 6, 7
MAX_PAYLOAD = 8192
IO_TIMEOUT_SECONDS = 3
PROCESS_TIMEOUT_SECONDS = 3
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
    prefix = struct.pack("<IQ6Q", len(code), budget, *registers, rip, rflags)
    if len(prefix) != 60:
        raise AssertionError("RUN prefix must be exactly 60 bytes")
    return prefix + code


def start_vmhost(path):
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
    process, channel = start_vmhost(path)
    try:
        channel.sendall(frame(HELLO, 0))
        expect_frame(channel, (READY, 0, struct.pack("<II", 1, 0)), "V1 READY")
    except Exception:
        channel.close()
        if process.poll() is None:
            process.kill()
            process.wait(timeout=PROCESS_TIMEOUT_SECONDS)
        raise
    return process, channel


def begin_run(channel, request_id, registers, rflags):
    channel.sendall(frame(
        RUN, request_id,
        run_payload(LOOP_CODE, (1 << 64) - 1, registers, 0, rflags),
    ))
    expect_frame(channel, (RUN_ACK, request_id, struct.pack("<I", 0)), f"RUN {request_id} ACK")


def check_result(channel, request_id, expected_registers, expected_rip, expected_flags,
                 expected_reason, expected_retired=None):
    kind, response_id, result = read_frame(channel)
    if kind != RESULT or response_id != request_id or len(result) != 64:
        raise RuntimeError(f"RUN {request_id} did not produce its full 64-byte RESULT")
    rax, rcx, rdx, rbx, rip, rflags, reason, reserved, retired = struct.unpack(
        "<6QIIQ", result
    )
    if (rax, rcx, rdx, rbx, rip, rflags, reason, reserved) != (
        *expected_registers, expected_rip, expected_flags, expected_reason, 0,
    ):
        raise RuntimeError(f"RUN {request_id} returned unexpected architectural state")
    if expected_retired is not None and retired != expected_retired:
        raise RuntimeError(f"RUN {request_id} retired {retired} instructions, expected {expected_retired}")


def require_exit(process, expected, label):
    try:
        status = process.wait(timeout=PROCESS_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired as error:
        raise RuntimeError(f"{label} did not exit within the bounded wait") from error
    if status != expected:
        raise RuntimeError(f"{label} exited with status {status}, expected {expected}")


def check_isolation(vmhost):
    children = []
    channels = []
    try:
        helper_a, channel_a = start_v1(vmhost)
        children.append(helper_a)
        channels.append(channel_a)
        helper_b, channel_b = start_v1(vmhost)
        children.append(helper_b)
        channels.append(channel_b)

        state_a = (0xA111, 0xA222, 0xA333, 0xA444)
        state_b = (0xB111, 0xB222, 0xB333, 0xB444)
        begin_run(channel_a, 11, state_a, 0x2)
        begin_run(channel_b, 22, state_b, 0x2)

        helper_a.send_signal(signal.SIGSEGV)
        require_exit(helper_a, -signal.SIGSEGV, "helper A after SIGSEGV")
        if helper_b.poll() is not None:
            raise RuntimeError("helper B exited when only helper A received SIGSEGV")

        channel_b.sendall(frame(STOP, 22))
        expect_frame(channel_b, (STOP_ACK, 22, struct.pack("<I", 0)), "STOP 22 ACK")
        check_result(channel_b, 22, state_b, 0, 0x2, expected_reason=2)

        fresh_state = (0xC111, 0xC222, 0xC333, 0xC444)
        channel_b.sendall(frame(
            RUN, 23, run_payload(LOOP_CODE, 1, fresh_state, 0, 0x42),
        ))
        expect_frame(channel_b, (RUN_ACK, 23, struct.pack("<I", 0)), "RUN 23 ACK")
        check_result(channel_b, 23, fresh_state, 2, 0x42, expected_reason=0,
                     expected_retired=1)

        channel_b.close()
        channels.remove(channel_b)
        require_exit(helper_b, 0, "helper B after idle EOF")
        print(
            "VMHost control V1 isolation smoke passed: SIGSEGV stayed within helper A; "
            "helper B completed exact STOP, fresh RUN, and idle EOF"
        )
    finally:
        for channel in channels:
            channel.close()
        for process in children:
            if process.poll() is None:
                process.kill()
            try:
                process.wait(timeout=PROCESS_TIMEOUT_SECONDS)
            except subprocess.TimeoutExpired:
                raise RuntimeError(f"failed to reap VMHost child PID {process.pid}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("vmhost", help="path to the signed od-vmhost-smoke executable")
    args = parser.parse_args()
    check_isolation(args.vmhost)


if __name__ == "__main__":
    try:
        main()
    except (EOFError, OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"vmhost_control_v1_isolation_smoke: {error}", file=sys.stderr)
        sys.exit(1)
