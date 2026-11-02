//! Embed mode (book_write's 교정/교열 and 디자인수정): the app as an editor inside a host page.

use std::sync::Arc;

use super::{DesignApp, Value, canvas, json};
use crate::embed;
use crate::state::UiState;

impl DesignApp {
    /// Fonts were added to the font database (an embed's host sent the book's): the interface
    /// takes the scripts they cover (Hangul tab names, font names) from the next frame.
    pub fn ui_fonts_added(&mut self) {
        self.fonts_added = true;
    }

    /// Turns this app into an embedded editor for the open document: chrome off, the type and
    /// selection tools only, every frame locked but the labelled movable ones (image:/lead:/table:).
    pub fn embed_begin(&mut self, config: embed::EmbedConfig) {
        if config.mode == embed::EmbedMode::Studio {
            self.studio_begin(config);
            return;
        }
        self.hide_chrome();
        self.embed = Some(embed::Embed { config, ..Default::default() });
        self.session.command_filter = Some(embed::command_allowed);
        self.embed_lock();
        self.select_tool("selection");
        // The lock must not be undoable: the document was just opened, so its history holds nothing else.
        // Nor the rulers' settings, which must also not count as a change.
        if let Some(st) = self.session.active_mut() {
            st.history.undo.clear();
            st.history.redo.clear();
            embed_ruler_settings(st);
            st.saved_doc = st.doc.clone();
            st.saved_revision = st.revision;
        }
        let doc = self.session.active().map(|d| d.doc.clone());
        let spread = match canvas::current_spread(self) {
            Some(designcraft_doc::SpreadRef::Doc(n)) => Some(n),
            _ => None,
        };
        let tool = self.session.tool_id();
        if let Some(e) = self.embed.as_mut() {
            e.last_doc = doc;
            e.last_spread = spread;
            e.last_tool = Some(tool);
        }
    }

    /// Studio mode: the full interface over the documents the host opened (tabs, in
    /// `config.documents`' order), changing paragraph-style definitions only. Every document is
    /// prepared: every item locked, no history; its styles as loaded are each `styleChanged`'s
    /// `before`. The chrome a 교정/교열 load hid comes back — unless the host sent `chrome: false`:
    /// then it's hidden as 교정/교열 hides it, and no style edits.
    pub(crate) fn studio_begin(&mut self, config: embed::EmbedConfig) {
        if !config.chrome {
            self.hide_chrome();
        } else if self.embed.as_ref().is_some_and(|e| !e.studio() || !e.config.chrome) {
            let d = UiState::default();
            self.ui.rulers = d.rulers;
            self.ui.frame_edges = d.frame_edges;
            self.ui.guides = d.guides;
            self.ui.baseline_grid = d.baseline_grid;
            self.ui.task_bar = d.task_bar;
            self.ui.hidden_panels = d.hidden_panels;
        }
        let names = config.documents.clone();
        let chrome = config.chrome;
        let embed = embed::Embed { config, ..Default::default() };
        self.session.command_filter = Some(embed.command_filter());
        self.embed = Some(embed);
        let active = self.session.active_index();
        for i in 0..self.session.documents().len() {
            self.session.set_active(i);
            self.embed_lock();
            let Some(st) = self.session.active_mut() else { continue };
            // The lock must not be undoable, nor count as a change: the document was just opened.
            st.history.undo.clear();
            st.history.redo.clear();
            // Before `last` is taken, or `studio_change` would see the settings as a change.
            if !chrome {
                embed_ruler_settings(st);
            }
            st.saved_doc = st.doc.clone();
            st.saved_revision = st.revision;
            let (name, editable) = names.get(i).map_or_else(|| (st.title(), false), |c| (c.name.clone(), c.editable));
            let doc = embed::StudioDoc { name, editable, loaded: embed::paragraph_styles(&st.doc), last: st.doc.clone() };
            let uid = st.uid;
            if let Some(e) = self.embed.as_mut() {
                e.studio.insert(uid, doc);
            }
        }
        if let Some(i) = active {
            self.session.set_active(i);
        }
        self.select_tool("selection");
        let spread = match canvas::current_spread(self) {
            Some(designcraft_doc::SpreadRef::Doc(n)) => Some(n),
            _ => None,
        };
        let tool = self.session.tool_id();
        if let Some(e) = self.embed.as_mut() {
            e.last_spread = spread;
            e.last_tool = Some(tool);
        }
    }

