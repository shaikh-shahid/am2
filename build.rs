use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/ghostty_bridge.m");
    println!("cargo:rerun-if-changed=src/terminal_bridge.m");
    #[cfg(target_os = "macos")]
    {
        let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
        let helper = out.join("ghostty-bridge");
        let status = Command::new("clang")
            .args([
                "-fobjc-arc",
                "-framework",
                "AppKit",
                "-framework",
                "ScriptingBridge",
            ])
            .arg("src/ghostty_bridge.m")
            .arg("-o")
            .arg(&helper)
            .env("CLANG_MODULE_CACHE_PATH", out.join("clang-module-cache"))
            .status()
            .expect("clang is required for the macOS Ghostty adapter");
        assert!(status.success(), "could not compile Ghostty bridge");
        println!("cargo:rustc-env=AM2_GHOSTTY_BRIDGE={}", helper.display());

        let terminal_helper = out.join("terminal-bridge");
        let status = Command::new("clang")
            .args([
                "-fobjc-arc",
                "-framework",
                "AppKit",
                "-framework",
                "ScriptingBridge",
            ])
            .arg("src/terminal_bridge.m")
            .arg("-o")
            .arg(&terminal_helper)
            .env("CLANG_MODULE_CACHE_PATH", out.join("clang-module-cache"))
            .status()
            .expect("clang is required for the macOS terminal adapters");
        assert!(status.success(), "could not compile terminal bridge");
        println!(
            "cargo:rustc-env=AM2_TERMINAL_BRIDGE={}",
            terminal_helper.display()
        );
    }
}
