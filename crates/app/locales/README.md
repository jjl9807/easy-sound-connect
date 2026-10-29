# Application translations

`en.yml` and `zh-CN.yml` contain application-owned display text. The library embeds these catalogs through `rust-i18n`, sharing the locale runtime used by GPUI Kit without requiring GUI dependencies for the probe.

Use stable semantic keys and complete messages with named `%{parameter}` placeholders. Keep both catalogs' keys and placeholder names aligned. GUI code uses `Language::tr` or `Language::tr_args`; shared presentation code uses `app::i18n`. Brand names, protocol identifiers and device-provided strings are values, not translation keys. Keep protocol eligibility and confirmation rules independent of translated text.

The saved language identifiers remain `en-US` and `zh-CN`; `Language::locale` maps English to the catalog and component locale `en`. Updating preferences refreshes windows and native tray menus. Stored activity messages retain their originating language rather than being translated again. A language added to the UI also needs an explicit supported preference and device display-language mapping.

Run `cargo test -p app --features gui` to check catalog coverage, placeholder consistency, English fallback and locale-independent prompt actions.
