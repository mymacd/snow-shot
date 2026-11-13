use std::collections::BTreeMap;

use snow_draw_engine_core::{
    DrawRect, Point, SnapGuide, SnapGuideAxis, SnapGuideKind, SnapQuery, SnapResult,
};
use snow_draw_engine_document::{
    ArrowData, Document, DocumentDelta, ElementData, ElementId, RectangleData,
    bracket_snap_points, rect_bounds,
};

use super::SpatialIndex;

#[derive(Clone, Debug, Default)]
pub struct SnapIndex {
    rects: Vec<Option<DrawRect>>,
    brackets: BTreeMap<ElementId, BracketSnapReference>,
    line_anchors: BTreeMap<ElementId, LineSnapReference>,
}

#[derive(Clone, Debug)]
struct BracketSnapReference {
    points: [Point<f64>; 3],
    proxy: RectangleData,
}

/// Endpoint/vertex targets of a straight line or arrow. A line start cannot be
/// aligned to a previous line's end through axis snapping alone, because the
/// arrow contributes no bounding rectangle to `rects`; these anchors are tested
/// by true 2D distance instead.
#[derive(Clone, Debug)]
struct LineSnapReference {
    anchors: Vec<Point<f64>>,
    proxy: RectangleData,
}

impl SnapIndex {
    pub fn rebuild(&mut self, document: &Document) {
        self.rects.clear();
        self.brackets.clear();
        self.line_anchors.clear();
        for state in document.element_states() {
            if !state.visible {
                continue;
            }
            if let Some(reference) = bracket_reference(state.data) {
                self.brackets.insert(state.id, reference);
            }
            if let Some(reference) = line_reference(state.data) {
                self.line_anchors.insert(state.id, reference);
            }
            let Some(rect) = snap_rectangle(state.data) else {
                continue;
            };
            self.ensure_capacity(state.id.index as usize);
            self.rects[state.id.index as usize] = Some(rect_bounds(&rect));
        }
    }

    pub fn refresh(&mut self, document: &Document, delta: &DocumentDelta) {
        let mut ids = delta.touched.clone();
        for id in &delta.removed {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }

        for id in ids {
            self.brackets.remove(&id);
            self.line_anchors.remove(&id);
            self.ensure_capacity(id.index as usize);
            self.rects[id.index as usize] = None;
            let Ok(element) = document.element(id) else {
                continue;
            };
            if !element.meta.visible {
                continue;
            }
            if let Some(reference) = bracket_reference(&element.data) {
                self.brackets.insert(id, reference);
            }
            if let Some(reference) = line_reference(&element.data) {
                self.line_anchors.insert(id, reference);
            }
            let Some(rect) = snap_rectangle(&element.data) else {
                continue;
            };
            self.rects[id.index as usize] = Some(rect_bounds(&rect));
        }
    }

    pub fn rect(&self, id: ElementId) -> Option<DrawRect> {
        self.rects
            .get(id.index as usize)
            .and_then(|bounds| bounds.as_ref().copied())
    }

