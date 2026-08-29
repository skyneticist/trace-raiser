//! Syntax-preserving KiCad PCB placement adapter.
//!
//! The adapter intentionally edits only a footprint's immediate `(at ...)`
//! expression. KiCad remains the parser and DRC authority for emitted boards.

mod design;
mod geometry;
pub use design::{DesignBuildError, DesignBuildOptions};

use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::fmt;

/// Defensive parser limits for untrusted board files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseLimits {
    pub max_input_bytes: usize,
    pub max_depth: usize,
    pub max_nodes: usize,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 64 * 1024 * 1024,
            max_depth: 256,
            max_nodes: 2_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq)]
struct Atom {
    value: String,
    quoted: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum NodeKind {
    Atom(Atom),
    List(Vec<Node>),
}

#[derive(Debug, Clone, PartialEq)]
struct Node {
    span: Span,
    kind: NodeKind,
}

impl Node {
    fn atom(&self) -> Option<&str> {
        match &self.kind {
            NodeKind::Atom(atom) => Some(&atom.value),
            NodeKind::List(_) => None,
        }
    }

    fn list(&self) -> Option<&[Node]> {
        match &self.kind {
            NodeKind::List(nodes) => Some(nodes),
            NodeKind::Atom(_) => None,
        }
    }

    fn head(&self) -> Option<&str> {
        self.list()?.first()?.atom()
    }

    fn children<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a Node> {
        self.list()
            .into_iter()
            .flatten()
            .filter(move |node| node.head() == Some(tag))
    }

    fn child(&self, tag: &str) -> Option<&Node> {
        self.list()?.iter().find(|node| node.head() == Some(tag))
    }

    fn atom_at(&self, index: usize) -> Option<&str> {
        self.list()?.get(index)?.atom()
    }

    fn atom_at_is_quoted(&self, index: usize) -> Option<bool> {
        match &self.list()?.get(index)?.kind {
            NodeKind::Atom(atom) => Some(atom.quoted),
            NodeKind::List(_) => None,
        }
    }

    fn contains_immediate_atom(&self, wanted: &str) -> bool {
        self.list()
            .is_some_and(|nodes| nodes.iter().any(|node| node.atom() == Some(wanted)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at byte {}", self.message, self.offset)
    }
}

impl Error for ParseError {}

struct Parser<'a> {
    input: &'a str,
    bytes: &'a [u8],
    position: usize,
    nodes: usize,
    limits: ParseLimits,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str, limits: ParseLimits) -> Result<Self, ParseError> {
        if input.len() > limits.max_input_bytes {
            return Err(ParseError {
                offset: limits.max_input_bytes,
                message: format!(
                    "input exceeds the {} byte parser limit",
                    limits.max_input_bytes
                ),
            });
        }
        Ok(Self {
            input,
            bytes: input.as_bytes(),
            position: 0,
            nodes: 0,
            limits,
        })
    }

    fn parse(mut self) -> Result<Node, ParseError> {
        self.skip_trivia();
        let root = self.parse_node(0)?;
        self.skip_trivia();
        if self.position != self.bytes.len() {
            return self.fail("unexpected data after the root expression");
        }
        Ok(root)
    }

    fn parse_node(&mut self, depth: usize) -> Result<Node, ParseError> {
        if depth > self.limits.max_depth {
            return self.fail(format!(
                "S-expression nesting exceeds the depth limit of {}",
                self.limits.max_depth
            ));
        }
        self.nodes += 1;
        if self.nodes > self.limits.max_nodes {
            return self.fail(format!(
                "S-expression node count exceeds the limit of {}",
                self.limits.max_nodes
            ));
        }

        self.skip_trivia();
        match self.bytes.get(self.position).copied() {
            Some(b'(') => self.parse_list(depth),
            Some(b'"') => self.parse_quoted(),
            Some(b')') => self.fail("unexpected closing parenthesis"),
            Some(_) => self.parse_atom(),
            None => self.fail("unexpected end of input"),
        }
    }

    fn parse_list(&mut self, depth: usize) -> Result<Node, ParseError> {
        let start = self.position;
        self.position += 1;
        let mut nodes = Vec::new();
        loop {
            self.skip_trivia();
            match self.bytes.get(self.position).copied() {
                Some(b')') => {
                    self.position += 1;
                    return Ok(Node {
                        span: Span {
                            start,
                            end: self.position,
                        },
                        kind: NodeKind::List(nodes),
                    });
                }
                Some(_) => nodes.push(self.parse_node(depth + 1)?),
                None => return self.fail("unterminated list"),
            }
        }
    }

