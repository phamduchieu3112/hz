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


---

## Batch 3 — Phone Sync & Group History Sync Fixes (`2026-11-26`)

### Completed Items

| Bug ID | Upstream Issue(s) | Chosen Option | Status | Summary |
| :--- | :--- | :--- | :---: | :--- |
| **HZ-04** | `#170` (part 3), `#201`, `#202` | **Option A + Option C** (Typed sync state machine + WS race fix + `cmd 590` hygiene + dynamic WASM import resolver + `--dev` sync diagnostics) | **Fixed** | Replaced fragile string polling on `LAST_MOBILE_SYNC_STATUS` with typed `SyncSessionState` / `SyncSessionPhase` / `SyncSessionEvent` + `tokio::sync::watch` (`sync_id`-gated so `transfer_error -1` fails immediately instead of confirming, `MobileBusyRestoring` stays `Busy`, and stale `sync_id` events are dropped); fixed `WS_OUTBOUND_TX` generation clobber race, handshake cancellation on receiver drop, and reconnect backoff reset in `socket.rs`; wired `cmd 590` server error ACKs into `SyncSessionState`; fixed `cmd 590` empty `"partition": "conversations"`, capped `queries` to `MAX_SYNC_V2_QUERIES` (`50`, newest active threads first), chunked `fetch_group_profiles` (`40`/call), added one-time TTL consumption for `temp_key`, guarded V1 `pull_mobile_msg` against duplicate POSTs, sent proactive `cmd 592` wakeup after 3s silence with a 15s V2 probe window, surfaced missing `libzproto` WASM / WS / V1 errors clearly, and replaced the 39 hardcoded `wasm-bindgen` hash bindings in `zproto_host.rs` with dynamic signature/prefix linking (`add_imports_for_module` + `strip_wasm_bindgen_hash`). |
| **HZ-07 (Backend / Sync subset)** | `#204` | **Option A** (Group history error/cursor fix + WS `521`/`511` group routing + prioritized streaming refresh + full sidebar fingerprint) | **Fixed** | Added `check_cloud_messages_response` in `chat.rs` so `/api/cm/getrecentv2` / `getoldv2` `error_code != 0` surfaces `SessionError`; split `WorkerCmd::LoadGroupHistory` into `HistoryLoadMode::{RefreshLatest, LoadOlder}` via `decide_history_load_outcome` so `"Đồng bộ"`, API errors, and 1-1 `get_last_msgs` never emit false `HistoryReachedStart`, while `LoadOlder` advances via `server_cursor` first; added `merge_previews_with_group_hint` for WS `cmd 521` / `cmd 511` group routing; added `poll_new_messages_streaming` prioritizing `SELECTED_THREAD_ID` first and streaming batches to the UI; expanded `compute_sidebar_cache_fingerprint` across all threads (past index 64) including unread/pin/mute flags; added `--dev` `log_sync_event` diagnostics. |

### Modified Files
- `src/logger.rs` — Added `log_sync_event(context, detail, is_error)` writing to `sync_debug.log` and `--dev` `LOGGER` (`SYNC` / `SYNC-ERR`) (`HZ-04`, `HZ-07`).
- `src/socket.rs` — Added `WS_OUTBOUND_GEN`, `ActiveOutboundGuard`, `is_ws_outbound_connected`, handshake `tx.closed()` cancellation, authenticated backoff reset, `cmd 590`/`591`/`592` `log_sync_event` tracing, and unit test (`HZ-04`).
- `src/zproto_host.rs` — Replaced hardcoded 39-hash linker bindings with `strip_wasm_bindgen_hash` + `add_imports_for_module` dynamic resolver and 2 unit tests (`HZ-04`).
- `src/chat.rs` — Added `check_cloud_messages_response`, `poll_new_messages_streaming`, `merge_previews_with_group_hint`, `MAX_SYNC_V2_QUERIES` (`50`), `"partition": "conversations"`, active-thread-first partition ordering, chunked `fetch_group_profiles`, `parse_sync_v2_cmd_ack`, `sync_id` on `UserConfirmed`/`UserRejected`/`MobileBusyRestoring`, `pull_mobile_msg` double-send guard, and 5 new unit tests (`HZ-04`, `HZ-07`).
- `src/history_sync.rs` — Added one-time TTL `take_latest_mobile_temp_key`, typed `SyncSessionState` / `SyncSessionPhase` / `SyncSessionEvent` + `watch` channel, rewritten Phase 1 probe/wakeup/confirm/transfer loops, actionable error reporting, and 4 new unit tests (`HZ-04`).
- `src/gui/worker.rs` — Routed mobile sync controls and `ev.cmd == 590` ACKs through `apply_sync_session_event`, passed `in_group_bucket` for WS `521`/`511`, added `SELECTED_THREAD_ID` + streaming `RefreshThreads`, added `HistoryLoadMode` / `decide_history_load_outcome`, and unit test (`HZ-04`, `HZ-07`).
- `src/gui/workspace.rs` — Replaced 64-thread sidebar cache hash with `compute_sidebar_cache_fingerprint`, updated 6 `LoadGroupHistory` callers (`RefreshLatest` / `LoadOlder`), and added unit test (`HZ-07`).
- `src/gui/modals.rs` — Updated `LoadGroupHistory` caller to `HistoryLoadMode::RefreshLatest` (`HZ-07`).
- `src/gui.rs` — Updated `SELECTED_THREAD_ID` on `switch_thread_with_draft` and `publish_known_threads` (`HZ-07`).

