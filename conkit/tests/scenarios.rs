mod support;

#[allow(dead_code)]
#[path = "support/scenario.rs"]
mod scenario;

use scenario::Suite;

#[test]
fn checked_in_scenarios_pass() {
    let suite = Suite::discover_workspace().expect("discover checked-in scenarios");

    suite.run().expect("checked-in scenarios should pass");
}

#[test]
fn checked_in_scenarios_cover_the_cli_contract() {
    let suite = Suite::discover_workspace().expect("discover checked-in scenarios");

    suite
        .audit_cli_coverage()
        .expect("checked-in scenarios should cover the CLI contract");
}
