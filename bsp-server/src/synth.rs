//! No-build compiler-argument synthesis (the C track of the hybrid design).
//!
//! - ObjC/C/C++: `synthesize_clang_args` — ports the Phase 2 spike `synth2.py`
//!   (8/8 clean parse): baseline settings + `-fobjc-arc` + recursive source-tree
//!   header-dir scan. See design §6.2/§6.4.
//! - Swift: `synthesize_swift_args` — ports the Phase 3c spike `synth_swift.py`
//!   (whole-module, `swiftc -typecheck` clean). See design §6.3.
//!
//! Output feeds `sourceKitOptions.compilerArguments` (bsp_protocol_spec.md §5).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::build_settings::{split_multi, BuildSettings};

/// Directory names never worth scanning for headers (VCS + build outputs).
const EXCLUDE_DIRS: &[&str] = &[
    ".git",
    ".svn",
    ".hg",
    "build",
    "Build",
    "DerivedData",
    "DerivedSources",
    "Intermediates.noindex",
];

/// Synthesize per-file clang arguments (ObjC/C/C++), file path last.
/// `augmented_include_dirs` is the target's precomputed scoped header-dir list
/// (see `scoped_include_dirs`), passed in so it is computed once per target.
pub fn synthesize_clang_args(
    settings: &BuildSettings,
    file: &Path,
    augmented_include_dirs: &[String],
) -> Vec<String> {
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let is_cpp = matches!(ext.as_str(), "mm" | "cpp" | "cc" | "cxx");

    let mut args: Vec<String> = Vec::new();

    // Language + standard.
    args.push("-x".into());
    args.push(if is_cpp { "objective-c++".into() } else { "objective-c".into() });
    let std_flag = if is_cpp {
        settings.get("CLANG_CXX_LANGUAGE_STANDARD").unwrap_or("gnu++20")
    } else {
        settings.get("GCC_C_LANGUAGE_STANDARD").unwrap_or("gnu17")
    };
    args.push(format!("-std={std_flag}"));

    // Target triple (native arch; PLATFORM_NAME=macosx).
    let dep = settings.get("MACOSX_DEPLOYMENT_TARGET").unwrap_or("11.0");
    args.push("-target".into());
    args.push(format!("aarch64-apple-macos{dep}"));

    // ARC (load-bearing — the fixture project fails 2/8 files without it).
    if settings.bool_flag("CLANG_ENABLE_OBJC_ARC") {
        args.push("-fobjc-arc".into());
    }

    // System root.
    if let Some(sdk) = settings.get("SDKROOT") {
        if !sdk.is_empty() {
            args.push("-isysroot".into());
            args.push(sdk.to_string());
        }
    }

    // Header search paths (build setting) + precomputed source-tree augmentation.
    for p in split_multi(settings.get("HEADER_SEARCH_PATHS").unwrap_or("")) {
        args.push("-I".into());
        args.push(p);
    }
    for dir in augmented_include_dirs {
        args.push("-I".into());
        args.push(dir.clone());
    }

    // User header search paths -> -iquote.
    for p in split_multi(settings.get("USER_HEADER_SEARCH_PATHS").unwrap_or("")) {
        args.push("-iquote".into());
        args.push(p);
    }

    // Framework search paths.
    for p in split_multi(settings.get("FRAMEWORK_SEARCH_PATHS").unwrap_or("")) {
        args.push("-F".into());
        args.push(p);
    }

    // Preprocessor definitions.
    for d in split_multi(settings.get("GCC_PREPROCESSOR_DEFINITIONS").unwrap_or("")) {
        args.push(format!("-D{d}"));
    }

    // Clang modules.
    if settings.bool_flag("CLANG_ENABLE_MODULES") {
        args.push("-fmodules".into());
    }

    // Passthrough OTHER_* flags.
    let other_key = if is_cpp { "OTHER_CPLUSPLUSFLAGS" } else { "OTHER_CFLAGS" };
    for f in split_multi(settings.get(other_key).unwrap_or("")) {
        args.push(f);
    }

    // File last.
    args.push(file.to_string_lossy().into_owned());
    args
}

