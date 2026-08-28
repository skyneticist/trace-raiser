use crate::geometry::{
    envelope_inside_outline, point_is_finite, polygon_bounds, polygon_is_simple, polygons_conflict,
    pose_is_finite, transform_point, transformed_envelope, Bounds,
};
use crate::{
    Component, Design, JumperMode, PadKind, PlacementDiagnostic, PlacementMetrics,
    PlacementOptions, PlacementOutcome, PlacementStatus, Point, Pose, DESIGN_SCHEMA_VERSION,
    SOLUTION_SCHEMA_VERSION,
};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::error::Error;
use std::fmt;

const SCORE_EPSILON: f64 = 1.0e-9;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesignValidationError {
    messages: Vec<String>,
}

impl DesignValidationError {
    pub fn messages(&self) -> &[String] {
        &self.messages
    }
}

impl fmt::Display for DesignValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.messages.len() == 1 {
            formatter.write_str(&self.messages[0])
        } else {
            write!(
                formatter,
                "design validation failed with {} issues: {}",
                self.messages.len(),
                self.messages.join("; ")
            )
        }
    }
}

impl Error for DesignValidationError {}

pub fn validate_design(design: &Design) -> Result<(), DesignValidationError> {
    let mut issues = Vec::new();
    if design.schema_version != DESIGN_SCHEMA_VERSION {
        issues.push(format!(
            "schema_version: expected {}, received {}",
            DESIGN_SCHEMA_VERSION, design.schema_version
        ));
    }
    if design.name.trim().is_empty() {
        issues.push("name: must not be empty".into());
    }
    validate_polygon("outline", &design.outline.vertices, &mut issues);

    let mut keepout_ids = HashSet::new();
    for (index, keepout) in design.placement_keepouts.iter().enumerate() {
        let path = format!("placement_keepouts[{index}]");
        if keepout.id.trim().is_empty() {
            issues.push(format!("{path}.id: must not be empty"));
        } else if !keepout_ids.insert(keepout.id.as_str()) {
            issues.push(format!("{path}.id: duplicate ID {}", keepout.id));
        }
        validate_polygon(
            &format!("{path}.polygon"),
            &keepout.polygon.vertices,
            &mut issues,
        );
        validate_nonnegative(
            &format!("{path}.clearance_mm"),
            keepout.clearance_mm,
            &mut issues,
        );
    }

    let mut component_ids = HashSet::new();
    let mut references = HashSet::new();
    let mut pad_keys = HashMap::<(&str, &str), &PadKind>::new();
    for (component_index, component) in design.components.iter().enumerate() {
        let path = format!("components[{component_index}]");
        if component.id.trim().is_empty() {
            issues.push(format!("{path}.id: must not be empty"));
        } else if !component_ids.insert(component.id.as_str()) {
            issues.push(format!("{path}.id: duplicate ID {}", component.id));
        }
        if component.reference.trim().is_empty() {
            issues.push(format!("{path}.reference: must not be empty"));
        } else if !references.insert(component.reference.as_str()) {
            issues.push(format!(
                "{path}.reference: duplicate reference {}",
                component.reference
            ));
        }
        if component.footprint.trim().is_empty() {
            issues.push(format!("{path}.footprint: must not be empty"));
        }
        let envelope = component.envelope;
        if ![
            envelope.min_x,
            envelope.min_y,
            envelope.max_x,
            envelope.max_y,
        ]
        .into_iter()
        .all(f64::is_finite)
            || envelope.width() <= 0.0
            || envelope.height() <= 0.0
        {
            issues.push(format!(
                "{path}.envelope: coordinates must be finite with positive width and height"
            ));
        }
        if !pose_is_finite(component.initial_pose) {
            issues.push(format!("{path}.initial_pose: must be finite"));
        }
        if component.allowed_rotations_degrees.is_empty() {
            issues.push(format!(
                "{path}.allowed_rotations_degrees: must contain at least one rotation"
            ));
        } else if component
            .allowed_rotations_degrees
            .iter()
            .any(|rotation| !rotation.is_finite())
        {
            issues.push(format!(
                "{path}.allowed_rotations_degrees: all rotations must be finite"
            ));
        } else if component.locked
            && !component.allowed_rotations_degrees.iter().any(|rotation| {
                rotations_equivalent(*rotation, component.initial_pose.rotation_degrees)
            })
        {
            issues.push(format!(
                "{path}: locked initial rotation is not in allowed_rotations_degrees"
            ));
        }

        let mut pad_ids = HashSet::new();
        for (pad_index, pad) in component.pads.iter().enumerate() {
            let pad_path = format!("{path}.pads[{pad_index}]");
            if pad.id.trim().is_empty() {
                issues.push(format!("{pad_path}.id: must not be empty"));
            } else if !pad_ids.insert(pad.id.as_str()) {
                issues.push(format!("{pad_path}.id: duplicate pad ID {}", pad.id));
            } else {
                pad_keys.insert((component.id.as_str(), pad.id.as_str()), &pad.kind);
            }
            if !point_is_finite(pad.local_position) {
                issues.push(format!("{pad_path}.local_position: must be finite"));
            }
            if !point_is_finite(pad.size) || pad.size.x <= 0.0 || pad.size.y <= 0.0 {
                issues.push(format!(
                    "{pad_path}.size: dimensions must be finite and positive"
                ));
            }
            if point_is_finite(pad.local_position)
                && point_is_finite(pad.size)
                && pad.size.x > 0.0
                && pad.size.y > 0.0
                && (pad.local_position.x - pad.size.x * 0.5 < envelope.min_x - SCORE_EPSILON
                    || pad.local_position.x + pad.size.x * 0.5 > envelope.max_x + SCORE_EPSILON
                    || pad.local_position.y - pad.size.y * 0.5 < envelope.min_y - SCORE_EPSILON
                    || pad.local_position.y + pad.size.y * 0.5 > envelope.max_y + SCORE_EPSILON)
            {
                issues.push(format!(
                    "{pad_path}: pad extents must fit inside the placement envelope"
                ));
            }
            if let Some(drill) = pad.drill_mm {
                if !drill.is_finite() || drill <= 0.0 {
                    issues.push(format!("{pad_path}.drill_mm: must be finite and positive"));
                } else if point_is_finite(pad.size) && drill >= pad.size.x.min(pad.size.y) {
                    issues.push(format!(
                        "{pad_path}.drill_mm: must be smaller than the pad diameter"
                    ));
                }
            }
            if pad.drill_mm.is_none() {
                issues.push(format!(
                    "{pad_path}.drill_mm: through-hole and NPTH pads require a drill"
                ));
            }
        }
    }

    let mut net_ids = HashSet::new();
    let mut claimed_terminals = BTreeMap::new();
    for (net_index, net) in design.nets.iter().enumerate() {
        let path = format!("nets[{net_index}]");
        if net.id.trim().is_empty() {
            issues.push(format!("{path}.id: must not be empty"));
        } else if !net_ids.insert(net.id.as_str()) {
            issues.push(format!("{path}.id: duplicate net ID {}", net.id));
        }
        if net.name.trim().is_empty() {
            issues.push(format!("{path}.name: must not be empty"));
        }
        let mut local_terminals = BTreeSet::new();
        for terminal in &net.terminals {
            let terminal_key = (terminal.component_id.as_str(), terminal.pad_id.as_str());
            if !local_terminals.insert(terminal_key) {
                issues.push(format!(
                    "{path}.terminals: duplicate terminal {}.{}",
                    terminal.component_id, terminal.pad_id
                ));
                continue;
            }
            match pad_keys.get(&terminal_key) {
                None => issues.push(format!(
                    "{path}.terminals: unknown terminal {}.{}",
                    terminal.component_id, terminal.pad_id
                )),
                Some(PadKind::NonPlatedThroughHole) => issues.push(format!(
                    "{path}.terminals: NPTH pad {}.{} cannot belong to a net",
                    terminal.component_id, terminal.pad_id
                )),
                Some(PadKind::ThroughHole) => {}
            }
            if let Some(previous_net) = claimed_terminals.insert(terminal_key, net.id.as_str()) {
                issues.push(format!(
                    "{path}.terminals: {}.{} is already assigned to net {}",
                    terminal.component_id, terminal.pad_id, previous_net
                ));
            }
        }
    }

    validate_positive(
        "placement_rules.grid_mm",
        design.placement_rules.grid_mm,
        &mut issues,
    );
    validate_nonnegative(
        "placement_rules.component_clearance_mm",
        design.placement_rules.component_clearance_mm,
        &mut issues,
    );
    validate_nonnegative(
        "placement_rules.edge_clearance_mm",
        design.placement_rules.edge_clearance_mm,
        &mut issues,
    );
    validate_positive(
        "routing_rules.grid_mm",
        design.routing_rules.grid_mm,
        &mut issues,
    );
    validate_positive(
        "routing_rules.trace_width_mm",
        design.routing_rules.trace_width_mm,
        &mut issues,
    );
    validate_nonnegative(
        "routing_rules.trace_clearance_mm",
        design.routing_rules.trace_clearance_mm,
        &mut issues,
    );
    validate_nonnegative(
        "routing_rules.edge_clearance_mm",
        design.routing_rules.edge_clearance_mm,
        &mut issues,
    );

    match design.jumper_policy.mode {
        JumperMode::Forbidden if !design.jumper_policy.approvals.is_empty() => issues.push(
            "jumper_policy.approvals: approvals must be empty when jumpers are forbidden".into(),
        ),
        JumperMode::Forbidden | JumperMode::UserApprovedOnly => {}
    }
    let mut approved_nets = HashSet::new();
    let mut approval_ids = HashSet::new();
    for (index, approval) in design.jumper_policy.approvals.iter().enumerate() {
        let path = format!("jumper_policy.approvals[{index}]");
        if approval.approval_id.trim().is_empty() {
            issues.push(format!("{path}.approval_id: must not be empty"));
        } else if !approval_ids.insert(approval.approval_id.as_str()) {
            issues.push(format!(
                "{path}.approval_id: duplicate approval ID {}",
                approval.approval_id
            ));
        }
        if !net_ids.contains(approval.net_id.as_str()) {
            issues.push(format!("{path}.net_id: unknown net {}", approval.net_id));
        }
        if approval.max_count == 0 {
            issues.push(format!("{path}.max_count: must be greater than zero"));
        }
        if !approved_nets.insert(approval.net_id.as_str()) {
            issues.push(format!(
                "{path}.net_id: duplicate approval for net {}",
                approval.net_id
            ));
        }
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(DesignValidationError { messages: issues })
    }
}

