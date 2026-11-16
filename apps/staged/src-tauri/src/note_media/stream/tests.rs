use super::*;
use crate::note_media::tests::{Fixture, PNG};
use axum::body::{Body, HttpBody};
use std::io::{Read, Write};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tower::ServiceExt;

const SECRET: &str = "5ec2e7";

fn get(path: &str, headers: &[(&str, &str)]) -> axum::http::Request<Body> {
    let mut request = axum::http::Request::builder().uri(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    request.body(Body::empty()).unwrap()
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
    let router = router(SECRET.into(), Arc::new(MediaIndex::default()), store);
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

/// The point of the stream: a seek on a video that is already open reads
/// the file and nothing else, while the rest of the app holds the store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seeks_while_the_store_connection_is_held() {
    let fixture = Fixture::new();
    let (file, mp4) = fixture.video(4096);
    let (store, asked) = fixture.store_source();
    let router = router(SECRET.into(), Arc::new(MediaIndex::default()), store);
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
    let router = router(SECRET.into(), Arc::new(MediaIndex::default()), store);
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

fn http_get(port: u16, path: &str) -> String {
    let mut socket = std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        socket,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nRange: bytes=4-7\r\nConnection: close\r\n\r\n"
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
    assert_eq!(
        init_script(&base),
        format!("window.__STAGED_MEDIA_STREAM__ = \"{base}\";")
    );

    let (port, secret) = (stream.port, stream.secret.clone());
    stream
        .serve(Arc::new(MediaIndex::default()), fixture.store_source().0)
        .unwrap();
    let response = http_get(port, &format!("/{secret}/{file}"));
    assert!(response.starts_with("HTTP/1.1 206 "), "{response}");
    assert!(response.ends_with("\r\n\r\nftyp"), "{response}");
    let response = http_get(port, &format!("/{}/{file}", other.secret));
    assert!(response.starts_with("HTTP/1.1 404 "), "{response}");
}
