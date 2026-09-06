use layout_core::{
    place, route, validate_route_solution, Design, PlacementOptions, PlacementStatus,
    RoutingOptions,
};
use std::fs;
use std::io::{self, Read};
use std::process::ExitCode;

const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(3),
        Err(message) => {
            eprintln!("route_json: {message}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<bool, String> {
    let mut arguments = std::env::args().skip(1);
    let input = arguments
        .next()
        .ok_or_else(|| "usage: route_json <design.json|-> [solution.json|-] [seed]".to_string())?;
    let output = arguments.next().unwrap_or_else(|| "-".into());
    let seed = arguments
        .next()
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|error| format!("invalid seed {value:?}: {error}"))
        })
        .transpose()?
        .unwrap_or_else(|| RoutingOptions::default().seed);
    if arguments.next().is_some() {
        return Err("too many arguments".into());
    }

    let source = if input == "-" {
        let mut source = String::new();
        io::stdin()
            .take((MAX_INPUT_BYTES + 1) as u64)
            .read_to_string(&mut source)
            .map_err(|error| format!("could not read stdin: {error}"))?;
        source
    } else {
        fs::read_to_string(&input).map_err(|error| format!("could not read {input}: {error}"))?
    };
    if source.len() > MAX_INPUT_BYTES {
        return Err(format!("input exceeds the {MAX_INPUT_BYTES} byte limit"));
    }
    let design: Design =
        serde_json::from_str(&source).map_err(|error| format!("invalid design JSON: {error}"))?;
    let placement =
        place(&design, PlacementOptions::default()).map_err(|error| error.to_string())?;
    if placement.status != PlacementStatus::Complete {
        return Err(format!(
            "routing requires complete placement, observed {:?}",
            placement.status
        ));
    }
    let options = RoutingOptions {
        seed,
        ..RoutingOptions::default()
    };
    let first = route(&design, &placement, options).map_err(|error| error.to_string())?;
    let second = route(&design, &placement, options).map_err(|error| error.to_string())?;
    if first != second {
        return Err("deterministic replay mismatch".into());
    }
    validate_route_solution(&design, &placement, &first).map_err(|error| error.to_string())?;

    let encoded = serde_json::to_string_pretty(&first)
        .map_err(|error| format!("could not encode solution: {error}"))?;
    if output == "-" {
        println!("{encoded}");
    } else {
        fs::write(&output, format!("{encoded}\n"))
            .map_err(|error| format!("could not write {output}: {error}"))?;
    }
    eprintln!(
        "routed={}/{} length_mm={:.3} bends={} segments={} seed={}",
        first.metrics.routed_net_count,
        first.metrics.total_net_count,
        first.metrics.total_trace_length_mm,
        first.metrics.bend_count,
        first.segments.len(),
        seed
    );
    Ok(first.unrouted_net_ids.is_empty())
}
