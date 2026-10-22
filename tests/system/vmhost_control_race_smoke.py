#!/usr/bin/env python3
"""Check VMHost stop races, partial-frame handling, and launcher completion."""

import argparse
import json
import socket
import struct
import subprocess
import sys


MAGIC = b"ODCP"
VERSION = 0
HELLO, READY, RUN, RUN_ACK, STOP, STOP_ACK, RESULT, ERROR = 1, 2, 3, 4, 5, 6, 7, 255
MAX_PAYLOAD = 8192


def frame(kind, request_id, payload=b""):
    if len(payload) > MAX_PAYLOAD:
        raise ValueError("payload exceeds protocol limit")
    return MAGIC + struct.pack("<HHII", VERSION, kind, request_id, len(payload)) + payload


def send_frame(channel, kind, request_id, payload=b""):
    channel.sendall(frame(kind, request_id, payload))


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
        raise RuntimeError("invalid VMHost response frame")
    return kind, request_id, read_exact(channel, payload_length)


def run_payload(code, budget, rax=0, rcx=0):
    return struct.pack("<IQ4QQQ", len(code), budget, rax, rcx, 0, 0, 0, 2) + code


def start_vmhost(path):
    parent, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    process = subprocess.Popen(
        [path, "--socket-fd", str(child.fileno())],
        pass_fds=(child.fileno(),),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    child.close()
    parent.settimeout(3)
    send_frame(parent, HELLO, 0)
    if read_frame(parent) != (READY, 0, struct.pack("<I", 1)):
        raise RuntimeError("VMHost ABI handshake failed")
    return process, parent


def begin_run(channel, request_id, code, budget, rax=0, rcx=0):
    send_frame(channel, RUN, request_id, run_payload(code, budget, rax, rcx))
    if read_frame(channel) != (RUN_ACK, request_id, struct.pack("<I", 0)):
        raise RuntimeError("VMHost did not acknowledge RUN")


def check_partial_frame(vmhost, partial):
    process, channel = start_vmhost(vmhost)
    try:
        # The finite loop takes long enough to leave a partial control frame
        # pending while its worker reaches its instruction budget.
        begin_run(channel, 7, bytes.fromhex("482bc175fb"), 2_000_000, rax=1 << 63, rcx=1)
        channel.sendall(partial)
        channel.settimeout(2)
        kind, request_id, payload = read_frame(channel)
        if kind != RESULT or request_id != 7 or len(payload) != 64:
            raise RuntimeError("VMHost failed to return RESULT with a partial request buffered")
        if struct.unpack_from("<I", payload, 48)[0] != 1:
            raise RuntimeError("partial-frame scenario did not reach its instruction budget")
        if process.wait(timeout=2) != 0:
            raise RuntimeError("VMHost exited unsuccessfully after RESULT")
    finally:
        channel.close()
        if process.poll() is None:
            process.kill()
            process.wait()


def check_disconnect(vmhost):
    process, channel = start_vmhost(vmhost)
    try:
        begin_run(channel, 8, bytes.fromhex("75fe"), (1 << 64) - 1)
        channel.close()
        try:
            status = process.wait(timeout=2)
        except subprocess.TimeoutExpired as error:
            raise RuntimeError("VMHost did not stop after the launcher disconnected") from error
        if status == 0:
            raise RuntimeError("VMHost unexpectedly reported success without a result receiver")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()


def check_launcher(launcher, vmhost, code, args, expected_reason):
    result = subprocess.run(
        [launcher, "--vmhost", vmhost, "--hex", code, *args],
        capture_output=True,
        text=True,
        timeout=5,
    )
    if result.returncode != 0:
        raise RuntimeError(f"launcher failed ({result.returncode}): {result.stderr.strip()}")
    try:
        report = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError(f"launcher returned invalid JSON: {result.stdout!r}") from error
    expected = expected_reason if isinstance(expected_reason, tuple) else (expected_reason,)
    if report.get("reason") not in expected:
        raise RuntimeError(f"launcher returned reason {report.get('reason')}, expected one of {expected}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("launcher", help="path to the built launcher smoke binary")
    parser.add_argument("vmhost", help="path to the built VMHost smoke binary")
    args = parser.parse_args()

    # Reproduce the original 100-run timing window. An early STOP must be kept
    # until the worker has entered the C ABI execution state.
    for _ in range(100):
        check_launcher(
            args.launcher,
            args.vmhost,
            "75fe",
            ["--budget", str((1 << 64) - 1), "--stop-after-ms", "0"],
            expected_reason=2,
        )

    # The guest returns before the requested 25 ms stop delay. The result wins.
    check_launcher(args.launcher, args.vmhost, "48b80100000000000000", ["--stop-after-ms", "25"], 0)

    # Either natural completion or accepted STOP may win this zero-delay race.
    for _ in range(20):
        check_launcher(args.launcher, args.vmhost, "48b80100000000000000", ["--stop-after-ms", "0"], (0, 2))

    header_partial = b"O"
    payload_partial = MAGIC + struct.pack("<HHII", VERSION, STOP, 7, 4) + b"\x00"
    check_partial_frame(args.vmhost, header_partial)
    check_partial_frame(args.vmhost, payload_partial)
    check_disconnect(args.vmhost)

    print("VMHost control race smoke passed: 100 immediate stops, early completion, partial header/payload, and disconnect")


if __name__ == "__main__":
    try:
        main()
    except (EOFError, OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"vmhost_control_race_smoke: {error}", file=sys.stderr)
        sys.exit(1)
