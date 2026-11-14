# Kết quả kiểm chứng zcode trên một máy Windows thật

Thực hiện ngày 2026-11-14 theo `docs/WINDOWS.md`, bằng Claude Code trên máy của người dùng.

## Máy

- Windows 11 Home Single Language 25H2 (10.0.26200), tài khoản thường (mức toàn vẹn Medium), ổ `C:` và `D:` đều NTFS.
- Rust 1.98.1 (msvc), có target `wasm32-unknown-unknown`.
- Git for Windows 2.53.0 cài ở `D:\Program Files\Git` (không nằm dưới `%ProgramFiles%`). `bash` đầu tiên trên PATH là trình khởi động WSL; `sh` không có trên PATH.
- Python 3.12.10 (chỉ có `python`, `python3` là stub của Store), Node và npm.
- Model thật: DeepSeek, model mặc định. Các kiểm tra cần kết quả lặp lại được thì phát lại phản hồi viết sẵn qua `ZCODE_SCRIPT`.

## Build và test

| | đạt | hỏng |
|---|---|---|
| Trước (commit `57c9ca4`) | 109 | 1 |
| Sau (commit `813bb3f`, đã gồm 10 commit mới của `origin/main`) | 120 | 0 |

Test hỏng lúc đầu: `commands_started_by_the_shell_tool_inherit_the_confinement`. Nguyên nhân là lỗi sản phẩm (lỗi 1 bên dưới), không phải test.

`cargo clippy --workspace --all-targets -- -D warnings` sạch.

Các test sandbox dùng bash của Git: nếu có một Git Bash đang mở trên máy lúc chạy test, chúng sẽ hỏng (xem lỗi 5).

## Các lỗi đã sửa

1. **Tool `bash` chạy nhầm WSL** (`f3b4be6`). Git cài ngoài `%ProgramFiles%` thì zcode rơi về `bash` trên PATH, tức trình khởi động WSL: mọi lệnh trả về "Access is denied". Giờ tìm bash theo `git` trên PATH, bỏ qua các trình khởi động WSL; không tìm thấy thì báo cần cài gì. Hooks (`sh`) và `zcode eval` (`bash`) dùng chung cách tìm này, nên hooks chạy được dù máy không có `sh` trên PATH.
2. **Checkpoint đi theo junction ra ngoài workspace** (`9cbc350`). Git trên Windows coi junction là thư mục thường: checkpoint chép cả nội dung phía sau junction, và `zcode rewind` ghi đè, xóa file ở đó bằng quyền của harness (Medium), không phải quyền của tool. Giờ thư mục liên kết bị loại khỏi checkpoint; khi khôi phục, liên kết đứng ở chỗ checkpoint có thư mục thật thì chỉ liên kết bị gỡ.
3. **`zcode eval` và `zcode probe` không chạy được** (`e540cf6`). Mỗi tác vụ có thư mục dữ liệu riêng, không có `allow_partial`, nên mọi tác vụ dừng ngay. Giờ lựa chọn `allow_partial` của người dùng được mang sang. Lệnh kiểm tra gọi `python3` được chuyển sang `python` khi `python3` không chạy.
4. **Giao diện terminal bị in đè** (`f00cd9b`). Dòng "tools are confined for writes only" in ra stderr đè lên màn hình, ở mọi session và đè cả ô nhập sau `/new`, `/rewind`. Giờ nó là một dòng trong bản ghi hội thoại, nói một lần.
5. **Bash hỏng khi có Git Bash khác đang mở, không rõ lý do** (`d82b900`). Xem mục "Giới hạn mới phát hiện". Chưa sửa được tận gốc; tool giờ giải thích, và `ZCODE_BASH` cho phép dùng một bản Git riêng.
6. **Timeout và hủy không dừng được cây tiến trình** (`6c5a67a`). Việc kết thúc nhóm tiến trình chỉ có trên Unix. Trên Windows một lệnh quá thời hạn được báo là "killed" nhưng các tiến trình con vẫn chạy, và zcode đợi chúng: một `pip install` treo làm lượt treo vô hạn. Hủy bằng Esc cũng để lại cây tiến trình. Giờ mỗi lần gọi tool chạy trong một job object; tiến trình cố ý để chạy nền vẫn sống sau lần gọi, như trên Unix.
7. **`npm install` và `pip install` không chạy trong sandbox** (`813bb3f`). Cả hai giữ cache dưới hồ sơ người dùng, nơi tiến trình Low không ghi được; pip còn treo ở bước tự kiểm tra phiên bản mới. Worker bị giam giờ nhận `npm_config_cache`, `PIP_CACHE_DIR` trong thư mục tạm của sandbox và `PIP_DISABLE_PIP_VERSION_CHECK=1`.

## Sandbox: chạy bằng model thật

Lệnh trong `docs/WINDOWS.md`, thư mục trống, `allow_partial = true`:

