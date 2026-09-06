//! Dependency-light KiCad PCB parser and binary STL generator.
//!
//! The WebAssembly build deliberately uses a raw pointer/length ABI instead of
//! `wasm-bindgen`; see `README.md` for the exact calling convention.

use i_overlay::core::extract::BooleanExtractionBuffer;
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::overlay::FloatOverlay;
use i_overlay::float::scale::FixedScaleFloatOverlay;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Bounds {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Trace {
    pub start: Point,
    pub end: Point,
    pub width: f64,
    pub layer: String,
    #[serde(default)]
    pub net_id: Option<i64>,
    #[serde(default)]
    pub net_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pad {
    pub position: Point,
    pub size: Point,
    pub drill: Option<f64>,
    /// KiCad pad kind: `thru_hole`, `np_thru_hole`, `smd`, or `connect`.
    #[serde(default = "default_pad_type")]
    pub pad_type: String,
    pub shape: String,
    pub rotation: f64,
    pub layers: Vec<String>,
    #[serde(default)]
    pub net_id: Option<i64>,
    #[serde(default)]
    pub net_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Via {
    pub position: Point,
    pub size: f64,
    pub drill: f64,
    pub layers: Vec<String>,
    #[serde(default)]
    pub net_id: Option<i64>,
    #[serde(default)]
    pub net_name: Option<String>,
}

/// Cached KiCad copper-zone fill geometry. `polygons` contains KiCad's
/// already-resolved fill, including thermal reliefs, clearances, and islands;
/// Copperline deliberately does not attempt to refill zone outlines.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CopperZone {
    pub layer: String,
    #[serde(default)]
    pub net_id: Option<i64>,
    #[serde(default)]
    pub net_name: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// `copper`, `teardrop`, or `keepout`.
    pub kind: String,
    #[serde(default)]
    pub polygons: Vec<Vec<Point>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Warning {
    pub code: String,
    pub message: String,
    pub severity: Severity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Stats {
    pub traces: usize,
    pub pads: usize,
    pub vias: usize,
    pub holes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Board {
    pub name: String,
    pub bounds: Bounds,
    pub outline: Vec<Point>,
    pub traces: Vec<Trace>,
    pub pads: Vec<Pad>,
    pub vias: Vec<Via>,
    #[serde(default)]
    pub zones: Vec<CopperZone>,
    pub warnings: Vec<Warning>,
    pub stats: Stats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub board_thickness: f64,
    pub trace_height: f64,
    /// Auto-mode trunk width floor. Preserve mode ignores this value.
    #[serde(alias = "min_trace_width")]
    pub trace_width: f64,
    pub hole_compensation: f64,
    /// Retained for ABI/JSON compatibility. V1 always emits mirrored B.Cu.
    #[serde(default = "default_side")]
    pub side: String,
    #[serde(default = "default_width_mode")]
    pub width_mode: String,
    #[serde(default = "default_trace_style")]
    pub trace_style: String,
    #[serde(default = "default_neckdown_width")]
    pub neckdown_width: f64,
    #[serde(default = "default_taper_length")]
    pub taper_length: f64,
    #[serde(default = "default_corner_radius")]
    pub corner_radius: f64,
    #[serde(default = "default_teardrop_length")]
    pub teardrop_length: f64,
    #[serde(default = "default_teardrop_strength")]
    pub teardrop_strength: f64,
    #[serde(default = "default_trace_clearance")]
    pub trace_clearance: f64,
}

fn default_pad_type() -> String {
    "thru_hole".into()
}
fn default_side() -> String {
    "B.Cu".into()
}
fn default_width_mode() -> String {
    "auto".into()
}
fn default_trace_style() -> String {
    // Preserve pre-style project output when these fields are absent.
    "technical".into()
}
fn default_neckdown_width() -> f64 {
    1.4
}
fn default_taper_length() -> f64 {
    4.0
}
fn default_corner_radius() -> f64 {
    3.0
}
fn default_teardrop_length() -> f64 {
    3.0
}
fn default_teardrop_strength() -> f64 {
    0.75
}
fn default_trace_clearance() -> f64 {
    0.5
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            board_thickness: 1.6,
            trace_height: 0.4,
            trace_width: 2.4,
            hole_compensation: 0.1,
            side: default_side(),
            width_mode: default_width_mode(),
            trace_style: default_trace_style(),
            neckdown_width: default_neckdown_width(),
            taper_length: default_taper_length(),
            corner_radius: default_corner_radius(),
            teardrop_length: default_teardrop_length(),
            teardrop_strength: default_teardrop_strength(),
            trace_clearance: default_trace_clearance(),
        }
    }
}

#[derive(Debug, Clone)]
enum Sexp {
    Atom(String),
    List(Vec<Sexp>),
}

impl Sexp {
    fn atom(&self) -> Option<&str> {
        match self {
            Self::Atom(s) => Some(s),
            _ => None,
        }
    }

    fn list(&self) -> Option<&[Sexp]> {
        match self {
            Self::List(v) => Some(v),
            _ => None,
        }
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            bytes: input.as_bytes(),
            pos: 0,
        }
    }

    fn parse(mut self) -> Result<Sexp, String> {
        self.skip_ws();
        let node = self.node()?;
        self.skip_ws();
        if self.pos != self.bytes.len() {
            return Err(format!("unexpected data at byte {}", self.pos));
        }
        Ok(node)
    }

    fn node(&mut self) -> Result<Sexp, String> {
        self.skip_ws();
        match self.bytes.get(self.pos).copied() {
            Some(b'(') => self.list(),
            Some(b'\"') => self.quoted().map(Sexp::Atom),
            Some(_) => self.atom().map(Sexp::Atom),
            None => Err("unexpected end of file".into()),
        }
    }

    fn list(&mut self) -> Result<Sexp, String> {
        self.pos += 1;
        let mut values = Vec::new();
        loop {
            self.skip_ws();
            match self.bytes.get(self.pos).copied() {
                Some(b')') => {
                    self.pos += 1;
                    return Ok(Sexp::List(values));
                }
                Some(_) => values.push(self.node()?),
                None => return Err("unterminated list".into()),
            }
        }
    }

    fn quoted(&mut self) -> Result<String, String> {
        self.pos += 1;
        let mut out = String::new();
        while let Some(ch) = self.bytes.get(self.pos).copied() {
            self.pos += 1;
            match ch {
                b'\"' => return Ok(out),
                b'\\' => {
                    let escaped = self
                        .bytes
                        .get(self.pos)
                        .copied()
                        .ok_or("unterminated escape")?;
                    self.pos += 1;
                    out.push(match escaped {
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        other => other as char,
                    });
                }
                other => out.push(other as char),
            }
        }
        Err("unterminated quoted string".into())
    }

    fn atom(&mut self) -> Result<String, String> {
        let start = self.pos;
        while let Some(ch) = self.bytes.get(self.pos).copied() {
            if ch.is_ascii_whitespace() || ch == b'(' || ch == b')' {
                break;
            }
            self.pos += 1;
        }
        if self.pos == start {
            return Err(format!("expected atom at byte {}", self.pos));
        }
        String::from_utf8(self.bytes[start..self.pos].to_vec())
            .map_err(|_| "invalid UTF-8 atom".into())
    }

    fn skip_ws(&mut self) {
        loop {
            while self
                .bytes
                .get(self.pos)
                .is_some_and(u8::is_ascii_whitespace)
            {
                self.pos += 1;
            }
            if self.bytes.get(self.pos) == Some(&b';') {
                while self.bytes.get(self.pos).is_some_and(|b| *b != b'\n') {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }
}

fn head(node: &Sexp) -> Option<&str> {
    node.list()?.first()?.atom()
}

fn children<'a>(node: &'a Sexp, tag: &'a str) -> impl Iterator<Item = &'a Sexp> {
    node.list()
        .into_iter()
        .flatten()
        .filter(move |n| head(n) == Some(tag))
}

fn child<'a>(node: &'a Sexp, tag: &'a str) -> Option<&'a Sexp> {
    children(node, tag).next()
}

fn nth_atom(node: &Sexp, index: usize) -> Option<&str> {
    node.list()?.get(index)?.atom()
}

fn number_at(node: &Sexp, index: usize) -> Option<f64> {
    nth_atom(node, index)?.parse().ok()
}

fn point_node(node: &Sexp) -> Option<Point> {
    Some(Point {
        x: number_at(node, 1)?,
        y: number_at(node, 2)?,
    })
}

fn warning(code: &str, message: impl Into<String>) -> Warning {
    Warning {
        code: code.into(),
        message: message.into(),
        severity: Severity::Warning,
    }
}

fn error_warning(code: &str, message: impl Into<String>) -> Warning {
    Warning {
        code: code.into(),
        message: message.into(),
        severity: Severity::Error,
    }
}

fn zone_targets_layer(node: &Sexp, wanted: &str) -> bool {
    child(node, "layer")
        .and_then(|layer| nth_atom(layer, 1))
        .is_some_and(|layer| layer == wanted || layer == "*.Cu")
        || child(node, "layers")
            .map(atoms_after_head)
            .is_some_and(|layers| {
                layers
                    .iter()
                    .any(|layer| layer == wanted || layer == "*.Cu")
            })
}

fn parse_zone(
    node: &Sexp,
    net_names: &HashMap<i64, String>,
) -> Option<(CopperZone, Option<Warning>)> {
    let fallback_layer = child(node, "layer")
        .and_then(|layer| nth_atom(layer, 1))
        .unwrap_or("");
    let keepout = child(node, "keepout").is_some();
    let name = child(node, "name")
        .and_then(|value| nth_atom(value, 1))
        .map(str::to_string);
    let structural_teardrop =
        child(node, "attr").is_some_and(|attributes| child(attributes, "teardrop").is_some());
    let teardrop = structural_teardrop
        || name
            .as_deref()
            .is_some_and(|value| value.starts_with("$teardrop_"));
    let net_id = child(node, "net")
        .and_then(|value| nth_atom(value, 1))
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|id| *id > 0);
    let net_name = child(node, "net_name")
        .and_then(|value| nth_atom(value, 1))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| net_id.and_then(|id| net_names.get(&id).cloned()));

    let mut polygons = Vec::new();
    for filled in children(node, "filled_polygon") {
        let layer = child(filled, "layer")
            .and_then(|value| nth_atom(value, 1))
            .unwrap_or(fallback_layer);
        if layer != "B.Cu" && layer != "*.Cu" {
            continue;
        }
        if let Some(points) = child(filled, "pts") {
            let polygon: Vec<_> = children(points, "xy").filter_map(point_node).collect();
            if polygon.len() >= 3 && polygon_area(&polygon).abs() > 1e-10 {
                polygons.push(polygon);
            }
        }
    }

    let targets_back = zone_targets_layer(node, "B.Cu") || !polygons.is_empty();
    if !targets_back {
        return None;
    }
    let kind = if keepout {
        "keepout"
    } else if teardrop {
        "teardrop"
    } else {
        "copper"
    };
    let issue = if !keepout && polygons.is_empty() {
        Some(error_warning(
            "UNFILLED_BCU_ZONE",
            "a B.Cu zone has no cached filled polygons; refill zones in KiCad and save the board before importing",
        ))
    } else {
        None
    };
    Some((
        CopperZone {
            layer: "B.Cu".into(),
            net_id,
            net_name,
            name,
            kind: kind.into(),
            polygons,
        },
        issue,
    ))
}

fn polygon_area(points: &[Point]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
        .map(|(a, b)| a.x * b.y - b.x * a.y)
        .sum::<f64>()
        * 0.5
}

fn close_enough(a: Point, b: Point) -> bool {
    (a.x - b.x).abs() < 1e-4 && (a.y - b.y).abs() < 1e-4
}

fn chain_lines(mut lines: Vec<(Point, Point)>) -> Vec<Vec<Point>> {
    let mut loops = Vec::new();
    while let Some((start, next)) = lines.pop() {
        let mut points = vec![start, next];
        let mut closed = false;
        loop {
            let end = *points.last().unwrap();
            if close_enough(end, start) {
                points.pop();
                closed = true;
                break;
            }
            let found = lines
                .iter()
                .position(|(a, b)| close_enough(*a, end) || close_enough(*b, end));
            let Some(index) = found else {
                break;
            };
            let (a, b) = lines.swap_remove(index);
            points.push(if close_enough(a, end) { b } else { a });
        }
        if closed && points.len() >= 3 {
            loops.push(points);
        }
    }
    loops
}

fn tessellate_arc(start: Point, mid: Point, end: Point) -> Option<Vec<(Point, Point)>> {
    let determinant =
        2.0 * (start.x * (mid.y - end.y) + mid.x * (end.y - start.y) + end.x * (start.y - mid.y));
    if determinant.abs() < 1e-10 {
        return None;
    }
    let start_norm = start.x * start.x + start.y * start.y;
    let mid_norm = mid.x * mid.x + mid.y * mid.y;
    let end_norm = end.x * end.x + end.y * end.y;
    let center = Point {
        x: (start_norm * (mid.y - end.y)
            + mid_norm * (end.y - start.y)
            + end_norm * (start.y - mid.y))
            / determinant,
        y: (start_norm * (end.x - mid.x)
            + mid_norm * (start.x - end.x)
            + end_norm * (mid.x - start.x))
            / determinant,
    };
    let radius = distance(start, center);
    if !radius.is_finite() || radius <= 1e-8 {
        return None;
    }
    let start_angle = (start.y - center.y).atan2(start.x - center.x);
    let mid_angle = (mid.y - center.y).atan2(mid.x - center.x);
    let end_angle = (end.y - center.y).atan2(end.x - center.x);
    let ccw_sweep = (end_angle - start_angle).rem_euclid(2.0 * PI);
    let mid_sweep = (mid_angle - start_angle).rem_euclid(2.0 * PI);
    let sweep = if mid_sweep <= ccw_sweep + 1e-8 {
        ccw_sweep
    } else {
        ccw_sweep - 2.0 * PI
    };
    if sweep.abs() < 1e-8 {
        return None;
    }
    // At most 0.03 mm chord error, with practical angular bounds so both tiny
    // and very large arcs remain deterministic without excessive polygons.
    let chord_angle =
        (2.0 * (1.0 - 0.03 / radius).clamp(-1.0, 1.0).acos()).clamp(PI / 72.0, PI / 12.0);
    let steps = ((sweep.abs() / chord_angle).ceil() as usize).clamp(2, 144);
    let points: Vec<_> = (0..=steps)
        .map(|index| {
            if index == 0 {
                start
            } else if index == steps {
                end
            } else {
                let angle = start_angle + sweep * index as f64 / steps as f64;
                Point {
                    x: center.x + radius * angle.cos(),
                    y: center.y + radius * angle.sin(),
                }
            }
        })
        .collect();
    Some(points.windows(2).map(|pair| (pair[0], pair[1])).collect())
}

/// Parse a KiCad 6+ `.kicad_pcb` document into the UI's stable JSON model.
pub fn parse_kicad(input: &str) -> Result<Board, String> {
    let root = Parser::new(input).parse()?;
    if head(&root) != Some("kicad_pcb") {
        return Err("expected a kicad_pcb root expression".into());
    }

    let mut outlines = Vec::<Vec<Point>>::new();
    let mut edge_lines = Vec::<(Point, Point)>::new();
    let mut traces = Vec::new();
    let mut pads = Vec::new();
    let mut vias = Vec::new();
    let mut zones = Vec::new();
    let mut warnings = Vec::new();
    let mut name = "Untitled PCB".to_string();
    let net_names: HashMap<i64, String> = children(&root, "net")
        .filter_map(|net| {
            Some((
                nth_atom(net, 1)?.parse().ok()?,
                nth_atom(net, 2)?.to_string(),
            ))
        })
        .collect();

    if let Some(title_block) = child(&root, "title_block") {
        if let Some(title) = child(title_block, "title").and_then(|n| nth_atom(n, 1)) {
            if !title.is_empty() {
                name = title.to_string();
            }
        }
    }

    for node in root.list().into_iter().flatten() {
        match head(node) {
            Some("gr_rect") if layer_is(node, "Edge.Cuts") => {
                if let (Some(a), Some(b)) = (
                    child(node, "start").and_then(point_node),
                    child(node, "end").and_then(point_node),
                ) {
                    outlines.push(vec![
                        a,
                        Point { x: b.x, y: a.y },
                        b,
                        Point { x: a.x, y: b.y },
                    ]);
                }
            }
            Some("gr_poly") if layer_is(node, "Edge.Cuts") => {
                if let Some(pts) = child(node, "pts") {
                    let polygon: Vec<_> = children(pts, "xy").filter_map(point_node).collect();
                    if polygon.len() >= 3 {
                        outlines.push(polygon);
                    }
                }
            }
            Some("gr_line") if layer_is(node, "Edge.Cuts") => {
                if let (Some(a), Some(b)) = (
                    child(node, "start").and_then(point_node),
                    child(node, "end").and_then(point_node),
                ) {
                    edge_lines.push((a, b));
                }
            }
            Some("gr_arc") if layer_is(node, "Edge.Cuts") => warnings.push(warning(
                "UNSUPPORTED_EDGE_ARC",
                "Edge.Cuts arcs are not yet included in the generated outline",
            )),
            Some("segment") => {
                let layer = child(node, "layer")
                    .and_then(|n| nth_atom(n, 1))
                    .unwrap_or("");
                if matches!(layer, "F.Cu" | "B.Cu") {
                    if let (Some(start), Some(end), Some(width)) = (
                        child(node, "start").and_then(point_node),
                        child(node, "end").and_then(point_node),
                        child(node, "width").and_then(|n| number_at(n, 1)),
                    ) {
                        let net_id = child(node, "net")
                            .and_then(|n| nth_atom(n, 1))
                            .and_then(|value| value.parse::<i64>().ok())
                            .filter(|id| *id > 0);
                        traces.push(Trace {
                            start,
                            end,
                            width,
                            layer: layer.into(),
                            net_name: net_id.and_then(|id| net_names.get(&id).cloned()),
                            net_id,
                        });
                    }
                }
            }
            Some("arc") => {
                let layer = child(node, "layer")
                    .and_then(|n| nth_atom(n, 1))
                    .unwrap_or("");
                if matches!(layer, "F.Cu" | "B.Cu") {
                    let geometry = (
                        child(node, "start").and_then(point_node),
                        child(node, "mid").and_then(point_node),
                        child(node, "end").and_then(point_node),
                        child(node, "width").and_then(|n| number_at(n, 1)),
                    );
                    if let (Some(start), Some(mid), Some(end), Some(width)) = geometry {
                        let net_id = child(node, "net")
                            .and_then(|n| nth_atom(n, 1))
                            .and_then(|value| value.parse::<i64>().ok())
                            .filter(|id| *id > 0);
                        if let Some(segments) = tessellate_arc(start, mid, end) {
                            traces.extend(segments.into_iter().map(|(start, end)| Trace {
                                start,
                                end,
                                width,
                                layer: layer.into(),
                                net_name: net_id.and_then(|id| net_names.get(&id).cloned()),
                                net_id,
                            }));
                        } else {
                            warnings.push(warning(
                                "INVALID_COPPER_ARC",
                                "a routed copper arc was degenerate and could not be included",
                            ));
                        }
                    }
                }
            }
            Some("via") => {
                let position = child(node, "at").and_then(point_node);
                let size = child(node, "size").and_then(|n| number_at(n, 1));
                let drill = child(node, "drill").and_then(|n| number_at(n, 1));
                if let (Some(position), Some(size), Some(drill)) = (position, size, drill) {
                    let layers = child(node, "layers")
                        .map(atoms_after_head)
                        .unwrap_or_else(|| vec!["F.Cu".into(), "B.Cu".into()]);
                    let net_id = child(node, "net")
                        .and_then(|n| nth_atom(n, 1))
                        .and_then(|value| value.parse::<i64>().ok())
                        .filter(|id| *id > 0);
                    vias.push(Via {
                        position,
                        size,
                        drill,
                        layers,
                        net_name: net_id.and_then(|id| net_names.get(&id).cloned()),
                        net_id,
                    });
                }
            }
            Some("zone") => {
                if let Some((zone, issue)) = parse_zone(node, &net_names) {
                    if let Some(issue) = issue {
                        warnings.push(issue);
                    }
                    zones.push(zone);
                }
            }
            Some("footprint") => parse_footprint(node, &mut pads, &mut warnings, &net_names),
            _ => {}
        }
    }

    outlines.extend(chain_lines(edge_lines));
    if outlines.is_empty() {
        return Err(
            "no closed Edge.Cuts outline made from gr_rect, gr_poly, or gr_line was found".into(),
        );
    }
    outlines.sort_by(|a, b| polygon_area(b).abs().total_cmp(&polygon_area(a).abs()));
    if outlines.len() > 1 {
        warnings.push(warning(
            "MULTIPLE_EDGE_LOOPS",
            format!(
                "found {} Edge.Cuts loops; only the largest is used as the board outline",
                outlines.len()
            ),
        ));
    }
    let mut outline = outlines.remove(0);
    if polygon_area(&outline) < 0.0 {
        outline.reverse();
    }
    let bounds = bounds(&outline).ok_or("outline has no points")?;
    let holes = pads
        .iter()
        .filter(|p| p.drill.is_some_and(|d| d > 0.0))
        .count()
        + vias.iter().filter(|v| v.drill > 0.0).count();
    let stats = Stats {
        traces: traces.len(),
        pads: pads.len(),
        vias: vias.len(),
        holes,
    };
    Ok(Board {
        name,
        bounds,
        outline,
        traces,
        pads,
        vias,
        zones,
        warnings,
        stats,
    })
}

fn layer_is(node: &Sexp, wanted: &str) -> bool {
    child(node, "layer").and_then(|n| nth_atom(n, 1)) == Some(wanted)
}

fn atoms_after_head(node: &Sexp) -> Vec<String> {
    node.list()
        .into_iter()
        .flatten()
        .skip(1)
        .filter_map(Sexp::atom)
        .map(str::to_string)
        .collect()
}

fn parse_footprint(
    node: &Sexp,
    pads: &mut Vec<Pad>,
    warnings: &mut Vec<Warning>,
    net_names: &HashMap<i64, String>,
) {
    let fp_at = child(node, "at");
    let origin = fp_at
        .and_then(point_node)
        .unwrap_or(Point { x: 0.0, y: 0.0 });
    let fp_rotation = fp_at.and_then(|n| number_at(n, 3)).unwrap_or(0.0);
    let fp_layer = child(node, "layer")
        .and_then(|n| nth_atom(n, 1))
        .unwrap_or("F.Cu");
    if fp_layer == "B.Cu" {
        warnings.push(warning(
            "BOTTOM_FOOTPRINT_TRANSFORM_APPROXIMATED",
            "bottom-footprint pad rotation is parsed, but KiCad's full mirrored local transform is not yet represented",
        ));
    }
    // KiCad board Y coordinates grow downward. Its positive footprint angle is
    // therefore clockwise in conventional Cartesian coordinates.
    let angle = fp_rotation.to_radians();
    let (sin, cos) = angle.sin_cos();
    for pad_node in children(node, "pad") {
        let pad_type = nth_atom(pad_node, 2).unwrap_or("unknown").to_string();
        let shape = nth_atom(pad_node, 3).unwrap_or("circle").to_string();
        if pad_type == "smd" {
            warnings.push(warning(
                "UNSUPPORTED_SMD_PAD",
                "SMD-only pad retained for preview but omitted from this through-hole-only printable process",
            ));
        }
        if shape == "custom" {
            warnings.push(warning(
                "UNSUPPORTED_CUSTOM_PAD",
                "custom pad primitives are retained as metadata but omitted from raised geometry",
            ));
        }
        let local_at = child(pad_node, "at");
        let local = local_at
            .and_then(point_node)
            .unwrap_or(Point { x: 0.0, y: 0.0 });
        let position = Point {
            x: origin.x + local.x * cos + local.y * sin,
            y: origin.y + local.y * cos - local.x * sin,
        };
        let size_node = child(pad_node, "size");
        let size = Point {
            x: size_node.and_then(|n| number_at(n, 1)).unwrap_or(1.0),
            y: size_node.and_then(|n| number_at(n, 2)).unwrap_or(1.0),
        };
        let drill = child(pad_node, "drill").and_then(drill_diameter);
        let rotation = fp_rotation + local_at.and_then(|n| number_at(n, 3)).unwrap_or(0.0);
        let layers = child(pad_node, "layers")
            .map(atoms_after_head)
            .unwrap_or_else(|| vec![fp_layer.into()]);
        let net_node = child(pad_node, "net");
        let net_id = net_node
            .and_then(|n| nth_atom(n, 1))
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|id| *id > 0);
        let net_name = net_node
            .and_then(|n| nth_atom(n, 2))
            .map(str::to_string)
            .or_else(|| net_id.and_then(|id| net_names.get(&id).cloned()));
        pads.push(Pad {
            position,
            size,
            drill,
            pad_type,
            shape,
            rotation,
            layers,
            net_id,
            net_name,
        });
    }
}

fn drill_diameter(node: &Sexp) -> Option<f64> {
    node.list()?
        .iter()
        .skip(1)
        .filter_map(Sexp::atom)
        .filter_map(|s| s.parse::<f64>().ok())
        .next()
}

fn bounds(points: &[Point]) -> Option<Bounds> {
    let first = *points.first()?;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (first.x, first.y, first.x, first.y);
    for p in points.iter().skip(1) {
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x);
        max_y = max_y.max(p.y);
    }
    Some(Bounds {
        min_x,
        min_y,
        max_x,
        max_y,
        width: max_x - min_x,
        height: max_y - min_y,
    })
}

#[derive(Clone, Copy)]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}

type GridPoint = [i64; 2];

fn grid_point(point: &[f64; 2]) -> GridPoint {
    [
        (point[0] * OVERLAY_SCALE).round() as i64,
        (point[1] * OVERLAY_SCALE).round() as i64,
    ]
}

/// Split every partition edge at nearby endpoints from every partition member.
/// Boolean intersections can round a shared vertex one fixed-grid unit away
/// from its source edge, so the on-segment check admits that quantization error
/// and keeps the rounded endpoint as a tiny kink. Earcut and the wall builder
/// then receive identical atomic boundary segments.
fn node_partition(parts: &[&PolyShapes]) -> Vec<PolyShapes> {
    let mut nodes = Vec::<GridPoint>::new();
    for contour in parts.iter().flat_map(|shapes| shapes.iter().flatten()) {
        nodes.extend(contour.iter().map(grid_point));
    }
    nodes.sort();
    nodes.dedup();
    let mut nodes_by_x = BTreeMap::<i64, Vec<GridPoint>>::new();
    let mut nodes_by_y = BTreeMap::<i64, Vec<GridPoint>>::new();
    for point in nodes {
        nodes_by_x.entry(point[0]).or_default().push(point);
        nodes_by_y.entry(point[1]).or_default().push(point);
    }

    parts
        .iter()
        .map(|shapes| {
            shapes
                .iter()
                .map(|shape| {
                    shape
                        .iter()
                        .map(|contour| {
                            let mut result = Vec::new();
                            for (a, b) in contour
                                .iter()
                                .zip(contour.iter().cycle().skip(1))
                                .take(contour.len())
                            {
                                let (a, b) = (grid_point(a), grid_point(b));
                                if a == b {
                                    continue;
                                }
                                let (dx, dy) =
                                    (b[0] as i128 - a[0] as i128, b[1] as i128 - a[1] as i128);
                                let length_squared = dx * dx + dy * dy;
                                let parameter = |point: GridPoint| {
                                    dx * point[0] as i128 + dy * point[1] as i128
                                };
                                let (start, end) = (parameter(a), parameter(b));
                                let mut splits = Vec::new();
                                let (index, low, high) = if dx.abs() >= dy.abs() {
                                    (
                                        &nodes_by_x,
                                        a[0].min(b[0]).saturating_sub(1),
                                        a[0].max(b[0]).saturating_add(1),
                                    )
                                } else {
                                    (
                                        &nodes_by_y,
                                        a[1].min(b[1]).saturating_sub(1),
                                        a[1].max(b[1]).saturating_add(1),
                                    )
                                };
                                for candidates in index.range(low..=high).map(|(_, points)| points)
                                {
                                    for &point in candidates {
                                        let (ap_x, ap_y) = (
                                            point[0] as i128 - a[0] as i128,
                                            point[1] as i128 - a[1] as i128,
                                        );
                                        let projection = dx * ap_x + dy * ap_y;
                                        if !(0..=length_squared).contains(&projection) {
                                            continue;
                                        }
                                        let cross = dx * ap_y - dy * ap_x;
                                        // At most one 1e-5 mm grid cell away from
                                        // the segment after independent rounding.
                                        if cross * cross <= length_squared {
                                            splits.push(point);
                                        }
                                    }
                                }
                                splits.sort_by_key(|point| parameter(*point));
                                splits.dedup();
                                if start > end {
                                    splits.reverse();
                                }
                                result.extend(splits.into_iter().take_while(|point| *point != b));
                            }
                            result
                                .into_iter()
                                .map(|point| {
                                    [
                                        point[0] as f64 / OVERLAY_SCALE,
                                        point[1] as f64 / OVERLAY_SCALE,
                                    ]
                                })
                                .collect()
                        })
                        .collect()
                })
                .collect()
        })
        .collect()
}

struct Mesh {
    triangles: Vec<[Vec3; 3]>,
}

impl Mesh {
    fn new() -> Self {
        Self {
            triangles: Vec::new(),
        }
    }
    fn triangle(&mut self, a: Vec3, b: Vec3, c: Vec3) {
        self.triangles.push([a, b, c]);
    }

    fn horizontal(&mut self, shapes: &PolyShapes, z: f64, upward: bool) -> Result<(), String> {
        for shape in shapes {
            if shape.first().is_none_or(|outer| outer.len() < 3) {
                continue;
            }
            self.horizontal_shape(shape, z, upward)?;
        }
        Ok(())
    }

    fn horizontal_shape(&mut self, rings: &PolyShape, z: f64, upward: bool) -> Result<(), String> {
        let mut coords = Vec::new();
        let mut hole_indices = Vec::new();
        for (index, ring) in rings.iter().enumerate() {
            if index > 0 {
                hole_indices.push(coords.len() / 2);
            }
            for p in ring {
                coords.extend([p[0], p[1]]);
            }
        }
        let indices = earcutr::earcut(&coords, &hole_indices, 2)
            .map_err(|e| format!("triangulation failed: {e}"))?;
        let vertices: Vec<Point> = rings
            .iter()
            .flatten()
            .map(|p| Point { x: p[0], y: p[1] })
            .collect();
        for tri in indices.chunks_exact(3) {
            let (a, b, c) = (vertices[tri[0]], vertices[tri[1]], vertices[tri[2]]);
            let ccw = orient(a, b, c) >= 0.0;
            if ccw == upward {
                self.triangle(v3(a, z), v3(b, z), v3(c, z));
            } else {
                self.triangle(v3(c, z), v3(b, z), v3(a, z));
            }
        }
        Ok(())
    }

    fn vertical(&mut self, shapes: &PolyShapes, z0: f64, z1: f64) {
        for contour in shapes.iter().flatten() {
            for (a, b) in contour
                .iter()
                .zip(contour.iter().cycle().skip(1))
                .take(contour.len())
            {
                let a = Point { x: a[0], y: a[1] };
                let b = Point { x: b[0], y: b[1] };
                // i_overlay emits CCW outers and CW holes. This winding puts
                // each wall normal on the material's exterior side.
                self.triangle(v3(a, z0), v3(b, z0), v3(b, z1));
                self.triangle(v3(a, z0), v3(b, z1), v3(a, z1));
            }
        }
    }

    fn validate_closed_manifold(&self) -> Result<(), String> {
        type VertexKey = [i64; 3];
        let key = |vertex: Vec3| {
            [vertex.x, vertex.y, vertex.z]
                .map(|coordinate| (f64::from(coordinate) * OVERLAY_SCALE).round() as i64)
        };
        let mut directed_edges = HashMap::<(VertexKey, VertexKey), usize>::new();
        let mut undirected_edges = HashMap::<(VertexKey, VertexKey), usize>::new();
        let mut faces = HashSet::<[VertexKey; 3]>::new();
        for triangle in &self.triangles {
            if !triangle
                .iter()
                .all(|vertex| vertex.x.is_finite() && vertex.y.is_finite() && vertex.z.is_finite())
            {
                return Err("generated mesh contains a non-finite vertex".into());
            }
            let keys = triangle.map(key);
            if keys[0] == keys[1] || keys[1] == keys[2] || keys[2] == keys[0] {
                return Err("generated mesh contains a collapsed triangle".into());
            }
            let ab = [
                keys[1][0] as i128 - keys[0][0] as i128,
                keys[1][1] as i128 - keys[0][1] as i128,
                keys[1][2] as i128 - keys[0][2] as i128,
            ];
            let ac = [
                keys[2][0] as i128 - keys[0][0] as i128,
                keys[2][1] as i128 - keys[0][1] as i128,
                keys[2][2] as i128 - keys[0][2] as i128,
            ];
            let cross = [
                ab[1] * ac[2] - ab[2] * ac[1],
                ab[2] * ac[0] - ab[0] * ac[2],
                ab[0] * ac[1] - ab[1] * ac[0],
            ];
            if cross == [0, 0, 0] {
                return Err("generated mesh contains a degenerate triangle".into());
            }
            let mut face = keys;
            face.sort();
            if !faces.insert(face) {
                return Err("generated mesh contains a duplicate triangle".into());
            }
            for (a, b) in [(keys[0], keys[1]), (keys[1], keys[2]), (keys[2], keys[0])] {
                *directed_edges.entry((a, b)).or_default() += 1;
                let edge = if a <= b { (a, b) } else { (b, a) };
                *undirected_edges.entry(edge).or_default() += 1;
            }
        }
        for ((a, b), count) in undirected_edges {
            let forward = directed_edges.get(&(a, b)).copied().unwrap_or(0);
            let reverse = directed_edges.get(&(b, a)).copied().unwrap_or(0);
            if count != 2 || forward != 1 || reverse != 1 {
                let coordinate = |value: i64| value as f64 / OVERLAY_SCALE;
                return Err(format!(
                    "generated mesh is not closed and consistently oriented at 1e-5 mm precision near edge ({:.5}, {:.5}, {:.5}) to ({:.5}, {:.5}, {:.5}); {count} incident faces ({forward} forward, {reverse} reverse)",
                    coordinate(a[0]),
                    coordinate(a[1]),
                    coordinate(a[2]),
                    coordinate(b[0]),
                    coordinate(b[1]),
                    coordinate(b[2]),
                ));
            }
        }
        Ok(())
    }

    fn binary_stl(&self) -> Vec<u8> {
        let mut bytes = vec![0u8; 84 + self.triangles.len() * 50];
        let title = b"pcb-core raised-trace board";
        bytes[..title.len()].copy_from_slice(title);
        bytes[80..84].copy_from_slice(&(self.triangles.len() as u32).to_le_bytes());
        let mut cursor = 84;
        for tri in &self.triangles {
            let n = normal(tri[0], tri[1], tri[2]);
            for value in [
                n.x, n.y, n.z, tri[0].x, tri[0].y, tri[0].z, tri[1].x, tri[1].y, tri[1].z,
                tri[2].x, tri[2].y, tri[2].z,
            ] {
                bytes[cursor..cursor + 4].copy_from_slice(&value.to_le_bytes());
                cursor += 4;
            }
            cursor += 2;
        }
        bytes
    }
}

fn orient(a: Point, b: Point, c: Point) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}
fn v3(p: Point, z: f64) -> Vec3 {
    let canonical = |value: f64| {
        let value = (value * OVERLAY_SCALE).round() / OVERLAY_SCALE;
        if value.abs() < 0.5 / OVERLAY_SCALE {
            0.0
        } else {
            value as f32
        }
    };
    Vec3 {
        x: canonical(p.x),
        y: canonical(p.y),
        z: canonical(z),
    }
}
fn normal(a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let (ux, uy, uz) = (b.x - a.x, b.y - a.y, b.z - a.z);
    let (vx, vy, vz) = (c.x - a.x, c.y - a.y, c.z - a.z);
    let (x, y, z) = (uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx);
    let length = (x * x + y * y + z * z).sqrt();
    if length > 0.0 {
        Vec3 {
            x: x / length,
            y: y / length,
            z: z / length,
        }
    } else {
        Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }
    }
}

