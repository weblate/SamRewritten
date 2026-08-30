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

// CClientUserMap vtable has 259 slots in current Steam. We only use slot 182
// (GetSubscribedApps), so the leading 182 slots are kept as opaque pointers
// to preserve offsets.

use crate::steam_client::steamworks_types::AppId_t;
use std::os::raw::c_void;

#[repr(C)]
pub struct IClientUser {
    pub vtable: *const IClientUserVTable,
}

#[repr(C)]
pub struct IClientUserVTable {
    pub _vt_pre_get_subscribed_apps: [*const c_void; 182],
    /// The bool adds expired and just-granted licences: `true` answers a
    /// superset, it is not an ownership filter.
    pub get_subscribed_apps: unsafe extern "C" fn(*mut IClientUser, *mut AppId_t, u32, bool) -> u32,
}