### Verification Results
- `cargo check --all-targets`: **PASS** (0 errors, 0 new warnings)
- `cargo clippy --all-targets -j 2`: **PASS** (0 errors, 0 new warnings)
- `cargo test -j 2`: **PASS** — **538 passed; 0 failed; 1 ignored** (2.71s)
  - New tests passing (14 added in Batch 3):
    - `socket::tests::ws_outbound_generation_prevents_stale_socket_clobber`
    - `zproto_host::tests::strip_wasm_bindgen_hash_handles_hashed_and_unhashed_names`
    - `zproto_host::tests::dynamic_import_resolver_links_rotated_wasm_bindgen_hashes`
    - `chat::send_identity_tests::build_sync_v2_payload_uses_conversations_partition_and_caps_queries`
    - `chat::send_identity_tests::extract_mobile_sync_controls_preserves_sync_id_on_confirm_reject_busy`
    - `chat::send_identity_tests::parse_sync_v2_cmd_ack_detects_server_rejection`
    - `chat::send_identity_tests::check_cloud_messages_response_surfaces_session_error_and_stop_retry`
    - `chat::send_identity_tests::merge_previews_with_group_hint_routes_cmd_521_group_payload`
    - `history_sync::tests::sync_state_transfer_error_minus_one_fails_immediately_and_never_confirms`
    - `history_sync::tests::sync_state_mobile_busy_never_confirms_and_stale_sync_id_is_ignored`
    - `history_sync::tests::sync_state_partition_skip_during_transfer_keeps_transferring_and_ws_ack_error_fails`
    - `history_sync::tests::take_latest_mobile_temp_key_consumes_once`
    - `gui::worker::tests::decide_history_load_outcome_never_marks_reached_start_on_refresh_or_error_or_direct`
    - `gui::workspace::quote_navigation_tests::sidebar_cache_fingerprint_detects_changes_past_index_64_and_unread_badges`

### Windows 10/11 `--gui` Manual QA Steps
1. **HZ-04 (`#170` part 3 / `#201` / `#202` Phone Message Sync):**
   - Launch `hz.exe --gui --dev`, log in (and also test right after **Logout → QR re-login** to exercise `WS_OUTBOUND_TX` replacement), then click `"Bắt đầu đồng bộ"` in Settings → Đồng bộ tin nhắn or the left-rail Sync button.
   - **iPhone (Sync V2):** Verify the mobile Zalo app receives the sync prompt (including after 3s proactive `cmd 592` wakeup if backgrounded), confirm on phone, and verify smooth progress without stalling at `70%` or failing with error `-1`. Inspect `--dev` DevTools logs (`SYNC` / `SYNC-ERR`) and `sync_debug.log` for `sync::cmd_590_send`, `ws::sync_ack`, and `sync::state`.
   - **Android (Sync V1 fallback):** Verify V2 probe cleanly times out/falls back to `pull_mobile_msg` (`cmd 12000`), triggers the mobile confirmation prompt, and imports the `.db.crypt` container without hanging.
   - **Missing WASM / Error path:** Temporarily unset/hide Zalo PC WASM (`HZ_ZPROTO_WASM` pointing nowhere and no Zalo PC installed) and trigger sync → verify `Hz` surfaces an explicit, actionable `libzproto` / Zalo PC notice rather than stalling silently for 3 minutes.
2. **HZ-07 (`#204` Group History Sync & Realtime UI Consistency):**
   - Open an active group thread (`Nhóm ...`) in `hz.exe --gui --dev`, send a message to the group from a mobile device → verify the message arrives promptly in the active chat view and updates the sidebar preview/unread badge (even for threads ranked past position 64).
   - Open the right Info Drawer and click `"Đồng bộ"` → verify recent messages refresh cleanly and it does **not** falsely display `"Đã hiển thị toàn bộ lịch sử tin nhắn của cuộc trò chuyện này"` when older history exists or on a transient network error.
   - Scroll to the top of the group and click `"Tải thêm tin nhắn cũ"` → verify older pages load cleanly and `"Đã hiển thị toàn bộ lịch sử..."` only appears once the server actually reaches the start of history.
