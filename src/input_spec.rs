//! Input specification for overlay rows. Describes what kind of input UI
//! the user needs to provide and how the answer is delivered back.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum YesNoFormat {
    /// Short form: "y" / "n"
    YN,
    /// Long form: "yes" / "no"
    YesNo,
    /// Claude Code's native picker — "1\n" / Esc.
    Numeric,
}

impl YesNoFormat {
    pub fn yes_text(&self) -> &'static str {
        match self {
            YesNoFormat::YN => "y\n",
            YesNoFormat::YesNo => "yes\n",
            YesNoFormat::Numeric => "1\n",
        }
    }
    pub fn no_text(&self) -> &'static str {
        match self {
            YesNoFormat::YN => "n\n",
            YesNoFormat::YesNo => "no\n",
            YesNoFormat::Numeric => "\x1b",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Choice {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    /// SendInput keystrokes to the source terminal window.
    Keystroke,
    /// Hook holds its stdout open; we send `{"answer": "..."}` JSON line back.
    BlockResponse,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputSpec {
    None,
    YesNo {
        format: YesNoFormat,
        delivery: Delivery,
    },
    SingleChoice {
        options: Vec<Choice>,
        allow_other: bool,
        delivery: Delivery,
    },
    MultiChoice {
        options: Vec<Choice>,
        allow_other: bool,
        delivery: Delivery,
    },
    TextInput {
        #[serde(skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
        delivery: Delivery,
    },
}

impl Default for InputSpec {
    fn default() -> Self {
        InputSpec::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn yes_no_serializes_with_format_and_delivery() {
        let spec = InputSpec::YesNo {
            format: YesNoFormat::Numeric,
            delivery: Delivery::Keystroke,
        };
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v, json!({"kind": "yes_no", "format": "numeric", "delivery": "keystroke"}));
    }

    #[test]
    fn single_choice_serializes_options() {
        let spec = InputSpec::SingleChoice {
            options: vec![
                Choice { label: "A".into(), description: None },
                Choice { label: "B".into(), description: Some("desc".into()) },
            ],
            allow_other: true,
            delivery: Delivery::BlockResponse,
        };
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v, json!({
            "kind": "single_choice",
            "options": [
                {"label": "A"},
                {"label": "B", "description": "desc"},
            ],
            "allow_other": true,
            "delivery": "block_response",
        }));
    }

    #[test]
    fn none_serializes_as_kind_only() {
        let spec = InputSpec::None;
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v, json!({"kind": "none"}));
    }

    #[test]
    fn text_input_omits_placeholder_when_none() {
        let spec = InputSpec::TextInput {
            placeholder: None,
            delivery: Delivery::BlockResponse,
        };
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v, json!({"kind": "text_input", "delivery": "block_response"}));
    }

    #[test]
    fn yes_no_format_keystrokes() {
        assert_eq!(YesNoFormat::YN.yes_text(), "y\n");
        assert_eq!(YesNoFormat::Numeric.no_text(), "\x1b");
    }

    #[test]
    fn deserialize_single_choice_from_hook_json() {
        let raw = json!({
            "kind": "single_choice",
            "options": [{"label": "A"}, {"label": "B", "description": "second"}],
            "allow_other": false,
            "delivery": "block_response",
        });
        let spec: InputSpec = serde_json::from_value(raw).unwrap();
        match spec {
            InputSpec::SingleChoice { options, allow_other, delivery: _ } => {
                assert_eq!(options.len(), 2);
                assert_eq!(options[1].description.as_deref(), Some("second"));
                assert!(!allow_other);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn deserialize_yes_no_from_hook_json() {
        let raw = json!({"kind": "yes_no", "format": "yn", "delivery": "keystroke"});
        let spec: InputSpec = serde_json::from_value(raw).unwrap();
        assert!(matches!(spec, InputSpec::YesNo { format: YesNoFormat::YN, delivery: Delivery::Keystroke }));
    }
}
