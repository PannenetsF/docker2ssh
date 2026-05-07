use crate::config::{ConfigFile, MappingSpec};
use anyhow::{Context as _, bail};
use bytes::Bytes;
use http_body_util::{BodyExt as _, Empty, Full};
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode, Uri};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde::Deserialize;
use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct DockerBackend {
    socket_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ActiveMapping {
    pub port: u16,
    pub container_ref: String,
    pub container_id_short: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContainerInspect {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "State")]
    pub state: ContainerState,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContainerState {
    #[serde(rename = "Running")]
    pub running: bool,
}

impl DockerBackend {
    #[cfg(test)]
    pub(crate) fn from_socket_path(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    pub fn from_env_or_config(cfg: &ConfigFile) -> anyhow::Result<Self> {
        let socket = env::var("D2S_DOCKER_SOCKET")
            .or_else(|_| env::var("D2S_DOCKER_ENDPOINT"))
            .unwrap_or_else(|_| cfg.docker_socket.clone());

        Ok(Self {
            socket_path: PathBuf::from(socket),
        })
    }

    pub async fn inspect_container(&self, name_or_id: &str) -> anyhow::Result<ContainerInspect> {
        let path = format!("/containers/{name_or_id}/json");
        let req = Request::builder()
            .method(Method::GET)
            .uri(build_docker_uri(&path)?)
            .body(Empty::<Bytes>::new())?;

        let resp = self.request(req).await?;
        if resp.status() == StatusCode::NOT_FOUND {
            bail!("container not found");
        }
        ensure_success(&resp, "inspect container")?;
        let body = resp.collect().await?.to_bytes();
        let inspect = serde_json::from_slice::<ContainerInspect>(&body)?;
        Ok(inspect)
    }

    pub async fn active_mappings(
        &self,
        mappings: &[MappingSpec],
    ) -> anyhow::Result<Vec<ActiveMapping>> {
        let mut out = Vec::new();
        for mapping in mappings {
            let inspect = self.inspect_container(&mapping.container).await?;
            if inspect.state.running {
                out.push(ActiveMapping {
                    port: mapping.port,
                    container_ref: mapping.container.clone(),
                    container_id_short: inspect.id.chars().take(12).collect(),
                });
            }
        }
        Ok(out)
    }

    pub(crate) async fn request<B>(&self, req: Request<B>) -> anyhow::Result<Response<Incoming>>
    where
        B: hyper::body::Body<Data = Bytes> + Send + Unpin + 'static,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let connector = UnixConnector::new(self.socket_path.clone());
        let client = Client::builder(TokioExecutor::new()).build(connector);
        let resp = client.request(req).await.context("docker request failed")?;
        Ok(resp)
    }
}

fn ensure_success(resp: &Response<Incoming>, op: &str) -> anyhow::Result<()> {
    if resp.status().is_success() {
        Ok(())
    } else {
        bail!("{op} failed with status {}", resp.status())
    }
}

#[derive(Clone)]
pub struct UnixConnector {
    socket_path: PathBuf,
}

impl UnixConnector {
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }
}

impl tower_service::Service<Uri> for UnixConnector {
    type Response = hyper_util::rt::TokioIo<tokio::net::UnixStream>;
    type Error = std::io::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, _req: Uri) -> Self::Future {
        let path = self.socket_path.clone();
        Box::pin(async move {
            let stream = tokio::net::UnixStream::connect(path).await?;
            Ok(hyper_util::rt::TokioIo::new(stream))
        })
    }
}

