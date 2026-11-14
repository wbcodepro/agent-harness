# Ba việc cần người làm bằng tay

Ba phần dưới đây không tự động hóa được: chúng cần một editor đang mở, tài khoản và trình duyệt của bạn, hoặc một máy Windows. Mỗi phần ghi rõ làm gì, thế nào là đạt, và cần gửi lại gì nếu hỏng.

Chuẩn bị chung (macOS hoặc Linux):

```sh
git pull
cargo build --release -p hame-cli        # tạo target/release/hame
```

Đặt API key vào `~/.hame/.env` (ví dụ `DEEPSEEK_API_KEY=...`), vì editor không khởi động `hame` từ thư mục dự án nên sẽ không thấy file `.env` ở đó.

## 1. ACP trong một editor thật

**Đã làm ngày 2026-11-14 trong IntelliJ IDEA 2026.1:** cả năm thao tác trong bảng dưới đều đạt. Cấu hình dùng là `~/.jetbrains/acp.json` với `{"command": "<đường dẫn hame>", "args": ["acp"], "env": {}}`. Lượt đó nêu hai điểm, đều đã sửa: model dùng lệnh chỉ có ở GNU trên macOS (prompt hệ thống giờ nói rõ hệ điều hành), và model nói không có hộp thoại xin phép dù IDE đã hỏi (kết quả của lời gọi được người duyệt giờ ghi rõ điều đó). Zed 1.28.1 cũng đã được thử cùng ngày và đạt cả năm thao tác; cấu hình cần `"type": "custom"` trong mục `agent_servers`. Lượt Zed nêu thêm một điểm, đã sửa: model gửi lại lệnh vừa bị người dùng từ chối (thông báo từ chối giờ nói rõ đừng thử lại).

### Zed

1. Cài Zed, mở `settings.json` (Cmd+,) và thêm, thay đường dẫn cho đúng máy bạn:

   ```json
   {
     "agent_servers": {
       "hame": {
         "command": "/Users/<bạn>/Projects/harness/target/release/hame",
         "args": ["acp"]
       }
     }
   }
   ```

2. Mở một thư mục dự án thử nghiệm (có git), mở bảng Agent, tạo luồng mới và chọn `hame`.

Cấu hình trên viết theo tài liệu của Zed và chưa được thử; nếu Zed báo khóa cấu hình không hợp lệ, gửi lại nguyên văn thông báo.

### IntelliJ IDEA (JetBrains AI Assistant)

Trong cửa sổ AI Chat, mở danh sách agent và chọn mục thêm agent tùy chỉnh; IDE mở một file cấu hình ACP. Khai báo cùng nội dung như trên (`command` là đường dẫn tới `hame`, `args` là `["acp"]`). Tôi chưa kiểm được tên file và khóa cấu hình chính xác của JetBrains, nên hãy theo mẫu IDE tự sinh ra.

### Cần thử gì

| Thao tác | Thế nào là đạt |
|---|---|
| Gửi "liệt kê các file trong dự án và tóm tắt README" | Câu trả lời hiện dần; các lần gọi tool hiện thành mục riêng có trạng thái |
| Gửi "tạo file notes.txt ghi chữ hello" | File xuất hiện trong dự án; editor hiện thay đổi |
| Trong lúc agent đang chạy, bấm dừng | Lượt dừng, gửi yêu cầu tiếp theo vẫn được |
| Đóng rồi mở lại luồng cũ (nếu editor hỗ trợ) | Hội thoại cũ hiện lại |
| Đặt `[sandbox] mode = "danger-full-access"` trong `~/.hame/config.toml`, rồi yêu cầu chạy một lệnh shell | Editor hỏi xin phép; từ chối thì lệnh không chạy |

Nếu hỏng: chạy editor từ terminal để thấy stderr của `hame`, và gửi lại thông báo lỗi cùng file log phiên `~/.hame/sessions/<id>/session.v1.jsonl` (kiểm tra trước rằng nó không chứa gì bạn không muốn chia sẻ).

## 2. Đăng nhập OAuth vào một server MCP thật

**Đã làm ngày 2026-11-14 với Notion** (`https://mcp.notion.com/mcp`), trong một thư mục dữ liệu thử nghiệm: đăng ký client, đăng nhập (người dùng bấm đồng ý trong trình duyệt), server trả 77 tool khi dùng token, token bị đánh dấu hết hạn được làm mới, rồi đăng xuất. Token thử nghiệm đã xóa; quyền cấp cho client `hame` vẫn còn ở phía Notion cho tới khi bạn thu hồi trong phần kết nối của tài khoản.

Các bước dưới đây để bạn đăng nhập cho thư mục dữ liệu thật của mình, hoặc thử với nhà cung cấp khác như Linear.

1. Thêm vào `~/.hame/config.toml` (ví dụ với Linear):

   ```toml
   [mcp.linear]
   url = "https://mcp.linear.app/mcp"
   ```

