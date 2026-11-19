#!/usr/bin/env python3
"""Exercise bounded control V1 framing and admission in one VMHost process."""

import argparse
import ctypes
import fcntl
import os
import select
import socket
import struct
import subprocess
import sys
from pathlib import Path
import time


MAGIC = b"ODCP"
VERSION = 1
HELLO, READY, RUN, RUN_ACK, STOP, STOP_ACK, RESULT, ERROR = 1, 2, 3, 4, 5, 6, 7, 255
INVALID_STATE = 6
INVALID_REQUEST_ID = 7
NO_ACTIVE_RUN = 9
STOP_REQUESTED = 2
MAX_PAYLOAD = 8192
PROCESS_TIMEOUT_SECONDS = 5
IO_TIMEOUT_SECONDS = 2
FRAGMENT_GAP_SECONDS = 0.05
BACKPRESSURE_WINDOW_SECONDS = 0.2
BACKPRESSURE_REQUESTS = 8192
COMPLETION_PRESSURE_REQUESTS = 380
BACKPRESSURE_INPUT_BUFFER_BYTES = 32768
BACKPRESSURE_OUTPUT_BUFFER_BYTES = 4096
PROC_PIDLISTFDS = 1
PROX_FDTYPE_PIPE = 6
CODE = bytes.fromhex("4803c3 4803c3")
RUN_RESULT = struct.pack(
    "<6QIIQ", 0x99, 0x22, 0x33, 0x44, 6, 0x206, 0, 0, 2
)


def frame(kind, request_id, payload=b""):
    if len(payload) > MAX_PAYLOAD:
        raise ValueError("payload exceeds control V1 limit")
    return MAGIC + struct.pack("<HHII", VERSION, kind, request_id, len(payload)) + payload


def run_payload(code=CODE, *, budget=2, rip=0):
    prefix = struct.pack("<IQ6Q", len(code), budget, 0x11, 0x22, 0x33, 0x44, rip, 0x202)
    if len(prefix) != 60:
        raise AssertionError("RUN prefix must be exactly 60 bytes")
    return prefix + code


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


def launch_vmhost(path, *, backpressure=False, keep_peer=False, stderr=subprocess.DEVNULL):
    parent, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    peer = None
    try:
        if backpressure:
            parent.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, BACKPRESSURE_INPUT_BUFFER_BYTES)
            child.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, BACKPRESSURE_INPUT_BUFFER_BYTES)
            child.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, BACKPRESSURE_OUTPUT_BUFFER_BYTES)
            parent.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, BACKPRESSURE_OUTPUT_BUFFER_BYTES)
        if backpressure or keep_peer:
            peer = child.dup()
        process = subprocess.Popen(
            [path, "--socket-fd", str(child.fileno())],
            pass_fds=(child.fileno(),),
            stdout=subprocess.DEVNULL,
            stderr=stderr,
        )
    except Exception:
        parent.close()
        child.close()
        if peer is not None:
            peer.close()
        raise
    child.close()
    parent.settimeout(IO_TIMEOUT_SECONDS)
    return process, parent, peer


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
    process, channel, _ = launch_vmhost(vmhost)
    try:
        hello = frame(HELLO, 0)
        send_fragmented(channel, (hello[:5], hello[5:]))
        expect_frame(channel, (READY, 0, struct.pack("<II", 1, 0)), "fragmented HELLO READY")

        # The last JNZ branches to itself. Its final displacement byte must
        # reach the decoder before admission and determines the returned RIP.
        code = bytes.fromhex("75 00") * 2047 + bytes.fromhex("75 fe")
        entry_rip = 4094
        request = frame(RUN, 1, run_payload(code, budget=1, rip=entry_rip))
        send_fragmented(channel, (request[:7], request[7:24], request[24:53], request[53:-1]))
        if select.select([channel], [], [], FRAGMENT_GAP_SECONDS)[0]:
            raise RuntimeError("VMHost responded before receiving the final maximum RUN byte")
        channel.sendall(request[-1:])
        expect_frame(channel, (RUN_ACK, 1, struct.pack("<I", 0)), "fragmented RUN ACK")
        expected = struct.pack("<6QIIQ", 0x11, 0x22, 0x33, 0x44, entry_rip, 0x202, 1, 0, 1)
        expect_frame(channel, (RESULT, 1, expected), "fragmented maximum RUN RESULT")

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
    process, channel, _ = launch_vmhost(vmhost)
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
    process, channel, _ = launch_vmhost(vmhost)
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
    process, channel, _ = launch_vmhost(vmhost)
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


