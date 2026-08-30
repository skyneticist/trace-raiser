use crate::geometry::{point_is_finite, polygon_contains_point, transform_point};
use crate::routing::{
    derived_bend_count, derived_unrouted_nets, distance, point_segment_distance, segment_distance,
};
use crate::{
    validate_design, validate_route_solution, AcceptedJumperProposal, Design, Jumper,
    JumperApproval, JumperMode, JumperPolicy, JumperProposal, Net, PlacementOutcome, Point,
    ProposedJumper, RouteSolution, SOLUTION_SCHEMA_VERSION,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fmt::Write;

const JUMPER_EPSILON: f64 = 1.0e-7;
const AUTHORITY_SCHEMA: &str = "copperline-jumper-authority-v1";
const PROPOSAL_PREFIX: &str = "jumper-proposal-v1:";
const APPROVAL_PREFIX: &str = "jumper-approval-v1:";
const MAX_JUMPERS_PER_PROPOSAL: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JumperProposalError {
    messages: Vec<String>,
}

impl JumperProposalError {
    pub fn messages(&self) -> &[String] {
        &self.messages
    }
}

impl fmt::Display for JumperProposalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.messages.len() == 1 {
            formatter.write_str(&self.messages[0])
        } else {
            write!(
                formatter,
                "jumper proposal failed with {} issues: {}",
                self.messages.len(),
                self.messages.join("; ")
            )
        }
    }
}

impl Error for JumperProposalError {}

#[derive(Serialize)]
struct AuthorityPayload<'a> {
    authority_schema: &'static str,
    design: &'a Design,
    placements: &'a BTreeMap<String, crate::Pose>,
    net_id: &'a str,
    jumpers: &'a [CanonicalJumper],
}

#[derive(Debug, Clone, Copy, Serialize)]
struct CanonicalJumper {
    start: Point,
    end: Point,
}

/// Produce reviewable jumper sets for every currently unrouted net.
///
/// Proposals carry no routing authority. Their stable IDs bind the exact
/// design, placement, net, and canonicalized jumper geometry.
pub fn propose_jumpers(
    design: &Design,
    placement: &PlacementOutcome,
    route: &RouteSolution,
) -> Result<Vec<JumperProposal>, JumperProposalError> {
    validate_route_solution(design, placement, route).map_err(|error| JumperProposalError {
        messages: error
            .messages()
            .iter()
            .map(|message| format!("route: {message}"))
            .collect(),
    })?;

    let unrouted = route
        .unrouted_net_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut proposals = Vec::new();
    for net in design
        .nets
        .iter()
        .filter(|net| unrouted.contains(net.id.as_str()))
    {
        let representatives = disconnected_terminal_representatives(design, placement, route, net);
        if representatives.len() <= 1 {
            return Err(JumperProposalError {
                messages: vec![format!(
                    "net {} is declared unrouted but has fewer than two conductive components",
                    net.id
                )],
            });
        }
        let jumpers = minimum_spanning_jumpers(&representatives);
        if jumpers.len() > MAX_JUMPERS_PER_PROPOSAL {
            return Err(JumperProposalError {
                messages: vec![format!(
                    "net {} requires {} jumpers, exceeding the review limit of {MAX_JUMPERS_PER_PROPOSAL}",
                    net.id,
                    jumpers.len()
                )],
            });
        }
        let proposal_id = proposal_id_for(design, placement, &net.id, &jumpers)?;
        proposals.push(JumperProposal {
            schema_version: SOLUTION_SCHEMA_VERSION,
            proposal_id,
            net_id: net.id.clone(),
            estimated_wire_length_mm: jumper_length(&jumpers),
            jumpers,
        });
    }
    proposals.sort_by(|first, second| first.net_id.cmp(&second.net_id));
    Ok(proposals)
}

