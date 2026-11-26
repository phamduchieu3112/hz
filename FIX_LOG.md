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


---

## Batch 4 — Wall / Friends' Timeline Fix (`2026-11-26`)

### Completed Items

| Bug ID | Upstream Issue(s) | Chosen Option | Status | Summary |
| :--- | :--- | :--- | :---: | :--- |
| **HZ-16** | `#156` | **Option A** (Native image aspect/crop + strict UID attribution + contact name/avatar resolution + standard post parsing + separate chat-media & `--dev` protocol tab) | **Fixed** | Preserved native aspect ratios across all Feed image renderers via `compute_feed_single_image_layout` and `feed_cover_uv` (`egui::Image::new(tex).uv(...)` with `maintain_aspect_ratio(false)` for single photos, cover banners, 2/3-column album grids, link thumbnails, and personal avatars); removed DM photo fabrication from `"Nhật Ký"` and moved 1-1 shared photos into a dedicated `"Ảnh chia sẻ trong chat"` tab (`collect_shared_chat_media`) with accurate `is_outgoing` sender attribution (`"Bạn"` vs friend name) and real message timestamps; added `normalize_feed_uid` (`_0` pversion suffix stripping), `is_raw_uid_label`, `is_usable_social_image_url`, `resolve_feed_author`, and `filter_feed_item_for_target`, removed the `.take(120)` batch cap (raised to `1000` in chunks of `40` resilient to per-chunk errors), passed `known_contacts` (`FeedProfileFallback`) from `self.threads` + self profile, and offloaded SQLite timeline reads onto `tokio::task::spawn_blocking`; extended `FeedItem` / `parse_feed_item_json` / `parse_timeline_feed_value` to parse & render standard timeline post types (`Text`, `Photo`, `Album`, `Video` with duration/play overlay, `Link` preview cards, `SharedPost` cards, `location`, and inline `comments`); gated the protocol inspector tab behind `self.dev_mode` and corrected the mislabeled `ERR_FEED_NO_NETWORK = 50001` vs `CMD_FEED_TIMELINE = 1230` label. |

### Modified Files
- `src/feed.rs` — Added `FeedProfileFallback`, `FeedPostKind`, `FeedLinkPreview`, `FeedSharedPost`, `FeedMedia::duration_secs`, `FeedItem` extended fields, `normalize_feed_uid`, `is_raw_uid_label`, `is_usable_social_image_url`, `resolve_feed_author`, `filter_feed_item_for_target`, `parse_timeline_feed_value`; fixed `SocialUserProfile::from_json`, `get_friend_social_profiles_batch`, `fetch_timeline_feed` (removed DM fabrication, added `known_contacts` enrichment + `spawn_blocking` SQLite load + missing UID resolution), and `parse_feed_item_json` across standard post types + 5 new unit tests.
- `src/gui.rs` — Replaced `FeedModalTab::Packet50001` with `FeedModalTab::SharedChatMedia` and `FeedModalTab::ProtocolDebug`; synchronized `self.feed_profile` on `GuiEvent::SocialTimelineLoaded` when a target UID is already selected.
- `src/gui/worker.rs` — Updated `WorkerCmd::FetchTimelineFeed` to pass `known_contacts: HashMap<String, FeedProfileFallback>` instead of DM `shared_media_by_uid`.
- `src/gui/modals.rs` — Added `feed_cover_uv`, `compute_feed_single_image_layout`, `SharedChatPhotoCard`, and `collect_shared_chat_media`; updated `trigger_feed_timeline_load`, `render_feed_page` (dev-gated `CMD_FEED_TIMELINE = 1230` tab), `render_feed_timeline_column` (aspect-accurate single/album/cover images, video overlay, link/shared-post/comment cards, strict UID filtering, honest desktop-protocol note), `render_feed_shared_chat_media_column`, `render_feed_avatars_column`, and `render_feed_packet_column` + 3 new unit tests.

