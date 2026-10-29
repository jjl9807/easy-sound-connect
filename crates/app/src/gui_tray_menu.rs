//! Application actions for native tray menus.

use crate::{
    desktop_integration::{MenuAction, MenuEntry},
    preferences::Language,
};

pub(super) fn build(language: Language) -> Vec<MenuEntry> {
    vec![
        MenuEntry {
            label: language.tr("tray.open_main_window"),
            action: MenuAction::OpenMain,
        },
        MenuEntry {
            label: language.tr("tray.quit"),
            action: MenuAction::Quit,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_locale_rebuilds_labels_without_changing_menu_actions() {
        let english = build(Language::English);
        let chinese = build(Language::Chinese);
        assert_eq!(
            english.iter().map(|entry| entry.action).collect::<Vec<_>>(),
            chinese.iter().map(|entry| entry.action).collect::<Vec<_>>()
        );
        assert_eq!(
            english
                .iter()
                .map(|entry| entry.label.as_str())
                .collect::<Vec<_>>(),
            ["Open main window", "Quit"]
        );
        assert_eq!(
            chinese
                .iter()
                .map(|entry| entry.label.as_str())
                .collect::<Vec<_>>(),
            ["打开主窗口", "退出"]
        );
    }
}
