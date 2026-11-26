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
