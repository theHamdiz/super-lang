function crc32(buf) {
  const table = new Uint32Array(256);
  for (let i = 0; i < 256; i++) {
    let c = i;
    for (let k = 0; k < 8; k++) c = (c & 1) ? (0xEDB88320 ^ (c >>> 1)) : (c >>> 1);
    table[i] = c >>> 0;
  }
  let crc = 0xFFFFFFFF;
  for (let i = 0; i < buf.length; i++) crc = table[(crc ^ buf[i]) & 0xFF] ^ (crc >>> 8);
  return (crc ^ 0xFFFFFFFF) >>> 0;
}
let seed = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ..";
let block = seed;
for (let e = 0; e < 6; e++) block = block + block;
let rng = 8137778n;
let acc = 0n;
for (let i = 0; i < 4096; i++) {
  rng = (rng * 1103515245n + 12345n) % 2147483648n;
  acc += (rng % 255n) + BigInt(block.charCodeAt(i % block.length));
}
const payload = Buffer.from(block.repeat(9));
const c1 = crc32(payload);
const c2 = crc32(Buffer.from(block));
const cs = BigInt(c1) + BigInt(c2) + (acc % 1000n);
console.log(`CHECKSUM=${cs}`);
