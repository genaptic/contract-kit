use assert_cmd::Command;

pub(crate) struct ConkitCli;

impl ConkitCli {
    pub(crate) fn command() -> Command {
        let mut command = Command::cargo_bin("conkit").expect("conkit CLI binary");
        command
            .env("COLUMNS", "100")
            .env("LINES", "24")
            .env("NO_COLOR", "1")
            .env_remove("CLICOLOR")
            .env_remove("CLICOLOR_FORCE");
        command
    }

    #[allow(dead_code)]
    pub(crate) const fn displayed_name() -> &'static str {
        "conkit"
    }
}
