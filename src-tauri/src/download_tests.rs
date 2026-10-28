//! Local HTTP transport fixtures only. Production URL validation stays enabled.
use super::*;
use tokio::{io::AsyncReadExt, net::TcpListener, task::JoinHandle};

fn candidate(url: String, expected_size: Option<u64>, sha1: Option<&str>) -> Candidate {
    Candidate {
        url,
        filename: "surprise.txt".into(),
        expected_size,
        mime: Some("text/plain".into()),
        sha1: sha1.map(str::to_owned),
        source: "test fixture",
    }
}

/// Return headers/body from a real local socket. This client exists only in
/// tests; it cannot be selected by the application or its source adapters.
async fn fixture(raw: &'static [u8], keep_open: bool) -> (reqwest::Response, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
            assert!(request.len() <= 8192, "fixture request headers too large");
        }
        socket.write_all(raw).await.unwrap();
        if keep_open {
            // A deterministic peer that never finishes its response, used to
            // exercise cancellation and the engine's inactivity deadline.
            std::future::pending::<()>().await;
        }
    });
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://{address}/surprise.txt"))
        .send()
        .await
        .unwrap();
    (response, server)
}

fn prepared(response: reqwest::Response, total: Option<u64>, checksum: Option<&str>) -> Prepared {
    Prepared {
        candidate: candidate(response.url().to_string(), total, checksum),
        total,
        response,
    }
}

fn assert_empty(directory: &Path) {
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 0);
}

async fn wait_for_partial(directory: &Path) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let has_partial = std::fs::read_dir(directory)
                .unwrap()
                .any(|entry| entry.unwrap().metadata().unwrap().len() > 0);
            if has_partial {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the fixture should have streamed bytes to its private partial file");
}

#[tokio::test]
async fn verified_stream_is_saved_with_expected_digest_and_progress() {
    let (response, server) = fixture(
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nabc",
        false,
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let mut progress = Vec::new();
    let receipt = download(
        prepared(
            response,
            Some(3),
            Some("a9993e364706816aba3e25717850c26c9cd0d89d"),
        ),
        directory.path(),
        &CancellationToken::new(),
        |bytes, total| progress.push((bytes, total)),
    )
    .await
    .unwrap();
    server.await.unwrap();
    assert_eq!(std::fs::read(&receipt.path).unwrap(), b"abc");
    assert_eq!(receipt.bytes, 3);
    assert_eq!(
        receipt.sha256,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(progress.last(), Some(&(3, Some(3))));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn chunked_unknown_size_finishes_without_buffering_the_entire_file() {
    let (response, server) = fixture(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n6\r\nhello \r\n5\r\nworld\r\n0\r\n\r\n",
        false,
    )
    .await;
    assert_eq!(response.content_length(), None);
    let directory = tempfile::tempdir().unwrap();
    let receipt = download(
        prepared(response, None, None),
        directory.path(),
        &CancellationToken::new(),
        |_, _| {},
    )
    .await
    .unwrap();
    server.await.unwrap();
    assert_eq!(std::fs::read(&receipt.path).unwrap(), b"hello world");
    assert_eq!(receipt.bytes, 11);
}

#[tokio::test]
async fn truncated_http_body_never_becomes_a_final_file() {
    let (response, server) = fixture(
        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\npartial",
        false,
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let error = download(
        prepared(response, Some(100), None),
        directory.path(),
        &CancellationToken::new(),
        |_, _| {},
    )
    .await
    .err()
    .unwrap();
    server.await.unwrap();
    assert!(error.to_string().contains("interrupted"), "{error:#}");
    assert_empty(directory.path());
}

#[tokio::test]
async fn body_larger_than_metadata_is_rejected_and_cleaned_up() {
    let (response, server) = fixture(
        b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabcdef",
        false,
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let error = download(
        prepared(response, Some(3), None),
        directory.path(),
        &CancellationToken::new(),
        |_, _| {},
    )
    .await
    .err()
    .unwrap();
    server.await.unwrap();
    assert!(
        error.to_string().contains("more data than declared"),
        "{error:#}"
    );
    assert_empty(directory.path());
}

#[tokio::test]
async fn metadata_larger_than_complete_response_is_rejected_and_cleaned_up() {
    let (response, server) = fixture(
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc",
        false,
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let error = download(
        prepared(response, Some(6), None),
        directory.path(),
        &CancellationToken::new(),
        |_, _| {},
    )
    .await
    .err()
    .unwrap();
    server.await.unwrap();
    assert!(error.to_string().contains("Incomplete"), "{error:#}");
    assert_empty(directory.path());
}

#[tokio::test]
async fn checksum_mismatch_never_publishes_the_file() {
    let (response, server) = fixture(
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc",
        false,
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let error = download(
        prepared(
            response,
            Some(3),
            Some("0000000000000000000000000000000000000000"),
        ),
        directory.path(),
        &CancellationToken::new(),
        |_, _| {},
    )
    .await
    .err()
    .unwrap();
    server.await.unwrap();
    assert!(error.to_string().contains("checksum mismatch"), "{error:#}");
    assert_empty(directory.path());
}

#[tokio::test]
async fn cancel_after_partial_write_removes_temporary_file() {
    let (response, server) = fixture(
        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npartial",
        true,
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().to_owned();
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();
    let worker = tokio::spawn(async move {
        download(
            prepared(response, Some(100), None),
            &destination,
            &worker_cancel,
            |_, _| {},
        )
        .await
    });
    wait_for_partial(directory.path()).await;
    cancel.cancel();
    let error = worker.await.unwrap().err().unwrap();
    server.abort();
    assert!(error.to_string().contains("cancelled"), "{error:#}");
    assert_empty(directory.path());
}

#[tokio::test]
async fn cancellation_before_start_creates_no_files() {
    let (response, server) = fixture(
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc",
        false,
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(download(
        prepared(response, Some(3), None),
        directory.path(),
        &cancel,
        |_, _| {}
    )
    .await
    .is_err());
    server.await.unwrap();
    assert_empty(directory.path());
}

#[tokio::test]
async fn stalled_stream_times_out_and_removes_temporary_file() {
    let (response, server) = fixture(
        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npartial",
        true,
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().to_owned();
    let worker = tokio::spawn(async move {
        download(
            prepared(response, Some(100), None),
            &destination,
            &CancellationToken::new(),
            |_, _| {},
        )
        .await
    });
    wait_for_partial(directory.path()).await;
    tokio::time::pause();
    tokio::time::advance(STALL_TIMEOUT + Duration::from_secs(1)).await;
    let error = worker.await.unwrap().err().unwrap();
    server.abort();
    assert!(error.to_string().contains("stalled"), "{error:#}");
    assert_empty(directory.path());
}

#[tokio::test]
async fn oversized_source_metadata_is_rejected_before_any_request() {
    let client = SafeClient::new().unwrap();
    let result = prepare(
        &client,
        candidate(
            "https://archive.org/not-requested.txt".into(),
            Some(MAX_BYTES + 1),
            None,
        ),
    )
    .await;
    assert!(result.err().unwrap().to_string().contains("2 GiB"));
}
