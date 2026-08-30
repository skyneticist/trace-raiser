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
  normalizeParsedBoard,
  parseKicad,
} from "./lib/pcb-core";
import {
  BALANCED_AUTO_LAYOUT_OPTIONS,
  generateAutoLayout,
  type AutoLayoutOptions,
  type AutoLayoutResult,
} from "./lib/auto-layout";
import { stlTo3mf } from "./lib/three-mf";
import { downloadBytes } from "./lib/browser-download";
import { buildTraceProfiles, traceProfilePolygons, type TraceProfile } from "./lib/manufacturing-geometry";
import { findClearanceConflicts, type ClearanceConflict } from "./lib/printability";
import { createPreviewProjection, RELIEF_Z_EXAGGERATION } from "./lib/preview-projection";
import { modelExportDisabledReason, summarizePrintabilityIssues } from "./lib/export-readiness";
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
type ThemeMode = "light" | "dim";
type ProposalView = "original" | "proposed";
type AutoLayoutQuality = "balanced" | "thorough";
type RoutingProposal = {
  sourceName: string;
  originalBoard: ParsedBoard;
  candidateBoard: ParsedBoard;
  result: AutoLayoutResult;
};
type PreparedModel = {
  stl: Uint8Array;
  threeMf: Uint8Array;
  file: File | null;
  stem: string;
};
type ExportSnapshot = {
  board: ParsedBoard;
  settings: GeneratorSettings;
  sourceName: string;
  retry: number;
};
type ExportPreparation =
  | { phase: "idle" }
  | { phase: "preparing"; snapshot: ExportSnapshot }
  | { phase: "ready"; snapshot: ExportSnapshot; prepared: PreparedModel }
  | { phase: "error"; snapshot: ExportSnapshot; message: string };
