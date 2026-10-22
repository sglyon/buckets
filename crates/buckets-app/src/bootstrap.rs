//! Materialize trusted bundled plugins while preserving user edits and settings.
use anyhow::{Context, Result};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

const NATIVE_SOURCE: &str = include_str!("../../../examples/plugins/native-tools/plugin.py");
const LEGACY_NATIVE_SOURCE: &str =
    include_str!("../../../examples/plugins/native-tools/legacy-v1.py");
const SNAPSHOT: &str = ".buckets-managed-source.py";

pub fn ensure_bundled_plugins(root: &Path) -> Result<Vec<String>> {
    let mut notices = vec![];
    for (id, manifest, source) in [
        (
            "git-status",
            include_str!("../../../examples/plugins/git-status/plugin.json"),
            include_str!("../../../examples/plugins/git-status/plugin.py"),
        ),
        (
            "native-tools",
            include_str!("../../../examples/plugins/native-tools/plugin.json"),
            NATIVE_SOURCE,
        ),
    ] {
        let directory = root.join(id);
        if directory.exists() {
            if id == "native-tools" {
                migrate_native(&directory, &mut notices)?;
            }
            add_missing_settings_fields(&directory, manifest, source)?;
            continue;
        }
        fs::create_dir_all(&directory).with_context(|| format!("Create bundled plugin {id}"))?;
        let mut manifest: serde_json::Value = serde_json::from_str(manifest)?;
        manifest["permissions"]["read_roots"] = serde_json::json!(["/"]);
        write_new(&directory.join("plugin.py"), source.as_bytes())?;
        write_new(&directory.join(SNAPSHOT), source.as_bytes())?;
        write_new(
            &directory.join("plugin.json"),
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
    }
    Ok(notices)
}
// Schemas describe unchanged first-party code. Never graft first-party setting
// semantics onto a user-edited executable or replace custom schema metadata.
fn add_missing_settings_fields(
    directory: &Path,
    bundled_manifest: &str,
    source: &str,
) -> Result<()> {
    if fs::read_to_string(directory.join("plugin.py"))
        .ok()
        .as_deref()
        != Some(source)
    {
        return Ok(());
    }
    let path = directory.join("plugin.json");
    let Ok(original) = fs::read(&path) else {
        return Ok(());
    };
    let Ok(mut manifest) = serde_json::from_slice::<serde_json::Value>(&original) else {
        return Ok(());
    };
    let Some(object) = manifest.as_object_mut() else {
        return Ok(());
    };
    if object.contains_key("settings_fields") {
        return Ok(());
    }
    let bundled: serde_json::Value = serde_json::from_str(bundled_manifest)?;
    let Some(fields) = bundled.get("settings_fields") else {
        return Ok(());
    };
    object.insert("settings_fields".into(), fields.clone());
    if fs::read(&path)? != original {
        return Ok(());
    }
    replace_atomic(&path, &serde_json::to_vec_pretty(&manifest)?)
}
fn migrate_native(directory: &Path, notices: &mut Vec<String>) -> Result<()> {
    let source_path = directory.join("plugin.py");
    let Ok(source) = fs::read_to_string(&source_path) else {
        return Ok(());
    };
    if source == NATIVE_SOURCE {
        return Ok(());
    }
    let prior = fs::read_to_string(directory.join(SNAPSHOT)).ok();
    if source != LEGACY_NATIVE_SOURCE && prior.as_deref() != Some(source.as_str()) {
        notices.push("Developer tools plugin was edited; embedded-terminal upgrade preserved your files. Reinstall this bundled plugin to upgrade.".into());
        return Ok(());
    }
    let manifest_path = directory.join("plugin.json");
    let Ok(mut manifest) = fs::read(&manifest_path)
        .ok()
        .and_then(|v| serde_json::from_slice::<serde_json::Value>(&v).ok())
        .ok_or(())
    else {
        return Ok(());
    };
    let valid = manifest.as_object().is_some()
        && manifest.get("capabilities").is_some_and(|v| v.is_array())
        && manifest.get("permissions").is_some_and(|v| v.is_object())
        && manifest.get("config").is_none_or(|v| v.is_object());
    if !valid {
        notices.push("Developer tools upgrade preserved an incompatible manifest; check its capabilities, permissions and config objects.".into());
        return Ok(());
    }
    let capabilities = manifest["capabilities"].as_array_mut().unwrap();
    if !capabilities.iter().any(|value| value == "terminal") {
        capabilities.push(serde_json::json!("terminal"));
    }
    manifest["permissions"]["terminal"] = true.into();
    if manifest.get("config").is_none() {
        manifest["config"] = serde_json::json!({});
    }
    if manifest["config"].get("terminal_target").is_none() {
        manifest["config"]["terminal_target"] = "embedded".into();
    }
    // Each replacement is atomic. An interrupted multi-file upgrade is retryable.
    replace_atomic(&manifest_path, &serde_json::to_vec_pretty(&manifest)?)?;
    replace_atomic(&source_path, NATIVE_SOURCE.as_bytes())?;
    replace_atomic(&directory.join(SNAPSHOT), NATIVE_SOURCE.as_bytes())?;
    Ok(())
}
fn replace_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().context("Plugin file has no parent")?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let temporary = parent.join(format!(".buckets-update-{}-{stamp}", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
fn write_new(path: &Path, contents: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(contents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn adds_missing_schema_to_unchanged_sources_and_preserves_user_config_or_schema() {
        let root = tempfile::tempdir().unwrap();
        super::ensure_bundled_plugins(root.path()).unwrap();
        for id in ["native-tools", "git-status"] {
            let path = root.path().join(id).join("plugin.json");
            let mut manifest: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            manifest.as_object_mut().unwrap().remove("settings_fields");
            manifest["config"]["custom_setting"] = serde_json::json!({"preserve":true});
            std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
            super::ensure_bundled_plugins(root.path()).unwrap();
            let updated: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert!(!updated["settings_fields"].as_array().unwrap().is_empty());
            assert_eq!(updated["config"], manifest["config"]);
            let mut custom = updated;
            custom["settings_fields"] = serde_json::json!([{ "custom_metadata":true }]);
            let bytes = serde_json::to_vec(&custom).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            super::ensure_bundled_plugins(root.path()).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
    #[test]
    fn schema_upgrade_does_not_modify_user_edited_source_or_manifest() {
        let root = tempfile::tempdir().unwrap();
        super::ensure_bundled_plugins(root.path()).unwrap();
        let dir = root.path().join("git-status");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("plugin.json")).unwrap()).unwrap();
        manifest.as_object_mut().unwrap().remove("settings_fields");
        let bytes = serde_json::to_vec(&manifest).unwrap();
        std::fs::write(dir.join("plugin.json"), &bytes).unwrap();
        std::fs::write(dir.join("plugin.py"), "# user edited source").unwrap();
        super::ensure_bundled_plugins(root.path()).unwrap();
        assert_eq!(std::fs::read(dir.join("plugin.json")).unwrap(), bytes);
    }
    #[test]
    fn installs_once_and_preserves_user_configuration() {
        let root = tempfile::tempdir().unwrap();
        super::ensure_bundled_plugins(root.path()).unwrap();
        let manifest = root.path().join("native-tools/plugin.json");
        let first = std::fs::read_to_string(&manifest).unwrap();
        assert!(first.contains("com.googlecode.iterm2"));
        std::fs::write(&manifest, "user-owned configuration").unwrap();
        super::ensure_bundled_plugins(root.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(manifest).unwrap(),
            "user-owned configuration"
        );
    }
    #[test]
    fn upgrades_exact_legacy_source_and_preserves_configuration() {
        let root = tempfile::tempdir().unwrap();
        super::ensure_bundled_plugins(root.path()).unwrap();
        let dir = root.path().join("native-tools");
        std::fs::write(dir.join("plugin.py"), super::LEGACY_NATIVE_SOURCE).unwrap();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("plugin.json")).unwrap()).unwrap();
        manifest["config"]["nvim"] = "/custom/nvim".into();
        manifest["capabilities"] = serde_json::json!(["commands"]);
        manifest["permissions"] = serde_json::json!({"read_roots":["/custom"],"network":false});
        manifest["config"]
            .as_object_mut()
            .unwrap()
            .remove("terminal_target");
        std::fs::write(
            dir.join("plugin.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(
            super::ensure_bundled_plugins(root.path())
                .unwrap()
                .is_empty()
        );
        let migrated: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("plugin.json")).unwrap()).unwrap();
        assert_eq!(migrated["config"]["nvim"], "/custom/nvim");
        assert_eq!(
            migrated["permissions"]["read_roots"],
            serde_json::json!(["/custom"])
        );
        assert_eq!(migrated["permissions"]["terminal"], true);
        assert!(
            migrated["capabilities"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("terminal"))
        );
        assert_eq!(migrated["config"]["terminal_target"], "embedded");
        assert_eq!(
            std::fs::read_to_string(dir.join("plugin.py")).unwrap(),
            super::NATIVE_SOURCE
        );
    }
    #[test]
    fn preserves_explicit_external_target_and_malformed_configuration() {
        let root = tempfile::tempdir().unwrap();
        super::ensure_bundled_plugins(root.path()).unwrap();
        let dir = root.path().join("native-tools");
        std::fs::write(dir.join("plugin.py"), super::LEGACY_NATIVE_SOURCE).unwrap();
        let manifest = serde_json::json!({"capabilities":["commands"],"permissions":{"read_roots":["/fixture"]},"config":{"terminal_target":"iterm2","terminal_bundle_id":"custom.id"}});
        std::fs::write(
            dir.join("plugin.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        super::ensure_bundled_plugins(root.path()).unwrap();
        let upgraded: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("plugin.json")).unwrap()).unwrap();
        assert_eq!(upgraded["config"], manifest["config"]);
        for malformed in [
            serde_json::json!({"capabilities":[],"permissions":false,"config":{}}),
            serde_json::json!({"capabilities":[],"permissions":{},"config":"custom"}),
        ] {
            std::fs::write(dir.join("plugin.py"), super::LEGACY_NATIVE_SOURCE).unwrap();
            let bytes = serde_json::to_vec(&malformed).unwrap();
            std::fs::write(dir.join("plugin.json"), &bytes).unwrap();
            assert_eq!(super::ensure_bundled_plugins(root.path()).unwrap().len(), 1);
            assert_eq!(std::fs::read(dir.join("plugin.json")).unwrap(), bytes);
            assert_eq!(
                std::fs::read_to_string(dir.join("plugin.py")).unwrap(),
                super::LEGACY_NATIVE_SOURCE
            );
        }
    }
    #[test]
    fn edited_source_is_preserved_and_reported() {
        let root = tempfile::tempdir().unwrap();
        super::ensure_bundled_plugins(root.path()).unwrap();
        let source = root.path().join("native-tools/plugin.py");
        std::fs::write(&source, "# custom script").unwrap();
        assert_eq!(super::ensure_bundled_plugins(root.path()).unwrap().len(), 1);
        assert_eq!(std::fs::read_to_string(source).unwrap(), "# custom script");
    }
}
