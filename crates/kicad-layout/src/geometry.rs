use crate::{EdgePrimitive, LocalBounds, Point};
use std::cmp::Ordering;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

const GEOMETRY_EPSILON_MM: f64 = 1.0e-9;
const ANGLE_EPSILON_RADIANS: f64 = 1.0e-12;
const OUTLINE_JOIN_TOLERANCE_MM: f64 = GEOMETRY_EPSILON_MM;
const COURTYARD_JOIN_TOLERANCE_MM: f64 = 0.02;
const COORDINATE_QUANTUM_MM: f64 = 1.0e-6;
const ARC_CHORD_TOLERANCE_MM: f64 = 0.005;
pub(crate) const OUTLINE_APPROXIMATION_GUARD_MM: f64 =
    ARC_CHORD_TOLERANCE_MM + 3.0 * COORDINATE_QUANTUM_MM;
const MAX_ARC_STEP_RADIANS: f64 = PI / 36.0;
const MAX_CURVE_SEGMENTS: usize = 4096;
const MAX_OUTLINE_VERTICES: usize = 4096;

#[derive(Debug, Clone, Copy)]
struct ArcGeometry {
    center: Point,
    radius: f64,
    start_angle: f64,
    sweep_angle: f64,
}

pub(crate) fn primitive_bounds(primitive: &EdgePrimitive) -> Result<LocalBounds, String> {
    match primitive {
        EdgePrimitive::Rectangle { start, end, radius } => {
            validate_rectangle(*start, *end, *radius)?;
            Ok(bounds_from_points([*start, *end]))
        }
        EdgePrimitive::Line { start, end } => {
            ensure_distinct(*start, *end, "graphic endpoints coincide")?;
            Ok(bounds_from_points([*start, *end]))
        }
        EdgePrimitive::Polygon { points } => {
            let points = normalize_polygon(points.clone());
            validate_polygon(&points)?;
            Ok(bounds_from_points(points))
        }
        EdgePrimitive::Arc { start, mid, end } => arc_bounds(*start, *mid, *end),
        EdgePrimitive::Circle { center, end } => {
            let radius = distance(*center, *end);
            if !radius.is_finite() || radius <= GEOMETRY_EPSILON_MM {
                return Err("circle has a zero or non-finite radius".into());
            }
            Ok(LocalBounds {
                min_x: center.x - radius,
                min_y: center.y - radius,
                max_x: center.x + radius,
                max_y: center.y + radius,
            })
        }
    }
}

pub(crate) fn validate_courtyard(primitives: &[EdgePrimitive]) -> Result<(), String> {
    if primitives.is_empty() {
        return Err("courtyard contains no supported geometry".into());
    }

    let mut open_primitives = Vec::new();
    for primitive in primitives {
        primitive_bounds(primitive)?;
        match primitive {
            EdgePrimitive::Rectangle { .. } => {}
            EdgePrimitive::Polygon { .. } | EdgePrimitive::Circle { .. } => {}
            EdgePrimitive::Line { .. } | EdgePrimitive::Arc { .. } => {
                open_primitives.push(primitive.clone());
            }
        }
    }

    if !open_primitives.is_empty() {
        for outline in stitch_open_primitives(&open_primitives, COURTYARD_JOIN_TOLERANCE_MM)? {
            validate_polygon(&outline)
                .map_err(|message| format!("courtyard chain is invalid: {message}"))?;
        }
    }
    Ok(())
}

