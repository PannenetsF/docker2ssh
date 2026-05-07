use crate::config::{ConfigStore, MappingSpec};
use crate::docker::{ActiveMapping, DockerBackend, docker_authorized_command};
use crate::proxy::{ProxyTarget, serve_channel_stream};
use anyhow::{Context as _, bail};
use russh::keys::{self, PrivateKey, PrivateKeyWithHashAlg, PublicKey};
use russh::keys::ssh_key::LineEnding;
use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use socket2::{Domain, Protocol, Socket, Type};
use std::env;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::process::Command;

pub struct ServeManager {
    store: ConfigStore,
    docker: DockerBackend,
}

impl ServeManager {
    pub fn new(store: ConfigStore, docker: DockerBackend) -> Self {
        Self { store, docker }
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let cfg = self.store.load().await?;
        let host_key_path = resolve_host_key_path(self.store.path(), cfg.host_key.as_deref())?;
        let host_key = load_or_generate_host_key(&host_key_path).await?;
        let authz = Arc::new(Authz::load(cfg.authorized_keys.as_deref()).await?);

        if cfg.mappings.is_empty() {
            println!("no mappings configured in {}", self.store.path().display());
            tokio::signal::ctrl_c().await?;
            return Ok(());
        }

        let mut handles = Vec::new();
        for mapping in cfg.mappings {
            let port = mapping.port;
            let server = PortServer::new(mapping, self.docker.clone(), authz.clone());
            let listeners = build_listeners(&cfg.listen_host, port)?;

            for (listen_label, listener) in listeners {
                let mut task_server = server.clone();
                let server_config = Arc::new(server::Config {
                    inactivity_timeout: Some(Duration::from_secs(3600)),
                    auth_rejection_time: Duration::from_secs(1),
                    auth_rejection_time_initial: Some(Duration::from_secs(0)),
                    keys: vec![host_key.clone()],
                    ..Default::default()
                });

                println!("listening on {listen_label} -> {}", server.mapping.container);
                handles.push(tokio::spawn(async move {
                    task_server
                        .run_on_socket(server_config, &listener)
                        .await
                        .map_err(anyhow::Error::from)
                }));
            }
        }

        tokio::signal::ctrl_c().await?;
        for handle in handles {
            handle.abort();
        }
        Ok(())
    }
}

fn build_listeners(listen_host: &str, port: u16) -> anyhow::Result<Vec<(String, TcpListener)>> {
    let normalized = normalize_listen_host(listen_host);
    if is_dual_stack_wildcard(&normalized) {
        return Ok(vec![
            (
                format!("0.0.0.0:{port}"),
                bind_v4(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port))?,
            ),
            (
                format!("[::]:{port}"),
                bind_v6(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port))?,
            ),
        ]);
    }

    let ip = normalized
        .parse::<IpAddr>()
        .with_context(|| format!("invalid listen_host: {listen_host}"))?;
    let listener = match ip {
        IpAddr::V4(v4) => bind_v4(SocketAddr::new(IpAddr::V4(v4), port))?,
        IpAddr::V6(v6) => bind_v6(SocketAddr::new(IpAddr::V6(v6), port))?,
    };
    Ok(vec![(display_socket_addr(&SocketAddr::new(ip, port)), listener)])
}

fn normalize_listen_host(listen_host: &str) -> String {
    listen_host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string()
}

fn is_dual_stack_wildcard(listen_host: &str) -> bool {
    matches!(listen_host, "0.0.0.0" | "::")
}

fn display_socket_addr(addr: &SocketAddr) -> String {
    match addr {
        SocketAddr::V4(_) => addr.to_string(),
        SocketAddr::V6(_) => format!("[{}]:{}", addr.ip(), addr.port()),
    }
}

fn bind_v4(addr: SocketAddr) -> anyhow::Result<TcpListener> {
    bind_socket(addr, false)
}

fn bind_v6(addr: SocketAddr) -> anyhow::Result<TcpListener> {
    bind_socket(addr, true)
}

