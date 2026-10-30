//! Text wrap around anchored objects.
//!
//! Page items wrap text through [`crate::frame_specs`]. An anchored object's position is known
//! only once its story is composed, so its wrap is applied afterwards:
//!
//! - **Other stories.** A story whose frames share a spread with another story's wrapping
//!   anchored objects is composed with those objects as obstacles, where the other story's own
//!   composition places them ([`neighbours`], [`compose_among`]). That composition doesn't
//!   include obstacles from further stories, so two stories whose anchored objects wrap each
//!   other's text don't depend on each other in a cycle.
//! - **Its own story.** A custom or above-line anchored object wraps the lines of its own story
//!   that come after the line holding its anchor; that line and the lines before it are never
//!   wrapped by it (an inline object sits in its line and wraps none of its own story). The story
//!   is composed again with the objects where the previous pass placed them until they stop
//!   moving, at most [`MAX_PASSES`] more times.
//!
//!   A wrap can move its own anchor: text kept together that fits at a column's foot unwrapped
//!   but not wrapped goes to the next column, where, unwrapped, it would fit back. An object found
//!   back in a place it has been keeps the wrap it was last composed with as well, so its text is
//!   wrapped wherever it lands; the stale wrap covers only the room the text left.

use std::collections::BTreeMap;

use designcraft_doc::{AnchorPosition, AnchoredObject, Document, Item, OBJECT_MARK, PageSide, SpreadRef, Story, StoryId, WrapMode};
use designcraft_geom::Affine;

use crate::{ComposeOptions, ComposedStory, Exclusion, FrameSpec};

/// Extra compositions of a story with its own anchored objects' wrap: at least this many, and two
/// per wrapping object up to [`MAX_PASSES_CAP`]. A pass settles at least the first object that
/// moved, and a cascade (each wrap moving the next object's anchor) takes about one pass per
/// object; a layout that keeps moving its objects (keep options, vertical justification) stops there.
pub(crate) const MAX_PASSES: usize = 4;
pub(crate) const MAX_PASSES_CAP: usize = 64;

/// How far an obstacle may move between passes and still count as settled (points).
const SETTLED: f64 = 0.01;

/// A wrapping anchored object where its story's composition placed it.
#[derive(Clone, Debug)]
pub(crate) struct Obstacle<'a> {
    spread: SpreadRef,
    item: &'a Item,
    /// The object's parent space → spread space.
    xf: Affine,
    /// Story byte of the anchor, for obstacles in their own story.
    anchor: Option<usize>,
}

fn wraps(o: &AnchoredObject) -> bool {
    o.item.wrap.mode != WrapMode::None && !o.item.hidden
}

/// Does `story` have an anchored object whose wrap can push text?
pub fn has_wrapping_objects(story: &Story) -> bool {
    story.objects.iter().any(|o| wraps(o))
}

/// Stories other than `sid` with wrapping anchored objects and a frame on a spread with one of
/// `sid`'s frames (in story id order).
pub fn neighbours(doc: &Document, sid: StoryId) -> Vec<StoryId> {
    let mut candidates: Vec<StoryId> = doc.stories.iter().filter(|(id, st)| **id != sid && has_wrapping_objects(st)).map(|(id, _)| *id).collect();
    if candidates.is_empty() {
        return candidates;
    }
    let spreads = story_spreads(doc, sid);
    candidates.retain(|c| story_spreads(doc, *c).iter().any(|s| spreads.contains(s)));
    candidates.sort_by_key(|c| c.0);
    candidates
}

/// Stories other than `sid` that `sid`'s wrapping anchored objects can push aside: those with a
/// frame on a spread with one of `sid`'s frames.
pub fn wrapped_by(doc: &Document, sid: StoryId) -> Vec<StoryId> {
    if !doc.story(sid).is_some_and(has_wrapping_objects) {
        return Vec::new();
    }
    let spreads = story_spreads(doc, sid);
    let mut out: Vec<StoryId> =
        doc.stories.keys().filter(|id| **id != sid && story_spreads(doc, **id).iter().any(|s| spreads.contains(s))).copied().collect();
    out.sort_by_key(|c| c.0);
    out
}

