use crate::geometry::{
    envelope_inside_outline, point_is_finite, polygon_contains_point, pose_is_finite,
    transform_point,
};
use crate::{
    Design, JumperMode, PlacementOutcome, PlacementStatus, Point, RouteSolution,
    SOLUTION_SCHEMA_VERSION,
};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::error::Error;
use std::fmt;

const EPSILON: f64 = 1.0e-7;

#[derive(Debug)]
struct PadObstacle {
    label: String,
    center: Point,
    radius_mm: f64,
    net_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteValidationError {
    messages: Vec<String>,
}

impl RouteValidationError {
    pub fn messages(&self) -> &[String] {
        &self.messages
    }
}

impl fmt::Display for RouteValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.messages.len() == 1 {
            formatter.write_str(&self.messages[0])
        } else {
            write!(
                formatter,
                "route validation failed with {} issues: {}",
                self.messages.len(),
                self.messages.join("; ")
            )
        }
    }
}

impl Error for RouteValidationError {}

/// Validate a route candidate independently of the algorithm that produced it.
///
/// This is intentionally not a KiCad DRC replacement. It verifies the canonical
/// single-layer contract before the candidate is emitted for KiCad parsing and
/// DRC, including connectivity, jumper authority, outline containment, and
/// centerline clearances between different nets.
pub fn validate_route_solution(
    design: &Design,
    placement: &PlacementOutcome,
    solution: &RouteSolution,
) -> Result<(), RouteValidationError> {
    let mut issues = Vec::new();
    if let Err(error) = crate::validate_design(design) {
        issues.extend(
            error
                .messages()
                .iter()
                .map(|message| format!("design: {message}")),
        );
    }
    if placement.schema_version != SOLUTION_SCHEMA_VERSION {
        issues.push(format!(
            "placement.schema_version: expected {}, received {}",
            SOLUTION_SCHEMA_VERSION, placement.schema_version
        ));
    }
    if placement.status != PlacementStatus::Complete {
        issues.push("placement.status: routing requires a complete legal placement".into());
    }
    if placement.placements.len() != design.components.len()
        || design
            .components
            .iter()
            .any(|component| !placement.placements.contains_key(&component.id))
    {
        issues
            .push("placement.placements: must contain every design component exactly once".into());
    }
    if placement
        .placements
        .values()
        .any(|pose| !pose_is_finite(*pose))
    {
        issues.push("placement.placements: every pose must be finite".into());
    }
    if solution.schema_version != SOLUTION_SCHEMA_VERSION {
        issues.push(format!(
            "solution.schema_version: expected {}, received {}",
            SOLUTION_SCHEMA_VERSION, solution.schema_version
        ));
    }

    let net_ids = design
        .nets
        .iter()
        .map(|net| net.id.as_str())
        .collect::<HashSet<_>>();
    let terminal_nets = design
        .nets
        .iter()
        .flat_map(|net| {
            net.terminals.iter().map(move |terminal| {
                (
                    (terminal.component_id.as_str(), terminal.pad_id.as_str()),
                    net.id.as_str(),
                )
            })
        })
        .collect::<HashMap<_, _>>();
    let mut pad_obstacles = Vec::new();
    for component in &design.components {
        let Some(pose) = placement.placements.get(&component.id).copied() else {
            continue;
        };
        for pad in &component.pads {
            pad_obstacles.push(PadObstacle {
                label: format!("{}.{}", component.reference, pad.id),
                center: transform_point(pad.local_position, pose),
                radius_mm: (pad.size.x * pad.size.x + pad.size.y * pad.size.y).sqrt() * 0.5,
                net_id: terminal_nets
                    .get(&(component.id.as_str(), pad.id.as_str()))
                    .map(|net_id| (*net_id).to_string()),
            });
        }
    }
    for (index, segment) in solution.segments.iter().enumerate() {
        let path = format!("segments[{index}]");
        if !net_ids.contains(segment.net_id.as_str()) {
            issues.push(format!("{path}.net_id: unknown net {}", segment.net_id));
        }
        if !point_is_finite(segment.start) || !point_is_finite(segment.end) {
            issues.push(format!("{path}: endpoints must be finite"));
            continue;
        }
        if distance(segment.start, segment.end) <= EPSILON {
            issues.push(format!("{path}: zero-length segments are invalid"));
        }
        if !segment.width_mm.is_finite()
            || segment.width_mm + EPSILON < design.routing_rules.trace_width_mm
        {
            issues.push(format!(
                "{path}.width_mm: must be finite and at least the design trace width"
            ));
            continue;
        }
        let half_envelope = segment.width_mm * 0.5 + design.routing_rules.edge_clearance_mm;
        let trace_envelope = segment_rectangle(segment.start, segment.end, half_envelope);
        if trace_envelope.is_empty()
            || !envelope_inside_outline(&trace_envelope, &design.outline.vertices, 0.0)
        {
            issues.push(format!(
                "{path}: copper width plus edge clearance leaves the board outline"
            ));
        }
        for pad in &pad_obstacles {
            if pad.net_id.as_deref() == Some(segment.net_id.as_str()) {
                continue;
            }
            let required =
                segment.width_mm * 0.5 + pad.radius_mm + design.routing_rules.trace_clearance_mm;
            if point_segment_distance(pad.center, segment.start, segment.end) + EPSILON < required {
                issues.push(format!(
                    "{path}: different-net or NPTH clearance violation to pad {}",
                    pad.label
                ));
            }
        }
    }

    for first in 0..solution.segments.len() {
        for second in (first + 1)..solution.segments.len() {
            let a = &solution.segments[first];
            let b = &solution.segments[second];
            if a.net_id == b.net_id
                || !net_ids.contains(a.net_id.as_str())
                || !net_ids.contains(b.net_id.as_str())
            {
                continue;
            }
            let required =
                a.width_mm * 0.5 + b.width_mm * 0.5 + design.routing_rules.trace_clearance_mm;
            if segment_distance(a.start, a.end, b.start, b.end) + EPSILON < required {
                issues.push(format!(
                    "segments[{first}] and segments[{second}]: different-net clearance violation"
                ));
            }
        }
    }

    let approvals_by_id = design
        .jumper_policy
        .approvals
        .iter()
        .map(|approval| (approval.approval_id.as_str(), approval))
        .collect::<HashMap<_, _>>();
    let mut jumper_counts = HashMap::<&str, usize>::new();
    for (index, jumper) in solution.jumpers.iter().enumerate() {
        let path = format!("jumpers[{index}]");
        if !net_ids.contains(jumper.net_id.as_str()) {
            issues.push(format!("{path}.net_id: unknown net {}", jumper.net_id));
        }
        if !point_is_finite(jumper.start) || !point_is_finite(jumper.end) {
            issues.push(format!("{path}: endpoints must be finite"));
        } else {
            if distance(jumper.start, jumper.end) <= EPSILON {
                issues.push(format!("{path}: zero-length jumpers are invalid"));
            }
            if !polygon_contains_point(&design.outline.vertices, jumper.start)
                || !polygon_contains_point(&design.outline.vertices, jumper.end)
            {
                issues.push(format!(
                    "{path}: endpoints must be inside the board outline"
                ));
            }
        }
        if point_is_finite(jumper.start)
            && !jumper_endpoint_attaches(
                design,
                placement,
                solution,
                index,
                jumper.net_id.as_str(),
                jumper.start,
            )
        {
            issues.push(format!("{path}.start: does not attach to its declared net"));
        }
        if point_is_finite(jumper.end)
            && !jumper_endpoint_attaches(
                design,
                placement,
                solution,
                index,
                jumper.net_id.as_str(),
                jumper.end,
            )
        {
            issues.push(format!("{path}.end: does not attach to its declared net"));
        }
        if jumper.approval_id.trim().is_empty() {
            issues.push(format!("{path}.approval_id: must not be empty"));
        } else {
            *jumper_counts
                .entry(jumper.approval_id.as_str())
                .or_default() += 1;
            if design.jumper_policy.mode == JumperMode::UserApprovedOnly {
                match approvals_by_id.get(jumper.approval_id.as_str()) {
                    Some(approval) if approval.net_id == jumper.net_id => {}
                    Some(approval) => issues.push(format!(
                        "{path}.approval_id: approval {} belongs to net {}, not {}",
                        jumper.approval_id, approval.net_id, jumper.net_id
                    )),
                    None => issues.push(format!(
                        "{path}.approval_id: no user approval named {}",
                        jumper.approval_id
                    )),
                }
            }
        }
    }
    match design.jumper_policy.mode {
        JumperMode::Forbidden if !solution.jumpers.is_empty() => {
            issues.push("jumpers: design policy forbids all jumpers".into());
        }
        JumperMode::UserApprovedOnly => {
            for (approval_id, count) in jumper_counts {
                if let Some(approval) = approvals_by_id.get(approval_id) {
                    if count > approval.max_count {
                        issues.push(format!(
                            "jumpers: approval {approval_id} permits {} jumper(s), but {count} were used",
                            approval.max_count
                        ));
                    }
                }
            }
        }
        JumperMode::Forbidden => {}
    }

    let derived_unrouted = derived_unrouted_nets(design, placement, solution);
    let declared_unrouted = solution
        .unrouted_net_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    for net_id in &declared_unrouted {
        if !net_ids.contains(net_id.as_str()) {
            issues.push(format!("unrouted_net_ids: contains unknown net {net_id}"));
        }
    }
    if declared_unrouted != derived_unrouted {
        issues.push(format!(
            "unrouted_net_ids: declared {:?}, but connectivity derives {:?}",
            declared_unrouted, derived_unrouted
        ));
    }

    let trace_length = solution
        .segments
        .iter()
        .map(|segment| distance(segment.start, segment.end))
        .sum::<f64>();
    let routed_count = design.nets.len().saturating_sub(derived_unrouted.len());
    if solution.metrics.routed_net_count != routed_count
        || solution.metrics.total_net_count != design.nets.len()
        || solution.metrics.jumper_count != solution.jumpers.len()
        || (solution.metrics.total_trace_length_mm - trace_length).abs() > 1.0e-6
    {
        issues
            .push("metrics: values do not match the independently recomputed route metrics".into());
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(RouteValidationError { messages: issues })
    }
}

