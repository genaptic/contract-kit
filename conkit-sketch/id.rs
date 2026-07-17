use std::borrow::Borrow;
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct SketchId {
    value: String,
}

impl SketchId {
    pub(crate) fn new(value: String, maximum_bytes: usize) -> Result<Self, SketchIdError> {
        if value.is_empty() {
            return Err(SketchIdError::Empty { original: value });
        }

        if value.trim() != value {
            return Err(SketchIdError::SurroundingWhitespace { original: value });
        }

        let actual = value.len();
        if actual > maximum_bytes {
            return Err(SketchIdError::TooLong {
                original: value,
                maximum: maximum_bytes,
                actual,
            });
        }

        if let Some((index, _)) = value
            .char_indices()
            .find(|(_, character)| character.is_control())
        {
            return Err(SketchIdError::ControlCharacter {
                original: value,
                index,
            });
        }

        Ok(Self { value })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for SketchId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Borrow<str> for SketchId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SketchIdError {
    Empty {
        original: String,
    },
    SurroundingWhitespace {
        original: String,
    },
    TooLong {
        original: String,
        maximum: usize,
        actual: usize,
    },
    ControlCharacter {
        original: String,
        index: usize,
    },
}

impl fmt::Display for SketchIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty { original } => write!(
                formatter,
                "sketch id \"{}\" must not be empty",
                original.escape_debug()
            ),
            Self::SurroundingWhitespace { original } => write!(
                formatter,
                "sketch id \"{}\" must not have surrounding whitespace",
                original.escape_debug()
            ),
            Self::TooLong {
                original,
                maximum,
                actual,
            } => write!(
                formatter,
                "sketch id \"{}\" is {actual} bytes; maximum is {maximum}",
                original.escape_debug()
            ),
            Self::ControlCharacter { original, index } => write!(
                formatter,
                "sketch id \"{}\" contains a control character at byte index {index}",
                original.escape_debug()
            ),
        }
    }
}

impl std::error::Error for SketchIdError {}

#[cfg(test)]
mod tests {
    use super::{SketchId, SketchIdError};

    const MAXIMUM_BYTES: usize = 32;

    #[test]
    fn empty_id_is_rejected_without_losing_the_original_scalar() {
        let error =
            SketchId::new(String::new(), MAXIMUM_BYTES).expect_err("an empty sketch ID must fail");

        assert_eq!(
            error,
            SketchIdError::Empty {
                original: String::new(),
            }
        );
    }

    #[test]
    fn surrounding_unicode_whitespace_is_rejected_without_trimming() {
        for value in [
            " answer_body",
            "answer_body ",
            "\u{00a0}answer_body\u{00a0}",
        ] {
            let error = SketchId::new(value.to_owned(), MAXIMUM_BYTES)
                .expect_err("surrounding whitespace must fail");

            assert_eq!(
                error,
                SketchIdError::SurroundingWhitespace {
                    original: value.to_owned(),
                }
            );
        }
    }

    #[test]
    fn maximum_is_measured_in_bytes() {
        let accepted = "éé";
        let rejected = "ééx";

        assert_eq!(
            SketchId::new(accepted.to_owned(), accepted.len())
                .expect("the exact byte limit must be accepted")
                .as_str(),
            accepted
        );

        let error = SketchId::new(rejected.to_owned(), accepted.len())
            .expect_err("one byte beyond the limit must fail");
        assert_eq!(
            error,
            SketchIdError::TooLong {
                original: rejected.to_owned(),
                maximum: accepted.len(),
                actual: rejected.len(),
            }
        );
    }

    #[test]
    fn unicode_control_character_reports_its_byte_index() {
        let value = "é\0body";
        let error = SketchId::new(value.to_owned(), MAXIMUM_BYTES)
            .expect_err("a control character must fail");

        assert_eq!(
            error,
            SketchIdError::ControlCharacter {
                original: value.to_owned(),
                index: 2,
            }
        );
        assert!(!error.to_string().contains('\0'));
        assert!(error.to_string().contains("\\0"));
    }

    #[test]
    fn internal_non_control_whitespace_is_preserved() {
        let value = "answer \u{00a0} body";
        let id = SketchId::new(value.to_owned(), MAXIMUM_BYTES)
            .expect("internal non-control whitespace must remain valid");

        assert_eq!(id.as_str(), value);
    }

    #[test]
    fn canonically_equivalent_unicode_ids_remain_byte_distinct() {
        let composed =
            SketchId::new("é".to_owned(), MAXIMUM_BYTES).expect("composed ID must be valid");
        let decomposed = SketchId::new("e\u{301}".to_owned(), MAXIMUM_BYTES)
            .expect("decomposed ID must be valid");

        assert_ne!(composed, decomposed);
        assert_eq!(composed.as_str(), "é");
        assert_eq!(decomposed.as_str(), "e\u{301}");
    }

    #[test]
    fn exact_duplicate_ids_remain_equal_for_callers_to_reject() {
        let first =
            SketchId::new("answer_body".to_owned(), MAXIMUM_BYTES).expect("first ID must be valid");
        let duplicate = SketchId::new("answer_body".to_owned(), MAXIMUM_BYTES)
            .expect("duplicate ID must be individually valid");

        assert_eq!(first, duplicate);
    }
}
