use crate::geometry::{envelope_inside_outline, polygon_bounds, pose_is_finite, transform_point};
use crate::routing::{
    derived_bend_count, distance, point_segment_distance, segment_distance, segment_rectangle,
};
use crate::{
    validate_design, validate_route_solution, CopperLayer, Design, Net, PlacementOutcome,
    PlacementStatus, Point, RouteMetrics, RouteSegment, RouteSolution, RoutingOptions,
    SOLUTION_SCHEMA_VERSION,
};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};
use std::error::Error;
use std::fmt;

const ROUTING_EPSILON: f64 = 1.0e-7;
const COST_SCALE: u64 = 1_000;
const DIAGONAL_COST: u64 = 1_414;
const NO_DIRECTION: u8 = 8;
const MAX_SEARCH_NODES: usize = 5_000_000;
const MAX_REROUTE_PASSES: usize = 64;
const MAX_GRID_COORDINATE: f64 = 1_000_000_000.0;
const DIRECTIONS: [(i64, i64); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingError {
    messages: Vec<String>,
}

impl RoutingError {
    pub fn messages(&self) -> &[String] {
        &self.messages
    }
}

impl fmt::Display for RoutingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.messages.len() == 1 {
            formatter.write_str(&self.messages[0])
        } else {
            write!(
                formatter,
                "routing failed with {} issues: {}",
                self.messages.len(),
                self.messages.join("; ")
            )
        }
    }
}

impl Error for RoutingError {}

