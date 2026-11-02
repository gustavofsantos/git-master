---
title: Installation
description: Requirements, the install script and its options, verifying the download, upgrading, uninstalling and building GitMaster from source.
status: partial
order: 10
---

GitMaster is a desktop app. On Linux, the install script downloads the prebuilt release for your processor, checks it, and installs it. On macOS, you build GitMaster from source with Rust.

## Requirements

| Requirement | Details |
|---|---|
| Git | Required. GitMaster uses the `git` program for its operations, and it is the only external program the app needs. The install script does not install Git. |
| Operating system | Linux for the prebuilt release. macOS builds from source. |
| Processor | amd64 (x86_64) or arm64 (aarch64). |
| Graphical session | GitMaster draws its window, so it needs a graphical session and a graphics driver that the UI toolkit, GPUI, can use. |
| Tools for the install script | `curl`, `tar`, and either `sha256sum` or `shasum`. |

> **Planned.** Moving a file to a branch or a worktree needs Git 2.38 or newer. With an older Git, those actions are disabled and the rest of the app works as usual.

## Install with the script

Run this command in a terminal on Linux:

```sh
curl -fsSL https://gitmaster.dev/install.sh | sh
```

The script is also published in the repository. If the domain is unreachable, run it from GitHub instead:

```sh
curl -fsSL https://raw.githubusercontent.com/gustavofsantos/git-master/main/install.sh | sh
```

The script does the following:

1. Detects your processor.
2. Downloads the Linux release archive for your processor from the latest release, or from the release you pass with `--version`.
3. Checks the archive against the release's `checksums.txt`. If the check fails, the script stops before it installs anything.
4. Puts the `gitmaster` binary in the directory given by `--dir`.
5. Installs the icons and a desktop entry.

On macOS the script stops with a message that points to [building from source](#build-from-source).

### Options

Pass options after `sh -s --`:

```sh
curl -fsSL https://gitmaster.dev/install.sh | sh -s -- --version v0.1.0 --dir /usr/local/bin
```

| Option | Meaning |
|---|---|
| `--version TAG` | The release to install, such as `v0.1.0`. The default is the latest release. The `v` is optional. |
| `--dir DIR` | The directory for the `gitmaster` binary. The default is `~/.local/bin`. The script does not use `sudo`, so choose a directory you can write to. |
| `-h`, `--help` | Prints the usage and exits. |

### Install a specific version

To install a release other than the latest, pass its tag with `--version`:

```sh
curl -fsSL https://gitmaster.dev/install.sh | sh -s -- --version v0.1.0
```

To check which version is installed, run:

```sh
gitmaster --version
```

The output has the form `gitmaster 0.1.0`, the release version without the `v`.

### What gets installed where

| Item | Location |
|---|---|
| `gitmaster` binary | `--dir`, by default `~/.local/bin/gitmaster` |
| Icons | `$XDG_DATA_HOME/icons/hicolor`, or `~/.local/share/icons/hicolor` when `XDG_DATA_HOME` is not set |
| Desktop entry | `gitmaster.desktop` in the `applications` folder of the same data directory |

The desktop entry starts the binary by its absolute path, so the application menu works even when `~/.local/bin` is not on the desktop session's `PATH`.

If `~/.local/bin` is not on your `PATH`, the script prints the line to add. Run `gitmaster --version` to check the installed version.

## Verify the download

Each release publishes a `checksums.txt` file with the SHA-256 hash of every archive. The install script checks the archive against it before installing.

To check an archive yourself, download it and `checksums.txt` from the [GitHub releases page](https://github.com/gustavofsantos/git-master/releases). Then run the check for your archive:

```sh
grep gitmaster_linux_amd64.tar.gz checksums.txt | sha256sum -c -
```

Replace the archive name with the one for your processor:

| Processor | Archive |
|---|---|
| amd64 (x86_64) | `gitmaster_linux_amd64.tar.gz` |
| arm64 (aarch64) | `gitmaster_linux_arm64.tar.gz` |

## Upgrade

Run the install command again. It replaces the binary. To install a specific release, add `--version`. Start GitMaster again to run the new version.

## Uninstall

The install script has no uninstall option. Remove what it installed. If you used `--dir` or `XDG_DATA_HOME`, use those locations instead.

```sh
rm -f ~/.local/bin/gitmaster
rm -f ~/.local/share/applications/gitmaster.desktop
find ~/.local/share/icons/hicolor -name gitmaster.png -delete
```

Removing the program does not remove your settings or app data. They are in `~/.config/gitmaster` and `~/.local/share/gitmaster`. Delete those folders yourself if you want to remove them too.

## Build from source

Use this path on macOS, or on Linux when you want a build of the repository itself. Building from source needs the stable Rust toolchain. The repository's `rust-toolchain.toml` selects the stable channel, and `rustup` reads that file, so every `cargo` command in the repository uses the latest stable release. Install Rust with [rustup](https://rustup.rs). If a Rust compiler from your distribution is on your `PATH`, put `~/.cargo/bin` before it.

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

`make install` also installs the icons and desktop entry on Linux, and `GitMaster.app` on macOS. The macOS app declares a minimum system version of 10.15.7. These variables change where it puts files:

| Variable | Default | Used for |
|---|---|---|
| `PREFIX` | Cargo's install root: `$CARGO_INSTALL_ROOT`, else `~/.cargo` | The binary goes in `PREFIX/bin` |
| `DATADIR` | `$XDG_DATA_HOME`, else `~/.local/share` | Linux icons and desktop entry |
| `APPDIR` | `~/Applications` | macOS app bundle |

To remove a source install, run `make uninstall` with the same variables.

> **Note.** The macOS app is ad-hoc signed, not signed with a developer identity. The first time you open it, macOS may ask you to confirm that you want to open an app from an unidentified developer.

## Windows

> **Planned.** GitMaster does not run on Windows yet, so there is no Windows build or installer.