fn derived_unrouted_nets(
    design: &Design,
    placement: &PlacementOutcome,
    solution: &RouteSolution,
) -> BTreeSet<String> {
    let component_lookup = design
        .components
        .iter()
        .map(|component| (component.id.as_str(), component))
        .collect::<HashMap<_, _>>();
    let mut unrouted = BTreeSet::new();
    for net in &design.nets {
        let mut nodes = Vec::new();
        let mut terminal_indices = Vec::new();
        for terminal in &net.terminals {
            let Some(component) = component_lookup.get(terminal.component_id.as_str()) else {
                continue;
            };
            let Some(pose) = placement.placements.get(&component.id) else {
                continue;
            };
            let Some(pad) = component.pads.iter().find(|pad| pad.id == terminal.pad_id) else {
                continue;
            };
            terminal_indices.push(nodes.len());
            nodes.push(transform_point(pad.local_position, *pose));
        }

        let mut conductive_edges = Vec::new();
        for segment in solution
            .segments
            .iter()
            .filter(|segment| segment.net_id == net.id)
        {
            let start = nodes.len();
            nodes.push(segment.start);
            nodes.push(segment.end);
            conductive_edges.push((start, start + 1, segment.start, segment.end, true));
        }
        for jumper in solution
            .jumpers
            .iter()
            .filter(|jumper| jumper.net_id == net.id)
        {
            let start = nodes.len();
            nodes.push(jumper.start);
            nodes.push(jumper.end);
            conductive_edges.push((start, start + 1, jumper.start, jumper.end, false));
        }

        if terminal_indices.len() <= 1 {
            continue;
        }
        let mut sets = DisjointSet::new(nodes.len());
        for (first, second, _, _, _) in &conductive_edges {
            sets.union(*first, *second);
        }
        for first in 0..nodes.len() {
            for second in (first + 1)..nodes.len() {
                if distance(nodes[first], nodes[second]) <= EPSILON {
                    sets.union(first, second);
                }
            }
        }
        for (node_index, node) in nodes.iter().copied().enumerate() {
            for (edge_start, _, start, end, is_copper) in &conductive_edges {
                if *is_copper && point_segment_distance(node, *start, *end) <= EPSILON {
                    sets.union(node_index, *edge_start);
                }
            }
        }
        for first in 0..conductive_edges.len() {
            for second in (first + 1)..conductive_edges.len() {
                let a = conductive_edges[first];
                let b = conductive_edges[second];
                if a.4 && b.4 && segments_intersect(a.2, a.3, b.2, b.3) {
                    sets.union(a.0, b.0);
                }
            }
        }
        let root = sets.find(terminal_indices[0]);
        if terminal_indices[1..]
            .iter()
            .any(|terminal| sets.find(*terminal) != root)
        {
            unrouted.insert(net.id.clone());
        }
    }
    unrouted
}