    /// An embed's hidden chrome: no frame edges, guides, baseline grid, task bar or panels. The
    /// rulers are the exception: always shown, display-only (`canvas` takes no gesture on them
    /// when chromeless), measuring each page in millimetres ([`embed_ruler_settings`]).
    pub(crate) fn hide_chrome(&mut self) {
        self.ui.rulers = true;
        self.ui.frame_edges = false;
        self.ui.guides = false;
        self.ui.baseline_grid = false;
        self.ui.task_bar = false;
        self.ui.hidden_panels = 1;
    }

    /// Studio mode without chrome: the paragraph style of the text under canvas point `p` in the
    /// active document, whatever the locks (studio locks every item, so the selection tool finds
    /// nothing): body text, a table cell, a running head or folio on the parent page. None where
    /// there's no text. Changes nothing.
    pub fn embed_paragraph_at(&self, p: designcraft_geom::Point) -> Option<String> {
        let zoom = self.view().map_or(1.0, |v| v.zoom).max(1e-6);
        self.session.paragraph_at(p, 4.0 / zoom).map(|h| h.style).filter(|s| !s.is_empty())
    }

    /// A primary click on the canvas of a studio without chrome: reports the paragraph style under
    /// `p` (a canvas point) as `paragraphSelected`, with the next poll.
    pub fn embed_pick_paragraph(&mut self, p: designcraft_geom::Point) {
        if !self.embed.as_ref().is_some_and(embed::Embed::studio_bare) {
            return;
        }
        let name = self.embed_paragraph_at(p);
        let Some(uid) = self.session.active().map(|st| st.uid) else { return };
        let title = self.session.active().map(|st| st.title()).unwrap_or_default();
        if let Some(e) = self.embed.as_mut() {
            let document = e.studio.get(&uid).map_or(title, |d| d.name.clone());
            e.outbox.push(embed::EmbedEvent::ParagraphSelected { document, name });
        }
    }

    /// A primary press on the canvas of a 교정/교열 embed: when it lands on a typeset formula (an
    /// anchored object labelled `math:…`, whatever the locks) it reports `formulaSelected` with
    /// the next poll, its box mapped by `to_css` (canvas → CSS pixels in the viewport), and
    /// returns true: the press is the host's, and the tool mustn't see it. Changes nothing. Each
    /// press reports once (a double-click, twice). Objects in table cells and footnotes aren't
    /// searched: a press there goes to the tool. A paragraph edit not reported yet goes first —
    /// the press places no caret, so the caret never leaves its paragraph to send it — so the
    /// host has the text as it is before it edits the formula.
    pub fn embed_pick_formula(&mut self, p: designcraft_geom::Point, to_css: impl Fn(designcraft_geom::Rect) -> designcraft_geom::Rect) -> bool {
        if !self.embed.as_ref().is_some_and(embed::Embed::proofread) {
            return false;
        }
        let Some(hit) = self.session.anchored_object_at(p).filter(|h| h.label.starts_with("math:")) else { return false };
        let r = to_css(hit.rect);
        let pending = self.embed.as_mut().and_then(|e| e.pending_text.take());
        let edited = pending.and_then(|p| self.embed_text_event(&p.change));
        let Some(e) = self.embed.as_mut() else { return false };
        e.outbox.extend(edited);
        e.outbox.push(embed::EmbedEvent::FormulaSelected { label: hit.label, rect: [r.x0, r.y0, r.width(), r.height()] });
        true
    }