/// Explicitly accept one exact proposal and return a consistent design/route pair.
///
/// This is the only API that mints an approval ID. The returned design contains
/// the bounded approval, and the returned route references it from each jumper.
pub fn accept_jumper_proposal(
    design: &Design,
    placement: &PlacementOutcome,
    route: &RouteSolution,
    proposal: &JumperProposal,
) -> Result<AcceptedJumperProposal, JumperProposalError> {
    validate_route_solution(design, placement, route).map_err(|error| JumperProposalError {
        messages: error
            .messages()
            .iter()
            .map(|message| format!("route: {message}"))
            .collect(),
    })?;
    validate_proposal(design, placement, route, proposal)?;
    if design
        .jumper_policy
        .approvals
        .iter()
        .any(|approval| approval.net_id == proposal.net_id)
    {
        return Err(JumperProposalError {
            messages: vec![format!(
                "net {} already has an immutable jumper approval",
                proposal.net_id
            )],
        });
    }

    let approval_id = approval_id_for(design, placement, &proposal.net_id, &proposal.jumpers)?;
    let approval = JumperApproval {
        approval_id: approval_id.clone(),
        net_id: proposal.net_id.clone(),
        max_count: proposal.jumpers.len(),
    };
    let mut accepted_design = design.clone();
    accepted_design.jumper_policy.mode = JumperMode::UserApprovedOnly;
    accepted_design
        .jumper_policy
        .approvals
        .push(approval.clone());
    accepted_design
        .jumper_policy
        .approvals
        .sort_by(|first, second| {
            first
                .net_id
                .cmp(&second.net_id)
                .then_with(|| first.approval_id.cmp(&second.approval_id))
        });
    validate_design(&accepted_design).map_err(|error| JumperProposalError {
        messages: error
            .messages()
            .iter()
            .map(|message| format!("accepted design: {message}"))
            .collect(),
    })?;

    let mut accepted_route = route.clone();
    accepted_route
        .jumpers
        .extend(proposal.jumpers.iter().map(|jumper| Jumper {
            net_id: proposal.net_id.clone(),
            start: jumper.start,
            end: jumper.end,
            approval_id: approval_id.clone(),
        }));
    let unrouted = derived_unrouted_nets(&accepted_design, placement, &accepted_route);
    accepted_route.unrouted_net_ids = unrouted.iter().cloned().collect();
    accepted_route.metrics.routed_net_count =
        accepted_design.nets.len().saturating_sub(unrouted.len());
    accepted_route.metrics.total_net_count = accepted_design.nets.len();
    accepted_route.metrics.bend_count = derived_bend_count(&accepted_route);
    accepted_route.metrics.jumper_count = accepted_route.jumpers.len();
    validate_route_solution(&accepted_design, placement, &accepted_route).map_err(|error| {
        JumperProposalError {
            messages: error
                .messages()
                .iter()
                .map(|message| format!("accepted route: {message}"))
                .collect(),
        }
    })?;

    Ok(AcceptedJumperProposal {
        approval,
        design: accepted_design,
        route: accepted_route,
    })
}

fn validate_proposal(
    design: &Design,
    placement: &PlacementOutcome,
    route: &RouteSolution,
    proposal: &JumperProposal,
) -> Result<(), JumperProposalError> {
    let mut messages = Vec::new();
    if proposal.schema_version != SOLUTION_SCHEMA_VERSION {
        messages.push(format!(
            "proposal.schema_version: expected {}, received {}",
            SOLUTION_SCHEMA_VERSION, proposal.schema_version
        ));
    }
    if !design.nets.iter().any(|net| net.id == proposal.net_id) {
        messages.push(format!("proposal.net_id: unknown net {}", proposal.net_id));
    }
    if !route
        .unrouted_net_ids
        .iter()
        .any(|net_id| net_id == &proposal.net_id)
    {
        messages.push(format!(
            "proposal.net_id: net {} is not currently unrouted",
            proposal.net_id
        ));
    }
    if proposal.jumpers.is_empty() || proposal.jumpers.len() > MAX_JUMPERS_PER_PROPOSAL {
        messages.push(format!(
            "proposal.jumpers: must contain between 1 and {MAX_JUMPERS_PER_PROPOSAL} jumpers"
        ));
    }
    let mut identities = HashSet::new();
    for (index, jumper) in proposal.jumpers.iter().enumerate() {
        if !point_is_finite(jumper.start) || !point_is_finite(jumper.end) {
            messages.push(format!(
                "proposal.jumpers[{index}]: endpoints must be finite"
            ));
            continue;
        }
        if distance(jumper.start, jumper.end) <= JUMPER_EPSILON {
            messages.push(format!(
                "proposal.jumpers[{index}]: zero-length jumpers are invalid"
            ));
        }
        if !polygon_contains_point(&design.outline.vertices, jumper.start)
            || !polygon_contains_point(&design.outline.vertices, jumper.end)
        {
            messages.push(format!(
                "proposal.jumpers[{index}]: endpoints must be inside the board outline"
            ));
        }
        let canonical = canonical_jumper(*jumper);
        if !identities.insert(canonical_key(canonical)) {
            messages.push(format!(
                "proposal.jumpers[{index}]: duplicate jumper geometry"
            ));
        }
    }
    let expected_length = jumper_length(&proposal.jumpers);
    if !proposal.estimated_wire_length_mm.is_finite()
        || (proposal.estimated_wire_length_mm - expected_length).abs() > JUMPER_EPSILON
    {
        messages
            .push("proposal.estimated_wire_length_mm: does not match the proposed geometry".into());
    }
    if messages.is_empty() {
        let expected_id = proposal_id_for(design, placement, &proposal.net_id, &proposal.jumpers)?;
        if proposal.proposal_id != expected_id {
            messages.push("proposal.proposal_id: does not match the immutable content".into());
        }
    }

    if messages.is_empty() {
        Ok(())
    } else {
        Err(JumperProposalError { messages })
    }
}