pub fn sanitize_path_for_container(
    path_and_query: &str,
    mapped_container_id: &str,
    accepted_refs: &[String],
) -> anyhow::Result<String> {
    let normalized = strip_version_prefix(path_and_query);
    let (path, query) = split_path_query(&normalized);

    if path == "/containers/json" {
        return Ok(format!(
            "/containers/json?filters=%7B%22id%22%3A%5B%22{mapped_container_id}%22%5D%7D"
        ));
    }

    if let Some(rest) = path.strip_prefix("/containers/") {
        let Some((target, suffix)) = rest.split_once('/') else {
            bail!("unsupported container path");
        };
        if !container_ref_matches(target, mapped_container_id, accepted_refs) {
            bail!("requested container does not match mapped container");
        }

        let mut out = format!("/containers/{mapped_container_id}/{suffix}");
        if let Some(query) = query {
            out.push('?');
            out.push_str(query);
        }
        return Ok(out);
    }

    let allowed_prefixes = [
        "/_ping",
        "/version",
        "/info",
        "/events",
        "/system/",
        "/auth",
        "/session",
        "/distribution/",
        "/build",
        "/images/",
        "/networks/",
        "/volumes/",
        "/plugins/",
        "/swarm",
        "/nodes",
        "/services",
        "/tasks",
        "/secrets",
        "/configs",
    ];

    if allowed_prefixes.iter().any(|prefix| path.starts_with(prefix)) {
        return Ok(normalized);
    }

    if path == "/" {
        return Ok(normalized);
    }

    bail!("path not allowed for mapped docker endpoint: {path}")
}

pub fn docker_authorized_command(command: &str) -> bool {
    matches!(
        command.trim(),
        "docker system dial-stdio" | "docker -H unix:///var/run/docker.sock system dial-stdio"
    )
}

pub fn full_body(bytes: Bytes) -> Full<Bytes> {
    Full::new(bytes)
}

pub fn build_docker_uri(path_and_query: &str) -> anyhow::Result<Uri> {
    let uri = format!("http://docker{path_and_query}").parse::<Uri>()?;
    Ok(uri)
}

fn strip_version_prefix(path_and_query: &str) -> String {
    if !path_and_query.starts_with("/v") {
        return path_and_query.to_string();
    }

    let trimmed = &path_and_query[1..];
    let Some(slash_idx) = trimmed.find('/') else {
        return path_and_query.to_string();
    };
    let version = &trimmed[..slash_idx];
    if version
        .strip_prefix('v')
        .is_some_and(|s| s.chars().next().is_some() && s.chars().all(|c| c.is_ascii_digit() || c == '.'))
    {
        format!("/{}", &trimmed[slash_idx + 1..])
    } else {
        path_and_query.to_string()
    }
}

fn split_path_query(path_and_query: &str) -> (&str, Option<&str>) {
    match path_and_query.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path_and_query, None),
    }
}

fn container_ref_matches(target: &str, mapped_container_id: &str, accepted_refs: &[String]) -> bool {
    if target == mapped_container_id || mapped_container_id.starts_with(target) {
        return true;
    }

    accepted_refs.iter().any(|candidate| {
        target == candidate || candidate.starts_with(target) || target.starts_with(candidate)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_standard_docker_dial_stdio_commands() {
        assert!(docker_authorized_command("docker system dial-stdio"));
        assert!(docker_authorized_command(
            "docker -H unix:///var/run/docker.sock system dial-stdio"
        ));
        assert!(!docker_authorized_command("bash"));
    }

    #[test]
    fn rewrite_versioned_container_path() {
        let path = sanitize_path_for_container(
            "/v1.46/containers/abc123/archive?path=/tmp",
            "abc123",
            &[String::from("abc123")],
        )
        .unwrap();
        assert_eq!(path, "/containers/abc123/archive?path=/tmp");
    }

    #[test]
    fn reject_other_container_path() {
        let err = sanitize_path_for_container(
            "/containers/zzz/archive?path=/tmp",
            "abc123",
            &[String::from("abc123"), String::from("my-container")],
        )
        .unwrap_err();
        assert!(err.to_string().contains("does not match"));
    }

    #[test]
    fn allow_container_name_alias() {
        let path = sanitize_path_for_container(
            "/containers/my-container/archive?path=/tmp",
            "abc123456789",
            &[String::from("my-container"), String::from("abc123456789")],
        )
        .unwrap();
        assert_eq!(path, "/containers/abc123456789/archive?path=/tmp");
    }
}
