//! Environment helpers for DTTN PTY benchmarks and tests.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

fn workspace_root() -> Result<PathBuf> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .map(|p| p.to_path_buf())
        .context("failed to resolve workspace root from CARGO_MANIFEST_DIR")
}

fn target_dir() -> Result<PathBuf> {
    Ok(std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            workspace_root()
                .expect("workspace root for target_dir fallback")
                .join("target")
        }))
}

fn local_dttn_binary_path() -> Result<PathBuf> {
    Ok(target_dir()?
        .join("debug")
        .join(format!("dttn{}", std::env::consts::EXE_SUFFIX)))
}

fn ensure_local_dttn_binary(binary: &std::path::Path) -> Result<()> {
    if binary.exists() {
        return Ok(());
    }

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let mut cmd = Command::new(&cargo);
    cmd.current_dir(workspace_root()?)
        .args([
            "build",
            "-p",
            "xai-grok-pager-bin",
            "--bin",
            "dttn",
        ])
        .stdin(Stdio::null())
        .envs(xai_tty_utils::pager_env());
    xai_tty_utils::detach_std_command(&mut cmd);
    let output = cmd
        .output()
        .with_context(|| format!("failed to spawn {cargo} to build dttn"))?;

    if !output.status.success() {
        bail!(
            "failed to build dttn (exit {:?})\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    if !binary.exists() {
        bail!(
            "dttn build completed but binary missing at {}",
            binary.display()
        );
    }
    Ok(())
}

/// Resolve the DTTN binary path.
///
/// Resolution order:
/// 1. `DTTN_BINARY` environment variable
/// 2. `PAGER_BINARY` compatibility environment variable
/// 3. `CARGO_BIN_EXE_dttn`, set by `cargo test`
/// 4. Build the composition-root package locally
pub fn pager_binary() -> Result<PathBuf> {
    for variable in ["DTTN_BINARY", "PAGER_BINARY"] {
        if let Ok(path) = std::env::var(variable) {
            let p = PathBuf::from(path);
            if !p.exists() {
                bail!("{variable} does not exist: {}", p.display());
            }
            // Bazel may provide a runfiles-relative path; portable_pty resolves
            // non-absolute paths through PATH instead of the current directory.
            return std::path::absolute(&p)
                .with_context(|| format!("failed to absolutize {variable}: {}", p.display()));
        }
    }

    if let Ok(path) = std::env::var("CARGO_BIN_EXE_dttn") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Ok(p);
        }
    }

    let binary = local_dttn_binary_path()?;
    ensure_local_dttn_binary(&binary)?;
    Ok(binary)
}
