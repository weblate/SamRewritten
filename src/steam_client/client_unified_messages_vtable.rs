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

use std::os::raw::{c_char, c_void};

#[repr(C)]
pub struct IClientUnifiedMessages {
    pub vtable: *const IClientUnifiedMessagesVTable,
}

#[repr(C)]
pub struct IClientUnifiedMessagesVTable {
    pub send_method: unsafe extern "C" fn(
        *mut IClientUnifiedMessages,
        *const c_char,
        *const c_void,
        u32,
        u64,
    ) -> u64,
    /// Raw: materialising an `EResult` our enum does not name would be undefined.
    pub get_method_response_info:
        unsafe extern "C" fn(*mut IClientUnifiedMessages, u64, *mut u32, *mut i32) -> bool,
    pub get_method_response_data:
        unsafe extern "C" fn(*mut IClientUnifiedMessages, u64, *mut c_void, u32, bool) -> bool,
    pub release_method: unsafe extern "C" fn(*mut IClientUnifiedMessages, u64) -> bool,
    pub send_notification: unsafe extern "C" fn(
        *mut IClientUnifiedMessages,
        *const c_char,
        *const c_void,
        u32,
    ) -> bool,
}

pub const INVALID_UNIFIED_MESSAGE_HANDLE: u64 = 0;
