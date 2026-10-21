//! Embed mode (book_write's 지면): the editor reduced to correcting text and moving labelled
//! frames on a grid, inside a host page that it reports each edit to.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use designcraft_doc::{CellAddr, Content, Document, Item, ItemId, SpreadRef, Story, StoryId};
use designcraft_geom::Rect;
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

pub fn command_allowed(id: &str, _params: &Value) -> bool {
    ALLOWED.contains(&id) || !DENIED.iter().any(|p| id.starts_with(p))
}

/// The app-level ids (handled by the UI, never reaching the engine) an embedded editor refuses:
/// view toggles and screen modes that would bring chrome back, panels, windows, dialogs and help.
/// Zooming, fitting and going to a page or spread stay.
pub fn ui_command_allowed(id: &str) -> bool {
    if let Some(v) = id.strip_prefix("view.") {
        return v != "rotateSpread"
            && (v.starts_with("fit") || v.starts_with("zoom") || v.ends_with("Spread") || v.ends_with("Page") || v == "goToPage");
    }
    !["panel.", "window.", "app.", "help."].iter().any(|p| id.starts_with(p))
}

/// What the host gave in `load`: the grid, and each movable frame's vertical position now.
#[derive(Clone, Debug, Default)]
pub struct EmbedConfig {
    pub grid: Option<Grid>,
    /// label → its v ("line" | "top" | "middle" | "bottom"), as the source has it.
    pub frames: std::collections::BTreeMap<String, String>,
}

/// An embedded editor's state between frames (the change tracker's last look).
#[derive(Default)]
pub struct Embed {
    pub config: EmbedConfig,
    pub(crate) last_doc: Option<Arc<Document>>,
    pub(crate) last_spread: Option<usize>,
    pub(crate) last_tool: Option<&'static str>,
    /// The paragraph edit not reported yet.
    pub(crate) pending_text: Option<PendingText>,
}

/// A paragraph being corrected, reported when the caret leaves it or after a second's pause.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingText {
    pub change: TextChange,
    /// When it last changed (egui time, seconds).
    pub at: f64,
}

/// One paragraph whose text changed: in `story`, or in one of its table cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextChange {
    pub story: StoryId,
    pub cell: Option<CellAddr>,
    pub para: usize,
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

/// The name the host knows a story by, from the label of a frame showing it: `story:main` →
/// "main", `lead:6` / `table:7` → the label itself. None for anything else.
pub fn story_name(label: &str) -> Option<String> {
    if let Some(n) = label.strip_prefix("story:") {
        return (!n.is_empty()).then(|| n.to_string());
    }
    matches!(Kind::of(label), Some(Kind::Lead | Kind::Table)).then(|| label.to_string())
}

/// Every named story of the document's pages (see [`story_name`]).
pub fn story_names(doc: &Document) -> BTreeMap<StoryId, String> {
    let mut out = BTreeMap::new();
    for sp in &doc.spreads {
        for item in &sp.items {
            item.walk(&mut |i| {
                if let (Some(t), Some(n)) = (i.text_frame(), story_name(&i.label)) {
                    out.entry(t.story).or_insert(n);
                }
            });
        }
    }
    out
}

/// A story's paragraphs as plain text.
pub fn paragraph_texts(story: &Story) -> Vec<String> {
    story.para_ranges().into_iter().map(|r| story.slice(r).to_string()).collect()
}

/// Each movable labelled frame on the document's pages (top level): label → (id, bounds).
pub fn labelled_bounds(doc: &Document) -> BTreeMap<String, (ItemId, Rect)> {
    doc.spreads.iter().flat_map(|sp| &sp.items).filter(|i| Kind::of(&i.label).is_some()).map(|i| (i.label.clone(), (i.id, i.bounds()))).collect()
}

/// A table's place among its story's tables, in text order (as the source map numbers them).
pub fn table_index(story: &Story, table: u64) -> Option<usize> {
    let mut order: Vec<(usize, u64)> = story.tables.keys().filter_map(|id| Some((story.table_anchor(*id)?, *id))).collect();
    order.sort();
    order.iter().position(|(_, id)| *id == table)
}