pub(crate) fn assemble_outline(primitives: &[EdgePrimitive]) -> Result<Vec<Point>, String> {
    let outline = match primitives {
        [EdgePrimitive::Rectangle { start, end, radius }] => {
            tessellate_rectangle(*start, *end, *radius)?
        }
        [EdgePrimitive::Polygon { points }] => normalize_polygon(points.clone()),
        [EdgePrimitive::Circle { center, end }] => tessellate_circle(*center, *end)?,
        [] => return Err("no Edge.Cuts primitives were found".into()),
        primitives
            if primitives.iter().all(|primitive| {
                matches!(primitive, EdgePrimitive::Line { .. } | EdgePrimitive::Arc { .. })
            }) =>
        {
            let mut outlines = stitch_open_primitives(primitives, OUTLINE_JOIN_TOLERANCE_MM)?;
            if outlines.len() != 1 {
                return Err(format!(
                    "expected one outer Edge.Cuts loop, found {}",
                    outlines.len()
                ));
            }
            outlines.pop().expect("one outline was established")
        }
        _ => {
            return Err(
                "v1 accepts one rectangle, polygon, circle, or one closed line/arc chain; multiple loops and mixed closed primitives are not supported"
                    .into(),
            )
        }
    };

    let outline = canonicalize_polygon(outline);
    validate_polygon(&outline).map_err(|message| format!("outline is invalid: {message}"))?;
    Ok(outline)
}

fn stitch_open_primitives(
    primitives: &[EdgePrimitive],
    join_tolerance_mm: f64,
) -> Result<Vec<Vec<Point>>, String> {
    let mut remaining = primitives.to_vec();
    let mut outlines = Vec::new();

    while !remaining.is_empty() {
        let first = remaining.remove(0);
        let (loop_start, _) = endpoints(&first).ok_or_else(|| {
            "only line and arc primitives can participate in an open chain".to_string()
        })?;
        let mut points = tessellate_oriented(&first, false)?;

        loop {
            let current = *points
                .last()
                .ok_or_else(|| "outline chain produced no points".to_string())?;
            if points_within(current, loop_start, join_tolerance_mm) {
                points[0] = midpoint(current, loop_start);
                points.pop();
                break;
            }

            let mut matches = Vec::new();
            for (index, primitive) in remaining.iter().enumerate() {
                let Some((start, end)) = endpoints(primitive) else {
                    continue;
                };
                if points_within(current, start, join_tolerance_mm) {
                    matches.push((index, false));
                }
                if points_within(current, end, join_tolerance_mm) {
                    matches.push((index, true));
                }
            }
            match matches.as_slice() {
                [] => return Err("graphics do not form closed connected loops".into()),
                [(index, reverse)] => {
                    let primitive = remaining.remove(*index);
                    let mut sampled = tessellate_oriented(&primitive, *reverse)?;
                    let sampled_start = *sampled
                        .first()
                        .ok_or_else(|| "outline chain produced no points".to_string())?;
                    let join = midpoint(current, sampled_start);
                    *points
                        .last_mut()
                        .expect("outline chain has a current point") = join;
                    sampled[0] = join;
                    points.extend(sampled.into_iter().skip(1));
                    if points.len() > MAX_OUTLINE_VERTICES + 1 {
                        return Err(format!(
                            "geometry exceeds the safety limit of {MAX_OUTLINE_VERTICES} vertices"
                        ));
                    }
                }
                _ => return Err("graphics form an ambiguous or branching junction".into()),
            }
        }

        outlines.push(points);
    }

    Ok(outlines)
}

fn endpoints(primitive: &EdgePrimitive) -> Option<(Point, Point)> {
    match primitive {
        EdgePrimitive::Line { start, end } | EdgePrimitive::Arc { start, end, .. } => {
            Some((*start, *end))
        }
        _ => None,
    }
}

fn tessellate_oriented(primitive: &EdgePrimitive, reverse: bool) -> Result<Vec<Point>, String> {
    match primitive {
        EdgePrimitive::Line { start, end } => {
            ensure_distinct(*start, *end, "line has coincident endpoints")?;
            if reverse {
                Ok(vec![*end, *start])
            } else {
                Ok(vec![*start, *end])
            }
        }
        EdgePrimitive::Arc { start, mid, end } => {
            if reverse {
                tessellate_arc(*end, *mid, *start)
            } else {
                tessellate_arc(*start, *mid, *end)
            }
        }
        _ => Err("closed primitives cannot be stitched into a line/arc chain".into()),
    }
}

