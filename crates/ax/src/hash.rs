//! Four identities (§10.2), not one.
//!
//! - def_id          persistent logical identity
//! - interface_hash  exported signature + effect row + contracts
//! - body_hash       normalized implementation AST
//! - build_hash      body_hash + exact dependency build_hashes + target + options

use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

pub fn def_id(module: &str, kind: &str, name: &str) -> String {
    format!("{module}::{kind}:{name}")
}

pub fn interface_hash(payload: &str) -> String {
    sha256_hex(payload.as_bytes())
}

pub fn body_hash(payload: &str) -> String {
    sha256_hex(payload.as_bytes())
}

pub fn build_hash(body: &str, deps: &[String], target: &str, options: &str) -> String {
    let mut h = Sha256::new();
    h.update(body.as_bytes());
    for d in deps {
        h.update(d.as_bytes());
    }
    h.update(target.as_bytes());
    h.update(options.as_bytes());
    hex::encode(h.finalize())
}

/// Semantic executable hash used by G2 replay.
pub fn exec_hash(build: &str, seed: u64) -> String {
    let mut h = Sha256::new();
    h.update(build.as_bytes());
    h.update(seed.to_le_bytes());
    hex::encode(h.finalize())
}