#[derive(Debug, Clone)]
struct PadObstacle {
    center: Point,
    radius_mm: f64,
    net_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct GridPoint {
    x: i64,
    y: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct GridEdge {
    first: GridPoint,
    second: GridPoint,
}

impl GridEdge {
    fn new(first: GridPoint, second: GridPoint) -> Self {
        if first <= second {
            Self { first, second }
        } else {
            Self {
                first: second,
                second: first,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct SearchState {
    node: GridPoint,
    direction: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct QueueEntry {
    estimated_total: u64,
    cost: u64,
    tie: u64,
    state: SearchState,
}

#[derive(Debug, Clone, Copy)]
struct GridBounds {
    min_x: i64,
    min_y: i64,
    max_x: i64,
    max_y: i64,
}

impl GridBounds {
    fn contains(self, point: GridPoint) -> bool {
        point.x >= self.min_x
            && point.x <= self.max_x
            && point.y >= self.min_y
            && point.y <= self.max_y
    }
}

struct SearchContext<'a> {
    design: &'a Design,
    net_id: &'a str,
    pad_obstacles: &'a [PadObstacle],
    existing_segments: &'a [RouteSegment],
    history: &'a HashMap<GridEdge, u32>,
    options: RoutingOptions,
    bounds: GridBounds,
    seed: u64,
}

struct SearchPath {
    points: Vec<Point>,
    grid_edges: Vec<GridEdge>,
}

struct RoutedNet {
    segments: Vec<RouteSegment>,
    grid_edges: Vec<GridEdge>,
}

struct PassCandidate {
    solution: RouteSolution,
    grid_edges: Vec<GridEdge>,
}

/// Generate deterministic B.Cu routes and independently validate the result.
///
/// Each pass routes nets sequentially with an eight-neighbor A* search. Failed
/// nets are promoted on the next pass, every earlier route is ripped up, and
/// historical path costs encourage alternate corridors. Exhaustion is a valid
/// partial result represented by `unrouted_net_ids`.
pub fn route(
    design: &Design,
    placement: &PlacementOutcome,
    options: RoutingOptions,
) -> Result<RouteSolution, RoutingError> {
    let bounds = validate_inputs(design, placement, options)?;
    let pad_obstacles = pad_obstacles(design, placement);
    let terminal_points = design
        .nets
        .iter()
        .map(|net| net_terminal_points(design, placement, net))
        .collect::<Vec<_>>();
    let mut history = HashMap::<GridEdge, u32>::new();
    let mut previously_unrouted = BTreeSet::<String>::new();
    let mut best: Option<PassCandidate> = None;

    for pass in 0..options.reroute_passes {
        let mut order = (0..design.nets.len()).collect::<Vec<_>>();
        order.sort_by(|first, second| {
            let first_net = &design.nets[*first];
            let second_net = &design.nets[*second];
            previously_unrouted
                .contains(&second_net.id)
                .cmp(&previously_unrouted.contains(&first_net.id))
                .then_with(|| {
                    terminal_points[*second]
                        .len()
                        .cmp(&terminal_points[*first].len())
                })
                .then_with(|| {
                    terminal_span(&terminal_points[*second])
                        .total_cmp(&terminal_span(&terminal_points[*first]))
                })
                .then_with(|| {
                    net_pass_tie(options.seed, pass, &first_net.id).cmp(&net_pass_tie(
                        options.seed,
                        pass,
                        &second_net.id,
                    ))
                })
                .then_with(|| first_net.id.cmp(&second_net.id))
        });

        let mut segments = Vec::new();
        let mut pass_edges = Vec::new();
        let mut unrouted = BTreeSet::new();
        for net_index in order {
            let net = &design.nets[net_index];
            let search_seed = mix64(
                options.seed
                    ^ stable_hash(net.id.as_bytes())
                    ^ (pass as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15),
            );
            let context = SearchContext {
                design,
                net_id: &net.id,
                pad_obstacles: &pad_obstacles,
                existing_segments: &segments,
                history: &history,
                options,
                bounds,
                seed: search_seed,
            };
            match route_net(net, &terminal_points[net_index], &context) {
                Some(routed) => {
                    pass_edges.extend(routed.grid_edges);
                    segments.extend(routed.segments);
                }
                None => {
                    unrouted.insert(net.id.clone());
                }
            }
        }

        let trace_length = segments
            .iter()
            .map(|segment| distance(segment.start, segment.end))
            .sum::<f64>();
        let mut solution = RouteSolution {
            schema_version: SOLUTION_SCHEMA_VERSION,
            segments,
            jumpers: Vec::new(),
            unrouted_net_ids: unrouted.iter().cloned().collect(),
            metrics: RouteMetrics {
                routed_net_count: design.nets.len().saturating_sub(unrouted.len()),
                total_net_count: design.nets.len(),
                total_trace_length_mm: trace_length,
                bend_count: 0,
                jumper_count: 0,
            },
        };
        solution.metrics.bend_count = derived_bend_count(&solution);
        validate_route_solution(design, placement, &solution).map_err(|error| RoutingError {
            messages: error
                .messages()
                .iter()
                .map(|message| format!("generated candidate: {message}"))
                .collect(),
        })?;

        let candidate = PassCandidate {
            solution,
            grid_edges: pass_edges,
        };
        if best
            .as_ref()
            .is_none_or(|current| pass_is_better(&candidate.solution, &current.solution))
        {
            best = Some(PassCandidate {
                solution: candidate.solution.clone(),
                grid_edges: candidate.grid_edges.clone(),
            });
        }
        for edge in &candidate.grid_edges {
            let usage = history.entry(*edge).or_default();
            *usage = usage.saturating_add(1);
        }
        previously_unrouted = unrouted;
    }

    Ok(best
        .expect("validated options require at least one reroute pass")
        .solution)
}

fn validate_inputs(
    design: &Design,
    placement: &PlacementOutcome,
    options: RoutingOptions,
) -> Result<GridBounds, RoutingError> {
    let mut messages = Vec::new();
    if let Err(error) = validate_design(design) {
        messages.extend(
            error
                .messages()
                .iter()
                .map(|message| format!("design: {message}")),
        );
    }
    if placement.schema_version != SOLUTION_SCHEMA_VERSION {
        messages.push(format!(
            "placement.schema_version: expected {}, received {}",
            SOLUTION_SCHEMA_VERSION, placement.schema_version
        ));
    }
    if placement.status != PlacementStatus::Complete {
        messages.push("placement.status: routing requires a complete legal placement".into());
    }
    if placement.placements.len() != design.components.len()
        || design
            .components
            .iter()
            .any(|component| !placement.placements.contains_key(&component.id))
    {
        messages.push("placement.placements: must contain every design component".into());
    }
    if placement
        .placements
        .values()
        .any(|pose| !pose_is_finite(*pose))
    {
        messages.push("placement.placements: every pose must be finite".into());
    }
    if options.max_search_nodes == 0 || options.max_search_nodes > MAX_SEARCH_NODES {
        messages.push(format!(
            "routing_options.max_search_nodes: must be between 1 and {MAX_SEARCH_NODES}"
        ));
    }
    if options.reroute_passes == 0 || options.reroute_passes > MAX_REROUTE_PASSES {
        messages.push(format!(
            "routing_options.reroute_passes: must be between 1 and {MAX_REROUTE_PASSES}"
        ));
    }
    for (name, value) in [
        ("bend_penalty_mm", options.bend_penalty_mm),
        ("congestion_penalty_mm", options.congestion_penalty_mm),
    ] {
        if !value.is_finite() || value < 0.0 {
            messages.push(format!(
                "routing_options.{name}: must be finite and nonnegative"
            ));
        }
    }

    let bounds = polygon_bounds(&design.outline).and_then(|bounds| {
        let grid = design.routing_rules.grid_mm;
        let coordinates = [
            (bounds.min_x / grid).floor(),
            (bounds.min_y / grid).floor(),
            (bounds.max_x / grid).ceil(),
            (bounds.max_y / grid).ceil(),
        ];
        coordinates
            .iter()
            .all(|value| value.is_finite() && value.abs() <= MAX_GRID_COORDINATE)
            .then_some(GridBounds {
                min_x: coordinates[0] as i64,
                min_y: coordinates[1] as i64,
                max_x: coordinates[2] as i64,
                max_y: coordinates[3] as i64,
            })
    });
    if bounds.is_none() {
        messages.push(format!(
            "routing grid exceeds the supported coordinate range of {MAX_GRID_COORDINATE:.0}"
        ));
    }

    if messages.is_empty() {
        Ok(bounds.expect("validated grid bounds"))
    } else {
        Err(RoutingError { messages })
    }
}

fn pad_obstacles(design: &Design, placement: &PlacementOutcome) -> Vec<PadObstacle> {
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
    design
        .components
        .iter()
        .flat_map(|component| {
            let pose = placement.placements[&component.id];
            let terminal_nets = &terminal_nets;
            component.pads.iter().map(move |pad| PadObstacle {
                center: transform_point(pad.local_position, pose),
                radius_mm: (pad.size.x * pad.size.x + pad.size.y * pad.size.y).sqrt() * 0.5,
                net_id: terminal_nets
                    .get(&(component.id.as_str(), pad.id.as_str()))
                    .map(|net_id| (*net_id).to_string()),
            })
        })
        .collect()
}

fn net_terminal_points(design: &Design, placement: &PlacementOutcome, net: &Net) -> Vec<Point> {
    let component_lookup = design
        .components
        .iter()
        .map(|component| (component.id.as_str(), component))
        .collect::<HashMap<_, _>>();
    let mut points = net
        .terminals
        .iter()
        .filter_map(|terminal| {
            let component = component_lookup.get(terminal.component_id.as_str())?;
            let pad = component
                .pads
                .iter()
                .find(|pad| pad.id == terminal.pad_id)?;
            Some(transform_point(
                pad.local_position,
                placement.placements[&component.id],
            ))
        })
        .collect::<Vec<_>>();
    points.sort_by(point_order);
    points.dedup_by(|first, second| distance(*first, *second) <= ROUTING_EPSILON);
    points
}

fn route_net(net: &Net, terminals: &[Point], context: &SearchContext<'_>) -> Option<RoutedNet> {
    if terminals.len() <= 1 {
        return Some(RoutedNet {
            segments: Vec::new(),
            grid_edges: Vec::new(),
        });
    }

    let mut tree_points = vec![terminals[0]];
    let mut tree_keys = HashSet::from([point_key(terminals[0])]);
    let mut remaining = terminals[1..].to_vec();
    let mut segments = Vec::<RouteSegment>::new();
    let mut grid_edges = Vec::new();

    while !remaining.is_empty() {
        let selected = remaining
            .iter()
            .enumerate()
            .min_by(|(_, first), (_, second)| {
                nearest_distance(**first, &tree_points)
                    .total_cmp(&nearest_distance(**second, &tree_points))
                    .then_with(|| point_order(first, second))
            })
            .map(|(index, _)| index)
            .expect("remaining terminals are nonempty");
        let source = remaining.remove(selected);
        let mut connection_targets = tree_points.clone();
        let mut connection_keys = tree_keys.clone();
        for segment in &segments {
            let projection = point_segment_projection(source, segment.start, segment.end);
            if connection_keys.insert(point_key(projection)) {
                connection_targets.push(projection);
            }
        }
        let path = astar_connect(source, &connection_targets, context)?;
        let compressed = compress_path(&path.points);
        for pair in compressed.windows(2) {
            segments.push(RouteSegment {
                net_id: net.id.clone(),
                start: pair[0],
                end: pair[1],
                width_mm: context.design.routing_rules.trace_width_mm,
                layer: CopperLayer::Back,
            });
        }
        grid_edges.extend(path.grid_edges);
        for point in path.points {
            if tree_keys.insert(point_key(point)) {
                tree_points.push(point);
            }
        }
    }

    Some(RoutedNet {
        segments,
        grid_edges,
    })
}

fn astar_connect(
    source: Point,
    targets: &[Point],
    context: &SearchContext<'_>,
) -> Option<SearchPath> {
    let mut direct_targets = targets.to_vec();
    direct_targets.sort_by(|first, second| {
        distance(source, *first)
            .total_cmp(&distance(source, *second))
            .then_with(|| point_order(first, second))
    });
    for target in direct_targets {
        if distance(source, target) <= ROUTING_EPSILON {
            return Some(SearchPath {
                points: vec![source],
                grid_edges: Vec::new(),
            });
        }
        if segment_is_legal(source, target, context) {
            return Some(SearchPath {
                points: vec![source, target],
                grid_edges: Vec::new(),
            });
        }
    }

    let grid = context.design.routing_rules.grid_mm;
    let mut goal_anchors = BTreeMap::<GridPoint, Vec<Point>>::new();
    for target in targets {
        for anchor in point_anchors(*target, grid, context.bounds) {
            let anchor_point = grid_point(anchor, grid);
            if distance(anchor_point, *target) <= ROUTING_EPSILON
                || segment_is_legal(anchor_point, *target, context)
            {
                goal_anchors.entry(anchor).or_default().push(*target);
            }
        }
    }
    if goal_anchors.is_empty() {
        return None;
    }
    for target_list in goal_anchors.values_mut() {
        target_list.sort_by(point_order);
        target_list.dedup_by(|first, second| distance(*first, *second) <= ROUTING_EPSILON);
    }
    let goal_nodes = goal_anchors.keys().copied().collect::<Vec<_>>();

    let mut queue = BinaryHeap::<Reverse<QueueEntry>>::new();
    let mut costs = HashMap::<SearchState, u64>::new();
    let mut parents = HashMap::<SearchState, Option<SearchState>>::new();
    for anchor in point_anchors(source, grid, context.bounds) {
        let anchor_point = grid_point(anchor, grid);
        if distance(source, anchor_point) > ROUTING_EPSILON
            && !segment_is_legal(source, anchor_point, context)
        {
            continue;
        }
        let direction = direction_between(source, anchor_point).unwrap_or(NO_DIRECTION);
        let state = SearchState {
            node: anchor,
            direction,
        };
        let cost = scaled_distance(source, anchor_point, grid);
        if costs.get(&state).is_some_and(|existing| *existing <= cost) {
            continue;
        }
        costs.insert(state, cost);
        parents.insert(state, None);
        queue.push(Reverse(QueueEntry {
            estimated_total: cost.saturating_add(octile_to_nearest(anchor, &goal_nodes)),
            cost,
            tie: state_tie(context.seed, state),
            state,
        }));
    }
    if queue.is_empty() {
        return None;
    }

    let bend_cost = scaled_penalty(context.options.bend_penalty_mm, grid);
    let congestion_cost = scaled_penalty(context.options.congestion_penalty_mm, grid);
    let mut expansions = 0;
    while let Some(Reverse(entry)) = queue.pop() {
        if costs.get(&entry.state).copied() != Some(entry.cost) {
            continue;
        }
        if let Some(targets_at_anchor) = goal_anchors.get(&entry.state.node) {
            let anchor_point = grid_point(entry.state.node, grid);
            let target = targets_at_anchor
                .iter()
                .min_by_key(|target| {
                    let direction = direction_between(anchor_point, **target);
                    scaled_distance(anchor_point, **target, grid).saturating_add(
                        direction
                            .filter(|direction| {
                                entry.state.direction != NO_DIRECTION
                                    && *direction != entry.state.direction
                            })
                            .map(|_| bend_cost)
                            .unwrap_or_default(),
                    )
                })
                .copied()
                .expect("goal anchor has a target");
            return Some(reconstruct_path(
                source,
                target,
                entry.state,
                &parents,
                grid,
            ));
        }
        if expansions >= context.options.max_search_nodes {
            return None;
        }
        expansions += 1;

        let current_point = grid_point(entry.state.node, grid);
        for (direction, (dx, dy)) in DIRECTIONS.iter().copied().enumerate() {
            let Some(next_x) = entry.state.node.x.checked_add(dx) else {
                continue;
            };
            let Some(next_y) = entry.state.node.y.checked_add(dy) else {
                continue;
            };
            let next_node = GridPoint {
                x: next_x,
                y: next_y,
            };
            if !context.bounds.contains(next_node) {
                continue;
            }
            let next_point = grid_point(next_node, grid);
            if !segment_is_legal(current_point, next_point, context) {
                continue;
            }
            let edge = GridEdge::new(entry.state.node, next_node);
            let step_cost = if dx != 0 && dy != 0 {
                DIAGONAL_COST
            } else {
                COST_SCALE
            };
            let next_direction = direction as u8;
            let turn_cost = if entry.state.direction != NO_DIRECTION
                && entry.state.direction != next_direction
            {
                bend_cost
            } else {
                0
            };
            let historical_cost =
                u64::from(context.history.get(&edge).copied().unwrap_or_default())
                    .saturating_mul(congestion_cost);
            let next_cost = entry
                .cost
                .saturating_add(step_cost)
                .saturating_add(turn_cost)
                .saturating_add(historical_cost);
            let next_state = SearchState {
                node: next_node,
                direction: next_direction,
            };
            if costs
                .get(&next_state)
                .is_some_and(|existing| *existing <= next_cost)
            {
                continue;
            }
            costs.insert(next_state, next_cost);
            parents.insert(next_state, Some(entry.state));
            queue.push(Reverse(QueueEntry {
                estimated_total: next_cost
                    .saturating_add(octile_to_nearest(next_node, &goal_nodes)),
                cost: next_cost,
                tie: state_tie(context.seed, next_state),
                state: next_state,
            }));
        }
    }
    None
}

fn reconstruct_path(
    source: Point,
    target: Point,
    goal: SearchState,
    parents: &HashMap<SearchState, Option<SearchState>>,
    grid: f64,
) -> SearchPath {
    let mut states = vec![goal];
    let mut cursor = goal;
    while let Some(Some(parent)) = parents.get(&cursor) {
        states.push(*parent);
        cursor = *parent;
    }
    states.reverse();

    let mut points = vec![source];
    for state in &states {
        let point = grid_point(state.node, grid);
        if points
            .last()
            .is_none_or(|previous| distance(*previous, point) > ROUTING_EPSILON)
        {
            points.push(point);
        }
    }
    if points
        .last()
        .is_none_or(|previous| distance(*previous, target) > ROUTING_EPSILON)
    {
        points.push(target);
    }
    let grid_edges = states
        .windows(2)
        .map(|pair| GridEdge::new(pair[0].node, pair[1].node))
        .collect();
    SearchPath { points, grid_edges }
}

fn segment_is_legal(start: Point, end: Point, context: &SearchContext<'_>) -> bool {
    if distance(start, end) <= ROUTING_EPSILON {
        return false;
    }
    let rules = &context.design.routing_rules;
    let half_envelope = rules.trace_width_mm * 0.5 + rules.edge_clearance_mm;
    let envelope = segment_rectangle(start, end, half_envelope);
    if envelope.is_empty()
        || !envelope_inside_outline(&envelope, &context.design.outline.vertices, 0.0)
    {
        return false;
    }
    if context.pad_obstacles.iter().any(|pad| {
        pad.net_id.as_deref() != Some(context.net_id)
            && point_segment_distance(pad.center, start, end) + ROUTING_EPSILON
                < rules.trace_width_mm * 0.5 + pad.radius_mm + rules.trace_clearance_mm
    }) {
        return false;
    }
    !context.existing_segments.iter().any(|segment| {
        segment_distance(start, end, segment.start, segment.end) + ROUTING_EPSILON
            < rules.trace_width_mm * 0.5 + segment.width_mm * 0.5 + rules.trace_clearance_mm
    })
}

fn point_anchors(point: Point, grid: f64, bounds: GridBounds) -> Vec<GridPoint> {
    let scaled_x = point.x / grid;
    let scaled_y = point.y / grid;
    let x_values = [scaled_x.floor() as i64, scaled_x.ceil() as i64];
    let y_values = [scaled_y.floor() as i64, scaled_y.ceil() as i64];
    let mut anchors = Vec::new();
    for x in x_values {
        for y in y_values {
            let anchor = GridPoint { x, y };
            if bounds.contains(anchor) && !anchors.contains(&anchor) {
                anchors.push(anchor);
            }
        }
    }
    anchors.sort_by(|first, second| {
        distance(point, grid_point(*first, grid))
            .total_cmp(&distance(point, grid_point(*second, grid)))
            .then_with(|| first.cmp(second))
    });
    anchors
}

fn grid_point(point: GridPoint, grid: f64) -> Point {
    Point {
        x: normalized_zero(point.x as f64 * grid),
        y: normalized_zero(point.y as f64 * grid),
    }
}

fn direction_between(first: Point, second: Point) -> Option<u8> {
    let dx = second.x - first.x;
    let dy = second.y - first.y;
    if dx.abs() <= ROUTING_EPSILON && dy.abs() <= ROUTING_EPSILON {
        return None;
    }
    let sx = if dx.abs() <= ROUTING_EPSILON {
        0
    } else if dx > 0.0 {
        1
    } else {
        -1
    };
    let sy = if dy.abs() <= ROUTING_EPSILON {
        0
    } else if dy > 0.0 {
        1
    } else {
        -1
    };
    DIRECTIONS
        .iter()
        .position(|direction| *direction == (sx, sy))
        .map(|index| index as u8)
}

fn octile_to_nearest(point: GridPoint, goals: &[GridPoint]) -> u64 {
    goals
        .iter()
        .map(|goal| {
            let dx = point.x.abs_diff(goal.x);
            let dy = point.y.abs_diff(goal.y);
            let diagonal = dx.min(dy);
            diagonal.saturating_mul(DIAGONAL_COST).saturating_add(
                dx.max(dy)
                    .saturating_sub(diagonal)
                    .saturating_mul(COST_SCALE),
            )
        })
        .min()
        .unwrap_or_default()
}

fn scaled_distance(first: Point, second: Point, grid: f64) -> u64 {
    scaled_penalty(distance(first, second), grid)
}

fn scaled_penalty(value_mm: f64, grid: f64) -> u64 {
    (value_mm / grid * COST_SCALE as f64)
        .round()
        .clamp(0.0, u64::MAX as f64) as u64
}

fn compress_path(points: &[Point]) -> Vec<Point> {
    let mut compressed = Vec::<Point>::new();
    for point in points {
        if compressed
            .last()
            .is_some_and(|previous| distance(*previous, *point) <= ROUTING_EPSILON)
        {
            continue;
        }
        while compressed.len() >= 2 {
            let first = compressed[compressed.len() - 2];
            let second = compressed[compressed.len() - 1];
            let cross = (second.x - first.x) * (point.y - second.y)
                - (second.y - first.y) * (point.x - second.x);
            let dot = (second.x - first.x) * (point.x - second.x)
                + (second.y - first.y) * (point.y - second.y);
            if cross.abs() > ROUTING_EPSILON || dot < -ROUTING_EPSILON {
                break;
            }
            compressed.pop();
        }
        compressed.push(*point);
    }
    compressed
}

fn nearest_distance(point: Point, targets: &[Point]) -> f64 {
    targets
        .iter()
        .map(|target| distance(point, *target))
        .fold(f64::INFINITY, f64::min)
}

fn point_segment_projection(point: Point, start: Point, end: Point) -> Point {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length_squared = dx * dx + dy * dy;
    if length_squared <= ROUTING_EPSILON * ROUTING_EPSILON {
        return start;
    }
    let fraction =
        (((point.x - start.x) * dx + (point.y - start.y) * dy) / length_squared).clamp(0.0, 1.0);
    Point {
        x: normalized_zero(start.x + fraction * dx),
        y: normalized_zero(start.y + fraction * dy),
    }
}

fn terminal_span(points: &[Point]) -> f64 {
    let Some(first) = points.first() else {
        return 0.0;
    };
    let (mut min_x, mut max_x) = (first.x, first.x);
    let (mut min_y, mut max_y) = (first.y, first.y);
    for point in &points[1..] {
        min_x = min_x.min(point.x);
        max_x = max_x.max(point.x);
        min_y = min_y.min(point.y);
        max_y = max_y.max(point.y);
    }
    max_x - min_x + max_y - min_y
}

fn pass_is_better(candidate: &RouteSolution, current: &RouteSolution) -> bool {
    candidate.metrics.routed_net_count > current.metrics.routed_net_count
        || (candidate.metrics.routed_net_count == current.metrics.routed_net_count
            && (candidate.metrics.total_trace_length_mm + ROUTING_EPSILON
                < current.metrics.total_trace_length_mm
                || ((candidate.metrics.total_trace_length_mm
                    - current.metrics.total_trace_length_mm)
                    .abs()
                    <= ROUTING_EPSILON
                    && candidate.metrics.bend_count < current.metrics.bend_count)))
}

fn point_order(first: &Point, second: &Point) -> std::cmp::Ordering {
    first
        .x
        .total_cmp(&second.x)
        .then_with(|| first.y.total_cmp(&second.y))
}

fn point_key(point: Point) -> (u64, u64) {
    (
        normalized_zero(point.x).to_bits(),
        normalized_zero(point.y).to_bits(),
    )
}

fn normalized_zero(value: f64) -> f64 {
    if value.abs() <= ROUTING_EPSILON {
        0.0
    } else {
        value
    }
}

fn net_pass_tie(seed: u64, pass: usize, net_id: &str) -> u64 {
    mix64(seed ^ (pass as u64).rotate_left(19) ^ stable_hash(net_id.as_bytes()))
}

fn state_tie(seed: u64, state: SearchState) -> u64 {
    mix64(
        seed ^ (state.node.x as u64).rotate_left(11)
            ^ (state.node.y as u64).rotate_left(37)
            ^ u64::from(state.direction).rotate_left(53),
    )
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
