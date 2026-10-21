//! `?embed`: the postMessage bridge between book_write's page and the embedded editor (EMBED.md).
//!
//! Host messages arrive as JSON strings on `window`'s `message` event (same origin only) and
//! queue up for `WebShell::logic`; editor events go to `window.parent()` as JSON strings.
//! A `load` fetches its documents (one, or studio's several), then its fonts into the shared
//! font database, into a slot the shell takes from on a later frame.

use crate::embed_protocol::{self, HostMessage};
use designcraft_ui_egui::embed::EmbedConfig;
use serde_json::Value;
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
use wasm_bindgen::JsCast as _;
use wasm_bindgen_futures::JsFuture;

/// Host messages not yet handled, oldest first.
pub type Queue = Rc<RefCell<VecDeque<HostMessage>>>;

/// The fetched documents (file name, bytes), in order, or why one couldn't be fetched.
type Fetched = Result<Vec<(String, Vec<u8>)>, String>;

pub async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or("no window")?;
    let response: web_sys::Response = JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|e| format!("{url}: {e:?}"))?
        .dyn_into()
        .map_err(|_| format!("{url}: not a Response"))?;
    if !response.ok() {
        return Err(format!("{url}: HTTP {}", response.status()));
    }
    let buffer = JsFuture::from(response.array_buffer().map_err(|e| format!("{url}: {e:?}"))?).await.map_err(|e| format!("{url}: {e:?}"))?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// Adds every font to the engine's shared font database; returns the faces added. A font that
/// can't be fetched, or adds none, is logged and skipped (`embed_protocol::add_font`): the rest
/// still load.
pub async fn load_fonts(urls: &[String]) -> usize {
    let mut faces = 0;
    for url in urls {
        faces += embed_protocol::add_font(url, fetch_bytes(url).await, |bytes| designcraft_fonts::FontDb::global().add_font(bytes));
    }
    faces
}

/// Sends `v` to the host page as a JSON string, to this page's own origin only.
pub fn post(v: &Value) {
    let Some(window) = web_sys::window() else { return };
    let Ok(origin) = window.location().origin() else { return };
    match window.parent() {
        Ok(Some(parent)) => {
            if let Err(e) = parent.post_message(&wasm_bindgen::JsValue::from_str(&v.to_string()), &origin) {
                log::error!("embed: couldn't post to the host: {e:?}");
            }
        }
        _ => log::warn!("embed: no parent window to post to"),
    }
}

/// Queues each same-origin host message (a JSON string) on `window` and wakes the app.
pub fn listen(queue: Queue, ctx: egui::Context) -> Result<(), wasm_bindgen::JsValue> {
    let window = web_sys::window().ok_or_else(|| wasm_bindgen::JsValue::from_str("no window"))?;
    let origin = window.location().origin()?;
    crate::web::add_dom_event_listener(&window, "message", move |event| {
        let Some(event) = event.dyn_ref::<web_sys::MessageEvent>() else { return };
        if event.origin() != origin {
            return;
        }
        let Some(text) = event.data().as_string() else { return };
        let Some(message) = serde_json::from_str::<Value>(&text).ok().as_ref().and_then(embed_protocol::parse) else {
            log::warn!("embed: ignored a message that isn't one of the host's");
            return;
        };
        queue.borrow_mut().push_back(message);
        ctx.request_repaint();
    })
}

/// An in-flight `load`: the documents, then the fonts, fetched into `slot`.
pub struct Loading {
    slot: Rc<RefCell<Option<Fetched>>>,
    pub spread: usize,
    pub config: EmbedConfig,
}

impl Loading {
    pub fn start(document_urls: Vec<String>, font_urls: Vec<String>, spread: usize, config: EmbedConfig, ctx: egui::Context) -> Self {
        let slot: Rc<RefCell<Option<Fetched>>> = Rc::default();
        let pending = slot.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let fetched = async {
                let mut documents = Vec::with_capacity(document_urls.len());
                for url in &document_urls {
                    documents.push((embed_protocol::file_stem(url), fetch_bytes(url).await?));
                }
                load_fonts(&font_urls).await;
                Ok(documents)
            }
            .await;
            *pending.borrow_mut() = Some(fetched);
            ctx.request_repaint();
        });
        Self { slot, spread, config }
    }

    /// The fetch's result, once it has finished.
    pub fn take(&self) -> Option<Fetched> {
        self.slot.borrow_mut().take()
    }
}