/// The paragraphs whose text changed from `last` to `cur`, stories and their table cells; Err with
/// the reason the embed refuses the edit: "paragraphs" (one added or removed) or "story" (text that
/// isn't in a story the host knows).
pub fn text_changes(last: &Document, cur: &Document) -> Result<Vec<TextChange>, &'static str> {
    fn diff(old: &Story, new: &Story, story: StoryId, cell: Option<CellAddr>, out: &mut Vec<TextChange>) -> Result<(), &'static str> {
        if old.text == new.text {
            return Ok(());
        }
        let changed = changed_paragraphs(&paragraph_texts(old), &paragraph_texts(new)).ok_or("paragraphs")?;
        out.extend(changed.into_iter().map(|para| TextChange { story, cell, para }));
        Ok(())
    }
    let names = story_names(cur);
    let mut out = Vec::new();
    for (sid, s) in &cur.stories {
        // A new story comes with a new frame, which the item check refuses.
        let Some(old) = last.stories.get(sid) else { continue };
        if Arc::ptr_eq(old, s) {
            continue;
        }
        let mut changes = Vec::new();
        diff(old, s, *sid, None, &mut changes)?;
        for (tid, t) in &s.tables {
            let Some(ot) = old.tables.get(tid) else { continue };
            let cols = t.columns.len().max(1);
            if Arc::ptr_eq(ot, t) || ot.columns.len() != t.columns.len() {
                continue;
            }
            for (k, (oc, c)) in ot.cells.iter().zip(&t.cells).enumerate() {
                diff(&oc.text, &c.text, *sid, Some(CellAddr { table: *tid, row: k / cols, col: k % cols }), &mut changes)?;
            }
        }
        if !changes.is_empty() && !names.contains_key(sid) {
            return Err("story");
        }
        out.extend(changes);
    }
    Ok(out)
}

fn items(doc: &Document) -> BTreeMap<ItemId, &Item> {
    let mut out = BTreeMap::new();
    for sp in &doc.spreads {
        for item in &sp.items {
            item.walk(&mut |i| {
                out.insert(i.id, i);
            });
        }
    }
    out
}

fn same_rect(a: Rect, b: Rect) -> bool {
    (a.x0 - b.x0).abs() < 1e-3 && (a.y0 - b.y0).abs() < 1e-3 && (a.x1 - b.x1).abs() < 1e-3 && (a.y1 - b.y1).abs() < 1e-3
}

/// An item's rotation, shear and flip (what a move or resize leaves alone).
fn orientation(i: &Item) -> [f64; 3] {
    let [a, b, c, d, ..] = i.xf.as_coeffs();
    let (u, v) = (a.hypot(b), c.hypot(d));
    let shear = if u * v > 1e-12 { (a * c + b * d) / (u * v) } else { 0.0 };
    [b.atan2(a), shear, (a * d - b * c).signum()]
}

fn graphic_xf(i: &Item) -> Option<[f64; 6]> {
    match &i.content {
        Content::Graphic(g) => Some(g.xf.as_coeffs()),
        _ => None,
    }
}

/// The stories whose text (or a cell's) changed.
fn changed_stories(last: &Document, cur: &Document) -> BTreeSet<StoryId> {
    cur.stories
        .iter()
        .filter(|(sid, s)| last.stories.get(sid).is_none_or(|o| !Arc::ptr_eq(o, s) && (o.text != s.text || o.tables != s.tables)))
        .map(|(sid, _)| *sid)
        .collect()
}

