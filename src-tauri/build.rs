use std::path::Path;
use std::process::Command;

fn main() {
    build_macos_helpers();
    tauri_build::build()
}

fn write_if_changed(path: &Path, contents: &[u8]) {
    if std::fs::read(path).is_ok_and(|existing| existing == contents) {
        return;
    }
    if let Err(e) = std::fs::write(path, contents) {
        println!("cargo:warning=could not write {}: {e}", path.display());
    }
}

fn build_macos_helpers() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let bin_dir = Path::new(&manifest_dir).join("bin");
    let _ = std::fs::create_dir_all(&bin_dir);

    write_if_changed(
        &bin_dir.join("README.txt"),
        b"Helper executables bundled with Caduceus.\n\n\
         caduceus-stt       Transcribe a WAV (batch).\n\
         caduceus-stt-live  Live mic + partial transcripts.\n\
         caduceus-record    Screen recording (ReplayKit).\n",
    );

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    compile_swift(
        &bin_dir,
        "macos/CaduceusSTT.swift",
        "caduceus-stt",
        "speech-to-text helper",
    );
    compile_swift(
        &bin_dir,
        "macos/CaduceusSTTLive.swift",
        "caduceus-stt-live",
        "live speech helper",
    );
}

fn compile_swift(bin_dir: &Path, source_rel: &str, output_name: &str, label: &str) {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let source = Path::new(&manifest_dir).join(source_rel);
    println!("cargo:rerun-if-changed={source_rel}");
    if !source.exists() {
        return;
    }

    let output = bin_dir.join(output_name);
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
            println!("cargo:warning=built macOS {label} ({output_name})");
        }
        Ok(out) => {
            println!(
                "cargo:warning=could not compile {label}; swiftc said: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Err(e) => {
            println!("cargo:warning=swiftc unavailable ({e}); {label} will be missing");
        }
    }
}
