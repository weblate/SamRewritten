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

use crate::backend::app_info::AppInfo;
use crate::backend::local_config::PlaytimeMap;
use crate::dev_println;
use crate::steam_client::steamworks_types::AppId_t;
use crate::utils::ipc_types::SamError;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

const KEY_PREFIX: &str = "user-collections.";
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub const FAVORITE_ID: &str = "favorite";
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub const HIDDEN_ID: &str = "hidden";

// Positions 0 and 3 exist in the format; Valve's matcher tests neither.
const GROUP_STATE: usize = 1;
const GROUP_FEATURES: usize = 2;
const GROUP_STORE_TAGS: usize = 4;
const GROUP_SUBSCRIPTION: usize = 5;
const GROUP_FRIENDS: usize = 6;
const GROUP_LANGUAGES: usize = 7;
const GROUP_CATEGORIES: usize = 8;
const METADATA_GROUPS: [usize; 5] = [
    GROUP_FEATURES,
    GROUP_STORE_TAGS,
    GROUP_SUBSCRIPTION,
    GROUP_LANGUAGES,
    GROUP_CATEGORIES,
];
const LANGUAGE_COUNT: u32 = 32;
const KNOWN_GROUP_COUNT: usize = 9;

const COMPAT_UNSUPPORTED: i32 = 1;
const COMPAT_PLAYABLE: i32 = 2;
const COMPAT_VERIFIED: i32 = 3;
const STEAMOS_UNSUPPORTED: i32 = 1;
const STEAMOS_COMPATIBLE: i32 = 2;

const OPTION_DECK_UNSUPPORTED: u32 = 15;
const OPTION_INSTALLED: u32 = 1;
const OPTION_PLAYED: u32 = 3;
const OPTION_UNPLAYED: u32 = 4;
const OPTION_EA_SUBSCRIPTION: u32 = 4000;
const EA_PLAY_APP_ID: u32 = 1_289_670;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedReason {
    SearchText,
    UnknownFilter,
    Unavailable,
    Offline,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CollectionModel {
    pub id: String,
    pub name: String,
    pub app_ids: Vec<AppId_t>,
    pub unsupported: Option<UnsupportedReason>,
}

#[derive(Deserialize)]
struct RawEntry {
    #[serde(default)]
    value: Option<String>,
}

#[derive(Deserialize, Default)]
struct FilterGroup {
    #[serde(default, rename = "rgOptions")]
    options: Vec<u32>,
    #[serde(default, rename = "bAcceptUnion")]
    accept_union: bool,
}

const SPEC_FORMAT_VERSION: u32 = 2;

#[derive(Deserialize, Default)]
struct FilterSpec {
    /// An unknown version -- absent included -- builds no filter: the collection
    /// is then static.
    #[serde(default, rename = "nFormatVersion")]
    format_version: Option<u32>,
    #[serde(default, rename = "strSearchText")]
    search_text: String,
    #[serde(default, rename = "filterGroups")]
    groups: Vec<FilterGroup>,
}

#[derive(Deserialize)]
struct RawCollection {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    added: Vec<AppId_t>,
    #[serde(default)]
    removed: Vec<AppId_t>,
    #[serde(default, rename = "filterSpec")]
    filter_spec: Option<FilterSpec>,
}

pub struct Collection {
    raw: RawCollection,
    unsupported: Option<UnsupportedReason>,
}

impl Collection {
    fn usable_spec(&self) -> Option<&FilterSpec> {
        if self.unsupported.is_some() {
            return None;
        }
        self.raw.filter_spec.as_ref()
    }

    pub fn needs_app_info(&self) -> bool {
        self.usable_spec()
            .is_some_and(|spec| METADATA_GROUPS.iter().any(|g| spec.group(*g).is_some()))
    }

    fn state_options(&self, wanted: &[u32]) -> bool {
        self.usable_spec()
            .and_then(|spec| spec.group(GROUP_STATE))
            .is_some_and(|group| group.options.iter().any(|o| wanted.contains(o)))
    }

    pub fn needs_playtimes(&self) -> bool {
        self.state_options(&[OPTION_PLAYED, OPTION_UNPLAYED])
    }

    pub fn needs_installed(&self) -> bool {
        self.state_options(&[OPTION_INSTALLED])
    }

    pub fn friend_ids(&self) -> Vec<u32> {
        self.usable_spec()
            .and_then(|spec| spec.group(GROUP_FRIENDS))
            .map(|group| group.options.clone())
            .unwrap_or_default()
    }
}

impl FilterSpec {
    /// Steam drops this option as it loads a spec, so a stored one means nothing.
    fn strip_ignored_options(&mut self) {
        if let Some(group) = self.groups.get_mut(GROUP_FEATURES) {
            group.options.retain(|o| *o != OPTION_DECK_UNSUPPORTED);
        }
    }

    /// Steam matches *no* game for a spec with nothing set, not every game.
    fn is_empty(&self) -> bool {
        self.search_text.is_empty() && self.groups.iter().all(|g| g.options.is_empty())
    }

    fn group(&self, index: usize) -> Option<&FilterGroup> {
        self.groups.get(index).filter(|g| !g.options.is_empty())
    }
}

pub struct LibraryFacts<'a> {
    pub playtimes: &'a PlaytimeMap,
    pub app_info: &'a HashMap<AppId_t, AppInfo>,
    pub installed: &'a HashSet<AppId_t>,
    /// Answering from facts that failed to load would empty a collection while
    /// still presenting it as an answer.
    pub have_app_info: bool,
    pub have_playtimes: bool,
    pub have_installed: bool,
    pub friends_owned: &'a HashMap<u32, HashSet<AppId_t>>,
    pub online: bool,
}

