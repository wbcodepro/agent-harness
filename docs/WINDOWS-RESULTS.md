# Kết quả kiểm chứng hame trên một máy Windows thật

Thực hiện ngày 2026-11-14 theo `docs/WINDOWS.md`, bằng Claude Code trên máy của người dùng.

## Máy

- Windows 11 Home Single Language 25H2 (10.0.26200), tài khoản thường (mức toàn vẹn Medium), ổ `C:` và `D:` đều NTFS.
- Rust 1.98.1 (msvc), có target `wasm32-unknown-unknown`.
- Git for Windows 2.53.0 cài ở `D:\Program Files\Git` (không nằm dưới `%ProgramFiles%`). `bash` đầu tiên trên PATH là trình khởi động WSL; `sh` không có trên PATH.
- Python 3.12.10 (chỉ có `python`, `python3` là stub của Store), Node và npm.
- Model thật: DeepSeek, model mặc định. Các kiểm tra cần kết quả lặp lại được thì phát lại phản hồi viết sẵn qua `HAME_SCRIPT`.

## Build và test

| | đạt | hỏng |
|---|---|---|
| Trước (commit `57c9ca4`) | 109 | 1 |
| Sau (commit `813bb3f`, đã gồm 10 commit mới của `origin/main`) | 120 | 0 |

Test hỏng lúc đầu: `commands_started_by_the_shell_tool_inherit_the_confinement`. Nguyên nhân là lỗi sản phẩm (lỗi 1 bên dưới), không phải test.

`cargo clippy --workspace --all-targets -- -D warnings` sạch.

Các test sandbox dùng bash của Git: nếu có một Git Bash đang mở trên máy lúc chạy test, chúng sẽ hỏng (xem lỗi 5).

## Các lỗi đã sửa

1. **Tool `bash` chạy nhầm WSL** (`f3b4be6`). Git cài ngoài `%ProgramFiles%` thì hame rơi về `bash` trên PATH, tức trình khởi động WSL: mọi lệnh trả về "Access is denied". Giờ tìm bash theo `git` trên PATH, bỏ qua các trình khởi động WSL; không tìm thấy thì báo cần cài gì. Hooks (`sh`) và `hame eval` (`bash`) dùng chung cách tìm này, nên hooks chạy được dù máy không có `sh` trên PATH.
2. **Checkpoint đi theo junction ra ngoài workspace** (`9cbc350`). Git trên Windows coi junction là thư mục thường: checkpoint chép cả nội dung phía sau junction, và `hame rewind` ghi đè, xóa file ở đó bằng quyền của harness (Medium), không phải quyền của tool. Giờ thư mục liên kết bị loại khỏi checkpoint; khi khôi phục, liên kết đứng ở chỗ checkpoint có thư mục thật thì chỉ liên kết bị gỡ.
3. **`hame eval` và `hame probe` không chạy được** (`e540cf6`). Mỗi tác vụ có thư mục dữ liệu riêng, không có `allow_partial`, nên mọi tác vụ dừng ngay. Giờ lựa chọn `allow_partial` của người dùng được mang sang. Lệnh kiểm tra gọi `python3` được chuyển sang `python` khi `python3` không chạy.
4. **Giao diện terminal bị in đè** (`f00cd9b`). Dòng "tools are confined for writes only" in ra stderr đè lên màn hình, ở mọi session và đè cả ô nhập sau `/new`, `/rewind`. Giờ nó là một dòng trong bản ghi hội thoại, nói một lần.
5. **Bash hỏng khi có Git Bash khác đang mở, không rõ lý do** (`d82b900`). Xem mục "Giới hạn mới phát hiện". Chưa sửa được tận gốc; tool giờ giải thích, và `HAME_BASH` cho phép dùng một bản Git riêng.
6. **Timeout và hủy không dừng được cây tiến trình** (`6c5a67a`). Việc kết thúc nhóm tiến trình chỉ có trên Unix. Trên Windows một lệnh quá thời hạn được báo là "killed" nhưng các tiến trình con vẫn chạy, và hame đợi chúng: một `pip install` treo làm lượt treo vô hạn. Hủy bằng Esc cũng để lại cây tiến trình. Giờ mỗi lần gọi tool chạy trong một job object; tiến trình cố ý để chạy nền vẫn sống sau lần gọi, như trên Unix.
7. **`npm install` và `pip install` không chạy trong sandbox** (`813bb3f`). Cả hai giữ cache dưới hồ sơ người dùng, nơi tiến trình Low không ghi được; pip còn treo ở bước tự kiểm tra phiên bản mới. Worker bị giam giờ nhận `npm_config_cache`, `PIP_CACHE_DIR` trong thư mục tạm của sandbox và `PIP_DISABLE_PIP_VERSION_CHECK=1`.

