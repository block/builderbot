use super::*;
use crate::note_media::tests::{Fixture, PNG};
use axum::body::{Body, HttpBody};
use std::io::{Read, Write};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tower::ServiceExt;

const SECRET: &str = "5ec2e7";
const HOST: &str = "127.0.0.1:49152";

/// A request as the webview sends it: the path, and the listener's address
/// as its `Host`.
fn get(path: &str, headers: &[(&str, &str)]) -> axum::http::Request<Body> {
    request(path, &[&[("Host", HOST)][..], headers].concat())
}

fn request(path: &str, headers: &[(&str, &str)]) -> axum::http::Request<Body> {
    let mut request = axum::http::Request::builder().uri(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    request.body(Body::empty()).unwrap()
}

fn test_router(index: Arc<MediaIndex>, store: Arc<StoreSource>) -> Router {
    router(HOST.into(), SECRET.into(), index, store)
}

async fn bytes(response: Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn serves_stored_video_by_id_to_holders_of_the_secret_only() {
    let fixture = Fixture::new();
    let (file, mp4) = fixture.video(4096);
    std::fs::write(fixture.dir.path().join("shot.png"), PNG).unwrap();
    let image = fixture.ingest("", "![a](shot.png)");
    let image = image.kept_ids.iter().next().unwrap();
    let (store, _) = fixture.store_source();
    let router = test_router(Arc::new(MediaIndex::default()), store);
    let status = |path: String| {
        let router = router.clone();
        async move { router.oneshot(get(&path, &[])).await.unwrap().status() }
    };

    let response = router
        .clone()
        .oneshot(get(
            &format!("/{SECRET}/{file}"),
            &[("Range", "bytes=8-15")],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()["content-type"], "video/mp4");
    assert_eq!(response.headers()["accept-ranges"], "bytes");
    assert_eq!(
        response.headers()["content-range"],
        format!("bytes 8-15/{}", mp4.len())
    );
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(response.headers()["cache-control"], "no-store");
    // A `<video>` load is not a CORS request; no other origin is let in.
    assert!(response
        .headers()
        .get("access-control-allow-origin")
        .is_none());
    assert_eq!(bytes(response).await, &mp4[8..16]);

    // WebKit's open-ended range, as it sends after a seek.
    let response = router
        .clone()
        .oneshot(get(
            &format!("/{SECRET}/{file}"),
            &[("Range", "bytes=4000-")],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(bytes(response).await, &mp4[4000..]);

    let mut head = get(&format!("/{SECRET}/{file}"), &[]);
    *head.method_mut() = axum::http::Method::HEAD;
    let response = router.clone().oneshot(head).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-length"], mp4.len().to_string());

    // The secret gates everything, and a miss looks like any other.
    for path in [
        format!("/wrong/{file}"),
        format!("/{}/{file}", SECRET.to_uppercase()),
        format!("//{file}"),
        format!("/{file}"),
        format!("/{SECRET}"),
        format!("/{SECRET}/"),
        "/".to_owned(),
    ] {
        assert_eq!(status(path.clone()).await, StatusCode::NOT_FOUND, "{path}");
    }
    // Only stored video, only by id: not an image, a PDF name, a missing id,
    // or anything that reads as a path.
    let (id, _) = file.split_once('.').unwrap();
    for name in [
        format!("{image}.png"),
        format!("{image}.mp4"),
        format!("{id}.webm"),
        format!("{id}.pdf"),
        format!("{id}.MP4"),
        format!("{id}"),
        "00000000-0000-4000-8000-000000000000.mp4".to_owned(),
        "..".to_owned(),
        "..%2F..%2Fetc%2Fpasswd".to_owned(),
        format!("..%2F{file}"),
        format!("{file}%00.mp4"),
        format!("{file}/extra"),
    ] {
        let path = format!("/{SECRET}/{name}");
        assert_eq!(status(path.clone()).await, StatusCode::NOT_FOUND, "{path}");
    }
    let mut post = get(&format!("/{SECRET}/{file}"), &[]);
    *post.method_mut() = axum::http::Method::POST;
    assert_eq!(
        router.oneshot(post).await.unwrap().status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
}

/// The secret is in a URL, and a page elsewhere that came by one could give
/// 127.0.0.1 a name of its own (DNS rebinding) to read the video as its own
/// origin. Only the address the page was given is answered; anything else is
/// the 404 a wrong secret gets.
#[tokio::test]
async fn answers_only_requests_addressed_to_its_own_host() {
    let fixture = Fixture::new();
    let (file, mp4) = fixture.video(4096);
    let (store, asked) = fixture.store_source();
    let router = test_router(Arc::new(MediaIndex::default()), store);
    let path = format!("/{SECRET}/{file}");
    let send = |request: axum::http::Request<Body>| router.clone().oneshot(request);

    for headers in [
        vec![],
        vec![("Host", "attacker.example:49152")],
        vec![("Host", "attacker.example")],
        vec![("Host", "localhost:49152")],
        vec![("Host", "127.0.0.1")],
        vec![("Host", "127.0.0.1:49153")],
        vec![("Host", "127.0.0.1:49152.attacker.example")],
        vec![("Host", "[::1]:49152")],
        vec![("Host", "")],
        // Two names: whichever a proxy or a parser picks, one is not ours.
        vec![("Host", HOST), ("Host", "attacker.example:49152")],
        vec![("Host", "attacker.example:49152"), ("Host", HOST)],
        // Not the forwarded name either.
        vec![("X-Forwarded-Host", HOST)],
    ] {
        let response = send(request(&path, &headers)).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{headers:?}");
        assert!(bytes(response).await.is_empty(), "{headers:?}");
    }
    // A request target that carries its own authority has to agree as well.
    for target in [
        format!("http://attacker.example:49152{path}"),
        format!("http://attacker.example{path}"),
    ] {
        let response = send(get(&target, &[])).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{target}");
    }
    // None of those reached the store.
    assert_eq!(asked.load(Ordering::SeqCst), 0);

    for target in [path.clone(), format!("http://{HOST}{path}")] {
        let response = send(get(&target, &[("Range", "bytes=0-3")])).await.unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT, "{target}");
        assert_eq!(bytes(response).await, &mp4[..4]);
    }
}

/// The point of the stream: a seek on a video that is already open reads
/// the file and nothing else, while the rest of the app holds the store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seeks_while_the_store_connection_is_held() {
    let fixture = Fixture::new();
    let (file, mp4) = fixture.video(4096);
    let (store, asked) = fixture.store_source();
    let router = test_router(Arc::new(MediaIndex::default()), store);
    let seek = |range: &'static str| {
        router
            .clone()
            .oneshot(get(&format!("/{SECRET}/{file}"), &[("Range", range)]))
    };
    assert_eq!(seek("bytes=0-1").await.unwrap().status(), 206);
    assert_eq!(asked.load(Ordering::SeqCst), 1);

    let held = fixture.store.hold_connection();
    let response = tokio::time::timeout(Duration::from_secs(10), seek("bytes=1000-1999"))
        .await
        .expect("a seek waited on the store connection")
        .unwrap();
    drop(held);
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(bytes(response).await, &mp4[1000..2000]);
    assert_eq!(asked.load(Ordering::SeqCst), 1);
}

/// WebKit asks for a WebM with no `Range` at all. The answer is the whole
/// file, but never in memory at once: it leaves in chunks of at most 64 KiB.
#[tokio::test]
async fn a_request_without_a_range_streams_the_whole_file_in_chunks() {
    let fixture = Fixture::new();
    let (file, mp4) = fixture.video(1024 * 1024 + 123);
    let (store, _) = fixture.store_source();
    let router = test_router(Arc::new(MediaIndex::default()), store);
    let response = router
        .oneshot(get(&format!("/{SECRET}/{file}"), &[]))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-length"], mp4.len().to_string());
    assert_eq!(response.headers()["accept-ranges"], "bytes");

    let mut body = response.into_body();
    let (mut received, mut chunks) = (Vec::new(), 0);
    while let Some(frame) =
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut body).poll_frame(cx)).await
    {
        if let Ok(data) = frame.unwrap().into_data() {
            assert!(data.len() <= 64 * 1024, "a {} byte chunk", data.len());
            chunks += 1;
            received.extend_from_slice(&data);
        }
    }
    assert!(chunks >= 17, "{chunks} chunks");
    assert!(received == mp4);
}

fn http_get(port: u16, host: &str, path: &str) -> String {
    let mut socket = std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        socket,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nRange: bytes=4-7\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = Vec::new();
    socket.read_to_end(&mut response).unwrap();
    String::from_utf8_lossy(&response).into_owned()
}

/// The real listener: loopback only, a port the system chose, a secret of
/// its own per bind, and the page told exactly that address.
#[test]
fn listens_on_loopback_with_a_fresh_port_and_secret() {
    let fixture = Fixture::new();
    let (file, _) = fixture.video(64);
    let stream = MediaStream::bind().unwrap();
    let other = MediaStream::bind().unwrap();
    let address = stream.listener.local_addr().unwrap();
    assert_eq!(address.ip(), Ipv4Addr::LOCALHOST);
    assert_ne!(address.port(), 0);
    assert_ne!(address.port(), other.port);
    assert_eq!(stream.secret.len(), 64);
    assert!(stream.secret.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_ne!(stream.secret, other.secret);
    let base = stream.base_url();
    assert_eq!(
        base,
        format!("http://127.0.0.1:{}/{}", address.port(), stream.secret)
    );
    assert_eq!(stream.host(), format!("127.0.0.1:{}", address.port()));

    let (port, host, secret) = (stream.port, stream.host(), stream.secret.clone());
    stream
        .serve(Arc::new(MediaIndex::default()), fixture.store_source().0)
        .unwrap();
    let response = http_get(port, &host, &format!("/{secret}/{file}"));
    assert!(response.starts_with("HTTP/1.1 206 "), "{response}");
    assert!(response.ends_with("\r\n\r\nftyp"), "{response}");
    let response = http_get(port, &host, &format!("/{}/{file}", other.secret));
    assert!(response.starts_with("HTTP/1.1 404 "), "{response}");
    // The right secret under another name for this machine, as a page that
    // rebound its own hostname to 127.0.0.1 would send it.
    for name in [
        format!("attacker.example:{port}"),
        format!("localhost:{port}"),
    ] {
        let response = http_get(port, &name, &format!("/{secret}/{file}"));
        assert!(response.starts_with("HTTP/1.1 404 "), "{name}: {response}");
    }
}

fn config(json: serde_json::Value) -> tauri::Config {
    serde_json::from_value(json).unwrap()
}

/// The address goes only to the app's own pages. An initialization script
/// runs in whatever a window loads, so it checks where it is first.
#[test]
fn the_page_script_names_the_address_to_the_app_origins_only() {
    let stream = MediaStream::bind().unwrap();
    let base = stream.base_url();
    let origins = ["tauri://localhost".to_owned()];
    assert_eq!(
        init_script(&base, &origins),
        format!(
            "if ([\"tauri://localhost\"].includes(window.location.origin)) \
             window.__STAGED_MEDIA_STREAM__ = \"{base}\";"
        )
    );
    // No origin, no page: never an unconditional assignment.
    assert!(init_script(&base, &[]).starts_with("if ([].includes(window.location.origin)) "));

    use tauri::plugin::Plugin;
    let plugin = plugin::<tauri::test::MockRuntime>(Some(&stream), &origins);
    let script = plugin.initialization_script_2().unwrap();
    assert_eq!(script.script, init_script(&base, &origins));
    // Not to frames: a note's content is never one, but nothing else is either.
    assert!(script.for_main_frame_only);
}

/// The origins have to be the ones the real page has, or video silently
/// stays on the protocol: Tauri's own protocol for a packaged build, the dev
/// server for `tauri dev`, wherever `--config` put it.
#[test]
fn page_origins_follow_the_app_url() {
    let packaged = if cfg!(any(windows, target_os = "android")) {
        vec!["http://tauri.localhost", "https://tauri.localhost"]
    } else {
        vec!["tauri://localhost"]
    };
    let checked_in =
        config(serde_json::from_str(include_str!("../../../tauri.conf.json")).unwrap());
    assert_eq!(page_origins_for(false, &checked_in), packaged);
    assert_eq!(
        page_origins_for(true, &checked_in),
        ["http://localhost:5174"]
    );
    // `just dev` moves the dev server to a port of the worktree's own.
    let moved = config(serde_json::json!({
        "identifier": "xyz.block.staged.test",
        "build": { "devUrl": "http://localhost:43117/", "frontendDist": "../dist" },
    }));
    assert_eq!(page_origins_for(true, &moved), ["http://localhost:43117"]);
    assert_eq!(page_origins_for(false, &moved), packaged);
    // Dev with no dev server is the bundled frontend, as in a packaged build.
    let bundled = config(serde_json::json!({
        "identifier": "xyz.block.staged.test",
        "build": { "frontendDist": "../dist" },
    }));
    assert_eq!(page_origins_for(true, &bundled), packaged);
    // A frontend served from a URL is only the page in a packaged build.
    let hosted = config(serde_json::json!({
        "identifier": "xyz.block.staged.test",
        "build": { "frontendDist": "https://staged.example/app/" },
    }));
    assert_eq!(page_origins_for(false, &hosted), ["https://staged.example"]);
    assert_eq!(page_origins_for(true, &hosted), packaged);
    // This build's own answer is one of the two.
    assert_eq!(
        page_origins(&checked_in),
        page_origins_for(tauri::is_dev(), &checked_in)
    );
}

/// `run` binds before the builder and tells the page through the plugin. A
/// bind that failed must tell it nothing, so the page keeps video on the
/// protocol (`resolveStagedVideoUrl`), and must not stop the app.
#[test]
fn a_failed_bind_tells_the_page_nothing() {
    use tauri::plugin::Plugin;
    let bound: std::io::Result<MediaStream> = Err(std::io::Error::other("no port"));
    let origins = ["tauri://localhost".to_owned()];
    let plugin = plugin::<tauri::test::MockRuntime>(bound.as_ref().ok(), &origins);
    assert!(plugin.initialization_script_2().is_none());
    assert!(plugin.initialization_script().is_none());

    let fixture = Fixture::new();
    let (file, mp4) = fixture.video(4096);
    let index = Arc::new(MediaIndex::default());
    let (store, _) = fixture.store_source();
    start(bound, index.clone(), store.clone());
    assert_protocol_serves(&index, &*store, &file, &mp4);
}

/// A listener that is bound but cannot serve has already been named to the
/// page. It must be closed, so a video fails to load from it at once and
/// moves to the protocol, and the protocol must still answer from the index
/// the two share.
#[test]
fn a_failed_serve_closes_the_listener_and_leaves_the_protocol_working() {
    let fixture = Fixture::new();
    let (file, mp4) = fixture.video(4096);
    let index = Arc::new(MediaIndex::default());
    let (store, _) = fixture.store_source();
    let stream = MediaStream::bind().unwrap();
    let port = stream.port;
    // Bound and not yet serving, a connection is accepted into the backlog.
    std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();

    let served = stream.serve_on(index.clone(), store.clone(), |_run| {
        Err(std::io::Error::other("no thread"))
    });
    assert_eq!(served.unwrap_err().to_string(), "no thread");
    let refused = std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap_err();
    assert_eq!(refused.kind(), std::io::ErrorKind::ConnectionRefused);
    assert_protocol_serves(&index, &*store, &file, &mp4);
}

fn assert_protocol_serves(index: &MediaIndex, store: &StoreSource, file: &str, mp4: &[u8]) {
    let request = tauri::http::Request::builder()
        .uri(format!("staged-media://localhost/{file}"))
        .header("Range", "bytes=8-15")
        .body(vec![])
        .unwrap();
    let response = crate::note_media::serving::response(index, store, request);
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.body(), &mp4[8..16]);
}
