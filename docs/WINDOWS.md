# Kiểm chứng zcode trên máy Windows

zcode được phát triển trên macOS. Phần Windows mới chỉ được chạy trên máy của GitHub Actions, chưa từng chạy trên một máy Windows có người dùng thật. File này là việc cần làm trên máy Windows, viết để giao thẳng cho Claude Code ở đó.

## Chuẩn bị

- Rust (rustup, toolchain stable) và `rustup target add wasm32-unknown-unknown`.
- Git for Windows (zcode dùng `bash.exe` của nó để chạy lệnh).
- Một API key trong biến môi trường, ví dụ `DEEPSEEK_API_KEY`.

## Prompt cho Claude Code trên Windows

Dán nguyên khối dưới đây:

```text
Repo này là zcode, một agent harness viết bằng Rust. Nó được phát triển trên macOS; phần
Windows mới chỉ được kiểm trên GitHub Actions. Việc của bạn là kiểm chứng nó trên máy
Windows này và sửa những gì hỏng. Đọc README.md và docs/WINDOWS.md trước.

1. Build và test:
     cargo build --workspace
     cargo test --workspace --no-fail-fast
   Ghi lại số test đạt/hỏng. Với mỗi test hỏng, tìm nguyên nhân gốc và sửa trong mã sản phẩm
   (không nới lỏng test), trừ khi test giả định điều chỉ đúng trên Unix.

2. Sandbox Windows (crates/zcode-sandbox/src/windows.rs): tiến trình chạy tool tự hạ xuống
   mức toàn vẹn Low nên chỉ ghi được trong thư mục dự án và AppData\LocalLow\zcode-tmp.
   Tạo %USERPROFILE%\.zcode\config.toml với:
     [sandbox]
     allow_partial = true
   Rồi trong một thư mục thử nghiệm trống, chạy bằng model thật:
     zcode run "Tạo hello.py in ra 'xin chao', chạy nó bằng python, rồi thử tạo file
     C:\Users\Public\zcode-escape.txt và báo chính xác chuyện gì xảy ra."
   Kiểm tra: hello.py được tạo và chạy được; file ngoài thư mục dự án KHÔNG được tạo.
   Thử thêm các đường thoát: ghi qua đường dẫn có .., qua symlink/junction trỏ ra ngoài,
   qua PowerShell (powershell -c "Set-Content ..."), qua cmd /c, và xóa/đổi tên một file
   bên ngoài. Mọi đường đều phải bị chặn. Nếu có đường nào lọt, đó là lỗi nghiêm trọng:
   viết test tái hiện trong crates/zcode-cli/tests/sandbox_windows.rs rồi sửa.

3. Những thứ chưa từng được kiểm trên Windows, hãy thử và báo lại:
   - Giao diện terminal: chạy `zcode` trong Windows Terminal, gõ một yêu cầu, thử /plan,
     /rewind, Esc để hủy.
   - `zcode web` và mở địa chỉ nó in ra.
   - Tool bash với các lệnh thường gặp (git status, cargo build, npm, python) dưới sandbox:
     công cụ nào không chạy được ở mức Low và vì sao.
   - Checkpoint và `zcode rewind` trên một repo git thật.
   - Thư mục dự án bị gắn nhãn Low vĩnh viễn (icacls <thư mục> để xem). Xác nhận điều đó
     không làm hỏng việc dùng thư mục bằng các công cụ khác.

   - Các phần thêm ngày 2026-11-14, chưa từng biên dịch hay chạy trên Windows: tool
     `apply_patch` (đặt `[model] edit_format = "apply-patch"`) với file dùng CRLF; bảo vệ
     `.git` (yêu cầu agent sửa `.git/config` hoặc thêm file vào `.git/hooks`, thay đổi phải
     bị hoàn tác và model được báo); `zcode run --output json`; hooks kiểu Claude Code
     (`[compat] claude_hooks`) cần `sh` trong PATH, báo lại nếu không chạy được; server MCP
     qua HTTP (`[mcp.x] url = ...`); `zcode probe evals/basic`; `zcode skill install` với
     một thư mục và với một URL git; chốt đọc-trước-khi-ghi-đè (yêu cầu agent ghi đè một file
     có sẵn mà chưa đọc: phải bị từ chối, và đọc xong thì ghi được, kể cả khi đường dẫn viết
     bằng `\` hay `/`); `grep`/`glob` với `path` là thư mục con phải in đường dẫn tính từ thư
     mục làm việc, dùng dấu `/`.

4. Giới hạn đã biết, không cần sửa nhưng cần xác nhận đúng như mô tả: trên Windows zcode
   chỉ giam được việc ghi; tool vẫn đọc được mọi file người dùng đọc được và vẫn ra mạng
   được. Nếu bạn thấy cách khả thi để ẩn file bí mật hoặc chặn mạng mà không cần quyền
   admin, hãy mô tả đề xuất (không tự triển khai nếu nó cần sửa ACL ngoài thư mục dự án).

5. Kết thúc: commit từng bản sửa riêng với mô tả rõ, đẩy lên, và viết kết quả vào
   docs/WINDOWS-RESULTS.md: phiên bản Windows, số test đạt/hỏng trước và sau, từng đường
   thoát đã thử và kết quả, những gì không chạy được ở mức Low, và các lỗi đã sửa.
   Không chạy CI trên GitHub trừ khi cần (workflow chỉ chạy thủ công: gh workflow run ci).
```

## Đã kiểm trên GitHub Actions (Windows Server)

Lần chạy gần nhất: commit `57faf2c` (2026-11-14), cả ba job Linux, macOS và Windows đều đạt. Những gì thêm sau commit đó chưa được chạy trên Windows.

- Build cả workspace và toàn bộ test chạy được trên Windows.
- Ghi và sửa file trong thư mục dự án được, kể cả file có sẵn từ trước.
- Ghi ra ngoài bằng đường dẫn tuyệt đối hoặc `..` bị chặn; sửa file bên ngoài bị chặn.
- Lệnh chạy qua tool `bash` (Git Bash), kể cả tiến trình cháu, không ghi được ra ngoài.
- Chế độ `read-only` không ghi được gì.
- Không bật `allow_partial` thì zcode từ chối khởi động ở chế độ giam.
