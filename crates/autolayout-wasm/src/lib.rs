//! Raw browser ABI for deterministic KiCad placement and B.Cu routing.

use kicad_layout::{BoardDocument, DesignBuildOptions, PatchOptions};
use layout_core::{
    place, route, JumperMode, JumperPolicy, PlacementOptions, PlacementRules, PlacementStatus,
    RoutingOptions, RoutingRules,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const RESULT_SCHEMA_VERSION: u32 = 1;
const MAX_RESTARTS: usize = 32;
const MAX_REFINEMENT_PASSES: usize = 8;
const MAX_GRID_POINTS: usize = 250_000;
const MAX_SEARCH_NODES: usize = 1_000_000;
const MAX_REROUTE_PASSES: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoLayoutOptions {
    pub seed: u64,
    pub placement_restarts: usize,
    pub placement_refinement_passes: usize,
    pub placement_max_grid_points: usize,
    pub placement_grid_mm: f64,
    pub component_clearance_mm: f64,
    pub edge_clearance_mm: f64,
    pub routing_grid_mm: f64,
    pub trace_width_mm: f64,
    pub trace_clearance_mm: f64,
    pub routing_max_search_nodes: usize,
    pub routing_reroute_passes: usize,
    pub bend_penalty_mm: f64,
    pub congestion_penalty_mm: f64,
}

impl Default for AutoLayoutOptions {
    fn default() -> Self {
        Self {
            seed: 424_242,
            placement_restarts: 8,
            placement_refinement_passes: 2,
            placement_max_grid_points: 100_000,
            placement_grid_mm: 1.0,
            component_clearance_mm: 1.0,
            edge_clearance_mm: 1.0,
            routing_grid_mm: 0.5,
            trace_width_mm: 1.0,
            trace_clearance_mm: 0.8,
            routing_max_search_nodes: 250_000,
            routing_reroute_passes: 6,
            bend_penalty_mm: 0.25,
            congestion_penalty_mm: 2.0,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AutoLayoutMetrics {
    pub component_count: usize,
    pub moved_component_count: usize,
    pub routed_net_count: usize,
    pub total_net_count: usize,
    pub segment_count: usize,
    pub trace_length_mm: f64,
    pub bend_count: usize,
    pub candidate_evaluations: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AutoLayoutResult {
    pub schema_version: u32,
    pub proposal_id: String,
    pub candidate_source: String,
    pub requires_kicad_drc: bool,
    pub seed: u64,
    pub metrics: AutoLayoutMetrics,
}

pub fn generate_candidate(
    source: &str,
    options: &AutoLayoutOptions,
) -> Result<AutoLayoutResult, String> {
    validate_options(options)?;
    let board = BoardDocument::parse(source).map_err(|error| error.to_string())?;
    if board.existing_copper_item_count() != 0 {
        return Err(format!(
            "AutoLayout requires an unrouted board; this file already contains {} copper object(s)",
            board.existing_copper_item_count()
        ));
    }

    let design = board
        .to_layout_design(&DesignBuildOptions {
            name: "browser-candidate".into(),
            footprint_envelopes: BTreeMap::new(),
            allowed_rotations_degrees: BTreeMap::new(),
            placement_keepouts: Vec::new(),
            placement_rules: PlacementRules {
                grid_mm: options.placement_grid_mm,
                component_clearance_mm: options.component_clearance_mm,
                edge_clearance_mm: options.edge_clearance_mm,
            },
            routing_rules: RoutingRules {
                grid_mm: options.routing_grid_mm,
                trace_width_mm: options.trace_width_mm,
                trace_clearance_mm: options.trace_clearance_mm,
                edge_clearance_mm: options.edge_clearance_mm,
            },
            jumper_policy: JumperPolicy {
                mode: JumperMode::Forbidden,
                approvals: Vec::new(),
            },
        })
        .map_err(|error| error.to_string())?;
    let placement = place(
        &design,
        PlacementOptions {
            seed: options.seed,
            restarts: options.placement_restarts,
            refinement_passes: options.placement_refinement_passes,
            max_grid_points: options.placement_max_grid_points,
        },
    )
    .map_err(|error| error.to_string())?;
    if placement.status != PlacementStatus::Complete {
        let detail = placement
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(if detail.is_empty() {
            format!("placement did not complete: {:?}", placement.status)
        } else {
            detail
        });
    }

    let routing = route(
        &design,
        &placement,
        RoutingOptions {
            seed: options.seed,
            max_search_nodes: options.routing_max_search_nodes,
            reroute_passes: options.routing_reroute_passes,
            bend_penalty_mm: options.bend_penalty_mm,
            congestion_penalty_mm: options.congestion_penalty_mm,
        },
    )
    .map_err(|error| error.to_string())?;
    if !routing.unrouted_net_ids.is_empty() {
        let names = routing
            .unrouted_net_ids
            .iter()
            .map(|id| {
                design
                    .nets
                    .iter()
                    .find(|net| net.id == *id)
                    .map(|net| net.name.as_str())
                    .unwrap_or(id)
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "single-layer routing could not connect {} net(s): {names}",
            routing.unrouted_net_ids.len()
        ));
    }

    let candidate_source = board
        .rewrite_routed_candidate(&design, &placement, &routing, PatchOptions::default())
        .map_err(|error| error.to_string())?;
    let moved_component_count = design
        .components
        .iter()
        .filter(|component| {
            placement
                .placements
                .get(&component.id)
                .is_some_and(|pose| pose != &component.initial_pose)
        })
        .count();
    let proposal_id = candidate_id(candidate_source.as_bytes());
    Ok(AutoLayoutResult {
        schema_version: RESULT_SCHEMA_VERSION,
        proposal_id,
        candidate_source,
        requires_kicad_drc: true,
        seed: options.seed,
        metrics: AutoLayoutMetrics {
            component_count: design.components.len(),
            moved_component_count,
            routed_net_count: routing.metrics.routed_net_count,
            total_net_count: routing.metrics.total_net_count,
            segment_count: routing.segments.len(),
            trace_length_mm: routing.metrics.total_trace_length_mm,
            bend_count: routing.metrics.bend_count,
            candidate_evaluations: placement.metrics.candidate_evaluations,
        },
    })
}

fn validate_options(options: &AutoLayoutOptions) -> Result<(), String> {
    let mut issues = Vec::new();
    if !(1..=MAX_RESTARTS).contains(&options.placement_restarts) {
        issues.push(format!(
            "placement_restarts must be between 1 and {MAX_RESTARTS}"
        ));
    }
    if options.placement_refinement_passes > MAX_REFINEMENT_PASSES {
        issues.push(format!(
            "placement_refinement_passes must not exceed {MAX_REFINEMENT_PASSES}"
        ));
    }
    if !(1..=MAX_GRID_POINTS).contains(&options.placement_max_grid_points) {
        issues.push(format!(
            "placement_max_grid_points must be between 1 and {MAX_GRID_POINTS}"
        ));
    }
    if !(1..=MAX_SEARCH_NODES).contains(&options.routing_max_search_nodes) {
        issues.push(format!(
            "routing_max_search_nodes must be between 1 and {MAX_SEARCH_NODES}"
        ));
    }
    if !(1..=MAX_REROUTE_PASSES).contains(&options.routing_reroute_passes) {
        issues.push(format!(
            "routing_reroute_passes must be between 1 and {MAX_REROUTE_PASSES}"
        ));
    }
    for (name, value, minimum, maximum) in [
        ("placement_grid_mm", options.placement_grid_mm, 0.05, 10.0),
        (
            "component_clearance_mm",
            options.component_clearance_mm,
            0.0,
            20.0,
        ),
        ("edge_clearance_mm", options.edge_clearance_mm, 0.0, 20.0),
        ("routing_grid_mm", options.routing_grid_mm, 0.05, 10.0),
        ("trace_width_mm", options.trace_width_mm, 0.1, 20.0),
        ("trace_clearance_mm", options.trace_clearance_mm, 0.0, 20.0),
        ("bend_penalty_mm", options.bend_penalty_mm, 0.0, 100.0),
        (
            "congestion_penalty_mm",
            options.congestion_penalty_mm,
            0.0,
            100.0,
        ),
    ] {
        if !value.is_finite() || value < minimum || value > maximum {
            issues.push(format!(
                "{name} must be finite and between {minimum} and {maximum}"
            ));
        }
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues.join("; "))
    }
}

fn candidate_id(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(71);
    encoded.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

#[cfg(target_arch = "wasm32")]
mod wasm_abi {
    use super::*;
    use std::{mem, slice, sync::Mutex};

    static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

    fn return_bytes(bytes: Vec<u8>) -> u64 {
        let boxed = bytes.into_boxed_slice();
        let len = boxed.len() as u32;
        let ptr = Box::into_raw(boxed) as *mut u8 as u32;
        ((ptr as u64) << 32) | u64::from(len)
    }

    unsafe fn input<'a>(ptr: u32, len: u32) -> &'a [u8] {
        slice::from_raw_parts(ptr as *const u8, len as usize)
    }

    #[no_mangle]
    pub extern "C" fn alloc(len: u32) -> u32 {
        let boxed = vec![0_u8; len as usize].into_boxed_slice();
        Box::into_raw(boxed) as *mut u8 as u32
    }

    #[no_mangle]
    pub unsafe extern "C" fn dealloc(ptr: u32, len: u32) {
        if ptr != 0 && len != 0 {
            let raw = slice::from_raw_parts_mut(ptr as *mut u8, len as usize);
            mem::drop(Box::from_raw(raw));
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn auto_layout(
        source_ptr: u32,
        source_len: u32,
        options_ptr: u32,
        options_len: u32,
    ) -> u64 {
        let result = (|| {
            let source = std::str::from_utf8(input(source_ptr, source_len))
                .map_err(|error| format!("board is not valid UTF-8: {error}"))?;
            let options: AutoLayoutOptions =
                serde_json::from_slice(input(options_ptr, options_len))
                    .map_err(|error| format!("invalid AutoLayout options: {error}"))?;
            let result = generate_candidate(source, &options)?;
            serde_json::to_vec(&result).map_err(|error| error.to_string())
        })();
        match result {
            Ok(bytes) => return_bytes(bytes),
            Err(error) => {
                *LAST_ERROR.lock().unwrap() = Some(error);
                0
            }
        }
    }

    #[no_mangle]
    pub extern "C" fn last_error() -> u64 {
        LAST_ERROR
            .lock()
            .unwrap()
            .take()
            .map(|message| return_bytes(message.into_bytes()))
            .unwrap_or(0)
    }
}