## Sandbox: chạy bằng model thật

Lệnh trong `docs/WINDOWS.md`, thư mục trống, `allow_partial = true`:

- `hello.py` được tạo và `python hello.py` in `xin chao`.
- `C:\Users\Public\hame-escape.txt` **không** được tạo. Model thử ba cách (chuyển hướng trong bash, `open()` của Python, tool `write`), cả ba bị từ chối quyền.
- Trước khi sửa lỗi 1, bước chạy Python không thực hiện được vì bash hỏng; việc chặn ghi vẫn đúng.

## Các đường thoát đã thử

Tất cả đều bị chặn. Trừ hai dòng cuối, chúng nằm trong test `no_route_leads_out_of_the_workspace` (`crates/hame-cli/tests/sandbox_windows.rs`).

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
| `hame rewind` qua junction (quyền của harness) | Đã lọt trước khi sửa lỗi 2; giờ bị chặn, có test |

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
| Thư mục tạm (`tempfile`, `os.tmpdir()`, `/tmp` trong bash) | Trỏ tới `AppData\LocalLow\hame-tmp`, ghi được |

## Những phần chưa từng kiểm trên Windows

- **Giao diện terminal.** Điều khiển qua một pseudo-console (ConPTY, cơ chế Windows Terminal dùng), không phải bằng tay trong cửa sổ Windows Terminal. Gõ yêu cầu, `/rewind` (file được khôi phục), `/plan` (không sửa gì), Esc (lượt bị hủy), `/quit` đều đúng. Lỗi in đè đã sửa (lỗi 4).
- **`hame web`.** Khởi động, in địa chỉ có token, trang tải được (HTTP 200). Chưa gửi yêu cầu qua giao diện web.
- **Checkpoint và `hame rewind`** trên repo git thật: file sửa được khôi phục, file mới bị xóa, `HEAD` và index của repo không đổi.
- **Nhãn Low vĩnh viễn.** `icacls` cho thấy thư mục dự án, file bên trong và `.git` mang nhãn Low. Git, PowerShell và cargo ở mức thường vẫn dùng thư mục bình thường.

Các phần thêm ngày 2026-11-14:

