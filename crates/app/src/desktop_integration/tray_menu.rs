//! Read-only headset information and application actions for native tray hosts.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    OpenMain,
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuEntry {
    Info(String),
    Action { label: String, action: MenuAction },
    Separator,
}