    /// Studio mode's `load`: closes every open document, opens each of `documents` (name, file
    /// bytes) as a tab in order, makes the first active, and begins studio over them with
    /// `config`. Sent again — after a save, or to take changes back — it starts over the same
    /// way. Returns each document's spread count, in order. The host's closes and opens run with
    /// the embed set aside, since studio refuses them from `run`.
    pub fn studio_load(&mut self, documents: &[(String, Vec<u8>)], config: embed::EmbedConfig) -> Result<Vec<usize>, String> {
        if documents.is_empty() {
            return Err("no documents".into());
        }
        let embed = self.embed.take();
        // Opening runs the queries studio allows (font.list for Find Font), even when the app
        // began as 교정/교열, whose filter refuses them.
        self.session.command_filter = Some(embed::studio_command_allowed);
        let opened = self.studio_open(documents, &config.documents);
        self.embed = embed;
        let counts = opened?;
        self.session.set_active(0);
        self.embed_begin(config);
        Ok(counts)
    }

    /// Opens `documents` as tabs, each named as the host names it in `names` (본문, 표지), else
    /// by the name it was fetched under.
    pub(crate) fn studio_open(&mut self, documents: &[(String, Vec<u8>)], names: &[embed::DocConfig]) -> Result<Vec<usize>, String> {
        while !self.session.documents().is_empty() {
            self.run("file.close", json!({"index": 0}))?;
        }
        let mut counts = Vec::with_capacity(documents.len());
        for (i, (fetched, bytes)) in documents.iter().enumerate() {
            let name = names.get(i).map(|d| d.name.as_str()).filter(|n| !n.is_empty()).unwrap_or(fetched);
            self.run("file.openBytes", json!({"name": name, "base64": designcraft_engine::cmd::base64_encode(bytes)}))
                .map_err(|e| format!("{name}: {e}"))?;
            // The host brings the book's fonts; a missing one shows in a fallback face.
            if self.ui.dialog.as_ref().is_some_and(|d| d.id == "findFont") {
                log::warn!("embed: {name} uses fonts the host didn't send");
                self.ui.dialog = None;
            }
            counts.push(self.session.active().map(|st| st.doc.spreads.len()).ok_or_else(|| format!("{name} didn't open"))?);
        }
        Ok(counts)
    }

    /// Whether the active document is a studio document the host made editable.
    pub(crate) fn studio_active_editable(&self) -> bool {
        let Some(st) = self.session.active() else { return false };
        self.embed.as_ref().and_then(|e| e.studio.get(&st.uid)).is_some_and(|d| d.editable)
    }

    /// Studio mode's change tracker: in the active document (against its own last look, so a tab
    /// switch compares nothing across documents), a paragraph style whose definition changed is
    /// reported against its loaded definition; anything else — or any change in a document that
    /// isn't editable — is undone.
    pub(crate) fn studio_poll(&mut self) -> Vec<embed::EmbedEvent> {
        use embed::EmbedEvent as E;
        let mut events = self.embed.as_mut().map(|e| std::mem::take(&mut e.outbox)).unwrap_or_default();
        let Some(st) = self.session.active() else { return events };
        if st.interaction.is_some() {
            return events;
        }
        let (uid, cur) = (st.uid, st.doc.clone());
        let Some(sd) = self.embed.as_ref().and_then(|e| e.studio.get(&uid)).cloned() else {
            // Opened after the host's load: not one of its documents, so nothing in it may change.
            let doc = embed::StudioDoc { name: st.title(), editable: false, loaded: embed::paragraph_styles(&cur), last: cur };
            if let Some(e) = self.embed.as_mut() {
                e.studio.insert(uid, doc);
            }
            return events;
        };
        let last = sd.last.clone();
        if !Arc::ptr_eq(&cur, &last) {
            let restyled = last.styles.paragraph != cur.styles.paragraph;
            match embed::studio_change(&last, &cur) {
                Ok(added) if sd.editable || !restyled => {
                    self.embed_lock_ids(&added);
                    let (was, now) = (embed::paragraph_styles(&last), embed::paragraph_styles(&cur));
                    let names: std::collections::BTreeSet<&String> = was.keys().chain(now.keys()).collect();
                    for name in names.into_iter().filter(|n| was.get(*n) != now.get(*n)) {
                        events.push(E::StyleChanged {
                            document: sd.name.clone(),
                            name: name.clone(),
                            before: sd.loaded.get(name).cloned().unwrap_or(Value::Null),
                            after: now.get(name).cloned().unwrap_or(Value::Null),
                        });
                    }
                }
                _ => {
                    self.embed_revert(&last);
                    events.push(E::Rejected("change"));
                }
            }
        }
        let doc = self.session.active().map(|st| st.doc.clone());
        if let Some(d) = &doc
            && d.spreads.len() != last.spreads.len()
        {
            events.push(E::Spreads(d.spreads.len()));
        }
        self.embed_view_events(&mut events);
        if let (Some(e), Some(doc)) = (self.embed.as_mut(), doc)
            && let Some(sd) = e.studio.get_mut(&uid)
        {
            sd.last = doc;
        }
        events
    }