fn circle(center: Point, radius: f64, segments: usize) -> Vec<Point> {
    circle_with_phase(center, radius, segments, 0.0)
}

fn circle_with_phase(center: Point, radius: f64, segments: usize, phase: f64) -> Vec<Point> {
    (0..segments)
        .map(|i| {
            let a = 2.0 * PI * i as f64 / segments as f64 + phase;
            Point {
                x: center.x + radius * a.cos(),
                y: center.y + radius * a.sin(),
            }
        })
        .collect()
}

fn capsule(start: Point, end: Point, width: f64, segments: usize) -> Vec<Point> {
    let theta = (end.y - start.y).atan2(end.x - start.x);
    if close_enough(start, end) {
        return circle(start, width * 0.5, segments * 2);
    }
    let half = segments.max(4) / 2;
    let mut points = Vec::with_capacity(half * 2 + 2);
    for i in 0..=half {
        let a = theta - PI / 2.0 + PI * i as f64 / half as f64;
        points.push(Point {
            x: end.x + width * 0.5 * a.cos(),
            y: end.y + width * 0.5 * a.sin(),
        });
    }
    for i in 0..=half {
        let a = theta + PI / 2.0 + PI * i as f64 / half as f64;
        points.push(Point {
            x: start.x + width * 0.5 * a.cos(),
            y: start.y + width * 0.5 * a.sin(),
        });
    }
    points
}