const THEME_PREFERENCE_KEY = "copperline-theme";
const EXPORT_PREPARATION_DELAY_MS = 160;
const DELIVERY_LOCK_MS = 700;
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
  const [sourceText, setSourceText] = useState<string | null>(null);
  const [sourceName, setSourceName] = useState("sample-sensor.kicad_pcb");
  const [settings, setSettings] = useState(DEFAULT_SETTINGS);
  const [viewMode, setViewMode] = useState<ViewMode>("top");
  const [theme, setTheme] = useState<ThemeMode>("light");
  const [preferredSlicer, setPreferredSlicer] = useState<SlicerPreference>("bambu");
  const [status, setStatus] = useState("Starting the geometry engine…");
  const [busy, setBusy] = useState(false);
  const [dragActive, setDragActive] = useState(false);
  const [showGuidance, setShowGuidance] = useState(false);
  const [exportPreparation, setExportPreparation] = useState<ExportPreparation>({ phase: "idle" });
  const [exportRetry, setExportRetry] = useState(0);
  const [autoLayoutQuality, setAutoLayoutQuality] = useState<AutoLayoutQuality>("balanced");
  const [autoLayoutOptions, setAutoLayoutOptions] = useState<AutoLayoutOptions>(() => ({
    ...BALANCED_AUTO_LAYOUT_OPTIONS,
  }));
  const [routingProposal, setRoutingProposal] = useState<RoutingProposal | null>(null);
  const [proposalView, setProposalView] = useState<ProposalView>("proposed");
  const [routingRequiresDrc, setRoutingRequiresDrc] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const sourceRevisionRef = useRef(0);
  const preparationRevisionRef = useRef(0);
  const deliveryLockRef = useRef<number | null>(null);
  const deliverySequenceRef = useRef(0);

  const loadKicadText = useCallback(async (text: string, name: string) => {
    const revision = ++sourceRevisionRef.current;
    setBusy(true);
    setStatus(`Reading ${name}…`);
    try {
      const parsed = await parseKicad(text);
      if (revision !== sourceRevisionRef.current) return;
      setBoard(parsed);
      setSourceText(text);
      setSourceName(name);
      setRoutingProposal(null);
      setRoutingRequiresDrc(false);
      setStatus("Board reconstructed locally. Ready to export.");
    } catch (error) {
      if (revision !== sourceRevisionRef.current) return;
      setStatus(error instanceof Error ? error.message : "That board could not be read.");
    } finally {
      if (revision === sourceRevisionRef.current) setBusy(false);
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

  const loadAutoLayoutSample = useCallback(async () => {
    try {
      const response = await fetch("/sample-unrouted.kicad_pcb");
      if (!response.ok) throw new Error("The AutoLayout sample is unavailable.");
      await loadKicadText(await response.text(), "sample-unrouted.kicad_pcb");
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "The AutoLayout sample could not be loaded.");
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
        if (window.localStorage.getItem(THEME_PREFERENCE_KEY) === "dim") setTheme("dim");
      } catch {
        // Device-local preferences are optional when storage is unavailable.
      }
    }, 0);
    return () => window.clearTimeout(timer);
  }, []);

  const toggleTheme = () => {
    setTheme((current) => {
      const next = current === "light" ? "dim" : "light";
      try {
        window.localStorage.setItem(THEME_PREFERENCE_KEY, next);
      } catch {
        // The visual preference remains usable for this session without storage.
      }
      return next;
    });
  };

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
          sourceRevisionRef.current += 1;
          setBoard(normalizeParsedBoard(project.board));
          setSourceText(null);
          // Any imported project is legacy-compatible. A missing settings
          // object must not opt an older project into newly generated styling.
          setSettings(normalizeSettings(project.settings ?? {}));
          setSourceName(project.source_name ?? file.name);
          setRoutingProposal(null);
          setRoutingRequiresDrc(false);
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

  const updateAutoLayoutOption = <K extends keyof AutoLayoutOptions>(
    key: K,
    value: AutoLayoutOptions[K],
  ) => setAutoLayoutOptions((current) => ({ ...current, [key]: value }));

  const autoLayoutEngineOptions = useMemo<AutoLayoutOptions>(() => (
    autoLayoutQuality === "thorough"
      ? {
          ...autoLayoutOptions,
          placement_restarts: 16,
          placement_refinement_passes: 4,
          placement_max_grid_points: 200_000,
          routing_max_search_nodes: 500_000,
          routing_reroute_passes: 12,
        }
      : {
          ...autoLayoutOptions,
          placement_restarts: 8,
          placement_refinement_passes: 2,
          placement_max_grid_points: 100_000,
          routing_max_search_nodes: 250_000,
          routing_reroute_passes: 6,
        }
  ), [autoLayoutOptions, autoLayoutQuality]);

  const previewBoard = routingProposal
    ? proposalView === "original"
      ? routingProposal.originalBoard
      : routingProposal.candidateBoard
    : board;
  const hasExistingRouting = Boolean(
    board && (
      board.traces.length > 0
      || board.vias.length > 0
      || (board.zones?.length ?? 0) > 0
    ),
  );
  const autoLayoutAvailable = Boolean(board && sourceText && !hasExistingRouting && !routingProposal);
  const autoLayoutUnavailableReason = !board
    ? "Load an unrouted KiCad board first."
    : !sourceText
      ? "Editable project files do not retain the original KiCad source. Re-open the .kicad_pcb file."
      : hasExistingRouting
        ? "This board already contains routed copper. AutoLayout never overwrites or merges existing routes."
        : null;

  const createRoutingProposal = async () => {
    if (!board || !sourceText || !autoLayoutAvailable || busy) return;
    const revision = sourceRevisionRef.current;
    const originalBoard = board;
    const originalSource = sourceText;
    const originalName = sourceName;
    setBusy(true);
    setStatus("Exploring deterministic placement and B.Cu routes locally…");
    try {
      const result = await generateAutoLayout(originalSource, autoLayoutEngineOptions);
      if (revision !== sourceRevisionRef.current) return;
      const candidateBoard = await parseKicad(result.candidate_source);
      if (revision !== sourceRevisionRef.current) return;
      setRoutingProposal({
        sourceName: originalName,
        originalBoard,
        candidateBoard,
        result,
      });
      setProposalView("proposed");
      setStatus(
        `Proposal ready: ${result.metrics.routed_net_count}/${result.metrics.total_net_count} nets routed in ${result.metrics.segment_count} segment${result.metrics.segment_count === 1 ? "" : "s"}.`,
      );
    } catch (error) {
      if (revision !== sourceRevisionRef.current) return;
      setStatus(error instanceof Error ? error.message : "AutoLayout could not create a legal proposal.");
    } finally {
      if (revision === sourceRevisionRef.current) setBusy(false);
    }
  };

  const discardRoutingProposal = () => {
    setRoutingProposal(null);
    setProposalView("proposed");
    setStatus("Routing proposal discarded. The source board is unchanged.");
  };

  const acceptRoutingProposal = () => {
    if (!routingProposal || busy) return;
    const acceptedName = `${cleanName(routingProposal.sourceName)}-autorouted.kicad_pcb`;
    try {
      downloadBytes(
        new TextEncoder().encode(routingProposal.result.candidate_source),
        acceptedName,
        "application/x-kicad-pcb",
      );
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "The routed candidate download could not be started.");
      return;
    }
    sourceRevisionRef.current += 1;
    setBoard(routingProposal.candidateBoard);
    setSourceText(routingProposal.result.candidate_source);
    setSourceName(acceptedName);
    setRoutingProposal(null);
    setRoutingRequiresDrc(true);
    setProposalView("proposed");
    setStatus("Candidate accepted; download requested. Run KiCad DRC, save, then re-import it before printable export.");
  };

  const redownloadAcceptedCandidate = () => {
    if (!routingRequiresDrc || !sourceText || busy) return;
    try {
      downloadBytes(
        new TextEncoder().encode(sourceText),
        sourceName,
        "application/x-kicad-pcb",
      );
      setStatus("Candidate download requested again. Run KiCad DRC, save, then re-import it before printable export.");
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "The routed candidate download could not be started.");
    }
  };

  const backTraces = useMemo(
    () => previewBoard?.traces.filter((trace) => trace.layer === "B.Cu") ?? [],
    [previewBoard],
  );

  const backPads = useMemo(
    () => previewBoard?.pads.filter((pad) => includesBackCopper(pad.layers)) ?? [],
    [previewBoard],
  );

  const backZones = useMemo(
    () => previewBoard?.zones?.filter((zone) => zone.layer === "B.Cu" && zone.kind !== "keepout") ?? [],
    [previewBoard],
  );

  const layerTraceCount = backTraces.length;
  const profileSummary = settings.width_mode === "preserve"
    ? "Preserve KiCad"
    : `${traceStyleLabel(settings.trace_style)} · Auto`;
  const widthSummary = settings.width_mode === "preserve"
    ? "Authored per trace"
    : `${settings.trace_width.toFixed(1)} / ${settings.neckdown_width.toFixed(1)} mm`;
  const modelSizeSummary = previewBoard
    ? `${previewBoard.bounds.width.toFixed(1)} × ${previewBoard.bounds.height.toFixed(1)} × ${(settings.board_thickness + settings.trace_height).toFixed(2)} mm`
    : "—";
  const verticalStackSummary = `${settings.board_thickness.toFixed(2)} + ${settings.trace_height.toFixed(2)} mm`;

  const unsupportedPadCount = useMemo(
    () => backPads.filter(isUnsupportedCopperPad).length,
    [backPads],
  );

  const clearanceConflicts = useMemo(
    () => previewBoard ? findClearanceConflicts(previewBoard, settings) : [],
    [previewBoard, settings],
  );

  const printabilityIssues = useMemo(() => {
    if (!previewBoard) return [];
    const warnings = previewBoard.warnings.filter(
      (warning) => !["UNSUPPORTED_SMD_PAD", "UNSUPPORTED_CUSTOM_PAD"].includes(warning.code),
    );
    if (routingProposal) {
      warnings.unshift({
        code: "routing-proposal-pending",
        severity: "error" as const,
        message: "This is a review-only routing proposal. Accept it only after comparing both views; printable export remains locked.",
      });
    }
    if (routingRequiresDrc) {
      warnings.unshift({
        code: "kicad-drc-required",
        severity: "error" as const,
        message: "KiCad DRC is still required. Open the downloaded candidate in KiCad, run Inspect → Design Rules Checker, save it, then re-import that checked board.",
      });
    }
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
      const netlessPairs = clearanceConflicts.filter(
        (conflict) => conflict.firstNet === null || conflict.secondNet === null,
      ).length;
      const first = clearanceConflicts[0];
      const firstNets = `${netLabel(first.firstNetName, first.firstNet)} ↔ ${netLabel(first.secondNetName, first.secondNet)}`;
      warnings.unshift({
        code: "clearance-conflicts",
        severity: "error" as const,
        message: `${clearanceConflicts.length} copper clearance ${clearanceConflicts.length === 1 ? "conflict is" : "conflicts are"} marked in red (${tracePairs} trace-to-trace${netlessPairs ? `; ${netlessPairs} involve unassigned nets` : ""}). First: ${firstNets}, ${first.gap.toFixed(2)} mm measured vs ${first.requiredClearance.toFixed(2)} mm required.`,
      });
    }
    return warnings;
  }, [clearanceConflicts, layerTraceCount, previewBoard, routingProposal, routingRequiresDrc, unsupportedPadCount]);

  const {
    blockerCount: blockingErrorCount,
    warningCount: nonBlockingWarningCount,
    orderedIssues: printableWarnings,
    firstBlocker,
  } = useMemo(() => summarizePrintabilityIssues(printabilityIssues), [printabilityIssues]);
  const exportBlocked = blockingErrorCount > 0;
  const eligibilityReason = modelExportDisabledReason(Boolean(board), busy, blockingErrorCount);

  useEffect(() => {
    const revision = ++preparationRevisionRef.current;

    if (!board || busy || exportBlocked) {
      return;
    }

    const snapshot = { board, settings, sourceName, retry: exportRetry };
    const timer = window.setTimeout(() => {
      if (revision !== preparationRevisionRef.current) return;
      setExportPreparation({ phase: "preparing", snapshot });
      setStatus("Preparing verified export files locally…");
      const stem = cleanName(sourceName);

      void generateStl(board, settings)
        .then((stl) => {
          if (revision !== preparationRevisionRef.current) return;
          const threeMf = stlTo3mf(stl, stem);
          if (revision !== preparationRevisionRef.current) return;
          let file: File | null = null;
          if (typeof File === "function") {
            try {
              file = new File([threeMf as BlobPart], `${stem}-back-copper.3mf`, { type: "model/3mf" });
            } catch {
              file = null;
            }
          }
          setExportPreparation({
            phase: "ready",
            snapshot,
            prepared: { stl, threeMf, file, stem },
          });
          setStatus("Verified STL and 3MF files are prepared locally.");
        })
        .catch((error) => {
          if (revision !== preparationRevisionRef.current) return;
          const message = error instanceof Error ? error.message : "The printable model could not be prepared.";
          setExportPreparation({ phase: "error", snapshot, message });
          setStatus(message);
        });
    }, EXPORT_PREPARATION_DELAY_MS);

    return () => {
      window.clearTimeout(timer);
      if (preparationRevisionRef.current === revision) preparationRevisionRef.current += 1;
    };
  }, [board, busy, exportBlocked, exportRetry, settings, sourceName]);

  const currentPreparation = exportPreparation.phase !== "idle"
    && exportPreparation.snapshot.board === board
    && exportPreparation.snapshot.settings === settings
    && exportPreparation.snapshot.sourceName === sourceName
    && exportPreparation.snapshot.retry === exportRetry
    ? exportPreparation
    : null;
  const preparationPhase = currentPreparation?.phase
    ?? (!eligibilityReason ? "debouncing" : "idle");
  const preparedModel = currentPreparation?.phase === "ready"
    ? currentPreparation.prepared
    : null;
  const modelReady = !eligibilityReason
    && preparationPhase === "ready"
    && preparedModel !== null;
  const exportDisabledReason = eligibilityReason
    ?? (preparationPhase === "error"
      ? "Export preparation failed."
      : modelReady
        ? null
        : "Preparing verified export files…");
  const exportReadinessState = exportBlocked || preparationPhase === "error"
    ? "is-blocked"
    : exportDisabledReason
      ? "is-pending"
      : "is-ready";

  const displayedStatus = !busy && !routingProposal && !routingRequiresDrc && board && blockingErrorCount > 0
    ? `${blockingErrorCount} printability ${blockingErrorCount === 1 ? "error blocks" : "errors block"} export.`
    : status;

  const acquireDelivery = () => {
    if (deliveryLockRef.current !== null) {
      setStatus("The previous export request is already being handled.");
      return false;
    }
    const token = ++deliverySequenceRef.current;
    deliveryLockRef.current = token;
    window.setTimeout(() => {
      if (deliveryLockRef.current === token) deliveryLockRef.current = null;
    }, DELIVERY_LOCK_MS);
    return true;
  };

  const releaseDelivery = () => {
    deliveryLockRef.current = null;
  };

  const exportModel = (kind: ExportKind) => {
    const prepared = preparedModel;
    if (!modelReady || !prepared || !acquireDelivery()) return;
    try {
      const model = kind === "3mf" ? prepared.threeMf : prepared.stl;
      downloadBytes(
        model,
        `${prepared.stem}-back-copper.${kind}`,
        kind === "3mf" ? "model/3mf" : "model/stl",
      );
      setStatus(`${kind.toUpperCase()} download requested from the browser. Review it in your slicer before printing.`);
    } catch (error) {
      releaseDelivery();
      setStatus(error instanceof Error ? error.message : "The model could not be generated.");
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

  const handoffToSlicer = () => {
    const prepared = preparedModel;
    if (!modelReady || !prepared || !acquireDelivery()) return;
    const selected = slicerOption(preferredSlicer);
    const fileName = `${prepared.stem}-back-copper.3mf`;
    const file = prepared.file;
    try {
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
        // Call share synchronously in the click task. Awaiting generation first
        // loses the transient user activation required by Web Share.
        void navigator.share({
          files: [file],
          title: fileName,
          text: `3MF prepared for ${selected.handoffLabel}`,
        }).then(() => {
          setStatus(`3MF shared. Choose ${selected.handoffLabel} if it is offered, then review the model before printing.`);
        }).catch((error) => {
          if (isCanceledShareError(error)) {
            setStatus("Sharing canceled. No file was downloaded.");
          } else {
            setStatus("The system share sheet did not accept the 3MF. Use Download 3MF instead.");
          }
        });
        return;
      }

      downloadBytes(prepared.threeMf, fileName, "model/3mf");
      setStatus(slicerFallbackMessage(preferredSlicer));
    } catch (error) {
      releaseDelivery();
      setStatus(error instanceof Error ? error.message : "The 3MF could not be prepared.");
    }
  };

  const exportProject = () => {
    if (!board || routingProposal || routingRequiresDrc || !acquireDelivery()) return;
    const project: ProjectFile = {
      format: "copperline-project",
      version: 1,
      source_name: sourceName,
      created_at: new Date().toISOString(),
      board,
      settings,
    };
    try {
      downloadBytes(
        new TextEncoder().encode(JSON.stringify(project, null, 2)),
        `${cleanName(sourceName)}.copperline.json`,
        "application/json",
      );
      setStatus("Editable Copperline project download started.");
    } catch (error) {
      releaseDelivery();
      setStatus(error instanceof Error ? error.message : "The editable project download could not be started.");
    }
  };

  return (
    <main className="studio-shell" data-theme={theme}>
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
        <div className="header-actions">
          <button
            type="button"
            className="theme-toggle"
            onClick={toggleTheme}
            aria-label={`Switch to ${theme === "light" ? "dim" : "light"} theme`}
            aria-pressed={theme === "dim"}
            title={`Use ${theme === "light" ? "dim" : "light"} theme`}
          >
            <span aria-hidden="true">{theme === "light" ? "◐" : "○"}</span>
            {theme === "light" ? "Dim" : "Light"}
          </button>
        </div>
      </header>

      <section className="workspace" aria-label="PCB model workspace">
        <aside className="controls-pane" aria-label="Source and shaping controls">
          <section className="side-section panel-card source-section" aria-labelledby="source-heading">
            <div className="section-heading">
              <span className="section-index">01</span>
              <div>
                <h1 id="source-heading">Source board</h1>
                <p>Choose a routed board or prepare an unrouted proposal. Files stay in this browser.</p>
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

            <div className="sample-actions">
              <button type="button" className="sample-button" onClick={() => void loadSample()} disabled={busy}>
                Reload routed sample <span>→</span>
              </button>
              <button type="button" className="sample-button" onClick={() => void loadAutoLayoutSample()} disabled={busy}>
                Try unrouted AutoLayout demo <span>→</span>
              </button>
            </div>

            <div className={`auto-layout-card ${routingProposal ? "has-proposal" : ""} ${routingRequiresDrc ? "requires-drc" : ""}`}>
              <div className="auto-layout-heading">
                <div>
                  <span>AUTO-PLACE + ROUTE</span>
                  <strong>{routingProposal ? "Review proposal" : routingRequiresDrc ? "Awaiting KiCad DRC" : "Single-layer B.Cu"}</strong>
                </div>
                <i>{routingProposal ? "READY" : routingRequiresDrc ? "CHECK" : "LOCAL"}</i>
              </div>

              {routingProposal ? (
                <>
                  <div className="proposal-compare" role="group" aria-label="Compare routing proposal">
                    <button
                      type="button"
                      className={proposalView === "original" ? "active" : ""}
                      aria-pressed={proposalView === "original"}
                      onClick={() => setProposalView("original")}
                    >
                      Original
                    </button>
                    <button
                      type="button"
                      className={proposalView === "proposed" ? "active" : ""}
                      aria-pressed={proposalView === "proposed"}
                      onClick={() => setProposalView("proposed")}
                    >
                      Proposed
                    </button>
                  </div>
                  <div className="proposal-metrics">
                    <div><span>Parts moved</span><strong>{routingProposal.result.metrics.moved_component_count}</strong></div>
                    <div><span>Nets routed</span><strong>{routingProposal.result.metrics.routed_net_count}/{routingProposal.result.metrics.total_net_count}</strong></div>
                    <div><span>Segments</span><strong>{routingProposal.result.metrics.segment_count}</strong></div>
                    <div><span>Trace length</span><strong>{routingProposal.result.metrics.trace_length_mm.toFixed(1)} mm</strong></div>
                    <div><span>Bends</span><strong>{routingProposal.result.metrics.bend_count}</strong></div>
                    <div><span>Seed</span><strong>{routingProposal.result.seed}</strong></div>
                  </div>
                  <p className="proposal-id" title={routingProposal.result.proposal_id}>
                    Replay {routingProposal.result.proposal_id.slice(7, 19)}
                  </p>
                  <div className="drc-boundary" role="note">
                    <strong>KiCad remains the authority.</strong>
                    <span>Accepting downloads your design choice; it does not certify DRC.</span>
                  </div>
                  <div className="proposal-actions">
                    <button type="button" className="accept-proposal" onClick={acceptRoutingProposal} disabled={busy}>
                      Accept + download
                    </button>
                    <button type="button" className="discard-proposal" onClick={discardRoutingProposal} disabled={busy}>
                      Discard
                    </button>
                  </div>
                </>
              ) : routingRequiresDrc ? (
                <>
                  <div className="drc-boundary" role="note">
                    <strong>KiCad DRC is required.</strong>
                    <span>Check and save this candidate in KiCad, then re-import it to unlock printable export.</span>
                  </div>
                  <button
                    type="button"
                    className="run-auto-layout"
                    onClick={redownloadAcceptedCandidate}
                    disabled={busy || !sourceText}
                  >
                    Download candidate again<span>↓</span>
                  </button>
                </>
              ) : (
                <>
                  <p className={`auto-layout-eligibility ${autoLayoutAvailable ? "is-ready" : ""}`}>
                    {autoLayoutAvailable
                      ? "Ready for a deterministic, review-first proposal."
                      : autoLayoutUnavailableReason}
                  </p>
                  <div className="quality-picker" role="group" aria-label="AutoLayout search quality">
                    <button
                      type="button"
                      className={autoLayoutQuality === "balanced" ? "active" : ""}
                      aria-pressed={autoLayoutQuality === "balanced"}
                      onClick={() => setAutoLayoutQuality("balanced")}
                    >
                      Balanced
                    </button>
                    <button
                      type="button"
                      className={autoLayoutQuality === "thorough" ? "active" : ""}
                      aria-pressed={autoLayoutQuality === "thorough"}
                      onClick={() => setAutoLayoutQuality("thorough")}
                    >
                      Thorough
                    </button>
                  </div>
                  <div className="route-settings">
                    <div className="route-settings-heading">Routing constraints</div>
                    <div className="route-setting-grid">
                      <label>
                        <span>Trace width <small>mm</small></span>
                        <input type="number" min="0.1" max="20" step="0.1" value={autoLayoutOptions.trace_width_mm} onChange={(event) => updateAutoLayoutOption("trace_width_mm", Number(event.target.value))} />
                      </label>
                      <label>
                        <span>Copper clearance <small>mm</small></span>
                        <input type="number" min="0" max="20" step="0.1" value={autoLayoutOptions.trace_clearance_mm} onChange={(event) => updateAutoLayoutOption("trace_clearance_mm", Number(event.target.value))} />
                      </label>
                      <label>
                        <span>Part clearance <small>mm</small></span>
                        <input type="number" min="0" max="20" step="0.1" value={autoLayoutOptions.component_clearance_mm} onChange={(event) => updateAutoLayoutOption("component_clearance_mm", Number(event.target.value))} />
                      </label>
                      <label>
                        <span>Edge clearance <small>mm</small></span>
                        <input type="number" min="0" max="20" step="0.1" value={autoLayoutOptions.edge_clearance_mm} onChange={(event) => updateAutoLayoutOption("edge_clearance_mm", Number(event.target.value))} />
                      </label>
                      <label className="seed-setting">
                        <span>Replay seed</span>
                        <input
                          type="number"
                          min="0"
                          max={Number.MAX_SAFE_INTEGER}
                          step="1"
                          value={autoLayoutOptions.seed}
                          onChange={(event) => updateAutoLayoutOption("seed", Math.max(0, Math.trunc(Number(event.target.value))))}
                        />
                      </label>
                    </div>
                  </div>
                  <button
                    type="button"
                    className="run-auto-layout"
                    onClick={() => void createRoutingProposal()}
                    disabled={!autoLayoutAvailable || busy}
                  >
                    {busy ? "Working locally…" : "Create routing proposal"}<span>→</span>
                  </button>
                  <small className="auto-layout-limits">Front-side through-hole parts · B.Cu traces · no existing copper</small>
                </>
              )}
            </div>
          </section>

          <section className="side-section build-section" aria-labelledby="build-heading">
            <div className="section-heading build-heading-row">
              <span className="section-index">02</span>
              <div>
                <h2 id="build-heading">Build setup</h2>
                <p>Shape the printable board and routed copper.</p>
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
              <span>Output layer</span>
              <strong>B.Cu · mirrored · local</strong>
            </div>

            <section className="control-group form-group" aria-labelledby="form-settings-heading">
              <h3 id="form-settings-heading"><span>Form</span><small>Printed substrate</small></h3>
              <div className="settings-grid">
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
            </section>

            <section className="control-group" aria-labelledby="routing-settings-heading">
              <h3 id="routing-settings-heading"><span>Routing</span><small>Widths and transitions</small></h3>
              <div className="settings-grid">
                <div className="settings-span">
                <WidthModeControl value={settings.width_mode} onChange={(value) => updateSetting("width_mode", value)} showHelper={showGuidance} />
                </div>
                {settings.width_mode === "auto" ? (
                  <>
                    <div className="settings-span">
                      <TraceStyleControl
                        value={settings.trace_style}
                        onChange={(value) => updateSetting("trace_style", value)}
                        showHelper={showGuidance}
                      />
                    </div>
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
                  </>
                ) : (
                  <div className="preserve-note settings-span">Uses the widths and routed paths authored in KiCad. Generated tapers, teardrops, and corner shaping are paused.</div>
                )}
              </div>
            </section>

            {settings.width_mode === "auto" && settings.trace_style === "vintage" && (
              <section className="control-group vintage-group" aria-labelledby="vintage-settings-heading">
                <h3 id="vintage-settings-heading"><span>Vintage profile</span><small>Organic trace shaping</small></h3>
                <div className="vintage-grid">
                  <Parameter
                    label="Corner radius"
                    helper="Target centerline radius for circular corner fillets. Short segments reduce the radius, and unsafe bends remain unchanged."
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
                    helper="How far a pad shoulder blends into its routed trace."
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
                    helper="How much of the available pad shoulder is used."
                    showHelper={showGuidance}
                    value={settings.teardrop_strength * 100}
                    min={0}
                    max={100}
                    step={5}
                    unit="%"
                    onChange={(value) => updateSetting("teardrop_strength", value / 100)}
                  />
                </div>
              </section>
            )}
          </section>
        </aside>

        <section className="preview-panel">
          <div className="preview-toolbar">
            <div>
              <span className="file-kicker">
                {routingProposal ? `${proposalView === "original" ? "ORIGINAL" : "PROPOSED"} · ROUTING REVIEW` : "ACTIVE BOARD"}
              </span>
              <strong title={sourceName}>{sourceName}</strong>
            </div>
            <div className="view-switch" role="group" aria-label="Preview angle">
              <button
                aria-pressed={viewMode === "angled"}
                className={viewMode === "angled" ? "active" : ""}
                title={`Raised form preview with ${RELIEF_Z_EXAGGERATION}× vertical emphasis`}
                onClick={() => setViewMode("angled")}
              >RELIEF</button>
              <button aria-pressed={viewMode === "top"} className={viewMode === "top" ? "active" : ""} onClick={() => setViewMode("top")}>TOP</button>
            </div>
          </div>

          <BoardCanvas board={previewBoard} settings={settings} viewMode={viewMode} conflicts={clearanceConflicts} theme={theme} />

          {clearanceConflicts.length > 0 && (
            <div className="conflict-legend" role="status"><span /> {clearanceConflicts.length} marked conflict {clearanceConflicts.length === 1 ? "location" : "locations"}</div>
          )}

          <div
            className={`validation-bar ${blockingErrorCount ? "has-errors" : printableWarnings.length ? "has-warnings" : "is-pass"} ${busy ? "is-busy" : ""}`}
            role="status"
            aria-live="polite"
          >
            <span className="validation-summary"><i className="status-dot" /><span>{displayedStatus}</span></span>
            <strong>{blockingErrorCount ? `${blockingErrorCount} ${blockingErrorCount === 1 ? "blocker" : "blockers"}` : printableWarnings.length ? `${printableWarnings.length} ${printableWarnings.length === 1 ? "warning" : "warnings"}` : "PASS"}</strong>
          </div>
        </section>

        <aside className="output-pane" aria-label="Board checks and export options">
          <section className="panel-card report-section" aria-labelledby="report-heading">
            <div className="section-heading inspect-heading">
              <span className="section-index">03</span>
              <div>
                <h2 id="report-heading">Board report</h2>
                <p>Geometry and export readiness.</p>
              </div>
            </div>
            <div className="board-summary">
              <div className="board-measure">
                <span>BOARD SIZE</span>
                <strong>{previewBoard ? `${previewBoard.bounds.width.toFixed(1)} × ${previewBoard.bounds.height.toFixed(1)}` : "—"}</strong>
                <small>millimeters</small>
              </div>

              <div className="stat-grid">
                <Stat label="Trace segments" value={layerTraceCount} />
                <Stat label="B.Cu pads" value={backPads.length} />
                <Stat label="Holes" value={previewBoard?.stats.holes ?? 0} />
                <Stat label="Vias" value={previewBoard?.stats.vias ?? 0} />
                <Stat label="B.Cu zones" value={backZones.length} />
                <Stat label="Zone fills" value={backZones.reduce((total, zone) => total + zone.polygons.length, 0)} />
              </div>
            </div>

            <div className="recipe-summary" aria-label="Output recipe">
              <div className="recipe-heading">OUTPUT RECIPE</div>
              <div className="recipe-grid">
                <RecipeStat label="Overall size" value={modelSizeSummary} />
                <RecipeStat label="Vertical stack" value={verticalStackSummary} />
                <RecipeStat label="Profile" value={profileSummary} />
                <RecipeStat label="Route widths" value={widthSummary} />
              </div>
            </div>

            <div className="printability-group">
              <div className="check-header">
                <span>PRINTABILITY</span>
                <span className="check-counts">
                  {blockingErrorCount > 0 && (
                    <span className="check-count error">{blockingErrorCount} {blockingErrorCount === 1 ? "BLOCKER" : "BLOCKERS"}</span>
                  )}
                  {nonBlockingWarningCount > 0 && (
                    <span className="check-count warning">{nonBlockingWarningCount} {nonBlockingWarningCount === 1 ? "WARNING" : "WARNINGS"}</span>
                  )}
                  {printableWarnings.length === 0 && <span className="check-count">PASS</span>}
                </span>
              </div>

              <div className="warning-list">
                {!previewBoard ? (
                  <div className="empty-check">Load a board to run geometry checks.</div>
                ) : printableWarnings.length === 0 ? (
                  <div className="pass-check"><span>✓</span><div><strong>Ready to form</strong><small>No known printability blockers.</small></div></div>
                ) : printableWarnings.map((warning, index) => (
                  <div className={`warning-item ${warning.severity}`} key={`${warning.code}-${index}`}>
                    <span>{warning.severity === "error" ? "!" : index + 1}</span>
                    <p>{warning.message}</p>
                  </div>
                ))}
              </div>
            </div>
          </section>

          <section className="panel-card export-section" aria-labelledby="export-heading">
            <div className="section-heading inspect-heading export-heading">
              <span className="section-index">04</span>
              <div>
                <h2 id="export-heading">Export model</h2>
                <p>Download or hand off the printable model.</p>
              </div>
            </div>
            <div className="export-stack">
              <div
                id="model-export-readiness"
                className={`export-readiness ${exportReadinessState}`}
                role="status"
              >
                <strong>{exportDisabledReason ?? "Model prepared."}</strong>
                {firstBlocker && !busy && <span>{firstBlocker.message}</span>}
                {!eligibilityReason && currentPreparation?.phase === "error" && (
                  <>
                    <span>{currentPreparation.message}</span>
                    <button type="button" className="export-retry" onClick={() => setExportRetry((value) => value + 1)}>
                      Retry preparation
                    </button>
                  </>
                )}
                {modelReady && preparedModel && (
                  <span>Prepared locally · 3MF {formatFileSize(preparedModel.threeMf.byteLength)} · STL {formatFileSize(preparedModel.stl.byteLength)}</span>
                )}
                {!exportDisabledReason && nonBlockingWarningCount > 0 && (
                  <span>{nonBlockingWarningCount} non-blocking {nonBlockingWarningCount === 1 ? "warning remains" : "warnings remain"} for review.</span>
                )}
              </div>
              <button
                type="button"
                className="primary-export"
                onClick={() => exportModel("3mf")}
                disabled={!modelReady}
                aria-describedby="model-export-readiness"
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
                <p>Saved on this device. The browser may offer any compatible destination.</p>
              </div>
              <button
                type="button"
                className="slicer-export"
                onClick={handoffToSlicer}
                disabled={!modelReady}
                aria-describedby="model-export-readiness"
              >
                {slicerActionLabel(preferredSlicer)} <span>↗</span>
              </button>
              <button
                type="button"
                className="secondary-export"
                onClick={() => exportModel("stl")}
                disabled={!modelReady}
                aria-describedby="model-export-readiness"
              >
                Download STL <span>↓</span>
              </button>
              <button
                type="button"
                className="project-export"
                onClick={exportProject}
                disabled={!board || busy || Boolean(routingProposal) || routingRequiresDrc}
              >
                Save editable project
              </button>
            </div>
          </section>
        </aside>
      </section>
    </main>
  );
}

