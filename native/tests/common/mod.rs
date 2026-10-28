//! Shared test helper: every `limits` run in the tests goes through the OS
//! sandbox, so build `../sandbox` (once per test binary) and point limits at it.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// Path of the freshly built limits-sandbox (panics if it cannot be built).
pub fn sandbox() -> &'static PathBuf {
    static SB: OnceLock<PathBuf> = OnceLock::new();
    SB.get_or_init(|| {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../sandbox");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let st = Command::new(cargo)
            .args(["build", "--release", "--quiet", "--manifest-path"])
            .arg(dir.join("Cargo.toml"))
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("RUSTC_WRAPPER")
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .status()
            .expect("run cargo to build ../sandbox");
        assert!(st.success(), "building ../sandbox failed");
        let p = dir.join("target/release/limits-sandbox");
        assert!(p.is_file(), "{} missing after build", p.display());
        p
    })
}

/// A `limits` command that uses the sandbox built by `sandbox()`.
pub fn limits() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_limits"));
    c.env("LIMITS_SANDBOX", sandbox());
    c
}
