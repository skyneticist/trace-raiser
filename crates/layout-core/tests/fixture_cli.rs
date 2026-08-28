use layout_core::{place, Design, PlacementOptions, PlacementStatus};

#[test]
fn checked_in_evaluation_fixture_is_valid_and_solvable() {
    let design: Design = serde_json::from_str(include_str!("../fixtures/two-pin.json")).unwrap();
    let outcome = place(&design, PlacementOptions::default()).unwrap();
    assert_eq!(outcome.status, PlacementStatus::Complete);
    assert_eq!(outcome.metrics.placed_components, 2);
}