fn jumper_endpoint_attaches(
    design: &Design,
    placement: &PlacementOutcome,
    solution: &RouteSolution,
    jumper_index: usize,
    net_id: &str,
    endpoint: Point,
) -> bool {
    for net in design.nets.iter().filter(|net| net.id == net_id) {
        for terminal in &net.terminals {
            let Some(component) = design
                .components
                .iter()
                .find(|component| component.id == terminal.component_id)
            else {
                continue;
            };
            let Some(pose) = placement.placements.get(&component.id) else {
                continue;
            };
            let Some(pad) = component.pads.iter().find(|pad| pad.id == terminal.pad_id) else {
                continue;
            };
            if distance(endpoint, transform_point(pad.local_position, *pose)) <= EPSILON {
                return true;
            }
        }
    }
    if solution
        .segments
        .iter()
        .filter(|segment| segment.net_id == net_id)
        .any(|segment| point_segment_distance(endpoint, segment.start, segment.end) <= EPSILON)
    {
        return true;
    }
    solution
        .jumpers
        .iter()
        .enumerate()
        .filter(|(index, jumper)| *index != jumper_index && jumper.net_id == net_id)
        .any(|(_, jumper)| {
            distance(endpoint, jumper.start) <= EPSILON || distance(endpoint, jumper.end) <= EPSILON
        })
}

