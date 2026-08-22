"use client";

import {
  ChangeEvent,
  DragEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  GeneratorSettings,
  ParsedBoard,
  generateStl,
  parseKicad,
} from "./lib/pcb-core";
import { stlTo3mf } from "./lib/three-mf";
import { buildTraceProfiles, traceProfilePolygons, type TraceProfile } from "./lib/manufacturing-geometry";
import { findClearanceConflicts, type ClearanceConflict } from "./lib/printability";
import { DEFAULT_SETTINGS, normalizeSettings, type SavedGeneratorSettings } from "./lib/settings";
import {
  SLICER_OPTIONS,
  SLICER_PREFERENCE_KEY,
  decideSlicerHandoff,
  isCanceledShareError,
  normalizeSlicerPreference,
  slicerActionLabel,
  slicerFallbackMessage,
  slicerOption,
  type SlicerPreference,
} from "./lib/slicer-handoff";

type ViewMode = "angled" | "top";
type ExportKind = "stl" | "3mf";
type WorkspacePanel = "source" | "shape" | "check" | "export";
type ProjectFile = {
  format: "copperline-project";
  version: 1;
  source_name: string;
  created_at: string;
  board: ParsedBoard;
  settings?: SavedGeneratorSettings;
};

export default function CopperlineStudio() {
  const [board, setBoard] = useState<ParsedBoard | null>(null);
  const [sourceName, setSourceName] = useState("sample-sensor.kicad_pcb");
  const [settings, setSettings] = useState(DEFAULT_SETTINGS);
  const [viewMode, setViewMode] = useState<ViewMode>("top");
  const [activePanel, setActivePanel] = useState<WorkspacePanel | null>(null);
  const [preferredSlicer, setPreferredSlicer] = useState<SlicerPreference>("bambu");
  const [status, setStatus] = useState("Starting the geometry engine…");
  const [busy, setBusy] = useState(false);
  const [dragActive, setDragActive] = useState(false);
  const [showGuidance, setShowGuidance] = useState(false);
  const [showAllWarnings, setShowAllWarnings] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  const loadKicadText = useCallback(async (text: string, name: string) => {
    setBusy(true);
    setStatus(`Reading ${name}…`);
    try {
      const parsed = await parseKicad(text);
      setBoard(parsed);
      setSourceName(name);
      setStatus("Board reconstructed locally. Ready to export.");
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "That board could not be read.");
    } finally {
      setBusy(false);
    }
  }, []);

  const loadSample = useCallback(async () => {
    try {
      const response = await fetch("/sample-sensor.kicad_pcb");
      if (!response.ok) throw new Error("The sample board is unavailable.");
      await loadKicadText(await response.text(), "sample-sensor.kicad_pcb");
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "The sample board could not be loaded.");
    }
  }, [loadKicadText]);

  useEffect(() => {
    const timer = window.setTimeout(() => void loadSample(), 0);
    return () => window.clearTimeout(timer);
  }, [loadSample]);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      try {
        setPreferredSlicer(normalizeSlicerPreference(window.localStorage.getItem(SLICER_PREFERENCE_KEY)));
      } catch {
        // Device-local preferences are optional when storage is unavailable.
      }
    }, 0);
    return () => window.clearTimeout(timer);
  }, []);

  useEffect(() => {
    const closeDrawer = (event: KeyboardEvent) => {
      if (event.key === "Escape") setActivePanel(null);
    };
    window.addEventListener("keydown", closeDrawer);
    return () => window.removeEventListener("keydown", closeDrawer);
  }, []);

  const importFile = useCallback(
    async (file: File) => {
      if (file.size > 12 * 1024 * 1024) {
        setStatus("Please choose a board smaller than 12 MB.");
        return;
      }

      const text = await file.text();
      if (file.name.endsWith(".json")) {
        try {
          const project = JSON.parse(text) as ProjectFile;
          if (project.format !== "copperline-project" || !project.board) {
            throw new Error("This is not a Copperline project file.");
          }
          setBoard(project.board);
          // Any imported project is legacy-compatible. A missing settings
          // object must not opt an older project into newly generated styling.
          setSettings(normalizeSettings(project.settings ?? {}));
          setSourceName(project.source_name ?? file.name);
          setStatus("Project restored locally. Ready to export.");
        } catch (error) {
          setStatus(error instanceof Error ? error.message : "That project file is invalid.");
        }
        return;
      }

      await loadKicadText(text, file.name);
    },
    [loadKicadText],
  );

  const onFileChange = (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    if (file && !busy) void importFile(file);
    event.target.value = "";
  };

  const onDrop = (event: DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    setDragActive(false);
    if (busy) return;
    const file = event.dataTransfer.files?.[0];
    if (file) void importFile(file);
  };

  const updateSetting = <K extends keyof GeneratorSettings>(
    key: K,
    value: GeneratorSettings[K],
  ) => setSettings((current) => ({ ...current, [key]: value }));

  const backTraces = useMemo(
    () => board?.traces.filter((trace) => trace.layer === "B.Cu") ?? [],
    [board],
  );

  const backPads = useMemo(
    () => board?.pads.filter((pad) => includesBackCopper(pad.layers)) ?? [],
    [board],
  );

  const backZones = useMemo(
    () => board?.zones?.filter((zone) => zone.layer === "B.Cu" && zone.kind !== "keepout") ?? [],
    [board],
  );

  const layerTraceCount = backTraces.length;

  const unsupportedPadCount = useMemo(
    () => backPads.filter(isUnsupportedCopperPad).length,
    [backPads],
  );

  const clearanceConflicts = useMemo(
    () => board ? findClearanceConflicts(board, settings) : [],
    [board, settings],
  );

  const printableWarnings = useMemo(() => {
    if (!board) return [];
    const warnings = board.warnings.filter(
      (warning) => !["UNSUPPORTED_SMD_PAD", "UNSUPPORTED_CUSTOM_PAD"].includes(warning.code),
    );
    if (layerTraceCount === 0) {
      warnings.unshift({
        code: "empty-layer",
        severity: "error" as const,
        message: "No routed traces were found on B.Cu. Route this as a single-sided back-copper board in KiCad.",
      });
    }
    if (unsupportedPadCount > 0) {
      warnings.unshift({
        code: "unsupported-pads",
        severity: "error" as const,
        message: `${unsupportedPadCount} back-copper ${unsupportedPadCount === 1 ? "pad uses" : "pads use"} an unsupported SMD, connector, or custom pad type. Replace with standard drilled through-hole pads.`,
      });
    }
    if (clearanceConflicts.length > 0) {
      const tracePairs = clearanceConflicts.filter((conflict) => conflict.kind === "trace-trace").length;
      const first = clearanceConflicts[0];
      const firstNets = `${netLabel(first.firstNetName, first.firstNet)} ↔ ${netLabel(first.secondNetName, first.secondNet)}`;
      warnings.unshift({
        code: "clearance-conflicts",
        severity: "error" as const,
        message: `${clearanceConflicts.length} different-net clearance ${clearanceConflicts.length === 1 ? "conflict is" : "conflicts are"} marked in red (${tracePairs} trace-to-trace). First: ${firstNets}, ${first.gap.toFixed(2)} mm measured vs ${first.requiredClearance.toFixed(2)} mm required.`,
      });
    }
    return warnings;
  }, [board, clearanceConflicts, layerTraceCount, unsupportedPadCount]);

  const exportBlocked = printableWarnings.some((warning) => warning.severity === "error");
  const blockingErrorCount = printableWarnings.filter(
    (warning) => warning.severity === "error",
  ).length;

  const displayedStatus = !busy && board && blockingErrorCount > 0
    ? `${blockingErrorCount} printability ${blockingErrorCount === 1 ? "error blocks" : "errors block"} export.`
    : status;

  const exportModel = async (kind: ExportKind) => {
    if (!board || busy || exportBlocked) return;
    setBusy(true);
    setStatus(`Generating ${kind.toUpperCase()} locally…`);
    try {
      const stl = await generateStl(board, settings);
      const model = kind === "3mf" ? stlTo3mf(stl, cleanName(sourceName)) : stl;
      downloadBlob(
        model,
        `${cleanName(sourceName)}-back-copper.${kind}`,
        kind === "3mf" ? "model/3mf" : "model/stl",
      );
      setStatus(`${kind.toUpperCase()} generated. Review it in your slicer before printing.`);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "The model could not be generated.");
    } finally {
      setBusy(false);
    }
  };

  const selectSlicer = (value: SlicerPreference) => {
    setPreferredSlicer(value);
    try {
      window.localStorage.setItem(SLICER_PREFERENCE_KEY, value);
    } catch {
      // Keep the in-session choice when browser storage is unavailable.
    }
  };

  const handoffToSlicer = async () => {
    if (!board || busy || exportBlocked) return;
    const selected = slicerOption(preferredSlicer);
    setBusy(true);
    setStatus(`Preparing a 3MF for ${selected.handoffLabel}…`);
    try {
      const stl = await generateStl(board, settings);
      const model = stlTo3mf(stl, cleanName(sourceName));
      const fileName = `${cleanName(sourceName)}-back-copper.3mf`;
      let file: File | null = null;
      if (typeof File === "function") {
        try {
          file = new File([model as BlobPart], fileName, { type: "model/3mf" });
        } catch {
          file = null;
        }
      }
      const shareAvailable = typeof navigator.share === "function";
      let canShareFile = false;
      if (file && shareAvailable && typeof navigator.canShare === "function") {
        try {
          canShareFile = navigator.canShare({ files: [file] });
        } catch {
          canShareFile = false;
        }
      }

      if (file && decideSlicerHandoff(shareAvailable, canShareFile) === "share") {
        try {
          await navigator.share({
            files: [file],
            title: fileName,
            text: `3MF prepared for ${selected.handoffLabel}`,
          });
          setStatus(`3MF shared. Choose ${selected.handoffLabel} if it is offered, then review the model before printing.`);
          return;
        } catch (error) {
          if (isCanceledShareError(error)) {
            setStatus("Sharing canceled. No file was downloaded.");
            return;
          }
        }
      }

      downloadBlob(model, fileName, "model/3mf");
      setStatus(slicerFallbackMessage(preferredSlicer));
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "The 3MF could not be prepared.");
    } finally {
      setBusy(false);
    }
  };

  const exportProject = () => {
    if (!board) return;
    const project: ProjectFile = {
      format: "copperline-project",
      version: 1,
      source_name: sourceName,
      created_at: new Date().toISOString(),
      board,
      settings,
    };
    downloadBlob(
      new TextEncoder().encode(JSON.stringify(project, null, 2)),
      `${cleanName(sourceName)}.copperline.json`,
      "application/json",
    );
    setStatus("Editable Copperline project saved.");
  };

  return (
    <main className="studio-shell">
      <header className="topbar">
        <div className="brand" aria-label="Copperline Studio">
          <span className="brand-mark" aria-hidden="true"><i /><i /><i /></span>
          <span>Copperline</span>
          <small>studio</small>
        </div>
        <div className="workspace-state">
          <span className="engine-state"><span className="status-dot" />Local processing</span>
          <span className="workspace-divider" aria-hidden="true" />
          <span className="engine-technology">Rust · WASM</span>
          <span className="workspace-divider" aria-hidden="true" />
          <span className="output-side">B.Cu output</span>
        </div>
        <button
          type="button"
          className={`topbar-export ${exportBlocked ? "has-errors" : ""}`}
          onClick={() => setActivePanel("export")}
          aria-controls="workspace-drawer"
          aria-expanded={activePanel === "export"}
        >
          <span>{exportBlocked ? `${blockingErrorCount} blockers` : "Export"}</span>
          <b aria-hidden="true">→</b>
        </button>
      </header>

      <section className={`workspace ${activePanel ? "has-drawer" : ""}`} aria-label="PCB model workspace">
        <nav className="workflow-rail" aria-label="Workspace tools">
          <button type="button" aria-pressed={activePanel === "source"} aria-controls="workspace-drawer" aria-expanded={activePanel === "source"} className={activePanel === "source" ? "active" : ""} onClick={() => setActivePanel((current) => current === "source" ? null : "source")}>
            <span aria-hidden="true">↑</span><small>Source</small>
          </button>
          <button type="button" aria-pressed={activePanel === "shape"} aria-controls="workspace-drawer" aria-expanded={activePanel === "shape"} className={activePanel === "shape" ? "active" : ""} onClick={() => setActivePanel((current) => current === "shape" ? null : "shape")}>
            <span aria-hidden="true">≈</span><small>Shape</small>
          </button>
          <button type="button" aria-pressed={activePanel === "check"} aria-controls="workspace-drawer" aria-expanded={activePanel === "check"} className={activePanel === "check" ? "active" : ""} onClick={() => setActivePanel((current) => current === "check" ? null : "check")}>
            <span aria-hidden="true">✓</span><small>Check</small>
            {printableWarnings.length > 0 && <i aria-hidden="true">{printableWarnings.length}</i>}
          </button>
          <button type="button" aria-pressed={activePanel === "export"} aria-controls="workspace-drawer" aria-expanded={activePanel === "export"} className={activePanel === "export" ? "active" : ""} onClick={() => setActivePanel((current) => current === "export" ? null : "export")}>
            <span aria-hidden="true">↓</span><small>Export</small>
          </button>
        </nav>

        {activePanel && <button type="button" className="drawer-backdrop" aria-hidden="true" tabIndex={-1} onClick={() => setActivePanel(null)} />}

        {(activePanel === "source" || activePanel === "shape") && (
        <aside id="workspace-drawer" className="context-drawer" aria-label={activePanel === "source" ? "Source board" : "Shape controls"}>
          <button type="button" className="drawer-close" aria-label="Close panel" onClick={() => setActivePanel(null)}>×</button>
          {activePanel === "source" && <section className="side-section source-section" aria-labelledby="source-heading">
            <div className="section-heading">
              <span className="section-index">01</span>
              <div>
                <h1 id="source-heading">Source board</h1>
                <p>From copper paths to printable form. Files stay in this browser.</p>
              </div>
            </div>

            <div
              className={`drop-zone ${dragActive ? "is-dragging" : ""} ${busy ? "is-disabled" : ""}`}
              onDragEnter={(event) => { event.preventDefault(); setDragActive(true); }}
              onDragOver={(event) => event.preventDefault()}
              onDragLeave={() => setDragActive(false)}
              onDrop={onDrop}
            >
              <input
                ref={inputRef}
                type="file"
                accept=".kicad_pcb,.json"
                onChange={onFileChange}
                aria-label="Choose a KiCad board or Copperline project"
              />
              <button type="button" className="upload-button" onClick={() => inputRef.current?.click()} disabled={busy}>
                <span className="upload-glyph" aria-hidden="true">↑</span>
                Choose board
              </button>
              <span>or drop .kicad_pcb here</span>
            </div>

            <button type="button" className="sample-button" onClick={() => void loadSample()} disabled={busy}>
              Reload sample board <span>→</span>
            </button>
          </section>}

          {activePanel === "shape" && <section className="side-section build-section" aria-labelledby="build-heading">
            <div className="section-heading build-heading-row">
              <span className="section-index">02</span>
              <div>
                <h2 id="build-heading">Build setup</h2>
                <p>All geometry controls remain available below.</p>
              </div>
              <button
                type="button"
                className={`guidance-toggle ${showGuidance ? "active" : ""}`}
                aria-pressed={showGuidance}
                onClick={() => setShowGuidance((current) => !current)}
              >
                {showGuidance ? "Hide guidance" : "Show guidance"}
              </button>
            </div>

            <div className="constraint-row" role="note">
              <span>Back copper only</span>
              <strong>B.Cu · mirrored</strong>
            </div>

            <div className="parameter-group">
              <h3>Board</h3>
              <Parameter
                label="Board thickness"
                helper="The thickness of the printed plastic board beneath the raised traces."
                showHelper={showGuidance}
                value={settings.board_thickness}
                min={0.8}
                max={3}
                step={0.1}
                unit="mm"
                onChange={(value) => updateSetting("board_thickness", value)}
              />
            </div>

            <div className="parameter-group">
              <h3>Trace geometry</h3>
              <WidthModeControl value={settings.width_mode} onChange={(value) => updateSetting("width_mode", value)} showHelper={showGuidance} />
              <Parameter
                label="Trace height"
                helper="How far each trace stands above the board so copper tape can be pressed and trimmed."
                showHelper={showGuidance}
                value={settings.trace_height}
                min={0.2}
                max={1.4}
                step={0.05}
                unit="mm"
                onChange={(value) => updateSetting("trace_height", value)}
              />
              {settings.width_mode === "auto" ? (
                <>
                  <TraceStyleControl
                    value={settings.trace_style}
                    onChange={(value) => updateSetting("trace_style", value)}
                    showHelper={showGuidance}
                  />
                  <Parameter
                    label="Trunk width"
                    helper="The broad printable width away from pads. Wider KiCad routes remain wider."
                    showHelper={showGuidance}
                    value={settings.trace_width}
                    min={1.2}
                    max={4}
                    step={0.1}
                    unit="mm"
                    onChange={(value) => setSettings((current) => ({ ...current, trace_width: value, neckdown_width: Math.min(current.neckdown_width, value) }))}
                  />
                  <Parameter
                    label="Neck-down width"
                    helper="The narrow width through a through-hole pad, keeping crowded pad exits separated."
                    showHelper={showGuidance}
                    value={settings.neckdown_width}
                    min={0.8}
                    max={settings.trace_width}
                    step={0.1}
                    unit="mm"
                    onChange={(value) => updateSetting("neckdown_width", value)}
                  />
                  <Parameter
                    label="Taper length"
                    helper="How far after the pad edge the narrow exit takes to widen into the trunk. Soft and Vintage styles use a zero-slope eased transition."
                    showHelper={showGuidance}
                    value={settings.taper_length}
                    min={0.5}
                    max={12}
                    step={0.5}
                    unit="mm"
                    onChange={(value) => updateSetting("taper_length", value)}
                  />
                  {settings.trace_style === "vintage" && (
                    <details className="shape-details">
                      <summary>Vintage shaping</summary>
                      <Parameter
                        label="Corner radius"
                        helper="Target reach of tangent corner bends. Tight turns are reduced or left unchanged; clearance checks block unsafe bends."
                        showHelper={showGuidance}
                        value={settings.corner_radius}
                        min={0.5}
                        max={12}
                        step={0.5}
                        unit="mm"
                        onChange={(value) => updateSetting("corner_radius", value)}
                      />
                      <Parameter
                        label="Teardrop length"
                        helper="How far a pad shoulder blends into its routed trace before the normal taper takes over."
                        showHelper={showGuidance}
                        value={settings.teardrop_length}
                        min={0.5}
                        max={10}
                        step={0.5}
                        unit="mm"
                        onChange={(value) => updateSetting("teardrop_length", value)}
                      />
                      <Parameter
                        label="Teardrop width"
                        helper="How much of the available pad shoulder is used. The shoulder never grows beyond the supported pad copper."
                        showHelper={showGuidance}
                        value={settings.teardrop_strength * 100}
                        min={0}
                        max={100}
                        step={5}
                        unit="%"
                        onChange={(value) => updateSetting("teardrop_strength", value / 100)}
                      />
                    </details>
                  )}
                </>
              ) : (
                <div className="preserve-note">Uses the widths and routed paths authored in KiCad. Generated tapers, teardrops, and corner shaping are paused.</div>
              )}
              <Parameter
                label="Clearance"
                helper="The required edge-to-edge gap between copper features on different electrical nets."
                showHelper={showGuidance}
                value={settings.trace_clearance}
                min={0}
                max={2}
                step={0.1}
                unit="mm"
                onChange={(value) => updateSetting("trace_clearance", value)}
              />
            </div>

            <div className="parameter-group">
              <h3>Printer fit</h3>
              <Parameter
                label="Hole compensation"
                helper="Adds a little diameter to drilled holes to offset FDM shrinkage and printer tolerance."
                showHelper={showGuidance}
                value={settings.hole_compensation}
                min={0}
                max={0.6}
                step={0.02}
                unit="mm"
                onChange={(value) => updateSetting("hole_compensation", value)}
              />
            </div>
          </section>}
        </aside>
        )}

        <section className="preview-panel">
          <div className="preview-toolbar">
            <div>
              <span className="file-kicker">ACTIVE BOARD</span>
              <strong title={sourceName}>{sourceName}</strong>
            </div>
            <div className="view-switch" role="group" aria-label="Preview angle">
              <button aria-pressed={viewMode === "angled"} className={viewMode === "angled" ? "active" : ""} onClick={() => setViewMode("angled")}>RELIEF</button>
              <button aria-pressed={viewMode === "top"} className={viewMode === "top" ? "active" : ""} onClick={() => setViewMode("top")}>TOP</button>
            </div>
          </div>

          <BoardCanvas board={board} settings={settings} viewMode={viewMode} conflicts={clearanceConflicts} />

          {clearanceConflicts.length > 0 && (
            <div className="conflict-legend" role="status"><span /> {clearanceConflicts.length} marked conflict {clearanceConflicts.length === 1 ? "location" : "locations"}</div>
          )}

          <button
            type="button"
            className={`validation-bar ${blockingErrorCount ? "has-errors" : printableWarnings.length ? "has-warnings" : "is-pass"} ${busy ? "is-busy" : ""}`}
            onClick={() => setActivePanel("check")}
            aria-label={`${displayedStatus} Open board checks.`}
            aria-controls="workspace-drawer"
            aria-expanded={activePanel === "check"}
          >
            <span className="validation-summary"><i className="status-dot" />{displayedStatus}</span>
            <strong>{blockingErrorCount ? `${blockingErrorCount} ${blockingErrorCount === 1 ? "blocker" : "blockers"}` : printableWarnings.length ? `${printableWarnings.length} ${printableWarnings.length === 1 ? "warning" : "warnings"}` : "PASS"}<b aria-hidden="true">›</b></strong>
          </button>
        </section>

        {(activePanel === "check" || activePanel === "export") && (
        <aside id="workspace-drawer" className="context-drawer report-drawer" aria-label={activePanel === "check" ? "Board report" : "Export options"}>
          <button type="button" className="drawer-close" aria-label="Close panel" onClick={() => setActivePanel(null)}>×</button>
          {activePanel === "check" && <>
          <div className="section-heading inspect-heading">
            <span className="section-index">03</span>
            <div>
              <h2>Board report</h2>
              <p>Geometry and export readiness.</p>
            </div>
          </div>
          <div className="board-measure">
            <span>BOARD SIZE</span>
            <strong>{board ? `${board.bounds.width.toFixed(1)} × ${board.bounds.height.toFixed(1)}` : "—"}</strong>
            <small>millimeters</small>
          </div>

          <div className="stat-grid">
            <Stat label="Trace segments" value={layerTraceCount} />
            <Stat label="B.Cu pads" value={backPads.length} />
            <Stat label="Holes" value={board?.stats.holes ?? 0} />
            <Stat label="Vias" value={board?.stats.vias ?? 0} />
            <Stat label="B.Cu zones" value={backZones.length} />
            <Stat label="Zone fills" value={backZones.reduce((total, zone) => total + zone.polygons.length, 0)} />
          </div>

          <div className="check-header">
            <span>PRINTABILITY</span>
            <span className={printableWarnings.length ? "check-count warning" : "check-count"}>
              {printableWarnings.length || "PASS"}
            </span>
          </div>

          <div className="warning-list">
            {!board ? (
              <div className="empty-check">Load a board to run geometry checks.</div>
            ) : printableWarnings.length === 0 ? (
              <div className="pass-check"><span>✓</span><div><strong>Ready to form</strong><small>No known printability blockers.</small></div></div>
            ) : (
              (showAllWarnings ? printableWarnings : printableWarnings.slice(0, 4)).map((warning, index) => (
                <div className={`warning-item ${warning.severity}`} key={`${warning.code}-${index}`}>
                  <span>{warning.severity === "error" ? "!" : index + 1}</span>
                  <p>{warning.message}</p>
                </div>
              ))
            )}
            {printableWarnings.length > 4 && (
              <button type="button" className="warning-toggle" onClick={() => setShowAllWarnings((current) => !current)}>
                {showAllWarnings ? "Show first four" : `Show all ${printableWarnings.length}`}
              </button>
            )}
          </div>
          </>}

          {activePanel === "export" && <>
          <div className="section-heading inspect-heading export-heading">
            <span className="section-index">04</span>
            <div>
              <h2>Export model</h2>
              <p>Download or hand the printable model to your preferred slicer.</p>
            </div>
          </div>
          <div className="export-stack">
            <button
              type="button"
              className="primary-export"
              onClick={() => void exportModel("3mf")}
              disabled={!board || busy || exportBlocked}
            >
              <span><small>RECOMMENDED</small>Download 3MF</span><b>↓</b>
            </button>
            <div className="slicer-preference">
              <label htmlFor="preferred-slicer">Preferred slicer</label>
              <select
                id="preferred-slicer"
                value={preferredSlicer}
                onChange={(event) => selectSlicer(normalizeSlicerPreference(event.target.value))}
              >
                {SLICER_OPTIONS.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
              </select>
              <p>Saved on this device. Your browser may offer this slicer or another compatible destination.</p>
            </div>
            <button
              type="button"
              className="slicer-export"
              onClick={() => void handoffToSlicer()}
              disabled={!board || busy || exportBlocked}
            >
              {slicerActionLabel(preferredSlicer)} <span>↗</span>
            </button>
            <button
              type="button"
              className="secondary-export"
              onClick={() => void exportModel("stl")}
              disabled={!board || busy || exportBlocked}
            >
              Download STL <span>↓</span>
            </button>
            <button type="button" className="project-export" onClick={exportProject} disabled={!board || busy}>
              Save editable project
            </button>
          </div>
          </>}
        </aside>
        )}
      </section>

    </main>
  );
}

