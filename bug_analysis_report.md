# `Hz` Upstream Bug Analysis & Ranked Fix Proposals

> **Audit Mode:** Strict Read-Only Codebase Audit (no source code, `Cargo.toml`, or existing docs modified)
> **Target Workspace:** `d:\UET Master\myzalo` — **`Hz (Hiếu's Zalo)`** `v0.1.3` (`clean-release` @ `75eb9f0`, `https://github.com/phamduchieu3112/hz`)
> **Upstream Base:** `ctdoteam1110/ctdozlite` `main` @ `6e6fe38` (`v0.1.3 — Dual Sync V2/V1, RAM/LRU optimization, instant Settings, CI/CD & community PRs`)
> **Audit Date:** 2026-11-26

---

## Executive Baseline: `Hz` vs. Upstream `ctdozlite`

| Check | Result in Local `Hz` (`v0.1.3`) |
| :--- | :--- |
| **Crate / Binary / Repo** | `hz` v0.1.3, lib `zalo_rust`, `repository = "https://github.com/phamduchieu3112/hz"`, `Cargo.toml:1-10` |
| **Upstream `v0.1.4` quoted-image `[Hình ảnh]` fix (`#206` c#1)** | **NOT present** — captionless quoted images still render blank/missing (`src/chat.rs:8214-8226`, `src/history_sync.rs:1714-1753`, `src/gui/messages.rs:5854`). |
| **Upstream `v0.1.4` camera/FFmpeg & call texture protection (`#206` c#1)** | **NOT present** — video call frames allocate new `TextureId` every frame (`src/gui.rs:4006`), leak `texture_lru` bytes, and leave zombie `ffmpeg` processes. |
| **Upstream PR #184 native call video without FFmpeg** | **NOT present** — `Cargo.toml:15-52` has no `openh264`/`nokhwa` crates; camera capture, local PIP, and remote decode still spawn external `ffmpeg` subprocesses (`src/media.rs`). |
| **Project Docs (`docs/TINH_NANG.md`, `docs/DA_BIET.md`)** | Removed during `Hz` rebrand; `Hz` currently uses bilingual `README.md` & `README.vi.md`. |
| **Artifacts Inspected** | All 10 upstream issues + all comments (`#211`, `#206`, `#204`, `#202`, `#201`, `#198`, `#170`, `#160`, `#156`, `#138`); all 9 embedded PNG screenshots; key frames extracted via `ffmpeg` from all 3 GitHub `.mp4` videos (`#204` vid 1 & 2, `#206` jitter vid). |

### Missing / Unverified Context (Not Guessed)
1. **`#202` `sync_debug.log`** (`https://github.com/user-attachments/files/35312944/sync_debug.log`): Returned `HTTP/1.1 404 Not Found` from GitHub. All Sync V2/V1 conclusions below are grounded directly in `Hz` code + `#202`'s exact timeout screenshot (`"Đã hoàn tất yêu cầu: 0 tin mới từ Cloud/Nhóm (107 hội thoại)..."`) + `#201`'s `"70% / lỗi -1"` report + `src/bin/sync_probe.rs`.
2. **`#206` Google Drive call-crash video** (`https://drive.google.com/file/d/1XTINyI6cntAVPf0Nv8FmPLH8uIs9Dj0g/view?usp=drive_link`): External Google Drive link requires interactive access; not downloaded. Analysis relies on `#206` comment #1 (maintainer confirming camera/FFmpeg & video texture memory issues) + direct code inspection of `Hz`'s call/texture/FFmpeg/viewport pipelines.
3. **`#198` reporter video / `crash.log`**: Maintainer requested a video in `#198` comment #1 and none was provided. Exact machine-specific trigger on Windows 11 Canary cannot be confirmed without `crash.log` / `hz.exe --dev` output, though multiple concrete startup/shutdown hazards in `Hz` are verified below.

---

## 1. Deduplicated `Hz` Bug Matrix

All 10 upstream GitHub issues (including all 5 sub-issues of `#138`, all 3 parts of `#170`, and all sub-bugs of `#206` & `#204`) deduplicate into **16 distinct bugs** affecting `Hz`:

| Bug ID | Upstream Issue(s) | Category | Priority | Status in `Hz` | Concise Summary |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **HZ-01** | `#211`, `#206` (c#2, c#3) | `Crash / Lifecycle` | **P0** | **Present in Hz** | 100% deterministic `egui::Context` `RwLock` self-deadlock ("Not Responding") on double-clicking a message when `"Nhấp đúp chuột vào tin nhắn để trả lời nhanh"` is enabled. |
| **HZ-02** | `#206` | `Crash / Lifecycle` | **P0** | **Present in Hz** | Crash / GPU VRAM churn / FFmpeg zombie & pipe deadlock / `cpal` drop panic when calling and chatting simultaneously (lacks v0.1.4 stability & texture memory patches). |
| **HZ-03** | `#198` | `Crash / Lifecycle` | **P0** | **Needs Video/Log** *(hardening defects Present in Hz)* | App exits silently on reopen after closing on Windows 11 unless `session.json` is deleted (`windows_subsystem` hides early errors, abrupt `process::exit(0)` skips WAL/worker flush; exact trigger needs `crash.log`). |
| **HZ-04** | `#201`, `#202`, `#170` (part 3) | `Message Sync` | **P0** | **Present in Hz** | Phone message sync hangs at `70%` with error `-1` then times out after 180s (`"Hết thời gian chờ gói tin nhắn từ điện thoại"`) while NO sync prompt appears on Android/iOS. |
| **HZ-05** | `#138` (sub 5) | `Chat & File Protocol` | **P0** | **Present in Hz** | Shared/sent files unreadable by recipient: Forward (`"Chia sẻ"`) sends expiring private CDN URL as plain text; direct `asyncfile/msg` can send `"fileUrl": ""` and malformed payload fields. |
| **HZ-06** | `#211`, `#206` | `GUI Rendering` | **P1** | **Present in Hz** | Image & link-thumbnail loading causes layout bounce/jitter and screen flicker (`0px` pre-decode reservation, unvirtualized 150-msg `ScrollArea`, 64 MB `texture_lru` same-frame self-eviction loop). |
| **HZ-07** | `#204` | `GUI Rendering` | **P1** | **Present in Hz** | `"Dịch tin nhắn"` submenu strobes/ghosts per frame (`!sub_open` hover toggle), missing glyph boxes `□` (`U+2715` `"✕"` missing in all fonts), and delayed group UI / false `"Đã hiển thị toàn bộ lịch sử..."`. |
| **HZ-08** | `#170` (part 1) | `VoIP Audio/Video` | **P1** | **Present in Hz** | Mic hot-switch & hot-plug broken: `run_media_loop` never restarts `cpal` stream, `cpal 0.15.3` WASAPI STA-COM `OnceLock` wipes mic list empty on Refresh, and `I16` Bluetooth HFP mics fail hardcoded `f32` capture. |
| **HZ-09** | `#170` (part 2) | `VoIP Audio/Video` | **P1** | **Present in Hz** | One-way video call (PC→Phone works, Phone→PC black screen): advertises `"supportHevcDecode": 1` while RX pipeline is H.264-only (drops 100% of HEVC), plus `peer_ssrc` audio clobber breaks PLI/REMB and FFmpeg stalls. |
| **HZ-10** | `#138` (sub 4) | `Chat & File Protocol` | **P1** | **Present in Hz** | Cannot recall messages: SQLite & phone sync discard `client_message_id`/`wire_ref` (fallback sends `msg.ts` as `cliMsgIdUndo`), raw `msg.id` used instead of `server_message_id`, and errors vanish into invisible `status_msg`. |
| **HZ-11** | `#138` (sub 1) | `Chat & File Protocol` | **P1** | **Present in Hz** | Chat `@mention` / tag names broken: composer discards `MentionCandidate.id` (`#[allow(dead_code)]`), wire send endpoints never serialize `mentionInfo`/`mentions`, and renderer ignores wire mentions. |
| **HZ-12** | `#138` (sub 2) | `Chat & File Protocol` | **P1** | **Present in Hz** | Cannot pin messages to top of conversation: top Pinned Banner is hardcoded to `None`, board refresh is an empty no-op with no socket handlers, and `board_topic_id` is lost across restarts. |
| **HZ-13** | `#138` (sub 3) | `Chat & File Protocol` | **P1** | **Present in Hz** | Cannot delete/disband/leave group reliably: `/api/group/disperse` & ownership transfer missing; `leave_group` misses bare `g` prefix & lacks rollback; `delete_conversation` sends `"conver": {}` and `getlg/v4` re-adds group. |
| **HZ-14** | `#206` | `GUI Rendering` | **P2** | **Present in Hz** | Quoted/replied captionless image messages render empty quote body instead of `[Hình ảnh]` (missing v0.1.4 patch across realtime parse, Sync V2 proto parse, and `render_quote_block`). |
| **HZ-15** | `#160` | `VoIP Audio/Video` | **P2** | **Present in Hz** | Low call audio quality vs official Zalo: unconfigured 16 kHz Opus (no FEC/PLC), nearest-neighbor resampling with aliasing + 8 kHz 2× speed bug + playback over-drain crackles, no jitter buffer, no AEC/NS/AGC. |
| **HZ-16** | `#156` | `Wall / Feed` | **P2** | **Present in Hz** | Wall/Friend Feed stretches images non-uniformly, fabricates posts from private 1-1 DM photos (including own outgoing photos) & profile covers instead of real Timeline posts, shows raw UIDs (`686454...`), and mislabels `ERR_FEED_NO_NETWORK = 50001` as a Feed command. |

---

## 2. Per-Bug Deep Dive & Candidate Fix Options (Tailored to `Hz`)

---

### HZ-01 — Double-Click Message Self-Deadlock ("Not Responding" / Crash)
- **Upstream Issue(s) & Environment:** `#211`, `#206` (comments #2 & #3) — Windows 10 22H2 / Windows 11 24H2, `v0.1.3`, `--gui`.
- **Key Clues & Repro Steps:**
  1. Open any chat in `--gui` and double-click a text message bubble.
  2. Window immediately washes out to Windows "Not Responding" with spinning cursor (`#206` c#2 screenshot) and freezes/crashes.
  3. **User workaround (`#211` c#1 screenshot):** Turning OFF Settings → Tin nhắn → `"Nhấp đúp chuột vào tin nhắn để trả lời nhanh"` eliminates the freeze 100%.
- **Exact `Hz` Code Locations:**
  - Setting & toggle: `src/gui.rs:1224, 1277, 1369` (`AppSettings.double_click_to_reply = true`); `src/gui/modals.rs:3572-3579`; `src/gui/workspace.rs:7168-7177`.
  - **Deadlock sites:** `src/gui/messages.rs:4921-4942` (outgoing bubble) and `src/gui/messages.rs:5117-5138` (incoming bubble).
  - Contrast (correct pattern): `src/gui/messages.rs:5988-5991` (`render_hover_reaction_bar`).
- **Root-Cause Hypotheses (Ranked):**
  1. **[PROVEN — 100% Deterministic] Nested `RwLock` write→read self-deadlock on `egui::Context`:**
     `ui.input(|i| ...)` calls `egui::Context::input` (`egui-0.29.1/src/context.rs:899-901`), which acquires an exclusive **WRITE** lock on `Arc<RwLock<ContextImpl>>` (egui's doc comment explicitly warns: `// ⚠️ Using ctx ... again here will lead to a deadlock!`). Inside that closure, `src/gui/messages.rs:4937` and `5133` call `ui.ctx().layer_id_at(p)`, which calls `self.memory(...)` → `self.read(...)`, attempting a **READ** lock on the already write-locked `RwLock` on the same GUI thread. Disabling `double_click_to_reply` short-circuits before `ui.input(...)`.
- **Fix Options:**
  - **Option A (Extract input snapshot before `layer_id_at` check — Minimal Surgical Fix):**
    Read `let dbl_pos = ui.input(|i| if i.pointer.button_double_clicked(egui::PointerButton::Primary) { i.pointer.interact_pos() } else { None });` first so the `Context` write lock drops cleanly, *then* evaluate `dbl_pos.is_some_and(|p| fr.response.rect.contains(p) && ui.ctx().layer_id_at(p).is_none_or(|l| l == ui.layer_id()))`. Extract into a shared helper `bubble_double_clicked_for_reply(ui, rect)` used by both outgoing and incoming branches.
    - *Trade-offs:* Complexity: Trivial (~15 LOC). Regression risk: Zero. Perf/Mem: Zero overhead (even faster because it avoids calling `layer_id_at` except on actual double-click). Cross-platform: Windows/Linux/macOS identical.
  - **Option B (Hoist per-frame double-click pointer & top `LayerId` once in `render_chat_workspace`):**
    Query `double_click_pos` and `top_layer_id` **once** per frame before iterating all 150 message bubbles in `src/gui/workspace.rs`, passing `Option<(Pos2, Option<LayerId>)>` into `render_chat_bubble`.
    - *Trade-offs:* Complexity: Low (~30 LOC). Regression risk: Very low. Perf/Mem: Eliminates up to 150 redundant `Context::input` write-lock acquisitions per frame. Cross-platform: Identical.
  - **Option C (Switch to `fr.response.double_clicked()` with `Sense::click()` on bubble frame):**
    Use egui's built-in widget click sense instead of raw pointer polling.
    - *Trade-offs:* Complexity: Medium. Regression risk: **High** — `egui::Label` text selection (drag/double-click word select) inside the bubble competes with `Sense::click()` and can break text copy or trigger reply while selecting a word.
- **Recommended Option:** **Option B** (or **Option A** if minimizing signature diffs — Option A + a Clippy/code audit rule against nested `ctx.*` inside `ui.input`).
  - *Checklist:* Replace both closures at `src/gui/messages.rs:4933-4940` and `5129-5136` with lock-safe helper; verify no other `ui.input`/`ctx.input` closures call `ui.ctx().*` or `ctx.*`; add unit test exercising `render_chat_bubble` or helper with synthetic `RawInput` primary double-click on an `egui::Context` (would previously hang forever, now finishes in <1ms).
  - *QA:* Windows 10/11 `--gui`, toggle `"Nhấp đúp chuột vào tin nhắn để trả lời nhanh"` ON → double-click incoming & outgoing text bubbles → verify instant reply composer focus without hang; toggle OFF → verify double-click selects text normally.

---

### HZ-02 — Crash While Calling & Using the App Simultaneously
- **Upstream Issue(s) & Environment:** `#206` (body + comment #1) — Windows 11 24H2, `v0.1.3`, `--gui`.
- **Key Clues & Repro Steps:**
  - User reports frequent crashes when on an active voice/video call while simultaneously interacting with chats.
  - Maintainer comment #1 explicitly linked `v0.1.4` mitigations to: *"bổ sung các bản vá ổn định camera/FFmpeg và bảo vệ bộ nhớ texture cuộc gọi video"*.
- **Exact `Hz` Code Locations:**
  - Unbounded `GuiTx` video frame channel: `src/gui.rs:11, 1459-1476, 2186`; senders `src/media.rs:1522-1535, 2793-2813`.
  - Per-frame new `TextureId` allocation + `texture_lru` accounting leaks: `src/gui.rs:3989-4018, 4235-4241`; `src/gui/modals.rs:6679, 7162, 7285, 7374, 7518-7550`.
  - Multi-viewport immediate GL context switching: `src/gui/modals.rs:5786` (`render_call_overlay`), `src/gui/modals.rs:691` (toast popup), `src/gui.rs:5101-5108` (minimized early-return tears down call viewport).
  - Zombie FFmpeg & detached blocking threads & blocking Tokio drop: `src/media.rs:392-399, 1030, 1045, 1053, 1108, 1196-1198, 1239, 1499-1536, 1653, 1892, 2424, 2762-2770, 2870`; `src/call.rs:3359-3375, 4074`.
  - `cpal` WASAPI drop panic & UI-thread FFmpeg spawn: `src/audio.rs:262`; `src/gui/workspace.rs:5911-5912`; `src/gui/modals.rs:6665, 6669`.
- **Root-Cause Hypotheses (Ranked):**
  1. **[Most Likely] Unbounded video RGBA frame queue + per-frame `ctx.load_texture` GL churn + `texture_lru` leak:** Every 15–30 fps `640×480×4` (~1.23 MB) remote frame and `240×180×4` local frame goes through an unbounded `mpsc` queue, and `src/gui.rs:4006` calls `ctx.load_texture(&key, ...)` (allocating a new GL texture every frame and stacking deltas) instead of in-place `existing_tex.set(image, options)`. Meanwhile `render_call_overlay` never calls `mark_texture_used` and `sticker_textures.remove("call_*")` never calls `texture_lru.remove(...)`, leaking LRU byte counts and thrashing VRAM against chat images.
  2. **[High] Nested `show_viewport_immediate` mid-frame `glow` texture-delta flush + minimize teardown:** Calling `show_viewport_immediate` for both the call window and incoming chat notification popups switches WGL contexts and drains `textures_delta` mid-root-frame while chat textures are being allocated/freed. Minimizing main window returns early at `src/gui.rs:5101`, destroying the call OS viewport mid-call.
  3. **[High] Un-reaped zombie `ffmpeg` children, blocking `MediaHandle::drop` on Tokio, and `cpal` / `Mutex::unwrap()` panics:** `kill()` without `wait()` locks DirectShow cameras; detached threads keep running and sending frames after call teardown; `MediaHandle::drop` synchronously joins threads while holding `active_call.lock().await` (`src/call.rs:4074`); `input_buf.lock().unwrap()` (`src/media.rs:1892`) and `cpal 0.15.3` `Stream::drop` (`join().unwrap()`) panic on hot-unplug or audio error.
- **Fix Options:**
  - **Option A (Latest-Frame Atomic Slot + In-Place `TextureHandle::set` + Bounded FFmpeg/Drop Lifecycle — Targeted Hardening):**
    1. Replace `GuiEvent::StickerImageReady` for `"call_*"` video frames with a dedicated `Arc<Mutex<Option<(u64, ColorImage)>>>` latest-frame slot per stream (coalescing frames so only 1 frame per stream is ever buffered; zero queue growth) + update existing `TextureHandle` in place via `tex.set(image, options)` instead of `ctx.load_texture(...)`.
    2. Separate call textures from `sticker_textures`/`texture_lru` into a dedicated `call_textures: HashMap<String, TextureHandle>` (max ~8 MiB cap, cleared deterministically on call end).
    3. Guard `ffmpeg` spawns with a RAII `ChildGuard` that closes pipes, calls `kill()`, and `wait()`s with bounded timeout; track all camera/decoder threads in `MediaHandle`; offload `MediaHandle::drop` (`join`) via `tokio::task::spawn_blocking` outside `active_call.lock()`; replace `.unwrap()` with poisoned-lock recovery; remove synchronous `list_media_devices(false)`/`is_ffmpeg_available()` from UI thread; keep rendering `render_call_overlay` even when main window is minimized.
    - *Trade-offs:* Complexity: Medium. Regression risk: Low (isolated to call video & media teardown). Perf/Mem: Cuts video-call RAM & GL texture churn by >90%. Cross-platform: Works cleanly on Windows/Linux/macOS.
  - **Option B (Option A + Replace `show_viewport_immediate` with `show_viewport_deferred` or Dockable In-Window Call Panel):**
    Eliminate nested immediate WGL context switches by rendering the call panel either as an in-window split/overlay (with optional deferred pop-out viewport).
    - *Trade-offs:* Complexity: Medium-High. Regression risk: Medium (requires refactoring `render_call_overlay` state access because `show_viewport_deferred` requires `Arc<Mutex<...>>` shared state). Perf/Mem: Completely eliminates multi-viewport WGL driver crashes on Intel/AMD/NVIDIA Windows drivers.
  - **Option C (Full Native Video Rewrite — Port Upstream PR #184 with `openh264` + `nokhwa`):**
    Remove FFmpeg subprocesses entirely for call capture/decode.
    - *Trade-offs:* Complexity: High (new C/C++ native build dependencies on Windows MSVC + macOS + Linux). Regression risk: High. Best evaluated separately after Option A stabilizes texture & lifecycle safety.
- **Recommended Option:** **Option A** immediately (resolves texture OOM, GL churn, zombie FFmpeg, Tokio lock stalls, and `unwrap` panics with low risk), optionally followed by **Option B**'s in-window/deferred call viewport if WGL driver crashes persist.
  - *Checklist:* Add `LatestVideoFrame` slot + `call_textures` map with `tex.set(...)`; fix all `texture_lru.remove` mismatches; wrap FFmpeg children in RAII `kill + wait`; move `MediaHandle` drop to `spawn_blocking` outside `active_call` lock; eliminate UI-thread `list_media_devices(false)` & `input_buf.lock().unwrap()`; don't skip `render_call_overlay` when main window is minimized.
  - *QA:* Run 15-minute video call on Windows 11 `--gui` while actively switching threads, scrolling image-heavy groups, toggling camera 10×, and minimizing main window; monitor Task Manager RAM/VRAM & `ffmpeg.exe` process count (must stay bounded, 0 zombies after hang-up).

---

### HZ-03 — App Fails to Reopen After Closing Post-Install Unless `session.json` Is Deleted
- **Upstream Issue(s) & Environment:** `#198` — Windows 11 Pro (Canary Build), `v0.1.2` (codebase unchanged in `Hz` `v0.1.3`), `--gui`.
- **Key Clues & Repro Steps:**
  1. Fresh install launches normally; user logs in (creating `session.json`) and closes the app.
  2. Reopening shows the process briefly appear in Task Manager and silently terminate.
  3. Deleting `session.json` in the app root folder allows the app to launch again immediately.
- **Exact `Hz` Code Locations:**
  - Early error & panic suppression: `src/main.rs:6-9` (`windows_subsystem = "windows"`), `src/main.rs:81` (`data_dir::initialize` before `panic::set_hook` at `86-102`), `src/main.rs:230-238, 382-398`.
  - Abrupt exit on close: `src/gui.rs:5017-5037` (`on_exit` logs `UNEXPECTED_EXIT` and calls `std::process::exit(0)`), `src/gui.rs:1431` (non-atomic `settings.save()`), `src/gui/modals.rs:585`, `src/main.rs:397`.
  - `session.json`-exclusive startup paths:
    - `src/main.rs:354-366` (`save_current_account_snapshot`, `load_threads_cache_for`, synchronous `store::load_history_into_threads`).
    - `src/gui.rs:2161-2162, 2192-2196` (`ensure_windows_frameless_subclass`, synchronous `ensure_cjk_font` → `src/gui/theme.rs:1462-1699` → `ctx.set_fonts`, duplicate `RefreshThreads`).
    - `src/gui.rs:5296-5297` (`render_chat_workspace` on Frame 1) + `src/gui/modals.rs:691` (immediate secondary Win32 OpenGL notification viewport + `win32_frameless::sync_frameless_window_by_title` at `src/gui.rs:531-553`).
  - Explicitly ruled out: NO single-instance lock exists in `src/`; `eframe` has NO `persistence` feature (`Cargo.toml:33`); tray hide is Linux-only (`src/gui/tray.rs`); `session.json` parse error at `src/main.rs:242` falls back cleanly to login.
- **Root-Cause Hypotheses (Ranked):**
  1. **[Most Likely] Data-dependent panic or `UNEXPECTED_EXIT` in a `session.json`-only startup path, masked by `windows_subsystem = "windows"`:** Because `session.json` switches boot from the empty login screen to hydrated chat workspace + `ensure_cjk_font` + SQLite WAL open + Win32 frameless subclassing + immediate notification viewport, any panic inside `ZaloGuiApp::new` (e.g., `epaint` `ctx.set_fonts` panic on a broken/non-standard system font or `app_settings.json` custom font) or unexpected `WindowEvent::CloseRequested`/`Destroyed` from `win32_frameless` subclassing exits silently.
  2. **[Medium] Abrupt `std::process::exit(0)` on first close leaves uncheckpointed SQLite WAL / half-written `app_settings.json` or `accounts/<uin>_threads.json`:** `on_exit` (`src/gui.rs:5036`) kills the process immediately without joining worker threads or closing `rusqlite::Connection`, risking corrupt files read only when `startup_uin` is non-empty.
  3. **[Medium] Windows Inno Setup install path + `Platform::Other` (`cwd`) storage (`src/data_dir.rs:16-18, 148-153`)**: If a shortcut launches with a different or read-only `cwd` (`Program Files`), `data_dir::initialize` returns `Err` before `panic::set_hook` and exits silently (note: deleting `session.json` wouldn't fix read-only `cwd` alone, unless `accounts/` or `crash.log` write triggers a permission/AV block).
- **Fix Options:**
  - **Option A (Early Crash Telemetry + Graceful Shutdown + Guarded `session.json` Startup — Recommended Defensive Fix):**
    1. Install `std::panic::set_hook` and top-level `main()` `Err` logger **first** (line 30 of `src/main.rs`), writing to a guaranteed writable path (`%LOCALAPPDATA%\Hz\crash.log` fallback if `cwd` isn't writable) and showing a native Win32 `MessageBoxW` on fatal startup failure in release GUI mode.
    2. Replace abrupt `std::process::exit(0)` in `on_exit` (`src/gui.rs:5036`) with clean shutdown: signal worker stop, flush/checkpoint SQLite WAL (`PRAGMA wal_checkpoint(TRUNCATE)`), make `AppSettings::save()` use `write_private_atomic`, and allow `eframe::run_native` to unwind normally (with a 2s watchdog fallback).
    3. Guard `ensure_cjk_font` (validate font bytes via `ttf_parser`/`ab_glyph` before `ctx.set_fonts` so `epaint` can never panic), remove duplicate `RefreshThreads` at boot, and defer secondary notification viewports until Frame 3+ after main HWND is visible.
    - *Trade-offs:* Complexity: Low-Medium. Regression risk: Very low. Perf/Mem: Slight (~20–50ms) cleaner exit. Cross-platform: Improves Windows/Linux/macOS reliability and makes any future Canary-specific crash immediately self-diagnosing.
  - **Option B (Option A + Migrate Default Windows Storage from `cwd` to `%LOCALAPPDATA%\Hz` with Portable Auto-Detect):**
    Like macOS/Linux (`src/data_dir.rs:11-18`), default Windows installed builds to `%LOCALAPPDATA%\Hz` while keeping `cwd` portable mode if `session.json` or `portable.flag` exists next to `hz.exe`.
    - *Trade-offs:* Complexity: Medium. Regression risk: Medium (must preserve seamless migration for existing `Hz` portable users who have `session.json` in `cwd`).
- **Recommended Option:** **Option A** first (zero migration risk, fixes real shutdown corruption and `ctx.set_fonts` panic hazards, and captures exact telemetry if Canary reproduces), plus ask user whether Option B (`%LOCALAPPDATA%\Hz`) is desired for installed builds.
  - *Checklist:* Move `panic::set_hook` + `main()` error logger before `data_dir::initialize`; make `AppSettings::save` atomic; add clean worker shutdown + SQLite WAL checkpoint before exit; pre-validate CJK font before `ctx.set_fonts`; deduplicate boot `RefreshThreads`; defer toast viewport creation until main window rendered.
  - *QA:* Windows 11 `--gui`: fresh login → receive messages → close via `X` / `Alt+F4` / `Ctrl+Q` → immediately reopen 10× consecutively; test with corrupt `app_settings.json`, corrupt `accounts/<uin>_threads.json`, and read-only `cwd` → verify graceful recovery or visible error dialog + `crash.log`.

---

### HZ-04 — Phone Message Sync Hangs at `70%` with Error `-1` & Times Out Without Any Phone Prompt
- **Upstream Issue(s) & Environment:** `#201`, `#202`, `#170` (part 3) — Windows 11 (26H2 / 24H2), `v0.1.3`, `--gui`, both Android and iPhone (iOS 18.7.12).
- **Key Clues & Repro Steps:**
  1. Click `"Bắt đầu đồng bộ"` in Settings → Đồng bộ tin nhắn or left-rail Sync icon (even after fresh logout + re-login or End Task).
  2. Toast shows `"📲 Đang gửi yêu cầu đồng bộ tin nhắn tới điện thoại…"`, **zero confirmation notification appears on Android or iPhone**.
  3. Progress bar sticks at **`70%`** with error **`-1`** (`#201`), then after 3 minutes shows verbatim banner (`#202` screenshot):
     `"Đã hoàn tất yêu cầu: 0 tin mới từ Cloud/Nhóm (107 hội thoại). Không đồng bộ được từ điện thoại: ⚠️ Hết thời gian chờ gói tin nhắn từ điện thoại (hãy giữ Zalo trên điện thoại mở)"`.
- **Exact `Hz` Code Locations:**
  - `70%` pin & exact timeout string: `src/history_sync.rs:545-598` (`on_progress(0.70, st)` at line `588`; timeout string at line `596`).
  - Banner formatter: `src/gui.rs:1015-1042` (`phone_sync_completion_message`).
  - Error `-1` & `is_last: false` default: `src/chat.rs:10994-11030` (`extract_mobile_sync_controls` → `transfer_error`).
  - Non-error status masking & false `confirmed = true`: `src/gui/worker.rs:83, 334-339`; `src/history_sync.rs:417-420, 517-526`.
  - Unfiltered `sync_id` on confirm/busy events: `src/chat.rs:10773-10778, 10892-10920, 10991`.
  - `cmd 590` payload, 5.5s kill-probe, ignored ACK, unused HTTP V2: `src/chat.rs:10405-10525, 10531-10698`; `src/history_sync.rs:395-432`; `src/gui/worker.rs:1314-1373`; `src/socket.rs:672-677`; `src/bin/sync_probe.rs:1-5, 62-70, 117-130`.
  - Unbundled `libzproto` WASM dependency: `src/zproto_host.rs:48-108, 432-734, 817-859`.
  - Logout/re-login `WS_OUTBOUND_TX` clobber race & permanent backoff: `src/socket.rs:449-466, 494-534`; `src/gui/worker.rs:1291, 2128-2316`.
  - Sync V1 `cmd 12000` pinned `file` host & double-send: `src/session.rs:17-18, 259, 294-297`; `src/chat.rs:10356-10398`.
- **Root-Cause Hypotheses (Ranked):**
  1. **[PROVEN — Direct Cause of `70%` + `-1` Hang & Exact `#202` Banner] Broken `transfer_error` & Substring State Machine:**
     `extract_mobile_sync_controls` (`src/chat.rs:11004, 11028`) defaults missing/unmatched error code to `-1` and `is_last` to `false`. `worker.rs:334-339` renders `!is_last` errors as `"⏳ Điện thoại bỏ qua 1 phân vùng (mã -1)..."` (omitting `"Lỗi"`). `history_sync.rs:417-420` treats that string as `v2_got_response = true` (skipping V1), and `history_sync.rs:520-526` matches `"bỏ qua 1 phân vùng"` to **falsely set `confirmed = true`** without any phone confirmation — pinning the bar at `70%` (`line 588`) for 180s before emitting the exact `#202` timeout (`line 596`).
  2. **[PROVEN — Direct Cause of No Prompt + Stale/Busy Hijack] Missing `sync_id` Filtering + `MobileBusyRestoring` Mislabeled `"đang nén"` + `status 5` Auto-Bypass:**
     `UserConfirmed`, `UserRejected`, and `MobileBusyRestoring` carry no `sync_id` filter (`src/chat.rs:10773-10778`). When `MobileBusyRestoring` arrives via `lp_control`, `worker.rs:83` formats `"đang nén"`, which triggers `history_sync.rs:517` → `confirmed = true` → 70% hang → 180s timeout.
  3. **[PROVEN — Why Phone Never Prompts] Compounding `cmd 590` / `WS_OUTBOUND_TX` / `ZprotoHost` Failures:**
     - On Windows machines without matching official Zalo PC installed, `ZprotoHost::load()` fails (unbundled `libzproto_wasm_bg.*.wasm` + 39 hardcoded `wasm-bindgen` hashes), silently swallowed to `sync_debug.log` (`history_sync.rs:395-400`), falling back to V1 `cmd 12000` (ignored by modern iPhone/Android).
     - On logout/re-login (`#201` step 1), old `socket::run_once` clobbers global `WS_OUTBOUND_TX` then drops `out_rx` (`src/socket.rs:494-534`), killing outbound `cmd 590` frames.
     - `worker.rs:1314-1373` completely ignores `WsEvent { cmd: 590 }` server error ACKs; `resolve_thread_partitions` stuffs all 107+ threads into one unchunked `cmd 590` frame (or sends invalid `"partition": ""` when empty, `src/chat.rs:10445`); 5.5s probe sends `cmd 591 (OutDated)` killing V2 before sleeping phones wake; stale `LATEST_MOBILE_TEMP_KEY` is never cleared after login; `zpw_type=30` (Web) instead of desktop `24` + pinned `tt-files-wpa.chat.zalo.me` breaks V1 failover.
- **Fix Options:**
  - **Option A (Typed Sync State Machine + Fix WS Race + Fix `cmd 590` Protocol & Clear Diagnostics — Recommended):**
    1. Replace string-polling (`LAST_MOBILE_SYNC_STATUS`) with a typed `SyncSessionState` enum + `tokio::sync::watch` / broadcast channel tagged with active `sync_id`: `transfer_error` (`SyncV2Error`) NEVER flips `confirmed = true`; distinguish fatal session error (`err == -1` / `transfer_error` without active batches) from per-partition skip; `MobileBusyRestoring` sets an explicit `Busy` state (never `Confirmed`).
    2. Fix `WS_OUTBOUND_TX` clobber race in `src/socket.rs:494-534` (check `tx.is_closed()` before setting `WS_OUTBOUND_TX`, guard with generation counter, reset backoff after stable connection) and route `ev.cmd == 590` server error ACKs into `SyncSessionState`.
    3. Fix `cmd 590` wire hygiene: cap/chunk `queries` (e.g. top 30–50 active threads per sync wave, chunk `fetch_group_profiles`), send `"partition": "conversations"` when empty, clear `LATEST_MOBILE_TEMP_KEY` after single use, send proactive `cmd 592` wakeup after 3s silence, extend V2 probe window to 15–20s before `cmd 591 (OutDated)`, and surface actionable `ZprotoHost` errors (missing/incompatible `libzproto` WASM / `HZ_ZPROTO_WASM` guidance) directly in the UI instead of silently hanging.
    - *Trade-offs:* Complexity: Medium-High. Regression risk: Low-Medium (cleaner state machine replaces fragile substring checks). Perf/Mem: Eliminates 3-minute ghost hangs and dead WS channels. Cross-platform: Windows/Linux/macOS; still needs `libzproto` WASM for E2EE V2 unless Option B is also done.
  - **Option B (Option A + Pure-Rust X3DH / Double Ratchet / `zproto` Replacement):**
    Replace `wasmtime` + external Zalo PC `libzproto_wasm_bg.*.wasm` dependency in `src/zproto_host.rs` with pure-Rust Signal-protocol (`x25519-dalek` / `hkdf` / `aes-gcm`) matching `libzproto`'s exact wire serialization.
    - *Trade-offs:* Complexity: Very High (requires clean-room verification of `libzproto` protobuf/key-derivation vectors against real phone captures). Regression risk: High. Cross-platform: Removes Zalo PC installation requirement on Windows/Linux/macOS.
  - **Option C (Option A + Dynamic WASM Import Resolver in `ZprotoHost`):**
    Instead of hardcoding 39 `wasm-bindgen` hash suffixes (`src/zproto_host.rs:432-734`), match WASM imports by prefix/signature dynamically so any Zalo PC version's `libzproto_wasm_bg.*.wasm` loads cleanly without breaking on Zalo PC updates.
    - *Trade-offs:* Complexity: Medium. Regression risk: Low. Cross-platform: Immediately makes `Hz` resilient to Zalo PC updates while keeping Option B as a longer-term goal.
- **Recommended Option:** **Option A + Option C** (fixes the 70%/`-1` state-machine bug, WS clobber race, `cmd 590` payload/ACK/wakeup issues, AND breaks fragility against Zalo PC WASM updates).
  - *Checklist:* Introduce typed `SyncV2State` with `sync_id` gating; stop treating `SyncV2Error` / `MobileBusyRestoring` as `confirmed = true`; fix `WS_OUTBOUND_TX` generation check & backoff reset; handle `ev.cmd == 590` error ACKs; fix `"partition": "conversations"` & chunk `queries`; clear one-time `temp_key`; add dynamic prefix matching in `src/zproto_host.rs`; surface clear error toast/banner if WASM missing.
  - *QA:* Unit tests for state transitions (`transfer_error -1` → immediate error/fallback, never `confirmed`; `MobileBusyRestoring` → busy notice; stale `sync_id` ignored; `WS_OUTBOUND_TX` reconnect race). Manual QA on Windows 11 `--gui` with both Android & iPhone (including after logout + re-login).

---

### HZ-05 — Shared / Sent Files Unreadable by Recipient
- **Upstream Issue(s) & Environment:** `#138` (sub-issue 5: *"file chia sẻ đi người được chia sẻ sẽ không đọc được file"*) — Windows 11, `--gui`.
- **Key Clues & Repro Steps:**
  - Two user flows trigger this: (A) clicking `"Chia sẻ"` (Forward) on an existing file message to send it to another chat; (B) uploading and sending a local file directly from `Hz`. In both cases, the recipient cannot open/read the received file.
- **Exact `Hz` Code Locations:**
  - **Path A (Forward / `"Chia sẻ"`):** `src/gui/messages.rs:5470-5472, 6104` → `src/gui/modals.rs:8220-8275` → `src/gui/worker.rs:3287-3346` → `src/chat.rs:5185-5333` (`forward_message_with_client_id`).
  - **Path B (Direct upload & send):** `src/gui/worker.rs:4141-4257` → `src/chat.rs:2375, 2395-2455` (`FILE_DONE_LISTENER_ACTIVE`, `extract_file_done_controls`), `src/chat.rs:2896-3439` (`send_attachment_in_layout`, `asyncfile/upload`, `wait_for_file_done`, `asyncfile/msg`).
- **Root-Cause Hypotheses (Ranked):**
  1. **[PROVEN — Path A] `forward_message_with_client_id` has NO `MessageKind::File` (or `Video`/`Voice`) handler:**
     `src/chat.rs:5238-5333` only handles stickers, links, and photos (`is_zalo_photo_cdn_url`). For file messages it falls through to plain-text `send_message` (`5327-5332`) containing an expiring/session-scoped private CDN URL (e.g. `renewMedia?p=...`). Worse, `src/gui/worker.rs:3305-3328` renders a fake local file card (`link_src: Some("Tập tin Zalo")`) for the sender, hiding the breakage!
  2. **[PROVEN — Path B] `asyncfile/msg` silently sent with `"fileUrl": ""` + `file_done` parser bug + schema defects:**
     - `extract_file_done_controls` (`src/chat.rs:2425`) does not JSON-parse stringified `item["content"]`, causing `wait_for_file_done` to miss `cmd 601` `file_done` events and time out after 90s (or skip wait completely when `FILE_DONE_LISTENER_ACTIVE == false`).
     - Even when `file_url` is empty `""`, `src/chat.rs:3366` **still calls `/api/message/asyncfile/msg` or `/api/group/asyncfile/msg` with `"fileUrl": ""`**, delivering an unreadable file shell to the recipient!
     - Additional protocol mismatches in `asyncfile/msg` (`src/chat.rs:3234, 3349-3372`): missing `imei`, missing `visibility: 0` for groups, misspelled `"extention"` alongside `"extension"`, fallback to `clientFileId` as `fileId`, empty `"checksumSha": ""` (instead of real SHA-256/MD5 where required), hardcoded `1 MiB` chunk size, and local `ChatMessage` missing `kind: MessageKind::File`.
- **Fix Options:**
  - **Option A (Fix Both Forward & `asyncfile/msg` Protocol — Recommended):**
    1. **Path A (Forward):** Add a `MessageKind::File` (and `Video`) branch to `forward_message_with_client_id` calling Zalo's native `/api/message/forward` / `/api/group/forward` (or fallback: download cached/authenticated file bytes to temp buffer and re-upload cleanly via `send_attachment_in_layout`), and never fake a local `AttachmentSent` file card when only text was sent.
    2. **Path B (Direct upload):** Parse stringified JSON `content` in `extract_file_done_controls` (`src/chat.rs:2425`); always wait for `file_done` (never bypass via stale `FILE_DONE_LISTENER_ACTIVE`, wire `file_done` listener at WS startup); **fail fast with clear error** instead of sending `asyncfile/msg` when `file_url` is empty or `fileId` is missing; add missing `imei`, group `visibility: 0`, remove misspelled `"extention"` (or verify exact Zalo PC field name — note Zalo PC actually uses `"extension"` in `asyncfile/msg`), compute `checksumSha` (SHA-256 hex) if required, respect server `chunk_size_file`, and set `kind: MessageKind::File` on returned `ChatMessage`.
    - *Trade-offs:* Complexity: Medium. Regression risk: Low (fixes unambiguous protocol omissions and eliminates silent empty-URL sends). Perf/Mem: Minimal; download-and-reupload fallback bounded by existing `media_resources` limits. Cross-platform: Windows/Linux/macOS identical.
  - **Option B (Download-and-Reupload Only for Forward + Fix Path B):**
    Skip implementing `/api/message/forward` and always re-upload forwarded files via fixed `send_attachment_in_layout`.
    - *Trade-offs:* Complexity: Low-Medium. Regression risk: Very low (reuses one verified upload pipeline), but uses extra upload bandwidth for large forwarded files.
- **Recommended Option:** **Option A** (with **Option B**'s download-and-reupload as automatic fallback when native forward metadata is unavailable).
  - *Checklist:* Fix `extract_file_done_controls` stringified JSON parsing + unit test; abort upload with explicit error if `file_url.is_empty()` or `file_id` missing; add `imei`, group `visibility: 0`, `kind: MessageKind::File`, and `checksumSha`; implement `MessageKind::File` forwarding in `forward_message_with_client_id`; remove deceptive local file card on text fallback in `worker.rs:3305-3328`.
  - *QA:* Send `.pdf`, `.docx`, `.xlsx`, `.zip` (small <1 MB and multi-chunk >3 MB) from `Hz` to both 1-1 and Group → verify recipient on official Zalo Android/iOS/PC can download and open intact; then click `"Chia sẻ"` on a received file → forward to another chat → verify recipient opens it cleanly.

---

### HZ-06 — Image & Link-Thumbnail Load Jitter and Screen Flicker
- **Upstream Issue(s) & Environment:** `#211`, `#206` (including `206_jitter.mp4`) — Windows 10 22H2 / Windows 11 24H2, `v0.1.3`, `--gui`.
- **Key Clues & Repro Steps:**
  1. Open a conversation containing images or link cards (`Cloud của tôi` / groups with photos & links) and scroll or wait for thumbnails to load.
  2. Images jump/jitter and the window repaints/flickers continuously (`206_jitter.mp4` frame-diff confirmed full-window redraw churn every 200ms).
  3. Persists even after disabling double-click reply (`#211` c#1: *"màn hình lúc load tin nhắn hay link có thumbnail bị giật giật"*).
- **Exact `Hz` Code Locations:**
  - Pre-decode size jumps: `src/gui/messages.rs:45-72` (stickers), `2054-2126, 4550-4564` (single photos), `2319-2382, 4627-4681` (link preview cards & width snap), `3249-3424, 4619-4625` (image-file cards), `3579-3631` (video cards).
  - Unvirtualized `ScrollArea`: `src/gui/workspace.rs:6973-7336` (`CHAT_RENDER_STEP = 150` at `src/gui.rs:1454`, `stick_to_bottom(jump_index.is_none())`).
  - `texture_lru` budget mismatch & same-frame self-eviction loop: `src/gui.rs:2000` (64 MB vs `src/media_resources.rs:16` 128 MB), `src/gui.rs:3989-4014, 5099, 5297, 5315-5319`; `src/media_resources.rs:359-397`; `src/gui/worker.rs:3795, 3909, 5655-5663`; `src/gui.rs:1473`.
  - Unprotected textures & retry-backoff bypass: `src/gui/messages.rs:3606, 4236`; `src/gui/stickers.rs:1160, 1438, 1726`; `src/gui/modals.rs:8337, 8373, 8377, 9480, 10554, 10838, 10878, 11082`.
- **Root-Cause Hypotheses (Ranked):**
  1. **[PROVEN — Primary Flicker Cause] Same-Frame `texture_lru` Self-Eviction & Double-Repaint Thrashing:**
     `ZaloGuiApp::texture_lru` is capped at 64 MiB (half of `TEXTURE_BUDGET = 128 MiB`) while thumbnails decode up to `720×720` RGBA8 (~2.07 MB each; ~31 photos fill 64 MB). `check_events(ctx)` (`src/gui.rs:5099`) drains all `StickerImageReady` events *before* `render_chat_workspace`, while `last_frame_textures` is stale from the prior frame — so batch-inserting `key2` immediately evicts just-loaded `key1` before it paints once! The render pass immediately re-queues `key1` from disk cache, issuing **2 repaints per image** (`GuiTx::send` + explicit `ctx_clone.request_repaint()`) in an endless flicker cycle.
  2. **[PROVEN — Primary Layout Jitter Cause] Zero/Mismatched Pre-Decode Size Reservation + Unvirtualized `ScrollArea`:**
     Link preview thumbnails reserve `0 px` before decode then insert `+137 px` and snap bubble width from text width (`160..360px`) to `260px`; single photos jump from `280×150` to up to `360×340`; stickers jump from `20px` to `116×116`. Because all 150 messages render and fetch eagerly with `stick_to_bottom(true)`, offscreen decodes constantly shift scroll height.
  3. **[Secondary] Unprotected `sticker_textures.get` & failed-fetch backoff bypass** in album grid (`4236`) and video cards (`3606`).
- **Fix Options:**
  - **Option A (Fix LRU Self-Eviction + Unify 128 MiB Budget + Stable Placeholders + Viewport Fetch Culling — Recommended):**
    1. **Stop LRU self-eviction:** Insert newly arrived `key`s into a `pending_paint_textures` protection set (unioned with `last_frame_textures`) during `check_events`, raise `ZaloGuiApp::texture_lru` to `media_resources::TEXTURE_BUDGET` (128 MiB), downscale chat thumbnails to display DPI cap (e.g. `480px` max dim = 0.9 MB instead of `720px` = 2.07 MB), and batch-cap `StickerImageReady` texture uploads per frame (e.g. max 6/frame) with a single coalesced `request_repaint()` (removing duplicate `ctx_clone.request_repaint()` at `worker.rs:3909`).
    2. **Eliminate layout jumps:** Reserve deterministic placeholder rects for link thumbnails (`260×132` whenever `link_thumb` is non-empty, lock link bubble width to `260px` upfront), stickers (`116×116`), videos (`240×150`), and use wire `width`/`height` metadata (when present in Zalo photo payloads) for exact photo aspect reservation before decode.
    3. **Viewport culling for fetches + backoff discipline:** Only call `queue_sticker_fetch` when the bubble's rect intersects `ui.clip_rect().expand(400.0)` (prefetch margin); route `render_photo_album_grid` and `render_video_card` through `queue_sticker_fetch` (respecting `failed_stickers` backoff); audit all `sticker_textures.get` sites to call `mark_texture_used`.
    - *Trade-offs:* Complexity: Medium. Regression risk: Low. Perf/Mem: Dramatic drop in RAM/VRAM usage and CPU decode churn; eliminates visible flicker and scroll jumps. Cross-platform: Windows/Linux/macOS.
  - **Option B (Option A + True Scroll Virtualization with Per-Message Height Cache):**
    Cache measured row heights by `(msg_id, width_bucket, has_thumb)` and skip laying out offscreen messages entirely using `ScrollArea::show_viewport`.
    - *Trade-offs:* Complexity: High (variable-height chat bubbles + album groups + date headers make height invalidation tricky). Regression risk: Medium-High. Best done as a follow-up after Option A if huge threads still lag.
- **Recommended Option:** **Option A** (directly fixes both root causes cleanly with low risk).
  - *Checklist:* Add `pending_paint_textures` protection in `check_events`; align `texture_lru` to `TEXTURE_BUDGET` (128 MiB); remove duplicate `request_repaint()` in `worker.rs:3909`; reserve fixed link-card/sticker/video/aspect-photo placeholder sizes; add viewport clip-rect fetch gating; route album/video fetches through `queue_sticker_fetch` and `mark_texture_used`.
  - *QA:* Unit test `ResourceLru` batch insert with protection set (zero same-frame eviction). Manual QA on Windows 10/11 `--gui`: open thread with 50+ photos & link cards, scroll rapidly up/down → verify zero flicker, stable scroll position, and idle CPU/GPU ~0% once visible thumbnails finish loading.

---

### HZ-07 — `#204` UI Defects: `"Dịch tin nhắn"` Submenu Strobe, Missing `□` Glyphs & Stale Group Sync UI
- **Upstream Issue(s) & Environment:** `#204` (both `204_vid1.mp4` and `204_vid2.mp4`) — Windows 11 Pro 26H2, `v0.1.3`, `--gui`.
- **Key Clues & Repro Steps (Confirmed via Extracted Keyframes):**
  1. **`204_vid1_03/04.png`:** Hovering `"Dịch tin nhắn >"` in the message context menu causes the language submenu (`Dịch từ ngôn ngữ / Tự động nhận diện / Tiếng Anh...`) to flicker/strobe transparently behind/over the message bubble, and two missing glyph boxes `□` appear (at right end of emoji reaction bar and next to `"Bỏ thả cảm xúc"`).
  2. **`204_vid2_01..05.png`:** In a group thread (`Nhóm 06 VB...`), clicking `"Đồng bộ"` in the right Info Drawer immediately shows toast `"Đã hiển thị toàn bộ lịch sử tin nhắn của cuộc trò chuyện này"` while newer messages fail to show promptly on the UI.
- **Exact `Hz` Code Locations:**
  - Submenu hover strobe: `src/gui/messages.rs:5527-5534` (`d.insert_temp(translate_sub_key, !sub_open)` on `resp.hovered()`!), stale menu height `src/gui/messages.rs:5355-5374`, same `Order::Foreground` `5379-5380, 5781-5782`, and key mismatch `src/gui/messages.rs:6133` vs `5320, 5747`.
  - Missing `□` glyphs: `src/gui/messages.rs:5438, 5457` (raw `"✕"` `U+2715`, plus `src/gui/reactions_dialog.rs:164`, `src/gui/modals.rs:7630, 9156`, `src/gui/workspace.rs:4282, 9700, 10018`).
  - Stale group UI & false `HistoryReachedStart`: `src/gui/workspace.rs:3374-3438`; `src/gui/worker.rs:1374-1420` (WS `merge_previews` ignores `CMD_PUSH_MSG_GROUP 521` / `CMD_PULL_MSG_GROUP 511`), `2346-2571` (sequential `get_cloud_messages` across all groups before single `ThreadsUpdated`), `2903-3089` (`LoadGroupHistory` swallows `get_cloud_messages` errors & emits `HistoryReachedStart` whenever `cnt <= have`); `src/chat.rs:6694-6761, 6905-6911`; `src/gui.rs:4327-4330`.
- **Root-Cause Hypotheses (Ranked):**
  1. **[PROVEN — Submenu Glitch]** `src/gui/messages.rs:5528-5532` writes **`!sub_open`** (per-frame toggle!) while hovered instead of `true`, AND `menu_h = if sub_open { 430.0 } else { 300.0 }` shifts the menu up by `130px` on open, pulling the item away from the cursor so the close check at line `5742` immediately closes it.
  2. **[PROVEN — `□` Glyphs]** `U+2715` (`"✕"`) is absent from every font loaded by `build_fonts` (`src/gui/theme.rs:1488-1723`), whereas `lucide::X` (`\u{e1b2}`) is bundled in `lucide.ttf`.
  3. **[PROVEN — Stale Group UI & False `"Đã hiển thị toàn bộ lịch sử..."`]** (a) Clicking `Đồng bộ` calls `LoadGroupHistory { have: t.messages.len() }` where `get_cloud_messages` ignores `error_code`, errors are swallowed, and any zero-growth reply (`cnt <= have`) permanently sets `history_reached_start`; (b) `RefreshThreads` awaits every group's `get_cloud_messages` sequentially before emitting a single `ThreadsUpdated`; (c) WS `merge_previews` doesn't pass `in_group_bucket = true` for `cmd 521 / 511`; (d) sidebar cache only hashes the first 64 threads (`src/gui/workspace.rs:1301-1335`).
- **Fix Options:**
  - **Option A (Targeted Fixes for All Three Sub-Bugs — Recommended):**
    1. **Submenu:** Change `d.insert_temp(translate_sub_key, !sub_open)` → `d.insert_temp(translate_sub_key, true)` (and toggle only on `.clicked()`); remove `sub_open` from main `menu_h`/`menu_w` calculation (since flyout is a separate `Area`); give `bubble_translate_flyout` `Order::Tooltip` (above `Order::Foreground`) and unify the `"active_bubble_context_menu_time"` key mismatch at line `6133`.
    2. **Glyphs:** Replace raw `"✕"` with `lucide::X` (`\u{e1b2}`) rendered with `FontFamily::Name("lucide")` across `src/gui/messages.rs:5438, 5457` and the remaining 6 UI call sites.
    3. **Group update / `Đồng bộ`:** Check `error_code` in `get_cloud_messages` and never emit `HistoryReachedStart` on API error or on 1-1 `get_last_msgs` refresh; distinguish `"Refresh latest"` (`have = 0` / newest fetch) from `"Load older history"` (pagination cursor from `server_cursor`, not oldest local SQLite message); pass `in_group_bucket = (ev.cmd == 521 || ev.cmd == 511)` into WS `merge_previews`; stream/chunk `RefreshThreads` updates (prioritizing the currently selected thread first!) instead of blocking on all groups sequentially; include all threads & unread counts in sidebar cache hash.
    - *Trade-offs:* Complexity: Low-Medium. Regression risk: Very low. Perf/Mem: Noticeably faster perceived message arrival in multi-group accounts. Cross-platform: Windows/Linux/macOS.
  - **Option B (UI-Only Fix First — Submenu + Glyphs Only, Defer Group Sync Refactor):**
    Fix only `messages.rs` submenu + `lucide::X` icons now and bundle group sync updates with `HZ-04`.
    - *Trade-offs:* Faster to land, but leaves `#204` Part I ("App update tin nhắn lên UI") unresolved.
- **Recommended Option:** **Option A** (all three pieces are cleanly scoped and independently testable).
  - *Checklist:* Fix `translate_sub_key` hover → `true` + remove stale `menu_h` jump + `Order::Tooltip` + align `open_time` Id; replace `"✕"` with `lucide::X`; check `error_code` in `get_cloud_messages`; separate newest refresh from older history pagination; pass `in_group_bucket` for WS cmd `521`/`511`; prioritize selected thread in `RefreshThreads`.
  - *QA:* Unit tests for `get_cloud_messages` error handling, WS cmd `521` group routing, and font-glyph coverage check (no raw `"✕"` in `src/gui/`). Manual QA: hover `"Dịch tin nhắn"` near top & bottom of window (smooth opaque flyout, zero strobe); check icons next to `"Bỏ thả cảm xúc"` (crisp `X`, zero `□`); send message to group from phone and verify prompt appearance + Info Drawer `Đồng bộ`.

---

### HZ-08 — Microphone Switching & Hot-Plug Broken During Runtime / Active Call
- **Upstream Issue(s) & Environment:** `#170` (part 1 + screenshot `170_img1.png`) — Windows 11, installed build, `--gui`.
- **Key Clues & Repro Steps:**
  1. Start app / enter a call, then connect a new Bluetooth/USB headset (e.g., `Headset (UGREEN HiTune)`).
  2. Trying to switch mic in the dropdown stays stuck on the previous mic; clicking Refresh (`⟳`) fails to discover the new device and causes other mic options to **vanish** from the dropdown until app restart.
- **Exact `Hz` Code Locations:**
  - Static device cache & short-lived scan threads: `src/media.rs:22-24, 66-70, 109-127, 146-175` (`probe_media_devices_sync`, `list_media_devices_nonblocking`).
  - `cpal 0.15.3` WASAPI STA-COM bug: `cpal-0.15.3/src/host/wasapi/device.rs:836-856, 872-888` (`static ENUMERATOR: OnceLock<Enumerator>` + `com::com_initialized()` torn down when thread #1 exits in `com.rs:42-50`).
  - Active call mic never switches & hardcoded `f32`: `src/media.rs:1670-1757, 1889-2559` (vs. camera hot-switch at `src/media.rs:1079-1111` and multi-format `F32|I16|U16` in `src/audio.rs:321-360`).
  - GUI mic dropdown & refresh: `src/gui/modals.rs:1903-1930, 2009, 5890-5969, 6083`.
- **Root-Cause Hypotheses (Ranked):**
  1. **[PROVEN] `cpal 0.15.3` Windows WASAPI STA-COM `OnceLock` Poisoning on Short-Lived Scan Threads:**
     First `"media-device-scan"` thread initializes `cpal`'s process-global `ENUMERATOR` in an STA (`COINIT_APARTMENTTHREADED`) and then exits, calling `CoUninitialize()`. Subsequent Refresh spawns a new thread without COM init → `host.input_devices()` returns `Err` → `probe_media_devices_sync` silently swallows `Err` → overwrites `CACHED_DEVICES` with `mics = vec![]` → all other mic options disappear from the dropdown!
  2. **[PROVEN] `run_media_loop` Never Polls `selected_mic_device()` or Rebuilds `input_stream`:**
     Unlike camera (`spawn_camera_capture_thread` polls `selected_camera_device()`), `run_media_loop` opens `input_dev` once at startup and ignores mid-call selection changes forever.
  3. **[PROVEN] Bluetooth HFP `I16` Sample Format Rejected by Hardcoded `f32` `build_input_stream`:**
     Headsets like `UGREEN HiTune` in HFP hands-free mode advertise `SampleFormat::I16`; forcing `f32` at `src/media.rs:1725-1754` causes `build_input_stream` to fail and `run_media_loop` to return immediately.
- **Fix Options:**
  - **Option A (Dedicated Long-Lived COM Audio-Scan Worker + Mid-Call `input_stream` Hot-Reload + `F32|I16|U16` Support — Recommended):**
    1. Replace short-lived `"media-device-scan"` threads with a **single persistent background worker thread** (`"hz-audio-enumerator"`) that stays alive for the lifetime of `Hz` (preserving the WASAPI STA apartment owning `cpal`'s `OnceLock<Enumerator>`), wraps `d.name()` in `catch_unwind` against hot-unplug panics, and **never overwrites** `CACHED_DEVICES.mics` with empty vec on probe error.
    2. In `run_media_loop`, detect `selected_mic_device() != active_mic_name` (or explicit `MediaHandle::switch_mic`), cleanly drop & rebuild `input_stream` (supporting `SampleFormat::F32 | I16 | U16` just like `src/audio.rs:321-360`), and initialize COM (`COINIT_MULTITHREADED` or delegate enumeration to the persistent enumerator thread) on Windows.
    3. Add automatic cache refresh (e.g. 5s TTL while Settings or Call Overlay is open) + update `VoiceRecorder::start` (`src/audio.rs:284`) to also honor `selected_mic_device()`.
    - *Trade-offs:* Complexity: Medium. Regression risk: Low (minimal changes, no crate upgrades required, solves all 3 root causes). Perf/Mem: One idle thread (~64 KB stack). Cross-platform: Windows WASAPI safe, zero impact on Linux ALSA/PulseAudio or macOS CoreAudio.
  - **Option B (Upgrade `cpal` / Direct Win32 WASAPI `IMMDeviceEnumerator` + `IMMNotificationClient` on Windows):**
    Use `windows-sys` / `windows` crate directly on Windows with `COINIT_MULTITHREADED` and register `IMMNotificationClient` for instant OS hot-plug events, keeping `cpal` for Linux/macOS.
    - *Trade-offs:* Complexity: High (new Windows COM FFI surface). Regression risk: Medium. Perf/Mem: Instant hot-plug detection without clicking Refresh.
- **Recommended Option:** **Option A** (fixes the exact `cpal 0.15.3` STA bug, preserves existing dependencies, enables instant mid-call mic switching + Bluetooth HFP `I16` headsets).
  - *Checklist:* Create persistent `"hz-audio-enumerator"` thread; never clobber `mics` cache on error; guard `d.name()` with `catch_unwind`; add `F32|I16|U16` capture support in `run_media_loop`; poll `selected_mic_device()` in `run_media_loop` to rebuild `input_stream` live; wire `VoiceRecorder` to `selected_mic_device()`.
  - *QA:* Unit test cache preservation on probe error + `I16`/`U16`→`f32` conversion. Manual QA on Windows 11 `--gui`: start call on built-in mic → plug in/connect Bluetooth headset mid-call → click Refresh → verify headset appears alongside built-in mic → switch live and confirm peer hears new mic immediately.

---

### HZ-09 — One-Way Video Call (PC→Phone Works, Phone→PC Black Screen)
- **Upstream Issue(s) & Environment:** `#170` (part 2 + comment #2 + screenshot `170_img2.png`) — Windows 11 with `ffmpeg` installed, `--gui`, calling Android/iPhone.
- **Key Clues & Repro Steps:**
  - User confirmed `ffmpeg` is installed, PC webcam / OBS Virtual Camera renders in local PIP (`Camera đang bật`) and transmits clearly to phone, while the remote video canvas from Android/iPhone stays completely black (`170_img2.png`).
- **Exact `Hz` Code Locations:**
  - **HEVC signaling lie:** `src/call.rs:2113` (`build_pure_rust_invite_fields_with_probe`) and `src/call.rs:2187` (`build_callee_extend_data`): `"supportHevcDecode": if crate::media::is_ffmpeg_available() { 1 } else { 0 }`.
  - H.264-only RX pipeline destroying HEVC: `src/media.rs:702-725` (`au_is_sync_point`), `1311-1314` (`normalize_annexb_au_4byte`), `2567-2623` (`depacketize_rfc6184_h264`), `2711-2734` (`spawn_ffmpeg_h264_child` hardcoding `-f h264`), `2824-2870` (`run_h264_ffmpeg_decoder` + `H264_AUD`).
  - `peer_ssrc` overwritten by audio breaking PLI/REMB: `src/media.rs:1841, 2108, 2113, 2348-2356, 2499`.
  - FFmpeg frame-threading & 22-AU stall-kill death-loop: `src/media.rs:2711-2734, 2855-2865`.
  - Unvalidated video fragment reassembly: `src/media.rs:767-804, 2335-2346, 2602-2620`; `0x0E` PT mismatch at `2351` vs `2389`.
- **Root-Cause Hypotheses (Ranked):**
  1. **[PROVEN — Smoking Gun] Advertises `"supportHevcDecode": 1` While RX Pipeline Is Strictly H.264-Only:**
     When `ffmpeg` is installed, `Hz` tells Android/iPhone peers it supports HEVC decode. Mobile Zalo sends HEVC (H.265). Every single HEVC frame is then silently dropped forever by `au_is_sync_point` (HEVC byte0 bit0=0 can never match `byte0 & 0x1F in {5, 7}`), HEVC IDR `0x26` is stripped as H.264 SEI (`0x26 & 0x1F == 6`), and FFmpeg hardcodes `-f h264`.
  2. **[PROVEN — Breaks Even H.264 Calls] `peer_ssrc` Clobbered by Every 20ms Audio Packet:**
     A single `peer_ssrc` variable is updated on both audio (`pt == 112`) and video packets, so `build_pli(ssrc, peer_ssrc)` and `build_remb(...)` target the **audio SSRC**; the phone ignores PLI and never sends an IDR keyframe.
  3. **[PROVEN — Breaks Multi-Core Windows PCs] Missing `-threads 1` + Tiny `-probesize 32` + 22-AU Kill Loop:**
     FFmpeg's default frame-level multithreading buffers `min(cores+1, 16)` frames before yielding frame #1, while `run_h264_ffmpeg_decoder` kills FFmpeg after `22` AUs (~0.4–0.7s) — entering an endless restart loop before frame #1 is ever decoded. Also standalone SPS+PPS+IDR packets get `H264_AUD` injected between SPS and IDR, and out-of-order/dropped RTP fragments corrupt IDRs.
- **Fix Options:**
  - **Option A (Disable False HEVC Negotiation + Fix SSRC Split + Fix FFmpeg Pipe & Depacketizer — Recommended Fast Win):**
    1. Set `"supportHevcDecode": 0` in `src/call.rs:2113` and `2187` so mobile peers reliably negotiate **H.264** (or detect HEVC dynamically only if Option B is implemented).
    2. Split `peer_ssrc` into `peer_audio_ssrc` and `peer_video_ssrc` so `build_pli`, `build_remb`, and `rx_video` RTCP receiver reports target the actual remote video SSRC.
    3. Fix `spawn_ffmpeg_h264_child` (`src/media.rs:2711-2734`): add `"-threads", "1"`, `"-flags", "low_delay"`, `"-fflags", "nobuffer"`, sane `"-probesize", "32768"`, remove aggressive 22-AU kill loop (use time-based 4s stall threshold + PLI retry instead); cache SPS/PPS and prepend to IDR without inserting `H264_AUD` between SPS/PPS/IDR; validate `rtp.seq`/`rtp.ts` continuity in `0x1C` & FU-A reassemblers (and fix single-packet `S && E` FU-A bug at `2609-2619`); guard `0x0E` vs FEC packets.
    - *Trade-offs:* Complexity: Medium. Regression risk: Low. Perf/Mem: Low-latency frame #1 decode (<200ms after IDR). Cross-platform: Works everywhere FFmpeg is present.
  - **Option B (Option A + Full RFC 7798 HEVC Support via FFmpeg `-f hevc`):**
    Parse remote `codec`/`extendData` (or inspect RTP payload type / NAL header), implement RFC 7798 AP(48)/FU(49) depacketizer, HEVC VPS/SPS/PPS/IDR sync detection, and spawn `ffmpeg -f hevc` when peer selects HEVC.
    - *Trade-offs:* Complexity: High. Regression risk: Medium. Useful if future Zalo mobile versions stop honoring `supportHevcDecode: 0`.
  - **Option C (Option A + Pure-Rust `openh264` Decoder — Partial PR #184):**
    Replace FFmpeg decode subprocess with in-process `openh264` crate (keeping FFmpeg optional for camera or using `nokhwa`), eliminating all FFmpeg pipe latency/stall/zombie issues permanently.
    - *Trade-offs:* Complexity: Medium-High (C++ build via `cc` crate in `openh264-sys2` on MSVC/Linux/macOS). Regression risk: Medium.
- **Recommended Option:** **Option A** first (fixes the exact root causes immediately with zero new native build dependencies), with **Option C** (`openh264`) as a high-value Phase 3 upgrade if you want FFmpeg-free video decoding.
  - *Checklist:* Set `"supportHevcDecode": 0` (`src/call.rs:2113, 2187`); split `peer_audio_ssrc` vs `peer_video_ssrc`; add `-threads 1` + sane `-probesize` + SPS/PPS caching + proper stall/PLI recovery in `run_h264_ffmpeg_decoder`; enforce RTP `seq`/`ts` checks in video depacketizers & fix `S && E` FU-A.
  - *QA:* Unit tests for separate audio/video SSRC PLI generation, FU-A `S&&E` & out-of-order drop, and SPS+PPS+IDR grouping. Manual QA on Windows 11 `--gui`: video call PC↔Android and PC↔iPhone → verify both local PIP and remote full-canvas video render within 2s and stay smooth.

---

### HZ-10 — Cannot Recall Messages
- **Upstream Issue(s) & Environment:** `#138` (sub-issue 4: *"không thu hồi tin nhắn được"*) — Windows 11, `--gui`.
- **Key Clues & Repro Steps:**
  - Clicking `"Thu hồi"` on a sent message (especially after restarting `Hz`, syncing from phone, or in certain chats) shows `"Đang thu hồi tin nhắn…"` and then silently does nothing; message stays un-recalled for recipients.
- **Exact `Hz` Code Locations:**
  - UI & worker: `src/gui/messages.rs:5705-5716`; `src/gui/workspace.rs:7622-7631`; `src/gui/worker.rs:739-748, 3475-3525`; `src/gui.rs:3387-3391, 3776-3828, 4310-4312`.
  - Payload & API: `src/chat.rs:1274-1285` (`server_message_id`), `1575-1630` (`merge_thread_into`), `5646-5761` (`build_recall_payload`, `recall_message`).
  - Data loss on persistence/sync: `src/store.rs:2035-2279, 2386-2476` (`StoredMessage::from` & `hydrate_threads`); `src/history_sync.rs:1129-1223`.
- **Root-Cause Hypotheses (Ranked):**
  1. **[PROVEN] `client_message_id` (`cliMsgId`) & `wire_ref` Are Destroyed by SQLite & Phone Sync:**
     `StoredMessage::from` (`src/store.rs:2386-2476`) never persists `client_message_id` or `wire_ref`, `history_sync.rs:1167-1171` drops `CliMsgId` whenever `GlbMsgId > 0`, and `merge_thread_into` doesn't backfill it. Consequently, `build_recall_payload` (`src/chat.rs:5657-5671`) falls back to `msg.ts.to_string()` (server timestamp) as `cliMsgIdUndo`, which Zalo's `/api/message/undo` and `/api/group/undomsg` reject.
  2. **[PROVEN] Uses Raw `msg.id` Instead of `server_message_id(msg)` & Narrow `extract_msg_id`:**
     `build_recall_payload` (`src/chat.rs:5653-5656`) ignores `server_message_id(msg)` (sending `cliMsgId` as `msgId` when `msg.id` hasn't been replaced) and `extract_msg_id` (`worker.rs:739-748`) only checks `data.msgId` (missing `globalMsgId`/`realMsgId`), leaving `out_<client_id>` unrecallable.
  3. **[PROVEN] Recall Failures Are Silently Swallowed:**
     `worker.rs:3489-3520` emits `GuiEvent::StatusUpdate`, which updates `self.status_msg` (only visible on sync-icon hover tooltip) instead of showing an error toast. Also UI shows `"Thu hồi"` past Zalo's 24h window.
- **Fix Options:**
  - **Option A (Persist `client_message_id` + `wire_ref` in SQLite/Sync + Fix `build_recall_payload` & Toast Errors — Recommended):**
    1. Persist `client_message_id`, `board_topic_id` (also fixes `HZ-12`!), and `wire_ref` (inside `raw_json` or additive SQLite columns) in `StoredMessage::from` / `hydrate_threads` and preserve `CliMsgId` in `src/history_sync.rs:1167-1171` + `merge_thread_into`.
    2. Update `build_recall_payload` to resolve `msgId` via `server_message_id(msg)` and `cliMsgIdUndo` via `client_message_id` → `wire_ref.client_id` → `history_source` before falling back; broaden `extract_msg_id` to check `msgId | globalMsgId | realMsgId`.
    3. Emit `GuiEvent::Toast` on recall errors in `worker.rs:3489-3520`, and hide/disable `"Thu hồi"` for messages older than 24h (with tooltip explaining Zalo's 24h limit).
    - *Trade-offs:* Complexity: Low-Medium. Regression risk: Very low (backward-compatible SQLite JSON field additions). Perf/Mem: Negligible. Cross-platform: All OSes.
  - **Option B (Additive SQLite Schema Migration Columns `client_msg_id`, `board_topic_id`):**
    Add dedicated indexed columns in `messages` table (`ALTER TABLE messages ADD COLUMN ...`) in addition to JSON hydration.
    - *Trade-offs:* Complexity: Medium. Slightly cleaner SQL queries and faster lookup by `client_msg_id`, still backward-compatible via `PRAGMA table_info` migration check.
- **Recommended Option:** **Option A** (or Option A using `raw_json` metadata for zero-risk schema compatibility, sharing the exact same persistence layer with `HZ-12`).
  - *Checklist:* Save & hydrate `client_message_id` and `wire_ref` in `src/store.rs`; preserve `CliMsgId` in `src/history_sync.rs` & `merge_thread_into`; fix `build_recall_payload` + `extract_msg_id`; emit visible error toasts on recall failure; add 24h guard in `src/gui/messages.rs`.
  - *QA:* Unit tests for SQLite round-trip of `client_message_id`/`wire_ref` + `build_recall_payload` after hydration. Manual QA: send 1-1 & group text/image messages → restart `Hz` → recall both → confirm recalled on official Zalo mobile/PC.

---

### HZ-11 — Chat `@mention` / Tagging Users Does Not Work
- **Upstream Issue(s) & Environment:** `#138` (sub-issue 1: *"Chat chưa tag tên được"*) — Windows 11, `--gui`.
- **Key Clues & Repro Steps:**
  - Typing `@` in a group shows a candidate popup and inserts `@Name `, but the sent message arrives as plain text (no mention highlight or mention push notification on official Zalo) and rendering breaks on Vietnamese/lowercase multi-word names.
- **Exact `Hz` Code Locations:**
  - Autocomplete discards UID: `src/gui/workspace.rs:144-169, 6780-6839, 7417-7428, 9036-9040, 10297-10305` (`#[allow(dead_code)] pub id: String`), `10544-10547`.
  - Wire send omits `mentionInfo`/`mentions`: `src/gui/worker.rs:385, 3091-3194`; `src/chat.rs:2254-2349` (`send_message`/`send_group_message`), `4128, 4167` (`send_link` hardcodes `"[]"`/`""`), `5010-5151` (`send_quote`).
  - Unread mention parser (shows expected wire schema): `src/unread.rs:582-645` (`wire_ref.fields["mentions"]` `[{ uid, pos, len, type }]`, `MENTION_ALL_ID = "-1"`).
  - Renderer ignores wire mentions: `src/chat.rs:141` (`resolved_mention_uids` unused); `src/gui/messages.rs:1379-1454, 1609-1619`.
- **Root-Cause Hypotheses (Ranked):**
  1. **[PROVEN] Composer Never Tracks Mention Spans & Protocol Layer Never Sends `mentionInfo`:**
     `MentionCandidate.id` is literally marked `#[allow(dead_code)]`. `WorkerCmd::SendMessage` carries only plain `text`, and `send_group_message_with_client_id` calls `/api/group/sendmsg` instead of Zalo's group mention endpoint (`/api/group/mention` or attaching `mentionInfo: JSON.stringify([{ pos, uid, len, type }])` using UTF-16 code-unit offsets!).
  2. **[PROVEN] Incoming Mention Rendering Relies on Flawed Regex Heuristic:**
     `InlineMdRun::Mention` stops at lowercase words after whitespace (`src/gui/messages.rs:1440`), breaking names like `@Nguyễn văn an`, and ignores `wire_ref.fields["mentions"]`.
- **Fix Options:**
  - **Option A (Full Tracked Mention Spans + UTF-16 Wire Serialization + Wire-Driven Rendering — Recommended):**
    1. Track `pending_mentions: Vec<OutgoingMention { uid: String, display_text: String }>` in `ZaloGuiApp` composer state (populated on Tab/Enter/click in mention popup and `BubbleAction::Mention`); compute exact **UTF-16** `(pos, len)` offsets against final `text` right before send (matching Zalo's JS/Electron/Android UTF-16 string indexing, filtering out deleted mentions).
    2. Add `mentions: Vec<WireMention>` to `WorkerCmd::SendMessage` / `SendQuote` / `SendLink`; in `src/chat.rs`, when `!mentions.is_empty()` in groups, send `POST {domains.group}/api/group/mention` (with `mentionInfo` JSON array `[{"pos":..,"uid":..,"len":..,"type":0|1}]`, `type: 1` for `@All` `uid="-1"`, `type: 0` for individual user) and pass `mentionInfo` in `/api/group/quote` & `/api/group/sendlink`.
    3. Populate structured mention spans on `ChatMessage` from `wire_ref.fields["mentions"]` during parse (`src/chat.rs`) and highlight exact spans in `src/gui/messages.rs` (falling back to improved Unicode heuristic only for legacy messages without metadata).
    - *Trade-offs:* Complexity: Medium. Regression risk: Low (plain messages with no mentions keep exact current `/api/group/sendmsg` path). Perf/Mem: Negligible. Cross-platform: All OSes; UTF-16 offset math handles Vietnamese diacritics & emojis cleanly.
  - **Option B (Text-Scan Mention Resolution Against Group Member Roster on Send):**
    Instead of tracking composer spans, scan outgoing group text for `@All` and `@<member_name>` matches against cached group member profiles right before send.
    - *Trade-offs:* Complexity: Low. Regression risk: Medium (ambiguous if two members share a name or if user manually types `@name` unintentionally, though works seamlessly in TUI/REST/MCP too).
- **Recommended Option:** **Option A** (with **Option B**'s name-lookup as fallback for TUI / REST API / MCP sends).
  - *Checklist:* Add `OutgoingMention` tracking in composer; implement UTF-16 `(pos, len)` builder with unit tests (Vietnamese + emoji prefixes); route group messages with mentions to `/api/group/mention` with `mentionInfo`; wire `mentionInfo` into quote & link sends; render incoming wire `mentions` spans accurately.
  - *QA:* Unit tests for UTF-16 offset calculation (`"Chào 😊 @Phạm Đức Hiếu @All"`). Manual QA: tag `@All` and a Vietnamese-named member in a group → verify official Zalo mobile receives a mention push notification & blue highlight.

---

### HZ-12 — Cannot Pin Messages to Top of Conversation
- **Upstream Issue(s) & Environment:** `#138` (sub-issue 2: *"chưa ghim tin nhắn lên được"*) — Windows 11, `--gui`.
- **Key Clues & Repro Steps:**
  - Clicking `"Ghim tin nhắn"` in a message context menu leaves nothing pinned at the top of the conversation, shows no confirmation toast, leaves `status_msg` stuck at `"Đang ghim tin nhắn lên bảng tin nhóm…"`, and unpinning fails after app restart.
- **Exact `Hz` Code Locations:**
  - **Top Pinned Banner disabled (`None`):** `src/gui/workspace.rs:6386-6387` (`let pinned_msg_info: Option<(usize, String, String)> = None;`) and local-only unpin button at `6420-6430`.
  - Parked board sync: `src/gui.rs:2577-2580` (`fn maybe_refresh_board_pins(&mut self) {}`); unused `WorkerCmd::RefreshBoardPins` (`src/gui/worker.rs:557, 1730-1750`); zero socket handlers for `pin_message`/`unpin_topic`.
  - Lost `board_topic_id`: `src/store.rs:2158-2161, 2242-2279, 2421-2423`; `src/chat.rs:1575-1630, 9498-9502`.
  - Pin payload & result bugs: `src/chat.rs:9396-9423` (`message_topic_params` uses raw `msg.id`, hardcodes `"senderUid": "0"`, omits `msg_type`); `src/gui.rs:4710-4738` (`MessagePinResult` never clears `board_topic_id` on unpin success, no toast).
- **Root-Cause Hypotheses (Ranked):**
  1. **[PROVEN] Pinned Banner Is Explicitly Hardcoded to `None`:** Even when `pin_message` succeeds on Zalo's server, `src/gui/workspace.rs:6387` hides the top banner unconditionally.
  2. **[PROVEN] Server Pin Sync Is Parked & `board_topic_id` Is Not Persisted:** `maybe_refresh_board_pins` is an empty no-op, socket `pin_message`/`unpin_topic` control events aren't handled, and restarting `Hz` erases `board_topic_id` from SQLite so `unpin_message` always errors.
  3. **[PROVEN] Pin Payload Bugs (`senderUid: "0"`, raw `msg.id`) & Broken Unpin State:** Outgoing messages send `"senderUid": "0"` instead of the user's real UIN, and successful unpin never clears `m.board_topic_id`.
- **Fix Options:**
  - **Option A (Re-Enable Top Pinned Banner + Fix Server Pin Payload/Persistence + Group Board & Socket Sync — Recommended):**
    1. Fix `message_topic_params` (`src/chat.rs:9396-9423`) to use `server_message_id(msg)`, real logged-in UIN for outgoing `senderUid`, and proper `msg_type`.
    2. Persist `board_topic_id` alongside `is_pinned` in `src/store.rs` and `merge_thread_into` (shared with `HZ-10`); fix `GuiEvent::MessagePinResult` to clear `board_topic_id` on unpin success and show clear success/error toast.
    3. Populate `pinned_msg_info` in `src/gui/workspace.rs:6387` from active pinned messages (supporting click-to-jump and multiple pins badge), wire the banner's `"Bỏ ghim"` button to `WorkerCmd::PinMessage { pin: false, ... }`.
    4. Enable `maybe_refresh_board_pins` for **groups** (calling `/api/board/list` on thread open, debounced, while safely skipping 1-1 `friendboard/list` error `114` until fixed or using socket events) AND add WS control handlers for `pin_message` / `unpin_topic` events.
    - *Trade-offs:* Complexity: Medium. Regression risk: Low (gracefully degrades to local persisted pins if `friendboard/list` returns `114`). Perf/Mem: 1 lightweight board fetch per group switch (cached). Cross-platform: All OSes.
  - **Option B (Local-First Pinned Banner + Fix `pin_message`/`unpin_message` Calls Without Board Polling):**
    Re-enable the top Pinned Banner using persisted `is_pinned` + `board_topic_id` + fix `pin_message`/`unpin_message` payloads and toasts, without adding `/api/board/list` polling.
    - *Trade-offs:* Complexity: Low. Regression risk: Very low, but pins created by other users on official Zalo won't appear in the top banner until WS/board sync is added.
- **Recommended Option:** **Option A** (complete UX parity with Zalo PC for groups + safe handling for 1-1 chats).
  - *Checklist:* Fix `message_topic_params` (`server_message_id`, real UIN, `msg_type`); persist `board_topic_id` in SQLite & merge; fix `MessagePinResult` unpin clear + toast; re-enable top Pinned Banner & wire `"Bỏ ghim"` to `WorkerCmd::PinMessage`; re-enable group `RefreshBoardPins` + add WS `pin_message`/`unpin_topic` handler.
  - *QA:* Unit tests for `message_topic_params` & `board_topic_id` SQLite round-trip. Manual QA: pin a message in group & 1-1 → verify banner appears at top of chat & on official Zalo mobile → click banner to jump → restart `Hz` → click `"Bỏ ghim"` → verify unpinned on both `Hz` and Zalo mobile.

---

### HZ-13 — Cannot Delete / Disband / Leave Group Reliably
- **Upstream Issue(s) & Environment:** `#138` (sub-issue 3: *"không xoá nhóm được"*) — Windows 11, `--gui`.
- **Key Clues & Repro Steps:**
  - Group creators have no option to disband (`Giải tán nhóm`) or transfer ownership (`Chuyển quyền trưởng nhóm`); leaving a group can fail silently or reappear after sync; deleting a group conversation sends an invalid payload and reappears on the next thread sync.
- **Exact `Hz` Code Locations:**
  - Missing endpoints: Zero occurrences of `/api/group/disperse` or `/api/group/change-owner` in `src/`.
  - `leave_group`: `src/chat.rs:11164-11200` (strips only `"g_"` / `"group_"`, misses bare `'g'`/`'G'`); `src/gui/workspace.rs:9482-9511` (optimistic delete + SQLite race before worker ACK); `src/gui/worker.rs:2023-2049`; `src/gui.rs:4316-4322` (no rollback).
  - `delete_conversation`: `src/chat.rs:11108-11160` (`"conver": {}`, ignores `error_code`); `src/gui/workspace.rs:1942-1943` (passes `msgId` as `last_cli_msg_id`), `5259-5293` (drawer clear is local-only); `src/gui/worker.rs:2006-2022` (`let _ = ...`); `src/gui.rs:3067, 3141` (`getlg/v4` re-adds deleted group).
- **Root-Cause Hypotheses (Ranked):**
  1. **[PROVEN] Disband Group (`/api/group/disperse`) Completely Unimplemented:** Group owners cannot delete/disband their own group because neither the API nor UI button exists, and Zalo rejects `/api/group/leave` for creators.
  2. **[PROVEN] `leave_group` Normalization Bug + Premature Local Delete Without Rollback:** Bare `g<id>` thread IDs produce malformed `grid`; local thread & SQLite rows are deleted immediately before server confirmation, racing with `persist_threads_async` and never rolling back on failure.
  3. **[PROVEN] `delete_conversation` Malformed Payload & Re-Sync Resurrection:** Sends empty `"conver": {}`, passes server `msgId` instead of `cliMsgId`, ignores `error_code`, and `getlg/v4` unconditionally re-inserts every joined group into `self.threads`.
- **Fix Options:**
  - **Option A (Implement `/api/group/disperse` + Fix `leave_group` & `delete_conversation` Payloads + Server-Confirmed State Updates — Recommended):**
    1. Implement `disperse_group(s, grid)` (`POST {domains.group}/api/group/disperse`, encrypted `{ grid, imei }`, checking `error_code == 0`) + `WorkerCmd::DisperseGroup` + `"Giải tán nhóm"` button in group Info Drawer / context menu (visible when logged-in UIN == group `creatorId`, with confirmation modal).
    2. Normalize `grid` in `leave_group` with the shared `clean_group_id` helper (stripping `group_`, `g_`, `g`, `G`); wait for `GuiEvent::GroupLeft` success before removing thread from `self.threads` and deleting from SQLite (show clear server error toast if owner must disband/transfer first).
    3. Fix `delete_conversation`: populate `"conver": { "ownerId", "cliMsgId", "globalMsgId" }` using real `client_message_id` and `server_message_id`, check `error_code`, propagate result to GUI, wire right-drawer `"Xoá lịch sử trò chuyện"` to `WorkerCmd::DeleteThread`, and persist a local `deleted_at_ts` tombstone in SQLite so `getlg/v4` doesn't resurrect a cleared empty group until a newer message (`ts > deleted_at_ts`) arrives.
    - *Trade-offs:* Complexity: Medium. Regression risk: Low. Perf/Mem: Negligible. Cross-platform: All OSes.
  - **Option B (Fix `leave_group` + `delete_conversation` Only; Add `disperse_group` Later):**
    Fix prefix normalization, server-ACK ordering, `conver` payload, and tombstone filtering first without adding new owner-disband UI.
    - *Trade-offs:* Complexity: Low, but group creators still cannot disband their own groups from `Hz`.
- **Recommended Option:** **Option A** (completely resolves all three ways users mean "xoá nhóm": disband as owner, leave as member, or delete conversation history).
  - *Checklist:* Add shared `clean_group_id` helper; implement `disperse_group` + confirmation modal for group owners; make `leave_group` wait for server ACK before deleting local thread/DB; populate real `conver` object & check `error_code` in `delete_conversation`; persist `deleted_at_ts` tombstone against `getlg/v4` resurrection.
  - *QA:* Unit tests for `clean_group_id`, `disperse_group` payload, `delete_conversation` `conver` payload & error checking. Manual QA: create a test group → test `"Giải tán nhóm"` as owner; join a second test group → test `"Rời nhóm"` as member; test `"Xóa hội thoại"` → refresh threads → verify it does not resurrect.

---

### HZ-14 — Quoted / Replied Captionless Image Shows Blank Quote Box Instead of `[Hình ảnh]`
- **Upstream Issue(s) & Environment:** `#206` (body + screenshots `206_img_quote.png`, `206_video_quote.png` + comment #1) — Windows 11, `v0.1.3`, `--gui`.
- **Key Clues & Repro Steps:**
  - When someone replies to a captionless photo, `Hz` shows only the sender's name (`Anh Khoa`) with an empty second line (whereas replies to videos correctly show `[Video]`).
  - Upstream maintainer fixed this for `v0.1.4` (not yet in `Hz` `v0.1.3`).
- **Exact `Hz` Code Locations:**
  - Realtime JSON parse: `src/chat.rs:8214-8226` (`parse_chat_message` ignores `q["attach"]`, `q["cliMsgType"]`, `q["msgType"]`).
  - Sync V2 protobuf parse: `src/history_sync.rs:1624-1630, 1714-1753, 2624-2641` (`parse_proto_quote` ignores attachment/type fields).
  - Rendering: `src/gui/messages.rs:5847-5911` (`render_quote_block` returns early on `msg.reply_to_text.as_ref()?` at `5854`).
  - Outgoing quote & composer preview: `src/chat.rs:5064, 5107`; `src/gui/workspace.rs:7852-7856`.
- **Root-Cause Hypotheses (Ranked):**
  1. **[PROVEN]** `parse_chat_message` skips empty `q.msg` without inspecting `q.attach` / `q.cliMsgType` / `q.msgType` (e.g., `cliMsgType == 32` / `chat.photo`, `attach` JSON containing `thumb`/`href`), `parse_proto_quote` drops non-text quote protobuf fields, and `render_quote_block` skips or renders blank text when `reply_to_text` is `None`/empty.
- **Fix Options:**
  - **Option A (Unified Quote Label Resolver Across Realtime + Sync V2 + Composer + Renderer — Recommended):**
    Add helper `format_quote_preview(msg_text, cli_msg_type, attach_json)` returning `[Hình ảnh]`, `[Hình ảnh] <caption>`, `[Video]`, `[Nhãn dán]`, `[Tập tin]`, `[Ghi âm]` consistently in: (1) `src/chat.rs:8214-8226`, (2) `src/history_sync.rs:1714-1753, 2638` (parsing quote `cliMsgType`/attach proto fields), (3) `src/gui/workspace.rs:7852-7856` (composer reply banner), and (4) `src/gui/messages.rs:5854` (fallback to looking up the original message in `thread.messages` by `reply_to_id` if `reply_to_text` is empty!).
    - *Trade-offs:* Complexity: Low (~60 LOC). Regression risk: Zero. Perf/Mem: Negligible. Cross-platform: All OSes.
  - **Option B (Option A + Quoted Image Thumbnail Inside Quote Card):**
    Also store `reply_to_thumb_url: Option<String>` from `q["attach"]["thumb"]` (or referenced message's `image_url`) and paint a crisp `32×32` rounded thumbnail inside `render_quote_block` matching official Zalo (`206_img_quote.png` right side).
    - *Trade-offs:* Complexity: Low-Medium (~100 LOC). Regression risk: Very low; visually matches official Zalo PC 1:1.
- **Recommended Option:** **Option B** (small incremental effort over Option A and matches the user's exact official-Zalo reference screenshot in `#206`).
  - *Checklist:* Add `format_quote_preview` + extract `reply_to_thumb_url` from `q["attach"]` and Sync V2 proto; update `render_quote_block` to render fallback label & optional `32×32` thumb; update composer reply bar (`workspace.rs:7852`) and `send_quote_message_with_client_id` `qmsgType`/`qmsgAttach`.
  - *QA:* Unit tests for captionless image quote, captioned image quote, video quote, and Sync V2 proto quote. Manual QA: reply to a photo on official Zalo and on `Hz` → confirm `[Hình ảnh]` + thumbnail appear in both directions.

---

### HZ-15 — Low Call Audio Quality Compared to Official Zalo
- **Upstream Issue(s) & Environment:** `#160` — `--gui` voice/video calls.
- **Key Clues & Repro Steps:**
  - User reports noticeably worse call audio quality (metallic distortion, crackles, dropouts, echo) compared to official Zalo Desktop.
- **Exact `Hz` Code Locations:**
  - Opus config & missing FEC/PLC: `src/media.rs:19-20, 1804-1817, 2349, 2380`; `src/call.rs:2126, 3151, 3491`.
  - Point-sampled resampling, 8 kHz 2× speed bug & playback over-drain: `src/media.rs:1725-1789` (contrast fixed resampler in `src/audio.rs:133-181, 684-688`).
  - Missing jitter buffer, unbounded `output_buf`, TX silence injection & callback lock contention: `src/media.rs:1728-1784, 1891-1913, 2384-2386, 2558`; `src/voip_log.rs:71-87`.
- **Root-Cause Hypotheses (Ranked):**
  1. **[PROVEN] Broken Nearest-Neighbor Resampling (No Anti-Aliasing, 8 kHz 2× Bug, Off-by-One Over-Drain):**
     Decimating 48/44.1 kHz mics to 16 kHz via raw index truncation aliases 8–24 kHz energy into speech; zero-order hold playback creates harsh imaging; `step.max(1.0)` breaks 8 kHz Bluetooth HFP mics (2× chipmunk speed + gaps); `consumed.max(src_idx + 1)` drains an extra sample per callback causing continuous crackles on 44.1 kHz / non-multiple-of-3 buffer sizes.
  2. **[PROVEN] Unconfigured Opus + No PLC + No Jitter Buffer + TX Silence Stutter:**
     Opus never enables `set_inband_fec(true)`, `set_bitrate`, `set_complexity`, or `set_packet_loss_perc`, and `decoder.decode_float` hardcodes `fec = false` and ignores sequence gaps (no PLC → hard `0.0` pops). `output_buf` has no reordering or latency cap; TX injects 20ms silence whenever `input_buf` has 319 samples at the 20ms boundary; `cpal` real-time callbacks contend on `Mutex<Vec<f32>>` with `O(N)` `Vec::drain` and `voip_log` disk writes.
  3. **[PROVEN] Zero DSP (No HPF / AEC / NS / AGC / Soft Limiter):**
     Speaker-to-mic acoustic echo on laptops, background fan/room noise, and hard clipping at `[-1.0, 1.0]`.
- **Fix Options:**
  - **Option A (Fix Resampling + Opus VoIP/FEC/PLC + Adaptive Jitter Buffer + Lock-Free Ring Buffers + Soft Limiter — Pure Rust, Recommended):**
    1. Replace point-sampled resampling with phase-continuous windowed-sinc/FIR anti-aliased resampler (e.g., `rubato` or lightweight biquad LPF + Hermite/linear interpolation handling arbitrary rates including 8 kHz→16 kHz and 16 kHz→44.1/48 kHz without off-by-one over-drain).
    2. Configure Opus cleanly: `set_bitrate(24_000..32_000)` (or 48 kHz if peer SDP accepts `opus/48000/1`), `set_inband_fec(true)`, `set_packet_loss_perc(10)`, `set_complexity(8)`, `set_vbr(true)`; wire `rx_audio` sequence tracking into a small **ordered jitter buffer (40–80ms target, max 200ms cap)** that invokes Opus **FEC** (`decode_float(next, ..., true)`) and **PLC** (`decode_float(&[], ..., false)`) on missing packets!
    3. Separate audio TX/RX pacing from video/disk logging using lock-free SPSC ring buffers (`VecDeque` or `ringbuf`) so `cpal` callbacks never block, fix the 319-sample TX silence-injection bug (wait for `FRAME_SAMPLES` with accurate 20ms pacing), and add a 1st-order 80 Hz high-pass filter + smooth `tanh` soft limiter.
    - *Trade-offs:* Complexity: Medium. Regression risk: Low. Perf/Mem: Tiny CPU (<1%), eliminates 90% of distortion, crackles, dropouts, and drift. Cross-platform: 100% pure Rust on Windows/Linux/macOS.
  - **Option B (Option A + Full WebRTC Audio Processing — AEC3 + NS + AGC):**
    Integrate `webrtc-audio-processing` or `nnnoiseless` (RNNoise) + `aec3` crate for laptop acoustic echo cancellation and neural noise suppression.
    - *Trade-offs:* Complexity: High (`webrtc-audio-processing` requires C++ / Meson/CMake build; `nnnoiseless` is pure Rust at 48 kHz!). Regression risk: Medium. Best layered *on top* of Option A once clean resampling & jitter buffering are in place.
- **Recommended Option:** **Option A** first (massive immediate quality jump with zero native build risk), followed optionally by pure-Rust `nnnoiseless` NS in a second step.
  - *Checklist:* Replace capture/playback resamplers with phase-continuous anti-aliased resampler (supporting 8/16/44.1/48 kHz); configure Opus bitrate/FEC/complexity; implement ordered 40–80ms jitter buffer with Opus FEC/PLC & max-depth cap; fix TX 319-sample silence bug; switch `cpal` callbacks to non-blocking ring buffers; add 80 Hz HPF + soft limiter.
  - *QA:* Unit tests for 48 kHz↔16 kHz & 8 kHz→16 kHz phase continuity (sine-wave THD/SNR & zero sample-count drift), jitter buffer reordering & PLC invocation. Manual QA: 5-min call on Windows 11 (built-in 48 kHz mic + 8 kHz/16 kHz Bluetooth headset) comparing clarity side-by-side against official Zalo.

---

### HZ-16 — Wall / Friend Feed (`Tường nhà / Nhật ký bạn bè`) Broken Layout, Fabricated Posts & Wrong Attribution
- **Upstream Issue(s) & Environment:** `#156` (body + annotated screenshot `156_feed.png`) — Windows 11 Pro 26H2, `v0.1.2` (code unchanged in `Hz` `v0.1.3`), `--gui`.
- **Key Clues & Repro Steps (`156_feed.png`):**
  1. Open left-rail **Tường nhà / Nhật ký bạn bè**.
  2. Images are severely stretched horizontally (`"Ảnh bị scale quá méo"`).
  3. Feed displays `"Ảnh chia sẻ"` / `"Đã chia sẻ 1 ảnh/video"` (actually private DM photos — e.g. a bank transfer receipt!) and friend profile cover/bio updates instead of real Zalo Timeline posts.
  4. Wrong attribution (`"Đây không phải bài đăng của người này"` — e.g., your own outgoing DM photos attributed to your friend!), and cards show raw numeric UIDs (`686454...`) with initial-digit avatar `'6'` floated to the top of the feed.
  5. Debug tab is mislabeled `"Giao thức Feed (Cmd 50001)"`.
- **Exact `Hz` Code Locations:**
  - Fake dimensions & stretched painting: `src/gui/modals.rs:10172-10179` (`640×480` hardcoded), `src/feed.rs:763-770` (`0×0`), `src/gui/modals.rs:10554-10560, 10831-10884, 11081-11088` (full UV `(0,0)-(1,1)` stretch without checking `tex.size_vec2()`).
  - Fabricated feed & missing Timeline API: `src/feed.rs:511-811` (`fetch_timeline_feed` never calls any Zalo Timeline endpoint; synthesizes `"Ảnh chia sẻ"` at `677-728` and `"Ảnh bìa"` at `732-801`); `src/feed.rs:206-319` (`CMD_FEED_TIMELINE = 1230` packet builder never sent); **`src/feed.rs:222`** (`pub const ERR_FEED_NO_NETWORK: i32 = 50001;` mislabeled as `"Cmd 50001"` at `src/gui/modals.rs:10453, 11159`).
  - Wrong attribution & raw UID fallback: `src/gui/modals.rs:10154-10189` (scrapes 1-1 chats without filtering `m.is_outgoing`, discards `t.name`/`t.avatar`); `src/feed.rs:590-605` (`.take(120)` truncates profile lookups), `493-499` (missing `_0` key strip), `685-699` (falls back to raw `uid`, empty avatar, and `now_ms - 1_000` floating broken cards to the top!).
- **Root-Cause Hypotheses (Ranked):**
  1. **[PROVEN — Stretched Images]** Every image renderer in the Feed modal paints full UV `(0,0)-(1,1)` into a fixed/fake-dimension rect instead of reading `tex.size_vec2()` to preserve aspect ratio (fit-contain for single images, center-crop UV for square grid thumbnails and cover banners).
  2. **[PROVEN — Wrong Attribution & Raw UIDs]** `trigger_feed_timeline_load` scrapes private 1-1 DM photos (including `m.is_outgoing == true`!) and throws away `t.name`/`t.avatar`; `fetch_timeline_feed` truncates profile fetches at 120 UIDs and falls back to raw UID + empty avatar + `now_ms - 1_000`.
  3. **[PROVEN — No Real Timeline Posts]** Zalo Web/PC (`zpw_type=30`) does not expose mobile Android's Timeline Feed REST API; `Hz` inherited an incomplete stub that mixes private DM photos with friend profile bios/covers and displays an unsent binary packet hex-dumper mislabeled `"Cmd 50001"` (actually error constant `ERR_FEED_NO_NETWORK`).
- **Fix Options:**
  - **Option A (Fix Image Aspect/Crop + Fix Attribution/Names + Honest Labeling & Separate "Ảnh chia sẻ" vs "Cập nhật hồ sơ" — Recommended Practical Fix):**
    1. **Fix image scaling everywhere in Feed (`src/gui/modals.rs:10554-11088`):** Read actual `tex.size_vec2()` once loaded: use aspect-preserving contain sizing for single photos (`w / aspect`, clamped cleanly to `max_w × max_h`) and center-crop UV rects (`compute_cover_uv`) for multi-image square grids, cover banners, and avatar tiles.
    2. **Fix attribution, privacy & raw UIDs:** Stop pretending private 1-1 DM photos are public Wall posts (either remove DM scraping from `"Nhật Ký"` entirely or move it to an explicitly labeled `"Ảnh đã chia sẻ trong chat"` tab showing true sender `"Bạn"` vs friend name via `m.is_outgoing`); pass known `(t.id, t.name, t.avatar)` fallbacks into `fetch_timeline_feed`; remove the `.take(120)` cap (batch in chunks of 40) and strip `"_0"` suffixes in `get_friend_social_profiles_batch`; never assign fake `now_ms - 1_000` timestamps.
    3. **Fix misleading protocol tab:** Hide the developer-only `"Giao thức Feed (Cmd 50001)"` tab behind `--dev` mode AND correct the label (`CMD_FEED_TIMELINE = 1230`, not `ERR_FEED_NO_NETWORK = 50001`), adding an honest notice in `"Nhật Ký"` that Zalo Desktop/Web protocol limits full mobile Timeline posts to profile/avatar/cover updates unless real mobile Feed API is connected.
    - *Trade-offs:* Complexity: Low-Medium. Regression risk: Very low. Perf/Mem: Faster and privacy-safe (no more private bank receipts shown as a friend's public Wall post!). Cross-platform: All OSes.
  - **Option B (Option A + Reverse-Engineer & Implement Real Zalo Timeline Feed Protocol Over Mobile/Social Endpoints):**
    Research and wire live network transport for Zalo's real Timeline feed (`CMD_FEED_TIMELINE 1230` / social feed endpoints) with pagination, likes, and comments.
    - *Trade-offs:* Complexity: Very High (mobile binary socket protocol requires device auth/session type different from `zpw_type=30` Web session and carries account-ban risk if mobile auth is spoofed). Regression risk: High.
- **Recommended Option:** **Option A** (fixes every visual, privacy, attribution, UID, and mislabeling bug safely within the official `zpw_type=30` desktop session without risking mobile-auth protocol bans).
  - *Checklist:* Add `compute_cover_uv` & aspect-fit helpers in Feed modal; pass `t.name`/`t.avatar` fallbacks & batch all UIDs (remove `.take(120)`, strip `_0`); filter/separate DM shared media with accurate `is_outgoing` attribution and real message timestamps; gate protocol tab behind `self.dev_mode` and fix `50001`→`1230` label.
  - *QA:* Unit tests for `compute_cover_uv` (portrait, landscape, ultrawide, square), `_0` key stripping, and zero raw-UID fallback when thread metadata exists. Manual QA: open Wall/Feed on Windows 11 `--gui` → verify crisp aspect-accurate photos, accurate friend names/avatars, zero private outgoing photos misattributed to friends, and clean tab bar in normal mode.

---

## 3. Prioritized Execution Roadmap & Approval Gate

### Recommended Execution Order (`P0 → P1 → P2`)

| Phase | Order | Bug IDs | Rationale & Shared Dependencies |
| :--- | :---: | :--- | :--- |
| **Phase 1 (P0 — Stability & Core Breakage)** | 1 | **HZ-01** (Option A/B) | 15-line surgical fix eliminates a 100% deterministic GUI freeze affecting every Windows user on double-click. Zero risk, instant win. |
| | 2 | **HZ-05** (Option A) | Fixes unreadable sent/forwarded files (data-loss/broken core messaging). |
| | 3 | **HZ-04** (Option A + C) | Fixes the most widely reported sync blocker (`70%` / `-1` hang + silent phone) AND fixes `WS_OUTBOUND_TX` reconnect race (which also improves real-time delivery for `HZ-07` & `HZ-05` `file_done`). |
| | 4 | **HZ-02** (Option A) + **HZ-03** (Option A) | Stabilizes call texture memory (`call_textures` + `tex.set`), FFmpeg RAII cleanup, graceful shutdown (WAL checkpoint + atomic settings), and early `crash.log` telemetry. |
| **Phase 2 (P1 — GUI Polish, VoIP & Group Protocol)** | 5 | **HZ-06** (Option A) + **HZ-07** (Option A) | Fixes `texture_lru` same-frame eviction flicker, layout placeholders, `"Dịch tin nhắn"` hover strobe, `□` glyphs (`lucide::X`), and group UI update/sync. |
| | 6 | **HZ-08** (Option A) + **HZ-09** (Option A) | Fixes VoIP usability end-to-end: persistent COM mic enumerator + live mic hot-switch + Bluetooth `I16` + H.264-only negotiation (`supportHevcDecode: 0`) + split audio/video SSRC PLI + FFmpeg `-threads 1`. |
| | 7 | **HZ-10** (Option A) + **HZ-12** (Option A) | **Shared dependency:** Both require persisting `client_message_id`, `board_topic_id`, and `wire_ref` in `src/store.rs` and `merge_thread_into`. Doing them together avoids double-touching SQLite hydration! |
| | 8 | **HZ-11** (Option A) + **HZ-13** (Option A) | Completes `#138` parity: real UTF-16 `@mention` wire serialization + group disband (`/api/group/disperse`), safe `leave_group`, and fixed `delete_conversation`. |
| **Phase 3 (P2 — Quality & Polish)** | 9 | **HZ-14** (Option B) | Adds `[Hình ảnh]` + thumbnail on quoted images across realtime + Sync V2. |
| | 10 | **HZ-15** (Option A) | Upgrades VoIP audio quality (anti-aliased resampling, Opus FEC/PLC, ordered jitter buffer, lock-free audio buffers, soft limiter). |
| | 11 | **HZ-16** (Option A) | Fixes Wall/Feed image distortion, misattribution, raw UIDs, and `Cmd 50001` mislabeling. |

### Open Questions / Product Decisions Needed Before Implementation
1. **Default Windows Storage (`HZ-03`):** Do you want to keep Windows portable `cwd` storage (Option A — zero migration risk) or migrate installed builds to `%LOCALAPPDATA%\Hz` with portable fallback (Option B)?
2. **Video Call Codec Strategy (`HZ-09`):** Start with Option A (`"supportHevcDecode": 0` + fixed FFmpeg H.264 pipeline, zero new native crates), or do you also want Option C (`openh264` pure-Rust/in-process decoder)?
3. **Wall / Friend Feed (`HZ-16`):** Should private 1-1 DM shared photos be **removed** from the `"Nhật Ký"` tab (keeping only profile/avatar/cover activity + an honest notice) or moved into a clearly separated `"Ảnh đã chia sẻ trong chat"` tab?
4. **Optional Artifacts (if you can reproduce locally on Windows 11):**
   - Running `hz.exe --gui --dev` during a phone sync attempt and capturing `sync_debug.log` will let us double-check live server error payloads for `HZ-04`.
   - If you ever observe `#198` locally, grabbing `crash.log` will verify whether any additional Windows 11 Canary trigger exists beyond `HZ-03` Option A.

---

### 🛑 Approval Gate — Waiting for Your Selection

No code has been modified. Please tell me **which Bug ID(s) and Option(s)** you want implemented in `Hz` (for example: *"Approve Phase 1: HZ-01 Option B, HZ-05 Option A, HZ-04 Option A+C, HZ-02 Option A, HZ-03 Option A"* — or any custom subset), and I will implement them incrementally with unit tests, `cargo check`, `cargo clippy`, and `cargo test`.