function BoardCanvas({
  board,
  settings,
  viewMode,
  conflicts,
  theme,
}: {
  board: ParsedBoard | null;
  settings: GeneratorSettings;
  viewMode: ViewMode;
  conflicts: ClearanceConflict[];
  theme: ThemeMode;
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
      drawBoard(context, rect.width, rect.height, board, settings, viewMode, conflicts, theme);
    };

    const observer = new ResizeObserver(render);
    observer.observe(container);
    render();
    return () => observer.disconnect();
  }, [board, conflicts, settings, theme, viewMode]);

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
  theme: ThemeMode,
) {
  const palette = CANVAS_THEMES[theme];
  context.clearRect(0, 0, width, height);
  drawGrid(context, width, height, palette);
  if (!board) {
    context.fillStyle = palette.canvasText;
    context.font = '500 13px "Fira Code", monospace';
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

  const { angled, project, reliefTraceHeight: reliefZ, scale } = createPreviewProjection({
    width,
    height,
    bounds: board.bounds,
    viewMode,
    boardThickness: settings.board_thickness,
    traceHeight: settings.trace_height,
  });

  if (angled) {
    context.save();
    context.shadowColor = palette.boardShadow;
    context.shadowBlur = 24;
    context.shadowOffsetY = 12;
    drawPolygon(context, outline.map((point) => project(point, -settings.board_thickness)));
    context.fillStyle = palette.boardEdge;
    context.fill();
    context.restore();
    fillExtrudedLayers(context, [outline], project, -settings.board_thickness, 0, scale, palette.boardEdge);
  }

  const topOutline = outline.map((point) => project(point));
  drawPolygon(context, topOutline);
  context.fillStyle = palette.board;
  context.fill();
  context.strokeStyle = palette.boardEdgeHighlight;
  context.lineWidth = 1.4;
  context.stroke();

  context.save();
  if (!angled) {
    drawPolygon(context, topOutline);
    context.clip();
  }

  const traceProfiles = buildTraceProfiles(board, settings);
  const visibleZones = (board.zones ?? []).filter((zone) => (
    zone.layer === "B.Cu"
    && zone.kind !== "keepout"
    && !(zone.kind === "teardrop" && settings.width_mode === "auto" && settings.trace_style === "vintage")
  ));
  context.lineCap = "round";
  context.lineJoin = "round";

  if (angled) {
    const raisedPads = board.pads.filter(isRaisedBackPad);
    const backVias = board.vias.filter((candidate) => includesBackCopper(candidate.layers));
    const copperPolygons = [
      ...visibleZones.flatMap((zone) => zone.polygons),
      ...traceProfiles.flatMap((profile) => traceProfilePolygons(profile)),
      ...raisedPads.map(previewPadPolygon),
      ...backVias.map((via) => ellipsePreviewPolygon(via.position, { x: via.size, y: via.size }, 0)),
    ].filter((polygon) => polygon.length >= 3);

    context.save();
    context.shadowColor = palette.traceShadow;
    context.shadowBlur = 8;
    context.shadowOffsetY = 5;
    fillProjectedPolygons(context, copperPolygons, project, 0, palette.traceRelief);
    context.restore();
    fillExtrudedLayers(context, copperPolygons, project, 0, reliefZ, scale, palette.traceRelief);
    fillProjectedPolygons(context, copperPolygons, project, reliefZ, palette.trace);
    strokeProfileCenters(context, traceProfiles, project, reliefZ, 0,
      Math.max(0.5, 0.055 * scale), palette.traceHighlight);

    for (const pad of board.pads) {
      if (pad.drill === null || pad.drill <= 0) continue;
      const holeDiameter = pad.drill + settings.hole_compensation;
      const hole = ellipsePreviewPolygon(pad.position, { x: holeDiameter, y: holeDiameter }, 0);
      fillProjectedPolygons(context, [hole], project, isRaisedBackPad(pad) ? reliefZ : 0, palette.hole);
    }
    for (const via of backVias) {
      const holeDiameter = via.drill + settings.hole_compensation;
      const hole = ellipsePreviewPolygon(via.position, { x: holeDiameter, y: holeDiameter }, 0);
      fillProjectedPolygons(context, [hole], project, reliefZ, palette.hole);
    }

    drawConflictMarkers(context, conflicts, project, reliefZ + settings.trace_height * 0.25, palette);
    context.restore();
    drawReliefLegend(context, height, settings, palette);
    return;
  }

  for (const zone of visibleZones) {
    for (const polygon of zone.polygons) {
      if (polygon.length < 3) continue;
      const projected = polygon.map((point) => project(point, reliefZ));
      drawPolygon(context, projected);
      context.shadowColor = angled ? palette.traceShadow : "transparent";
      context.shadowBlur = angled ? 7 : 0;
      context.shadowOffsetY = angled ? 4 : 0;
      context.fillStyle = palette.trace;
      context.fill();
    }
  }

  // Draw every relief polygon before any copper surface so adjacent tapers
  // remain visually continuous without dark segment seams.
  fillTraceProfiles(context, traceProfiles, project, reliefZ, 0,
    settings.trace_height * .8 + 2 / scale, palette.traceRelief);

  const copperLift = angled ? settings.trace_height * scale * .2 : 0;
  fillTraceProfiles(context, traceProfiles, project, reliefZ, copperLift, 0, palette.trace);
  strokeProfileCenters(context, traceProfiles, project, reliefZ, copperLift,
    Math.max(0.5, 0.06 * scale), palette.traceHighlight);

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
      context.shadowColor = palette.traceShadow;
      context.shadowBlur = angled ? 7 : 2;
      context.shadowOffsetY = angled ? 4 : 1;
      context.fillStyle = palette.trace;
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
      context.fillStyle = palette.hole;
      context.fill();
    }
    context.restore();
  }

  for (const via of board.vias.filter((candidate) => includesBackCopper(candidate.layers))) {
    const point = project(via.position, reliefZ);
    context.beginPath();
    context.arc(point.x, point.y, (via.size * scale) / 2, 0, Math.PI * 2);
    context.fillStyle = palette.trace;
    context.fill();
    context.beginPath();
    context.arc(point.x, point.y, ((via.drill + settings.hole_compensation) * scale) / 2, 0, Math.PI * 2);
    context.fillStyle = palette.hole;
    context.fill();
  }

  drawConflictMarkers(context, conflicts, project, reliefZ, palette);
  context.restore();
  drawScaleRuler(context, scale, height, palette);
}

