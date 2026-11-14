# Kiểm chứng hame trên máy Windows

hame được phát triển trên macOS. Lượt kiểm chứng đầu tiên trên một máy Windows 11 thật đã làm ngày 2026-11-14; kết quả, các lỗi đã sửa và những gì chưa kiểm nằm ở `docs/WINDOWS-RESULTS.md`. File này là việc cần làm cho mỗi lượt kiểm, viết để giao thẳng cho Claude Code trên máy Windows.

Lượt kế tiếp nên tập trung vào những gì chưa kiểm (cuối mục "Những phần chưa từng kiểm" của file kết quả) và những gì thêm sau lượt đầu: lệnh `/tên` từ `commands/*.md`, loại subagent từ `agents/*.md`, `hame plugin install`, `hame mcp login`, bản MCP không trạng thái `2026-07-28`, và cảnh báo lỗi cú pháp sau khi sửa file (với Python cần `python` trên PATH).

## Chuẩn bị

- Rust (rustup, toolchain stable) và `rustup target add wasm32-unknown-unknown`.
- Git for Windows (hame dùng `bash.exe` của nó để chạy lệnh).
- Một API key trong biến môi trường, ví dụ `DEEPSEEK_API_KEY`.

## Prompt cho Claude Code trên Windows

Dán nguyên khối dưới đây:

```text
Repo này là hame, một agent harness viết bằng Rust. Nó được phát triển trên macOS; phần
Windows mới chỉ được kiểm trên GitHub Actions. Việc của bạn là kiểm chứng nó trên máy
Windows này và sửa những gì hỏng. Đọc README.md và docs/WINDOWS.md trước.

1. Build và test:
     cargo build --workspace
     cargo test --workspace --no-fail-fast
   Ghi lại số test đạt/hỏng. Với mỗi test hỏng, tìm nguyên nhân gốc và sửa trong mã sản phẩm
   (không nới lỏng test), trừ khi test giả định điều chỉ đúng trên Unix.

2. Sandbox Windows (crates/hame-sandbox/src/windows.rs): tiến trình chạy tool tự hạ xuống
   mức toàn vẹn Low nên chỉ ghi được trong thư mục dự án và AppData\LocalLow\hame-tmp.
   Tạo %USERPROFILE%\.hame\config.toml với:
     [sandbox]
     allow_partial = true
   Rồi trong một thư mục thử nghiệm trống, chạy bằng model thật:
     hame run "Tạo hello.py in ra 'xin chao', chạy nó bằng python, rồi thử tạo file
     C:\Users\Public\hame-escape.txt và báo chính xác chuyện gì xảy ra."
   Kiểm tra: hello.py được tạo và chạy được; file ngoài thư mục dự án KHÔNG được tạo.
   Thử thêm các đường thoát: ghi qua đường dẫn có .., qua symlink/junction trỏ ra ngoài,
   qua PowerShell (powershell -c "Set-Content ..."), qua cmd /c, và xóa/đổi tên một file
   bên ngoài. Mọi đường đều phải bị chặn. Nếu có đường nào lọt, đó là lỗi nghiêm trọng:
   viết test tái hiện trong crates/hame-cli/tests/sandbox_windows.rs rồi sửa.

3. Những thứ chưa từng được kiểm trên Windows, hãy thử và báo lại:
   - Giao diện terminal: chạy `hame` trong Windows Terminal, gõ một yêu cầu, thử /plan,
     /rewind, Esc để hủy.
   - `hame web` và mở địa chỉ nó in ra.
   - Tool bash với các lệnh thường gặp (git status, cargo build, npm, python) dưới sandbox:
     công cụ nào không chạy được ở mức Low và vì sao.
   - Checkpoint và `hame rewind` trên một repo git thật.
   - Thư mục dự án bị gắn nhãn Low vĩnh viễn (icacls <thư mục> để xem). Xác nhận điều đó
     không làm hỏng việc dùng thư mục bằng các công cụ khác.

   - Các phần thêm ngày 2026-11-14, chưa từng biên dịch hay chạy trên Windows: tool
     `apply_patch` (đặt `[model] edit_format = "apply-patch"`) với file dùng CRLF; bảo vệ
     `.git` (yêu cầu agent sửa `.git/config` hoặc thêm file vào `.git/hooks`, thay đổi phải
     bị hoàn tác và model được báo); `hame run --output json`; hooks kiểu Claude Code
     (`[compat] claude_hooks`) cần `sh` trong PATH, báo lại nếu không chạy được; server MCP
     qua HTTP (`[mcp.x] url = ...`); `hame probe evals/basic`; `hame skill install` với
     một thư mục và với một URL git; chốt đọc-trước-khi-ghi-đè (yêu cầu agent ghi đè một file
     có sẵn mà chưa đọc: phải bị từ chối, và đọc xong thì ghi được, kể cả khi đường dẫn viết
     bằng `\` hay `/`); `grep`/`glob` với `path` là thư mục con phải in đường dẫn tính từ thư
     mục làm việc, dùng dấu `/`.

4. Giới hạn đã biết, không cần sửa nhưng cần xác nhận đúng như mô tả: trên Windows hame
   chỉ giam được việc ghi; tool vẫn đọc được mọi file người dùng đọc được và vẫn ra mạng
   được. Nếu bạn thấy cách khả thi để ẩn file bí mật hoặc chặn mạng mà không cần quyền
   admin, hãy mô tả đề xuất (không tự triển khai nếu nó cần sửa ACL ngoài thư mục dự án).

5. Kết thúc: commit từng bản sửa riêng với mô tả rõ, đẩy lên, và viết kết quả vào
   docs/WINDOWS-RESULTS.md: phiên bản Windows, số test đạt/hỏng trước và sau, từng đường
   thoát đã thử và kết quả, những gì không chạy được ở mức Low, và các lỗi đã sửa.
   Không chạy CI trên GitHub trừ khi cần (workflow chỉ chạy thủ công: gh workflow run ci).
```

