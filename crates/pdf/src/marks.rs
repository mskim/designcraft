//! Printer's marks drawn in the slug area: crop marks at the trim corners, bleed marks at the bleed
//! corners, registration targets beside each edge, fold marks where a spread's pages meet, colour
//! bars above the trim and a page-information line. All but the colour bars use `[Registration]`
//! (every plate); the colour bars are process colours.

use designcraft_color::Color;
use designcraft_geom::kurbo::{Circle, Shape as _};
use designcraft_geom::{BezPath, Point, Rect};
use krilla::num::NormalizedF32;
use krilla::paint::{LineCap, LineJoin, Stroke, StrokeDash};
use krilla::surface::Surface;

use crate::export::{Exporter, MARK_LEN, Sheet, civil, device, mark_start, registration, solid_fill, to_path};

/// Colour-bar patch size and the registration target's outer radius, in points.
const PATCH: f64 = 9.0;
const TARGET_R: f64 = 5.0;

impl Exporter<'_> {
    pub(crate) fn marks(&mut self, s: &mut Surface, sh: &Sheet, title: &str, created: Option<i64>) {
        let m = self.opts.marks;
        if !m.any() {
            return;
        }
        let bleed = if self.opts.bleed { self.doc.settings.bleed.map(|v| v.max(0.0)) } else { [0.0; 4] };
        let start = mark_start(self.opts, bleed);
        let t = sh.trim;
        let mut bp = BezPath::new();
        let mut seg = |a: (f64, f64), b: (f64, f64)| {
            bp.move_to(Point::new(a.0, a.1));
            bp.line_to(Point::new(b.0, b.1));
        };
        if m.crop {
            // Each trim corner: a horizontal mark beside it and a vertical one above/below it.
            for (x, sx) in [(t.x0, -1.0), (t.x1, 1.0)] {
                for (y, sy) in [(t.y0, -1.0), (t.y1, 1.0)] {
                    seg((x + sx * start, y), (x + sx * (start + MARK_LEN), y));
                    seg((x, y + sy * start), (x, y + sy * (start + MARK_LEN)));
                }
            }
        }
        let b = sh.bleed;
        if m.bleed && (b.x0 < t.x0 || b.y0 < t.y0 || b.x1 > t.x1 || b.y1 > t.y1) {
            // Shorter marks on the bleed lines, outside the crop-mark start.
            let len = MARK_LEN / 2.0;
            for (x, bx, sx) in [(t.x0, b.x0, -1.0), (t.x1, b.x1, 1.0)] {
                for (y, by, sy) in [(t.y0, b.y0, -1.0), (t.y1, b.y1, 1.0)] {
                    seg((x + sx * start, by), (x + sx * (start + len), by));
                    seg((bx, y + sy * start), (bx, y + sy * (start + len)));
                }
            }
        }
        // Registration targets: the middle of each side, in the marks' band.
        let mid = start + MARK_LEN / 2.0;
        let targets = [
            ((t.x0 + t.x1) / 2.0, t.y0 - mid),
            ((t.x0 + t.x1) / 2.0, t.y1 + mid),
            (t.x0 - mid, (t.y0 + t.y1) / 2.0),
            (t.x1 + mid, (t.y0 + t.y1) / 2.0),
        ];
        if m.registration {
            for &(x, y) in &targets {
                seg((x - TARGET_R - 2.0, y), (x + TARGET_R + 2.0, y));
                seg((x, y - TARGET_R - 2.0), (x, y + TARGET_R + 2.0));
            }
            for &(x, y) in &targets {
                bp.extend(Circle::new((x, y), TARGET_R).path_elements(0.05));
            }
        }
        self.stroke_marks(s, &bp, m.weight, None);
        if m.registration {
            let mut dots = BezPath::new();
            for &(x, y) in &targets {
                dots.extend(Circle::new((x, y), TARGET_R * 0.4).path_elements(0.05));
            }
            if let Some(p) = to_path(&dots) {
                s.set_stroke(None);
                s.set_fill(Some(solid_fill(registration(1.0, self.rgb_only), 1.0)));
                s.draw_path(&p);
                s.set_fill(None);
            }
        }
        if m.folds && !sh.folds.is_empty() {
            // Dashed, on each fold's line above and below the trim, like the crop marks.
            let mut fp = BezPath::new();
            for &x in &sh.folds {
                for (y, sy) in [(t.y0, -1.0), (t.y1, 1.0)] {
                    fp.move_to(Point::new(x, y + sy * start));
                    fp.line_to(Point::new(x, y + sy * (start + MARK_LEN)));
                }
            }
            self.stroke_marks(s, &fp, m.weight, Some(vec![3.0, 2.0]));
        }
        if m.color_bars {
            self.color_bars(s, sh, start);
        }
        if m.page_info {
            let mut info = format!("{title}    Page {}", sh.label);
            if let Some(c) = created.map(civil) {
                info.push_str(&format!("    {:04}-{:02}-{:02} {:02}:{:02} UTC", c.0, c.1, c.2, c.3, c.4));
            }
            let at = (t.x0 + start + 6.0, t.y1 + start + 10.0);
            self.plain_text(s, &info, at, 6.0, registration(1.0, self.rgb_only));
        }
    }

    fn stroke_marks(&mut self, s: &mut Surface, bp: &BezPath, weight: f64, dash: Option<Vec<f32>>) {
        let Some(p) = to_path(bp) else { return };
        s.set_fill(None);
        s.set_stroke(Some(Stroke {
            paint: registration(1.0, self.rgb_only).into(),
            width: weight.max(0.05) as f32,
            miter_limit: 4.0,
            line_cap: LineCap::Butt,
            line_join: LineJoin::Miter,
            opacity: NormalizedF32::ONE,
            dash: dash.map(|array| StrokeDash { array, offset: 0.0 }),
        }));
        s.draw_path(&p);
        s.set_stroke(None);
    }

    /// Solid C, M, Y, K, their overprints (CM, CY, MY, CMY) from the left and a black tint ramp
    /// from the right, in the band above the trim. Patches that would cover the top registration
    /// target or a fold mark are left out.
    fn color_bars(&mut self, s: &mut Surface, sh: &Sheet, start: f64) {
        let t = sh.trim;
        let y0 = t.y0 - start - MARK_LEN / 2.0 - PATCH / 2.0;
        let cx = (t.x0 + t.x1) / 2.0;
        let clear = |x: f64| {
            let r = x..x + PATCH;
            !(x + PATCH > cx - TARGET_R - 3.0 && x < cx + TARGET_R + 3.0) && !sh.folds.iter().any(|f| r.contains(f))
        };
        let solids = [
            (1.0, 0.0, 0.0, 0.0),
            (0.0, 1.0, 0.0, 0.0),
            (0.0, 0.0, 1.0, 0.0),
            (0.0, 0.0, 0.0, 1.0),
            (1.0, 1.0, 0.0, 0.0),
            (1.0, 0.0, 1.0, 0.0),
            (0.0, 1.0, 1.0, 0.0),
            (1.0, 1.0, 1.0, 0.0),
        ];
        let left: Vec<(f64, Color)> =
            solids.iter().enumerate().map(|(i, &(c, m, y, k))| (t.x0 + 6.0 + i as f64 * PATCH, Color::cmyk(c, m, y, k))).collect();
        let right: Vec<(f64, Color)> =
            (1..=10).map(|i| (t.x1 - 6.0 - (11 - i) as f64 * PATCH, Color::cmyk(0.0, 0.0, 0.0, i as f32 / 10.0))).collect();
        let left_end = left.last().map_or(t.x0, |p| p.0 + PATCH);
        let right = right.into_iter().filter(|p| p.0 >= left_end);
        for (x, c) in left.into_iter().chain(right).filter(|p| clear(p.0)) {
            let r = Rect::new(x, y0, x + PATCH, y0 + PATCH);
            if let Some(p) = to_path(&r.to_path(0.1)) {
                s.set_stroke(None);
                s.set_fill(Some(solid_fill(device(&c, self.rgb_only), 1.0)));
                s.draw_path(&p);
            }
        }
        s.set_fill(None);
    }
}