    /// The spread and tool events: what the user is looking at and holding, when it changed.
    pub(crate) fn embed_view_events(&mut self, events: &mut Vec<embed::EmbedEvent>) {
        let spread = match canvas::current_spread(self) {
            Some(designcraft_doc::SpreadRef::Doc(n)) => Some(n),
            _ => None,
        };
        let tool = self.session.tool_id();
        if let Some(e) = self.embed.as_mut() {
            if spread.is_some() && spread != e.last_spread {
                e.last_spread = spread;
                events.extend(spread.map(embed::EmbedEvent::Spread));
            }
            if e.last_tool != Some(tool) {
                e.last_tool = Some(tool);
                events.push(embed::EmbedEvent::Tool(tool));
            }
        }
    }

    /// Locks every item without a movable label (labelled ones stay unlocked; studio: every item), keeping the
    /// selection and the caret (`object.lock` deselects).
    pub(crate) fn embed_lock(&mut self) {
        let Some(st) = self.session.active() else { return };
        let all = self.embed.as_ref().is_some_and(embed::Embed::studio);
        let mut lock = Vec::new();
        for spread in &st.doc.spreads {
            for item in &spread.items {
                item.walk(&mut |i| {
                    if all || embed::Kind::of(&i.label).is_none() {
                        lock.push(i.id);
                    }
                });
            }
        }
        self.embed_lock_ids(&lock);
    }

    /// Locks `ids` past the command filter, keeping the selection and the caret, and leaves no
    /// undo step for it (an undone lock would only be locked again).
    pub(crate) fn embed_lock_ids(&mut self, ids: &[designcraft_doc::ItemId]) {
        if ids.is_empty() {
            return;
        }
        let Some(st) = self.session.active() else { return };
        let selection = st.selection.clone();
        let ids: Vec<u64> = ids.iter().map(|i| i.0).collect();
        self.embed_trusted("object.lock", json!({ "ids": ids }));
        if let Some(st) = self.session.active_mut() {
            st.selection = selection;
        }
    }

    /// Runs an embed's own edit past the command filter and folds it into the step before it, so
    /// one undo takes back the user's edit and what the embed did about it together.
    pub(crate) fn embed_trusted(&mut self, id: &str, params: Value) {
        let before = self.session.active().map(|st| st.doc.clone());
        if let Err(e) = self.session.execute_trusted(id, &params) {
            log::warn!("embed: {id} failed: {e}");
        }
        if let (Some(st), Some(before)) = (self.session.active_mut(), before)
            && st.history.undo.last().is_some_and(|e| Arc::ptr_eq(&e.doc, &before))
        {
            st.history.undo.pop();
        }
    }