fn story_spreads(doc: &Document, sid: StoryId) -> Vec<SpreadRef> {
    let mut out = Vec::new();
    for f in doc.story(sid).map(|s| s.frames.as_slice()).unwrap_or_default() {
        if let Some(loc) = doc.find(*f)
            && !out.contains(&loc.spread)
        {
            out.push(loc.spread);
        }
    }
    out
}

/// The wrapping anchored objects `cs` (a composition of `story`) places. `own`: obstacles for the
/// story itself (inline objects left out, anchors recorded).
pub(crate) fn obstacles<'a>(doc: &Document, story: &'a Story, cs: &ComposedStory, own: bool) -> Vec<Obstacle<'a>> {
    let mut out = Vec::new();
    if !has_wrapping_objects(story) {
        return out;
    }
    let anchors: Vec<usize> = story.text.match_indices(OBJECT_MARK).map(|(i, _)| i).collect();
    for ft in cs.frames.iter().filter(|ft| !ft.objects.is_empty()) {
        let Some(loc) = doc.find(ft.frame) else { continue };
        let Some(frame) = doc.item_at(&loc) else { continue };
        // Text that isn't shown pushes nothing aside.
        if frame.hidden || doc.layer(frame.layer).is_some_and(|l| !l.visible) {
            continue;
        }
        let fxf = doc.parent_xf(&loc) * doc.text_xf(frame);
        for o in &ft.objects {
            let Some(obj) = story.objects.get(o.index).filter(|obj| wraps(obj)) else { continue };
            if own && matches!(obj.position, AnchorPosition::Inline { .. }) {
                continue;
            }
            let xf = fxf * Affine::translate(o.origin.to_vec2());
            if !xf.transform_rect_bbox(obj.item.bounds()).is_finite() {
                continue;
            }
            let anchor = if own {
                let Some(&a) = anchors.get(o.index) else { continue };
                Some(a)
            } else {
                None
            };
            out.push(Obstacle { spread: loc.spread, item: &obj.item, xf, anchor });
        }
    }
    out
}

/// The exclusions `obs` add to each of `specs` (frames that ignore wrap, type on a path and
/// frames the obstacle doesn't reach get none). They are built as for page items
/// ([`crate::wrap_obstacles`]): sides, contours, custom boundaries, the wrap of a graphic inside.
pub(crate) fn exclusions(doc: &Document, specs: &[FrameSpec], obs: &[Obstacle]) -> Vec<Vec<Exclusion>> {
    let mut out = vec![Vec::new(); specs.len()];
    if obs.is_empty() {
        return out;
    }
    for (spec, ex) in specs.iter().zip(&mut out) {
        if spec.opts.ignore_wrap || spec.opts.path.is_some() {
            continue;
        }
        let Some(loc) = doc.find(spec.id) else { continue };
        let Some(item) = doc.item_at(&loc) else { continue };
        let inv = (doc.parent_xf(&loc) * doc.text_xf(item)).inverse();
        let sp = doc.spread(loc.spread);
        let left = |x: f64| sp.and_then(|sp| sp.page_at_x(x).and_then(|pi| sp.pages.get(pi))).is_some_and(|p| p.side == PageSide::Left);
        let a = spec.area;
        for ob in obs.iter().filter(|o| o.spread == loc.spread) {
            let mut found = Vec::new();
            crate::wrap_obstacles(ob.item, ob.xf, inv, spec.id, &left, 0, &mut found);
            for mut e in found {
                let r = e.rect;
                if r.is_finite() && r.x0 < a.x1 && r.x1 > a.x0 && r.y0 < a.y1 && r.y1 > a.y0 {
                    e.after = ob.anchor;
                    ex.push(e);
                }
            }
        }
    }
    out
}

