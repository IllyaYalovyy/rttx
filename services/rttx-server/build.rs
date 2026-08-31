use std::process::Command;

fn main() {
    // Packagers building from a tarball (no .git) set RTTX_GIT_HASH explicitly;
    // otherwise ask git for the current commit.
    let hash = std::env::var("RTTX_GIT_HASH")
        .ok()
        .filter(|h| !h.trim().is_empty())
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "--short", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    println!("cargo:rustc-env=GIT_HASH={hash}");
    println!("cargo:rerun-if-env-changed=RTTX_GIT_HASH");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
}