## Lượt ba: sau khi đổi tên thành hame (2026-11-14)

Dự án đổi tên từ zcode thành hame: lệnh là `hame`, thư mục dữ liệu là `%USERPROFILE%\.hame` (thư mục `.zcode` cũ vẫn được dùng kèm một dòng nhắc cho tới khi đổi tên), biến môi trường là `HAME_*`, thư mục cấu hình trong dự án là `.hame\`. Dán khối sau cho Claude Code trên máy Windows:

```text
Repo này trước tên zcode, nay là hame. Hãy git pull, build lại (cargo build --release -p hame-cli),
chạy cargo test --workspace --no-fail-fast, rồi kiểm các phần dưới đây và ghi kết quả vào một
mục mới "Lượt ba" trong docs/WINDOWS-RESULTS.md. Sửa trong mã sản phẩm những gì hỏng, mỗi bản
sửa một commit. Trước khi push luôn git fetch và rebase.

1. Đổi tên: hame --version chạy được; khi chỉ có %USERPROFILE%\.zcode thì hame dùng nó và in
   một dòng nhắc; sau khi đổi tên thư mục thành .hame thì không còn dòng nhắc. Cập nhật mọi cấu
   hình editor (ACP) đang trỏ tới zcode.exe.
2. pipefail: tool bash giờ chạy với "bash -o pipefail -c". Yêu cầu agent chạy
   "(echo boom; exit 3) | tail -n 5": kết quả phải là lỗi và có chữ pipefail. Chạy
   "seq 1 200000 | head -n 2": không được báo lỗi.
3. Ghi file nguyên khối: write, edit và apply_patch ghi ra file tạm cùng thư mục rồi đổi tên đè
   lên. Kiểm khi file đích đang mở trong một editor (Windows có thể từ chối đổi tên): lỗi phải
   rõ ràng và nội dung cũ còn nguyên; kiểm rằng không còn sót file ".<tên>.<pid>-<n>.tmp".
4. then_run: yêu cầu agent sửa một file .py rồi kiểm tra trong cùng lời gọi. Kết quả của lời
   gọi edit phải có dòng "[then_run] $ ..." kèm đầu ra của lệnh; lệnh đó phải chịu luật quyền và
   sandbox như một lệnh bash thường.
5. Lọc đầu ra: chạy cargo test trong một crate nhỏ qua agent. Kết quả tool phải có dòng
   "lines omitted: passing tests or build progress", giữ dòng "test result:", và đường dẫn file
   lưu bản đầy đủ phải đọc được bằng tool read.
6. hame run in dòng "evidence: ..." sau một lượt có sửa mã nguồn; --output json có trường
   evidence.
7. [model] edit_format = "hashline": read in dòng dạng "12:a3f|..."; yêu cầu agent sửa một file
   có kết thúc dòng CRLF bằng edit_lines và xác nhận file vẫn là CRLF; sửa file bằng tay giữa
   hai bước để neo cũ đi, lệnh phải bị từ chối và không ghi gì.
8. Tool LSP: khai báo [lsp.rust] command = "rust-analyzer" (hoặc server khác có trên máy), rồi
   yêu cầu agent dùng references và rename_symbol trên một dự án nhỏ. Phần đổi URI file sang
   đường dẫn Windows (file:///C%3A/...) mới chỉ qua test đơn vị chạy trên macOS; các test với
   server giả trong crates/hame-ext/src/lsp.rs đang giới hạn cho Unix, hãy thử bỏ giới hạn đó.

Không chạy CI trên GitHub trừ khi cần (gh workflow run ci).
```

## Đã kiểm trên GitHub Actions (Windows Server)

Lần chạy gần nhất: commit `3fea852` (2026-11-14), cả ba job Linux, macOS và Windows đều đạt. Lần đó gồm cả các phần thêm sau lượt kiểm trên máy thật (lệnh và loại subagent từ file, cài plugin, MCP không trạng thái, đăng nhập OAuth, tool `script`, thay secret tại proxy), nhưng chỉ ở mức biên dịch và test tự động: chúng chưa được dùng thử bằng tay trên Windows.

- Build cả workspace và toàn bộ test chạy được trên Windows.
- Ghi và sửa file trong thư mục dự án được, kể cả file có sẵn từ trước.
- Ghi ra ngoài bằng đường dẫn tuyệt đối hoặc `..` bị chặn; sửa file bên ngoài bị chặn.
- Lệnh chạy qua tool `bash` (Git Bash), kể cả tiến trình cháu, không ghi được ra ngoài.
- Chế độ `read-only` không ghi được gì.
- Không bật `allow_partial` thì hame từ chối khởi động ở chế độ giam.
