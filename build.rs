use std::{env, path::PathBuf, process::Command};

fn command_output(program: &str, args: &[&str]) -> String {
    let output = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to run {program}: {error}"));
    if !output.status.success() {
        panic!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8(output.stdout)
        .expect("command output was not UTF-8")
        .trim()
        .to_owned()
}

fn main() {
    println!("cargo:rerun-if-changed=packaging/macos/updater_bootstrap.m");

    let target = env::var("TARGET").expect("TARGET was not set by Cargo");
    if !target.ends_with("apple-darwin") {
        return;
    }

    let clang_target = match target.as_str() {
        "aarch64-apple-darwin" => "arm64-apple-macos13.0",
        "x86_64-apple-darwin" => "x86_64-apple-macos13.0",
        other => panic!("unsupported macOS target: {other}"),
    };
    let clang = command_output("xcrun", &["--find", "clang"]);
    let sdk = command_output("xcrun", &["--sdk", "macosx", "--show-sdk-path"]);
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR was not set by Cargo"));
    let object = out.join("updater_bootstrap.o");

    let status = Command::new(clang)
        .args([
            "-fobjc-arc",
            "-fmodules",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-target",
            clang_target,
            "-isysroot",
            &sdk,
            "-c",
            "packaging/macos/updater_bootstrap.m",
            "-o",
        ])
        .arg(&object)
        .status()
        .expect("failed to compile the macOS updater bootstrap");
    assert!(
        status.success(),
        "failed to compile the macOS updater bootstrap"
    );

    // Link the object directly so the Objective-C class and its +load method
    // cannot be discarded as an unreferenced member of a static archive.
    println!(
        "cargo:rustc-link-arg-bin=jcode-desktop={}",
        object.display()
    );
    println!("cargo:rustc-link-lib=framework=AppKit");
}