fn feature_matches(option: u32, info: &AppInfo) -> Option<bool> {
    let full = info.controller_support == "full";
    let partial = info.controller_support == "partial";
    let any_category = |ids: &[u32]| ids.iter().any(|id| info.has_category(*id));

    Some(match option {
        1 => full || info.has_category(28),
        2 => full || partial || any_category(&[28, 18]),
        // VR tests live on the client's app overview, not in appinfo.vdf.
        3 | 26 => return None,
        4 => info.has_category(29),
        5 => info.has_category(30),
        6 => info.has_category(22),
        7 => info.has_category(2),
        8 => any_category(&[1, 36, 37, 27, 20, 24]),
        9 => any_category(&[9, 38, 39]),
        10 => info.has_category(23),
        11 => info.has_category(44),
        12 => info.deck_compat >= COMPAT_VERIFIED,
        13 => info.deck_compat >= COMPAT_PLAYABLE,
        14 => info.deck_compat != COMPAT_UNSUPPORTED,
        16 => any_category(&[55, 56]),
        17 => info.has_category(56),
        18 => any_category(&[57, 58]),
        19 => info.has_category(58),
        20 => info.has_category(59),
        21 => info.has_category(60),
        22 => info.has_category(61),
        23 => info.has_category(62),
        24 => info.steamos_compat >= STEAMOS_COMPATIBLE,
        25 => info.steamos_compat != STEAMOS_UNSUPPORTED,
        27 => any_category(&[39, 37, 24]),
        // Valve's matcher has no AnyController branch, so it matches nothing there.
        28 => false,
        29 => info.steam_machine_compat >= COMPAT_VERIFIED,
        30 => info.steam_machine_compat >= COMPAT_PLAYABLE,
        31 => info.steam_machine_compat != COMPAT_UNSUPPORTED,
        _ => return None,
    })
}

fn state_matches(option: u32, played: bool, installed: bool) -> Option<bool> {
    Some(match option {
        OPTION_INSTALLED => installed,
        OPTION_PLAYED => played,
        OPTION_UNPLAYED => !played,
        _ => return None,
    })
}

fn group_matches(group: &FilterGroup, mut predicate: impl FnMut(u32) -> bool) -> bool {
    if group.accept_union {
        group.options.iter().any(|o| predicate(*o))
    } else {
        group.options.iter().all(|o| predicate(*o))
    }
}