def stop_acks_survive_output_backpressure(vmhost):
    process, channel, peer = launch_vmhost(vmhost, backpressure=True)
    try:
        # The duplicate shares the open file description used by VMHost.
        with peer:
            channel.sendall(frame(HELLO, 0))
            expect_frame(channel, (READY, 0, struct.pack("<II", 1, 0)), "backpressure READY")
            channel.sendall(frame(RUN, 1, run_payload()))
            expect_frame(channel, (RUN_ACK, 1, struct.pack("<I", 0)), "initial RUN ACK")
            expect_frame(channel, (RESULT, 1, RUN_RESULT), "initial RUN RESULT")
            if not (fcntl.fcntl(peer.fileno(), fcntl.F_GETFL) & os.O_NONBLOCK):
                raise RuntimeError("VMHost V1 peer remains blocking after READY")

        request_ids = range(100, 100 + BACKPRESSURE_REQUESTS)
        requests = memoryview(b"".join(frame(STOP, request_id) for request_id in request_ids))
        expected = b"".join(
            frame(STOP_ACK, request_id, struct.pack("<I", NO_ACTIVE_RUN))
            for request_id in request_ids
        )
        channel.setblocking(False)
        sent = 0
        deadline = time.monotonic() + IO_TIMEOUT_SECONDS
        while sent < len(requests):
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeError("could not observe backpressure with ACKs pending")
            try:
                sent += channel.send(requests[sent:])
            except BlockingIOError:
                _, writable, _ = select.select(
                    [], [channel], [], min(remaining, BACKPRESSURE_WINDOW_SECONDS)
                )
                if not writable and select.select([channel], [], [], 0)[0]:
                    break
        else:
            raise RuntimeError("all STOP requests fit without observing backpressure")

        # Drain ACKs while finishing the interrupted write. Keep both directions
        # moving so a full buffer cannot deadlock the test client.
        received = bytearray()
        deadline = time.monotonic() + IO_TIMEOUT_SECONDS
        while sent < len(requests) or len(received) < len(expected):
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeError(
                    f"pressure recovery stalled: {sent} bytes sent, {len(received)} received"
                )
            readable, writable, _ = select.select(
                [channel] if len(received) < len(expected) else [],
                [channel] if sent < len(requests) else [],
                [],
                remaining,
            )
            if readable:
                chunk = channel.recv(min(8192, len(expected) - len(received)))
                if not chunk:
                    raise EOFError("VMHost closed during pressure recovery")
                received.extend(chunk)
            if writable:
                sent += channel.send(requests[sent:])
        if received != expected:
            raise RuntimeError("backpressure recovery lost, duplicated, reordered or changed a STOP ACK")

        channel.settimeout(IO_TIMEOUT_SECONDS)
        channel.sendall(frame(RUN, 2, run_payload()))
        expect_frame(channel, (RUN_ACK, 2, struct.pack("<I", 0)), "recovered RUN ACK")
        expect_frame(channel, (RESULT, 2, RUN_RESULT), "recovered RUN RESULT")
        channel.close()
        channel = None
        finish_process(process, 0, "idle EOF after backpressure")
    finally:
        if channel is not None:
            channel.close()
        if process.poll() is None:
            process.kill()
            process.wait()


def process_pipe_fds(pid):
    # macOS sys/proc_info.h declares proc_fdinfo as int32_t + uint32_t.
    proc_pidinfo = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True).proc_pidinfo
    proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_int]
    proc_pidinfo.restype = ctypes.c_int
    buffer = ctypes.create_string_buffer(1024)
    size = proc_pidinfo(pid, PROC_PIDLISTFDS, 0, buffer, len(buffer))
    if size <= 0:
        error = ctypes.get_errno()
        raise OSError(error, f"proc_pidinfo({pid}): {os.strerror(error)}")
    record = struct.Struct("=iI")
    if size >= len(buffer) or size % record.size:
        raise RuntimeError(f"incomplete descriptor list for VMHost {pid}: {size} bytes")
    return {fd for fd, kind in record.iter_unpack(buffer.raw[:size]) if kind == PROX_FDTYPE_PIPE}


def wait_for_condition(condition, failure):
    deadline = time.monotonic() + IO_TIMEOUT_SECONDS
    while not condition():
        if time.monotonic() >= deadline:
            raise RuntimeError(failure)
        time.sleep(0.005)


