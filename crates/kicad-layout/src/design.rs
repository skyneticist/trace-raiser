use crate::{BoardDocument, EdgePrimitive, Point as KicadPoint};
use layout_core::{
    validate_design, Component, Design, JumperPolicy, LocalAabb, Net, Pad, PadKind,
    PlacementKeepout, PlacementRules, Point, Polygon, Pose, RoutingRules, TerminalRef,
    DESIGN_SCHEMA_VERSION,
};
use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fmt;

const OUTLINE_JOIN_TOLERANCE_MM: f64 = 1.0e-6;

#[derive(Debug, Clone)]
pub struct DesignBuildOptions {
    pub name: String,
    /// Courtyard-derived envelopes keyed by footprint UUID or reference.
    pub footprint_envelopes: BTreeMap<String, LocalAabb>,
    /// Optional rotation sets keyed by footprint UUID or reference.
    pub allowed_rotations_degrees: BTreeMap<String, Vec<f64>>,
    pub placement_keepouts: Vec<PlacementKeepout>,
    pub placement_rules: PlacementRules,
    pub routing_rules: RoutingRules,
    pub jumper_policy: JumperPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesignBuildError {
    UnsupportedOutline(String),
    DuplicateNetOrdinal(i64),
    MissingEnvelope {
        footprint_id: String,
        reference: String,
    },
    UnsupportedFootprintLayer {
        reference: String,
        layer: String,
    },
    UnsupportedPadKind {
        reference: String,
        pad: String,
        kind: String,
    },
    UnknownPadNet {
        reference: String,
        pad: String,
        ordinal: i64,
    },
    NetNameMismatch {
        reference: String,
        pad: String,
        expected: String,
        actual: String,
    },
    InvalidDesign(Vec<String>),
}

impl fmt::Display for DesignBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedOutline(message) => write!(formatter, "unsupported Edge.Cuts: {message}"),
            Self::DuplicateNetOrdinal(ordinal) => {
                write!(formatter, "duplicate KiCad net ordinal {ordinal}")
            }
            Self::MissingEnvelope {
                footprint_id,
                reference,
            } => write!(
                formatter,
                "footprint {reference} ({footprint_id}) has no explicit placement envelope"
            ),
            Self::UnsupportedFootprintLayer { reference, layer } => write!(
                formatter,
                "footprint {reference} is on {layer}; the v1 contract requires front-side THT parts"
            ),
            Self::UnsupportedPadKind {
                reference,
                pad,
                kind,
            } => write!(
                formatter,
                "footprint {reference} pad {pad} uses unsupported KiCad pad kind {kind}"
            ),
            Self::UnknownPadNet {
                reference,
                pad,
                ordinal,
            } => write!(
                formatter,
                "footprint {reference} pad {pad} references unknown net ordinal {ordinal}"
            ),
            Self::NetNameMismatch {
                reference,
                pad,
                expected,
                actual,
            } => write!(
                formatter,
                "footprint {reference} pad {pad} names its net {actual}, but ordinal authority says {expected}"
            ),
            Self::InvalidDesign(messages) => write!(
                formatter,
                "converted design violates the canonical contract: {}",
                messages.join("; ")
            ),
        }
    }
}

impl Error for DesignBuildError {}

