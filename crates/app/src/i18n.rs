//! Application translations using the same rust-i18n runtime as GPUI Kit.
//!
//! Catalogs are embedded at compile time. An explicit locale keeps background
//! work and probe output independent of changes to the component locale.

/// Translate an application key, falling back to English when unavailable.
pub fn translate(locale: &str, key: &str) -> String {
    rust_i18n::t!(key, locale = locale).into_owned()
}

/// Translate a complete message with named `%{name}` placeholders.
pub fn translate_args(locale: &str, key: &str, arguments: &[(&str, String)]) -> String {
    let template = translate(locale, key);
    let (names, values): (Vec<_>, Vec<_>) = arguments
        .iter()
        .map(|(name, value)| (*name, std::borrow::Cow::Borrowed(value.as_str())))
        .unzip();
    rust_i18n::replace_patterns_cow(&template, &names, &values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn placeholders(text: &str) -> BTreeSet<&str> {
        text.split("%{")
            .skip(1)
            .map(|part| part.split_once('}').expect("closed placeholder").0)
            .collect()
    }

    #[test]
    fn catalogs_have_matching_keys_and_parameters_and_are_embedded() {
        let english: BTreeMap<String, String> =
            serde_yaml::from_str(include_str!("../locales/en.yml")).unwrap();
        let chinese: BTreeMap<String, String> =
            serde_yaml::from_str(include_str!("../locales/zh-CN.yml")).unwrap();
        assert!(!english.is_empty());
        assert_eq!(
            english.keys().collect::<Vec<_>>(),
            chinese.keys().collect::<Vec<_>>()
        );
        for (key, value) in &english {
            assert!(!value.is_empty(), "empty English translation: {key}");
            assert!(!chinese[key].is_empty(), "empty Chinese translation: {key}");
            assert_eq!(placeholders(value), placeholders(&chinese[key]), "{key}");
            assert_eq!(translate("en", key), *value, "{key}");
            assert_eq!(translate("zh-CN", key), chinese[key], "{key}");
            assert_eq!(translate("unsupported-locale", key), *value, "{key}");
        }
    }

    #[test]
    fn complete_messages_interpolate_values_without_translating_device_data() {
        let arguments = [
            ("label", "WF-1000XM6 %{error}".to_owned()),
            ("error", "0x76".to_owned()),
        ];
        assert_eq!(
            translate_args("en", "session.operation_failed", &arguments),
            "Could not confirm WF-1000XM6 %{error}: 0x76"
        );
        assert_eq!(
            translate_args("zh-CN", "session.operation_failed", &arguments),
            "无法确认WF-1000XM6 %{error}：0x76"
        );
    }
}