function drawScaleRuler(context: CanvasRenderingContext2D, pixelsPerMillimeter: number, height: number, palette: CanvasPalette) {
  const millimeters = [20, 10, 5, 2, 1].find((candidate) => candidate * pixelsPerMillimeter <= 120) ?? 1;
  const length = millimeters * pixelsPerMillimeter;
  const x = 20;
  const y = height - 50;
  context.save();
  context.strokeStyle = palette.ruler;
  context.fillStyle = palette.ruler;
  context.lineWidth = 1;
  context.beginPath();
  context.moveTo(x, y - 6);
  context.lineTo(x, y);
  context.lineTo(x + length, y);
  context.lineTo(x + length, y - 6);
  context.stroke();
  context.font = '600 10px "Fira Code", monospace';
  context.textAlign = "left";
  context.fillText("0", x, y - 10);
  context.textAlign = "right";
  context.fillText(`${millimeters} mm`, x + length, y - 10);
  context.restore();
}

function fillProjectedPolygons(
  context: CanvasRenderingContext2D,
  polygons: Array<Array<{ x: number; y: number }>>,
  project: (point: { x: number; y: number }, z?: number) => { x: number; y: number },
  z: number,
  fillStyle: string,
) {
  context.fillStyle = fillStyle;
  for (const polygon of polygons) {
    if (polygon.length < 3) continue;
    drawPolygon(context, polygon.map((point) => project(point, z)));
    context.fill();
  }
}