fn tessellate_arc(start: Point, mid: Point, end: Point) -> Result<Vec<Point>, String> {
    let (arc, reversed, canonical_start, canonical_end) = canonical_arc_geometry(start, mid, end)?;
    let segments = curve_segment_count(arc.radius, arc.sweep_angle.abs())?;
    let mut points = Vec::with_capacity(segments + 1);
    points.push(canonical_start);
    for index in 1..segments {
        let fraction = index as f64 / segments as f64;
        let angle = arc.start_angle + arc.sweep_angle * fraction;
        points.push(Point {
            x: arc.center.x + arc.radius * angle.cos(),
            y: arc.center.y + arc.radius * angle.sin(),
        });
    }
    points.push(canonical_end);
    if reversed {
        points.reverse();
    }
    Ok(points)
}

fn tessellate_circle(center: Point, end: Point) -> Result<Vec<Point>, String> {
    let radius = distance(center, end);
    if !radius.is_finite() || radius <= GEOMETRY_EPSILON_MM {
        return Err("circle has a zero or non-finite radius".into());
    }
    let segments = curve_segment_count(radius, TAU)?;
    Ok((0..segments)
        .map(|index| {
            let angle = TAU * index as f64 / segments as f64;
            Point {
                x: center.x + radius * angle.cos(),
                y: center.y + radius * angle.sin(),
            }
        })
        .collect())
}

fn tessellate_rectangle(start: Point, end: Point, radius: f64) -> Result<Vec<Point>, String> {
    validate_rectangle(start, end, radius)?;
    let min_x = start.x.min(end.x);
    let min_y = start.y.min(end.y);
    let max_x = start.x.max(end.x);
    let max_y = start.y.max(end.y);
    if radius <= GEOMETRY_EPSILON_MM {
        return Ok(vec![
            Point { x: min_x, y: min_y },
            Point { x: max_x, y: min_y },
            Point { x: max_x, y: max_y },
            Point { x: min_x, y: max_y },
        ]);
    }

    let mut points = vec![Point {
        x: min_x + radius,
        y: min_y,
    }];
    points.push(Point {
        x: max_x - radius,
        y: min_y,
    });
    append_center_arc(
        &mut points,
        Point {
            x: max_x - radius,
            y: min_y + radius,
        },
        radius,
        -FRAC_PI_2,
        FRAC_PI_2,
    )?;
    points.push(Point {
        x: max_x,
        y: max_y - radius,
    });
    append_center_arc(
        &mut points,
        Point {
            x: max_x - radius,
            y: max_y - radius,
        },
        radius,
        0.0,
        FRAC_PI_2,
    )?;
    points.push(Point {
        x: min_x + radius,
        y: max_y,
    });
    append_center_arc(
        &mut points,
        Point {
            x: min_x + radius,
            y: max_y - radius,
        },
        radius,
        FRAC_PI_2,
        FRAC_PI_2,
    )?;
    points.push(Point {
        x: min_x,
        y: min_y + radius,
    });
    append_center_arc(
        &mut points,
        Point {
            x: min_x + radius,
            y: min_y + radius,
        },
        radius,
        PI,
        FRAC_PI_2,
    )?;
    if points_close(
        points[0],
        *points.last().expect("rounded rectangle has points"),
    ) {
        points.pop();
    }
    Ok(points)
}

fn append_center_arc(
    points: &mut Vec<Point>,
    center: Point,
    radius: f64,
    start_angle: f64,
    sweep_angle: f64,
) -> Result<(), String> {
    let segments = curve_segment_count(radius, sweep_angle.abs())?;
    for index in 1..=segments {
        let angle = start_angle + sweep_angle * index as f64 / segments as f64;
        points.push(Point {
            x: center.x + radius * angle.cos(),
            y: center.y + radius * angle.sin(),
        });
    }
    Ok(())
}

fn validate_rectangle(start: Point, end: Point, radius: f64) -> Result<(), String> {
    ensure_finite(start, "rectangle start")?;
    ensure_finite(end, "rectangle end")?;
    let width = (start.x - end.x).abs();
    let height = (start.y - end.y).abs();
    if width <= GEOMETRY_EPSILON_MM || height <= GEOMETRY_EPSILON_MM {
        return Err("rectangle has zero width or height".into());
    }
    if !radius.is_finite() || radius < 0.0 {
        return Err("rectangle has an invalid radius".into());
    }
    let maximum_radius = width.min(height) * 0.5;
    if radius > maximum_radius + GEOMETRY_EPSILON_MM {
        return Err(format!(
            "rectangle radius {radius} exceeds the maximum {maximum_radius}"
        ));
    }
    Ok(())
}