impl BoardDocument {
    /// Convert parsed KiCad records into the canonical optimization model.
    ///
    /// Footprint envelopes are required from a courtyard-aware caller. Inferring
    /// a component body from pad extents is intentionally not accepted because
    /// it can produce physically impossible placements.
    pub fn to_layout_design(
        &self,
        options: &DesignBuildOptions,
    ) -> Result<Design, DesignBuildError> {
        let outline = convert_outline(&self.edge_cuts)?;
        let mut nets_by_ordinal = BTreeMap::new();
        for net in self.nets.iter().filter(|net| net.ordinal != 0) {
            if nets_by_ordinal
                .insert(
                    net.ordinal,
                    Net {
                        id: net_id(net.ordinal),
                        name: net.name.clone(),
                        terminals: Vec::new(),
                    },
                )
                .is_some()
            {
                return Err(DesignBuildError::DuplicateNetOrdinal(net.ordinal));
            }
        }

        let mut components = Vec::with_capacity(self.footprints.len());
        for footprint in &self.footprints {
            if footprint.layer != "F.Cu" {
                return Err(DesignBuildError::UnsupportedFootprintLayer {
                    reference: footprint.reference.clone(),
                    layer: footprint.layer.clone(),
                });
            }
            let envelope = options
                .footprint_envelopes
                .get(&footprint.id)
                .or_else(|| options.footprint_envelopes.get(&footprint.reference))
                .copied()
                .ok_or_else(|| DesignBuildError::MissingEnvelope {
                    footprint_id: footprint.id.clone(),
                    reference: footprint.reference.clone(),
                })?;
            let rotations = options
                .allowed_rotations_degrees
                .get(&footprint.id)
                .or_else(|| options.allowed_rotations_degrees.get(&footprint.reference))
                .cloned()
                .unwrap_or_else(|| {
                    if footprint.locked {
                        vec![footprint.pose.rotation_degrees]
                    } else {
                        vec![0.0, 90.0, 180.0, 270.0]
                    }
                });

            let mut pad_occurrences = HashMap::<String, usize>::new();
            let mut pads = Vec::with_capacity(footprint.pads.len());
            for (index, pad) in footprint.pads.iter().enumerate() {
                let base_id = if pad.number.is_empty() {
                    format!("@{}", index + 1)
                } else {
                    pad.number.clone()
                };
                let occurrence = pad_occurrences.entry(base_id.clone()).or_default();
                *occurrence += 1;
                let pad_id = if *occurrence == 1 {
                    base_id
                } else {
                    format!("{base_id}#{occurrence}")
                };
                let kind = match pad.kind.as_str() {
                    "thru_hole" => PadKind::ThroughHole,
                    "np_thru_hole" => PadKind::NonPlatedThroughHole,
                    other => {
                        return Err(DesignBuildError::UnsupportedPadKind {
                            reference: footprint.reference.clone(),
                            pad: pad.number.clone(),
                            kind: other.to_string(),
                        })
                    }
                };

                if let Some(ordinal) = pad.net_ordinal.filter(|ordinal| *ordinal != 0) {
                    let net = nets_by_ordinal.get_mut(&ordinal).ok_or_else(|| {
                        DesignBuildError::UnknownPadNet {
                            reference: footprint.reference.clone(),
                            pad: pad.number.clone(),
                            ordinal,
                        }
                    })?;
                    if let Some(actual_name) = &pad.net_name {
                        if actual_name != &net.name {
                            return Err(DesignBuildError::NetNameMismatch {
                                reference: footprint.reference.clone(),
                                pad: pad.number.clone(),
                                expected: net.name.clone(),
                                actual: actual_name.clone(),
                            });
                        }
                    }
                    net.terminals.push(TerminalRef {
                        component_id: footprint.id.clone(),
                        pad_id: pad_id.clone(),
                    });
                }

                pads.push(Pad {
                    id: pad_id,
                    local_position: Point {
                        x: pad.local_pose.x,
                        y: pad.local_pose.y,
                    },
                    size: Point {
                        x: pad.size.x,
                        y: pad.size.y,
                    },
                    drill_mm: pad.drill,
                    kind,
                });
            }

            components.push(Component {
                id: footprint.id.clone(),
                reference: footprint.reference.clone(),
                footprint: footprint
                    .library_link
                    .clone()
                    .unwrap_or_else(|| "<unknown>".into()),
                envelope,
                pads,
                initial_pose: Pose {
                    x: footprint.pose.x,
                    y: footprint.pose.y,
                    rotation_degrees: footprint.pose.rotation_degrees,
                },
                locked: footprint.locked,
                allowed_rotations_degrees: rotations,
            });
        }

        let design = Design {
            schema_version: DESIGN_SCHEMA_VERSION,
            name: options.name.clone(),
            outline,
            placement_keepouts: options.placement_keepouts.clone(),
            components,
            nets: nets_by_ordinal.into_values().collect(),
            placement_rules: options.placement_rules.clone(),
            routing_rules: options.routing_rules.clone(),
            jumper_policy: options.jumper_policy.clone(),
        };
        validate_design(&design)
            .map_err(|error| DesignBuildError::InvalidDesign(error.messages().to_vec()))?;
        Ok(design)
    }
}

fn net_id(ordinal: i64) -> String {
    format!("kicad-net-{ordinal}")
}

fn convert_outline(edge_cuts: &[EdgePrimitive]) -> Result<Polygon, DesignBuildError> {
    match edge_cuts {
        [EdgePrimitive::Rectangle { start, end }] => {
            let min_x = start.x.min(end.x);
            let min_y = start.y.min(end.y);
            let max_x = start.x.max(end.x);
            let max_y = start.y.max(end.y);
            Ok(Polygon {
                vertices: vec![
                    Point { x: min_x, y: min_y },
                    Point { x: max_x, y: min_y },
                    Point { x: max_x, y: max_y },
                    Point { x: min_x, y: max_y },
                ],
            })
        }
        [EdgePrimitive::Polygon { points }] => Ok(Polygon {
            vertices: points.iter().copied().map(convert_point).collect(),
        }),
        [] => Err(DesignBuildError::UnsupportedOutline(
            "no Edge.Cuts primitives were found".into(),
        )),
        primitives
            if primitives
                .iter()
                .all(|primitive| matches!(primitive, EdgePrimitive::Line { .. })) =>
        {
            stitch_line_outline(primitives)
        }
        _ => Err(DesignBuildError::UnsupportedOutline(
            "v1 accepts one rectangle, one polygon, or one closed chain of straight lines; arcs and multiple loops are not yet supported".into(),
        )),
    }
}

