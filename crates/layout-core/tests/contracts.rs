use layout_core::{
    place, validate_design, validate_route_solution, Component, CopperLayer, Design, Jumper,
    JumperApproval, JumperMode, JumperPolicy, LocalAabb, Net, Pad, PadKind, PlacementOptions,
    PlacementRules, PlacementStatus, Point, Polygon, Pose, RouteMetrics, RouteSegment,
    RouteSolution, RoutingRules, TerminalRef, DESIGN_SCHEMA_VERSION, SOLUTION_SCHEMA_VERSION,
};

fn pad() -> Pad {
    Pad {
        id: "1".into(),
        local_position: Point { x: 0.0, y: 0.0 },
        size: Point { x: 2.0, y: 2.0 },
        drill_mm: Some(1.0),
        kind: PadKind::ThroughHole,
    }
}

fn component(id: &str, x: f64, y: f64, locked: bool) -> Component {
    Component {
        id: id.into(),
        reference: id.into(),
        footprint: "Fixture:THT".into(),
        envelope: LocalAabb {
            min_x: -1.0,
            min_y: -1.0,
            max_x: 1.0,
            max_y: 1.0,
        },
        pads: vec![pad()],
        initial_pose: Pose {
            x,
            y,
            rotation_degrees: 0.0,
        },
        locked,
        allowed_rotations_degrees: vec![0.0, 90.0],
    }
}

fn terminal(component_id: &str) -> TerminalRef {
    TerminalRef {
        component_id: component_id.into(),
        pad_id: "1".into(),
    }
}

fn rules() -> (PlacementRules, RoutingRules) {
    (
        PlacementRules {
            grid_mm: 1.0,
            component_clearance_mm: 0.5,
            edge_clearance_mm: 1.0,
        },
        RoutingRules {
            grid_mm: 0.5,
            trace_width_mm: 1.0,
            trace_clearance_mm: 1.0,
            edge_clearance_mm: 1.0,
        },
    )
}

fn two_pin_design(locked: bool) -> Design {
    let (placement_rules, routing_rules) = rules();
    Design {
        schema_version: DESIGN_SCHEMA_VERSION,
        name: "two-pin fixture".into(),
        outline: Polygon {
            vertices: vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: 20.0, y: 0.0 },
                Point { x: 20.0, y: 10.0 },
                Point { x: 0.0, y: 10.0 },
            ],
        },
        placement_keepouts: Vec::new(),
        components: vec![
            component("J1", 3.0, 5.0, locked),
            component("J2", 17.0, 5.0, locked),
        ],
        nets: vec![Net {
            id: "N1".into(),
            name: "SIGNAL".into(),
            terminals: vec![terminal("J1"), terminal("J2")],
        }],
        placement_rules,
        routing_rules,
        jumper_policy: JumperPolicy {
            mode: JumperMode::Forbidden,
            approvals: Vec::new(),
        },
    }
}

fn straight_route() -> RouteSolution {
    RouteSolution {
        schema_version: SOLUTION_SCHEMA_VERSION,
        segments: vec![RouteSegment {
            net_id: "N1".into(),
            start: Point { x: 3.0, y: 5.0 },
            end: Point { x: 17.0, y: 5.0 },
            width_mm: 1.0,
            layer: CopperLayer::Back,
        }],
        jumpers: Vec::new(),
        unrouted_net_ids: Vec::new(),
        metrics: RouteMetrics {
            routed_net_count: 1,
            total_net_count: 1,
            total_trace_length_mm: 14.0,
            jumper_count: 0,
        },
    }
}

#[test]
fn canonical_design_round_trips_through_json() {
    let design = two_pin_design(false);
    let encoded = serde_json::to_string_pretty(&design).unwrap();
    let decoded: Design = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, design);
    validate_design(&decoded).unwrap();
}

#[test]
fn design_rejects_pad_extents_outside_declared_envelope() {
    let mut design = two_pin_design(false);
    design.components[0].pads[0].local_position.x = 2.0;
    let error = validate_design(&design).unwrap_err();
    assert!(error.to_string().contains("pad extents must fit"));
}

#[test]
fn placer_uses_rotation_when_it_is_the_only_legal_fit() {
    let mut design = two_pin_design(false);
    design.components.truncate(1);
    design.nets.clear();
    design.outline.vertices = vec![
        Point { x: 0.0, y: 0.0 },
        Point { x: 6.0, y: 0.0 },
        Point { x: 6.0, y: 10.0 },
        Point { x: 0.0, y: 10.0 },
    ];
    design.components[0].envelope = LocalAabb {
        min_x: -3.0,
        min_y: -1.0,
        max_x: 3.0,
        max_y: 1.0,
    };
    design.components[0].initial_pose = Pose {
        x: 3.0,
        y: 5.0,
        rotation_degrees: 0.0,
    };

    let outcome = place(&design, PlacementOptions::default()).unwrap();
    assert_eq!(outcome.status, PlacementStatus::Complete);
    assert!((outcome.placements["J1"].rotation_degrees - 90.0).abs() < 1.0e-9);
}

#[test]
fn placer_returns_search_exhausted_with_partial_evidence() {
    let mut design = two_pin_design(false);
    design.nets.clear();
    design.outline.vertices = vec![
        Point { x: 0.0, y: 0.0 },
        Point { x: 8.0, y: 0.0 },
        Point { x: 8.0, y: 8.0 },
        Point { x: 0.0, y: 8.0 },
    ];
    for component in &mut design.components {
        component.envelope = LocalAabb {
            min_x: -2.0,
            min_y: -2.0,
            max_x: 2.0,
            max_y: 2.0,
        };
        component.initial_pose = Pose {
            x: 4.0,
            y: 4.0,
            rotation_degrees: 0.0,
        };
    }

    let outcome = place(&design, PlacementOptions::default()).unwrap();
    assert_eq!(outcome.status, PlacementStatus::SearchExhausted);
    assert_eq!(outcome.metrics.placed_components, 1);
    assert_eq!(outcome.unplaced_component_ids.len(), 1);
    assert_eq!(outcome.diagnostics[0].code, "placement_search_exhausted");
}