fn curve_segment_count(radius: f64, sweep: f64) -> Result<usize, String> {
    let sagitta_step = if ARC_CHORD_TOLERANCE_MM >= radius {
        PI
    } else {
        2.0 * (1.0 - ARC_CHORD_TOLERANCE_MM / radius).acos()
    };
    let max_step = sagitta_step.min(MAX_ARC_STEP_RADIANS);
    if !max_step.is_finite() || max_step <= ANGLE_EPSILON_RADIANS {
        return Err("curve radius is too large to tessellate safely".into());
    }
    let segments = (sweep / max_step).ceil().max(1.0) as usize;
    if segments > MAX_CURVE_SEGMENTS {
        return Err(format!(
            "curve requires {segments} segments, exceeding the safety limit of {MAX_CURVE_SEGMENTS}"
        ));
    }
    Ok(segments)
}

fn arc_bounds(start: Point, mid: Point, end: Point) -> Result<LocalBounds, String> {
    let (arc, _, _, _) = canonical_arc_geometry(start, mid, end)?;
    let mut points = vec![start, end];
    for angle in [0.0, FRAC_PI_2, PI, 3.0 * FRAC_PI_2] {
        if angle_on_sweep(angle, arc.start_angle, arc.sweep_angle) {
            points.push(Point {
                x: arc.center.x + arc.radius * angle.cos(),
                y: arc.center.y + arc.radius * angle.sin(),
            });
        }
    }
    Ok(bounds_from_points(points))
}

fn canonical_arc_geometry(
    start: Point,
    mid: Point,
    end: Point,
) -> Result<(ArcGeometry, bool, Point, Point), String> {
    let reversed = compare_points(start, end) == Ordering::Greater;
    let (canonical_start, canonical_end) = if reversed { (end, start) } else { (start, end) };
    let geometry = arc_geometry_ordered(canonical_start, mid, canonical_end)?;
    Ok((geometry, reversed, canonical_start, canonical_end))
}

fn arc_geometry_ordered(start: Point, mid: Point, end: Point) -> Result<ArcGeometry, String> {
    ensure_finite(start, "arc start")?;
    ensure_finite(mid, "arc midpoint")?;
    ensure_finite(end, "arc end")?;
    ensure_distinct(start, mid, "arc start and midpoint coincide")?;
    ensure_distinct(mid, end, "arc midpoint and end coincide")?;
    ensure_distinct(start, end, "arc start and end coincide")?;

    let bx = mid.x - start.x;
    let by = mid.y - start.y;
    let cx = end.x - start.x;
    let cy = end.y - start.y;
    let determinant = 2.0 * (bx * cy - by * cx);
    let scale = bx.hypot(by).max(cx.hypot(cy)).max(1.0);
    if determinant.abs() <= 1.0e-12 * scale * scale {
        return Err("arc points are collinear or numerically degenerate".into());
    }

    let b_length_squared = bx * bx + by * by;
    let c_length_squared = cx * cx + cy * cy;
    let center = Point {
        x: start.x + (b_length_squared * cy - c_length_squared * by) / determinant,
        y: start.y + (bx * c_length_squared - cx * b_length_squared) / determinant,
    };
    let radius = distance(center, start);
    if !radius.is_finite() || radius <= GEOMETRY_EPSILON_MM {
        return Err("arc has a zero or non-finite radius".into());
    }

    let start_angle = (start.y - center.y).atan2(start.x - center.x);
    let mid_angle = (mid.y - center.y).atan2(mid.x - center.x);
    let end_angle = (end.y - center.y).atan2(end.x - center.x);
    let counterclockwise_sweep = positive_angle(end_angle - start_angle);
    let counterclockwise_mid = positive_angle(mid_angle - start_angle);
    let sweep_angle = if counterclockwise_mid <= counterclockwise_sweep + ANGLE_EPSILON_RADIANS {
        counterclockwise_sweep
    } else {
        counterclockwise_sweep - TAU
    };
    if sweep_angle.abs() <= ANGLE_EPSILON_RADIANS {
        return Err("arc has a zero sweep".into());
    }

    Ok(ArcGeometry {
        center,
        radius,
        start_angle,
        sweep_angle,
    })
}

