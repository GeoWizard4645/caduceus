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
         caduceus-native    Vision OCR, CoreAudio device switching, colour sampling.\n\
         caduceus-record    Screen and meeting recording, with system audio.\n",
    );

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    compile_swift(
        &bin_dir,
        "macos/CaduceusSTT.swift",
        "caduceus-stt",
        "speech-to-text helper",
        SPEECH_HELPER_ID,
    );
    compile_swift(
        &bin_dir,
        "macos/CaduceusSTTLive.swift",
        "caduceus-stt-live",
        "live speech helper",
        SPEECH_HELPER_ID,
    );
    // Vision and CoreAudio need no TCC grant of their own, so this one carries
    // its own identifier and no usage-description strings.
    compile_swift(
        &bin_dir,
        "macos/CaduceusNative.swift",
        "caduceus-native",
        "OCR and audio helper",
        "com.caduceus.desktop.native-helper",
    );
    // The recorder asks for Screen Recording *and* the microphone, so it needs
    // the usage-description plist like the speech helpers — but its own
    // identifier, because Screen Recording is a grant people should be able to
    // reason about separately from dictation.
    compile_swift(
        &bin_dir,
        "macos/CaduceusRecorder.swift",
        "caduceus-record",
        "screen and meeting recorder",
        "com.caduceus.desktop.recorder",
    );
}

/// Signing identifier for the helpers that ask for microphone and speech access.
///
/// Shared between both speech helpers because TCC keys its grant on this string:
/// giving them separate identifiers would mean two prompts for one capability.
const SPEECH_HELPER_ID: &str = "com.caduceus.desktop.speech-helper";

fn compile_swift(
    bin_dir: &Path,
    source_rel: &str,
    output_name: &str,
    label: &str,
    identifier: &str,
) {
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
            seal_helper_signature(&output, label, identifier);
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

/// Re-sign a helper so its embedded Info.plist is part of the signature.
///
/// Linking the plist in is only half the job. `ld` leaves the binary
/// *linker-signed*, and `codesign -dv` on that reports `Info.plist=not bound` —
/// TCC reads usage-description strings through the code signature, so an
/// unbound section is invisible to it. The result is the exact failure the
/// plist was added to prevent: no prompt, no authorisation callback, and a
/// helper that blocks on its semaphore until it times out.
///
/// Tauri signs `Contents/MacOS` and the bundle itself but not nested files
/// under `Resources`, so this has to happen here. Ad-hoc is all that is
/// available without a Developer ID, and it is enough — binding the plist is
/// what matters, not who signed it.
fn seal_helper_signature(output: &Path, label: &str, identifier: &str) {
    let signed = Command::new("codesign")
        .args(["--force", "--sign", "-", "--identifier"])
        .arg(identifier)
        .arg(output)
        .output();

    match signed {
        Ok(out) if out.status.success() => {}
        Ok(out) => println!(
            "cargo:warning=could not sign {label}; microphone and speech prompts will not appear. codesign said: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => println!(
            "cargo:warning=codesign unavailable ({e}); {label} will not be able to ask for microphone or speech permission"
        ),
    }
}
