//! `xcodebuild -showBuildSettings -json` invocation + pure parsing.
//!
//! See design_project_recognition_bsp.md §6.2 (setting → flag mapping) and
//! §11.3 Q4 (the whitespace-split / dedupe rule confirmed by the Phase 2 spike).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::runner;

/// Bound on the `xcodebuild -showBuildSettings` wait. A first-run SPM
/// resolution / license / network stall must not hang the single-threaded
/// session forever; on timeout we kill the child and degrade to no settings.
const SHOW_BUILD_SETTINGS_TIMEOUT_SECS: u64 = 90;

/// Parsed build settings for a single target (flat key → value).
pub struct BuildSettings {
    map: serde_json::Map<String, Value>,
}

impl BuildSettings {
    /// String value for `key`, if present and a string.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).and_then(Value::as_str)
    }

    /// Xcode boolean setting: true only when the value is exactly "YES".
    pub fn bool_flag(&self, key: &str) -> bool {
        self.get(key) == Some("YES")
    }
}

// ── Thin subprocess wrapper (kept separate from parsing for testability) ──

/// Run `xcodebuild <args>` with a bounded wait, returning stdout (see
/// `runner::run_bounded`).
pub fn run_xcodebuild(args: &[String]) -> Result<String, String> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    runner::run_bounded("xcodebuild", &refs, Duration::from_secs(SHOW_BUILD_SETTINGS_TIMEOUT_SECS))
}

// ── Pure parsing ──

/// Parse `xcodebuild -list -json` output → the list of scheme names.
pub fn parse_scheme_list(json: &str) -> Vec<String> {
    let v: Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    // Shape: { "workspace"|"project": { "schemes": [..] } }
    for container in ["workspace", "project"] {
        if let Some(schemes) = v
            .get(container)
            .and_then(|c| c.get("schemes"))
            .and_then(Value::as_array)
        {
            return schemes.iter().filter_map(Value::as_str).map(str::to_string).collect();
        }
    }
    Vec::new()
}

/// Parse the `[{action, buildSettings, target}]` array. When `target` is given,
/// return the entry whose `"target"` matches; otherwise (or if no match) the
/// first entry.
pub fn parse_build_settings_for_target(
    json: &str,
    target: Option<&str>,
) -> Result<BuildSettings, String> {
    let entries: Value =
        serde_json::from_str(json).map_err(|e| format!("parse -showBuildSettings JSON: {e}"))?;
    let arr = entries
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| "empty -showBuildSettings array".to_string())?;
    let entry = target
        .and_then(|name| {
            arr.iter()
                .find(|e| e.get("target").and_then(Value::as_str) == Some(name))
        })
        .unwrap_or(&arr[0]);
    let map = entry
        .get("buildSettings")
        .and_then(Value::as_object)
        .ok_or_else(|| "missing buildSettings object".to_string())?
        .clone();
    Ok(BuildSettings { map })
}

/// Q4 rule (confirmed by the Phase 2 spike): split a space-joined build-setting
/// value on whitespace runs, drop empty tokens, order-preserving dedupe.
pub fn split_multi(value: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for tok in value.split_whitespace() {
        if seen.insert(tok) {
            out.push(tok.to_string());
        }
    }
    out
}

/// Derive the Xcode index-store path from build settings, for BSP index
/// advertisement (bsp_protocol_spec.md §1). Returns `Some` only if the store
/// exists on disk. `BUILD_ROOT`/`SYMROOT`/`BUILD_DIR` look like
/// `<DerivedData>/<Proj>-<hash>/Build/Products`; the store lives at
/// `<DerivedData>/<Proj>-<hash>/Index.noindex/DataStore`.
pub fn derive_index_store_path(settings: &BuildSettings) -> Option<PathBuf> {
    let build_root = settings
        .get("BUILD_ROOT")
        .or_else(|| settings.get("SYMROOT"))
        .or_else(|| settings.get("BUILD_DIR"))?;
    let index = index_store_from_build_root(Path::new(build_root))?;
    index.is_dir().then_some(index)
}

/// Pure: map a BUILD_ROOT-like path to `<DerivedData root>/Index.noindex/DataStore`
/// by finding the `Build` ancestor. Does not touch the filesystem.
fn index_store_from_build_root(build_root: &Path) -> Option<PathBuf> {
    let mut cur = Some(build_root);
    while let Some(p) = cur {
        if p.file_name().is_some_and(|n| n == "Build") {
            return p.parent().map(|dd| dd.join("Index.noindex").join("DataStore"));
        }
        cur = p.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_multi_handles_whitespace_anomalies_and_dedupe() {
        // Double space, trailing space, and a duplicated entry (all real HSP
        // anomalies from the PAEScreenProvider fixture).
        assert_eq!(split_multi("/a/inc  /b/inc "), vec!["/a/inc", "/b/inc"]);
        assert_eq!(split_multi(" x y x z y "), vec!["x", "y", "z"]);
        assert_eq!(split_multi(""), Vec::<String>::new());
        assert_eq!(split_multi("   "), Vec::<String>::new());
    }

    #[test]
    fn parse_build_settings_extracts_target_settings() {
        let json = r#"[
            { "action": "build", "target": "App", "buildSettings": {
                "SDKROOT": "/SDKs/MacOSX.sdk",
                "CLANG_ENABLE_OBJC_ARC": "YES",
                "CLANG_ENABLE_MODULES": "NO",
                "GCC_C_LANGUAGE_STANDARD": "gnu17"
            } }
        ]"#;
        let bs = parse_build_settings_for_target(json, None).unwrap();
        assert_eq!(bs.get("SDKROOT"), Some("/SDKs/MacOSX.sdk"));
        assert!(bs.bool_flag("CLANG_ENABLE_OBJC_ARC"));
        assert!(!bs.bool_flag("CLANG_ENABLE_MODULES"));
        assert_eq!(bs.get("GCC_C_LANGUAGE_STANDARD"), Some("gnu17"));
        assert_eq!(bs.get("MISSING"), None);
    }

    #[test]
    fn index_store_strips_build_root_to_derived_data() {
        let br = Path::new(
            "/Users/me/Library/Developer/Xcode/DerivedData/Proj-abc123/Build/Products",
        );
        assert_eq!(
            index_store_from_build_root(br),
            Some(PathBuf::from(
                "/Users/me/Library/Developer/Xcode/DerivedData/Proj-abc123/Index.noindex/DataStore"
            ))
        );
        // No "Build" ancestor → None.
        assert_eq!(index_store_from_build_root(Path::new("/tmp/nope")), None);
    }
}
