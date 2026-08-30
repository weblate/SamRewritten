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

//! `appinfo.vdf`: a header, a length-prefixed blob per app, then the string table.

use crate::dev_println;
use crate::utils::ipc_types::SamError;
use std::collections::{HashMap, HashSet};
use std::path::Path;

const MAGIC_V29: u32 = 0x0756_4429;

/// infoState, lastUpdated, token, text hash, changeNumber, binary hash.
const APP_HEADER_LEN: usize = 4 + 4 + 8 + 20 + 4 + 20;

/// Valve's blobs nest about four deep; skipping recurses, so runaway nesting would
/// blow the stack.
const MAX_DEPTH: u8 = 32;

#[derive(Debug, Default, Clone)]
pub struct AppInfo {
    pub controller_support: String,
    pub languages: HashSet<u32>,
    pub store_tags: HashSet<u32>,
    pub categories: HashSet<u32>,
    pub deck_compat: i32,
    pub steamos_compat: i32,
    pub steam_machine_compat: i32,
    pub mastersub_appid: u32,
}

impl AppInfo {
    pub fn has_category(&self, id: u32) -> bool {
        self.categories.contains(&id)
    }

    pub fn has_store_tag(&self, id: u32) -> bool {
        self.store_tags.contains(&id)
    }

    pub fn has_language(&self, index: u32) -> bool {
        self.languages.contains(&index)
    }
}

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
    depth: u8,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8], pos: usize) -> Self {
        Self { buf, pos, depth: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn cstr(&mut self) -> Option<String> {
        let rest = self.buf.get(self.pos..)?;
        let len = rest.iter().position(|b| *b == 0)?;
        let s = String::from_utf8_lossy(&rest[..len]).into_owned();
        self.pos += len + 1;
        Some(s)
    }

    fn skip_value(&mut self, kv_type: u8) -> Option<()> {
        match kv_type {
            0 => self.skip_block(),
            1 | 5 => self.cstr().map(|_| ()),
            2 | 3 | 4 | 6 => self.take(4).map(|_| ()),
            7 => self.take(8).map(|_| ()),
            _ => None,
        }
    }

    fn skip_block(&mut self) -> Option<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return None;
        }
        loop {
            let kv_type = self.u8()?;
            if kv_type == 8 {
                self.depth -= 1;
                return Some(());
            }
            self.u32()?;
            self.skip_value(kv_type)?;
        }
    }

    fn key<'s>(&mut self, strings: &'s [String]) -> Option<&'s str> {
        strings.get(self.u32()? as usize).map(String::as_str)
    }
}

fn read_string_table(buf: &[u8], offset: i64) -> Option<Vec<String>> {
    let mut cur = Cursor::new(buf, usize::try_from(offset).ok()?);
    // From a file-supplied offset, so it never sizes an allocation: a corrupt one
    // would abort the process uncatchably.
    let count = cur.u32()? as usize;
    if count > buf.len().saturating_sub(cur.pos) {
        return None;
    }
    let mut out = Vec::with_capacity(count.min(16 * 1024));
    for _ in 0..count {
        out.push(cur.cstr()?);
    }
    Some(out)
}

fn read_int_values(cur: &mut Cursor, out: &mut HashSet<u32>) -> Option<()> {
    loop {
        let kv_type = cur.u8()?;
        if kv_type == 8 {
            return Some(());
        }
        cur.u32()?;
        match kv_type {
            2 => {
                out.insert(cur.i32()? as u32);
            }
            7 => {
                let raw = cur.take(8)?;
                out.insert(u64::from_le_bytes(raw.try_into().ok()?) as u32);
            }
            other => cur.skip_value(other)?,
        }
    }
}

const LANGUAGE_ORDER: [&str; 32] = [
    "english",
    "german",
    "french",
    "italian",
    "koreana",
    "spanish",
    "schinese",
    "tchinese",
    "russian",
    "thai",
    "japanese",
    "portuguese",
    "polish",
    "danish",
    "dutch",
    "finnish",
    "norwegian",
    "swedish",
    "hungarian",
    "czech",
    "romanian",
    "turkish",
    "brazilian",
    "bulgarian",
    "greek",
    "arabic",
    "ukrainian",
    "latam",
    "vietnamese",
    "sc_schinese",
    "indonesian",
    "malay",
];

fn language_index(name: &str) -> Option<u32> {
    // Steam accepts either spelling for Korean.
    let name = if name == "korean" { "koreana" } else { name };
    LANGUAGE_ORDER
        .iter()
        .position(|known| *known == name)
        .map(|index| index as u32)
}

fn read_supported_languages(
    cur: &mut Cursor,
    strings: &[String],
    out: &mut HashSet<u32>,
) -> Option<()> {
    loop {
        let kv_type = cur.u8()?;
        if kv_type == 8 {
            return Some(());
        }
        let key = cur.key(strings)?;
        let index = language_index(key);
        if kv_type != 0 {
            cur.skip_value(kv_type)?;
            continue;
        }
        if read_language_entry(cur, strings)?
            && let Some(index) = index
        {
            out.insert(index);
        }
    }
}

