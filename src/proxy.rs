use crate::docker::{
    DockerBackend, build_docker_uri, full_body, sanitize_path_for_container,
};
use anyhow::Context as _;
use bytes::Bytes;
use http_body_util::{Either, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};

#[derive(Clone)]
pub struct ProxyTarget {
    backend: DockerBackend,
    container_id: String,
    accepted_refs: Vec<String>,
}

impl ProxyTarget {
    pub async fn resolve(backend: DockerBackend, container_ref: &str) -> anyhow::Result<Self> {
        let inspect = backend.inspect_container(container_ref).await?;
        let short_id: String = inspect.id.chars().take(12).collect();
        let mut accepted_refs = vec![
            container_ref.to_string(),
            inspect.id.clone(),
            short_id,
            inspect.name.trim_start_matches('/').to_string(),
        ];
        accepted_refs.sort();
        accepted_refs.dedup();

        Ok(Self {
            backend,
            container_id: inspect.id,
            accepted_refs,
        })
    }
}

pub async fn serve_channel_stream<S>(stream: S, target: ProxyTarget) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let io = TokioIo::new(stream);
    let service = service_fn(move |req| {
        let target = target.clone();
        async move { handle_request(req, target).await }
    });

    http1::Builder::new()
        .keep_alive(true)
        .serve_connection(io, service)
        .await
        .context("proxy HTTP connection failed")?;

    Ok(())
}

async fn handle_request(
    req: Request<Incoming>,
    target: ProxyTarget,
) -> Result<Response<Either<Incoming, Full<Bytes>>>, std::convert::Infallible> {
    let original = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

    let path_and_query = match sanitize_path_for_container(
        original,
        &target.container_id,
        &target.accepted_refs,
    ) {
        Ok(path) => path,
        Err(err) => {
            return Ok(simple_response(StatusCode::FORBIDDEN, err.to_string()));
        }
    };

    let uri = match build_docker_uri(&path_and_query) {
        Ok(uri) => uri,
        Err(err) => {
            return Ok(simple_response(StatusCode::BAD_REQUEST, err.to_string()));
        }
    };

    let (parts, body) = req.into_parts();
    let mut builder = Request::builder()
        .method(parts.method)
        .version(parts.version)
        .uri(uri);
    for (name, value) in &parts.headers {
        builder = builder.header(name, value);
    }

    let proxied = match builder.body(body) {
        Ok(req) => req,
        Err(err) => {
            return Ok(simple_response(
                StatusCode::BAD_REQUEST,
                format!("invalid request: {err}"),
            ));
        }
    };

    match target.backend.request(proxied).await {
        Ok(resp) => Ok(resp.map(Either::Left)),
        Err(err) => Ok(simple_response(
            StatusCode::BAD_GATEWAY,
            format!("docker backend request failed: {err}"),
        )),
    }
}

fn simple_response(
    status: StatusCode,
    message: String,
) -> Response<Either<Incoming, Full<Bytes>>> {
    let mut resp = Response::new(Either::Right(full_body(Bytes::from(message))));
    *resp.status_mut() = status;
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use std::sync::Arc;
    use tokio::net::UnixListener;
    use tokio::sync::{Mutex, oneshot};

    #[tokio::test]
    async fn forwards_ping_over_unix_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let socket_path = tmp.path().join("docker.sock");
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen_clone = seen.clone();

        let listener = UnixListener::bind(&socket_path).unwrap();
        let (done_tx, done_rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let service = service_fn(move |req: Request<Incoming>| {
                let seen_clone = seen_clone.clone();
                async move {
                    seen_clone
                        .lock()
                        .await
                        .push(req.uri().path_and_query().unwrap().as_str().to_string());
                    Ok::<_, std::convert::Infallible>(Response::new(full_body(Bytes::from("OK"))))
                }
            });
            http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
                .unwrap();
            let _ = done_tx.send(());
        });

        let target = ProxyTarget {
            backend: DockerBackend::from_socket_path(socket_path.clone()),
            container_id: "abc123".to_string(),
            accepted_refs: vec!["abc123".to_string(), "my-container".to_string()],
        };

        let (client_side, server_side) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            serve_channel_stream(server_side, target).await.unwrap();
        });

        let mut client_side = client_side;
        client_side
            .write_all(b"GET /_ping HTTP/1.1\r\nHost: docker\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut raw = Vec::new();
        client_side.read_to_end(&mut raw).await.unwrap();
        assert!(String::from_utf8_lossy(&raw).contains("\r\n\r\nOK"));

        done_rx.await.unwrap();
        let paths = seen.lock().await.clone();
        assert_eq!(paths, vec!["/_ping"]);
    }
}