fn with_exclusions(specs: &[FrameSpec], added: &[Vec<Exclusion>]) -> Vec<FrameSpec> {
    specs
        .iter()
        .zip(added)
        .map(|(s, ex)| {
            let mut s = s.clone();
            s.exclusions.extend(ex.iter().cloned());
            s
        })
        .collect()
}

/// Where an own-story object's wrap fell in one pass: each exclusion it adds, with the index of
/// the frame spec it is in.
type Place = Vec<(usize, Exclusion)>;

/// The exclusions `added` (per frame spec) by the object they belong to: its anchor's story byte.
fn places(added: Vec<Vec<Exclusion>>) -> BTreeMap<usize, Place> {
    let mut out: BTreeMap<usize, Place> = BTreeMap::new();
    for (fi, list) in added.into_iter().enumerate() {
        for e in list {
            if let Some(anchor) = e.after {
                out.entry(anchor).or_default().push((fi, e));
            }
        }
    }
    out
}

fn same_place(a: &Place, b: &Place) -> bool {
    let near = |x: &Exclusion, y: &Exclusion| {
        x.mode == y.mode
            && x.after == y.after
            && (x.rect.x0 - y.rect.x0).abs() <= SETTLED
            && (x.rect.y0 - y.rect.y0).abs() <= SETTLED
            && (x.rect.x1 - y.rect.x1).abs() <= SETTLED
            && (x.rect.y1 - y.rect.y1).abs() <= SETTLED
    };
    a.len() == b.len() && a.iter().zip(b).all(|((fa, x), (fb, y))| fa == fb && near(x, y))
}

/// Compose story `sid` with the wrapping anchored objects of `others` (compositions of its
/// [`neighbours`]) and of its own.
pub(crate) fn compose_among(doc: &Document, sid: StoryId, opts: &ComposeOptions, others: &[&ComposedStory]) -> ComposedStory {
    let Some(st) = doc.story(sid) else { return ComposedStory { story: sid, ..Default::default() } };
    let mut specs = crate::frame_specs(doc, sid);
    if !others.is_empty() {
        let mut obs = Vec::new();
        for cs in others {
            if let Some(other) = doc.story(cs.story) {
                obs.extend(obstacles(doc, other, cs, false));
            }
        }
        let added = exclusions(doc, &specs, &obs);
        if added.iter().any(|e| !e.is_empty()) {
            specs = with_exclusions(&specs, &added);
        }
    }
    let mut out = crate::compose(doc, st, &specs, opts);
    if !has_wrapping_objects(st) {
        return out;
    }
    // By object (its anchor): the places the last pass was composed with, and every place it has been.
    let mut applied: BTreeMap<usize, Vec<Place>> = BTreeMap::new();
    let mut seen: BTreeMap<usize, Vec<Place>> = BTreeMap::new();
    let wrapping = st.objects.iter().filter(|o| wraps(o)).count();
    for _ in 0..(2 * wrapping).clamp(MAX_PASSES, MAX_PASSES_CAP) {
        let found = places(exclusions(doc, &specs, &obstacles(doc, st, &out, true)));
        let composed_with = |k: &usize, p: &Place| applied.get(k).is_some_and(|a| a.iter().any(|q| same_place(p, q)));
        if found.len() == applied.len() && found.iter().all(|(k, p)| composed_with(k, p)) {
            break;
        }
        let mut next: BTreeMap<usize, Vec<Place>> = BTreeMap::new();
        for (k, p) in found {
            let back = seen.get(&k).is_some_and(|s| s.iter().any(|q| same_place(&p, q)));
            let mut kept = if back { applied.remove(&k).unwrap_or_default() } else { Vec::new() };
            if !kept.iter().any(|q| same_place(&p, q)) {
                kept.push(p.clone());
            }
            seen.entry(k).or_default().push(p);
            next.insert(k, kept);
        }
        let mut added = vec![Vec::new(); specs.len()];
        for (fi, e) in next.values().flatten().flatten() {
            added[*fi].push(e.clone());
        }
        out = crate::compose(doc, st, &with_exclusions(&specs, &added), opts);
        applied = next;
    }
    out
}
