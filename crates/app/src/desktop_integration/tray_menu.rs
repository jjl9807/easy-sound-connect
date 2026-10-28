//! Application actions for native tray hosts.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    OpenMain,
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuEntry {
    pub label: String,
    pub action: MenuAction,
}
