mod ipc;
mod server;
mod ssh;

use anyhow::{Context, Result};
use clap::Parser;
use ipc::{HostMsg, PluginMsg};
use russh::{
    MethodKind, MethodSet,
    keys::{Algorithm, PrivateKey, ssh_key::LineEnding},
    server::Server as _,
};
use server::{AppState, SshServer};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
    net::TcpListener,
    sync::{broadcast, mpsc},
};

const DEFAULT_MAX_LINE_BYTES: usize = 8192;
const DEFAULT_PORT: u16 = 2222;

#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Args {
    /// Address to bind, for example 0.0.0.0:2222.
    #[arg(long, value_name = "ADDR")]
    pub bind: Option<SocketAddr>,

    /// Host/IP to bind when --bind is not used.
    #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::UNSPECIFIED))]
    pub host: IpAddr,

    /// TCP port to listen on.
    #[arg(long, default_value_t = DEFAULT_PORT)]
    pub port: u16,

    /// Path to the persistent SSH host key. Generated on first run if absent.
    #[arg(long, value_name = "PATH")]
    pub host_key: Option<PathBuf>,

    /// Maximum bytes accepted for one incoming line before it is dropped.
    #[arg(long, default_value_t = DEFAULT_MAX_LINE_BYTES)]
    pub max_line_bytes: usize,

    /// Maximum simultaneous SSH sessions.
    #[arg(long, default_value_t = 128)]
    pub max_connections: usize,

    /// Payload limit reported to Supply Drop. SSH defaults to unlimited.
    #[arg(long, default_value_t = 0)]
    pub payload_limit: u32,

    /// Append CRLF to outbound BBS messages that do not already end in a newline.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub append_newline: bool,

    /// Allow the SSH "none" auth method, letting clients in without a password
    /// or key. The BBS still runs its own login flow over the terminal.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub allow_none_auth: bool,

    /// Drop sessions that send no data for this many seconds. 0 disables.
    #[arg(long, default_value_t = 3600)]
    pub inactivity_timeout: u64,

    /// Grace period for flushing BBS output after a client sends EOF. This
    /// matters for non-interactive use such as `ssh bbs help` or piped input,
    /// where EOF arrives before the BBS has answered.
    #[arg(long, default_value_t = 750)]
    pub eof_drain_ms: u64,
}

impl Args {
    fn listen_addr(&self) -> SocketAddr {
        self.bind
            .unwrap_or_else(|| SocketAddr::new(self.host, self.port))
    }

    /// Resolve the host key path, falling back to a per-user data directory so
    /// the same key is reused across restarts and clients do not see host key
    /// mismatch warnings.
    fn host_key_path(&self) -> PathBuf {
        if let Some(path) = &self.host_key {
            return path.clone();
        }

        if let Ok(dir) =
            std::env::var("STATE_DIRECTORY").or_else(|_| std::env::var("XDG_DATA_HOME"))
            && !dir.is_empty()
        {
            return Path::new(&dir).join("supply-drop-ssh/host_key");
        }

        if let Ok(home) = std::env::var("HOME")
            && !home.is_empty()
        {
            return Path::new(&home).join(".local/share/supply-drop-ssh/host_key");
        }

        PathBuf::from("supply-drop-ssh-host-key")
    }

    fn inactivity(&self) -> Option<Duration> {
        if self.inactivity_timeout == 0 {
            None
        } else {
            Some(Duration::from_secs(self.inactivity_timeout))
        }
    }

    fn method_set(&self) -> MethodSet {
        let mut methods = vec![
            MethodKind::PublicKey,
            MethodKind::Password,
            MethodKind::KeyboardInteractive,
        ];
        if self.allow_none_auth {
            methods.insert(0, MethodKind::None);
        }
        MethodSet::from(methods.as_slice())
    }
}

