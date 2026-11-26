//! Window-local editor geometry shared by painting and input.

/// Convert an integer display extent to GPUI's floating-point pixel arithmetic.
/// Source byte positions never pass through this conversion.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "GPUI uses f32 pixels; subpixel precision beyond 16 million display units is not meaningful"
)]
pub fn display_units(units: usize) -> f32 {
    units as f32
}

/// Resolve a nonnegative display-row count; Rust saturates out-of-range casts.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "flooring and clamping is the intended pixel-to-row conversion; this never converts source byte offsets"
)]
pub fn whole_rows(rows: f32) -> usize {
    rows.max(0.0).floor() as usize
}

#[must_use]
pub fn horizontal_scroll_limit(
    max_display_columns: usize,
    monospace_cell_width: f32,
    text_viewport_width: f32,
) -> f32 {
    (display_units(max_display_columns) * monospace_cell_width - text_viewport_width.max(0.0))
        .max(0.0)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorGeometry {
    origin_x: f32,
    origin_y: f32,
    viewport_width: f32,
    viewport_height: f32,
    header_height: f32,
    gutter_width: f32,
    line_height: f32,
    center_width: f32,
    merge_layout: bool,
    divider_ratios: Option<[f32; 2]>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalHit {
    pub left_side: bool,
    pub incoming_side: bool,
    pub row: usize,
    pub in_gutter: bool,
    pub text_x: f32,
}

impl EditorGeometry {
    #[must_use]
    pub fn new(
        origin_x: f32,
        origin_y: f32,
        viewport_width: f32,
        viewport_height: f32,
        header_height: f32,
        gutter_width: f32,
        line_height: f32,
    ) -> Self {
        Self {
            origin_x,
            origin_y,
            viewport_width: viewport_width.max(0.0),
            viewport_height: viewport_height.max(0.0),
            header_height: header_height.max(0.0),
            gutter_width: gutter_width.max(0.0),
            line_height: line_height.max(f32::EPSILON),
            center_width: 0.0,
            merge_layout: false,
            divider_ratios: None,
        }
    }

    #[must_use]
    pub fn with_center_width(mut self, width: f32) -> Self {
        self.center_width = width.clamp(0.0, self.viewport_width);
        self
    }

    #[must_use]
    pub fn with_merge_layout(mut self, enabled: bool) -> Self {
        self.merge_layout = enabled;
        if enabled {
            self.center_width = 0.0;
        }
        self
    }

    #[must_use]
    pub fn with_divider_ratios(mut self, ratios: Option<[f32; 2]>) -> Self {
        self.divider_ratios = ratios;
        self
    }

    fn pane_space(self) -> f32 {
        self.viewport_width - self.center_width
    }

    fn equal_pane_width(self) -> f32 {
        self.pane_space() / if self.merge_layout { 3.0 } else { 2.0 }
    }

    fn minimum_pane_width(self) -> f32 {
        self.equal_pane_width().min(120.0)
    }

    fn dividers(self) -> [f32; 2] {
        let total = self.pane_space();
        let minimum = self.minimum_pane_width();
        let Some(ratios) = self.divider_ratios else {
            let equal = self.equal_pane_width();
            return [
                equal,
                if self.merge_layout {
                    equal * 2.0
                } else {
                    total
                },
            ];
        };

        let reserved = if self.merge_layout {
            minimum * 2.0
        } else {
            minimum
        };
        let first = (ratios[0] * total).clamp(minimum, (total - reserved).max(minimum));
        let second = if self.merge_layout {
            (ratios[1] * total).clamp(first + minimum, (total - minimum).max(first + minimum))
        } else {
            total
        };

        [first, second]
    }

    /// Move one divider without moving its neighbor. Coordinates are local to the editor.
    #[must_use]
    pub fn resized_dividers(self, divider: usize, x: f32) -> Option<[f32; 2]> {
        let total = self.pane_space();
        let minimum = self.minimum_pane_width();
        if self.equal_pane_width() <= minimum {
            return self.divider_ratios;
        }

        let [first, second] = self.dividers();
        if divider == 0 {
            let first = x.clamp(minimum, (second - minimum).max(minimum));
            Some([first / total, second / total])
        } else {
            let second = x.clamp(first + minimum, (total - minimum).max(first + minimum));
            Some([first / total, second / total])
        }
    }

    #[must_use]
    pub fn incoming_pane_left(self) -> f32 {
        self.dividers()[1]
    }

    #[must_use]
    pub fn center_width(self) -> f32 {
        self.center_width
    }

    #[must_use]
    pub fn pane_width(self) -> f32 {
        self.dividers()[0]
    }

    #[must_use]
    pub fn right_pane_width(self) -> f32 {
        let [first, second] = self.dividers();
        second - first
    }

    #[must_use]
    pub fn incoming_pane_width(self) -> f32 {
        (self.pane_space() - self.incoming_pane_left()).max(0.0)
    }

    #[must_use]
    pub fn right_pane_left(self) -> f32 {
        self.pane_width() + self.center_width
    }

    #[must_use]
    pub fn content_width(self) -> f32 {
        self.viewport_width
    }

    #[must_use]
    pub fn rows_viewport_height(self) -> f32 {
        (self.viewport_height - self.header_height).max(0.0)
    }

    #[must_use]
    pub fn text_viewport_width(self) -> f32 {
        self.text_width(self.pane_width())
    }

    #[must_use]
    pub fn text_width(self, pane_width: f32) -> f32 {
        (pane_width - self.gutter_width).max(0.0)
    }

    #[must_use]
    pub fn hit(
        self,
        window_x: f32,
        window_y: f32,
        vertical_scroll: f32,
        horizontal_scroll: f32,
    ) -> LocalHit {
        let local_x = window_x - self.origin_x;
        let local_y = window_y - self.origin_y;
        let left_side = local_x < self.pane_width();
        let incoming_side = self.merge_layout && local_x >= self.incoming_pane_left();
        let pane_x = if left_side {
            local_x
        } else if incoming_side {
            local_x - self.incoming_pane_left()
        } else {
            local_x - self.right_pane_left()
        };
        let row_y = (local_y - self.header_height + vertical_scroll).max(0.0);
        let in_gutter = pane_x < self.gutter_width;

        LocalHit {
            left_side,
            incoming_side,
            row: whole_rows(row_y / self.line_height),
            in_gutter,
            text_x: if in_gutter {
                0.0
            } else {
                pane_x - self.gutter_width + horizontal_scroll
            },
        }
    }

    #[must_use]
    pub fn vertical_scroll_limit(self, alignment_rows: usize) -> f32 {
        (display_units(alignment_rows) * self.line_height + self.line_height
            - self.rows_viewport_height())
        .max(0.0)
    }

    /// Reveal a change's first row with up to three preceding context rows.
    /// Tall deletions show their beginning rather than jumping past the gap to
    /// reveal the right-hand insertion caret at the end of the deleted block.
    #[must_use]
    pub fn change_scroll_top(self, first_row: usize, alignment_rows: usize) -> f32 {
        let context_rows = (whole_rows(self.rows_viewport_height() / self.line_height) / 4).min(3);
        (display_units(first_row.saturating_sub(context_rows)) * self.line_height)
            .min(self.vertical_scroll_limit(alignment_rows))
    }

    #[must_use]
    pub fn visible_row_top(self, row: usize, first_row: usize, row_offset: f32) -> f32 {
        display_units(row - first_row) * self.line_height - row_offset
    }
}