| Phần | Kết quả |
|---|---|
| `apply_patch` với file CRLF | Được; dòng sửa giữ CRLF. File mới tạo dùng LF |
| Bảo vệ `.git` | Sửa `.git/config` (ghi thẳng và qua `git config`) và thêm file vào `.git/hooks` (qua bash và tool `write`) đều bị hoàn tác, model được báo |
| `hame run --output json` và `stream-json` | Mỗi dòng stdout là JSON hợp lệ |
| Hooks kiểu Claude Code (`claude_hooks = ["project"]`) | Chạy được sau lỗi 1: hook nhận JSON qua stdin, mã thoát 2 chặn lời gọi. Hook `[[hook]]` của hame cũng vậy |
| Server MCP qua HTTP | Được, với một server thử viết bằng Python: initialize, liệt kê tool, gọi tool, header `Authorization` lấy từ biến môi trường |
| `hame probe evals/basic` | Sau lỗi 3: cả 4 cấu hình, 5/5 tác vụ đạt |
| `hame skill install` | Từ thư mục, từ URL git `file://` và từ một repo công khai trên GitHub đều được; skill có dấu hiệu nguy hiểm bị từ chối |
| Chốt đọc-trước-khi-ghi-đè | Ghi đè khi chưa đọc bị từ chối; đọc bằng `\` rồi ghi bằng `/` (và ngược lại) được |
| `grep`, `glob` với `path` là thư mục con | In đường dẫn tính từ thư mục làm việc, dùng `/`, kể cả khi `path` viết bằng `\` |

Chưa kiểm: server MCP stdio trong sandbox, `hame acp` trong một editor, subagent với `isolate`, `diagnostics` (LSP), plugin WebAssembly ngoài test có sẵn, `[approval] reviewer`.

## Giới hạn đã biết: xác nhận

Đúng như mô tả. Tool trong sandbox đọc được file ngoài dự án (có trong test) và ra mạng được (`curl`, `pip`, `npm`). hame từ chối khởi động ở chế độ giam khi chưa có `allow_partial = true`.

## Giới hạn mới phát hiện

- **Git Bash đang mở làm bash trong sandbox không chạy được.** Bash của Git (MSYS) giữ trạng thái dùng chung trong các đối tượng kernel đặt tên theo bản cài. Khi một bash mức Medium tạo chúng trước, bash mức Low bị từ chối và chết ngay với `NtCreateDirectoryObject ... 0xC0000022`. Đặt biến `CYGWIN_TESTING` hay chạy qua một junction đều không tách được. Một bản Git thứ hai ở thư mục khác thì không bị ảnh hưởng (đã kiểm bằng một bản chép của `usr\bin`), nên `HAME_BASH` là cách vòng.
- **Chiều ngược lại.** Nếu bash trong sandbox chạy trước, các đối tượng đó mang nhãn Low, và một Git Bash mở sau sẽ dùng chung vùng nhớ với tiến trình bị giam. Về lý thuyết một tool độc hại có thể dùng nó để tác động lên Git Bash của người dùng. Chưa thử khai thác; một bản Git riêng cho hame cũng loại bỏ điều này.
- **File chuyển vào dự án sau lần chạy đầu** giữ nhãn cũ, tool không sửa hay xóa được (đã tái hiện). hame chỉ gắn nhãn khi thư mục gốc chưa có nhãn. Cách sửa tay ghi trong README.
- **Cả `AppData\LocalLow` ghi được**, không chỉ `hame-tmp`: Windows gắn sẵn nhãn Low cho nó (theo `icacls`; chưa thử ghi vào thư mục của chương trình khác ở đó).

## Đề xuất: ẩn file bí mật và chặn mạng không cần quyền admin

Chưa triển khai và chưa thử; ghi lại hướng đi.

Chạy tool trong một **AppContainer** thay cho mức Low. Tạo AppContainer không cần quyền admin, và nó cho ba thứ còn thiếu:

- Đọc bị từ chối theo mặc định: tiến trình chỉ đọc được nơi có ACE cho SID của container hoặc cho `ALL APPLICATION PACKAGES`. `~/.ssh`, `.env` ngoài dự án, dữ liệu của hame tự động bị ẩn.
- Không có capability `internetClient` thì không ra mạng được; có thể chỉ cho phép loopback để đi qua proxy egress như trên macOS và Linux.
- Không gian tên đối tượng riêng, nên lỗi đụng độ với Git Bash ở trên cũng hết.

Cái giá, và lý do chưa làm: tool phải đọc được chính các chương trình nó chạy. `C:\Windows` và `C:\Program Files` có sẵn ACE cho `ALL APPLICATION PACKAGES`, nhưng Git ở `D:\Program Files\Git`, Python dưới `AppData\Local\Programs`, `~/.cargo`, `~/.rustup`, Node thì không. Phải thêm ACE ở những nơi đó, tức sửa ACL ngoài thư mục dự án, điều `docs/WINDOWS.md` yêu cầu không tự làm. Một bản thử nên bắt đầu bằng việc liệt kê chính xác những thư mục cần cấp quyền trên một máy thật rồi để người dùng quyết định.

Các hướng khác đã cân nhắc: restricted token không ẩn được file do chính người dùng sở hữu nếu không sửa ACL; luật Windows Firewall cần quyền admin; biến môi trường proxy chỉ có tác dụng với chương trình tự nguyện tuân theo.

# Lượt hai

Thực hiện ngày 2026-11-14 trên cùng máy, theo khối lệnh ở mục 3 của `docs/KIEM-THU-TAY.md`: các phần thêm sau lượt đầu. Các lần kiểm phát lại phản hồi viết sẵn qua `HAME_SCRIPT`, trừ chỗ ghi "model thật".

## Build và test

| | đạt | hỏng |
|---|---|---|
| Sau `git pull` (commit `f54f2e4`) | 127 | 0 |
| Sau các bản sửa của lượt này | 134 | 0 |

Bảy test thêm là các test client MCP, trước đây chỉ biên dịch trên Unix (xem lỗi 4). `cargo clippy --workspace --all-targets -- -D warnings` sạch.

## Các lỗi đã sửa

Mỗi lỗi một commit, tên commit ghi trong ngoặc.

1. **Tool `script` không chạy được** ("The script tool runs on Windows"). Tool lấy cái đầu tiên trong `python3`, `python` mà *khởi động* được. `python3` trên Windows là stub khởi động rồi không chạy gì, và dưới lớp bọc sandbox thì tên nào cũng "khởi động được", nên mọi script kết thúc bằng "Python may have failed to start it". Giờ mỗi ứng viên được yêu cầu chạy một chương trình rỗng, một lần.
2. **`[secrets] substitute` bị từ chối hẳn** ("Secrets can be substituted on Windows"). hame không khởi động, báo cần chế độ allowlist trong khi đang ở đúng chế độ đó: trên Windows proxy không bao giờ được bật vì không ép được tool đi qua. Giờ proxy được bật khi có secret cần thay; nó được *mời*, không bị *ép* (xem bảng bên dưới), hame nói rõ điều đó lúc khởi động và prompt hệ thống không còn nói mạng bị giới hạn. Git for Windows được chuyển sang backend OpenSSL cho tool bị giam, vì backend mặc định (Schannel) không đọc file chứng chỉ của proxy.
3. **`fetch` của Node bỏ qua proxy** ("Node's fetch goes through the egress proxy"). Không riêng Windows: thiếu `NODE_USE_ENV_PROXY`, `fetch` không nhìn biến proxy, nên secret được gửi đi dưới dạng giá trị giả. Worker sau proxy giờ được đặt biến này.
4. **Test client MCP không chạy trên Windows** ("The MCP client tests run on Windows too"). Chúng bị giới hạn cho Unix chỉ vì hai tên chương trình, `python3` và `true`.
5. **Server MCP stdio và language server không khởi động** ("MCP and language servers start on Windows"). Chúng chỉ được thừa hưởng biến môi trường tên đúng là `PATH`, `HOME`…; Windows đặt tên là `Path`, nên khi hame chạy từ PowerShell hay cmd, server không có đường tìm chương trình và không có `SystemRoot` (thiếu nó Python không khởi động): mọi server MCP stdio báo "stopped responding". Dưới cargo và Python tên biến viết hoa nên test và kịch bản phát lại không thấy lỗi; nó lộ ra ở lần chạy bằng model thật. Giờ cả hai dùng chung bộ lọc của worker, không phân biệt hoa thường.

## Từng mục

| Mục | Kết quả |
|---|---|
| 1. Lệnh từ file (`.hame\commands\tong-ket.md`, `hame run "/tong-ket README.md"`) | Đạt. Prompt ghi trong log là "Tóm tắt README.md trong ba dòng." Tên lệnh không tồn tại được gửi nguyên văn |
| 2. Loại subagent (`.hame\agents\kiem-tra.md`, `tools: Read, Grep`) | Đạt, cả với model thật: session con chỉ khai báo `read` và `grep` |
| 3. `hame plugin install` | Đạt: từ thư mục, từ URL git `file://`, từ marketplace với `--plugin`; marketplace thiếu `--plugin` thì liệt kê các plugin có. `plugin list` đúng; `plugin update` (một plugin và tất cả) lấy được file mới từ thư mục và từ commit mới của kho git |
| 4. Tool `script` với server MCP stdio viết bằng Python, `defer_tools_over = 0` | Hỏng lúc đầu (lỗi 1 và 5). Sau khi sửa: script gọi tool hai lần trong một bước, chạy bằng `python` 3.12, ghi được trong dự án, ghi ra ngoài bị từ chối. Chính server MCP cũng chạy trong sandbox: nó tạo được file trong dự án, không tạo được file bên ngoài |
| 5. Thay secret tại proxy | Hỏng lúc đầu (lỗi 2). Sau khi sửa: xem bảng dưới |
| 6. Cảnh báo cú pháp | Đạt: sửa `.py` cho sai thụt lề thì kết quả `edit` có "the file no longer parses (…line 3…)"; sửa hợp lệ thì không có cảnh báo |
| 7. MCP không trạng thái `2026-07-28` và `hame mcp login` | Sau lỗi 4, cả 7 test trong `mcp.rs` đạt, gồm bản không trạng thái qua stdio và qua HTTP. 4 test của `mcp_auth.rs` (máy chủ cấp quyền giả lập) đạt; test thứ năm cần server thật và bị bỏ qua theo thiết kế. Chưa đăng nhập với nhà cung cấp thật trên máy này |