    /// Takes the document back to `last` (what the embed last saw): undoing the steps since, or,
    /// when the history doesn't lead there, putting it back directly. Leaves nothing to redo.
    pub(crate) fn embed_revert(&mut self, last: &Arc<designcraft_doc::Document>) {
        let Some(st) = self.session.active_mut() else { return };
        let Some(k) = st.history.undo.iter().rposition(|e| Arc::ptr_eq(&e.doc, last)) else {
            st.doc = last.clone();
            st.bump_revision();
            return;
        };
        let steps = st.history.undo.len() - k;
        let redo = st.history.redo.len();
        for _ in 0..steps {
            if let Err(e) = self.session.execute_trusted("edit.undo", &json!({})) {
                log::warn!("embed: undo failed: {e}");
                break;
            }
        }
        if let Some(st) = self.session.active_mut() {
            st.history.redo.truncate(redo);
            if !Arc::ptr_eq(&st.doc, last) {
                st.doc = last.clone();
                st.bump_revision();
            }
        }
    }

    /// The change tracker, run once a frame after `ui()`: what the user did since the last look,
    /// as events for the host. Text corrections are reported per paragraph when the caret leaves
    /// it or after a second's pause (`now`, seconds); a dropped labelled frame snaps to the grid on
    /// its own page; any other change is undone (Smart Text Reflow's pages excepted).
    pub fn embed_poll(&mut self, now: f64) -> Vec<embed::EmbedEvent> {
        use embed::EmbedEvent as E;
        if self.embed.as_ref().is_some_and(embed::Embed::studio) {
            return self.studio_poll();
        }
        // Events raised outside the poll (a formula picked by a click) go first.
        let mut events = self.embed.as_mut().map(|e| std::mem::take(&mut e.outbox)).unwrap_or_default();
        let Some(last) = self.embed.as_ref().map(|e| e.last_doc.clone()) else { return events };
        let Some(st) = self.session.active() else { return events };
        if st.interaction.is_some() {
            return events;
        }
        let cur = st.doc.clone();
        let last = last.unwrap_or_else(|| cur.clone());
        if !Arc::ptr_eq(&cur, &last) {
            let rejected = match (embed::text_changes(&last, &cur), embed::structural_change(&last, &cur)) {
                (Err(reason), _) => Err(reason),
                (_, Err(())) => Err("change"),
                (Ok(text), Ok(added)) => Ok((text, added)),
            };
            match rejected {
                Err(reason) => {
                    self.embed_revert(&last);
                    events.push(E::Rejected(reason));
                }
                Ok((text, added)) => {
                    for change in text {
                        let pending = self.embed.as_mut().and_then(|e| e.pending_text.take());
                        if let Some(p) = pending.filter(|p| p.change != change) {
                            events.extend(self.embed_text_event(&p.change));
                        }
                        if let Some(e) = self.embed.as_mut() {
                            e.pending_text = Some(embed::PendingText { change, at: now });
                        }
                    }
                    self.embed_lock_ids(&added);
                    for m in embed::frame_moves(&last, &cur) {
                        events.extend(self.embed_snap(&m));
                    }
                }
            }
        }
        // The pending edit goes when the caret has left its paragraph, or after a second.
        let pending = self.embed.as_ref().and_then(|e| e.pending_text.clone());
        if let Some(p) = pending {
            let here = self.session.active().and_then(|st| {
                let t = st.selection.text?;
                let story = st.doc.text_story(t.story, t.cell)?;
                Some(t.story == p.change.story && t.cell == p.change.cell && story.para_at(t.focus) == p.change.para)
            });
            if here != Some(true) || now - p.at >= 1.0 {
                events.extend(self.embed_text_event(&p.change));
                if let Some(e) = self.embed.as_mut() {
                    e.pending_text = None;
                }
            }
        }
        let doc = self.session.active().map(|st| st.doc.clone());
        if let Some(d) = &doc
            && d.spreads.len() != last.spreads.len()
        {
            events.push(E::Spreads(d.spreads.len()));
        }
        let spread = match canvas::current_spread(self) {
            Some(designcraft_doc::SpreadRef::Doc(n)) => Some(n),
            _ => None,
        };
        let tool = self.session.tool_id();
        if let Some(e) = self.embed.as_mut() {
            if spread.is_some() && spread != e.last_spread {
                e.last_spread = spread;
                events.extend(spread.map(E::Spread));
            }
            if e.last_tool != Some(tool) {
                e.last_tool = Some(tool);
                events.push(E::Tool(tool));
            }
            e.last_doc = doc;
        }
        events
    }

