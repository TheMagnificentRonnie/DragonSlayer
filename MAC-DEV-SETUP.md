# DragonSlayer on macOS — setup for total beginners

> **Just want to use DragonSlayer?** You don't need any of this. Download the ready-made Mac app instead — see [Install on macOS](README.md#macos) in the README. This guide is for building DragonSlayer from source code yourself.

Assumes: you have a Mac. That's it.

Not assumed: that you've ever opened Terminal, used a package manager, written code, typed a git command, or heard of Rust. Every step tells you exactly what to do, what will happen, how long it takes, and what to do if it goes wrong.

Time budget: about **30–60 minutes** the first time (most of it waiting for downloads). Every step after this is a one-line command.

## What you're going to do

1. Open Terminal.
2. Install Apple's developer command-line tools.
3. Install Homebrew (a program-installer).
4. Install four programs Homebrew provides.
5. Install Rust (the programming language DragonSlayer is written in).
6. Configure git so it knows who you are.
7. Set up your Mac so it can talk to GitHub.
8. Download DragonSlayer.
9. Build DragonSlayer.
10. Run DragonSlayer.

Copy each grey command block and paste into Terminal. Press **Return** to run it. Wait for the `$` (or `%`) prompt to come back before pasting the next one.

---

## 1. Open Terminal

Press **⌘ + Space** together. This opens Spotlight (a little search bar).

Type: **Terminal**

Press **Return**. A window with a mostly-empty grey/black background opens. That's Terminal. Keep it open — you'll live in it for the next hour.

You'll see something like:

```
YourName@Your-Mac ~ %
```

The `%` (or `$` on older Macs) is the **prompt**. You type after it.

**To paste**: **⌘ + V**. Right-click → Paste also works.

**If you paste and see multiple lines run one after another**, that's fine — that's intended. Just wait for the prompt to come back before pasting more.

## 2. Install the Xcode Command Line Tools

What it is: a package Apple ships with the compiler (`clang`), the version control system (`git`), and the header files Rust needs to build. You don't have to use Xcode itself — just the command-line part.

Size: about 3 GB. Time: 5–20 minutes depending on your internet.

Paste this and press Return:

```sh
xcode-select --install
```

**A dialog box appears** saying "The `xcode-select` command requires the command line developer tools. Would you like to install the tools now?" — click **Install**, then **Agree** to the licence.

A progress bar appears. Go make tea. Do not close the dialog.

When it's done, verify:

```sh
xcode-select -p
```

Should print: `/Library/Developer/CommandLineTools`

```sh
git --version
```

Should print a git version (e.g. `git version 2.39.3`).

**If the dialog says "The software cannot be installed at this time"**: try again in 10 minutes — Apple's server is busy.

**If you get "command not found"** after the install: quit Terminal (⌘Q), open it again.

## 3. Install Homebrew

What it is: a program-installer for Macs. Instead of hunting websites for downloads, you type `brew install <thing>` and it fetches, installs and updates it.

Paste this whole block and press Return:

```sh
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
```

**It will ask for your Mac login password.** Type it — you won't see any characters as you type, that's normal. Press Return.

**It shows a list of what it will install** and asks you to press Return to continue. Do that.

The install takes 3–10 minutes.

**At the very end**, Homebrew prints two lines that start with `==> Next steps:` and something like:

```
Run these two commands in your terminal to add Homebrew to your PATH:
    (echo; echo 'eval "$(/opt/homebrew/bin/brew shellenv)"') >> /Users/you/.zprofile
    eval "$(/opt/homebrew/bin/brew shellenv)"
```

