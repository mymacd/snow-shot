use super::*;
use crate::DocumentModel;
use snow_draw_engine_core::{
    ColorRgba8,
    arrow::{ArrowType, StrokeStyle},
};
use snow_draw_engine_document::{ArrowData, ElementMeta, LinearElementKind, Transaction};

fn line_fixture() -> (DocumentModel, ArrowData) {
    let mut model = DocumentModel::new();
    let id = model.allocate_element_id();
    let mut arrow = ArrowData::from_global_points(
        &[Point::new(100., 200.), Point::new(180., 290.)],
        ColorRgba8 {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        },
        2.,
        StrokeStyle::Solid,
        ArrowType::Straight,
        None,
        None,
    )
    .unwrap();
    arrow.linear_kind = LinearElementKind::Line;
    let mut insert = Transaction::new("insert line reference");
    insert.insert_arrow(id, ElementMeta::default(), arrow.clone());
    model.apply_transaction(insert).unwrap();
    (model, arrow)
}

fn query(x: f64, y: f64, threshold: f64) -> SnapQuery {
    SnapQuery {
        point: Point::new(x, y),
        threshold,
        include_grid: false,
        grid_size: 20.,
    }
}

fn assert_near(actual: Point<f64>, expected: Point<f64>) {
    assert!(
        (actual.x - expected.x).abs() < 1e-6 && (actual.y - expected.y).abs() < 1e-6,
        "{actual:?} != {expected:?}"
    );
}

#[test]
fn a_new_line_start_snaps_exactly_onto_a_previous_line_end() {
    let (model, arrow) = line_fixture();
    let end = arrow.end();
    // Off in both axes, so axis alignment alone could not converge here.
    let snapped = model.snap_point(&query(end.x + 1.5, end.y - 1.5, 4.));
    assert_near(snapped.point, end);
    assert_eq!(snapped.guides.len(), 1);
    assert_eq!(snapped.guides[0].kind, SnapGuideKind::Point);
    assert_eq!(snapped.guides[0].markers.len(), 1);
    assert_near(snapped.guides[0].markers[0], end);
}

#[test]
fn the_line_start_is_a_target_too() {
    let (model, arrow) = line_fixture();
    let start = arrow.start();
    let snapped = model.snap_point(&query(start.x - 1., start.y + 1., 4.));
    assert_near(snapped.point, start);
}

#[test]
fn a_cursor_outside_the_threshold_is_left_alone() {
    let (model, arrow) = line_fixture();
    let end = arrow.end();
    let cursor = Point::new(end.x + 10., end.y + 10.);
    let snapped = model.snap_point(&query(cursor.x, cursor.y, 2.));
    assert_near(snapped.point, cursor);
    assert!(snapped.guides.is_empty());
}

#[test]
fn the_eligibility_filter_can_exclude_the_line() {
    let (model, arrow) = line_fixture();
    let end = arrow.end();
    let cursor = Point::new(end.x + 1., end.y - 1.);
    let snapped = model.snap_point_filtered(&query(cursor.x, cursor.y, 4.), |_, _| false);
    assert_near(snapped.point, cursor);
    assert!(snapped.guides.is_empty());
}

#[test]
fn the_eligibility_filter_sees_the_line_bounding_proxy() {
    let (model, arrow) = line_fixture();
    let end = arrow.end();
    let mut visited = false;
    let snapped = model.snap_point_filtered(&query(end.x + 1., end.y - 1., 4.), |_, proxy| {
        assert_near(proxy.center, Point::new(140., 245.));
        assert!((proxy.width - 80.).abs() < 1e-6 && (proxy.height - 90.).abs() < 1e-6);
        visited = true;
        true
    });
    assert!(visited);
    assert_near(snapped.point, end);
}
