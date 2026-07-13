//! Portable Windows file-name validation shared by every platform.
//!
//! Contract and report paths are logical and portable. Rejecting names that
//! Windows cannot represent on every host prevents a catalog generated on one
//! platform from becoming unwritable on another.

use crate::error::CliError;

/// Borrowed Windows file-name component to validate.
#[derive(Debug)]
pub(super) struct WindowsFileName<'a> {
    value: &'a str,
}

impl<'a> WindowsFileName<'a> {
    /// Creates a validator for one path component.
    pub(super) fn new(value: &'a str) -> Self {
        Self { value }
    }

    /// Validates the component against Windows-specific restrictions.
    ///
    /// # Errors
    ///
    /// Returns an error if the component contains a forbidden character, ends
    /// in a space or dot, or uses a reserved device name.
    pub(super) fn validate(&self) -> Result<(), CliError> {
        if let Some(character) = self.invalid_character() {
            return Err(CliError::WindowsInvalidFileNameCharacter {
                component: self.value.to_owned(),
                character,
            });
        }

        if self.value.ends_with([' ', '.']) {
            return Err(CliError::WindowsTrailingSpaceOrDot {
                component: self.value.to_owned(),
            });
        }

        if self.is_reserved_device_name() {
            return Err(CliError::WindowsReservedDeviceName {
                component: self.value.to_owned(),
            });
        }

        Ok(())
    }

    /// Returns the first character forbidden in a Windows file-name component.
    fn invalid_character(&self) -> Option<char> {
        self.value.chars().find(|character| {
            *character <= '\u{001f}'
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
        })
    }

    /// Returns whether the stem is a reserved Windows device name.
    fn is_reserved_device_name(&self) -> bool {
        let stem = self.value.split('.').next().unwrap_or(self.value);
        let upper = stem.to_ascii_uppercase();

        matches!(
            upper.as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "COM¹"
                | "COM²"
                | "COM³"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
                | "LPT¹"
                | "LPT²"
                | "LPT³"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::WindowsFileName;

    #[test]
    fn rejects_con() {
        assert!(WindowsFileName::new("CON").validate().is_err());
    }

    #[test]
    fn rejects_con_with_extension() {
        assert!(WindowsFileName::new("con.exe").validate().is_err());
    }

    #[test]
    fn rejects_nul() {
        assert!(WindowsFileName::new("NUL").validate().is_err());
    }

    #[test]
    fn rejects_lpt1() {
        assert!(WindowsFileName::new("LPT1").validate().is_err());
    }

    #[test]
    fn rejects_superscript_com_and_lpt_device_names() {
        assert!(WindowsFileName::new("COM¹").validate().is_err());
        assert!(WindowsFileName::new("LPT³.txt").validate().is_err());
    }

    #[test]
    fn rejects_trailing_space_or_dot() {
        assert!(WindowsFileName::new("report ").validate().is_err());
        assert!(WindowsFileName::new("report.").validate().is_err());
    }

    #[test]
    fn rejects_reserved_characters() {
        for character in ['<', '>', ':', '"', '/', '\\', '|', '?', '*'] {
            let component = format!("contract{character}name.yaml");
            assert!(
                WindowsFileName::new(&component).validate().is_err(),
                "{character:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_c0_control_characters() {
        for byte in 0_u8..=0x1f {
            let character = char::from(byte);
            let component = format!("contract{character}name.yaml");
            assert!(
                WindowsFileName::new(&component).validate().is_err(),
                "U+{byte:04X} should be rejected"
            );
        }
    }

    #[test]
    fn accepts_normal_contract_file_names() {
        WindowsFileName::new("src")
            .validate()
            .expect("ordinary directory should be valid");
        WindowsFileName::new("lib.yaml")
            .validate()
            .expect("ordinary contract file should be valid");
    }
}
