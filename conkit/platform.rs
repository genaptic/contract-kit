//! Portable path rules and shared executable identity.
//!
//! Every supported target uses the installed `conkit` command name and rejects
//! Windows-invalid components so generated catalogs remain portable across
//! supported hosts.

use std::ffi::OsStr;

use crate::error::CliError;

mod windows_names;

use windows_names::WindowsFileName;

/// Executable name displayed in help output on every supported platform.
pub(crate) const EXECUTABLE_NAME: &str = "conkit";

/// Cross-platform validation for portable logical path components.
pub(crate) struct PortablePathRules;

impl PortablePathRules {
    /// Validates one component against the portable filename policy.
    ///
    /// # Errors
    ///
    /// Returns an error if the component is not UTF-8 or violates the Windows
    /// filename restrictions enforced on every platform.
    pub(crate) fn validate_component(component: &OsStr) -> Result<(), CliError> {
        let component = component.to_str().ok_or(CliError::NonUtf8PathComponent)?;

        WindowsFileName::new(component).validate()
    }
}
