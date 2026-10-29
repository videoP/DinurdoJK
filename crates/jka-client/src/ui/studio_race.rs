//! Baked into preview AND exports, independent of gameplay HUD toggles.
use super::{textured_rect_with_source, ProportionalFont, UiStudioLabel, UiVertex};
use taffy::prelude::{auto, length, AvailableSpace, FlexDirection, Size, Style, TaffyTree};

#[derive(Clone, Debug)]
pub struct StudioRaceRunnerUi {
    pub route_index: usize,
    pub rank: usize,
    pub gap: Option<f64>,
    pub focused: bool,
    pub name: String,
    pub color: [u8; 3],
    pub position: [f32; 3],
    pub elapsed: f64,
    pub finished: bool,
    pub reference: bool,
}
#[derive(Clone, Debug)]
pub struct StudioRaceUi {
    pub section_clock: bool,
    pub phase: String,
    pub section: usize,
    pub runners: Vec<StudioRaceRunnerUi>,
    pub route: Vec<[f32; 3]>,
    pub(crate) map: Option<std::sync::Arc<super::StudioRaceMap>>,
    pub output_time: f64,
    /// Overlay font choice (see `renderer::load_studio_overlay_font`); the
    /// renderer reloads its atlas when this changes.
    pub font: String,
    /// Standings size multiplier (1 = the default size).
    pub readout_scale: f32,
}

// Standings layout in 1080p pixels, scaled with the output height.
const MARGIN_X: f32 = 22.0;
const MARGIN_Y: f32 = 20.0;
const PANEL_W: f32 = 400.0;
const ROW_H: f32 = 26.0;
const RANK_W: f32 = 38.0;
const TIME_W: f32 = 80.0;
const GAP_W: f32 = 76.0;
const NAME_CAP: f32 = 18.0 * 0.85;

fn readout_scale(count: usize, height: u32, scale: f32) -> f32 {
    let wanted = height as f32 / 1080.0 * scale.clamp(0.4, 2.5);
    let fit = (height as f32 - 28.0) / (count.max(1) as f32 * ROW_H);
    wanted.min(fit).max(0.1)
}

/// Screen rectangle [x, y, w, h] the standings occupy.
pub(crate) fn leaderboard_bounds(count: usize, width: u32, height: u32, scale: f32) -> [f32; 4] {
    let s = readout_scale(count, height, scale);
    [
        MARGIN_X * s,
        MARGIN_Y * s,
        (PANEL_W * s).min(width as f32 - 24.0),
        count as f32 * ROW_H * s,
    ]
}

/// Text in physical pixels with a uniform scale, so a TrueType face keeps its
/// proportions at every aspect ratio. `cap` is the cap height in pixels.
struct Pen<'a> {
    font: Option<&'a ProportionalFont>,
    width: u32,
    height: u32,
}

impl Pen<'_> {
    fn scale(font: &ProportionalFont, cap: f32) -> f32 {
        cap / font.glyphs[b'M' as usize].baseline.max(1) as f32
    }

    fn measure(&self, value: &str, cap: f32) -> f32 {
        let bytes = crate::cgame::text_to_jka_bytes(value);
        match self.font {
            Some(font) => {
                let k = Self::scale(font, cap);
                bytes
                    .iter()
                    .map(|&b| font.glyphs[b as usize].horiz_advance.max(0) as f32 * k)
                    .sum()
            }
            None => bytes.len() as f32 * 6.0 * cap / 7.0,
        }
    }

    fn draw(
        &self,
        out: &mut Vec<UiVertex>,
        value: &str,
        x: f32,
        baseline: f32,
        cap: f32,
        color: [f32; 4],
    ) {
        self.draw_weighted(out, value, x, baseline, cap, color, false);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_weighted(
        &self,
        out: &mut Vec<UiVertex>,
        value: &str,
        x: f32,
        baseline: f32,
        cap: f32,
        color: [f32; 4],
        bold: bool,
    ) {
        let Some(font) = self.font else {
            let scale = cap / 7.0;
            super::text::text(
                out,
                value,
                x,
                baseline - 8.0 * scale,
                scale,
                color,
                self.width,
                self.height,
            );
            if bold {
                super::text::text(
                    out,
                    value,
                    x + 0.7,
                    baseline - 8.0 * scale,
                    scale,
                    color,
                    self.width,
                    self.height,
                );
            }
            return;
        };
        let k = Self::scale(font, cap);
        let bytes = crate::cgame::text_to_jka_bytes(value);
        let shadow = (cap / 9.0).clamp(0.75, 2.0);
        // One shadow, then the foreground; the focused row alone gets a
        // second foreground stroke. Never paint another shadow over the text.
        let passes = [
            (shadow, [0.0, 0.0, 0.0, color[3] * 0.9]),
            (0.0, color),
            (0.7, color),
        ];
        for &(offset, tint) in &passes[..if bold { 3 } else { 2 }] {
            let mut cursor = x;
            for &byte in &bytes {
                let glyph = font.glyphs[byte as usize];
                if glyph.width > 0 && glyph.height > 0 && byte != b' ' {
                    textured_rect_with_source(
                        out,
                        cursor + glyph.horiz_offset as f32 * k + offset,
                        baseline - glyph.baseline as f32 * k + offset,
                        glyph.width as f32 * k,
                        glyph.height as f32 * k,
                        [glyph.s, glyph.t],
                        [glyph.s2, glyph.t2],
                        tint,
                        2.0,
                        self.width,
                        self.height,
                    );
                }
                cursor += glyph.horiz_advance.max(0) as f32 * k;
            }
        }
    }

    /// Longest prefix of `value` that fits `width`.
    fn fit(&self, value: &str, cap: f32, width: f32) -> String {
        let mut fitted: String = value.to_owned();
        while !fitted.is_empty() && self.measure(&fitted, cap) > width {
            fitted.pop();
        }
        fitted
    }
}

