//! Shared scroll geometry and pixel-bounded diff overview; never source coordinates.

use std::ops::Range;

use yori_diff::Alignment;

use crate::geometry::{display_units, whole_rows};

#[derive(Clone, Copy, Debug)]
pub struct HorizontalScrollTrack {
    width: f32,
    max_scroll: f32,
    thumb_width: f32,
}

impl HorizontalScrollTrack {
    #[must_use]
    pub fn new(viewport_width: f32, max_scroll: f32, track_width: f32) -> Self {
        let width = track_width.max(0.0);
        let viewport_width = viewport_width.max(0.0);
        let max_scroll = max_scroll.max(0.0);
        let content_width = viewport_width + max_scroll;
        let thumb_width = if content_width > 0.0 {
            (width * viewport_width / content_width).clamp(24.0_f32.min(width), width)
        } else {
            0.0
        };

        Self {
            width,
            max_scroll,
            thumb_width,
        }
    }

    #[must_use]
    pub fn max_scroll(self) -> f32 {
        self.max_scroll
    }

    #[must_use]
    pub fn thumb(self, scroll: f32) -> Range<f32> {
        let left = if self.max_scroll > 0.0 {
            scroll.clamp(0.0, self.max_scroll) / self.max_scroll * (self.width - self.thumb_width)
        } else {
            0.0
        };

        left..left + self.thumb_width
    }

    #[must_use]
    pub fn scroll_for_thumb(self, left: f32) -> f32 {
        let travel = self.width - self.thumb_width;
        if travel <= 0.0 {
            return 0.0;
        }

        (left / travel).clamp(0.0, 1.0) * self.max_scroll
    }

