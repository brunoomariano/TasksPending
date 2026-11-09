//! The strip under the hero that switches between the Activity area and
//! the Board.

use std::rc::Rc;

use gtk4 as gtk;
use gtk4::prelude::*;

use super::Ctx;
use crate::keymap::{Command as KeyCommand, hinted};
use crate::present::{Tab, TabView, tabs};

/// The tab buttons, updated in place.
pub struct Tabs {
    pub root: gtk::Box,
    buttons: Vec<(Tab, gtk::ToggleButton, gtk::Label)>,
}

/// Builds the strip with a button per tab; clicking one shows that tab.
pub fn build(ctx: &Rc<Ctx>) -> Tabs {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    root.add_css_class("tabs");
    let mut buttons = Vec::new();
    let mut first: Option<gtk::ToggleButton> = None;
    for view in tabs(None) {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        content.append(&gtk::Label::new(Some(view.label)));
        let count = gtk::Label::builder().visible(false).build();
        count.add_css_class("count");
        content.append(&count);
        let key = match view.tab {
            Tab::Activity => KeyCommand::ActivityTab,
            Tab::Board => KeyCommand::BoardTab,
        };
        let button = gtk::ToggleButton::builder()
            .child(&content)
            .tooltip_text(hinted(view.label, key))
            .build();
        button.add_css_class("tab");
        // One of the two is always down: the group keeps the one pressed
        // from being released by a second click.
        button.set_group(first.as_ref());
        first.get_or_insert_with(|| button.clone());
        {
            let ctx = ctx.clone();
            let tab = view.tab;
            button.connect_toggled(move |button| {
                if button.is_active() {
                    ctx.show_tab(tab);
                }
            });
        }
        root.append(&button);
        buttons.push((view.tab, button, count));
    }
    Tabs { root, buttons }
}

/// Presses the button of `current` and writes the counts of `views`.
pub fn update(tabs: &Tabs, views: &[TabView; 2], current: Tab) {
    for (tab, button, count) in &tabs.buttons {
        if let Some(view) = views.iter().find(|view| view.tab == *tab) {
            count.set_label(&view.count.map(|n| n.to_string()).unwrap_or_default());
            count.set_visible(view.count.is_some());
        }
        // Only when it differs: pressing it again would show the tab again.
        if button.is_active() != (*tab == current) {
            button.set_active(*tab == current);
        }
    }
}
