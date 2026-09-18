//! SSH server plumbing: connection handler, authentication, and the bridge
//! between SSH channels and Supply Drop's IPC protocol.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use russh::{
    Channel, ChannelId, Pty,
    server::{Auth, Handle, Msg, Session},
};
use tokio::sync::{Mutex, mpsc};

use crate::{
    Args,
    ipc::PluginMsg,
    ssh::{LineEditor, LineEvent, text_to_wire},
};

/// Commands the host (Supply Drop) can issue against a live session.
#[derive(Debug)]
pub enum ConnectionCommand {
    Send { text: String, hide_input: bool },
    Kick,
    Shutdown,
}

#[derive(Clone)]
pub struct ConnectionHandle {
    pub tx: mpsc::Sender<ConnectionCommand>,
}

/// State shared by every SSH connection.
#[derive(Clone)]
pub struct AppState {
    pub connections: Arc<Mutex<HashMap<String, ConnectionHandle>>>,
    pub to_host: mpsc::Sender<PluginMsg>,
    pub args: Arc<Args>,
    pub conn_counter: Arc<AtomicU64>,
}

impl AppState {
    pub fn new(to_host: mpsc::Sender<PluginMsg>, args: Arc<Args>) -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            to_host,
            args,
            conn_counter: Arc::new(AtomicU64::new(1)),
        }
    }

    pub async fn connection_count(&self) -> usize {
        self.connections.lock().await.len()
    }

    pub async fn send_to_connection(&self, id: &str, text: String, hide_input: bool) {
        let tx = self
            .connections
            .lock()
            .await
            .get(id)
            .map(|conn| conn.tx.clone());

        if let Some(tx) = tx {
            if tx
                .send(ConnectionCommand::Send { text, hide_input })
                .await
                .is_err()
            {
                eprintln!("connection {id} command channel closed");
            }
        } else {
            eprintln!("send for unknown connection {id}");
        }
    }

    pub async fn kick_connection(&self, id: &str) {
        let removed = self.connections.lock().await.remove(id);
        if let Some(conn) = removed {
            let _ = conn.tx.send(ConnectionCommand::Kick).await;
        }
    }

    pub async fn shutdown_connections(&self) {
        let handles: Vec<_> = self.connections.lock().await.values().cloned().collect();
        for conn in handles {
            let _ = conn.tx.send(ConnectionCommand::Shutdown).await;
        }
    }
}

/// Factory that hands a fresh [`SessionHandler`] to each accepted connection.
#[derive(Clone)]
pub struct SshServer {
    state: AppState,
}

impl SshServer {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl russh::server::Server for SshServer {
    type Handler = SessionHandler;

    fn new_client(&mut self, peer_addr: Option<SocketAddr>) -> Self::Handler {
        SessionHandler {
            state: self.state.clone(),
            peer: peer_addr,
            user: None,
            session: None,
            pty_channels: Vec::new(),
        }
    }