function fillExtrudedLayers(
  context: CanvasRenderingContext2D,
  polygons: Array<Array<{ x: number; y: number }>>,
  project: (point: { x: number; y: number }, z?: number) => { x: number; y: number },
  bottomZ: number,
  topZ: number,
  scale: number,
  fillStyle: string,
) {
  const pixelHeight = Math.abs(topZ - bottomZ) * scale;
  const layers = Math.max(2, Math.min(18, Math.ceil(pixelHeight / 1.25)));
  for (let layer = 0; layer < layers; layer += 1) {
    const z = bottomZ + (topZ - bottomZ) * (layer / layers);
    fillProjectedPolygons(context, polygons, project, z, fillStyle);
  }
}

function previewPadPolygon(pad: ParsedBoard["pads"][number]) {
  if (pad.shape === "circle" || pad.shape === "oval") {
    return ellipsePreviewPolygon(pad.position, pad.size, pad.rotation);
  }
  const halfWidth = pad.size.x / 2;
  const halfHeight = pad.size.y / 2;
  return [
    { x: -halfWidth, y: -halfHeight },
    { x: halfWidth, y: -halfHeight },
    { x: halfWidth, y: halfHeight },
    { x: -halfWidth, y: halfHeight },
  ].map((point) => rotatePreviewPoint(point, pad.position, pad.rotation));
}

