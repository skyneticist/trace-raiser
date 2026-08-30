use crate::{LocalAabb, Point, Polygon, Pose};

const EPSILON: f64 = 1.0e-9;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Bounds {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

pub(crate) fn point_is_finite(point: Point) -> bool {
    point.x.is_finite() && point.y.is_finite()
}

pub(crate) fn pose_is_finite(pose: Pose) -> bool {
    pose.x.is_finite() && pose.y.is_finite() && pose.rotation_degrees.is_finite()
}

pub(crate) fn polygon_bounds(polygon: &Polygon) -> Option<Bounds> {
    let first = *polygon.vertices.first()?;
    let mut bounds = Bounds {
        min_x: first.x,
        min_y: first.y,
        max_x: first.x,
        max_y: first.y,
    };
    for point in &polygon.vertices[1..] {
        bounds.min_x = bounds.min_x.min(point.x);
        bounds.min_y = bounds.min_y.min(point.y);
        bounds.max_x = bounds.max_x.max(point.x);
        bounds.max_y = bounds.max_y.max(point.y);
    }
    Some(bounds)
}

pub(crate) fn polygon_signed_area(polygon: &[Point]) -> f64 {
    if polygon.len() < 3 {
        return 0.0;
    }
    polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .take(polygon.len())
        .map(|(a, b)| a.x * b.y - b.x * a.y)
        .sum::<f64>()
        * 0.5
}

pub(crate) fn polygon_is_simple(polygon: &[Point]) -> bool {
    if polygon.len() < 3 || polygon_signed_area(polygon).abs() <= EPSILON {
        return false;
    }
    if polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .take(polygon.len())
        .any(|(a, b)| distance_squared(*a, *b) <= EPSILON * EPSILON)
    {
        return false;
    }

    let length = polygon.len();
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
                polygon[first],
                polygon[first_next],
                polygon[second],
                polygon[second_next],
            ) {
                return false;
            }
        }
    }
    true
}

pub(crate) fn transform_point(point: Point, pose: Pose) -> Point {
    let radians = pose.rotation_degrees.to_radians();
    let (sin, cos) = radians.sin_cos();
    Point {
        x: pose.x + point.x * cos - point.y * sin,
        y: pose.y + point.x * sin + point.y * cos,
    }
}

pub(crate) fn transformed_envelope(envelope: LocalAabb, pose: Pose) -> Vec<Point> {
    [
        Point {
            x: envelope.min_x,
            y: envelope.min_y,
        },
        Point {
            x: envelope.max_x,
            y: envelope.min_y,
        },
        Point {
            x: envelope.max_x,
            y: envelope.max_y,
        },
        Point {
            x: envelope.min_x,
            y: envelope.max_y,
        },
    ]
    .into_iter()
    .map(|point| transform_point(point, pose))
    .collect()
}

pub(crate) fn polygon_contains_point(polygon: &[Point], point: Point) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    if edges(polygon).any(|(a, b)| point_on_segment(point, a, b)) {
        return true;
    }

    let mut inside = false;
    for (a, b) in edges(polygon) {
        let crosses_y = (a.y > point.y) != (b.y > point.y);
        if crosses_y {
            let crossing_x = (b.x - a.x) * (point.y - a.y) / (b.y - a.y) + a.x;
            if point.x < crossing_x {
                inside = !inside;
            }
        }
    }
    inside
}

pub(crate) fn polygon_distance(first: &[Point], second: &[Point]) -> f64 {
    if first.is_empty() || second.is_empty() {
        return f64::INFINITY;
    }
    if polygons_intersect_or_contain(first, second) {
        return 0.0;
    }
    boundary_distance(first, second)
}

fn boundary_distance(first: &[Point], second: &[Point]) -> f64 {
    edges(first)
        .flat_map(|first_edge| {
            edges(second).map(move |second_edge| {
                segment_distance(first_edge.0, first_edge.1, second_edge.0, second_edge.1)
            })
        })
        .fold(f64::INFINITY, f64::min)
}

pub(crate) fn envelope_inside_outline(
    envelope: &[Point],
    outline: &[Point],
    clearance_mm: f64,
) -> bool {
    if !envelope
        .iter()
        .all(|point| polygon_contains_point(outline, *point))
    {
        return false;
    }

    // This catches a rectangle spanning a concavity even when all four corners
    // happen to lie inside the board.
    if edges(envelope).any(|candidate| {
        edges(outline).any(|boundary| {
            segments_properly_intersect(candidate.0, candidate.1, boundary.0, boundary.1)
        })
    }) {
        return false;
    }

    boundary_distance(envelope, outline) + EPSILON >= clearance_mm
}

pub(crate) fn polygons_conflict(first: &[Point], second: &[Point], clearance_mm: f64) -> bool {
    polygons_intersect_or_contain(first, second)
        || polygon_distance(first, second) + EPSILON < clearance_mm
}

fn polygons_intersect_or_contain(first: &[Point], second: &[Point]) -> bool {
    edges(first).any(|a| edges(second).any(|b| segments_intersect(a.0, a.1, b.0, b.1)))
        || first
            .first()
            .is_some_and(|point| polygon_contains_point(second, *point))
        || second
            .first()
            .is_some_and(|point| polygon_contains_point(first, *point))
}

