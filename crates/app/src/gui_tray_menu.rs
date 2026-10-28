//! Application actions for native tray menus.

use crate::{
    desktop_integration::{MenuAction, MenuEntry},
    preferences::Language,
};

pub(super) fn build(language: Language) -> Vec<MenuEntry> {
    vec![
        MenuEntry {
            label: language.text("Open main window", "打开主窗口").into(),
            action: MenuAction::OpenMain,
        },
        MenuEntry {
            label: language.text("Quit", "退出").into(),
            action: MenuAction::Quit,
        },
    ]
}