fn unsupported_reason(spec: &FilterSpec) -> Option<UnsupportedReason> {
    if !spec.search_text.is_empty() {
        return Some(UnsupportedReason::SearchText);
    }
    if (KNOWN_GROUP_COUNT..spec.groups.len()).any(|i| spec.group(i).is_some()) {
        return Some(UnsupportedReason::UnknownFilter);
    }

    let probe = AppInfo::default();
    let known = |group: Option<&FilterGroup>, check: &dyn Fn(u32) -> bool| {
        group.is_none_or(|g| g.options.iter().all(|o| check(*o)))
    };
    let all_known = known(spec.group(GROUP_STATE), &|o| {
        state_matches(o, false, false).is_some()
    }) && known(spec.group(GROUP_FEATURES), &|o| {
        feature_matches(o, &probe).is_some()
    }) && known(spec.group(GROUP_SUBSCRIPTION), &|o| {
        o == OPTION_EA_SUBSCRIPTION
    }) && known(spec.group(GROUP_LANGUAGES), &|o| o < LANGUAGE_COUNT);

    (!all_known).then_some(UnsupportedReason::UnknownFilter)
}

fn matches(spec: &FilterSpec, app_id: AppId_t, facts: &LibraryFacts) -> bool {
    if let Some(group) = spec.group(GROUP_STATE) {
        let played = facts
            .playtimes
            .get(&app_id)
            .and_then(|p| p.last_played)
            .is_some_and(|last| last > 0);
        let installed = facts.installed.contains(&app_id);
        if !group_matches(group, |o| {
            state_matches(o, played, installed).unwrap_or(false)
        }) {
            return false;
        }
    }

    // Steam evaluates a blank overview for apps it has no metadata on.
    let missing = AppInfo::default();
    let info = facts.app_info.get(&app_id).unwrap_or(&missing);
    if let Some(group) = spec.group(GROUP_FEATURES)
        && !group_matches(group, |o| feature_matches(o, info).unwrap_or(false))
    {
        return false;
    }
    if let Some(group) = spec.group(GROUP_STORE_TAGS)
        && !group_matches(group, |o| info.has_store_tag(o))
    {
        return false;
    }
    if let Some(group) = spec.group(GROUP_SUBSCRIPTION)
        && !group_matches(group, |o| {
            o == OPTION_EA_SUBSCRIPTION && info.mastersub_appid == EA_PLAY_APP_ID
        })
    {
        return false;
    }
    // Valve also drops a game the filtered friend is the one lending, which
    // nothing we can read names.
    if let Some(group) = spec.group(GROUP_FRIENDS)
        && !group_matches(group, |o| {
            facts
                .friends_owned
                .get(&o)
                .is_some_and(|owned| owned.contains(&app_id))
        })
    {
        return false;
    }
    if let Some(group) = spec.group(GROUP_LANGUAGES)
        && !group_matches(group, |o| info.has_language(o))
    {
        return false;
    }
    if let Some(group) = spec.group(GROUP_CATEGORIES)
        && !group_matches(group, |o| info.has_category(o))
    {
        return false;
    }
    true
}

fn parse_str(contents: &str) -> Result<Vec<Collection>, SamError> {
    let entries: Vec<(String, RawEntry)> = serde_json::from_str(contents).map_err(|e| {
        dev_println!("ORCH", "Failed to parse collections: {e}");
        SamError::UnknownError
    })?;

    let mut out = Vec::new();
    for (key, entry) in entries {
        if !key.starts_with(KEY_PREFIX) {
            continue;
        }
        let Some(value) = entry.value else { continue };
        let mut raw: RawCollection = match serde_json::from_str(&value) {
            Ok(raw) => raw,
            Err(e) => {
                dev_println!("ORCH", "Skipping collection {key}: {e}");
                continue;
            }
        };
        raw.filter_spec
            .take_if(|spec| spec.format_version != Some(SPEC_FORMAT_VERSION));
        if let Some(spec) = raw.filter_spec.as_mut() {
            spec.strip_ignored_options();
        }
        let unsupported = raw.filter_spec.as_ref().and_then(unsupported_reason);
        out.push(Collection { raw, unsupported });
    }
    Ok(out)
}

pub fn parse(path: &Path) -> Result<Vec<Collection>, SamError> {
    let contents = std::fs::read_to_string(path).map_err(|e| {
        dev_println!("ORCH", "Failed to read {}: {e}", path.display());
        SamError::UnknownError
    })?;
    parse_str(&contents)
}