### Verification Results
- `cargo check --all-targets`: **PASS** (0 errors, 0 new warnings)
- `cargo clippy --all-targets -j 2`: **PASS** (0 errors, 0 new warnings)
- `cargo test -j 2`: **PASS** — **546 passed; 0 failed; 1 ignored** (16.14s)
  - New tests passing (8 added in Batch 4):
    - `feed::tests::normalize_feed_uid_strips_pversion_suffix_and_preserves_plain_ids`
    - `feed::tests::social_profile_from_json_normalizes_pversion_key_and_filters_default_urls`
    - `feed::tests::resolve_feed_author_never_reattributes_nonempty_uid_by_display_name`
    - `feed::tests::filter_feed_item_for_target_matches_strictly_by_normalized_uid`
    - `feed::tests::parse_timeline_feed_supports_standard_post_types`
    - `gui::modals::tests::feed_cover_uv_center_crops_portrait_landscape_ultrawide_and_square`
    - `gui::modals::tests::compute_feed_single_image_layout_preserves_native_aspect_ratio`
    - `gui::modals::tests::collect_shared_chat_media_attributes_outgoing_to_self_and_incoming_to_friend`

### Windows 10/11 `--gui` Manual QA Steps
1. **Image Aspect Ratio & Crop (`#156`):**
   - Launch `hz.exe --gui` and open **Tường nhà / Nhật ký bạn bè** from the left rail.
   - Inspect single portrait/landscape/ultrawide photos, friend cover banners, 2–6+ photo album grids, and **Kho Ảnh Cá Nhân** (`130×130` tiles) → verify crisp aspect-accurate framing with zero horizontal/vertical stretching or squishing, and click images to confirm `ImageViewerModal` opens cleanly.
2. **Author Attribution, Display Names & Avatars (`#156`):**
   - Verify no raw numeric UIDs (`686454...`) or digit-initial fallback avatars appear on feed cards or the left friends list for known contacts.
   - Click a friend in the left sidebar or click `"Xem hồ sơ & Nhật ký"` from a chat Info Drawer → verify strict per-UID filtering (no posts from another user with the same display name).
   - Open the **`Ảnh chia sẻ trong chat`** tab → verify outgoing 1-1 photos (e.g., sent receipts) are attributed to **your own name/avatar** (`"Bạn đã gửi trong chat với ..."`), never attributed to the friend.
3. **Standard Timeline Post Types & Protocol Tab (`#156`):**
   - With a test SQLite DB containing `timeline_feed_1` / `user_feed` posts (`ZALO_TIMELINE_DB`), verify text-only posts, single photos, multi-photo albums, video cards (play badge + duration + click to open URL), link preview cards, shared/reposted cards, locations, and inline comments all render cleanly without blocking the UI thread.
   - Confirm `"Giao thức Feed (Cmd 1230)"` is hidden in normal `hz.exe --gui` mode and visible only in `hz.exe --gui --dev` with the corrected `CMD_FEED_TIMELINE = 1230` / `ERR_FEED_NO_NETWORK = 50001` explanation.

---

## Batch 5 — VoIP Audio/Video Reliability & Quality Fixes (`2026-11-26`)

### Completed Items

