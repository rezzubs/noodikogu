//! Text normalization for searchable fields.
//!
//! Used to compute `value_normalized`-style columns (currently just
//! `titles.value_normalized`; other normalized-text columns may reuse this
//! later). See `docs/decisions/0005-hand-rolled-search.md` for why this
//! particular algorithm was chosen.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Normalizes `input` for case- and diacritic-insensitive search matching:
/// Unicode NFD decomposition, removal of combining diacritic marks
/// (Unicode general category Mn), NFC recomposition, then Unicode-aware
/// lowercasing.
///
/// Scripts that don't decompose into a base letter plus a combining mark
/// (e.g. CJK ideographs, most Cyrillic letters) are left untouched - not
/// because of script-specific logic, but as a side effect of there being
/// no combining mark to strip.
pub(crate) fn normalize_text(input: &str) -> String {
    input
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .nfc()
        .collect::<String>()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_latin_diacritics_and_lowercases() {
        assert_eq!(normalize_text("Müller"), "muller");
    }

    #[test]
    fn strips_estonian_diacritics() {
        assert_eq!(normalize_text("Õnnistus"), "onnistus");
    }

    #[test]
    fn lowercases_diacritic_free_text_unchanged_otherwise() {
        assert_eq!(normalize_text("Ave Maria"), "ave maria");
    }

    #[test]
    fn lowercases_cyrillic_without_altering_letters() {
        assert_eq!(normalize_text("Владимир"), "владимир");
    }

    #[test]
    fn leaves_cjk_untouched() {
        assert_eq!(normalize_text("北京"), "北京");
    }
}
