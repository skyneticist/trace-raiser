#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: $0 <candidate.kicad_pcb> [report.json]" >&2
  exit 64
fi

candidate=$1
report=${2:-"${candidate%.kicad_pcb}.drc.json"}

if [[ ! -f "$candidate" ]]; then
  echo "candidate does not exist: $candidate" >&2
  exit 66
fi

if [[ -n "${KICAD_CLI:-}" ]]; then
  if [[ ! -x "$KICAD_CLI" ]]; then
    echo "KICAD_CLI is not executable: $KICAD_CLI" >&2
    exit 78
  fi
  kicad_cli=$KICAD_CLI
elif command -v kicad-cli >/dev/null 2>&1; then
  kicad_cli=$(command -v kicad-cli)
elif [[ -x /Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli ]]; then
  kicad_cli=/Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli
else
  echo "kicad-cli is required for authoritative candidate validation" >&2
  echo "install it, put it on PATH, or set KICAD_CLI to its executable path" >&2
  exit 69
fi

kicad_version=$("$kicad_cli" --version)

"$kicad_cli" pcb drc \
  --format json \
  --severity-all \
  --exit-code-violations \
  --output "$report" \
  "$candidate"

echo "KiCad $kicad_version strict DRC passed: $report"