    fn handle_session_error(&mut self, error: <Self::Handler as russh::server::Handler>::Error) {
        eprintln!("ssh session error: {error}");
    }
}

/// Per-channel state for a session that has been registered with Supply Drop.
struct ActiveSession {
    conn_id: String,
    channel: ChannelId,
    editor: LineEditor,
    /// Raised by the outbound pump when the BBS sends a `hide_input` prompt,
    /// and consumed by the reader before it echoes the next keystroke.
    hide_input: Arc<AtomicBool>,
    /// Retains the sender so the pump task stays alive for the session.
    _cmd_tx: mpsc::Sender<ConnectionCommand>,
}

/// Handles a single SSH connection.
pub struct SessionHandler {
    state: AppState,
    peer: Option<SocketAddr>,
    user: Option<String>,
    session: Option<ActiveSession>,
    /// Channels that were granted a PTY, and therefore need server-side echo.
    pty_channels: Vec<ChannelId>,
}

impl SessionHandler {
    fn peer_label(&self) -> String {
        self.peer
            .map(|addr| addr.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }

    /// Accept authentication, recording the username Supply Drop will see.
    fn accept_auth(&mut self, user: &str) -> Auth {
        self.user = Some(user.to_owned());
        Auth::Accept
    }

    /// Register the channel with Supply Drop and start the outbound pump.
    ///
    /// Called on `shell`/`exec` request rather than on channel open, because a
    /// PTY request arrives between the two and decides whether we echo.
    async fn start_session(
        &mut self,
        channel: ChannelId,
        handle: Handle,
        echo: bool,
    ) -> Result<(), russh::Error> {
        if self.session.is_some() {
            return Ok(());
        }

        let id_num = self.state.conn_counter.fetch_add(1, Ordering::Relaxed);
        let peer = self.peer_label();
        let conn_id = format!("ssh:{peer}:{id_num}");

        let (cmd_tx, cmd_rx) = mpsc::channel::<ConnectionCommand>(64);
        self.state
            .connections
            .lock()
            .await
            .insert(conn_id.clone(), ConnectionHandle { tx: cmd_tx.clone() });

        if self
            .state
            .to_host
            .send(PluginMsg::Open {
                id: conn_id.clone(),
            })
            .await
            .is_err()
        {
            eprintln!("{conn_id}: host channel closed before open");
            self.state.connections.lock().await.remove(&conn_id);
            return Err(russh::Error::Disconnect);
        }

        let user = self.user.clone().unwrap_or_else(|| "-".to_string());
        eprintln!("{conn_id}: connected from {peer} as {user:?} (echo={echo})");

        let editor = LineEditor::new(self.state.args.max_line_bytes, echo);
        let hide_input = Arc::new(AtomicBool::new(false));

        // Outbound pump: applies host commands to the SSH channel, and raises
        // the shared hide_input flag for password prompts.
        tokio::spawn(outbound_pump(
            conn_id.clone(),
            channel,
            handle,
            cmd_rx,
            self.state.args.clone(),
            hide_input.clone(),
        ));

        self.session = Some(ActiveSession {
            conn_id,
            channel,
            editor,
            hide_input,
            _cmd_tx: cmd_tx,
        });

        Ok(())
    }

    /// Tear the session down and notify Supply Drop exactly once.
    async fn finish_session(&mut self, session: &mut Session) {
        if let Some(active) = self.session.take() {
            let removed = self
                .state
                .connections
                .lock()
                .await
                .remove(&active.conn_id)
                .is_some();

            if removed {
                let _ = self
                    .state
                    .to_host
                    .send(PluginMsg::Close {
                        id: active.conn_id.clone(),
                    })
                    .await;
            }

            eprintln!("{}: disconnected", active.conn_id);
            let _ = session.eof(active.channel);
            let _ = session.exit_status_request(active.channel, 0);
            let _ = session.close(active.channel);
        }
    }
}

/// Forwards host `send`/`kick`/`shutdown` commands onto the SSH channel.
async fn outbound_pump(
    conn_id: String,
    channel: ChannelId,
    handle: Handle,
    mut cmd_rx: mpsc::Receiver<ConnectionCommand>,
    args: Arc<Args>,
    hide_input_flag: Arc<AtomicBool>,
) {
    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            ConnectionCommand::Send { text, hide_input } => {
                if hide_input {
                    // Tell the reader side to stop echoing the next line.
                    hide_input_flag.store(true, Ordering::SeqCst);
                }

                let bytes = text_to_wire(&text, args.append_newline);
                if handle.data(channel, bytes).await.is_err() {
                    eprintln!("{conn_id}: write error, channel closed");
                    break;
                }
            }
            ConnectionCommand::Kick | ConnectionCommand::Shutdown => {
                let _ = handle.eof(channel).await;
                let _ = handle.close(channel).await;
                break;
            }
        }
    }
}

impl russh::server::Handler for SessionHandler {
    type Error = russh::Error;

    /// Supply Drop performs its own login flow over the terminal, so the SSH
    /// layer only needs to establish an encrypted session. Every method is
    /// accepted and the credentials are not treated as BBS credentials.
    async fn auth_none(&mut self, user: &str) -> Result<Auth, Self::Error> {
        if self.state.args.allow_none_auth {
            Ok(self.accept_auth(user))
        } else {
            Ok(Auth::reject())
        }
    }

    async fn auth_password(&mut self, user: &str, _password: &str) -> Result<Auth, Self::Error> {
        Ok(self.accept_auth(user))
    }

