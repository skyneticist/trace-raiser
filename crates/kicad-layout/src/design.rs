use crate::{geometry, BoardDocument, EdgePrimitive};
use layout_core::{
    validate_design, Component, Design, JumperPolicy, LocalAabb, Net, Pad, PadKind,
    PlacementKeepout, PlacementRules, Point, Polygon, Pose, RoutingRules, TerminalRef,
    DESIGN_SCHEMA_VERSION,
};
use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone)]
pub struct DesignBuildOptions {
    pub name: String,
    /// Optional trusted envelope overrides keyed by footprint UUID or
    /// reference. Parsed F.CrtYd/B.CrtYd geometry is used when no override is
    /// present.
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
    DuplicateNetName(String),
    MissingEnvelope {
        footprint_id: String,
        reference: String,
    },
    EnvelopeOverrideTooSmall {
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
            Self::DuplicateNetName(name) => write!(formatter, "duplicate KiCad net name {name}"),
            Self::MissingEnvelope {
                footprint_id,
                reference,
            } => write!(
                formatter,
                "footprint {reference} ({footprint_id}) has no usable F.CrtYd or explicit placement envelope"
            ),
            Self::EnvelopeOverrideTooSmall {
                footprint_id,
                reference,
            } => write!(
                formatter,
                "footprint {reference} ({footprint_id}) has a manual envelope smaller than its parsed courtyard"
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
    /// Footprint envelopes come from parsed courtyard geometry or a trusted
    /// explicit override. Inferring a component body from pad extents is
    /// intentionally not accepted because it can produce physically impossible
    /// placements.
    pub fn to_layout_design(
        &self,
        options: &DesignBuildOptions,
    ) -> Result<Design, DesignBuildError> {
        let outline = convert_outline(&self.edge_cuts)?;
        let outline_has_curves = self.edge_cuts.iter().any(|primitive| match primitive {
            EdgePrimitive::Arc { .. } | EdgePrimitive::Circle { .. } => true,
            EdgePrimitive::Rectangle { radius, .. } => *radius > 0.0,
            EdgePrimitive::Polygon { .. } | EdgePrimitive::Line { .. } => false,
        });
        let outline_approximation_guard = if outline_has_curves {
            geometry::OUTLINE_APPROXIMATION_GUARD_MM
        } else {
            0.0
        };
        let mut nets_by_ordinal = BTreeMap::new();
        let mut ordinals_by_name = HashMap::new();
        let mut named_nets = BTreeMap::new();
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
            if ordinals_by_name
                .insert(net.name.clone(), net.ordinal)
                .is_some()
            {
                return Err(DesignBuildError::DuplicateNetName(net.name.clone()));
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
            let envelope_override = options
                .footprint_envelopes
                .get(&footprint.id)
                .or_else(|| options.footprint_envelopes.get(&footprint.reference))
                .copied();
            let front_courtyard = footprint.front_courtyard_envelope.map(convert_bounds);
            let back_courtyard = footprint.back_courtyard_envelope.map(convert_bounds);
            let parsed_union = match (front_courtyard, back_courtyard) {
                (Some(front), Some(back)) => Some(union_envelopes(front, back)),
                (Some(front), None) => Some(front),
                (None, Some(back)) => Some(back),
                (None, None) => None,
            };
            let envelope = if let Some(envelope_override) = envelope_override {
                if parsed_union.is_some_and(|parsed| !envelope_contains(envelope_override, parsed))
                {
                    return Err(DesignBuildError::EnvelopeOverrideTooSmall {
                        footprint_id: footprint.id.clone(),
                        reference: footprint.reference.clone(),
                    });
                }
                envelope_override
            } else {
                let front = front_courtyard.ok_or_else(|| DesignBuildError::MissingEnvelope {
                    footprint_id: footprint.id.clone(),
                    reference: footprint.reference.clone(),
                })?;
                back_courtyard
                    .map(|back| union_envelopes(front, back))
                    .unwrap_or(front)
            };
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
                } else if let Some(name) = pad.net_name.as_deref().filter(|name| !name.is_empty()) {
                    let net = if let Some(ordinal) = ordinals_by_name.get(name) {
                        nets_by_ordinal
                            .get_mut(ordinal)
                            .expect("name index references an established ordinal net")
                    } else {
                        named_nets.entry(name.to_string()).or_insert_with(|| Net {
                            id: named_net_id(name),
                            name: name.to_string(),
                            terminals: Vec::new(),
                        })
                    };
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

        let mut placement_rules = options.placement_rules.clone();
        placement_rules.edge_clearance_mm += outline_approximation_guard;
        let mut routing_rules = options.routing_rules.clone();
        routing_rules.edge_clearance_mm += outline_approximation_guard;

        let design = Design {
            schema_version: DESIGN_SCHEMA_VERSION,
            name: options.name.clone(),
            outline,
            placement_keepouts: options.placement_keepouts.clone(),
            components,
            nets: nets_by_ordinal
                .into_values()
                .chain(named_nets.into_values())
                .collect(),
            placement_rules,
            routing_rules,
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

fn named_net_id(name: &str) -> String {
    format!("kicad-net-name:{name}")
}

fn convert_bounds(bounds: crate::LocalBounds) -> LocalAabb {
    LocalAabb {
        min_x: bounds.min_x,
        min_y: bounds.min_y,
        max_x: bounds.max_x,
        max_y: bounds.max_y,
    }
}

fn union_envelopes(first: LocalAabb, second: LocalAabb) -> LocalAabb {
    LocalAabb {
        min_x: first.min_x.min(second.min_x),
        min_y: first.min_y.min(second.min_y),
        max_x: first.max_x.max(second.max_x),
        max_y: first.max_y.max(second.max_y),
    }
}

fn envelope_contains(outer: LocalAabb, inner: LocalAabb) -> bool {
    const EPSILON: f64 = 1.0e-9;
    outer.min_x <= inner.min_x + EPSILON
        && outer.min_y <= inner.min_y + EPSILON
        && outer.max_x + EPSILON >= inner.max_x
        && outer.max_y + EPSILON >= inner.max_y
}

fn convert_outline(edge_cuts: &[EdgePrimitive]) -> Result<Polygon, DesignBuildError> {
    let vertices =
        geometry::assemble_outline(edge_cuts).map_err(DesignBuildError::UnsupportedOutline)?;
    Ok(Polygon {
        vertices: vertices
            .into_iter()
            .map(|point| Point {
                x: point.x,
                y: point.y,
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use layout_core::{JumperMode, PlacementStatus};

    const SAMPLE: &str = include_str!("../../../public/sample-sensor.kicad_pcb");

    fn options() -> DesignBuildOptions {
        DesignBuildOptions {
            name: "sample".into(),
            footprint_envelopes: BTreeMap::new(),
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
        assert!((design.components[0].envelope.min_x + 2.0).abs() < 1.0e-9);
        assert!((design.components[0].envelope.max_y - 29.0).abs() < 1.0e-9);
        assert!((design.placement_rules.edge_clearance_mm - 1.0).abs() < 1.0e-12);
        assert!((design.routing_rules.edge_clearance_mm - 1.0).abs() < 1.0e-12);
        let placement = layout_core::place(
            &design,
            layout_core::PlacementOptions {
                seed: 7,
                restarts: 1,
                refinement_passes: 0,
                max_grid_points: 10_000,
            },
        )
        .unwrap();
        assert_eq!(placement.status, PlacementStatus::Complete);
    }

    #[test]
    fn missing_footprint_envelope_fails_closed() {
        let mut board = BoardDocument::parse(SAMPLE).unwrap();
        board.footprints[1].front_courtyard_envelope = None;
        assert!(matches!(
            board.to_layout_design(&options()),
            Err(DesignBuildError::MissingEnvelope { reference, .. }) if reference == "J2"
        ));
    }

    #[test]
    fn unions_back_courtyard_and_rejects_smaller_override() {
        let source = r#"(kicad_pcb
          (footprint "L:TwoSided" (layer "F.Cu") (at 10 10)
            (property "Reference" "J1")
            (fp_rect (start -1 -1) (end 1 1)
              (stroke (width 0.05) (type solid)) (fill none) (layer "F.CrtYd"))
            (fp_rect (start -2 -1.5) (end 2 1.5)
              (stroke (width 0.05) (type solid)) (fill none) (layer "B.CrtYd")))
          (gr_rect (start 0 0) (end 20 20)
            (stroke (width 0.05) (type solid)) (fill none) (layer "Edge.Cuts")))"#;
        let board = BoardDocument::parse(source).unwrap();
        let design = board.to_layout_design(&options()).unwrap();
        assert_eq!(
            design.components[0].envelope,
            LocalAabb {
                min_x: -2.0,
                min_y: -1.5,
                max_x: 2.0,
                max_y: 1.5,
            }
        );

        let mut too_small = options();
        too_small.footprint_envelopes.insert(
            "J1".into(),
            LocalAabb {
                min_x: -1.0,
                min_y: -1.0,
                max_x: 1.0,
                max_y: 1.0,
            },
        );
        assert!(matches!(
            board.to_layout_design(&too_small),
            Err(DesignBuildError::EnvelopeOverrideTooSmall { reference, .. }) if reference == "J1"
        ));
    }

    #[test]
    fn back_courtyard_does_not_replace_missing_front_courtyard() {
        let source = r#"(kicad_pcb
          (footprint "L:BackOnly" (layer "F.Cu") (at 10 10)
            (property "Reference" "J1")
            (fp_rect (start -2 -2) (end 2 2)
              (stroke (width 0.05) (type solid)) (fill none) (layer "B.CrtYd")))
          (gr_rect (start 0 0) (end 20 20)
            (stroke (width 0.05) (type solid)) (fill none) (layer "Edge.Cuts")))"#;
        let board = BoardDocument::parse(source).unwrap();
        assert!(matches!(
            board.to_layout_design(&options()),
            Err(DesignBuildError::MissingEnvelope { reference, .. }) if reference == "J1"
        ));
    }

    #[test]
    fn converts_kicad_ten_name_based_pad_nets() {
        let source = r#"(kicad_pcb
          (footprint "L:One" (layer "F.Cu") (at 5 5)
            (property "Reference" "J1")
            (fp_rect (start -2 -2) (end 2 2)
              (stroke (width 0.05) (type solid)) (fill none) (layer "F.CrtYd"))
            (pad "1" thru_hole circle (at 0 0) (size 2 2) (drill 1)
              (layers "*.Cu" "*.Mask") (net "GND")))
          (footprint "L:Two" (layer "F.Cu") (at 15 5)
            (property "Reference" "J2")
            (fp_rect (start -2 -2) (end 2 2)
              (stroke (width 0.05) (type solid)) (fill none) (layer "F.CrtYd"))
            (pad "1" thru_hole circle (at 0 0) (size 2 2) (drill 1)
              (layers "*.Cu" "*.Mask") (net "GND")))
          (gr_rect (start 0 0) (end 20 10)
            (stroke (width 0.05) (type solid)) (fill none) (layer "Edge.Cuts")))"#;
        let board = BoardDocument::parse(source).unwrap();
        assert_eq!(board.footprints[0].pads[0].net_name.as_deref(), Some("GND"));
        assert_eq!(board.footprints[0].pads[0].net_ordinal, None);
        let design = board.to_layout_design(&options()).unwrap();
        assert_eq!(design.nets.len(), 1);
        assert_eq!(design.nets[0].id, "kicad-net-name:GND");
        assert_eq!(design.nets[0].terminals.len(), 2);
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

    #[test]
    fn tessellates_rounded_rectangle_and_guards_clearance() {
        let source = r#"(kicad_pcb
          (gr_rect (start 173.5 44.5) (end 221.5 96.5) (radius 1.2)
            (stroke (width 0.05) (type solid)) (fill none)
            (layer "Edge.Cuts")))"#;
        let board = BoardDocument::parse(source).unwrap();
        let design = board.to_layout_design(&DesignBuildOptions {
            name: "rounded rectangle".into(),
            footprint_envelopes: BTreeMap::new(),
            allowed_rotations_degrees: BTreeMap::new(),
            placement_keepouts: Vec::new(),
            placement_rules: options().placement_rules,
            routing_rules: options().routing_rules,
            jumper_policy: options().jumper_policy,
        });
        let design = design.unwrap();
        assert!(design.outline.vertices.len() > 70);
        assert!((design.placement_rules.edge_clearance_mm - 1.005003).abs() < 1.0e-12);
        let min_x = design
            .outline
            .vertices
            .iter()
            .map(|point| point.x)
            .fold(f64::INFINITY, f64::min);
        let max_x = design
            .outline
            .vertices
            .iter()
            .map(|point| point.x)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!((min_x - 173.5).abs() < 1.0e-9);
        assert!((max_x - 221.5).abs() < 1.0e-9);
    }

    #[test]
    fn rejects_multiple_edge_loops_and_degenerate_arcs() {
        let multiple = r#"(kicad_pcb
          (gr_circle (center 0 0) (end 10 0)
            (stroke (width 0.05) (type solid)) (fill none) (layer "Edge.Cuts"))
          (gr_circle (center 20 0) (end 21 0)
            (stroke (width 0.05) (type solid)) (fill none) (layer "Edge.Cuts")))"#;
        let board = BoardDocument::parse(multiple).unwrap();
        assert!(matches!(
            board.to_layout_design(&DesignBuildOptions {
                name: "multiple".into(),
                footprint_envelopes: BTreeMap::new(),
                allowed_rotations_degrees: BTreeMap::new(),
                placement_keepouts: Vec::new(),
                placement_rules: options().placement_rules,
                routing_rules: options().routing_rules,
                jumper_policy: options().jumper_policy,
            }),
            Err(DesignBuildError::UnsupportedOutline(message)) if message.contains("multiple loops")
        ));

        let degenerate = r#"(kicad_pcb
          (gr_arc (start 0 0) (mid 1 0) (end 2 0)
            (stroke (width 0.05) (type solid)) (layer "Edge.Cuts"))
          (gr_line (start 2 0) (end 0 0)
            (stroke (width 0.05) (type solid)) (layer "Edge.Cuts")))"#;
        let board = BoardDocument::parse(degenerate).unwrap();
        assert!(matches!(
            board.to_layout_design(&DesignBuildOptions {
                name: "degenerate".into(),
                footprint_envelopes: BTreeMap::new(),
                allowed_rotations_degrees: BTreeMap::new(),
                placement_keepouts: Vec::new(),
                placement_rules: options().placement_rules,
                routing_rules: options().routing_rules,
                jumper_policy: options().jumper_policy,
            }),
            Err(DesignBuildError::UnsupportedOutline(message)) if message.contains("collinear")
        ));
    }
}