### Thay secret tại proxy trên Windows

Cấu hình: `[network] mode = "allowlist"`, `allow = ["httpbin.org", "example.com"]` (hoặc `github.com`), `substitute = { DEMO_TOKEN = ["httpbin.org"] }`.

| Kiểm tra | Kết quả |
|---|---|
| `echo $DEMO_TOKEN` trong sandbox | Giá trị giả `hame-secret-DEMO_TOKEN-…` |
| `HTTPS_PROXY`, `SSL_CERT_FILE`, `CURL_CA_BUNDLE`, `NODE_EXTRA_CA_CERTS` | Được đặt; proxy thật sự nhận kết nối |
| Python (`urllib`) tới `httpbin.org` | httpbin trả về header có secret thật, đầu ra bị che thành `[secret:DEMO_TOKEN]` |
| Node (`fetch`) tới `httpbin.org` | Trước lỗi 3: đi thẳng, gửi giá trị giả. Sau: như Python |
| git tới `httpbin.org` và tới `github.com` | Trước lỗi 2: `SEC_E_UNTRUSTED_ROOT`. Sau: "SSL certificate verified via OpenSSL"; `git ls-remote` một repo GitHub chạy được |
| curl của Git for Windows tới `httpbin.org` | **Không được**: bản này chỉ có Schannel, bỏ qua `CURL_CA_BUNDLE`, từ chối chứng chỉ của proxy. `CURL_SSL_BACKEND=openssl` không đổi được gì |
| curl tới host khác trong allowlist (`example.com`, HTTPS) | Được, 200 |
| curl qua proxy tới host ngoài allowlist | Proxy trả 403 |
| curl `--noproxy "*"` tới host ngoài allowlist | **Được, 200**: proxy không bị ép. Chương trình đi vòng chỉ cầm giá trị giả |

