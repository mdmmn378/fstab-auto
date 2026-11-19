# fstab-auto

An interactive terminal UI for mounting NAS shares on Linux. Answer a few
questions and the share is mounted, either until the next reboot or
permanently through `/etc/fstab`.

Instead of remembering

```sh
sudo mount -t cifs -o username=alice,uid=1001 //nas01/projects /mnt/nas
```

run `fstab-auto` and fill in the server, share, username, password and mount
directory one step at a time. If the directory does not exist, it offers to
create it.

Currently supports **CIFS / SMB** on **Linux** (amd64 and arm64). The code is
laid out so other protocols (NFS, SSHFS) can be added later. An Apple silicon
build is published too; on macOS it runs in `--dry-run` mode only.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/mdmmn378/fstab-auto/main/install.sh | sh
```

This downloads the binary for your system from the latest GitHub release,
verifies its SHA-256 checksum, installs it to `/usr/local/bin`, and adds tab
completion for the shells it finds (bash, zsh, fish).

| Release asset | System |
| --- | --- |
| `fstab-auto-x86_64-unknown-linux-musl.tar.gz` | Linux amd64 (static) |
| `fstab-auto-aarch64-unknown-linux-musl.tar.gz` | Linux arm64 (static) |
| `fstab-auto-aarch64-apple-darwin.tar.gz` | macOS, Apple silicon |

| Variable | Meaning | Default |
| --- | --- | --- |
| `FSTAB_AUTO_VERSION` | Version to install, e.g. `0.2.0` | latest release |
| `FSTAB_AUTO_INSTALL_DIR` | Where the binary goes | `/usr/local/bin` |
| `FSTAB_AUTO_NO_COMPLETIONS` | Set to `1` to skip shell completions | unset |
| `FSTAB_AUTO_BASE_URL` | Download from a mirror instead of GitHub | unset |

```sh
# a specific version
curl -fsSL https://raw.githubusercontent.com/mdmmn378/fstab-auto/main/install.sh | FSTAB_AUTO_VERSION=0.2.0 sh
# uninstall
curl -fsSL https://raw.githubusercontent.com/mdmmn378/fstab-auto/main/install.sh | sh -s -- --uninstall
```

Mounting CIFS shares needs the `cifs-utils` package
(`sudo apt install cifs-utils`); the installer and the wizard both tell you if
it is missing.

## Use

```sh
fstab-auto            # asks for sudo when needed
fstab-auto --dry-run  # explore without changing anything, no root required
```

There are three tabs, switched with `Tab` / `Shift+Tab`:

- **New mount** walks through protocol, server, share, username, password,
  mount directory, options and persistence, then shows a review with the exact
  command or fstab line before anything happens.
- **Mounts** lists CIFS shares that are mounted or configured in fstab. Unmount
  with `u`, mount with `m`, and stop mounting at boot with `d`.
- **Help** lists the keys and file locations.

| Key | Action |
| --- | --- |
| `Enter` | Confirm the step and continue |
| `Esc` | Go back one step |
| `↑` / `↓` | Choose an option, field or share |
| `Ctrl+R` | Show or hide the password while typing |
| `Ctrl+U` | Clear the current field |
| `q`, `Ctrl+C` | Quit (`q` only outside text fields) |

Files on the share are owned by the user who started `fstab-auto` (the one
behind `sudo`), not root. Change the owner, the domain or add any `mount.cifs`
option such as `vers=3.0,ro` in the Options step.

### Temporary or permanent

| | Temporary | Permanent |
| --- | --- | --- |
| Lasts until | reboot or unmount | removed in the Mounts tab |
| Changes on disk | none | fstab entry and a credentials file |
| Password | passed to `mount.cifs` through the environment | stored in a root-only file |

A permanent mount:

1. writes the credentials to `/etc/fstab-auto/credentials/<mount>.cred`
   (mode 600, directory mode 700, root only);
2. copies `/etc/fstab` to `/etc/fstab.fstab-auto.bak`, then adds one entry
   marked `# Added by fstab-auto` with `_netdev,nofail` so an unreachable NAS
   never blocks booting. The file is replaced atomically and every existing
   line is kept as it is;
3. mounts the share through that entry, which proves the entry works.

If mounting fails (wrong password, unknown share, host down), the fstab entry
and the credentials file are removed again and the error is explained.

The password never appears on a command line, in the process list, in the
review screen or in fstab.

### Options

```
fstab-auto [OPTIONS]

    --dry-run                 Walk through everything without changing the system
    --fstab <FILE>            fstab file to manage [default: /etc/fstab]
    --credentials-dir <DIR>   Directory for credential files [default: /etc/fstab-auto/credentials]
-h, --help                    Print help
-V, --version                 Print version

fstab-auto completions <bash|zsh|fish|elvish|powershell>   Print a completion script
```

### Shell completion

The installer sets this up. To do it by hand, or for a shell it does not
cover, generate the script from the binary itself; each release archive also
ships the scripts in `completions/`.

```sh
fstab-auto completions bash | sudo tee /usr/local/share/bash-completion/completions/fstab-auto >/dev/null
fstab-auto completions zsh  | sudo tee /usr/local/share/zsh/site-functions/_fstab-auto >/dev/null
fstab-auto completions fish > ~/.config/fish/completions/fstab-auto.fish
```

## Develop

Requirements: [Rust](https://rustup.rs) (stable), [Task](https://taskfile.dev),
[lefthook](https://lefthook.dev), and Docker for the end-to-end test.

```sh
task setup   # install git hooks (format + clippy on commit, tests on push)
task run     # start the TUI in dry-run mode; works on macOS too
task check   # format check, clippy, unit tests: what CI runs
task e2e     # real mounts against a Samba container, driven through the TUI
task         # list all tasks
```

`task e2e` starts a Samba server and a privileged Ubuntu 24.04 client in
Docker, drives the TUI with tmux as an unprivileged user, and checks the
result on the system: mount state, file ownership, fstab contents, credential
file permissions, rollback after a wrong password, guest access.
`task e2e:shell` gives you a shell in that client to try it by hand.

### Layout

```
src/
  main.rs          command line, sudo re-exec
  model.rs         MountRequest: what the user asked for
  protocol/        per-protocol knowledge (cifs.rs: options, validation)
  system/          the host: fstab editing, /proc/mounts, commands, operations
  tui/             wizard state, app state, rendering
docker/            Samba server and client images for the end-to-end test
tests/e2e.sh       the end-to-end scenario
install.sh         curl installer
```

To add a protocol: add a module next to `protocol/cifs.rs`, a variant to
`ProtocolKind`, and enable its row in `PROTOCOL_CHOICES`.

## Release

Releases are started by hand, not by pushing to `main`.

```sh
task bump -- 0.2.0      # sets the version in Cargo.toml and Cargo.lock
git commit -am "Release 0.2.0" && git push
task release -- 0.2.0   # or: GitHub -> Actions -> Release -> Run workflow
```

The Release workflow asks for the version, checks that it matches
`Cargo.toml`, builds Linux binaries for amd64 and arm64 and a macOS binary for
Apple silicon (each archive includes the shell completions), then creates the
tag `v0.2.0` and the GitHub release with the archives and checksums. The
installer picks up the new release automatically.

## License

[MIT](LICENSE)
