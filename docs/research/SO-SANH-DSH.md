# hame và DeepSeek Harness: so sánh theo mã nguồn

Đọc ngày 2026-11-15 trên bản `deepseek-ai/deepseek-harness` mới nhất (commit `39163dd3`, 2026-11-13, bản 0.2.2-rc.3): khoảng 1,39 triệu dòng TypeScript trong 6.518 file, 57 package. Bốn agent đọc song song bốn mảng (vòng lặp và ngữ cảnh; tool, sandbox, quyền; lớp model; mở rộng, nhiều agent, giao diện, đo đạc), sau đó các điểm chính được kiểm lại trực tiếp trên mã: không có giới hạn số bước, không có `pipefail`, `promoteOnTimeout`, bản tóm tắt dùng lại tiền tố cache (`compaction-basic/src/summarizer.ts`), client LSP không nhận `publishDiagnostics`, MCP chưa có OAuth (`mcp-manager/README.md`).

## Tóm tắt

DSH mạnh ở **kỷ luật giữ cache** và **độ bền của log**: mọi thay đổi (system prompt, tool, ngữ cảnh chạy) được nối thêm thay vì viết lại đầu prompt, bản tóm tắt khi nén là một tiền tố đúng của request trước nên trúng cache, mỗi lần thử lại được ghi vào log trước khi chờ. Họ cũng có nhiều tính năng sản phẩm (desktop Electron, lịch chạy định kỳ, webhook GitHub, chợ plugin, PTC).

hame mạnh ở **độ chính xác và an toàn**: hoàn thành phải có bằng chứng (lệnh kiểm tra, `stop_on_red`, người chấm độc lập), leo thang model khi kẹt, kiểm tra cú pháp và chẩn đoán LSP sau khi sửa, sandbox chặn cả đọc và mạng, secret không tới được model, plugin WASM bị cô lập, và có bộ eval tác vụ lập trình có bộ kiểm ẩn. DSH để model tự nói "xong" (README của gói `goal` ghi rõ "No independent evaluator"), không có bộ eval tác vụ lập trình trong repo (`benchmarks/` chỉ đo hiệu năng).

## Theo từng mảng

| Mảng | DSH | hame |
|---|---|---|
| Vòng lặp | Không giới hạn số bước; hết `max_tokens` là dừng lượt; lỗi giữa stream ghi `assistant/attempt` rồi thử lại cả bước | Giới hạn 100 bước, cảnh báo trước 3 bước; tiếp tục khi hết token (2 lần); stream đứt thì thử lại 3 lần |
| Log phiên | JSONL chỉ ghi thêm, fsync theo lô 200 ms, bắt buộc flush trước mỗi request và mỗi tool; có zstd, migrate định dạng v0→v4, tìm kiếm FTS5 giữa các phiên; không ghi delta stream | JSONL chỉ ghi thêm, ghi cả delta stream (gộp 50 ms/4 KB) nên crash không mất phần đã stream; có rewind và checkpoint workspace |
| Thử lại | 5 lần, 500 ms→10 s, jitter 10%, tôn trọng `Retry-After` (quá trần thì bỏ), số lần thử ghi vào log | 5 lần trước khi stream, 3 lần khi stream đứt; chưa đọc `Retry-After` |
| Nén ngữ cảnh | Ở mọi bước khi chạm min(0,8×cửa sổ, cửa sổ−đầu ra); cắt kết quả tool lớn trước (không cần model); giữ nguyên ~16% cuối; **bản tóm tắt gửi lại nguyên tiền tố hội thoại + một câu yêu cầu ở cuối nên trúng cache**; tràn cửa sổ thì nén lại và thử lại | Ở mọi bước khi vượt ngưỡng; xóa kết quả tool cũ trước (tùy chọn); bản tóm tắt gửi transcript dạng chữ với system prompt riêng, **không trúng cache** |
| Giữ cache | System prompt đổi thì nối vào lịch sử; thêm/bớt tool gửi dạng delta; thông tin chạy (cwd, sandbox) gửi dạng nhắc nhở chỉ khi đổi; cấu hình gọi model cố định theo phiên | Tiền tố ổn định, `prompt_cache_key`, `cache_control`; system prompt hoặc tool đổi thì viết lại đầu prompt |
| Hoàn thành | Model tự gọi `complete`; gói `goal` chạy tối đa 256 vòng, chỉ có luật về quyền đổi trạng thái | Lệnh kiểm tra của dự án, `stop_on_red`, checklist, người chấm độc lập, trạng thái bằng chứng (`passed/failing/unchecked`) |
| Leo thang | Không có; chỉ nhắc khi gọi tool lặp 3/5/8 lần | Tăng mức suy luận, đổi model mạnh hơn hoặc nhà cung cấp khác khi kẹt; cũng nhắc khi lặp |
| Sửa file | Search-replace chính xác (chỉ chuẩn hóa CRLF), `apply_patch` có so khớp mờ 4 bậc (tắt mặc định); báo lỗi không kèm gợi ý; không kiểm tra cú pháp; LSP chỉ tra cứu, không chẩn đoán | Search-replace, hashline, apply_patch, ghi cả file; kiểm tra cú pháp sau khi sửa; chẩn đoán LSP; `then_run`; liệt kê nơi gọi và test sau khi đổi chữ ký; LSP `rename_symbol` |
| Shell | `bash -c` mới mỗi lần, không `pipefail`, giữ 64 KB phần cuối, phần thừa ra file; **lệnh quá giờ tự chuyển thành tác vụ nền**, xong thì đánh thức agent; có bash PTY bền | Tiến trình mới mỗi lần, `pipefail`, giữ đầu và cuối, lọc dòng test đạt/tiến trình build, phần đầy đủ ra file; PTY |
| Sandbox | Linux bwrap (dự phòng Landlock, không seccomp); macOS chỉ chặn ghi; Windows Low integrity (tự ghi là "partial"); tool file trong tiến trình chỉ kiểm đường dẫn; **không kiểm soát mạng**; secret chỉ lọc biến môi trường theo tên, file credential đọc được | Mọi tool có tác dụng phụ chạy trong worker bị giam (Seatbelt, Landlock+seccomp, Low integrity); chặn đọc thư mục nhạy cảm; proxy mạng theo danh sách tên miền; thay secret tại proxy nên model chỉ thấy tên |
| Quyền | Ba mức có sẵn; model xin mức rộng hơn kèm lý do ngay trong lời gọi tool | Mức quyền và luật allow/ask/deny |
| Model | DeepSeek (endpoint kiểu Anthropic) và pi-ai cho các nhà cung cấp khác; giữ chữ ký thinking theo chuỗi băm tiền tố | DeepSeek, OpenAI (chat, Responses), Anthropic, endpoint tương thích; nhớ tham số bị từ chối; gợi ý khi hết tiền |
| Chi phí | Đo token theo nguồn; không tính tiền, không ngân sách | Đo theo nguồn, ngân sách token cho cả cây subagent |
| Nhiều agent | Subagent bất đồng bộ (`send_message`, `wait_agent`); **fork con từ lượt của cha để dùng lại cache**; gọi Claude Code, Codex, ACP làm agent con; tool `workflow` để model viết kịch bản `agent()`/`parallel()` với đầu ra theo schema; worktree là tool riêng, không tự động | Subagent (explore/work), worktree riêng tự động khi `isolate`, chạy song song, giới hạn độ sâu, ngân sách token, bảng việc chung, người chấm độc lập |
| Mở rộng | Mọi thứ là plugin Cordis chạy chung tiến trình, không cô lập; chợ plugin; MCP stdio/HTTP, chưa có OAuth; PTC: model viết TypeScript gọi tool trong một tiến trình Node bị giam | MCP stdio/HTTP/stateless, OAuth, tool hoãn khai báo; skills; hooks/lệnh/plugin dạng Claude Code; plugin WASM khai báo quyền; tool `script` chuỗi các tool hoãn |
| Giao diện | CLI headless/sdk/acp/web, desktop Electron, web React, SDK JSON-RPC qua stdio và Python, LiveShare; không có TUI | TUI, `hame run`, web, ACP (đã thử trong Zed và IntelliJ), JSON-RPC qua stdio và WebSocket |
| Kiểm thử | ~2.000 file phiên ghi sẵn phát lại làm test, kiểm cả file trong workspace; benchmark hiệu năng có ngân sách CPU/RAM; không có bộ eval tác vụ lập trình trong repo | Test tất định với model giả; `hame eval` với bộ tác vụ có bộ kiểm ẩn, nhiều lần chạy, phần giữ lại, chạy harness khác trên cùng tác vụ; `hame probe` |