fn ray_exit_distance(origin: Point, direction: Point, polygon: &[Point]) -> Option<f64> {
    let cross = |a: Point, b: Point| a.x * b.y - a.y * b.x;
    polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .take(polygon.len())
        .filter_map(|(&a, &b)| {
            let edge = Point {
                x: b.x - a.x,
                y: b.y - a.y,
            };
            let delta = Point {
                x: a.x - origin.x,
                y: a.y - origin.y,
            };
            let denominator = cross(direction, edge);
            if denominator.abs() < 1e-10 {
                return None;
            }
            let t = cross(delta, edge) / denominator;
            let u = cross(delta, direction) / denominator;
            (t >= -1e-8 && (-1e-8..=1.0 + 1e-8).contains(&u)).then_some(t.max(0.0))
        })
        .filter(|distance| *distance > 1e-8)
        .min_by(f64::total_cmp)
}

fn ray_exit_hit(origin: Point, direction: Point, polygon: &[Point]) -> Option<(Point, Point)> {
    let cross = |a: Point, b: Point| a.x * b.y - a.y * b.x;
    polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .take(polygon.len())
        .filter_map(|(&a, &b)| {
            let edge = Point {
                x: b.x - a.x,
                y: b.y - a.y,
            };
            let delta = Point {
                x: a.x - origin.x,
                y: a.y - origin.y,
            };
            let denominator = cross(direction, edge);
            if denominator.abs() < 1e-10 {
                return None;
            }
            let t = cross(delta, edge) / denominator;
            let u = cross(delta, direction) / denominator;
            if t <= 1e-8 || !(-1e-8..=1.0 + 1e-8).contains(&u) {
                return None;
            }
            let tangent = normalized(edge);
            Some((
                t,
                Point {
                    x: origin.x + direction.x * t,
                    y: origin.y + direction.y * t,
                },
                tangent,
            ))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, point, tangent)| (point, tangent))
}

fn cubic_bezier(start: Point, control_1: Point, control_2: Point, end: Point, t: f64) -> Point {
    let inverse = 1.0 - t;
    Point {
        x: inverse.powi(3) * start.x
            + 3.0 * inverse * inverse * t * control_1.x
            + 3.0 * inverse * t * t * control_2.x
            + t.powi(3) * end.x,
        y: inverse.powi(3) * start.y
            + 3.0 * inverse * inverse * t * control_1.y
            + 3.0 * inverse * t * t * control_2.y
            + t.powi(3) * end.y,
    }
}

fn smootherstep(value: f64) -> f64 {
    let t = value.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn smootherstep_derivative(value: f64) -> f64 {
    if !(0.0..1.0).contains(&value) {
        return 0.0;
    }
    30.0 * value * value * (value - 1.0) * (value - 1.0)
}

fn width_envelope(
    distance: f64,
    exit: f64,
    taper: f64,
    neck: f64,
    trunk: f64,
    smooth: bool,
) -> f64 {
    if distance <= exit {
        neck
    } else if taper <= 1e-9 {
        trunk
    } else {
        let progress = ((distance - exit) / taper).clamp(0.0, 1.0);
        neck + (trunk - neck)
            * if smooth {
                smootherstep(progress)
            } else {
                progress
            }
    }
}

#[derive(Clone, Copy)]
struct VariableTraceProfile<'a> {
    trunk: f64,
    neck: f64,
    taper: f64,
    start_exit: Option<f64>,
    end_exit: Option<f64>,
    style: &'a str,
    teardrop_length: f64,
    start_shoulder: Option<f64>,
    end_shoulder: Option<f64>,
}

fn variable_trace_width_and_slope(
    length: f64,
    profile: VariableTraceProfile<'_>,
    distance: f64,
) -> (f64, f64) {
    let side_width = |travelled: f64, exit: f64, shoulder: Option<f64>| {
        if profile.style == "vintage" {
            if let Some(shoulder) = shoulder {
                let after_exit = (travelled - exit).max(0.0);
                if after_exit <= profile.teardrop_length {
                    let progress = after_exit / profile.teardrop_length.max(1e-9);
                    return (
                        shoulder + (profile.neck - shoulder) * smootherstep(progress),
                        (profile.neck - shoulder) * smootherstep_derivative(progress)
                            / profile.teardrop_length.max(1e-9),
                    );
                }
                let progress = ((after_exit - profile.teardrop_length) / profile.taper.max(1e-9))
                    .clamp(0.0, 1.0);
                return (
                    profile.neck + (profile.trunk - profile.neck) * smootherstep(progress),
                    (profile.trunk - profile.neck) * smootherstep_derivative(progress)
                        / profile.taper.max(1e-9),
                );
            }
        }
        let progress = ((travelled - exit) / profile.taper.max(1e-9)).clamp(0.0, 1.0);
        let smooth = profile.style != "technical";
        (
            width_envelope(
                travelled,
                exit,
                profile.taper,
                profile.neck,
                profile.trunk,
                smooth,
            ),
            if progress > 0.0 && progress < 1.0 {
                (profile.trunk - profile.neck)
                    * if smooth {
                        smootherstep_derivative(progress)
                    } else {
                        1.0
                    }
                    / profile.taper.max(1e-9)
            } else {
                0.0
            },
        )
    };
    let candidates = [
        profile
            .start_exit
            .map(|exit| side_width(distance, exit, profile.start_shoulder)),
        profile.end_exit.map(|exit| {
            let (width, slope) = side_width(length - distance, exit, profile.end_shoulder);
            (width, -slope)
        }),
    ];
    let width_range = profile.trunk - profile.neck;
    if profile.style != "technical"
        && profile.start_exit.is_some()
        && profile.end_exit.is_some()
        && width_range > 1e-9
    {
        let permission = candidates.map(|candidate| {
            candidate.map(|(width, _)| ((width - profile.neck) / width_range).clamp(0.0, 1.0))
        });
        let first_permission = permission[0].unwrap_or(1.0);
        let second_permission = permission[1].unwrap_or(1.0);
        let first_slope = candidates[0].map(|(_, slope)| slope).unwrap_or(0.0);
        let second_slope = candidates[1].map(|(_, slope)| slope).unwrap_or(0.0);
        return (
            profile.neck + width_range * first_permission * second_permission,
            first_slope * second_permission + second_slope * first_permission,
        );
    }
    let mut selected = (profile.trunk, 0.0);
    for candidate in candidates.into_iter().flatten() {
        let selected_change = (selected.0 - profile.trunk).abs();
        let candidate_change = (candidate.0 - profile.trunk).abs();
        if candidate_change > selected_change + 1e-9
            || ((candidate_change - selected_change).abs() <= 1e-9 && candidate.0 < selected.0)
        {
            selected = candidate;
        }
    }
    selected
}

fn variable_trace_width(length: f64, profile: VariableTraceProfile<'_>, distance: f64) -> f64 {
    variable_trace_width_and_slope(length, profile, distance).0
}

fn variable_trace_polygon(
    start: Point,
    end: Point,
    profile: VariableTraceProfile<'_>,
) -> Vec<Point> {
    let VariableTraceProfile {
        trunk,
        neck,
        taper,
        start_exit,
        end_exit,
        style,
        teardrop_length,
        start_shoulder,
        end_shoulder,
    } = profile;
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length = (dx * dx + dy * dy).sqrt();
    if length <= 1e-9 {
        return circle(start, trunk * 0.5, 24);
    }
    let direction = Point {
        x: dx / length,
        y: dy / length,
    };
    let normal = Point {
        x: -direction.y,
        y: direction.x,
    };
    let mut positions = vec![0.0, length];
    if let Some(exit) = start_exit {
        positions.extend([exit.min(length), (exit + taper).min(length)]);
        if start_shoulder.is_some() {
            positions.push((exit + teardrop_length).min(length));
            positions.push((exit + teardrop_length + taper).min(length));
        }
    }
    if let Some(exit) = end_exit {
        positions.extend([(length - exit).max(0.0), (length - exit - taper).max(0.0)]);
        if end_shoulder.is_some() {
            positions.push((length - exit - teardrop_length).max(0.0));
            positions.push((length - exit - teardrop_length - taper).max(0.0));
        }
    }
    if style != "technical" {
        let steps = ((length / 0.45).ceil() as usize).clamp(2, 192);
        positions.extend((0..=steps).map(|index| length * index as f64 / steps as f64));
    }
    positions.sort_by(f64::total_cmp);
    positions.dedup_by(|a, b| (*a - *b).abs() < 1e-8);
    // The min of two linear envelopes can switch inside a source interval.
    let mut crossings = Vec::new();
    if style == "technical" {
        if let (Some(se), Some(ee)) = (start_exit, end_exit) {
            let difference = |s: f64| {
                width_envelope(s, se, taper, neck, trunk, false)
                    - width_envelope(length - s, ee, taper, neck, trunk, false)
            };
            for pair in positions.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                let (da, db) = (difference(a), difference(b));
                if da * db < 0.0 {
                    crossings.push(a + (b - a) * (-da) / (db - da));
                }
            }
        }
    } else if let (Some(se), Some(ee)) = (start_exit, end_exit) {
        let intersection = (length - ee + se) * 0.5;
        let start_ramp_end = se + taper;
        let end_ramp_start = length - ee - taper;
        if intersection >= se - 1e-9
            && intersection <= start_ramp_end + 1e-9
            && intersection >= end_ramp_start - 1e-9
            && intersection <= length - ee + 1e-9
        {
            crossings.push(intersection.clamp(0.0, length));
        }
    }
    positions.extend(crossings);
    positions.sort_by(f64::total_cmp);
    positions.dedup_by(|a, b| (*a - *b).abs() < 1e-8);
    let centers: Vec<_> = positions
        .iter()
        .map(|&s| {
            (
                Point {
                    x: start.x + direction.x * s,
                    y: start.y + direction.y * s,
                },
                variable_trace_width(length, profile, s),
            )
        })
        .collect();
    let half = 8usize;
    let theta = dy.atan2(dx);
    let mut points = Vec::new();
    let (end_center, end_width) = *centers.last().unwrap();
    for i in 0..=half {
        let a = theta - PI / 2.0 + PI * i as f64 / half as f64;
        points.push(Point {
            x: end_center.x + end_width * 0.5 * a.cos(),
            y: end_center.y + end_width * 0.5 * a.sin(),
        });
    }
    for &(center, width) in centers
        .iter()
        .rev()
        .skip(1)
        .take(centers.len().saturating_sub(2))
    {
        points.push(Point {
            x: center.x + normal.x * width * 0.5,
            y: center.y + normal.y * width * 0.5,
        });
    }
    let (start_center, start_width) = centers[0];
    for i in 0..=half {
        let a = theta + PI / 2.0 + PI * i as f64 / half as f64;
        points.push(Point {
            x: start_center.x + start_width * 0.5 * a.cos(),
            y: start_center.y + start_width * 0.5 * a.sin(),
        });
    }
    for &(center, width) in centers.iter().skip(1).take(centers.len().saturating_sub(2)) {
        points.push(Point {
            x: center.x - normal.x * width * 0.5,
            y: center.y - normal.y * width * 0.5,
        });
    }
    points
}

fn rectangle(center: Point, size: Point, rotation: f64) -> Vec<Point> {
    let (sin, cos) = rotation.to_radians().sin_cos();
    [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)]
        .into_iter()
        .map(|(sx, sy)| {
            let (x, y) = (sx * size.x, sy * size.y);
            Point {
                x: center.x + x * cos - y * sin,
                y: center.y + x * sin + y * cos,
            }
        })
        .collect()
}

fn oval(center: Point, size: Point, rotation: f64) -> Vec<Point> {
    let (mut a, mut b, width) = if size.x >= size.y {
        (
            Point {
                x: center.x - (size.x - size.y) / 2.0,
                y: center.y,
            },
            Point {
                x: center.x + (size.x - size.y) / 2.0,
                y: center.y,
            },
            size.y,
        )
    } else {
        (
            Point {
                x: center.x,
                y: center.y - (size.y - size.x) / 2.0,
            },
            Point {
                x: center.x,
                y: center.y + (size.y - size.x) / 2.0,
            },
            size.x,
        )
    };
    if rotation != 0.0 {
        a = rotate_about(a, center, rotation);
        b = rotate_about(b, center, rotation);
    }
    capsule(a, b, width, 16)
}

fn rotate_about(p: Point, center: Point, degrees: f64) -> Point {
    let (sin, cos) = degrees.to_radians().sin_cos();
    let (x, y) = (p.x - center.x, p.y - center.y);
    Point {
        x: center.x + x * cos - y * sin,
        y: center.y + x * sin + y * cos,
    }
}

fn pad_polygon(pad: &Pad) -> Vec<Point> {
    match pad.shape.as_str() {
        "circle" => circle(pad.position, pad.size.x.min(pad.size.y) * 0.5, 24),
        "oval" => oval(pad.position, pad.size, pad.rotation),
        _ => rectangle(pad.position, pad.size, pad.rotation),
    }
}

fn includes_layer(layers: &[String], wanted: &str) -> bool {
    layers.iter().any(|l| l == wanted || l == "*.Cu")
}

fn map_point(p: Point, board: &Board) -> Point {
    Point {
        // V1 is deliberately single-sided: B.Cu is mirrored onto the printable
        // top face, like viewing the finished board from its copper side.
        x: board.bounds.max_x - p.x,
        y: p.y - board.bounds.min_y,
    }
}

#[derive(Clone, Copy)]
struct Drill {
    center: Point,
    radius: f64,
}

struct CopperFeature {
    polygon: Vec<Point>,
    net_id: Option<i64>,
    net_name: Option<String>,
    anchors: Vec<Point>,
}

const OVERLAY_SCALE: f64 = 100_000.0;
type PolyPath = Vec<[f64; 2]>;
type PolyShape = Vec<PolyPath>;
type PolyShapes = Vec<PolyShape>;

fn distance(a: Point, b: Point) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

fn point_in_polygon(point: Point, polygon: &[Point]) -> bool {
    let mut inside = false;
    for (&a, &b) in polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .take(polygon.len())
    {
        if ((a.y > point.y) != (b.y > point.y))
            && point.x < (b.x - a.x) * (point.y - a.y) / (b.y - a.y) + a.x
        {
            inside = !inside;
        }
    }
    inside
}

fn point_segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length2 = dx * dx + dy * dy;
    if length2 <= f64::EPSILON {
        return distance(p, a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / length2).clamp(0.0, 1.0);
    distance(
        p,
        Point {
            x: a.x + t * dx,
            y: a.y + t * dy,
        },
    )
}

fn polygon_clearance(point: Point, polygon: &[Point]) -> f64 {
    polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .take(polygon.len())
        .map(|(&a, &b)| point_segment_distance(point, a, b))
        .fold(f64::INFINITY, f64::min)
}

fn collect_drills(board: &Board, compensation: f64) -> Result<Vec<Drill>, String> {
    let mut drills = Vec::<Drill>::new();
    for (position, diameter) in board
        .pads
        .iter()
        .filter_map(|p| p.drill.map(|d| (p.position, d)))
        .chain(board.vias.iter().map(|v| (v.position, v.drill)))
    {
        if diameter <= 0.0 {
            continue;
        }
        let drill = Drill {
            center: map_point(position, board),
            radius: (diameter + compensation).max(0.05) * 0.5,
        };
        if let Some(existing) = drills
            .iter_mut()
            .find(|d| distance(d.center, drill.center) < 1e-5)
        {
            existing.radius = existing.radius.max(drill.radius);
        } else {
            drills.push(drill);
        }
    }
    for (i, a) in drills.iter().enumerate() {
        for b in drills.iter().skip(i + 1) {
            if distance(a.center, b.center) < a.radius + b.radius - 1e-5 {
                return Err("compensated drill holes overlap; reduce hole compensation or fix the PCB layout".into());
            }
        }
    }
    Ok(drills)
}

fn path_from_points(mut points: Vec<Point>) -> PolyPath {
    if polygon_area(&points) < 0.0 {
        points.reverse();
    }
    points.into_iter().map(|p| [p.x, p.y]).collect()
}

fn canonicalize_shapes(shapes: &mut PolyShapes) {
    for point in shapes.iter_mut().flatten().flatten() {
        for coordinate in point {
            *coordinate = (*coordinate * OVERLAY_SCALE).round() / OVERLAY_SCALE;
            if coordinate.abs() < 0.5 / OVERLAY_SCALE {
                *coordinate = 0.0;
            }
        }
    }
    for shape in shapes.iter_mut() {
        for contour in shape.iter_mut() {
            loop {
                if contour.len() < 3 {
                    break;
                }
                let mut remove = None;
                for i in 0..contour.len() {
                    let a = contour[(i + contour.len() - 1) % contour.len()];
                    let b = contour[i];
                    let c = contour[(i + 1) % contour.len()];
                    let ab = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
                    let bc = ((c[0] - b[0]).powi(2) + (c[1] - b[1]).powi(2)).sqrt();
                    let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
                    let rounding_area = (ab + bc) * (0.5 / OVERLAY_SCALE);
                    if (a == b) || (b == c) || cross.abs() <= rounding_area {
                        remove = Some(i);
                        break;
                    }
                }
                if let Some(index) = remove {
                    contour.remove(index);
                } else {
                    break;
                }
            }
        }
        if shape.first().is_none_or(|outer| outer.len() < 3) {
            shape.clear();
        } else {
            shape.retain(|contour| contour.len() >= 3);
        }
    }
    shapes.retain(|shape| !shape.is_empty());
}

fn boolean_shapes(
    subject: &PolyShapes,
    clip: &PolyShapes,
    rule: OverlayRule,
) -> Result<PolyShapes, String> {
    let mut result = subject
        .overlay_with_fixed_scale_as::<i64>(clip, rule, FillRule::NonZero, OVERLAY_SCALE)
        .map_err(|error| format!("polygon boolean operation failed: {error:?}"))?;
    canonicalize_shapes(&mut result);
    Ok(result)
}

