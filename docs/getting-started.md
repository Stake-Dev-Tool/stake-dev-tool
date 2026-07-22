# Getting started

There are two ways to use Stake Dev Tool:

- **Cloud** — create an account at [app.stakedevtool.com](https://app.stakedevtool.com)
  and use the web workbench with zero install ([pricing](https://stakedevtool.com/pricing)),
  or [self-host the whole platform](../deploy/README.md) for free.
- **Desktop** — install the app below for the fastest local dev loop
  (front hot-reload, local math, instant restarts).

## Install the desktop app

Grab the latest build from the
[Releases page](https://github.com/Stake-Dev-Tool/stake-dev-tool/releases/latest):

| Platform              | File                                           | Notes                                     |
| --------------------- | ---------------------------------------------- | ----------------------------------------- |
| Windows 10/11 (x64)   | `Stake-Dev-Tool-vX.Y.Z-windows-x64.exe`        | NSIS installer                            |
| macOS Apple Silicon   | `Stake-Dev-Tool-vX.Y.Z-macos-arm64.app.tar.gz` | Extract, then see [macOS first launch](#macos-first-launch) |
| Debian / Ubuntu (x64) | `Stake-Dev-Tool-vX.Y.Z-linux-x64.deb`          | `sudo apt install ./<file>.deb`           |
| Other Linux (x64)     | `Stake-Dev-Tool-vX.Y.Z-linux-x64.AppImage`     | `chmod +x` then run                       |

> Intel Macs aren't supported — open an issue if that's a blocker.

The app checks GitHub Releases on startup and shows a banner when a newer
version is published. Updates are Minisign-verified and installed silently
(passive NSIS on Windows, replace-in-place on macOS/Linux).

### macOS first launch

The macOS build is not yet signed with an Apple Developer ID, so on first
launch Gatekeeper shows:

> "Stake Dev Tool.app" is damaged and can't be opened. You should move it
> to the Bin.

The app **isn't** damaged — macOS just blocks unsigned downloads. To unblock
it, run this once in Terminal after moving the app to `/Applications`:

```bash
xattr -dr com.apple.quarantine "/Applications/Stake Dev Tool.app"
```

Alternative one-time bypass: right-click the app → **Open** → confirm
**Open** in the dialog.

## First run

1. **Launch the app** and click **Install Local CA** in the amber banner.
   This gives you HTTPS locally with zero browser warnings. One prompt on
   macOS, silent on Windows; on Linux the `.deb` pulls `libnss3-tools`
   automatically (AppImage users: `sudo apt install libnss3-tools`).
   Firefox uses its own certificate store — trust the CA manually if needed.
2. **Browse…** to your game's math folder (layout below).
3. Enter the **Front URL** of your game's frontend
   (e.g. `http://localhost:5174`).
4. **Launch test view** — a Chromium window opens with your game running at
   every enabled resolution, each iframe in its own session.
5. **Save** the profile to reload the whole setup in one click next time.

The test view sidebar covers balance, currency, language, device, social
mode, custom resolutions, force / bookmark / replay, and per-frame mute.

## Math folder layout

```
<math_root>/
└── <game-slug>/
    ├── index.json            # { "modes": [{ "name", "cost", "events", "weights" }, …] }
    ├── lookuptable_<mode>.csv     # eventId,weight,payoutMultiplier
    └── books_<mode>.jsonl.zst     # one event per line, zstd-compressed
```

Modes are auto-detected from `index.json`. This is the exact format produced
by the Stake Engine math SDK — point the app at your simulation output and go.

## Next steps

- [Architecture & HTTP API](architecture.md) — how it all fits together,
  RGS endpoints, running the LGS standalone.
- [Self-hosting](../deploy/README.md) — run the cloud platform on your own
  server.
- [`sdt` CLI](../crates/cli/README.md) — push math revisions from CI.
