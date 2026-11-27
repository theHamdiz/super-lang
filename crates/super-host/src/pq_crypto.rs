//! Post-quantum + extended hash host primitives (FIPS 203/204, BLAKE2/3, SHA-3).

use std::sync::{Arc, Mutex, OnceLock};

use blake2::{Blake2b512, Blake2s256, Digest as BlakeDigest};
use ml_dsa::{
    EncodedVerifyingKey, Generate as MlDsaGenerate, KeyExport as MlDsaKeyExport, KeyInit, Keypair,
    MlDsa65, Signature, SignatureEncoding, Signer, Verifier, VerifyingKey,
};
use ml_kem::kem::Key;
use ml_kem::ml_kem_768::{self as kem768, MlKem768};
use ml_kem::{Decapsulate, Encapsulate, Kem};
use sha3::{
    Keccak256, Sha3_256, Sha3_512, Shake128, Shake256,
    digest::{Digest as Sha3Digest, ExtendableOutput, Update as Sha3Update, XofReader},
};
use super_runtime::{HostCallback, HostEnv, Value};
use x25519_dalek::{PublicKey, StaticSecret};

fn cb(f: impl Fn(&[Value]) -> Value + Send + Sync + 'static) -> HostCallback {
    Arc::new(f)
}

fn arg_str(args: &[Value], i: usize) -> String {
    args.get(i).map(Value::as_str).unwrap_or_default()
}

fn arg_int(args: &[Value], i: usize) -> i64 {
    args.get(i).map(Value::as_int).unwrap_or(0)
}

fn ok_str(s: impl Into<String>) -> Value {
    Value::Str(s.into())
}

fn err_val(msg: impl Into<String>) -> Value {
    Value::Error(msg.into())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = from_hex(bytes[i])?;
        let lo = from_hex(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Some(out)
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn input_bytes(args: &[Value]) -> Vec<u8> {
    match args.first() {
        Some(Value::Bytes(b)) => b.clone(),
        Some(v) => v.as_str().into_bytes(),
        None => Vec::new(),
    }
}

fn shake_hex(xof: impl ExtendableOutput, out_len: usize) -> String {
    let n = out_len.clamp(1, 1 << 16);
    let mut buf = vec![0u8; n];
    xof.finalize_xof().read(&mut buf);
    hex_encode(&buf)
}

struct PqStore {
    next: i64,
    mldsa: std::collections::HashMap<i64, ml_dsa::SigningKey<MlDsa65>>,
}

fn store() -> &'static Mutex<PqStore> {
    static S: OnceLock<Mutex<PqStore>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(PqStore {
            next: 1,
            mldsa: std::collections::HashMap::new(),
        })
    })
}

struct GrRng;
impl rand_core::RngCore for GrRng {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        getrandom::fill(&mut b).ok();
        u32::from_le_bytes(b)
    }
    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        getrandom::fill(&mut b).ok();
        u64::from_le_bytes(b)
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        getrandom::fill(dest).ok();
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}
impl rand_core::CryptoRng for GrRng {}

