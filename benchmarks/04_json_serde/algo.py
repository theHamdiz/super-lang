#!/usr/bin/env python3
N=10000
rng=4861952
def lcg():
    global rng
    rng=(rng*1103515245+12345)%2147483648
    return rng
cs=0
for _ in range(N):
    id=lcg()%1000000
    total_cents=lcg()%100000
    items=lcg()%8+1
    cs += id+total_cents+items*17
    cs += id+total_cents+items*17
print(f"CHECKSUM={cs}")