fn angle_on_sweep(angle: f64, start_angle: f64, sweep_angle: f64) -> bool {
    if sweep_angle > 0.0 {
        positive_angle(angle - start_angle) <= sweep_angle + ANGLE_EPSILON_RADIANS
    } else {
        positive_angle(start_angle - angle) <= -sweep_angle + ANGLE_EPSILON_RADIANS
    }
}

fn positive_angle(angle: f64) -> f64 {
    angle.rem_euclid(TAU)
}

fn bounds_from_points(points: impl IntoIterator<Item = Point>) -> LocalBounds {
    let mut points = points.into_iter();
    let first = points.next().expect("validated geometry contains a point");
    points.fold(
        LocalBounds {
            min_x: first.x,
            min_y: first.y,
            max_x: first.x,
            max_y: first.y,
        },
        |mut bounds, point| {
            bounds.min_x = bounds.min_x.min(point.x);
            bounds.min_y = bounds.min_y.min(point.y);
            bounds.max_x = bounds.max_x.max(point.x);
            bounds.max_y = bounds.max_y.max(point.y);
            bounds
        },
    )
}

fn normalize_polygon(mut points: Vec<Point>) -> Vec<Point> {
    if points.len() > 1 && points_close(points[0], *points.last().expect("length checked")) {
        points.pop();
    }
    points
}

fn canonicalize_polygon(points: Vec<Point>) -> Vec<Point> {
    let mut canonical = Vec::with_capacity(points.len());
    for point in points {
        let point = Point {
            x: quantize(point.x),
            y: quantize(point.y),
        };
        if canonical.last().is_some_and(|previous| *previous == point) {
            continue;
        }
        canonical.push(point);
    }
    if canonical.len() > 1 && canonical.first() == canonical.last() {
        canonical.pop();
    }
    if polygon_signed_area(&canonical) < 0.0 {
        canonical.reverse();
    }
    if let Some((start, _)) = canonical
        .iter()
        .enumerate()
        .min_by(|(_, first), (_, second)| {
            first
                .x
                .total_cmp(&second.x)
                .then_with(|| first.y.total_cmp(&second.y))
        })
    {
        canonical.rotate_left(start);
    }
    canonical
}

fn quantize(value: f64) -> f64 {
    let quantized = (value / COORDINATE_QUANTUM_MM).round() * COORDINATE_QUANTUM_MM;
    if quantized == -0.0 {
        0.0
    } else {
        quantized
    }
}

fn validate_polygon(points: &[Point]) -> Result<(), String> {
    if points.len() < 3 {
        return Err("polygon has fewer than three vertices".into());
    }
    if points.len() > MAX_OUTLINE_VERTICES {
        return Err(format!(
            "polygon has {} vertices, exceeding the safety limit of {MAX_OUTLINE_VERTICES}",
            points.len()
        ));
    }
    for point in points {
        ensure_finite(*point, "polygon vertex")?;
    }
    if polygon_signed_area(points).abs() <= GEOMETRY_EPSILON_MM * GEOMETRY_EPSILON_MM {
        return Err("polygon has zero area".into());
    }
    if edges(points).any(|(start, end)| points_close(start, end)) {
        return Err("polygon contains a zero-length edge".into());
    }

    let length = points.len();
    for first in 0..length {
        let first_next = (first + 1) % length;
        for second in (first + 1)..length {
            let second_next = (second + 1) % length;
            if first == second
                || first_next == second
                || second_next == first
                || (first == 0 && second_next == 0)
            {
                continue;
            }
            if segments_intersect(
                points[first],
                points[first_next],
                points[second],
                points[second_next],
            ) {
                return Err("polygon self-intersects".into());
            }
        }
    }
    Ok(())
}

