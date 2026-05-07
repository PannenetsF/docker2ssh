use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    /// Host/IP to bind SSH listeners to.
    #[serde(default = "default_listen_host")]
    pub listen_host: String,

    /// Docker Unix socket path on the host.
    /// You can override via env `D2S_DOCKER_SOCKET` or `D2S_DOCKER_ENDPOINT`.
    #[serde(default = "default_docker_socket")]
    pub docker_socket: String,

    /// Optional OpenSSH authorized_keys file path. If absent, server accepts any key (unsafe).
    #[serde(default)]
    pub authorized_keys: Option<PathBuf>,

    /// OpenSSH private host key path (ed25519). If missing, auto-generated.
    #[serde(default)]
    pub host_key: Option<PathBuf>,

    #[serde(default)]
    pub mappings: Vec<MappingSpec>,
}

fn default_listen_host() -> String {
    "0.0.0.0".to_string()
}

fn default_docker_socket() -> String {
    "/var/run/docker.sock".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappingSpec {
    pub port: u16,
    /// Container name or id.
    pub container: String,
    /// Optional shell binary inside the container, e.g. /bin/bash.
    #[serde(default)]
    pub shell: Option<String>,
}

pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub async fn load_or_create(path_override: Option<PathBuf>) -> anyhow::Result<Self> {
        let path = match path_override {
            Some(p) => p,
            None => default_config_path()?,
        };

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        if fs::metadata(&path).await.is_err() {
            let cfg = ConfigFile {
                listen_host: default_listen_host(),
                docker_socket: default_docker_socket(),
                authorized_keys: None,
                host_key: None,
                mappings: vec![],
            };
            let s = toml::to_string_pretty(&cfg)?;
            fs::write(&path, s).await?;
        }

        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn load(&self) -> anyhow::Result<ConfigFile> {
        let raw = fs::read_to_string(&self.path).await?;
        let cfg: ConfigFile =
            toml::from_str(&raw).with_context(|| format!("invalid TOML: {}", self.path.display()))?;
        Ok(cfg)
    }

    pub async fn save(&self, cfg: &ConfigFile) -> anyhow::Result<()> {
        let s = toml::to_string_pretty(cfg)?;
        fs::write(&self.path, s).await?;
        Ok(())
    }

    pub async fn upsert_mapping(
        &self,
        port: u16,
        container: String,
        shell: Option<Option<String>>,
    ) -> anyhow::Result<()> {
        let mut cfg = self.load().await?;
        if let Some(m) = cfg.mappings.iter_mut().find(|m| m.port == port) {
            m.container = container;
            if let Some(shell) = shell {
                m.shell = shell;
            }
        } else {
            cfg.mappings.push(MappingSpec {
                port,
                container,
                shell: shell.unwrap_or(None),
            });
            cfg.mappings.sort_by_key(|m| m.port);
        }
        self.save(&cfg).await?;
        Ok(())
    }

    pub async fn remove_mapping(&self, port: u16) -> anyhow::Result<()> {
        let mut cfg = self.load().await?;
        cfg.mappings.retain(|m| m.port != port);
        self.save(&cfg).await?;
        Ok(())
    }
}

fn default_config_path() -> anyhow::Result<PathBuf> {
    let base = dirs::config_dir().context("could not determine config dir")?;
    Ok(base.join("d2s").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn upsert_and_remove_mapping() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let store = ConfigStore::load_or_create(Some(path)).await.unwrap();

        store
            .upsert_mapping(2222, "abc".to_string(), Some(Some("/bin/bash".to_string())))
            .await
            .unwrap();
        store
            .upsert_mapping(2223, "def".to_string(), None)
            .await
            .unwrap();
        store
            .upsert_mapping(2222, "zzz".to_string(), None)
            .await
            .unwrap();

        let cfg = store.load().await.unwrap();
        assert_eq!(cfg.mappings.len(), 2);
        assert_eq!(cfg.mappings[0].port, 2222);
        assert_eq!(cfg.mappings[0].container, "zzz");
        assert_eq!(cfg.mappings[0].shell.as_deref(), Some("/bin/bash"));

        store.remove_mapping(2222).await.unwrap();
        let cfg = store.load().await.unwrap();
        assert_eq!(cfg.mappings.len(), 1);
        assert_eq!(cfg.mappings[0].port, 2223);
    }
}
