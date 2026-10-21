//! book_write ⇄ embedded editor messages (EMBED.md): parsing the host's, writing the editor's.

use designcraft_ui_egui::embed::{EmbedConfig, EmbedEvent, Grid};
use serde_json::{Value, json};

#[derive(Debug)]
pub enum HostMessage {
    Load { document_url: String, font_urls: Vec<String>, spread: usize, config: EmbedConfig },
    ShowSpread(usize),
    SetTool(String),
}

/// A host message, or None for anything that isn't one (unknown type, missing field).
pub fn parse(v: &Value) -> Option<HostMessage> {
    match v.get("type")?.as_str()? {
        "load" => Some(HostMessage::Load {
            document_url: v.get("documentUrl")?.as_str()?.to_string(),
            font_urls: v
                .get("fontUrls")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|u| u.as_str().map(String::from)).collect())
                .unwrap_or_default(),
            spread: v.get("spread").and_then(Value::as_u64).unwrap_or(0) as usize,
            config: EmbedConfig {
                grid: v.get("grid").and_then(Grid::from_json),
                frames: v
                    .get("frames")
                    .and_then(Value::as_object)
                    .map(|m| m.iter().map(|(k, f)| (k.clone(), f.get("v").and_then(Value::as_str).unwrap_or("").to_string())).collect())
                    .unwrap_or_default(),
            },
        }),
        "showSpread" => Some(HostMessage::ShowSpread(v.get("spread")?.as_u64()? as usize)),
        "setTool" => Some(HostMessage::SetTool(v.get("tool")?.as_str()?.to_string())),
        _ => None,
    }
}

/// Sent once, when the app has started.
pub fn ready() -> Value {
    json!({"type": "ready"})
}

