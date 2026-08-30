#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo_root"

cargo fmt --manifest-path crates/layout-core/Cargo.toml -- --check
cargo test --manifest-path crates/layout-core/Cargo.toml
cargo clippy --manifest-path crates/layout-core/Cargo.toml --all-targets -- -D warnings

cargo fmt --manifest-path crates/kicad-layout/Cargo.toml -- --check
cargo test --manifest-path crates/kicad-layout/Cargo.toml
cargo clippy --manifest-path crates/kicad-layout/Cargo.toml --all-targets -- -D warnings

replay_dir=$(mktemp -d)
trap 'rm -rf "$replay_dir"' EXIT

cargo run --quiet --manifest-path crates/kicad-layout/Cargo.toml \
  --example evaluate_corpus -- \
  crates/kicad-layout/fixtures/corpus.json "$replay_dir/corpus-report.json"

cargo run --quiet --manifest-path crates/layout-core/Cargo.toml \
  --example place_json -- \
  crates/layout-core/fixtures/two-pin.json "$replay_dir/first.json" 424242
cargo run --quiet --manifest-path crates/layout-core/Cargo.toml \
  --example place_json -- \
  crates/layout-core/fixtures/two-pin.json "$replay_dir/second.json" 424242

cmp "$replay_dir/first.json" "$replay_dir/second.json"
shasum -a 256 "$replay_dir/first.json"

cargo run --quiet --manifest-path crates/layout-core/Cargo.toml \
  --example route_json -- \
  crates/layout-core/fixtures/routing-smoke.json "$replay_dir/route.json" 424242
shasum -a 256 "$replay_dir/route.json"

validate_kicad_fixture() {
  local source=$1
  local fixture_name=$2
  local candidate="$replay_dir/$fixture_name.kicad_pcb"
  local report="$replay_dir/$fixture_name.drc.json"
  cp "$source" "$candidate"
  if scripts/validate-kicad-candidate.sh "$candidate" "$report"; then
    return 0
  else
    local validation_status=$?
    if [[ $validation_status -eq 69 && "${REQUIRE_KICAD_DRC:-0}" != 1 ]]; then
      echo "KiCad DRC skipped for $fixture_name: set REQUIRE_KICAD_DRC=1 to require it"
    else
      return "$validation_status"
    fi
  fi
}

validate_kicad_fixture public/sample-sensor.kicad_pcb sample-sensor
validate_kicad_fixture crates/kicad-layout/fixtures/curved-outline.kicad_pcb curved-outline

cargo run --quiet --manifest-path crates/kicad-layout/Cargo.toml \
  --example emit_routed_candidate -- \
  crates/kicad-layout/fixtures/kicad10-rounded-name-nets.kicad_pcb \
  "$replay_dir/emitted-route.kicad_pcb"
shasum -a 256 "$replay_dir/emitted-route.kicad_pcb"
validate_kicad_fixture \
  "$replay_dir/emitted-route.kicad_pcb" \
  generated-kicad10-route

echo "AutoLayout checks passed"