| Bug ID | Upstream Issue(s) | Chosen Option | Status | Summary |
| :--- | :--- | :--- | :---: | :--- |
| **HZ-08** | `#170` (Audio input) | **Option A** (Persistent COM audio enumerator + fault-safe cache + mid-call stream hot-reload + `F32`/`I16`/`U16`) | **Fixed** | Moved all `cpal` host enumeration and device resolution onto a process-lifetime `"hz-audio-enumerator"` thread so `cpal 0.15.3`'s WASAPI `OnceLock<Enumerator>` STA COM apartment is never torn down across short-lived scans (also wired `play_notification_chime` and `VoiceRecorder::start`); added `merge_mic_scan_result` + `catch_unwind` device name reads so transient errors or empty WASAPI glitches never wipe previously listed mics; added `list_media_devices_with_ttl` (`5s` auto-refresh in Settings and Call Overlay without blocking egui); added `MIC_SELECTION_EPOCH`, `build_call_input_stream`/`build_call_output_stream` (`F32`/`I16`/`U16` for Bluetooth HFP), and live `run_media_loop` input-stream drop/rebuild on mic switch or stream error recovery; added `--dev` `audio::mic_select`, `audio::mic_switch`, `audio::device_scan`, and `audio::voice_recorder_open` telemetry. |
| **HZ-09** | `#170` (Incoming video) | **Option A** (Disable false HEVC negotiation + split audio/video SSRC + validated depacketizer + SPS/PPS cache + FFmpeg low-delay fix) | **Fixed** | Set `"supportHevcDecode": 0` in `build_pure_rust_invite_fields_with_probe` and `build_callee_extend_data` (`src/call.rs`) so Android/iPhone peers negotiate H.264 instead of sending undecodable HEVC; split `peer_ssrc` into `peer_audio_ssrc` and `peer_video_ssrc` (`resolve_video_feedback_ssrc`) so audio packets never hijack `PLI`/`REMB`/`RR` targets; guarded `0x0E` vs non-video/FEC payloads (`is_incoming_video_rtp`); replaced raw fragment buffers with `VideoDepacketizer` enforcing RTP `seq`/`ts` continuity, single-packet `S && E` FU-A handling, and `pli_gap` recovery on corrupt fragments; added `H264AccessUnitPreparer` to cache out-of-band SPS/PPS without splitting them from IDRs via premature `H264_AUD`; updated `spawn_ffmpeg_h264_child` & `run_local_preview_decoder` (`-fflags nobuffer -flags low_delay -threads 1 -probesize 32768 -analyzeduration 0`) and replaced the 22-AU kill loop with a 4s stall threshold + soft PLI retry; added `--dev` `video::pli_sent`, `video::keyframe`, `video::first_frame`, `video::stall`, and `video::decode_stats` telemetry. |
| **HZ-15** | `#160` (Audio quality) | **Option A** (VoIP Opus FEC/bitrate/complexity + anti-aliased Hermite resampler + lock-free SPSC ring buffers + adaptive jitter buffer + HPF & soft limiter) | **Fixed** | Configured Opus VoIP encoder (`32 kbps` constrained VBR, `Signal::Voice`, complexity `8`, in-band FEC enabled, `10%` loss hint); replaced point-sampled mutex resampling with zero-`unsafe` lock-free SPSC `AudioRingBuffer`s and phase-continuous `AudioResampler` (2nd-order Butterworth `BiquadLpf` + 4-point Catmull-Rom Hermite across 8/16/44.1/48 kHz without 8 kHz 2× speed bugs or playback over-drain crackles, plus smooth underrun decay); added 80 Hz `HighPassFilter80Hz` and smooth `tanh` `soft_limit`; added ordered adaptive `AudioJitterBuffer` (40–80 ms adaptive target, 200 ms cap) invoking Opus LBRR FEC (`decode_float(..., true)`) and PLC (`decode_float(&[], ..., false)`) on a 20 ms playout ticker; fixed the 319-sample TX silence-injection bug and added `--dev` `audio::quality_stats` telemetry. |

### Modified Files
- `src/logger.rs` — Added `log_voip_event`, shared `push_dev_log_entry`, serialized logger tests with `TEST_LOGGER_LOCK`, and added unit test (`HZ-08`, `HZ-09`, `HZ-15`).
- `src/call.rs` — Set `"supportHevcDecode": 0` in invite/callee `extendData` builders and added unit test (`HZ-09`).
- `src/audio.rs` — Updated `VoiceRecorder::start()` to resolve `selected_mic_device()` via `crate::media::resolve_input_device` with `--dev` telemetry (`HZ-08`).
- `src/gui/modals.rs` — Updated Settings Audio/Video and Call Overlay device queries to `list_media_devices_with_ttl(false, Some(Duration::from_secs(5)))` (`HZ-08`).
- `src/media.rs` — Added persistent `"hz-audio-enumerator"`, fault-safe cache merge + TTL refresh, `MIC_SELECTION_EPOCH` live mic switching, `F32`/`I16`/`U16` stream builders, `AudioRingBuffer`, `BiquadLpf` + `AudioResampler`, `HighPassFilter80Hz`, `soft_limit`, `configure_voip_opus_encoder`, `AudioJitterBuffer`, `resolve_video_feedback_ssrc`, `is_incoming_video_rtp`, `VideoDepacketizer`, `H264AccessUnitPreparer`, FFmpeg low-delay flags + 4s stall recovery, `--dev` VoIP telemetry, and 11 new unit tests (`HZ-08`, `HZ-09`, `HZ-15`).

