# Embedding DesignCraft as a spread editor (`?embed`)

With `?embed` on its URL, the web app runs as an embedded spread editor: book_write's 지면. A
host page loads one document into it, with the book's fonts. The person reading it can correct
text and move or resize labelled frames, which snap to the grid. The editor reports each edit to
the host, and the host saves it.

The editor reports what changed. It never interprets an edit; that is the host's job.

Code: `crates/ui-egui/src/embed.rs` (limits, snapping, change tracking),
`DesignApp::embed_begin` / `embed_poll` in `crates/ui-egui/src/lib.rs`,
`apps/designcraft-web/src/embed_protocol.rs` (the messages) and `embed_bridge.rs` (postMessage,
fetching). To try it by hand: `packaging/web/embed-harness.html`.

## Building and serving

- Build the bundle as usual: `trunk build --release` in `apps/designcraft-web` (or
  `packaging/web/package.sh`). It lands in `dist/web/`.
- Serve it from **the host's own origin**, e.g. `/designcraft-web/index.html?embed`. The editor
  accepts messages only from its own origin and posts only to it, so a cross-origin host gets
  nothing.
- Build it at the same revision as the `designcraft-cli` that lays the pages out
  (`DESIGNCRAFT_REV`), so the browser and the server engines are the same build.
- The `.wasm` is about 49 MB raw. Precompress it (`brotli -k *.wasm *.js`) and serve
  `Content-Encoding: br`, with `Content-Type: application/wasm`.
- Cache the hashed `.wasm` and `.js` forever (`Cache-Control: public, max-age=31536000,
  immutable`) under a path keyed by the revision, and send `index.html` with `no-cache`.
- The rest of `packaging/web/README.md` (HOSTING.md) applies: MIME types, HTTPS for WebGPU,
  `?webgl` and `?cpu`.

## Messages

Both directions use `postMessage` with a **JSON string** (`JSON.stringify` the object before
sending, `JSON.parse` what arrives), not a structured object. A message the editor can't read
(not a string, not JSON, an unknown `type`, a missing required field) is logged and ignored.

### Host → editor

```js
{ type: "load",
  documentUrl: "/…/spread.designcraft",                // required
  fontUrls: ["/…/NotoSerifKR-Regular.ttf", …],         // default []
  spread: 0,                                           // 0-based spread to open on; default 0
  grid: { columns: 6, rows: 12, lineHeight: 22.0,      // the source map's grid
          textAreas: [[x0, y0, x1, y1], …],            // one per page (0-based), page coordinates, points
          firstPageTop: 180.0 },                       // optional: on page 1, "top" is below the heading
  frames: { "lead:6": { v: "line" }, "table:7": { v: "top" }, "image:4": {} } }
                                                       // each movable frame's v as the source has it
{ type: "showSpread", spread: 3 }
{ type: "setTool", tool: "type" }                      // "type" | "selection"
```

- `load` closes any open document, fetches the fonts and then the document, and opens it. A
  `load` sent while another is still fetching replaces it.
- `grid` is optional, but without it a dropped frame isn't snapped and no `frameMoved` is sent.
  All four of `columns`, `rows`, `lineHeight` and `textAreas` must be there, or the grid is
  dropped.
- `frames[label].v == "line"` lets a lead box or table snap back to the line it sat at.
- `setTool` with any other tool is refused, silently.

### Editor → host

```js
{ type: "ready" }                                      // once, when the app has started
{ type: "loaded", spreadCount: 12 }                    // after each load
{ type: "spread", spread: 3 }                          // the spread in view changed
{ type: "textEdited", story: "main", paragraph: 4, text: "고친 문단" }
{ type: "textEdited", story: "table:7", paragraph: 0, text: "…", cell: { table: 0, row: 1, col: 2 } }
{ type: "frameMoved", label: "image:4", col: 1, width: 3, lines: 8, v: "top" }
{ type: "frameMoved", label: "table:7", col: 2, width: 4, v: "middle" }
{ type: "spreads", count: 13 }                         // text reflowed onto a new page, or back
{ type: "tool", tool: "type" }                         // the tool changed
{ type: "rejected", reason: "paragraphs" }             // an edit the editor undid
{ type: "error", message: "…" }
```

