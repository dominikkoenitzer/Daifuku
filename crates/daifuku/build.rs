//! Puts the daemon inside `daifuku.exe` when DAIFUKU_EMBED_DAEMON names a
//! built `daifukud.exe`.
//!
//! The release workflow sets it after building and checking the daemon, so
//! the `daifuku.exe` it publishes on its own is the only file a user needs:
//! install writes the daemon it carries into Program Files. Every other
//! build carries nothing, and install takes `daifukud.exe` from next to
//! `daifuku.exe`, as in the zip.

use std::path::PathBuf;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=DAIFUKU_EMBED_DAEMON");
    println!("cargo::rustc-check-cfg=cfg(embedded_daemon)");

    let Some(path) = std::env::var_os("DAIFUKU_EMBED_DAEMON") else {
        return;
    };
    let path = PathBuf::from(path);
    assert!(
        path.is_absolute(),
        "DAIFUKU_EMBED_DAEMON must be an absolute path, got {}",
        path.display()
    );
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("DAIFUKU_EMBED_DAEMON is {}: {e}", path.display()));
    assert!(
        bytes.starts_with(b"MZ"),
        "DAIFUKU_EMBED_DAEMON is {}, which is not a Windows program",
        path.display()
    );
    println!("cargo::rerun-if-changed={}", path.display());
    println!(
        "cargo::rustc-env=DAIFUKU_EMBEDDED_DAEMON={}",
        path.display()
    );
    println!("cargo::rustc-cfg=embedded_daemon");
}
