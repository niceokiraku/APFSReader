//! Interface language: Japanese on a Japanese Windows, English otherwise.
//! `APFSREADER_LANG=en` or `ja` overrides.

use std::sync::OnceLock;

pub fn japanese() -> bool {
    static JA: OnceLock<bool> = OnceLock::new();
    *JA.get_or_init(|| match std::env::var("APFSREADER_LANG").ok().as_deref() {
        Some("ja") => true,
        Some("en") => false,
        _ => {
            use windows::Win32::Globalization::GetUserDefaultUILanguage;
            // The low ten bits are the primary language; 0x11 is Japanese.
            let id = unsafe { GetUserDefaultUILanguage() };
            id & 0x3FF == 0x11
        }
    })
}

/// Pick the Japanese or English text.
pub fn t(ja: &'static str, en: &'static str) -> &'static str {
    if japanese() {
        ja
    } else {
        en
    }
}
