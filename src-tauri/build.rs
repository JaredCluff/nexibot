fn main() {
    tauri_build::build();

    // Stage native DLLs for Windows bundling.
    // Dependency build scripts (ort-sys, sherpa-rs-sys) place DLLs in
    // target/{profile}/deps/ before this build script runs. We copy them
    // to the src-tauri root so the Tauri bundler includes them as flat
    // resources alongside the executable (configured in tauri.conf.windows.json).
    #[cfg(windows)]
    stage_native_dlls();

    // Compile Swift bridge for macOS Speech recognition
    #[cfg(target_os = "macos")]
    if let Err(e) = compile_swift_bridge() {
        eprintln!("Swift bridge build failed: {}", e);
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn stage_native_dlls() {
    use std::fs;
    use std::path::Path;

    let out_dir = std::env::var("OUT_DIR").unwrap();
    // OUT_DIR = {target}/{profile}/build/{package}-{hash}/out
    // Navigate up to {target}/{profile}/
    let profile_dir = Path::new(&out_dir)
        .parent()
        .unwrap() // {package}-{hash}
        .parent()
        .unwrap() // build/
        .parent()
        .unwrap(); // {profile}/

    let deps_dir = profile_dir.join("deps");

    let dll_names = [
        "onnxruntime.dll",
        "onnxruntime_providers_shared.dll",
        "sherpa-onnx-c-api.dll",
        "sherpa-onnx-cxx-api.dll",
        "DirectML.dll",
        "cargs.dll",
    ];

    // Stage DLLs at the root of src-tauri/ so the Tauri bundler places them
    // alongside the executable (Windows DLL search requires this).
    let staging_dir = Path::new(".");

    let mut staged = 0;
    for dll_name in &dll_names {
        // Check deps/ first (populated by dependency build scripts), then profile root
        let candidates = [deps_dir.join(dll_name), profile_dir.join(dll_name)];

        if let Some(source) = candidates.iter().find(|p| p.exists()) {
            let dest = staging_dir.join(dll_name);
            match fs::copy(source, &dest) {
                Ok(_) => staged += 1,
                Err(e) => {
                    println!("cargo:warning=Failed to stage {}: {}", dll_name, e);
                }
            }
        } else {
            println!(
                "cargo:warning=Native DLL not found (first build?): {}",
                dll_name
            );
        }
    }

    if staged > 0 {
        println!("cargo:warning=Staged {} native DLLs for bundling", staged);
    }

    // Re-run if DLLs change
    for dll_name in &dll_names {
        println!("cargo:rerun-if-changed={}", dll_name);
    }
}

#[cfg(target_os = "macos")]
fn compile_swift_bridge() -> Result<(), String> {
    use std::path::Path;
    use std::process::Command;

    let swift_source = "src/platform/speech_bridge.swift";
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let object_file = Path::new(&out_dir).join("speech_bridge.o");
    let lib_file = Path::new(&out_dir).join("libspeech_bridge.a");

    println!("cargo:rerun-if-changed={}", swift_source);

    // Detect target architecture
    let target = std::env::var("TARGET").unwrap();
    let swift_target = if target.contains("aarch64") {
        "aarch64-apple-macosx11.0"
    } else {
        "x86_64-apple-macosx10.15"
    };

    println!(
        "cargo:warning=Compiling Swift bridge for target: {}",
        swift_target
    );

    // Resolve swiftc: honour SWIFT_EXEC, then DEVELOPER_DIR, then known Xcode path,
    // then fall back to PATH lookup (xcrun). This avoids xcrun failures when the
    // shell environment has a stale Xcode license state.
    let developer_dir = std::env::var("DEVELOPER_DIR")
        .unwrap_or_else(|_| "/Applications/Xcode.app/Contents/Developer".to_string());
    let swiftc_candidates = [
        std::env::var("SWIFT_EXEC").unwrap_or_default(),
        format!("{}/Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc", developer_dir),
        "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc".to_string(),
    ];
    let swiftc = swiftc_candidates
        .iter()
        .find(|p| !p.is_empty() && Path::new(p.as_str()).exists())
        .map(|s| s.as_str())
        .unwrap_or("swiftc");

    // Build the primary SDK path from DEVELOPER_DIR.
    // When running with Command Line Tools (no Xcode), the SDK lives at
    // /Library/Developer/CommandLineTools/SDKs/MacOSX.sdk instead of the
    // Xcode platform path. Fall back to xcrun --show-sdk-path.
    let sdk_candidate = format!(
        "{}/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk",
        developer_dir
    );
    let sdk = if Path::new(&sdk_candidate).exists() {
        sdk_candidate
    } else {
        // Try xcrun as a fallback (works with Command Line Tools)
        Command::new("xcrun")
            .args(["--show-sdk-path"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && Path::new(s.as_str()).exists())
            .unwrap_or(sdk_candidate)
    };

    // Compile Swift to object file
    let status = Command::new(swiftc)
        .args([
            "-c",
            swift_source,
            "-o",
            object_file.to_str().unwrap(),
            "-sdk",
            &sdk,
            "-target",
            swift_target,
        ])
        .status()
        .map_err(|e| {
            format!(
                "Failed to execute swiftc. Make sure Xcode is installed. ({})",
                e
            )
        })?;

    if !status.success() {
        return Err("Swift compilation failed".to_string());
    }

    // Create static library
    let status = Command::new("ar")
        .args([
            "rcs",
            lib_file.to_str().unwrap(),
            object_file.to_str().unwrap(),
        ])
        .status()
        .map_err(|e| format!("Failed to execute ar: {}", e))?;

    if !status.success() {
        return Err("Failed to create static library".to_string());
    }

    // Link the library and frameworks.
    // cargo:rustc-link-lib reaches the [lib] compile step.
    // cargo:rustc-link-arg-bins is required for the [[bin]] link step: when both
    // [lib] and [[bin]] coexist in the same package, Cargo propagates the -L search
    // paths from the build script to the binary but NOT the -l library names.
    // The explicit rustc-link-arg-bins directives below work around that gap.
    println!("cargo:rustc-link-search=native={}", out_dir);
    println!("cargo:rustc-link-lib=static=speech_bridge");
    println!("cargo:rustc-link-arg-bins=-L{}", out_dir);
    println!("cargo:rustc-link-arg-bins=-lspeech_bridge");

    println!("cargo:rustc-link-lib=framework=Speech");
    println!("cargo:rustc-link-arg-bins=-framework");
    println!("cargo:rustc-link-arg-bins=Speech");

    println!("cargo:rustc-link-lib=framework=AVFoundation");
    println!("cargo:rustc-link-arg-bins=-framework");
    println!("cargo:rustc-link-arg-bins=AVFoundation");

    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-arg-bins=-framework");
    println!("cargo:rustc-link-arg-bins=Foundation");

    // Add Swift library search paths.
    // Derive the canonical Swift lib path from the swiftc binary that was
    // actually used — this covers both full Xcode and Command Line Tools
    // installs.  Xcode: .../Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc
    // CLT:             /Library/Developer/CommandLineTools/usr/bin/swiftc
    // Both have their Swift stdlib / compat shims two directories up in lib/swift/macosx.
    let swiftc_canonical = Command::new("xcrun")
        .args(["--find", "swiftc"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| swiftc.to_string());
    let swift_lib_dir = Path::new(&swiftc_canonical)
        .parent() // bin/
        .and_then(|p| p.parent()) // usr/
        .map(|p| p.join("lib/swift/macosx"))
        .filter(|p| p.exists());

    // Always add the Xcode toolchain path (works when Xcode is installed).
    println!(
        "cargo:rustc-link-search={}/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx",
        developer_dir
    );
    println!(
        "cargo:rustc-link-arg-bins=-L{}/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx",
        developer_dir
    );
    // Also add the path derived from the actual swiftc binary (CLT fallback).
    if let Some(ref p) = swift_lib_dir {
        let ps = p.to_string_lossy();
        println!("cargo:rustc-link-search=native={}", ps);
        println!("cargo:rustc-link-arg-bins=-L{}", ps);
    }
    println!("cargo:rustc-link-search=/usr/lib/swift");
    println!("cargo:rustc-link-arg-bins=-L/usr/lib/swift");

    // Link Swift runtime libraries
    println!("cargo:rustc-link-lib=dylib=swiftCore");
    println!("cargo:rustc-link-arg-bins=-lswiftCore");
    println!("cargo:rustc-link-lib=dylib=swiftFoundation");
    println!("cargo:rustc-link-arg-bins=-lswiftFoundation");
    println!("cargo:rustc-link-lib=dylib=swiftObjectiveC");
    println!("cargo:rustc-link-arg-bins=-lswiftObjectiveC");
    println!("cargo:rustc-link-lib=dylib=swiftDarwin");
    println!("cargo:rustc-link-arg-bins=-lswiftDarwin");

    Ok(())
}
