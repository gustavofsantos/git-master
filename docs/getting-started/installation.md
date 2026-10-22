---
title: Installation
description: Requirements, the install script and its options, verifying the download, upgrading, uninstalling and building GitMaster from source.
status: partial
order: 10
---

GitMaster is a desktop app for macOS and Linux. The install script downloads the release for your system, checks it, and installs it. You can also build GitMaster from source with Rust.

## Requirements

| Requirement | Details |
|---|---|
| Git | Required. GitMaster runs `git` for every operation, and it is the only external program the app needs. The install script does not install Git. |
| Operating system | macOS, or Linux. The macOS app declares a minimum system version of 10.15.7. |
| Processor | amd64 (x86_64) or arm64 (aarch64). |
| Graphical session (Linux) | GitMaster draws its window, so it needs a graphical session and a graphics driver that the UI toolkit, GPUI, can use. |
| Tools for the install script | `curl`, `tar`, and either `sha256sum` or `shasum`. |

## Install with the script

Run this command in a terminal:

```sh
curl -fsSL https://raw.githubusercontent.com/gustavofsantos/git-master/main/install.sh | sh
```

The script does the following:

1. Detects your operating system and processor.
2. Downloads the release archive for your system from the latest release, or from the release you pass with `--version`.
3. Checks the archive against the release's `checksums.txt`. If the check fails, the script stops before it installs anything.
4. Puts the `gitmaster` binary in the directory given by `--dir`.
5. On Linux, installs the icons and a desktop entry. On macOS, installs `GitMaster.app`.

### Options

Pass options after `sh -s --`:

```sh
curl -fsSL https://raw.githubusercontent.com/gustavofsantos/git-master/main/install.sh | sh -s -- --version v0.1.0 --dir /usr/local/bin
```

| Option | Meaning |
|---|---|
| `--version TAG` | The release to install, such as `v0.1.0`. The default is the latest release. The `v` is optional. |
| `--dir DIR` | The directory for the `gitmaster` binary. The default is `~/.local/bin`. The script does not use `sudo`, so choose a directory you can write to. |
| `--app-dir DIR` | macOS only: the directory for `GitMaster.app`. The default is `~/Applications`. |
| `-h`, `--help` | Prints the usage and exits. |

### What gets installed where

| Item | Linux | macOS |
|---|---|---|
| `gitmaster` binary | `--dir`, by default `~/.local/bin/gitmaster` | `--dir`, by default `~/.local/bin/gitmaster` |
| Icons | `$XDG_DATA_HOME/icons/hicolor`, or `~/.local/share/icons/hicolor` when `XDG_DATA_HOME` is not set | Inside the app bundle |
| Desktop entry | `gitmaster.desktop` in the `applications` folder of the same data directory | Not used |
| `GitMaster.app` | Not used | `--app-dir`, by default `~/Applications/GitMaster.app` |

On Linux, the desktop entry starts the binary by its absolute path, so the application menu works even when `~/.local/bin` is not on the desktop session's `PATH`.

If `~/.local/bin` is not on your `PATH`, the script prints the line to add. Run `gitmaster --version` to check the installed version.

> **Note.** The macOS app is ad-hoc signed, not signed with a developer identity. The script removes the download quarantine flag. The first time you open the app, macOS may still ask you to confirm that you want to open an app from an unidentified developer.

## Verify the download

Each release publishes a `checksums.txt` file with the SHA-256 hash of every archive. The install script checks the archive against it before installing.

To check an archive yourself, download it and `checksums.txt` from the [GitHub releases page](https://github.com/gustavofsantos/git-master/releases). Then run the check for your archive.

On Linux:

```sh
grep gitmaster_linux_amd64.tar.gz checksums.txt | sha256sum -c -
```

On macOS, print the hash of the archive and compare it with the line in `checksums.txt`:

```sh
shasum -a 256 gitmaster_darwin_arm64.tar.gz
grep gitmaster_darwin_arm64.tar.gz checksums.txt
```

Replace the archive name with the one for your system:

| System | Archive |
|---|---|
| Linux, amd64 | `gitmaster_linux_amd64.tar.gz` |
| Linux, arm64 | `gitmaster_linux_arm64.tar.gz` |
| macOS, Intel (amd64) | `gitmaster_darwin_amd64.tar.gz` |
| macOS, Apple silicon (arm64) | `gitmaster_darwin_arm64.tar.gz` |

## Upgrade

Run the install command again. It replaces the binary, and on macOS it replaces `GitMaster.app`. To install a specific release, add `--version`. Start GitMaster again to run the new version.

## Uninstall

The install script has no uninstall option. Remove what it installed. If you used `--dir`, `--app-dir` or `XDG_DATA_HOME`, use those locations instead.

On Linux:

```sh
rm -f ~/.local/bin/gitmaster
rm -f ~/.local/share/applications/gitmaster.desktop
find ~/.local/share/icons/hicolor -name gitmaster.png -delete
```

On macOS:

```sh
rm -f ~/.local/bin/gitmaster
rm -rf ~/Applications/GitMaster.app
```

Removing the program does not remove your settings or app data. On Linux they are in `~/.config/gitmaster` and `~/.local/share/gitmaster`. Delete those folders yourself if you want to remove them too.

## Build from source

Building from source needs the stable Rust toolchain. The repository's `rust-toolchain.toml` selects the stable channel, and `rustup` reads that file, so every `cargo` command in the repository uses the latest stable release. Install Rust with [rustup](https://rustup.rs). If a Rust compiler from your distribution is on your `PATH`, put `~/.cargo/bin` before it.

1. On Linux, install the development packages. On Debian and Ubuntu:

   ```sh
   sudo apt-get install -y build-essential pkg-config libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev
   ```

2. Clone the repository and enter it:

   ```sh
   git clone https://github.com/gustavofsantos/git-master
   cd git-master
   ```

3. Build the release binary. The binary is written to `target/release/gitmaster`.

   ```sh
   make build
   ```

4. Install it:

   ```sh
   make install
   ```

   Without options, `make install` puts the binary in `~/.cargo/bin/gitmaster`. To put it in the same place the install script uses, set the prefix:

   ```sh
   make install PREFIX=$HOME/.local
   ```

`make install` also installs the icons and desktop entry on Linux, and `GitMaster.app` on macOS. These variables change where it puts files:

| Variable | Default | Used for |
|---|---|---|
| `PREFIX` | Cargo's install root: `$CARGO_INSTALL_ROOT`, else `~/.cargo` | The binary goes in `PREFIX/bin` |
| `DATADIR` | `$XDG_DATA_HOME`, else `~/.local/share` | Linux icons and desktop entry |
| `APPDIR` | `~/Applications` | macOS app bundle |

To remove a source install, run `make uninstall` with the same variables.

## Windows

> **Planned.** GitMaster does not run on Windows yet, so there is no Windows build or installer.
