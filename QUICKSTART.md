# Quickstart For Supply Drop Sysops

This gets SSH access running on an existing Supply Drop BBS instance.

Supply Drop BBS v0.6.0 or newer is required. The Linux package registers the plugin through Supply Drop's `plugins.d` drop-in system; it does not edit `config.toml`.

## 1. Install The Plugin

Linux on Raspberry Pi OS, Ubuntu, or Debian.

Raspberry Pi / ARM64:

```sh
curl -fsSL -o supply-drop-ssh.deb \
  https://github.com/rneese1969/supply-drop-ssh-transport-plugin/releases/latest/download/supply-drop-ssh-transport-plugin_arm64.deb
sudo dpkg -i supply-drop-ssh.deb
```

Intel / AMD64:

```sh
curl -fsSL -o supply-drop-ssh.deb \
  https://github.com/rneese1969/supply-drop-ssh-transport-plugin/releases/latest/download/supply-drop-ssh-transport-plugin_amd64.deb
sudo dpkg -i supply-drop-ssh.deb
```

The Debian package installs the plugin, generates its SSH host key, registers SSH with Supply Drop, and restarts Supply Drop if it is already running.

macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/Mesh-America/supply-drop-ssh-transport-plugin/main/scripts/install.sh | sh
```

Windows PowerShell:

```powershell
iwr https://raw.githubusercontent.com/Mesh-America/supply-drop-ssh-transport-plugin/main/scripts/install.ps1 -UseBasicParsing | iex
```

Verify:

```sh
supply-drop-ssh --version
sudo supply-drop-bbs plugin list
```

The Debian package installs `/usr/bin/supply-drop-ssh` and registers the plugin with:

```sh
supply-drop-bbs plugin add ssh /usr/bin/supply-drop-ssh
```

That command creates `/etc/supply-drop-bbs/plugins.d/ssh.toml` on first run. If SSH is already registered, it exits successfully without changing anything. No arguments are required for the standard listener because the plugin defaults to `0.0.0.0:2222`.

Architecture-detecting alternative:

```sh
ARCH=$(dpkg --print-architecture)
curl -fsSL \
  "https://github.com/Mesh-America/supply-drop-ssh-transport-plugin/releases/latest/download/supply-drop-ssh-transport-plugin_${ARCH}.deb" \
  -o supply-drop-ssh.deb