function ellipsePreviewPolygon(
  center: { x: number; y: number },
  size: { x: number; y: number },
  rotation: number,
  segments = 32,
) {
  return Array.from({ length: segments }, (_, index) => {
    const angle = (index / segments) * Math.PI * 2;
    return rotatePreviewPoint(
      { x: Math.cos(angle) * size.x / 2, y: Math.sin(angle) * size.y / 2 },
      center,
      rotation,
    );
  });
}

function rotatePreviewPoint(
  point: { x: number; y: number },
  center: { x: number; y: number },
  rotation: number,
) {
  const radians = rotation * Math.PI / 180;
  const cosine = Math.cos(radians);
  const sine = Math.sin(radians);
  return {
    x: center.x + point.x * cosine - point.y * sine,
    y: center.y + point.x * sine + point.y * cosine,
  };
}

function drawConflictMarkers(
  context: CanvasRenderingContext2D,
  conflicts: ClearanceConflict[],
  project: (point: { x: number; y: number }, z?: number) => { x: number; y: number },
  z: number,
  palette: CanvasPalette,
) {
  for (const conflict of conflicts) {
    const point = project(conflict.location, z);
    context.beginPath();
    context.arc(point.x, point.y, 8, 0, Math.PI * 2);
    context.fillStyle = palette.conflictHalo;
    context.fill();
    context.beginPath();
    context.arc(point.x, point.y, 4, 0, Math.PI * 2);
    context.fillStyle = palette.conflict;
    context.fill();
    context.strokeStyle = palette.boardEdgeHighlight;
    context.lineWidth = 1.25;
    context.stroke();
  }
}

