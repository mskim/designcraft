//! Embed mode (book_write's 지면): the editor reduced to correcting text and moving labelled
//! frames on a grid, inside a host page that it reports each edit to.

use serde_json::Value;

/// The tools an embedded editor may use: the type tool and the selection tool.
pub const TOOLS: &[&str] = &["type", "selection"];

/// Command prefixes an embedded editor refuses: formatting, styles, and anything that adds,
/// removes or rearranges objects, frames, tables, pages or threads. Text typing and deleting (text.insert, text.delete …), the
/// selection, view and file.open/close stay allowed.
pub const DENIED: &[&str] = &[
    // formatting and styles
    "type.",
    "style.",
    "swatch.",
    "strokeStyle.",
    "color.",
    "font.",
    "condition.",
    "page.",
    // adding, removing or rearranging objects, frames, tables, pages, layers and threads
    "object.",
    "frame.",
    "path.",
    "line.",
    "place.",
    "table.",
    "layer.",
    "layout.",
    "guide.",
    "flex.",
    "liquid.",
    "states.",
    "library.",
    "snippet.",
    "conveyor.",
    "data.",
    "anchor.",
    "anchored.",
    "footnote.",
    "endnote.",
    "math.",
    "xref.",
    "variables.",
    "hyperlink.",
    "xml.",
    "toc.",
    "index.",
    "book.",
    "links.",
    "script.",
    "prefs.",
    "edit.cut",
    "edit.paste",
    "edit.duplicate",
    "edit.clear",
    "edit.stepAndRepeat",
    "transform.scale",
    "transform.flip",
    "transform.shear",
    "transform.clear",
    "text.thread",
    "text.unthread",
    "text.removeFromThread",
    "text.release",
    "story.link",
    "story.unlink",
    "story.placeAndLink",
    "story.updateLink",
    "story.setDirection",
    "file.",
    "document.",
];

/// Ids allowed although their prefix is denied: opening and closing the host's document. The
/// embed's own lock, snap and undo go through Session::execute_trusted, past the filter.
pub const ALLOWED: &[&str] = &["file.openBytes", "file.close", "object.findByLabel", "story.links"];

pub fn tool_allowed(id: &str) -> bool {
    TOOLS.contains(&id)
}

pub fn command_allowed(id: &str) -> bool {
    ALLOWED.contains(&id) || !DENIED.iter().any(|p| id.starts_with(p))
}

/// The text area's 6 × 12 grid and body lines, per page (page coordinates, points).
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub columns: u32,
    pub rows: u32,
    pub line_height: f32,
    pub text_areas: Vec<[f32; 4]>,
    /// Where page 1's text starts, below the heading area (None: the text area's top).
    pub first_page_top: Option<f32>,
}

impl Grid {
    /// The area a frame on `page` (0-based) snaps inside: the text area, below the heading on page 1.
    pub fn area(&self, page: usize) -> Option<[f32; 4]> {
        let mut a = *self.text_areas.get(page)?;
        if page == 0
            && let Some(top) = self.first_page_top
        {
            a[1] = a[1].max(top);
        }
        Some(a)
    }
}

impl Grid {
    pub fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            columns: v.get("columns")?.as_u64()? as u32,
            rows: v.get("rows")?.as_u64()? as u32,
            line_height: v.get("lineHeight")?.as_f64()? as f32,
            text_areas: v
                .get("textAreas")?
                .as_array()?
                .iter()
                .map(|a| {
                    let a = a.as_array()?;
                    Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32, a.get(2)?.as_f64()? as f32, a.get(3)?.as_f64()? as f32])
                })
                .collect::<Option<Vec<_>>>()?,
            first_page_top: v.get("firstPageTop").and_then(Value::as_f64).map(|t| t as f32),
        })
    }
}

/// A frame's kind, from its label's prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Image,
    Lead,
    Table,
}

impl Kind {
    pub fn of(label: &str) -> Option<Self> {
        match label.split_once(':')?.0 {
            "image" => Some(Self::Image),
            "lead" => Some(Self::Lead),
            "table" => Some(Self::Table),
            _ => None,
        }
    }
}

/// Where a dropped frame lands on the grid, as the app writes it back.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub col: u32,
    pub width: u32,
    /// Height in body lines; None for a table (its height is its content's).
    pub lines: Option<u32>,
    /// "line" | "top" | "middle" | "bottom".
    pub v: &'static str,
    /// The snapped rect, page coordinates.
    pub rect: [f32; 4],
}