pub fn resolve(
    collections: Vec<Collection>,
    library: &[AppId_t],
    facts: &LibraryFacts,
) -> Vec<CollectionModel> {
    collections
        .into_iter()
        .map(|collection| {
            let unsupported = collection.unsupported.or_else(|| {
                let missing_friend = collection
                    .friend_ids()
                    .iter()
                    .any(|id| !facts.friends_owned.contains_key(id));
                if missing_friend && !facts.online {
                    return Some(UnsupportedReason::Offline);
                }
                let starved = missing_friend
                    || (collection.needs_app_info() && !facts.have_app_info)
                    || (collection.needs_playtimes() && !facts.have_playtimes)
                    || (collection.needs_installed() && !facts.have_installed);
                starved.then_some(UnsupportedReason::Unavailable)
            });
            let raw = collection.raw;
            let mut app_ids = Vec::new();

            if unsupported.is_none() {
                let mut members: HashSet<AppId_t> = HashSet::new();
                if let Some(spec) = raw.filter_spec.as_ref().filter(|spec| !spec.is_empty()) {
                    members.extend(
                        library
                            .iter()
                            .copied()
                            .filter(|id| matches(spec, *id, facts)),
                    );
                }
                // Steam's order: matches, additions, then removals (dynamic only).
                members.extend(raw.added.iter().copied());
                if raw.filter_spec.is_some() {
                    for id in &raw.removed {
                        members.remove(id);
                    }
                }
                app_ids.extend(library.iter().copied().filter(|id| members.contains(id)));
            }

            CollectionModel {
                id: raw.id,
                name: raw.name,
                app_ids,
                unsupported,
            }
        })
        .collect()
}

