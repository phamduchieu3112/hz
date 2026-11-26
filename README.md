<div align="center">

# ⚡ Hz (Hiếu's Zalo)

🌐 **English** | [Tiếng Việt](./README.vi.md)

[![Rust](https://img.shields.io/badge/Rust-2021-CE422B?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![egui](https://img.shields.io/badge/GUI-egui%20%2F%20eframe-4F46E5?style=flat-square&logo=rust&logoColor=white)](https://github.com/emilk/egui)
[![ratatui](https://img.shields.io/badge/TUI-ratatui-059669?style=flat-square&logo=gnubash&logoColor=white)](https://ratatui.rs/)
[![SQLite](https://img.shields.io/badge/Storage-SQLite-003B57?style=flat-square&logo=sqlite&logoColor=white)](https://www.sqlite.org/)
[![Version](https://img.shields.io/badge/Version-0.1.3-2563EB?style=flat-square)](https://github.com/phamduchieu3112/hz)
[![License](https://img.shields.io/badge/License-MIT%20%7C%20Apache--2.0-64748B?style=flat-square)](#license)

</div>

> Native Rust Zalo desktop & terminal client (`egui` GUI & `ratatui` TUI) with local SQLite message storage, REST/MCP automation, and zero Electron or Node.js overhead.

<!-- Add screenshot here -->
<!-- ![Hz Desktop GUI](./docs/screenshot.png) -->

---

## 📌 Overview

| Field | Details |
| :--- | :--- |
| **Application** | **Hz** (`Hiếu's Zalo`) `v0.1.3` |
| **Interfaces** | Native GUI (`eframe` / `egui` `0.29`) · Terminal UI (`ratatui` `0.29`) · Headless CLI / API / MCP |
| **Storage** | Local per-account SQLite (`rusqlite` `0.32` bundled) · Offline history & full-text search |
| **Networking** | `tokio` `1` · `reqwest` `0.12` (`rustls-tls`) · `tokio-tungstenite` `0.24` · HTTP/SOCKS proxy support |
| **Media & Calls** | `cpal` · `opus` · `rusty_mp3` · `image` · Optional `ffmpeg` for H.264 video calls |
| **Automation** | Local REST API · MCP JSON-RPC 2.0 (`stdio` & HTTP) · Auto-reply · Scheduler · Rhai scripts · Telegram bridge |
| **License** | Dual-licensed under `MIT` or `Apache-2.0` |

---

## ✨ Key Features

- **Native Performance:** Pure Rust desktop GUI (`egui` + `glow`) and keyboard-driven TUI (`ratatui`) — lightweight memory footprint with no Electron or Node.js runtime.
- **Session & Multi-Account Support:** QR code and password login, persistent sessions, multi-account snapshots, plus `--profile` and `--data-dir` isolation.
- **Local History & Sync:** Per-account SQLite storage with Dual Sync V2/V1, offline browsing (`--offline`), fast local search, and conversation export.
- **Rich Messaging & Media:** Direct & group chats, `@mention` user tagging (autocomplete + UTF-16 `mentionInfo`), message pinning & top banner, 24h message recall, reactions, stickers, polls, document previews (`PDF`, Excel/Office, images, video), safe file upload & forwarding, group leave/disband management, social feed, and voice/video calls.
- **Documentation:** See [Features (VI)](./docs/TINH_NANG.md) and [Known Issues (VI)](./docs/DA_BIET.md).
- **Automation & AI Integration:**
  - **Local REST API & MCP Server** (`--api-port` / `--mcp`) for Claude Desktop, Cursor, or custom scripts.
  - **Scheduled Messages & Auto-Reply Rules** stored locally.
  - **LLM & Per-Thread Agent** (`--agent`) supporting OpenAI-compatible endpoints and local Ollama.
  - **Telegram ↔ Zalo Bridge** (`--tg-bridge`) with quota-managed local transfer buffer.

---

## 🏗️ Project Structure

```text
hz/
├── .cargo/config.toml         # Build env defaults (CMAKE_POLICY_VERSION_MINIMUM)
├── Cargo.toml                 # Crate manifest, dependencies & binaries
├── build.rs                   # Platform build script & Windows icon embedding
├── vendor/egui-winit/         # Patched egui-winit integration
└── src/
    ├── main.rs                # Entry point (GUI, TUI, MCP, REST API, Agent, Bridge)
    ├── lib.rs                 # Core library exports (`zalo_rust`)
    ├── gui.rs & src/gui/      # Native egui desktop UI (workspace, messages, modals, theme, tray, worker)
    ├── tui.rs & ui.rs         # Terminal interface (ratatui + crossterm)
    ├── chat.rs, feed.rs, poll.rs, reactions.rs, unread.rs
    │                          # Zalo messaging, feed, polls, reactions & read state
    ├── auth.rs, login.rs, qr.rs, session.rs, crypto.rs, param_cipher.rs, zproto_host.rs
    │                          # Authentication, session lifecycle & protocol encryption
    ├── store.rs, history_sync.rs, data_dir.rs, account_manager.rs
    │                          # SQLite persistence, Dual Sync V2/V1 & multi-account data dirs
    ├── call.rs, audio.rs, media.rs, doc_preview.rs
    │                          # VoIP/video calling, audio codecs, media cache & document previews
    ├── api_mcp.rs, agent.rs, llm.rs, scheduler.rs, thread_script.rs, tg_bridge.rs
    │                          # Local REST API, MCP server, LLM agent, scheduler, Rhai & Telegram bridge
    ├── assets/                # App icons, bundled fonts (BeVietnamPro, OpenSans, NotoEmoji, Lucide), stickers
    └── bin/                   # Diagnostic & probe utilities (`compare`, `probe_media`, `call_probe`, etc.)
```

---

## 🚀 Prerequisites & Build

### Prerequisites

| Requirement | Details |
| :--- | :--- |
| **Rust Toolchain** | `rustc >= 1.75` and `cargo` |
| **Windows Build Tools** | Visual Studio C++ Build Tools (MSVC `x86/x64`) & CMake |
| **Optional** | `ffmpeg` on `PATH` for H.264 video calls |

> **Note:** `.cargo/config.toml` already sets `CMAKE_POLICY_VERSION_MINIMUM = "3.5"` for compatibility with CMake `>= 4.0`.

### Build & Run

```powershell
# Build release binary (target\release\hz.exe on Windows, target/release/hz on Linux/macOS)
cargo build --release --bin hz

# Run Native GUI (default)
.\target\release\hz.exe

# Run Terminal UI (TUI) with QR login
.\target\release\hz.exe --tui --qr

# Run headless local REST API + MCP HTTP server
.\target\release\hz.exe --api-port 17890 --api-only

# Run MCP JSON-RPC 2.0 server over stdio
.\target\release\hz.exe --mcp
```

---

## ⌨️ CLI Options

| Flag | Description |
| :--- | :--- |
| `--gui` / `--tui` | Launch native `egui` GUI (default) or `ratatui` TUI |
| `--qr` | Sign in via QR code (TUI/CLI mode) |
| `--phone <NUM> --pass <PW> [--iso VN]` | Sign in via phone number and password |
| `--offline` | Start with local SQLite history without connecting to Zalo servers |
| `--logout` | Clear saved `session.json` and `threads_cache.json` |
| `--data-dir <PATH>` / `--profile <NAME>` | Use a custom data directory or isolated named profile |
| `--print-data-dir` | Print the resolved data directory path and exit |
| `--proxy <URL>` | Route session traffic through an HTTP/SOCKS proxy |
| `--api-port <PORT>` / `--api-only` | Start local REST + MCP HTTP server (default port `17890`) |
| `--mcp` | Run MCP JSON-RPC 2.0 server over `stdio` (optionally scoped with `--thread <ID>`) |
| `--agent --thread <ID>` | Run autonomous per-thread LLM agent (`--agent-send` to send, `--agent-poll <SECS>`) |
| `--auth status\|set <KEY>\|clear` | Inspect or manage the LLM API key in OS keychain |
| `--tg-bridge --tg-map "<MAP>"` | Bridge Telegram chats/topics to Zalo threads (`--tg-owner`, `--bridge-quota-gb`, `--media`, `--dry-run`) |
| `--dev` / `-v` | Enable verbose logging and protocol diagnostics |
| `--check-update` / `--version` | Check GitHub Releases or print current version |

---

## ⚙️ Environment Variables & Data Storage

### Environment Variables

| Variable | Purpose |
| :--- | :--- |
| `HZ_API_PORT` | Default port for local REST & MCP HTTP server (fallback: `17890`) |
| `HZ_API_TOKEN` | Optional bearer/header token to protect local REST & MCP HTTP endpoints |
| `HZ_API_KEY` | LLM API key override (takes precedence over settings & keychain) |
| `HZ_BASE_URL` | OpenAI-compatible LLM base URL override |
| `HZ_MODEL` | LLM model ID override |
| `OLLAMA_MODEL` | Default local Ollama model when using local fallback (default: `llama3.1`) |
| `TELEGRAM_BOT_TOKEN` | Telegram Bot API token for `--tg-bridge` mode |
| `HZ_ZPROTO_WASM` | Custom path to `libzproto` WASM module |
| `HZ_BOARD_TRACE` | Set to `1` to write group board call diagnostics to `board_trace.log` |

### Default Data Locations

| Platform | Default Storage Path |
| :--- | :--- |
| **Windows** | Current working directory (or path specified via `--data-dir` / `--profile`) |
| **macOS** | `~/Library/Application Support/hz` |
| **Linux** | `$XDG_DATA_HOME/hz` (fallback: `~/.local/share/hz`) |

---

## 🔌 Local REST API & MCP

When launched with `--api-port 17890`, Hz exposes a local-only HTTP server on `127.0.0.1`:

| Method & Endpoint | Description |
| :--- | :--- |
| `GET /api/v1/status` | Service health and active session status |
| `GET /api/v1/threads` | List cached conversations |
| `GET /api/v1/threads/{id}/messages` | Read message history for a thread |
| `GET /api/v1/search?q=<keyword>` | Search local SQLite message history |
| `POST /api/v1/messages/send` | Send a message to a conversation |
| `POST /api/v1/messages/schedule` | Schedule a message for later delivery |
| `GET /api/v1/scheduled` · `DELETE /api/v1/scheduled/{id}` | List or cancel scheduled messages |
| `GET /api/v1/autoreply` · `POST /api/v1/autoreply` | View or update auto-reply rules |
| `GET /api/v1/export` | Export conversation data |
| `POST /api/v1/mcp` | Model Context Protocol (MCP) JSON-RPC 2.0 endpoint |

---

## 🛡️ Security & Privacy

- All session credentials (`session.json`, `accounts/`), local databases (`*.db`), keys (`sync_v2_keys.json`), and user settings (`app_settings.json`, `autoreply_rules.json`) are stored locally and ignored by `.gitignore`.
- Sensitive fields (`zpw_sek`, `zpw_enk`, `imei`, proxy credentials, API keys) are redacted in logs.
- Local REST/MCP HTTP binds strictly to `127.0.0.1` and supports token authentication via `HZ_API_TOKEN`.

---

## 🔗 Author & Links

| Info | Details |
| :--- | :--- |
| **Author** | Phạm Đức Hiếu (`pdhieuuu`) |
| **Website** | [phamduchieu.site](https://phamduchieu.site/) |
| **Repository** | [github.com/phamduchieu3112/hz](https://github.com/phamduchieu3112/hz) |
| **Email** | [phamduchieu.robotics@gmail.com](mailto:phamduchieu.robotics@gmail.com) |
| **Discord** | `pdhieuuu` |

[![Website](https://img.shields.io/badge/Website-phamduchieu.site-4F46E5?style=flat-square&logo=googlechrome&logoColor=white)](https://phamduchieu.site/)<br>
[![Email](https://img.shields.io/badge/Email-phamduchieu.robotics%40gmail.com-D14836?style=flat-square&logo=gmail&logoColor=white)](mailto:phamduchieu.robotics@gmail.com)<br>
[![Discord](https://img.shields.io/badge/Discord-pdhieuuu-5865F2?style=flat-square&logo=discord&logoColor=white)](#)

---

## License

Dual-licensed under **MIT** or **Apache-2.0** at your option.
