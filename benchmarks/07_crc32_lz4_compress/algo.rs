//! 07_crc32 — IEEE CRC32 on expanded LCG-scrambled block
const SEED: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ..";

fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (i, e) in t.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
        }
        *e = c;
    }
    t
}

fn crc32(table: &[u32; 256], data: &[u8]) -> u32 {
    !data
        .iter()
        .fold(0xFFFF_FFFFu32, |crc, &b| table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8))
}

fn main() {
    let table = crc_table();
    let mut block = SEED.to_vec();
    for _ in 0..6 {
        block.extend_from_within(..);
    }
    let block_len = block.len();

    let mut rng: u64 = 8137778;
    let mut acc: i64 = 0;
    for i in 0..4096usize {
        rng = (rng.wrapping_mul(1103515245).wrapping_add(12345)) % 2147483648;
        acc += (rng % 255) as i64 + block[i % block_len] as i64;
    }

    let payload = block.repeat(9);
    let c1 = crc32(&table, &payload);
    let c2 = crc32(&table, &block);
    let cs = c1 as i64 + c2 as i64 + (acc % 1000);
    println!("CHECKSUM={cs}");
}