/// `None` means we could not tell, which is not the same as nothing installed.
pub fn installed_apps(path: &Path) -> Option<HashSet<AppId_t>> {
    #[derive(Deserialize)]
    struct Folder {
        #[serde(default)]
        apps: HashMap<String, String>,
    }

    let contents = std::fs::read_to_string(path)
        .inspect_err(|e| dev_println!("ORCH", "Failed to read {}: {e}", path.display()))
        .ok()?;
    let folders: HashMap<String, Folder> = keyvalues_serde::from_str(&contents)
        .inspect_err(|e| dev_println!("ORCH", "Failed to parse libraryfolders.vdf: {e}"))
        .ok()?;
    Some(
        folders
            .values()
            .flat_map(|folder| folder.apps.keys())
            .filter_map(|id| id.parse::<AppId_t>().ok())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::local_config::AppPlaytime;

    const FIXTURE: &str = r#"[
      ["GameReleased", {"key":"GameReleased","timestamp":1,"value":"{}","version":"1"}],
      ["user-collections.favorite", {"key":"user-collections.favorite","timestamp":2,
        "value":"{\"id\":\"favorite\",\"name\":\"Favoris\",\"added\":[240,730,99999],\"removed\":[]}"}],
      ["user-collections.uc-gone", {"key":"user-collections.uc-gone","timestamp":3,
        "is_deleted":true,"version":"5"}],
      ["user-collections.uc-farm", {"key":"user-collections.uc-farm","timestamp":4,
        "value":"{\"id\":\"uc-farm\",\"name\":\"Farm\",\"added\":[555],\"removed\":[240],\"filterSpec\":{\"nFormatVersion\":2,\"strSearchText\":\"\",\"filterGroups\":[{\"rgOptions\":[],\"bAcceptUnion\":true},{\"rgOptions\":[4],\"bAcceptUnion\":false},{\"rgOptions\":[4],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false}]}}"}],
      ["user-collections.uc-friend", {"key":"user-collections.uc-friend","timestamp":5,
        "value":"{\"id\":\"uc-friend\",\"name\":\"Squad\",\"added\":[],\"removed\":[],\"filterSpec\":{\"nFormatVersion\":2,\"strSearchText\":\"\",\"filterGroups\":[{\"rgOptions\":[],\"bAcceptUnion\":true},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[12345],\"bAcceptUnion\":true},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false}]}}"}],
      ["user-collections.uc-vr", {"key":"user-collections.uc-vr","timestamp":6,
        "value":"{\"id\":\"uc-vr\",\"name\":\"VR\",\"added\":[],\"removed\":[],\"filterSpec\":{\"nFormatVersion\":2,\"strSearchText\":\"\",\"filterGroups\":[{\"rgOptions\":[],\"bAcceptUnion\":true},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[3],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false}]}}"}],
      ["user-collections.uc-search", {"key":"user-collections.uc-search","timestamp":7,
        "value":"{\"id\":\"uc-search\",\"name\":\"Search\",\"added\":[],\"removed\":[],\"filterSpec\":{\"nFormatVersion\":2,\"strSearchText\":\"portal\",\"filterGroups\":[]}}"}],
      ["user-collections.uc-noversion", {"key":"user-collections.uc-noversion","timestamp":9,
        "value":"{\"id\":\"uc-noversion\",\"name\":\"uc-noversion\",\"added\":[555],\"removed\":[],\"filterSpec\":{\"strSearchText\":\"\",\"filterGroups\":[{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[4],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false}]}}"}],
      ["user-collections.uc-ignored", {"key":"user-collections.uc-ignored","timestamp":9,
        "value":"{\"id\":\"uc-ignored\",\"name\":\"uc-ignored\",\"added\":[],\"removed\":[],\"filterSpec\":{\"nFormatVersion\":2,\"strSearchText\":\"\",\"filterGroups\":[{\"rgOptions\":[1],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[2],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false}]}}"}],
      ["user-collections.uc-lang", {"key":"user-collections.uc-lang","timestamp":9,
        "value":"{\"id\":\"uc-lang\",\"name\":\"uc-lang\",\"added\":[],\"removed\":[],\"filterSpec\":{\"nFormatVersion\":2,\"strSearchText\":\"\",\"filterGroups\":[{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[0],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false}]}}"}],
      ["user-collections.uc-static", {"key":"user-collections.uc-static","timestamp":9,
        "value":"{\"id\":\"uc-static\",\"name\":\"uc-static\",\"added\":[240,730],\"removed\":[730]}"}],
      ["user-collections.uc-newformat", {"key":"user-collections.uc-newformat","timestamp":9,
        "value":"{\"id\":\"uc-newformat\",\"name\":\"uc-newformat\",\"added\":[240,777],\"removed\":[240],\"filterSpec\":{\"nFormatVersion\":3,\"strSearchText\":\"\",\"filterGroups\":[{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[4],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false}]}}"}],
      ["user-collections.uc-blank", {"key":"user-collections.uc-blank","timestamp":9,
        "value":"{\"id\":\"uc-blank\",\"name\":\"Blank\",\"added\":[777],\"removed\":[],\"filterSpec\":{\"nFormatVersion\":2,\"strSearchText\":\"\",\"filterGroups\":[{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false}]}}"}],
      ["user-collections.uc-legacy", {"key":"user-collections.uc-legacy","timestamp":9,
        "value":"{\"id\":\"uc-legacy\",\"name\":\"Legacy\",\"added\":[777],\"removed\":[],\"filterSpec\":{\"nFormatVersion\":2,\"strSearchText\":\"\",\"filterGroups\":[{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[15],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false}]}}"}],
      ["user-collections.uc-both", {"key":"user-collections.uc-both","timestamp":9,
        "value":"{\"id\":\"uc-both\",\"name\":\"Both\",\"added\":[555],\"removed\":[555],\"filterSpec\":{\"nFormatVersion\":2,\"strSearchText\":\"\",\"filterGroups\":[{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[4],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false}]}}"}],
      ["user-collections.uc-installed", {"key":"user-collections.uc-installed","timestamp":9,
        "value":"{\"id\":\"uc-installed\",\"name\":\"Installed\",\"added\":[],\"removed\":[],\"filterSpec\":{\"nFormatVersion\":2,\"strSearchText\":\"\",\"filterGroups\":[{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[1],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false}]}}"}],
      ["user-collections.uc-union", {"key":"user-collections.uc-union","timestamp":9,
        "value":"{\"id\":\"uc-union\",\"name\":\"Union\",\"added\":[],\"removed\":[],\"filterSpec\":{\"nFormatVersion\":2,\"strSearchText\":\"\",\"filterGroups\":[{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[4,6],\"bAcceptUnion\":true},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false}]}}"}],
      ["user-collections.uc-all", {"key":"user-collections.uc-all","timestamp":9,
        "value":"{\"id\":\"uc-all\",\"name\":\"All of\",\"added\":[],\"removed\":[],\"filterSpec\":{\"nFormatVersion\":2,\"strSearchText\":\"\",\"filterGroups\":[{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[4,6],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false}]}}"}],
      ["user-collections.uc-removed", {"key":"user-collections.uc-removed","timestamp":9,
        "value":"{\"id\":\"uc-removed\",\"name\":\"Removed\",\"added\":[],\"removed\":[730],\"filterSpec\":{\"nFormatVersion\":2,\"strSearchText\":\"\",\"filterGroups\":[{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[4],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false}]}}"}],
      ["user-collections.uc-deck", {"key":"user-collections.uc-deck","timestamp":9,
        "value":"{\"id\":\"uc-deck\",\"name\":\"Deck\",\"added\":[],\"removed\":[],\"filterSpec\":{\"nFormatVersion\":2,\"strSearchText\":\"\",\"filterGroups\":[{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[15,4],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false}]}}"}],
      ["user-collections.uc-future", {"key":"user-collections.uc-future","timestamp":9,
        "value":"{\"id\":\"uc-future\",\"name\":\"Future\",\"added\":[],\"removed\":[],\"filterSpec\":{\"nFormatVersion\":2,\"strSearchText\":\"\",\"filterGroups\":[{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[4],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[],\"bAcceptUnion\":false},{\"rgOptions\":[1],\"bAcceptUnion\":false}]}}"}]
    ]"#;

    fn card_game() -> AppInfo {
        AppInfo {
            categories: HashSet::from([29]),
            ..AppInfo::default()
        }
    }

    fn english_card_game() -> AppInfo {
        AppInfo {
            languages: HashSet::from([0]),
            ..card_game()
        }
    }

    fn parsed() -> Vec<Collection> {
        parse_str(FIXTURE).expect("fixture should parse")
    }

    fn resolved() -> HashMap<String, CollectionModel> {
        let library = [240, 730, 555, 777, 999];
        let playtimes = PlaytimeMap::from([(
            240,
            AppPlaytime {
                playtime_minutes: Some(10),
                last_played: Some(1_600_000_000),
            },
        )]);
        let app_info = HashMap::from([(730, english_card_game()), (999, card_game())]);
        let installed = HashSet::new();
        let friends_owned = HashMap::from([(12345, HashSet::from([730, 777]))]);
        let facts = LibraryFacts {
            playtimes: &playtimes,
            app_info: &app_info,
            installed: &installed,
            have_app_info: true,
            have_playtimes: true,
            have_installed: true,
            friends_owned: &friends_owned,
            online: true,
        };
        resolve(parsed(), &library, &facts)
            .into_iter()
            .map(|c| (c.id.clone(), c))
            .collect()
    }

    #[test]
    fn non_collection_keys_and_tombstones_are_skipped() {
        let ids: Vec<String> = parsed().into_iter().map(|c| c.raw.id).collect();
        assert!(
            !ids.iter().any(|id| id == "uc-gone"),
            "tombstone kept: {ids:?}"
        );
        assert!(!ids.iter().any(|id| id == "GameReleased"));
        assert_eq!(ids.len(), 19);
    }

    #[test]
    fn a_static_collection_is_its_added_list_intersected_with_the_library() {
        let models = resolved();
        let favorite = &models[FAVORITE_ID];
        assert_eq!(favorite.unsupported, None);
        assert_eq!(favorite.app_ids, vec![240, 730]);
    }

    #[test]
    fn a_dynamic_collection_matches_then_adds_manually() {
        let models = resolved();
        let farm = &models["uc-farm"];
        assert_eq!(farm.unsupported, None);
        assert_eq!(farm.app_ids, vec![730, 555, 999]);
    }

    #[test]
    fn a_short_group_list_is_treated_as_empty_trailing_groups() {
        assert!(
            parsed()
                .iter()
                .any(|c| c.raw.id == "uc-farm" && c.unsupported.is_none())
        );
    }

    #[test]
    fn installed_apps_are_collected_across_every_library_folder() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/games\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"240\"\t\t\"1\"\n\t\t}\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"/more\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"730\"\t\t\"2\"\n\t\t}\n\t}\n}\n";
        let path = std::env::temp_dir().join(format!(
            "sam_test_libraryfolders_{}.vdf",
            std::process::id()
        ));
        std::fs::write(&path, vdf).expect("fixture should write");
        let installed = installed_apps(&path);
        std::fs::remove_file(&path).ok();
        assert_eq!(installed, Some(HashSet::from([240, 730])));
    }

    #[test]
    fn a_missing_library_folders_file_reads_as_unknown_not_as_nothing_installed() {
        assert_eq!(
            installed_apps(Path::new("/nonexistent/libraryfolders.vdf")),
            None
        );
    }

    #[test]
    fn a_spec_with_nothing_set_matches_nothing_rather_than_everything() {
        let models = resolved();
        assert_eq!(models["uc-blank"].unsupported, None);
        assert_eq!(models["uc-blank"].app_ids, vec![777]);
        assert_eq!(models["uc-legacy"].app_ids, vec![777]);
    }

    #[test]
    fn the_positions_steam_never_tests_are_ignored_rather_than_refused() {
        let models = resolved();
        assert_eq!(models["uc-ignored"].unsupported, None);
        assert_eq!(models["uc-ignored"].app_ids, vec![240, 730, 555, 777, 999]);
    }

    #[test]
    fn a_language_filter_reads_the_languages_appinfo_lists() {
        let models = resolved();
        assert_eq!(models["uc-lang"].unsupported, None);
        assert_eq!(models["uc-lang"].app_ids, vec![730]);
    }

    #[test]
    fn a_language_past_the_ones_we_know_is_refused() {
        let spec = format!(
            "{{\"id\":\"x\",\"name\":\"x\",\"added\":[],\"removed\":[],\"filterSpec\":             {{\"nFormatVersion\":2,\"strSearchText\":\"\",\"filterGroups\":[{}]}}}}",
            (0..9)
                .map(|i| if i == GROUP_LANGUAGES {
                    "{\"rgOptions\":[99],\"bAcceptUnion\":false}"
                } else {
                    "{\"rgOptions\":[],\"bAcceptUnion\":false}"
                })
                .collect::<Vec<_>>()
                .join(",")
        );
        let json = format!(
            "[[\"user-collections.x\",{{\"key\":\"user-collections.x\",\"value\":{}}}]]",
            serde_json::to_string(&spec).expect("string should serialise")
        );
        let parsed = parse_str(&json).expect("fixture should parse");
        assert_eq!(
            parsed[0].unsupported,
            Some(UnsupportedReason::UnknownFilter)
        );
    }

    #[test]
    fn a_static_collection_ignores_its_removed_list() {
        assert_eq!(resolved()["uc-static"].app_ids, vec![240, 730]);
    }

    #[test]
    fn a_spec_in_an_unknown_format_version_is_treated_as_no_filter() {
        let models = resolved();
        assert_eq!(models["uc-newformat"].unsupported, None);
        assert_eq!(models["uc-newformat"].app_ids, vec![240, 777]);
        assert_eq!(models["uc-noversion"].app_ids, vec![555]);
    }

    #[test]
    fn a_removal_beats_an_addition_of_the_same_game() {
        assert_eq!(resolved()["uc-both"].app_ids, vec![730, 999]);
    }

    #[test]
    fn a_collection_is_refused_when_the_files_behind_it_could_not_be_read() {
        let library = [240, 730];
        let playtimes = PlaytimeMap::new();
        let app_info = HashMap::new();
        let installed = HashSet::new();
        let friends_owned = HashMap::new();
        let facts = LibraryFacts {
            playtimes: &playtimes,
            app_info: &app_info,
            installed: &installed,
            have_app_info: false,
            have_playtimes: false,
            have_installed: false,
            friends_owned: &friends_owned,
            online: true,
        };
        let models: HashMap<String, CollectionModel> = resolve(parsed(), &library, &facts)
            .into_iter()
            .map(|c| (c.id.clone(), c))
            .collect();
        assert_eq!(
            models["uc-union"].unsupported,
            Some(UnsupportedReason::Unavailable),
            "needs appinfo.vdf"
        );
        assert_eq!(
            models["uc-installed"].unsupported,
            Some(UnsupportedReason::Unavailable),
            "needs the play state files"
        );
        assert_eq!(models[FAVORITE_ID].unsupported, None);
        assert_eq!(models["uc-blank"].unsupported, None);
    }

    #[test]
    fn only_the_collections_that_needed_the_missing_file_are_refused() {
        let starve = |playtimes: bool, installed: bool| {
            let times = PlaytimeMap::new();
            let app_info = HashMap::from([(730, card_game()), (999, card_game())]);
            let apps = HashSet::new();
            let facts = LibraryFacts {
                playtimes: &times,
                app_info: &app_info,
                installed: &apps,
                have_app_info: true,
                have_playtimes: playtimes,
                have_installed: installed,
                friends_owned: &HashMap::new(),
                online: true,
            };
            resolve(parsed(), &[240, 730], &facts)
                .into_iter()
                .map(|c| (c.id.clone(), c.unsupported))
                .collect::<HashMap<String, Option<UnsupportedReason>>>()
        };

        let no_playtimes = starve(false, true);
        assert_eq!(
            no_playtimes["uc-farm"],
            Some(UnsupportedReason::Unavailable)
        );
        assert_eq!(no_playtimes["uc-installed"], None);

        let no_installed = starve(true, false);
        assert_eq!(no_installed["uc-farm"], None);
        assert_eq!(
            no_installed["uc-installed"],
            Some(UnsupportedReason::Unavailable)
        );
    }

    #[test]
    fn a_group_is_a_union_or_an_intersection_of_its_options() {
        let models = resolved();
        assert_eq!(models["uc-union"].app_ids, vec![730, 999]);
        assert!(models["uc-all"].app_ids.is_empty());
    }

    #[test]
    fn a_removal_takes_a_game_back_out_of_a_filters_own_matches() {
        assert_eq!(resolved()["uc-removed"].app_ids, vec![999]);
    }

    #[test]
    fn the_option_steam_ignores_is_dropped_rather_than_failed() {
        let models = resolved();
        assert_eq!(models["uc-deck"].unsupported, None);
        assert_eq!(models["uc-deck"].app_ids, vec![730, 999]);
    }

    #[test]
    fn a_group_added_after_this_evaluator_makes_a_collection_unsupported() {
        assert_eq!(
            resolved()["uc-future"].unsupported,
            Some(UnsupportedReason::UnknownFilter)
        );
    }

    #[test]
    fn filters_we_cannot_reproduce_are_refused_rather_than_guessed() {
        let models = resolved();
        assert_eq!(
            models["uc-vr"].unsupported,
            Some(UnsupportedReason::UnknownFilter)
        );
        assert_eq!(
            models["uc-search"].unsupported,
            Some(UnsupportedReason::SearchText)
        );
        for id in ["uc-vr", "uc-search"] {
            assert!(
                models[id].app_ids.is_empty(),
                "{id} should resolve to nothing"
            );
        }
    }

    #[test]
    fn a_friend_filter_keeps_the_games_that_friend_owns() {
        let models = resolved();
        assert_eq!(models["uc-friend"].unsupported, None);
        assert_eq!(models["uc-friend"].app_ids, vec![730, 777]);
    }

    #[test]
    fn a_friend_whose_library_could_not_be_read_refuses_the_collection() {
        let library = [240, 730];
        let playtimes = PlaytimeMap::new();
        let app_info = HashMap::new();
        let installed = HashSet::new();
        let friends_owned = HashMap::new();
        let facts = LibraryFacts {
            playtimes: &playtimes,
            app_info: &app_info,
            installed: &installed,
            have_app_info: true,
            have_playtimes: true,
            have_installed: true,
            friends_owned: &friends_owned,
            online: true,
        };
        let models: HashMap<String, CollectionModel> = resolve(parsed(), &library, &facts)
            .into_iter()
            .map(|c| (c.id.clone(), c))
            .collect();
        assert_eq!(
            models["uc-friend"].unsupported,
            Some(UnsupportedReason::Unavailable)
        );
        assert!(models["uc-friend"].app_ids.is_empty());
    }
}
