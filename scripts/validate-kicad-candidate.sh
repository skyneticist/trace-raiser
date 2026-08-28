#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: $0 <candidate.kicad_pcb> [report.json]" >&2
  exit 64
fi

candidate=$1
report=${2:-"${candidate%.kicad_pcb}.drc.json"}

if ! command -v kicad-cli >/dev/null 2>&1; then
  echo "kicad-cli is required for authoritative candidate validation" >&2
  exit 69
fi

if [[ ! -f "$candidate" ]]; then
  echo "candidate does not exist: $candidate" >&2
  exit 66
fi

kicad-cli pcb drc \
  --format json \
  --exit-code-violations \
  --output "$report" \
  "$candidate"

echo "KiCad DRC passed: $report"