- `hello.py` được tạo và `python hello.py` in `xin chao`.
- `C:\Users\Public\zcode-escape.txt` **không** được tạo. Model thử ba cách (chuyển hướng trong bash, `open()` của Python, tool `write`), cả ba bị từ chối quyền.
- Trước khi sửa lỗi 1, bước chạy Python không thực hiện được vì bash hỏng; việc chặn ghi vẫn đúng.

## Các đường thoát đã thử

Tất cả đều bị chặn. Trừ hai dòng cuối, chúng nằm trong test `no_route_leads_out_of_the_workspace` (`crates/zcode-cli/tests/sandbox_windows.rs`).

| Đường | Kết quả |
|---|---|
| Tool `write`, `edit` với đường dẫn tuyệt đối ra ngoài | Access is denied |
| `..` qua tool `write` và qua bash | Bị chặn |
| Junction có sẵn trong workspace trỏ ra ngoài: ghi, nối thêm, xóa qua nó | Bị chặn; việc gắn nhãn workspace không lan sang đích của junction |
| Tool tự tạo junction ra ngoài (`mklink /J`, `New-Item -ItemType Junction`) | Windows từ chối tiến trình Low tạo junction tới nơi nó không ghi được |
| Symlink thư mục (`mklink /D`) | Tài khoản không có quyền tạo symlink; chưa kiểm được trên máy bật Developer Mode |
| Hard link tới file bên ngoài (`mklink /H`) | Access is denied |
| PowerShell `Set-Content`, `Add-Content` | Bị chặn |
| `cmd /c echo > ...`, `>>` | Bị chặn |
| Xóa và đổi tên file bên ngoài (`rm`, `mv`, `del`, `ren`), chuyển file bên ngoài vào workspace | Bị chặn |
| Tiến trình cháu (bash trong bash) | Bị chặn |
| `zcode rewind` qua junction (quyền của harness) | Đã lọt trước khi sửa lỗi 2; giờ bị chặn, có test |

Chưa thử: ổ không phải NTFS (FAT32, exFAT) và thư mục mạng. Nhãn toàn vẹn không tồn tại ở đó, nên nhiều khả năng tool ghi được; cần kiểm trước khi tin.

## Công cụ thường gặp ở mức Low

Chạy qua tool `bash` trong một repo git thật:

| Lệnh | Kết quả |
|---|---|
| `git status`, `git add`, `git commit` | Được |
| `cargo build`, `cargo run`, kể cả có một dependency từ crates.io | Được (dependency có thể đã nằm sẵn trong cache của cargo; tải mới chưa được kiểm riêng) |
| `npm run`, `node` | Được |
| `npm install` | Hỏng với EPERM ở cache trước lỗi 7; giờ được |
| `python`, `python -m venv`, `pip install` trong venv hoặc `--target` trong dự án | Được sau lỗi 7 |
| `pip install` vào Python toàn cục | pip báo `site-packages` không ghi được (đúng ý đồ) và chuyển sang cài cho người dùng, cũng nằm ngoài dự án; chỉ thử với gói đã có sẵn, chưa thử cài gói mới theo cách này |
| `python3` | Không có trên Windows; model tự chuyển sang `python` |
| `curl https://example.com` | Được: mạng không bị giới hạn |
| Thư mục tạm (`tempfile`, `os.tmpdir()`, `/tmp` trong bash) | Trỏ tới `AppData\LocalLow\zcode-tmp`, ghi được |

## Những phần chưa từng kiểm trên Windows

- **Giao diện terminal.** Điều khiển qua một pseudo-console (ConPTY, cơ chế Windows Terminal dùng), không phải bằng tay trong cửa sổ Windows Terminal. Gõ yêu cầu, `/rewind` (file được khôi phục), `/plan` (không sửa gì), Esc (lượt bị hủy), `/quit` đều đúng. Lỗi in đè đã sửa (lỗi 4).
- **`zcode web`.** Khởi động, in địa chỉ có token, trang tải được (HTTP 200). Chưa gửi yêu cầu qua giao diện web.
- **Checkpoint và `zcode rewind`** trên repo git thật: file sửa được khôi phục, file mới bị xóa, `HEAD` và index của repo không đổi.
- **Nhãn Low vĩnh viễn.** `icacls` cho thấy thư mục dự án, file bên trong và `.git` mang nhãn Low. Git, PowerShell và cargo ở mức thường vẫn dùng thư mục bình thường.

Các phần thêm ngày 2026-11-14:

