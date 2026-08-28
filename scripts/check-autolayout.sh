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
echo "AutoLayout checks passed"