fn bind_socket(addr: SocketAddr, v6_only: bool) -> anyhow::Result<TcpListener> {
    let domain = if addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    if matches!(domain, Domain::IPV6) {
        socket.set_only_v6(v6_only)?;
    }
    socket.bind(&addr.into())?;
    socket.listen(128)?;
    let listener: std::net::TcpListener = socket.into();
    listener.set_nonblocking(true)?;
    Ok(TcpListener::from_std(listener)?)
}

#[derive(Clone)]
struct PortServer {
    mapping: MappingSpec,
    docker: DockerBackend,
    authz: Arc<Authz>,
}

impl PortServer {
    fn new(mapping: MappingSpec, docker: DockerBackend, authz: Arc<Authz>) -> Self {
        Self {
            mapping,
            docker,
            authz,
        }
    }
}

impl server::Server for PortServer {
    type Handler = PortHandler;

    fn new_client(&mut self, _peer_addr: Option<std::net::SocketAddr>) -> Self::Handler {
        PortHandler {
            mapping: self.mapping.clone(),
            docker: self.docker.clone(),
            authz: self.authz.clone(),
            channels: HashMap::new(),
        }
    }

    fn handle_session_error(&mut self, error: <Self::Handler as server::Handler>::Error) {
        eprintln!("session error on port {}: {error:#}", self.mapping.port);
    }
}

struct PortHandler {
    mapping: MappingSpec,
    docker: DockerBackend,
    authz: Arc<Authz>,
    channels: HashMap<ChannelId, ChannelState>,
}

struct ChannelState {
    channel: Channel<Msg>,
    pty_requested: bool,
}

impl server::Handler for PortHandler {
    type Error = anyhow::Error;