def active_run_completes_under_output_backpressure(vmhost):
    process, channel, peer = launch_vmhost(vmhost, backpressure=True)
    try:
        with channel, peer:
            channel.sendall(frame(HELLO, 0))
            expect_frame(channel, (READY, 0, struct.pack("<II", 1, 0)), "completion pressure READY")
            idle_pipes = process_pipe_fds(process.pid)
            loop = bytes.fromhex("75 fe")
            channel.sendall(frame(RUN, 1, run_payload(loop, budget=2**64 - 1)))
            expect_frame(channel, (RUN_ACK, 1, struct.pack("<I", 0)), "completion pressure RUN ACK")
            run_pipes = process_pipe_fds(process.pid) - idle_pipes
            if len(run_pipes) != 2:
                raise RuntimeError(f"expected both RUN completion pipe ends, found {run_pipes}")

            # 7600 bytes of mismatch ACKs saturate the 4096-byte socket buffer
            # while leaving room to read the matching STOP and queue RESULT.
            request_ids = range(100, 100 + COMPLETION_PRESSURE_REQUESTS)
            channel.sendall(b"".join(frame(STOP, request_id) for request_id in request_ids))
            wait_for_condition(
                lambda: not select.select([], [peer], [], 0)[1],
                "VMHost output socket stayed writable during completion probe",
            )
            channel.sendall(frame(STOP, 1))
            wait_for_condition(
                lambda: not (run_pipes & process_pipe_fds(process.pid)),
                "RUN completion pipe did not retire while VMHost output was blocked",
            )
            if select.select([], [peer], [], 0)[1]:
                raise RuntimeError("VMHost output became writable before the client drained it")

            mismatch = struct.pack("<I", INVALID_REQUEST_ID)
            for request_id in request_ids:
                expect_frame(channel, (STOP_ACK, request_id, mismatch), "mismatched STOP under pressure")
            expect_frame(channel, (STOP_ACK, 1, struct.pack("<I", 0)), "matching STOP under pressure")
            kind, request_id, result = read_frame(channel)
            expected = struct.pack("<6QII", 0x11, 0x22, 0x33, 0x44, 0, 0x202, STOP_REQUESTED, 0)
            # The retired-instruction count depends on when the owner sees STOP.
            if (kind, request_id, result[:-8]) != (RESULT, 1, expected):
                raise RuntimeError(
                    f"incorrect RESULT after completion under pressure: {(kind, request_id, result)!r}"
                )
        finish_process(process, 0, "idle EOF after completion under backpressure")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()


def invalid_state_discards_batch(vmhost, site, code):
    process, channel, peer = launch_vmhost(vmhost, keep_peer=True, stderr=subprocess.PIPE)
    try:
        with channel, process.stderr:
            # Retain the host endpoint through exit to detect unread request bytes.
            with peer:
                channel.sendall(frame(HELLO, 0))
                expect_frame(channel, (READY, 0, struct.pack("<II", 1, 0)), f"{site} failure READY")
                channel.sendall(frame(RUN, 1, run_payload(code)) + frame(STOP, 77))
                expect_frame(
                    channel,
                    (ERROR, 1, struct.pack("<III", INVALID_STATE, 1, RUN)),
                    f"{site} failure INVALID_STATE rejection",
                )
                finish_process(process, 0, f"exit after {site} INVALID_STATE")
                if select.select([peer], [], [], 0)[0] and peer.recv(1, socket.MSG_PEEK):
                    raise RuntimeError(f"{site} failure: unread request bytes after INVALID_STATE")
            if channel.recv(1) != b"":
                raise RuntimeError(f"{site} failure: unexpected output after INVALID_STATE")
            diagnostic = process.stderr.read()
            if diagnostic != f"fixture_invalid_state={site}\n".encode("ascii"):
                raise RuntimeError(f"{site} failure: unexpected ABI fixture trace {diagnostic!r}")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "vmhost", nargs="?", default=str(Path(__file__).resolve().parents[2] / "apps" / ".build" / "debug" / "od-vmhost"),
        help="path to the built VMHost binary (default: apps/.build/debug/od-vmhost)",
    )
    parser.add_argument(
        "--admission-failures", action="store_true",
        help="check the dedicated VMHost build with the admission failure fixture bridge",
    )
    args = parser.parse_args()
    if args.admission_failures:
        invalid_state_discards_batch(args.vmhost, "create", CODE[:3])
        invalid_state_discards_batch(args.vmhost, "begin", CODE)
        print("VMHost control V1 smoke passed: create/begin INVALID_STATE closes the frame batch")
        return
    fragmented_hello_and_run(args.vmhost)
    coalesced_runs_are_decoded_and_admitted_in_order(args.vmhost)
    partial_frame_eof_tears_down(args.vmhost)
    oversized_length_closes_without_body(args.vmhost)
    stop_acks_survive_output_backpressure(args.vmhost)
    active_run_completes_under_output_backpressure(args.vmhost)
    print(
        "VMHost control V1 smoke passed: fragmented/coalesced frames, "
        "maximum RUN length, bounded EOF teardown, oversized length, backpressure, active completion"
    )


if __name__ == "__main__":
    try:
        main()
    except (EOFError, OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"vmhost_control_v1_frames_smoke: {error}", file=sys.stderr)
        sys.exit(1)
