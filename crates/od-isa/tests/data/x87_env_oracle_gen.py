#!/usr/bin/env python3
"""Image list of the S2-32c FNSTENV/FNSAVE/FRSTOR oracle: one 108-byte FRSTOR image per line (hex, memory order).
Feed it to x87_env_oracle on the Core i5-3210M; the answers are in x87_env_oracle_i5_3210m.txt."""
import random
random.seed(20261122)
FCWS=[0x007f,0x047f,0x087f,0x0c7f,0x027f,0x067f,0x0a7f,0x0e7f,0x037f,0x077f,0x0b7f,0x0f7f]
def reg(kind):
    """One 10-byte register image in memory order (mantissa 8 bytes LE, sign/exponent 2 bytes LE)."""
    s=random.getrandbits(1)<<15
    m=random.getrandbits(64)
    if kind==0: e=random.randint(1,0x7ffe); m|=1<<63            # normal
    elif kind==1: e=0; m=0                                      # zero
    elif kind==2: e=0; m&=(1<<63)-1; m=m or 1                   # denormal
    elif kind==3: e=0; m|=1<<63                                 # pseudo-denormal
    elif kind==4: e=0x7fff; m=1<<63                             # infinity
    elif kind==5: e=0x7fff; m|=(1<<63)|(1<<62)                  # quiet NaN
    elif kind==6: e=0x7fff; m&=(1<<63)-1; m=m or 1              # pseudo-NaN / invalid (integer bit clear)
    elif kind==7: e=random.randint(1,0x7ffe); m&=(1<<63)-1      # unnormal
    else: e=random.getrandbits(15)                              # raw bits
    return m.to_bytes(8,'little')+(s|e).to_bytes(2,'little')
def image(i):
    top=random.randint(0,7)
    c=random.getrandbits(4)  # C0..C2 are bits 8..10, C3 is bit 14
    fsw=(top<<11)|((c&7)<<8)|((c>>3)<<14)|(random.getrandbits(7)&0x7f)
    fsw&=~0x8080
    tags=0
    for p in range(8): tags|=(random.choice([0,1,2,3,3,0])<<(2*p))
    if i%7==0: tags=0xffff
    fcw=FCWS[i%12]
    b=bytearray(108)
    b[0:2]=fcw.to_bytes(2,'little'); b[2:4]=random.getrandbits(16).to_bytes(2,'little')
    b[4:6]=fsw.to_bytes(2,'little'); b[6:8]=random.getrandbits(16).to_bytes(2,'little')
    b[8:10]=tags.to_bytes(2,'little'); b[10:12]=random.getrandbits(16).to_bytes(2,'little')
    b[12:16]=random.getrandbits(32).to_bytes(4,'little')
    b[16:18]=(0 if i%5==0 else random.getrandbits(16)).to_bytes(2,'little')
    b[18:20]=random.getrandbits(16).to_bytes(2,'little')
    b[20:24]=random.getrandbits(32).to_bytes(4,'little')
    b[24:26]=(0 if i%4==0 else random.getrandbits(16)).to_bytes(2,'little')
    b[26:28]=random.getrandbits(16).to_bytes(2,'little')
    for k in range(8):
        kind=(i+k)%9 if i%3 else random.randint(0,8)
        b[28+10*k:38+10*k]=reg(kind)
    return bytes(b).hex()
for i in range(320): print(image(i))
# FSW with only sticky flags and C bits, TOP 0 and 7, all tags empty / all valid
for top in (0,7):
    for tg in (0xffff,0x0000):
        b=bytearray(108); b[0:2]=(0x037f).to_bytes(2,'little'); b[4:6]=((top<<11)|0x4020).to_bytes(2,'little'); b[8:10]=tg.to_bytes(2,'little')
        for k in range(8): b[28+10*k:38+10*k]=reg(0)
        print(bytes(b).hex())