### Verification Results
- `cargo check --all-targets`: **PASS** (0 errors, 0 new warnings)
- `cargo clippy --all-targets -j 2`: **PASS** (0 errors, 0 new warnings)
- `cargo test -j 2`: **PASS** — **559 passed; 0 failed; 1 ignored** (2.24s)
  - New tests passing (13 added in Batch 5):
    - `logger::tests::log_voip_event_records_only_in_dev_mode`
    - `call::tests::invite_and_callee_extend_data_disable_hevc_and_advertise_h264`
    - `media::tests::merge_mic_scan_result_preserves_cache_on_error_and_merges_hotplug`
    - `media::tests::pcm_integer_sample_conversions_roundtrip_accurately`
    - `media::tests::video_feedback_ssrc_is_not_clobbered_by_audio_ssrc`
    - `media::tests::is_incoming_video_rtp_rejects_fec_on_0x0e_and_accepts_h264`
    - `media::tests::video_depacketizer_handles_fu_a_single_se_and_drops_seq_or_ts_breaks`
    - `media::tests::h264_au_preparer_caches_sps_pps_without_splitting_aud_and_prepends_to_idr`
    - `media::tests::voip_opus_encoder_config_enables_fec_bitrate_complexity_and_voice`
    - `media::tests::audio_resampler_preserves_rate_and_sine_clarity_across_8k_44k1_48k`
    - `media::tests::audio_ring_buffer_is_fifo_bounded_and_wraparound_safe`
    - `media::tests::audio_jitter_buffer_reorders_invokes_fec_and_plc_and_caps_latency`
    - `media::tests::hpf_80hz_blocks_dc_and_soft_limiter_bounds_peaks`

### Windows 10/11 `--gui` Manual QA Steps
1. **HZ-08 (`#170` Live Microphone Switching & Hot-Plug):**
   - Launch `hz.exe --gui --dev`, open **Settings → Âm thanh & Video cuộc gọi**, and confirm built-in mics appear.
   - Start a voice/video call on the built-in mic, then connect a Bluetooth headset (e.g., `UGREEN HiTune` HFP `I16` mode) or USB mic mid-call.
   - Wait up to 5s (or click Refresh `⟳`) in the Call Overlay or Settings → verify **both** the built-in mic and the newly connected headset appear (unselected mics do **not** vanish).
   - Switch between mics live during the call and test recording a Voice Message → inspect `--dev` DevTools (`VOIP` logs `audio::device_scan`, `audio::mic_switch`, `audio::voice_recorder_open`) and confirm the peer immediately hears the newly selected mic without restarting `Hz`.
2. **HZ-09 (`#170` Incoming Video Call on Windows PC):**
   - With `ffmpeg` installed in `PATH`, place a 1-1 video call from `hz.exe --gui --dev` to **Android** and **iPhone** (both outgoing and incoming directions).
   - Confirm remote video renders inside the main call canvas within ~2 seconds (no black screen) alongside the local PIP preview, and stays stable when toggling remote camera off/on.
   - Inspect `--dev` DevTools (`VOIP` logs `video::pli_sent`, `video::keyframe`, `video::first_frame`, `video::decode_stats`) and `voip.log` to verify H.264 negotiation, clean SPS/PPS caching, and `0` continuous FFmpeg restart loops.
3. **HZ-15 (`#160` Call Audio Quality vs Official Zalo):**
   - Run a 5-minute call on Windows 10/11 (`hz.exe --gui --dev`) using (a) a 48 kHz / 44.1 kHz built-in/USB mic and (b) an 8 kHz / 16 kHz Bluetooth HFP headset, comparing speech clarity side-by-side against official Zalo Desktop.
   - Confirm clear voice with no metallic aliasing, no 2× chipmunk speed on 8 kHz Bluetooth, no buffer-overdrain crackles, and graceful recovery under brief Wi-Fi packet loss (`VOIP` `audio::quality_stats` showing `jb_target_ms`, `fec`, and `plc` activity).


---

## Batch 6 — Upstream `#138` (File Sharing, `@mention` Tagging, Pin/Unpin, Recall & Group Disband/Delete) (`2026-11-27`)

### Completed Items (Strict Priority Order)

