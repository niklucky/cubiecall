fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS");
    // libwebrtc ships Objective-C categories (e.g. NSString+StdString) inside a
    // static library; without -ObjC the linker drops them and WebRTC crashes
    // with "unrecognized selector" on first use.
    if target_os.as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-bins=-ObjC");
    }
    if target_os.as_deref() == Ok("windows") {
        windows::resources();
    }
}

/// Cubie.exe's icon and version: a resource script compiled with the Windows
/// SDK's rc.exe, or llvm-rc when cross-compiling (cargo-xwin). The manifest
/// (Common Controls 6, per-monitor DPI) comes with GPUI, which embeds its own.
mod windows {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::{env, fs};

    pub fn resources() {
        for var in ["CUBIE_VERSION", "CUBIE_BUILD", "RC"] {
            println!("cargo:rerun-if-env-changed={var}");
        }
        let assets = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("assets");
        let icon = assets.join("Cubie.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
        let script = out.join("cubie.rc");
        let res = out.join("cubie.res");
        fs::write(&script, script_text(&icon)).expect("write cubie.rc");

        let Some(rc) = find_rc() else {
            println!(
                "cargo:warning=No resource compiler (rc.exe or llvm-rc; set RC to one): \
                 Cubie.exe gets no icon or version"
            );
            return;
        };
        let llvm = rc
            .file_stem()
            .is_some_and(|s| s.to_string_lossy().starts_with("llvm-rc"));
        let mut command = Command::new(&rc);
        if llvm {
            // No #include or #define to expand, and no clang to find.
            command.arg("-no-preprocess");
        } else {
            command.arg("/nologo");
        }
        // Relative names, run from OUT_DIR: llvm-rc reads an absolute path
        // like `/Users/…` as its `/U` switch, and then finds no input.
        command
            .current_dir(&out)
            .arg("/fo")
            .arg("cubie.res")
            .arg("cubie.rc");
        match command.status() {
            Ok(status) if status.success() => {
                println!("cargo:rustc-link-arg-bins={}", res.display());
            }
            Ok(status) => println!(
                "cargo:warning={} failed ({status}) on {}: Cubie.exe gets no icon or version",
                rc.display(),
                script.display()
            ),
            Err(e) => println!(
                "cargo:warning=couldn't run {}: {e}: Cubie.exe gets no icon or version",
                rc.display()
            ),
        }
    }

    /// The resource script. Paths use forward slashes, which both compilers
    /// take as they are (backslashes would be escapes in their strings).
    fn script_text(icon: &Path) -> String {
        let version = env::var("CUBIE_VERSION")
            .unwrap_or_else(|_| env::var("CARGO_PKG_VERSION").unwrap())
            .replace('"', "");
        let build = env::var("CUBIE_BUILD")
            .ok()
            .and_then(|b| b.trim().parse::<u32>().ok());
        let mut numbers = version
            .split(['.', '-', '+'])
            .take(3)
            .map(|n| n.parse::<u16>().unwrap_or(0))
            .collect::<Vec<_>>();
        numbers.resize(3, 0);
        // A field holds 16 bits; a bigger build number shows only in the text.
        numbers.push(build.and_then(|b| u16::try_from(b).ok()).unwrap_or(0));
        let numeric = numbers
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let file_version = match build {
            Some(build) => format!("{version} ({build})"),
            None => version.clone(),
        };
        let path = |p: &Path| p.display().to_string().replace('\\', "/");
        format!(
            r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEFLAGSMASK 0x3f
FILEFLAGS 0x0
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0x0
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "FileDescription", "Cubie"
            VALUE "FileVersion", "{file_version}"
            VALUE "InternalName", "Cubie"
            VALUE "OriginalFilename", "Cubie.exe"
            VALUE "ProductName", "Cubie"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#,
            icon = path(icon),
        )
    }

    /// `RC` if set; else the newest Windows SDK's rc.exe (Windows hosts);
    /// else rc.exe or llvm-rc on the PATH.
    fn find_rc() -> Option<PathBuf> {
        if let Some(rc) = env::var_os("RC") {
            return Some(rc.into());
        }
        sdk_rc()
            .or_else(|| on_path("rc.exe"))
            .or_else(|| on_path("llvm-rc"))
            .or_else(|| on_path("llvm-rc.exe"))
    }

    fn sdk_rc() -> Option<PathBuf> {
        if !cfg!(windows) {
            return None;
        }
        let arch = if env::var("HOST").is_ok_and(|h| h.starts_with("aarch64")) {
            "arm64"
        } else {
            "x64"
        };
        // A developer prompt names the SDK in use; otherwise take the newest.
        if let Some(bin) = env::var_os("WindowsSdkVerBinPath") {
            let rc = PathBuf::from(bin).join(arch).join("rc.exe");
            if rc.is_file() {
                return Some(rc);
            }
        }
        let kits = PathBuf::from(env::var_os("ProgramFiles(x86)")?).join("Windows Kits/10/bin");
        let version = |path: &Path| {
            path.file_name()?
                .to_str()?
                .split('.')
                .map(|n| n.parse::<u32>().ok())
                .collect::<Option<Vec<_>>>()
        };
        fs::read_dir(kits)
            .ok()?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter_map(|dir| Some((version(&dir)?, dir.join(arch).join("rc.exe"))))
            .filter(|(_, rc)| rc.is_file())
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, rc)| rc)
    }

    fn on_path(name: &str) -> Option<PathBuf> {
        env::split_paths(&env::var_os("PATH")?)
            .map(|dir| dir.join(name))
            .find(|path| path.is_file())
    }
}