/// Load the host key from disk, generating and persisting one on first run.
fn load_or_create_host_key(path: &Path) -> Result<PrivateKey> {
    if path.exists() {
        let key = PrivateKey::read_openssh_file(path)
            .with_context(|| format!("failed to read host key {}", path.display()))?;
        eprintln!("loaded ssh host key from {}", path.display());
        return Ok(key);
    }

    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
        .map_err(|err| anyhow::anyhow!("failed to generate host key: {err}"))?;
    key.write_openssh_file(path, LineEnding::LF)
        .with_context(|| format!("failed to write host key {}", path.display()))?;

    // The host key is private material; keep it owner-readable only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }

    eprintln!("generated new ssh host key at {}", path.display());
    Ok(key)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Arc::new(Args::parse());
    run(args).await
}

async fn run(args: Arc<Args>) -> Result<()> {
    let host_key = load_or_create_host_key(&args.host_key_path())?;

    let config = Arc::new(russh::server::Config {
        inactivity_timeout: args.inactivity(),
        auth_rejection_time: Duration::from_secs(1),
        auth_rejection_time_initial: Some(Duration::from_secs(0)),
        methods: args.method_set(),
        keys: vec![host_key],
        nodelay: true,
        ..Default::default()
    });

    let listener = TcpListener::bind(args.listen_addr())
        .await
        .with_context(|| format!("failed to bind {}", args.listen_addr()))?;
    eprintln!("supply-drop-ssh listening on {}", listener.local_addr()?);

    let (to_host, host_rx) = mpsc::channel::<PluginMsg>(1024);
    let state = AppState::new(to_host.clone(), args.clone());
    let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

    tokio::spawn(stdout_writer(host_rx));
    tokio::spawn(stdin_reader(state.clone(), shutdown_tx.clone()));

    to_host
        .send(PluginMsg::Ready {
            payload_limit: Some(args.payload_limit),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        })
        .await
        .context("stdout writer stopped before ready")?;

    let mut ssh_server = SshServer::new(state.clone());
    let running = ssh_server.run_on_socket(config, &listener);
    let running_handle = running.handle();

    tokio::select! {
        result = running => {
            if let Err(err) = result {
                eprintln!("ssh server stopped: {err}");
            }
        }
        _ = shutdown_rx.recv() => {
            running_handle.shutdown("supply drop plugin shutting down".to_owned());
        }
        signal = tokio::signal::ctrl_c() => {
            if let Err(err) = signal {
                eprintln!("ctrl-c handler error: {err}");
            }
            running_handle.shutdown("supply drop plugin shutting down".to_owned());
        }
    }

    eprintln!("shutting down ssh listener");
    state.shutdown_connections().await;
    let _ = shutdown_tx.send(());
    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok(())
}

async fn stdout_writer(mut rx: mpsc::Receiver<PluginMsg>) {
    let mut stdout = BufWriter::new(tokio::io::stdout());
    while let Some(msg) = rx.recv().await {
        match serde_json::to_vec(&msg) {
            Ok(encoded) => {
                if stdout.write_all(&encoded).await.is_err()
                    || stdout.write_all(b"\n").await.is_err()
                    || stdout.flush().await.is_err()
                {
                    eprintln!("stdout closed while writing IPC message");
                    break;
                }
            }
            Err(err) => eprintln!("failed to encode IPC message: {err}"),
        }
    }
}

async fn stdin_reader(state: AppState, shutdown_tx: broadcast::Sender<()>) {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();

    loop {
        match lines.next_line().await {
            Ok(Some(raw)) => {
                if raw.trim().is_empty() {
                    continue;
                }

                match serde_json::from_str::<HostMsg>(&raw) {
                    Ok(HostMsg::Send {
                        id,
                        text,
                        hide_input,
                    }) => {
                        state
                            .send_to_connection(&id, text, hide_input.unwrap_or(false))
                            .await
                    }
                    Ok(HostMsg::Kick { id }) => state.kick_connection(&id).await,
                    Ok(HostMsg::Shutdown) => {
                        state.shutdown_connections().await;
                        let _ = shutdown_tx.send(());
                        break;
                    }
                    Err(err) => eprintln!("bad json from Supply Drop: {err}: {raw:?}"),
                }
            }
            Ok(None) => {
                let _ = shutdown_tx.send(());
                break;
            }
            Err(err) => {
                eprintln!("stdin read error: {err}");
                let _ = shutdown_tx.send(());
                break;
            }
        }
    }
}
