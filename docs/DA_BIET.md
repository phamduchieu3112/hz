# Các vấn đề đã biết & Giới hạn giao thức (`Hz` `v0.1.3`)

> Tài liệu ghi nhận trạng thái các lỗi/giới hạn hiện có trong **Hz (Hiếu's Zalo)** và các ràng buộc từ phía máy chủ Zalo Desktop/Web (`zpw_type=30`).

---

## 1. Các hạng mục Upstream `#138` đã được xử lý trong `Hz`
Các lỗi trong issue `#138` đã được khắc phục ở mức giao thức và giao diện:
- **Chia sẻ & chuyển tiếp tập tin (`HZ-05`):** Không còn gửi `asyncfile/msg` khi `fileUrl` rỗng hoặc chuyển tiếp `/renewMedia` thành văn bản thuần; đã bổ sung `checksumSha` (SHA-256), `extension`, `imei`, `visibility: 0` và chuyển tiếp bằng tải lại sạch.
- **Gắn thẻ `@mention` (`HZ-11`):** Autocomplete composer lưu `uid` và tính offset UTF-16 gửi qua `/api/group/mention`, `/api/group/quote`, `/api/group/sendlink`, đồng thời tô sáng chính xác thẻ tên tiếng Việt nhiều từ từ metadata wire.
- **Ghim / Bỏ ghim tin nhắn (`HZ-12`):** Đã bật lại thanh Tin ghim đầu hội thoại, sửa `message_topic_params`, lưu bền `board_topic_id` trong SQLite, và đồng bộ bảng tin nhóm qua API + WebSocket.
- **Thu hồi tin nhắn (`HZ-10`):** Đã lưu bền `client_message_id` và `wire_ref` trong SQLite/Sync, ưu tiên `server_message_id` khi thu hồi, hiển thị toast lỗi khi thất bại và áp dụng giới hạn 24 giờ.
- **Xoá / Rời / Giải tán nhóm (`HZ-13`):** Đã hỗ trợ giải tán nhóm cho trưởng nhóm (`/api/group/disperse`), chuẩn hoá ID nhóm khi rời nhóm, hoàn thiện `conver` cho `deleteconver` và dùng tombstone SQLite chống hồi sinh hội thoại cũ.

---

## 2. Giới hạn & Hành vi đã biết còn lại

1. **Đọc lại danh sách ghim 1-1 (`friendboard/list`):**
   - API `friendboard/list` / `board/oneone/list` của Zalo trả về mã lỗi `114` trên một số định dạng tham số trong phiên Web/PC. Vì vậy **Hz** chỉ tự động poll bảng tin cho **nhóm**; hội thoại 1-1 dựa trên ghim đã lưu trong SQLite cục bộ và các sự kiện control WebSocket (`pin_message` / `unpin_topic`).
2. **Giới hạn thu hồi 24 giờ:**
   - Máy chủ Zalo chỉ cho phép thu hồi tin nhắn của chính bạn trong vòng **24 giờ** kể từ lúc gửi. Quá mốc này, menu chuột phải hiển thị trạng thái vô hiệu `"Thu hồi (quá 24 giờ)"`.
3. **Đồng bộ lịch sử E2EE từ điện thoại (Sync V2):**
   - Giao thức Sync V2 cần module WASM `libzproto` (tự động phát hiện từ Zalo PC hoặc chỉ định qua biến môi trường `HZ_ZPROTO_WASM`). Nếu không có `libzproto`, ứng dụng tự chuyển về Sync V1 hoặc thông báo rõ cho người dùng.
4. **Cuộc gọi video H.264:**
   - Yêu cầu có `ffmpeg` trong `PATH` (hỗ trợ luồng giải mã H.264 độ trễ thấp; tắt quảng cáo HEVC để tương thích đa thiết bị).
5. **Tường nhà / Nhật ký bạn bè (`Nhật Ký`):**
   - Giao thức Zalo Web/PC (`zpw_type=30`) không cung cấp REST API Timeline đầy đủ như ứng dụng di động; ảnh chia sẻ trong chat 1-1 được tách riêng sang tab `"Ảnh chia sẻ trong chat"`.
6. **Xác nhận quyền trưởng nhóm trước khi giải tán:**
   - Nút `"Giải tán nhóm"` chỉ hiển thị sau khi danh sách thành viên nhóm (`creatorId`) được tải xong. Nếu trưởng nhóm muốn rời nhóm mà không giải tán, máy chủ Zalo yêu cầu chuyển quyền trưởng nhóm trên thiết bị di động/Zalo PC chính thức trước.