function drawReliefLegend(
  context: CanvasRenderingContext2D,
  height: number,
  settings: GeneratorSettings,
  palette: CanvasPalette,
) {
  const label = `Z ×${RELIEF_Z_EXAGGERATION} · BOARD ${settings.board_thickness.toFixed(1)} · TRACE ${settings.trace_height.toFixed(2)} mm`;
  context.save();
  context.font = '650 10px "Fira Code", monospace';
  const boxWidth = context.measureText(label).width + 18;
  const boxHeight = 26;
  const x = 18;
  const y = height - boxHeight - 18;
  context.beginPath();
  context.roundRect(x, y, boxWidth, boxHeight, 6);
  context.fillStyle = palette.reliefBadge;
  context.fill();
  context.strokeStyle = palette.reliefOutline;
  context.lineWidth = 1;
  context.stroke();
  context.fillStyle = palette.reliefBadgeText;
  context.textAlign = "left";
  context.textBaseline = "middle";
  context.fillText(label, x + 9, y + boxHeight / 2 + 0.5);
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

type CanvasPalette = {
  background: string;
  grid: string;
  canvasText: string;
  board: string;
  boardEdge: string;
  boardEdgeHighlight: string;
  boardShadow: string;
  trace: string;
  traceShadow: string;
  traceRelief: string;
  traceHighlight: string;
  hole: string;
  conflict: string;
  conflictHalo: string;
  reliefOutline: string;
  reliefBadge: string;
  reliefBadgeText: string;
  ruler: string;
};

const CANVAS_THEMES: Record<ThemeMode, CanvasPalette> = {
  light: {
    background: "#6f8994", grid: "rgba(244, 239, 231, .18)", canvasText: "rgba(247, 242, 234, .78)",
    board: "#f1eadf", boardEdge: "#456879", boardEdgeHighlight: "#fffaf4", boardShadow: "rgba(38, 57, 67, .28)",
    trace: "#b96862", traceShadow: "rgba(63, 79, 86, .28)", traceRelief: "#7d6261", traceHighlight: "rgba(255, 239, 223, .72)",
    hole: "#355766", conflict: "#a84f4d", conflictHalo: "rgba(168, 79, 77, .3)", reliefOutline: "rgba(255, 244, 235, .45)", reliefBadge: "rgba(43, 62, 71, .78)", reliefBadgeText: "#fffaf4", ruler: "rgba(247, 242, 234, .86)",
  },
  dim: {
    background: "#304752", grid: "rgba(207, 220, 222, .12)", canvasText: "rgba(224, 232, 231, .72)",
    board: "#d8d4cc", boardEdge: "#405e6c", boardEdgeHighlight: "#f2eee7", boardShadow: "rgba(15, 27, 34, .4)",
    trace: "#c77a74", traceShadow: "rgba(20, 31, 37, .4)", traceRelief: "#75575a", traceHighlight: "rgba(255, 226, 210, .62)",
    hole: "#263a45", conflict: "#dd817c", conflictHalo: "rgba(221, 129, 124, .28)", reliefOutline: "rgba(241, 236, 226, .34)", reliefBadge: "rgba(14, 25, 31, .78)", reliefBadgeText: "#f2eee7", ruler: "rgba(221, 230, 229, .78)",
  },
};

function drawGrid(context: CanvasRenderingContext2D, width: number, height: number, palette: CanvasPalette) {
  context.fillStyle = palette.background;
  context.fillRect(0, 0, width, height);
  context.strokeStyle = palette.grid;
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
    vintage: "Adds eased pad teardrops and constant-radius fillets at safe, unbranched corners.",
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

function RecipeStat({ label, value }: { label: string; value: string }) {
  return <div><span>{label}</span><strong>{value}</strong></div>;
}

function cleanName(value: string) {
  return value
    .replace(/\.(kicad_pcb|copperline\.json|json)$/i, "")
    .replace(/[^a-z0-9-_]+/gi, "-")
    .replace(/^-+|-+$/g, "") || "copperline-board";
}

function formatFileSize(bytes: number) {
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(bytes < 10 * 1024 * 1024 ? 1 : 0)} MB`;
}

function traceStyleLabel(value: GeneratorSettings["trace_style"]) {
  if (value === "technical") return "Technical";
  if (value === "soft") return "Soft";
  return "Vintage";
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
