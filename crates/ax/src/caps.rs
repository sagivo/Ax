//! Capability containment. Handle-based, attenuable, no widening.
//!
//! A `ReadCap` is an open directory (or an overlay map for tests), never a
//! string prefix. Path resolution uses `openat`-style join: `..` cannot
//! escape the root, and absolute paths are rejected.

use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug)]
pub struct ReadCap {
    /// Canonical directory this capability is confined to.
    pub root: PathBuf,
    /// Overlay wins over the host fs (used by `test.read_cap`).
    pub overlay: indexmap::IndexMap<String, String>,
}

#[derive(Clone, Debug)]
pub enum CapError {
    Escape,
    Absolute,
    NotFound(String),
    Io(String),
    Denied(&'static str),
}

impl CapError {
    pub fn as_str(&self) -> String {
        match self {
            CapError::Escape => "capability escape".into(),
            CapError::Absolute => "absolute path rejected".into(),
            CapError::NotFound(p) => format!("not found: {p}"),
            CapError::Io(s) => s.clone(),
            CapError::Denied(s) => (*s).into(),
        }
    }
}

impl ReadCap {
    pub fn open_dir(root: impl AsRef<Path>) -> Result<Self, CapError> {
        let root = fs::canonicalize(root.as_ref()).map_err(|e| CapError::Io(e.to_string()))?;
        if !root.is_dir() {
            return Err(CapError::Io("not a directory".into()));
        }
        Ok(Self {
            root,
            overlay: indexmap::IndexMap::new(),
        })
    }

    pub fn overlay(files: indexmap::IndexMap<String, String>) -> Self {
        Self {
            root: PathBuf::from("/."),
            overlay: files,
        }
    }

    /// Attenuate to a subdirectory. Cannot widen.
    pub fn sub(&self, rel: &str) -> Result<Self, CapError> {
        let joined = confine(&self.root, rel)?;
        if !self.overlay.is_empty() {
            // overlay attenuation: keep only keys under rel/
            let prefix = format!("{rel}/");
            let mut files = indexmap::IndexMap::new();
            for (k, v) in &self.overlay {
                if let Some(rest) = k.strip_prefix(&prefix) {
                    files.insert(rest.to_string(), v.clone());
                } else if k == rel {
                    files.insert(k.clone(), v.clone());
                }
            }
            return Ok(Self {
                root: joined,
                overlay: files,
            });
        }
        Self::open_dir(joined)
    }

    pub fn read(&self, rel: &str) -> Result<String, CapError> {
        if let Some(s) = self.overlay.get(rel) {
            return Ok(s.clone());
        }
        let path = confine(&self.root, rel)?;
        fs::read_to_string(path).map_err(|_| CapError::NotFound(rel.into()))
    }
}

/// Join `root` / `rel` with no escape. Rejects absolute paths, `..` that
/// would leave `root`, and NUL.
pub fn confine(root: &Path, rel: &str) -> Result<PathBuf, CapError> {
    if rel.is_empty() {
        return Ok(root.to_path_buf());
    }
    if rel.contains('\0') {
        return Err(CapError::Denied("nul in path"));
    }
    let p = Path::new(rel);
    if p.is_absolute() {
        return Err(CapError::Absolute);
    }
    let mut out = root.to_path_buf();
    let mut depth = 0i32;
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if depth <= 0 {
                    return Err(CapError::Escape);
                }
                out.pop();
                depth -= 1;
            }
            Component::Normal(s) => {
                out.push(s);
                depth += 1;
            }
            Component::RootDir | Component::Prefix(_) => return Err(CapError::Absolute),
        }
    }
    Ok(out)
}

/// There is no widening operation. This exists so the red team has
/// something to call — it always fails.
pub fn widen(_cap: &ReadCap, _to: &Path) -> Result<ReadCap, CapError> {
    Err(CapError::Denied("no widening operation"))
}

/// Strict mode: raw native FFI is forbidden.
pub fn strict_forbid_ffi(has_trusted_extern: bool) -> Result<(), CapError> {
    if has_trusted_extern {
        Err(CapError::Denied("raw native FFI forbidden in strict mode"))
    } else {
        Ok(())
    }
}