    #[must_use]
    pub fn jump(self, x: f32) -> f32 {
        self.scroll_for_thumb(x - self.thumb_width / 2.0)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ScrollTrack {
    height: f32,
    content_height: f32,
    thumb_height: f32,
}

impl ScrollTrack {
    #[must_use]
    pub fn new(rows: usize, line_height: f32, viewport_height: f32) -> Self {
        let height = viewport_height.max(0.0);
        // The editor includes one trailing canvas row for exact EOF selection.
        let content_height = ((display_units(rows) + 1.0) * line_height).max(height);
        let thumb_height = if content_height > 0.0 {
            (height * height / content_height).clamp(24.0_f32.min(height), height)
        } else {
            0.0
        };

        Self {
            height,
            content_height,
            thumb_height,
        }
    }

    #[must_use]
    pub fn max_scroll(self) -> f32 {
        (self.content_height - self.height).max(0.0)
    }

    #[must_use]
    pub fn thumb(self, scroll: f32) -> Range<f32> {
        let top = if self.max_scroll() > 0.0 {
            scroll.clamp(0.0, self.max_scroll()) / self.max_scroll()
                * (self.height - self.thumb_height)
        } else {
            0.0
        };

        top..top + self.thumb_height
    }

    #[must_use]
    pub fn scroll_for_thumb(self, top: f32) -> f32 {
        let travel = self.height - self.thumb_height;
        if travel <= 0.0 {
            return 0.0;
        }

        (top / travel).clamp(0.0, 1.0) * self.max_scroll()
    }

    #[must_use]
    pub fn jump(self, y: f32) -> f32 {
        self.scroll_for_thumb(y - self.thumb_height / 2.0)
    }

    #[must_use]
    pub fn marker(self, rows: Range<usize>, line_height: f32) -> Range<f32> {
        if self.content_height <= 0.0 || self.height <= 0.0 {
            return 0.0..0.0;
        }

        let scale = self.height * line_height / self.content_height;
        let top = (display_units(rows.start) * scale).min((self.height - 2.0).max(0.0));
        let bottom = (display_units(rows.end) * scale)
            .max(top + 2.0)
            .min(self.height);

        top..bottom
    }

    /// Aggregate overlapping tiny hunks at display-pixel resolution. Work scales
    /// with hunks plus track height; the renderer never paints one element per row.
    #[must_use]
    pub fn bands(self, alignment: &Alignment, line_height: f32) -> Vec<OverviewBand> {
        self.bands_for(
            alignment.blocks().iter().map(|block| {
                (
                    block.rows.clone(),
                    !block.left.is_empty(),
                    !block.right.is_empty(),
                )
            }),
            line_height,
        )
    }

    /// Aggregate logical change ranges after a caller maps them into presentation rows.
    #[must_use]
    pub fn bands_for(
        self,
        ranges: impl IntoIterator<Item = (Range<usize>, bool, bool)>,
        line_height: f32,
    ) -> Vec<OverviewBand> {
        let mut pixels = vec![0_u8; whole_rows(self.height.ceil())];
        for (rows, left, right) in ranges {
            let marker = self.marker(rows, line_height);
            let start = whole_rows(marker.start).min(pixels.len());
            let end = whole_rows(marker.end.ceil()).min(pixels.len());
            let sides = u8::from(left) | (u8::from(right) << 1);

            for pixel in &mut pixels[start..end] {
                *pixel |= sides;
            }
        }

        let mut bands = Vec::new();
        let mut start = 0;
        while start < pixels.len() {
            let sides = pixels[start];
            let mut end = start + 1;
            while end < pixels.len() && pixels[end] == sides {
                end += 1;
            }

            if sides != 0 {
                bands.push(OverviewBand {
                    top: display_units(start),
                    bottom: display_units(end).min(self.height),
                    left: sides & 1 != 0,
                    right: sides & 2 != 0,
                });
            }
            start = end;
        }

        bands
    }
}

#[derive(Debug, PartialEq)]
pub struct OverviewBand {
    pub top: f32,
    pub bottom: f32,
    pub left: bool,
    pub right: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::EditorGeometry;
    use yori_document::Document;

    #[test]
    fn horizontal_thumb_represents_viewport_and_round_trips_across_the_range() {
        let track = HorizontalScrollTrack::new(400.0, 600.0, 800.0);

        assert!((track.max_scroll() - 600.0).abs() < f32::EPSILON);
        assert_eq!(track.thumb(0.0), 0.0..320.0);
        assert_eq!(track.thumb(300.0), 240.0..560.0);
        assert_eq!(track.thumb(600.0), 480.0..800.0);

        for fraction in [0.0, 0.2, 0.5, 1.0] {
            let scroll = track.max_scroll() * fraction;
            let thumb = track.thumb(scroll);
            assert!((track.scroll_for_thumb(thumb.start) - scroll).abs() <= f32::EPSILON);
        }

        assert!(track.jump(-100.0).abs() < f32::EPSILON);
        assert!((track.jump(900.0) - track.max_scroll()).abs() < f32::EPSILON);
    }

    #[test]
    fn horizontal_track_handles_non_overflow_and_tiny_bounds() {
        for (viewport, max_scroll, width) in [
            (400.0, 0.0, 800.0),
            (0.0, 100.0, 10.0),
            (100.0, 100_000.0, 10.0),
            (0.0, 0.0, 0.0),
        ] {
            let track = HorizontalScrollTrack::new(viewport, max_scroll, width);
            let thumb = track.thumb(100.0);

            assert!(thumb.start.is_finite() && thumb.end.is_finite());
            assert!(thumb.start >= 0.0 && thumb.end <= width);
            assert!(track.scroll_for_thumb(40.0).is_finite());
        }

        assert_eq!(
            HorizontalScrollTrack::new(400.0, 0.0, 800.0).thumb(0.0),
            0.0..800.0
        );
    }

    #[test]
    fn thumb_round_trips_and_reaches_both_ends_even_with_minimum_height() {
        for rows in [0, 1, 30, 5_000, 100_000] {
            let track = ScrollTrack::new(rows, 22.0, 600.0);
            let editor = EditorGeometry::new(0.0, 0.0, 1200.0, 664.0, 64.0, 98.0, 22.0);
            assert!((track.max_scroll() - editor.vertical_scroll_limit(rows)).abs() < f32::EPSILON);

            for fraction in [0.0, 0.2, 0.5, 1.0] {
                let scroll = track.max_scroll() * fraction;
                let thumb = track.thumb(scroll);
                assert!((track.scroll_for_thumb(thumb.start) - scroll).abs() <= 0.25);
                assert!(thumb.start >= 0.0 && thumb.end <= 600.0);
            }

            assert!(track.jump(-100.0).abs() < f32::EPSILON);
            assert!((track.jump(700.0) - track.max_scroll()).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn zero_and_tiny_viewports_do_not_produce_invalid_geometry() {
        for height in [0.0, 1.0, 10.0] {
            let track = ScrollTrack::new(100_000, 22.0, height);
            let thumb = track.thumb(100.0);
            assert!(thumb.start.is_finite() && thumb.end.is_finite());
            assert!(thumb.end <= height);
            assert!(track.scroll_for_thumb(40.0).is_finite());
        }
    }

    #[test]
    fn overview_distinguishes_added_removed_and_modified_blocks() {
        for (left, right, removed, added) in [
            ("", "added\n", false, true),
            ("removed\n", "", true, false),
            ("old\n", "new\n", true, true),
        ] {
            let left = Document::from_bytes(left.as_bytes().to_vec()).unwrap();
            let right = Document::from_bytes(right.as_bytes().to_vec()).unwrap();
            let alignment = Alignment::between(&left, &right);
            let track = ScrollTrack::new(alignment.rows().len(), 22.0, 600.0);
            let bands = track.bands(&alignment, 22.0);

            assert!(!bands.is_empty());
            assert!(
                bands
                    .iter()
                    .all(|band| band.left == removed && band.right == added)
            );
            assert!(
                bands
                    .iter()
                    .all(|band| band.top >= 0.0 && band.bottom <= 600.0)
            );
        }
    }

    #[test]
    fn markers_use_alignment_extent_not_each_sides_source_length() {
        let track = ScrollTrack::new(999, 22.0, 500.0);
        let middle = track.marker(500..510, 22.0);
        let last = track.marker(998..999, 22.0);

        assert_eq!(middle, 250.0..255.0);
        assert_eq!(last, 498.0..500.0);
    }
}
