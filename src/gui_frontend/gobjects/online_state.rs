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

use crate::gui_frontend::request::{GetSteamOnline, Request};
use glib::Object;
use gtk::gio::spawn_blocking;
use gtk::glib;
use gtk::glib::MainContext;
use std::cell::OnceCell;

glib::wrapper! {
    pub struct GOnlineState(ObjectSubclass<imp::GOnlineState>);
}

impl Default for GOnlineState {
    fn default() -> Self {
        Object::new()
    }
}

thread_local! {
    static STATE: OnceCell<GOnlineState> = const { OnceCell::new() };
}

pub fn online_state() -> GOnlineState {
    STATE.with(|cell| cell.get_or_init(GOnlineState::default).clone())
}

/// Asked per library refresh, not polled: nothing else notices Steam restarting.
pub fn probe() {
    let handle = spawn_blocking(|| GetSteamOnline.request());
    MainContext::default().spawn_local(async move {
        let Ok(Ok(online)) = handle.await else {
            return;
        };
        let state = online_state();
        if state.online() != online {
            state.set_online(online);
        }
    });
}

mod imp {
    use glib::Properties;
    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;
    use std::cell::Cell;

    #[derive(Properties)]
    #[properties(wrapper_type = super::GOnlineState)]
    pub struct GOnlineState {
        #[property(get, set, default = true)]
        online: Cell<bool>,
    }

    impl Default for GOnlineState {
        fn default() -> Self {
            Self {
                online: Cell::new(true),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GOnlineState {
        const NAME: &'static str = "GOnlineState";
        type Type = super::GOnlineState;
    }

    #[glib::derived_properties]
    impl ObjectImpl for GOnlineState {}
}