Điều cần nhớ: trên Windows tool đọc được mọi file bạn đọc được. Secret chỉ thật sự nằm ngoài tầm tool khi nó chỉ có trong biến môi trường của hame; nếu nó nằm trong một file `.env`, tool đọc được file đó.

## Chưa kiểm

- Tool `diagnostics`: máy không có language server nào (`rust-analyzer` chỉ là proxy của rustup, chưa cài component). Lỗi 5 sửa cùng một lỗi ở phần khởi động language server, nhưng chưa có lần chạy nào xác nhận. (Đã kiểm ở lượt ba, xem bên dưới.)
- `hame mcp login` với nhà cung cấp thật, `hame acp` trong một editor: cần người làm theo `docs/KIEM-THU-TAY.md`.
- Các mục còn lại của lượt đầu: subagent với `isolate`, `[approval] reviewer`, `hame web` qua giao diện, ổ không phải NTFS.

# Lượt ba

Thực hiện ngày 2026-11-14 trên cùng máy, theo khối lệnh "Lượt ba" trong `docs/WINDOWS.md`, sau khi dự án đổi tên từ zcode thành hame. Các lần kiểm phát lại phản hồi viết sẵn qua `HAME_SCRIPT` trên binary thật, trừ chỗ ghi "model thật". Để kiểm mục 8, máy được cài thêm component `rust-analyzer` của rustup.

