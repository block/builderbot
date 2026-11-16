//! Loopback HTTP streaming of note video for the desktop webview.
//!
//! The `staged-media://` protocol ([`super::serving`]) cannot stream: wry
//! starts every request on the app main thread and hands the response back
//! through it as one buffer. Over that protocol WebKit asks for an MP4 one
//! sample at a time, each request waiting for the last, so a click on the
//! timeline is 40 to 270 requests and a drag is hundreds a second, all of
//! them queued behind whatever else the main thread is doing. A WebM is asked
//! for whole, with no `Range`, and so read whole into memory.
//!
//! Over HTTP WebKit does its own ranged networking: a few long-lived
//! requests per video, read off the socket as the decoder needs them. So the
//! desktop app serves video (and nothing else) from a listener of its own,
//! with tower-http's `ServeFile`, which streams the file in 64 KiB chunks and
//! honours `Range`. The listener runs on its own small runtime and finds
//! files through the [`MediaIndex`], so once a video has been opened, playing
//! and scrubbing it need neither the app main thread, nor the app's runtime,
//! nor the store.
//!
//! What may reach it:
//!
//! - It listens on 127.0.0.1 only, on a port the system picks.
//! - Every path starts with a 256-bit secret drawn at launch. It is handed to
//!   the app's own pages (see [`plugin`]) and to nothing else, and a `<video>`
//!   cannot send a header, so the path is where it has to go. A page in the
//!   user's browser can find the port but not the secret.
//! - The `Host` header has to be `127.0.0.1:<port>`. The secret is in a URL,
//!   and a URL can leave the app (`videoStream.ts` closes the ways the page
//!   offers). A page elsewhere that came by one could point a name of its own
//!   at 127.0.0.1 and read the video as its own origin; under that name the
//!   answer is the same 404 a wrong secret gets. Under the real address the
//!   video is another origin's, sent with no CORS header.
//! - The only thing behind the secret is `<id>.<ext>` for a stored video,
//!   resolved through the store exactly as the protocol does. No part of the
//!   request is used as a path.
//!
//! If the listener cannot be bound the page is told nothing and video stays
//! on the protocol. If it is bound and then cannot serve, it is closed, the
//! first `<video>` fails to load from it, and `videoStream.ts` moves that
//! video to the protocol and stops using the stream. A listener that stops
//! later is found out the same way, or by a video that already has part of
//! its clip when its load stalls: WebKit reports no error for that one.

use super::serving::{MediaIndex, StoreSource};
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use rand::RngExt;
use std::net::{Ipv4Addr, TcpListener};
use std::sync::Arc;
use subtle::ConstantTimeEq;

/// The page global [`plugin`] sets to [`MediaStream::base_url`]; read by
/// `resolveStagedVideoUrl` in `mediaUrl.ts`.
const PAGE_GLOBAL: &str = "__STAGED_MEDIA_STREAM__";

/// A bound listener and its secret, not yet serving. Binding is separate
/// from serving because the page has to be told the address when its window
/// is built, which is before the app is set up.
pub(crate) struct MediaStream {
    listener: TcpListener,
    port: u16,
    secret: String,
}

impl MediaStream {
    pub(crate) fn bind() -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let secret: [u8; 32] = rand::rng().random();
        Ok(Self {
            listener,
            port,
            secret: hex::encode(secret),
        })
    }

    /// What a video's file name is appended to: `http://127.0.0.1:<port>/<secret>`.
    pub(crate) fn base_url(&self) -> String {
        format!("http://{}/{}", self.host(), self.secret)
    }

    /// The only `Host` a request may name: the address the page was given.
    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// Start answering, on a thread and runtime of its own: the app's runtime
    /// has workers and a blocking pool that commands waiting on the store can
    /// fill, and a seek should not queue behind them.
    pub(crate) fn serve(
        self,
        index: Arc<MediaIndex>,
        store: Arc<StoreSource>,
    ) -> std::io::Result<()> {
        self.serve_on(index, store, |run| {
            std::thread::Builder::new()
                .name("staged-media-stream".into())
                .spawn(run)
                .map(drop)
        })
    }

    /// [`Self::serve`], with the thread left to `spawn`. Every failure drops
    /// the listener, so the address the page already holds refuses
    /// connections at once instead of leaving them to wait in its backlog.
    fn serve_on(
        self,
        index: Arc<MediaIndex>,
        store: Arc<StoreSource>,
        spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("staged-media-stream")
            .enable_all()
            .build()?;
        let router = router(self.host(), self.secret, index, store);
        let listener = self.listener;
        spawn(Box::new(move || {
            let served = runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener)?;
                axum::serve(listener, router).await
            });
            if let Err(e) = served {
                log::error!("[media stream] listener stopped: {e}");
            }
        }))
    }
}

/// `setup`'s half of the wiring: start answering on what [`MediaStream::bind`]
/// bound before the builder, or say why video is on the protocol instead.
/// Neither failure stops the app, and it is reported here because the logger
/// is not up when the listener is bound.
pub(crate) fn start(
    bound: std::io::Result<MediaStream>,
    index: Arc<MediaIndex>,
    store: Arc<StoreSource>,
) {
    match bound {
        // The page already has the address. The first video to load from it
        // fails, and the page then stops using it (`videoStream.ts`).
        Ok(stream) => {
            if let Err(e) = stream.serve(index, store) {
                log::warn!("[media stream] could not start, video uses staged-media: {e}");
            }
        }
        Err(e) => log::warn!("[media stream] unavailable, video uses staged-media: {e}"),
    }
}

