use layout_core::{place, Design, PlacementOptions, PlacementStatus};
use std::fs;
use std::io::{self, Read};
use std::process::ExitCode;

const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;

fn main() -> ExitCode {
    match run() {
        Ok(status) => match status {
            PlacementStatus::Complete => ExitCode::SUCCESS,
            PlacementStatus::InfeasibleFixedConstraints => ExitCode::from(2),
            PlacementStatus::SearchExhausted => ExitCode::from(3),
        },
        Err(message) => {
            eprintln!("place_json: {message}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<PlacementStatus, String> {
    let mut arguments = std::env::args().skip(1);
    let input = arguments
        .next()
        .ok_or_else(|| "usage: place_json <design.json|-> [solution.json|-] [seed]".to_string())?;
    let output = arguments.next().unwrap_or_else(|| "-".into());
    let seed = arguments
        .next()
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|error| format!("invalid seed {value:?}: {error}"))
        })
        .transpose()?
        .unwrap_or_else(|| PlacementOptions::default().seed);
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
        return Err(format!("input exceeds the {} byte limit", MAX_INPUT_BYTES));
    }
    let design: Design =
        serde_json::from_str(&source).map_err(|error| format!("invalid design JSON: {error}"))?;
    let solution = place(
        &design,
        PlacementOptions {
            seed,
            ..PlacementOptions::default()
        },
    )
    .map_err(|error| error.to_string())?;
    let encoded = serde_json::to_string_pretty(&solution)
        .map_err(|error| format!("could not encode solution: {error}"))?;
    if output == "-" {
        println!("{encoded}");
    } else {
        fs::write(&output, format!("{encoded}\n"))
            .map_err(|error| format!("could not write {output}: {error}"))?;
    }
    eprintln!(
        "status={:?} placed={}/{} hpwl_mm={:.3} candidates={} seed={}",
        solution.status,
        solution.metrics.placed_components,
        solution.metrics.total_components,
        solution.metrics.half_perimeter_wire_length_mm,
        solution.metrics.candidate_evaluations,
        solution.seed
    );
    Ok(solution.status)
}
