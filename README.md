# Supply Drop SSH Transport Plugin

Standalone SSH process transport plugin for [Supply Drop BBS](https://supplydrop.meshamerica.com/).

Supply Drop is mesh-first but transport-agnostic. This plugin lets ordinary SSH clients connect to the BBS by translating SSH sessions into the Supply Drop process transport protocol over stdin/stdout.

Project site: <https://mesh-america.github.io/supply-drop-ssh-transport-plugin/>

## Quickstart

Full sysop quickstart: [QUICKSTART.md](QUICKSTART.md)

Maintainer checklist: [MAINTAINER_CHECKLIST.md](MAINTAINER_CHECKLIST.md)

Linux on Raspberry Pi OS, Ubuntu, or Debian.

Raspberry Pi / ARM64:

```sh
curl -fsSL -o supply-drop-ssh.deb \
  https://github.com/Mesh-America/supply-drop-ssh-transport-plugin/releases/latest/download/supply-drop-ssh-transport-plugin_arm64.deb
sudo dpkg -i supply-drop-ssh.deb
```

Intel / AMD64:

```sh
curl -fsSL -o supply-drop-ssh.deb \
  https://github.com/Mesh-America/supply-drop-ssh-transport-plugin/releases/latest/download/supply-drop-ssh-transport-plugin_amd64.deb
sudo dpkg -i supply-drop-ssh.deb
```

Supply Drop BBS v0.6.0 or newer is required. The Debian package installs the binary and registers it with Supply Drop:

```sh
supply-drop-bbs plugin add ssh /usr/bin/supply-drop-ssh
```

This creates `/etc/supply-drop-bbs/plugins.d/ssh.toml` and never edits `config.toml`. No arguments are required for the standard SSH listener because the plugin defaults to `0.0.0.0:2222`.

Confirm Supply Drop is running, then connect:

```sh
ssh -p 2222 <bbs-host>
```

Port `2222` is used instead of `22` so the plugin can run alongside the system SSH daemon without conflict. Any username is accepted at the SSH layer because the BBS runs its own login flow once the encrypted session is established.

In PuTTY, leave the `SSH` radio button selected and set `Port` to `2222`.

If you cannot connect, first confirm the plugin is listening:

```sh
ss -ltnp | grep ':2222'
```

`connection refused` means the host was reached, but no process accepted TCP on port `2222`. On the host, run `sudo ss -ltnp | grep ':2222'`, then check `sudo supply-drop-bbs plugin list` and `sudo journalctl -u supply-drop-bbs -n 100 --no-pager`.

On Windows:

```powershell
Get-NetTCPConnection -LocalPort 2222 -State Listen
```

## Status

Implemented:

- Multiple simultaneous SSH sessions.
- Supply Drop process transport frames: `ready`, `open`, `recv`, `close`, `send`, `kick`, and `shutdown`.
- Encrypted transport using modern SSH key exchange, ciphers, and MACs via [`russh`](https://crates.io/crates/russh).
- A persistent Ed25519 host key, generated on first start.
- `none`, `password`, `publickey`, and `keyboard-interactive` authentication, all accepted at the SSH layer.
- Server-side line editing for PTY sessions: echo, backspace/delete, `Ctrl-U`, and stripped ANSI arrow-key sequences.
- Reliable password echo suppression for `hide_input` prompts.
- Non-interactive support for `ssh bbs <command>` and piped stdin.
- CRLF, LF, and CR-NUL line handling.
- Graceful shutdown when Supply Drop stops the plugin.

## Why SSH Instead Of Telnet

Telnet is plaintext: usernames, passwords, and session content are visible to anyone with network visibility. SSH encrypts the whole session, so the BBS can be exposed beyond trusted local infrastructure without a VPN or a TLS wrapper.

SSH also moves the line discipline into the server. A Telnet client performs local echo and assembles lines itself, and password masking depends on the client honouring `IAC WILL ECHO`. Under SSH with a PTY the client sends every keystroke in raw mode, so this plugin echoes input, handles editing keys, and suppresses echo for password prompts itself. `hide_input` is therefore dependable rather than best-effort.

## How It Works

Supply Drop starts this binary as a process plugin. The plugin:

1. Loads or generates its SSH host key.
2. Binds an SSH listener, by default `0.0.0.0:2222`.
3. Writes `{"t":"ready","payload_limit":0,"version":"0.2.0"}` to stdout.
4. Accepts each SSH session channel as one Supply Drop connection.
5. Emits `open` when a client opens a shell or runs a command.
6. Emits `recv` for each complete line typed by that client.
7. Receives `send` frames from Supply Drop and writes the text back to the SSH client.
8. Emits `close` when the client disconnects or is kicked.

Stdout is reserved for JSON messages to Supply Drop. Logs and diagnostics always go to stderr.

## Protocol Mapping

The IPC protocol is unchanged from the Telnet transport, so Supply Drop needs no modification. Only the connection id prefix differs: it is now `ssh:` instead of `tcp:`.

Plugin to Supply Drop:

```json
{"t":"ready","payload_limit":0,"version":"0.2.0"}
{"t":"open","id":"ssh:127.0.0.1:52124:1"}
{"t":"recv","id":"ssh:127.0.0.1:52124:1","line":"help"}
{"t":"close","id":"ssh:127.0.0.1:52124:1"}
```

Supply Drop to plugin:

```json
{"t":"send","id":"ssh:127.0.0.1:52124:1","text":"Welcome to Supply Drop","hide_input":false}
{"t":"kick","id":"ssh:127.0.0.1:52124:1"}
{"t":"shutdown"}
```

`ready.version` is optional in the Supply Drop process transport protocol and is populated from this plugin's Cargo package version. Supply Drop BBS v0.6.1 and newer show it in the admin Plugins table.

`send.text` is display-ready text without a line terminator. This plugin appends `\r\n` when `--append-newline true` and the text does not already end in `\r` or `\n`, because an SSH PTY expects CRLF line endings.

## SSH Behavior

### Authentication

The BBS performs its own login flow over the terminal, so the SSH layer only establishes an encrypted session. Every offered credential is accepted and none of it is treated as a BBS credential:

- `none` lets clients in without a password or key, which most closely matches the old Telnet experience. Disable it with `--allow-none-auth false` if you would rather clients present something.
- `password` accepts any password.
- `publickey` accepts any key.
- `keyboard-interactive` accepts any response.

The SSH username is logged for diagnostics but is not passed to Supply Drop, which prompts for its own username.

### Host Key

On first start the plugin generates an Ed25519 host key and writes it with `0600` permissions. It is reused on every later start, so clients do not see host key mismatch warnings.

The location is chosen in this order:

1. `--host-key <PATH>`.
2. `$STATE_DIRECTORY/supply-drop-ssh/host_key`, which systemd sets when `StateDirectory=` is configured.
3. `$XDG_DATA_HOME/supply-drop-ssh/host_key`.
4. `$HOME/.local/share/supply-drop-ssh/host_key`.
5. `supply-drop-ssh-host-key` in the working directory.

The Debian package pre-generates the key at `/var/lib/supply-drop-ssh/host_key` during install and prints its fingerprint. The key is preserved on `remove` and deleted only on `purge`.

Print the fingerprint to share with users:

```sh
ssh-keygen -lf /var/lib/supply-drop-ssh/host_key.pub
```

### Terminal Handling

When a client requests a PTY, the plugin enables server-side echo and line editing:

- Printable characters are echoed as typed.
- Backspace and delete erase the previous character, including whole multi-byte UTF-8 characters.
- `Ctrl-U` clears the pending line.
- `Ctrl-C` and `Ctrl-D` on an empty line end the session.
- ANSI escape sequences such as arrow keys are stripped so they never reach the BBS.

When Supply Drop sends `hide_input: true`, the plugin stops echoing until the user submits that line, then resumes. The password is never written back to the client, so masking does not depend on client cooperation.

Sessions without a PTY, such as `ssh bbs help` or piped stdin, disable echo and are treated as a plain line stream. Because such clients send EOF immediately, the plugin waits `--eof-drain-ms` for the BBS to finish replying before closing the channel.

## Install

Most operators should install a prebuilt package from GitHub Releases. Rust is not required on the Supply Drop host.

Supply Drop BBS v0.6.0 or newer is required. Linux packages use `supply-drop-bbs plugin add` to register `/etc/supply-drop-bbs/plugins.d/ssh.toml`. They do not write to `config.toml`.

Recommended Linux install on Raspberry Pi OS, Ubuntu, or Debian.

Raspberry Pi / ARM64:

```sh
curl -fsSL -o supply-drop-ssh.deb \
  https://github.com/Mesh-America/supply-drop-ssh-transport-plugin/releases/latest/download/supply-drop-ssh-transport-plugin_arm64.deb
sudo dpkg -i supply-drop-ssh.deb
```

Intel / AMD64:

```sh
curl -fsSL -o supply-drop-ssh.deb \
  https://github.com/Mesh-America/supply-drop-ssh-transport-plugin/releases/latest/download/supply-drop-ssh-transport-plugin_amd64.deb
sudo dpkg -i supply-drop-ssh.deb
```

The Debian package installs `/usr/bin/supply-drop-ssh`, creates `/var/lib/supply-drop-ssh`, generates the host key, registers the plugin, and restarts `supply-drop-bbs` if the service is already running.

Architecture-detecting alternative:

```sh
ARCH=$(dpkg --print-architecture)   # arm64 or amd64
curl -fsSL \
  "https://github.com/Mesh-America/supply-drop-ssh-transport-plugin/releases/latest/download/supply-drop-ssh-transport-plugin_${ARCH}.deb" \
  -o supply-drop-ssh.deb
sudo dpkg -i supply-drop-ssh.deb
```

macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/Mesh-America/supply-drop-ssh-transport-plugin/main/scripts/install.sh | sh
```

The macOS installer detects Apple Silicon or Intel and installs the binary as `/usr/local/bin/supply-drop-ssh`.

Windows PowerShell:

```powershell
iwr https://raw.githubusercontent.com/Mesh-America/supply-drop-ssh-transport-plugin/main/scripts/install.ps1 -UseBasicParsing | iex
```

The Windows installer installs to `C:\Program Files\SupplyDrop\bin\supply-drop-ssh.exe` by default. Run PowerShell as Administrator when installing there.

The public installer commands work once this repository is public. While testing from a private repository, run the installer script from a local checkout after `gh auth login`; the scripts fall back to GitHub CLI for private release downloads.

Compatibility script options:

```sh
VERSION=0.2.0 INSTALL_DIR="$HOME/.local/bin" BIN_NAME=supply-drop-ssh sh scripts/install.sh
```

```powershell
.\scripts\install.ps1 -Version 0.2.0 -InstallDir "$HOME\bin" -BinName "supply-drop-ssh.exe"
```

To install only the binary without registering it with Supply Drop:

```sh
REGISTER_PLUGIN=false sh scripts/install.sh
```

```powershell
.\scripts\install.ps1 -RegisterPlugin:$false
```

Verify the install:

```sh
supply-drop-ssh --version
```

## Manual Release Install

Use this path when you want to download and inspect the release package yourself.

Linux Debian package.

Raspberry Pi / ARM64:

```sh
curl -L -o supply-drop-ssh.deb \
  https://github.com/Mesh-America/supply-drop-ssh-transport-plugin/releases/latest/download/supply-drop-ssh-transport-plugin_arm64.deb
sudo dpkg -i supply-drop-ssh.deb
```

Intel / AMD64:

```sh
curl -L -o supply-drop-ssh.deb \
  https://github.com/Mesh-America/supply-drop-ssh-transport-plugin/releases/latest/download/supply-drop-ssh-transport-plugin_amd64.deb
sudo dpkg -i supply-drop-ssh.deb
```

Linux ZIP fallback:

```sh
ASSET=linux-arm64   # use linux-x86_64 on Intel/AMD Linux
curl -L -o supply-drop-ssh.zip \
  "https://github.com/Mesh-America/supply-drop-ssh-transport-plugin/releases/latest/download/supply-drop-ssh-transport-plugin-${ASSET}.zip"
unzip -o supply-drop-ssh.zip -d supply-drop-ssh
sudo install -m 755 supply-drop-ssh/*/supply-drop-ssh /usr/local/bin/supply-drop-ssh
```

macOS:

```sh
curl -L -o supply-drop-ssh.zip \
  https://github.com/Mesh-America/supply-drop-ssh-transport-plugin/releases/latest/download/supply-drop-ssh-transport-plugin-macos-arm64.zip
unzip -o supply-drop-ssh.zip -d supply-drop-ssh
sudo install -m 755 supply-drop-ssh/*/supply-drop-ssh /usr/local/bin/supply-drop-ssh
```

macOS Intel:

```sh
curl -L -o supply-drop-ssh.zip \
  https://github.com/Mesh-America/supply-drop-ssh-transport-plugin/releases/latest/download/supply-drop-ssh-transport-plugin-macos-x86_64.zip
unzip -o supply-drop-ssh.zip -d supply-drop-ssh
sudo install -m 755 supply-drop-ssh/*/supply-drop-ssh /usr/local/bin/supply-drop-ssh
```

Windows:

Download and extract:

```text
https://github.com/Mesh-America/supply-drop-ssh-transport-plugin/releases/latest/download/supply-drop-ssh-transport-plugin-windows-x86_64.zip
```

Then register Supply Drop with the extracted `supply-drop-ssh.exe` path using `supply-drop-bbs plugin add`.

## Build From Source

```sh
cargo build --release
```

The release binary is:

```sh
target/release/supply-drop-ssh
```

On Windows, Cargo appends `.exe`.

Building requires a C toolchain because the SSH cryptography dependencies include native code. On Debian and Ubuntu, `sudo apt-get install build-essential pkg-config` is sufficient.

## Run By Hand

```sh
cargo run -- --port 2222
```

Connect from another terminal:

```sh
ssh -p 2222 localhost
```

When testing repeatedly with throwaway host keys, skip the client's known-hosts bookkeeping:

```sh
ssh -p 2222 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null localhost
```

Useful options:

```text
--bind <ADDR>                  Bind an exact address, e.g. 127.0.0.1:2222
--host <IP>                    Bind host when --bind is not used, default 0.0.0.0
--port <PORT>                  Bind port, default 2222
--host-key <PATH>              Persistent SSH host key, generated if absent
--max-connections <N>          Maximum simultaneous sessions, default 128
--max-line-bytes <N>           Maximum accepted input line length, default 8192
--payload-limit <N>            Payload limit reported to Supply Drop, default 0/unlimited
--append-newline <BOOL>        Add CRLF to outbound messages, default true
--allow-none-auth <BOOL>       Allow the SSH "none" auth method, default true
--inactivity-timeout <SECS>    Drop idle sessions, default 3600, 0 disables
--eof-drain-ms <MS>            Output flush window after client EOF, default 750
```

For a throwaway host key during local debugging:

```sh
cargo run -- --bind 127.0.0.1:2222 --host-key /tmp/dev-host-key
```

## Configure Supply Drop

If you built from source instead of installing a release, copy the binary somewhere the Supply Drop service user can execute it:

```sh
sudo cp target/release/supply-drop-ssh /usr/local/bin/supply-drop-ssh
sudo chmod 755 /usr/local/bin/supply-drop-ssh
```

Register a manually installed binary with Supply Drop BBS v0.6.0 or newer:

```sh
supply-drop-bbs plugin add ssh /usr/local/bin/supply-drop-ssh
```

This creates `/etc/supply-drop-bbs/plugins.d/ssh.toml` on first run. If the plugin is already registered, it prints `already configured — no changes made` and exits successfully. Do not edit `config.toml` for plugin registration.

Make sure the Supply Drop service user can read and write the host key path. When running under systemd, the cleanest option is a state directory:

```ini
[Service]
StateDirectory=supply-drop-ssh
```

Otherwise pass an explicit path in the drop-in:

```toml
args = ["--host-key", "/var/lib/supply-drop-ssh/host_key"]
```

Restart Supply Drop after registering the plugin.

## Versioning And Releases

The plugin uses semantic versioning in `Cargo.toml`. Release tags use `vMAJOR.MINOR.PATCH`, for example `v0.2.0`.

Maintainer release flow:

```sh
cargo test
cargo clippy --all-targets -- -D warnings
git tag v0.2.0
git push origin main v0.2.0
```

Pushing a version tag starts the release workflow. It builds Linux Debian packages, Linux ZIP fallbacks, Windows ZIPs, and macOS ZIPs, then publishes them as GitHub Release assets with stable names for the `releases/latest/download/...` install URLs above.

## Operating Notes

The default registration binds to `0.0.0.0:2222`, which is suitable when the host firewall and network exposure are intentional:

```sh
supply-drop-bbs plugin add ssh /usr/bin/supply-drop-ssh
```

For a non-default bind address, update the generated drop-in file in `/etc/supply-drop-bbs/plugins.d/ssh.toml` and restart Supply Drop. For local-only access:

```toml
args = ["--bind", "127.0.0.1:2222"]
```

Port `2222` avoids colliding with the system SSH daemon on port `22`. Do not move this plugin to port `22` on a host you administer remotely, because it does not provide shell access and would displace your real administrative SSH service.

Session content is encrypted, so the plaintext exposure of the old Telnet transport is gone. Two things still deserve attention. First, the plugin accepts any credential by design, so anyone who can reach port `2222` can reach the BBS login prompt; keep the BBS's own authentication and any firewall rules in place. Second, publish the host key fingerprint so users can verify it on first connect.

## Testing

Run the automated checks:

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Manual session test:

1. Start the plugin with `cargo run -- --bind 127.0.0.1:2222 --host-key /tmp/dev-host-key`.
2. Connect with `ssh -p 2222 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null localhost`.
3. Type `help` and press Enter.
4. Watch stdout from the plugin. You should see `ready`, `open`, and `recv` JSON frames.
5. Send `{"t":"shutdown"}` to the plugin's stdin or stop the process.

Because the plugin speaks the process transport protocol on stdin/stdout, it can be driven without Supply Drop. Start it, then paste a `send` frame into its stdin to write text to a connected client.

## Troubleshooting

Connection refused from another machine:

The client reached the host, but nothing accepted TCP on port `2222`. On the Supply Drop host, run:

```sh
sudo ss -ltnp | grep ':2222'
sudo supply-drop-bbs plugin list
sudo journalctl -u supply-drop-bbs -n 100 --no-pager
```

If nothing is listening, Supply Drop did not start the plugin or the plugin exited. If it is listening on `127.0.0.1:2222`, remove the local-only `args` line from `/etc/supply-drop-bbs/plugins.d/ssh.toml` and restart Supply Drop for LAN access. If it is listening on `0.0.0.0:2222`, check firewall or network rules.

If `sudo supply-drop-bbs plugin list` says `No process plugins configured.`, register the plugin:

```sh
sudo supply-drop-bbs plugin add ssh /usr/bin/supply-drop-ssh
```

Host key verification failed:

The client stored a different key for this host and port, usually because the host key was regenerated. Confirm the current fingerprint with `ssh-keygen -lf /var/lib/supply-drop-ssh/host_key.pub`, then remove the stale entry:

```sh
ssh-keygen -R '[bbs-host]:2222'
```

If the key changes on every restart, the plugin cannot persist it. Check that the path from `--host-key` is writable by the Supply Drop service user, and look for `generated new ssh host key` appearing repeatedly in the service logs where `loaded ssh host key` is expected.

Permission denied when connecting:

The plugin accepts any credential, so this usually means the `none` method was disabled with `--allow-none-auth false` while the client offered nothing else. Try `ssh -p 2222 -o PreferredAuthentications=password bbs-host`, or re-enable `none`.

No JSON appears on stdout:

The plugin may have failed to bind its port or to read its host key. Check the Supply Drop service logs with `sudo journalctl -u supply-drop-bbs -n 100 --no-pager`.

Supply Drop says the plugin is unhealthy:

Make sure the configured `command` path exists, is executable by the Supply Drop service user, and starts without needing an interactive shell. Also confirm the host key path is writable by that user.

Typed characters do not appear:

Echo is only enabled for sessions that request a PTY. If you are connecting with a command or piped input, no PTY is allocated and input is not echoed. Force one with `ssh -tt`.

Passwords echo during login:

Confirm Supply Drop is sending `hide_input: true` with the password prompt. Unlike Telnet, masking is applied by this plugin and does not depend on the client, so a visible password means the flag did not arrive.

`ssh bbs <command>` exits before printing a reply:

The client sends EOF immediately, and the plugin allows `--eof-drain-ms` for the BBS to answer. Raise it for a slow BBS:

```toml
args = ["--eof-drain-ms", "2000"]
```

Messages appear double-spaced:

Supply Drop sends `send.text` without a terminator. This plugin adds CRLF by default. If another layer already adds line endings, run with `--append-newline false`.

## Source Of Truth

This plugin follows the Supply Drop process transport docs:

- <https://supplydrop.meshamerica.com/PROCESS_TRANSPORTS.html>
- <https://supplydrop.meshamerica.com/PROCESS_TRANSPORTS_OPS.html>