/// Extract Q, Q-R, and Q∩R from one overlay graph so every partition member
/// shares the exact same fixed-grid intersection vertices.
fn partition_shapes(
    board_region: &PolyShapes,
    copper_region: &PolyShapes,
) -> Result<(PolyShapes, PolyShapes, PolyShapes), String> {
    if copper_region.is_empty() {
        return Ok((board_region.clone(), board_region.clone(), Vec::new()));
    }
    let mut overlay = FloatOverlay::<[f64; 2], i64>::from_subj_and_clip_fixed_scale(
        board_region,
        copper_region,
        OVERLAY_SCALE,
    )
    .map_err(|error| format!("polygon partition operation failed: {error:?}"))?;
    let graph = overlay
        .build_graph_view(FillRule::NonZero)
        .ok_or("polygon partition operation produced no graph")?;
    let mut buffer = BooleanExtractionBuffer::<i64>::default();
    let mut board = graph.extract_shapes(OverlayRule::Subject, &mut buffer);
    let mut plain = graph.extract_shapes(OverlayRule::Difference, &mut buffer);
    let mut copper = graph.extract_shapes(OverlayRule::Intersect, &mut buffer);
    canonicalize_shapes(&mut board);
    canonicalize_shapes(&mut plain);
    canonicalize_shapes(&mut copper);
    Ok((board, plain, copper))
}

fn reject_point_tangencies(shapes: &PolyShapes) -> Result<(), String> {
    let mut owners = HashMap::<[u64; 2], (usize, usize)>::new();
    for (shape_index, shape) in shapes.iter().enumerate() {
        for (contour_index, contour) in shape.iter().enumerate() {
            for point in contour {
                let key = [point[0].to_bits(), point[1].to_bits()];
                if owners.insert(key, (shape_index, contour_index)).is_some() {
                    return Err("raised copper has a point-only tangency; overlap the features slightly or separate them to produce a manifold print".into());
                }
            }
        }
    }
    Ok(())
}

fn validate_annulus(polygon: &[Point], center: Point, drills: &[Drill]) -> Result<(), String> {
    if let Some(drill) = drills
        .iter()
        .find(|drill| distance(drill.center, center) < 1.0 / OVERLAY_SCALE)
    {
        let clearance = polygon_clearance(drill.center, polygon) - drill.radius;
        if !point_in_polygon(drill.center, polygon) || clearance <= 1.0 / OVERLAY_SCALE {
            return Err("a compensated drill consumes its raised pad/via annulus; reduce hole compensation or increase the copper diameter".into());
        }
    }
    Ok(())
}

#[derive(Clone)]
struct PadAttachment {
    exit: f64,
    shoulder: f64,
    polygon: Vec<Point>,
}

fn matching_pad_attachment(
    board: &Board,
    trace: &Trace,
    endpoint: Point,
    direction: Point,
) -> Option<PadAttachment> {
    board
        .pads
        .iter()
        .filter(|pad| {
            pad.pad_type == "thru_hole"
                && pad.shape != "custom"
                && includes_layer(&pad.layers, "B.Cu")
        })
        .filter_map(|pad| {
            let mut mapped = pad.clone();
            mapped.position = map_point(pad.position, board);
            mapped.rotation = -pad.rotation;
            let polygon = pad_polygon(&mapped);
            let trace_net = positive_net_id(trace.net_id);
            let pad_net = positive_net_id(pad.net_id);
            let known_match = trace_net.is_some() && trace_net == pad_net;
            let legacy_center = trace_net.is_none()
                && pad_net.is_none()
                && distance(endpoint, mapped.position) < 1e-5;
            if !(known_match && point_in_polygon(endpoint, &polygon) || legacy_center) {
                return None;
            }
            ray_exit_distance(endpoint, direction, &polygon).map(|exit| PadAttachment {
                exit,
                shoulder: pad.size.x.min(pad.size.y) * 0.95,
                polygon,
            })
        })
        .max_by(|a, b| a.exit.total_cmp(&b.exit))
}

#[cfg(test)]
fn matching_pad_exit(
    board: &Board,
    trace: &Trace,
    endpoint: Point,
    direction: Point,
) -> Option<f64> {
    matching_pad_attachment(board, trace, endpoint, direction).map(|attachment| attachment.exit)
}

#[cfg(test)]
fn trace_polygon(board: &Board, trace: &Trace, settings: &Settings) -> Vec<Point> {
    trace_polygon_with_trim(board, trace, settings, TraceTrim::default())
}

fn effective_teardrop_lengths(
    trace_length: f64,
    start: Option<&PadAttachment>,
    end: Option<&PadAttachment>,
    settings: &Settings,
) -> (f64, f64) {
    if settings.trace_style != "vintage" || settings.teardrop_strength <= 1e-9 {
        return (0.0, 0.0);
    }
    let usable = (trace_length
        - start.map(|attachment| attachment.exit).unwrap_or(0.0)
        - end.map(|attachment| attachment.exit).unwrap_or(0.0))
    .max(0.0);
    let requested = settings.teardrop_length.max(0.0);
    let (start_length, end_length) = match (start.is_some(), end.is_some()) {
        (true, true) => {
            let each = requested.min(usable * 0.48);
            (each, each)
        }
        (true, false) => (requested.min(usable), 0.0),
        (false, true) => (0.0, requested.min(usable)),
        (false, false) => (0.0, 0.0),
    };
    // Sub-millimetre slivers create poor FDM geometry and are visually worse
    // than omitting the optional lobe on an exceptionally short connection.
    let printable = |length: f64| if length >= 0.25 { length } else { 0.0 };
    (printable(start_length), printable(end_length))
}

fn teardrop_lobe_polygon(
    endpoint: Point,
    direction: Point,
    attachment: &PadAttachment,
    neck_width: f64,
    length: f64,
    strength: f64,
) -> Option<Vec<Point>> {
    if length < 0.25 || strength <= 1e-9 {
        return None;
    }
    let normal = Point {
        x: -direction.y,
        y: direction.x,
    };
    let shoulder_width =
        neck_width + (attachment.shoulder - neck_width).max(0.0) * strength.clamp(0.0, 1.0);
    let shoulder_half = shoulder_width * 0.5;
    let shoulder_origin = |sign: f64| Point {
        x: endpoint.x + normal.x * shoulder_half * sign,
        y: endpoint.y + normal.y * shoulder_half * sign,
    };
    let (upper, upper_edge) = ray_exit_hit(shoulder_origin(1.0), direction, &attachment.polygon)?;
    let (lower, lower_edge) = ray_exit_hit(shoulder_origin(-1.0), direction, &attachment.polygon)?;
    let tip_center = Point {
        x: endpoint.x + direction.x * (attachment.exit + length),
        y: endpoint.y + direction.y * (attachment.exit + length),
    };
    let upper_tip = Point {
        x: tip_center.x + normal.x * neck_width * 0.5,
        y: tip_center.y + normal.y * neck_width * 0.5,
    };
    let lower_tip = Point {
        x: tip_center.x - normal.x * neck_width * 0.5,
        y: tip_center.y - normal.y * neck_width * 0.5,
    };
    let curve = |start: Point, edge: Point, end: Point, side: f64| {
        let chord = Point {
            x: end.x - start.x,
            y: end.y - start.y,
        };
        let tangent = if edge.x * chord.x + edge.y * chord.y >= 0.0 {
            edge
        } else {
            Point {
                x: -edge.x,
                y: -edge.y,
            }
        };
        let span = distance(start, end);
        let side_coordinate = |point: Point| (point.x * normal.x + point.y * normal.y) * side;
        let inward_slope = -(tangent.x * normal.x + tangent.y * normal.y) * side;
        let side_drop = (side_coordinate(start) - side_coordinate(end)).max(0.0);
        let monotonic_limit = if inward_slope > 1e-8 {
            side_drop * 0.9 / inward_slope
        } else {
            span * 0.34
        };
        let shoulder_handle = (span * 0.34).min(monotonic_limit.max(0.0));
        let tip_handle = span * 0.30;
        let control_1 = Point {
            x: start.x + tangent.x * shoulder_handle,
            y: start.y + tangent.y * shoulder_handle,
        };
        let control_2 = Point {
            x: end.x - direction.x * tip_handle,
            y: end.y - direction.y * tip_handle,
        };
        (0..=12)
            .map(|index| cubic_bezier(start, control_1, control_2, end, index as f64 / 12.0))
            .collect::<Vec<_>>()
    };
    let upper_curve = curve(upper, upper_edge, upper_tip, 1.0);
    let lower_curve = curve(lower, lower_edge, lower_tip, -1.0);
    let mut polygon = upper_curve;
    polygon.extend(lower_curve.into_iter().rev());
    (polygon_area(&polygon).abs() > 1e-8).then_some(polygon)
}

#[derive(Clone, Copy, Default)]
struct TraceTrim {
    start: f64,
    end: f64,
}

fn trace_polygon_with_trim(
    board: &Board,
    trace: &Trace,
    settings: &Settings,
    trim: TraceTrim,
) -> Vec<Point> {
    let source_start = map_point(trace.start, board);
    let source_end = map_point(trace.end, board);
    if settings.width_mode == "preserve" {
        return capsule(source_start, source_end, trace.width, 16);
    }
    let trunk = trace.width.max(settings.trace_width);
    let neck = trunk.min(settings.neckdown_width);
    let source_length = distance(source_start, source_end);
    let direction = if source_length > 1e-9 {
        Point {
            x: (source_end.x - source_start.x) / source_length,
            y: (source_end.y - source_start.y) / source_length,
        }
    } else {
        Point { x: 1.0, y: 0.0 }
    };
    let start_trim = trim.start.clamp(0.0, source_length * 0.45);
    let end_trim = trim.end.clamp(0.0, source_length * 0.45);
    let start = Point {
        x: source_start.x + direction.x * start_trim,
        y: source_start.y + direction.y * start_trim,
    };
    let end = Point {
        x: source_end.x - direction.x * end_trim,
        y: source_end.y - direction.y * end_trim,
    };
    let start_attachment = (start_trim <= 1e-9)
        .then(|| matching_pad_attachment(board, trace, source_start, direction))
        .flatten();
    let reverse = Point {
        x: -direction.x,
        y: -direction.y,
    };
    let end_attachment = (end_trim <= 1e-9)
        .then(|| matching_pad_attachment(board, trace, source_end, reverse))
        .flatten();
    let (start_lobe_length, end_lobe_length) = effective_teardrop_lengths(
        (source_length - start_trim - end_trim).max(0.0),
        start_attachment.as_ref(),
        end_attachment.as_ref(),
        settings,
    );
    variable_trace_polygon(
        start,
        end,
        VariableTraceProfile {
            trunk,
            neck,
            taper: settings.taper_length,
            start_exit: start_attachment
                .as_ref()
                .map(|attachment| attachment.exit + start_lobe_length),
            end_exit: end_attachment
                .as_ref()
                .map(|attachment| attachment.exit + end_lobe_length),
            style: &settings.trace_style,
            teardrop_length: settings.teardrop_length,
            start_shoulder: None,
            end_shoulder: None,
        },
    )
}

fn trace_teardrop_polygons_with_trim(
    board: &Board,
    trace: &Trace,
    settings: &Settings,
    trim: TraceTrim,
) -> Vec<Vec<Point>> {
    if settings.width_mode != "auto"
        || settings.trace_style != "vintage"
        || settings.teardrop_strength <= 1e-9
    {
        return Vec::new();
    }
    let start = map_point(trace.start, board);
    let end = map_point(trace.end, board);
    let trace_length = distance(start, end);
    if trace_length <= 1e-9 {
        return Vec::new();
    }
    let direction = Point {
        x: (end.x - start.x) / trace_length,
        y: (end.y - start.y) / trace_length,
    };
    let reverse = Point {
        x: -direction.x,
        y: -direction.y,
    };
    let start_trim = trim.start.clamp(0.0, trace_length * 0.45);
    let end_trim = trim.end.clamp(0.0, trace_length * 0.45);
    let start_attachment = (start_trim <= 1e-9)
        .then(|| matching_pad_attachment(board, trace, start, direction))
        .flatten();
    let end_attachment = (end_trim <= 1e-9)
        .then(|| matching_pad_attachment(board, trace, end, reverse))
        .flatten();
    let (start_length, end_length) = effective_teardrop_lengths(
        (trace_length - start_trim - end_trim).max(0.0),
        start_attachment.as_ref(),
        end_attachment.as_ref(),
        settings,
    );
    let neck = trace
        .width
        .max(settings.trace_width)
        .min(settings.neckdown_width);
    let mut polygons = Vec::new();
    if let Some(attachment) = start_attachment.as_ref() {
        if let Some(lobe) = teardrop_lobe_polygon(
            start,
            direction,
            attachment,
            neck,
            start_length,
            settings.teardrop_strength,
        ) {
            polygons.push(lobe);
        }
    }
    if let Some(attachment) = end_attachment.as_ref() {
        if let Some(lobe) = teardrop_lobe_polygon(
            end,
            reverse,
            attachment,
            neck,
            end_length,
            settings.teardrop_strength,
        ) {
            polygons.push(lobe);
        }
    }
    polygons
}

#[derive(Clone, Copy)]
struct CornerNodeEntry {
    trace_index: usize,
    at_start: bool,
    node: Point,
    other: Point,
    length: f64,
}

#[derive(Clone, Copy)]
struct PathSample {
    point: Point,
    tangent: Point,
    width: f64,
}

#[derive(Clone, Copy)]
struct CircularFillet {
    center: Point,
    radius: f64,
    start: Point,
    end: Point,
    start_angle: f64,
    sweep: f64,
}

fn normalized(vector: Point) -> Point {
    let length = (vector.x * vector.x + vector.y * vector.y).sqrt();
    if length <= 1e-9 {
        Point { x: 1.0, y: 0.0 }
    } else {
        Point {
            x: vector.x / length,
            y: vector.y / length,
        }
    }
}

fn circular_fillet(
    node: Point,
    first_direction: Point,
    second_direction: Point,
    reach: f64,
    interior: f64,
) -> Option<CircularFillet> {
    let half_interior = interior * 0.5;
    let sin_half = half_interior.sin();
    let bisector = Point {
        x: first_direction.x + second_direction.x,
        y: first_direction.y + second_direction.y,
    };
    let bisector_length = (bisector.x * bisector.x + bisector.y * bisector.y).sqrt();
    if reach <= 1e-9 || sin_half <= 1e-9 || bisector_length <= 1e-9 {
        return None;
    }
    let radius = reach * half_interior.tan();
    if !radius.is_finite() || radius <= 1e-9 {
        return None;
    }
    let center_scale = radius / (sin_half * bisector_length);
    let center = Point {
        x: node.x + bisector.x * center_scale,
        y: node.y + bisector.y * center_scale,
    };
    let start = Point {
        x: node.x + first_direction.x * reach,
        y: node.y + first_direction.y * reach,
    };
    let end = Point {
        x: node.x + second_direction.x * reach,
        y: node.y + second_direction.y * reach,
    };
    let start_radius = Point {
        x: start.x - center.x,
        y: start.y - center.y,
    };
    let end_radius = Point {
        x: end.x - center.x,
        y: end.y - center.y,
    };
    let start_angle = start_radius.y.atan2(start_radius.x);
    let sweep = (start_radius.x * end_radius.y - start_radius.y * end_radius.x)
        .atan2(start_radius.x * end_radius.x + start_radius.y * end_radius.y);
    (sweep.abs() > 1e-9).then_some(CircularFillet {
        center,
        radius,
        start,
        end,
        start_angle,
        sweep,
    })
}

fn swept_path_polygon(samples: &[PathSample]) -> Vec<Point> {
    if samples.len() < 2 {
        return Vec::new();
    }
    let mut left = Vec::with_capacity(samples.len());
    let mut right = Vec::with_capacity(samples.len());
    for sample in samples {
        let direction = normalized(sample.tangent);
        let normal = Point {
            x: -direction.y,
            y: direction.x,
        };
        left.push(Point {
            x: sample.point.x + normal.x * sample.width * 0.5,
            y: sample.point.y + normal.y * sample.width * 0.5,
        });
        right.push(Point {
            x: sample.point.x - normal.x * sample.width * 0.5,
            y: sample.point.y - normal.y * sample.width * 0.5,
        });
    }
    let cap_steps = 8usize;
    let end = samples[samples.len() - 1];
    let end_tangent = normalized(end.tangent);
    let end_theta = end_tangent.y.atan2(end_tangent.x);
    let start = samples[0];
    let start_tangent = normalized(start.tangent);
    let start_theta = start_tangent.y.atan2(start_tangent.x);
    let mut polygon = left;
    for index in 1..=cap_steps {
        let angle = end_theta + PI / 2.0 - PI * index as f64 / cap_steps as f64;
        polygon.push(Point {
            x: end.point.x + angle.cos() * end.width * 0.5,
            y: end.point.y + angle.sin() * end.width * 0.5,
        });
    }
    polygon.extend(right.into_iter().rev().skip(1));
    for index in 1..=cap_steps {
        let angle = start_theta - PI / 2.0 - PI * index as f64 / cap_steps as f64;
        polygon.push(Point {
            x: start.point.x + angle.cos() * start.width * 0.5,
            y: start.point.y + angle.sin() * start.width * 0.5,
        });
    }
    polygon
}

fn protected_corner_node(board: &Board, point: Point) -> bool {
    board.pads.iter().any(|pad| {
        if pad.pad_type != "thru_hole"
            || pad.shape == "custom"
            || !includes_layer(&pad.layers, "B.Cu")
        {
            return false;
        }
        let mut mapped = pad.clone();
        mapped.position = map_point(pad.position, board);
        mapped.rotation = -pad.rotation;
        point_in_polygon(point, &pad_polygon(&mapped))
    }) || board.vias.iter().any(|via| {
        includes_layer(&via.layers, "B.Cu")
            && distance(point, map_point(via.position, board)) <= via.size * 0.5 + 1e-8
    })
}

fn same_trace_circuit(first: &Trace, second: &Trace) -> bool {
    match (
        positive_net_id(first.net_id),
        positive_net_id(second.net_id),
    ) {
        (Some(first), Some(second)) => first == second,
        (None, None) => true,
        _ => false,
    }
}

fn positive_net_id(net_id: Option<i64>) -> Option<i64> {
    net_id.filter(|id| *id > 0)
}