function BoardCanvas({
  board,
  settings,
  viewMode,
  conflicts,
}: {
  board: ParsedBoard | null;
  settings: GeneratorSettings;
  viewMode: ViewMode;
  conflicts: ClearanceConflict[];
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const container = canvas.parentElement;
    if (!container) return;

    const render = () => {
      const rect = canvas.getBoundingClientRect();
      const ratio = Math.min(window.devicePixelRatio || 1, 2);
      canvas.width = Math.max(1, Math.floor(rect.width * ratio));
      canvas.height = Math.max(1, Math.floor(rect.height * ratio));
      const context = canvas.getContext("2d");
      if (!context) return;
      context.scale(ratio, ratio);
      drawBoard(context, rect.width, rect.height, board, settings, viewMode, conflicts);
    };

    const observer = new ResizeObserver(render);
    observer.observe(container);
    render();
    return () => observer.disconnect();
  }, [board, conflicts, settings, viewMode]);

  return <canvas ref={canvasRef} className="board-canvas" aria-label="Generated board preview" />;
}

function drawBoard(
  context: CanvasRenderingContext2D,
  width: number,
  height: number,
  board: ParsedBoard | null,
  settings: GeneratorSettings,
  viewMode: ViewMode,
  conflicts: ClearanceConflict[],
) {
  context.clearRect(0, 0, width, height);
  drawGrid(context, width, height);
  if (!board) {
    context.fillStyle = "rgba(221, 210, 197, .68)";
    context.font = "500 13px var(--font-geist-mono), monospace";
    context.textAlign = "center";
    context.fillText("WAITING FOR BOARD GEOMETRY", width / 2, height / 2);
    return;
  }

  const { min_x: minX, min_y: minY, width: boardWidth, height: boardHeight } = board.bounds;
  const outline = board.outline.length >= 3
    ? board.outline
    : [
        { x: minX, y: minY },
        { x: minX + boardWidth, y: minY },
        { x: minX + boardWidth, y: minY + boardHeight },
        { x: minX, y: minY + boardHeight },
      ];

  const padX = 68;
  const padY = 72;
  const availableWidth = Math.max(80, width - padX * 2);
  const availableHeight = Math.max(80, height - padY * 2);
  const angled = viewMode === "angled";
  const scale = angled
    ? Math.min(availableWidth / (boardWidth + boardHeight * 0.42), availableHeight / (boardHeight * 0.58 + boardWidth * 0.13))
    : Math.min(availableWidth / boardWidth, availableHeight / boardHeight);
  const centerX = width / 2;
  const centerY = height / 2 + (angled ? 18 : 0);

  const project = (point: { x: number; y: number }, z = 0) => {
    const x = point.x - minX - boardWidth / 2;
    const y = point.y - minY - boardHeight / 2;
    const mirroredX = -x;
    return angled
      ? { x: centerX + (mirroredX - y * 0.42) * scale, y: centerY + (mirroredX * 0.13 + y * 0.58) * scale - z * scale }
      : { x: centerX + mirroredX * scale, y: centerY + y * scale - z * scale };
  };

  if (angled) {
    context.save();
    context.shadowColor = "rgba(0, 0, 0, .52)";
    context.shadowBlur = 42;
    context.shadowOffsetY = 26;
    drawPolygon(context, outline.map((point) => project(point, -settings.board_thickness)));
    context.fillStyle = "#312e29";
    context.fill();
    context.restore();
  }

  const topOutline = outline.map((point) => project(point));
  drawPolygon(context, topOutline);
  const boardFill = context.createLinearGradient(0, topOutline[0]?.y ?? 0, 0, height);
  boardFill.addColorStop(0, "#ebe2d5");
  boardFill.addColorStop(1, "#bdb2a3");
  context.fillStyle = boardFill;
  context.fill();
  context.strokeStyle = "#f7eee2";
  context.lineWidth = 1.4;
  context.stroke();

  context.save();
  drawPolygon(context, topOutline);
  context.clip();

  const reliefZ = angled ? settings.trace_height : 0;
  const traceProfiles = buildTraceProfiles(board, settings);
  const visibleZones = (board.zones ?? []).filter((zone) => (
    zone.layer === "B.Cu"
    && zone.kind !== "keepout"
    && !(zone.kind === "teardrop" && settings.width_mode === "auto" && settings.trace_style === "vintage")
  ));
  context.lineCap = "round";
  context.lineJoin = "round";

  for (const zone of visibleZones) {
    for (const polygon of zone.polygons) {
      if (polygon.length < 3) continue;
      const projected = polygon.map((point) => project(point, reliefZ));
      drawPolygon(context, projected);
      context.shadowColor = angled ? "rgba(65, 28, 10, .44)" : "transparent";
      context.shadowBlur = angled ? 7 : 0;
      context.shadowOffsetY = angled ? 4 : 0;
      context.fillStyle = "#c76231";
      context.fill();
    }
  }

  // Draw every relief polygon before any copper surface so adjacent tapers
  // remain visually continuous without dark segment seams.
  fillTraceProfiles(context, traceProfiles, project, reliefZ, 0,
    settings.trace_height * .8 + 2 / scale, "rgba(65, 28, 10, .44)");

  const copperLift = angled ? settings.trace_height * scale * .2 : 0;
  fillTraceProfiles(context, traceProfiles, project, reliefZ, copperLift, 0, "#c76231");
  strokeProfileCenters(context, traceProfiles, project, reliefZ, copperLift,
    Math.max(0.6, 0.075 * scale), "rgba(255, 220, 178, .82)");

  for (const pad of board.pads) {
    const raised = isRaisedBackPad(pad);
    const hasHole = pad.drill !== null && pad.drill > 0;
    if (!raised && !hasHole) continue;
    const point = project(pad.position, raised ? reliefZ : 0);
    context.save();
    context.translate(point.x, point.y);
    // The printable B.Cu view reflects X, which reverses non-circular pad angles.
    context.rotate((-pad.rotation * Math.PI) / 180);
    if (raised) {
      context.shadowColor = "rgba(58, 22, 7, .48)";
      context.shadowBlur = angled ? 7 : 2;
      context.shadowOffsetY = angled ? 4 : 1;
      context.fillStyle = "#c76231";
      if (pad.shape === "circle" || pad.shape === "oval") {
        context.beginPath();
        context.ellipse(0, 0, (pad.size.x * scale) / 2, (pad.size.y * scale) / 2, 0, 0, Math.PI * 2);
        context.fill();
      } else {
        context.fillRect((-pad.size.x * scale) / 2, (-pad.size.y * scale) / 2, pad.size.x * scale, pad.size.y * scale);
      }
    }
    if (hasHole) {
      context.beginPath();
      context.arc(0, 0, (((pad.drill ?? 0) + settings.hole_compensation) * scale) / 2, 0, Math.PI * 2);
      context.shadowColor = "transparent";
      context.fillStyle = "#191816";
      context.fill();
    }
    context.restore();
  }

  for (const via of board.vias.filter((candidate) => includesBackCopper(candidate.layers))) {
    const point = project(via.position, reliefZ);
    context.beginPath();
    context.arc(point.x, point.y, (via.size * scale) / 2, 0, Math.PI * 2);
    context.fillStyle = "#c76231";
    context.fill();
    context.beginPath();
    context.arc(point.x, point.y, ((via.drill + settings.hole_compensation) * scale) / 2, 0, Math.PI * 2);
    context.fillStyle = "#191816";
    context.fill();
  }

  for (const conflict of conflicts) {
    const point = project(conflict.location, reliefZ + (angled ? settings.trace_height * .2 : 0));
    context.beginPath();
    context.arc(point.x, point.y, 8, 0, Math.PI * 2);
    context.fillStyle = "rgba(198, 42, 49, .3)";
    context.fill();
    context.beginPath();
    context.arc(point.x, point.y, 4, 0, Math.PI * 2);
    context.fillStyle = "#ff4f55";
    context.fill();
    context.strokeStyle = "#fff4f2";
    context.lineWidth = 1.25;
    context.stroke();
  }
  context.restore();

  if (angled) {
    context.strokeStyle = "rgba(255, 209, 166, .32)";
    context.lineWidth = 1;
    drawPolygon(context, outline.map((point) => project(point, reliefZ)));
    context.stroke();
  }
  if (!angled) drawScaleRuler(context, scale, height);
}

