# DTTN-CLI on Windows

DTTN-CLI provides a native 64-bit Windows executable named `dttn.exe`. The first Windows release target is `x86_64-pc-windows-msvc` and is intended for Windows 10/11 with Windows Terminal or another ConPTY-capable terminal.

## Install a released build

The installer runs without administrator privileges. It downloads the Windows ZIP and its SHA-256 file from the selected GitHub release, verifies the archive, installs `dttn.exe` under the current user profile, and adds that directory to the user `PATH`.

Download and inspect the installer before running it:

```powershell
Invoke-WebRequest `
  https://raw.githubusercontent.com/yichensunjoe/DTTN-CLI/main/scripts/install-windows.ps1 `
  -OutFile install-dttn.ps1

Get-Content .\install-dttn.ps1
PowerShell -NoProfile -ExecutionPolicy Bypass -File .\install-dttn.ps1
```

Open a new terminal and verify the installation:

```powershell
dttn --help
dttn config models
```

Install a specific release tag:

```powershell
.\install-dttn.ps1 -Version v0.1.0
```

Install to a custom directory without changing `PATH`:

```powershell
.\install-dttn.ps1 `
  -InstallDir "$HOME\Tools\DTTN" `
  -NoPathUpdate
```

## Manual installation

1. Download `dttn-windows-x86_64.zip` and `dttn-windows-x86_64.zip.sha256` from the same GitHub release.
2. Verify the archive:

```powershell
$expected = (Get-Content .\dttn-windows-x86_64.zip.sha256 -Raw).Split()[0]
$actual = (Get-FileHash .\dttn-windows-x86_64.zip -Algorithm SHA256).Hash
if ($actual -ne $expected) { throw 'Checksum mismatch' }
```

3. Extract `dttn.exe` to a directory on your user `PATH`.

## Build from source

Required software:

- Git for Windows
- Rust toolchain specified by `rust-toolchain.toml`
- Visual Studio Build Tools with the Desktop development with C++ workload
- Protocol Buffers compiler (`protoc`)

Example:

```powershell
git clone https://github.com/yichensunjoe/DTTN-CLI.git
Set-Location DTTN-CLI
choco install protoc --no-progress -y
cargo build --locked --release `
  -p xai-grok-pager-bin `
  --bin dttn `
  --features release-dist

.\target\release\dttn.exe --help
```

## Configuration

PowerShell environment-variable syntax differs from POSIX shells:

```powershell
$env:OPENAI_API_KEY = '...'
$env:DTTN_HOME = "$HOME\.dttn"
dttn config models
dttn
```

To persist an environment variable for the current user:

```powershell
[Environment]::SetEnvironmentVariable('OPENAI_API_KEY', '...', 'User')
```

Avoid putting API keys directly into scripts, shell history, source control, or issue logs.

## Current platform boundary

The Windows build is native, but the existing kernel sandbox backend is implemented for Unix systems. On Windows, the `sandbox-enforce` feature currently has no kernel-enforcement backend. Treat commands and external tools launched by DTTN as unsandboxed until a Windows security backend is implemented.

The release workflow validates compilation, binary startup, archive integrity, and a real offline installer round trip on `windows-latest`. Full-screen TUI behavior should additionally be tested manually in Windows Terminal before a release is marked stable.

## Uninstall

Remove the installed executable and, if desired, remove its directory from the user `PATH`:

```powershell
Remove-Item "$env:LOCALAPPDATA\Programs\DTTN\bin\dttn.exe" -Force
```

The installer does not delete DTTN configuration or session data. Remove those separately only when they are no longer needed.
