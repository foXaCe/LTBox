fn main() {
    // Generate both Rust and the subset font under OUT_DIR. The upstream
    // builder accepts an absolute module path, avoiding tracked source writes.
    println!("cargo:rerun-if-changed=fonts/lucide.toml");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let module = out.join("icon").to_string_lossy().replace('\\', "/");
    let definition = std::fs::read_to_string("fonts/lucide.toml").expect("Read icon definition");
    assert!(definition.contains("module = \"icon\""));
    let definition = definition.replace("module = \"icon\"", &format!("module = {module:?}"));
    let generated_definition = out.join("lucide.toml");
    if std::fs::read_to_string(&generated_definition)
        .ok()
        .as_deref()
        != Some(&definition)
    {
        std::fs::write(&generated_definition, definition).expect("Write generated icon definition");
    }
    iced_lucide::build(&generated_definition).expect("Generate Lucide icons");
    // The pinned generator targets iced git; our iced 0.14 uses with_name.
    let generated = out.join("icon.rs");
    let source = std::fs::read_to_string(&generated).expect("Read generated icon module");
    let adapted = source.replace("Font::new(", "Font::with_name(");
    if adapted != source {
        std::fs::write(generated, adapted).expect("Adapt generated icon module for iced 0.14");
    }
    emit_git_hash();

    // `#[cfg(target_os = "windows")]` would evaluate against the HOST build.rs
    // runs on, not the cargo `--target`; on a Linux `cargo xwin` cross build
    // that host is Linux, so the block would be skipped and the .exe would
    // ship without the icon resource and the stack-reserve linker arg (the
    // taskbar icon would still work, since that one is set at runtime via
    // `iced::window::icon`). `CARGO_CFG_TARGET_OS` carries the BUILD TARGET's
    // OS instead, so gate on that.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        embed_windows_resources();
        // Bump the main-thread stack reserve to 8 MB on Windows. The
        // default 1 MB is too tight for debug builds: iced + cosmic-text
        // shaping with the Noto Sans CJK bundle and the deeply nested
        // widget trees in the wizard exec screens blow past 1 MB and
        // trip `STATUS_STACK_OVERFLOW` (0xc00000fd) when a wizard's
        // confirm step pushes the app into the exec view.
        println!("cargo:rustc-link-arg=/STACK:8388608");
    }
}

/// Emit `LTBOX_GIT_HASH` (short commit) for the About panel. The crate sits two
/// levels below the repo root; rerun when `HEAD` or the refs move so the
/// embedded hash stays current. Empty when this is not the LTBox checkout
/// (e.g. a source tarball, possibly unpacked inside an unrelated repo), which
/// the About panel renders as the bare version.
fn emit_git_hash() {
    match ltbox_git_hash() {
        Some(hash) => {
            // Watch the *resolved* HEAD / refs / packed-refs so a commit or
            // checkout retriggers the script. `git rev-parse --git-path`
            // handles worktrees / submodules, where `.git` is a file pointing
            // elsewhere and the literal `../../.git/HEAD` would never change.
            for spec in ["HEAD", "packed-refs", "refs"] {
                if let Some(p) = run_git(&["rev-parse", "--git-path", spec]) {
                    let p = p.trim();
                    if !p.is_empty() {
                        println!("cargo:rerun-if-changed={p}");
                    }
                }
            }
            println!("cargo:rustc-env=LTBOX_GIT_HASH={hash}");
        }
        None => {
            // Not the LTBox checkout (tarball / no git). Watch the conventional
            // path so a later `git init` retriggers, and emit no hash.
            println!("cargo:rerun-if-changed=../../.git/HEAD");
            println!("cargo:rustc-env=LTBOX_GIT_HASH=");
        }
    }
}

/// Short `HEAD` commit, but only when git's toplevel is exactly this LTBox
/// checkout. A tarball nested under another Git repo would otherwise report the
/// parent repo's commit (`git rev-parse` walks upward), embedding an unrelated
/// build identifier.
fn ltbox_git_hash() -> Option<String> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    // <repo>/crates/ltbox-gui → <repo>.
    let expected_root = std::path::Path::new(&manifest).parent()?.parent()?;
    let expected_root = std::fs::canonicalize(expected_root).ok()?;

    let toplevel = run_git(&["rev-parse", "--show-toplevel"])?;
    if std::fs::canonicalize(toplevel.trim()).ok()? != expected_root {
        return None;
    }
    let hash = run_git(&["rev-parse", "--short", "HEAD"])?
        .trim()
        .to_string();
    (!hash.is_empty()).then_some(hash)
}

