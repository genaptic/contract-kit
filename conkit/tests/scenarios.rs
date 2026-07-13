mod support;

#[allow(dead_code)]
#[path = "support/scenario.rs"]
mod scenario;

use scenario::Suite;

const EXPECTED_SCENARIO_COUNT: usize = 136;

#[test]
fn checked_in_scenario_inventory_contains_136_independent_leaves() {
    let suite = Suite::discover_workspace().expect("discover checked-in scenarios");

    assert_eq!(suite.scenario_ids().len(), EXPECTED_SCENARIO_COUNT);
}

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