    async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
        if self.authz.insecure_allow_none {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn auth_publickey_offered(
        &mut self,
        _user: &str,
        public_key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        if self.authz.allows(public_key)? {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn auth_publickey(
        &mut self,
        _user: &str,
        public_key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        if self.authz.allows(public_key)? {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        self.channels.insert(
            channel.id(),
            ChannelState {
                channel,
                pty_requested: false,
            },
        );
        Ok(true)
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some(state) = self.channels.get_mut(&channel) else {
            bail!("missing channel state for pty request");
        };
        state.pty_requested = true;
        session.channel_success(channel)?;
        Ok(())
    }

    async fn env_request(
        &mut self,
        channel: ChannelId,
        _variable_name: &str,
        _variable_value: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some(state) = self.channels.remove(&channel) else {
            bail!("missing channel state for shell request");
        };
        let mapping = self.mapping.clone();
        session.channel_success(channel)?;
        tokio::spawn(async move {
            if let Err(err) = spawn_container_shell(state.channel, mapping, state.pty_requested).await {
                eprintln!("shell task ended with error: {err:#}");
            }
        });
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).trim().to_string();
        let Some(state) = self.channels.remove(&channel) else {
            bail!("missing channel state for exec request");
        };

        if docker_authorized_command(&command) {
            let target = ProxyTarget::resolve(self.docker.clone(), &self.mapping.container).await?;
            session.channel_success(channel)?;
            tokio::spawn(async move {
                if let Err(err) = serve_channel_stream(state.channel.into_stream(), target).await {
                    eprintln!("proxy task ended with error: {err:#}");
                }
            });
        } else {
            let mapping = self.mapping.clone();
            session.channel_success(channel)?;
            tokio::spawn(async move {
                if let Err(err) =
                    spawn_container_exec(state.channel, mapping, state.pty_requested, command).await
                {
                    eprintln!("exec task ended with error: {err:#}");
                }
            });
        }
        Ok(())
    }
}

async fn spawn_container_shell(
    channel: Channel<Msg>,
    mapping: MappingSpec,
    pty_requested: bool,
) -> anyhow::Result<()> {
    let shell = resolve_container_shell(&mapping).await?;
    let mut cmd = docker_exec_base_command(&mapping.container, pty_requested);
    cmd.arg(shell);
    run_attached_process(channel, cmd).await
}

async fn spawn_container_exec(
    channel: Channel<Msg>,
    mapping: MappingSpec,
    pty_requested: bool,
    shell_command: String,
) -> anyhow::Result<()> {
    let shell = resolve_container_shell(&mapping).await?;
    let mut cmd = docker_exec_base_command(&mapping.container, pty_requested);
    cmd.arg(shell).arg("-c").arg(shell_command);
    run_attached_process(channel, cmd).await
}

async fn resolve_container_shell(mapping: &MappingSpec) -> anyhow::Result<String> {
    if let Some(shell) = mapping.shell.clone() {
        validate_shell_candidate(&mapping.container, &shell).await?;
        return Ok(shell);
    }

    for shell in ["/bin/bash", "/bin/sh", "sh"] {
        if validate_shell_candidate(&mapping.container, shell).await.is_ok() {
            return Ok(shell.to_string());
        }
    }

    bail!(
        "could not determine shell for container {}; configure one with `d2s config set {} {} --shell <path>`",
        mapping.container,
        mapping.port,
        mapping.container
    )
}

async fn validate_shell_candidate(container: &str, shell: &str) -> anyhow::Result<()> {
    let mut cmd = docker_exec_base_command(container, false);
    cmd.arg(shell).arg("-c").arg("exit 0");
    let status = cmd
        .status()
        .await
        .with_context(|| format!("failed to probe shell {shell}"))?;
    if status.success() {
        Ok(())
    } else {
        bail!("shell probe failed for {shell}");
    }
}

fn docker_exec_base_command(container: &str, pty_requested: bool) -> Command {
    let bin = env::var_os("D2S_DOCKER_BIN").unwrap_or_else(|| "docker".into());
    let mut cmd = Command::new(bin);
    cmd.arg("exec").arg("-i");
    if pty_requested {
        cmd.arg("-t");
    }
    cmd.arg(container);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd
}

async fn run_attached_process(channel: Channel<Msg>, mut cmd: Command) -> anyhow::Result<()> {
    let mut child = cmd.spawn().context("failed to spawn docker exec")?;
    let mut child_stdin = child.stdin.take().context("missing child stdin")?;
    let mut child_stdout = child.stdout.take().context("missing child stdout")?;
    let mut child_stderr = child.stderr.take().context("missing child stderr")?;

    let (mut read_half, write_half) = channel.split();
    let mut writer = write_half.make_writer();
    let mut stderr_writer = write_half.make_writer_ext(Some(1));

    let stdin_task = async {
        while let Some(msg) = read_half.wait().await {
            match msg {
                russh::ChannelMsg::Data { data } => {
                    child_stdin.write_all(&data).await?;
                }
                russh::ChannelMsg::Eof | russh::ChannelMsg::Close => {
                    break;
                }
                russh::ChannelMsg::WindowAdjusted { .. } => {}
                _ => {}
            }
        }
        child_stdin.shutdown().await?;
        Ok::<(), anyhow::Error>(())
    };
    let stdout_task = async {
        let _ = tokio::io::copy(&mut child_stdout, &mut writer).await?;
        Ok::<(), anyhow::Error>(())
    };
    let stderr_task = async {
        let _ = tokio::io::copy(&mut child_stderr, &mut stderr_writer).await?;
        Ok::<(), anyhow::Error>(())
    };

    let (stdin_res, stdout_res, stderr_res, status_res) =
        tokio::join!(stdin_task, stdout_task, stderr_task, child.wait());

    stdin_res?;
    stdout_res?;
    stderr_res?;
    let status = status_res?;
    let code = status.code().unwrap_or(255).max(0) as u32;

    let _ = writer.shutdown().await;
    let _ = stderr_writer.shutdown().await;
    let _ = write_half.exit_status(code).await;
    let _ = write_half.eof().await;
    let _ = write_half.close().await;
    Ok(())
}

struct Authz {
    insecure_allow_none: bool,
    allowed_keys: HashSet<String>,
}

impl Authz {
    async fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        match path {
            Some(path) => {
                let raw = fs::read_to_string(path)
                    .await
                    .with_context(|| format!("failed to read authorized_keys: {}", path.display()))?;
                let mut allowed_keys = HashSet::new();
                for line in raw.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    let key = PublicKey::from_openssh(line)
                        .with_context(|| format!("invalid authorized key line: {line}"))?;
                    allowed_keys.insert(key.to_openssh()?);
                }
                Ok(Self {
                    insecure_allow_none: false,
                    allowed_keys,
                })
            }
            None => Ok(Self {
                insecure_allow_none: true,
                allowed_keys: HashSet::new(),
            }),
        }
    }

    fn allows(&self, key: &PublicKey) -> anyhow::Result<bool> {
        if self.insecure_allow_none {
            return Ok(true);
        }
        Ok(self.allowed_keys.contains(&key.to_openssh()?))
    }
}

fn resolve_host_key_path(config_path: &Path, configured: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(path) = configured {
        return Ok(path.to_path_buf());
    }
    let parent = config_path
        .parent()
        .context("config file must have a parent directory")?;
    Ok(parent.join("host_key_ed25519"))
}

async fn load_or_generate_host_key(path: &Path) -> anyhow::Result<PrivateKey> {
    if fs::metadata(path).await.is_ok() {
        let pem = fs::read_to_string(path).await?;
        return Ok(PrivateKey::from_openssh(&pem)?);
    }

    let key = PrivateKey::random(&mut rand::rng(), keys::Algorithm::Ed25519)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    fs::write(path, key.to_openssh(LineEnding::LF)?).await?;
    Ok(key)
}

pub mod doctor {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    pub async fn run(
        active: &[ActiveMapping],
        host: &str,
        user: &str,
        identity: Option<&Path>,
    ) -> anyhow::Result<()> {
        if active.is_empty() {
            println!("no active mappings");
            return Ok(());
        }

        let mut failed = false;
        for mapping in active {
            match ping_mapping(host, user, identity, mapping.port).await {
                Ok(()) => println!("ok   {} -> {}", mapping.port, mapping.container_ref),
                Err(err) => {
                    failed = true;
                    println!("fail {} -> {}: {}", mapping.port, mapping.container_ref, err);
                }
            }
        }

        if failed {
            bail!("doctor found unreachable mappings");
        }
        Ok(())
    }