| Bug ID | Upstream Issue(s) | Chosen Option | Status | Summary |
| :--- | :--- | :--- | :---: | :--- |
| **HZ-05** | `#138` (sub 5) | **Option A + Option B fallback** (Validated `asyncfile/msg` + `file_done` fix + authenticated re-upload forwarding) | **Fixed** | Updated `extract_file_done_controls` (`src/chat.rs`) to parse stringified JSON `content` and `data`, added `mark_file_done_listener_active()` at WebSocket startup (`src/gui/worker.rs`), added `resolve_upload_chunk_size`, `sha256_hex_bytes`, and `build_asyncfile_msg_payload` (rejects empty `fileUrl` or `-1` `fileId`, computes `checksum` MD5 + `checksumSha` SHA-256, uses `"extension"`, adds `imei` and group `visibility: 0`, sets `MessageKind::File`/`Video`); added `is_forwardable_file_message`, `is_forwardable_video_message`, `forward_file_or_video_via_reupload`, and `forward_message_detailed_with_client_id` so `"Chia sẻ"` downloads with session auth and re-uploads files cleanly instead of forwarding expiring `/renewMedia` URLs as plain text or faking local `AttachmentSent` cards. |
| **HZ-11** | `#138` (sub 1) | **Option A + Option B fallback** (Composer span tracking + UTF-16 `mentionInfo` + wire-driven rendering) | **Fixed** | Added `WireMention`, `OutgoingMention`, `build_wire_mentions` (exact UTF-16 `pos`/`len` offsets with member-name scan fallback), `serialize_mention_info`, `build_group_mention_payload` (routing non-empty mentions to `/api/group/mention`), `send_group_message_with_mentions`, `send_link_message_with_mentions`, `send_quote_message_with_mentions`, `extract_wire_mention_spans`, `extract_wire_mention_spans_for_display` (`extract_wire_mention_spans_for_text`), and `extract_resolved_mention_uids_from_wire` in `src/chat.rs`; removed `#[allow(dead_code)]` on `MentionCandidate.id` and tracked `composer_pending_mentions` across keyboard/click/context-menu mentions (`src/gui/workspace.rs`, `src/gui.rs`, `src/gui/worker.rs`); added `parse_inline_md_runs_with_mentions` and `render_chat_markdown_block_with_mentions` (`src/gui/messages.rs`) to highlight multi-word/lowercase Vietnamese tags from wire offsets. |
| **HZ-12** | `#138` (sub 2) | **Option A** (Top Pinned Banner + pin payload/persistence fix + group board refresh & WS pin controls) | **Fixed** | Persisted `board_topic_id`, `client_message_id`, `wire_ref` (`server_id`, `client_id`, `sender`, `mentions`), and attachment `msg_type`s across `StoredMessage::from`, `hydrate_threads` (`src/store.rs`), and `merge_thread_into` (`src/chat.rs`); updated `message_topic_params` (`server_message_id`, `client_message_id`, real outgoing `my_uid`, `msg_type`, media preview fallback); added `BoardPinControlEvent` / `extract_board_pin_controls` (`src/chat.rs`) and `GuiEvent::BoardPinControlsReceived` (`src/gui/worker.rs`, `src/gui.rs`); fixed `GuiEvent::MessagePinResult` (toast feedback, clearing `board_topic_id` on unpin, preserving it on failed unpin); enabled throttled 60s `maybe_refresh_board_pins` for groups; re-enabled the top Pinned Message Banner (`src/gui/workspace.rs`) with multi-pin `+N` cycling, click-to-jump (`QuoteJump`), and server-synced `"Bỏ ghim"`. |
| **HZ-10** | `#138` (sub 4) | **Option A** (SQLite/Sync `client_message_id` & `wire_ref` persistence + `server_message_id` recall + toast errors + 24h guard) | **Fixed** | Preserved `client_message_id` and `wire_ref` across `src/store.rs` and `src/history_sync.rs` (both backup DB import and `decode_sync_v2_proto_message`); added `resolve_recall_server_id` and `can_recall_message` (24-hour window) and updated `build_recall_payload` (`src/chat.rs`); broadened `extract_msg_id` (`msgId | globalMsgId | realMsgId`) and added `GuiEvent::RecallFailed` (`src/gui/worker.rs`, `src/gui.rs`) to surface visible error toasts; gated `"Thu hồi"` in `src/gui/messages.rs` with disabled `"Thu hồi (quá 24 giờ)"` feedback past 24 hours. |
| **HZ-13** | `#138` (sub 3) | **Option A** (`/api/group/disperse` + fixed `leave_group` & `delete_conversation` + SQLite tombstones + server-ACK delete) | **Fixed** | Added shared `clean_group_id`, `build_disperse_group_payload` / `disperse_group` (`/api/group/disperse`), `build_leave_group_payload` (`clean_group_id`), and `build_delete_conversation_payload` / `delete_conversation_with_meta` (real `conver: { ownerId, cliMsgId, globalMsgId }` + `error_code` check) in `src/chat.rs`; added `deleted_threads` SQLite tombstones (`record_thread_deleted`, `clear_thread_deleted`, `list_deleted_threads` in `src/store.rs`) to block `getlg/v4` resurrection until `newest_ts > deleted_at_ts` (`src/gui.rs`); added `WorkerCmd::DisperseGroup`, `GuiEvent::GroupDispersed`, `GuiEvent::ThreadDeleted`, `is_current_user_group_creator`, `"Giải tán nhóm"` button + `"Xác nhận giải tán nhóm"` dialog, and made leave/disband/delete wait for server ACK before removing local state (`src/gui/worker.rs`, `src/gui.rs`, `src/gui/workspace.rs`). |

