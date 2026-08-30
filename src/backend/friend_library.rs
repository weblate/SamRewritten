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

use crate::dev_println;
use crate::steam_client::client_unified_messages_wrapper::ClientUnifiedMessages;
use crate::steam_client::steamworks_types::{AppId_t, EResult};
use crate::steam_client::wrapper_types::SteamClientError;
use crate::utils::app_paths::get_temp_cache_dir;
use crate::utils::ipc_types::SamError;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const METHOD: &str = "Player.GetOwnedGames#1";

/// What Steam's own client keeps this answer for.
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

fn cache_path(account_id: u32) -> PathBuf {
    get_temp_cache_dir().join(format!("friend-games-{account_id}.json"))
}

pub fn cached_owned_games(account_id: u32) -> Option<HashSet<AppId_t>> {
    let path = cache_path(account_id);
    if fs::metadata(&path).ok()?.modified().ok()?.elapsed().ok()? > CACHE_TTL {
        return None;
    }
    let apps: Vec<AppId_t> = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
    Some(apps.into_iter().collect())
}

fn store(account_id: u32, owned: &HashSet<AppId_t>) {
    let apps: Vec<AppId_t> = owned.iter().copied().collect();
    match serde_json::to_vec(&apps) {
        Ok(bytes) => {
            if let Err(e) = fs::write(cache_path(account_id), bytes) {
                dev_println!("ORCH", "Could not cache friend {account_id}: {e}");
            }
        }
        Err(e) => dev_println!("ORCH", "Could not encode friend {account_id}: {e}"),
    }
}

const STEAM_ID64_BASE: u64 = 0x0110_0001_0000_0000;