    async fn ping_mapping(
        host: &str,
        user: &str,
        identity: Option<&Path>,
        port: u16,
    ) -> anyhow::Result<()> {
        let config = Arc::new(russh::client::Config::default());
        let mut session = russh::client::connect(config, format!("{host}:{port}"), Client {})
            .await
            .with_context(|| format!("connect failed on {host}:{port}"))?;

        if let Some(identity) = identity {
            let pem = fs::read_to_string(identity).await?;
            let key = Arc::new(PrivateKey::from_openssh(&pem)?);
            let ok = session
                .authenticate_publickey(
                    user,
                    PrivateKeyWithHashAlg::new(
                        key,
                        session.best_supported_rsa_hash().await?.flatten(),
                    ),
                )
                .await?
                .success();
            if !ok {
                bail!("public key authentication failed");
            }
        } else {
            let ok = session.authenticate_none(user).await?.success();
            if !ok {
                bail!("none authentication failed; pass --identity or remove authorized_keys");
            }
        }

        let channel = session.channel_open_session().await?;
        channel.exec(true, "docker system dial-stdio").await?;
        let mut stream = channel.into_stream();
        stream
            .write_all(b"GET /_ping HTTP/1.0\r\nHost: docker\r\n\r\n")
            .await?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await?;
        let body = String::from_utf8_lossy(&buf);
        if !body.contains("200 OK") || !body.contains("\r\n\r\nOK") {
            bail!("unexpected docker ping response: {body}");
        }
        Ok(())
    }

    struct Client;

    impl russh::client::Handler for Client {
        type Error = anyhow::Error;