### Modified & Created Files
- `src/chat.rs` — `HZ-05` (`sha256_hex_bytes`, `mark_file_done_listener_active`, `resolve_upload_chunk_size`, `build_asyncfile_msg_payload`, `extract_file_done_controls`, `is_forwardable_file_message`, `is_forwardable_video_message`, `forward_file_or_video_via_reupload`, `forward_message_detailed_with_client_id`), `HZ-11` (`clean_group_id`, `WireMention`, `OutgoingMention`, `build_wire_mentions`, `serialize_mention_info`, `build_group_mention_payload`, `send_group_message_with_mentions`, `send_link_message_with_mentions`, `send_quote_message_with_mentions`, `extract_wire_mention_spans`, `extract_wire_mention_spans_for_text`, `extract_resolved_mention_uids_from_wire`), `HZ-12` (`merge_thread_into` backfill, `message_topic_params`, `BoardPinControlEvent`, `extract_board_pin_controls`), `HZ-10` (`resolve_recall_server_id`, `can_recall_message`, `build_recall_payload`), `HZ-13` (`build_delete_conversation_payload`, `delete_conversation_with_meta`, `build_leave_group_payload`, `build_disperse_group_payload`, `disperse_group`), and 5 new unit tests.
- `src/store.rs` — Persisted/hydrated `client_message_id`, `board_topic_id`, `wire_ref` (`server_id`, `client_id`, `sender`, `mentions`), and attachment `msg_type`; added `deleted_threads` tombstone table/methods and 1 new unit test (`HZ-10`, `HZ-12`, `HZ-13`).
- `src/history_sync.rs` — Preserved `client_message_id` / `wire_client_id` / `wire_server_id` in SQLite backup import and `decode_sync_v2_proto_message` (`HZ-10`, `HZ-12`).
- `src/gui.rs` — Added `MentionMessageSent`, `RecallFailed`, `BoardPinControlsReceived`, `GroupDispersed`, `ThreadDeleted`, `composer_pending_mentions`, `board_pins_refreshed_at`, `pinned_banner_index`, `pending_disperse_group`, `deleted_thread_tombstones`, `is_current_user_group_creator`, `remove_thread_locally_with_tombstone`, enabled group `maybe_refresh_board_pins`, and fixed `MessagePinResult` & `GroupLeft` (`HZ-11`, `HZ-12`, `HZ-10`, `HZ-13`).
- `src/gui/worker.rs` — Activated `file_done` listener at WS startup, updated `WorkerCmd::Forward`, added `mentions` to `SendMessage` / `SendQuote`, forwarded `BoardPinControlsReceived`, added `request_repaint()` on pin/board events, broadened `extract_msg_id`, emitted `RecallFailed`, handled `DisperseGroup` & `DeleteThread` with server ACK, and added 1 new unit test (`HZ-05`, `HZ-11`, `HZ-12`, `HZ-10`, `HZ-13`).
- `src/gui/workspace.rs` — Wired `MentionCandidate.id` & `composer_pending_mentions`, re-enabled top Pinned Message Banner (cycle + jump + unpin), added creator-gated `"Giải tán nhóm"` menu/drawer entries + `"Xác nhận giải tán nhóm"` dialog, wired Info Drawer `"Xoá lịch sử trò chuyện"` to `WorkerCmd::DeleteThread`, and waited for server ACK on leave/delete/disband (`HZ-11`, `HZ-12`, `HZ-13`).
- `src/gui/messages.rs` — Added `parse_inline_md_runs_with_mentions` & `render_chat_markdown_block_with_mentions`, enforced 24h recall guard in context menu, and added 1 new unit test (`HZ-11`, `HZ-10`).
- `src/gui/modals.rs` — Updated `WorkerCmd::SendMessage` call sites with `mentions: Vec::new()` (`HZ-11`).
- `docs/TINH_NANG.md`, `docs/DA_BIET.md` (created), `README.md`, `README.vi.md` (updated) — Documented newly supported `#138` features and remaining protocol notes.