pub fn place(
    design: &Design,
    options: PlacementOptions,
) -> Result<PlacementOutcome, DesignValidationError> {
    validate_design(design)?;
    let mut option_issues = Vec::new();
    if options.restarts == 0 {
        option_issues.push("placement_options.restarts: must be greater than zero".into());
    }
    if options.max_grid_points == 0 {
        option_issues.push("placement_options.max_grid_points: must be greater than zero".into());
    }
    if !option_issues.is_empty() {
        return Err(DesignValidationError {
            messages: option_issues,
        });
    }

    let component_lookup: HashMap<&str, &Component> = design
        .components
        .iter()
        .map(|component| (component.id.as_str(), component))
        .collect();
    let mut fixed = BTreeMap::new();
    for component in design
        .components
        .iter()
        .filter(|component| component.locked)
    {
        if let Some(diagnostic) = candidate_violation(
            design,
            component,
            component.initial_pose,
            &fixed,
            &component_lookup,
        ) {
            let mut placements = fixed;
            placements.insert(component.id.clone(), component.initial_pose);
            return Ok(outcome(
                design,
                options.seed,
                PlacementStatus::InfeasibleFixedConstraints,
                placements,
                vec![diagnostic],
                0,
                0,
            ));
        }
        fixed.insert(component.id.clone(), component.initial_pose);
    }

    let base_order = movable_component_order(design);
    if base_order.is_empty() {
        return Ok(outcome(
            design,
            options.seed,
            PlacementStatus::Complete,
            fixed,
            Vec::new(),
            0,
            0,
        ));
    }

    let board_bounds = polygon_bounds(&design.outline).expect("validated outline has bounds");
    let mut evaluations = 0_u64;
    let mut best: Option<BTreeMap<String, Pose>> = None;
    let mut restarts_completed = 0;

    for restart in 0..options.restarts {
        let mut order = base_order.clone();
        if restart > 0 {
            deterministic_shuffle(
                &mut order,
                mix64(options.seed ^ (restart as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)),
            );
        }
        let grid = sampled_grid(
            board_bounds,
            design.placement_rules.grid_mm,
            options.max_grid_points,
            mix64(options.seed ^ restart as u64),
        );
        let mut placements = fixed.clone();
        for component_index in order {
            let component = &design.components[component_index];
            let Some(candidate) = best_candidate(
                design,
                component,
                &placements,
                &component_lookup,
                &grid,
                options.seed,
                restart,
                &mut evaluations,
            ) else {
                break;
            };
            placements.insert(component.id.clone(), candidate);
        }
        restarts_completed += 1;

        if placements.len() == design.components.len() {
            refine(
                design,
                &mut placements,
                &component_lookup,
                &grid,
                options,
                restart,
                &mut evaluations,
            );
        }

        if best.as_ref().is_none_or(|current| {
            placement_is_better(design, &placements, current, &component_lookup)
        }) {
            best = Some(placements);
        }
    }

    let placements = best.unwrap_or(fixed);
    let status = if placements.len() == design.components.len() {
        PlacementStatus::Complete
    } else {
        PlacementStatus::SearchExhausted
    };
    let diagnostics = if status == PlacementStatus::SearchExhausted {
        vec![PlacementDiagnostic {
            code: "placement_search_exhausted".into(),
            message: format!(
                "placed {} of {} components after {} deterministic restart(s); increase the board area, relax constraints, reduce the grid, or raise the search budget",
                placements.len(),
                design.components.len(),
                restarts_completed
            ),
            component_ids: design
                .components
                .iter()
                .filter(|component| !placements.contains_key(&component.id))
                .map(|component| component.id.clone())
                .collect(),
        }]
    } else {
        Vec::new()
    };
    Ok(outcome(
        design,
        options.seed,
        status,
        placements,
        diagnostics,
        evaluations,
        restarts_completed,
    ))
}

