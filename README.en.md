# dsh-come｜DSH Companion

Turns [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) into a **tray-resident Windows desktop shell**: system tray icon + process supervision (crash self-healing / backoff restarts) + one-click open/restart — no more typing `dsh web` by hand.

> **Who it is for**: people who already have `dsh` (or Node.js) installed and want a resident tray entry that starts on double-click and pulls the engine back up when it dies. When pieces are missing, the admin page / wizard installs them properly (node via winget, dsh via `npm install -g` — no npx temp pull). Developers can also just use the official `npx @deepseek-ai/dsh web`; this project wraps the engine guarding and the desktop experience.

> 🚀 **Current direction (2026-08-27, v4)**: **thin shell + zero shell UI** — the shell only does three things: guarding (profile groups) / bootstrap install / environment manifest (come.patch.yml), plus self-update; everything user-visible is a dsh plugin (see `docs/direction-v4.md`). No plugin marketplace (dsh-market does that), no version management (follows system dsh), no status page (the dsh web UI already has one).

## What it does

```
dsh-come.exe (Rust single exe, out-of-process supervisor)
├── supervision   spawns `dsh web` (system dsh from PATH; crash auto-restart with
│                 exponential backoff + healthy-period reset; rolling logs)
├── doctor        evidence-driven self-healing: scan → triage → mode-based repair → escalation
├── bootstrap     installs what's missing (node → winget LTS; dsh → npm install -g)
├── tray          open UI (topmost) / status line / restart engine / stop engine /
│                 open log folder / check updates / exit
├── admin page    http://127.0.0.1:3081: status + install Node/dsh + start/stop (emergency surface only; plugin/version management lives in the dsh web UI)
└── patch         come.patch.yml mounted via `dsh --patch` (disables dsh-market detached restart)
```

Tray menu (status refreshed every 3s): **Open dsh UI** (topmost), status line (`Running ✓ http://127.0.0.1:3080` or a stage hint), **Restart engine**, **Stop engine** (stops for real regardless of who started it — saves memory), **Open log folder**, **Check for updates / Update to vX**, **Exit**. The browser opens automatically once the engine is ready; if dsh/Node is missing, it installs automatically (or use the admin page at `http://127.0.0.1:3081` to install and control manually).

## Self-healing (doctor)

Evidence-driven self-healing with **no hardcoded checks** — every finding comes from actually scanning the environment (orphan file:// plugin entries / broken cordis.patch.yml / partial downloads / occupied port / orphan processes); future causes that take dsh down are recognized too.

- **Mode ladder** (escalates on repeated failure): Inspect (report only) → Treat (auto 🟢, recommends 🟡/🔴) → Attend (auto 🟢+🟡, recommends 🔴) → Emergency (everything; 🔴 backed up first)
- **Hooks**: Treat on first boot; each crash escalates the mode before restarting (Treat→Attend→Emergency), with one Emergency fallback before giving up
- **Manual**: `dsh-come doctor` (inspect by default) / `dsh-come doctor --mode attend`
- **Safety**: every modification is backed up (`.bak`); the running engine is never touched by process handling; suspicious dsh processes without port evidence are only auto-killed in Emergency (avoids killing other instances you started)

## Quick start

**Windows**: download `dsh-come.exe` from [GitHub Releases](https://github.com/qing3a/dsh-come/releases) and run it. Or build from source:

```bash
git clone https://github.com/qing3a/dsh-come
cd dsh-come
cargo run --release
```

**macOS / Linux** (one-liner install; registers a launchd/systemd watchdog automatically):

```bash
curl -fsSL https://github.com/qing3a/dsh-come/releases/latest/download/install.sh | sh
# or wget -qO- … | sh; from a clone: sh scripts/install.sh
```

**Prerequisites**: Windows 10/11, macOS, or mainstream Linux (Node.js auto-installed via winget LTS if missing; dsh auto-installed via `npm install -g @deepseek-ai/dsh` — both can be done manually from the admin page with progress). On Linux the watchdog needs a systemd user session (falls back to tray / `--no-tray` residency without crash-revive otherwise); on macOS it's a launchd LaunchAgent (login autostart + KeepAlive).

## Self-update (P0, no code signing)

- Distribution: GitHub Releases + `update.json` (`{version, url, sha256}`), produced automatically by the GitHub Actions release pipeline on `v*` tags (`.github/workflows/release.yml`)
- Check: silently once per day on startup; `Check for updates` in the tray forces a check; **ask-first** (never silent install)
- Install: download → SHA256 verify → backup `dsh-come.exe.bak` → swap via a helper script (watchdog task temporarily disabled during the swap, re-enabled after) → relaunch
- No code-signing certificate by design: HTTPS + SHA256 against a compromised download channel is enough for a personal project; the only cost is the one-time SmartScreen "unknown publisher" warning (More info → Run anyway). If an enterprise pack ever needs SmartScreen-free installs, Azure Trusted Signing (~$10/mo) is the cheaper route.
- CLI: `dsh-come update` prints `{current, latest, available}` as JSON.

## Key design decisions

| Decision | Why |
|---|---|
| **Follows system dsh, no version management** | Install/upgrade belongs to system npm (`npm install -g @deepseek-ai/dsh`); the shell does not pin/rollback/smoke-test versions |
| **Missing = proper install, no npx temp pull** | Temp pulls can't guarantee availability/consistency; node missing → winget LTS (one UAC prompt), dsh missing → user-level `npm install -g`; wizard auto-triggers, admin page as manual fallback |
| **No data isolation** | No `DSH_HOME` override; dsh uses its system default (`%USERPROFILE%\.dsh`), identical to terminal usage |
| **Out-of-process supervisor** | Crash self-healing / tray / logs all live in the shell; dsh updates don't affect the shell |
| **Shell only touches "doorknobs"** | Only start command / port probe / process management (`docs/cli-contract.md`); no CLI output parsing, no internal files, no plugin APIs |

## Contract surface (docs/cli-contract.md)

- C1 `dsh web --host <host> --port <port>` (system dsh from PATH; install flow if missing, no npx fallback)
- C2 `GET http://127.0.0.1:<port>/` → HTTP 200 (readiness probe)
- C3 `dsh --patch <path>` (come.patch.yml overlay)
- C4/C5 reserved (v2 smoke test / plugin management)

## Relation to dsh-tray

[`dsh-tray`](https://github.com/qing3a/dsh-tray) is an **in-process** dsh plugin (tray/toasts, lives and dies with dsh); this project is the **out-of-process** shell (guards the dsh process). They complement each other: when both are installed, dsh-tray detects dsh-come and downgrades itself.

## License

MIT. The tray icon is a code-generated 32x32 rounded icon (`src/tray.rs`), unrelated to DeepSeek AI trademarks.

## Roadmap (docs/direction-v4.md)

- ✅ v1 (current): process supervision / tray / auto-open browser / come.patch.yml / crash backoff restart / doctor self-healing
- ✅ P0 (2026-08-27): self-update (GitHub Releases + GitHub Actions auto release + SHA256 verify + ask-first) / i18n (zh/en: tray/notify/CLI/admin page)
- ✅ Cross-platform (2026-08-29): 3-platform matrix release (win / macOS universal / linux) + `install.sh` one-liner + launchd/systemd watchdog + per-platform update manifest (`update-{win,macos,linux}.json`)
- 🔜 P1: multi-instance (single-machine profile groups); P2: everything-as-plugin (come-manager / md-studio template / dsh-market listing)