function drawScaleRuler(context: CanvasRenderingContext2D, pixelsPerMillimeter: number, height: number) {
  const millimeters = [20, 10, 5, 2, 1].find((candidate) => candidate * pixelsPerMillimeter <= 120) ?? 1;
  const length = millimeters * pixelsPerMillimeter;
  const x = 20;
  const y = height - 50;
  context.save();
  context.strokeStyle = "rgba(211, 201, 189, .82)";
  context.fillStyle = "rgba(211, 201, 189, .9)";
  context.lineWidth = 1;
  context.beginPath();
  context.moveTo(x, y - 6);
  context.lineTo(x, y);
  context.lineTo(x + length, y);
  context.lineTo(x + length, y - 6);
  context.stroke();
  context.font = "600 10px var(--font-geist-mono), monospace";
  context.textAlign = "left";
  context.fillText("0", x, y - 10);
  context.textAlign = "right";
  context.fillText(`${millimeters} mm`, x + length, y - 10);
  context.restore();
}

function fillTraceProfiles(
  context: CanvasRenderingContext2D,
  profiles: TraceProfile[],
  project: (point: { x: number; y: number }, z?: number) => { x: number; y: number },
  z: number,
  lift: number,
  extraWidth: number,
  fillStyle: string,
) {
  context.fillStyle = fillStyle;
  for (const profile of profiles) {
    for (const polygon of traceProfilePolygons(profile, extraWidth)) {
      const first = project(polygon[0], z);
      context.beginPath();
      context.moveTo(first.x, first.y - lift);
      for (const point of polygon.slice(1)) {
        const projected = project(point, z);
        context.lineTo(projected.x, projected.y - lift);
      }
      context.closePath();
      context.fill();
    }
  }
}