fn read_language_entry(cur: &mut Cursor, strings: &[String]) -> Option<bool> {
    let mut supported = false;
    loop {
        let kv_type = cur.u8()?;
        if kv_type == 8 {
            return Some(supported);
        }
        let key = cur.key(strings)?;
        match (kv_type, key) {
            (1, "supported") => supported = cur.cstr()? == "true",
            (2, "supported") => supported = cur.i32()? != 0,
            (other, _) => cur.skip_value(other)?,
        }
    }
}

fn read_category_keys(cur: &mut Cursor, strings: &[String], out: &mut HashSet<u32>) -> Option<()> {
    loop {
        let kv_type = cur.u8()?;
        if kv_type == 8 {
            return Some(());
        }
        let key = cur.key(strings)?;
        if let Some(id) = key.strip_prefix("category_").and_then(|n| n.parse().ok()) {
            out.insert(id);
        }
        cur.skip_value(kv_type)?;
    }
}

fn read_deck_compat(cur: &mut Cursor, strings: &[String], out: &mut AppInfo) -> Option<()> {
    loop {
        let kv_type = cur.u8()?;
        if kv_type == 8 {
            return Some(());
        }
        let key = cur.key(strings)?;
        let slot = match key {
            "category" => Some(&mut out.deck_compat),
            "steamos_compatibility" => Some(&mut out.steamos_compat),
            "steam_machine_compatibility" => Some(&mut out.steam_machine_compat),
            _ => None,
        };
        match (kv_type, slot) {
            (2, Some(slot)) => *slot = cur.i32()?,
            (other, _) => cur.skip_value(other)?,
        }
    }
}

fn read_common(cur: &mut Cursor, strings: &[String], out: &mut AppInfo) -> Option<()> {
    loop {
        let kv_type = cur.u8()?;
        if kv_type == 8 {
            return Some(());
        }
        let key = cur.key(strings)?;
        match (kv_type, key) {
            (1, "controller_support") => out.controller_support = cur.cstr()?,
            (2, "mastersubs_granting_app") => out.mastersub_appid = cur.i32()? as u32,
            (0, "store_tags") => read_int_values(cur, &mut out.store_tags)?,
            (0, "category") => read_category_keys(cur, strings, &mut out.categories)?,
            (0, "supported_languages") => {
                read_supported_languages(cur, strings, &mut out.languages)?
            }
            (0, "steam_deck_compatibility") => read_deck_compat(cur, strings, out)?,
            (other, _) => cur.skip_value(other)?,
        }
    }
}

/// Steam nests `common` under an `appinfo` root on some apps, top level on others.
fn read_app(cur: &mut Cursor, strings: &[String], out: &mut AppInfo, depth: u8) -> Option<()> {
    loop {
        let kv_type = cur.u8()?;
        if kv_type == 8 {
            return Some(());
        }
        let key = cur.key(strings)?;
        if kv_type != 0 {
            cur.skip_value(kv_type)?;
            continue;
        }
        match key {
            "common" => read_common(cur, strings, out)?,
            "appinfo" if depth == 0 => read_app(cur, strings, out, depth + 1)?,
            _ => cur.skip_block()?,
        }
    }
}

