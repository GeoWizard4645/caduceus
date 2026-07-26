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

    // Usage-description strings for the helpers, linked into the binary. Without
    // this section macOS never prompts and never calls the authorisation
    // callback, so the helper hangs instead of failing — see the plist itself.
    let helper_plist = Path::new(&manifest_dir).join("macos/HelperInfo.plist");
    println!("cargo:rerun-if-changed=macos/HelperInfo.plist");

    let output = bin_dir.join(output_name);
    let up_to_date = match (
        std::fs::metadata(&output),
        std::fs::metadata(&source),
        std::fs::metadata(&helper_plist),
    ) {
        (Ok(out), Ok(src), plist) => match (out.modified(), src.modified()) {
            (Ok(out_time), Ok(src_time)) => {
                let plist_time = plist.ok().and_then(|m| m.modified().ok());
                out_time >= src_time && plist_time.is_none_or(|t| out_time >= t)
            }
            _ => false,
        },
        _ => false,
    };
    if up_to_date {
        return;
    }

    let mut cmd = Command::new("swiftc");
    cmd.arg("-O").arg("-o").arg(&output).arg(&source);
    if helper_plist.is_file() {
        for arg in [
            "-Xlinker",
            "-sectcreate",
            "-Xlinker",
            "__TEXT",
            "-Xlinker",
            "__info_plist",
            "-Xlinker",
        ] {
            cmd.arg(arg);
        }
        cmd.arg(&helper_plist);
    } else {
        println!("cargo:warning=macos/HelperInfo.plist is missing; {label} will not be able to ask for microphone or speech permission");
    }

    match cmd.output() {
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