/// Tells the app's pages where the stream is, before any of their script
/// runs: a main-frame initialization script that sets [`PAGE_GLOBAL`]. With
/// no stream it sets nothing, and the page keeps video on the protocol.
///
/// An initialization script runs in every main-frame document a window ever
/// loads, so it names the address only where `location.origin` is one of
/// `page_origins` ([`page_origins`]). Every window loads the app's own page
/// today; this keeps the secret from any other page one might navigate to.
pub(crate) fn plugin<R: tauri::Runtime>(
    stream: Option<&MediaStream>,
    page_origins: &[String],
) -> tauri::plugin::TauriPlugin<R> {
    let builder = tauri::plugin::Builder::new("staged-media-stream");
    match stream {
        Some(stream) => builder.js_init_script(init_script(&stream.base_url(), page_origins)),
        None => builder,
    }
    .build()
}

fn init_script(base_url: &str, page_origins: &[String]) -> String {
    format!(
        "if ({}.includes(window.location.origin)) window.{PAGE_GLOBAL} = {};",
        serde_json::json!(page_origins),
        serde_json::Value::from(base_url)
    )
}

/// The origins the app's own pages have, as `location.origin` reads in them.
/// Follows Tauri's choice of app URL (`AppManager::get_app_url`): the dev
/// server under `tauri dev`, where `--config` can move it to another port, and
/// otherwise the bundled frontend on Tauri's own protocol.
pub(crate) fn page_origins(config: &tauri::Config) -> Vec<String> {
    page_origins_for(tauri::is_dev(), config)
}

fn page_origins_for(dev: bool, config: &tauri::Config) -> Vec<String> {
    use tauri::utils::config::FrontendDist;
    let served_from = if dev {
        config.build.dev_url.as_ref()
    } else {
        match &config.build.frontend_dist {
            Some(FrontendDist::Url(url)) => Some(url),
            _ => None,
        }
    };
    match served_from {
        Some(url) => vec![url.origin().ascii_serialization()],
        // Windows and Android serve the protocol from an http(s) origin,
        // chosen per window.
        None if cfg!(any(windows, target_os = "android")) => vec![
            "http://tauri.localhost".to_owned(),
            "https://tauri.localhost".to_owned(),
        ],
        None => vec!["tauri://localhost".to_owned()],
    }
}

struct Served {
    host: String,
    secret: String,
    index: Arc<MediaIndex>,
    store: Arc<StoreSource>,
}

fn router(host: String, secret: String, index: Arc<MediaIndex>, store: Arc<StoreSource>) -> Router {
    Router::new()
        .route("/{secret}/{file}", get(serve_video))
        .with_state(Arc::new(Served {
            host,
            secret,
            index,
            store,
        }))
}

/// Whether the request names `host` and nothing else: exactly one `Host`
/// header with that value, and no other authority in the request target.
/// WebKit sends this for the address the page was given. A request made
/// under any other name for this machine does not, and neither does one with
/// no `Host` at all (HTTP/2, which no webview speaks to a plain-text origin).
fn addressed_to(host: &str, headers: &HeaderMap, uri: &Uri) -> bool {
    let mut named = headers.get_all(header::HOST).iter();
    named
        .next()
        .is_some_and(|value| value.as_bytes() == host.as_bytes())
        && named.next().is_none()
        && uri
            .authority()
            .map_or(true, |authority| authority.as_str() == host)
}

/// `GET` or `HEAD /<secret>/<id>.<ext>`, addressed to this listener. Anything
/// that is not the secret followed by a stored video, or that names another
/// host, is a 404, with nothing to tell the cases apart.
async fn serve_video(
    State(served): State<Arc<Served>>,
    Path((secret, file)): Path<(String, String)>,
    request: Request,
) -> Response {
    let authorized: bool = secret.as_bytes().ct_eq(served.secret.as_bytes()).into();
    let addressed = addressed_to(&served.host, request.headers(), request.uri());
    let is_video = super::refs::parse_media_file(&file)
        .and_then(|(_, ext)| super::files::mime_for_extension(ext))
        .is_some_and(|mime| mime.starts_with("video/"));
    if !authorized || !addressed || !is_video {
        return StatusCode::NOT_FOUND.into_response();
    }
    // The first request for a video asks the store, which can block; the
    // rest find it in the index, which costs one `stat`.
    let located =
        tokio::task::spawn_blocking(move || served.index.locate(&file, &*served.store)).await;
    let (path, mime) = match located {
        Ok(Ok(found)) => found,
        Ok(Err(status)) => return status.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let mut service = tower_http::services::ServeFile::new_with_mime(
        path,
        &mime.parse().expect("validated MIME"),
    );
    match service.try_call(request).await {
        Ok(response) => {
            let mut response = response.into_response();
            let headers = response.headers_mut();
            headers.insert(
                "X-Content-Type-Options",
                HeaderValue::from_static("nosniff"),
            );
            // The address and the secret are new at every launch, so a
            // stored copy could never be asked for again.
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests;