fn validate_polygon(path: &str, vertices: &[Point], issues: &mut Vec<String>) {
    if vertices.iter().any(|point| !point_is_finite(*point)) {
        issues.push(format!("{path}: every vertex must be finite"));
    } else if !polygon_is_simple(vertices) {
        issues.push(format!(
            "{path}: must be a non-self-intersecting polygon with nonzero area"
        ));
    }
}

fn validate_positive(path: &str, value: f64, issues: &mut Vec<String>) {
    if !value.is_finite() || value <= 0.0 {
        issues.push(format!("{path}: must be finite and greater than zero"));
    }
}

fn validate_nonnegative(path: &str, value: f64, issues: &mut Vec<String>) {
    if !value.is_finite() || value < 0.0 {
        issues.push(format!("{path}: must be finite and nonnegative"));
    }
}

fn candidate_violation(
    design: &Design,
    component: &Component,
    pose: Pose,
    placements: &BTreeMap<String, Pose>,
    component_lookup: &HashMap<&str, &Component>,
) -> Option<PlacementDiagnostic> {
    let envelope = transformed_envelope(component.envelope, pose);
    if !envelope_inside_outline(
        &envelope,
        &design.outline.vertices,
        design.placement_rules.edge_clearance_mm,
    ) {
        return Some(PlacementDiagnostic {
            code: "component_outside_outline".into(),
            message: format!(
                "{} violates the board outline or edge clearance",
                component.reference
            ),
            component_ids: vec![component.id.clone()],
        });
    }
    for keepout in &design.placement_keepouts {
        if polygons_conflict(&envelope, &keepout.polygon.vertices, keepout.clearance_mm) {
            return Some(PlacementDiagnostic {
                code: "component_in_keepout".into(),
                message: format!(
                    "{} intersects placement keepout {}",
                    component.reference, keepout.id
                ),
                component_ids: vec![component.id.clone()],
            });
        }
    }
    for (other_id, other_pose) in placements {
        let other = component_lookup
            .get(other_id.as_str())
            .expect("validated placement component");
        let other_envelope = transformed_envelope(other.envelope, *other_pose);
        if polygons_conflict(
            &envelope,
            &other_envelope,
            design.placement_rules.component_clearance_mm,
        ) {
            return Some(PlacementDiagnostic {
                code: "component_overlap".into(),
                message: format!(
                    "{} overlaps or violates clearance to {}",
                    component.reference, other.reference
                ),
                component_ids: vec![component.id.clone(), other.id.clone()],
            });
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn best_candidate(
    design: &Design,
    component: &Component,
    placements: &BTreeMap<String, Pose>,
    component_lookup: &HashMap<&str, &Component>,
    grid: &[Point],
    seed: u64,
    restart: usize,
    evaluations: &mut u64,
) -> Option<Pose> {
    let mut rotations = component
        .allowed_rotations_degrees
        .iter()
        .copied()
        .map(normalize_rotation)
        .collect::<Vec<_>>();
    rotations.sort_by(f64::total_cmp);
    rotations.dedup_by(|a, b| rotations_equivalent(*a, *b));

    let mut best: Option<(f64, u64, Pose)> = None;
    let mut seen = HashSet::new();
    for rotation in rotations {
        let initial = Pose {
            x: component.initial_pose.x,
            y: component.initial_pose.y,
            rotation_degrees: rotation,
        };
        consider_candidate(
            design,
            component,
            initial,
            placements,
            component_lookup,
            seed,
            restart,
            evaluations,
            &mut seen,
            &mut best,
        );
        for point in grid {
            consider_candidate(
                design,
                component,
                Pose {
                    x: point.x,
                    y: point.y,
                    rotation_degrees: rotation,
                },
                placements,
                component_lookup,
                seed,
                restart,
                evaluations,
                &mut seen,
                &mut best,
            );
        }
    }
    best.map(|(_, _, pose)| pose)
}

#[allow(clippy::too_many_arguments)]
fn consider_candidate(
    design: &Design,
    component: &Component,
    candidate: Pose,
    placements: &BTreeMap<String, Pose>,
    component_lookup: &HashMap<&str, &Component>,
    seed: u64,
    restart: usize,
    evaluations: &mut u64,
    seen: &mut HashSet<(u64, u64, u64)>,
    best: &mut Option<(f64, u64, Pose)>,
) {
    let key = (
        normalized_zero(candidate.x).to_bits(),
        normalized_zero(candidate.y).to_bits(),
        normalize_rotation(candidate.rotation_degrees).to_bits(),
    );
    if !seen.insert(key) {
        return;
    }
    *evaluations = evaluations.saturating_add(1);
    if candidate_violation(design, component, candidate, placements, component_lookup).is_some() {
        return;
    }
    let score =
        objective_with_candidate(design, placements, component, candidate, component_lookup);
    let tie = mix64(
        seed ^ (restart as u64).rotate_left(17)
            ^ stable_hash(component.id.as_bytes())
            ^ candidate.x.to_bits().rotate_left(7)
            ^ candidate.y.to_bits().rotate_left(23)
            ^ candidate.rotation_degrees.to_bits(),
    );
    if best.as_ref().is_none_or(|(best_score, best_tie, _)| {
        score.total_cmp(best_score) == Ordering::Less
            || ((score - *best_score).abs() <= SCORE_EPSILON && tie < *best_tie)
    }) {
        *best = Some((score, tie, candidate));
    }
}

#[allow(clippy::too_many_arguments)]
fn refine(
    design: &Design,
    placements: &mut BTreeMap<String, Pose>,
    component_lookup: &HashMap<&str, &Component>,
    grid: &[Point],
    options: PlacementOptions,
    restart: usize,
    evaluations: &mut u64,
) {
    let mut component_ids = design
        .components
        .iter()
        .filter(|component| !component.locked)
        .map(|component| component.id.clone())
        .collect::<Vec<_>>();
    component_ids.sort();

    for pass in 0..options.refinement_passes {
        let before = objective(design, placements, true, component_lookup);
        for component_id in &component_ids {
            let component = component_lookup
                .get(component_id.as_str())
                .expect("validated refinement component");
            let previous = placements
                .remove(component_id)
                .expect("complete placement contains component");
            let candidate = best_candidate(
                design,
                component,
                placements,
                component_lookup,
                grid,
                options.seed ^ (pass as u64).rotate_left(31),
                restart,
                evaluations,
            )
            .unwrap_or(previous);
            placements.insert(component_id.clone(), candidate);
        }
        let after = objective(design, placements, true, component_lookup);
        if after + SCORE_EPSILON >= before {
            break;
        }
    }
}

fn movable_component_order(design: &Design) -> Vec<usize> {
    let degree = |component_id: &str| {
        design
            .nets
            .iter()
            .filter(|net| {
                net.terminals
                    .iter()
                    .any(|terminal| terminal.component_id == component_id)
            })
            .count()
    };
    let mut indices = design
        .components
        .iter()
        .enumerate()
        .filter_map(|(index, component)| (!component.locked).then_some(index))
        .collect::<Vec<_>>();
    indices.sort_by(|first, second| {
        let first_component = &design.components[*first];
        let second_component = &design.components[*second];
        degree(&second_component.id)
            .cmp(&degree(&first_component.id))
            .then_with(|| {
                second_component
                    .envelope
                    .area()
                    .total_cmp(&first_component.envelope.area())
            })
            .then_with(|| first_component.reference.cmp(&second_component.reference))
            .then_with(|| first_component.id.cmp(&second_component.id))
    });
    indices
}

fn sampled_grid(bounds: Bounds, grid_mm: f64, limit: usize, seed: u64) -> Vec<Point> {
    let first_x = (bounds.min_x / grid_mm).ceil() * grid_mm;
    let first_y = (bounds.min_y / grid_mm).ceil() * grid_mm;
    let count_x = (((bounds.max_x - first_x) / grid_mm).floor().max(0.0) as usize) + 1;
    let count_y = (((bounds.max_y - first_y) / grid_mm).floor().max(0.0) as usize) + 1;
    let total = count_x.saturating_mul(count_y);
    let stride = total.div_ceil(limit).max(1);
    let offset = if stride == 1 {
        0
    } else {
        seed as usize % stride
    };
    (offset..total)
        .step_by(stride)
        .take(limit)
        .map(|index| {
            let x_index = index % count_x;
            let y_index = index / count_x;
            Point {
                x: normalized_zero(first_x + x_index as f64 * grid_mm),
                y: normalized_zero(first_y + y_index as f64 * grid_mm),
            }
        })
        .collect()
}

fn objective_with_candidate(
    design: &Design,
    placements: &BTreeMap<String, Pose>,
    component: &Component,
    candidate: Pose,
    component_lookup: &HashMap<&str, &Component>,
) -> f64 {
    let (wire_length, displacement) = placement_costs(
        design,
        placements,
        Some((&component.id, candidate)),
        true,
        component_lookup,
    );
    wire_length + displacement * 0.02
}

fn objective(
    design: &Design,
    placements: &BTreeMap<String, Pose>,
    fallback: bool,
    component_lookup: &HashMap<&str, &Component>,
) -> f64 {
    let (wire_length, displacement) =
        placement_costs(design, placements, None, fallback, component_lookup);
    wire_length + displacement * 0.02
}

fn placement_costs(
    design: &Design,
    placements: &BTreeMap<String, Pose>,
    candidate: Option<(&String, Pose)>,
    fallback_to_initial: bool,
    component_lookup: &HashMap<&str, &Component>,
) -> (f64, f64) {
    let pose_for = |component: &Component| {
        candidate
            .filter(|(id, _)| id.as_str() == component.id)
            .map(|(_, pose)| pose)
            .or_else(|| placements.get(&component.id).copied())
            .or(fallback_to_initial.then_some(component.initial_pose))
    };

    let mut hpwl = 0.0;
    for net in &design.nets {
        let mut points = Vec::new();
        for terminal in &net.terminals {
            let Some(component) = component_lookup.get(terminal.component_id.as_str()) else {
                continue;
            };
            let Some(pose) = pose_for(component) else {
                continue;
            };
            let Some(pad) = component.pads.iter().find(|pad| pad.id == terminal.pad_id) else {
                continue;
            };
            points.push(transform_point(pad.local_position, pose));
        }
        if points.len() >= 2 {
            let (mut min_x, mut max_x) = (points[0].x, points[0].x);
            let (mut min_y, mut max_y) = (points[0].y, points[0].y);
            for point in &points[1..] {
                min_x = min_x.min(point.x);
                max_x = max_x.max(point.x);
                min_y = min_y.min(point.y);
                max_y = max_y.max(point.y);
            }
            hpwl += max_x - min_x + max_y - min_y;
        }
    }

    let displacement = design
        .components
        .iter()
        .filter_map(|component| pose_for(component).map(|pose| (component, pose)))
        .map(|(component, pose)| {
            let dx = pose.x - component.initial_pose.x;
            let dy = pose.y - component.initial_pose.y;
            (dx * dx + dy * dy).sqrt()
        })
        .sum::<f64>();
    (hpwl, displacement)
}

fn outcome(
    design: &Design,
    seed: u64,
    status: PlacementStatus,
    placements: BTreeMap<String, Pose>,
    diagnostics: Vec<PlacementDiagnostic>,
    candidate_evaluations: u64,
    restarts_completed: usize,
) -> PlacementOutcome {
    let unplaced_component_ids = design
        .components
        .iter()
        .filter(|component| !placements.contains_key(&component.id))
        .map(|component| component.id.clone())
        .collect::<Vec<_>>();
    let component_lookup: HashMap<&str, &Component> = design
        .components
        .iter()
        .map(|component| (component.id.as_str(), component))
        .collect();
    PlacementOutcome {
        schema_version: SOLUTION_SCHEMA_VERSION,
        status,
        seed,
        metrics: PlacementMetrics {
            placed_components: placements.len(),
            total_components: design.components.len(),
            half_perimeter_wire_length_mm: placement_costs(
                design,
                &placements,
                None,
                false,
                &component_lookup,
            )
            .0,
            total_displacement_mm: design
                .components
                .iter()
                .filter_map(|component| {
                    placements.get(&component.id).map(|pose| {
                        let dx = pose.x - component.initial_pose.x;
                        let dy = pose.y - component.initial_pose.y;
                        (dx * dx + dy * dy).sqrt()
                    })
                })
                .sum(),
            candidate_evaluations,
            restarts_completed,
        },
        placements,
        unplaced_component_ids,
        diagnostics,
    }
}

fn placement_is_better(
    design: &Design,
    candidate: &BTreeMap<String, Pose>,
    current: &BTreeMap<String, Pose>,
    component_lookup: &HashMap<&str, &Component>,
) -> bool {
    candidate.len() > current.len()
        || (candidate.len() == current.len()
            && objective(design, candidate, false, component_lookup) + SCORE_EPSILON
                < objective(design, current, false, component_lookup))
}

fn normalize_rotation(rotation: f64) -> f64 {
    normalized_zero(rotation.rem_euclid(360.0))
}

fn rotations_equivalent(first: f64, second: f64) -> bool {
    let difference = (normalize_rotation(first) - normalize_rotation(second)).abs();
    difference <= SCORE_EPSILON || (360.0 - difference).abs() <= SCORE_EPSILON
}

fn normalized_zero(value: f64) -> f64 {
    if value.abs() <= SCORE_EPSILON {
        0.0
    } else {
        value
    }
}

fn deterministic_shuffle(values: &mut [usize], seed: u64) {
    let mut state = seed;
    for index in (1..values.len()).rev() {
        state = mix64(state);
        values.swap(index, state as usize % (index + 1));
    }
}

fn stable_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

fn mix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        JumperPolicy, LocalAabb, Net, Pad, PlacementRules, Polygon, RoutingRules, TerminalRef,
    };

    fn design() -> Design {
        let component = |id: &str, x: f64, locked: bool| Component {
            id: id.into(),
            reference: id.into(),
            footprint: "Test:THT".into(),
            envelope: LocalAabb {
                min_x: -1.0,
                min_y: -1.0,
                max_x: 1.0,
                max_y: 1.0,
            },
            pads: vec![Pad {
                id: "1".into(),
                local_position: Point { x: 0.0, y: 0.0 },
                size: Point { x: 2.0, y: 2.0 },
                drill_mm: Some(1.0),
                kind: PadKind::ThroughHole,
            }],
            initial_pose: Pose {
                x,
                y: 5.0,
                rotation_degrees: 0.0,
            },
            locked,
            allowed_rotations_degrees: vec![0.0, 90.0],
        };
        Design {
            schema_version: DESIGN_SCHEMA_VERSION,
            name: "test".into(),
            outline: Polygon {
                vertices: vec![
                    Point { x: 0.0, y: 0.0 },
                    Point { x: 20.0, y: 0.0 },
                    Point { x: 20.0, y: 10.0 },
                    Point { x: 0.0, y: 10.0 },
                ],
            },
            placement_keepouts: Vec::new(),
            components: vec![component("A", 3.0, true), component("B", 17.0, false)],
            nets: vec![Net {
                id: "N1".into(),
                name: "SIGNAL".into(),
                terminals: vec![
                    TerminalRef {
                        component_id: "A".into(),
                        pad_id: "1".into(),
                    },
                    TerminalRef {
                        component_id: "B".into(),
                        pad_id: "1".into(),
                    },
                ],
            }],
            placement_rules: PlacementRules {
                grid_mm: 1.0,
                component_clearance_mm: 1.0,
                edge_clearance_mm: 1.0,
            },
            routing_rules: RoutingRules {
                grid_mm: 0.5,
                trace_width_mm: 1.0,
                trace_clearance_mm: 0.8,
                edge_clearance_mm: 1.0,
            },
            jumper_policy: JumperPolicy {
                mode: JumperMode::Forbidden,
                approvals: Vec::new(),
            },
        }
    }

    #[test]
    fn deterministic_placement_preserves_fixed_component() {
        let design = design();
        let first = place(&design, PlacementOptions::default()).unwrap();
        let second = place(&design, PlacementOptions::default()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.status, PlacementStatus::Complete);
        assert_eq!(first.placements["A"], design.components[0].initial_pose);
        assert_ne!(first.placements["A"], first.placements["B"]);
    }

    #[test]
    fn reports_infeasible_fixed_geometry() {
        let mut design = design();
        design.components[0].initial_pose.x = 0.0;
        let outcome = place(&design, PlacementOptions::default()).unwrap();
        assert_eq!(outcome.status, PlacementStatus::InfeasibleFixedConstraints);
        assert_eq!(outcome.diagnostics[0].code, "component_outside_outline");
    }

    #[test]
    fn rejects_cross_net_terminal_assignment() {
        let mut design = design();
        design.nets.push(Net {
            id: "N2".into(),
            name: "OTHER".into(),
            terminals: vec![TerminalRef {
                component_id: "A".into(),
                pad_id: "1".into(),
            }],
        });
        let error = validate_design(&design).unwrap_err();
        assert!(error.to_string().contains("already assigned"));
    }
}