fn edges(points: &[Point]) -> impl Iterator<Item = (Point, Point)> + '_ {
    points
        .iter()
        .copied()
        .zip(points.iter().copied().cycle().skip(1))
        .take(points.len())
}

fn orientation(a: Point, b: Point, c: Point) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

fn point_on_segment(point: Point, start: Point, end: Point) -> bool {
    orientation(start, end, point).abs() <= EPSILON
        && point.x >= start.x.min(end.x) - EPSILON
        && point.x <= start.x.max(end.x) + EPSILON
        && point.y >= start.y.min(end.y) - EPSILON
        && point.y <= start.y.max(end.y) + EPSILON
}

fn segments_intersect(a: Point, b: Point, c: Point, d: Point) -> bool {
    let ab_c = orientation(a, b, c);
    let ab_d = orientation(a, b, d);
    let cd_a = orientation(c, d, a);
    let cd_b = orientation(c, d, b);
    if ((ab_c > EPSILON && ab_d < -EPSILON) || (ab_c < -EPSILON && ab_d > EPSILON))
        && ((cd_a > EPSILON && cd_b < -EPSILON) || (cd_a < -EPSILON && cd_b > EPSILON))
    {
        return true;
    }
    (ab_c.abs() <= EPSILON && point_on_segment(c, a, b))
        || (ab_d.abs() <= EPSILON && point_on_segment(d, a, b))
        || (cd_a.abs() <= EPSILON && point_on_segment(a, c, d))
        || (cd_b.abs() <= EPSILON && point_on_segment(b, c, d))
}

fn segments_properly_intersect(a: Point, b: Point, c: Point, d: Point) -> bool {
    let ab_c = orientation(a, b, c);
    let ab_d = orientation(a, b, d);
    let cd_a = orientation(c, d, a);
    let cd_b = orientation(c, d, b);
    ((ab_c > EPSILON && ab_d < -EPSILON) || (ab_c < -EPSILON && ab_d > EPSILON))
        && ((cd_a > EPSILON && cd_b < -EPSILON) || (cd_a < -EPSILON && cd_b > EPSILON))
}

fn segment_distance(a: Point, b: Point, c: Point, d: Point) -> f64 {
    if segments_intersect(a, b, c, d) {
        return 0.0;
    }
    point_segment_distance(a, c, d)
        .min(point_segment_distance(b, c, d))
        .min(point_segment_distance(c, a, b))
        .min(point_segment_distance(d, a, b))
}

fn point_segment_distance(point: Point, start: Point, end: Point) -> f64 {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length_squared = dx * dx + dy * dy;
    if length_squared <= EPSILON * EPSILON {
        return distance_squared(point, start).sqrt();
    }
    let projection =
        (((point.x - start.x) * dx + (point.y - start.y) * dy) / length_squared).clamp(0.0, 1.0);
    let closest = Point {
        x: start.x + projection * dx,
        y: start.y + projection * dy,
    };
    distance_squared(point, closest).sqrt()
}

fn distance_squared(first: Point, second: Point) -> f64 {
    let dx = first.x - second.x;
    let dy = first.y - second.y;
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    #[test]
    fn rejects_self_intersecting_polygon() {
        assert!(!polygon_is_simple(&[
            point(0.0, 0.0),
            point(5.0, 5.0),
            point(0.0, 5.0),
            point(5.0, 0.0),
        ]));
    }

    #[test]
    fn detects_rectangle_crossing_concave_outline() {
        let outline = [
            point(0.0, 0.0),
            point(6.0, 0.0),
            point(6.0, 6.0),
            point(4.0, 6.0),
            point(4.0, 2.0),
            point(2.0, 2.0),
            point(2.0, 6.0),
            point(0.0, 6.0),
        ];
        let bridge = [
            point(1.0, 3.0),
            point(5.0, 3.0),
            point(5.0, 4.0),
            point(1.0, 4.0),
        ];
        assert!(!envelope_inside_outline(&bridge, &outline, 0.0));
    }

    #[test]
    fn measures_clearance_to_containing_outline_boundary() {
        let outline = [
            point(0.0, 0.0),
            point(10.0, 0.0),
            point(10.0, 10.0),
            point(0.0, 10.0),
        ];
        let inside = [
            point(2.0, 2.0),
            point(4.0, 2.0),
            point(4.0, 4.0),
            point(2.0, 4.0),
        ];
        assert!(envelope_inside_outline(&inside, &outline, 2.0));
        assert!(!envelope_inside_outline(&inside, &outline, 2.01));
    }

    #[test]
    fn computes_separation_between_polygons() {
        let first = [
            point(0.0, 0.0),
            point(1.0, 0.0),
            point(1.0, 1.0),
            point(0.0, 1.0),
        ];
        let second = [
            point(3.0, 0.0),
            point(4.0, 0.0),
            point(4.0, 1.0),
            point(3.0, 1.0),
        ];
        assert!((polygon_distance(&first, &second) - 2.0).abs() < 1.0e-9);
        assert!(polygons_conflict(&first, &second, 2.1));
        assert!(!polygons_conflict(&first, &second, 2.0));
    }
}
