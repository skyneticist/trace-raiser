//! Syntax-preserving KiCad PCB placement adapter.
//!
//! The adapter intentionally edits only a footprint's immediate `(at ...)`
//! expression. KiCad remains the parser and DRC authority for emitted boards.

mod design;
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
                        kind: NodeKind::Atom(Atom { value }),
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
            kind: NodeKind::Atom(Atom { value }),
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
    at_span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EdgePrimitive {
    Rectangle {
        start: Point,
        end: Point,
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
            .filter_map(|node| {
                Some(NetRecord {
                    ordinal: parse_i64(node.atom_at(1)?)?,
                    name: node.atom_at(2)?.to_string(),
                })
            })
            .collect();

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

        let edge_cuts = root
            .list()
            .into_iter()
            .flatten()
            .filter_map(parse_edge_primitive)
            .collect();

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

    Ok(FootprintRecord {
        id: uuid.clone().unwrap_or_else(|| reference.clone()),
        reference,
        value: property("Value"),
        library_link: node.atom_at(1).map(str::to_string),
        uuid,
        layer: node
            .child("layer")
            .and_then(|layer| layer.atom_at(1))
            .unwrap_or("F.Cu")
            .to_string(),
        locked: node.contains_immediate_atom("locked"),
        pose,
        pads,
        at_span: at.span,
    })
}

fn parse_pad(node: &Node) -> Result<PadRecord, KicadError> {
    let at = node.child("at");
    let size = node.child("size");
    let net = node.child("net");
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
        net_ordinal: net.and_then(|node| node.atom_at(1)).and_then(parse_i64),
        net_name: net.and_then(|node| node.atom_at(2)).map(str::to_string),
    })
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

fn parse_edge_primitive(node: &Node) -> Option<EdgePrimitive> {
    if !matches!(
        node.head(),
        Some("gr_rect" | "gr_poly" | "gr_line" | "gr_arc")
    ) || node.child("layer")?.atom_at(1)? != "Edge.Cuts"
    {
        return None;
    }
    let point = |tag: &str| {
        let child = node.child(tag)?;
        Some(Point {
            x: parse_f64(child.atom_at(1))?,
            y: parse_f64(child.atom_at(2))?,
        })
    };
    match node.head()? {
        "gr_rect" => Some(EdgePrimitive::Rectangle {
            start: point("start")?,
            end: point("end")?,
        }),
        "gr_line" => Some(EdgePrimitive::Line {
            start: point("start")?,
            end: point("end")?,
        }),
        "gr_arc" => Some(EdgePrimitive::Arc {
            start: point("start")?,
            mid: point("mid")?,
            end: point("end")?,
        }),
        "gr_poly" => {
            let points = node
                .child("pts")?
                .children("xy")
                .filter_map(|xy| {
                    Some(Point {
                        x: parse_f64(xy.atom_at(1))?,
                        y: parse_f64(xy.atom_at(2))?,
                    })
                })
                .collect::<Vec<_>>();
            (points.len() >= 3).then_some(EdgePrimitive::Polygon { points })
        }
        _ => None,
    }
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
        assert!(matches!(
            board.edge_cuts.as_slice(),
            [EdgePrimitive::Rectangle { .. }]
        ));
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
}
