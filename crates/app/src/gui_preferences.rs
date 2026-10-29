//! Language choices share the application locale identifiers and saved preferences.

use crate::{gui_shell, preferences::Language};
use gpui_kit::component::{
    IndexPath,
    select::{SelectEvent, SelectItem, SelectState},
};
use gpui_kit::{AppContext, Context, Entity, SharedString, Subscription, Window};

pub(super) type LanguageSelect = SelectState<Vec<Language>>;

impl SelectItem for Language {
    type Value = Language;

    fn value(&self) -> &Language {
        self
    }

    fn title(&self) -> SharedString {
        // Use each language's own name so the choice remains recognizable
        // regardless of the currently selected interface language.
        match self {
            Self::English => self.tr("ui.language_english"),
            Self::Chinese => self.tr("ui.language_chinese"),
        }
        .into()
    }
}

pub(super) fn language_select(
    selected: Language,
    window: &mut Window,
    cx: &mut Context<crate::Desktop>,
) -> (Entity<LanguageSelect>, Subscription) {
    let choices = vec![Language::English, Language::Chinese];
    let index = choices
        .iter()
        .position(|language| *language == selected)
        .map(IndexPath::new);
    let state = cx.new(|cx| SelectState::new(choices, index, window, cx));
    let subscription = cx.subscribe(&state, |_, _, event, cx| {
        if let SelectEvent::Confirm(Some(language)) = event {
            let language = *language;
            gui_shell::change_preferences(cx, move |preferences| preferences.language = language);
        }
    });
    (state, subscription)
}
