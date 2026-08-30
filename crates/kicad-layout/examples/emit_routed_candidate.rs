use kicad_layout::{BoardDocument, DesignBuildOptions, PatchOptions};
use layout_core::{
    place, route, JumperMode, JumperPolicy, PlacementOptions, PlacementRules, PlacementStatus,
    RoutingOptions, RoutingRules,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const MAX_BOARD_BYTES: u64 = 64 * 1024 * 1024;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("emit_routed_candidate: {message}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let input = arguments.next().ok_or_else(|| {
        "usage: emit_routed_candidate <input.kicad_pcb> <output.kicad_pcb>".to_string()
    })?;
    let output = arguments.next().ok_or_else(|| {
        "usage: emit_routed_candidate <input.kicad_pcb> <output.kicad_pcb>".to_string()
    })?;
    if arguments.next().is_some() {
        return Err("too many arguments".into());
    }

    let input = PathBuf::from(input);
    let output = PathBuf::from(output);
    let source = read_bounded(&input)?;
    let board = BoardDocument::parse(&source).map_err(|error| error.to_string())?;
    if board.existing_copper_item_count() != 0 {
        return Err(format!(
            "input already contains {} routed copper object(s)",
            board.existing_copper_item_count()
        ));
    }

    let design = board
        .to_layout_design(&design_options())
        .map_err(|error| error.to_string())?;
    let placement = place(
        &design,
        PlacementOptions {
            seed: 424_242,
            restarts: 2,
            refinement_passes: 0,
            max_grid_points: 10_000,
        },
    )
    .map_err(|error| error.to_string())?;
    if placement.status != PlacementStatus::Complete {
        return Err(format!(
            "placement did not complete: {:?}",
            placement.status
        ));
    }
    let routing = route(
        &design,
        &placement,
        RoutingOptions {
            seed: 424_242,
            ..RoutingOptions::default()
        },
    )
    .map_err(|error| error.to_string())?;
    if !routing.unrouted_net_ids.is_empty() {
        return Err(format!(
            "routing left {} net(s) incomplete",
            routing.unrouted_net_ids.len()
        ));
    }

    let first = board
        .rewrite_routed_candidate(&design, &placement, &routing, PatchOptions::default())
        .map_err(|error| error.to_string())?;
    let replay = board
        .rewrite_routed_candidate(&design, &placement, &routing, PatchOptions::default())
        .map_err(|error| error.to_string())?;
    if first.as_bytes() != replay.as_bytes() {
        return Err("same-input emission was not byte-identical".into());
    }
    let reparsed = BoardDocument::parse(&first).map_err(|error| error.to_string())?;
    if reparsed.existing_copper_item_count() != routing.segments.len() {
        return Err("emitted board did not contain every generated segment".into());
    }
    fs::write(&output, first)
        .map_err(|error| format!("could not write {}: {error}", output.display()))?;
    println!(
        "emitted={} segments={} routed_nets={}/{}",
        output.display(),
        routing.segments.len(),
        routing.metrics.routed_net_count,
        routing.metrics.total_net_count
    );
    Ok(())
}

fn design_options() -> DesignBuildOptions {
    DesignBuildOptions {
        name: "generated-kicad-smoke".into(),
        footprint_envelopes: BTreeMap::new(),
        allowed_rotations_degrees: BTreeMap::new(),
        placement_keepouts: Vec::new(),
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

fn read_bounded(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
    if metadata.len() > MAX_BOARD_BYTES {
        return Err(format!(
            "{} exceeds the {MAX_BOARD_BYTES} byte limit",
            path.display()
        ));
    }
    fs::read_to_string(path).map_err(|error| format!("could not read {}: {error}", path.display()))
}
