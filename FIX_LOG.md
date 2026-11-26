# `Hz` Fix Log

Tracking completed fixes from `bug_analysis_report.md` (branch `clean-release`, `hz` v0.1.3).

---

## Batch 1 — Quick-Win UI & Crash Fixes (`2026-11-26`)

### Completed Items

| Bug ID | Upstream Issue(s) | Chosen Option | Status | Summary |
| :--- | :--- | :--- | :---: | :--- |
| **HZ-01** | `#211`, `#206` (c#2, c#3) | **Option 1** (Local lock-safe helper) | **Fixed** | Extracted `bubble_double_clicked_for_reply(ui, rect)` in `src/gui/messages.rs` to read `dbl_pos` inside `ui.input(...)`, release the `ContextImpl` write lock, and only then evaluate `ui.ctx().layer_id_at(p)`. Eliminates the deterministic `egui::Context` `RwLock` self-deadlock on double-clicking a message bubble with zero signature churn. |
| **HZ-07 (UI-only subset)** | `#204` | **Option 1** (Full UI fix) | **Fixed (UI subset)** | Fixed `"Dịch tin nhắn"` submenu per-frame hover strobe (`!sub_open` → `true` when `!sub_open`), removed stale `sub_open` resize from main `menu_w`/`menu_h`, elevated `bubble_translate_flyout` to `Order::Tooltip`, aligned `bubble_context_menu_open_time` Id in `render_hover_reaction_bar`, and replaced all 8 raw `U+2715` `"✕"` strings with `lucide::X`. *(Note: `HZ-07` group sync / `LoadGroupHistory` remains pending for Batch 3.)* |
| **HZ-14** | `#206` | **Option 1** (Unified label helper across realtime, Sync V2, composer & renderer) | **Fixed** | Added `format_quote_preview(...)` and `format_message_quote_preview(...)` in `src/chat.rs`, wired them into `parse_chat_message`, `send_quote_message_with_client_id`, `src/history_sync.rs` (`ParsedProtoQuote` `cli_msg_type` + `attach` + `decode_sync_v2_proto_message`), composer reply preview (`src/gui/workspace.rs`), and `render_quote_block` (`src/gui/messages.rs`). Captionless quoted images now display `[Hình ảnh]` consistently instead of blank/missing quote boxes. |

### Modified Files
- `src/gui/messages.rs` — `HZ-01` (`bubble_double_clicked_for_reply` + unit test), `HZ-07` (submenu hover/size/order/Id + `lucide::X`), `HZ-14` (`render_quote_block` & `is_frameless_photo` guard).
- `src/gui/reactions_dialog.rs` — `HZ-07` (`lucide::X` on clear-reaction button).
- `src/gui/modals.rs` — `HZ-07` (`lucide::X` on doc-preview search clear & video viewer close button).
- `src/gui/workspace.rs` — `HZ-07` (`lucide::X` on keyword watch & script delete buttons), `HZ-14` (`format_message_quote_preview` in composer reply banner).
- `src/chat.rs` — `HZ-14` (`format_quote_preview`, `format_message_quote_preview`, `parse_chat_message`, `send_quote_message_with_client_id`, and unit test).
- `src/history_sync.rs` — `HZ-14` (`ParsedProtoQuote` `cli_msg_type`/`attach`, `parse_proto_quote`, `decode_sync_v2_proto_message`, and unit test).

### Verification Results
- `cargo check --all-targets`: **PASS** (0 errors, 0 new warnings)
- `cargo clippy --all-targets`: **PASS** (0 errors)
- `cargo test`: **PASS** — **516 passed; 0 failed; 1 ignored** (2.80s)
  - New tests passing:
    - `gui::messages::tests::double_click_for_reply_does_not_deadlock_on_real_egui_context`
    - `chat::oa_card_tests::quote_preview_labels_captionless_and_captioned_media`
    - `history_sync::tests::sync_v2_proto_quote_labels_captionless_and_captioned_image`

### Pending / Deferred from This Batch
- **HZ-07 (Group Sync / `LoadGroupHistory` subset):** Deferred to **Batch 3** as requested.

---

## Batch 2 — Lifecycle Hardening, Texture/Layout Stability & Active-Call Stability (`2026-11-26`)

### Completed Items

| Bug ID | Upstream Issue(s) | Chosen Option | Status | Summary |
| :--- | :--- | :--- | :---: | :--- |
| **HZ-03** | `#198` | **Option A** (Early crash telemetry + graceful shutdown + guarded `session.json` startup) | **Fixed** | Installed `panic::set_hook` and top-level `run_main()` error logging before `data_dir::initialize` with writable fallback (`%LOCALAPPDATA%\Hz\crash.log` / temp) and a native Win32 `MessageBoxW` error dialog in release GUI mode; replaced abrupt `std::process::exit(0)` in `main.rs`, `on_exit`, and `"Thoát hoàn toàn"` with `WorkerCmd::Shutdown` (ending active call + `MessageStore::checkpoint_wal` `PRAGMA wal_checkpoint(TRUNCATE)`) plus a 2s teardown watchdog; made `AppSettings::save` and fallback `threads_cache.json` writes atomic (`write_private_atomic`); pre-validated disk fonts via `validate_font_bytes` inside `with_suppressed_panic_log`; removed duplicate boot `RefreshThreads`; deferred corner notification viewports until `rendered_frames >= 3`. |
| **HZ-02** | `#206` (remaining) | **Option A** (Latest-frame atomic slot + in-place `TextureHandle::set` + bounded FFmpeg/drop lifecycle) | **Fixed** | Replaced unbounded `GuiEvent::StickerImageReady` call video frames with coalescing `CALL_VIDEO_FRAMES` (`publish_call_video_frame` / `drain_call_video_frames`) and a dedicated `call_textures` map (`8 MiB` cap) updated in place via `TextureHandle::set`; wrapped `ffmpeg` children in RAII `ChildGuard` (closing pipes, killing, and bounded `try_wait` reaping) and tracked/joined decoder & capture `JoinHandle`s; recovered poisoned `input_buf` locks and guarded `cpal::Stream` drops with `catch_unwind`; offloaded `MediaHandle` / `PureRustCallState` / `GroupMemberLeg` drops to `spawn_blocking` outside `active_call.lock()`; replaced UI-thread `is_ffmpeg_available()` / `list_media_devices(false)` with non-blocking probes warmed by the worker; kept `render_call_overlay` alive when the main window is minimized. |
| **HZ-06** | `#206`, `#211` (remaining) | **Option A** (LRU self-eviction fix + 128 MiB budget + stable placeholders + viewport fetch culling) | **Fixed** | Unified `ZaloGuiApp::texture_lru` to `TEXTURE_BUDGET` (`128 MiB`), protected same-frame batches via `insert_protecting_two` + `pending_paint_textures`, capped uploads at 6/frame (`deferred_sticker_ready`), lowered non-HD chat thumbnail decode cap to `480px`, and removed redundant `request_repaint()` calls after `gui_tx.send`; added deterministic placeholders for stickers (`116×116`), link thumbnails (`260×132` upfront width), videos (`240×150`), image files (`240×150`), and wire-dimension photo sizing (`extract_wire_photo_dims` + `compute_photo_display_size`); gated media/link fetches by `is_cursor_near_viewport` (`clip_rect().expand(400.0)`); completed `mark_texture_used` and `failed_stickers` backoff coverage across modals, stickers, albums, video cards, and shared photos. |

### Modified Files
- `src/logger.rs` — `with_suppressed_panic_log`, `is_panic_log_suppressed`, `append_crash_log`, `append_crash_log_to_candidates`, and 2 unit tests (`HZ-03`).
- `src/main.rs` — Early panic hook, `run_main` error logger + Win32 `MessageBoxW` fatal startup dialog, and clean `eframe` exit (`HZ-03`).
- `src/store.rs` — `MessageStore::checkpoint_wal` and unit test (`HZ-03`).
- `src/chat.rs` — Atomic `threads_cache.json` fallback write in `save_threads_cache_for` (`HZ-03`).
- `src/gui/theme.rs` — `validate_font_bytes` + `try_insert_validated_font` before `ctx.set_fonts` and unit test (`HZ-03`).
- `src/gui/worker.rs` — `WorkerCmd::Shutdown` (call teardown + WAL checkpoint), removed duplicate boot `RefreshThreads`, warmed non-blocking FFmpeg/device probes, lowered chat `target_decode_max_dim` to `480`, removed redundant `request_repaint()` (`HZ-03`, `HZ-02`, `HZ-06`).
- `src/media.rs` — `CALL_VIDEO_FRAMES` coalescing mailbox, `is_ffmpeg_available_nonblocking`, `ChildGuard` RAII kill/reap, tracked thread joins, poisoned-lock recovery, `catch_unwind` stream drops, `MediaHandle::request_stop`, and 2 unit tests (`HZ-02`).
- `src/audio.rs` — `catch_unwind` around `cpal::Stream` drop in `VoiceRecorder::drop` (`HZ-02`).
- `src/call.rs` — `drop_media_async`, `drop_call_leg_async`, and `drop_call_state_async` outside `active_call.lock()` (`HZ-02`).
- `src/media_resources.rs` — `ResourceLru::insert_protecting_two` and unit test (`HZ-06`).
- `src/gui.rs` — Atomic `AppSettings::save`, `rendered_frames` tracking, clean `on_exit` shutdown + 2s watchdog, `call_textures` isolation + `apply_call_video_frames`, minimized `render_call_overlay`, `TEXTURE_BUDGET` (128 MiB), `pending_paint_textures`, 6/frame upload batching, and `is_permanently_failed_sticker_fetch` (`HZ-03`, `HZ-02`, `HZ-06`).
- `src/gui/modals.rs` — Clean `"Thoát hoàn toàn"` close, deferred toast viewport until `rendered_frames >= 3`, `call_textures` migration, non-blocking camera/FFmpeg checks, and `mark_texture_used` / `request_sticker_image` backoff audit (`HZ-03`, `HZ-02`, `HZ-06`).
- `src/gui/messages.rs` — `is_cursor_near_viewport`, `extract_wire_photo_dims`, `compute_photo_display_size`, `cover_uv_for_box`, deterministic sticker/link/video/image-file placeholders, album/video backoff + viewport culling, `mark_texture_used`, and unit test (`HZ-06`).
- `src/gui/stickers.rs` — `mark_texture_used` on sticker pack/grid/suggest textures (`HZ-06`).
- `src/gui/workspace.rs` — Non-blocking camera/FFmpeg check on video call start and `request_sticker_image` backoff in shared-media gallery (`HZ-02`, `HZ-06`).
- `src/property_invariants.rs` — Updated `test_target_decode_max_dim_bounds_avatars_and_hd_photos` for the `480px` chat thumbnail cap (`HZ-06`).

### Verification Results
- `cargo check --all-targets`: **PASS** (0 errors, 0 new warnings)
- `cargo clippy --all-targets -j 2`: **PASS** (0 errors, 0 new warnings)
- `cargo test -j 2`: **PASS** — **524 passed; 0 failed; 1 ignored** (4.51s)
  - New tests passing:
    - `logger::tests::append_crash_log_falls_back_to_next_writable_candidate`
    - `logger::tests::panic_log_suppression_guard_restores_on_unwind`
    - `store::tests::checkpoint_wal_truncates_cleanly_and_preserves_data`
    - `gui::theme::tests::validate_font_bytes_rejects_corrupt_and_accepts_bundled_ttf`
    - `media::tests::call_video_mailbox_coalesces_to_latest_frame_and_clears`
    - `media::tests::child_guard_kills_and_reaps_without_zombie`
    - `media_resources::admission_and_drop_tests::insert_protecting_two_prevents_same_frame_batch_self_eviction`
    - `gui::messages::md_block_tests::extract_wire_photo_dims_and_display_size_match_pre_and_post_decode`

### Windows 10/11 `--gui` Manual QA Steps
1. **HZ-03 (`#198` Reopen After Close):**
   - Launch `hz.exe --gui`, log in (`session.json` created), open several threads so SQLite WAL has recent writes, then close via window `X`, `Alt+F4`, `"Thoát hoàn toàn"`, and `Ctrl+Q`.
   - Confirm `hz.exe` exits cleanly from Task Manager within ~2s and immediately reopens across **10 consecutive close/reopen cycles** without deleting `session.json`.
   - Place a corrupt font path in `app_settings.json` (`font_family`) and launch `hz.exe --gui` → verify fallback to bundled font without crashing; launch from a read-only working directory with a forced fatal error → verify `%LOCALAPPDATA%\Hz\crash.log` and Win32 error dialog appear.
2. **HZ-02 (`#206` Stability While Calling & Using UI):**
   - Start a 10–15 minute 1-1 or group video call in `hz.exe --gui` while actively switching chats, scrolling photo-heavy threads, toggling Camera On/Off 10×, and minimizing/restoring the main window.
   - Verify the call window stays alive when minimized, RAM/VRAM remains stable (no runaway leak), and Task Manager shows **0 lingering `ffmpeg.exe`** processes after hang-up.
3. **HZ-06 (`#206` / `#211` Image & Link-Thumbnail Flicker / Layout Jitter):**
   - Open a thread with 50+ photos, albums, stickers, videos, and rich link cards (`Cloud của tôi` / active group).
   - Scroll rapidly up and down and switch back/forth between threads → verify zero whole-screen flicker, zero same-frame thumbnail disappearing/reloading, stable bubble widths/heights when thumbnails finish loading, and ~0% idle CPU once visible media settles.