    fn parse_quoted(&mut self) -> Result<Node, ParseError> {
        let start = self.position;
        self.position += 1;
        let mut decoded = Vec::new();
        loop {
            let byte = self
                .bytes
                .get(self.position)
                .copied()
                .ok_or_else(|| ParseError {
                    offset: start,
                    message: "unterminated quoted string".into(),
                })?;
            self.position += 1;
            match byte {
                b'"' => {
                    let value = String::from_utf8(decoded).map_err(|_| ParseError {
                        offset: start,
                        message: "quoted string contains invalid UTF-8".into(),
                    })?;
                    return Ok(Node {
                        span: Span {
                            start,
                            end: self.position,
                        },
                        kind: NodeKind::Atom(Atom {
                            value,
                            quoted: true,
                        }),
                    });
                }
                b'\\' => {
                    let escaped =
                        self.bytes
                            .get(self.position)
                            .copied()
                            .ok_or_else(|| ParseError {
                                offset: self.position,
                                message: "unterminated escape sequence".into(),
                            })?;
                    self.position += 1;
                    decoded.push(match escaped {
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        other => other,
                    });
                }
                other => decoded.push(other),
            }
        }
    }

    fn parse_atom(&mut self) -> Result<Node, ParseError> {
        let start = self.position;
        while let Some(byte) = self.bytes.get(self.position).copied() {
            if byte.is_ascii_whitespace() || matches!(byte, b'(' | b')' | b';') {
                break;
            }
            self.position += 1;
        }
        if self.position == start {
            return self.fail("expected an atom");
        }
        let value = self.input[start..self.position].to_string();
        Ok(Node {
            span: Span {
                start,
                end: self.position,
            },
            kind: NodeKind::Atom(Atom {
                value,
                quoted: false,
            }),
        })
    }

    fn skip_trivia(&mut self) {
        loop {
            while self
                .bytes
                .get(self.position)
                .is_some_and(u8::is_ascii_whitespace)
            {
                self.position += 1;
            }
            if self.bytes.get(self.position) != Some(&b';') {
                return;
            }
            while self
                .bytes
                .get(self.position)
                .is_some_and(|byte| *byte != b'\n')
            {
                self.position += 1;
            }
        }
    }

