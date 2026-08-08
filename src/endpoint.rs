//! The HTTP surface both halves expose: `/metrics` for a scraper, `/health` for a supervisor.
//!
//! Deliberately unauthenticated and off unless asked for. What it serves is operational, not
//! private: counts of streams and connections, never a peer, an address, or anything about what a
//! wallet asked. Even so it belongs on loopback or a private network, because it reveals that this
//! machine runs the proxy and how busy it is.

use std::convert::Infallible;
use std::future::Future;

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use prometheus::Registry;
use tokio::net::TcpListener;

use crate::health::Health;
use crate::metrics;

/// The exposition format's own content type, which is what a scraper expects to be handed.
const EXPOSITION: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Serve until `shutdown` resolves.
///
/// The listener arrives already bound so that a port already in use is reported by whoever
/// configured it, at startup, rather than from in here once the half is otherwise running.
pub async fn serve(
    listener: TcpListener,
    registry: Registry,
    health: Health,
    shutdown: impl Future<Output = ()>,
) {
    tokio::pin!(shutdown);

    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = &mut shutdown => return,
        };

        let (connection, _) = match accepted {
            Ok(accepted) => accepted,
            // One refused connection is not a reason to stop reporting: a scraper will be back.
            Err(error) => {
                tracing::debug!(%error, "an observability connection was not accepted");
                continue;
            }
        };

        let registry = registry.clone();
        let health = health.clone();
        tokio::spawn(async move {
            let service = service_fn(move |request| {
                let registry = registry.clone();
                let health = health.clone();
                async move { Ok::<_, Infallible>(route(&request, &registry, &health)) }
            });
            if let Err(error) = http1::Builder::new()
                .serve_connection(TokioIo::new(connection), service)
                .await
            {
                tracing::debug!(%error, "an observability connection ended badly");
            }
        });
    }
}

fn route(
    request: &Request<Incoming>,
    registry: &Registry,
    health: &Health,
) -> Response<Full<Bytes>> {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return reply(StatusCode::METHOD_NOT_ALLOWED, "text/plain", String::new());
    }

    match request.uri().path() {
        "/metrics" => match metrics::encode(registry) {
            Ok(encoded) => reply(StatusCode::OK, EXPOSITION, encoded),
            Err(error) => {
                tracing::error!(%error, "gathering metrics failed");
                reply(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "text/plain",
                    String::new(),
                )
            }
        },
        // A half that is still registering answers, and says so, rather than refusing the
        // connection: "starting" and "down" call for different reactions from a supervisor.
        "/health" => {
            let status = if health.state().is_ready() {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            };
            reply(status, "application/json", health.as_json())
        }
        _ => reply(StatusCode::NOT_FOUND, "text/plain", String::new()),
    }
}

fn reply(status: StatusCode, content_type: &str, body: String) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, content_type)
        .body(Full::new(Bytes::from(body)))
        // The builder only fails on a header this function does not build, so the fallback is
        // unreachable rather than a case worth handling.
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::State;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A server on an ephemeral port, with the means to stop it and to wait for it to be gone.
    struct Serving {
        address: std::net::SocketAddr,
        stop: tokio::sync::oneshot::Sender<()>,
        stopped: tokio::task::JoinHandle<()>,
    }

    async fn serving(health: Health) -> Serving {
        let registry = crate::metrics::ClientMetrics::new()
            .unwrap()
            .registry()
            .clone();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, shutdown) = tokio::sync::oneshot::channel();
        let stopped = tokio::spawn(async move {
            serve(listener, registry, health, async {
                let _ = shutdown.await;
            })
            .await;
        });
        Serving {
            address,
            stop,
            stopped,
        }
    }

    async fn get(address: std::net::SocketAddr, path: &str) -> String {
        let mut connection = tokio::net::TcpStream::connect(address).await.unwrap();
        connection
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = String::new();
        connection.read_to_string(&mut response).await.unwrap();
        response
    }

    #[tokio::test]
    async fn a_starting_half_is_not_ready_yet() {
        let serving = serving(Health::starting()).await;
        assert!(
            get(serving.address, "/health")
                .await
                .starts_with("HTTP/1.1 503")
        );
    }

    #[tokio::test]
    async fn a_serving_half_reports_itself_ready() {
        let health = Health::starting();
        health.advance_to(State::Serving);
        let serving = serving(health).await;

        assert!(
            get(serving.address, "/health")
                .await
                .contains("{\"state\":\"serving\"}")
        );
    }

    #[tokio::test]
    async fn the_metrics_endpoint_serves_the_exposition_format() {
        let serving = serving(Health::starting()).await;
        let response = get(serving.address, "/metrics").await;

        assert!(
            response.contains("lwd_mixnet_client_connections_total"),
            "{response}"
        );
    }

    #[tokio::test]
    async fn an_unknown_path_is_not_found() {
        let serving = serving(Health::starting()).await;
        assert!(get(serving.address, "/").await.starts_with("HTTP/1.1 404"));
    }

    #[tokio::test]
    async fn a_write_is_not_allowed() {
        let serving = serving(Health::starting()).await;
        let mut connection = tokio::net::TcpStream::connect(serving.address)
            .await
            .unwrap();
        connection
            .write_all(b"POST /metrics HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        connection.read_to_string(&mut response).await.unwrap();

        assert!(response.starts_with("HTTP/1.1 405"), "{response}");
    }

    #[tokio::test]
    async fn shutting_down_ends_the_server() {
        let serving = serving(Health::starting()).await;
        get(serving.address, "/health").await;

        serving.stop.send(()).unwrap();

        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(5), serving.stopped)
                .await
                .is_ok(),
            "the server should have returned once told to shut down"
        );
    }
}