fn stitch_line_outline(lines: &[EdgePrimitive]) -> Result<Polygon, DesignBuildError> {
    let mut remaining = lines
        .iter()
        .filter_map(|line| match line {
            EdgePrimitive::Line { start, end } => Some((*start, *end)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let Some((start, end)) = remaining.pop() else {
        return Err(DesignBuildError::UnsupportedOutline(
            "line-chain outline is empty".into(),
        ));
    };
    let mut vertices = vec![start, end];
    while !remaining.is_empty() {
        let current = *vertices.last().expect("line chain has a current point");
        let Some((index, next)) = remaining.iter().enumerate().find_map(|(index, (a, b))| {
            if points_close(current, *a) {
                Some((index, *b))
            } else if points_close(current, *b) {
                Some((index, *a))
            } else {
                None
            }
        }) else {
            return Err(DesignBuildError::UnsupportedOutline(
                "straight Edge.Cuts do not form one connected loop".into(),
            ));
        };
        remaining.swap_remove(index);
        vertices.push(next);
    }
    if !points_close(
        *vertices.first().expect("line chain has a start"),
        *vertices.last().expect("line chain has an end"),
    ) {
        return Err(DesignBuildError::UnsupportedOutline(
            "straight Edge.Cuts chain is not closed".into(),
        ));
    }
    vertices.pop();
    Ok(Polygon {
        vertices: vertices.into_iter().map(convert_point).collect(),
    })
}

fn points_close(first: KicadPoint, second: KicadPoint) -> bool {
    (first.x - second.x).abs() <= OUTLINE_JOIN_TOLERANCE_MM
        && (first.y - second.y).abs() <= OUTLINE_JOIN_TOLERANCE_MM
}

fn convert_point(point: KicadPoint) -> Point {
    Point {
        x: point.x,
        y: point.y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layout_core::{JumperMode, PlacementStatus};

    const SAMPLE: &str = include_str!("../../../public/sample-sensor.kicad_pcb");

    fn options() -> DesignBuildOptions {
        DesignBuildOptions {
            name: "sample".into(),
            footprint_envelopes: BTreeMap::from([
                (
                    "J1".into(),
                    LocalAabb {
                        min_x: -2.0,
                        min_y: -2.0,
                        max_x: 2.0,
                        max_y: 29.0,
                    },
                ),
                (
                    "J2".into(),
                    LocalAabb {
                        min_x: -2.0,
                        min_y: -2.0,
                        max_x: 2.0,
                        max_y: 29.0,
                    },
                ),
            ]),
            allowed_rotations_degrees: BTreeMap::new(),
            placement_keepouts: Vec::new(),
            placement_rules: PlacementRules {
                grid_mm: 1.0,
                component_clearance_mm: 1.0,
                edge_clearance_mm: 1.0,
            },
            routing_rules: RoutingRules {
                grid_mm: 0.5,
                trace_width_mm: 1.4,
                trace_clearance_mm: 0.8,
                edge_clearance_mm: 1.0,
            },
            jumper_policy: JumperPolicy {
                mode: JumperMode::Forbidden,
                approvals: Vec::new(),
            },
        }
    }

    #[test]
    fn converts_sample_without_guessing_footprint_bodies() {
        let board = BoardDocument::parse(SAMPLE).unwrap();
        let design = board.to_layout_design(&options()).unwrap();
        assert_eq!(design.components.len(), 2);
        assert_eq!(design.nets.len(), 4);
        assert!(design.nets.iter().all(|net| net.terminals.len() == 2));
        assert_eq!(design.outline.vertices.len(), 4);
        let placement =
            layout_core::place(&design, layout_core::PlacementOptions::default()).unwrap();
        assert_eq!(placement.status, PlacementStatus::Complete);
    }

    #[test]
    fn missing_footprint_envelope_fails_closed() {
        let board = BoardDocument::parse(SAMPLE).unwrap();
        let mut options = options();
        options.footprint_envelopes.remove("J2");
        assert!(matches!(
            board.to_layout_design(&options),
            Err(DesignBuildError::MissingEnvelope { reference, .. }) if reference == "J2"
        ));
    }

    #[test]
    fn stitches_unordered_straight_edge_cuts() {
        let source = r#"(kicad_pcb
          (gr_line (start 10 0) (end 10 10) (layer "Edge.Cuts"))
          (gr_line (start 0 10) (end 0 0) (layer "Edge.Cuts"))
          (gr_line (start 0 0) (end 10 0) (layer "Edge.Cuts"))
          (gr_line (start 10 10) (end 0 10) (layer "Edge.Cuts")))"#;
        let board = BoardDocument::parse(source).unwrap();
        let design = board.to_layout_design(&DesignBuildOptions {
            name: "line outline".into(),
            footprint_envelopes: BTreeMap::new(),
            allowed_rotations_degrees: BTreeMap::new(),
            placement_keepouts: Vec::new(),
            placement_rules: options().placement_rules,
            routing_rules: options().routing_rules,
            jumper_policy: options().jumper_policy,
        });
        assert_eq!(design.unwrap().outline.vertices.len(), 4);
    }
}