    fn fail<T>(&self, message: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError {
            offset: self.position,
            message: message.into(),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum KicadError {
    Syntax(ParseError),
    Semantic { offset: usize, message: String },
    Patch(String),
}

impl fmt::Display for KicadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(error) => error.fmt(formatter),
            Self::Semantic { offset, message } => write!(formatter, "{message} at byte {offset}"),
            Self::Patch(message) => formatter.write_str(message),
        }
    }
}

impl Error for KicadError {}

impl From<ParseError> for KicadError {
    fn from(value: ParseError) -> Self {
        Self::Syntax(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalBounds {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub x: f64,
    pub y: f64,
    pub rotation_degrees: f64,
}

impl Pose {
    fn finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.rotation_degrees.is_finite()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NetRecord {
    pub ordinal: i64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PadRecord {
    pub number: String,
    pub kind: String,
    pub shape: String,
    pub local_pose: Pose,
    pub size: Point,
    pub drill: Option<f64>,
    pub layers: Vec<String>,
    pub net_ordinal: Option<i64>,
    pub net_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FootprintRecord {
    /// UUID when present, otherwise the reference designator.
    pub id: String,
    pub reference: String,
    pub value: Option<String>,
    pub library_link: Option<String>,
    pub uuid: Option<String>,
    pub layer: String,
    pub locked: bool,
    pub pose: Pose,
    pub pads: Vec<PadRecord>,
    /// Conservative local bounds derived from F.CrtYd centerline geometry.
    pub front_courtyard_envelope: Option<LocalBounds>,
    /// Conservative local bounds derived from B.CrtYd centerline geometry.
    pub back_courtyard_envelope: Option<LocalBounds>,
    at_span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EdgePrimitive {
    Rectangle {
        start: Point,
        end: Point,
        radius: f64,
    },
    Polygon {
        points: Vec<Point>,
    },
    Line {
        start: Point,
        end: Point,
    },
    Arc {
        start: Point,
        mid: Point,
        end: Point,
    },
    Circle {
        center: Point,
        end: Point,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PatchOptions {
    pub override_locked: bool,
}

/// Parsed board plus the untouched source used for localized placement patches.
#[derive(Debug, Clone)]
pub struct BoardDocument {
    source: String,
    pub nets: Vec<NetRecord>,
    pub footprints: Vec<FootprintRecord>,
    pub edge_cuts: Vec<EdgePrimitive>,
}

impl BoardDocument {
    pub fn parse(source: &str) -> Result<Self, KicadError> {
        Self::parse_with_limits(source, ParseLimits::default())
    }

    pub fn parse_with_limits(source: &str, limits: ParseLimits) -> Result<Self, KicadError> {
        let root = Parser::new(source, limits)?.parse()?;
        if root.head() != Some("kicad_pcb") {
            return Err(KicadError::Semantic {
                offset: root.span.start,
                message: "expected a kicad_pcb root expression".into(),
            });
        }

        let nets = root
            .children("net")
            .map(parse_net_record)
            .collect::<Result<Vec<_>, _>>()?;

        let mut footprints = Vec::new();
        let mut identities = HashSet::new();
        for node in root.children("footprint") {
            let footprint = parse_footprint(node)?;
            if !identities.insert(footprint.id.clone()) {
                return Err(KicadError::Semantic {
                    offset: node.span.start,
                    message: format!("duplicate footprint identity {}", footprint.id),
                });
            }
            footprints.push(footprint);
        }

        let mut edge_cuts = Vec::new();
        for node in root.list().into_iter().flatten() {
            if node.child("layer").and_then(|layer| layer.atom_at(1)) != Some("Edge.Cuts") {
                continue;
            }
            let Some(head) = node.head() else {
                continue;
            };
            if head.starts_with("gr_") {
                edge_cuts.push(parse_graphic_primitive(node, "gr", "Edge.Cuts")?);
            }
        }

        Ok(Self {
            source: source.to_string(),
            nets,
            footprints,
            edge_cuts,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Return the exact original document without applying changes.
    pub fn render_unchanged(&self) -> String {
        self.source.clone()
    }

    /// Apply localized footprint pose changes keyed by UUID or reference.
    pub fn rewrite_placements(
        &self,
        placements: &BTreeMap<String, Pose>,
        options: PatchOptions,
    ) -> Result<String, KicadError> {
        let mut patches = Vec::<(Span, String)>::new();
        let known: HashSet<&str> = self
            .footprints
            .iter()
            .flat_map(|footprint| {
                std::iter::once(footprint.id.as_str())
                    .chain(std::iter::once(footprint.reference.as_str()))
            })
            .collect();
        for requested in placements.keys() {
            if !known.contains(requested.as_str()) {
                return Err(KicadError::Patch(format!(
                    "placement request references unknown footprint {requested}"
                )));
            }
        }

        for footprint in &self.footprints {
            let requested = placements
                .get(&footprint.id)
                .or_else(|| placements.get(&footprint.reference));
            let Some(pose) = requested.copied() else {
                continue;
            };
            if !pose.finite() {
                return Err(KicadError::Patch(format!(
                    "placement for {} contains a non-finite coordinate",
                    footprint.reference
                )));
            }
            if footprint.locked && !options.override_locked && pose != footprint.pose {
                return Err(KicadError::Patch(format!(
                    "footprint {} is locked and cannot be moved",
                    footprint.reference
                )));
            }
            if pose == footprint.pose {
                continue;
            }
            patches.push((
                footprint.at_span,
                format!(
                    "(at {} {} {})",
                    format_number(pose.x),
                    format_number(pose.y),
                    format_number(normalize_rotation(pose.rotation_degrees))
                ),
            ));
        }

        patches.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
        let mut output = self.source.clone();
        for (span, replacement) in patches {
            output.replace_range(span.start..span.end, &replacement);
        }
        Ok(output)
    }
}

fn parse_footprint(node: &Node) -> Result<FootprintRecord, KicadError> {
    let property = |name: &str| {
        node.children("property")
            .find(|property| property.atom_at(1) == Some(name))
            .and_then(|property| property.atom_at(2))
            .map(str::to_string)
    };
    let reference = property("Reference").ok_or_else(|| KicadError::Semantic {
        offset: node.span.start,
        message: "footprint is missing its Reference property".into(),
    })?;
    let uuid = node
        .child("uuid")
        .and_then(|uuid| uuid.atom_at(1))
        .map(str::to_string);
    let at = node.child("at").ok_or_else(|| KicadError::Semantic {
        offset: node.span.start,
        message: format!("footprint {reference} is missing its at expression"),
    })?;
    let pose = parse_pose(at)?;
    let pads = node
        .children("pad")
        .map(parse_pad)
        .collect::<Result<Vec<_>, _>>()?;
    let layer = node
        .child("layer")
        .and_then(|layer| layer.atom_at(1))
        .unwrap_or("F.Cu")
        .to_string();
    if node
        .list()
        .into_iter()
        .flatten()
        .any(|child| child.child("layer").and_then(|layer| layer.atom_at(1)) == Some("Edge.Cuts"))
    {
        return Err(KicadError::Semantic {
            offset: node.span.start,
            message: format!(
                "footprint {reference} owns Edge.Cuts geometry; footprint-owned board cutouts are not supported by the v1 outline model"
            ),
        });
    }
    let front_courtyard_envelope = parse_courtyard_envelope(node, &reference, "F.CrtYd")?;
    let back_courtyard_envelope = parse_courtyard_envelope(node, &reference, "B.CrtYd")?;

    Ok(FootprintRecord {
        id: uuid.clone().unwrap_or_else(|| reference.clone()),
        reference,
        value: property("Value"),
        library_link: node.atom_at(1).map(str::to_string),
        uuid,
        layer,
        locked: node.contains_immediate_atom("locked"),
        pose,
        pads,
        front_courtyard_envelope,
        back_courtyard_envelope,
        at_span: at.span,
    })
}

fn parse_pad(node: &Node) -> Result<PadRecord, KicadError> {
    let at = node.child("at");
    let size = node.child("size");
    let (net_ordinal, net_name) = parse_pad_net(node)?;
    Ok(PadRecord {
        number: node.atom_at(1).unwrap_or("").to_string(),
        kind: node.atom_at(2).unwrap_or("unknown").to_string(),
        shape: node.atom_at(3).unwrap_or("unknown").to_string(),
        local_pose: at.map(parse_pose).transpose()?.unwrap_or(Pose {
            x: 0.0,
            y: 0.0,
            rotation_degrees: 0.0,
        }),
        size: Point {
            x: size
                .and_then(|node| parse_f64(node.atom_at(1)))
                .unwrap_or(0.0),
            y: size
                .and_then(|node| parse_f64(node.atom_at(2)))
                .unwrap_or(0.0),
        },
        drill: node.child("drill").and_then(first_numeric_after_head),
        layers: node
            .child("layers")
            .and_then(Node::list)
            .map(|nodes| {
                nodes
                    .iter()
                    .skip(1)
                    .filter_map(Node::atom)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        net_ordinal,
        net_name,
    })
}

fn parse_net_record(node: &Node) -> Result<NetRecord, KicadError> {
    let ordinal = node
        .atom_at(1)
        .filter(|_| node.atom_at_is_quoted(1) == Some(false))
        .and_then(parse_i64)
        .filter(|ordinal| *ordinal >= 0)
        .ok_or_else(|| KicadError::Semantic {
            offset: node.span.start,
            message: "top-level net has a missing or invalid ordinal".into(),
        })?;
    let name = node
        .atom_at(2)
        .ok_or_else(|| KicadError::Semantic {
            offset: node.span.start,
            message: format!("top-level net {ordinal} is missing its name"),
        })?
        .to_string();
    Ok(NetRecord { ordinal, name })
}

fn parse_pad_net(node: &Node) -> Result<(Option<i64>, Option<String>), KicadError> {
    let mut nets = node.children("net");
    let Some(net) = nets.next() else {
        return Ok((None, None));
    };
    if nets.next().is_some() {
        return Err(KicadError::Semantic {
            offset: net.span.start,
            message: "pad has duplicate net expressions".into(),
        });
    }

    let first = net.atom_at(1).ok_or_else(|| KicadError::Semantic {
        offset: net.span.start,
        message: "pad net expression is missing its net identifier".into(),
    })?;
    if let Some(name) = net.atom_at(2) {
        let ordinal = parse_i64(first)
            .filter(|_| net.atom_at_is_quoted(1) == Some(false))
            .filter(|ordinal| *ordinal >= 0)
            .ok_or_else(|| KicadError::Semantic {
                offset: net.span.start,
                message: "legacy pad net expression has an invalid ordinal".into(),
            })?;
        Ok((Some(ordinal), Some(name.to_string())))
    } else {
        if net.atom_at_is_quoted(1) == Some(false) && parse_i64(first).is_some() {
            return Err(KicadError::Semantic {
                offset: net.span.start,
                message: "legacy pad net expression is missing its name".into(),
            });
        }
        // KiCad 10 writes quoted, name-only pad nets. Retaining whether an atom
        // was quoted distinguishes a numeric-looking net name from a malformed
        // legacy ordinal record with no name.
        Ok((None, Some(first.to_string())))
    }
}

fn parse_pose(node: &Node) -> Result<Pose, KicadError> {
    let parse_coordinate = |index: usize, label: &str| {
        node.atom_at(index)
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| KicadError::Semantic {
                offset: node.span.start,
                message: format!("at expression has an invalid {label} coordinate"),
            })
    };
    Ok(Pose {
        x: parse_coordinate(1, "x")?,
        y: parse_coordinate(2, "y")?,
        rotation_degrees: node
            .atom_at(3)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0.0),
    })
}

fn parse_courtyard_envelope(
    footprint: &Node,
    reference: &str,
    courtyard_layer: &str,
) -> Result<Option<LocalBounds>, KicadError> {
    let mut primitives = Vec::new();
    let mut envelope: Option<LocalBounds> = None;

    for node in footprint.list().into_iter().flatten() {
        if node.child("layer").and_then(|layer| layer.atom_at(1)) != Some(courtyard_layer) {
            continue;
        }
        let head = node.head().ok_or_else(|| KicadError::Semantic {
            offset: node.span.start,
            message: format!("footprint {reference} has an invalid {courtyard_layer} expression"),
        })?;
        if matches!(head, "fp_text" | "fp_text_box" | "property") {
            continue;
        }
        if !head.starts_with("fp_") {
            return Err(KicadError::Semantic {
                offset: node.span.start,
                message: format!(
                    "footprint {reference} has unsupported {courtyard_layer} expression {head}"
                ),
            });
        }

        let primitive = parse_graphic_primitive(node, "fp", courtyard_layer)?;
        parse_stroke_width(node, courtyard_layer)?;
        let bounds =
            geometry::primitive_bounds(&primitive).map_err(|message| KicadError::Semantic {
                offset: node.span.start,
                message: format!("footprint {reference} has invalid courtyard geometry: {message}"),
            })?;
        envelope = Some(match envelope {
            Some(mut current) => {
                current.min_x = current.min_x.min(bounds.min_x);
                current.min_y = current.min_y.min(bounds.min_y);
                current.max_x = current.max_x.max(bounds.max_x);
                current.max_y = current.max_y.max(bounds.max_y);
                current
            }
            None => bounds,
        });
        primitives.push(primitive);
    }

    if primitives.is_empty() {
        return Ok(None);
    }
    geometry::validate_courtyard(&primitives).map_err(|message| KicadError::Semantic {
        offset: footprint.span.start,
        message: format!("footprint {reference} has an invalid {courtyard_layer}: {message}"),
    })?;
    Ok(envelope)
}

fn parse_graphic_primitive(
    node: &Node,
    family: &str,
    layer: &str,
) -> Result<EdgePrimitive, KicadError> {
    let head = node.head().ok_or_else(|| KicadError::Semantic {
        offset: node.span.start,
        message: format!("invalid graphic expression on {layer}"),
    })?;
    let kind = head
        .strip_prefix(family)
        .and_then(|suffix| suffix.strip_prefix('_'))
        .ok_or_else(|| KicadError::Semantic {
            offset: node.span.start,
            message: format!("unsupported graphic primitive {head} on {layer}"),
        })?;
    let point = |tag: &str| parse_required_point(node, tag, layer);

    match kind {
        "rect" => Ok(EdgePrimitive::Rectangle {
            start: point("start")?,
            end: point("end")?,
            radius: parse_optional_nonnegative_f64(node, "radius", layer)?.unwrap_or(0.0),
        }),
        "line" => Ok(EdgePrimitive::Line {
            start: point("start")?,
            end: point("end")?,
        }),
        "arc" => Ok(EdgePrimitive::Arc {
            start: point("start")?,
            mid: point("mid")?,
            end: point("end")?,
        }),
        "circle" => Ok(EdgePrimitive::Circle {
            center: point("center")?,
            end: point("end")?,
        }),
        "poly" => {
            let points_node = required_unique_child(node, "pts", layer)?;
            let points = points_node
                .children("xy")
                .map(|xy| parse_xy(xy, layer))
                .collect::<Result<Vec<_>, _>>()?;
            if points.len() < 3 {
                return Err(KicadError::Semantic {
                    offset: node.span.start,
                    message: format!("{head} on {layer} has fewer than three points"),
                });
            }
            Ok(EdgePrimitive::Polygon { points })
        }
        _ => Err(KicadError::Semantic {
            offset: node.span.start,
            message: format!("unsupported graphic primitive {head} on {layer}"),
        }),
    }
}

fn parse_required_point(node: &Node, tag: &str, layer: &str) -> Result<Point, KicadError> {
    let point = required_unique_child(node, tag, layer)?;
    parse_xy(point, layer)
}

fn required_unique_child<'a>(
    node: &'a Node,
    tag: &str,
    layer: &str,
) -> Result<&'a Node, KicadError> {
    let mut child = None;
    for candidate in node.list().into_iter().flatten() {
        if candidate.head() != Some(tag) {
            continue;
        }
        if child.is_some() {
            return Err(KicadError::Semantic {
                offset: candidate.span.start,
                message: format!(
                    "{} on {layer} has duplicate {tag} expressions",
                    node.head().unwrap_or("graphic")
                ),
            });
        }
        child = Some(candidate);
    }
    child.ok_or_else(|| KicadError::Semantic {
        offset: node.span.start,
        message: format!(
            "{} on {layer} is missing its {tag} expression",
            node.head().unwrap_or("graphic")
        ),
    })
}

fn parse_xy(node: &Node, layer: &str) -> Result<Point, KicadError> {
    let coordinate = |index: usize, axis: &str| -> Result<f64, KicadError> {
        let value = node
            .atom_at(index)
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite())
            .ok_or_else(|| KicadError::Semantic {
                offset: node.span.start,
                message: format!("graphic point on {layer} has an invalid {axis} coordinate"),
            })?;
        Ok(value)
    };
    Ok(Point {
        x: coordinate(1, "x")?,
        y: coordinate(2, "y")?,
    })
}

fn parse_stroke_width(node: &Node, layer: &str) -> Result<f64, KicadError> {
    let width = node
        .child("stroke")
        .and_then(|stroke| stroke.child("width"))
        .or_else(|| node.child("width"))
        .and_then(|width| width.atom_at(1))
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| KicadError::Semantic {
            offset: node.span.start,
            message: format!(
                "{} on {layer} has a missing or invalid stroke width",
                node.head().unwrap_or("graphic")
            ),
        })?;
    Ok(width)
}

fn parse_optional_nonnegative_f64(
    node: &Node,
    tag: &str,
    layer: &str,
) -> Result<Option<f64>, KicadError> {
    let mut values = node.children(tag);
    let Some(value_node) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(KicadError::Semantic {
            offset: value_node.span.start,
            message: format!(
                "{} on {layer} has duplicate {tag} expressions",
                node.head().unwrap_or("graphic")
            ),
        });
    }
    let value = value_node
        .atom_at(1)
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| KicadError::Semantic {
            offset: value_node.span.start,
            message: format!(
                "{} on {layer} has an invalid {tag}",
                node.head().unwrap_or("graphic")
            ),
        })?;
    Ok(Some(value))
}

fn first_numeric_after_head(node: &Node) -> Option<f64> {
    node.list()?
        .iter()
        .skip(1)
        .filter_map(Node::atom)
        .find_map(|atom| atom.parse().ok())
}

fn parse_f64(value: Option<&str>) -> Option<f64> {
    value?.parse().ok()
}

fn parse_i64(value: &str) -> Option<i64> {
    value.parse().ok()
}

fn normalize_rotation(rotation: f64) -> f64 {
    let normalized = rotation.rem_euclid(360.0);
    if normalized == -0.0 || (360.0 - normalized).abs() < 1e-12 {
        0.0
    } else {
        normalized
    }
}

fn format_number(value: f64) -> String {
    let value = if value.abs() < 0.000_000_000_5 {
        0.0
    } else {
        value
    };
    let formatted = format!("{value:.9}");
    let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-0" {
        "0".into()
    } else {
        trimmed.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../../../public/sample-sensor.kicad_pcb");
    const CURVED_OUTLINE: &str = include_str!("../fixtures/curved-outline.kicad_pcb");

    #[test]
    fn parses_sample_into_first_class_footprints_and_preserves_noop() {
        let board = BoardDocument::parse(SAMPLE).expect("sample should parse");
        assert_eq!(board.nets.len(), 5);
        assert_eq!(board.footprints.len(), 2);
        assert_eq!(board.footprints[0].reference, "J1");
        assert_eq!(board.footprints[0].pads.len(), 4);
        assert_eq!(board.footprints[0].pads[1].number, "2");
        assert_eq!(
            board.footprints[0].pads[1].net_name.as_deref(),
            Some("SENSOR_DATA")
        );
        assert_eq!(board.edge_cuts.len(), 1);
        let courtyard = board.footprints[0]
            .front_courtyard_envelope
            .expect("sample footprint has a courtyard");
        assert!((courtyard.min_x + 2.0).abs() < 1.0e-9);
        assert!((courtyard.min_y + 2.0).abs() < 1.0e-9);
        assert!((courtyard.max_x - 2.0).abs() < 1.0e-9);
        assert!((courtyard.max_y - 29.0).abs() < 1.0e-9);
        assert_eq!(board.render_unchanged().as_bytes(), SAMPLE.as_bytes());
        assert_eq!(
            board
                .rewrite_placements(&BTreeMap::new(), PatchOptions::default())
                .unwrap()
                .as_bytes(),
            SAMPLE.as_bytes()
        );
    }

    #[test]
    fn parses_the_external_curved_outline_contract_fixture() {
        let board = BoardDocument::parse(CURVED_OUTLINE).unwrap();
        assert_eq!(board.edge_cuts.len(), 8);
        assert_eq!(
            board
                .edge_cuts
                .iter()
                .filter(|primitive| matches!(primitive, EdgePrimitive::Arc { .. }))
                .count(),
            4
        );
        assert_eq!(board.render_unchanged(), CURVED_OUTLINE);
    }

    #[test]
    fn patches_only_the_top_level_footprint_pose() {
        let source = r#"(kicad_pcb
  ; keep this comment and unknown extension exactly
  (vendor_extension (at 100 200))
  (footprint "Lib:Part" locked
    (layer "F.Cu")
    (at 10 20)
    (property "Reference" "U1" (at 1 2 90) (layer "F.SilkS"))
    (property "Value" "µcontroller" (at 0 0) (layer "F.Fab"))
    (pad "1" thru_hole circle (at 0 0) (size 2 2) (drill 1) (layers "*.Cu") (net 1 "N\"1"))))"#;
        let board = BoardDocument::parse(source).unwrap();
        let mut placements = BTreeMap::new();
        placements.insert(
            "U1".into(),
            Pose {
                x: 12.5,
                y: 22.25,
                rotation_degrees: 450.0,
            },
        );
        let output = board
            .rewrite_placements(
                &placements,
                PatchOptions {
                    override_locked: true,
                },
            )
            .unwrap();
        assert!(output.contains("(at 12.5 22.25 90)"));
        assert!(output.contains("(property \"Reference\" \"U1\" (at 1 2 90)"));
        assert!(output.contains("; keep this comment and unknown extension exactly"));
        assert!(output.contains("(vendor_extension (at 100 200))"));
        assert!(output.contains("µcontroller"));
    }

    #[test]
    fn locked_footprints_require_an_explicit_override() {
        let source = r#"(kicad_pcb (footprint "L:P" locked (layer "F.Cu") (at 1 2) (property "Reference" "J1")))"#;
        let board = BoardDocument::parse(source).unwrap();
        let placements = BTreeMap::from([(
            "J1".into(),
            Pose {
                x: 2.0,
                y: 2.0,
                rotation_degrees: 0.0,
            },
        )]);
        let error = board
            .rewrite_placements(&placements, PatchOptions::default())
            .unwrap_err();
        assert!(error.to_string().contains("locked"));
    }

    #[test]
    fn rejects_unknown_and_non_finite_placement_requests() {
        let source =
            r#"(kicad_pcb (footprint "L:P" (layer "F.Cu") (at 1 2) (property "Reference" "J1")))"#;
        let board = BoardDocument::parse(source).unwrap();
        let unknown = BTreeMap::from([(
            "X99".into(),
            Pose {
                x: 1.0,
                y: 2.0,
                rotation_degrees: 0.0,
            },
        )]);
        assert!(board
            .rewrite_placements(&unknown, PatchOptions::default())
            .unwrap_err()
            .to_string()
            .contains("unknown footprint"));

        let non_finite = BTreeMap::from([(
            "J1".into(),
            Pose {
                x: f64::NAN,
                y: 2.0,
                rotation_degrees: 0.0,
            },
        )]);
        assert!(board
            .rewrite_placements(&non_finite, PatchOptions::default())
            .unwrap_err()
            .to_string()
            .contains("non-finite"));
    }

    #[test]
    fn enforces_depth_and_input_limits() {
        let nested = "((((a))))";
        let error = BoardDocument::parse_with_limits(
            nested,
            ParseLimits {
                max_input_bytes: 100,
                max_depth: 2,
                max_nodes: 100,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("depth limit"));

        let error = BoardDocument::parse_with_limits(
            "(kicad_pcb)",
            ParseLimits {
                max_input_bytes: 5,
                max_depth: 10,
                max_nodes: 10,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("input exceeds"));
    }

    #[test]
    fn duplicate_identities_fail_closed() {
        let source = r#"(kicad_pcb
          (footprint "L:A" (layer "F.Cu") (at 1 2) (property "Reference" "R1"))
          (footprint "L:B" (layer "F.Cu") (at 3 4) (property "Reference" "R1")))"#;
        let error = BoardDocument::parse(source).unwrap_err();
        assert!(error.to_string().contains("duplicate footprint identity"));
    }

    #[test]
    fn parses_numeric_name_only_pad_nets_and_rejects_malformed_net_records() {
        let numeric_name = r#"(kicad_pcb
          (footprint "L:NumericNet" (layer "F.Cu") (at 0 0)
            (property "Reference" "U1")
            (pad "1" thru_hole circle (at 0 0) (size 1 1) (drill 0.5)
              (layers "*.Cu" "*.Mask") (net "123"))))"#;
        let board = BoardDocument::parse(numeric_name).unwrap();
        assert_eq!(board.footprints[0].pads[0].net_ordinal, None);
        assert_eq!(board.footprints[0].pads[0].net_name.as_deref(), Some("123"));

        let malformed_top_level = r#"(kicad_pcb (net nope "GND"))"#;
        assert!(BoardDocument::parse(malformed_top_level)
            .unwrap_err()
            .to_string()
            .contains("invalid ordinal"));

        let missing_pad_identifier = r#"(kicad_pcb
          (footprint "L:MissingNet" (layer "F.Cu") (at 0 0)
            (property "Reference" "U1")
            (pad "1" thru_hole circle (net))))"#;
        assert!(BoardDocument::parse(missing_pad_identifier)
            .unwrap_err()
            .to_string()
            .contains("missing its net identifier"));

        let missing_legacy_name = r#"(kicad_pcb
          (footprint "L:MissingNetName" (layer "F.Cu") (at 0 0)
            (property "Reference" "U1")
            (pad "1" thru_hole circle (net 1))))"#;
        assert!(BoardDocument::parse(missing_legacy_name)
            .unwrap_err()
            .to_string()
            .contains("missing its name"));

        let quoted_legacy_ordinal = r#"(kicad_pcb
          (footprint "L:QuotedOrdinal" (layer "F.Cu") (at 0 0)
            (property "Reference" "U1")
            (pad "1" thru_hole circle (net "1" "GND"))))"#;
        assert!(BoardDocument::parse(quoted_legacy_ordinal)
            .unwrap_err()
            .to_string()
            .contains("invalid ordinal"));

        let duplicate_pad_net = r#"(kicad_pcb
          (footprint "L:DuplicateNet" (layer "F.Cu") (at 0 0)
            (property "Reference" "U1")
            (pad "1" thru_hole circle (net "A") (net "B"))))"#;
        assert!(BoardDocument::parse(duplicate_pad_net)
            .unwrap_err()
            .to_string()
            .contains("duplicate net expressions"));
    }

    #[test]
    fn extracts_circle_courtyard_from_centerline_geometry() {
        let source = r#"(kicad_pcb
          (footprint "L:Round" (layer "F.Cu") (at 1 2)
            (property "Reference" "J1")
            (fp_circle (center 2 3) (end 7 3)
              (stroke (width 0.2) (type solid))
              (fill none) (layer "F.CrtYd"))))"#;
        let board = BoardDocument::parse(source).unwrap();
        assert_eq!(
            board.footprints[0].front_courtyard_envelope,
            Some(LocalBounds {
                min_x: -3.0,
                min_y: -2.0,
                max_x: 7.0,
                max_y: 8.0,
            })
        );
    }

