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

//! The app list's left sidebar: profile card, library filters, sort order. The
//! widgets are views onto GSettings keys; `settings_bindings` re-runs the
//! filter and sorter when they change.

use crate::backend::steam_collections::{
    CollectionModel, FAVORITE_ID, HIDDEN_ID, UnsupportedReason,
};
use crate::gui_frontend::gobjects::online_state::{GOnlineState, online_state};
use crate::gui_frontend::i18n::{tr, tr_noop};
use crate::gui_frontend::profile_view::identity::Identity;
use crate::gui_frontend::widgets::shimmer_image::ShimmerImage;
use gtk::gio::Settings;
use gtk::glib;
use gtk::glib::clone;
use gtk::pango::EllipsizeMode;
use gtk::prelude::*;
use gtk::{
    Align, Box, Button, CheckButton, DropDown, Label, ListItem, Orientation, PolicyType,
    ProgressBar, ScrolledWindow, Separator, SignalListItemFactory, Spinner, StringList,
    StringObject,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub(super) const SIDEBAR_WIDTH: i32 = 232;
const AVATAR_SIZE: i32 = 48;

struct FilterSpec {
    key: &'static str,
    label: &'static str,
    needs_counts: bool,
    /// The checkbox shows the negation of the key. Only `filter-junk` uses it:
    /// junk is hidden by default, so a `Hide junk` box would sit permanently
    /// ticked for everyone.
    invert: bool,
}

const FILTERS: &[FilterSpec] = &[
    FilterSpec {
        key: "filter-hide-without-achievements",
        needs_counts: true,
        label: tr_noop("Hide with no achievements"),
        invert: false,
    },
    FilterSpec {
        key: "filter-hide-fully-unlocked",
        needs_counts: true,
        label: tr_noop("Hide at 100%"),
        invert: false,
    },
    FilterSpec {
        key: "filter-hide-no-unlocked",
        needs_counts: true,
        label: tr_noop("Hide at 0%"),
        invert: false,
    },
    FilterSpec {
        key: "filter-hide-never-launched",
        needs_counts: false,
        label: tr_noop("Hide never launched"),
        invert: false,
    },
    FilterSpec {
        key: "filter-only-idling",
        needs_counts: false,
        label: tr_noop("Only currently idling"),
        invert: false,
    },
    FilterSpec {
        key: "filter-hide-steam-hidden",
        needs_counts: false,
        label: tr_noop("Hide hidden in Steam"),
        invert: false,
    },
    FilterSpec {
        key: "filter-junk",
        needs_counts: false,
        label: tr_noop("Show junk"),
        invert: true,
    },
];

struct SortSpec {
    value: &'static str,
    label: &'static str,
    needs_counts: bool,
}

const SORT_MODES: &[SortSpec] = &[
    SortSpec {
        value: "app_id",
        label: tr_noop("App ID"),
        needs_counts: false,
    },
    SortSpec {
        value: "alphabetical",
        label: tr_noop("Name"),
        needs_counts: false,
    },
    SortSpec {
        value: "last_played",
        label: tr_noop("Last played"),
        needs_counts: false,
    },
    SortSpec {
        value: "playtime",
        label: tr_noop("Playtime"),
        needs_counts: false,
    },
    SortSpec {
        value: "completion",
        label: tr_noop("Completion"),
        needs_counts: true,
    },
    SortSpec {
        value: "remaining",
        label: tr_noop("Achievements left"),
        needs_counts: true,
    },
];

pub(super) fn drop_counts_dependent_settings(settings: &Settings) {
    if sort_needs_counts(settings.string("app-sort").as_str())
        && let Err(e) = settings.set_string("app-sort", "alphabetical")
    {
        eprintln!("[CLIENT] Error saving app-sort setting: {e:?}");
    }
    for spec in FILTERS.iter().filter(|spec| spec.needs_counts) {
        if settings.boolean(spec.key)
            && let Err(e) = settings.set_boolean(spec.key, false)
        {
            eprintln!("[CLIENT] Error saving {} setting: {e:?}", spec.key);
        }
    }
}

pub(super) fn sort_needs_counts(value: &str) -> bool {
    SORT_MODES
        .iter()
        .any(|spec| spec.value == value && spec.needs_counts)
}

pub(super) struct Sidebar {
    pub widget: Box,
    avatar: ShimmerImage,
    name_label: Label,
    profile_button: Button,
    loading_button: Button,
    loading_spinner: Spinner,
    loading_progress: ProgressBar,
    collection_dropdown: DropDown,
    collection_names: StringList,
    collection_entries: Rc<RefCell<Vec<CollectionEntry>>>,
    collection_rebuilding: Rc<Cell<bool>>,
}

struct CollectionEntry {
    id: String,
    unsupported: Option<UnsupportedReason>,
}

fn unsupported_tooltip(reason: UnsupportedReason) -> String {
    match reason {
        UnsupportedReason::SearchText => {
            tr("Filters on a search term, which SamRewritten cannot reproduce exactly.")
        }
        UnsupportedReason::UnknownFilter => {
            tr("Uses a Steam filter SamRewritten cannot reproduce exactly.")
        }
        UnsupportedReason::Unavailable => {
            tr("Needs information from Steam that could not be read. Refresh to try again.")
        }
        UnsupportedReason::Offline => {
            tr("Needs your friends' games, which Steam can only tell us when it is online.")
        }
    }
    .to_string()
}

fn collection_label(model: &CollectionModel) -> String {
    match model.id.as_str() {
        // Steam freezes a system collection's name in whichever language made it.
        FAVORITE_ID => tr("Favorites").to_string(),
        _ if model.name.is_empty() => model.id.clone(),
        _ => model.name.clone(),
    }
}

fn section_label(text: &str) -> Label {
    Label::builder()
        .label(text)
        .xalign(0.0)
        .margin_top(6)
        .css_classes(["heading", "dim-label"])
        .build()
}

fn build_profile_card(avatar: &ShimmerImage, name: &Label) -> Button {
    avatar.set_size_request(AVATAR_SIZE, AVATAR_SIZE);
    avatar.set_valign(Align::Center);

    let text_box = Box::builder()
        .orientation(Orientation::Vertical)
        .valign(Align::Center)
        .hexpand(true)
        .build();
    text_box.append(name);
    text_box.append(
        &Label::builder()
            .label(tr("View profile").as_str())
            .xalign(0.0)
            .css_classes(["dim-label", "caption"])
            .build(),
    );

    let content = Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(10)
        .build();
    content.append(avatar);
    content.append(&text_box);

    Button::builder()
        .css_classes(["flat"])
        .margin_top(6)
        .margin_bottom(6)
        .child(&content)
        .build()
}

fn wire_check(settings: &Settings, spec: &'static FilterSpec, check: &CheckButton) {
    let shown = |value: bool| if spec.invert { !value } else { value };

    check.set_active(shown(settings.boolean(spec.key)));
    check.connect_toggled(clone!(
        #[strong]
        settings,
        move |check| {
            let value = shown(check.is_active());
            if settings.boolean(spec.key) != value
                && let Err(e) = settings.set_boolean(spec.key, value)
            {
                eprintln!("[CLIENT] Error saving {} setting: {e:?}", spec.key);
            }
        }
    ));
    settings.connect_changed(
        Some(spec.key),
        clone!(
            #[weak]
            check,
            move |s, _| {
                let active = shown(s.boolean(spec.key));
                if check.is_active() != active {
                    check.set_active(active);
                }
            }
        ),
    );
}

pub(super) fn build_sidebar(settings: &Settings) -> Sidebar {
    let avatar = ShimmerImage::new();
    let name_label = Label::builder()
        .label(tr("Steam user").as_str())
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["heading"])
        .build();

    let content = Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(6)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build();

    let profile_button = build_profile_card(&avatar, &name_label);
    content.append(&profile_button);
    content.append(&Separator::new(Orientation::Horizontal));

    let offline_notice = Label::builder()
        .label(tr("Steam is offline. What needs its servers is turned off.").as_str())
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .margin_top(6)
        .css_classes(["caption", "warning"])
        .build();
    content.append(&offline_notice);

    let loading_spinner = Spinner::builder().valign(Align::Center).build();
    let loading_title = Label::builder()
        .label(tr("Fetching completion…").as_str())
        .xalign(0.0)
        .wrap(true)
        .css_classes(["caption"])
        .build();
    let loading_subtitle = Label::builder()
        .label(tr("Click to cancel").as_str())
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label", "caption"])
        .build();
    let loading_progress = ProgressBar::builder()
        .valign(Align::Center)
        .margin_top(3)
        .margin_bottom(3)
        .build();
    let loading_text = Box::builder()
        .orientation(Orientation::Vertical)
        .valign(Align::Center)
        .hexpand(true)
        .build();
    loading_text.append(&loading_title);
    loading_text.append(&loading_progress);
    loading_text.append(&loading_subtitle);
    let loading_content = Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .build();
    loading_content.append(&loading_spinner);
    loading_content.append(&loading_text);
    let loading_button = Button::builder()
        .margin_top(6)
        .css_classes(["flat"])
        .child(&loading_content)
        .build();
    content.append(&loading_button);

    content.append(&section_label(tr("Filters").as_str()));
    let mut counted: Vec<CheckButton> = Vec::new();
    for spec in FILTERS {
        let check = CheckButton::with_label(tr(spec.label).as_str());
        wire_check(settings, spec, &check);
        if spec.needs_counts {
            counted.push(check.clone());
        }
        content.append(&check);
    }

    content.append(&section_label(tr("Steam collection").as_str()));
    let collection_names = StringList::new(&[tr("All games").as_str()]);
    let collection_entries: Rc<RefCell<Vec<CollectionEntry>>> =
        Rc::new(RefCell::new(vec![CollectionEntry {
            id: String::new(),
            unsupported: None,
        }]));
    let collection_rebuilding: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let list_factory = SignalListItemFactory::new();
    list_factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<ListItem>() else {
            return;
        };
        let label = Label::builder()
            .xalign(0.0)
            .ellipsize(EllipsizeMode::End)
            .build();
        item.set_child(Some(&label));
    });
    list_factory.connect_bind(clone!(
        #[strong]
        collection_entries,
        move |_, item| {
            let Some(item) = item.downcast_ref::<ListItem>() else {
                return;
            };
            let Some(label) = item.child().and_downcast::<Label>() else {
                return;
            };
            let text = item
                .item()
                .and_downcast::<StringObject>()
                .map(|s| s.string())
                .unwrap_or_default();
            label.set_label(&text);

            let reason = collection_entries
                .borrow()
                .get(item.position() as usize)
                .and_then(|entry| entry.unsupported);
            let usable = reason.is_none();
            // Touch only the ListItem and our label: the row widget behind them
            // crashes the list on its next recycle.
            item.set_selectable(usable);
            item.set_activatable(usable);
            // Dimmed, not insensitive: an insensitive widget may never be picked
            // for the tooltip that carries the reason.
            if usable {
                label.remove_css_class("dim-label");
            } else {
                label.add_css_class("dim-label");
            }
            label.set_tooltip_text(reason.map(unsupported_tooltip).as_deref());
        }
    ));
    let collection_dropdown = DropDown::builder()
        .model(&collection_names)
        .list_factory(&list_factory)
        .build();
    content.append(&collection_dropdown);

    settings.connect_changed(
        Some("filter-collection"),
        clone!(
            #[weak]
            collection_dropdown,
            #[strong]
            collection_entries,
            move |s, _| {
                let id = s.string("filter-collection");
                let position = collection_entries
                    .borrow()
                    .iter()
                    .position(|entry| entry.id == id)
                    .unwrap_or(0) as u32;
                if collection_dropdown.selected() != position {
                    collection_dropdown.set_selected(position);
                }
            }
        ),
    );

    content.append(&Separator::new(Orientation::Horizontal));
    content.append(&section_label(tr("Sort by").as_str()));

    // Grouped radios cannot be bound the way the checkboxes are: write on
    // toggle, read back on change.
    let current = settings.string("app-sort");
    let mut first: Option<CheckButton> = None;
    let mut radios: Vec<(&str, CheckButton)> = Vec::with_capacity(SORT_MODES.len());
    for spec in SORT_MODES {
        let radio = CheckButton::with_label(tr(spec.label).as_str());
        match first {
            Some(ref group) => radio.set_group(Some(group)),
            None => first = Some(radio.clone()),
        }
        radio.set_active(current == spec.value);
        radio.connect_toggled(clone!(
            #[strong]
            settings,
            move |radio| {
                if radio.is_active()
                    && settings.string("app-sort") != spec.value
                    && let Err(e) = settings.set_string("app-sort", spec.value)
                {
                    eprintln!("[CLIENT] Error saving app-sort setting: {e:?}");
                }
            }
        ));
        if spec.needs_counts {
            counted.push(radio.clone());
        }
        content.append(&radio);
        radios.push((spec.value, radio));
    }
    let online = online_state();
    let profile_entry = profile_button.clone();
    let apply_online = clone!(
        #[strong]
        settings,
        move |state: &GOnlineState| {
            let online = state.online();
            offline_notice.set_visible(!online);
            profile_entry.set_sensitive(online);
            for control in &counted {
                control.set_sensitive(online);
            }
            if !online {
                drop_counts_dependent_settings(&settings);
            }
        }
    );
    settings.connect_changed(Some("app-sort"), move |s, _| {
        let value = s.string("app-sort");
        for (mode, radio) in &radios {
            if (value == *mode) != radio.is_active() {
                radio.set_active(value == *mode);
            }
        }
    });

    // After the radios listen: it can drop the sort, and nothing would move.
    apply_online(&online);
    online.connect_online_notify(apply_online);

    let reset_button = Button::builder()
        .label(tr("Reset filters").as_str())
        .halign(Align::Fill)
        .margin_top(12)
        .build();
    reset_button.connect_clicked(clone!(
        #[strong]
        settings,
        move |_| {
            for spec in FILTERS {
                settings.reset(spec.key);
            }
            settings.reset("filter-collection");
        }
    ));
    content.append(&reset_button);

    let scroller = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .propagate_natural_height(true)
        .vexpand(true)
        .child(&content)
        .build();

    // hexpand is pinned off on purpose: the profile card's text column sets it,
    // and it would propagate up, letting the sidebar take half the window.
    let widget = Box::builder()
        .orientation(Orientation::Horizontal)
        .width_request(SIDEBAR_WIDTH)
        .hexpand(false)
        .css_classes(["view"])
        .build();
    widget.append(&scroller);
    widget.append(&Separator::new(Orientation::Vertical));

    let sidebar = Sidebar {
        widget,
        avatar,
        name_label,
        profile_button,
        loading_button,
        loading_spinner,
        loading_progress,
        collection_dropdown,
        collection_names,
        collection_entries,
        collection_rebuilding,
    };
    sidebar.set_counts_loading(false, 0.0);
    sidebar
}