    pub fn snap_point_filtered(
        &self,
        document: &Document,
        spatial_index: &SpatialIndex,
        query: &SnapQuery,
        mut include_candidate: impl FnMut(ElementId, &RectangleData) -> bool,
    ) -> SnapResult {
        let mut snap_x = nearest_grid_value(query.point.x, query.include_grid, query.grid_size);
        let mut snap_y = nearest_grid_value(query.point.y, query.include_grid, query.grid_size);
        let mut best_x_distance = snap_x
            .map(|candidate| (candidate - query.point.x).abs())
            .unwrap_or(f64::INFINITY);
        let mut best_y_distance = snap_y
            .map(|candidate| (candidate - query.point.y).abs())
            .unwrap_or(f64::INFINITY);

        // Exact 2D snapping to line and arrow vertices. Axis alignment alone
        // cannot land a new line's start on a previous line's end: arrows carry
        // no bounding rectangle in `rects`, and aligning the two axes
        // independently only converges when the cursor is already almost on the
        // vertex. A nearest-vertex test within the threshold wins instead.
        let mut point_snap: Option<(f64, Point<f64>)> = None;
        for (id, reference) in &self.line_anchors {
            if !include_candidate(*id, &reference.proxy) {
                continue;
            }
            for anchor in &reference.anchors {
                let dx = anchor.x - query.point.x;
                let dy = anchor.y - query.point.y;
                let distance = (dx * dx + dy * dy).sqrt();
                if distance > query.threshold {
                    continue;
                }
                let is_better = match point_snap {
                    Some((best, _)) => distance < best,
                    None => true,
                };
                if is_better {
                    point_snap = Some((distance, *anchor));
                }
            }
        }
        if let Some((_, anchor)) = point_snap {
            return SnapResult {
                point: anchor,
                guides: vec![SnapGuide {
                    kind: SnapGuideKind::Point,
                    axis: SnapGuideAxis::Vertical,
                    start: anchor,
                    end: anchor,
                    markers: vec![anchor],
                    label: None,
                }],
            };
        }

        spatial_index.with_point_candidate_ids(query.point, query.threshold, |candidate_ids| {
            for id in candidate_ids {
                let Some(bounds) = self.rect(*id) else {
                    continue;
                };
                let Some(rect) = document
                    .element(*id)
                    .ok()
                    .and_then(|element| snap_rectangle(&element.data))
                else {
                    continue;
                };
                if !include_candidate(*id, &rect) {
                    continue;
                }

                let candidates_x = [bounds.center_x(), bounds.min_x, bounds.max_x];
                for candidate in candidates_x {
                    let distance = (candidate - query.point.x).abs();
                    if distance <= query.threshold
                        && (distance < best_x_distance
                            || (distance == best_x_distance
                                && snap_x.is_none_or(|value| candidate < value)))
                    {
                        snap_x = Some(candidate);
                        best_x_distance = distance;
                    }
                }

                let candidates_y = [bounds.center_y(), bounds.min_y, bounds.max_y];
                for candidate in candidates_y {
                    let distance = (candidate - query.point.y).abs();
                    if distance <= query.threshold
                        && (distance < best_y_distance
                            || (distance == best_y_distance
                                && snap_y.is_none_or(|value| candidate < value)))
                    {
                        snap_y = Some(candidate);
                        best_y_distance = distance;
                    }
                }
            }
        });

        // Axis alignment is independent of proximity on the perpendicular axis.
        // Keep these sparse targets outside the local spatial candidate query.
        let mut reference_x = None;
        let mut reference_y = None;
        for (id, reference) in &self.brackets {
            if !include_candidate(*id, &reference.proxy) {
                continue;
            }
            for candidate in reference.points {
                let distance_x = (candidate.x - query.point.x).abs();
                if distance_x <= query.threshold
                    && (distance_x < best_x_distance
                        || (distance_x == best_x_distance
                            && snap_x.is_none_or(|value| candidate.x < value)))
                {
                    snap_x = Some(candidate.x);
                    best_x_distance = distance_x;
                    reference_x = Some(candidate);
                }
                let distance_y = (candidate.y - query.point.y).abs();
                if distance_y <= query.threshold
                    && (distance_y < best_y_distance
                        || (distance_y == best_y_distance
                            && snap_y.is_none_or(|value| candidate.y < value)))
                {
                    snap_y = Some(candidate.y);
                    best_y_distance = distance_y;
                    reference_y = Some(candidate);
                }
            }
        }

        snap_x = snap_x.filter(|value| (*value - query.point.x).abs() <= query.threshold);
        snap_y = snap_y.filter(|value| (*value - query.point.y).abs() <= query.threshold);
        let point = Point::new(
            snap_x.unwrap_or(query.point.x),
            snap_y.unwrap_or(query.point.y),
        );
        let mut guides = Vec::new();

        if let Some(value) = snap_x {
            guides.push(if let Some(reference) = reference_x {
                bracket_point_guide(SnapGuideAxis::Vertical, point, reference)
            } else {
                SnapGuide {
                    kind: SnapGuideKind::Point,
                    axis: SnapGuideAxis::Vertical,
                    start: Point::new(value, query.point.y),
                    end: Point::new(value, query.point.y),
                    markers: vec![Point::new(value, query.point.y)],
                    label: None,
                }
            });
        }
        if let Some(value) = snap_y {
            guides.push(if let Some(reference) = reference_y {
                bracket_point_guide(SnapGuideAxis::Horizontal, point, reference)
            } else {
                SnapGuide {
                    kind: SnapGuideKind::Point,
                    axis: SnapGuideAxis::Horizontal,
                    start: Point::new(query.point.x, value),
                    end: Point::new(query.point.x, value),
                    markers: vec![Point::new(query.point.x, value)],
                    label: None,
                }
            });
        }

        SnapResult { point, guides }
    }