## Build và test

| | đạt | hỏng |
|---|---|---|
| Sau `git pull` (commit `fdb2916`), `cargo build --release -p hame-cli` đạt | 166 | 0 |
| Sau các bản sửa của lượt này | 171 | 0 |

Một test bị bỏ qua theo thiết kế (cần server OAuth thật). Năm test thêm là mô-đun test LSP (server giả và cách viết URI), trước đây chỉ chạy trên Unix (xem lỗi 2). `cargo clippy --workspace --all-targets -- -D warnings` sạch.

## Các lỗi đã sửa

Mỗi lỗi một commit, tên commit ghi trong ngoặc.

1. **Language server không dùng được trên Windows** ("Language servers work on Windows"). Với rust-analyzer thật, `diagnostics` luôn hết giờ chờ: báo cáo được lưu theo URI server gửi (`file:///c:/...`) và tìm theo URI hame tự viết (`file:///C%3A/...`), hai cách viết của cùng một file không bao giờ khớp. Giờ báo cáo được lưu theo file. Ký tự ổ đĩa server gửi chữ thường được đổi thành chữ hoa như đường dẫn trên máy, để `references` và `rename_symbol` so và hiển thị đúng. Thư mục làm việc được giữ ở dạng `C:\...`: ở dạng `\\?\C:\...` (dạng `canonicalize` trả về) nó tới server thành một URI không chỉ tới file nào. Đường dẫn trong kết quả các tool này dùng `/`, như `grep` và `glob`.
2. **Test LSP chỉ chạy trên Unix** (cùng commit). Server giả viết URI kiểu Unix (`"file://" + path`); giờ nó dùng dạng Windows khi chạy trên Windows, và cả mười test đạt trên máy này.
3. **`references` hỏng ngay sau khi server khởi động** ("Retry a language server request answered with content modified"). rust-analyzer trả lỗi `-32801 content modified` trong lúc còn đọc dự án; giao thức nói client nên gửi lại, hame thì đưa thẳng lỗi cho model. Giờ yêu cầu được gửi lại tối đa năm lần, mỗi lần đợi lâu hơn. Không riêng Windows.
4. **`edit_lines` báo một neo cũ hai lần** ("edit_lines reports a stale one-line anchor once"). Thiếu `to` thì `from` được dùng cho cả hai đầu và bị kiểm hai lần. Không riêng Windows.

## Từng mục