pub fn read(path: &Path, wanted: &HashSet<u32>) -> Result<HashMap<u32, AppInfo>, SamError> {
    let buf = std::fs::read(path).map_err(|e| {
        dev_println!("ORCH", "Failed to read {}: {e}", path.display());
        SamError::UnknownError
    })?;

    let mut head = Cursor::new(&buf, 0);
    let magic = head.u32().ok_or(SamError::UnknownError)?;
    head.u32().ok_or(SamError::UnknownError)?;
    if magic != MAGIC_V29 {
        dev_println!("ORCH", "Unsupported appinfo.vdf magic {magic:#x}");
        return Err(SamError::UnknownError);
    }
    let table_offset = head.i64().ok_or(SamError::UnknownError)?;
    let strings = read_string_table(&buf, table_offset).ok_or(SamError::UnknownError)?;

    let mut out = HashMap::with_capacity(wanted.len());
    let mut pos = head.pos;
    loop {
        let mut header = Cursor::new(&buf, pos);
        let Some(app_id) = header.u32() else { break };
        if app_id == 0 {
            break;
        }
        let Some(size) = header.u32() else { break };
        let Some(next) = pos
            .checked_add(8)
            .and_then(|p| p.checked_add(size as usize))
        else {
            break;
        };
        if next > buf.len() {
            break;
        }
        if wanted.contains(&app_id) {
            let mut info = AppInfo::default();
            let mut body = Cursor::new(&buf[..next], pos + 8 + APP_HEADER_LEN);
            if read_app(&mut body, &strings, &mut info, 0).is_some() {
                out.insert(app_id, info);
            } else {
                dev_println!("ORCH", "Malformed appinfo entry for app {app_id}");
            }
        }
        pos = next;
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Builder {
        strings: Vec<String>,
        body: Vec<u8>,
    }

    impl Builder {
        fn new() -> Self {
            Self {
                strings: Vec::new(),
                body: Vec::new(),
            }
        }

        fn key(&mut self, name: &str) -> u32 {
            if let Some(i) = self.strings.iter().position(|s| s == name) {
                return i as u32;
            }
            self.strings.push(name.to_string());
            self.strings.len() as u32 - 1
        }

        fn open(&mut self, name: &str) -> &mut Self {
            let index = self.key(name);
            self.body.push(0);
            self.body.extend_from_slice(&index.to_le_bytes());
            self
        }

        fn close(&mut self) -> &mut Self {
            self.body.push(8);
            self
        }

        fn int(&mut self, name: &str, value: i32) -> &mut Self {
            let index = self.key(name);
            self.body.push(2);
            self.body.extend_from_slice(&index.to_le_bytes());
            self.body.extend_from_slice(&value.to_le_bytes());
            self
        }

        fn string(&mut self, name: &str, value: &str) -> &mut Self {
            let index = self.key(name);
            self.body.push(1);
            self.body.extend_from_slice(&index.to_le_bytes());
            self.body.extend_from_slice(value.as_bytes());
            self.body.push(0);
            self
        }

        fn finish(&self, app_ids: &[u32]) -> Vec<u8> {
            let mut out = Vec::new();
            out.extend_from_slice(&MAGIC_V29.to_le_bytes());
            out.extend_from_slice(&1u32.to_le_bytes());
            let offset_at = out.len();
            out.extend_from_slice(&0i64.to_le_bytes());
            for app_id in app_ids {
                out.extend_from_slice(&app_id.to_le_bytes());
                let size = APP_HEADER_LEN + self.body.len() + 1;
                out.extend_from_slice(&(size as u32).to_le_bytes());
                out.extend(std::iter::repeat_n(0u8, APP_HEADER_LEN));
                out.extend_from_slice(&self.body);
                out.push(8);
            }
            out.extend_from_slice(&0u32.to_le_bytes());

            let table = out.len() as i64;
            out[offset_at..offset_at + 8].copy_from_slice(&table.to_le_bytes());
            out.extend_from_slice(&(self.strings.len() as u32).to_le_bytes());
            for s in &self.strings {
                out.extend_from_slice(s.as_bytes());
                out.push(0);
            }
            out
        }
    }

    fn sample() -> Vec<u8> {
        let mut b = Builder::new();
        b.open("appinfo");
        b.open("common");
        b.string("controller_support", "full");
        b.int("mastersubs_granting_app", 1_289_670);
        b.open("store_tags").int("0", 19).int("1", 492).close();
        b.open("category")
            .int("category_22", 1)
            .int("category_29", 1)
            .close();
        b.open("steam_deck_compatibility")
            .int("category", 3)
            .int("steamos_compatibility", 2)
            .close();
        b.close();
        b.open("extended").string("developer", "irrelevant").close();
        b.close();
        b.finish(&[240, 730])
    }

    fn read_bytes(bytes: &[u8], wanted: &[u32]) -> Result<HashMap<u32, AppInfo>, SamError> {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "sam_test_appinfo_{}_{}.vdf",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&path, bytes).expect("fixture should write");
        let result = read(&path, &wanted.iter().copied().collect());
        std::fs::remove_file(&path).ok();
        result
    }

    #[test]
    fn common_fields_are_read_and_other_apps_skipped_whole() {
        let apps = read_bytes(&sample(), &[730]).expect("fixture should parse");
        assert_eq!(apps.len(), 1, "only the wanted app is kept");
        let info = &apps[&730];
        assert_eq!(info.controller_support, "full");
        assert_eq!(info.mastersub_appid, 1_289_670);
        assert!(info.has_store_tag(19) && info.has_store_tag(492));
        assert!(info.has_category(22) && info.has_category(29));
        assert!(!info.has_category(1));
        assert_eq!(info.deck_compat, 3);
        assert_eq!(info.steamos_compat, 2);
        assert_eq!(info.steam_machine_compat, 0);
    }

    #[test]
    fn a_truncated_file_fails_instead_of_returning_half_an_app() {
        let full = sample();
        let cut = read_bytes(&full[..full.len() / 2], &[240]);
        assert!(cut.is_err(), "a truncated string table has to be an error");
    }

    #[test]
    fn a_wrong_magic_is_refused() {
        let mut bytes = sample();
        bytes[0] ^= 0xff;
        assert!(read_bytes(&bytes, &[240]).is_err());
    }

    #[test]
    fn an_absurd_string_table_count_is_refused_rather_than_allocated() {
        let mut bytes = sample();
        bytes[8..16].copy_from_slice(&0i64.to_le_bytes());
        assert!(read_bytes(&bytes, &[240]).is_err());
    }

    #[test]
    fn runaway_nesting_is_refused_rather_than_followed() {
        let mut b = Builder::new();
        let index = b.key("nested");
        b.open("appinfo");
        for _ in 0..(MAX_DEPTH as usize + 200) {
            b.body.push(0);
            b.body.extend_from_slice(&index.to_le_bytes());
        }
        let bytes = b.finish(&[240]);
        let apps = read_bytes(&bytes, &[240]).expect("the walk itself must not fail");
        assert!(apps.is_empty(), "the malformed app is dropped, not parsed");
    }
}