#[test]
fn overlapping_locked_components_are_infeasible_constraints() {
    let mut design = two_pin_design(true);
    design.components[1].initial_pose = design.components[0].initial_pose;
    let outcome = place(&design, PlacementOptions::default()).unwrap();
    assert_eq!(outcome.status, PlacementStatus::InfeasibleFixedConstraints);
    assert_eq!(outcome.diagnostics[0].code, "component_overlap");
}

#[test]
fn route_validator_accepts_connected_single_layer_route() {
    let design = two_pin_design(true);
    let placement = place(&design, PlacementOptions::default()).unwrap();
    validate_route_solution(&design, &placement, &straight_route()).unwrap();
}

#[test]
fn route_validator_rejects_false_unrouted_and_metric_claims() {
    let design = two_pin_design(true);
    let placement = place(&design, PlacementOptions::default()).unwrap();
    let mut route = straight_route();
    route.unrouted_net_ids.push("N1".into());
    route.metrics.routed_net_count = 0;
    let error = validate_route_solution(&design, &placement, &route).unwrap_err();
    assert!(error.to_string().contains("connectivity derives"));
}

#[test]
fn route_validator_rejects_different_net_crossing() {
    let mut design = two_pin_design(true);
    design.outline.vertices = vec![
        Point { x: 0.0, y: 0.0 },
        Point { x: 20.0, y: 0.0 },
        Point { x: 20.0, y: 12.0 },
        Point { x: 0.0, y: 12.0 },
    ];
    design.components = vec![
        component("A1", 3.0, 6.0, true),
        component("A2", 17.0, 6.0, true),
        component("B1", 10.0, 2.0, true),
        component("B2", 10.0, 10.0, true),
    ];
    design.nets = vec![
        Net {
            id: "A".into(),
            name: "A".into(),
            terminals: vec![terminal("A1"), terminal("A2")],
        },
        Net {
            id: "B".into(),
            name: "B".into(),
            terminals: vec![terminal("B1"), terminal("B2")],
        },
    ];
    let placement = place(&design, PlacementOptions::default()).unwrap();
    assert_eq!(placement.status, PlacementStatus::Complete);
    let route = RouteSolution {
        schema_version: SOLUTION_SCHEMA_VERSION,
        segments: vec![
            RouteSegment {
                net_id: "A".into(),
                start: Point { x: 3.0, y: 6.0 },
                end: Point { x: 17.0, y: 6.0 },
                width_mm: 1.0,
                layer: CopperLayer::Back,
            },
            RouteSegment {
                net_id: "B".into(),
                start: Point { x: 10.0, y: 2.0 },
                end: Point { x: 10.0, y: 10.0 },
                width_mm: 1.0,
                layer: CopperLayer::Back,
            },
        ],
        jumpers: Vec::new(),
        unrouted_net_ids: Vec::new(),
        metrics: RouteMetrics {
            routed_net_count: 2,
            total_net_count: 2,
            total_trace_length_mm: 22.0,
            jumper_count: 0,
        },
    };
    let error = validate_route_solution(&design, &placement, &route).unwrap_err();
    assert!(error.to_string().contains("different-net clearance"));
}

#[test]
fn route_validator_checks_np_through_hole_clearance() {
    let mut design = two_pin_design(true);
    let mut mounting_hole = component("H1", 10.0, 7.0, true);
    mounting_hole.pads[0].kind = PadKind::NonPlatedThroughHole;
    design.components.push(mounting_hole);
    let placement = place(&design, PlacementOptions::default()).unwrap();
    assert_eq!(placement.status, PlacementStatus::Complete);

    let error = validate_route_solution(&design, &placement, &straight_route()).unwrap_err();
    assert!(error.to_string().contains("NPTH clearance violation"));
}

#[test]
fn jumper_requires_explicit_design_authority() {
    let mut design = two_pin_design(true);
    let placement = place(&design, PlacementOptions::default()).unwrap();
    let jumper_route = RouteSolution {
        schema_version: SOLUTION_SCHEMA_VERSION,
        segments: Vec::new(),
        jumpers: vec![Jumper {
            net_id: "N1".into(),
            start: Point { x: 3.0, y: 5.0 },
            end: Point { x: 17.0, y: 5.0 },
            approval_id: "review-0001".into(),
        }],
        unrouted_net_ids: Vec::new(),
        metrics: RouteMetrics {
            routed_net_count: 1,
            total_net_count: 1,
            total_trace_length_mm: 0.0,
            jumper_count: 1,
        },
    };
    let forbidden = validate_route_solution(&design, &placement, &jumper_route).unwrap_err();
    assert!(forbidden.to_string().contains("forbids all jumpers"));

    design.jumper_policy = JumperPolicy {
        mode: JumperMode::UserApprovedOnly,
        approvals: vec![JumperApproval {
            approval_id: "review-0001".into(),
            net_id: "N1".into(),
            max_count: 1,
        }],
    };
    let mut unapproved_route = jumper_route.clone();
    unapproved_route.jumpers[0].approval_id = "review-not-granted".into();
    let unapproved = validate_route_solution(&design, &placement, &unapproved_route).unwrap_err();
    assert!(unapproved.to_string().contains("no user approval named"));

    validate_route_solution(&design, &placement, &jumper_route).unwrap();
}