| Mục | Kết quả |
|---|---|
| 1. Đổi tên | Đạt. `hame --version` in `hame 0.0.1`. Khi chỉ có `%USERPROFILE%\.zcode`, hame dùng nó và in "hame: using C:\Users\ADMIN\.zcode, the data directory from when this was zcode. Rename it to C:\Users\ADMIN\.hame when convenient."; thư mục đã được đổi tên thành `.hame` và dòng nhắc không còn, session và cấu hình cũ vẫn đọc được. Không có cấu hình editor nào trên máy trỏ tới `zcode.exe` (đã tìm trong thư mục cấu hình của Zed, JetBrains và VS Code) |
| 2. pipefail | Đạt. `(echo boom; exit 3) \| tail -n 5` là lỗi, có "exit code 3" và chữ "pipefail". `seq 1 200000 \| head -n 2` và `yes \| head -n 1` không báo lỗi. `grep … \| wc -l` không khớp gì thì báo lỗi kèm giải thích grep thoát 1 khi không có dòng khớp |
| 3. Ghi file nguyên khối | Đạt. Giữ file bằng `CreateFileW` theo ba cách. Chia sẻ đọc+ghi+xóa: đổi tên đè thành công. Chia sẻ đọc+ghi nhưng không cho xóa (cách nhiều editor và trình xem giữ file): đổi tên bị từ chối, hame ghi thẳng vào file, thành công. Chỉ chia sẻ đọc (như Excel): `write` và `edit` đều báo "The process cannot access the file because it is being used by another process. (os error 32)", nội dung cũ còn nguyên. Không còn sót file `.tmp` nào. Notepad và VS Code không giữ file mở nên không gây ra trường hợp này |
| 4. then_run | Đạt. Kết quả `edit` có "[then_run] $ python test_calc.py" và đầu ra `ok`. Lệnh bị luật `deny` chặn thì không chạy ("Denied by the user's permission rules"), lệnh ghi ra ngoài dự án bị sandbox chặn ("Permission denied"); cả hai trường hợp đều ghi rõ là thay đổi đã được áp dụng còn lệnh sau đó thất bại |
| 5. Lọc đầu ra | Đạt. `cargo test` với 60 test: kết quả có "[60 lines omitted: passing tests or build progress]", giữ cả hai dòng "test result:", và ghi đường dẫn bản đầy đủ (trong `<home>\spill\`); tool `read` đọc được file đó |
| 6. evidence | Đạt. Có sửa mà không chạy gì: "evidence: none; code was changed and nothing was run afterwards"; chạy được: "evidence: passed after the last change: python m.py"; hỏng: "evidence: FAILING; …". `--output json` có `evidence` (`unchecked`, `passed`, `failing`) và `evidence_command` |
| 7. hashline | Đạt. `read` in `1:d39\|def f():`. `edit_lines` sửa file CRLF và file vẫn là CRLF (`b'def f():\r\n    return 2\r\n\r\nprint(f())\r\n'`). Sửa file bằng tay rồi dùng neo cũ: bị từ chối, "Nothing was written … it has changed since you read it", file không đổi. Dòng ngắn (dòng trống) mang hash của dòng đầy đủ phía trên: đúng thiết kế |
| 8. Tool LSP | Hỏng lúc đầu (lỗi 1 và 3), đạt sau khi sửa. Với rust-analyzer thật chạy trong sandbox (mặc định), trên một dự án hai file: `diagnostics` báo "No problems found", rồi sau một sửa đổi báo `src/main.rs:7:24: error: cannot find value …`; `references` liệt kê 4 chỗ dùng trong 2 file; `rename_symbol` đổi `total` thành `sum_all` với 4 sửa đổi trong 2 file và `cargo build` vẫn đạt. Đã thử gửi URI cả hai dạng `file:///C%3A/...` và `file:///C:/...` thẳng tới rust-analyzer: nó nhận cả hai, nên chỗ hỏng là ở chiều trả về |

## Chưa kiểm

- Một editor thật giữ file trong lúc hame ghi: mục 3 được mô phỏng bằng cách giữ file qua `CreateFileW`, không phải bằng một editor cụ thể.
- Language server khác rust-analyzer (pyright, typescript-language-server…): máy không có.
- `hame acp` trong editor và `hame mcp login` với nhà cung cấp thật: vẫn cần người làm theo `docs/KIEM-THU-TAY.md`.