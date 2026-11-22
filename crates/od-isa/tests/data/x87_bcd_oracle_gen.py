#!/usr/bin/env python3
"""Operand list of the S2-32b FBLD/FBSTP oracle: `op fcw sw0 x` per line (x: 10 bytes in memory order,
packed BCD for fbld, binary80 for fbstp). Run it, feed the list to x87_bcd_oracle on the Core i5-3210M."""
import random
random.seed(20261123)
FCWS=[0x007f,0x047f,0x087f,0x0c7f,0x027f,0x067f,0x0a7f,0x0e7f,0x037f,0x077f,0x0b7f,0x0f7f]
SW0S=[0x0000,0x4700,0x5f00,0x1800,0x3f00]
def b80(sign,e,m):
    se=((sign&1)<<15)|((e+16383)&0x7fff)
    return m.to_bytes(8,'little').hex()+se.to_bytes(2,'little').hex()
def bcd(sign,val,junk=0):
    digs=f"{val:018d}"
    b=bytearray()
    for i in range(9):
        lo=int(digs[17-2*i]);hi=int(digs[16-2*i])
        b.append(lo|(hi<<4))
    b.append((0x80 if sign else 0)|junk)
    return bytes(b).hex()
n=0
def emit(op,x):
    global n
    n+=1
    print(op,"%04x"%FCWS[n%12],"%04x"%random.choice(SW0S),x)
for _ in range(100):
    emit("fbld",bcd(random.getrandbits(1),random.randint(0,10**random.randint(1,18)-1)))
for v in (0,1,9,10,99,10**17,10**18-1):
    for s in (0,1): emit("fbld",bcd(s,v))
for j in (0x01,0x10,0x7f): emit("fbld",bcd(0,12345,j))
for _ in range(120):
    # integers and fractions below 10^18
    e=random.randint(0,59)
    m=random.getrandbits(63)|(1<<63)
    emit("fbstp",b80(random.getrandbits(1),e,m))
for _ in range(80):
    e=random.randint(-70,-1)
    emit("fbstp",b80(random.getrandbits(1),e,random.getrandbits(63)|(1<<63)))
for s in (0,1):
    emit("fbstp","00"*8+("0080" if s else "0000"))
    for v in (1,2,10,123456789012345678):
        emit("fbstp",b80(s,v.bit_length()-1,v<<(64-v.bit_length())))
# exact halves via mantissa construction: value = m * 2^(e-63)
for s in (0,1):
    for (m,e) in ((1<<63,-1),(3<<62,0),(5<<61,1),(7<<61,1)):
        emit("fbstp",b80(s,e,m))