function strokeProfileCenters(
  context: CanvasRenderingContext2D,
  profiles: TraceProfile[],
  project: (point: { x: number; y: number }, z?: number) => { x: number; y: number },
  z: number,
  lift: number,
  lineWidth: number,
  strokeStyle: string,
) {
  context.strokeStyle = strokeStyle;
  context.lineWidth = lineWidth;
  for (const profile of profiles) {
    if (profile.centerline.length < 2) continue;
    const start = project(profile.centerline[0].point, z);
    context.beginPath();
    context.moveTo(start.x, start.y - lift);
    for (const sample of profile.centerline.slice(1)) {
      const point = project(sample.point, z);
      context.lineTo(point.x, point.y - lift);
    }
    context.stroke();
  }
}

function drawGrid(context: CanvasRenderingContext2D, width: number, height: number) {
  const background = context.createLinearGradient(0, 0, 0, height);
  background.addColorStop(0, "#2c2925");
  background.addColorStop(1, "#181715");
  context.fillStyle = background;
  context.fillRect(0, 0, width, height);
  context.strokeStyle = "rgba(214, 199, 181, .13)";
  context.lineWidth = 1;
  for (let x = 18; x < width; x += 24) {
    for (let y = 18; y < height; y += 24) {
      context.beginPath();
      context.arc(x, y, 0.75, 0, Math.PI * 2);
      context.stroke();
    }
  }
}

