// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Paul <abonnementspaul (at) gmail.com>
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, version 3.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

#[cfg(test)]
mod tests {
    use crate::backend::app_manager::AppManager;
    use crate::backend::connected_steam::ConnectedSteam;
    use crate::backend::key_value::KeyValue;
    use crate::steam_client::steam_apps_001_wrapper::SteamApps001AppDataKeys;
    use std::env;
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard};

    static STEAM: Mutex<()> = Mutex::new(());

    fn steam_guard() -> MutexGuard<'static, ()> {
        let guard = STEAM.lock().unwrap_or_else(|e| e.into_inner());
        // A machine can hold several installs and the locator picks by preference,
        // not by what is live; only one running install is unambiguous.
        #[cfg(target_os = "linux")]
        if let [root] = crate::utils::steam_ns::running_steam_install_roots().as_slice() {
            let _ = crate::utils::steam_locator::TEST_INSTALL_ROOT.set(root.clone());
        }
        // Same refusal the orchestrator makes: connecting to another live Steam
        // half-succeeds, and every call after that is refused for no stated reason.
        #[cfg(target_os = "linux")]
        assert!(
            crate::utils::steam_ns::loaded_install_is_running(),
            "Steam is not running from {}, the install these tests load. Start \
             that Steam, or set SAM_STEAM_INSTALL_ROOT to the one that is running.",
            crate::utils::steam_locator::SteamLocator::get_local_steam_install_root_folders()
                .first()
                .map_or_else(
                    || "any known install".to_owned(),
                    |p| p.display().to_string()
                )
        );
        guard
    }

    #[test]
    fn get_achievements_with_callback() {
        let _steam = steam_guard();
        let mut app_manager =
            AppManager::new_connected(206690, false).expect("Failed to create app manager");
        let achievements = app_manager
            .get_achievements(true, "")
            .expect("Failed to get achievements");
        println!("{achievements:?}")
    }

    #[test]
    fn get_stats_no_message() {
        let _steam = steam_guard();
        let mut app_manager =
            AppManager::new_connected(480, false).expect("Failed to create app manager");
        let stats = app_manager.get_statistics("").expect("Failed to get stats");
        println!("{stats:?}")
    }

    #[test]
    fn get_stats_stealth() {
        let _steam = steam_guard();
        let mut app_manager =
            AppManager::new_connected(480, true).expect("Failed to create app manager");
        let stats = app_manager.get_statistics("").expect("Failed to get stats");
        println!("{stats:?}")
    }

    #[test]
    fn get_global_percentages_stealth() {
        let _steam = steam_guard();
        let mut app_manager =
            AppManager::new_connected(206690, true).expect("Failed to create app manager");
        let achievements = app_manager
            .get_achievements(true, "")
            .expect("Failed to get achievements");
        let with_percent: Vec<_> = achievements
            .iter()
            .filter_map(|a| a.global_achieved_percent.map(|p| (a.id.clone(), p)))
            .collect();
        println!(
            "{} of {} achievements carry a global percentage",
            with_percent.len(),
            achievements.len()
        );
        for (id, percent) in with_percent.iter().take(10) {
            println!("  {id}: {percent:.2}%");
        }
        assert!(
            !with_percent.is_empty(),
            "no global percentages came back in stealth mode"
        );
    }

    #[test]
    fn stealth_unlock_roundtrip() {
        let _steam = steam_guard();
        const ACHIEVEMENT: &str = "ACH_WIN_ONE_GAME";
        let mut app_manager =
            AppManager::new_connected(480, true).expect("Failed to create app manager");

        let before = app_manager
            .get_achievements(false, "")
            .expect("Failed to get achievements");
        let start = before
            .iter()
            .find(|a| a.id == ACHIEVEMENT)
            .unwrap_or_else(|| panic!("{ACHIEVEMENT} not in Spacewar's schema"));
        println!(
            "before: achieved={} at {:?}",
            start.is_achieved, start.unlock_time
        );

        assert!(
            app_manager
                .set_achievement(ACHIEVEMENT, true, true)
                .expect("unlock failed"),
            "unlock did not store"
        );

        let unlocked = app_manager
            .get_achievements(false, "")
            .expect("Failed to re-read achievements");
        let after = unlocked.iter().find(|a| a.id == ACHIEVEMENT).unwrap();
        println!(
            "after unlock: achieved={} at {:?}",
            after.is_achieved, after.unlock_time
        );
        assert!(
            after.is_achieved,
            "achievement did not read back as unlocked"
        );

        assert!(
            app_manager
                .set_achievement(ACHIEVEMENT, false, true)
                .expect("relock failed"),
            "relock did not store"
        );

        let relocked = app_manager
            .get_achievements(false, "")
            .expect("Failed to re-read achievements");
        let end = relocked.iter().find(|a| a.id == ACHIEVEMENT).unwrap();
        println!("after relock: achieved={}", end.is_achieved);
        assert!(
            !end.is_achieved,
            "achievement did not read back as relocked"
        );
    }

    #[test]
    fn stealth_reads_a_users_achievements() {
        let _steam = steam_guard();
        use crate::backend::stats_access::{StatsAccess, Stealth};
        use crate::backend::user_unlock_times::read_schema_achievements;
        use std::rc::Rc;

        const APP: u32 = 206690;

        let steam = Rc::new(ConnectedSteam::new(true).expect("Failed to connect to Steam"));
        let me = steam
            .user
            .get_steam_id()
            .expect("Failed to read the SteamID");
        let stealth = Stealth::new(steam, APP).expect("Failed to open IClientUserStats");

        stealth.prime().expect("Failed to load our own stats");

        let probe = read_schema_achievements(APP).expect("Failed to read the schema");
        let (probe_name, _) = probe
            .first()
            .expect("no achievements in the schema")
            .clone();
        let was_achieved = stealth
            .get_achievement_and_unlock_time(&probe_name)
            .expect("probe achievement unreadable")
            .0;
        if !was_achieved {
            stealth.set_achievement(&probe_name).expect("unlock failed");
            assert!(
                stealth.store_stats().expect("store failed"),
                "store refused"
            );
        }

        stealth
            .request_other_user_stats(me)
            .expect("RequestUserStats refused our own SteamID");

        let names = read_schema_achievements(APP).expect("Failed to read the schema");
        assert!(!names.is_empty(), "no achievements in the schema for {APP}");

        let mut unlocked = 0;
        for (api_name, _) in &names {
            let own = stealth
                .get_achievement_and_unlock_time(api_name)
                .unwrap_or_else(|e| panic!("{api_name} unreadable as our own: {e:?}"));
            let as_other = stealth
                .get_other_user_achievement(me, api_name)
                .unwrap_or_else(|| panic!("{api_name} unreadable as another user"));
            assert_eq!(own, as_other, "{api_name} disagrees between the two paths");
            if own.0 {
                assert_ne!(own.1, 0, "{api_name} is unlocked with no timestamp");
                unlocked += 1;
            }
        }
        assert!(
            unlocked > 0,
            "nothing unlocked, so no timestamp was compared"
        );
        println!(
            "{} achievements agree, {unlocked} of them unlocked",
            names.len()
        );

        if !was_achieved {
            stealth
                .clear_achievement(&probe_name)
                .expect("relock failed");
            assert!(
                stealth.store_stats().expect("store failed"),
                "store refused"
            );
        }
    }

    #[test]
    fn stealth_stat_roundtrip() {
        let _steam = steam_guard();
        let mut app_manager =
            AppManager::new_connected(480, true).expect("Failed to create app manager");
        app_manager
            .get_statistics("")
            .expect("Failed to load stat definitions");

        let before = app_manager.read_float_stat_state("AverageSpeed").current;
        assert!(
            app_manager
                .set_stat_f32("AverageSpeed", 12.5)
                .expect("float write failed"),
            "float write did not store"
        );
        assert_eq!(
            app_manager.read_float_stat_state("AverageSpeed").current,
            Some(12.5),
            "AverageSpeed did not read back as written"
        );

        let games = app_manager
            .read_int_stat_state("NumGames")
            .current
            .expect("NumGames unreadable");
        assert!(
            app_manager
                .set_stat_i32("NumGames", games + 1)
                .expect("int write failed"),
            "int write did not store"
        );
        assert_eq!(
            app_manager.read_int_stat_state("NumGames").current,
            Some(games + 1),
            "NumGames did not read back as written"
        );

        if let Some(before) = before {
            let _ = app_manager.set_stat_f32("AverageSpeed", before);
        }
    }

    #[test]
    fn reset_stats_no_message() {
        let _steam = steam_guard();
        let app_manager =
            AppManager::new_connected(480, false).expect("Failed to create app manager");
        let success = app_manager
            .reset_all_stats(true)
            .expect("Failed to get stats");
        println!("Success: {success:?}")
    }

    #[test]
    fn brute_force_app001_keys() {
        let _steam = steam_guard();
        // ISteamApps001::GetAppData does not read a file in this process. It is
        // an IPC shim: the call is marshalled over the Steam pipe to the running
        // Steam client, which answers from its in-memory appinfo (the `common`
        // section of each app). That data is backed on disk by, and refreshed
        // into, appcache/appinfo.vdf (binary KV, v29: header + string table),
        // and kept current from Valve's servers. So GetAppData needs Steam
        // running; parsing appinfo.vdf directly is the offline equivalent.

        let connected_steam = ConnectedSteam::new(true).expect("Failed to create connected steam");
        let try_force = |key: &str| {
            let null_terminated_key = format!("{key}\0");
            println!(
                "{key}:\t {}",
                connected_steam
                    .apps_001
                    .get_app_data(&220, &null_terminated_key)
                    .unwrap_or("Failure".to_string())
            );
        };

        try_force(&SteamApps001AppDataKeys::Name.as_string());
        try_force(&SteamApps001AppDataKeys::Logo.as_string());
        try_force(&SteamApps001AppDataKeys::SmallCapsule("english").as_string());
        try_force("subscribed");

        try_force("metascore");
        try_force("metascore/score");
        try_force("metascorescore");
        try_force("metascorerating");
        try_force("metascore/rating");
        try_force("metascore_rating");
        try_force("metascore_rating");

        try_force("metacritic");
        try_force("metacritic/score");
        try_force("metacritic/url");
        try_force("metacriticurl/english");
        try_force("metacritic/url/english");
        try_force("metacriticscore");
        try_force("metacritic_score");
        try_force("metacriticrating");
        try_force("metacritic/rating");
        try_force("metacritic_rating");
        try_force("metacritic_rating");

        try_force("developer");
        try_force("developer/english");
        try_force("extended/developer");
        try_force("state");
        try_force("homepage");
        try_force("clienticon");
    }

    #[test]
    fn keyval() {
        #[cfg(target_os = "linux")]
        let home = env::var("HOME").expect("Failed to get home directory");
        #[cfg(target_os = "linux")]
        let bin_file = PathBuf::from(
            home + "/snap/steam/common/.local/share/Steam/appcache/stats/UserGameStatsSchema_730.bin",
        );
        #[cfg(target_os = "windows")]
        let program_files =
            env::var("ProgramFiles(x86)").expect("Failed to get Program Files directory");
        #[cfg(target_os = "windows")]
        let bin_file =
            PathBuf::from(program_files + "\\Steam\\appcache\\stats\\UserGameStatsSchema_480.bin");

        let kv = KeyValue::load_as_binary(bin_file).expect("Failed to load key value");
        println!("{kv:?}");
    }
}
