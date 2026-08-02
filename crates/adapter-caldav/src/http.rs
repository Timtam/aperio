//! Shared HTTP send helper — one retry on transient connection failures.
//!
//! iCloud's CalDAV hosts (`pXX-caldav.icloud.com`) close idle keep-alive
//! connections well below reqwest's default 90 s pool-idle window. A
//! request that rides such a dead pooled socket fails with reqwest's
//! "error sending request" — a connection-level failure *before* any
//! HTTP response exists, so the server demonstrably never processed the
//! request and one replay on a fresh connection is safe for every verb
//! we use. The shorter `pool_idle_timeout` on the shared clients (see
//! `CaldavAdapter::new`) makes the stale-socket case rare; this retry
//! heals the rest (including genuine one-off network blips).
//!
//! Deliberately NOT retried: timeouts (the server may be mid-processing
//! — replaying a write could double-apply it), redirect-policy errors,
//! and anything after response bytes started flowing.

use reqwest::{RequestBuilder, Response};

/// True for connection-level failures where the request was never
/// processed by the server: connect errors and send-phase errors
/// (stale pooled socket, connection reset during send).
pub(crate) fn is_transient_send_error(err: &reqwest::Error) -> bool {
    if err.is_timeout() || err.is_redirect() || err.is_status() || err.is_body() || err.is_decode()
    {
        return false;
    }
    err.is_connect() || err.is_request()
}

/// `send()` with a single retry on a transient connection failure.
pub(crate) trait SendRetrying {
    async fn send_retrying(self) -> reqwest::Result<Response>;
}

impl SendRetrying for RequestBuilder {
    async fn send_retrying(self) -> reqwest::Result<Response> {
        // `try_clone` is `None` only for streaming bodies; every CalDAV
        // request carries a string/no body. A non-clonable request just
        // skips the retry and surfaces the original error.
        let retry = self.try_clone();
        match self.send().await {
            Err(err) if is_transient_send_error(&err) => match retry {
                Some(builder) => builder.send().await,
                None => Err(err),
            },
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Spawn a raw TCP server. For each accepted connection it runs the
    /// next entry of `behaviours`: `None` ⇒ drop the socket immediately
    /// (the stale-keep-alive shape: connect succeeds, then the request
    /// hits a dead socket), `Some(response)` ⇒ read the request and
    /// write the raw HTTP response. Returns (url, connection counter).
    async fn spawn_server(behaviours: Vec<Option<&'static str>>) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let connections = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&connections);
        tokio::spawn(async move {
            for behaviour in behaviours {
                let (mut sock, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => return,
                };
                counter.fetch_add(1, Ordering::SeqCst);
                match behaviour {
                    None => drop(sock),
                    Some(response) => {
                        let mut buf = [0u8; 2048];
                        let _ = sock.read(&mut buf).await;
                        let _ = sock.write_all(response.as_bytes()).await;
                        let _ = sock.shutdown().await;
                    }
                }
            }
        });
        (url, connections)
    }

    const OK: &str = "HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n";
    const SERVER_ERROR: &str = "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\n\r\n";

    #[tokio::test]
    async fn retries_once_when_the_connection_dies_before_a_response() {
        // First connection dies mid-send (the stale-pool shape); the
        // retry lands on a healthy one.
        let (url, connections) = spawn_server(vec![None, Some(OK)]).await;
        let response = reqwest::Client::new()
            .get(&url)
            .send_retrying()
            .await
            .expect("retry should succeed");
        assert_eq!(response.status(), 200);
        assert_eq!(connections.load(Ordering::SeqCst), 2, "exactly one retry");
    }

    #[tokio::test]
    async fn gives_up_after_one_retry() {
        // Both attempts die → the error surfaces; no endless loop.
        let (url, connections) = spawn_server(vec![None, None]).await;
        let err = reqwest::Client::new()
            .get(&url)
            .send_retrying()
            .await
            .expect_err("two dead connections must fail");
        assert!(is_transient_send_error(&err));
        assert_eq!(connections.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn does_not_retry_an_http_error_status() {
        // A 500 is a real server answer — it must pass through untouched
        // (status handling is the caller's business), with NO replay.
        let (url, connections) = spawn_server(vec![Some(SERVER_ERROR), Some(OK)]).await;
        let response = reqwest::Client::new()
            .get(&url)
            .send_retrying()
            .await
            .expect("a status response is not a send error");
        assert_eq!(response.status(), 500);
        assert_eq!(connections.load(Ordering::SeqCst), 1, "no retry on 5xx");
    }
}
