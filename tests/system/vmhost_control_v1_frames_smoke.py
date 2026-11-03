#!/usr/bin/env python3
"""Exercise bounded control V1 framing and admission in one VMHost process."""

import argparse
import socket
import struct
import subprocess
import sys
import time


MAGIC = b"ODCP"
VERSION = 1
HELLO, READY, RUN, RUN_ACK, ERROR, RESULT = 1, 2, 3, 4, 255, 7
MAX_PAYLOAD = 8192
PROCESS_TIMEOUT_SECONDS = 5
IO_TIMEOUT_SECONDS = 2
FRAGMENT_GAP_SECONDS = 0.05
CODE = bytes.fromhex("4889c1 4889da")
RUN_RESULT = struct.pack(
    "<6QIIQ", 0x11, 0x11, 0x44, 0x44, 6, 0x202, 0, 0, 2
)


def frame(kind, request_id, payload=b""):
    if len(payload) > MAX_PAYLOAD:
        raise ValueError("payload exceeds control V1 limit")
    return MAGIC + struct.pack("<HHII", VERSION, kind, request_id, len(payload)) + payload


def run_payload():
    prefix = struct.pack("<IQ6Q", len(CODE), 2, 0x11, 0x22, 0x33, 0x44, 0, 0x202)
    if len(prefix) != 60:
        raise AssertionError("RUN prefix must be exactly 60 bytes")
    return prefix + CODE


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


def expect_frame(channel, expected, label):
    actual = read_frame(channel)
    if actual != expected:
        raise RuntimeError(f"{label}: expected {expected!r}, received {actual!r}")


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


def finish_process(process, expected_status, label):
    try:
        status = process.wait(timeout=PROCESS_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired as error:
        process.kill()
        process.wait()
        raise RuntimeError(f"{label}: VMHost did not exit within the process timeout") from error
    if status != expected_status:
        raise RuntimeError(f"{label}: expected VMHost exit {expected_status}, received {status}")


def send_fragmented(channel, chunks):
    for index, chunk in enumerate(chunks):
        channel.sendall(chunk)
        if index + 1 != len(chunks):
            time.sleep(FRAGMENT_GAP_SECONDS)


def fragmented_hello_and_run(vmhost):
    process, channel = launch_vmhost(vmhost)
    try:
        hello = frame(HELLO, 0)
        send_fragmented(channel, (hello[:5], hello[5:]))
        expect_frame(channel, (READY, 0, struct.pack("<II", 1, 0)), "fragmented HELLO READY")

        request = frame(RUN, 1, run_payload())
        send_fragmented(channel, (request[:7], request[7:24], request[24:53], request[53:]))
        expect_frame(channel, (RUN_ACK, 1, struct.pack("<I", 0)), "fragmented RUN ACK")
        expect_frame(channel, (RESULT, 1, RUN_RESULT), "fragmented RUN RESULT")

        channel.close()
        channel = None
        finish_process(process, 0, "idle EOF after fragmented frames")
    finally:
        if channel is not None:
            channel.close()
        if process.poll() is None:
            process.kill()
            process.wait()


def coalesced_runs_are_decoded_and_admitted_in_order(vmhost):
    process, channel = launch_vmhost(vmhost)
    try:
        channel.sendall(frame(HELLO, 0))
        expect_frame(channel, (READY, 0, struct.pack("<II", 1, 0)), "coalesced RUN READY")

        # Both frames are structurally valid. The second is rejected as BUSY
        # because the first RUN has already been admitted in this receive batch.
        channel.sendall(frame(RUN, 1, run_payload()) + frame(RUN, 2, run_payload()))
        expect_frame(channel, (RUN_ACK, 1, struct.pack("<I", 0)), "coalesced first RUN ACK")
        expect_frame(
            channel,
            (ERROR, 2, struct.pack("<III", 3, 1, RUN)),
            "coalesced second RUN BUSY rejection",
        )
        expect_frame(channel, (RESULT, 1, RUN_RESULT), "coalesced first RUN RESULT")

        channel.close()
        channel = None
        finish_process(process, 0, "idle EOF after coalesced frames")
    finally:
        if channel is not None:
            channel.close()
        if process.poll() is None:
            process.kill()
            process.wait()


def expect_truncated_frame_eof(vmhost, partial_frame, label):
    process, channel = launch_vmhost(vmhost)
    try:
        channel.sendall(frame(HELLO, 0))
        expect_frame(channel, (READY, 0, struct.pack("<II", 1, 0)), f"{label} READY")
        channel.sendall(partial_frame)
        channel.close()
        channel = None
        finish_process(process, 1, label)
    finally:
        if channel is not None:
            channel.close()
        if process.poll() is None:
            process.kill()
            process.wait()


def partial_frame_eof_tears_down(vmhost):
    partial_header = frame(RUN, 1, run_payload())[:9]
    expect_truncated_frame_eof(vmhost, partial_header, "EOF in partial RUN header")

    complete_header = MAGIC + struct.pack("<HHII", VERSION, RUN, 1, 61)
    partial_body = complete_header + bytes(17)
    expect_truncated_frame_eof(vmhost, partial_body, "EOF in partial RUN body")


def oversized_length_closes_without_body(vmhost):
    process, channel = launch_vmhost(vmhost)
    try:
        channel.sendall(frame(HELLO, 0))
        expect_frame(channel, (READY, 0, struct.pack("<II", 1, 0)), "oversized length READY")
        invalid_header = MAGIC + struct.pack("<HHII", VERSION, RUN, 1, MAX_PAYLOAD + 1)
        channel.sendall(invalid_header)

        try:
            unexpected = channel.recv(1)
        except socket.timeout as error:
            raise RuntimeError("oversized declared length was not rejected before body arrival") from error
        if unexpected:
            raise RuntimeError("VMHost sent a response for a fatal oversized frame")
        finish_process(process, 1, "oversized declared length")
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
    fragmented_hello_and_run(args.vmhost)
    coalesced_runs_are_decoded_and_admitted_in_order(args.vmhost)
    partial_frame_eof_tears_down(args.vmhost)
    oversized_length_closes_without_body(args.vmhost)
    print("VMHost control V1 smoke passed: fragmented/coalesced frames, bounded EOF teardown, oversized length")


if __name__ == "__main__":
    try:
        main()
    except (EOFError, OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"vmhost_control_v1_frames_smoke: {error}", file=sys.stderr)
        sys.exit(1)
