use crate::config::ConfigStore;
use crate::docker::DockerBackend;
use crate::ssh_server::ServeManager;
use anyhow::Context as _;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "d2s",
    about = "docker2ssh: expose SSH ports that speak Docker protocol"
)]
pub struct Cli {
    /// Path to config file (TOML). Defaults to ~/.config/d2s/config.toml (or platform equivalent).
    #[arg(long)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Run the SSH server(s) based on active config.
    Serve,

    /// Manage port<->container mappings.
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },

    /// Show active mappings (only running containers).
    Show,

    /// Verify active mappings are reachable via SSH + Docker _ping.
    Doctor {
        /// SSH username for doctor connection (doesn't need to exist on server).
        #[arg(long, default_value = "docker")]
        user: String,

        /// Optional private key for doctor SSH auth (OpenSSH format).
        /// If not provided and no authorized_keys is configured, doctor runs in "insecure" mode.
        #[arg(long)]
        identity: Option<PathBuf>,

        /// Host to connect to for doctor checks.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Add or update a mapping.
    Set {
        port: u16,
        container: String,
        /// Optional shell inside the container, e.g. /bin/bash.
        #[arg(long)]
        shell: Option<String>,
        /// Clear any previously configured shell override.
        #[arg(long)]
        clear_shell: bool,
    },
    /// Remove a mapping by port.
    Rm { port: u16 },
    /// List all configured mappings (includes stopped containers).
    List,
}

impl Cli {
    pub async fn run(self) -> anyhow::Result<()> {
        let store = ConfigStore::load_or_create(self.config.clone()).await?;

        match self.cmd.unwrap_or(Cmd::Serve) {
            Cmd::Serve => {
                let cfg = store.load().await?;
                let docker = DockerBackend::from_env_or_config(&cfg)?;
                ServeManager::new(store, docker).run().await?;
            }
            Cmd::Config { cmd } => match cmd {
                ConfigCmd::Set {
                    port,
                    container,
                    shell,
                    clear_shell,
                } => {
                    if shell.is_some() && clear_shell {
                        anyhow::bail!("--shell and --clear-shell cannot be used together");
                    }
                    let cfg = store.load().await?;
                    let docker = DockerBackend::from_env_or_config(&cfg)?;
                    // Validate container exists (running or stopped).
                    docker
                        .inspect_container(&container)
                        .await
                        .with_context(|| format!("container not found: {container}"))?;

                    let shell = if clear_shell {
                        Some(None)
                    } else {
                        shell.map(Some)
                    };
                    store.upsert_mapping(port, container, shell).await?;
                }
                ConfigCmd::Rm { port } => {
                    store.remove_mapping(port).await?;
                }
                ConfigCmd::List => {
                    let cfg = store.load().await?;
                    for m in cfg.mappings {
                        match m.shell {
                            Some(shell) => {
                                println!("{} -> {} [shell={}]", m.port, m.container, shell)
                            }
                            None => println!("{} -> {}", m.port, m.container),
                        }
                    }
                }
            },
            Cmd::Show => {
                let cfg = store.load().await?;
                let docker = DockerBackend::from_env_or_config(&cfg)?;
                let active = docker.active_mappings(&cfg.mappings).await?;
                for a in active {
                    println!(
                        "{} -> {} ({})",
                        a.port, a.container_ref, a.container_id_short
                    );
                }
            }
            Cmd::Doctor {
                user,
                identity,
                host,
            } => {
                let cfg = store.load().await?;
                let docker = DockerBackend::from_env_or_config(&cfg)?;
                let active = docker.active_mappings(&cfg.mappings).await?;
                crate::ssh_server::doctor::run(&active, &host, &user, identity.as_deref()).await?;
            }
        }

        Ok(())
    }
}
