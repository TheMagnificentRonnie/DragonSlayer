# macOS developer setup

Zero-to-running-app on a fresh Mac. Copy each block into Terminal.

Everything below is for **Intel or Apple Silicon** Macs (macOS 12 Monterey or newer). ARM (M1/M2/M3/M4) and x86_64 both work — Homebrew handles the difference.

## Contents

- [1. Xcode Command Line Tools](#1-xcode-command-line-tools)
- [2. Homebrew](#2-homebrew)
- [3. System libraries (libgphoto2, ffmpeg, pkg-config)](#3-system-libraries)
- [4. Rust](#4-rust)
- [5. Git first-time setup](#5-git-first-time-setup)
- [6. SSH key for GitHub](#6-ssh-key-for-github)
- [7. Clone DragonSlayer](#7-clone-dragonslayer)
- [8. Build](#8-build)
- [9. Run](#9-run)
- [10. Editor setup (optional)](#10-editor-setup-optional)
- [11. Troubleshooting](#11-troubleshooting)

---

## 1. Xcode Command Line Tools

Provides `clang`, `git`, `make` and the SDK headers Rust needs.

```sh
xcode-select --install
```

A dialog will pop up. Click **Install**, agree to the licence, and wait. It takes 5–15 minutes on a decent connection.

Verify:

```sh
xcode-select -p     # should print /Library/Developer/CommandLineTools
clang --version     # should print an Apple clang version
git --version       # ≥ 2.30 is fine
```

## 2. Homebrew

Most macOS dev tooling comes from [Homebrew](https://brew.sh). Install:

```sh
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
```

At the end the installer prints two `eval` lines to add `brew` to your `PATH`. Run them, then persist them in your shell config:

```sh
# Apple Silicon
echo 'eval "$(/opt/homebrew/bin/brew shellenv)"' >> ~/.zprofile
eval "$(/opt/homebrew/bin/brew shellenv)"

# Intel Mac (only if the installer printed /usr/local instead of /opt/homebrew)
echo 'eval "$(/usr/local/bin/brew shellenv)"' >> ~/.zprofile
eval "$(/usr/local/bin/brew shellenv)"
```

Verify:

```sh
brew --version
brew doctor          # should say "Your system is ready to brew."
```

## 3. System libraries

DragonSlayer needs:

- **libgphoto2** — camera control (LGPL, dynamically linked)
- **ffmpeg** — video compile (invoked as a subprocess)
- **pkg-config** — how the Rust build finds libgphoto2
- **git-lfs** — optional but useful for the repo if we later add large binaries

```sh
brew install libgphoto2 ffmpeg pkg-config git-lfs
```

Verify:

```sh
pkg-config --modversion libgphoto2   # e.g. 2.5.34
ffmpeg -version | head -1            # should mention libx264
gphoto2 --version | head -1          # optional CLI, useful for testing the camera outside DragonSlayer
```

## 4. Rust

Install [rustup](https://rustup.rs) — the official Rust toolchain manager.

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Accept the default install (option **1**). At the end, source the cargo env:

```sh
source "$HOME/.cargo/env"
```

DragonSlayer needs Rust **1.95** or newer. If your version is older, run `rustup update`.

Verify:

```sh
rustc --version                  # ≥ 1.95
cargo --version
rustup component add clippy      # linter, used by `cargo clippy`
rustup component add rustfmt     # formatter, used by `cargo fmt`
```

## 5. Git first-time setup

Once per machine. Substitute your name and the email you use on GitHub:

```sh
git config --global user.name  "Your Name"
git config --global user.email "you@example.com"

git config --global init.defaultBranch main
git config --global pull.rebase false       # merge on `git pull`; change to true if you prefer rebase
git config --global core.editor "code --wait"   # or nano, vim, whatever you use
git config --global core.autocrlf input     # don't convert LF to CRLF on commit
```

Sanity check:

```sh
git config --global --list | grep -E "user\.|init\.|pull\.|core\."
```

## 6. SSH key for GitHub

Push and pull without typing a password. If you already have `~/.ssh/id_ed25519` you can skip the `ssh-keygen` step.

```sh
# Generate an ed25519 key. Press Enter at each prompt to accept defaults.
# You can set a passphrase or leave blank; blank is more convenient, a
# passphrase is safer.
ssh-keygen -t ed25519 -C "you@example.com"

# Start the ssh-agent and load the key
eval "$(ssh-agent -s)"
ssh-add --apple-use-keychain ~/.ssh/id_ed25519

# Persist that agent behaviour across reboots
cat >> ~/.ssh/config <<'EOF'
Host github.com
  AddKeysToAgent yes
  UseKeychain yes
  IdentityFile ~/.ssh/id_ed25519
EOF

# Copy the public key to your clipboard
pbcopy < ~/.ssh/id_ed25519.pub
```

Now open <https://github.com/settings/ssh/new>, paste, give it a title (e.g. *"MacBook Pro"*), and save.

Test:

```sh
ssh -T git@github.com
```

You should see: *"Hi TheMagnificentRonnie! You've successfully authenticated..."*

## 7. Clone DragonSlayer

Pick where you keep your code. `~/src` or `~/Developer` are common.

```sh
mkdir -p ~/Developer && cd ~/Developer
git clone git@github.com:TheMagnificentRonnie/DragonSlayer.git
cd DragonSlayer
```

If you're not a collaborator on the repo, use HTTPS and open pull requests from your own fork:

```sh
git clone https://github.com/TheMagnificentRonnie/DragonSlayer.git
cd DragonSlayer
git remote add upstream https://github.com/TheMagnificentRonnie/DragonSlayer.git
```

## 8. Build

### Mock-camera build (no libgphoto2 needed)

Fast, useful for UI work.

```sh
cargo build --release
cargo test --workspace
```

Expected: `Finished 'release' profile [optimized] target(s)` and `test result: ok. 10 passed`.

### Real-camera build

```sh
cargo build --release --features gphoto2 -p dragonslayer-cli -p dragonslayer-app
```

If pkg-config complains it can't find libgphoto2:

```sh
brew reinstall libgphoto2 pkg-config
export PKG_CONFIG_PATH="$(brew --prefix)/lib/pkgconfig"
cargo clean
cargo build --release --features gphoto2 -p dragonslayer-cli -p dragonslayer-app
```

### Lint and format

Before opening a PR:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets    # should be silent
cargo test --workspace
```

## 9. Run

### Mock camera (no hardware)

```sh
cargo run --release -p dragonslayer-app -- --mock
```

The mock camera renders a moving orange square with live view and simulates RAW+JPEG, so the whole capture / onion-skin / compile pipeline exercises without a real camera plugged in.

### Real camera

Before plugging in, release the camera from macOS's own capture daemons:

```sh
# Quit Photos and Image Capture first (⌘Q in each), then:
killall ptpcamerad
```

Now plug in the camera on USB, turn it on, set the USB mode to **PC** / **PC(Tether)** / **PTP** (see the in-app Help for camera-specific menus), and:

```sh
cargo run --release --features gphoto2 -p dragonslayer-app
```

Or via the CLI:

```sh
cargo run --release --features gphoto2 -p dragonslayer-cli -- cameras
cargo run --release --features gphoto2 -p dragonslayer-cli -- new ~/Films/MyFilm
cargo run --release --features gphoto2 -p dragonslayer-cli -- capture ~/Films/MyFilm sc010 --count 3
```

### Convenience aliases

If you'll launch the app often, add to `~/.zshrc`:

```sh
alias dsl-app='cargo run --release --features gphoto2 -p dragonslayer-app --'
alias dsl='cargo run --release --features gphoto2 -p dragonslayer-cli --'
```

Then:

```sh
dsl cameras
dsl-app ~/Films/MyFilm
```

## 10. Editor setup (optional)

### VS Code

```sh
brew install --cask visual-studio-code
code --install-extension rust-lang.rust-analyzer
code --install-extension tamasfe.even-better-toml
code --install-extension vadimcn.vscode-lldb
```

Open the project:

```sh
code ~/Developer/DragonSlayer
```

`rust-analyzer` picks up the workspace automatically. The included `Cargo.toml` at the root defines the `dragonslayer-*` members so all crates get analysed together.

### RustRover / IntelliJ

Just open the folder — the Rust plugin autodetects the workspace.

### Terminal-only

`neovim` + `rustaceanvim` + `nvim-lspconfig`, or plain `vim` + `rust.vim`. Whichever you already use.

## 11. Troubleshooting

### `cargo build` fails with "no acceptable C compiler found in $PATH"

You skipped step 1. Run `xcode-select --install`.

### `Could not find libgphoto2` (pkg-config error)

Either libgphoto2 isn't installed or `PKG_CONFIG_PATH` doesn't point at Homebrew:

```sh
brew install libgphoto2 pkg-config
export PKG_CONFIG_PATH="$(brew --prefix)/lib/pkgconfig"
echo 'export PKG_CONFIG_PATH="$(brew --prefix)/lib/pkgconfig"' >> ~/.zshrc
```

Then `cargo clean && cargo build --features gphoto2`.

### `Unable to find libclang` (bindgen error)

The `libgphoto2-sys` crate uses `bindgen`, which needs libclang. Xcode Command Line Tools include one:

```sh
xcode-select --install
# If still failing, point bindgen at Homebrew's clang:
brew install llvm
export LIBCLANG_PATH="$(brew --prefix llvm)/lib"
```

### Camera not found

- Quit Photos and Image Capture, then `killall ptpcamerad`.
- Plug the camera into a different USB port.
- Check the camera's USB mode is *PC* / *PC(Tether)* / *PTP*, not *Mass Storage* / *Card Reader*.
- Turn the camera off, wait 3 s, on again (fixes wedged PTP sessions).
- Full checklist inside the app: **Help → Troubleshooting**.

### `dyld: Library not loaded: libgphoto2.6.dylib` at runtime

The built binary can't find libgphoto2 at runtime. Homebrew's install location must be on the loader path:

```sh
brew reinstall libgphoto2
# If still failing:
export DYLD_LIBRARY_PATH="$(brew --prefix)/lib:$DYLD_LIBRARY_PATH"
```

### `ssh: connect to host github.com port 22: Connection refused`

Your network blocks outbound SSH. Use HTTPS instead:

```sh
git remote set-url origin https://github.com/TheMagnificentRonnie/DragonSlayer.git
```

You'll be prompted for credentials on push. Use a [GitHub Personal Access Token](https://github.com/settings/tokens) as the password; it can be cached by macOS's Keychain via `git config --global credential.helper osxkeychain` (usually already configured).

### `cargo build` succeeds but the app window is blank / black

Update wgpu's Metal backend — usually resolved by:

```sh
rustup update stable
cargo clean
cargo build --release --features gphoto2 -p dragonslayer-app
```

If the window is still blank after an update, check the Console app for `Metal` or `wgpu` errors and open an issue with the log.

### Rust says "unresolved import" but everything compiles

Restart rust-analyzer (`⌘⇧P` → "Rust Analyzer: Restart Server" in VS Code). This happens after major dependency updates.

---

## Verifying everything works — one-liner

```sh
brew list libgphoto2 ffmpeg pkg-config >/dev/null && \
  rustc --version | grep -q 'rustc 1\.\(9[5-9]\|[0-9][0-9][0-9]\)' && \
  git remote get-url origin >/dev/null && \
  cargo test --workspace 2>&1 | grep -q '10 passed' && \
  echo "✅ DragonSlayer dev environment is good."
```

---

## Where to go next

- [`README.md`](README.md) — features, project format, quick start
- [`MANUAL_TESTING.md`](MANUAL_TESTING.md) — full manual test plan (do §0–§4 to sanity-check a fresh build)
- [`dragonslayer-spec.md`](dragonslayer-spec.md) — design document
- [`CAMERAS.md`](CAMERAS.md) — camera compatibility
- The in-app **Help** modal (press **H** in the app)
