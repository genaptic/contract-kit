mod support;

use std::{collections::BTreeMap, ffi::OsString};

use predicates::prelude::*;
use support::ConkitCli;

#[test]
fn root_help_uses_platform_display_name() {
    ConkitCli::command()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "Usage: {}",
            ConkitCli::displayed_name()
        )));
}

#[test]
fn command_pins_help_presentation_environment() {
    let command = ConkitCli::command();
    let actual = command
        .get_envs()
        .map(|(name, value)| (name.to_owned(), value.map(ToOwned::to_owned)))
        .collect::<BTreeMap<OsString, Option<OsString>>>();
    let expected = BTreeMap::from([
        (OsString::from("CLICOLOR"), None),
        (OsString::from("CLICOLOR_FORCE"), None),
        (OsString::from("COLUMNS"), Some(OsString::from("100"))),
        (OsString::from("LINES"), Some(OsString::from("24"))),
        (OsString::from("NO_COLOR"), Some(OsString::from("1"))),
    ]);

    assert_eq!(actual, expected);
}