fn polygon_signed_area(points: &[Point]) -> f64 {
    edges(points)
        .map(|(first, second)| first.x * second.y - second.x * first.y)
        .sum::<f64>()
        * 0.5
}

fn edges(points: &[Point]) -> impl Iterator<Item = (Point, Point)> + '_ {
    points
        .iter()
        .copied()
        .zip(points.iter().copied().cycle().skip(1))
        .take(points.len())
}

fn segments_intersect(a: Point, b: Point, c: Point, d: Point) -> bool {
    let ab_c = orientation(a, b, c);
    let ab_d = orientation(a, b, d);
    let cd_a = orientation(c, d, a);
    let cd_b = orientation(c, d, b);
    if ((ab_c > GEOMETRY_EPSILON_MM && ab_d < -GEOMETRY_EPSILON_MM)
        || (ab_c < -GEOMETRY_EPSILON_MM && ab_d > GEOMETRY_EPSILON_MM))
        && ((cd_a > GEOMETRY_EPSILON_MM && cd_b < -GEOMETRY_EPSILON_MM)
            || (cd_a < -GEOMETRY_EPSILON_MM && cd_b > GEOMETRY_EPSILON_MM))
    {
        return true;
    }
    (ab_c.abs() <= GEOMETRY_EPSILON_MM && point_on_segment(c, a, b))
        || (ab_d.abs() <= GEOMETRY_EPSILON_MM && point_on_segment(d, a, b))
        || (cd_a.abs() <= GEOMETRY_EPSILON_MM && point_on_segment(a, c, d))
        || (cd_b.abs() <= GEOMETRY_EPSILON_MM && point_on_segment(b, c, d))
}

fn point_on_segment(point: Point, start: Point, end: Point) -> bool {
    orientation(start, end, point).abs() <= GEOMETRY_EPSILON_MM
        && point.x >= start.x.min(end.x) - GEOMETRY_EPSILON_MM
        && point.x <= start.x.max(end.x) + GEOMETRY_EPSILON_MM
        && point.y >= start.y.min(end.y) - GEOMETRY_EPSILON_MM
        && point.y <= start.y.max(end.y) + GEOMETRY_EPSILON_MM
}

fn orientation(a: Point, b: Point, c: Point) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

fn ensure_finite(point: Point, label: &str) -> Result<(), String> {
    if point.x.is_finite() && point.y.is_finite() {
        Ok(())
    } else {
        Err(format!("{label} is non-finite"))
    }
}

fn ensure_distinct(first: Point, second: Point, message: &str) -> Result<(), String> {
    ensure_finite(first, "graphic point")?;
    ensure_finite(second, "graphic point")?;
    if distance(first, second) <= GEOMETRY_EPSILON_MM {
        Err(message.into())
    } else {
        Ok(())
    }
}

fn distance(first: Point, second: Point) -> f64 {
    (first.x - second.x).hypot(first.y - second.y)
}

fn points_close(first: Point, second: Point) -> bool {
    points_within(first, second, OUTLINE_JOIN_TOLERANCE_MM)
}

fn points_within(first: Point, second: Point, tolerance_mm: f64) -> bool {
    distance(first, second) <= tolerance_mm
}

fn midpoint(first: Point, second: Point) -> Point {
    let (first, second) = if compare_points(first, second) == Ordering::Greater {
        (second, first)
    } else {
        (first, second)
    };
    Point {
        x: first.x + (second.x - first.x) * 0.5,
        y: first.y + (second.y - first.y) * 0.5,
    }
}