/// Snaps a frame dropped at `rect` (page coordinates) inside `area` to whole grid columns and body
/// lines, and to the nearest of the vertical positions open to it: top, middle, bottom, and (a
/// lead box or table that sat at its line, at `line_y`) its line. `columns`/`line_height` from Grid.
pub fn snap(kind: Kind, rect: [f32; 4], area: [f32; 4], columns: u32, line_height: f32, line_y: Option<f32>) -> Placement {
    let [ax0, ay0, ax1, ay1] = area;
    let col_w = (ax1 - ax0) / columns as f32;
    let width = (((rect[2] - rect[0]) / col_w).round() as u32).clamp(1, columns);
    let col = ((((rect[0] - ax0) / col_w).round() as i64) + 1).clamp(1, (columns - width + 1) as i64) as u32;
    let lines = (((rect[3] - rect[1]) / line_height).round() as u32).max(1);
    let h = if kind == Kind::Table { rect[3] - rect[1] } else { lines as f32 * line_height }.min(ay1 - ay0);
    let mut options = vec![("top", ay0), ("middle", ay0 + (ay1 - ay0 - h) / 2.0), ("bottom", ay1 - h)];
    if kind != Kind::Image
        && let Some(y) = line_y
    {
        options.push(("line", y));
    }
    let (v, y) = options.into_iter().min_by(|a, b| (a.1 - rect[1]).abs().total_cmp(&(b.1 - rect[1]).abs())).unwrap_or(("top", ay0));
    let x = ax0 + (col - 1) as f32 * col_w;
    Placement { col, width, lines: (kind != Kind::Table).then_some(lines), v, rect: [x, y, x + width as f32 * col_w, y + h] }
}

/// Which paragraphs differ between two versions of a story, by index, when it still has as many
/// paragraphs; None when a paragraph was added or removed (the embed undoes that edit).
pub fn changed_paragraphs(old: &[String], new: &[String]) -> Option<Vec<usize>> {
    (old.len() == new.len()).then(|| (0..new.len()).filter(|&i| old[i] != new[i]).collect())
}

/// What an embedded editor tells its host (embed_protocol turns these into messages).
#[derive(Clone, Debug, PartialEq)]
pub enum EmbedEvent {
    Loaded { spread_count: usize },
    Spread(usize),
    TextEdited { story: String, paragraph: usize, text: String, cell: Option<(usize, usize, usize)> },
    FrameMoved { label: String, placement: Placement },
    Spreads(usize),
    Tool(&'static str),
    Rejected(&'static str),
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: [f32; 4] = [60.0, 100.0, 360.0, 580.0]; // 300 wide (50 a column), 480 tall

    #[test]
    fn only_the_type_and_selection_tools() {
        assert!(tool_allowed("type") && tool_allowed("selection"));
        assert!(!tool_allowed("directSelection") && !tool_allowed("pen") && !tool_allowed("rectangle"));
    }

    #[test]
    fn formatting_and_object_commands_are_refused_text_and_view_are_not() {
        for id in [
            "type.bold",
            "type.char",
            "type.kernStep",
            "style.paragraph.apply",
            "object.delete",
            "object.lock",
            "object.unlockAll",
            "object.label",
            "frame.create",
            "edit.paste",
            "table.insertRow",
            "file.save",
            "file.export",
            "layout.overrideParentItems",
        ] {
            assert!(!command_allowed(id), "{id}");
        }
        for id in [
            "text.insert",
            "text.delete",
            "edit.undo",
            "edit.redo",
            "view.fitSpread",
            "selection.set",
            "transform.move",
            "transform.resize",
            "file.openBytes",
            "file.close",
        ] {
            assert!(command_allowed(id), "{id}");
        }
    }

    #[test]
    fn a_drop_snaps_to_columns_lines_and_the_nearest_position() {
        let p = snap(Kind::Image, [112.0, 108.0, 262.0, 283.0], AREA, 6, 22.0, None); // ~3 columns from col 2, ~8 lines, near the top
        assert_eq!((p.col, p.width, p.lines, p.v), (2, 3, Some(8), "top"));
        assert_eq!(p.rect, [110.0, 100.0, 260.0, 276.0]);
        let bottom = snap(Kind::Image, [60.0, 400.0, 360.0, 570.0], AREA, 6, 22.0, Some(400.0));
        assert_eq!(bottom.v, "bottom"); // an image never sits at its line
    }

    #[test]
    fn a_lead_box_or_table_can_go_back_to_its_line() {
        assert_eq!(snap(Kind::Lead, [210.0, 302.0, 360.0, 390.0], AREA, 6, 22.0, Some(300.0)).v, "line");
        let table = snap(Kind::Table, [210.0, 300.0, 360.0, 378.0], AREA, 6, 22.0, Some(300.0)); // its line 0 away, middle (301) 1
        assert_eq!((table.v, table.lines, table.rect[3] - table.rect[1]), ("line", None, 78.0)); // a table keeps its height
    }

    #[test]
    fn width_and_column_stay_inside_the_grid() {
        let p = snap(Kind::Image, [300.0, 100.0, 420.0, 200.0], AREA, 6, 22.0, None);
        assert!(p.col + p.width - 1 <= 6);
    }

    #[test]
    fn page_one_snaps_below_the_heading() {
        let grid = Grid { columns: 6, rows: 12, line_height: 22.0, text_areas: vec![AREA, AREA], first_page_top: Some(190.0) };
        assert_eq!(grid.area(0).map(|a| a[1]), Some(190.0));
        assert_eq!(grid.area(1).map(|a| a[1]), Some(100.0));
    }

    #[test]
    fn changed_paragraphs_by_index_or_none_when_the_count_changes() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(changed_paragraphs(&a(&["가", "나", "다"]), &a(&["가", "나!", "다"])), Some(vec![1]));
        assert_eq!(changed_paragraphs(&a(&["가", "나"]), &a(&["가", "나", ""])), None);
    }
}