impl Sidebar {
    pub(super) fn set_counts_loading(&self, loading: bool, fraction: f64) {
        self.loading_button.set_visible(loading);
        if loading {
            self.loading_spinner.start();
            self.loading_progress.set_fraction(fraction.clamp(0.0, 1.0));
        } else {
            self.loading_spinner.stop();
        }
    }

    pub(super) fn connect_counts_load_clicked(&self, f: impl Fn() + 'static) {
        self.loading_button.connect_clicked(move |_| f());
    }

    pub(super) fn connect_profile_clicked(&self, f: impl Fn() + 'static) {
        self.profile_button.connect_clicked(move |_| f());
    }

    pub(super) fn set_collections(&self, models: &[CollectionModel], selected_id: &str) {
        let mut entries = vec![CollectionEntry {
            id: String::new(),
            unsupported: None,
        }];
        let mut labels = vec![tr("All games").to_string()];
        for model in models {
            if model.id == HIDDEN_ID {
                continue;
            }
            labels.push(collection_label(model));
            entries.push(CollectionEntry {
                id: model.id.clone(),
                unsupported: model.unsupported,
            });
        }
        let selected = entries
            .iter()
            .position(|entry| entry.id == selected_id)
            .unwrap_or(0);

        // Rows bind as they are added, and each splice hop looks like a click.
        *self.collection_entries.borrow_mut() = entries;
        self.collection_rebuilding.set(true);
        let additions: Vec<&str> = labels.iter().map(String::as_str).collect();
        self.collection_names
            .splice(0, self.collection_names.n_items(), &additions);
        self.collection_dropdown.set_selected(selected as u32);
        self.collection_rebuilding.set(false);
    }

