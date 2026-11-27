#!/usr/bin/env python3
import zlib
seed=b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.."
block=seed
for _ in range(6):
    block=block+block
rng=8137778
acc=0
for i in range(4096):
    rng=(rng*1103515245+12345)%2147483648
    acc += (rng%255)+block[i%len(block)]
payload=block*9
c1=zlib.crc32(payload)&0xffffffff
c2=zlib.crc32(block)&0xffffffff
cs=c1+c2+(acc%1000)
print(f"CHECKSUM={cs}")