fn trace_source_width_and_slope_at_distance(
    board: &Board,
    trace: &Trace,
    settings: &Settings,
    travelled: f64,
) -> (f64, f64) {
    if settings.width_mode == "preserve" {
        return (trace.width, 0.0);
    }
    let start = map_point(trace.start, board);
    let end = map_point(trace.end, board);
    let length = distance(start, end);
    if length <= 1e-9 {
        return (trace.width.max(settings.trace_width), 0.0);
    }
    let direction = Point {
        x: (end.x - start.x) / length,
        y: (end.y - start.y) / length,
    };
    let reverse = Point {
        x: -direction.x,
        y: -direction.y,
    };
    let start_attachment = matching_pad_attachment(board, trace, start, direction);
    let end_attachment = matching_pad_attachment(board, trace, end, reverse);
    let (start_lobe_length, end_lobe_length) = effective_teardrop_lengths(
        length,
        start_attachment.as_ref(),
        end_attachment.as_ref(),
        settings,
    );
    let trunk = trace.width.max(settings.trace_width);
    let profile = VariableTraceProfile {
        trunk,
        neck: trunk.min(settings.neckdown_width),
        taper: settings.taper_length,
        start_exit: start_attachment
            .as_ref()
            .map(|attachment| attachment.exit + start_lobe_length),
        end_exit: end_attachment
            .as_ref()
            .map(|attachment| attachment.exit + end_lobe_length),
        style: &settings.trace_style,
        teardrop_length: settings.teardrop_length,
        start_shoulder: None,
        end_shoulder: None,
    };
    variable_trace_width_and_slope(length, profile, travelled.clamp(0.0, length))
}

#[cfg(test)]
fn trace_source_width_at_distance(
    board: &Board,
    trace: &Trace,
    settings: &Settings,
    travelled: f64,
) -> f64 {
    trace_source_width_and_slope_at_distance(board, trace, settings, travelled).0
}

fn bounded_hermite_width(
    t: f64,
    start: f64,
    end: f64,
    slopes: (f64, f64),
    length: f64,
    bounds: (f64, f64),
) -> f64 {
    let (mut first_slope, mut second_slope) = slopes;
    let (minimum, maximum) = bounds;
    let secant = (end - start) / length.max(1e-9);
    if secant.abs() > 1e-9 {
        if first_slope * secant < 0.0 {
            first_slope = 0.0;
        }
        if second_slope * secant < 0.0 {
            second_slope = 0.0;
        }
        let alpha = first_slope / secant;
        let beta = second_slope / secant;
        let magnitude = alpha.hypot(beta);
        if magnitude > 3.0 {
            let scale = 3.0 / magnitude;
            first_slope *= scale;
            second_slope *= scale;
        }
    }
    let control_1 = (start + first_slope * length / 3.0).clamp(minimum, maximum);
    let control_2 = (end - second_slope * length / 3.0).clamp(minimum, maximum);
    let inverse = 1.0 - t;
    inverse.powi(3) * start
        + 3.0 * inverse * inverse * t * control_1
        + 3.0 * inverse * t * t * control_2
        + t.powi(3) * end
}

fn corner_geometry(board: &Board, settings: &Settings) -> (Vec<TraceTrim>, Vec<CopperFeature>) {
    let mut trims = vec![TraceTrim::default(); board.traces.len()];
    if settings.width_mode != "auto" || settings.trace_style != "vintage" {
        return (trims, Vec::new());
    }
    let mut nodes = BTreeMap::<(i64, i64), Vec<CornerNodeEntry>>::new();
    for (trace_index, trace) in board.traces.iter().enumerate() {
        if trace.layer != "B.Cu" {
            continue;
        }
        let start = map_point(trace.start, board);
        let end = map_point(trace.end, board);
        let length = distance(start, end);
        if length <= 1e-9 {
            continue;
        }
        for (node, other, at_start) in [(start, end, true), (end, start, false)] {
            let key = (
                (node.x * 10_000.0).round() as i64,
                (node.y * 10_000.0).round() as i64,
            );
            nodes.entry(key).or_default().push(CornerNodeEntry {
                trace_index,
                at_start,
                node,
                other,
                length,
            });
        }
    }

    let mut features = Vec::new();
    for entries in nodes.into_values() {
        if entries.len() != 2 || protected_corner_node(board, entries[0].node) {
            continue;
        }
        let first = entries[0];
        let second = entries[1];
        let first_trace = &board.traces[first.trace_index];
        let second_trace = &board.traces[second.trace_index];
        if !same_trace_circuit(first_trace, second_trace) {
            continue;
        }
        let first_direction = normalized(Point {
            x: first.other.x - first.node.x,
            y: first.other.y - first.node.y,
        });
        let second_direction = normalized(Point {
            x: second.other.x - second.node.x,
            y: second.other.y - second.node.y,
        });
        let dot = (first_direction.x * second_direction.x + first_direction.y * second_direction.y)
            .clamp(-1.0, 1.0);
        let interior = dot.acos();
        let deflection = PI - interior;
        if !(PI / 36.0..=PI * 5.0 / 6.0).contains(&deflection) {
            continue;
        }
        let wanted = settings.corner_radius * (deflection * 0.5).tan();
        let reach = wanted.min(first.length * 0.34).min(second.length * 0.34);
        let first_join = trace_source_width_and_slope_at_distance(
            board,
            first_trace,
            settings,
            if first.at_start {
                reach
            } else {
                first.length - reach
            },
        );
        let second_join = trace_source_width_and_slope_at_distance(
            board,
            second_trace,
            settings,
            if second.at_start {
                reach
            } else {
                second.length - reach
            },
        );
        let first_width = first_join.0;
        let second_width = second_join.0;
        if reach < first_width.max(second_width) * 0.3 {
            continue;
        }
        let Some(fillet) = circular_fillet(
            first.node,
            first_direction,
            second_direction,
            reach,
            interior,
        ) else {
            continue;
        };
        if fillet.radius < first_width.max(second_width) * 0.55 {
            continue;
        }
        if first.at_start {
            trims[first.trace_index].start = trims[first.trace_index].start.max(reach);
        } else {
            trims[first.trace_index].end = trims[first.trace_index].end.max(reach);
        }
        if second.at_start {
            trims[second.trace_index].start = trims[second.trace_index].start.max(reach);
        } else {
            trims[second.trace_index].end = trims[second.trace_index].end.max(reach);
        }
        let steps = ((deflection / (PI / 36.0)).ceil() as usize).clamp(12, 36);
        let length = fillet.radius * fillet.sweep.abs();
        let first_slope = first_join.1 * if first.at_start { -1.0 } else { 1.0 };
        let second_slope = second_join.1 * if second.at_start { 1.0 } else { -1.0 };
        let minimum_width = first_width
            .min(second_width)
            .min(
                first_trace
                    .width
                    .max(settings.trace_width)
                    .min(settings.neckdown_width),
            )
            .min(
                second_trace
                    .width
                    .max(settings.trace_width)
                    .min(settings.neckdown_width),
            );
        let maximum_width = first_width
            .max(second_width)
            .max(first_trace.width.max(settings.trace_width))
            .max(second_trace.width.max(settings.trace_width));
        let samples: Vec<_> = (0..=steps)
            .map(|index| {
                let t = index as f64 / steps as f64;
                let angle = fillet.start_angle + fillet.sweep * t;
                let radial = Point {
                    x: angle.cos(),
                    y: angle.sin(),
                };
                PathSample {
                    point: if index == 0 {
                        fillet.start
                    } else if index == steps {
                        fillet.end
                    } else {
                        Point {
                            x: fillet.center.x + radial.x * fillet.radius,
                            y: fillet.center.y + radial.y * fillet.radius,
                        }
                    },
                    tangent: if fillet.sweep >= 0.0 {
                        Point {
                            x: -radial.y,
                            y: radial.x,
                        }
                    } else {
                        Point {
                            x: radial.y,
                            y: -radial.x,
                        }
                    },
                    width: bounded_hermite_width(
                        t,
                        first_width,
                        second_width,
                        (first_slope, second_slope),
                        length,
                        (minimum_width, maximum_width),
                    ),
                }
            })
            .collect();
        features.push(CopperFeature {
            polygon: swept_path_polygon(&samples),
            net_id: positive_net_id(first_trace.net_id),
            net_name: first_trace.net_name.clone(),
            anchors: vec![fillet.start, fillet.end],
        });
    }
    (trims, features)
}

fn features_coordinate_connected(a: &CopperFeature, b: &CopperFeature) -> bool {
    a.anchors.iter().any(|point| {
        b.anchors
            .iter()
            .any(|other| distance(*point, *other) < 1e-5)
            || point_in_polygon(*point, &b.polygon)
    }) || b
        .anchors
        .iter()
        .any(|point| point_in_polygon(*point, &a.polygon))
}

fn shapes_area(shapes: &PolyShapes) -> f64 {
    shapes
        .iter()
        .map(|shape| {
            shape
                .iter()
                .map(|contour| {
                    let points: Vec<_> =
                        contour.iter().map(|p| Point { x: p[0], y: p[1] }).collect();
                    polygon_area(&points)
                })
                .sum::<f64>()
                .abs()
        })
        .sum()
}

fn point_segment_nearest(p: Point, a: Point, b: Point) -> (f64, Point) {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length2 = dx * dx + dy * dy;
    let t = if length2 <= f64::EPSILON {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / length2).clamp(0.0, 1.0)
    };
    let nearest = Point {
        x: a.x + t * dx,
        y: a.y + t * dy,
    };
    (distance(p, nearest), nearest)
}

fn shapes_distance(a: &PolyShapes, b: &PolyShapes) -> (f64, Point) {
    let mut minimum = f64::INFINITY;
    let mut midpoint = Point { x: 0.0, y: 0.0 };
    for ca in a.iter().flatten() {
        for cb in b.iter().flatten() {
            for (&a0, &a1) in ca.iter().zip(ca.iter().cycle().skip(1)).take(ca.len()) {
                let pa0 = Point { x: a0[0], y: a0[1] };
                let pa1 = Point { x: a1[0], y: a1[1] };
                for (&b0, &b1) in cb.iter().zip(cb.iter().cycle().skip(1)).take(cb.len()) {
                    let pb0 = Point { x: b0[0], y: b0[1] };
                    let pb1 = Point { x: b1[0], y: b1[1] };
                    for (point, a, b) in [
                        (pa0, pb0, pb1),
                        (pa1, pb0, pb1),
                        (pb0, pa0, pa1),
                        (pb1, pa0, pa1),
                    ] {
                        let (candidate, nearest) = point_segment_nearest(point, a, b);
                        if candidate < minimum {
                            minimum = candidate;
                            midpoint = Point {
                                x: (point.x + nearest.x) * 0.5,
                                y: (point.y + nearest.y) * 0.5,
                            };
                        }
                    }
                }
            }
        }
    }
    (minimum, midpoint)
}

fn net_label(id: Option<i64>, name: &Option<String>, index: usize) -> String {
    match (id, name) {
        (Some(id), Some(name)) => format!("net {id} ({name})"),
        (Some(id), None) => format!("net {id}"),
        _ => format!("unknown circuit {}", index + 1),
    }
}