pub fn steam_id64(account_id: u32) -> u64 {
    STEAM_ID64_BASE | u64::from(account_id)
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn take_varint(bytes: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*pos)?;
        *pos += 1;
        if shift == 63 && byte & 0x7e != 0 {
            return None;
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

fn advance(bytes: &[u8], pos: &mut usize, n: usize) -> Option<()> {
    let end = pos.checked_add(n)?;
    (end <= bytes.len()).then(|| *pos = end)
}

fn skip(bytes: &[u8], pos: &mut usize, wire: u64) -> Option<()> {
    match wire {
        0 => take_varint(bytes, pos).map(|_| ()),
        1 => advance(bytes, pos, 8),
        2 => {
            let len = take_varint(bytes, pos)? as usize;
            advance(bytes, pos, len)
        }
        5 => advance(bytes, pos, 4),
        _ => None,
    }
}

fn encode_request(account_id: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    out.push(0x08);
    put_varint(&mut out, steam_id64(account_id));
    out.extend_from_slice(&[0x18, 0x01]);
    out.extend_from_slice(&[0x30, 0x00]);
    out
}

fn decode_app_ids(bytes: &[u8]) -> Option<HashSet<AppId_t>> {
    let mut out = HashSet::new();
    let mut pos = 0usize;
    while pos < bytes.len() {
        let tag = take_varint(bytes, &mut pos)?;
        let (field, wire) = (tag >> 3, tag & 7);
        if field != 2 || wire != 2 {
            skip(bytes, &mut pos, wire)?;
            continue;
        }
        let len = take_varint(bytes, &mut pos)? as usize;
        let end = pos.checked_add(len)?;
        let game = bytes.get(pos..end)?;
        pos = end;

        let mut inner = 0usize;
        while inner < game.len() {
            let tag = take_varint(game, &mut inner)?;
            let (field, wire) = (tag >> 3, tag & 7);
            if field == 1 && wire == 0 {
                if let Ok(app_id) = AppId_t::try_from(take_varint(game, &mut inner)?) {
                    out.insert(app_id);
                }
            } else {
                skip(game, &mut inner, wire)?;
            }
        }
    }
    Some(out)
}

pub fn owned_games(
    unified: &ClientUnifiedMessages,
    account_id: u32,
    deadline: Instant,
) -> Result<HashSet<AppId_t>, SamError> {
    let response = match unified.call(METHOD, &encode_request(account_id), deadline) {
        Ok(response) => response,
        // A refusal is permanent; anything else Steam answers is transient.
        Err(SteamClientError::MethodResultFailed(_, result))
            if result == EResult::k_EResultAccessDenied as i32 =>
        {
            dev_println!("ORCH", "{METHOD} for {account_id} was refused");
            return Ok(HashSet::new());
        }
        Err(e) => {
            dev_println!("ORCH", "{METHOD} for {account_id} failed: {e}");
            return Err(SamError::UnknownError);
        }
    };
    let owned = decode_app_ids(&response).ok_or_else(|| {
        dev_println!(
            "ORCH",
            "{METHOD} for {account_id} returned a malformed body"
        );
        SamError::UnknownError
    })?;
    store(account_id, &owned);
    Ok(owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(app_id: u32, playtime: u32) -> Vec<u8> {
        let mut inner = vec![0x08];
        put_varint(&mut inner, u64::from(app_id));
        inner.push(0x20);
        put_varint(&mut inner, u64::from(playtime));
        let mut out = vec![0x12];
        put_varint(&mut out, inner.len() as u64);
        out.extend_from_slice(&inner);
        out
    }

    #[test]
    fn the_request_matches_what_steams_own_library_sends() {
        let bytes = encode_request(58903702);
        let mut pos = 1usize;
        assert_eq!(bytes[0], 0x08);
        assert_eq!(
            take_varint(&bytes, &mut pos),
            Some(STEAM_ID64_BASE + 58903702)
        );
        assert_eq!(&bytes[pos..], &[0x18, 0x01, 0x30, 0x00]);
    }

    #[test]
    fn an_account_id_widens_to_an_individual_public_steam_id() {
        assert_eq!(steam_id64(1), 76561197960265729);
    }

    #[test]
    fn only_the_app_ids_are_taken_out_of_the_games_list() {
        let mut body = vec![0x08, 0x02];
        body.extend(game(240, 12));
        body.extend(game(730, 0));
        assert_eq!(decode_app_ids(&body), Some(HashSet::from([240, 730])));
    }

    #[test]
    fn an_empty_body_is_an_empty_library_rather_than_an_error() {
        assert_eq!(decode_app_ids(&[]), Some(HashSet::new()));
    }

    #[test]
    fn a_truncated_body_is_refused() {
        let body = game(240, 12);
        for cut in 1..body.len() {
            assert_eq!(decode_app_ids(&body[..cut]), None, "cut at {cut}");
        }
    }

    #[test]
    fn a_cached_library_round_trips_and_expires() {
        // A real account id would collide with a live cache entry.
        let account_id = u32::MAX - 7;
        let owned = HashSet::from([240, 730, 440]);
        store(account_id, &owned);
        assert_eq!(cached_owned_games(account_id), Some(owned));

        let path = cache_path(account_id);
        let stale = std::time::SystemTime::now() - CACHE_TTL - Duration::from_secs(60);
        fs::File::options()
            .write(true)
            .open(&path)
            .expect("cache file should exist")
            .set_modified(stale)
            .expect("mtime should be settable");
        assert_eq!(cached_owned_games(account_id), None);

        let _ = fs::remove_file(&path);
        assert_eq!(cached_owned_games(account_id), None);
    }

    #[test]
    fn unknown_fields_of_every_wire_type_are_skipped() {
        let mut body = vec![0x0d, 1, 2, 3, 4];
        body.push(0x11);
        body.extend_from_slice(&[0; 8]);
        body.extend_from_slice(&[0x1a, 0x02, 0xff, 0xff]);
        body.extend(game(440, 5));
        assert_eq!(decode_app_ids(&body), Some(HashSet::from([440])));
    }
}
