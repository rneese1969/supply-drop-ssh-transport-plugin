# Changelog

All notable changes to this project will be documented here.

This project follows semantic versioning. Tags use the `vMAJOR.MINOR.PATCH` format.

## Unreleased

## 0.2.0 - 2026-05-14

Converted the transport from Telnet to SSH. The Supply Drop process transport protocol is unchanged, so no BBS-side changes are required.

### Changed

- Replaced the plaintext TCP/Telnet listener with an SSH server built on [`russh`](https://crates.io/crates/russh). Sessions are now encrypted end to end.
- Changed the default listen port from `2323` to `2222`. Port `2222` avoids colliding with the system SSH daemon on port `22`.
- Renamed the package to `supply-drop-ssh-transport-plugin` and the installed binary to `supply-drop-ssh`.
- Renamed the plugin registration from `telnet` to `ssh`, so the drop-in is now `/etc/supply-drop-bbs/plugins.d/ssh.toml`.
- Changed the connection id prefix in `open`, `recv`, and `close` frames from `tcp:` to `ssh:`.
- Moved line editing into the server. A Telnet client echoes locally and assembles lines itself, but an SSH client with a PTY sends raw keystrokes, so the plugin now performs echo and line assembly.
- Made `hide_input` password masking reliable. Echo suppression is applied by the server instead of relying on the client to honour Telnet `IAC WILL ECHO`.

### Added

- A persistent Ed25519 host key, generated on first start and reused afterwards so clients do not see host key mismatch warnings. Location is configurable with `--host-key` and defaults to a systemd `StateDirectory`, `XDG_DATA_HOME`, or `$HOME/.local/share` path.
- Host key generation and fingerprint printing in the Debian package's `postinst`, using `/var/lib/supply-drop-ssh`. The key is preserved on `remove` and erased only on `purge`.
- `none`, `password`, `publickey`, and `keyboard-interactive` authentication, all accepted at the SSH layer because Supply Drop performs the real login over the terminal. `none` can be disabled with `--allow-none-auth false`.
- Interactive editing support for backspace and delete including multi-byte UTF-8 characters, `Ctrl-U` to clear the line, `Ctrl-C` and `Ctrl-D` to end the session, and stripping of ANSI arrow-key escape sequences.
- Support for non-interactive sessions such as `ssh bbs <command>` and piped stdin, with a configurable `--eof-drain-ms` window so queued BBS output is flushed before the channel closes.
- `--inactivity-timeout` to drop idle sessions.
- Expanded unit tests covering line assembly, echo, password masking, editing keys, and escape-sequence handling.

### Removed

- The Telnet option negotiation state machine and the `--no-telnet-negotiation` flag, which have no SSH equivalent.

## 0.1.5 - 2026-05-13

- Migrated existing Debian package drop-in configs from the old `/usr/local/bin/supply-drop-telnet` command path to `/usr/bin/supply-drop-telnet` during package configuration.
- Added troubleshooting docs for hosts that have both old `/usr/local/bin` and new `/usr/bin` Telnet plugin binaries installed.

## 0.1.4 - 2026-05-13

- Added `version` to the process transport `ready` frame so Supply Drop BBS v0.6.1 and newer can show the plugin version in the admin Plugins table.
- Added copy buttons to command examples on the GitHub Pages site.
- Reworked the GitHub Pages install flow, README quickstart, and sysop quickstart around direct Linux `.deb` installs for Raspberry Pi/ARM64 and Intel/AMD64 hosts.

## 0.1.3 - 2026-05-13

- Updated installers to require Supply Drop BBS v0.6.0 or newer and register the Telnet plugin through `supply-drop-bbs plugin add`.
- Corrected installer registration to use the Supply Drop positional CLI form, for example `supply-drop-bbs plugin add telnet /usr/bin/supply-drop-telnet`.
- Switched plugin registration docs from direct `config.toml` edits to `/etc/supply-drop-bbs/plugins.d/telnet.toml` drop-in registration.
- Added installer opt-outs for binary-only installs without Supply Drop registration.
- Added Debian package release assets for Linux `amd64` and `arm64`, matching the Supply Drop BBS v0.6.0 `dpkg -i` install flow.
- Updated Linux install docs to recommend `sudo dpkg -i` and require Supply Drop BBS v0.6.0 or newer.

## 0.1.2 - 2026-05-12

- Added Unix and Windows installer scripts.
- Added architecture-specific macOS release packages for Apple Silicon and Intel hosts.
- Included installer scripts inside release packages.

## 0.1.1 - 2026-05-12

- Added Linux ARM64 release packaging for small Linux hosts and ARM-based Supply Drop deployments.
- Updated GitHub Actions to Node 24-era action versions.

## 0.1.0 - 2026-05-12

- Initial Telnet process transport plugin for Supply Drop BBS.
- Added process IPC support for `ready`, `open`, `recv`, `close`, `send`, `kick`, and `shutdown`.
- Added Telnet line handling, basic option negotiation, and best-effort password echo suppression.
- Added operator documentation for building, installing, configuring, and troubleshooting the plugin.
