//! M5 content-addressed package mechanism.
//!
//! Research-v1 ships `core`, `alloc`, `str`, `fmt`, `collections`, `json`,
//! `fs`, `test` in-tree. `net`, `tls`, `crypto`, `regex`, `time` are
//! versioned *external* components: a manifest + hash, not compiler-coupled.

use crate::hash;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const PACK_API: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackManifest {
    pub api: u32,
    pub name: String,
    pub version: String,
    pub kind: PackKind,
    /// sha256 of the concatenated source files, sorted by path.
    pub source_hash: String,
    pub files: BTreeMap<String, String>,
    pub deps: Vec<PackDep>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PackKind {
    /// Ships in the compiler / runtime.
    Builtin,
    /// Versioned external component (net/tls/crypto/…).
    Component,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackDep {
    pub name: String,
    pub version: String,
}

pub fn builtin_packs() -> Vec<PackManifest> {
    vec![
        builtin("core", "0.1.0"),
        builtin("alloc", "0.1.0"),
        builtin("str", "0.1.0"),
        builtin("fmt", "0.1.0"),
        builtin("collections", "0.1.0"),
        builtin("json", "0.1.0"),
        builtin("fs", "0.1.0"),
        builtin("test", "0.1.0"),
    ]
}

pub fn component_stubs() -> Vec<PackManifest> {
    ["net", "tls", "crypto", "regex", "time"]
        .into_iter()
        .map(|n| PackManifest {
            api: PACK_API,
            name: n.into(),
            version: "0.0.0-reserved".into(),
            kind: PackKind::Component,
            source_hash: hash::sha256_hex(n.as_bytes()),
            files: BTreeMap::new(),
            deps: vec![PackDep {
                name: "core".into(),
                version: "0.1.0".into(),
            }],
        })
        .collect()
}

fn builtin(name: &str, version: &str) -> PackManifest {
    let mut files = BTreeMap::new();
    files.insert(format!("std/{name}/lib.ax"), hash::sha256_hex(name.as_bytes()));
    let payload = files
        .iter()
        .map(|(p, h)| format!("{p}:{h}"))
        .collect::<Vec<_>>()
        .join("\n");
    PackManifest {
        api: PACK_API,
        name: name.into(),
        version: version.into(),
        kind: PackKind::Builtin,
        source_hash: hash::sha256_hex(payload.as_bytes()),
        files,
        deps: Vec::new(),
    }
}

pub fn packs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../std")
}

pub fn write_registry(dir: &Path) -> Result<Vec<PackManifest>, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut all = builtin_packs();
    all.extend(component_stubs());
    for p in &all {
        let sub = dir.join(&p.name);
        std::fs::create_dir_all(&sub).map_err(|e| e.to_string())?;
        let path = sub.join("pack.axpack");
        let json = serde_json::to_string_pretty(p).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())?;
    }
    Ok(all)
}

pub fn load_pack(dir: &Path, name: &str) -> Result<PackManifest, String> {
    let path = dir.join(name).join("pack.axpack");
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

pub fn list_text() -> String {
    let mut o = String::new();
    o.push_str("builtin:\n");
    for p in builtin_packs() {
        o.push_str(&format!("  {}@{}  {}\n", p.name, p.version, p.source_hash));
    }
    o.push_str("components (reserved, not shipped in v1):\n");
    for p in component_stubs() {
        o.push_str(&format!("  {}@{}  {}\n", p.name, p.version, p.source_hash));
    }
    o
}
