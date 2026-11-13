use super::*;

#[test]
fn a_point_of_the_window_is_divided_by_the_zoom() {
    assert_eq!(from_window(2.0, 120.0, 48.0), (60.0, 24.0));
    assert_eq!(from_window(0.5, -9.0, 3.0), (-18.0, 6.0), "a delta or a velocity keeps its sign");
}

#[test]
fn a_rect_of_the_app_is_multiplied_by_the_zoom_origin_and_size_alike() {
    assert_eq!(to_window(2.0, [10.0, 20.0, 30.0, 40.0]), [20.0, 40.0, 60.0, 80.0]);
    assert_eq!(to_window(0.5, [10.0, 20.0, 30.0, 40.0]), [5.0, 10.0, 15.0, 20.0]);
}

#[test]
fn an_elements_corner_in_the_window_is_the_point_a_touch_there_reports() {
    for zoom in [0.5, 1.0, 2.0] {
        let [x, y, ..] = to_window(zoom, [12.0, 34.0, 56.0, 78.0]);
        assert_eq!(from_window(zoom, x, y), (12.0, 34.0), "zoom {zoom}");
    }
}