/// Whether the items changed only as an embed allows from `last` to `cur`: labelled frames moved
/// or resized, and Smart Text Reflow's pages and frames of the primary story added or removed.
/// Ok holds the frames reflow added (to lock); Err(()) is any other change: a new or removed item,
/// an unlabelled item moved, a label changed, a labelled one rotated, sheared, flipped or re-cropped.
#[allow(clippy::result_unit_err)]
pub fn structural_change(last: &Document, cur: &Document) -> Result<Vec<ItemId>, ()> {
    let primary = cur.settings.primary_story.or(last.settings.primary_story);
    let reflow = |i: &Item| primary.is_some() && i.text_frame().map(|t| t.story) == primary;
    let pages = cur.page_count().cmp(&last.page_count());
    let text = changed_stories(last, cur);
    let (old, new) = (items(last), items(cur));
    let mut added = Vec::new();
    for (id, i) in &new {
        let Some(o) = old.get(id) else {
            if reflow(i) && pages.is_gt() {
                added.push(*id);
                continue;
            }
            return Err(());
        };
        if o.label != i.label {
            return Err(());
        }
        let moved = !same_rect(o.bounds(), i.bounds());
        if Kind::of(&i.label).is_some() {
            let recropped = !moved && graphic_xf(o) != graphic_xf(i);
            if orientation(o).iter().zip(orientation(i)).any(|(a, b)| (a - b).abs() > 1e-6) || recropped {
                return Err(());
            }
        } else if moved && !i.text_frame().is_some_and(|t| text.contains(&t.story)) {
            // An auto-sizing frame grows with its text; nothing else moves an unlabelled item.
            return Err(());
        }
    }
    let removed: Vec<_> = old.iter().filter(|(id, _)| !new.contains_key(id)).collect();
    if removed.iter().any(|(_, o)| !(reflow(o) && pages.is_lt())) {
        return Err(());
    }
    if !pages.is_eq() && added.is_empty() && removed.is_empty() {
        return Err(());
    }
    Ok(added)
}

/// A labelled frame that moved: where it was and where it is (spread coordinates), and the page
/// it was on (which it snaps back onto).
#[derive(Clone, Debug, PartialEq)]
pub struct FrameMove {
    pub id: ItemId,
    pub label: String,
    pub kind: Kind,
    pub was: Rect,
    pub now: Rect,
    /// Absolute page index, its spread, and the page's left edge in that spread.
    pub page: usize,
    pub spread: SpreadRef,
    pub page_x: f64,
}

