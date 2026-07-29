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
         caduceus-parakeet-live  MacParakeet-style local live transcription (Apple Silicon).\n\
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
    build_parakeet_helper(&bin_dir);
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

/// Build the Apple-Silicon Parakeet helper through SwiftPM.
///
/// FluidAudio/CoreML is Apple-Silicon-only. The existing universal Apple
/// Speech helper remains in the bundle and is selected as the fallback on
/// Intel or when this optional build is unavailable.
fn build_parakeet_helper(bin_dir: &Path) {
    if std::env::consts::ARCH != "aarch64" {
        return;
    }

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let package_dir = Path::new(&manifest_dir).join("macos");
    let output = bin_dir.join("caduceus-parakeet-live");
    // Under `target/` so Tauri's dev watcher (see `.taurignore`) never sees
    // SwiftPM churn and restart cargo in a loop.
    let build_path = Path::new(&manifest_dir).join("target").join("swift-parakeet");
    let module_cache = build_path.join("module-cache");
    let _ = std::fs::create_dir_all(&module_cache);
    println!("cargo:rerun-if-changed=macos/Package.swift");
    println!("cargo:rerun-if-changed=macos/Package.resolved");
    println!("cargo:rerun-if-changed=macos/ParakeetLive");

    let built = build_path
        .join("arm64-apple-macosx")
        .join("release")
        .join("caduceus-parakeet-live");

    if parakeet_bundle_up_to_date(&output, &package_dir) {
        return;
    }

    let status = Command::new("swift")
        .current_dir(&package_dir)
        // Keep Swift/Clang caches inside the project. This also makes builds
        // work in sandboxed CI where ~/.cache and ~/Library/Caches are read-only.
        .env("CLANG_MODULE_CACHE_PATH", &module_cache)
        .env("SWIFTPM_MODULECACHE_OVERRIDE", &module_cache)
        .args([
            "build",
            "--build-path",
        ])
        .arg(&build_path)
        .args([
            "--disable-sandbox",
            "-c",
            "release",
            "--arch",
            "arm64",
            "--product",
            "caduceus-parakeet-live",
        ])
        .status();

    if !matches!(status, Ok(value) if value.success()) {
        println!("cargo:warning=Parakeet helper did not build; Caduceus will use Apple Speech");
        return;
    }
    if std::fs::copy(&built, &output).is_err() {
        println!("cargo:warning=Parakeet helper built but could not be bundled");
        return;
    }
    seal_helper_signature(&output, "Parakeet live speech helper", SPEECH_HELPER_ID);
}

fn parakeet_bundle_up_to_date(output: &Path, package_dir: &Path) -> bool {
    let Ok(out_meta) = std::fs::metadata(output) else {
        return false;
    };
    let Ok(out_time) = out_meta.modified() else {
        return false;
    };
    for path in [
        package_dir.join("Package.swift"),
        package_dir.join("Package.resolved"),
    ] {
        if path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .is_some_and(|t| t > out_time)
        {
            return false;
        }
    }
    let parakeet_src = package_dir.join("ParakeetLive");
    if newest_mtime(&parakeet_src).is_some_and(|t| t > out_time) {
        return false;
    }
    true
}

fn newest_mtime(root: &Path) -> Option<std::time::SystemTime> {
    let mut newest: Option<std::time::SystemTime> = None;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read = std::fs::read_dir(&dir).ok()?;
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(t) = entry.metadata().and_then(|m| m.modified()) {
                newest = Some(newest.map_or(t, |n: std::time::SystemTime| n.max(t)));
            }
        }
    }
    newest
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

    if !helper_plist.is_file() {
        println!("cargo:warning=macos/HelperInfo.plist is missing; {label} will not be able to ask for microphone or speech permission");
    }

    // Built for both architectures and merged.
    //
    // `swiftc` with no `-target` compiles for the *host*, and the release
    // script's Intel leg reuses whatever the Apple Silicon leg already put in
    // `bin/` because it is newer than the source. The result shipped in the
    // "universal" DMG for months: a fat main binary next to four thin arm64
    // helpers. On an Intel Mac every one of them is found by `is_file()` — so
    // the code takes the "helper present" path — and then fails to exec, which
    // is a worse failure than the missing-helper message it would otherwise
    // have shown. Dictation, OCR, colour sampling, audio switching and
    // recording were all dead there.
    let mut slices = Vec::new();
    let mut built_any = false;
    for (arch, triple) in [
        ("arm64", "arm64-apple-macos11"),
        ("x86_64", "x86_64-apple-macos11"),
    ] {
        let slice = output.with_extension(arch);
        match compile_slice(&source, &slice, triple, &helper_plist) {
            Ok(()) => {
                slices.push(slice);
                built_any = true;
            }
            Err(e) => {
                // A missing SDK slice is normal on a machine that has never
                // needed it. One architecture is still a working build for the
                // person doing it; the release script is where both matter.
                println!("cargo:warning={label}: no {arch} slice ({e})");
            }
        }
    }

    if !built_any {
        println!("cargo:warning=could not compile {label} for any architecture");
        return;
    }

    let merged = if slices.len() > 1 {
        let lipo = Command::new("lipo")
            .arg("-create")
            .args(&slices)
            .arg("-output")
            .arg(&output)
            .output();
        matches!(lipo, Ok(out) if out.status.success())
    } else {
        std::fs::copy(&slices[0], &output).is_ok()
    };

    for slice in &slices {
        let _ = std::fs::remove_file(slice);
    }

    if !merged {
        println!("cargo:warning=could not assemble {label}");
        return;
    }

    println!(
        "cargo:warning=built macOS {label} ({output_name}, {})",
        if slices.len() > 1 {
            "universal"
        } else {
            "this architecture only"
        }
    );
    // Signed after lipo: merging rewrites the file and would invalidate a
    // signature applied to either slice.
    seal_helper_signature(&output, label, identifier);
}

/// Compile one architecture's slice of a helper.
fn compile_slice(
    source: &Path,
    output: &Path,
    target: &str,
    helper_plist: &Path,
) -> Result<(), String> {
    let mut cmd = Command::new("swiftc");
    cmd.arg("-O")
        .arg("-target")
        .arg(target)
        .arg("-o")
        .arg(output)
        .arg(source);

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
        cmd.arg(helper_plist);
    }

    match cmd.output() {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr)
            .trim()
            .lines()
            .last()
            .unwrap_or("swiftc failed")
            .to_string()),
        Err(e) => Err(e.to_string()),
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