fn disconnected_terminal_representatives(
    design: &Design,
    placement: &PlacementOutcome,
    route: &RouteSolution,
    net: &Net,
) -> Vec<Point> {
    let components = design
        .components
        .iter()
        .map(|component| (component.id.as_str(), component))
        .collect::<HashMap<_, _>>();
    let mut terminal_points = net
        .terminals
        .iter()
        .filter_map(|terminal| {
            let component = components.get(terminal.component_id.as_str())?;
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
    terminal_points.sort_by(point_order);

    let mut nodes = terminal_points.clone();
    let terminal_count = nodes.len();
    let mut edges = Vec::new();
    for segment in route
        .segments
        .iter()
        .filter(|segment| segment.net_id == net.id)
    {
        let start = nodes.len();
        nodes.push(segment.start);
        nodes.push(segment.end);
        edges.push((start, start + 1, segment.start, segment.end, true));
    }
    for jumper in route
        .jumpers
        .iter()
        .filter(|jumper| jumper.net_id == net.id)
    {
        let start = nodes.len();
        nodes.push(jumper.start);
        nodes.push(jumper.end);
        edges.push((start, start + 1, jumper.start, jumper.end, false));
    }

    let mut sets = DisjointSet::new(nodes.len());
    for (first, second, _, _, _) in &edges {
        sets.union(*first, *second);
    }
    for first in 0..nodes.len() {
        for second in (first + 1)..nodes.len() {
            if distance(nodes[first], nodes[second]) <= JUMPER_EPSILON {
                sets.union(first, second);
            }
        }
    }
    for (node_index, node) in nodes.iter().copied().enumerate() {
        for (edge_start, _, start, end, copper) in &edges {
            if *copper && point_segment_distance(node, *start, *end) <= JUMPER_EPSILON {
                sets.union(node_index, *edge_start);
            }
        }
    }
    for first in 0..edges.len() {
        for second in (first + 1)..edges.len() {
            if edges[first].4
                && edges[second].4
                && segment_distance(
                    edges[first].2,
                    edges[first].3,
                    edges[second].2,
                    edges[second].3,
                ) <= JUMPER_EPSILON
            {
                sets.union(edges[first].0, edges[second].0);
            }
        }
    }

    let mut roots = HashSet::new();
    let mut representatives = Vec::new();
    for (index, point) in terminal_points.into_iter().enumerate().take(terminal_count) {
        let root = sets.find(index);
        if roots.insert(root) {
            representatives.push(point);
        }
    }
    representatives.sort_by(point_order);
    representatives
}

fn minimum_spanning_jumpers(points: &[Point]) -> Vec<ProposedJumper> {
    let mut connected = vec![points[0]];
    let mut remaining = points[1..].to_vec();
    let mut jumpers = Vec::new();
    while !remaining.is_empty() {
        let mut best: Option<(f64, Point, Point, usize)> = None;
        for start in &connected {
            for (end_index, end) in remaining.iter().enumerate() {
                let candidate = (distance(*start, *end), *start, *end, end_index);
                if best.as_ref().is_none_or(|current| {
                    candidate.0.total_cmp(&current.0).is_lt()
                        || (candidate.0.total_cmp(&current.0).is_eq()
                            && (point_order(&candidate.1, &current.1).is_lt()
                                || (point_order(&candidate.1, &current.1).is_eq()
                                    && point_order(&candidate.2, &current.2).is_lt())))
                }) {
                    best = Some(candidate);
                }
            }
        }
        let (_, start, end, end_index) = best.expect("remaining component has a nearest edge");
        jumpers.push(ProposedJumper { start, end });
        connected.push(remaining.remove(end_index));
    }
    jumpers
}

fn proposal_id_for(
    design: &Design,
    placement: &PlacementOutcome,
    net_id: &str,
    jumpers: &[ProposedJumper],
) -> Result<String, JumperProposalError> {
    authority_digest(design, placement, net_id, jumpers)
        .map(|digest| format!("{PROPOSAL_PREFIX}{digest}"))
}

fn approval_id_for(
    design: &Design,
    placement: &PlacementOutcome,
    net_id: &str,
    jumpers: &[ProposedJumper],
) -> Result<String, JumperProposalError> {
    authority_digest(design, placement, net_id, jumpers)
        .map(|digest| format!("{APPROVAL_PREFIX}{digest}"))
}

pub(crate) fn approval_id_for_jumpers(
    design: &Design,
    placement: &PlacementOutcome,
    net_id: &str,
    jumpers: &[&Jumper],
) -> Result<String, String> {
    let proposed = jumpers
        .iter()
        .map(|jumper| ProposedJumper {
            start: jumper.start,
            end: jumper.end,
        })
        .collect::<Vec<_>>();
    approval_id_for(design, placement, net_id, &proposed).map_err(|error| error.to_string())
}

fn authority_digest(
    design: &Design,
    placement: &PlacementOutcome,
    net_id: &str,
    jumpers: &[ProposedJumper],
) -> Result<String, JumperProposalError> {
    let mut authority_design = design.clone();
    authority_design.jumper_policy = JumperPolicy {
        mode: JumperMode::UserApprovedOnly,
        approvals: Vec::new(),
    };
    let canonical = canonical_jumpers(jumpers);
    let payload = AuthorityPayload {
        authority_schema: AUTHORITY_SCHEMA,
        design: &authority_design,
        placements: &placement.placements,
        net_id,
        jumpers: &canonical,
    };
    let encoded = serde_json::to_vec(&payload).map_err(|error| JumperProposalError {
        messages: vec![format!(
            "could not encode jumper authority payload: {error}"
        )],
    })?;
    let digest = Sha256::digest(encoded);
    let mut hexadecimal = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut hexadecimal, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(hexadecimal)
}

fn canonical_jumpers(jumpers: &[ProposedJumper]) -> Vec<CanonicalJumper> {
    let mut canonical = jumpers
        .iter()
        .copied()
        .map(canonical_jumper)
        .collect::<Vec<_>>();
    canonical.sort_by(|first, second| {
        point_order(&first.start, &second.start).then_with(|| point_order(&first.end, &second.end))
    });
    canonical
}

fn canonical_jumper(jumper: ProposedJumper) -> CanonicalJumper {
    let start = normalized_point(jumper.start);
    let end = normalized_point(jumper.end);
    if point_order(&start, &end).is_gt() {
        CanonicalJumper {
            start: end,
            end: start,
        }
    } else {
        CanonicalJumper { start, end }
    }
}

fn canonical_key(jumper: CanonicalJumper) -> (u64, u64, u64, u64) {
    (
        jumper.start.x.to_bits(),
        jumper.start.y.to_bits(),
        jumper.end.x.to_bits(),
        jumper.end.y.to_bits(),
    )
}

fn normalized_point(point: Point) -> Point {
    Point {
        x: if point.x.abs() <= JUMPER_EPSILON {
            0.0
        } else {
            point.x
        },
        y: if point.y.abs() <= JUMPER_EPSILON {
            0.0
        } else {
            point.y
        },
    }
}

fn jumper_length(jumpers: &[ProposedJumper]) -> f64 {
    jumpers
        .iter()
        .map(|jumper| distance(jumper.start, jumper.end))
        .sum()
}

fn point_order(first: &Point, second: &Point) -> std::cmp::Ordering {
    first
        .x
        .total_cmp(&second.x)
        .then_with(|| first.y.total_cmp(&second.y))
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