function drawPolygon(context: CanvasRenderingContext2D, points: Array<{ x: number; y: number }>) {
  if (!points.length) return;
  context.beginPath();
  context.moveTo(points[0].x, points[0].y);
  points.slice(1).forEach((point) => context.lineTo(point.x, point.y));
  context.closePath();
}

function WidthModeControl({
  value,
  onChange,
  showHelper,
}: {
  value: GeneratorSettings["width_mode"];
  onChange: (value: GeneratorSettings["width_mode"]) => void;
  showHelper: boolean;
}) {
  return (
    <div className="width-mode-field">
      <div><strong>Width mode</strong><span>Auto recommended</span></div>
      <div className="width-mode-options" role="group" aria-label="Trace width mode">
        <button type="button" aria-pressed={value === "auto"} className={value === "auto" ? "active" : ""} onClick={() => onChange("auto")}>Auto neck-down</button>
        <button type="button" aria-pressed={value === "preserve"} className={value === "preserve" ? "active" : ""} onClick={() => onChange("preserve")}>Preserve KiCad</button>
      </div>
      {showHelper && <p>{value === "auto" ? "Builds broad trunks, then narrows smoothly only where a route exits a through-hole pad." : "Keeps every routed width exactly as authored in KiCad."}</p>}
    </div>
  );
}

