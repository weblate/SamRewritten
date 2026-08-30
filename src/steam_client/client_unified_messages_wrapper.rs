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

use crate::steam_client::client_engine_wrapper::ClientEngineInner;
use crate::steam_client::client_unified_messages_vtable::{
    IClientUnifiedMessages, IClientUnifiedMessagesVTable, INVALID_UNIFIED_MESSAGE_HANDLE,
};
use crate::steam_client::steamworks_types::EResult;
use crate::steam_client::wrapper_types::SteamClientError;
use std::ffi::CString;
use std::os::raw::c_void;
use std::rc::Rc;
use std::time::{Duration, Instant};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const MAX_RESPONSE_BYTES: u32 = 16 * 1024 * 1024;

pub struct ClientUnifiedMessages {
    ptr: *mut IClientUnifiedMessages,
    #[allow(dead_code)]
    engine: Rc<ClientEngineInner>,
}

impl ClientUnifiedMessages {
    pub unsafe fn from_raw(
        ptr: *mut IClientUnifiedMessages,
        engine: Rc<ClientEngineInner>,
    ) -> Self {
        Self { ptr, engine }
    }

    fn vtable(&self) -> Result<&IClientUnifiedMessagesVTable, SteamClientError> {
        unsafe { (*self.ptr).vtable.as_ref() }.ok_or(SteamClientError::NullVtable)
    }

    /// The handle owns a buffer inside the client, so every exit path releases it.
    pub fn call(
        &self,
        method: &str,
        request: &[u8],
        deadline: Instant,
    ) -> Result<Vec<u8>, SteamClientError> {
        let vt = self.vtable()?;
        let name = CString::new(method).map_err(|_| {
            SteamClientError::MethodCallFailed(format!("{method} is not a valid method name"))
        })?;
        let handle = unsafe {
            (vt.send_method)(
                self.ptr,
                name.as_ptr(),
                request.as_ptr() as *const c_void,
                request.len() as u32,
                0,
            )
        };
        if handle == INVALID_UNIFIED_MESSAGE_HANDLE {
            return Err(SteamClientError::MethodCallFailed(format!(
                "{method} was not accepted"
            )));
        }

        let result = self.await_response(vt, handle, method, deadline);
        unsafe { (vt.release_method)(self.ptr, handle) };
        result
    }

    fn await_response(
        &self,
        vt: &IClientUnifiedMessagesVTable,
        handle: u64,
        method: &str,
        deadline: Instant,
    ) -> Result<Vec<u8>, SteamClientError> {
        let deadline = deadline.min(Instant::now() + RESPONSE_TIMEOUT);
        loop {
            let mut size: u32 = 0;
            let mut result: i32 = 0;
            if unsafe { (vt.get_method_response_info)(self.ptr, handle, &mut size, &mut result) } {
                if result != EResult::k_EResultOK as i32 {
                    return Err(SteamClientError::MethodResultFailed(
                        method.to_owned(),
                        result,
                    ));
                }
                if size == 0 {
                    return Ok(Vec::new());
                }
                if size > MAX_RESPONSE_BYTES {
                    return Err(SteamClientError::MethodCallFailed(format!(
                        "{method} answered {size} bytes"
                    )));
                }
                let mut buffer = vec![0u8; size as usize];
                let read = unsafe {
                    (vt.get_method_response_data)(
                        self.ptr,
                        handle,
                        buffer.as_mut_ptr() as *mut c_void,
                        size,
                        false,
                    )
                };
                return if read {
                    Ok(buffer)
                } else {
                    Err(SteamClientError::MethodCallFailed(format!(
                        "{method} answered nothing"
                    )))
                };
            }
            if Instant::now() >= deadline {
                return Err(SteamClientError::MethodCallFailed(format!(
                    "{method} timed out"
                )));
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}
