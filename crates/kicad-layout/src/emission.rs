use crate::{
    format_number, geometry, BoardDocument, DesignBuildOptions, EdgePrimitive, KicadError,
    PatchOptions, Pose, Span,
};
use layout_core::{validate_route_solution, Design, PlacementOutcome, RouteSegment, RouteSolution};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

const HASH_DOMAIN: &[u8] = b"copperline:kicad-track:v1";

#[derive(Debug, Clone)]
enum NetReference {
    Ordinal(i64),
    Name(String),
}

impl BoardDocument {
    /// Rewrite footprint poses and insert a complete validated B.Cu route.
    ///
    /// The original document is never reserialized. Placement edits are
    /// limited to immediate footprint `(at ...)` expressions and generated
    /// segments are inserted immediately before the root closing parenthesis.
    /// Existing routed copper, partial routes, and jumper-bearing candidates
    /// fail closed because they need explicit product workflows of their own.
    pub fn rewrite_routed_candidate(
        &self,
        design: &Design,
        placement: &PlacementOutcome,
        route: &RouteSolution,
        options: PatchOptions,
    ) -> Result<String, KicadError> {
        self.validate_design_binding(design)?;
        validate_route_solution(design, placement, route).map_err(|error| {
            KicadError::Patch(format!(
                "route candidate failed independent validation: {error}"
            ))
        })?;
        if !route.unrouted_net_ids.is_empty() {
            return Err(KicadError::Patch(format!(
                "cannot emit a partial route with {} unrouted net(s)",
                route.unrouted_net_ids.len()
            )));
        }
        if !route.jumpers.is_empty() {
            return Err(KicadError::Patch(
                "cannot encode reviewed physical jumpers as KiCad copper segments".into(),
            ));
        }
        if self.existing_copper_items != 0 && !route.segments.is_empty() {
            return Err(KicadError::Patch(format!(
                "board already contains {} routed copper object(s); generated routing cannot be merged implicitly",
                self.existing_copper_items
            )));
        }

        let placements = placement
            .placements
            .iter()
            .map(|(id, pose)| {
                (
                    id.clone(),
                    Pose {
                        x: pose.x,
                        y: pose.y,
                        rotation_degrees: pose.rotation_degrees,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut patches = self.placement_patches(&placements, options)?;

        let segments = canonical_segments(route)?;
        if !segments.is_empty() {
            let net_references = self.net_references(design)?;
            let insertion = self.render_segments(&segments, &net_references)?;
            patches.push((
                Span {
                    start: self.root_insertion_offset,
                    end: self.root_insertion_offset,
                },
                insertion,
            ));
        }

        let output = self.apply_patches(patches);
        let reparsed = Self::parse(&output).map_err(|error| {
            KicadError::Patch(format!(
                "internal error: emitted candidate did not reparse: {error}"
            ))
        })?;
        if reparsed.existing_copper_items != self.existing_copper_items + segments.len() {
            return Err(KicadError::Patch(
                "internal error: emitted candidate did not contain the expected segment count"
                    .into(),
            ));
        }
        Ok(output)
    }

    fn validate_design_binding(&self, design: &Design) -> Result<(), KicadError> {
        let guard = if self.edge_cuts.iter().any(|primitive| match primitive {
            EdgePrimitive::Arc { .. } | EdgePrimitive::Circle { .. } => true,
            EdgePrimitive::Rectangle { radius, .. } => *radius > 0.0,
            EdgePrimitive::Polygon { .. } | EdgePrimitive::Line { .. } => false,
        }) {
            geometry::OUTLINE_APPROXIMATION_GUARD_MM
        } else {
            0.0
        };
        if design.placement_rules.edge_clearance_mm + f64::EPSILON < guard
            || design.routing_rules.edge_clearance_mm + f64::EPSILON < guard
        {
            return Err(KicadError::Patch(
                "design is missing the required curved-outline clearance guard".into(),
            ));
        }

        let footprint_envelopes = design
            .components
            .iter()
            .map(|component| (component.id.clone(), component.envelope))
            .collect();
        let allowed_rotations_degrees = design
            .components
            .iter()
            .map(|component| {
                (
                    component.id.clone(),
                    component.allowed_rotations_degrees.clone(),
                )
            })
            .collect();
        let mut placement_rules = design.placement_rules.clone();
        placement_rules.edge_clearance_mm -= guard;
        let mut routing_rules = design.routing_rules.clone();
        routing_rules.edge_clearance_mm -= guard;
        let rebuilt = self
            .to_layout_design(&DesignBuildOptions {
                name: design.name.clone(),
                footprint_envelopes,
                allowed_rotations_degrees,
                placement_keepouts: design.placement_keepouts.clone(),
                placement_rules,
                routing_rules,
                jumper_policy: design.jumper_policy.clone(),
            })
            .map_err(|error| {
                KicadError::Patch(format!(
                    "candidate design cannot be reconstructed from this board: {error}"
                ))
            })?;
        if rebuilt != *design {
            return Err(KicadError::Patch(
                "candidate design does not exactly match this board's authoritative geometry and connectivity"
                    .into(),
            ));
        }
        Ok(())
    }

    fn net_references(
        &self,
        design: &Design,
    ) -> Result<BTreeMap<String, NetReference>, KicadError> {
        let mut references = BTreeMap::new();
        for net in &design.nets {
            let reference = if let Some(name) = net.id.strip_prefix("kicad-net-name:") {
                if name != net.name {
                    return Err(KicadError::Patch(format!(
                        "canonical net ID {} does not match net name {}",
                        net.id, net.name
                    )));
                }
                NetReference::Name(net.name.clone())
            } else if let Some(ordinal) = net
                .id
                .strip_prefix("kicad-net-")
                .and_then(|value| value.parse::<i64>().ok())
            {
                let source_net = self
                    .nets
                    .iter()
                    .find(|source_net| source_net.ordinal == ordinal)
                    .ok_or_else(|| {
                        KicadError::Patch(format!(
                            "canonical net {} has no source KiCad ordinal",
                            net.id
                        ))
                    })?;
                if source_net.name != net.name {
                    return Err(KicadError::Patch(format!(
                        "canonical net {} changed source name {} to {}",
                        net.id, source_net.name, net.name
                    )));
                }
                NetReference::Ordinal(ordinal)
            } else {
                return Err(KicadError::Patch(format!(
                    "net {} is not a canonical KiCad net ID",
                    net.id
                )));
            };
            references.insert(net.id.clone(), reference);
        }
        Ok(references)
    }

    fn render_segments(
        &self,
        segments: &[RouteSegment],
        net_references: &BTreeMap<String, NetReference>,
    ) -> Result<String, KicadError> {
        let newline = if self.source.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let mut insertion = String::new();
        if !self.source[..self.root_insertion_offset].ends_with('\n') {
            insertion.push_str(newline);
        }

        let board_digest = Sha256::digest(self.source.as_bytes());
        let mut emitted_uuids = HashSet::new();
        for (index, segment) in segments.iter().enumerate() {
            let net_reference = net_references.get(&segment.net_id).ok_or_else(|| {
                KicadError::Patch(format!(
                    "segment references unmapped canonical net {}",
                    segment.net_id
                ))
            })?;
            let net_expression = match net_reference {
                NetReference::Ordinal(ordinal) => ordinal.to_string(),
                NetReference::Name(name) => quote_atom(name)?,
            };
            let uuid = (0_u32..1024)
                .map(|nonce| track_uuid(&board_digest, segment, index, nonce))
                .find(|uuid| {
                    !self.existing_uuids.contains(uuid) && emitted_uuids.insert(uuid.clone())
                })
                .ok_or_else(|| {
                    KicadError::Patch(
                        "could not allocate a deterministic non-colliding track UUID".into(),
                    )
                })?;
            insertion.push_str(&self.top_level_indent);
            insertion.push_str("(segment (start ");
            insertion.push_str(&format_number(segment.start.x));
            insertion.push(' ');
            insertion.push_str(&format_number(segment.start.y));
            insertion.push_str(") (end ");
            insertion.push_str(&format_number(segment.end.x));
            insertion.push(' ');
            insertion.push_str(&format_number(segment.end.y));
            insertion.push_str(") (width ");
            insertion.push_str(&format_number(segment.width_mm));
            insertion.push_str(") (layer \"B.Cu\") (net ");
            insertion.push_str(&net_expression);
            insertion.push_str(") (uuid \"");
            insertion.push_str(&uuid);
            insertion.push_str("\"))");
            insertion.push_str(newline);
        }
        Ok(insertion)
    }
}

fn canonical_segments(route: &RouteSolution) -> Result<Vec<RouteSegment>, KicadError> {
    let mut segments = route.segments.clone();
    for segment in &mut segments {
        if point_order(&segment.end, &segment.start) == Ordering::Less {
            std::mem::swap(&mut segment.start, &mut segment.end);
        }
    }
    segments.sort_by(|first, second| {
        first
            .net_id
            .cmp(&second.net_id)
            .then_with(|| point_order(&first.start, &second.start))
            .then_with(|| point_order(&first.end, &second.end))
            .then_with(|| first.width_mm.total_cmp(&second.width_mm))
    });
    if segments.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(KicadError::Patch(
            "route candidate contains duplicate copper segments".into(),
        ));
    }
    Ok(segments)
}

fn point_order(first: &layout_core::Point, second: &layout_core::Point) -> Ordering {
    first
        .x
        .total_cmp(&second.x)
        .then_with(|| first.y.total_cmp(&second.y))
}

fn quote_atom(value: &str) -> Result<String, KicadError> {
    let mut quoted = String::from("\"");
    for character in value.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            control if control.is_control() => {
                return Err(KicadError::Patch(
                    "KiCad net name contains an unsupported control character".into(),
                ))
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    Ok(quoted)
}

fn track_uuid(board_digest: &[u8], segment: &RouteSegment, index: usize, nonce: u32) -> String {
    let mut hasher = Sha256::new();
    hash_field(&mut hasher, HASH_DOMAIN);
    hash_field(&mut hasher, board_digest);
    hash_field(&mut hasher, segment.net_id.as_bytes());
    hash_field(&mut hasher, format_number(segment.start.x).as_bytes());
    hash_field(&mut hasher, format_number(segment.start.y).as_bytes());
    hash_field(&mut hasher, format_number(segment.end.x).as_bytes());
    hash_field(&mut hasher, format_number(segment.end.y).as_bytes());
    hash_field(&mut hasher, format_number(segment.width_mm).as_bytes());
    hasher.update((index as u64).to_le_bytes());
    hasher.update(nonce.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn hash_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}
