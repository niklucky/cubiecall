//! Rectangles, shapes and anchors for placing floating windows, in AppKit's
//! coordinates on every platform: points, `y` from the bottom of the main
//! screen and growing upwards. Windows converts at its edge
//! (`platform_win.rs`), so the call widget's layout is the same everywhere.

/// The visible shape of a floating window.
#[derive(Clone, Debug)]
pub enum Mask {
    /// Rounded rectangle with this corner radius.
    Rounded(f64),
}

/// Which point of the window stays put when it's resized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    Center,
    /// Resize handles: the corner opposite the handle stays put.
    TopLeft,
    /// A point of the content stays put: `before` from the window's top-left
    /// corner now, `after` from it once resized. The call widget's pill, by
    /// the corner its buttons end at, with things growing around it.
    Point {
        before: (f32, f32),
        after: (f32, f32),
    },
}

/// A rectangle on screen, in AppKit's coordinates: `y` is the bottom edge
/// and grows upwards.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn top(&self) -> f64 {
        self.y + self.h
    }

    /// How far apart two rectangles are; 0 if they touch or overlap.
    pub fn distance(&self, other: &Rect) -> f64 {
        let dx = (other.x - self.right())
            .max(self.x - other.right())
            .max(0.0);
        let dy = (other.y - self.top()).max(self.y - other.top()).max(0.0);
        dx.hypot(dy)
    }
}

#[cfg(test)]
mod tests {
    use super::Rect;

    #[test]
    fn how_far_apart_two_rectangles_are() {
        let pill = Rect {
            x: 100.0,
            y: 100.0,
            w: 300.0,
            h: 52.0,
        };
        let at = |x: f64, y: f64| Rect {
            x,
            y,
            w: 80.0,
            h: 80.0,
        };
        // Over it, a gap of 8; touching; overlapping.
        assert_eq!(pill.distance(&at(200.0, 160.0)), 8.0);
        assert_eq!(pill.distance(&at(200.0, 152.0)), 0.0);
        assert_eq!(pill.distance(&at(200.0, 120.0)), 0.0);
        // Past a corner: straight-line distance.
        assert_eq!(pill.distance(&at(403.0, 156.0)), 5.0);
        assert_eq!(at(403.0, 156.0).distance(&pill), 5.0);
    }
}
