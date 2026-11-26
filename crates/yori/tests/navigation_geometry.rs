use yori::geometry::EditorGeometry;

#[test]
fn jumps_leave_context_and_clamp_at_document_edges() {
    // Toolbar + file header occupy 80 pixels, leaving twenty visible text rows.
    let geometry = EditorGeometry::new(15.0, 25.0, 1000.0, 480.0, 80.0, 82.0, 20.0);

    assert!((geometry.change_scroll_top(10, 100) - 140.0).abs() < f32::EPSILON);
    assert!(geometry.change_scroll_top(0, 100).abs() < f32::EPSILON);
    assert!(
        (geometry.change_scroll_top(99, 100) - geometry.vertical_scroll_limit(100)).abs()
            < f32::EPSILON
    );

    let hit = geometry.hit(15.0 + 82.0, 25.0 + 80.0, 0.0, 0.0);
    assert_eq!(hit.row, 0);
    assert!(!hit.in_gutter);
}

#[test]
fn tiny_viewports_prioritize_the_target_row_over_context() {
    let geometry = EditorGeometry::new(0.0, 0.0, 400.0, 100.0, 80.0, 82.0, 20.0);
    assert!((geometry.change_scroll_top(10, 100) - 200.0).abs() < f32::EPSILON);
    assert!(geometry.change_scroll_top(0, 1).abs() < f32::EPSILON);
}

#[test]
fn resized_diff_keeps_the_connector_channel_and_moves_right_pane_hits() {
    let geometry =
        EditorGeometry::new(15.0, 25.0, 1024.0, 480.0, 80.0, 82.0, 20.0).with_center_width(64.0);
    let resized = geometry.with_divider_ratios(geometry.resized_dividers(0, 240.0));

    assert!((resized.pane_width() - 240.0).abs() < 0.001);
    assert!((resized.right_pane_left() - 304.0).abs() < 0.001);
    assert!((resized.right_pane_width() - 720.0).abs() < 0.001);

    let hit = resized.hit(15.0 + 304.0 + 82.0 + 12.0, 125.0, 0.0, 0.0);
    assert!(!hit.left_side && !hit.incoming_side && !hit.in_gutter);
    assert!((hit.text_x - 12.0).abs() < 0.001);
}

#[test]
fn merge_dividers_resize_adjacent_panes_without_crossing() {
    let geometry = EditorGeometry::new(0.0, 0.0, 960.0, 480.0, 80.0, 82.0, 20.0)
        .with_center_width(64.0)
        .with_merge_layout(true);
    let resized = geometry.with_divider_ratios(geometry.resized_dividers(0, 900.0));

    assert!((resized.pane_width() - 520.0).abs() < 0.001);
    assert!((resized.right_pane_width() - 120.0).abs() < 0.001);
    assert!((resized.incoming_pane_left() - 640.0).abs() < 0.001);

    let resized = resized.with_divider_ratios(resized.resized_dividers(1, 900.0));
    assert!((resized.pane_width() - 520.0).abs() < 0.001);
    assert!((resized.incoming_pane_width() - 120.0).abs() < 0.001);
    assert!(resized.hit(940.0, 125.0, 0.0, 0.0).incoming_side);
}
