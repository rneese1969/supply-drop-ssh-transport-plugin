# Maintainer Checklist

Use this checklist whenever changing code, docs, installers, release packaging, or the GitHub Pages site.

The main rule: behavior, docs, and release artifacts must agree. If a command, installer action, CLI flag, port, protocol detail, or troubleshooting step changes in code, update every place that teaches it.

## Before Editing

- Check current repo state with `git status --short --branch`.
- Read the files you plan to change before editing.
- If the change affects Supply Drop process transport behavior, compare it against:
  - <https://supplydrop.meshamerica.com/PROCESS_TRANSPORTS.html>
  - <https://supplydrop.meshamerica.com/PROCESS_TRANSPORTS_OPS.html>
- If the change affects sysop setup, test or verify the exact command shape. Do not assume CLI syntax from memory.

## Before Commit

- Run formatting/lint checks when code changed:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

- Run tests:

```sh
cargo test
```

- If the GitHub Pages site changed, render it locally and inspect desktop/mobile screenshots:

```sh
npx playwright screenshot --viewport-size=1440,900 file:///D:/Projects/supply-drop-ssh-transport-plugin/docs/index.html docs/_shot-desktop.png
npx playwright screenshot --viewport-size=390,844 file:///D:/Projects/supply-drop-ssh-transport-plugin/docs/index.html docs/_shot-mobile.png
```

- Delete temporary screenshots before committing.
- Run whitespace validation:

```sh
git diff --check
```

- Update docs that mirror behavior:
  - `README.md` for project overview, install, config, troubleshooting.
  - `QUICKSTART.md` for sysop setup.
  - `docs/index.html` and `docs/styles.css` for GitHub Pages.
  - `CHANGELOG.md` for user-visible changes that belong in the next release.

## After Commit And Push

- Push the branch or `main`.
- Watch GitHub Actions:

```sh
gh run list --limit 5
gh run watch <run-id> --exit-status
```

- Confirm both CI and Pages succeed when relevant.
- If Pages changed, open the live site and sanity-check the deployed page:
  - <https://mesh-america.github.io/supply-drop-ssh-transport-plugin/>

## Release Checklist

Only cut a release when users need a new binary or installer artifact.

- Update `Cargo.toml` version.
- Update `CHANGELOG.md` with the release date and user-visible changes.
- Run:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

- Create and push a semver tag:

```sh
git tag vMAJOR.MINOR.PATCH
git push origin main vMAJOR.MINOR.PATCH
```

- Watch the release workflow.
- Confirm the GitHub Release has the expected assets:
  - Debian ARM64 package (`supply-drop-ssh-transport-plugin_arm64.deb`)
  - Debian AMD64 package (`supply-drop-ssh-transport-plugin_amd64.deb`)
  - Linux ARM64 ZIP fallback
  - Linux x86_64 ZIP fallback
  - Windows x86_64
  - macOS Apple Silicon
  - macOS Intel
- Verify installer docs still point to the right latest-release asset names.

## Documentation Drift Triggers

Update docs immediately when any of these change:

- CLI flags or defaults such as `--bind`, `--port`, `--append-newline`, `--allow-none-auth`, `--host-key`, or `--eof-drain-ms`.
- Supply Drop plugin registration behavior, including the `plugins.d` drop-in path and `supply-drop-bbs plugin add` syntax.
- Minimum supported Supply Drop BBS version.
- Installer URLs, binary names, install paths, or supported architectures.
- Debian package maintainer scripts, dependencies, or post-install registration behavior.
- SSH behavior: line endings, host key handling, authentication methods, echo suppression, password input, or backspace handling.
- Troubleshooting steps discovered from a real sysop install.
- Release workflow asset names.

## Manual Smoke Tests

Standalone plugin transport test:

```sh
supply-drop-ssh --bind 127.0.0.1:2222
ssh -p 2222 localhost
```

Supply Drop integration test on a host:

```sh
supply-drop-bbs plugin add ssh /usr/bin/supply-drop-ssh
sudo systemctl restart supply-drop-bbs
sudo ss -ltnp | grep ':2222'
sudo supply-drop-bbs plugin list
sudo journalctl -u supply-drop-bbs -n 100 --no-pager
```

PuTTY smoke test:

- Host: BBS host or IP.
- Port: `2222`.
- Connection type: leave `SSH` selected.
- Confirm login, `H`, and at least one command round trip.

SSH-specific smoke tests:

- Confirm the host key fingerprint is stable across a plugin restart, so returning clients are not warned. Compare `ssh-keygen -lf /var/lib/supply-drop-ssh/host_key.pub` before and after `sudo systemctl restart supply-drop-bbs`.
- Confirm a password prompt sent with `hide_input` does not echo the typed characters.
- Confirm backspace edits the current line rather than emitting stray characters.
- Confirm `ssh -p 2222 <bbs-host> help` returns BBS output, which exercises the non-PTY path and the EOF drain window.