2. Đăng nhập:

   ```sh
   target/release/hame mcp login linear
   ```

   Trình duyệt mở trang của nhà cung cấp. Đồng ý cấp quyền; trang chuyển về `127.0.0.1` và báo đã đăng nhập; terminal in `signed in to linear`.

3. Kiểm tra token được dùng:

   ```sh
   target/release/hame run "Dùng các tool của linear, liệt kê 3 issue gần nhất của tôi."
   ```

| Kiểm tra | Thế nào là đạt |
|---|---|
| Sau bước 2 | Có file `~/.hame/mcp-auth/linear.json`, chỉ bạn đọc được (`ls -l` cho `-rw-------`) |
| Bước 3 | Model gọi được tool `mcp__linear__...` và trả về dữ liệu thật |
| Chạy lại bước 3 sau vài giờ (khi token đã hết hạn) | Vẫn chạy được mà không phải đăng nhập lại: token tự làm mới lúc phiên bắt đầu |
| `hame mcp logout linear` rồi chạy lại bước 3 | Server MCP báo bị từ chối và gợi ý `hame mcp login linear` |

Điều cần biết trước: lệnh `login` đăng ký một client tên `hame` ở máy chủ cấp quyền của nhà cung cấp và lưu token trên máy bạn; thu hồi quyền trong phần cài đặt tài khoản của dịch vụ đó khi không dùng nữa. Token không được làm mới giữa một phiên dài.

Nếu hỏng: gửi lại nguyên văn dòng lỗi `hame: ...`. Đừng gửi nội dung file token.

## 3. Windows: những phần thêm sau lượt kiểm đầu

Lượt đầu đã ghi ở `docs/WINDOWS-RESULTS.md`. Các phần dưới đây thêm sau đó; trên Windows chúng mới qua CI (biên dịch và test tự động), chưa ai dùng thử. Dán khối sau cho Claude Code trên máy Windows:

```text
Repo này là hame. Lượt kiểm chứng đầu trên máy này đã ghi ở docs/WINDOWS-RESULTS.md.
Hãy git pull, build lại, chạy cargo test --workspace --no-fail-fast, rồi kiểm các phần
thêm sau lượt đó và ghi kết quả vào một mục mới "Lượt hai" trong docs/WINDOWS-RESULTS.md.
Sửa trong mã sản phẩm những gì hỏng, mỗi bản sửa một commit.

1. Lệnh từ file: tạo .hame\commands\tong-ket.md với nội dung "Tóm tắt $ARGUMENTS trong
   ba dòng." rồi chạy hame run "/tong-ket README.md". Prompt gửi cho model phải là nội
   dung đã thay tham số.
2. Loại subagent: tạo .hame\agents\kiem-tra.md (front matter name, description,
   tools: Read, Grep) và yêu cầu agent giao việc cho subagent loại đó. Session con chỉ
   được có tool read và grep.
3. hame plugin install với một thư mục plugin cục bộ, với một URL git, và với một
   marketplace kèm --plugin; rồi hame plugin list và hame plugin update.
4. Tool script: đặt [context] defer_tools_over = 0 và khai báo một server MCP thử (stdio,
   viết bằng Python). Yêu cầu agent dùng script gọi tool đó hai lần. Xác nhận script chạy
   bằng python (máy này không có python3), và rằng nó không ghi được ra ngoài dự án.
5. Thay secret tại proxy: [network] mode = "allowlist" và
   [secrets] substitute = { DEMO_TOKEN = ["httpbin.org"] }, đặt biến môi trường
   DEMO_TOKEN là một giá trị thử. Trong sandbox chạy
   curl -H "Authorization: Bearer %DEMO_TOKEN%" https://httpbin.org/headers
   Kỳ vọng: echo %DEMO_TOKEN% trong sandbox in ra giá trị giả (hame-secret-...); httpbin
   trả về header có secret thật (đầu ra bị che thành [secret:DEMO_TOKEN]); một host khác
   trong allowlist vẫn truy cập được bằng HTTPS. Lưu ý: Windows vốn không giới hạn mạng
   của tool, nên hãy báo rõ proxy có thật sự được dùng không (biến HTTPS_PROXY), và curl,
   node, python có chấp nhận chứng chỉ của proxy không (SSL_CERT_FILE, CURL_CA_BUNDLE,
   NODE_EXTRA_CA_CERTS).
6. Cảnh báo cú pháp: yêu cầu agent sửa một file .py sao cho sai thụt lề. Kết quả của tool
   edit phải có dòng "no longer parses" (kiểm bằng python trên PATH).
7. MCP qua HTTP với server chỉ nói bản không trạng thái 2026-07-28 (xem server mẫu trong
   test của crates/hame-ext/src/mcp.rs) và hame mcp login với máy chủ giả lập trong
   test của crates/hame-ext/src/mcp_auth.rs: chỉ cần xác nhận các test đó đạt trên máy
   này; đăng nhập với nhà cung cấp thật do người dùng tự làm theo docs/KIEM-THU-TAY.md.

Không chạy CI trên GitHub trừ khi cần (gh workflow run ci).
```
