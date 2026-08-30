use crate::utils::ipc_types::SamError;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

#[cfg(all(test, target_os = "linux"))]
pub static TEST_INSTALL_ROOT: OnceLock<PathBuf> = OnceLock::new();

pub struct SteamLocator {
    lib_path: OnceLock<Option<PathBuf>>,
    user_game_stats_schema_prefix: OnceLock<Option<String>>,
    local_app_banner_file_prefix: OnceLock<Option<String>>,
}

impl SteamLocator {
    pub fn new() -> Self {
        Self {
            lib_path: OnceLock::new(),
            user_game_stats_schema_prefix: OnceLock::new(),
            local_app_banner_file_prefix: OnceLock::new(),
        }
    }

    pub fn global() -> &'static RwLock<SteamLocator> {
        static INSTANCE: OnceLock<RwLock<SteamLocator>> = OnceLock::new();
        INSTANCE.get_or_init(|| RwLock::new(SteamLocator::new()))
    }

    pub fn get_lib_path(&self, silent: bool) -> Option<PathBuf> {
        self.lib_path
            .get_or_init(|| Self::get_steamclient_lib_path(silent))
            .clone()
    }

    pub fn get_user_game_stats_schema(&self, app_id: &u32) -> Result<PathBuf, SamError> {
        let prefix = self
            .user_game_stats_schema_prefix
            .get_or_init(Self::get_user_game_stats_schema_prefix)
            .as_ref()
            .ok_or(SamError::UnknownError)?;
        Ok(PathBuf::from(format!("{}{}.bin", prefix, app_id)))
    }

    #[cfg(target_os = "linux")]
    fn install_root_holding(relative: &str) -> Option<PathBuf> {
        Self::get_local_steam_install_root_folders()
            .into_iter()
            .find(|root| root.join(relative).exists())
    }

    #[cfg(target_os = "windows")]
    fn steam_root_from_registry() -> Option<PathBuf> {
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;

        let subkey = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey("SOFTWARE\\Valve\\Steam")
            .ok()?;
        let steam_path: String = subkey.get_value("SteamPath").ok()?;
        Some(PathBuf::from(steam_path))
    }

    #[cfg(target_os = "windows")]
    fn install_root_holding(relative: &str) -> Option<PathBuf> {
        let root = Self::steam_root_from_registry()?;
        root.join(relative).exists().then_some(root)
    }

    fn find_in_install_roots(relative: &str) -> Option<PathBuf> {
        Some(Self::install_root_holding(relative)?.join(relative))
    }

    fn collections_relative(account_id: u32) -> String {
        format!("userdata/{account_id}/config/cloudstorage/cloud-storage-namespace-1.json")
    }

    /// Pinned to the install the collections came from, not whichever root holds
    /// the file first: a forgotten install's stale metadata is a wrong answer.
    fn in_collections_install(account_id: u32, relative: &str) -> Option<PathBuf> {
        let path =
            Self::install_root_holding(&Self::collections_relative(account_id))?.join(relative);
        path.exists().then_some(path)
    }

    pub fn get_local_config_path(account_id: u32) -> Option<PathBuf> {
        Self::find_in_install_roots(&format!("userdata/{account_id}/config/localconfig.vdf"))
    }

    pub fn get_collections_path(account_id: u32) -> Option<PathBuf> {
        Self::find_in_install_roots(&Self::collections_relative(account_id))
    }

    pub fn get_collections_local_config_path(account_id: u32) -> Option<PathBuf> {
        Self::in_collections_install(
            account_id,
            &format!("userdata/{account_id}/config/localconfig.vdf"),
        )
    }

    pub fn get_app_info_path(account_id: u32) -> Option<PathBuf> {
        Self::in_collections_install(account_id, "appcache/appinfo.vdf")
    }

    pub fn get_library_folders_path(account_id: u32) -> Option<PathBuf> {
        Self::in_collections_install(account_id, "steamapps/libraryfolders.vdf")
    }

    pub fn get_local_app_banner_file_prefix_cached(&self) -> Option<String> {
        self.local_app_banner_file_prefix
            .get_or_init(Self::get_local_app_banner_file_prefix)
            .clone()
    }

    #[cfg(target_os = "linux")]
    pub fn get_steamclient_lib_path(silent: bool) -> Option<PathBuf> {
        use std::path::Path;

        if let Ok(path_str) = std::env::var("SAM_STEAMCLIENT_PATH") {
            return Some(Path::new(&path_str).to_owned());
        }

        let steam_install_paths: Vec<PathBuf> = Self::get_local_steam_install_root_folders()
            .into_iter()
            .map(|path| path.join("linux64/steamclient.so"))
            .filter(|path| path.exists())
            .collect();

        let first_path = steam_install_paths.first()?;

        if !silent && steam_install_paths.len() > 1 {
            eprintln!("[STEAM LOCATOR] Found multiple Steam installations. Using the first one.");
            for path in &steam_install_paths {
                eprintln!("[STEAM LOCATOR] - {}", path.display());
            }
        }

        Some(first_path.clone())
    }

    #[cfg(target_os = "windows")]
    pub fn get_steamclient_lib_path(_silent: bool) -> Option<PathBuf> {
        use std::path::PathBuf;
        use winreg::RegKey;
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

        const REG_PATH: &str = "SOFTWARE\\Valve\\Steam";
        const VALUE_NAME: &str = "SteamPath";

        // Try HKEY_CURRENT_USER first
        if let Ok(subkey) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(REG_PATH) {
            if let Ok(value) = subkey.get_value::<String, _>(VALUE_NAME) {
                let path = PathBuf::from(value).join("steamclient64.dll");
                return Some(path);
            }
        }

        // Fallback to HKEY_LOCAL_MACHINE
        if let Ok(subkey) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(REG_PATH) {
            if let Ok(value) = subkey.get_value::<String, _>(VALUE_NAME) {
                let path = PathBuf::from(value).join("steamclient64.dll");
                return Some(path);
            }
        }

        None
    }

    #[cfg(target_os = "linux")]
    fn get_user_game_stats_schema_prefix() -> Option<String> {
        // Defers to the install-root resolution so SAM_* / SNAP_REAL_HOME /
        // default precedence stays in one place.
        let dirs = Self::get_local_steam_install_root_folders();

        if dirs.is_empty() {
            return None;
        }

        Some(dirs[0].to_str()?.to_owned() + "/appcache/stats/UserGameStatsSchema_")
    }

    #[cfg(target_os = "windows")]
    pub fn get_user_game_stats_schema_prefix() -> Option<String> {
        use winreg::{RegKey, enums::HKEY_CURRENT_USER};

        let steam_key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey("SOFTWARE\\Valve\\Steam")
            .ok()?;

        let steam_path: String = steam_key.get_value("SteamPath").ok()?;

        Some(steam_path + "/appcache/stats/UserGameStatsSchema_")
    }

    #[cfg(target_os = "linux")]
    pub fn get_local_steam_install_root_folders() -> Vec<PathBuf> {
        use std::path::PathBuf;

        // Explicit override wins over everything.
        if let Ok(path) = std::env::var("SAM_STEAM_INSTALL_ROOT") {
            return vec![PathBuf::from(path)];
        }

        #[cfg(test)]
        if let Some(root) = TEST_INSTALL_ROOT.get() {
            return vec![root.clone()];
        }

        // When SAM itself runs as a snap, the real home (with the user's Steam
        // installs) is exposed via SNAP_REAL_HOME rather than HOME.
        let home = std::env::var("SNAP_REAL_HOME")
            .or_else(|_| std::env::var("HOME"))
            .expect("Failed to get home dir");
        let home_path = PathBuf::from(home);

        // Flatpak first: it requires the PID-namespace join, so we prefer it when
        // present. The GUI surfaces a warning when more than one install is found.
        let potential_dirs = [
            home_path.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
            home_path.join("snap/steam/common/.local/share/Steam"),
            home_path.join(".local/share/Steam"),
            home_path.join(".steam/steam"),
            home_path.join(".steam/debian-installation"),
            home_path.join(".steam/root"),
        ];

        let mut seen = std::collections::HashSet::new();
        potential_dirs
            .into_iter()
            .filter(|path| path.is_dir())
            .filter(|path| {
                seen.insert(std::fs::canonicalize(path).unwrap_or_else(|_| path.clone()))
            })
            .collect()
    }

    #[cfg(target_os = "linux")]
    pub fn get_local_app_banner_file_prefix() -> Option<String> {
        let dirs = Self::get_local_steam_install_root_folders();

        if dirs.is_empty() {
            None
        } else {
            Some(dirs[0].to_str()?.to_owned() + "/appcache/librarycache/")
        }
    }

    #[cfg(target_os = "windows")]
    pub fn get_local_app_banner_file_prefix() -> Option<String> {
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;

        let subkey = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey("SOFTWARE\\Valve\\Steam")
            .ok()?;

        let value = subkey.get_value::<String, &'static str>("SteamPath").ok()?;

        Some(value + "/appcache/librarycache/")
    }
}