- **`loaded`** comes after the document opened; the editor then shows `spread` (clamped to the
  last one). A `spread` message follows when that isn't the spread first in view.
- **`textEdited`**: `story` is `"main"` or `"heading"` (the frames designcraft_rb labels
  `story:main` / `story:heading`), or a lead box's or table's frame label (`lead:6`,
  `table:7`). `paragraph` is the 0-based paragraph in that story, or in the cell. `cell.table`
  is the table's place among its story's tables, in text order. `text` is the paragraph's plain
  text as it is now; the host diffs it against what it rendered, and keeps its own inline marks.
  It is sent when the caret leaves the paragraph, or 1 s after the last change.
- **`frameMoved`** is sent on drop, already snapped: `col` is 1-based, `width` in columns,
  `lines` in body lines (a table has none: its height is its content's), and `v` one of
  `"line"`, `"top"`, `"middle"`, `"bottom"` (an image is never `"line"`). A frame dropped on
  another spread goes back to its own page first.
- **`tool`** comes for any change, including one the host asked for with `setTool`, the V and
  T keys, and a double-click into text.
- **`rejected`** reasons: `"paragraphs"` (a paragraph was added or removed, e.g. Return or a
  delete across paragraphs), `"story"` (text changed in a story the host doesn't know),
  `"change"` (anything other than text and moving labelled frames).
- **`error`**: a font or the document couldn't be fetched (`<url>: HTTP 404`), or the document
  didn't open. A failed font fails the whole `load`.

### After a save

The editor doesn't save. When the host has saved an edit and laid the unit out again, it sends
`load` again with the new document and grid, and the **same `spread`**, so the reader stays where
they were.

## Fonts

The host serves the book's fonts and lists them in `fontUrls`. The editor fetches each one and
adds its faces to the font database in memory (`FontDb::global().add_font`). They stay for the
life of the page: a face whose family and style is already loaded is skipped (and logged), so
sending the same fonts with every `load` is cheap. If the document still uses a font the host
didn't send, the Find Font dialog is closed (there's no chrome to answer it in) and a warning is
logged; the text shows in a fallback face.

## What's locked, and why

The host's source is markdown; the editor's document is a rendering of it. Only edits the host
can write back to the source are allowed:

- **No chrome.** Menus, panels, document tabs, the task bar, rulers, guides, frame edges, the
  baseline grid, scrollbars and the context menu are off, from before the first `load`. Panel,
  window, app and help commands, and the view toggles that bring chrome back, are refused;
  zooming, fitting and going to a page or spread stay. book_write draws the two-button toolbar
  itself and sends `setTool`.
- **Two tools.** The type tool and the selection tool. Anything else (including the V/T keys
  reaching another tool) falls back to selection.
- **Text only.** The type tool types and deletes characters. Formatting, styles, swatches,
  fonts, tables, threading, cut, paste, save, export and the rest are refused (`DENIED` in
  `embed.rs`).
- **Unlabelled frames are locked.** Every item without a movable label (`image:<n>`,
  `lead:<n>`, `table:<n>`) is locked on load. Labelled ones can be moved and resized, not
  rotated, sheared, flipped or re-cropped.
- **Dropped files are ignored.** The editor shows only the host's document.

### How it's enforced

- **The engine's command filter.** `Session::command_filter` is set to
  `embed::command_allowed`, so a refused command fails ("not available here") for every caller,
  tool previews included. The embed's own lock, snap and undo run past it
  (`Session::execute_trusted`).
- **A backstop.** After every frame, `embed_poll` compares the document with the last one it
  saw. Anything other than text in a known story without a paragraph added or removed, a
  labelled frame moved, or reflow, is undone and reported as `rejected`.
- **Reflow is allowed.** Smart Text Reflow may add or remove pages and the main story's frames
  as text grows or shrinks; the new frames are locked, and `spreads` reports the new count.
- **One undo step.** The snap after a drop, and the lock on reflowed frames, fold into the
  user's own undo step, so Undo takes the edit and what the embed did about it back together.
  The undo itself is a change like any other and is reported (a `textEdited` or `frameMoved`).