### Verification Results
- `cargo check --all-targets`: **PASS** (0 errors, 0 new warnings)
- `cargo clippy --all-targets -j 2`: **PASS** (0 errors, 0 new warnings on modified/added code)
- `cargo test -j 2`: **PASS** — **567 passed; 0 failed; 1 ignored** (2.34s)
  - New tests passing (8 added in Batch 6):
    - `chat::send_identity_tests::hz05_file_done_stringified_json_and_asyncfile_payload_validation`
    - `chat::send_identity_tests::hz11_build_wire_mentions_utf16_offsets_and_group_payload`
    - `chat::send_identity_tests::hz12_message_topic_params_and_ws_pin_controls`
    - `chat::send_identity_tests::hz10_recall_resolves_wire_server_id_and_enforces_24h_limit`
    - `chat::send_identity_tests::hz13_group_id_normalization_disperse_leave_and_delete_conver_payloads`
    - `store::tests::hz10_hz12_hz13_persists_pin_recall_wire_metadata_and_deleted_tombstones`
    - `gui::messages::md_block_tests::hz11_wire_mention_spans_highlight_lowercase_multiword_vietnamese_names`
    - `gui::worker::tests::extract_msg_id_reads_global_and_real_ids_from_data_or_top_level`

### Windows 10/11 `--gui` Manual QA Steps
1. **HZ-05 (`#138` File Sharing & Forwarding):**
   - Launch `hz.exe --gui`, open a 1-1 chat and a group chat, and send a `.pdf`, `.docx`, and multi-chunk `.zip` (>2 MB). Confirm official Zalo Mobile/PC recipients can download and open every file intact (never receiving an empty file shell).
   - Right-click a received file message (including `/renewMedia` attachments) → click `"Chia sẻ"` (Forward) to another 1-1 or group thread → confirm the recipient receives a working, downloadable file card (never a plain-text `/renewMedia` URL).
2. **HZ-11 (`#138` `@mention` Tagging):**
   - In a group chat, type `@` (including after emojis like `Chào 😊 @`), pick `@All` and a member with a Vietnamese multi-word name via `Tab`/`Enter` and mouse click → send both a normal message and a quoted reply.
   - Confirm the tagged user receives a mention notification on official Zalo Mobile/PC, and both outgoing and incoming messages highlight `@All` and multi-word/lowercase names cleanly in `Hz`.
3. **HZ-12 (`#138` Pin / Unpin Messages):**
   - Right-click a message in a group chat (and a 1-1 chat) → click `"Ghim tin nhắn"` → verify toast `"📌 Đã ghim tin nhắn"` and that the top **Tin ghim** banner appears immediately.
   - Pin a second message → test `+N` cycling, click the banner text to jump to the pinned message, restart `hz.exe --gui` to verify SQLite persistence, then click `"Bỏ ghim"` on the banner → confirm unpinned on both `Hz` and official Zalo Mobile/PC.
4. **HZ-10 (`#138` Recall Messages):**
   - Send a text message and an image/file in both 1-1 and group chats, restart `hz.exe --gui` (so messages hydrate from SQLite), then right-click → `"Thu hồi"` → verify both disappear on official Zalo Mobile/PC.
   - Inspect context menu on an outgoing message older than 24 hours → confirm `"Thu hồi (quá 24 giờ)"` is disabled with the 24h tooltip.
5. **HZ-13 (`#138` Delete / Disband / Leave Group):**
   - Create a temporary test group in `Hz` (you are `creatorId`): open the sidebar context menu or right Group Info Drawer → verify `"Giải tán nhóm"` is visible → click it, confirm in `"Xác nhận giải tán nhóm"` → verify the group is permanently disbanded and stays gone after `"Làm mới"`.
   - In a group where you are a non-creator member → verify `"Giải tán nhóm"` is hidden, click `"Rời nhóm"` → confirm it removes only after server confirmation and does not reappear on thread refresh; test `"Xoá hội thoại"` / `"Xoá lịch sử trò chuyện"` and verify the tombstone keeps it hidden until new activity arrives.