function TraceStyleControl({
  value,
  onChange,
  showHelper,
}: {
  value: GeneratorSettings["trace_style"];
  onChange: (value: GeneratorSettings["trace_style"]) => void;
  showHelper: boolean;
}) {
  const descriptions: Record<GeneratorSettings["trace_style"], string> = {
    technical: "Keeps straight routed segments and the original linear width transition.",
    soft: "Keeps routed paths straight but eases pad tapers for softer shoulders.",
    vintage: "Adds eased pad teardrops and tangent bends at safe, unbranched corners.",
  };
  return (
    <div className="width-mode-field trace-style-field">
      <div><strong>Trace style</strong><span>Geometry preset</span></div>
      <div className="width-mode-options trace-style-options" role="group" aria-label="Trace geometry style">
        {(["technical", "soft", "vintage"] as const).map((style) => (
          <button
            type="button"
            key={style}
            aria-pressed={value === style}
            className={value === style ? "active" : ""}
            onClick={() => onChange(style)}
          >
            {style[0].toUpperCase() + style.slice(1)}
          </button>
        ))}
      </div>
      {showHelper && <p>{descriptions[value]}</p>}
    </div>
  );
}

function Parameter({
  label,
  helper,
  showHelper,
  value,
  min,
  max,
  step,
  unit,
  onChange,
}: {
  label: string;
  helper: string;
  showHelper: boolean;
  value: number;
  min: number;
  max: number;
  step: number;
  unit: string;
  onChange: (value: number) => void;
}) {
  const progress = ((value - min) / (max - min)) * 100;
  const inputId = `parameter-${label.toLowerCase().replaceAll(" ", "-")}`;
  return (
    <div className="parameter">
      <div><label htmlFor={inputId}>{label}</label><output>{value.toFixed(step < 0.1 ? 2 : 1)} <small>{unit}</small></output></div>
      {showHelper && <p id={`${inputId}-help`}>{helper}</p>}
      <input
        id={inputId}
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        aria-valuetext={`${value.toFixed(step < 0.1 ? 2 : 1)} ${unit}`}
        aria-describedby={showHelper ? `${inputId}-help` : undefined}
        style={{ "--range-progress": `${progress}%` } as React.CSSProperties}
        onChange={(event) => onChange(Number(event.target.value))}
      />
    </div>
  );
}

