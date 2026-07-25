use std::path::Path;
use std::process::Command;

fn main() {
    build_macos_stt_helper();
    tauri_build::build()
}

/// Write a file only when its contents would actually change.
///
/// Rewriting an identical file still bumps its mtime, which any file watcher
/// treats as a change. During `tauri dev` that is enough to trigger a rebuild,
/// which re-runs this script — an infinite loop. (`src-tauri/.taurignore` also
/// excludes `bin/`; this is the belt to that pair of braces.)
fn write_if_changed(path: &Path, contents: &[u8]) {
    if std::fs::read(path).is_ok_and(|existing| existing == contents) {
        return;
    }
    if let Err(e) = std::fs::write(path, contents) {
        println!("cargo:warning=could not write {}: {e}", path.display());
    }
}

/// Compile the macOS speech-to-text helper, if we can.
///
/// This is deliberately **best-effort**: a missing or broken `swiftc` must not
/// fail the build, because the helper is optional — Orbit falls back to an HTTP
/// speech-to-text endpoint, and everything except the "System" STT backend works
/// without it. The `bin/` directory always ends up with at least one file so the
/// bundler's resource glob never resolves to nothing.
fn build_macos_stt_helper() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let bin_dir = Path::new(&manifest_dir).join("bin");
    let _ = std::fs::create_dir_all(&bin_dir);

    // Keeps the resource glob non-empty on every platform, and explains itself
    // to anyone who finds it in the bundle.
    write_if_changed(
        &bin_dir.join("README.txt"),
        b"Helper executables bundled with Orbit.\n\n\
         orbit-stt  macOS only. Transcribes a WAV file using Apple's Speech\n\
         framework. Built from macos/OrbitSTT.swift by build.rs. If it is\n\
         missing, Orbit's \"System\" speech-to-text backend reports that it is\n\
         unavailable and you can use an HTTP endpoint instead.\n",
    );

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let source = Path::new(&manifest_dir).join("macos/OrbitSTT.swift");
    println!("cargo:rerun-if-changed=macos/OrbitSTT.swift");
    if !source.exists() {
        return;
    }

    let output = bin_dir.join("orbit-stt");

    // Skip the compile when the binary is already newer than its source, for
    // the same watcher reason as `write_if_changed` above.
    let up_to_date = match (std::fs::metadata(&output), std::fs::metadata(&source)) {
        (Ok(out), Ok(src)) => match (out.modified(), src.modified()) {
            (Ok(out_time), Ok(src_time)) => out_time >= src_time,
            _ => false,
        },
        _ => false,
    };
    if up_to_date {
        return;
    }

    match Command::new("swiftc")
        .arg("-O")
        .arg("-o")
        .arg(&output)
        .arg(&source)
        .output()
    {
        Ok(out) if out.status.success() => {
            println!("cargo:warning=built the macOS speech-to-text helper (bin/orbit-stt)");
        }
        Ok(out) => {
            println!(
                "cargo:warning=could not compile the macOS speech-to-text helper; \
                 the \"System\" voice backend will be unavailable. swiftc said: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Err(e) => {
            println!(
                "cargo:warning=swiftc is not available ({e}); the \"System\" voice backend \
                 will be unavailable. Install the Xcode Command Line Tools to enable it."
            );
        }
    }
}
