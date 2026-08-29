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

cargo run --quiet --manifest-path crates/layout-core/Cargo.toml \
  --example place_json -- \
  crates/layout-core/fixtures/two-pin.json "$replay_dir/first.json" 424242
cargo run --quiet --manifest-path crates/layout-core/Cargo.toml \
  --example place_json -- \
  crates/layout-core/fixtures/two-pin.json "$replay_dir/second.json" 424242

cmp "$replay_dir/first.json" "$replay_dir/second.json"
shasum -a 256 "$replay_dir/first.json"

cp public/sample-sensor.kicad_pcb "$replay_dir/sample-sensor.kicad_pcb"
if scripts/validate-kicad-candidate.sh \
  "$replay_dir/sample-sensor.kicad_pcb" \
  "$replay_dir/sample-sensor.drc.json"; then
  :
else
  validation_status=$?
  if [[ $validation_status -eq 69 && "${REQUIRE_KICAD_DRC:-0}" != 1 ]]; then
    echo "KiCad DRC skipped: set REQUIRE_KICAD_DRC=1 to require it"
  else
    exit "$validation_status"
  fi
fi

echo "AutoLayout checks passed"