fn build_regions(
    board: &Board,
    settings: &Settings,
    outline: Vec<Point>,
    drills: &[Drill],
) -> Result<(PolyShapes, PolyShapes, PolyShapes), String> {
    let board_outer = vec![vec![path_from_points(outline)]];
    let drill_shapes: PolyShapes = drills
        .iter()
        // Half-step phase avoids zero-area Earcut bridge triangles when a
        // concentric round pad/via uses the same segment count.
        .map(|drill| {
            vec![path_from_points(circle_with_phase(
                drill.center,
                drill.radius,
                24,
                PI / 24.0,
            ))]
        })
        .collect();
    let board_region = if drill_shapes.is_empty() {
        board_outer.clone()
    } else {
        boolean_shapes(&board_outer, &drill_shapes, OverlayRule::Difference)?
    };

    let (trace_trims, corner_features) = corner_geometry(board, settings);
    let mut features = Vec::<CopperFeature>::new();
    for (trace_index, trace) in board
        .traces
        .iter()
        .enumerate()
        .filter(|(_, trace)| trace.layer == "B.Cu")
    {
        let start = map_point(trace.start, board);
        let end = map_point(trace.end, board);
        let polygon = trace_polygon_with_trim(board, trace, settings, trace_trims[trace_index]);
        features.push(CopperFeature {
            polygon,
            net_id: positive_net_id(trace.net_id),
            net_name: trace.net_name.clone(),
            anchors: vec![start, end],
        });
        for lobe in
            trace_teardrop_polygons_with_trim(board, trace, settings, trace_trims[trace_index])
        {
            features.push(CopperFeature {
                polygon: lobe,
                net_id: positive_net_id(trace.net_id),
                net_name: trace.net_name.clone(),
                anchors: vec![start, end],
            });
        }
    }
    features.extend(corner_features);
    for pad in board.pads.iter().filter(|pad| {
        pad.pad_type == "thru_hole" && pad.shape != "custom" && includes_layer(&pad.layers, "B.Cu")
    }) {
        let mut mapped = pad.clone();
        mapped.position = map_point(pad.position, board);
        mapped.rotation = -pad.rotation;
        let polygon = pad_polygon(&mapped);
        validate_annulus(&polygon, mapped.position, drills)?;
        features.push(CopperFeature {
            polygon,
            net_id: positive_net_id(pad.net_id),
            net_name: pad.net_name.clone(),
            anchors: vec![mapped.position],
        });
    }
    for via in board
        .vias
        .iter()
        .filter(|via| includes_layer(&via.layers, "B.Cu"))
    {
        let center = map_point(via.position, board);
        let polygon = circle(center, via.size * 0.5, 24);
        validate_annulus(&polygon, center, drills)?;
        features.push(CopperFeature {
            polygon,
            net_id: positive_net_id(via.net_id),
            net_name: via.net_name.clone(),
            anchors: vec![center],
        });
    }
    for zone in board.zones.iter().filter(|zone| zone.layer == "B.Cu") {
        if zone.kind == "keepout" {
            continue;
        }
        if zone.polygons.is_empty() {
            return Err(
                "a B.Cu zone has no cached fill; refill zones in KiCad and re-import the saved board"
                    .into(),
            );
        }
        // KiCad's authored teardrops are themselves cached zone fills. In
        // generated Vintage mode Copperline replaces them with its own lobes;
        // all other modes preserve the authored zone exactly.
        if zone.kind == "teardrop"
            && settings.width_mode == "auto"
            && settings.trace_style == "vintage"
        {
            continue;
        }
        let mut polygons: Vec<Vec<Point>> = zone
            .polygons
            .iter()
            .filter(|polygon| polygon.len() >= 3)
            .map(|polygon| {
                polygon
                    .iter()
                    .copied()
                    .map(|point| map_point(point, board))
                    .collect()
            })
            .collect();
        polygons.sort_by(|a, b| {
            let key = |polygon: &[Point]| {
                polygon.iter().fold(
                    (f64::INFINITY, f64::INFINITY, 0.0f64),
                    |(min_x, min_y, _), point| {
                        (
                            min_x.min(point.x),
                            min_y.min(point.y),
                            polygon_area(polygon).abs(),
                        )
                    },
                )
            };
            let ka = key(a);
            let kb = key(b);
            ka.0.total_cmp(&kb.0)
                .then_with(|| ka.1.total_cmp(&kb.1))
                .then_with(|| ka.2.total_cmp(&kb.2))
        });
        for polygon in polygons {
            features.push(CopperFeature {
                anchors: polygon.clone(),
                polygon,
                net_id: positive_net_id(zone.net_id),
                net_name: zone.net_name.clone(),
            });
        }
    }

    let copper_region = if features.is_empty() {
        Vec::new()
    } else {
        let mut parent: Vec<usize> = (0..features.len()).collect();
        fn root(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        for i in 0..features.len() {
            for j in i + 1..features.len() {
                let same_known = features[i].net_id.filter(|id| *id > 0).is_some()
                    && features[i].net_id == features[j].net_id;
                let connected_unknown = features[i].net_id.is_none()
                    && features[j].net_id.is_none()
                    && features_coordinate_connected(&features[i], &features[j]);
                if same_known || connected_unknown {
                    let ri = root(&mut parent, i);
                    let rj = root(&mut parent, j);
                    if ri != rj {
                        parent[rj] = ri;
                    }
                }
            }
        }
        let mut grouped = BTreeMap::<usize, Vec<usize>>::new();
        for i in 0..features.len() {
            let r = root(&mut parent, i);
            grouped.entry(r).or_default().push(i);
        }
        let mut groups = Vec::<(Option<i64>, Option<String>, PolyShapes)>::new();
        for indices in grouped.into_values() {
            let raw: PolyShapes = indices
                .iter()
                .map(|&i| vec![path_from_points(features[i].polygon.clone())])
                .collect();
            let union = boolean_shapes(&raw, &board_outer, OverlayRule::Subject)?;
            let region = boolean_shapes(&union, &board_region, OverlayRule::Intersect)?;
            let first = &features[indices[0]];
            groups.push((first.net_id, first.net_name.clone(), region));
        }
        for i in 0..groups.len() {
            for j in i + 1..groups.len() {
                let intersection =
                    boolean_shapes(&groups[i].2, &groups[j].2, OverlayRule::Intersect)?;
                let left = net_label(groups[i].0, &groups[i].1, i);
                let right = net_label(groups[j].0, &groups[j].1, j);
                if shapes_area(&intersection) > 1e-8 {
                    let location = intersection
                        .iter()
                        .flatten()
                        .flatten()
                        .next()
                        .copied()
                        .unwrap_or([0.0, 0.0]);
                    return Err(format!(
                        "copper collision between {left} and {right} near ({:.3}, {:.3}) mm",
                        board.bounds.max_x - location[0],
                        location[1] + board.bounds.min_y
                    ));
                }
                let (actual, location) = shapes_distance(&groups[i].2, &groups[j].2);
                let board_location = Point {
                    x: board.bounds.max_x - location.x,
                    y: location.y + board.bounds.min_y,
                };
                if actual < 1e-6 {
                    return Err(format!(
                        "point-only tangency between {left} and {right} near ({:.3}, {:.3}) mm",
                        board_location.x, board_location.y
                    ));
                }
                if actual + 1e-6 < settings.trace_clearance {
                    return Err(format!("copper clearance violation between {left} and {right} near ({:.3}, {:.3}) mm: {actual:.3} mm < required {:.3} mm",board_location.x,board_location.y,settings.trace_clearance));
                }
            }
        }
        let all: PolyShapes = groups.into_iter().flat_map(|group| group.2).collect();
        boolean_shapes(&all, &board_outer, OverlayRule::Intersect)?
    };
    reject_point_tangencies(&copper_region)?;
    let (board_region, board_top_region, copper_region) =
        partition_shapes(&board_region, &copper_region)?;
    Ok((board_region, copper_region, board_top_region))
}

/// Generate a binary STL from a parsed board and explicit manufacturing settings.
pub fn generate_stl(board: &Board, settings: &Settings) -> Result<Vec<u8>, String> {
    if !settings.board_thickness.is_finite() || settings.board_thickness <= 0.0 {
        return Err("board_thickness must be positive".into());
    }
    if !settings.trace_height.is_finite() || settings.trace_height <= 0.0 {
        return Err("trace_height must be positive".into());
    }
    if !settings.trace_width.is_finite() || settings.trace_width <= 0.0 {
        return Err("trace_width must be positive".into());
    }
    if !matches!(settings.width_mode.as_str(), "auto" | "preserve") {
        return Err("width_mode must be 'auto' or 'preserve'".into());
    }
    if !matches!(
        settings.trace_style.as_str(),
        "technical" | "soft" | "vintage"
    ) {
        return Err("trace_style must be 'technical', 'soft', or 'vintage'".into());
    }
    if !settings.neckdown_width.is_finite() || settings.neckdown_width <= 0.0 {
        return Err("neckdown_width must be positive".into());
    }
    if !settings.taper_length.is_finite() || settings.taper_length <= 0.0 {
        return Err("taper_length must be positive".into());
    }
    if !settings.corner_radius.is_finite() || settings.corner_radius <= 0.0 {
        return Err("corner_radius must be positive".into());
    }
    if !settings.teardrop_length.is_finite() || settings.teardrop_length <= 0.0 {
        return Err("teardrop_length must be positive".into());
    }
    if !settings.teardrop_strength.is_finite() || !(0.0..=1.0).contains(&settings.teardrop_strength)
    {
        return Err("teardrop_strength must be between 0 and 1".into());
    }
    if !settings.trace_clearance.is_finite() || settings.trace_clearance < 0.0 {
        return Err("trace_clearance must be nonnegative".into());
    }
    if !settings.hole_compensation.is_finite() {
        return Err("hole_compensation must be finite".into());
    }
    let mut outline: Vec<_> = board
        .outline
        .iter()
        .copied()
        .map(|p| map_point(p, board))
        .collect();
    if polygon_area(&outline) < 0.0 {
        outline.reverse();
    }
    let drills = collect_drills(board, settings.hole_compensation)?;
    for drill in &drills {
        if !point_in_polygon(drill.center, &outline)
            || polygon_clearance(drill.center, &outline) < drill.radius - 1e-6
        {
            return Err("a compensated drill lies outside or intersects the board outline".into());
        }
    }
    let (board_region, copper_region, board_top_region) =
        build_regions(board, settings, outline, &drills)?;
    let partition = node_partition(&[&board_top_region, &copper_region, &board_region]);
    let (board_top_region, copper_region, board_region) =
        (&partition[0], &partition[1], &partition[2]);
    let mut mesh = Mesh::new();
    // Q (board minus drills) is a single solid below the raised copper. Emit
    // one bottom and one exterior wall from Q; P (plain board top) and R
    // (raised copper) only partition its stepped top. Noding all three regions
    // preserves every transition vertex where R meets the board boundary.
    mesh.horizontal(board_region, 0.0, false)?;
    mesh.vertical(board_region, 0.0, settings.board_thickness);
    mesh.horizontal(board_top_region, settings.board_thickness, true)?;
    if !copper_region.is_empty() {
        mesh.vertical(
            copper_region,
            settings.board_thickness,
            settings.board_thickness + settings.trace_height,
        );
        mesh.horizontal(
            copper_region,
            settings.board_thickness + settings.trace_height,
            true,
        )?;
    }
    mesh.validate_closed_manifold()?;
    Ok(mesh.binary_stl())
}

#[cfg(target_arch = "wasm32")]
mod wasm_abi {
    use super::*;
    use std::{mem, slice, sync::Mutex};

    static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

    fn return_bytes(bytes: Vec<u8>) -> u64 {
        let boxed = bytes.into_boxed_slice();
        let len = boxed.len() as u32;
        let ptr = Box::into_raw(boxed) as *mut u8 as u32;
        ((ptr as u64) << 32) | len as u64
    }

    unsafe fn input<'a>(ptr: u32, len: u32) -> &'a [u8] {
        slice::from_raw_parts(ptr as *const u8, len as usize)
    }

    #[no_mangle]
    pub extern "C" fn alloc(len: u32) -> u32 {
        let boxed = vec![0u8; len as usize].into_boxed_slice();
        Box::into_raw(boxed) as *mut u8 as u32
    }

    #[no_mangle]
    pub unsafe extern "C" fn dealloc(ptr: u32, len: u32) {
        if ptr != 0 && len != 0 {
            let raw = slice::from_raw_parts_mut(ptr as *mut u8, len as usize);
            mem::drop(Box::from_raw(raw));
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn parse_kicad(ptr: u32, len: u32) -> u64 {
        let result = std::str::from_utf8(input(ptr, len))
            .map_err(|e| e.to_string())
            .and_then(super::parse_kicad);
        let json = match result {
            Ok(board) => serde_json::to_vec(&board)
                .unwrap_or_else(|e| format!("{{\"error\":{:?}}}", e.to_string()).into_bytes()),
            Err(error) => serde_json::to_vec(&serde_json::json!({"error":error})).unwrap(),
        };
        return_bytes(json)
    }

    #[no_mangle]
    pub unsafe extern "C" fn generate_stl(
        board_ptr: u32,
        board_len: u32,
        settings_ptr: u32,
        settings_len: u32,
    ) -> u64 {
        let result = (|| {
            let board: Board = serde_json::from_slice(input(board_ptr, board_len))
                .map_err(|e| format!("invalid board JSON: {e}"))?;
            let settings: Settings = serde_json::from_slice(input(settings_ptr, settings_len))
                .map_err(|e| format!("invalid settings JSON: {e}"))?;
            super::generate_stl(&board, &settings)
        })();
        match result {
            Ok(bytes) => return_bytes(bytes),
            Err(error) => {
                *LAST_ERROR.lock().unwrap() = Some(error);
                0
            }
        }
    }

    #[no_mangle]
    pub extern "C" fn last_error() -> u64 {
        LAST_ERROR
            .lock()
            .unwrap()
            .take()
            .map(|s| return_bytes(s.into_bytes()))
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    const FIXTURE: &str = include_str!("../tests/fixtures/representative.kicad_pcb");

    #[test]
    fn parses_outline_tracks_vias_and_rotated_pads() {
        let board = parse_kicad(FIXTURE).unwrap();
        assert_eq!(board.name, "Raised Trace Demo");
        assert_eq!(board.outline.len(), 4);
        assert_eq!(
            board.stats,
            Stats {
                traces: 2,
                pads: 3,
                vias: 1,
                holes: 3
            }
        );
        assert_eq!(board.bounds.width, 40.0);
        assert_eq!(board.traces[1].net_id, Some(2));
        assert_eq!(board.traces[1].net_name.as_deref(), Some("SIG"));
        assert_eq!(board.pads[0].net_id, Some(2));
        assert_eq!(board.vias[0].net_name.as_deref(), Some("SIG"));
        let rotated = &board.pads[1];
        assert!((rotated.position.x - 20.0).abs() < 1e-6);
        assert!((rotated.position.y - 8.0).abs() < 1e-6);
        assert!(
            board.zones.is_empty(),
            "front-copper zones are intentionally ignored"
        );
        assert!(board
            .warnings
            .iter()
            .any(|w| w.code == "UNSUPPORTED_SMD_PAD"));
    }

    #[test]
    fn imports_exact_back_zone_fills_and_distinguishes_keepouts_and_teardrops() {
        let source = r#"(kicad_pcb (version 20240108)
          (net 1 "GND")
          (gr_rect (start 0 0) (end 20 20) (layer "Edge.Cuts"))
          (zone (net 1) (net_name "GND") (layer "B.Cu") (name "plane")
            (hatch edge 0.5)
            (polygon (pts (xy 4 4) (xy 16 4) (xy 16 10) (xy 4 10)))
            (filled_polygon (layer "B.Cu")
              (pts (xy 4 4) (xy 16 4) (xy 16 10) (xy 4 10))))
          (zone (net 0) (net_name "") (layer "B.Cu")
            (hatch edge 0.5) (keepout (tracks not_allowed) (copperpour not_allowed))
            (polygon (pts (xy 1 1) (xy 2 1) (xy 2 2) (xy 1 2))))
          (zone (net 1) (net_name "GND") (layer "F.Cu")
            (filled_polygon (layer "F.Cu")
              (pts (xy 1 1) (xy 3 1) (xy 3 3) (xy 1 3))))
          (zone (net 1) (net_name "GND") (layer "B.Cu") (name "$teardrop_padvia$")
            (attr (teardrop (type padvia)))
            (filled_polygon (layer "B.Cu")
              (pts (xy 17 17) (xy 18 17) (xy 18 18) (xy 17 18)))))"#;
        let board = parse_kicad(source).unwrap();
        assert_eq!(board.zones.len(), 3);
        assert_eq!(board.zones[0].kind, "copper");
        assert_eq!(board.zones[0].polygons.len(), 1);
        assert_eq!(board.zones[1].kind, "keepout");
        assert_eq!(board.zones[2].kind, "teardrop");
        assert!(!board
            .warnings
            .iter()
            .any(|warning| warning.code == "UNFILLED_BCU_ZONE"));

        let mut preserve_authored = standard_settings();
        preserve_authored.trace_style = "technical".into();
        let stl = generate_stl(&board, &preserve_authored).unwrap();
        assert!((top_area(&stl, 2.0) - 73.0).abs() < 0.05);

        let mut generated_vintage = preserve_authored;
        generated_vintage.trace_style = "vintage".into();
        let stl = generate_stl(&board, &generated_vintage).unwrap();
        assert!((top_area(&stl, 2.0) - 72.0).abs() < 0.05);
    }

    #[test]
    fn unfilled_back_zone_is_an_explicit_export_blocker() {
        let source = r#"(kicad_pcb (version 20240108)
          (net 1 "GND")
          (gr_rect (start 0 0) (end 20 20) (layer "Edge.Cuts"))
          (zone (net 1) (net_name "GND") (layer "B.Cu")
            (hatch edge 0.5)
            (polygon (pts (xy 4 4) (xy 16 4) (xy 16 10) (xy 4 10)))))"#;
        let board = parse_kicad(source).unwrap();
        let warning = board
            .warnings
            .iter()
            .find(|warning| warning.code == "UNFILLED_BCU_ZONE")
            .expect("missing unfilled-zone warning");
        assert_eq!(warning.severity, Severity::Error);
        assert!(generate_stl(&board, &standard_settings())
            .unwrap_err()
            .contains("refill zones in KiCad"));
    }

    #[test]
    fn bottom_footprint_transform_warning_does_not_block_export() {
        let source = r#"(kicad_pcb (version 20240108)
          (net 1 "SIG")
          (gr_rect (start 0 0) (end 20 20) (layer "Edge.Cuts"))
          (segment (start 4 10) (end 16 10) (width 1) (layer "B.Cu") (net 1))
          (footprint "Legacy:Back" (layer "B.Cu") (at 4 10)
            (pad "1" thru_hole circle (at 0 0) (size 3 3) (drill 1)
              (layers "*.Cu") (net 1 "SIG"))))"#;
        let board = parse_kicad(source).unwrap();
        let issue = board
            .warnings
            .iter()
            .find(|warning| warning.code == "BOTTOM_FOOTPRINT_TRANSFORM_APPROXIMATED")
            .expect("missing bottom-footprint warning");

        assert_eq!(issue.severity, Severity::Warning);
        assert!(generate_stl(&board, &standard_settings()).is_ok());
    }

    #[test]
    fn fractured_filled_zone_preserves_an_internal_void() {
        let source = r#"(kicad_pcb (version 20240108)
          (net 1 "GND")
          (gr_rect (start 0 0) (end 20 20) (layer "Edge.Cuts"))
          (zone (net 1) (net_name "GND") (layer "B.Cu")
            (filled_polygon (layer "B.Cu") (pts
              (xy 2 2) (xy 18 2) (xy 18 18) (xy 2 18)
              (xy 2 10) (xy 8 10) (xy 8 12) (xy 12 12)
              (xy 12 8) (xy 8 8) (xy 8 10) (xy 2 10)))))"#;
        let board = parse_kicad(source).unwrap();
        let stl = generate_stl(&board, &standard_settings()).unwrap();
        assert!((top_area(&stl, 2.0) - 240.0).abs() < 0.05);
        let triangles = stl_triangles(&stl);
        assert!(!triangles.iter().any(|triangle| {
            triangle.iter().all(|vertex| (vertex[2] - 2.0).abs() < 1e-5)
                && point_in_triangle((10.0, 10.0), *triangle)
        }));
    }

    #[test]
    fn tessellates_routed_copper_arcs_without_dropping_their_net() {
        let source = r#"(kicad_pcb (version 20240108)
          (net 1 "ARC_NET")
          (gr_rect (start 0 0) (end 20 20) (layer "Edge.Cuts"))
          (arc (start 5 10) (mid 10 5) (end 15 10)
            (width 0.6) (layer "B.Cu") (net 1)))"#;
        let board = parse_kicad(source).unwrap();
        assert!(board.traces.len() >= 6);
        assert_eq!(
            board.traces.first().unwrap().start,
            Point { x: 5.0, y: 10.0 }
        );
        assert_eq!(board.traces.last().unwrap().end, Point { x: 15.0, y: 10.0 });
        assert!(board.traces.iter().all(|trace| trace.net_id == Some(1)));
        assert!(board
            .traces
            .iter()
            .all(|trace| trace.net_name.as_deref() == Some("ARC_NET")));
        assert!(!board
            .warnings
            .iter()
            .any(|warning| warning.code == "UNSUPPORTED_COPPER_ARC"));
    }

    #[test]
    fn creates_binary_stl_with_base_holes_and_raised_copper() {
        let board = parse_kicad(FIXTURE).unwrap();
        let settings = Settings {
            board_thickness: 1.6,
            trace_height: 0.35,
            trace_width: 2.4,
            hole_compensation: 0.15,
            side: "F.Cu".into(),
            ..Settings::default()
        };
        let stl = generate_stl(&board, &settings).unwrap();
        assert!(stl.len() > 84);
        let triangles = u32::from_le_bytes(stl[80..84].try_into().unwrap()) as usize;
        assert!(triangles > 100);
        assert_eq!(stl.len(), 84 + triangles * 50);
        assert!(stl[84..]
            .chunks_exact(50)
            .all(|triangle| triangle[..48].iter().any(|b| *b != 0)));
    }

    fn stl_triangles(stl: &[u8]) -> Vec<[[f32; 3]; 3]> {
        stl[84..]
            .chunks_exact(50)
            .map(|record| {
                let f = |offset| f32::from_le_bytes(record[offset..offset + 4].try_into().unwrap());
                [
                    [f(12), f(16), f(20)],
                    [f(24), f(28), f(32)],
                    [f(36), f(40), f(44)],
                ]
            })
            .collect()
    }

    type VertexKey = [u32; 3];

    fn vertex_key(vertex: [f32; 3]) -> VertexKey {
        vertex.map(f32::to_bits)
    }

    fn assert_closed_manifold(stl: &[u8], allowed_z: &[f32]) {
        let triangles = stl_triangles(stl);
        let mut directed_edges = HashMap::<(VertexKey, VertexKey), usize>::new();
        let mut undirected_edges = HashMap::<(VertexKey, VertexKey), usize>::new();
        let mut faces = HashSet::<[VertexKey; 3]>::new();
        let mut signed_volume = 0.0f64;
        for triangle in triangles {
            assert!(triangle.iter().flatten().all(|value| value.is_finite()));
            assert!(triangle
                .iter()
                .all(|vertex| allowed_z.iter().any(|z| (vertex[2] - z).abs() < 1e-5)));
            let grid_keys = triangle.map(|vertex| {
                vertex.map(|coordinate| (f64::from(coordinate) * OVERLAY_SCALE).round() as i64)
            });
            assert!(
                grid_keys[0] != grid_keys[1]
                    && grid_keys[1] != grid_keys[2]
                    && grid_keys[2] != grid_keys[0],
                "collapsed triangle {triangle:?}"
            );
            let ab = [
                grid_keys[1][0] as i128 - grid_keys[0][0] as i128,
                grid_keys[1][1] as i128 - grid_keys[0][1] as i128,
                grid_keys[1][2] as i128 - grid_keys[0][2] as i128,
            ];
            let ac = [
                grid_keys[2][0] as i128 - grid_keys[0][0] as i128,
                grid_keys[2][1] as i128 - grid_keys[0][1] as i128,
                grid_keys[2][2] as i128 - grid_keys[0][2] as i128,
            ];
            let cross = [
                ab[1] * ac[2] - ab[2] * ac[1],
                ab[2] * ac[0] - ab[0] * ac[2],
                ab[0] * ac[1] - ab[1] * ac[0],
            ];
            assert!(cross != [0, 0, 0], "degenerate triangle {triangle:?}");
            let keys = triangle.map(vertex_key);
            let mut face = keys;
            face.sort();
            assert!(faces.insert(face), "duplicate STL triangle");
            for (a, b) in [(keys[0], keys[1]), (keys[1], keys[2]), (keys[2], keys[0])] {
                *directed_edges.entry((a, b)).or_default() += 1;
                let edge = if a <= b { (a, b) } else { (b, a) };
                *undirected_edges.entry(edge).or_default() += 1;
            }
            signed_volume += (triangle[0][0] as f64
                * (triangle[1][1] as f64 * triangle[2][2] as f64
                    - triangle[1][2] as f64 * triangle[2][1] as f64)
                - triangle[0][1] as f64
                    * (triangle[1][0] as f64 * triangle[2][2] as f64
                        - triangle[1][2] as f64 * triangle[2][0] as f64)
                + triangle[0][2] as f64
                    * (triangle[1][0] as f64 * triangle[2][1] as f64
                        - triangle[1][1] as f64 * triangle[2][0] as f64))
                / 6.0;
        }
        for (&(a, b), &count) in &undirected_edges {
            assert_eq!(count, 2, "non-manifold edge {a:?}--{b:?}");
            assert_eq!(directed_edges.get(&(a, b)).copied().unwrap_or(0), 1);
            assert_eq!(directed_edges.get(&(b, a)).copied().unwrap_or(0), 1);
        }
        assert!(signed_volume > 0.0, "mesh has non-positive signed volume");
    }

    fn assert_closed_manifold_after_weld(stl: &[u8], tolerance: f64) {
        type WeldedKey = [i64; 3];
        let key = |vertex: [f32; 3]| {
            vertex.map(|coordinate| (f64::from(coordinate) / tolerance).round() as i64)
        };
        let mut directed_edges = HashMap::<(WeldedKey, WeldedKey), usize>::new();
        let mut undirected_edges = HashMap::<(WeldedKey, WeldedKey), usize>::new();
        let mut faces = HashSet::<[WeldedKey; 3]>::new();
        for triangle in stl_triangles(stl) {
            let keys = triangle.map(key);
            assert!(
                keys[0] != keys[1] && keys[1] != keys[2] && keys[2] != keys[0],
                "triangle collapsed after a {tolerance} mm weld: {triangle:?}"
            );
            let mut face = keys;
            face.sort();
            assert!(
                faces.insert(face),
                "duplicate triangle after a {tolerance} mm weld"
            );
            for (a, b) in [(keys[0], keys[1]), (keys[1], keys[2]), (keys[2], keys[0])] {
                *directed_edges.entry((a, b)).or_default() += 1;
                let edge = if a <= b { (a, b) } else { (b, a) };
                *undirected_edges.entry(edge).or_default() += 1;
            }
        }
        for (&(a, b), &count) in &undirected_edges {
            assert_eq!(
                count, 2,
                "non-manifold edge after a {tolerance} mm weld: {a:?}--{b:?}"
            );
            assert_eq!(directed_edges.get(&(a, b)).copied().unwrap_or(0), 1);
            assert_eq!(directed_edges.get(&(b, a)).copied().unwrap_or(0), 1);
        }
    }

    fn point_in_triangle(point: (f32, f32), tri: [[f32; 3]; 3]) -> bool {
        let sign = |(px, py): (f32, f32), a: [f32; 3], b: [f32; 3]| {
            (px - b[0]) * (a[1] - b[1]) - (a[0] - b[0]) * (py - b[1])
        };
        let d1 = sign(point, tri[0], tri[1]);
        let d2 = sign(point, tri[1], tri[2]);
        let d3 = sign(point, tri[2], tri[0]);
        !((d1 < -1e-5 || d2 < -1e-5 || d3 < -1e-5) && (d1 > 1e-5 || d2 > 1e-5 || d3 > 1e-5))
    }

    fn plain_board() -> Board {
        Board {
            name: "topology".into(),
            bounds: Bounds {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 30.0,
                max_y: 20.0,
                width: 30.0,
                height: 20.0,
            },
            outline: vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: 30.0, y: 0.0 },
                Point { x: 30.0, y: 20.0 },
                Point { x: 0.0, y: 20.0 },
            ],
            traces: vec![],
            pads: vec![],
            vias: vec![],
            zones: vec![],
            warnings: vec![],
            stats: Stats {
                traces: 0,
                pads: 0,
                vias: 0,
                holes: 0,
            },
        }
    }

    fn standard_settings() -> Settings {
        Settings {
            board_thickness: 1.6,
            trace_height: 0.4,
            trace_width: 2.4,
            hole_compensation: 0.1,
            side: "B.Cu".into(),
            ..Settings::default()
        }
    }

    fn tht_pad(position: Point, size: Point, drill: Option<f64>, shape: &str) -> Pad {
        Pad {
            position,
            size,
            drill,
            pad_type: "thru_hole".into(),
            shape: shape.into(),
            rotation: 0.0,
            layers: vec!["*.Cu".into()],
            net_id: None,
            net_name: None,
        }
    }

    fn top_area(stl: &[u8], z: f32) -> f64 {
        stl_triangles(stl)
            .into_iter()
            .filter(|tri| tri.iter().all(|v| (v[2] - z).abs() < 1e-5))
            .map(|tri| {
                (((tri[1][0] - tri[0][0]) * (tri[2][1] - tri[0][1])
                    - (tri[1][1] - tri[0][1]) * (tri[2][0] - tri[0][0]))
                    .abs()
                    * 0.5) as f64
            })
            .sum()
    }

    fn vertical_span(polygon: &[Point], x: f64) -> f64 {
        let mut ys = Vec::new();
        for (&a, &b) in polygon
            .iter()
            .zip(polygon.iter().cycle().skip(1))
            .take(polygon.len())
        {
            if ((a.x <= x && b.x >= x) || (b.x <= x && a.x >= x)) && (b.x - a.x).abs() > 1e-9 {
                let t = (x - a.x) / (b.x - a.x);
                if (-1e-8..=1.0 + 1e-8).contains(&t) {
                    ys.push(a.y + t * (b.y - a.y));
                }
            }
        }
        ys.iter().copied().fold(f64::NEG_INFINITY, f64::max)
            - ys.iter().copied().fold(f64::INFINITY, f64::min)
    }

    fn net_trace(start: Point, end: Point, width: f64, id: i64, name: &str) -> Trace {
        Trace {
            start,
            end,
            width,
            layer: "B.Cu".into(),
            net_id: Some(id),
            net_name: Some(name.into()),
        }
    }

    fn net_pad(position: Point, id: i64, name: &str) -> Pad {
        let mut pad = tht_pad(position, Point { x: 1.8, y: 1.8 }, Some(0.8), "circle");
        pad.net_id = Some(id);
        pad.net_name = Some(name.into());
        pad
    }

    #[test]
    fn mirrored_rotated_pad_uses_reflected_angle_for_taper_exit() {
        let mut board = plain_board();
        let angle = 10.0f64.to_radians();
        let start = Point { x: 15.0, y: 10.0 };
        let end = Point {
            x: start.x + angle.cos() * 8.0,
            y: start.y + angle.sin() * 8.0,
        };
        let trace = net_trace(start, end, 2.0, 1, "SIG");
        let mut pad = net_pad(start, 1, "SIG");
        pad.size = Point { x: 6.0, y: 2.0 };
        pad.shape = "rect".into();
        pad.rotation = 30.0;
        board.traces.push(trace.clone());
        board.pads.push(pad.clone());

        let mapped_start = map_point(start, &board);
        let mapped_end = map_point(end, &board);
        let length = distance(mapped_start, mapped_end);
        let direction = Point {
            x: (mapped_end.x - mapped_start.x) / length,
            y: (mapped_end.y - mapped_start.y) / length,
        };
        let mut reflected = pad.clone();
        reflected.position = map_point(pad.position, &board);
        reflected.rotation = -pad.rotation;
        let expected =
            ray_exit_distance(mapped_start, direction, &pad_polygon(&reflected)).unwrap();
        let actual = matching_pad_exit(&board, &trace, mapped_start, direction).unwrap();
        assert!((actual - expected).abs() < 1e-9);

        let mut unreflected = reflected;
        unreflected.rotation = pad.rotation;
        let wrong = ray_exit_distance(mapped_start, direction, &pad_polygon(&unreflected)).unwrap();
        assert!((actual - wrong).abs() > 0.1);
    }

    #[test]
    fn identical_inputs_produce_identical_stl_bytes() {
        let board = parse_kicad(FIXTURE).unwrap();
        let settings = standard_settings();
        assert_eq!(
            generate_stl(&board, &settings).unwrap(),
            generate_stl(&board, &settings).unwrap()
        );
    }

    #[test]
    fn connected_trace_and_annular_pad_never_cap_through_hole() {
        let board = parse_kicad(FIXTURE).unwrap();
        let settings = Settings {
            board_thickness: 1.6,
            trace_height: 0.35,
            trace_width: 2.4,
            hole_compensation: 0.15,
            side: "front".into(),
            ..Settings::default()
        };
        let triangles = stl_triangles(&generate_stl(&board, &settings).unwrap());
        // Pad 1 is at board (20,10), mirrored B.Cu output is local (30,5).
        let center = (30.0, 5.0);
        assert!(
            !triangles.iter().any(|tri| {
                let horizontal =
                    (tri[0][2] - tri[1][2]).abs() < 1e-5 && (tri[1][2] - tri[2][2]).abs() < 1e-5;
                horizontal && point_in_triangle(center, *tri)
            }),
            "a horizontal STL facet caps the through-hole axis"
        );
        assert_closed_manifold(&generate_stl(&board, &settings).unwrap(), &[0.0, 1.6, 1.95]);
    }

    #[test]
    fn trace_width_is_a_floor_and_side_setting_cannot_enable_front_copper() {
        let make_board = |width, layer: &str| Board {
            name: "width".into(),
            bounds: Bounds {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 20.0,
                max_y: 10.0,
                width: 20.0,
                height: 10.0,
            },
            outline: vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: 20.0, y: 0.0 },
                Point { x: 20.0, y: 10.0 },
                Point { x: 0.0, y: 10.0 },
            ],
            traces: vec![Trace {
                start: Point { x: 5.0, y: 5.0 },
                end: Point { x: 15.0, y: 5.0 },
                width,
                layer: layer.into(),
                net_id: None,
                net_name: None,
            }],
            pads: vec![],
            vias: vec![],
            zones: vec![],
            warnings: vec![],
            stats: Stats {
                traces: 1,
                pads: 0,
                vias: 0,
                holes: 0,
            },
        };
        let settings = Settings {
            board_thickness: 1.6,
            trace_height: 0.4,
            trace_width: 2.4,
            hole_compensation: 0.0,
            side: "F.Cu".into(),
            ..Settings::default()
        };
        let raised_span = |board: &Board| {
            let triangles = stl_triangles(&generate_stl(board, &settings).unwrap());
            let ys: Vec<_> = triangles
                .iter()
                .flatten()
                .filter(|v| v[2] > 1.6 + 1e-5)
                .map(|v| v[1])
                .collect();
            ys.iter().copied().fold(f32::NEG_INFINITY, f32::max)
                - ys.iter().copied().fold(f32::INFINITY, f32::min)
        };
        assert!((raised_span(&make_board(0.5, "B.Cu")) - 2.4).abs() < 0.01);
        assert!((raised_span(&make_board(3.2, "B.Cu")) - 3.2).abs() < 0.01);
        assert!(
            !raised_span(&make_board(0.5, "F.Cu")).is_finite(),
            "F.Cu must not be emitted in back-copper mode"
        );
    }

    #[test]
    fn board_json_matches_ui_contract() {
        let value = serde_json::to_value(parse_kicad(FIXTURE).unwrap()).unwrap();
        for key in [
            "name", "bounds", "outline", "traces", "pads", "vias", "zones", "warnings", "stats",
        ] {
            assert!(value.get(key).is_some(), "missing {key}");
        }
        assert!(value["bounds"].get("min_x").is_some());
        assert!(value["traces"][0].get("start").is_some());
        assert_eq!(value["pads"][0]["pad_type"], "thru_hole");
    }

    #[test]
    fn npth_is_drilled_without_raised_copper_and_side_defaults_to_back() {
        let source = r#"(kicad_pcb (version 20240108)
          (gr_rect (start 0 0) (end 20 10) (layer "Edge.Cuts"))
          (footprint "MountingHole" (layer "B.Cu") (at 5 5)
            (pad "" np_thru_hole circle (at 0 0) (size 3 3) (drill 3) (layers "*.Cu" "*.Mask"))
            (pad "1" smd custom (at 5 0) (size 2 2) (layers "B.Cu"))))"#;
        let board = parse_kicad(source).unwrap();
        assert_eq!(board.pads[0].pad_type, "np_thru_hole");
        assert!(board
            .warnings
            .iter()
            .any(|w| w.code == "UNSUPPORTED_CUSTOM_PAD"));
        assert!(board
            .warnings
            .iter()
            .any(|w| w.code == "UNSUPPORTED_SMD_PAD"));
        let settings: Settings = serde_json::from_str(
            r#"{"board_thickness":1.6,"trace_height":0.35,"trace_width":2.4,"hole_compensation":0.0}"#,
        )
        .unwrap();
        assert_eq!(settings.side, "B.Cu");
        assert_eq!(settings.width_mode, "auto");
        assert_eq!(settings.trace_style, "technical");
        assert_eq!(settings.neckdown_width, 1.4);
        assert_eq!(settings.taper_length, 4.0);
        assert_eq!(settings.corner_radius, 3.0);
        assert_eq!(settings.teardrop_length, 3.0);
        assert_eq!(settings.teardrop_strength, 0.75);
        assert_eq!(settings.trace_clearance, 0.5);
        let triangles = stl_triangles(&generate_stl(&board, &settings).unwrap());
        assert!(triangles
            .iter()
            .flatten()
            .all(|vertex| vertex[2] <= 1.6 + 1e-5));
        // Board-space (5,5) mirrors to local output (15,5).
        assert!(!triangles.iter().any(|tri| {
            let horizontal =
                (tri[0][2] - tri[1][2]).abs() < 1e-5 && (tri[1][2] - tri[2][2]).abs() < 1e-5;
            horizontal && point_in_triangle((15.0, 5.0), *tri)
        }));
    }

    #[test]
    fn unified_mesh_handles_overlaps_islands_drills_and_partial_edge_clips() {
        let mut board = plain_board();
        board.traces = vec![
            Trace {
                start: Point { x: 2.0, y: 10.0 },
                end: Point { x: 20.0, y: 10.0 },
                width: 0.5,
                layer: "B.Cu".into(),
                net_id: None,
                net_name: None,
            },
            Trace {
                start: Point { x: 10.0, y: 2.0 },
                end: Point { x: 10.0, y: 16.0 },
                width: 0.5,
                layer: "B.Cu".into(),
                net_id: None,
                net_name: None,
            },
            Trace {
                start: Point { x: -2.0, y: 4.0 },
                end: Point { x: 5.0, y: 4.0 },
                width: 0.5,
                layer: "B.Cu".into(),
                net_id: None,
                net_name: None,
            },
            Trace {
                start: Point { x: 22.0, y: 3.0 },
                end: Point { x: 27.0, y: 3.0 },
                width: 0.5,
                layer: "B.Cu".into(),
                net_id: None,
                net_name: None,
            },
        ];
        board.pads = vec![
            tht_pad(
                Point { x: 10.0, y: 10.0 },
                Point { x: 4.0, y: 4.0 },
                Some(1.0),
                "circle",
            ),
            Pad {
                position: Point { x: 25.0, y: 15.0 },
                size: Point { x: 3.0, y: 3.0 },
                drill: Some(2.0),
                pad_type: "np_thru_hole".into(),
                shape: "circle".into(),
                rotation: 0.0,
                layers: vec!["*.Cu".into()],
                net_id: None,
                net_name: None,
            },
        ];
        board.vias = vec![Via {
            position: Point { x: 20.0, y: 10.0 },
            size: 3.0,
            drill: 1.0,
            layers: vec!["F.Cu".into(), "B.Cu".into()],
            net_id: None,
            net_name: None,
        }];
        let settings = standard_settings();
        let stl = generate_stl(&board, &settings).unwrap();
        assert_closed_manifold(&stl, &[0.0, 1.6, 2.0]);
        for center in [(20.0, 10.0), (10.0, 10.0), (5.0, 15.0)] {
            assert!(
                !stl_triangles(&stl).iter().any(|tri| {
                    let horizontal = (tri[0][2] - tri[1][2]).abs() < 1e-5
                        && (tri[1][2] - tri[2][2]).abs() < 1e-5;
                    horizontal && point_in_triangle(center, *tri)
                }),
                "drill axis {center:?} was capped"
            );
        }
        let raw_area: f64 = board
            .traces
            .iter()
            .map(|trace| {
                polygon_area(&capsule(
                    map_point(trace.start, &board),
                    map_point(trace.end, &board),
                    trace.width.max(settings.trace_width),
                    16,
                ))
                .abs()
            })
            .sum::<f64>()
            + polygon_area(&circle(
                map_point(Point { x: 10.0, y: 10.0 }, &board),
                2.0,
                24,
            ))
            .abs()
            + polygon_area(&circle(
                map_point(Point { x: 20.0, y: 10.0 }, &board),
                1.5,
                24,
            ))
            .abs();
        assert!(
            top_area(&stl, 2.0) < raw_area - 1.0,
            "overlap area was emitted more than once"
        );
    }

    #[test]
    fn shared_partition_graph_nodes_boundary_transitions() {
        let board: PolyShapes = vec![vec![vec![[0.0, 0.0], [2.0, 0.0], [2.0, 1.0], [0.0, 1.0]]]];
        let copper: PolyShapes = vec![vec![vec![
            [1.0, 0.0],
            [2.0, 0.0],
            [2.0, 1.0],
            [1.0, 1.0],
            [1.0, 0.5],
        ]]];
        let (board, plain, copper) = partition_shapes(&board, &copper).unwrap();
        let partition = node_partition(&[&plain, &copper, &board]);
        let (plain, copper, board) = (&partition[0], &partition[1], &partition[2]);
        let mut mesh = Mesh::new();
        mesh.horizontal(board, 0.0, false).unwrap();
        mesh.vertical(board, 0.0, 1.0);
        mesh.horizontal(plain, 1.0, true).unwrap();
        mesh.vertical(copper, 1.0, 2.0);
        mesh.horizontal(copper, 2.0, true).unwrap();
        mesh.validate_closed_manifold().unwrap();
        let stl = mesh.binary_stl();

        assert_closed_manifold(&stl, &[0.0, 1.0, 2.0]);
        assert_closed_manifold_after_weld(&stl, 1e-5);
    }

    #[test]
    fn mesh_validation_accepts_non_collinear_faces_at_grid_resolution() {
        let [a, b, c] = [
            Point { x: 0.0, y: 0.0 },
            Point {
                x: 1.0 / OVERLAY_SCALE,
                y: 0.0,
            },
            Point { x: 0.0, y: 0.05 },
        ];
        let (z0, z1) = (0.0, 1.0);
        let mut mesh = Mesh::new();
        mesh.triangle(v3(a, z0), v3(c, z0), v3(b, z0));
        mesh.triangle(v3(a, z1), v3(b, z1), v3(c, z1));
        for (start, end) in [(a, b), (b, c), (c, a)] {
            mesh.triangle(v3(start, z0), v3(end, z0), v3(end, z1));
            mesh.triangle(v3(start, z0), v3(end, z1), v3(start, z1));
        }

        mesh.validate_closed_manifold().unwrap();
        assert_closed_manifold_after_weld(&mesh.binary_stl(), 1.0 / OVERLAY_SCALE);
    }

    #[test]
    fn angled_edge_clip_is_manifold_after_slicer_precision_weld() {
        let mut board = plain_board();
        board.outline = vec![
            Point { x: 0.0, y: 0.0 },
            Point { x: 30.0, y: 0.0 },
            Point { x: 30.0, y: 12.0 },
            Point { x: 25.0, y: 20.0 },
            Point { x: 5.0, y: 20.0 },
            Point { x: 0.0, y: 13.0 },
        ];
        board.traces.push(net_trace(
            Point {
                x: 13.770551132038236,
                y: 10.79933114349842,
            },
            Point {
                x: 33.33822785876691,
                y: 16.126999682746828,
            },
            1.7006752873654478,
            1,
            "EDGE",
        ));
        let settings = Settings {
            trace_height: 0.95,
            width_mode: "preserve".into(),
            trace_style: "technical".into(),
            trace_clearance: 0.0,
            hole_compensation: 0.0,
            ..standard_settings()
        };
        let stl = generate_stl(&board, &settings).unwrap();

        assert_closed_manifold(&stl, &[0.0, 1.6, 2.55]);
        assert_closed_manifold_after_weld(&stl, 1e-5);
    }

    #[test]
    fn shallow_trace_and_zone_edge_clips_share_partition_vertices() {
        let mut board = plain_board();
        board.traces.push(net_trace(
            Point { x: 5.0, y: 0.00007 },
            Point {
                x: 35.0,
                y: 0.00007,
            },
            0.1,
            1,
            "EDGE",
        ));
        let mut settings = standard_settings();
        settings.width_mode = "preserve".into();
        settings.trace_clearance = 0.0;
        let stl = generate_stl(&board, &settings).unwrap();
        assert_closed_manifold(&stl, &[0.0, 1.6, 2.0]);
        assert_closed_manifold_after_weld(&stl, 1e-5);

        board.traces.clear();
        board.zones.push(CopperZone {
            layer: "B.Cu".into(),
            net_id: Some(1),
            net_name: Some("EDGE".into()),
            name: None,
            kind: "copper".into(),
            polygons: vec![vec![
                Point { x: -1.0, y: 20.0 },
                Point {
                    x: 0.1,
                    y: 20.00013,
                },
                Point { x: 31.0, y: 10.0 },
            ]],
        });
        let stl = generate_stl(&board, &settings).unwrap();
        assert_closed_manifold(&stl, &[0.0, 1.6, 2.0]);
        assert_closed_manifold_after_weld(&stl, 1e-5);

        let mut drilled = plain_board();
        let mut npth = tht_pad(
            Point { x: 15.0, y: 10.0 },
            Point { x: 2.0, y: 2.0 },
            Some(2.0),
            "circle",
        );
        npth.pad_type = "np_thru_hole".into();
        drilled.pads.push(npth);
        drilled.traces.push(net_trace(
            Point { x: 5.0, y: 10.0 },
            Point { x: 25.0, y: 10.0 },
            1.0,
            1,
            "EDGE",
        ));
        let mut drill_settings = settings;
        drill_settings.hole_compensation = 0.0;
        let stl = generate_stl(&drilled, &drill_settings).unwrap();
        assert_closed_manifold(&stl, &[0.0, 1.6, 2.0]);
        assert_closed_manifold_after_weld(&stl, 1e-5);
    }

    #[test]
    fn empty_and_whole_board_copper_are_manifold() {
        let board = plain_board();
        let settings = standard_settings();
        assert_closed_manifold(&generate_stl(&board, &settings).unwrap(), &[0.0, 1.6, 2.0]);

        let mut covered = plain_board();
        covered.pads.push(tht_pad(
            Point { x: 15.0, y: 10.0 },
            Point { x: 40.0, y: 30.0 },
            None,
            "rect",
        ));
        let stl = generate_stl(&covered, &settings).unwrap();
        assert_closed_manifold(&stl, &[0.0, 1.6, 2.0]);
        assert!((top_area(&stl, 2.0) - 600.0).abs() < 0.01);
        assert_eq!(top_area(&stl, 1.6), 0.0);
    }

    #[test]
    fn point_only_copper_tangency_and_consumed_annulus_are_rejected() {
        let mut tangent = plain_board();
        tangent.pads = vec![
            tht_pad(
                Point { x: 10.0, y: 10.0 },
                Point { x: 2.0, y: 2.0 },
                None,
                "circle",
            ),
            tht_pad(
                Point { x: 12.0, y: 10.0 },
                Point { x: 2.0, y: 2.0 },
                None,
                "circle",
            ),
        ];
        let error = generate_stl(&tangent, &standard_settings()).unwrap_err();
        assert!(error.contains("point-only tangency"));

        let mut consumed = plain_board();
        consumed.vias.push(Via {
            position: Point { x: 10.0, y: 10.0 },
            size: 1.0,
            drill: 1.0,
            layers: vec!["B.Cu".into()],
            net_id: None,
            net_name: None,
        });
        let error = generate_stl(&consumed, &standard_settings()).unwrap_err();
        assert!(error.contains("consumes its raised pad/via annulus"));
    }

    #[test]
    fn auto_neckdown_starts_at_pad_exit_and_preserve_keeps_source_width() {
        let mut board = plain_board();
        board
            .pads
            .push(net_pad(Point { x: 25.0, y: 10.0 }, 1, "ROW1"));
        let trace = net_trace(
            Point { x: 25.0, y: 10.0 },
            Point { x: 5.0, y: 10.0 },
            0.6,
            1,
            "ROW1",
        );
        board.traces.push(trace.clone());
        let settings = standard_settings();
        let polygon = trace_polygon(&board, &trace, &settings);
        assert!((vertical_span(&polygon, 5.0) - 1.4).abs() < 0.02);
        assert!((vertical_span(&polygon, 5.85) - 1.4).abs() < 0.03);
        assert!((vertical_span(&polygon, 9.0) - 2.18).abs() < 0.08);
        assert!((vertical_span(&polygon, 10.0) - 2.4).abs() < 0.03);

        let preserve = Settings {
            width_mode: "preserve".into(),
            ..settings.clone()
        };
        let polygon = trace_polygon(&board, &trace, &preserve);
        assert!((vertical_span(&polygon, 10.0) - 0.6).abs() < 0.02);
    }

    #[test]
    fn soft_tapers_and_vintage_pad_shoulders_are_curved_and_manifold() {
        let mut board = plain_board();
        board
            .pads
            .push(net_pad(Point { x: 25.0, y: 10.0 }, 1, "FLOW"));
        let trace = net_trace(
            Point { x: 25.0, y: 10.0 },
            Point { x: 5.0, y: 10.0 },
            0.6,
            1,
            "FLOW",
        );
        board.traces.push(trace.clone());

        let mut soft = standard_settings();
        soft.trace_style = "soft".into();
        let soft_polygon = trace_polygon(&board, &trace, &soft);
        // Quarterway through the ramp, quintic easing stays narrower than the
        // old linear envelope (1.65 mm at this position).
        assert!(vertical_span(&soft_polygon, 6.9) < 1.65);

        let mut vintage = soft;
        vintage.trace_style = "vintage".into();
        vintage.teardrop_strength = 1.0;
        let lobes =
            trace_teardrop_polygons_with_trim(&board, &trace, &vintage, TraceTrim::default());
        assert_eq!(lobes.len(), 1);
        let shoulder_span = vertical_span(&lobes[0], 6.2);
        let tip_span = vertical_span(&lobes[0], 8.7);
        assert!(
            shoulder_span > tip_span,
            "lobe widened from {shoulder_span} to {tip_span}"
        );
        assert_closed_manifold(&generate_stl(&board, &vintage).unwrap(), &[0.0, 1.6, 2.0]);
    }

    #[test]
    fn vintage_corner_blends_only_degree_two_nodes_and_stays_manifold() {
        let fillet = circular_fillet(
            Point { x: 15.0, y: 10.0 },
            Point { x: -1.0, y: 0.0 },
            Point { x: 0.0, y: 1.0 },
            3.0,
            PI / 2.0,
        )
        .unwrap();
        assert!((fillet.center.x - 12.0).abs() < 1e-12);
        assert!((fillet.center.y - 13.0).abs() < 1e-12);
        assert!((fillet.radius - 3.0).abs() < 1e-12);
        assert!((fillet.sweep - PI / 2.0).abs() < 1e-12);
        for index in 0..=12 {
            let angle = fillet.start_angle + fillet.sweep * index as f64 / 12.0;
            let point = Point {
                x: fillet.center.x + angle.cos() * fillet.radius,
                y: fillet.center.y + angle.sin() * fillet.radius,
            };
            assert!((distance(point, fillet.center) - 3.0).abs() < 1e-12);
        }
        let reversed_fillet = circular_fillet(
            Point { x: 15.0, y: 10.0 },
            Point { x: 0.0, y: 1.0 },
            Point { x: -1.0, y: 0.0 },
            3.0,
            PI / 2.0,
        )
        .unwrap();
        assert!((reversed_fillet.radius - fillet.radius).abs() < 1e-12);
        assert!((reversed_fillet.sweep + fillet.sweep).abs() < 1e-12);

        let mut board = plain_board();
        board.traces = vec![
            net_trace(
                Point { x: 9.0, y: 10.0 },
                Point { x: 15.0, y: 10.0 },
                0.6,
                1,
                "BEND",
            ),
            net_trace(
                Point { x: 15.0, y: 10.0 },
                Point { x: 15.0, y: 18.0 },
                0.6,
                1,
                "BEND",
            ),
        ];
        board
            .pads
            .push(net_pad(Point { x: 9.0, y: 10.0 }, 1, "BEND"));
        let mut settings = standard_settings();
        settings.trace_style = "vintage".into();
        let (trims, corners) = corner_geometry(&board, &settings);
        assert_eq!(corners.len(), 1);
        assert!(trims[0].end > 0.0);
        assert!(trims[1].start > 0.0);
        let expected_join_width = trace_source_width_at_distance(
            &board,
            &board.traces[0],
            &settings,
            distance(
                map_point(board.traces[0].start, &board),
                map_point(board.traces[0].end, &board),
            ) - trims[0].end,
        );
        let corner_join_width = distance(corners[0].polygon[0], corners[0].anchors[0]) * 2.0;
        assert!((corner_join_width - expected_join_width).abs() < 1e-9);
        assert!(corner_join_width < settings.trace_width - 0.3);
        let lobes =
            trace_teardrop_polygons_with_trim(&board, &board.traces[0], &settings, trims[0]);
        let source_start = map_point(board.traces[0].start, &board);
        let source_end = map_point(board.traces[0].end, &board);
        let source_length = distance(source_start, source_end);
        let source_direction = Point {
            x: (source_end.x - source_start.x) / source_length,
            y: (source_end.y - source_start.y) / source_length,
        };
        let maximum_lobe_distance = lobes
            .iter()
            .flatten()
            .map(|point| {
                (point.x - source_start.x) * source_direction.x
                    + (point.y - source_start.y) * source_direction.y
            })
            .fold(0.0, f64::max);
        assert!(maximum_lobe_distance <= source_length - trims[0].end + 1e-9);
        assert_closed_manifold(&generate_stl(&board, &settings).unwrap(), &[0.0, 1.6, 2.0]);

        board.traces.push(net_trace(
            Point { x: 15.0, y: 10.0 },
            Point { x: 25.0, y: 10.0 },
            0.6,
            1,
            "BEND",
        ));
        assert!(corner_geometry(&board, &settings).1.is_empty());

        board.traces = vec![
            net_trace(
                Point { x: 5.0, y: 10.0 },
                Point { x: 15.0, y: 10.0 },
                0.6,
                1,
                "SHARP",
            ),
            net_trace(
                Point { x: 15.0, y: 10.0 },
                Point { x: 5.0, y: 18.0 },
                0.6,
                1,
                "SHARP",
            ),
        ];
        assert!(corner_geometry(&board, &settings).1.is_empty());
    }

    #[test]
    fn vintage_corner_preserves_taper_width_slope_without_overshoot() {
        let mut board = plain_board();
        board.traces = vec![
            net_trace(
                Point { x: 2.0, y: 10.0 },
                Point { x: 13.0, y: 10.0 },
                0.6,
                1,
                "FLOW",
            ),
            net_trace(
                Point { x: 13.0, y: 10.0 },
                Point { x: 13.0, y: 18.0 },
                0.6,
                1,
                "FLOW",
            ),
        ];
        let mut source_pad = net_pad(Point { x: 2.0, y: 10.0 }, 1, "FLOW");
        source_pad.size = Point { x: 4.0, y: 4.0 };
        board.pads.push(source_pad);
        let mut settings = standard_settings();
        settings.trace_style = "vintage".into();
        settings.trace_width = 2.5;
        settings.taper_length = 6.0;
        settings.teardrop_strength = 0.55;
        settings.corner_radius = 3.0;

        let (trims, corners) = corner_geometry(&board, &settings);
        assert_eq!(corners.len(), 1);
        let first_length = distance(
            map_point(board.traces[0].start, &board),
            map_point(board.traces[0].end, &board),
        );
        let first_join = trace_source_width_and_slope_at_distance(
            &board,
            &board.traces[0],
            &settings,
            first_length - trims[0].end,
        );
        let second_join = trace_source_width_and_slope_at_distance(
            &board,
            &board.traces[1],
            &settings,
            trims[1].start,
        );
        let arc_length = trims[0].end * PI / 2.0;
        let step = 1.0 / 18.0;
        let chord_length = arc_length * step;
        let previous_width = trace_source_width_at_distance(
            &board,
            &board.traces[0],
            &settings,
            first_length - trims[0].end - chord_length,
        );
        let incoming_chord_slope = (first_join.0 - previous_width) / chord_length;
        let next_width = bounded_hermite_width(
            step,
            first_join.0,
            second_join.0,
            (first_join.1, second_join.1),
            arc_length,
            (settings.neckdown_width, settings.trace_width),
        );
        let first_chord_slope = (next_width - first_join.0) / chord_length;

        assert!(first_join.1 > 0.1);
        assert!(
            (incoming_chord_slope - first_chord_slope).abs() < 0.05,
            "width slope jumped from {} to {}",
            incoming_chord_slope,
            first_chord_slope
        );
        for index in 0..=18 {
            let t = index as f64 / 18.0;
            let width = bounded_hermite_width(
                t,
                first_join.0,
                second_join.0,
                (first_join.1, second_join.1),
                arc_length,
                (settings.neckdown_width, settings.trace_width),
            );
            assert!(
                (settings.neckdown_width - 1e-9..=settings.trace_width + 1e-9).contains(&width)
            );
            let reversed = bounded_hermite_width(
                1.0 - t,
                second_join.0,
                first_join.0,
                (-second_join.1, -first_join.1),
                arc_length,
                (settings.neckdown_width, settings.trace_width),
            );
            assert!((width - reversed).abs() < 1e-12);
        }
        assert_closed_manifold(&generate_stl(&board, &settings).unwrap(), &[0.0, 1.6, 2.0]);
    }

    #[test]
    fn short_two_ended_taper_and_dip_breakout_remain_manifold() {
        let mut short = plain_board();
        short.pads = vec![
            net_pad(Point { x: 25.0, y: 10.0 }, 1, "SHORT"),
            net_pad(Point { x: 19.0, y: 10.0 }, 1, "SHORT"),
        ];
        let trace = net_trace(
            Point { x: 25.0, y: 10.0 },
            Point { x: 19.0, y: 10.0 },
            0.5,
            1,
            "SHORT",
        );
        short.traces.push(trace.clone());
        let settings = standard_settings();
        let polygon = trace_polygon(&short, &trace, &settings);
        let middle = vertical_span(&polygon, 8.0);
        assert!(
            middle > 1.4 && middle < 2.4,
            "short dual taper width was {middle}"
        );
        assert_closed_manifold(&generate_stl(&short, &settings).unwrap(), &[0.0, 1.6, 2.0]);

        let mut dip = plain_board();
        dip.pads = vec![
            net_pad(Point { x: 25.0, y: 8.73 }, 1, "D0"),
            net_pad(Point { x: 25.0, y: 11.27 }, 2, "D1"),
        ];
        dip.traces = vec![
            net_trace(
                Point { x: 25.0, y: 8.73 },
                Point { x: 5.0, y: 4.0 },
                0.5,
                1,
                "D0",
            ),
            net_trace(
                Point { x: 25.0, y: 11.27 },
                Point { x: 5.0, y: 16.0 },
                0.5,
                2,
                "D1",
            ),
        ];
        assert_closed_manifold(&generate_stl(&dip, &settings).unwrap(), &[0.0, 1.6, 2.0]);
    }

    #[test]
    fn overlapping_vintage_tapers_have_a_flat_midpoint() {
        let polygon = variable_trace_polygon(
            Point { x: 0.0, y: 0.0 },
            Point { x: 20.0, y: 0.0 },
            VariableTraceProfile {
                trunk: 3.0,
                neck: 1.4,
                taper: 12.0,
                start_exit: Some(5.0),
                end_exit: Some(5.0),
                style: "vintage",
                teardrop_length: 3.0,
                start_shoulder: None,
                end_shoulder: None,
            },
        );
        let left = vertical_span(&polygon, 9.8);
        let middle = vertical_span(&polygon, 10.0);
        let right = vertical_span(&polygon, 10.2);

        assert!(middle >= left && middle >= right);
        assert!((left - right).abs() < 1e-9);
        assert!(middle - left < 0.01, "midpoint rise was {}", middle - left);
    }

    #[test]
    fn same_net_t_junction_is_legal_and_different_net_overlap_is_rejected() {
        let mut board = plain_board();
        board.traces = vec![
            net_trace(
                Point { x: 5.0, y: 10.0 },
                Point { x: 25.0, y: 10.0 },
                0.5,
                1,
                "GND",
            ),
            net_trace(
                Point { x: 15.0, y: 10.0 },
                Point { x: 15.0, y: 3.0 },
                0.5,
                1,
                "GND",
            ),
        ];
        board.vias.push(Via {
            position: Point { x: 15.0, y: 10.0 },
            size: 3.0,
            drill: 0.8,
            layers: vec!["F.Cu".into()],
            net_id: Some(2),
            net_name: Some("IGNORED_FRONT".into()),
        });
        let settings = standard_settings();
        assert_closed_manifold(&generate_stl(&board, &settings).unwrap(), &[0.0, 1.6, 2.0]);
        board.traces[1].net_id = Some(2);
        board.traces[1].net_name = Some("VCC".into());
        let error = generate_stl(&board, &settings).unwrap_err();
        assert!(error.contains("copper collision between"), "{error}");
        assert!(
            error.contains("net 1 (GND)") && error.contains("net 2 (VCC)"),
            "{error}"
        );
        assert!(error.contains("near ("));
    }

    #[test]
    fn zero_net_ids_use_legacy_coordinate_connectivity() {
        let mut board = plain_board();
        board.traces = vec![
            net_trace(
                Point { x: 5.0, y: 10.0 },
                Point { x: 25.0, y: 10.0 },
                0.5,
                0,
                "",
            ),
            net_trace(
                Point { x: 15.0, y: 10.0 },
                Point { x: 15.0, y: 3.0 },
                0.5,
                0,
                "",
            ),
        ];
        let settings = standard_settings();
        assert_closed_manifold(&generate_stl(&board, &settings).unwrap(), &[0.0, 1.6, 2.0]);

        board.traces[1].start = Point { x: 15.0, y: 16.0 };
        let error = generate_stl(&board, &settings).unwrap_err();
        assert!(error.contains("copper collision between"), "{error}");
    }
}

fn open_manifold_check(stl: &stl::Stl, thresholds: &[f64]) {
    let manifold = stl::check_manifold(stl);
    assert!(manifold.is_closed(), "STL is not a closed manifold");
    assert!(manifold.is_watertight(), "STL is not watertight");
    for &threshold in thresholds {
        assert!(manifold.max_gap() <= threshold, "Max gap exceeds threshold {}", threshold);
    }
}

fn assert_closed_manifold(stl: &stl::Stl, thresholds: &[f64]) {
    open_manifold_check(stl, thresholds);
}