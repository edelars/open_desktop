#!/usr/bin/env python3
"""Exercise independent VMHost processes and survivor execution on macOS."""

import argparse
import itertools
import signal
import socket
import struct
import subprocess
import sys
from pathlib import Path


MAGIC = b"ODCP"
VERSION = 0
MAX_PAYLOAD = 8192
DIAGNOSTICS_DIR = None
_CHILD_LOG_SEQUENCE = itertools.count(1)


def send_frame(channel, kind, request_id, payload=b""):
    if len(payload) > MAX_PAYLOAD:
        raise ValueError("payload exceeds protocol limit")
    channel.sendall(MAGIC + struct.pack("<HHII", VERSION, kind, request_id, len(payload)) + payload)


def read_exact(channel, length):
    data = bytearray()
    while len(data) < length:
        chunk = channel.recv(length - len(data))
        if not chunk:
            raise EOFError("VMHost closed the control channel")
        data.extend(chunk)
    return bytes(data)


def read_frame(channel):
    header = read_exact(channel, 16)
    magic, version, kind, request_id, payload_length = struct.unpack("<4sHHII", header)
    if magic != MAGIC or version != VERSION or payload_length > MAX_PAYLOAD:
        raise RuntimeError("invalid VMHost response frame")
    return kind, request_id, read_exact(channel, payload_length)


def start_vmhost(executable):
    stdout = stderr = subprocess.DEVNULL
    opened_logs = []
    if DIAGNOSTICS_DIR is not None:
        sequence = next(_CHILD_LOG_SEQUENCE)
        try:
            stdout = (DIAGNOSTICS_DIR / f"vmhost-{sequence:03d}.stdout.log").open("xb")
            opened_logs.append(stdout)
            stderr = (DIAGNOSTICS_DIR / f"vmhost-{sequence:03d}.stderr.log").open("xb")
            opened_logs.append(stderr)
        except Exception:
            for output in opened_logs:
                output.close()
            raise
    parent, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        process = subprocess.Popen(
            [executable, "--socket-fd", str(child.fileno())],
            pass_fds=(child.fileno(),),
            stdout=stdout,
            stderr=stderr,
        )
    except Exception:
        parent.close()
        child.close()
        raise
    finally:
        for output in opened_logs:
            output.close()
    child.close()
    parent.settimeout(5)
    return process, parent


def request_run(channel, request_id, code, budget, rax):
    payload = struct.pack("<IQ4QQQ", len(code), budget, rax, 0, 0, 0, 0, 2) + code
    send_frame(channel, 3, request_id, payload)
    kind, response_id, status = read_frame(channel)
    if (kind, response_id, status) != (4, request_id, struct.pack("<I", 0)):
        raise RuntimeError("VMHost did not acknowledge RUN")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("vmhost", help="path to the signed od-vmhost-smoke executable")
    parser.add_argument("--diagnostics-dir", type=Path)
    args = parser.parse_args()
    global DIAGNOSTICS_DIR
    if args.diagnostics_dir is not None:
        if args.diagnostics_dir.is_symlink() or not args.diagnostics_dir.is_dir():
            raise RuntimeError("diagnostics directory must be an existing non-symlink directory")
        DIAGNOSTICS_DIR = args.diagnostics_dir.resolve()

    processes = []
    channels = []
    try:
        first, first_channel = start_vmhost(args.vmhost)
        processes.append(first)
        channels.append(first_channel)
        second, second_channel = start_vmhost(args.vmhost)
        processes.append(second)
        channels.append(second_channel)

        for channel in channels:
            send_frame(channel, 1, 0)
            if read_frame(channel) != (2, 0, struct.pack("<I", 1)):
                raise RuntimeError("VMHost ABI handshake failed")

        # Run guest code in both independent helpers before simulating one fatal signal.
        loop = bytes.fromhex("75fe")
        request_run(first_channel, 1, loop, (1 << 64) - 1, 0x111)
        request_run(second_channel, 2, loop, (1 << 64) - 1, 0x222)
        first.send_signal(signal.SIGSEGV)
        if first.wait(timeout=3) != -signal.SIGSEGV:
            raise RuntimeError("faulted helper did not exit from SIGSEGV")

        # The surviving helper must remain responsive to its own STOP request.
        send_frame(second_channel, 5, 2)
        if read_frame(second_channel) != (6, 2, struct.pack("<I", 0)):
            raise RuntimeError("surviving VMHost did not acknowledge STOP")
        kind, response_id, payload = read_frame(second_channel)
        if (kind, response_id) != (7, 2) or len(payload) != 64:
            raise RuntimeError("surviving VMHost returned an invalid RESULT")
        rax = struct.unpack_from("<Q", payload, 0)[0]
        reason = struct.unpack_from("<I", payload, 48)[0]
        if rax != 0x222 or reason != 2 or struct.unpack_from("<Q", payload, 56)[0] == 0:
            raise RuntimeError(f"surviving VMHost returned RAX={rax}, reason={reason}")
        if second.wait(timeout=3) != 0:
            raise RuntimeError("surviving VMHost exited unsuccessfully")

        print("VMHost isolation smoke passed: one active helper received SIGSEGV; the other returned its own state after STOP")
    finally:
        for channel in channels:
            channel.close()
        for process in processes:
            if process.poll() is None:
                process.kill()
                process.wait()


if __name__ == "__main__":
    try:
        main()
    except (EOFError, OSError, RuntimeError, ValueError) as error:
        print(f"vmhost_isolation_smoke: {error}", file=sys.stderr)
        sys.exit(1)
