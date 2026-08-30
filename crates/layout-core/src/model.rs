use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DESIGN_SCHEMA_VERSION: u32 = 1;
pub const SOLUTION_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Pose {
    pub x: f64,
    pub y: f64,
    pub rotation_degrees: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct LocalAabb {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl LocalAabb {
    pub fn width(self) -> f64 {
        self.max_x - self.min_x
    }

    pub fn height(self) -> f64 {
        self.max_y - self.min_y
    }

    pub fn area(self) -> f64 {
        self.width() * self.height()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Polygon {
    pub vertices: Vec<Point>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlacementKeepout {
    pub id: String,
    pub polygon: Polygon,
    #[serde(default)]
    pub clearance_mm: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PadKind {
    ThroughHole,
    NonPlatedThroughHole,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pad {
    pub id: String,
    pub local_position: Point,
    pub size: Point,
    pub drill_mm: Option<f64>,
    pub kind: PadKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Component {
    /// Stable KiCad UUID when available, otherwise a stable generated ID.
    pub id: String,
    pub reference: String,
    pub footprint: String,
    /// Courtyard-derived placement envelope in footprint-local coordinates.
    pub envelope: LocalAabb,
    pub pads: Vec<Pad>,
    pub initial_pose: Pose,
    #[serde(default)]
    pub locked: bool,
    /// Explicitly allowed rotations. Empty is invalid rather than meaning any.
    pub allowed_rotations_degrees: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct TerminalRef {
    pub component_id: String,
    pub pad_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Net {
    pub id: String,
    pub name: String,
    pub terminals: Vec<TerminalRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlacementRules {
    pub grid_mm: f64,
    pub component_clearance_mm: f64,
    pub edge_clearance_mm: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoutingRules {
    pub grid_mm: f64,
    pub trace_width_mm: f64,
    pub trace_clearance_mm: f64,
    pub edge_clearance_mm: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JumperMode {
    Forbidden,
    UserApprovedOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JumperApproval {
    /// Stable ID minted only when the user accepts the proposed jumper.
    pub approval_id: String,
    pub net_id: String,
    pub max_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JumperPolicy {
    pub mode: JumperMode,
    #[serde(default)]
    pub approvals: Vec<JumperApproval>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Design {
    pub schema_version: u32,
    pub name: String,
    pub outline: Polygon,
    #[serde(default)]
    pub placement_keepouts: Vec<PlacementKeepout>,
    pub components: Vec<Component>,
    pub nets: Vec<Net>,
    pub placement_rules: PlacementRules,
    pub routing_rules: RoutingRules,
    pub jumper_policy: JumperPolicy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlacementStatus {
    Complete,
    InfeasibleFixedConstraints,
    SearchExhausted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlacementDiagnostic {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub component_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlacementMetrics {
    pub placed_components: usize,
    pub total_components: usize,
    pub half_perimeter_wire_length_mm: f64,
    pub total_displacement_mm: f64,
    pub candidate_evaluations: u64,
    pub restarts_completed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlacementOutcome {
    pub schema_version: u32,
    pub status: PlacementStatus,
    pub seed: u64,
    pub placements: BTreeMap<String, Pose>,
    #[serde(default)]
    pub unplaced_component_ids: Vec<String>,
    #[serde(default)]
    pub diagnostics: Vec<PlacementDiagnostic>,
    pub metrics: PlacementMetrics,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlacementOptions {
    pub seed: u64,
    pub restarts: usize,
    pub refinement_passes: usize,
    pub max_grid_points: usize,
}

impl Default for PlacementOptions {
    fn default() -> Self {
        Self {
            seed: 0xC0_77_E2_11,
            restarts: 8,
            refinement_passes: 2,
            max_grid_points: 100_000,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct RoutingOptions {
    pub seed: u64,
    pub max_search_nodes: usize,
    pub reroute_passes: usize,
    pub bend_penalty_mm: f64,
    pub congestion_penalty_mm: f64,
}

impl Default for RoutingOptions {
    fn default() -> Self {
        Self {
            seed: 0xA5_7A_12_0E,
            max_search_nodes: 250_000,
            reroute_passes: 6,
            bend_penalty_mm: 0.25,
            congestion_penalty_mm: 2.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CopperLayer {
    Back,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouteSegment {
    pub net_id: String,
    pub start: Point,
    pub end: Point,
    pub width_mm: f64,
    pub layer: CopperLayer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Jumper {
    pub net_id: String,
    pub start: Point,
    pub end: Point,
    /// Stable approval reference generated by the user-facing review step.
    pub approval_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouteMetrics {
    pub routed_net_count: usize,
    pub total_net_count: usize,
    pub total_trace_length_mm: f64,
    #[serde(default)]
    pub bend_count: usize,
    pub jumper_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouteSolution {
    pub schema_version: u32,
    pub segments: Vec<RouteSegment>,
    #[serde(default)]
    pub jumpers: Vec<Jumper>,
    #[serde(default)]
    pub unrouted_net_ids: Vec<String>,
    pub metrics: RouteMetrics,
}