/// The labelled frames whose bounds changed from `last` to `cur`, but for one that only grew or
/// shrank with its text (auto-size).
pub fn frame_moves(last: &Document, cur: &Document) -> Vec<FrameMove> {
    let text = changed_stories(last, cur);
    let before = labelled_bounds(last);
    let mut out = Vec::new();
    for (label, (id, now)) in labelled_bounds(cur) {
        let Some(&(old_id, was)) = before.get(&label) else { continue };
        let Some(kind) = Kind::of(&label) else { continue };
        let autosized = cur.item(id).and_then(Item::text_frame).is_some_and(|t| text.contains(&t.story));
        if old_id != id || same_rect(was, now) || autosized {
            continue;
        }
        let Some(page) = last.page_of_item(id) else { continue };
        let Some((si, pi)) = last.page_loc(page) else { continue };
        let Some(p) = last.spreads.get(si).and_then(|s| s.pages.get(pi)) else { continue };
        out.push(FrameMove { id, label, kind, was, now, page, spread: SpreadRef::Doc(si), page_x: p.x });
    }
    out
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
            assert!(!command_allowed(id, &Value::Null), "{id}");
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
            assert!(command_allowed(id, &Value::Null), "{id}");
        }
    }

    #[test]
    fn view_toggles_panels_and_windows_are_refused_navigation_is_not() {
        for id in [
            "view.rulers",
            "view.frameEdges",
            "view.screenMode",
            "view.togglePresentation",
            "view.rotateSpread",
            "panel.show",
            "window.panel",
            "app.preferences",
        ] {
            assert!(!ui_command_allowed(id), "{id}");
        }
        for id in [
            "view.fitSpread",
            "view.zoomIn",
            "view.zoom",
            "view.nextSpread",
            "view.firstPage",
            "view.goToPage",
            "text.insert",
            "edit.undo",
            "tool.select",
            "file.openBytes",
        ] {
            assert!(ui_command_allowed(id), "{id}");
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

    mod app {
        use super::super::*;
        use crate::{DesignApp, Inbox, Services};
        use designcraft_engine::Session;
        use designcraft_tools::{CanvasLayout, PointerEvent, PointerKind};
        use serde_json::json;

        /// An app with a document holding an unlabelled text frame and one labelled `image:1`,
        /// in embed mode. Returns (app, the unlabelled frame, its story, the labelled frame).
        fn embedded() -> (DesignApp, u64, u64, u64) {
            let mut app = DesignApp::new(Session::new(), Services { inbox: Some(Inbox::default()), ..Default::default() });
            app.run("file.new", json!({})).unwrap();
            let body = app.run("frame.create", json!({"rect": [36, 36, 300, 200], "content": "text", "text": "본문"})).unwrap();
            let image = app.run("frame.create", json!({"rect": [36, 300, 300, 400], "content": "text"})).unwrap();
            let image_id = image["id"].as_u64().unwrap();
            app.run("object.label", json!({"label": "image:1", "ids": [image_id]})).unwrap();
            app.embed_begin(EmbedConfig::default());
            (app, body["id"].as_u64().unwrap(), body["story"].as_u64().unwrap(), image_id)
        }

        fn locked(app: &DesignApp, id: u64) -> bool {
            let st = app.session.active().unwrap();
            let mut found = None;
            for sp in &st.doc.spreads {
                for item in &sp.items {
                    item.walk(&mut |i| {
                        if i.id.0 == id {
                            found = Some(i.locked);
                        }
                    });
                }
            }
            found.unwrap()
        }

        fn refused(r: Result<serde_json::Value, String>) -> bool {
            r.is_err_and(|e| e.contains("not available here"))
        }

        const LINE: f32 = 22.0;

        /// A grid whose text area is each page inside 36 pt margins.
        fn grid_of(app: &DesignApp) -> Grid {
            let d = &app.session.active().unwrap().doc;
            let text_areas = d.pages().map(|(_, _, p)| [36.0, 36.0, p.width as f32 - 36.0, p.height as f32 - 36.0]).collect();
            Grid { columns: 6, rows: 12, line_height: LINE, text_areas, first_page_top: None }
        }

        /// A one-page document whose primary text frame is labelled `story:main` and holds two
        /// paragraphs, in embed mode. Returns (app, the main story).
        fn text_doc() -> (DesignApp, u64) {
            let mut app = DesignApp::new(Session::new(), Services { inbox: Some(Inbox::default()), ..Default::default() });
            app.run("file.new", json!({"pages": 1, "primaryTextFrame": true})).unwrap();
            let doc = app.session.active().unwrap().doc.clone();
            let sid = doc.settings.primary_story.unwrap();
            let frame = doc.story(sid).unwrap().frames[0];
            app.run("object.label", json!({"label": "story:main", "ids": [frame.0]})).unwrap();
            app.run("text.select", json!({"story": sid.0, "anchor": 0, "focus": 0})).unwrap();
            app.run("text.insert", json!({"text": "첫 문단\n둘째 문단"})).unwrap();
            let grid = grid_of(&app);
            app.embed_begin(EmbedConfig { grid: Some(grid), ..Default::default() });
            (app, sid.0)
        }

        /// Three facing pages with an `image:1` frame on page 2 (the left page of spread 1), in
        /// embed mode. Returns (app, the frame).
        fn frame_doc() -> (DesignApp, u64) {
            let mut app = DesignApp::new(Session::new(), Services { inbox: Some(Inbox::default()), ..Default::default() });
            app.run("file.new", json!({"pages": 3, "facingPages": true})).unwrap();
            let x = app.session.active().unwrap().doc.spreads[1].pages[0].x;
            let image = app.run("frame.create", json!({"spread": 1, "rect": [x + 60.0, 80.0, x + 260.0, 260.0], "content": "graphic"})).unwrap();
            let id = image["id"].as_u64().unwrap();
            app.run("object.label", json!({"label": "image:1", "ids": [id]})).unwrap();
            let grid = grid_of(&app);
            app.embed_begin(EmbedConfig { grid: Some(grid), ..Default::default() });
            (app, id)
        }

        fn paragraphs(app: &DesignApp, story: u64) -> Vec<String> {
            paragraph_texts(app.session.active().unwrap().doc.story(designcraft_doc::StoryId(story)).unwrap())
        }

        fn bounds(app: &DesignApp, id: u64) -> designcraft_geom::Rect {
            app.session.active().unwrap().doc.item(designcraft_doc::ItemId(id)).unwrap().bounds()
        }

        fn close(a: [f64; 4], b: [f32; 4]) -> bool {
            a.iter().zip(b).all(|(a, b)| (*a as f32 - b).abs() < 0.01)
        }

        #[test]
        fn stories_are_named_by_their_frames_labels() {
            assert_eq!(story_name("story:main").as_deref(), Some("main"));
            assert_eq!(story_name("lead:6").as_deref(), Some("lead:6"));
            assert_eq!(story_name("table:7").as_deref(), Some("table:7"));
            assert_eq!(story_name("image:4"), None);
            assert_eq!(story_name(""), None);
            let (app, story) = text_doc();
            let names = story_names(&app.session.active().unwrap().doc);
            assert_eq!(names.get(&designcraft_doc::StoryId(story)).map(String::as_str), Some("main"));
        }

        #[test]
        fn a_text_edit_is_reported_after_a_pause() {
            let (mut app, story) = text_doc();
            let end = "첫 문단".len();
            app.run("text.select", json!({"story": story, "anchor": end, "focus": end})).unwrap();
            app.run("text.insert", json!({"text": "!"})).unwrap();
            assert_eq!(app.embed_poll(0.0), vec![]);
            assert_eq!(app.embed_poll(1.5), vec![EmbedEvent::TextEdited { story: "main".into(), paragraph: 0, text: "첫 문단!".into(), cell: None }]);
            assert_eq!(app.embed_poll(3.0), vec![], "reported once");
        }

        #[test]
        fn moving_the_caret_out_flushes_the_edit() {
            let (mut app, story) = text_doc();
            let end = "첫 문단".len();
            app.run("text.select", json!({"story": story, "anchor": end, "focus": end})).unwrap();
            app.run("text.insert", json!({"text": "!"})).unwrap();
            let next = "첫 문단!\n".len();
            app.run("text.select", json!({"story": story, "anchor": next, "focus": next})).unwrap();
            assert_eq!(app.embed_poll(0.1), vec![EmbedEvent::TextEdited { story: "main".into(), paragraph: 0, text: "첫 문단!".into(), cell: None }]);
        }

        #[test]
        fn a_new_paragraph_is_undone() {
            let (mut app, story) = text_doc();
            app.run("text.select", json!({"story": story, "anchor": 2, "focus": 2})).unwrap();
            app.run("text.insert", json!({"text": "\n"})).unwrap();
            assert_eq!(paragraphs(&app, story).len(), 3);
            assert_eq!(app.embed_poll(0.0), vec![EmbedEvent::Rejected("paragraphs")]);
            assert_eq!(paragraphs(&app, story), vec!["첫 문단", "둘째 문단"]);
            assert_eq!(app.embed_poll(2.0), vec![], "nothing pending");
        }

        #[test]
        fn a_move_snaps_to_the_grid() {
            let (mut app, id) = frame_doc();
            app.session.execute("transform.move", &json!({"ids": [id], "dx": 37, "dy": 101})).unwrap();
            let moved = bounds(&app, id);
            let (page_x, grid) = (app.session.active().unwrap().doc.spreads[1].pages[0].x, grid_of(&app));
            let rect = [(moved.x0 - page_x) as f32, moved.y0 as f32, (moved.x1 - page_x) as f32, moved.y1 as f32];
            let expected = snap(Kind::Image, rect, grid.area(1).unwrap(), 6, LINE, None);
            assert_eq!(app.embed_poll(0.0), vec![EmbedEvent::FrameMoved { label: "image:1".into(), placement: expected.clone() }]);
            let b = bounds(&app, id);
            let r = expected.rect;
            assert!(close([b.x0 - page_x, b.y0, b.x1 - page_x, b.y1], r), "{b:?} vs {r:?}");
            assert_eq!(app.embed_poll(0.1), vec![], "the snap isn't a move of its own");
            // One undo takes back the move and its snap.
            app.run("edit.undo", json!({})).unwrap();
            assert!((bounds(&app, id).x0 - (page_x + 60.0)).abs() < 0.01);
        }

        #[test]
        fn no_events_while_dragging() {
            let (mut app, id) = frame_doc();
            {
                let st = app.session.active_mut().unwrap();
                st.interaction = Some(designcraft_engine::Interaction {
                    label: "Move".into(),
                    doc: st.doc.clone(),
                    selection: st.selection.clone(),
                    preview: None,
                    revision: st.revision,
                });
            }
            app.session.execute("transform.move", &json!({"ids": [id], "dx": 40, "dy": 40})).unwrap();
            assert_eq!(app.embed_poll(0.0), vec![]);
            app.session.active_mut().unwrap().interaction = None;
            assert!(matches!(app.embed_poll(0.1).as_slice(), [EmbedEvent::FrameMoved { .. }]));
        }

        #[test]
        fn the_backstop_undoes_anything_else() {
            let (mut app, _) = frame_doc();
            let count = |app: &DesignApp| app.session.active().unwrap().doc.spreads.iter().map(|s| s.items.len()).sum::<usize>();
            let before = count(&app);
            app.session.execute_trusted("frame.create", &json!({"rect": [40, 40, 200, 200], "content": "text"})).unwrap();
            assert_eq!(count(&app), before + 1);
            assert_eq!(app.embed_poll(0.0), vec![EmbedEvent::Rejected("change")]);
            assert_eq!(count(&app), before);
        }

        #[test]
        fn reflow_is_allowed_both_ways() {
            let (mut app, story) = text_doc();
            let spreads = |app: &DesignApp| app.session.active().unwrap().doc.spreads.len();
            let n = spreads(&app);
            app.run("text.select", json!({"story": story, "anchor": 0, "focus": 0})).unwrap();
            app.run("text.insert", json!({"text": "말이 이어지고 지면이 빠르게 찬다. ".repeat(300)})).unwrap();
            let caret = app.session.active().unwrap().selection.text;
            let grown = spreads(&app);
            assert!(grown > n);
            let events = app.embed_poll(0.0);
            assert!(events.contains(&EmbedEvent::Spreads(grown)), "{events:?}");
            assert!(!events.iter().any(|e| matches!(e, EmbedEvent::Rejected(_))), "{events:?}");
            assert_eq!(app.session.active().unwrap().selection.text, caret, "the caret stays");
            let doc = app.session.active().unwrap().doc.clone();
            let frames = &doc.story(designcraft_doc::StoryId(story)).unwrap().frames;
            assert!(frames.len() > 1 && frames.iter().all(|f| locked(&app, f.0)), "the new frames are locked");
            // Typing on keeps the frames (no Rejected, and nothing to lock again).
            app.run("text.insert", json!({"text": "더"})).unwrap();
            assert!(!app.embed_poll(0.1).iter().any(|e| matches!(e, EmbedEvent::Rejected(_))));
            // Delete the text again: the pages go.
            let end = paragraphs(&app, story)[0].len();
            app.run("text.select", json!({"story": story, "anchor": 0, "focus": end})).unwrap();
            app.run("text.delete", json!({})).unwrap();
            assert_eq!(spreads(&app), n);
            let events = app.embed_poll(0.2);
            assert!(events.contains(&EmbedEvent::Spreads(n)), "{events:?}");
            assert!(!events.iter().any(|e| matches!(e, EmbedEvent::Rejected(_))), "{events:?}");
        }

        #[test]
        fn a_drop_on_the_next_page_snaps_back() {
            let (mut app, id) = frame_doc();
            let (page_x, width) = {
                let p = &app.session.active().unwrap().doc.spreads[1].pages[0];
                (p.x, p.width)
            };
            app.session.execute("transform.move", &json!({"ids": [id], "dx": width, "dy": 0})).unwrap();
            assert_eq!(app.session.active().unwrap().doc.page_of_item(designcraft_doc::ItemId(id)), Some(2));
            let events = app.embed_poll(0.0);
            let [EmbedEvent::FrameMoved { placement, .. }] = events.as_slice() else { panic!("{events:?}") };
            let area = grid_of(&app).area(1).unwrap();
            assert!(placement.rect[0] >= area[0] && placement.rect[2] <= area[2] + 0.01, "{placement:?}");
            assert_eq!(app.session.active().unwrap().doc.page_of_item(designcraft_doc::ItemId(id)), Some(1));
            let b = bounds(&app, id);
            assert!(close([b.x0 - page_x, b.y0, b.x1 - page_x, b.y1], placement.rect));
        }

        #[test]
        fn a_table_cell_edit_names_its_table_and_cell() {
            let (mut app, story) = text_doc();
            let end = "첫 문단".len();
            app.run("text.select", json!({"story": story, "anchor": end, "focus": end})).unwrap();
            app.session.execute_trusted("table.insert", &json!({"rows": 2, "cols": 2})).unwrap();
            // Accept the table as the document's (as the host's would be).
            let doc = app.session.active().unwrap().doc.clone();
            if let Some(e) = app.embed.as_mut() {
                e.last_doc = Some(doc.clone());
            }
            let s = doc.story(designcraft_doc::StoryId(story)).unwrap();
            let (&tid, _) = s.tables.iter().next().unwrap();
            let cell = designcraft_doc::CellAddr { table: tid, row: 1, col: 0 };
            let sel = designcraft_doc::TextSel { story: designcraft_doc::StoryId(story), anchor: 0, focus: 0, frame: None, cell: Some(cell) };
            app.session.active_mut().unwrap().selection.text = Some(sel);
            app.run("text.insert", json!({"text": "칸"})).unwrap();
            assert_eq!(app.embed_poll(0.0), vec![]);
            assert_eq!(
                app.embed_poll(1.5),
                vec![EmbedEvent::TextEdited { story: "main".into(), paragraph: 0, text: "칸".into(), cell: Some((0, 1, 0)) }]
            );
        }

        #[test]
        fn chrome_is_off() {
            let (app, ..) = embedded();
            assert!(!app.ui.rulers && !app.ui.frame_edges && !app.ui.guides && !app.ui.baseline_grid && !app.ui.task_bar);
            assert!(app.embed.is_some());
        }

        #[test]
        fn only_the_type_and_selection_tools() {
            let (mut app, ..) = embedded();
            assert_eq!(app.session.tool_id(), "selection");
            assert!(refused(app.run("tool.select", json!({"tool": "directSelection"}))));
            assert!(app.run("tool.select", json!({"tool": "type"})).is_ok());
            assert_eq!(app.session.tool_id(), "type");
            app.session.set_tool("pen");
            app.embed_enforce();
            assert_eq!(app.session.tool_id(), "selection");
        }

        #[test]
        fn formatting_is_refused_view_and_text_are_not() {
            let (mut app, _, story, _) = embedded();
            assert!(refused(app.run("type.bold", json!({}))));
            assert!(refused(app.run("view.rulers", json!({}))));
            assert!(!app.ui.rulers);
            assert!(refused(app.run("app.preferences", json!({}))));
            assert!(!refused(app.run("view.fitSpread", json!({}))));
            // Text typed into a locked frame's story, and undone.
            app.run("text.select", json!({"story": story, "anchor": 0, "focus": 0})).unwrap();
            app.run("text.insert", json!({"text": "새 "})).unwrap();
            let text =
                |app: &mut DesignApp| app.session.execute("story.get", &json!({"story": story})).unwrap()["text"].as_str().unwrap().to_string();
            assert_eq!(text(&mut app), "새 본문");
            app.run("edit.undo", json!({})).unwrap();
            assert_eq!(text(&mut app), "본문");
        }

        #[test]
        fn the_engine_refuses_formatting_from_every_caller() {
            let (mut app, body, ..) = embedded();
            assert!(app.session.execute("type.bold", &json!({})).is_err_and(|e| e.to_string().contains("not available here")));
            app.session.execute_trusted("object.unlockAll", &json!({})).unwrap();
            assert!(!locked(&app, body));
        }

        #[test]
        fn unlabelled_frames_are_locked_for_good() {
            let (mut app, body, _, image) = embedded();
            assert!(locked(&app, body));
            assert!(!locked(&app, image));
            let _ = app.run("edit.undo", json!({}));
            assert!(locked(&app, body), "the lock isn't undoable");
        }

        #[test]
        fn a_type_tool_drag_draws_no_frame() {
            let (mut app, ..) = embedded();
            app.run("tool.select", json!({"tool": "type"})).unwrap();
            let count = |app: &DesignApp| app.session.active().unwrap().doc.spreads.iter().map(|s| s.items.len()).sum::<usize>();
            let before = count(&app);
            let (x, y) = {
                let d = &app.session.active().unwrap().doc;
                let r = CanvasLayout::new(d, false).page_rect(d, 0).unwrap();
                (r.x0 + 340.0, r.y0 + 450.0)
            };
            let v = app.view_info();
            let _ = app.session.pointer(&PointerEvent::new(PointerKind::Down, x, y), v);
            let _ = app.session.pointer(&PointerEvent::new(PointerKind::Drag, x + 120.0, y + 90.0), v);
            let _ = app.session.pointer(&PointerEvent::new(PointerKind::Up, x + 120.0, y + 90.0), v);
            assert_eq!(count(&app), before);
            assert!(app.session.active().unwrap().interaction.is_none());
        }
    }
}
