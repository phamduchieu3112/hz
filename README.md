# Hz (Hiếu's Zalo)

Native Rust Zalo desktop client (`egui` GUI & `ratatui` TUI) with local SQLite message storage — no Electron, no Node.js.

- **Author:** Phạm Đức Hiếu (`pdhieuuu`)
- **Email:** phamduchieu.robotics@gmail.com
- **Website:** https://phamduchieu.site/

## Prerequisites (Windows)

- Rust toolchain (`rustc >= 1.75`, `cargo`)
- Visual Studio C++ Build Tools (MSVC `x86/x64`)
- CMake (if CMake `>= 4.0`, set `CMAKE_POLICY_VERSION_MINIMUM=3.5`)
- Optional: `ffmpeg` on `PATH` for H.264 video calls

## Build & Run

```powershell
# Build release binary (target\release\hz.exe)
$env:CMAKE_POLICY_VERSION_MINIMUM = "3.5"
cargo build --release --bin hz

# Run GUI
.\target\release\hz.exe

# Run TUI with QR login
.\target\release\hz.exe --tui --qr
```