/// Header-dir augmentation scoped to a target: union of header-containing dirs
/// recursively under each `root` (the target's project dir + shared `common/`,
/// NOT the whole workspace — a workspace-wide scan collides sibling headers and
/// fails, Phase 3b spike). `existing_hsp` (the target's HEADER_SEARCH_PATHS) is
/// excluded to avoid emitting a dir twice. This is what moved the Phase 2 spike
/// from 5/8→8/8, now scoped per target.
pub fn scoped_include_dirs(roots: &[PathBuf], existing_hsp: &HashSet<String>) -> Vec<String> {
    let mut seen: HashSet<String> = existing_hsp.clone();
    let mut out: Vec<String> = Vec::new();
    for root in roots {
        for dir in augment_header_dirs(root, &seen) {
            seen.insert(dir.clone());
            out.push(dir);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Every directory under `srcroot` that contains a header (`.h`/`.hpp`/`.hh`),
/// excluding VCS/build junk and `*.xcodeproj`, and any dir already in `existing`.
/// Bounded to the real tree; symlinked dirs are not followed.
fn augment_header_dirs(srcroot: &Path, existing: &HashSet<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut stack = vec![srcroot.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut has_header = false;
        for entry in entries.flatten() {
            let Ok(ftype) = entry.file_type() else { continue };
            if ftype.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if EXCLUDE_DIRS.contains(&name.as_ref()) || name.ends_with(".xcodeproj") {
                    continue;
                }
                stack.push(entry.path());
            } else if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                if matches!(ext, "h" | "hpp" | "hh") {
                    has_header = true;
                }
            }
        }
        if has_header {
            let s = dir.to_string_lossy().into_owned();
            if !existing.contains(&s) {
                out.push(s);
            }
        }
    }
    out.sort();
    out
}

// ── Swift whole-module synthesis (Phase 3c) ──

/// `-swift-version` value from `SWIFT_VERSION`: strip a trailing `.0` (5.0→5)
/// but keep discrete minors like 4.2. `None` when unset.
pub fn swift_version_flag(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    Some(raw.strip_suffix(".0").unwrap_or(raw).to_string())
}

/// Target triple for the Swift path: arch from `NATIVE_ARCH` (fallback arm64),
/// OS/deployment from `PLATFORM_NAME` + the platform's deployment-target key.
/// (The clang path keeps its own inline `aarch64-apple-macos` triple unchanged,
/// since it is proven and its test pins that exact spelling.)
pub fn target_triple(settings: &BuildSettings) -> String {
    let arch = settings
        .get("NATIVE_ARCH")
        .filter(|s| !s.is_empty())
        .unwrap_or("arm64");
    let platform = settings.get("PLATFORM_NAME").unwrap_or("macosx");
    let (os, suffix, dep_key, dep_default) = match platform {
        "iphoneos" => ("ios", "", "IPHONEOS_DEPLOYMENT_TARGET", "17.0"),
        "iphonesimulator" => ("ios", "-simulator", "IPHONEOS_DEPLOYMENT_TARGET", "17.0"),
        "appletvos" => ("tvos", "", "TVOS_DEPLOYMENT_TARGET", "17.0"),
        "appletvsimulator" => ("tvos", "-simulator", "TVOS_DEPLOYMENT_TARGET", "17.0"),
        "watchos" => ("watchos", "", "WATCHOS_DEPLOYMENT_TARGET", "10.0"),
        "watchsimulator" => ("watchos", "-simulator", "WATCHOS_DEPLOYMENT_TARGET", "10.0"),
        _ => ("macos", "", "MACOSX_DEPLOYMENT_TARGET", "11.0"),
    };
    let dep = settings.get(dep_key).filter(|s| !s.is_empty()).unwrap_or(dep_default);
    format!("{arch}-apple-{os}{dep}{suffix}")
}

/// Synthesize whole-module swiftc arguments. Identical for every `.swift` file
/// in the target, so the caller computes it once. Does NOT include an action
/// verb (`-typecheck` etc.) — SourceKit-LSP drives its own, mirroring the clang
/// path which omits `-fsyntax-only`. `xcc_include_dirs` is the cached source-tree
/// augmentation (same set as the ObjC path). File list goes last.
pub fn synthesize_swift_args(
    settings: &BuildSettings,
    module_swift_files: &[PathBuf],
    srcroot: &Path,
    xcc_include_dirs: &[String],
    module_cache_dir: &Path,
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    if let Some(m) = settings.get("PRODUCT_MODULE_NAME").filter(|s| !s.is_empty()) {
        args.push("-module-name".into());
        args.push(m.to_string());
    }
    if let Some(sdk) = settings.get("SDKROOT").filter(|s| !s.is_empty()) {
        args.push("-sdk".into());
        args.push(sdk.to_string());
    }
    args.push("-target".into());
    args.push(target_triple(settings));

    if let Some(v) = swift_version_flag(settings.get("SWIFT_VERSION").unwrap_or("")) {
        args.push("-swift-version".into());
        args.push(v);
    }

    // Module/PCH cache lives outside the project tree.
    args.push("-module-cache-path".into());
    args.push(module_cache_dir.to_string_lossy().into_owned());

    // Bridging header (resolve relative to srcroot).
    if let Some(bridge) = settings
        .get("SWIFT_OBJC_BRIDGING_HEADER")
        .filter(|s| !s.is_empty())
    {
        let p = Path::new(bridge);
        let abs = if p.is_absolute() { p.to_path_buf() } else { srcroot.join(p) };
        args.push("-import-objc-header".into());
        args.push(abs.to_string_lossy().into_owned());
    }

    // Frameworks.
    for p in split_multi(settings.get("FRAMEWORK_SEARCH_PATHS").unwrap_or("")) {
        args.push("-F".into());
        args.push(p);
    }

    // Clang-importer include paths: HEADER_SEARCH_PATHS + augmented source-tree
    // dirs, each passed to the embedded clang as a `-Xcc -I<dir>` pair.
    for p in split_multi(settings.get("HEADER_SEARCH_PATHS").unwrap_or("")) {
        args.push("-Xcc".into());
        args.push(format!("-I{p}"));
    }
    for dir in xcc_include_dirs {
        args.push("-Xcc".into());
        args.push(format!("-I{dir}"));
    }

    // Preprocessor defs → clang (-Xcc -D); Swift conditions → swift -D.
    for d in split_multi(settings.get("GCC_PREPROCESSOR_DEFINITIONS").unwrap_or("")) {
        args.push("-Xcc".into());
        args.push(format!("-D{d}"));
    }
    for c in split_multi(settings.get("SWIFT_ACTIVE_COMPILATION_CONDITIONS").unwrap_or("")) {
        args.push(format!("-D{c}"));
    }

    // Passthrough OTHER_SWIFT_FLAGS.
    for f in split_multi(settings.get("OTHER_SWIFT_FLAGS").unwrap_or("")) {
        args.push(f);
    }

    // Whole-module file list last.
    for f in module_swift_files {
        args.push(f.to_string_lossy().into_owned());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_settings::parse_build_settings_for_target;
    use std::path::PathBuf;

    fn fixture_settings() -> BuildSettings {
        // HEADER_SEARCH_PATHS has a double space + a duplicate (real anomalies).
        let json = r#"[
            { "action": "build", "target": "App", "buildSettings": {
                "SDKROOT": "/SDKs/MacOSX.sdk",
                "MACOSX_DEPLOYMENT_TARGET": "11.0",
                "HEADER_SEARCH_PATHS": "/a/inc  /a/inc /b/inc",
                "FRAMEWORK_SEARCH_PATHS": "/F ",
                "GCC_PREPROCESSOR_DEFINITIONS": "DEBUG=1 ",
                "CLANG_ENABLE_MODULES": "YES",
                "CLANG_ENABLE_OBJC_ARC": "YES",
                "GCC_C_LANGUAGE_STANDARD": "gnu17",
                "CLANG_CXX_LANGUAGE_STANDARD": "gnu++20"
            } }
        ]"#;
        parse_build_settings_for_target(json, None).unwrap()
    }

    fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.iter().position(|a| a == flag).map(|i| args[i + 1].as_str())
    }

    #[test]
    fn objc_file_gets_expected_core_flags() {
        let bs = fixture_settings();
        let args = synthesize_clang_args(&bs, Path::new("/proj/App/A.m"), &[]);

        assert_eq!(flag_value(&args, "-x"), Some("objective-c"));
        assert!(args.contains(&"-std=gnu17".to_string()));
        assert!(args.contains(&"-fobjc-arc".to_string()));
        assert!(args.contains(&"-fmodules".to_string()));
        assert_eq!(flag_value(&args, "-isysroot"), Some("/SDKs/MacOSX.sdk"));
        assert_eq!(flag_value(&args, "-target"), Some("aarch64-apple-macos11.0"));
        assert!(args.contains(&"-DDEBUG=1".to_string()));
        // File path is last.
        assert_eq!(args.last().map(String::as_str), Some("/proj/App/A.m"));
    }

    #[test]
    fn header_search_paths_are_deduped() {
        let bs = fixture_settings();
        let args = synthesize_clang_args(&bs, Path::new("/proj/A.m"), &[]);
        let dup_count = args
            .windows(2)
            .filter(|w| w[0] == "-I" && w[1] == "/a/inc")
            .count();
        assert_eq!(dup_count, 1, "duplicated HSP entry must collapse to one -I");
    }

    #[test]
    fn cpp_file_uses_cxx_language_and_standard() {
        let bs = fixture_settings();
        let args = synthesize_clang_args(&bs, Path::new("/proj/A.mm"), &[]);
        assert_eq!(flag_value(&args, "-x"), Some("objective-c++"));
        assert!(args.contains(&"-std=gnu++20".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("/proj/A.mm"));
    }

    #[test]
    fn augmentation_scans_real_tree() {
        // Build a tiny tree: <tmp>/inc/foo.h → the "inc" dir must appear as -I.
        let tmp = std::env::temp_dir().join(format!("xcode-bsp-aug-{}", std::process::id()));
        let inc = tmp.join("inc");
        std::fs::create_dir_all(&inc).unwrap();
        std::fs::write(inc.join("foo.h"), b"// header").unwrap();

        let bs = fixture_settings();
        let aug = scoped_include_dirs(std::slice::from_ref(&tmp), &HashSet::new());
        let args = synthesize_clang_args(&bs, &PathBuf::from("/proj/A.m"), &aug);
        let inc_str = inc.to_string_lossy().into_owned();
        let found = args.windows(2).any(|w| w[0] == "-I" && w[1] == inc_str);

        std::fs::remove_dir_all(&tmp).ok();
        assert!(found, "source-tree header dir should be augmented as -I");
    }

    #[test]
    fn scoped_include_dirs_excludes_build_outputs() {
        let tmp = std::env::temp_dir().join(format!("xcode-bsp-scope-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::write(tmp.join("src/a.h"), b"//").unwrap();
        std::fs::create_dir_all(tmp.join("Build/gen")).unwrap();
        std::fs::write(tmp.join("Build/gen/b.h"), b"//").unwrap();
        std::fs::create_dir_all(tmp.join("DerivedData/x")).unwrap();
        std::fs::write(tmp.join("DerivedData/x/c.h"), b"//").unwrap();

        let dirs = scoped_include_dirs(std::slice::from_ref(&tmp), &HashSet::new());
        let has = |suffix: &str| dirs.iter().any(|d| d.ends_with(suffix));
        std::fs::remove_dir_all(&tmp).ok();

        assert!(has("/src"), "real source header dir must be included");
        assert!(!dirs.iter().any(|d| d.contains("/Build/")), "Build/ excluded");
        assert!(!dirs.iter().any(|d| d.contains("/DerivedData/")), "DerivedData/ excluded");
    }

    // ── Swift synthesis ──

    #[test]
    fn swift_version_flag_strip_rule() {
        assert_eq!(swift_version_flag("5.0"), Some("5".to_string()));
        assert_eq!(swift_version_flag("4.2"), Some("4.2".to_string()));
        assert_eq!(swift_version_flag("5"), Some("5".to_string()));
        assert_eq!(swift_version_flag("6"), Some("6".to_string()));
        assert_eq!(swift_version_flag(""), None);
        assert_eq!(swift_version_flag("  "), None);
    }

    fn settings_from(json: &str) -> BuildSettings {
        parse_build_settings_for_target(json, None).unwrap()
    }

    #[test]
    fn target_triple_macos_and_ios() {
        let macos = settings_from(
            r#"[{"buildSettings":{"PLATFORM_NAME":"macosx","MACOSX_DEPLOYMENT_TARGET":"10.15","NATIVE_ARCH":"arm64"}}]"#,
        );
        assert_eq!(target_triple(&macos), "arm64-apple-macos10.15");

        let ios = settings_from(
            r#"[{"buildSettings":{"PLATFORM_NAME":"iphoneos","IPHONEOS_DEPLOYMENT_TARGET":"17.0","NATIVE_ARCH":"arm64"}}]"#,
        );
        assert_eq!(target_triple(&ios), "arm64-apple-ios17.0");
    }

    fn swift_fixture_settings() -> BuildSettings {
        settings_from(
            r#"[{ "buildSettings": {
                "PRODUCT_MODULE_NAME": "MyApp",
                "SDKROOT": "/SDKs/MacOSX.sdk",
                "PLATFORM_NAME": "macosx",
                "MACOSX_DEPLOYMENT_TARGET": "12.0",
                "NATIVE_ARCH": "arm64",
                "SWIFT_VERSION": "5.0",
                "SWIFT_OBJC_BRIDGING_HEADER": "MyApp/Bridge.h",
                "HEADER_SEARCH_PATHS": "/hsp/one"
            } }]"#,
        )
    }

    #[test]
    fn swift_args_have_whole_module_recipe() {
        let bs = swift_fixture_settings();
        let files = vec![
            PathBuf::from("/proj/App/A.swift"),
            PathBuf::from("/proj/App/B.swift"),
        ];
        let xcc = vec!["/aug/dir".to_string()];
        let cache = Path::new("/tmp/modcache-xyz");
        let args = synthesize_swift_args(&bs, &files, Path::new("/proj"), &xcc, cache);

        let pos = |flag: &str| args.iter().position(|a| a == flag);
        let val = |flag: &str| pos(flag).map(|i| args[i + 1].as_str());

        assert_eq!(val("-module-name"), Some("MyApp"));
        assert_eq!(val("-sdk"), Some("/SDKs/MacOSX.sdk"));
        assert_eq!(val("-target"), Some("arm64-apple-macos12.0"));
        assert_eq!(val("-swift-version"), Some("5"));
        assert_eq!(val("-module-cache-path"), Some("/tmp/modcache-xyz"));
        // bridging header resolved relative to srcroot
        assert_eq!(val("-import-objc-header"), Some("/proj/MyApp/Bridge.h"));

        // -Xcc -I<dir> PAIRS (two separate argv entries each).
        assert!(args.windows(2).any(|w| w[0] == "-Xcc" && w[1] == "-I/hsp/one"));
        assert!(args.windows(2).any(|w| w[0] == "-Xcc" && w[1] == "-I/aug/dir"));

        // all module files present, and last.
        assert!(args.contains(&"/proj/App/A.swift".to_string()));
        assert!(args.contains(&"/proj/App/B.swift".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("/proj/App/B.swift"));
    }

    #[test]
    fn swift_args_omit_bridging_when_absent() {
        let bs = settings_from(
            r#"[{"buildSettings":{"PRODUCT_MODULE_NAME":"M","SDKROOT":"/S","SWIFT_VERSION":"5.0"}}]"#,
        );
        let args = synthesize_swift_args(
            &bs,
            &[PathBuf::from("/p/A.swift")],
            Path::new("/p"),
            &[],
            Path::new("/tmp/mc"),
        );
        assert!(!args.iter().any(|a| a == "-import-objc-header"));
    }
}