/// An editor event as the host reads it.
pub fn event(e: &EmbedEvent) -> Value {
    match e {
        EmbedEvent::Loaded { spread_count } => json!({"type": "loaded", "spreadCount": spread_count}),
        EmbedEvent::Spread(n) => json!({"type": "spread", "spread": n}),
        EmbedEvent::TextEdited { story, paragraph, text, cell } => {
            let mut m = json!({"type": "textEdited", "story": story, "paragraph": paragraph, "text": text});
            if let Some((table, row, col)) = cell {
                m["cell"] = json!({"table": table, "row": row, "col": col});
            }
            m
        }
        EmbedEvent::FrameMoved { label, placement: p } => {
            let mut m = json!({"type": "frameMoved", "label": label, "col": p.col, "width": p.width, "v": p.v});
            if let Some(lines) = p.lines {
                m["lines"] = json!(lines);
            }
            m
        }
        EmbedEvent::Spreads(count) => json!({"type": "spreads", "count": count}),
        EmbedEvent::Tool(tool) => json!({"type": "tool", "tool": tool}),
        EmbedEvent::Rejected(reason) => json!({"type": "rejected", "reason": reason}),
        EmbedEvent::Error(message) => json!({"type": "error", "message": message}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use designcraft_ui_egui::embed::Placement;

    #[test]
    fn a_full_load_parses() {
        let m = parse(&json!({
            "type": "load",
            "documentUrl": "/b/1/spread.designcraft",
            "fontUrls": ["/f/a.ttf", "/f/b.ttf"],
            "spread": 2,
            "grid": {"columns": 6, "rows": 12, "lineHeight": 22.0, "textAreas": [[60, 100, 360, 580]], "firstPageTop": 180.0},
            "frames": {"lead:6": {"v": "line"}, "table:7": {"v": "top"}, "image:4": {}}
        }));
        let Some(HostMessage::Load { document_url, font_urls, spread, config }) = m else {
            panic!("not a load: {m:?}");
        };
        assert_eq!(document_url, "/b/1/spread.designcraft");
        assert_eq!(font_urls, vec!["/f/a.ttf".to_string(), "/f/b.ttf".to_string()]);
        assert_eq!(spread, 2);
        let grid = config.grid.unwrap_or_else(|| panic!("no grid"));
        assert_eq!((grid.columns, grid.rows, grid.line_height), (6, 12, 22.0));
        assert_eq!(grid.text_areas, vec![[60.0, 100.0, 360.0, 580.0]]);
        assert_eq!(grid.first_page_top, Some(180.0));
        assert_eq!(config.frames.get("lead:6").map(String::as_str), Some("line"));
        assert_eq!(config.frames.get("table:7").map(String::as_str), Some("top"));
        assert_eq!(config.frames.get("image:4").map(String::as_str), Some(""));
    }

    #[test]
    fn a_bare_load_defaults_fonts_spread_grid_and_frames() {
        let Some(HostMessage::Load { font_urls, spread, config, .. }) = parse(&json!({"type": "load", "documentUrl": "/d"})) else {
            panic!("not a load");
        };
        assert!(font_urls.is_empty() && spread == 0 && config.grid.is_none() && config.frames.is_empty());
    }

    #[test]
    fn a_load_without_a_document_url_is_none() {
        assert!(parse(&json!({"type": "load", "fontUrls": []})).is_none());
    }

    #[test]
    fn unknown_or_untyped_messages_are_none() {
        assert!(parse(&json!({"type": "explode"})).is_none());
        assert!(parse(&json!({"spread": 1})).is_none());
        assert!(parse(&json!("load")).is_none());
    }

    #[test]
    fn show_spread_and_set_tool() {
        assert!(matches!(parse(&json!({"type": "showSpread", "spread": 3})), Some(HostMessage::ShowSpread(3))));
        assert!(parse(&json!({"type": "showSpread"})).is_none());
        assert!(matches!(parse(&json!({"type": "setTool", "tool": "type"})), Some(HostMessage::SetTool(t)) if t == "type"));
        assert!(parse(&json!({"type": "setTool"})).is_none());
    }

    #[test]
    fn ready_message() {
        assert_eq!(ready(), json!({"type": "ready"}));
    }

    #[test]
    fn each_event_as_json() {
        assert_eq!(event(&EmbedEvent::Loaded { spread_count: 12 }), json!({"type": "loaded", "spreadCount": 12}));
        assert_eq!(event(&EmbedEvent::Spread(3)), json!({"type": "spread", "spread": 3}));
        assert_eq!(event(&EmbedEvent::Spreads(13)), json!({"type": "spreads", "count": 13}));
        assert_eq!(event(&EmbedEvent::Tool("type")), json!({"type": "tool", "tool": "type"}));
        assert_eq!(event(&EmbedEvent::Rejected("paragraphs")), json!({"type": "rejected", "reason": "paragraphs"}));
        assert_eq!(event(&EmbedEvent::Error("boom".into())), json!({"type": "error", "message": "boom"}));
    }

    #[test]
    fn text_edited_with_and_without_a_cell() {
        let main = EmbedEvent::TextEdited { story: "main".into(), paragraph: 4, text: "고친 문단".into(), cell: None };
        assert_eq!(event(&main), json!({"type": "textEdited", "story": "main", "paragraph": 4, "text": "고친 문단"}));
        let cell = EmbedEvent::TextEdited { story: "table:7".into(), paragraph: 0, text: "x".into(), cell: Some((0, 1, 2)) };
        assert_eq!(
            event(&cell),
            json!({"type": "textEdited", "story": "table:7", "paragraph": 0, "text": "x", "cell": {"table": 0, "row": 1, "col": 2}})
        );
    }

    #[test]
    fn frame_moved_has_lines_except_for_a_table() {
        let image =
            EmbedEvent::FrameMoved { label: "image:4".into(), placement: Placement { col: 1, width: 3, lines: Some(8), v: "top", rect: [0.0; 4] } };
        assert_eq!(event(&image), json!({"type": "frameMoved", "label": "image:4", "col": 1, "width": 3, "lines": 8, "v": "top"}));
        let table =
            EmbedEvent::FrameMoved { label: "table:7".into(), placement: Placement { col: 2, width: 4, lines: None, v: "middle", rect: [0.0; 4] } };
        let j = event(&table);
        assert_eq!(j, json!({"type": "frameMoved", "label": "table:7", "col": 2, "width": 4, "v": "middle"}));
        assert!(j.get("lines").is_none());
    }
}
