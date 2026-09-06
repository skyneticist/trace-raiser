use kicad_layout::{BoardDocument, DesignBuildOptions};
use layout_core::{
    place, Design, JumperMode, JumperPolicy, PlacementMetrics, PlacementOptions, PlacementRules,
    PlacementStatus, RoutingRules,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

const CORPUS_SCHEMA_VERSION: u32 = 1;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_BOARD_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct CorpusManifest {
    schema_version: u32,
    cases: Vec<CorpusCase>,
}

#[derive(Debug, Deserialize)]
struct CorpusCase {
    id: String,
    board: String,
    expect: ExpectedResult,
    #[serde(default)]
    placement_options: Option<PlacementOptions>,
    #[serde(default)]
    expected_replay_fingerprint: Option<String>,
    #[serde(default)]
    error_contains: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExpectedResult {
    Complete,
    ParseError,
    DesignError,
}

#[derive(Debug, Serialize)]
struct CorpusReport {
    schema_version: u32,
    manifest: String,
    passed: bool,
    cases: Vec<CaseReport>,
}

#[derive(Debug, Serialize)]
struct CaseReport {
    id: String,
    passed: bool,
    observed: String,
    elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    replay_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metrics: Option<PlacementMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Serialize)]
struct ReplayArtifact<'a> {
    schema_version: u32,
    evaluator_version: &'static str,
    case_id: &'a str,
    design: &'a Design,
    placement: &'a layout_core::PlacementOutcome,
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(message) => {
            eprintln!("evaluate_corpus: {message}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<bool, String> {
    let mut arguments = std::env::args().skip(1);
    let manifest_argument = arguments
        .next()
        .ok_or_else(|| "usage: evaluate_corpus <manifest.json> [report.json|-]".to_string())?;
    let report_argument = arguments.next().unwrap_or_else(|| "-".into());
    if arguments.next().is_some() {
        return Err("too many arguments".into());
    }

    let manifest_path = PathBuf::from(&manifest_argument);
    let source = read_bounded(&manifest_path, MAX_MANIFEST_BYTES)?;
    let manifest: CorpusManifest =
        serde_json::from_str(&source).map_err(|error| format!("invalid manifest JSON: {error}"))?;
    validate_manifest(&manifest)?;
    let manifest_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));

    let cases = manifest
        .cases
        .iter()
        .map(|case| evaluate_case(case, manifest_dir))
        .collect::<Vec<_>>();
    let passed = cases.iter().all(|case| case.passed);
    let report = CorpusReport {
        schema_version: CORPUS_SCHEMA_VERSION,
        manifest: manifest_argument,
        passed,
        cases,
    };
    let encoded = serde_json::to_string_pretty(&report)
        .map_err(|error| format!("could not encode report: {error}"))?;
    if report_argument == "-" {
        println!("{encoded}");
    } else {
        fs::write(&report_argument, format!("{encoded}\n"))
            .map_err(|error| format!("could not write {report_argument}: {error}"))?;
    }
    eprintln!(
        "corpus={} passed={}/{}",
        if passed { "pass" } else { "fail" },
        report.cases.iter().filter(|case| case.passed).count(),
        report.cases.len()
    );
    Ok(passed)
}

fn validate_manifest(manifest: &CorpusManifest) -> Result<(), String> {
    if manifest.schema_version != CORPUS_SCHEMA_VERSION {
        return Err(format!(
            "unsupported corpus schema {}, expected {CORPUS_SCHEMA_VERSION}",
            manifest.schema_version
        ));
    }
    if manifest.cases.is_empty() {
        return Err("corpus contains no cases".into());
    }
    let mut ids = HashSet::new();
    for case in &manifest.cases {
        if case.id.trim().is_empty() {
            return Err("corpus case has an empty id".into());
        }
        if !ids.insert(&case.id) {
            return Err(format!("duplicate corpus case id {}", case.id));
        }
        match case.expect {
            ExpectedResult::Complete if case.expected_replay_fingerprint.is_none() => {
                return Err(format!(
                    "complete case {} is missing expected_replay_fingerprint",
                    case.id
                ));
            }
            ExpectedResult::ParseError | ExpectedResult::DesignError
                if case.error_contains.is_none() =>
            {
                return Err(format!("error case {} is missing error_contains", case.id));
            }
            _ => {}
        }
    }
    Ok(())
}