pub(crate) fn draw(
    out: &mut Vec<UiVertex>,
    race: &StudioRaceUi,
    font: Option<&ProportionalFont>,
    width: u32,
    height: u32,
) {
    super::studio_race_map::draw(out, race, width, height);
    let pen = Pen {
        font,
        width,
        height,
    };
    let s = readout_scale(race.runners.len(), height, race.readout_scale);
    let [x0, y0, panel_w, _] =
        leaderboard_bounds(race.runners.len(), width, height, race.readout_scale);
    let mut tree: TaffyTree<()> = TaffyTree::new();
    let mut rows = Vec::new();
    let mut cells = Vec::new();
    for _ in 0..race.runners.len() {
        let row_h = ROW_H * s;
        let mut row = Vec::new();
        for col in 0..4 {
            row.push(
                tree.new_leaf(Style {
                    size: Size {
                        width: if col == 1 {
                            auto()
                        } else {
                            length([RANK_W, 0.0, TIME_W, GAP_W][col] * s)
                        },
                        height: length(row_h),
                    },
                    flex_grow: if col == 1 { 1.0 } else { 0.0 },
                    flex_shrink: 0.0,
                    ..Default::default()
                })
                .unwrap(),
            );
        }
        rows.push(
            tree.new_with_children(
                Style {
                    size: Size {
                        width: length(panel_w),
                        height: length(row_h),
                    },
                    ..Default::default()
                },
                &row,
            )
            .unwrap(),
        );
        cells.push(row);
    }
    let root = tree
        .new_with_children(
            Style {
                flex_direction: FlexDirection::Column,
                size: Size {
                    width: length(panel_w),
                    height: auto(),
                },
                ..Default::default()
            },
            &rows,
        )
        .unwrap();
    if tree
        .compute_layout(
            root,
            Size {
                width: AvailableSpace::Definite(panel_w),
                height: AvailableSpace::MaxContent,
            },
        )
        .is_err()
    {
        return;
    }
    for (index, row) in cells.iter().enumerate() {
        let runner = &race.runners[index];
        let color = [
            runner.color[0] as f32 / 255.0,
            runner.color[1] as f32 / 255.0,
            runner.color[2] as f32 / 255.0,
            1.0,
        ];
        let cap = NAME_CAP * s;
        let values = [
            format!("#{}", runner.rank),
            runner.name.clone(),
            format!("{:.2}s", runner.elapsed),
            runner
                .gap
                .map_or_else(|| "--".into(), |g| format!("+{g:.2}")),
        ];
        let row_layout = tree.layout(rows[index]).unwrap();
        let baseline = y0 + row_layout.location.y + (row_layout.size.height + cap) * 0.5;
        for (col, &node) in row.iter().enumerate() {
            let cell = tree.layout(node).unwrap();
            let value = if col == 1 {
                pen.fit(&values[col], cap, cell.size.width - 6.0 * s)
            } else {
                values[col].clone()
            };
            let x = x0
                + cell.location.x
                + if col >= 2 {
                    cell.size.width - pen.measure(&value, cap) - 3.0 * s
                } else {
                    0.0
                };
            pen.draw_weighted(out, &value, x, baseline, cap, color, runner.focused);
        }
    }
}

/// Studio world name tags, centered on their projected point.
pub(crate) fn draw_labels(
    out: &mut Vec<UiVertex>,
    labels: &[UiStudioLabel],
    font: Option<&ProportionalFont>,
    width: u32,
    height: u32,
) {
    let pen = Pen {
        font,
        width,
        height,
    };
    for label in labels {
        let alpha = label.alpha.clamp(0.0, 1.0);
        if alpha <= 0.01 {
            continue;
        }
        let cap = 18.0 * label.scale.clamp(0.2, 2.0) * height as f32 / 1080.0;
        let x = label.pos[0] * width as f32 - pen.measure(&label.text, cap) * 0.5;
        let color = [
            label.color[0],
            label.color[1],
            label.color[2],
            label.color[3] * alpha,
        ];
        pen.draw(
            out,
            &label.text,
            x,
            label.pos[1] * height as f32,
            cap,
            color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standings_fit_32_runners_and_scale_with_the_setting() {
        let small = leaderboard_bounds(32, 1920, 1080, 1.0);
        assert!(small[1] + small[3] <= 1080.0, "{small:?}");
        let few = leaderboard_bounds(4, 1920, 1080, 1.0);
        let big = leaderboard_bounds(4, 1920, 1080, 1.5);
        assert!((big[2] / few[2] - 1.5).abs() < 1e-3);
        assert!(
            few[2] <= 400.0 + 1e-3,
            "standings retain the approved width: {few:?}"
        );
    }
}
