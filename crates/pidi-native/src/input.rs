//! Touch contacts as pixel coordinates on the 800×480 surface.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchEvent {
    Down { id: i32, x: i32, y: i32 },
    Move { id: i32, x: i32, y: i32 },
    Up { id: i32 },
}

pub fn norm_to_px(x: f32, y: f32, width: i32, height: i32) -> (i32, i32) {
    (
        (x.clamp(0.0, 1.0) * (width - 1) as f32).round() as i32,
        (y.clamp(0.0, 1.0) * (height - 1) as f32).round() as i32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_sdl_fingers_map_onto_the_panel() {
        assert_eq!(norm_to_px(0.0, 0.0, 800, 480), (0, 0));
        assert_eq!(norm_to_px(1.0, 1.0, 800, 480), (799, 479));
        let (x, y) = norm_to_px(0.5, 0.5, 800, 480);
        assert!((x - 400).abs() <= 1);
        assert!((y - 240).abs() <= 1);
    }
}
