use crate::hotkey::Shortcut;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub menu_hotkey: String,
    pub middle_click: bool,
    /// Stable tool ID -> global shortcut. Empty by default.
    pub tool_hotkeys: BTreeMap<String, String>,
    /// Disabled IDs retain their saved shortcut but cannot run from any GUI entry.
    pub disabled_tools: BTreeSet<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            menu_hotkey: "Ctrl+Alt+Q".into(),
            middle_click: false,
            tool_hotkeys: BTreeMap::new(),
            disabled_tools: BTreeSet::new(),
        }
    }
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        if let Some(path) = std::env::var_os("TRAYKIT_CONFIG") {
            return Ok(path.into());
        }
        Ok(
            PathBuf::from(std::env::var_os("APPDATA").context("APPDATA is unavailable")?)
                .join("TrayKit/config.toml"),
        )
    }

    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let config: Self = toml::from_str(&text)
                    .with_context(|| format!("Invalid config: {}", path.display()))?;
                config.validate()?;
                Ok(config)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("Cannot read {}", path.display())),
        }
    }

    pub fn validate(&self) -> Result<()> {
        let known = crate::tools::registered();
        for id in &self.disabled_tools {
            ensure!(
                known.iter().any(|tool| tool.id() == id),
                "Unknown disabled tool ID: {id}"
            );
        }
        let mut bindings = vec![self.menu_hotkey.parse::<Shortcut>()?];
        for (id, value) in &self.tool_hotkeys {
            ensure!(
                known.iter().any(|tool| tool.id() == id),
                "Unknown tool ID: {id}"
            );
            let binding = value.parse::<Shortcut>()?;
            if self.is_enabled(id) {
                ensure!(
                    !bindings.contains(&binding),
                    "Duplicate active global shortcut: {value}"
                );
                bindings.push(binding);
            }
        }
        Ok(())
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        !self.disabled_tools.contains(id)
    }

    // Called only by the explicit Save action, never by image conversion or startup.
    pub fn save(&self) -> Result<()> {
        self.validate()?;
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        use std::{io::Write, os::windows::ffi::OsStrExt};
        use windows::{
            Win32::Storage::FileSystem::{
                MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
            },
            core::PCWSTR,
        };
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        let result = (|| -> Result<()> {
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(toml::to_string_pretty(self)?.as_bytes())?;
            file.sync_all()?;
            drop(file);
            let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            unsafe {
                MoveFileExW(
                    PCWSTR(from.as_ptr()),
                    PCWSTR(to.as_ptr()),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            }?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result.with_context(|| format!("Cannot atomically save {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_roundtrip_and_unknown_keys_fail() {
        let config = Config::default();
        assert_eq!(
            toml::from_str::<Config>(&toml::to_string(&config).unwrap()).unwrap(),
            config
        );
        assert!(toml::from_str::<Config>("typo = true").is_err());
        assert_eq!(toml::from_str::<Config>("").unwrap(), config);
    }
    #[test]
    fn disabled_bindings_are_dormant_but_reenable_checks_conflicts() {
        let mut config = Config::default();
        config.disabled_tools.insert("text-clean".into());
        config
            .tool_hotkeys
            .insert("text-clean".into(), config.menu_hotkey.clone());
        config.validate().unwrap();
        config.disabled_tools.clear();
        assert!(config.validate().is_err());
    }

    #[test]
    fn legacy_config_and_tool_enablement_roundtrip() {
        let legacy = "menu_hotkey = \"Ctrl+Alt+Q\"\nmiddle_click = false\n[tool_hotkeys]\nclipboard-image = \"Ctrl+Alt+F11\"\n";
        let mut config: Config = toml::from_str(legacy).unwrap();
        config.validate().unwrap();
        assert!(config.is_enabled("clipboard-image"));
        config.disabled_tools.insert("clipboard-image".into());
        config.disabled_tools.insert("text-clean".into());
        config.validate().unwrap();
        assert!(!config.is_enabled("clipboard-image"));
        assert_eq!(config.tool_hotkeys["clipboard-image"], "Ctrl+Alt+F11");
        assert_eq!(
            toml::from_str::<Config>(&toml::to_string(&config).unwrap()).unwrap(),
            config
        );
        config.disabled_tools.insert("nonexistent".into());
        assert!(config.validate().is_err());
    }

    #[test]
    fn duplicate_or_unknown_tool_bindings_fail() {
        let mut config = Config::default();
        config
            .tool_hotkeys
            .insert("clipboard-image".into(), "Alt+Ctrl+Q".into());
        assert!(config.validate().is_err());
        config.tool_hotkeys.clear();
        config.tool_hotkeys.insert("typo".into(), "Ctrl+F8".into());
        assert!(config.validate().is_err());
    }
}