    /// The `TextEdited` for a changed paragraph, with its text as it is now.
    pub(crate) fn embed_text_event(&self, c: &embed::TextChange) -> Option<embed::EmbedEvent> {
        let doc = &self.session.active()?.doc;
        let story = embed::story_names(doc).remove(&c.story)?;
        let text = embed::paragraph_texts(doc.text_story(c.story, c.cell)?).get(c.para)?.clone();
        let cell = match c.cell {
            Some(a) => Some((embed::table_index(doc.story(c.story)?, a.table)?, a.row, a.col)),
            None => None,
        };
        Some(embed::EmbedEvent::TextEdited { story, paragraph: c.para, text, cell })
    }

    /// Snaps a dropped labelled frame to the grid of the page it was on (back onto it, if it was
    /// dropped on another) and says where it went.
    pub(crate) fn embed_snap(&mut self, m: &embed::FrameMove) -> Option<embed::EmbedEvent> {
        let e = self.embed.as_ref()?;
        let grid = e.config.grid.as_ref()?;
        let line_y = (e.config.frames.get(&m.label).map(String::as_str) == Some("line")).then_some(m.was.y0 as f32);
        let px = m.page_x;
        let rect = [(m.now.x0 - px) as f32, m.now.y0 as f32, (m.now.x1 - px) as f32, m.now.y1 as f32];
        let placement = embed::snap(m.kind, rect, grid.area(m.page)?, grid.columns, grid.line_height, line_y);
        // Dropped on another spread: back onto its own first.
        if self.session.active()?.doc.find(m.id).map(|l| l.spread) != Some(m.spread) {
            let to = serde_json::to_value(m.spread).ok()?;
            self.embed_trusted("transform.move", json!({"ids": [m.id.0], "dx": 0, "dy": 0, "toSpread": to}));
        }
        let from = self.session.active()?.doc.item(m.id)?.bounds();
        let r = placement.rect;
        let to = [r[0] as f64 + px, r[1] as f64, r[2] as f64 + px, r[3] as f64];
        self.embed_trusted("transform.resize", json!({"from": [from.x0, from.y0, from.x1, from.y1], "to": to, "ids": [m.id.0]}));
        Some(embed::EmbedEvent::FrameMoved { label: m.label.clone(), placement })
    }

    /// Puts the tool back to the selection tool when something switched it to one an embed doesn't allow.
    pub fn embed_enforce(&mut self) {
        let allowed = match &self.embed {
            Some(e) if e.studio() => embed::studio_tool_allowed,
            Some(_) => embed::tool_allowed,
            None => return,
        };
        if !allowed(self.session.tool_id()) {
            self.session.set_tool("selection");
        }
    }
}

/// A chromeless embed's rulers: millimetres both ways, each page measured from its own top-left
/// corner (no zero point moved off it). Written on the document directly, not as a command: it
/// leaves no undo entry, and the caller takes it as the saved state. Only the view reads these
/// settings; a chromeless embed never saves the document back.
fn embed_ruler_settings(st: &mut designcraft_engine::DocState) {
    use designcraft_geom::Unit::Millimeters;
    let s = &st.doc.settings;
    if s.horizontal_units == Millimeters
        && s.vertical_units == Millimeters
        && s.ruler_origin == designcraft_doc::RulerOrigin::Page
        && s.zero_point == [0.0, 0.0]
    {
        return;
    }
    let s = &mut Arc::make_mut(&mut st.doc).settings;
    s.horizontal_units = Millimeters;
    s.vertical_units = Millimeters;
    s.ruler_origin = designcraft_doc::RulerOrigin::Page;
    s.zero_point = [0.0, 0.0];
    st.bump_revision();
}
