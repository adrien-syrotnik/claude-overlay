//! Heuristic detection of y/N-style confirmation prompts in free-form messages.

use regex::Regex;
use once_cell::sync::Lazy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YesNoFormat {
    /// Short form: user answers with "y" / "n"
    YN,
    /// Long form: user answers with "yes" / "no"
    YesNo,
}

impl YesNoFormat {
    pub fn yes_text(&self) -> &'static str {
        match self {
            YesNoFormat::YN => "y\n",
            YesNoFormat::YesNo => "yes\n",
        }
    }
    pub fn no_text(&self) -> &'static str {
        match self {
            YesNoFormat::YN => "n\n",
            YesNoFormat::YesNo => "no\n",
        }
    }
}

/// Detect y/N-style prompts. Returns Some(format) if match, None otherwise.
pub fn detect_yn_prompt(msg: &str) -> Option<YesNoFormat> {
    static YESNO: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)[\[\(]\s*yes\s*/\s*no\s*[\]\)]").unwrap()
    });
    static YN: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)[\[\(]\s*y\s*/\s*n\s*[\]\)]").unwrap()
    });
    if YESNO.is_match(msg) {
        Some(YesNoFormat::YesNo)
    } else if YN.is_match(msg) {
        Some(YesNoFormat::YN)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_y_n_uppercase_n_default() {
        assert_eq!(detect_yn_prompt("Proceed? [y/N]"), Some(YesNoFormat::YN));
    }

    #[test]
    fn detects_y_n_uppercase_y_default() {
        assert_eq!(detect_yn_prompt("Save [Y/n]?"), Some(YesNoFormat::YN));
    }

    #[test]
    fn detects_parentheses_form() {
        assert_eq!(detect_yn_prompt("Continue (y/n)"), Some(YesNoFormat::YN));
    }

    #[test]
    fn detects_yes_no_brackets() {
        assert_eq!(detect_yn_prompt("Confirm? [yes/no]"), Some(YesNoFormat::YesNo));
    }

    #[test]
    fn detects_yes_no_parentheses() {
        assert_eq!(detect_yn_prompt("Really? (yes/no)"), Some(YesNoFormat::YesNo));
    }

    #[test]
    fn ignores_unrelated_message() {
        assert_eq!(detect_yn_prompt("What is your name?"), None);
    }

    #[test]
    fn ignores_free_text_containing_yes() {
        assert_eq!(detect_yn_prompt("Add 'yes' to the shopping list"), None);
    }

    #[test]
    fn prefers_yes_no_over_yn_if_both_present() {
        assert_eq!(
            detect_yn_prompt("ambiguous [yes/no] and [y/n]"),
            Some(YesNoFormat::YesNo)
        );
    }

    #[test]
    fn yes_text_short_format() {
        assert_eq!(YesNoFormat::YN.yes_text(), "y\n");
    }

    #[test]
    fn no_text_long_format() {
        assert_eq!(YesNoFormat::YesNo.no_text(), "no\n");
    }

    #[test]
    fn whitespace_tolerant() {
        assert_eq!(detect_yn_prompt("prompt [ y / N ] ?"), Some(YesNoFormat::YN));
    }
}
