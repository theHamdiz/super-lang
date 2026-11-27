
let N=10000, rng=4861952n, cs=0n;
function lcg(){ rng=(rng*1103515245n+12345n)%2147483648n; return rng; }
for (let i=0;i<N;i++){
  const id=lcg()%1000000n, total=lcg()%100000n, items=lcg()%8n+1n;
  cs += id+total+items*17n; cs += id+total+items*17n;
}
console.log(`CHECKSUM=${cs}`);