| Phần | Kết quả |
|---|---|
| `apply_patch` với file CRLF | Được; dòng sửa giữ CRLF. File mới tạo dùng LF |
| Bảo vệ `.git` | Sửa `.git/config` (ghi thẳng và qua `git config`) và thêm file vào `.git/hooks` (qua bash và tool `write`) đều bị hoàn tác, model được báo |
| `zcode run --output json` và `stream-json` | Mỗi dòng stdout là JSON hợp lệ |
| Hooks kiểu Claude Code (`claude_hooks = ["project"]`) | Chạy được sau lỗi 1: hook nhận JSON qua stdin, mã thoát 2 chặn lời gọi. Hook `[[hook]]` của zcode cũng vậy |
| Server MCP qua HTTP | Được, với một server thử viết bằng Python: initialize, liệt kê tool, gọi tool, header `Authorization` lấy từ biến môi trường |
| `zcode probe evals/basic` | Sau lỗi 3: cả 4 cấu hình, 5/5 tác vụ đạt |
| `zcode skill install` | Từ thư mục, từ URL git `file://` và từ một repo công khai trên GitHub đều được; skill có dấu hiệu nguy hiểm bị từ chối |
| Chốt đọc-trước-khi-ghi-đè | Ghi đè khi chưa đọc bị từ chối; đọc bằng `\` rồi ghi bằng `/` (và ngược lại) được |
| `grep`, `glob` với `path` là thư mục con | In đường dẫn tính từ thư mục làm việc, dùng `/`, kể cả khi `path` viết bằng `\` |

Chưa kiểm: server MCP stdio trong sandbox, `zcode acp` trong một editor, subagent với `isolate`, `diagnostics` (LSP), plugin WebAssembly ngoài test có sẵn, `[approval] reviewer`.

## Giới hạn đã biết: xác nhận

Đúng như mô tả. Tool trong sandbox đọc được file ngoài dự án (có trong test) và ra mạng được (`curl`, `pip`, `npm`). zcode từ chối khởi động ở chế độ giam khi chưa có `allow_partial = true`.

## Giới hạn mới phát hiện

- **Git Bash đang mở làm bash trong sandbox không chạy được.** Bash của Git (MSYS) giữ trạng thái dùng chung trong các đối tượng kernel đặt tên theo bản cài. Khi một bash mức Medium tạo chúng trước, bash mức Low bị từ chối và chết ngay với `NtCreateDirectoryObject ... 0xC0000022`. Đặt biến `CYGWIN_TESTING` hay chạy qua một junction đều không tách được. Một bản Git thứ hai ở thư mục khác thì không bị ảnh hưởng (đã kiểm bằng một bản chép của `usr\bin`), nên `ZCODE_BASH` là cách vòng.
- **Chiều ngược lại.** Nếu bash trong sandbox chạy trước, các đối tượng đó mang nhãn Low, và một Git Bash mở sau sẽ dùng chung vùng nhớ với tiến trình bị giam. Về lý thuyết một tool độc hại có thể dùng nó để tác động lên Git Bash của người dùng. Chưa thử khai thác; một bản Git riêng cho zcode cũng loại bỏ điều này.
- **File chuyển vào dự án sau lần chạy đầu** giữ nhãn cũ, tool không sửa hay xóa được (đã tái hiện). zcode chỉ gắn nhãn khi thư mục gốc chưa có nhãn. Cách sửa tay ghi trong README.
- **Cả `AppData\LocalLow` ghi được**, không chỉ `zcode-tmp`: Windows gắn sẵn nhãn Low cho nó (theo `icacls`; chưa thử ghi vào thư mục của chương trình khác ở đó).

## Đề xuất: ẩn file bí mật và chặn mạng không cần quyền admin

Chưa triển khai và chưa thử; ghi lại hướng đi.

Chạy tool trong một **AppContainer** thay cho mức Low. Tạo AppContainer không cần quyền admin, và nó cho ba thứ còn thiếu:

- Đọc bị từ chối theo mặc định: tiến trình chỉ đọc được nơi có ACE cho SID của container hoặc cho `ALL APPLICATION PACKAGES`. `~/.ssh`, `.env` ngoài dự án, dữ liệu của zcode tự động bị ẩn.
- Không có capability `internetClient` thì không ra mạng được; có thể chỉ cho phép loopback để đi qua proxy egress như trên macOS và Linux.
- Không gian tên đối tượng riêng, nên lỗi đụng độ với Git Bash ở trên cũng hết.

Cái giá, và lý do chưa làm: tool phải đọc được chính các chương trình nó chạy. `C:\Windows` và `C:\Program Files` có sẵn ACE cho `ALL APPLICATION PACKAGES`, nhưng Git ở `D:\Program Files\Git`, Python dưới `AppData\Local\Programs`, `~/.cargo`, `~/.rustup`, Node thì không. Phải thêm ACE ở những nơi đó, tức sửa ACL ngoài thư mục dự án, điều `docs/WINDOWS.md` yêu cầu không tự làm. Một bản thử nên bắt đầu bằng việc liệt kê chính xác những thư mục cần cấp quyền trên một máy thật rồi để người dùng quyết định.

Các hướng khác đã cân nhắc: restricted token không ẩn được file do chính người dùng sở hữu nếu không sửa ACL; luật Windows Firewall cần quyền admin; biến môi trường proxy chỉ có tác dụng với chương trình tự nguyện tuân theo.
