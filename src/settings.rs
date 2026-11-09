//! Settings saved in %APPDATA%\MarkSync\settings.json.

use crate::fsutil::KindFilter;
use crate::organize::{OrgMode, Scheme};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum LayoutChoice {
    #[default]
    Keep,
    Flat,
    Organized,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum DupAction {
    #[default]
    RecycleBin,
    MoveToFolder,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceCfg {
    pub root: PathBuf,
    /// Folders explicitly ticked (true) or unticked (false).
    pub states: Vec<(PathBuf, bool)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub lang: Option<String>,
    pub installer_lang: Option<String>,
    pub theme: Theme,
    pub skip_hidden: bool,
    pub sources: Vec<SourceCfg>,
    pub phone_sources: Vec<crate::phone::PhoneSource>,
    pub target: Option<PathBuf>,
    pub kinds: KindFilter,
    pub layout: LayoutChoice,
    pub layout_scheme: Scheme,
    pub org_root: Option<PathBuf>,
    pub org_dest: Option<PathBuf>,
    pub org_scheme: Scheme,
    pub org_mode: OrgMode,
    pub org_kinds: KindFilter,
    pub org_keep_albums: bool,
    pub org_flag_dates: bool,
    pub org_remove_dups: bool,
    pub dup_roots: Vec<PathBuf>,
    pub dup_kinds: KindFilter,
    pub dup_action: DupAction,
    pub last_undo: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            lang: None,
            installer_lang: None,
            theme: Theme::Dark,
            skip_hidden: true,
            sources: Vec::new(),
            phone_sources: Vec::new(),
            target: None,
            kinds: KindFilter::default(),
            layout: LayoutChoice::Keep,
            layout_scheme: Scheme::YearMonth,
            org_root: None,
            org_dest: None,
            org_scheme: Scheme::YearMonth,
            org_mode: OrgMode::MoveInPlace,
            org_kinds: KindFilter::default(),
            org_keep_albums: true,
            org_flag_dates: true,
            org_remove_dups: true,
            dup_roots: Vec::new(),
            dup_kinds: KindFilter::default(),
            dup_action: DupAction::RecycleBin,
            last_undo: None,
        }
    }
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir());
    base.join("MarkSync")
}

fn file() -> PathBuf {
    data_dir().join("settings.json")
}

impl Settings {
    pub fn org_options(&self) -> crate::organize::OrgOptions {
        crate::organize::OrgOptions {
            keep_albums: self.org_keep_albums,
            flag_dates: self.org_flag_dates,
            remove_dups: self.org_remove_dups,
        }
    }

    pub fn load() -> Self {
        let mut s: Settings = std::fs::read_to_string(file())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        // a language picked in a (re)installation wins once over the saved one
        let reg = installer_language();
        if reg.is_some() && reg != s.installer_lang {
            s.lang = reg.clone();
            s.installer_lang = reg;
            s.save();
        }
        s
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(data_dir());
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let tmp = file().with_extension("tmp");
            if std::fs::write(&tmp, t).is_ok() {
                let _ = std::fs::rename(tmp, file());
            }
        }
    }
}

/// The language picked in the installer (HKCU\Software\MarkSync\Language).
fn installer_language() -> Option<String> {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\MarkSync")
        .ok()?
        .get_value::<String, _>("Language")
        .ok()
}
