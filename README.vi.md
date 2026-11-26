<div align="center">

# ⚡ Hz (Hiếu's Zalo)

🌐 [English](./README.md) | **Tiếng Việt**

[![Rust](https://img.shields.io/badge/Rust-2021-CE422B?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![egui](https://img.shields.io/badge/GUI-egui%20%2F%20eframe-4F46E5?style=flat-square&logo=rust&logoColor=white)](https://github.com/emilk/egui)
[![ratatui](https://img.shields.io/badge/TUI-ratatui-059669?style=flat-square&logo=gnubash&logoColor=white)](https://ratatui.rs/)
[![SQLite](https://img.shields.io/badge/Storage-SQLite-003B57?style=flat-square&logo=sqlite&logoColor=white)](https://www.sqlite.org/)
[![Version](https://img.shields.io/badge/Version-0.1.3-2563EB?style=flat-square)](https://github.com/phamduchieu3112/hz)
[![License](https://img.shields.io/badge/License-MIT%20%7C%20Apache--2.0-64748B?style=flat-square)](#giấy-phép)

</div>

> Ứng dụng Zalo Desktop & Terminal viết bằng Rust thuần (`egui` GUI & `ratatui` TUI) với lưu trữ tin nhắn SQLite cục bộ, tích hợp REST/MCP tự động hóa — không Electron, không Node.js.

<!-- Thêm ảnh chụp màn hình tại đây -->
<!-- ![Giao diện Hz Desktop](./docs/screenshot.png) -->

---

## 📌 Tổng quan

| Mục | Chi tiết |
| :--- | :--- |
| **Ứng dụng** | **Hz** (`Hiếu's Zalo`) `v0.1.3` |
| **Giao diện** | Native GUI (`eframe` / `egui` `0.29`) · Terminal UI (`ratatui` `0.29`) · Headless CLI / API / MCP |
| **Lưu trữ** | SQLite cục bộ tách riêng theo tài khoản (`rusqlite` `0.32` tích hợp sẵn) · Xem lịch sử offline & tìm kiếm |
| **Kết nối mạng** | `tokio` `1` · `reqwest` `0.12` (`rustls-tls`) · `tokio-tungstenite` `0.24` · Hỗ trợ HTTP/SOCKS proxy |
| **Media & Cuộc gọi** | `cpal` · `opus` · `rusty_mp3` · `image` · Tùy chọn `ffmpeg` cho cuộc gọi video H.264 |
| **Tự động hóa** | Local REST API · MCP JSON-RPC 2.0 (`stdio` & HTTP) · Trả lời tự động · Hẹn giờ gửi · Script Rhai · Cầu nối Telegram |
| **Giấy phép** | Giấy phép kép `MIT` hoặc `Apache-2.0` |

---

## ✨ Tính năng nổi bật

- **Hiệu năng Native:** Giao diện đồ họa Rust thuần (`egui` + `glow`) và giao diện dòng lệnh (`ratatui`) — tiêu thụ RAM thấp, khởi động nhanh, không cần Electron hay Node.js.
- **Quản lý phiên & Đa tài khoản:** Đăng nhập bằng mã QR hoặc mật khẩu, lưu phiên cục bộ, chuyển đổi nhiều tài khoản, hỗ trợ tách biệt dữ liệu qua `--profile` và `--data-dir`.
- **Lịch sử cục bộ & Đồng bộ:** Lưu trữ tin nhắn trong SQLite theo từng tài khoản, hỗ trợ Dual Sync V2/V1, đọc tin nhắn khi ngoại tuyến (`--offline`), tìm kiếm nhanh và xuất dữ liệu hội thoại.
- **Nhắn tin & Đa phương tiện đầy đủ:** Chat cá nhân và nhóm, thả cảm xúc, nhãn dán, bình chọn (poll), xem trước tài liệu (`PDF`, Excel/Office, ảnh, video), Nhật ký (Feed), gọi thoại và gọi video.
- **Tự động hóa & Tích hợp AI:**
  - **Local REST API & MCP Server** (`--api-port` / `--mcp`) để kết nối Claude Desktop, Cursor hoặc script tự động hóa.
  - **Hẹn giờ gửi tin & Trả lời tự động** lưu trữ hoàn toàn cục bộ.
  - **Trợ lý LLM & Agent theo hội thoại** (`--agent`) hỗ trợ API tương thích OpenAI và Ollama cục bộ.
  - **Cầu nối Telegram ↔ Zalo** (`--tg-bridge`) kèm bộ đệm truyền tệp có giới hạn dung lượng (quota LRU).

---

## 🏗️ Cấu trúc dự án

```text
hz/
├── .cargo/config.toml         # Biến môi trường build mặc định (CMAKE_POLICY_VERSION_MINIMUM)
├── Cargo.toml                 # Khai báo crate, dependencies & các binary
├── build.rs                   # Build script theo nền tảng & nhúng icon Windows
├── vendor/egui-winit/         # Bản vá tích hợp egui-winit
└── src/
    ├── main.rs                # Điểm khởi chạy (GUI, TUI, MCP, REST API, Agent, Bridge)
    ├── lib.rs                 # Thư viện lõi (`zalo_rust`)
    ├── gui.rs & src/gui/      # Giao diện egui (workspace, tin nhắn, modal, theme, khay hệ thống, worker)
    ├── tui.rs & ui.rs         # Giao diện dòng lệnh TUI (ratatui + crossterm)
    ├── chat.rs, feed.rs, poll.rs, reactions.rs, unread.rs
    │                          # Nhắn tin Zalo, tường nhà, bình chọn, cảm xúc & trạng thái chưa đọc
    ├── auth.rs, login.rs, qr.rs, session.rs, crypto.rs, param_cipher.rs, zproto_host.rs
    │                          # Xác thực, vòng đời phiên & mã hóa giao thức
    ├── store.rs, history_sync.rs, data_dir.rs, account_manager.rs
    │                          # Lưu trữ SQLite, Dual Sync V2/V1 & thư mục dữ liệu đa tài khoản
    ├── call.rs, audio.rs, media.rs, doc_preview.rs
    │                          # Gọi thoại/video, codec âm thanh, bộ đệm media & xem trước tài liệu
    ├── api_mcp.rs, agent.rs, llm.rs, scheduler.rs, thread_script.rs, tg_bridge.rs
    │                          # REST API, MCP server, LLM agent, hẹn giờ gửi, Rhai & cầu nối Telegram
    ├── assets/                # Icon ứng dụng, phông chữ nhúng (BeVietnamPro, OpenSans, NotoEmoji, Lucide), sticker
    └── bin/                   # Công cụ chẩn đoán & kiểm thử (`compare`, `probe_media`, `call_probe`, v.v.)
```

---

## 🚀 Yêu cầu & Biên dịch

### Yêu cầu môi trường

| Thành phần | Chi tiết |
| :--- | :--- |
| **Rust Toolchain** | `rustc >= 1.75` và `cargo` |
| **Công cụ build Windows** | Visual Studio C++ Build Tools (MSVC `x86/x64`) và CMake |
| **Tùy chọn** | `ffmpeg` trong `PATH` để hỗ trợ gọi video H.264 |

> **Lưu ý:** File `.cargo/config.toml` đã cấu hình sẵn `CMAKE_POLICY_VERSION_MINIMUM = "3.5"` để tương thích với CMake `>= 4.0`.

### Biên dịch & Chạy ứng dụng

```powershell
# Biên dịch bản release (target\release\hz.exe trên Windows, target/release/hz trên Linux/macOS)
cargo build --release --bin hz

# Chạy giao diện đồ họa GUI (mặc định)
.\target\release\hz.exe

# Chạy giao diện Terminal (TUI) và đăng nhập QR
.\target\release\hz.exe --tui --qr

# Chạy máy chủ REST API + MCP HTTP không giao diện (headless)
.\target\release\hz.exe --api-port 17890 --api-only

# Chạy máy chủ MCP JSON-RPC 2.0 qua stdio
.\target\release\hz.exe --mcp
```

---

## ⌨️ Tham số dòng lệnh (CLI)

| Tham số | Mô tả |
| :--- | :--- |
| `--gui` / `--tui` | Mở giao diện đồ họa `egui` (mặc định) hoặc giao diện dòng lệnh `ratatui` |
| `--qr` | Đăng nhập bằng mã QR (chế độ TUI/CLI) |
| `--phone <SĐT> --pass <MK> [--iso VN]` | Đăng nhập bằng số điện thoại và mật khẩu |
| `--offline` | Mở ứng dụng với lịch sử SQLite cục bộ, không kết nối máy chủ Zalo |
| `--logout` | Đăng xuất và xóa `session.json`, `threads_cache.json` |
| `--data-dir <PATH>` / `--profile <NAME>` | Chỉ định thư mục dữ liệu riêng hoặc tách biệt theo tên hồ sơ |
| `--print-data-dir` | In đường dẫn thư mục dữ liệu đang dùng và thoát |
| `--proxy <URL>` | Định tuyến kết nối phiên qua HTTP/SOCKS proxy |
| `--api-port <PORT>` / `--api-only` | Khởi chạy REST API + MCP HTTP cục bộ (cổng mặc định `17890`) |
| `--mcp` | Chạy máy chủ MCP JSON-RPC 2.0 qua `stdio` (có thể khóa vào 1 hội thoại với `--thread <ID>`) |
| `--agent --thread <ID>` | Chạy LLM agent tự động cho 1 hội thoại (`--agent-send` để gửi thật, `--agent-poll <GIÂY>`) |
| `--auth status\|set <KEY>\|clear` | Kiểm tra hoặc quản lý LLM API key trong OS keychain |
| `--tg-bridge --tg-map "<MAP>"` | Cầu nối Telegram ↔ Zalo (`--tg-owner`, `--bridge-quota-gb`, `--media`, `--dry-run`) |
| `--dev` / `-v` | Bật log chi tiết và công cụ chẩn đoán giao thức |
| `--check-update` / `--version` | Kiểm tra bản phát hành mới trên GitHub hoặc in phiên bản hiện tại |

---

## ⚙️ Biến môi trường & Lưu trữ dữ liệu

### Biến môi trường

| Biến môi trường | Công dụng |
| :--- | :--- |
| `HZ_API_PORT` | Cổng mặc định cho máy chủ REST & MCP HTTP cục bộ (mặc định: `17890`) |
| `HZ_API_TOKEN` | Token xác thực tùy chọn để bảo vệ các endpoint REST & MCP HTTP cục bộ |
| `HZ_API_KEY` | Ghi đè LLM API key (ưu tiên cao hơn cài đặt GUI và keychain) |
| `HZ_BASE_URL` | Ghi đè URL máy chủ LLM tương thích OpenAI |
| `HZ_MODEL` | Ghi đè tên model LLM |
| `OLLAMA_MODEL` | Model Ollama mặc định khi chạy cục bộ (mặc định: `llama3.1`) |
| `TELEGRAM_BOT_TOKEN` | Token Telegram Bot dùng cho chế độ `--tg-bridge` |
| `HZ_ZPROTO_WASM` | Đường dẫn tùy chỉnh tới file WASM `libzproto` |
| `HZ_BOARD_TRACE` | Đặt `1` để ghi log chẩn đoán bảng tin nhóm vào `board_trace.log` |

### Thư mục lưu trữ mặc định

| Hệ điều hành | Đường dẫn lưu trữ mặc định |
| :--- | :--- |
| **Windows** | Thư mục làm việc hiện tại (hoặc đường dẫn chỉ định qua `--data-dir` / `--profile`) |
| **macOS** | `~/Library/Application Support/hz` |
| **Linux** | `$XDG_DATA_HOME/hz` (mặc định dự phòng: `~/.local/share/hz`) |

---

## 🔌 Local REST API & MCP

Khi khởi chạy với `--api-port 17890`, Hz mở máy chủ HTTP cục bộ chỉ lắng nghe trên `127.0.0.1`:

| Phương thức & Endpoint | Mô tả |
| :--- | :--- |
| `GET /api/v1/status` | Trạng thái hoạt động và thông tin phiên hiện tại |
| `GET /api/v1/threads` | Lấy danh sách hội thoại đã lưu cache |
| `GET /api/v1/threads/{id}/messages` | Đọc lịch sử tin nhắn của một hội thoại |
| `GET /api/v1/search?q=<từ_khóa>` | Tìm kiếm tin nhắn trong cơ sở dữ liệu SQLite cục bộ |
| `POST /api/v1/messages/send` | Gửi tin nhắn tới một hội thoại |
| `POST /api/v1/messages/schedule` | Đặt lịch gửi tin nhắn tự động |
| `GET /api/v1/scheduled` · `DELETE /api/v1/scheduled/{id}` | Xem danh sách hoặc hủy tin nhắn đã hẹn giờ |
| `GET /api/v1/autoreply` · `POST /api/v1/autoreply` | Xem hoặc cập nhật quy tắc trả lời tự động |
| `GET /api/v1/export` | Xuất dữ liệu hội thoại |
| `POST /api/v1/mcp` | Endpoint Model Context Protocol (MCP) JSON-RPC 2.0 |

---

## 🛡️ Bảo mật & Quyền riêng tư

- Toàn bộ dữ liệu phiên (`session.json`, `accounts/`), cơ sở dữ liệu cục bộ (`*.db`), khóa đồng bộ (`sync_v2_keys.json`) và cấu hình cá nhân (`app_settings.json`, `autoreply_rules.json`) đều được lưu cục bộ và chặn bởi `.gitignore`.
- Các trường nhạy cảm (`zpw_sek`, `zpw_enk`, `imei`, thông tin proxy, API key) được tự động che ẩn khi ghi log.
- Máy chủ REST/MCP HTTP chỉ lắng nghe nội bộ tại `127.0.0.1` và hỗ trợ xác thực qua `HZ_API_TOKEN`.

---

## 🔗 Tác giả & Liên hệ

| Mục | Chi tiết |
| :--- | :--- |
| **Tác giả** | Phạm Đức Hiếu (`pdhieuuu`) |
| **Website** | [phamduchieu.site](https://phamduchieu.site/) |
| **Mã nguồn** | [github.com/phamduchieu3112/hz](https://github.com/phamduchieu3112/hz) |
| **Email** | [phamduchieu.robotics@gmail.com](mailto:phamduchieu.robotics@gmail.com) |
| **Discord** | `pdhieuuu` |

[![Website](https://img.shields.io/badge/Website-phamduchieu.site-4F46E5?style=flat-square&logo=googlechrome&logoColor=white)](https://phamduchieu.site/)<br>
[![Email](https://img.shields.io/badge/Email-phamduchieu.robotics%40gmail.com-D14836?style=flat-square&logo=gmail&logoColor=white)](mailto:phamduchieu.robotics@gmail.com)<br>
[![Discord](https://img.shields.io/badge/Discord-pdhieuuu-5865F2?style=flat-square&logo=discord&logoColor=white)](#)

---

## Giấy phép

Phát hành theo giấy phép kép **MIT** hoặc **Apache-2.0** tùy bạn lựa chọn.