        async fn check_server_key(&mut self, _server_public_key: &PublicKey) -> Result<bool, Self::Error> {
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use http_body_util::Full;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use std::os::unix::fs::PermissionsExt;
    use std::net::TcpListener;
    use std::sync::{Mutex, OnceLock};
    use tokio::net::UnixListener;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    async fn spawn_test_server(shell_override: Option<&str>) -> (std::net::SocketAddr, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let docker_bin = tmp.path().join("docker");
        std::fs::write(
            &docker_bin,
            r#"#!/bin/sh
if [ "$1" != "exec" ]; then
  echo "unexpected docker verb: $1" >&2
  exit 2
fi
shift
while [ $# -gt 0 ]; do
  case "$1" in
    -i|-t)
      shift
      ;;
    *)
      container="$1"
      shift
      break
      ;;
  esac
done
if [ "$container" != "my-container" ]; then
  echo "unexpected container: $container" >&2
  exit 3
fi
if [ "$1" = "/bin/bash" ] || [ "$1" = "/bin/sh" ] || [ "$1" = "sh" ]; then
  shell="$1"
  shift
  if [ $# -eq 0 ]; then
    cat
    exit 0
  fi
  if [ "$1" = "-c" ]; then
    shift
    /bin/sh -lc "$1"
    exit $?
  fi
fi
if [ "$1" = "sh" ] && [ $# -eq 1 ]; then
  cat
  exit 0
fi
if [ "$1" = "sh" ] && [ "$2" = "-lc" ]; then
  shift 2
  /bin/sh -lc "$1"
  exit $?
fi
echo "unexpected args: $*" >&2
exit 4
"#,
        )
        .unwrap();
        std::fs::set_permissions(&docker_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let addr = TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap();
        let host_key = PrivateKey::random(&mut rand::rng(), keys::Algorithm::Ed25519).unwrap();
        let authz = Arc::new(Authz {
            insecure_allow_none: true,
            allowed_keys: HashSet::new(),
        });
        let server = PortServer::new(
            MappingSpec {
                port: addr.port(),
                container: "my-container".to_string(),
                shell: shell_override.map(str::to_string),
            },
            DockerBackend::from_socket_path(tmp.path().join("unused.sock")),
            authz,
        );
        let mut task_server = server.clone();
        let server_cfg = Arc::new(server::Config {
            auth_rejection_time: Duration::from_secs(1),
            auth_rejection_time_initial: Some(Duration::from_secs(0)),
            keys: vec![host_key],
            ..Default::default()
        });

        unsafe {
            std::env::set_var("D2S_DOCKER_BIN", &docker_bin);
        }
        tokio::spawn(async move {
            task_server.run_on_address(server_cfg, addr).await.unwrap();
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        (addr, tmp)
    }

    async fn connect_test_client(addr: std::net::SocketAddr) -> russh::client::Handle<Client> {
        let config = Arc::new(russh::client::Config::default());
        let mut session = russh::client::connect(config, addr, Client {}).await.unwrap();
        let ok = session.authenticate_none("root").await.unwrap().success();
        assert!(ok);
        session
    }

    struct Client;

    impl russh::client::Handler for Client {
        type Error = anyhow::Error;

        async fn check_server_key(&mut self, _server_public_key: &PublicKey) -> Result<bool, Self::Error> {
            Ok(true)
        }
    }

    #[tokio::test]
    async fn doctor_round_trip_over_ssh() {
        let tmp = tempfile::tempdir().unwrap();
        let docker_sock = tmp.path().join("docker.sock");
        let unix_listener = UnixListener::bind(&docker_sock).unwrap();

        tokio::spawn(async move {
            for _ in 0..2 {
                let (stream, _) = unix_listener.accept().await.unwrap();
                let service = service_fn(|req: Request<hyper::body::Incoming>| async move {
                    let path = req.uri().path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
                    let body = if path == "/containers/my-container/json" {
                        Bytes::from_static(
                            br#"{"Id":"abc123def456","Name":"/my-container","State":{"Running":true}}"#,
                        )
                    } else {
                        Bytes::from_static(b"OK")
                    };
                    Ok::<_, std::convert::Infallible>(Response::new(Full::new(body)))
                });
                let _ = http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                    .await;
            }
        });

        let addr = TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap();
        let host_key = PrivateKey::random(&mut rand::rng(), keys::Algorithm::Ed25519).unwrap();
        let authz = Arc::new(Authz {
            insecure_allow_none: true,
            allowed_keys: HashSet::new(),
        });
        let server = PortServer::new(
            MappingSpec {
                port: addr.port(),
                container: "my-container".to_string(),
                shell: None,
            },
            DockerBackend::from_socket_path(docker_sock.clone()),
            authz,
        );
        let mut task_server = server.clone();
        let server_cfg = Arc::new(server::Config {
            auth_rejection_time: Duration::from_secs(1),
            auth_rejection_time_initial: Some(Duration::from_secs(0)),
            keys: vec![host_key],
            ..Default::default()
        });
        tokio::spawn(async move {
            task_server.run_on_address(server_cfg, addr).await.unwrap();
        });

        tokio::time::sleep(Duration::from_millis(100)).await;
        doctor::run(
            &[ActiveMapping {
                port: addr.port(),
                container_ref: "my-container".to_string(),
                container_id_short: "abc123".to_string(),
            }],
            "127.0.0.1",
            "docker",
            None,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn wildcard_listen_host_creates_dual_stack_listeners() {
        let listeners = build_listeners("0.0.0.0", 0).unwrap();
        assert_eq!(listeners.len(), 2);
        assert_eq!(listeners[0].0, "0.0.0.0:0");
        assert_eq!(listeners[1].0, "[::]:0");
        assert!(listeners[0].1.local_addr().unwrap().is_ipv4());
        assert!(listeners[1].1.local_addr().unwrap().is_ipv6());
    }

    #[tokio::test]
    async fn bracketed_ipv6_listen_host_is_supported() {
        let listeners = build_listeners("[::1]", 0).unwrap();
        assert_eq!(listeners.len(), 1);
        assert_eq!(listeners[0].0, "[::1]:0");
        assert!(listeners[0].1.local_addr().unwrap().is_ipv6());
    }

    #[tokio::test]
    async fn shell_request_attaches_to_container() {
        let _guard = env_lock().lock().unwrap();
        let (addr, _tmp) = spawn_test_server(None).await;
        let session = connect_test_client(addr).await;
        let mut channel = session.channel_open_session().await.unwrap();
        channel
            .request_pty(true, "xterm", 80, 24, 0, 0, &[])
            .await
            .unwrap();
        channel.request_shell(true).await.unwrap();

        let mut writer = channel.make_writer();
        writer.write_all(b"hello from shell\n").await.unwrap();
        writer.shutdown().await.unwrap();

        let mut reader = channel.make_reader();
        let mut out = Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        assert!(String::from_utf8_lossy(&out).contains("hello from shell"));
        unsafe {
            std::env::remove_var("D2S_DOCKER_BIN");
        }
    }

    #[tokio::test]
    async fn exec_request_runs_in_container() {
        let _guard = env_lock().lock().unwrap();
        let (addr, _tmp) = spawn_test_server(None).await;
        let session = connect_test_client(addr).await;
        let channel = session.channel_open_session().await.unwrap();
        channel.exec(true, "printf exec-ok").await.unwrap();

        let mut reader = channel.into_stream();
        let mut out = Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        assert!(String::from_utf8_lossy(&out).contains("exec-ok"));
        unsafe {
            std::env::remove_var("D2S_DOCKER_BIN");
        }
    }

    #[tokio::test]
    async fn configured_shell_override_is_used() {
        let _guard = env_lock().lock().unwrap();
        let (addr, _tmp) = spawn_test_server(Some("/bin/bash")).await;
        let session = connect_test_client(addr).await;
        let channel = session.channel_open_session().await.unwrap();
        channel.exec(true, "printf shell-override").await.unwrap();

        let mut reader = channel.into_stream();
        let mut out = Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        assert!(String::from_utf8_lossy(&out).contains("shell-override"));
        unsafe {
            std::env::remove_var("D2S_DOCKER_BIN");
        }
    }
}
