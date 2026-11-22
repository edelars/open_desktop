#!/usr/bin/env python3
"""Generates the operand list of the S2-32 x87 oracle (op fcw sw0 x y per line, binary80 as 20 hex digits
in memory order). Run: gen_golden.py > in.txt; feed it to x87_oracle on the Core i5-3210M."""
import random
random.seed(20261122)
FCWS = [0x007f,0x047f,0x087f,0x0c7f,0x027f,0x067f,0x0a7f,0x0e7f,0x037f,0x077f,0x0b7f,0x0f7f]
SW0S = [0x0000,0x4700,0x5f00,0x1800,0x3f00]
def b80(sign, e, m):
    se = ((sign & 1) << 15) | ((e + 16383) & 0x7fff)
    return m.to_bytes(8, 'little').hex() + se.to_bytes(2, 'little').hex()
def rnd(lo, hi, signs=(0, 1)):
    return b80(random.choice(signs), random.randint(lo, hi), random.getrandbits(63) | (1 << 63))
Z = "00" * 10
NZ = "00" * 8 + "0080"
ONE = b80(0, 0, 1 << 63)
cnt = 0
def emit(op, x, y=Z):
    global cnt
    cnt += 1
    print(op, "%04x" % FCWS[cnt % 12], "%04x" % random.choice(SW0S), x, y)
for op in ("fsin", "fcos", "fsincos", "fptan"):
    for _ in range(110): emit(op, rnd(-40, 62))
    for _ in range(30): emit(op, rnd(-16000, -40))
    for e in range(-80, -28): emit(op, rnd(e, e))
    for _ in range(24): emit(op, rnd(63, 70))
    for s in (0, 1): emit(op, NZ if s else Z)
for _ in range(110): emit("f2xm1", rnd(-40, -1))
for _ in range(30): emit("f2xm1", rnd(-16000, -40))
for s in (0, 1):
    emit("f2xm1", b80(s, 0, 1 << 63)); emit("f2xm1", b80(s, -1, 1 << 63)); emit("f2xm1", NZ if s else Z)
for _ in range(110): emit("fyl2x", rnd(-30, 30, (0,)), rnd(-10, 10))
for k in (-5, -1, 0, 1, 3, 10):
    for s in (0, 1): emit("fyl2x", b80(0, k, 1 << 63), rnd(-3, 5, (s,)))
for _ in range(30): emit("fyl2x", rnd(-1, 0, (0,)), rnd(-10, 10))
for _ in range(100): emit("fyl2xp1", rnd(-40, -2), rnd(-10, 10))
for _ in range(30): emit("fyl2xp1", rnd(-1000, -40), rnd(-10, 10))
for s in (0, 1): emit("fyl2xp1", NZ if s else Z, rnd(-3, 5))
for _ in range(110): emit("fpatan", rnd(-20, 20), rnd(-20, 20))
for _ in range(40):  # x positive, tiny |y/x| below and around the 2^-40 threshold
    emit("fpatan", rnd(-3, 3, (0,)), rnd(-52, -30))
for _ in range(30): emit("fpatan", rnd(-70, 70), rnd(-70, 70))
for ys in (0, 1):
    for xs in (0, 1): emit("fpatan", b80(xs, 0, 1 << 63), b80(ys, 0, 1 << 63))
for _ in range(80): emit("fscale", rnd(-100, 100), rnd(0, 12))
for s in (0, 1):
    emit("fscale", Z if not s else NZ, b80(0, 2, 1 << 63)); emit("fscale", rnd(3, 3, (s,)), Z); emit("fscale", rnd(3, 3, (s,)), rnd(-3, -1))
for _ in range(60): emit("fxtract", rnd(-16382, 16383))

# Operands next to multiples of pi/2: the 66-bit pi of the hardware reduction is visible here.
def atan_inv(n, P):
    x = (1 << P) // n; s = x; n2 = n * n; k = 1; sign = 1
    while x:
        x //= n2; k += 2; sign = -sign
        s += sign * (x // k)
    return s
PP = 400
PI = 4 * (4 * atan_inv(5, PP) - atan_inv(239, PP))  # pi * 2^PP
def near(k):
    v = k * PI // 2  # k*pi/2 * 2^PP
    e = v.bit_length() - 1 - PP
    m = v >> (v.bit_length() - 64)
    return e, m
ks = list(range(1, 9)) + [random.randint(1 << 10, 1 << 60) for _ in range(6)]
for k in ks:
    e, m = near(k)
    for d in (-1, 0, 1):
        mm = m + d
        for op in ("fsin", "fcos", "fsincos", "fptan"):
            emit(op, b80(0, e, mm))