/// Run `git <args>` from the crate dir; `None` on spawn failure or non-zero exit.
fn run_git(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Compile + link the Windows resource (icon + version metadata).
///
/// Two paths because the tools available depend on the build host:
///
/// * Host = Windows: defer to `winresource`, which knows how to find
///   `rc.exe` in the installed Windows SDK and produce a `.lib` the
///   MSVC linker links automatically.
///
/// * Host = Linux (cargo-xwin cross): `winresource` only looks for
///   `rc.exe` via Windows SDK paths and fails. Drive `llvm-rc`
///   directly to compile the `.rc` into a `.res` blob, then pass the
///   `.res` to lld-link via `cargo:rustc-link-arg`. lld-link consumes
///   `.res` natively.
fn embed_windows_resources() {
    let host = std::env::var("HOST").unwrap_or_default();
    let host_is_windows = host.contains("windows");

    println!("cargo:rerun-if-changed=assets/icon.ico");

    if host_is_windows {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "LTBox");
        res.set("FileDescription", "LTBox");
        res.set("InternalName", "LTBox");
        res.set("OriginalFilename", "ltbox.exe");
        res.compile().expect("Failed to compile Windows resources");
        return;
    }

    // Linux host → write a minimal .rc, run llvm-rc, link the .res.
    use std::io::Write;
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR set by cargo"));
    let rc_path = out_dir.join("ltbox.rc");
    let res_path = out_dir.join("ltbox.res");
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let icon_abs = std::path::Path::new(&manifest_dir)
        .join("assets")
        .join("icon.ico");

    // RT_GROUP_ICON `MAINICON` so Windows shell pulls the multi-res
    // ICO in. VERSIONINFO mirrors the winresource defaults so the
    // file properties dialog shows the same product / file desc /
    // internal name as the host-Windows build.
    let pkg_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let parts: Vec<u32> = pkg_version
        .split(['.', '-'])
        .filter_map(|p| p.parse().ok())
        .chain(std::iter::repeat(0))
        .take(4)
        .collect();
    let (v0, v1, v2, v3) = (parts[0], parts[1], parts[2], parts[3]);
    let icon_str = icon_abs.display().to_string().replace('\\', "/");
    let rc_src = format!(
        r#"#pragma code_page(65001)
MAINICON ICON "{icon_str}"

1 VERSIONINFO
FILEVERSION {v0},{v1},{v2},{v3}
PRODUCTVERSION {v0},{v1},{v2},{v3}
FILEOS 0x40004
FILETYPE 0x1
{{
  BLOCK "StringFileInfo"
  {{
    BLOCK "040904b0"
    {{
      VALUE "ProductName", "LTBox\0"
      VALUE "FileDescription", "LTBox\0"
      VALUE "InternalName", "LTBox\0"
      VALUE "OriginalFilename", "ltbox.exe\0"
      VALUE "FileVersion", "{pkg_version}\0"
      VALUE "ProductVersion", "{pkg_version}\0"
    }}
  }}
  BLOCK "VarFileInfo"
  {{
    VALUE "Translation", 0x0409, 0x04B0
  }}
}}
"#,
    );
    let mut f = std::fs::File::create(&rc_path).expect("create ltbox.rc");
    f.write_all(rc_src.as_bytes()).expect("write ltbox.rc");
    drop(f);

    // `llvm-rc` is installed by the CI workflow's `apt-get install
    // llvm` step. The `/fo` MSVC-style flag points at the output
    // .res; llvm-rc emits a Microsoft-format .res that lld-link
    // links directly with no .cvtres step.
    let llvm_rc = std::env::var("LLVM_RC").unwrap_or_else(|_| "llvm-rc".into());
    let status = std::process::Command::new(&llvm_rc)
        .arg(format!("/fo{}", res_path.display()))
        .arg(&rc_path)
        .status()
        .unwrap_or_else(|e| panic!("spawn {llvm_rc}: {e}"));
    assert!(
        status.success(),
        "{llvm_rc} failed on {}",
        rc_path.display()
    );

    println!("cargo:rustc-link-arg-bin=ltbox={}", res_path.display());
}