    pub(super) fn connect_collection_selected(&self, f: impl Fn(String) + 'static) {
        let entries = Rc::clone(&self.collection_entries);
        let rebuilding = Rc::clone(&self.collection_rebuilding);
        let last_usable = RefCell::new(String::new());
        self.collection_dropdown
            .connect_selected_notify(move |drop| {
                let entry = entries
                    .borrow()
                    .get(drop.selected() as usize)
                    .map(|entry| (entry.id.clone(), entry.unsupported.is_some()));
                let Some((id, unsupported)) = entry else {
                    return;
                };
                if unsupported {
                    let back = entries
                        .borrow()
                        .iter()
                        .position(|entry| {
                            entry.id == *last_usable.borrow() && entry.unsupported.is_none()
                        })
                        .unwrap_or(0);
                    drop.set_selected(back as u32);
                    return;
                }
                *last_usable.borrow_mut() = id.clone();
                if !rebuilding.get() {
                    f(id);
                }
            });
    }

    pub(super) fn set_identity(&self, identity: &Identity) {
        let persona = identity.persona.borrow();
        if !persona.is_empty() {
            self.name_label.set_label(&persona);
        }
        if let Some(image) = identity.avatar.borrow().as_ref() {
            self.avatar
                .set_rgba(image.width as i32, image.height as i32, &image.rgba);
        }
    }
}
