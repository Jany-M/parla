# Parla

Local-only meeting notes for Windows: records yourself or your meetings (any app), transcribes verbatim, labels speakers and translates them live in a floating overlay, then summarizes on demand.

Local AI/LLMs only (max privacy), no accounts, no subscriptions, no cloud, free forever.

## Support this Project if you like it

[![Buy Me A Coffee](https://img.shields.io/badge/Buy%20Me%20A%20Coffee-shambix-FFDD00?style=flat-square&logo=buy-me-a-coffee&logoColor=black)](https://www.buymeacoffee.com/shambix) or [![Sponsor this on GitHub](https://img.shields.io/github/sponsors/Jany-M?label=Sponsor&logo=githubsponsors&logoColor=white&color=EA4AAA)](https://github.com/sponsors/Jany-M)

## Models

Downloaded on first use and run fully offline (see `resources/manifest.json`) for max privacy (nothing leaves your PC).

| Role | Model | Notes |
|---|---|---|
| Speech recognition (STT) | Parakeet TDT 0.6B v3 (int8) | 25 European languages incl. IT/EN/FR; CPU. CC-BY-4.0 (NVIDIA) |
| Voice activity (VAD) | Silero VAD | Finds where people are speaking. MIT |
| Speaker labels | WeSpeaker ResNet34 | Separates voices for per-person labels. Apache-2.0 |
| Live translation | Hy-MT2 1.8B (Q4_K_M) | Loaded only while translation is on. Apache-2.0 (Tencent) |
| Summaries (8 GB+) | Qwen3.5 2B (Q4_K_M) | Compact notes model. Apache-2.0 (Alibaba) |
| Summaries (16 GB+) | Qwen3.5 4B (Q4_K_M) | Higher-quality notes when RAM allows. Apache-2.0 (Alibaba) |
| LLM runtime | llama.cpp (Windows CPU build) | Serves translation and summary GGUFs locally. MIT |

You can also point translate/summarize slots at a local GGUF or an OpenAI-compatible endpoint (e.g. LM Studio, Ollama).

## Install, data and uninstall

Download and install the latest .exe from GitHub, or build it yourself (see step below).

* Per-user install (no admin). Everything the app creates lives in `%LOCALAPPDATA%\Parla`
  (settings, meetings DB, downloaded models, llama.cpp runtime, logs, webview profile).
* **Uninstall** (Windows Settings → Apps → Parla) stops any background model server and deletes that folder
  plus the app's webview/roaming leftovers. In-app: Settings → “Delete all my data…” wipes the same data
  without uninstalling. Updating the app keeps your data.

## Build the installer (Windows)

```powershell
.\scripts\build.ps1              # tests + installer  -> release\Parla_<version>_x64-setup.exe
.\scripts\build.ps1 -SkipTests   # faster
.\scripts\build.ps1 -TestsOnly   # tests only
```

Prerequisites are checked by the script (Rust MSVC, Node 20+, VS Build Tools C++ workload).

## Tests

| Suite | Command |
|---|---|
| Rust core (pipeline, store, models, translation, summary…) | `cargo test -p parla-core --lib` |
| Real-model smoke test (optional) | set `PARLA_TEST_MODELS` to a folder with `stt/ vad/ speaker/ en.wav`, then `cargo test -p parla-core --features sherpa --test real_models` |
| Frontend logic + UI flows (mock backend) | `npm test` |

`npm run dev` opens the UI in a browser against the mock backend.

## License

[MIT](LICENSE): use, modify, and redistribute freely (including commercially), provided you keep the copyright notice and license text so the original author is credited.

## Author

Jany  Martelli


## Need a custom web / mobile / cloud / AI app developed?

See tech firm [Shambix](https://www.shambix.com) services or email [info@shambix.com](mailto:info@shambix.com).
