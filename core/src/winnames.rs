//! Translating between volume names and what Windows can show; shared by the
//! mount front end and the extract command.

use unicode_normalization::UnicodeNormalization;

/// Seconds between 1601-01-01 (FILETIME) and 1970-01-01 (Unix).
const FILETIME_EPOCH_GAP: i64 = 11_644_473_600;

/// Unix seconds to a Windows FILETIME (100 ns ticks since 1601). 0 stays 0.
pub fn filetime(unix_secs: i64) -> u64 {
    if unix_secs <= 0 {
        return 0;
    }
    ((unix_secs + FILETIME_EPOCH_GAP) as u64).saturating_mul(10_000_000)
}

/// A name Windows will accept. macOS volumes may contain characters Windows
/// reserves (`:` is common, since HFS+ stores a Finder "/" as ":"). Each is
/// replaced by its full-width look-alike so the name stays readable and
/// distinct; control characters and a trailing dot or space become `_`.
pub fn windows_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '<' => '＜',
            '>' => '＞',
            ':' => '：',
            '"' => '＂',
            '/' => '／',
            '\\' => '＼',
            '|' => '｜',
            '?' => '？',
            '*' => '＊',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    if out.ends_with('.') || out.ends_with(' ') {
        out.pop();
        out.push('_');
    }
    if is_reserved_device_name(&out) {
        out.insert(0, '_');
    }
    out
}

/// `CON`, `NUL`, `COM1` and friends cannot be created as files, with or
/// without an extension.
fn is_reserved_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end().to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}

/// Windows compares names case-insensitively and does not distinguish
/// composed from decomposed forms.
pub fn fold(name: &str) -> String {
    name.nfc().collect::<String>().to_lowercase()
}

/// `shown` rewritten into the Unicode normalisation form `requested` was
/// written in. WinFsp expects the normalised name it gets back to equal the
/// requested one apart from case, so a request spelled in decomposed form
/// (as macOS and HFS+ write names) must get a decomposed name back even though
/// listings present composed ones.
pub fn same_form_as(requested: &str, shown: &str) -> String {
    let composed: String = requested.nfc().collect();
    if requested != composed && requested == requested.nfd().collect::<String>() {
        shown.nfd().collect()
    } else {
        shown.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalised_name_keeps_the_requested_form() {
        let nfd = "u\u{308}ber"; // decomposed request
        assert_eq!(same_form_as(nfd, "Über"), "U\u{308}ber");
        // A composed request keeps the composed spelling (case still the volume's).
        assert_eq!(same_form_as("\u{fc}ber", "Über"), "Über");
        // Plain ASCII is untouched.
        assert_eq!(same_form_as("readme", "README"), "README");
    }

    #[test]
    fn reserved_characters_are_replaced_not_dropped() {
        assert_eq!(windows_name("a:b"), "a：b");
        assert_eq!(windows_name("what?*"), "what？＊");
        assert_eq!(windows_name("tab\there"), "tab_here");
        assert_eq!(windows_name("dots.."), "dots._");
        assert_eq!(windows_name("plain name.txt"), "plain name.txt");
    }

    #[test]
    fn reserved_device_names_are_prefixed() {
        assert_eq!(windows_name("CON"), "_CON");
        assert_eq!(windows_name("nul.txt"), "_nul.txt");
        assert_eq!(windows_name("Com3"), "_Com3");
        assert_eq!(windows_name("console"), "console");
        assert_eq!(windows_name("COM10"), "COM10");
    }

    #[test]
    fn distinct_names_stay_distinct() {
        assert_ne!(windows_name("a:b"), windows_name("a_b"));
    }

    #[test]
    fn fold_ignores_case_and_normalisation() {
        assert_eq!(fold("Ünï"), fold("U\u{308}ni\u{308}"));
        assert_eq!(fold("README.TXT"), fold("readme.txt"));
    }

    #[test]
    fn filetime_conversion() {
        assert_eq!(filetime(0), 0);
        assert_eq!(filetime(-5), 0);
        // 2000-01-01T00:00:00Z
        assert_eq!(filetime(946_684_800), 125_911_584_000_000_000);
    }
}