**Copy those two exact commands from your Terminal** (they're personalised with your username) and paste them one by one. Press Return after each.

**On an Intel Mac** the path will be `/usr/local/bin/brew` instead of `/opt/homebrew/bin/brew`. Whatever Homebrew's installer prints on YOUR machine is the right thing.

Verify:

```sh
brew --version
```

Should print something like `Homebrew 4.x.x`.

**If `brew` is "command not found"** after all that: quit Terminal (⌘Q), open again, retry.

## 4. Install the four things DragonSlayer needs

One command. Homebrew downloads and installs them.

```sh
brew install libgphoto2 ffmpeg pkg-config git
```

Time: 5–15 minutes. Homebrew will chat a lot about what it's doing. Ignore the chatter unless it says "Error:" at the end.

- **libgphoto2** is the camera library. It's what lets DragonSlayer talk to your DSLR over USB.
- **ffmpeg** is the video builder. It turns your photos into a movie.
- **pkg-config** is a helper that tells Rust where libgphoto2 lives on your Mac.
- **git** — Apple's git is fine but Homebrew's is newer.

Verify (all three should print a version, not an error):

```sh
ffmpeg -version | head -1
pkg-config --modversion libgphoto2
git --version
```

The middle one should print something like `2.5.34` — that's libgphoto2 found. (Don't try `gphoto2 --version`: that's a separate command-line program DragonSlayer doesn't need, and `brew install libgphoto2` doesn't include it, so it will say "command not found".)

## 5. Install Rust

Rust is the language DragonSlayer is written in. `rustup` is the tool that installs and updates it.

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**A menu appears** with three options. Just press **1** and **Return** — the default install is what you want.

It downloads and installs. Takes 2–5 minutes.

**At the very end** it says "To configure your current shell, run: `source "$HOME/.cargo/env"`". Do that:

```sh
source "$HOME/.cargo/env"
```

Verify:

```sh
rustc --version
```

Should print something like `rustc 1.98.0`. **If the number after `1.` is less than 95, run** `rustup update`.

## 6. Tell git who you are

Substitute your real name and the email you use on GitHub. **These lines are per-machine — do them once, ever.**

```sh
git config --global user.name  "Your Real Name"
git config --global user.email "you@example.com"
git config --global init.defaultBranch main
git config --global pull.rebase false
git config --global core.autocrlf input
```

`--global` means "for every project on this Mac". `user.name` and `user.email` show up on your commits.

## 7. Set your Mac up to talk to GitHub

**Only building and running DragonSlayer? Skip this step.** The repository is public, so downloading it (step 8, Option A) needs no login or token. You only need this step to *push* changes back to GitHub.

Two options. Pick one.

### Option A — HTTPS (easier, five steps)

This uses your GitHub username and a **Personal Access Token** (not your password — GitHub doesn't accept passwords over HTTPS anymore).

1. Open <https://github.com/settings/tokens/new?scopes=repo&description=DragonSlayer%20on%20my%20Mac> in your browser.
2. GitHub asks you to sign in / confirm.
3. Under "Expiration" pick **90 days** (or "No expiration" if you're lazy).
4. Scroll down and click **Generate token**.
5. GitHub shows a token that starts `ghp_` or `github_pat_`. **Copy it — you won't see it again.**

You'll paste this token the first time you push to GitHub. macOS remembers it in the Keychain, so you paste it once.

Skip to step 8.

### Option B — SSH key (more setup, no password prompts)

Better long-term. One five-minute setup, then it just works forever.

```sh
ssh-keygen -t ed25519 -C "you@example.com"
```

Press **Return** three times to accept every default (default location, no passphrase, confirm no passphrase). Passphrases are safer; blank is easier. Your call.

```sh
eval "$(ssh-agent -s)"
ssh-add --apple-use-keychain ~/.ssh/id_ed25519
```

Persist the agent so it survives reboots:

```sh
mkdir -p ~/.ssh && cat >> ~/.ssh/config <<'EOF'
Host github.com
  AddKeysToAgent yes
  UseKeychain yes
  IdentityFile ~/.ssh/id_ed25519
EOF
```

Copy your public key to the clipboard:

```sh
pbcopy < ~/.ssh/id_ed25519.pub
```

Open <https://github.com/settings/ssh/new>, paste (**⌘V**) into the "Key" box, name it something like *"MacBook Pro"*, and click **Add SSH key**.

Test it:

```sh
ssh -T git@github.com
```

The first time it asks *"Are you sure you want to continue connecting?"* — type **yes** and Return.

Should print: `Hi <your-username>! You've successfully authenticated…`

## 8. Download DragonSlayer

Pick where to keep it. `~/Developer` is Apple's suggestion:

```sh
mkdir -p ~/Developer && cd ~/Developer
```

Then either (matching your choice in step 7):

```sh
# Option A — HTTPS
git clone https://github.com/TheMagnificentRonnie/DragonSlayer.git

# Option B — SSH
git clone git@github.com:TheMagnificentRonnie/DragonSlayer.git
```

**First time you push (later) with Option A**, git asks for a username and password. Your GitHub username, and for the password paste the `ghp_...` token from step 7. macOS Keychain remembers it forever after.

```sh
cd DragonSlayer
```

## 9. Build DragonSlayer

Point Rust at Homebrew's `pkg-config` (needs doing once per Terminal window, or add to your shell profile):

```sh
export PKG_CONFIG_PATH="$(brew --prefix)/lib/pkgconfig"
```

**Optional but nice** — make that permanent so you don't have to type it in every new Terminal:

```sh
echo 'export PKG_CONFIG_PATH="$(brew --prefix)/lib/pkgconfig"' >> ~/.zprofile
```

Now build:

```sh
cargo build --release --features gphoto2 -p dragonslayer-cli -p dragonslayer-app
```

The first build takes **5–15 minutes** — Rust downloads and compiles every dependency once. Subsequent builds are seconds.

**Success looks like** a line at the end saying `Finished 'release' profile [optimized] target(s) in 4m 27s`.

**Failure looks like** red text saying `error[...]`. If you see that, jump to Troubleshooting below.

## 10. Run DragonSlayer

### Without a camera (try the app)

```sh
cargo run --release --features gphoto2 -p dragonslayer-app -- --mock
```

(Keep `--features gphoto2` even with `--mock`. Leaving it out is a *different* build, so Rust recompiles the whole app, and again when you switch back.)

The app window opens with a fake camera showing a moving orange square. Press **H** for the in-app help. Press **Space** to capture a fake frame.

### With your camera

1. Turn the camera on. Put it in **PC** / **PC(Tether)** / **PTP** mode via its menu. (Panasonic Lumix, e.g. GH5: **Setup menu (spanner) → USB Mode → PC(Tether)**, or **PC(Storage)** if there's no Tether option.)
2. Plug it into a USB port on the Mac.
3. **Quit Photos and Image Capture** if they open — they'll steal the camera.
4. In Terminal:

```sh
killall ptpcamerad
```

That's a macOS background daemon that grabs cameras. Killing it releases yours. It restarts itself when needed.

5. Now run the app:

```sh
cargo run --release --features gphoto2 -p dragonslayer-app
```

The dot in the top right of the app window should be **green** with your camera's name next to it.

Click **New project…**, pick a folder, press **Space** to capture.

## 11. Every day after this

Once the setup is done, each session is:

```sh
cd ~/Developer/DragonSlayer
git pull                                                # get updates
cargo run --release --features gphoto2 -p dragonslayer-app
```

If you want to always build the CLI too:

```sh
cargo run --release --features gphoto2 -p dragonslayer-cli -- cameras
cargo run --release --features gphoto2 -p dragonslayer-cli -- new ~/Films/MyFilm
```

Add these to your shell so it's shorter:

```sh
echo "alias dsl-app='cd ~/Developer/DragonSlayer && cargo run --release --features gphoto2 -p dragonslayer-app --'" >> ~/.zprofile
echo "alias dsl='cd ~/Developer/DragonSlayer && cargo run --release --features gphoto2 -p dragonslayer-cli --'" >> ~/.zprofile
```

Restart Terminal, then just:

```sh
dsl-app
dsl cameras
```

## 12. Make a double-clickable app (optional)

Tired of Terminal? This builds a proper `DragonSlayer.app`, puts it in the Applications folder in your home folder, and adds a **DragonSlayer** shortcut to your Desktop:

```sh
cd ~/Developer/DragonSlayer
scripts/package-macos.sh --install
```

The app is self-contained: it carries its own copy of libgphoto2, the camera drivers and ffmpeg, and frees the camera from macOS (`killall ptpcamerad`) every time it starts. Drag it to your Dock if you like.

It's a *copy*, so `git pull` alone doesn't update it. After pulling, re-run `scripts/package-macos.sh --install`.

The same script (without `--install`) is how the release zip is made: it writes `target/dist/dragonslayer-<version>-macos-arm64.zip`.

---

## Troubleshooting

### "command not found: brew"

Terminal doesn't know where Homebrew is. Quit Terminal (⌘Q), open again. If still nothing, re-run the two `eval` lines from the end of step 3.

### "command not found: cargo"

Same but for Rust:

```sh
source "$HOME/.cargo/env"
```

Add to your profile so it's permanent:

```sh
echo 'source "$HOME/.cargo/env"' >> ~/.zprofile
```

### "Could not find libgphoto2" during `cargo build`

`PKG_CONFIG_PATH` isn't set. From step 9:

```sh
export PKG_CONFIG_PATH="$(brew --prefix)/lib/pkgconfig"
```

If it still fails after that:

```sh
brew reinstall libgphoto2 pkg-config
cargo clean
export PKG_CONFIG_PATH="$(brew --prefix)/lib/pkgconfig"
cargo build --release --features gphoto2 -p dragonslayer-cli -p dragonslayer-app
```

### "unable to find libclang" during `cargo build`

You haven't fully installed the Xcode Command Line Tools. Re-run step 2. If still failing:

```sh
brew install llvm
export LIBCLANG_PATH="$(brew --prefix llvm)/lib"
```

### `git push` asks for a password and rejects the one you type

GitHub doesn't accept your account password over HTTPS. Go back to step 7 Option A and use a Personal Access Token as the password. Or set up SSH keys (step 7 Option B).

### `git clone` says "Permission denied (publickey)"

You picked SSH (Option B) but the key isn't uploaded to GitHub. Re-run:

```sh
pbcopy < ~/.ssh/id_ed25519.pub
```

Then paste into <https://github.com/settings/ssh/new>.

### The app opens but the window is blank / black

Update Rust and rebuild:

```sh
rustup update stable
cargo clean
cargo build --release --features gphoto2 -p dragonslayer-app
```

### "Camera not found" — but it's plugged in

In order:

1. Turn the camera off. Wait 3 seconds. Turn it on.
2. Unplug USB. Wait 3 seconds. Plug back in (different port is fine).
3. Quit Photos and Image Capture again. Run `killall ptpcamerad` again.
4. Check the camera's USB mode: **PC** / **PC(Tether)** / **PTP** — not *Mass Storage* / *Card Reader*.
5. Check the camera's battery isn't flat.

The most common cause is a wedged PTP session — a previous run crashed while the camera was open. The camera off/on fixes it 90% of the time. Nothing on the camera is damaged.

### `dyld[…]: Library not loaded: libgphoto2.6.dylib` at runtime

The compiled app can't find libgphoto2 at run time.

```sh
brew reinstall libgphoto2
```

If still failing, tell the app loader where Homebrew installs libraries:

```sh
export DYLD_LIBRARY_PATH="$(brew --prefix)/lib:$DYLD_LIBRARY_PATH"
```

Add to your profile so it persists:

```sh
echo 'export DYLD_LIBRARY_PATH="$(brew --prefix)/lib:$DYLD_LIBRARY_PATH"' >> ~/.zprofile
```

### "ffmpeg: could not run ffmpeg: No such file or directory" when exporting

DragonSlayer can't find ffmpeg. Two usual causes:

- **You started it outside Terminal** (a home-made app, Automator, a Dock shortcut to the raw binary). Programs opened from Finder don't get Homebrew's folders on their `PATH`, so they can't see `/opt/homebrew/bin/ffmpeg` even though Terminal can. Use `scripts/package-macos.sh --install` (step 12) instead — its app carries its own ffmpeg.
- **ffmpeg isn't installed.** `ffmpeg -version` in Terminal says "command not found" → `brew install ffmpeg`.

### "This Mac says the app is damaged or from an unidentified developer"

If you downloaded the pre-built app (not built yourself): it isn't signed with a paid Apple Developer certificate, so macOS blocks the first launch. See [Install on macOS](README.md#macos) in the README for the one-time fix, or in Terminal:

```sh
xattr -dr com.apple.quarantine /Applications/DragonSlayer.app
```

If you built from source, this doesn't apply — the OS trusts what it just built.

### Anything else

- Open **Help** inside the app (press **H**) — has a full troubleshooting section for camera-side issues.
- The last few lines of `~/Library/Logs/dragonslayer.log` usually explain what happened (0.2.1-beta and later; older Mac builds didn't write a log). Open it with `open ~/Library/Logs/dragonslayer.log`.
- Open an issue at <https://github.com/TheMagnificentRonnie/DragonSlayer/issues>. Attach the last 30 lines of the log and the exact error you saw.

---

## What just happened

You installed a compiler toolchain (Xcode CLT), a package manager (Homebrew), a camera library (libgphoto2), a video builder (ffmpeg), a build helper (pkg-config), a programming language (Rust), a version control system (git — you already had Apple's, now you have a newer one), and set your Mac up to identify with GitHub. Then you cloned the DragonSlayer source, built it, and ran it.

None of that is DragonSlayer-specific. Every Rust project on macOS starts the same way. Every future DragonSlayer session is now three commands:

```sh
cd ~/Developer/DragonSlayer
git pull
cargo run --release --features gphoto2 -p dragonslayer-app
```

That's it. Welcome in.
