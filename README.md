# ![Parla](src-tauri/icons/32x32.png) Parla

[![Version](https://img.shields.io/github/v/release/Jany-M/parla?include_prereleases&sort=semver&label=version&style=flat-square)](https://github.com/Jany-M/parla/releases/latest) [![Builds (Win/MacOS)](https://github.com/Jany-M/parla/actions/workflows/ci-release.yml/badge.svg)](https://github.com/Jany-M/parla/actions/workflows/ci-release.yml)

Meeting notes for Windows and macOS: records yourself or your meetings (any app), transcribes verbatim, recognizes different speakers and can translate them live in a floating overlay, then summarizes on demand.

Local AI/LLMs only (max privacy), no accounts, no subscriptions, no cloud, no high-end GPU required, free forever.

*Parla is built to stay lightweight and run on laptops with modest hardware, not only high-end machines.*

## Support this Project if you like it

[![Buy Me A Coffee](https://img.shields.io/badge/Buy%20Me%20A%20Coffee-shambix-FFDD00?style=flat-square&logo=buy-me-a-coffee&logoColor=black)](https://www.buymeacoffee.com/shambix) or [![Sponsor this on GitHub](https://img.shields.io/github/sponsors/Jany-M?label=Sponsor&logo=githubsponsors&logoColor=white&color=EA4AAA)](https://github.com/sponsors/Jany-M)

## Features

* **Record & pause** in one continuous transcript (resume without starting a new meeting)
* **Auto-saves** transcripts as you go, including any live translations
* **Translate on demand** - turn the floating overlay on when you need it
* **Summarize on demand** after a meeting (or skip it)
* **Export to Markdown** (`.md`) for notes elsewhere

<img width="2874" height="1995" alt="screenshot_recording" src="https://github.com/user-attachments/assets/2a79146c-2f8f-44d9-bd01-d325ef5d921b" />

<img width="3822" height="1209" alt="screenshot_library" src="https://github.com/user-attachments/assets/88f27b9c-91b9-4207-83c7-d0278b7ab9b7" />

## Requirements

| | Minimum | Notes |
|---|---|---|
| OS | Windows 10/11 (x64) or macOS 14.2+ (universal Intel + Apple Silicon) | WebView2 is already on most Windows PCs |
| RAM | **8 GB** | Uses the compact summary model; 16 GB+ can use the larger one |
| Disk | ~3 GB | App is small; models download on first use (~3 GB for the 8 GB set, ~4.5 GB if you also keep the 4B summary) |
| CPU | Modern x64 with AVX2, or Apple Silicon | Default models run on CPU; no GPU required |

The first-run wizard detects your RAM and only offers the summary model that fits. On machines under ~16 GB, the larger 4B summary model is not downloadable; on 16 GB+ you can switch between 2B and 4B in Settings.

## Install, data and uninstall

[Download](https://github.com/Jany-M/parla/releases) the latest build from GitHub, or build it yourself (see steps below).

### Windows

Install the `*-setup.exe` (per-user, no admin). App data lives in `%LOCALAPPDATA%\Parla`
(settings, meetings DB, downloaded models, llama.cpp runtime, logs, webview profile).

**Uninstall** (Windows Settings → Apps → Parla) stops any background model server and deletes that folder
plus the app's webview/roaming leftovers. In-app: Settings → “Delete all my data…” wipes the same data
without uninstalling. Updating the app keeps your data. The in-app **Update** button works on Windows.

### macOS

Open the universal `.dmg` and drag Parla to Applications. Builds are **unsigned** (no Apple Developer fee):

1. First launch: if Gatekeeper blocks the app, **right-click → Open** (once), or run:
   `xattr -dr com.apple.quarantine /Applications/Parla.app`
2. Grant **Microphone** in System Settings if the mic meter stays flat. For system audio (what other apps play), allow Screen Recording / system audio if macOS prompts.
3. Data lives in `~/Library/Application Support/Parla`.
4. In-app update is Windows-only (silent NSIS). On Mac the app opens the Releases page so you can install the new DMG; Gatekeeper steps above still apply. Update helper details land in `logs/update.log` (Open logs).

## Build

### Windows installer

```powershell
.\scripts\build.ps1              # tests + installer  -> release\Parla_<version>_x64-setup.exe
.\scripts\build.ps1 -SkipTests   # faster
.\scripts\build.ps1 -TestsOnly   # tests only
```

Missing Rust, Node.js, or VS C++ Build Tools are installed automatically via winget when possible.

### macOS DMG (must run on a Mac)

```bash
chmod +x ./scripts/build-macos.sh
./scripts/build-macos.sh                 # tests + universal DMG -> release/
./scripts/build-macos.sh --skip-tests    # faster
```

Missing Rust (rustup), Node.js 22+ (Homebrew), or Xcode Command Line Tools are installed automatically when possible. The script refuses to run on non-Darwin hosts. CI builds the same universal target on `macos-latest`.

## Tests

| Suite | Command |
|---|---|
| Rust core (pipeline, store, models, translation, summary…) | `cargo test -p parla-core --lib` |
| Real-model smoke test (optional) | set `PARLA_TEST_MODELS` to a folder with `stt/ vad/ speaker/ en.wav`, then `cargo test -p parla-core --features sherpa --test real_models` |
| Frontend logic + UI flows (mock backend) | `npm test` |

`npm run dev` opens the UI in a browser against the mock backend.

## Models

Downloaded on first use and run fully offline (see `resources/manifest.json`) for max privacy (nothing leaves your PC).

| Role | Model | Notes |
|---|---|---|
| Speech recognition (STT) | Parakeet TDT 0.6B v3 (int8) | 25 European languages incl. IT/EN/FR; CPU. CC-BY-4.0 (NVIDIA) |
| Voice activity (VAD) | Silero VAD | Finds where people are speaking. MIT |
| Speaker labels | WeSpeaker ResNet34 | Separates voices for per-person labels. Apache-2.0 |
| Live translation | Hy-MT2 1.8B (Q4_K_M) | Loaded only while translation is on. Apache-2.0 (Tencent) |
| Summaries (8 GB+) | Qwen3.5 2B (Q4_K_M) | Default on modest laptops. Apache-2.0 (Alibaba) |
| Summaries (16 GB+) | Qwen3.5 4B (Q4_K_M) | Offered only when RAM allows; switchable in Settings. Apache-2.0 (Alibaba) |
| LLM runtime | llama.cpp | Windows: CPU x64 zip. macOS: arch-matched arm64 or x64 tarball (Intel vs Apple Silicon). MIT |

You can also point translate/summarize slots at a local GGUF or an OpenAI-compatible endpoint (e.g. LM Studio, Ollama).

## License

[![License: MIT](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)

Use, modify, and redistribute freely (including commercially), provided you keep the copyright notice and license text so the original author is credited.

## Author

Jany Martelli

## Need a custom web / mobile / cloud / AI app developed?

See tech firm [Shambix](https://www.shambix.com/?utm_source=parla&utm_medium=referral&utm_campaign=projects&utm_content=github-readme) services or email [info@shambix.com](mailto:info@shambix.com).