/// Register BLAKE2/3, SHA-3/SHAKE/Keccak, ML-KEM-768, ML-DSA-65, hybrid X25519+ML-KEM, HBS-OTS.
pub fn register_pq_crypto(env: &mut HostEnv) {
    env.register_fn(
        "sys_blake3",
        cb(|args| ok_str(hex_encode(blake3::hash(&input_bytes(args)).as_bytes()))),
    );
    env.register_fn(
        "sys_blake2s",
        cb(|args| ok_str(hex_encode(&Blake2s256::digest(input_bytes(args))))),
    );
    env.register_fn(
        "sys_blake2b",
        cb(|args| ok_str(hex_encode(&Blake2b512::digest(input_bytes(args))))),
    );
    env.register_fn(
        "sys_sha3_256",
        cb(|args| ok_str(hex_encode(&Sha3_256::digest(input_bytes(args))))),
    );
    env.register_fn(
        "sys_sha3_512",
        cb(|args| ok_str(hex_encode(&Sha3_512::digest(input_bytes(args))))),
    );
    env.register_fn(
        "sys_keccak256",
        cb(|args| ok_str(hex_encode(&Keccak256::digest(input_bytes(args))))),
    );
    env.register_fn(
        "sys_shake128",
        cb(|args| {
            let data = input_bytes(args);
            let out_len = arg_int(args, 1).clamp(1, 65_536) as usize;
            let mut h = Shake128::default();
            Sha3Update::update(&mut h, &data);
            ok_str(shake_hex(h, out_len))
        }),
    );
    env.register_fn(
        "sys_shake256",
        cb(|args| {
            let data = input_bytes(args);
            let out_len = arg_int(args, 1).clamp(1, 65_536) as usize;
            let mut h = Shake256::default();
            Sha3Update::update(&mut h, &data);
            ok_str(shake_hex(h, out_len))
        }),
    );

    env.register_fn(
        "sys_mlkem768_keygen",
        cb(|_| {
            let (dk, ek) = MlKem768::generate_keypair();
            ok_str(format!(
                "{}:{}",
                hex_encode(dk.to_bytes().as_slice()),
                hex_encode(ek.to_bytes().as_slice())
            ))
        }),
    );
    env.register_fn(
        "sys_mlkem768_encaps",
        cb(|args| {
            let Some(ek_bytes) = hex_decode(&arg_str(args, 0)) else {
                return err_val("bad ek hex");
            };
            let Ok(key) = Key::<kem768::EncapsulationKey>::try_from(ek_bytes.as_slice()) else {
                return err_val("bad ek length");
            };
            let Ok(ek) = kem768::EncapsulationKey::new(&key) else {
                return err_val("invalid ek");
            };
            let (ct, ss) = ek.encapsulate();
            ok_str(format!(
                "{}:{}",
                hex_encode(ct.as_slice()),
                hex_encode(ss.as_slice())
            ))
        }),
    );
    env.register_fn(
        "sys_mlkem768_decaps",
        cb(|args| {
            let Some(seed_bytes) = hex_decode(&arg_str(args, 0)) else {
                return err_val("bad seed hex");
            };
            if seed_bytes.len() != 64 {
                return err_val("seed must be 64 bytes");
            }
            let Some(ct_bytes) = hex_decode(&arg_str(args, 1)) else {
                return err_val("bad ct hex");
            };
            let mut seed = [0u8; 64];
            seed.copy_from_slice(&seed_bytes);
            let dk = kem768::DecapsulationKey::from_seed(seed.into());
            let Ok(ct) = kem768::Ciphertext::try_from(ct_bytes.as_slice()) else {
                return err_val("bad ct length");
            };
            ok_str(hex_encode(dk.decapsulate(&ct).as_slice()))
        }),
    );

    env.register_fn(
        "sys_hybrid_kem_keygen",
        cb(|_| {
            let x_sk = StaticSecret::random_from_rng(GrRng);
            let x_pk = PublicKey::from(&x_sk);
            let (dk, ek) = MlKem768::generate_keypair();
            ok_str(format!(
                "{}:{}:{}:{}",
                hex_encode(x_sk.as_bytes()),
                hex_encode(x_pk.as_bytes()),
                hex_encode(dk.to_bytes().as_slice()),
                hex_encode(ek.to_bytes().as_slice())
            ))
        }),
    );
    env.register_fn(
        "sys_hybrid_kem_encaps",
        cb(|args| {
            let Some(x_pk_b) = hex_decode(&arg_str(args, 0)) else {
                return err_val("bad x25519 pk");
            };
            if x_pk_b.len() != 32 {
                return err_val("x25519 pk must be 32 bytes");
            }
            let Some(ek_bytes) = hex_decode(&arg_str(args, 1)) else {
                return err_val("bad pq ek");
            };
            let Ok(key) = Key::<kem768::EncapsulationKey>::try_from(ek_bytes.as_slice()) else {
                return err_val("bad pq ek length");
            };
            let Ok(ek) = kem768::EncapsulationKey::new(&key) else {
                return err_val("invalid pq ek");
            };
            let eph = StaticSecret::random_from_rng(GrRng);
            let eph_pk = PublicKey::from(&eph);
            let mut pk_arr = [0u8; 32];
            pk_arr.copy_from_slice(&x_pk_b);
            let x_ss = eph.diffie_hellman(&PublicKey::from(pk_arr));
            let (ct, pq_ss) = ek.encapsulate();
            let mut cat = Vec::with_capacity(64);
            cat.extend_from_slice(x_ss.as_bytes());
            cat.extend_from_slice(pq_ss.as_slice());
            let hybrid = Sha3_256::digest(&cat);
            ok_str(format!(
                "{}:{}:{}",
                hex_encode(eph_pk.as_bytes()),
                hex_encode(ct.as_slice()),
                hex_encode(&hybrid)
            ))
        }),
    );
    env.register_fn(
        "sys_hybrid_kem_decaps",
        cb(|args| {
            let Some(x_sk_b) = hex_decode(&arg_str(args, 0)) else {
                return err_val("bad x sk");
            };
            let Some(seed_b) = hex_decode(&arg_str(args, 1)) else {
                return err_val("bad pq seed");
            };
            let Some(eph_b) = hex_decode(&arg_str(args, 2)) else {
                return err_val("bad eph pk");
            };
            let Some(ct_b) = hex_decode(&arg_str(args, 3)) else {
                return err_val("bad pq ct");
            };
            if x_sk_b.len() != 32 || seed_b.len() != 64 || eph_b.len() != 32 {
                return err_val("bad hybrid key sizes");
            }
            let mut sk = [0u8; 32];
            sk.copy_from_slice(&x_sk_b);
            let mut eph = [0u8; 32];
            eph.copy_from_slice(&eph_b);
            let mut seed = [0u8; 64];
            seed.copy_from_slice(&seed_b);
            let x_ss = StaticSecret::from(sk).diffie_hellman(&PublicKey::from(eph));
            let dk = kem768::DecapsulationKey::from_seed(seed.into());
            let Ok(ct) = kem768::Ciphertext::try_from(ct_b.as_slice()) else {
                return err_val("bad ct");
            };
            let pq_ss = dk.decapsulate(&ct);
            let mut cat = Vec::with_capacity(64);
            cat.extend_from_slice(x_ss.as_bytes());
            cat.extend_from_slice(pq_ss.as_slice());
            ok_str(hex_encode(&Sha3_256::digest(&cat)))
        }),
    );

    env.register_fn(
        "sys_mldsa65_keygen",
        cb(|_| {
            let sk = ml_dsa::SigningKey::<MlDsa65>::generate();
            let vk_hex = hex_encode(sk.verifying_key().to_bytes().as_slice());
            let mut g = store().lock().unwrap();
            let id = g.next;
            g.next += 1;
            g.mldsa.insert(id, sk);
            ok_str(format!("{id}:{vk_hex}"))
        }),
    );
    env.register_fn(
        "sys_mldsa65_sign",
        cb(|args| {
            let id = arg_int(args, 0);
            let msg = arg_str(args, 1).into_bytes();
            let g = store().lock().unwrap();
            let Some(sk) = g.mldsa.get(&id) else {
                return err_val("unknown mldsa key id");
            };
            ok_str(hex_encode(sk.sign(&msg).to_bytes().as_slice()))
        }),
    );
    env.register_fn(
        "sys_mldsa65_verify",
        cb(|args| {
            let Some(vk_b) = hex_decode(&arg_str(args, 0)) else {
                return Value::Bool(false);
            };
            let msg = arg_str(args, 1).into_bytes();
            let Some(sig_b) = hex_decode(&arg_str(args, 2)) else {
                return Value::Bool(false);
            };
            let Ok(vk_enc) = EncodedVerifyingKey::<MlDsa65>::try_from(vk_b.as_slice()) else {
                return Value::Bool(false);
            };
            let vk = VerifyingKey::<MlDsa65>::new(&vk_enc);
            let Ok(sig) = Signature::<MlDsa65>::try_from(sig_b.as_slice()) else {
                return Value::Bool(false);
            };
            Value::Bool(vk.verify(&msg, &sig).is_ok())
        }),
    );

    // Winternitz W=16 hash-based one-time signature (SPHINCS+-family primitive).
    env.register_fn(
        "sys_hbs_keygen",
        cb(|_| {
            let mut sk = [0u8; 32];
            getrandom::fill(&mut sk).ok();
            let mut pubs = Vec::with_capacity(64 * 32);
            for i in 0u8..64 {
                let mut node = Sha3_256::digest([&sk[..], &[i]].concat());
                for _ in 0..15 {
                    node = Sha3_256::digest(node);
                }
                pubs.extend_from_slice(&node);
            }
            ok_str(format!(
                "{}:{}",
                hex_encode(&sk),
                hex_encode(&Sha3_256::digest(&pubs))
            ))
        }),
    );
    env.register_fn(
        "sys_hbs_sign",
        cb(|args| {
            let Some(sk) = hex_decode(&arg_str(args, 0)) else {
                return err_val("bad sk");
            };
            let digest = Sha3_256::digest(arg_str(args, 1).as_bytes());
            let mut sig = Vec::with_capacity(64 * 32);
            for i in 0u8..64 {
                let nibble = if i % 2 == 0 {
                    digest[(i / 2) as usize] >> 4
                } else {
                    digest[(i / 2) as usize] & 0x0f
                };
                let mut node = Sha3_256::digest([&sk[..], &[i]].concat());
                for _ in 0..nibble {
                    node = Sha3_256::digest(node);
                }
                sig.extend_from_slice(&node);
            }
            ok_str(hex_encode(&sig))
        }),
    );
    env.register_fn(
        "sys_hbs_verify",
        cb(|args| {
            let Some(pk) = hex_decode(&arg_str(args, 0)) else {
                return Value::Bool(false);
            };
            let Some(sig) = hex_decode(&arg_str(args, 2)) else {
                return Value::Bool(false);
            };
            if pk.len() != 32 || sig.len() != 64 * 32 {
                return Value::Bool(false);
            }
            let digest = Sha3_256::digest(arg_str(args, 1).as_bytes());
            let mut pubs = Vec::with_capacity(64 * 32);
            for i in 0usize..64 {
                let nibble = if i % 2 == 0 {
                    (digest[i / 2] >> 4) as u32
                } else {
                    (digest[i / 2] & 0x0f) as u32
                };
                let mut cur = [0u8; 32];
                cur.copy_from_slice(&sig[i * 32..(i + 1) * 32]);
                for _ in nibble..15 {
                    cur = Sha3_256::digest(cur).into();
                }
                pubs.extend_from_slice(&cur);
            }
            Value::Bool(Sha3_256::digest(&pubs).as_slice() == pk.as_slice())
        }),
    );

    env.register_fn(
        "sys_kdf_sha3",
        cb(|args| {
            let ikm = arg_str(args, 0).into_bytes();
            let info = arg_str(args, 1).into_bytes();
            let n = arg_int(args, 2).clamp(1, 1024) as usize;
            let mut h = Sha3_512::new();
            Sha3Digest::update(&mut h, b"super-kdf-v1");
            Sha3Digest::update(&mut h, (ikm.len() as u32).to_be_bytes());
            Sha3Digest::update(&mut h, &ikm);
            Sha3Digest::update(&mut h, (info.len() as u32).to_be_bytes());
            Sha3Digest::update(&mut h, &info);
            let mut out = Sha3Digest::finalize(h).to_vec();
            while out.len() < n {
                let next = Sha3_512::digest(&out);
                out.extend_from_slice(&next);
            }
            out.truncate(n);
            ok_str(hex_encode(&out))
        }),
    );
}