fn evaluate_case(case: &CorpusCase, manifest_dir: &Path) -> CaseReport {
    let started = Instant::now();
    let finish = |passed: bool,
                  observed: String,
                  replay_fingerprint: Option<String>,
                  metrics: Option<PlacementMetrics>,
                  detail: Option<String>| CaseReport {
        id: case.id.clone(),
        passed,
        observed,
        elapsed_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        replay_fingerprint,
        metrics,
        detail,
    };
    let board_path = manifest_dir.join(&case.board);
    let source = match read_bounded(&board_path, MAX_BOARD_BYTES) {
        Ok(source) => source,
        Err(message) => return finish(false, "read_error".into(), None, None, Some(message)),
    };

    let board = match BoardDocument::parse(&source) {
        Ok(board) => board,
        Err(error) => {
            let detail = error.to_string();
            let passed = matches!(case.expect, ExpectedResult::ParseError)
                && case
                    .error_contains
                    .as_ref()
                    .is_some_and(|expected| detail.contains(expected));
            return finish(passed, "parse_error".into(), None, None, Some(detail));
        }
    };
    if matches!(case.expect, ExpectedResult::ParseError) {
        return finish(
            false,
            "parse_success".into(),
            None,
            None,
            Some("expected parsing to fail".into()),
        );
    }

    let design = match board.to_layout_design(&design_options(&case.id)) {
        Ok(design) => design,
        Err(error) => {
            let detail = error.to_string();
            let passed = matches!(case.expect, ExpectedResult::DesignError)
                && case
                    .error_contains
                    .as_ref()
                    .is_some_and(|expected| detail.contains(expected));
            return finish(passed, "design_error".into(), None, None, Some(detail));
        }
    };
    if matches!(case.expect, ExpectedResult::DesignError) {
        return finish(
            false,
            "design_success".into(),
            None,
            None,
            Some("expected design conversion to fail".into()),
        );
    }

    let outcome = match place(&design, case.placement_options.unwrap_or_default()) {
        Ok(outcome) => outcome,
        Err(error) => {
            return finish(
                false,
                "placement_error".into(),
                None,
                None,
                Some(error.to_string()),
            )
        }
    };
    let fingerprint = match replay_fingerprint(&case.id, &design, &outcome) {
        Ok(fingerprint) => fingerprint,
        Err(message) => {
            return finish(
                false,
                "fingerprint_error".into(),
                None,
                Some(outcome.metrics),
                Some(message),
            )
        }
    };
    let passed = outcome.status == PlacementStatus::Complete
        && case.expected_replay_fingerprint.as_deref() == Some(fingerprint.as_str());
    let detail = if passed {
        None
    } else {
        Some(format!(
            "expected complete with fingerprint {}, observed {} with {fingerprint}",
            case.expected_replay_fingerprint
                .as_deref()
                .unwrap_or("<missing>"),
            placement_status(outcome.status)
        ))
    };
    finish(
        passed,
        placement_status(outcome.status).into(),
        Some(fingerprint),
        Some(outcome.metrics),
        detail,
    )
}

fn design_options(name: &str) -> DesignBuildOptions {
    DesignBuildOptions {
        name: name.into(),
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

fn replay_fingerprint(
    case_id: &str,
    design: &Design,
    placement: &layout_core::PlacementOutcome,
) -> Result<String, String> {
    let artifact = ReplayArtifact {
        schema_version: CORPUS_SCHEMA_VERSION,
        evaluator_version: env!("CARGO_PKG_VERSION"),
        case_id,
        design,
        placement,
    };
    let bytes = serde_json::to_vec(&artifact)
        .map_err(|error| format!("could not encode replay artifact: {error}"))?;
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    Ok(format!("fnv1a64:{hash:016x}"))
}

fn placement_status(status: PlacementStatus) -> &'static str {
    match status {
        PlacementStatus::Complete => "complete",
        PlacementStatus::InfeasibleFixedConstraints => "infeasible_fixed_constraints",
        PlacementStatus::SearchExhausted => "search_exhausted",
    }
}

fn read_bounded(path: &Path, max_bytes: u64) -> Result<String, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
    if metadata.len() > max_bytes {
        return Err(format!(
            "{} exceeds the {max_bytes} byte limit",
            path.display()
        ));
    }
    fs::read_to_string(path).map_err(|error| format!("could not read {}: {error}", path.display()))
}