    fn ensure_capacity(&mut self, index: usize) {
        if self.rects.len() <= index {
            self.rects.resize(index + 1, None);
        }
    }
}

fn bracket_reference(data: &ElementData) -> Option<BracketSnapReference> {
    let ElementData::Arrow(arrow) = data else {
        return None;
    };
    let points = bracket_snap_points(arrow)?;
    let bounds = point_bounds(&points)?;
    Some(BracketSnapReference {
        points,
        // Only the eligibility callback sees this proxy. Its edges and center
        // must never become alignment targets.
        proxy: arrow_snap_proxy(arrow, bounds),
    })
}

fn line_reference(data: &ElementData) -> Option<LineSnapReference> {
    let ElementData::Arrow(arrow) = data else {
        return None;
    };
    // Brackets, distance and angle annotations carry computed geometry and are
    // deliberately excluded; only plain lines and arrows act as targets.
    if !(arrow.is_line() || arrow.is_regular_arrow()) || arrow.points.len() < 2 {
        return None;
    }
    let anchors = arrow.global_points();
    if anchors.len() < 2
        || anchors
            .iter()
            .any(|point| !point.x.is_finite() || !point.y.is_finite())
    {
        return None;
    }
    let bounds = point_bounds(&anchors)?;
    Some(LineSnapReference {
        anchors,
        // Only the eligibility callback sees this proxy. Its edges and center
        // must never become alignment targets.
        proxy: arrow_snap_proxy(arrow, bounds),
    })
}

fn point_bounds(points: &[Point<f64>]) -> Option<DrawRect> {
    let first = points.first()?;
    let mut bounds = DrawRect::new(first.x, first.y, first.x, first.y);
    for point in points {
        bounds.min_x = bounds.min_x.min(point.x);
        bounds.min_y = bounds.min_y.min(point.y);
        bounds.max_x = bounds.max_x.max(point.x);
        bounds.max_y = bounds.max_y.max(point.y);
    }
    Some(bounds)
}

fn arrow_snap_proxy(arrow: &ArrowData, bounds: DrawRect) -> RectangleData {
    RectangleData {
        text_element_id: None,
        rectangle_kind: Default::default(),
        highlight_shape: Default::default(),
        center: bounds.center(),
        width: bounds.width(),
        height: bounds.height(),
        rotation: 0.0,
        fill: arrow.fill,
        fill_style: arrow.fill_style,
        stroke: arrow.stroke,
        stroke_width: arrow.stroke_width,
        stroke_style: arrow.stroke_style,
        stroke_rendering_style: Default::default(),
        stroke_rendering_seed: 0,
        corner_radii: Default::default(),
        opacity: arrow.opacity,
    }
}

fn bracket_point_guide(axis: SnapGuideAxis, point: Point<f64>, reference: Point<f64>) -> SnapGuide {
    let (start, end) = if axis == SnapGuideAxis::Vertical {
        (
            Point::new(point.x, point.y.min(reference.y)),
            Point::new(point.x, point.y.max(reference.y)),
        )
    } else {
        (
            Point::new(point.x.min(reference.x), point.y),
            Point::new(point.x.max(reference.x), point.y),
        )
    };
    SnapGuide {
        kind: SnapGuideKind::Point,
        axis,
        start,
        end,
        markers: if point == reference {
            vec![point]
        } else {
            vec![point, reference]
        },
        label: None,
    }
}

fn snap_rectangle(data: &ElementData) -> Option<RectangleData> {
    match data {
        ElementData::Rectangle(rect) => Some(*rect),
        ElementData::SpeechBubble(value) => {
            Some(snow_draw_engine_document::speech_bubble_body_proxy(value))
        }
        _ => None,
    }
}

fn nearest_grid_value(value: f64, include_grid: bool, grid_size: f64) -> Option<f64> {
    if !include_grid || !grid_size.is_finite() || grid_size <= 0.0 {
        return None;
    }
    Some((value / grid_size).round() * grid_size)
}

#[cfg(test)]
#[path = "bracket_snap_tests.rs"]
mod bracket_snap_tests;

#[cfg(test)]
#[path = "line_snap_tests.rs"]
mod line_snap_tests;