fn segment_rectangle(start: Point, end: Point, half_width: f64) -> Vec<Point> {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length = (dx * dx + dy * dy).sqrt();
    if length <= EPSILON || !half_width.is_finite() {
        return Vec::new();
    }
    let unit_x = dx / length;
    let unit_y = dy / length;
    let offset_x = -unit_y * half_width;
    let offset_y = unit_x * half_width;
    let start_x = start.x - unit_x * half_width;
    let start_y = start.y - unit_y * half_width;
    let end_x = end.x + unit_x * half_width;
    let end_y = end.y + unit_y * half_width;
    vec![
        Point {
            x: start_x + offset_x,
            y: start_y + offset_y,
        },
        Point {
            x: end_x + offset_x,
            y: end_y + offset_y,
        },
        Point {
            x: end_x - offset_x,
            y: end_y - offset_y,
        },
        Point {
            x: start_x - offset_x,
            y: start_y - offset_y,
        },
    ]
}

fn distance(first: Point, second: Point) -> f64 {
    ((first.x - second.x).powi(2) + (first.y - second.y).powi(2)).sqrt()
}

fn orientation(a: Point, b: Point, c: Point) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

fn point_on_segment(point: Point, start: Point, end: Point) -> bool {
    orientation(start, end, point).abs() <= EPSILON
        && point.x >= start.x.min(end.x) - EPSILON
        && point.x <= start.x.max(end.x) + EPSILON
        && point.y >= start.y.min(end.y) - EPSILON
        && point.y <= start.y.max(end.y) + EPSILON
}

fn segments_intersect(a: Point, b: Point, c: Point, d: Point) -> bool {
    let ab_c = orientation(a, b, c);
    let ab_d = orientation(a, b, d);
    let cd_a = orientation(c, d, a);
    let cd_b = orientation(c, d, b);
    if ((ab_c > EPSILON && ab_d < -EPSILON) || (ab_c < -EPSILON && ab_d > EPSILON))
        && ((cd_a > EPSILON && cd_b < -EPSILON) || (cd_a < -EPSILON && cd_b > EPSILON))
    {
        return true;
    }
    (ab_c.abs() <= EPSILON && point_on_segment(c, a, b))
        || (ab_d.abs() <= EPSILON && point_on_segment(d, a, b))
        || (cd_a.abs() <= EPSILON && point_on_segment(a, c, d))
        || (cd_b.abs() <= EPSILON && point_on_segment(b, c, d))
}

fn segment_distance(a: Point, b: Point, c: Point, d: Point) -> f64 {
    if segments_intersect(a, b, c, d) {
        return 0.0;
    }
    point_segment_distance(a, c, d)
        .min(point_segment_distance(b, c, d))
        .min(point_segment_distance(c, a, b))
        .min(point_segment_distance(d, a, b))
}

fn point_segment_distance(point: Point, start: Point, end: Point) -> f64 {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length_squared = dx * dx + dy * dy;
    if length_squared <= EPSILON * EPSILON {
        return distance(point, start);
    }
    let projection =
        (((point.x - start.x) * dx + (point.y - start.y) * dy) / length_squared).clamp(0.0, 1.0);
    distance(
        point,
        Point {
            x: start.x + projection * dx,
            y: start.y + projection * dy,
        },
    )
}

struct DisjointSet {
    parents: Vec<usize>,
}

impl DisjointSet {
    fn new(length: usize) -> Self {
        Self {
            parents: (0..length).collect(),
        }
    }

    fn find(&mut self, index: usize) -> usize {
        let parent = self.parents[index];
        if parent != index {
            self.parents[index] = self.find(parent);
        }
        self.parents[index]
    }

    fn union(&mut self, first: usize, second: usize) {
        let first_root = self.find(first);
        let second_root = self.find(second);
        if first_root != second_root {
            self.parents[second_root] = first_root;
        }
    }
}
