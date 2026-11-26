# Tính năng của Hz (Hiếu's Zalo) `v0.1.3`

> Tài liệu tổng hợp các tính năng đang được hỗ trợ trong **Hz (Hiếu's Zalo)** — ứng dụng Zalo Desktop & Terminal viết bằng Rust thuần (`egui` GUI, `ratatui` TUI, SQLite cục bộ, REST API & MCP).

---

## 1. Giao diện & Đa nền tảng
- **GUI Native (`--gui`, mặc định):** Xây dựng trên `eframe`/`egui 0.29` với nền sáng/tối, khay hệ thống (Linux), cửa sổ thông báo gọn nhẹ và trình xem ảnh/video/tài liệu tích hợp.
- **Terminal UI (`--tui`):** Giao diện dòng lệnh `ratatui` tối ưu bàn phím, hỗ trợ đăng nhập QR hoặc mật khẩu.
- **Đa tài khoản & Cô lập dữ liệu:** Chuyển đổi nhiều tài khoản trong app, hỗ trợ `--profile <NAME>` và `--data-dir <PATH>`.

## 2. Nhắn tin, Gắn thẻ & Đa phương tiện
- **Chat 1-1 và Nhóm:** Gửi/nhận tin nhắn thời gian thực qua WebSocket (`zpw_ws`) và long-polling dự phòng.
- **Gắn thẻ `@mention` (Mới cập nhật `#138`):**
  - Gõ `@` trong ô soạn thảo để gợi ý thành viên nhóm hoặc `@All` kèm avatar và vai trò (`Trưởng nhóm`, `Phó nhóm`).
  - Tính toán chính xác offset UTF-16 (`pos`, `len`, `type`) gửi qua `/api/group/mention`, `/api/group/quote`, `/api/group/sendlink`, và hiển thị nổi bật đúng thẻ tên tiếng Việt nhiều từ / chữ thường ở cả tin gửi đi và tin nhận về.
- **Chia sẻ & Chuyển tiếp tập tin (`#138`):**
  - Tải lên nhiều phần (`asyncfile/upload`), chờ sự kiện `file_done` (`cmd 601`, hỗ trợ cả `content`/`data` JSON chuỗi hoá), kiểm tra chặt `fileId` và `fileUrl`, gửi kèm `checksum` (MD5), `checksumSha` (SHA-256), `extension`, `imei` và `visibility: 0` cho nhóm.
  - Chuyển tiếp (`"Chia sẻ"`) tập tin và video thông qua tải về có xác thực + tải lên lại sạch sẽ, giúp người nhận mở/tải bình thường thay vì nhận liên kết `/renewMedia` hết hạn dạng văn bản.
- **Ghim / Bỏ ghim tin nhắn (`#138`):**
  - Ghim và bỏ ghim tin nhắn 1-1 và nhóm (`message_topic_params` dùng `server_message_id`, `client_message_id`, `senderUid` thật của người gửi, `msg_type`).
  - Thanh **Tin ghim** cố định đầu hội thoại hỗ trợ xem lần lượt nhiều tin ghim (`+N`), bấm để nhảy tới tin gốc, bấm `"Bỏ ghim"` để đồng bộ lên máy chủ, và lưu bền `board_topic_id` qua SQLite.
  - Đồng bộ bảng tin nhóm theo thời gian thực qua sự kiện control WebSocket và làm mới bảng tin nhóm (`/api/board/pin/list`, `/api/board/list`).
- **Thu hồi tin nhắn (`#138`):**
  - Thu hồi tin nhắn 1-1 và nhóm (`/api/message/undo`, `/api/group/undomsg`), bảo toàn `client_message_id` và `wire_ref` qua SQLite và đồng bộ điện thoại, hiển thị thông báo lỗi rõ ràng nếu máy chủ từ chối, và khóa nút thu hồi sau giới hạn 24 giờ của Zalo.
- **Quản lý hội thoại & Nhóm (`#138`):**
  - **Giải tán nhóm** (`/api/group/disperse`) dành riêng cho trưởng nhóm (`creatorId`) kèm kiểm tra quyền và hộp thoại xác nhận rõ ràng.
  - **Rời nhóm** (`/api/group/leave`) chuẩn hoá ID nhóm và chỉ xoá hội thoại cục bộ sau khi máy chủ xác nhận thành công.
  - **Xoá hội thoại** (`/api/message/deleteconver`, `/api/group/deleteconver`) gửi đầy đủ metadata `conver` (`ownerId`, `cliMsgId`, `globalMsgId`) và lưu tombstone SQLite tránh nhóm bị đồng bộ lại khi chưa có tin mới.
- **Các tiện ích khác:** Trả lời trích dẫn (kể cả ảnh không chú thích), cảm xúc, nhãn dán, bình chọn (poll), ghi chú & từ khóa nhóm, hẹn giờ gửi, nhắn tự xoá cục bộ, xem trước PDF/Excel/Office.

## 3. Lưu trữ cục bộ, Đồng bộ & Tự động hóa
- **SQLite cục bộ & Tìm kiếm:** Lưu trữ lịch sử chat theo tài khoản, hỗ trợ `--offline`, tìm kiếm nhanh và xuất dữ liệu.
- **Dual Sync V2/V1:** Đồng bộ lịch sử từ điện thoại (Android/iOS) và đồng bộ tin nhắn nhóm từ Cloud.
- **Gọi thoại & Gọi video:** Codec Opus VoIP (FEC/PLC, adaptive jitter buffer, anti-aliased Hermite resampling), chuyển mic nóng khi đang gọi, và video H.264 qua `ffmpeg`.
- **REST API, MCP & AI:** Máy chủ HTTP cục bộ (`--api-port`), MCP JSON-RPC 2.0 (`--mcp`), LLM Agent (`--agent`), script Rhai theo hội thoại và cầu nối Telegram (`--tg-bridge`).