## Nên học từ DSH, xếp theo lợi ích trên công sức

| # | Kỹ thuật | Lợi ích | Ghi chú |
|---|---|---|---|
| 1 | Bản tóm tắt khi nén gửi lại nguyên tiền tố hội thoại, câu yêu cầu đặt cuối | Rẻ: phần lớn token đầu vào của lần nén trúng cache | **Đã làm** (2026-11-15), tự bật khi model tóm tắt là model chính; model không trả bản tóm tắt bằng chữ thì quay về cách cũ. Đo với Anthropic thật (`crates/hame-context/tests/live_compaction.rs`): lần tóm tắt đọc 39.090 token từ cache, chỉ 212 token mới. Cách cũ cắt mỗi kết quả tool còn 2.000 byte và trả giá đầy đủ: khi kết quả tool lớn, hai cách tốn gần bằng nhau nhưng cách mới thấy đủ nội dung; khi kết quả nhỏ (thường gặp, vì hame đã lọc đầu ra), cách mới rẻ hơn khoảng 10 lần ở phần đầu vào |
| 2 | Lệnh quá giờ chuyển thành tác vụ nền thay vì bị giết, model đọc tiếp sau | Nhanh: không phải chạy lại build/test dài | Cần tool đọc đầu ra tác vụ nền |
| 3 | Đọc `Retry-After`; quá trần thì báo lỗi ngay | Ít lần thử vô ích khi bị giới hạn tốc độ | Nhỏ |
| 4 | Fork subagent từ lịch sử của cha | Rẻ: con dùng lại cache của cha | Hợp với việc con cần ngữ cảnh của cha |
| 5 | Thay đổi system prompt hoặc tool nối thêm vào lịch sử thay vì viết lại đầu prompt | Giữ cache khi bật/tắt tool giữa phiên | Phụ thuộc nhà cung cấp hỗ trợ |
| 6 | Khi crash giữa một lời gọi tool, ghi rõ "chưa chạy" hay "không rõ kết quả, chỉ chạy lại nếu an toàn" | Chính xác khi tiếp tục phiên | Nhỏ |
| 7 | Tràn cửa sổ ngữ cảnh thì nén ngay rồi thử lại request | Bền | Nhỏ |

Không nên chép: plugin chạy chung tiến trình không cô lập, sandbox chỉ chặn ghi, để model tự quyết định hoàn thành, không giới hạn số bước.
