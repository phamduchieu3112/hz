# Hz (Hiếu's Zalo) v0.1.4

## Tiếng Việt

### Điểm nổi bật
- **Gắn thẻ `@mention` đầy đủ cho nhóm lớn (500–1000+ thành viên):**
  - Tự động tải toàn bộ danh sách thành viên nhóm theo từng đợt bất đồng bộ (off-UI thread), hiển thị tiến độ tải mượt mà và giữ nguyên tổng số thành viên thực tế của máy chủ trên thanh tiêu đề & ngăn thông tin nhóm.
  - Ghim `@All` (`Báo cho cả nhóm`) ở đầu danh sách, hỗ trợ tìm kiếm tiếng Việt không dấu theo thời gian thực và cuộn mượt ảo hóa (`show_rows`) cho hàng trăm thành viên.
  - Gửi đúng offset UTF-16 qua `/api/group/mention`, `/api/group/quote`, `/api/group/sendlink` và tô sáng chính xác thẻ tên tiếng Việt nhiều từ.
- **Tính năng nhắn tin & quản lý nhóm hoàn chỉnh:**
  - **Chia sẻ & chuyển tiếp tệp tin** đáng tin cậy (xác thực `asyncfile/msg`, kiểm tra `checksum` MD5/SHA-256, tải & tải lại có xác thực khi chuyển tiếp tệp/video).
  - **Ghim / bỏ ghim tin nhắn** với thanh **Tin ghim** trên đầu cuộc trò chuyện (chuyển nhanh nhiều tin ghim `+N`, bấm để nhảy tới tin gốc, đồng bộ hai chiều qua SQLite & WebSocket).
  - **Thu hồi tin nhắn** ổn định kể cả sau khi khởi động lại ứng dụng, kèm giới hạn 24 giờ và thông báo lỗi rõ ràng.
  - **Rời nhóm, giải tán nhóm & xóa hội thoại:** Hỗ trợ giải tán nhóm dành cho trưởng nhóm, chuẩn hóa ID nhóm khi rời nhóm và dùng tombstone SQLite để tránh hội thoại đã xóa xuất hiện lại.
- **Gọi thoại & Video Call ổn định hơn trên Windows:**
  - **Cải thiện chất lượng âm thanh VoIP:** Opus VoIP (FEC, 32 kbps VBR, PLC), bộ lọc chống răng cưa Hermite cho tai nghe Bluetooth 8/16 kHz và micro 44.1/48 kHz, khử DC 80 Hz và jitter buffer thích ứng.
  - **Cắm nóng micro & nhận video H.264:** Tự động nhận micro/tai nghe Bluetooth mới cắm trong cuộc gọi, giải mã H.264 ổn định với `ffmpeg.exe` đóng gói sẵn trong bộ cài đặt (không cần cài thêm thủ công).
- **Ổn định khởi động, giao diện & đồng bộ:**
  - Tự động lưu an toàn cài đặt, dọn WAL SQLite khi thoát để mở lại ứng dụng liên tục không lỗi, chống nhấp nháy ảnh/link-preview (giới hạn LRU 128 MiB + placeholder cố định), sửa khóa khi nhấp đúp trả lời tin nhắn, sửa hiển thị ảnh trích dẫn và tỉ lệ ảnh nhật ký/tường cá nhân.
  - Sửa đồng bộ điện thoại (Sync V2) & tải lịch sử nhóm ổn định.
- **Bộ cài đặt Windows 1 tệp & tự động cập nhật:**
  - Cung cấp bộ cài đặt `Hz-Setup-v0.1.4.exe` đóng gói sẵn `hz.exe` + `ffmpeg.exe` (cài đặt per-user không cần quyền Admin, tự đóng/khởi động lại khi nâng cấp).
  - Tự động kiểm tra và tải bản cập nhật mới trực tiếp từ GitHub Releases của `phamduchieu3112/hz`.

---

## English Summary

### Highlights
- **Full Large-Group `@mention` & Member Loading:** Progressive off-UI-thread chunked group member profile resolution, accurate server `totalMember` preservation in headers/drawers, `@All` (`Báo cho cả nhóm`) pinned at the top, Vietnamese accent-insensitive live filtering, virtualized `show_rows` scrolling for 600–1000+ members, and UTF-16 wire mention offsets.
- **Messaging & Group Management:** Reliable file sharing/forwarding (`asyncfile/msg` validation + authenticated re-upload), top Pinned Message Banner with multi-pin cycling & SQLite/WS sync, persistent 24h-guarded message recall, creator-only group disband, normalized leave-group, and SQLite deleted-thread tombstones.
- **Windows VoIP & Video Call Quality:** Opus VoIP with in-band FEC/PLC, anti-aliased Hermite resampling across 8/16/44.1/48 kHz devices, live mic hot-plug detection, H.264 incoming video decoding, and bundle-first `ffmpeg.exe` discovery (zero extra setup required).
- **Stability, Sync & Packaging:** Graceful shutdown with SQLite WAL checkpoint, 128 MiB texture LRU + stable media placeholders, double-click reply deadlock fix, Sync V2 / group history sync hardening, single-file per-user Windows installer (`Hz-Setup-v0.1.4.exe`), and in-app auto-updater synced to `phamduchieu3112/hz`.