    #[test]
    fn malformed_or_unsupported_authoritative_geometry_fails_closed() {
        let malformed_edge = r#"(kicad_pcb
          (gr_arc (start 0 0) (end 10 0) (layer "Edge.Cuts")))"#;
        assert!(BoardDocument::parse(malformed_edge)
            .unwrap_err()
            .to_string()
            .contains("missing its mid"));

        let unsupported_edge = r#"(kicad_pcb
          (gr_curve (pts (xy 0 0) (xy 1 0) (xy 1 1) (xy 0 1))
            (layer "Edge.Cuts")))"#;
        assert!(BoardDocument::parse(unsupported_edge)
            .unwrap_err()
            .to_string()
            .contains("unsupported graphic primitive gr_curve"));

        let malformed_polygon = r#"(kicad_pcb
          (gr_poly (pts (xy 0 0) (xy nope 0) (xy 1 1))
            (layer "Edge.Cuts")))"#;
        assert!(BoardDocument::parse(malformed_polygon)
            .unwrap_err()
            .to_string()
            .contains("invalid x coordinate"));

        let duplicate_start = r#"(kicad_pcb
          (gr_rect (start 0 0) (start 1 1) (end 10 10)
            (layer "Edge.Cuts")))"#;
        assert!(BoardDocument::parse(duplicate_start)
            .unwrap_err()
            .to_string()
            .contains("duplicate start expressions"));

        let non_finite_coordinate = r#"(kicad_pcb
          (gr_circle (center NaN 0) (end 1 0) (layer "Edge.Cuts")))"#;
        assert!(BoardDocument::parse(non_finite_coordinate)
            .unwrap_err()
            .to_string()
            .contains("invalid x coordinate"));

        let open_courtyard = r#"(kicad_pcb
          (footprint "L:Open" (layer "F.Cu") (at 0 0)
            (property "Reference" "U1")
            (fp_line (start 0 0) (end 1 0)
              (stroke (width 0.05) (type solid)) (layer "F.CrtYd"))))"#;
        assert!(BoardDocument::parse(open_courtyard)
            .unwrap_err()
            .to_string()
            .contains("closed connected loops"));
    }

    #[test]
    fn preserves_nonzero_rounded_rectangle_radius() {
        let source = r#"(kicad_pcb
          (gr_rect (start 0 0) (end 48 52) (radius 1.2)
            (stroke (width 0.05) (type solid)) (fill none)
            (layer "Edge.Cuts")))"#;
        let board = BoardDocument::parse(source).unwrap();
        assert!(matches!(
            board.edge_cuts.as_slice(),
            [EdgePrimitive::Rectangle { radius, .. }] if (*radius - 1.2).abs() < 1.0e-12
        ));
    }

    #[test]
    fn ignores_courtyard_text_but_rejects_footprint_owned_board_cuts() {
        let with_text = r#"(kicad_pcb
          (footprint "L:Text" (layer "F.Cu") (at 0 0)
            (property "Reference" "U1")
            (fp_text user "note" (at 0 0) (layer "F.CrtYd"))
            (fp_rect (start -1 -1) (end 1 1)
              (stroke (width 0.05) (type solid)) (fill none) (layer "F.CrtYd"))))"#;
        assert!(BoardDocument::parse(with_text).is_ok());

        let with_cutout = r#"(kicad_pcb
          (footprint "L:Cutout" (layer "F.Cu") (at 0 0)
            (property "Reference" "U1")
            (fp_circle (center 0 0) (end 1 0)
              (stroke (width 0.05) (type solid)) (fill none) (layer "Edge.Cuts"))))"#;
        assert!(BoardDocument::parse(with_cutout)
            .unwrap_err()
            .to_string()
            .contains("footprint-owned board cutouts"));
    }
}