sudo dpkg -i supply-drop-ssh.deb
```

## 2. Restart Supply Drop

The Debian package restarts Supply Drop automatically if the service is running. If it was stopped during install, start or restart it:

```sh
sudo systemctl restart supply-drop-bbs
```

Then check that the plugin is configured:

```sh
sudo supply-drop-bbs plugin list
```

Also confirm the SSH port is listening.

Linux:

```sh
ss -ltnp | grep ':2222'
```

Windows PowerShell:

```powershell
Get-NetTCPConnection -LocalPort 2222 -State Listen
```

## 3. Publish Your Host Key Fingerprint

The plugin generates a persistent SSH host key on first install. Print its fingerprint and share it with your users so they can verify the BBS on first connect:

```sh
ssh-keygen -lf /var/lib/supply-drop-ssh/host_key.pub
```

The key is reused across restarts and upgrades. It is kept when the package is removed and deleted only on `purge`.

## 4. Connect

From another terminal:

```sh
ssh -p 2222 <bbs-host>
```

Any username works. The SSH layer only sets up the encrypted session, and the BBS then runs its own login prompts.

PuTTY users:

- Set `Host Name` to your BBS host or IP.
- Set `Port` to `2222`.
- Leave the `SSH` radio button selected.

For a same-machine test:

```sh
ssh -p 2222 localhost
```

Type `help` and press Enter.

## 5. Expose The Port Intentionally

SSH encrypts the session, so unlike Telnet the traffic is not readable in transit. Two things still need your attention.

The plugin accepts any SSH username, password, or key by design, because Supply Drop performs the real login. Anyone who can reach port `2222` reaches the BBS login prompt, so keep the BBS's own authentication enabled and firewall the port to the audience you intend.

Port `2222` is used rather than `22` so the plugin coexists with the system SSH daemon. Do not move it to port `22` on a machine you administer remotely; this plugin serves the BBS, not a shell, and would displace your administrative access.

The default install listens on `0.0.0.0:2222`, which is suitable for LAN or public access when your firewall rules are intentional:

```sh
supply-drop-bbs plugin add ssh /usr/bin/supply-drop-ssh
```

For private/local access only, update `/etc/supply-drop-bbs/plugins.d/ssh.toml` after registration:

```toml
args = ["--bind", "127.0.0.1:2222"]
```

Open TCP port `2222` in your host firewall or cloud firewall only if you intend remote users to connect.

## Troubleshooting

### Connection refused from another machine

If PuTTY, `ssh`, or `nc` says `connection refused`, the client reached the Pi, but nothing accepted TCP on port `2222`.

On the Pi, run:

```sh
sudo ss -ltnp | grep ':2222'
```

No output means the plugin is not listening. Check the plugin state and logs:

```sh
sudo supply-drop-bbs plugin list
sudo journalctl -u supply-drop-bbs -n 100 --no-pager
```

If `ss` shows `127.0.0.1:2222`, the plugin is only reachable from the Pi itself. For LAN access, remove the local-only `args` line from `/etc/supply-drop-bbs/plugins.d/ssh.toml` so the plugin uses its default `0.0.0.0:2222` listener.

If `ss` shows `0.0.0.0:2222`, the plugin is listening on the network. Check the Pi firewall, router rules, or whether the client is using the right host address.

If `ssh -p 2222 localhost` cannot connect:

- Check `sudo journalctl -u supply-drop-bbs -n 100 --no-pager`.
- Confirm the plugin path in `/etc/supply-drop-bbs/plugins.d/ssh.toml` exists and is executable.
- Confirm no other service is already using port `2222`.

On a Raspberry Pi or other Linux host, these commands usually narrow it down fast:

```sh
supply-drop-ssh --version
sudo ss -ltnp | grep ':2222'
sudo supply-drop-bbs plugin list
sudo journalctl -u supply-drop-bbs -n 100 --no-pager
```

If no process is listening on `2222`, Supply Drop probably did not start the plugin. Recheck `/etc/supply-drop-bbs/plugins.d/ssh.toml` and the Supply Drop service logs.

If `sudo supply-drop-bbs plugin list` says `No process plugins configured.`, register the plugin:

```sh
sudo supply-drop-bbs plugin add ssh /usr/bin/supply-drop-ssh
```

Then restart Supply Drop.

If `/usr/bin/supply-drop-ssh --bind 127.0.0.1:0` shows a `ready` frame with `version`, but Supply Drop admin still shows no version, Supply Drop may still be launching an old `/usr/local/bin` copy:

```sh
sudo grep -n '/usr/local/bin/supply-drop-ssh' /etc/supply-drop-bbs/plugins.d/ssh.toml
sudo supply-drop-bbs plugin remove ssh
sudo supply-drop-bbs plugin add ssh /usr/bin/supply-drop-ssh
sudo systemctl restart supply-drop-bbs
```

If remote users cannot connect:

- Confirm the plugin is bound to `0.0.0.0:2222`.
- Check the host firewall.
- Check any router, VPS, or cloud security group rules.

### Host key verification failed

The user's SSH client stored a different key for this host and port. This normally means the host key was regenerated. Confirm the current fingerprint:

```sh
ssh-keygen -lf /var/lib/supply-drop-ssh/host_key.pub
```

Then have the user drop the stale entry:

```sh
ssh-keygen -R '[bbs-host]:2222'
```

If the fingerprint changes after every restart, the plugin cannot persist its key. Check that `/var/lib/supply-drop-ssh` is writable by the Supply Drop service user, and look for repeated `generated new ssh host key` lines in the logs where `loaded ssh host key` is expected.

### Permission denied when connecting

The plugin accepts any credential, so this usually means the `none` auth method was turned off with `--allow-none-auth false` while the client offered nothing else. Have the user try:

```sh
ssh -p 2222 -o PreferredAuthentications=password <bbs-host>
```

### Typed characters do not appear

Echo is handled by the server and enabled only for sessions that request a terminal. Interactive `ssh` connections get one automatically. If you are piping input or running `ssh bbs <command>`, add `-tt` to force a terminal.

### Passwords echo during login

Masking is applied by this plugin rather than the client, so a visible password means Supply Drop did not send `hide_input: true` with that prompt. Check the BBS prompt configuration.