fn compare_points(first: Point, second: Point) -> Ordering {
    first
        .x
        .total_cmp(&second.x)
        .then_with(|| first.y.total_cmp(&second.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1.0e-9, "{actual} != {expected}");
    }

    #[test]
    fn arc_bounds_include_cardinal_extrema() {
        let bounds = primitive_bounds(&EdgePrimitive::Arc {
            start: Point { x: 0.0, y: 1.0 },
            mid: Point { x: -1.0, y: 0.0 },
            end: Point { x: 0.0, y: -1.0 },
        })
        .unwrap();
        assert_close(bounds.min_x, -1.0);
        assert_close(bounds.max_x, 0.0);
        assert_close(bounds.min_y, -1.0);
        assert_close(bounds.max_y, 1.0);
    }

    #[test]
    fn major_arc_bounds_include_every_crossed_cardinal_extremum() {
        let bounds = primitive_bounds(&EdgePrimitive::Arc {
            start: Point { x: 1.0, y: 0.0 },
            mid: Point { x: 0.0, y: -1.0 },
            end: Point { x: 0.0, y: 1.0 },
        })
        .unwrap();
        assert_close(bounds.min_x, -1.0);
        assert_close(bounds.max_x, 1.0);
        assert_close(bounds.min_y, -1.0);
        assert_close(bounds.max_y, 1.0);
    }

    #[test]
    fn circle_tessellation_honors_sagitta_and_complexity_limits() {
        let center = Point { x: 3.0, y: -7.0 };
        let radius = 100.0;
        let points = tessellate_circle(
            center,
            Point {
                x: center.x + radius,
                y: center.y,
            },
        )
        .unwrap();
        let maximum_sagitta = edges(&points)
            .map(|(start, end)| {
                let midpoint = Point {
                    x: (start.x + end.x) * 0.5,
                    y: (start.y + end.y) * 0.5,
                };
                radius - distance(center, midpoint)
            })
            .fold(0.0, f64::max);
        assert!(maximum_sagitta <= ARC_CHORD_TOLERANCE_MM + 1.0e-12);

        assert!(
            tessellate_circle(Point { x: 0.0, y: 0.0 }, Point { x: 1.0e9, y: 0.0 },)
                .unwrap_err()
                .contains("safety limit")
        );
    }

    #[test]
    fn circle_canonicalization_is_independent_of_radius_point_phase() {
        let x_axis = assemble_outline(&[EdgePrimitive::Circle {
            center: Point { x: 0.0, y: 0.0 },
            end: Point { x: 10.0, y: 0.0 },
        }])
        .unwrap();
        let three_four_five = assemble_outline(&[EdgePrimitive::Circle {
            center: Point { x: 0.0, y: 0.0 },
            end: Point { x: 6.0, y: 8.0 },
        }])
        .unwrap();
        assert_eq!(x_axis, three_four_five);
    }

    #[test]
    fn reversing_a_nonsymmetric_arc_preserves_quantized_samples() {
        let quantized = |points: Vec<Point>| {
            points
                .into_iter()
                .map(|point| Point {
                    x: quantize(point.x),
                    y: quantize(point.y),
                })
                .collect::<Vec<_>>()
        };
        for (start, mid, end) in [
            (
                Point { x: 2.0, y: 1.0 },
                Point { x: 8.0, y: -3.0 },
                Point { x: 11.0, y: 7.0 },
            ),
            (
                Point {
                    x: -1584.063729,
                    y: -989.297409,
                },
                Point {
                    x: -1711.655142,
                    y: -957.501252,
                },
                Point {
                    x: -1583.057873,
                    y: -285.119991,
                },
            ),
        ] {
            let forward = tessellate_arc(start, mid, end).unwrap();
            let mut reversed = tessellate_arc(end, mid, start).unwrap();
            reversed.reverse();
            assert_eq!(quantized(forward), quantized(reversed));

            let forward_outline = assemble_outline(&[
                EdgePrimitive::Arc { start, mid, end },
                EdgePrimitive::Line {
                    start: end,
                    end: start,
                },
            ])
            .unwrap();
            let reversed_outline = assemble_outline(&[
                EdgePrimitive::Line { start, end },
                EdgePrimitive::Arc {
                    start: end,
                    mid,
                    end: start,
                },
            ])
            .unwrap();
            assert_eq!(forward_outline, reversed_outline);
        }
    }

    #[test]
    fn courtyard_chaining_matches_kicad_without_loosening_board_outlines() {
        let almost_closed = vec![
            EdgePrimitive::Line {
                start: Point { x: 0.0, y: 0.0 },
                end: Point { x: 10.0, y: 0.0 },
            },
            EdgePrimitive::Line {
                start: Point { x: 10.01, y: 0.0 },
                end: Point { x: 10.0, y: 10.0 },
            },
            EdgePrimitive::Line {
                start: Point { x: 10.0, y: 10.01 },
                end: Point { x: 0.0, y: 10.0 },
            },
            EdgePrimitive::Line {
                start: Point { x: -0.01, y: 10.0 },
                end: Point { x: 0.0, y: 0.01 },
            },
        ];
        validate_courtyard(&almost_closed).unwrap();
        assert!(assemble_outline(&almost_closed)
            .unwrap_err()
            .contains("closed connected loops"));
    }

    #[test]
    fn reversed_arcs_stitch_with_lines() {
        let primitives = vec![
            EdgePrimitive::Line {
                start: Point { x: 0.0, y: 0.0 },
                end: Point { x: 10.0, y: 0.0 },
            },
            EdgePrimitive::Line {
                start: Point { x: 0.0, y: 10.0 },
                end: Point { x: 0.0, y: 0.0 },
            },
            EdgePrimitive::Arc {
                start: Point { x: 0.0, y: 10.0 },
                mid: Point { x: 10.0, y: 10.0 },
                end: Point { x: 10.0, y: 0.0 },
            },
        ];
        let outline = assemble_outline(&primitives).unwrap();
        assert!(outline.len() > 20);
        validate_polygon(&outline).unwrap();
    }

    #[test]
    fn rejects_collinear_arc_and_branching_chain() {
        assert!(primitive_bounds(&EdgePrimitive::Arc {
            start: Point { x: 0.0, y: 0.0 },
            mid: Point { x: 1.0, y: 0.0 },
            end: Point { x: 2.0, y: 0.0 },
        })
        .unwrap_err()
        .contains("collinear"));

        let branching = vec![
            EdgePrimitive::Line {
                start: Point { x: 0.0, y: 0.0 },
                end: Point { x: 1.0, y: 0.0 },
            },
            EdgePrimitive::Line {
                start: Point { x: 1.0, y: 0.0 },
                end: Point { x: 1.0, y: 1.0 },
            },
            EdgePrimitive::Line {
                start: Point { x: 1.0, y: 0.0 },
                end: Point { x: 2.0, y: 0.0 },
            },
        ];
        assert!(assemble_outline(&branching)
            .unwrap_err()
            .contains("branching"));
    }

    #[test]
    fn canonical_outline_is_independent_of_primitive_order_and_direction() {
        let ordered = vec![
            EdgePrimitive::Line {
                start: Point { x: 0.0, y: 0.0 },
                end: Point { x: 10.0, y: 0.0 },
            },
            EdgePrimitive::Arc {
                start: Point { x: 10.0, y: 0.0 },
                mid: Point { x: 15.0, y: 5.0 },
                end: Point { x: 10.0, y: 10.0 },
            },
            EdgePrimitive::Line {
                start: Point { x: 10.0, y: 10.0 },
                end: Point { x: 0.0, y: 10.0 },
            },
            EdgePrimitive::Line {
                start: Point { x: 0.0, y: 10.0 },
                end: Point { x: 0.0, y: 0.0 },
            },
        ];
        let shuffled_and_reversed = vec![
            EdgePrimitive::Line {
                start: Point { x: 0.0, y: 0.0 },
                end: Point { x: 0.0, y: 10.0 },
            },
            EdgePrimitive::Line {
                start: Point { x: 0.0, y: 10.0 },
                end: Point { x: 10.0, y: 10.0 },
            },
            EdgePrimitive::Line {
                start: Point { x: 10.0, y: 0.0 },
                end: Point { x: 0.0, y: 0.0 },
            },
            EdgePrimitive::Arc {
                start: Point { x: 10.0, y: 10.0 },
                mid: Point { x: 15.0, y: 5.0 },
                end: Point { x: 10.0, y: 0.0 },
            },
        ];
        assert_eq!(
            assemble_outline(&ordered).unwrap(),
            assemble_outline(&shuffled_and_reversed).unwrap()
        );
    }
}
