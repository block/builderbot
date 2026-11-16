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
//!   user's browser can find the port but not the secret, DNS rebinding
//!   included.
//! - The only thing behind the secret is `<id>.<ext>` for a stored video,
//!   resolved through the store exactly as the protocol does. No part of the
//!   request is used as a path.
//!
//! If the listener cannot start the page is told nothing and video stays on
//! the protocol; `videoPreview.ts` also moves a single `<video>` back to the
//! protocol when its stream fails.

use super::serving::{MediaIndex, StoreSource};
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
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
        format!("http://127.0.0.1:{}/{}", self.port, self.secret)
    }

    /// Start answering, on a thread and runtime of its own: the app's runtime
    /// has workers and a blocking pool that commands waiting on the store can
    /// fill, and a seek should not queue behind them.
    pub(crate) fn serve(
        self,
        index: Arc<MediaIndex>,
        store: Arc<StoreSource>,
    ) -> std::io::Result<()> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("staged-media-stream")
            .enable_all()
            .build()?;
        let router = router(self.secret, index, store);
        let listener = self.listener;
        std::thread::Builder::new()
            .name("staged-media-stream".into())
            .spawn(move || {
                let served = runtime.block_on(async move {
                    let listener = tokio::net::TcpListener::from_std(listener)?;
                    axum::serve(listener, router).await
                });
                if let Err(e) = served {
                    log::error!("[media stream] listener stopped: {e}");
                }
            })?;
        Ok(())
    }
}

/// Tells the app's pages where the stream is, before any of their script
/// runs: a main-frame initialization script that sets [`PAGE_GLOBAL`]. With
/// no stream it sets nothing, and the page keeps video on the protocol.
pub(crate) fn plugin<R: tauri::Runtime>(
    stream: Option<&MediaStream>,
) -> tauri::plugin::TauriPlugin<R> {
    let builder = tauri::plugin::Builder::new("staged-media-stream");
    match stream {
        Some(stream) => builder.js_init_script(init_script(&stream.base_url())),
        None => builder,
    }
    .build()
}

fn init_script(base_url: &str) -> String {
    format!(
        "window.{PAGE_GLOBAL} = {};",
        serde_json::Value::from(base_url)
    )
}

struct Served {
    secret: String,
    index: Arc<MediaIndex>,
    store: Arc<StoreSource>,
}

fn router(secret: String, index: Arc<MediaIndex>, store: Arc<StoreSource>) -> Router {
    Router::new()
        .route("/{secret}/{file}", get(serve_video))
        .with_state(Arc::new(Served {
            secret,
            index,
            store,
        }))
}

/// `GET` or `HEAD /<secret>/<id>.<ext>`. Anything that is not the secret
/// followed by a stored video is a 404, with nothing to tell the cases apart.
async fn serve_video(
    State(served): State<Arc<Served>>,
    Path((secret, file)): Path<(String, String)>,
    request: Request,
) -> Response {
    let authorized: bool = secret.as_bytes().ct_eq(served.secret.as_bytes()).into();
    let is_video = super::refs::parse_media_file(&file)
        .and_then(|(_, ext)| super::files::mime_for_extension(ext))
        .is_some_and(|mime| mime.starts_with("video/"));
    if !authorized || !is_video {
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