function Stat({ label, value }: { label: string; value: number }) {
  return <div><span>{label}</span><strong>{value}</strong></div>;
}

function downloadBlob(bytes: Uint8Array, name: string, type: string) {
  const blob = new Blob([bytes as BlobPart], { type });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = name;
  anchor.click();
  URL.revokeObjectURL(url);
}

function cleanName(value: string) {
  return value
    .replace(/\.(kicad_pcb|copperline\.json|json)$/i, "")
    .replace(/[^a-z0-9-_]+/gi, "-")
    .replace(/^-+|-+$/g, "") || "copperline-board";
}

function includesBackCopper(layers: string[]) {
  return layers.includes("B.Cu") || layers.includes("*.Cu");
}

function isUnsupportedCopperPad(pad: ParsedBoard["pads"][number]) {
  const kind = resolvedPadType(pad);
  if (pad.shape === "custom") return true;
  return kind !== "thru_hole" && kind !== "np_thru_hole";
}

function isRaisedBackPad(pad: ParsedBoard["pads"][number]) {
  return resolvedPadType(pad) === "thru_hole" && pad.shape !== "custom" && includesBackCopper(pad.layers);
}

function resolvedPadType(pad: ParsedBoard["pads"][number]) {
  return pad.pad_type ?? pad.pad_kind ?? pad.kind ?? (pad.drill !== null ? "thru_hole" : "smd");
}

function netLabel(name: string | null, id: number | null) {
  return name || (id != null && id > 0 ? `net ${id}` : "unknown net");
}