    async fn auth_publickey_offered(
        &mut self,
        _user: &str,
        _public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        _public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(self.accept_auth(user))
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _submethods: &str,
        _response: Option<russh::server::Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        Ok(self.accept_auth(user))
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Enforce the connection cap before accepting more terminal sessions.
        if self.state.connection_count().await >= self.state.args.max_connections {
            eprintln!("rejecting {}: connection limit reached", self.peer_label());
            reply
                .reject(russh::ChannelOpenFailure::ResourceShortage)
                .await;
            return Ok(());
        }

        let _ = channel;
        reply.accept().await;
        Ok(())
    }

    /// A PTY means an interactive terminal: the client is in raw mode and we
    /// must echo. Recorded before `shell_request` arrives.
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if !self.pty_channels.contains(&channel) {
            self.pty_channels.push(channel);
        }
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let echo = self.pty_channels.contains(&channel);
        session.channel_success(channel)?;
        self.start_session(channel, session.handle(), echo).await?;
        Ok(())
    }

    /// `ssh host <command>` and piped input arrive as an exec request. There is
    /// no PTY, so echo stays off and the BBS sees a plain line stream.
    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let echo = self.pty_channels.contains(&channel);
        session.channel_success(channel)?;
        self.start_session(channel, session.handle(), echo).await?;

        // Treat the command string itself as the first line of input.
        if !data.is_empty() {
            let mut payload = data.to_vec();
            payload.push(b'\n');
            self.data(channel, &payload, session).await?;
        }

        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _channel: ChannelId,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some(active) = self.session.as_mut() else {
            return Ok(());
        };

        if active.channel != channel {
            return Ok(());
        }

        // Pick up a hide_input request raised by the outbound pump.
        if active.hide_input.swap(false, Ordering::SeqCst) {
            active.editor.set_hide_input(true);
        }

        let conn_id = active.conn_id.clone();
        let mut close_reason: Option<&str> = None;

        for event in active.editor.push(data) {
            match event {
                LineEvent::Echo(bytes) => {
                    session.data(channel, bytes)?;
                }
                LineEvent::Line(line) => {
                    if self
                        .state
                        .to_host
                        .send(PluginMsg::Recv {
                            id: conn_id.clone(),
                            line,
                        })
                        .await
                        .is_err()
                    {
                        eprintln!("{conn_id}: host channel closed on recv");
                        close_reason = Some("host closed");
                        break;
                    }
                }
                LineEvent::LineTooLong => {
                    eprintln!("{conn_id}: dropping overlong input line");
                    session.data(channel, b"\r\nLine too long.\r\n".to_vec())?;
                }
                LineEvent::Interrupt => {
                    close_reason = Some("interrupt");
                    break;
                }
                LineEvent::Eof => {
                    close_reason = Some("eof");
                    break;
                }
            }
        }

        if let Some(reason) = close_reason {
            eprintln!("{conn_id}: closing session ({reason})");
            self.finish_session(session).await;
        }

        Ok(())
    }

    /// The client will send no more input. This happens immediately for
    /// non-interactive invocations such as `ssh host help` or piped stdin, so
    /// we must not tear the session down right away: the BBS still has queued
    /// output to deliver. Allow a short drain window, then close.
    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some(active) = self.session.as_ref() else {
            return Ok(());
        };

        if active.channel != channel {
            return Ok(());
        }

        let handle = session.handle();
        let drain = Duration::from_millis(self.state.args.eof_drain_ms);
        tokio::spawn(async move {
            tokio::time::sleep(drain).await;
            let _ = handle.eof(channel).await;
            let _ = handle.close(channel).await;
        });

        Ok(())
    }

    async fn channel_close(
        &mut self,
        _channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.finish_session(session).await;
        Ok(())
    }
}

impl Drop for SessionHandler {
    fn drop(&mut self) {
        // The channel handlers normally clean up, but a dropped connection
        // (network loss, protocol error) must still release the slot and tell
        // Supply Drop the user is gone.
        if let Some(active) = self.session.take() {
            let state = self.state.clone();
            let conn_id = active.conn_id;
            tokio::spawn(async move {
                let removed = state.connections.lock().await.remove(&conn_id).is_some();
                if removed {
                    let _ = state
                        .to_host
                        .send(PluginMsg::Close {
                            id: conn_id.clone(),
                        })
                        .await;
                    eprintln!("{conn_id}: disconnected");
                }
            });
        }
    }
}
