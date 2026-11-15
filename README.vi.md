# hame

[English](README.md) · Tiếng Việt

`hame` là một agent harness viết bằng Rust: một binary duy nhất chạy mô hình ngôn ngữ như một agent trong terminal, trong editor (ACP) hoặc trong trình duyệt. Ban đầu nó là agent lập trình; bản 0.1 thêm các tác vụ mà hame tự kiểm chứng kết quả, và các tool điều khiển trình duyệt web và ứng dụng macOS.

Một tác vụ chạy theo trình tự **kết quả → tool → kiểm chứng → bằng chứng**:

1. **Kết quả.** Bạn nêu điều phải đúng khi xong việc (một file tác vụ: kết quả cần đạt, điều kiện nghiệm thu, phạm vi được cấp quyền, ngân sách).
2. **Tool.** Agent làm việc với file và shell, và nếu bạn cấp quyền thì với trình duyệt và ứng dụng desktop. Nó được dặn ưu tiên file, lệnh và API, rồi đến dữ liệu cấu trúc của trang và của accessibility, ảnh chụp màn hình sau cùng.
3. **Kiểm chứng.** hame tự kiểm các điều kiện nghiệm thu trên trạng thái thật (file, JSON, CSV, mã thoát của lệnh, trang đang mở, cửa sổ ứng dụng), không dựa vào lời báo cáo của agent. Điều kiện nào chưa đạt thì agent bị gửi lại làm tiếp, tối đa hai lần.
4. **Bằng chứng.** Báo cáo kiểm chứng, biên nhận cho mọi thao tác trên ứng dụng, log session, ảnh chụp và file tải về được lưu lại, và tùy chọn một trace của mọi lần gọi model và gọi tool. `hame task` chỉ thoát với mã 0 khi kết quả đã được kiểm chứng.

Ba ý chính của thiết kế:

- **Log là nguồn sự thật.** Mỗi session là một file JSONL chỉ ghi thêm. Mọi thứ model nhìn thấy (prompt hệ thống, danh sách tool, tin nhắn) đều được dựng lại từ log, nên resume, fork, rewind, nén ngữ cảnh và phát lại đều dùng chung một cơ chế.
- **Tool không chạy trong tiến trình của harness.** Mỗi lần gọi tool chạy trong một tiến trình worker bị hệ điều hành giam (Seatbelt trên macOS, Landlock trên Linux). Không giam được thì từ chối chạy.
- **Mở rộng mà không nới quyền.** Plugin là WebAssembly và chỉ đọc được nơi bạn cấp; server MCP chạy trong cùng sandbox với tool.

## Cài đặt

hame được build từ mã nguồn. Cần Rust 1.89 trở lên và `git`. Tool trình duyệt cần thêm Python 3 có Playwright và Google Chrome; tool desktop cần macOS 14 trở lên và trình biên dịch Swift. Các bước theo từng nền tảng và tính năng ở [docs/release/INSTALL.md](docs/release/INSTALL.md), những gì đã được kiểm ở đâu ở [docs/release/SUPPORT-MATRIX.md](docs/release/SUPPORT-MATRIX.md) (tiếng Anh).

```sh
cargo install --locked --path crates/hame-cli   # cài vào ~/.cargo/bin/hame
cargo build --release -p hame-cli               # hoặc chỉ build target/release/hame
```

Rồi chạy `hame setup`: chọn nhà cung cấp, dán API key (không hiện lên màn hình, lưu vào `~/.hame/.env` chỉ bạn đọc được), và hame tự kiểm tra key, sandbox, `bash`, `git`, kèm một request thử tốn vài token. `hame doctor` chạy lại các bước kiểm tra đó bất cứ lúc nào (`--ping` để gửi request thử).

Cũng có thể tự đặt key vào biến môi trường hoặc file `~/.hame/.env` (hoặc `.env` trong thư mục làm việc):

```sh
DEEPSEEK_API_KEY=...     # hoặc ANTHROPIC_API_KEY / CLAUDE_API_KEY, OPENAI_API_KEY
```

| Nhà cung cấp | Biến chứa key | Model mặc định |
|---|---|---|
| `deepseek` | `DEEPSEEK_API_KEY` | `deepseek-flash` |
| `anthropic` | `ANTHROPIC_API_KEY` hoặc `CLAUDE_API_KEY` | `claude-opus-5-5` |
| `openai` | `OPENAI_API_KEY` | không có: truyền `--model` hoặc đặt `[model] name` |
| máy chủ tương thích (`provider = "openai"` cùng `base_url`) | `HAME_API_KEY` | không có |

`[model] api_key_env = "TÊN"` đọc key từ một biến khác. Khi không có `--provider`, `HAME_PROVIDER` hay `[model] provider`, hame lấy nhà cung cấp đầu tiên trong DeepSeek, Anthropic, OpenAI mà nó tìm thấy key. Thư mục dữ liệu là `~/.hame`; đổi bằng `--home` hoặc `HAME_HOME`.

## Dùng

```sh
hame setup                            # chọn nhà cung cấp, lưu key, kiểm tra mọi thứ
hame doctor [--ping]                  # kiểm tra cấu hình, key, sandbox, chương trình cần thiết, trình duyệt và desktop
hame                                  # giao diện terminal
hame run "sửa lỗi làm test_stats.py thất bại"     # một lượt, không tương tác
hame task task.toml [--json]          # làm tới khi các điều kiện của file tác vụ đạt; chỉ thoát 0 khi đã kiểm chứng
hame run --trace session "..."        # ghi thêm trace các lần gọi model và tool cạnh log session
hame workflow list | show <tên> | trust <tên> | untrust <tên>   # workflow đã lưu và việc tin dùng chúng
hame sessions gc [--older-than-hours 24] [--dry-run]   # xóa ảnh và file đã lưu mà không session nào còn dùng
hame run --session <id> "tiếp tục"    # nối tiếp một session
hame run --plan "..."                 # chỉ khảo sát và đề xuất kế hoạch, không sửa gì
hame run --output json "..."          # một đối tượng JSON khi xong (stream-json: thêm từng sự kiện một dòng)
hame web                              # giao diện web trên 127.0.0.1
hame acp                              # máy chủ Agent Client Protocol cho editor
hame sessions | show <id> | fork <id> | rewind <id> | recover <id>
hame import dsh <session.jsonl>       # nhập session của DeepSeek Harness (v2–v4)
hame eval evals/harder                # chạy bộ tác vụ, báo tỷ lệ đạt, thời gian, token (--keep <thư mục>: giữ log của tác vụ hỏng; --runs N: chạy N lần, báo tỷ lệ đạt từng tác vụ)
hame eval evals/harder --command 'codex exec "$HAME_EVAL_PROMPT"'   # đo một agent khác trên cùng bộ
hame eval evals/expert                # mười tác vụ khó hơn, có bộ chấm ẩn (Python, Rust, Go, Node)
hame skill install <thư mục | URL git> [--path thư/mục/con]   # quét rồi cài một skill vào ~/.hame/skills
hame skill list
hame plugin install <thư mục | URL git> [--path thư/mục/con]  # quét rồi cài một plugin kiểu Claude Code vào ~/.hame/plugins
hame plugin install <marketplace> --plugin <tên>               # cài một plugin từ danh mục của một marketplace
hame plugin update [tên]                                       # lấy lại từ nguồn, quét lại, thay bản đã cài
hame plugin list
hame mcp login <server> | logout <server>   # đăng nhập OAuth vào một server MCP qua HTTP
hame probe evals/harder --model <tên> # thử model với từng định dạng sửa file và bộ tool, in cấu hình tốt nhất
```

Chọn model bằng `--provider deepseek|openai|anthropic` và `--model <tên>`, hoặc `HAME_PROVIDER` / `HAME_MODEL`. Mặc định: `deepseek-flash` với DeepSeek, `claude-opus-5-5` với Anthropic; OpenAI cần chỉ rõ model.

Cuối mỗi lần `hame run`, một dòng tổng kết in ra stderr: số token gửi đi, bao nhiêu phần trăm được đọc từ cache của nhà cung cấp, số token nhận về và số request. Nếu phần lớn prompt không được dùng lại giữa các request, hame cảnh báo.

Trong giao diện terminal: `/plan`, `/rewind`, `/new`, `/quit`; Esc dừng lượt đang chạy. Gõ rồi Enter trong lúc một lượt đang chạy sẽ gửi lời nhắn đó cho chính lượt ấy: model nhận nó ở request kế tiếp, không phải đợi lượt kết thúc (qua ACP là thông báo mở rộng `_hame/steer`).

### Trong editor (ACP)

`hame acp` nói Agent Client Protocol qua stdin/stdout.

**IntelliJ IDEA** (AI Assistant): tạo `~/.jetbrains/acp.json` khai báo agent, rồi khởi động lại IDE và chọn `hame (local ACP)` trong AI Chat:

```json
{ "command": "/đường/dẫn/tới/hame", "args": ["acp"], "env": {} }
```

Đã kiểm trong IntelliJ IDEA 2026.1 ngày 2026-11-14, cả năm việc đều đạt: câu trả lời hiện dần và lời gọi tool hiện thành mục riêng; tạo file; dừng giữa chừng rồi hỏi tiếp; mở lại hội thoại cũ; IDE hỏi xin phép trước khi chạy lệnh shell ở chế độ `danger-full-access`.

**Zed:** thêm vào `~/.config/zed/settings.json`, rồi chọn `hame` trong menu External Agents:

```json
{ "agent_servers": { "hame": { "type": "custom", "command": "/đường/dẫn/tới/hame", "args": ["acp"] } } }
```

Đã kiểm trong Zed 1.28.1 ngày 2026-11-14, cũng năm việc đó đều đạt (dừng giữa chừng được kiểm bằng phím Esc; nút Stop bấm bằng chuột chưa xác nhận được). Thiếu `"type": "custom"` thì Zed không nhận agent.

Máy chủ cũng được kiểm thử tự động bằng runtime client chính thức của giao thức (`agent-client-protocol`), gồm cả luồng xin phép.

## Tác vụ có kết quả được kiểm chứng

`hame task FILE` chạy một tác vụ mô tả trong file TOML, rồi tự kiểm các điều kiện nghiệm thu:

```toml
# task.toml
id = "ledger-summary"                       # chữ, số, - và _
outcome = "Xuất sổ cái từ 10 đến 20/01/2026 từ ứng dụng cục bộ và ghi tổng vào out/summary.txt"
constraints = ["Dùng hồ sơ trình duyệt 'work'", "Không sửa file nào khác"]

[budget]
max_steps = 40              # số request tới model
max_tokens = 400000         # token vào + ra của agent và các subagent
deadline_ms = 600000        # thời gian thực

[[acceptance]]              # mọi điều kiện đều phải đạt
kind = "csv_shape"
path = "out/ledger.csv"
rows = 11
columns = ["date", "description", "amount"]

[[acceptance]]
kind = "file_contains"
path = "out/summary.txt"
text = "Tổng: 1.234,50"

[[authorization.grants]]    # agent được làm gì, ở đâu
channel = "browser"
origin = "http://127.0.0.1:*"
profile = "work"
capabilities = ["observe", "interact", "file_write"]
```

```sh
hame task task.toml                          # tiến trình in ra stderr; cuối cùng là kết luận và bằng chứng
hame task task.toml --json                   # một đối tượng JSON: trạng thái, verified, từng điều kiện kèm bằng chứng, biên nhận, token, thời gian
hame task task.toml --trace session          # ghi thêm trace.v1.jsonl cạnh log session
hame task task.toml --cwd ~/work/project --provider anthropic --model claude-haiku-5-5 --yes
```

Các loại điều kiện (`kind`): `file_exists`, `file_absent` (`path`); `file_contains` (`path`, `text`, khớp chính xác, giữ nguyên Unicode); `file_sha256` (`path`, `sha256`); `json_value` (`path`, `pointer` theo RFC 6901, `equals`); `csv_shape` (`path`, `rows` không tính dòng tiêu đề, `columns`); `command` (`command`, `exit`, mặc định 0; chạy bằng `sh -c` trong thư mục làm việc, trong cùng sandbox với các tool của agent, tắt mạng, chỉ có các biến môi trường cơ bản (không có thông tin xác thực), giới hạn 120 giây: agent có thể sửa những file mà lệnh đó chạy, như một bài test hay Makefile, nên bước kiểm không được nhiều quyền hơn agent; với `--sandbox danger-full-access` nó chạy không bị giam, như mọi thứ khác); `browser_state` (`url_contains`, `text_contains`, `profile`; đọc trang đang mở); `desktop_state` (`app` là bundle id, `window_title_contains`, `value_contains`; đọc ứng dụng đang chạy). Đường dẫn tính từ thư mục làm việc. File tác vụ không có điều kiện nghiệm thu bị từ chối, vì không thể kiểm chứng nó đã xong. Cũng bị từ chối: điều kiện không kiểm gì cả (`text`, `command` hay `path` rỗng; `browser_state` không có `url_contains` lẫn `text_contains`; `desktop_state` không có `window_title_contains` lẫn `value_contains`) và mọi trường viết sai hay không biết, trong điều kiện, quyền và ngân sách: `text_contians` là lỗi, không phải một điều kiện luôn đạt, và `orgin` là lỗi, không phải quyền cho mọi origin. `browser_state` có `profile` sẽ không đạt khi trình duyệt không xác nhận được profile. Tham số workflow chèn vào điều kiện `command` được quote cho shell, mỗi tham số là một từ.

Agent được cho biết các điều kiện để nhắm tới; hame tự đánh giá chúng sau đó trên trạng thái thật. Khi agent kết thúc lượt mà còn điều kiện chưa đạt, nó bị gửi lại cùng bằng chứng của điều kiện hỏng, tối đa hai lần. `hame task` thoát 0 khi mọi điều kiện đạt, 1 khi có điều kiện không đạt (hoặc lượt bị lỗi), 130 khi bị ngắt. Log ghi `task/start` (hợp đồng tác vụ), `task/verified` (báo cáo) và các biên nhận thao tác. `max_steps`, `max_tokens`, `deadline_ms` và `max_cost_usd` được kiểm trước mỗi request tới model. `max_tokens` tính cả các subagent mà tác vụ khởi chạy (nó giới hạn ngân sách chung `[limits] tree_tokens`). Chi phí được tính theo bảng giá có ghi ngày, chỉ gồm các model Anthropic; khi đặt ngân sách chi phí mà giá của model không rõ, tác vụ dừng lại thay vì chạy không đo được. Chi tiêu của subagent được đếm bằng token nhưng không quy ra tiền, nên tác vụ có `max_cost_usd` không được dùng tool subagent. Request thất bại mà provider không báo usage thì không được tính.

Quyền cấp trong file tác vụ được cộng thêm vào quyền trong `config.toml`, chỉ cho lần chạy đó, và bật tool trình duyệt hay desktop khi chúng nêu kênh tương ứng.

## Điều khiển trình duyệt và desktop

Hai tool chạy phía host, ngoài sandbox của shell: `browser` (Chrome qua Playwright; mới kiểm trên macOS arm64, xem [SUPPORT-MATRIX](docs/release/SUPPORT-MATRIX.md)) và `desktop` (ứng dụng macOS qua giao diện accessibility). Cả hai đều tắt cho tới khi cấu hình bật lên hoặc có quyền cấp nêu kênh của chúng, và mỗi thao tác được đối chiếu với quyền cấp trước khi gửi đi.

```toml
[computer]
browser = true                  # cung cấp tool browser
desktop = true                  # cung cấp tool desktop (macOS 14+)
headless = true                 # trình duyệt không có cửa sổ (mặc định)
browser_channel = "chrome"      # chrome (Google Chrome đã cài, mặc định) | msedge | chromium
# batch = true                  # hành động batch của công cụ (mặc định bật; chỉ tắt để đo)
# observation = "semantic"      # semantic (mặc định) | screenshot (cần grant "screen"; để đo chi phí thị giác)
# profiles_dir = "/đường/dẫn"   # hồ sơ trình duyệt lâu dài; mặc định ~/.hame/browser-profiles

[[computer.grant]]              # đọc và dùng trang của máy chủ cục bộ, chỉ hồ sơ "work"
channel = "browser"
origin = "http://127.0.0.1:*"
profile = "work"
capabilities = ["observe", "interact"]

[[computer.grant]]              # TextEdit: đọc, bấm, gõ, lưu file qua hộp thoại Save
channel = "desktop"
app = "com.apple.TextEdit"
capabilities = ["observe", "interact", "global_input", "commit", "file_write"]

[[computer.grant]]              # chụp cửa sổ TextEdit và bấm vào điểm ảnh của ảnh chụp (không bấm Save, Delete, ...)
channel = "visual"
app = "com.apple.TextEdit"
capabilities = ["screen", "global_input"]
```

Một quyền cấp có `channel` (`browser`, `desktop`, hoặc `visual` cho ảnh chụp và cú bấm theo điểm ảnh của cửa sổ ứng dụng), các mẫu tùy chọn `app` (bundle id), `origin` và `profile` (`*` khớp mọi chuỗi ký tự, không phân biệt hoa thường; bỏ trống là khớp tất cả), và `capabilities`:

| Năng lực | Cho phép |
|---|---|
| `observe` | đọc trang, cửa sổ và cây accessibility; chờ; trích dữ liệu |
| `interact` | bấm, điền, chọn, nhấn phím trên phần tử, đặt giá trị, mục menu chỉ mở thêm thứ gì đó |
| `commit` | thao tác gửi, lưu, xóa, thanh toán, mua (nhận ra theo tên nút, phương thức của form, hoặc model khai `commit: true`), bằng bất kỳ đường nào: trên desktop còn gồm Return hoặc Space khi nó bấm một nút, cmd+s, cmd+w, cmd+q, cmd+d, cmd+delete, ký tự xuống dòng gõ vào bất cứ đâu ngoài vùng văn bản của một trình soạn tài liệu (TextEdit, Pages, Numbers, Keynote, Notes, Word; ở ứng dụng khác như Messages hay Slack, Return có thể gửi tin), và cú bấm theo điểm ảnh vào một nút như vậy hoặc vào thứ hame không nhận ra được |
| `file_write` | tải file về workspace (xem bên dưới); xác nhận hộp thoại lưu hoặc xuất file (nút Save hay Export, Return hoặc ký tự xuống dòng gõ trong hộp thoại, cú bấm vào Save) và cmd+s. Trên desktop quyền này cấp theo ứng dụng, không theo thư mục: ứng dụng lưu vào nơi hộp thoại đang trỏ tới, có thể là bất kỳ thư mục nào bạn ghi được |
| `clipboard` | đọc và đặt clipboard (desktop; nội dung cũ được khôi phục), và cmd+c, cmd+x, cmd+v |
| `global_input` | sự kiện bàn phím, tổ hợp phím và cú bấm theo điểm ảnh; cần cửa sổ đích ở trên cùng và giữ một khóa liên tiến trình. Riêng nó chỉ đủ cho việc gõ chữ và các phím không commit; xem `commit` |
| `screen` | ảnh chụp màn hình (lưu thành artifact) |

Mô hình an toàn:

- **Quyền chỉ đến từ bạn**: `[computer]` trong `~/.hame/config.toml`, hoặc file tác vụ bạn đưa cho `hame task`. `.hame/config.toml` của dự án không cấp được quyền (nó chỉ được đặt `[verify]`).
- **Quyền điều khiển desktop và trình duyệt tách biệt với sandbox của shell và không bị sandbox giam.** Các helper chạy dưới tài khoản của bạn, với quyền riêng tư macOS (Accessibility, Screen Recording) của terminal và cookie trong hồ sơ trình duyệt. Seatbelt, Landlock và danh sách mạng cho phép áp dụng cho `bash` và các tool bị giam khác, không cho hai tool này. Hãy cấp hẹp: mẫu app hoặc origin, và chỉ những năng lực tác vụ cần.
- Helper trình duyệt chặn điều hướng cấp trang tới origin không có quyền, trước khi request rời máy. Mỗi lần chuyển hướng được kiểm hai lần: `Location` được phân giải theo cách Chrome phân giải (dấu gạch ngược tính như gạch chéo, nên `/\evil.example/` dẫn tới evil.example; đích nào không phân giải chắc chắn được thì bị từ chối), rồi request mà Chrome tạo ra sau đó được kiểm tại chính URL Chrome đã tính. Các request của chính trang (script, ảnh, gọi API) không bị lọc, và trình duyệt không đi qua proxy của hame.
- File tải về chỉ được lưu trong workspace và không bao giờ đi qua symlink: đích là một symlink (dù trỏ đi đâu), hoặc đường dẫn thư mục đi ra ngoài qua một symlink, đều bị từ chối. File có sẵn chỉ bị thay khi lời gọi có `overwrite: true`, và chỉ khi nó là file thường; nội dung được ghi vào một file tạm mới rồi đổi tên vào chỗ, nên không bao giờ ghi vào một file có sẵn, một symlink hay một hard link khác của nó. Khi đường dẫn là thư mục, tên file trang đề nghị được rút về một tên file đơn (không dấu phân cách, `..`, dấu chấm đầu tên, ký tự điều khiển hay tên mà Windows dành riêng; tối đa 120 byte). Upload chỉ đọc file thường trong workspace, không qua symlink và không có hard link khác, và trình duyệt nhận một bản sao riêng.
- Mọi thao tác có tác động đều được ghi nhật ký (`not_started`, `dispatched`, rồi `completed`, `failed` hoặc `outcome_unknown`) trước và sau khi gửi. Thao tác commit mang khóa idempotency; một thao tác có thể đã có hiệu lực sẽ không được lặp lại cho tới khi trang hay cửa sổ được quan sát lại và agent nêu thao tác trước đó (`reconciled`). Sau crash, các thao tác đã gửi đi thành `outcome_unknown` và agent được dặn kiểm tra trạng thái; hame không bao giờ tự làm lại chúng.
- Ref (`e12`, `o3:e7`) thuộc về một lần quan sát. Ref mà trang đã chuyển đi, phần tử đã đổi hoặc bị xóa, hay cửa sổ đã đóng đều bị từ chối (`stale_ref`, `stale_window`), hai đích cùng khớp bị từ chối là `ambiguous`; tool không bao giờ tự nhắm sang một phần tử "giống giống". Nhập liệu vật lý dừng khi focus chuyển đi (`interference`) hoặc có hộp thoại bật lên (`unexpected_dialog`) và báo chính xác đã gõ được gì.
- Việc nhận ra commit là heuristic theo tên nút và phương thức form: một nút gửi dữ liệu bằng JavaScript có thể bị tính là `interact`. Đừng chỉ dựa vào việc không cấp `commit` để bảo vệ một trang mà một cú bấm có thể gây hại. Trên desktop, nhập liệu vật lý được phân loại theo nơi đang có focus bàn phím (hoặc thứ nằm dưới điểm ảnh được bấm), đọc từ ứng dụng ngay trước khi gửi; helper từ chối gửi nếu điều đó đã đổi trong lúc ấy, và khi không đọc được thì cần bộ năng lực chặt hơn. Gõ chữ bình thường vào một ô, và Return trong vùng văn bản của tài liệu, chỉ cần `global_input`.

Hồ sơ trình duyệt là thư mục lâu dài (`~/.hame/browser-profiles/<tên>`), nên hồ sơ đã đăng nhập sẽ giữ đăng nhập; hãy coi nó như hồ sơ trình duyệt của chính bạn. File tải về nằm trong workspace, theo các luật ở trên. Helper desktop là một chương trình Swift được biên dịch ở lần dùng đầu vào `~/.hame/helpers/` (khoảng 4 giây). Chi tiết từng tool: `crates/hame-browser`, [crates/hame-desktop/README.md](crates/hame-desktop/README.md).

## Workflow

Workflow là một chuỗi lời gọi tool đã lưu, có tham số, có kiểm tra trước và sau, cho việc đã từng làm. File đặt ở `.hame/workflows/TÊN.toml` trong dự án, hoặc `~/.hame/workflows/TÊN.toml`; bản của dự án được ưu tiên. Khi có workflow, agent có thêm tool `workflow` liệt kê chúng. Ví dụ đầy đủ (định dạng file) ở [README tiếng Anh](README.md#workflows).

Workflow chỉ chạy nếu bạn đã tin dùng đúng file đó: `hame workflow show TÊN` in ra nó cần gì, kiểm gì và làm gì, và `hame workflow trust TÊN` ghi SHA-256 của nó vào `~/.hame/trusted-workflows.json` sau khi bạn xác nhận (trong script thì dùng `--yes`). Sửa file là mất tin dùng; không thể tin dùng từ bên trong một session (tool của agent không ghi được thư mục dữ liệu của hame). Mỗi bước vẫn đi qua luật quyền, quyền cấp, hỏi phép và nhật ký thao tác như lời gọi của model, và nhắm phần tử theo vai trò, nhãn hoặc selector, không bao giờ theo tọa độ hay ref đã ghi. Điều kiện trước không đạt (ứng dụng đã khác đi) thì không bước nào chạy; một lần chạy dừng ở bước hỏng đầu tiên và báo phần đã xong; tham số không khớp mẫu bị từ chối trước khi chạy.

## Tool có sẵn

`read`, `write`, `edit`, `bash`, `job_output`, `glob`, `grep`, `repo_map`, `subagent`, `task_create` / `task_update` / `task_list`; thêm `skill` khi có skill; khi có cấu hình language server thì thêm `diagnostics`, `references` và `rename_symbol`. Có `browser` và `desktop` khi được [bật hoặc cấp quyền](#điều-khiển-trình-duyệt-và-desktop); có `workflow` khi có [workflow](#workflow). Với `edit_format = "apply-patch"`, tool `apply_patch` thay cho `edit` và `write`.

`write` từ chối ghi đè một file đã tồn tại mà model chưa đọc trong phiên (nó phải đọc trước, vì nội dung file thường là thứ đề bài phụ thuộc vào); khi ghi làm file nhỏ đi quá nửa, kết quả nói rõ là cả file đã bị thay. `edit` không tìm thấy đoạn cần thay thì chỉ ra khớp tới dòng nào và khác ở đâu. `grep` ghi kèm tên hàm hoặc lớp bao quanh mỗi dòng khớp. Sau mỗi lần `edit`, `write` hay `apply_patch`, nếu file nguồn đang hợp lệ trở nên sai cú pháp thì kết quả báo ngay dòng lỗi (Python được kiểm bằng `python3` khi có; Rust, JS, TS, Go bằng tree-sitter).

Lệnh `bash` chưa xong khi hết `timeout_ms` (mặc định 120 giây) không bị giết mà chuyển thành tác vụ nền, vẫn trong sandbox: kết quả trả ngay phần đầu ra đã có cùng mã tác vụ, và model gọi `job_output` để chờ tiếp và đọc phần còn lại, hoặc dừng nó bằng `kill: true`. Build hay bộ test dài vì thế không phải chạy lại từ đầu. Tác vụ bị dừng khi hame thoát, hoặc sau một giờ. Lệnh test còn đang chạy nền không được tính là bằng chứng đã qua: lớp kiểm tra hoàn thành nhắc model chờ kết quả trước khi kết thúc.

Gọi model: khi nhà cung cấp trả về `Retry-After`, hame chờ đúng thời gian đó (tối đa 60 giây, lâu hơn thì báo lỗi ngay thay vì chờ im lặng). Khi request bị từ chối vì vượt cửa sổ ngữ cảnh, hame nén lịch sử ngay (giữ nguyên khoảng một phần tư cuối) rồi gửi lại một lần.

`references` và `rename_symbol` hỏi thẳng language server thay cho một chuỗi `grep` rồi `edit`: model chỉ ra symbol bằng file, số dòng và tên (không phải đếm cột). `references` liệt kê mọi nơi dùng symbol dưới dạng `path:line: nội dung dòng`, tối đa 100 dòng. `rename_symbol` đổi tên ở mọi file trong một lần gọi và trả về số chỗ sửa theo từng file; mọi chỗ sửa được tính xong trước khi ghi, và nếu có chỗ nằm ngoài thư mục làm việc hoặc server đòi đổi tên, tạo hay xoá file thì không file nào bị đổi. Khi không có server cho loại file đó, hoặc server không hỗ trợ, kết quả bảo model quay về `grep` và `edit`.

`edit` và `apply_patch` chấp nhận sai lệch về khoảng trắng, thụt lề và kiểu xuống dòng khi chỉ có đúng một chỗ khớp; dòng không đổi luôn giữ nguyên như trong file. Tham số gọi tool bị bọc trong code fence hoặc lẫn chữ được gỡ ra trước khi chạy.

Khi tool từ server MCP và plugin nhiều hơn `defer_tools_over`, chúng không nằm trong danh sách tool gửi cho model (mỗi tool khai báo tốn token ở mọi request). Model tìm bằng `tool_search` và chạy bằng `tool_call`; luật quyền và hook vẫn thấy tên thật của tool được gọi. Khi đó còn có tool `script`: model viết một đoạn Python gọi các tool ấy như hàm (`call(tên, **tham_số)`), nên một chuỗi lời gọi chỉ tốn một bước và chỉ những gì đoạn mã in ra mới vào ngữ cảnh. Đoạn mã chạy trong sandbox như lệnh shell; từng lời gọi bên trong vẫn qua luật quyền và hook; đoạn mã nhận kết quả đầy đủ của từng lời gọi, chỉ phần nó in ra mới chịu giới hạn `spill_at_bytes`. Cần `python3` (hoặc `python`); tắt bằng `[context] scripts = false`.

Các lời gọi chỉ đọc mà model đưa ra cùng lúc được chạy song song. `subagent` giao việc cho một agent mới có ngữ cảnh riêng; với `isolate` nó làm trên một git worktree và nhánh riêng.

## An toàn

Mặc định (`workspace-write`):

- Chỉ ghi được trong thư mục làm việc và thư mục tạm.
- Không đọc được `~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.config/gh`…, dữ liệu riêng của hame, và các file `.env`.
- Mạng chỉ đi qua proxy cục bộ tới danh sách tên miền cho phép.
- Biến môi trường của harness không tới được tool; secret chỉ tới khi bạn cấp (`[secrets] grant`), và giá trị của mọi secret bị thay bằng `[secret:TÊN]` trong đầu ra tool trước khi ghi log.
- Trước mỗi lượt và sau mỗi bước có thay đổi, workspace được lưu vào một kho git riêng ngoài dự án; `hame rewind` khôi phục file và tạo session mới dừng ngay trước lượt đó.
- Thay đổi mà một lần gọi tool gây ra cho `.git/hooks` và `.git/config` của repo ở thư mục làm việc bị hoàn tác ngay sau lần gọi đó và model được báo (`[sandbox] protect_git = false` để tắt). Đây là phát hiện rồi hoàn tác, không phải ngăn chặn: hook vẫn tồn tại trong lúc lệnh đang chạy. Repo do chính lần gọi đó tạo ra (`git init`) không được bảo vệ.
- Luật quyền cho `bash` được so với từng lệnh trong dòng (các phần ngăn bởi `;`, `&&`, `||`, `|`, phần trong `$(...)` và backtick, lệnh sau `sudo`/`env`…): luật `deny`/`ask` có hiệu lực dù lệnh nằm ở đâu trong dòng, còn luật `allow` chỉ áp dụng khi mọi lệnh trong dòng đều được cho phép. Bộ tách lệnh không phải là một trình phân tích shell đầy đủ.
- Với `[approval] reviewer = true`, một model thứ hai xem từng lời gọi cần xin phép (chỉ thấy tên tool, tham số và thư mục làm việc, không thấy hội thoại) và chỉ tự đồng ý khi lời gọi rõ ràng vô hại; mọi trường hợp khác, kể cả khi nó trả lời không rõ, vẫn được hỏi bạn. Mọi phán quyết được ghi vào `~/.hame/approvals.jsonl`. Đây là một model, có thể phán sai: nó giảm số lần bị hỏi, không thay cho sandbox.

- Với `[secrets] substitute`, secret không vào sandbox: biến môi trường của tool chứa một giá trị giả, và proxy mạng thay nó bằng secret thật trong phần đầu của request (dòng request và header, kể cả `Authorization: Basic`) gửi tới đúng các host bạn nêu. Với HTTPS, proxy phải đọc được request nên nó tự kết thúc kết nối TLS của tool cho riêng các host đó, bằng một chứng chỉ gốc sinh ra cho lần chạy này, rồi mở kết nối TLS đã xác thực của nó tới host thật; các host khác vẫn được chuyển tiếp nguyên vẹn. Tool được trỏ tới chứng chỉ gốc ấy qua `SSL_CERT_FILE`, `CURL_CA_BUNDLE`, `REQUESTS_CA_BUNDLE`, `GIT_SSL_CAINFO`, `PIP_CERT`, `CARGO_HTTP_CAINFO` và `NODE_EXTRA_CA_CERTS`; chương trình không đọc các biến này sẽ từ chối kết nối tới host đó. Thân request cũng được thay khi nó khai báo `Content-Length` và không quá 1 MiB (độ dài được sửa lại cho khớp). Giới hạn: thân gửi kiểu chunked hoặc lớn hơn thì đi qua nguyên vẹn; với HTTP không mã hóa chỉ phần đầu được thay; chỉ HTTP/1.1; mỗi kết nối một request; một host bạn nêu mà lại gửi trả header thì secret sẽ quay về đầu ra (và bị che như mọi secret). Danh sách chứng chỉ trao cho tool được lấy từ kho chứng chỉ của hệ điều hành, cộng thêm chứng chỉ của proxy.

`--sandbox read-only` cấm ghi; `--sandbox danger-full-access` bỏ giam, khi đó mọi tool không chỉ-đọc đều phải được bạn đồng ý, trừ `browser` và `desktop` (xem dưới).

Tool `browser` và `desktop` là ngoại lệ của nguyên tắc "tool không chạy trong tiến trình của harness": chúng thao tác trên hồ sơ trình duyệt và ứng dụng của bạn từ ngoài sandbox, trong phạm vi quyền bạn cấp, như mô tả ở [Điều khiển trình duyệt và desktop](#điều-khiển-trình-duyệt-và-desktop). Chúng tắt nếu không được cấu hình hay cấp quyền. Với `[approval] default = "ask"` (mặc định), từng lệnh gọi của chúng không bị hỏi riêng, vì quyền đã cấp quyết định mỗi hành động được làm gì; với `default = "deny"` hoặc `"allow"`, chúng theo mặc định đó như mọi tool khác, và một `[[approval.rule]]` cho `browser` hay `desktop` vẫn áp dụng.

Giới hạn cần biết:

- Thư mục tạm ghi được theo mặc định (`[sandbox] writable_tmp = false` để tắt). Dự án nằm trong `/tmp` sẽ ghi được sang các thư mục anh em.
- Trên Linux, Landlock không ẩn được file theo tên: `.env` trong workspace vẫn đọc được (giá trị vẫn bị che trong log); thư mục cha của một đường dẫn bị ẩn không liệt kê được; giới hạn mạng theo cổng TCP, UDP không bị chặn. Cần Linux 6.7 trở lên.
- Windows chỉ giam được việc **ghi**: tiến trình chạy tool bị hạ xuống mức toàn vẹn Low nên chỉ ghi được trong thư mục dự án và một thư mục tạm riêng (`AppData\LocalLow\hame-tmp`). Tool vẫn đọc được mọi file bạn đọc được và vẫn ra mạng được, nên phải bật rõ ràng bằng `allow_partial = true` trong `[sandbox]`; không bật thì các chế độ giam từ chối khởi động. Thư mục dự án được gắn nhãn Low vĩnh viễn (chương trình Low khác của cùng người dùng cũng ghi được vào đó). Cả `AppData\LocalLow` cũng ghi được, vì Windows gắn sẵn nhãn Low cho nó. File hoặc thư mục được **chuyển** (không phải chép) vào dự án sau lần chạy đầu giữ nhãn cũ nên tool không sửa được; gắn lại bằng `icacls <thư mục dự án> /setintegritylevel "(OI)(CI)L" /T`. Tool `bash` dùng bash của Git for Windows, tìm theo `git` trên PATH. Bash đó giữ trạng thái dùng chung theo từng bản cài: khi một Git Bash khác đang mở ngoài sandbox, bash trong sandbox không khởi động được (tool sẽ báo rõ); đóng nó lại, hoặc đặt biến môi trường `HAME_BASH` trỏ tới `bash.exe` của một bản Git thứ hai chỉ dành cho hame. Proxy mạng không ép được trên Windows; nó chỉ được bật khi có `[secrets] substitute`, và chỉ có tác dụng với chương trình tuân theo biến proxy (Python, Node, git; curl đi kèm Git for Windows từ chối chứng chỉ của proxy). Chương trình đi vòng proxy tới được mọi host nhưng chỉ cầm giá trị giả. Đã kiểm trên GitHub Actions và trên một máy Windows 11 thật; kết quả và những gì chưa kiểm ở `docs/WINDOWS-RESULTS.md`.
- Phần mềm này chưa qua kiểm toán bảo mật. Với mã không tin cậy, hãy chạy trong máy ảo hoặc container.

## Cấu hình

`~/.hame/config.toml`. Mọi mục đều tùy chọn; khóa lạ là lỗi.

```toml
[model]
provider = "anthropic"          # deepseek | openai | anthropic
name = "claude-opus-5-5"
effort = "high"                 # mức suy luận (Anthropic, OpenAI)
api = "responses"               # với openai: responses (mặc định) | chat-completions
summary_model = "claude-haiku-5-5"   # model rẻ hơn để tóm tắt lịch sử
tools = "full"                  # full | minimal (chỉ file, tìm kiếm, shell; cho model nhỏ)
edit_format = "search-replace"  # search-replace | whole-file | apply-patch | hashline (read gắn mã băm ngắn vào từng dòng; edit_lines sửa theo mã đó, và từ chối khi file đã đổi)
max_tool_calls = 32             # số lời gọi tool tối đa trong một phản hồi
fuse_actions = true             # edit/edit_lines/write/apply_patch nhận tham số then_run: lệnh chạy ngay sau khi sửa, kết quả trả về cùng lời gọi
temperature = 0.0               # chỉ với API chat-completions (DeepSeek, máy chủ tương thích, model chạy tại chỗ)
escalate_to = "claude-fable-5-2"     # model mạnh hơn tiếp quản phần còn lại của lượt khi model chính kẹt
escalate_provider = "anthropic"      # tùy chọn: model mạnh thuộc nhà cung cấp khác (ví dụ model chính chạy tại chỗ)
escalate_effort = "high"             # không kèm escalate_to: vẫn model đó nhưng suy luận kỹ hơn khi lượt bị kẹt, nên có thể đặt effort thấp cho việc thường ngày
escalate_after = 2              # sau bấy nhiêu lần kiểm tra hỏng hoặc gọi lặp trong một lượt

[sandbox]
mode = "workspace-write"        # read-only | workspace-write | danger-full-access
allow_partial = false           # Windows: chấp nhận chỉ giam việc ghi
protect_git = true              # hoàn tác thay đổi của tool vào .git/hooks và .git/config
writable = ["/data/cache"]
unreadable = ["/data/private"]

[network]
mode = "allowlist"              # off | allowlist | open
allow = ["github.com", "*.crates.io"]

[[rule]]                        # luật đầu tiên khớp sẽ thắng
tool = "bash"
pattern = "git push*"
action = "ask"                  # allow | ask | deny

[approval]
default = "ask"                 # với lời gọi không luật nào khớp: allow | ask | deny
reviewer = false                # true: một model (summary_model) trả lời câu hỏi xin phép trước;
                                # nó chỉ có thể đồng ý với lệnh rõ ràng vô hại, còn lại vẫn hỏi bạn

[secrets]
grant = ["GITHUB_TOKEN"]        # trao thẳng secret cho tool
substitute = { NPM_TOKEN = ["registry.npmjs.org"] }   # tool chỉ nhận giá trị giả; proxy thay bằng secret thật
                                # trong request tới các host này (cần [network] mode = "allowlist")

[context]
compact_at_tokens = 100000      # tóm tắt phần lịch sử cũ khi vượt ngưỡng
clear_tool_results = false      # true: vượt ngưỡng thì xóa kết quả tool cũ trước, chưa đủ mới tóm tắt
spill_at_bytes = 16000          # đầu ra dài hơn thì lưu ra file, chỉ xem trước
project_facts = true            # nhớ theo từng dự án lệnh kiểm tra đã đạt lần trước và lệnh không có trên máy, báo cho phiên sau (~/.hame/facts/)
dedupe_results = true           # kết quả tool giống hệt một kết quả trước còn trong ngữ cảnh thì chỉ ghi một dòng trỏ tới kết quả đó
condense_output = true          # bỏ dòng test đạt và tiến trình build khỏi đầu ra lệnh; dòng lỗi và tổng kết giữ nguyên văn, bản đầy đủ lưu ra file
defer_tools_over = 20           # nhiều tool MCP/plugin hơn số này thì không khai báo cho model;
                                # model tìm bằng tool_search và chạy bằng tool_call

[verify]
commands = ["cargo test -q"]    # phải đạt trước khi một lượt có sửa file được kết thúc
checklist = false               # true: trước khi kết thúc, yêu cầu model rà lại từng điểm của đề bài
stop_on_red = true              # model định dừng khi lệnh test/build cuối cùng còn hỏng, hoặc khi vừa sửa mã nguồn
                                # mà chưa chạy gì: nhắc nó làm tiếp (tối đa 2 lần mỗi lượt)
review = false                  # true: trước khi kết thúc, một agent mới (chỉ thấy đề bài và file) dò những luật
                                # dễ sót rồi báo vi phạm cho model sửa; một lần mỗi lượt, tốn thêm token của agent đó

[limits]
tree_tokens = 2000000           # ngân sách token cho agent và mọi subagent; 0 = không giới hạn
subagent_depth = 1
max_steps = 100                 # số request tối đa trong một lượt (hame run --max-steps ghi đè)

[mcp.docs]                      # server MCP (stdio); mặc định chạy trong sandbox
command = "docs-server"
args = ["--stdio"]

[mcp.tracker]                   # server MCP qua HTTP (streamable HTTP)
url = "https://mcp.example.com/mcp"
headers = { Authorization = "Bearer $TRACKER_TOKEN" }   # $TÊN lấy từ biến môi trường hoặc file .env

[lsp.rust]                      # language server cho `diagnostics`, `references`, `rename_symbol`
command = "rust-analyzer"
extensions = ["rs"]

[[hook]]                        # lệnh chạy quanh mỗi lần gọi tool; thoát mã 2 = phản đối
event = "pre_tool"
tool = "bash"
command = "./scripts/check-command.sh"

[compat]
claude_hooks = ["user"]         # chạy hooks viết cho Claude Code: "user" = ~/.claude/settings.json,
                                # "project" = .claude/settings(.local).json của dự án; mặc định không chạy
claude_plugins = ["/đường/dẫn/tới/plugin"]   # plugin đóng gói cho Claude Code: nạp skills, hooks và server MCP của nó

[plugin.wc]                     # plugin WebAssembly và nơi nó được đọc
path = "plugins/wordcount.wasm"
read = ["docs"]

[computer]                      # tool trình duyệt và desktop; xem "Điều khiển trình duyệt và desktop"
browser = false
desktop = false
headless = true
browser_channel = "chrome"      # chrome | msedge | chromium
batch = true
observation = "semantic"        # semantic | screenshot

[[computer.grant]]
channel = "browser"             # browser | desktop | visual
origin = "http://127.0.0.1:*"   # hoặc: app = "com.apple.TextEdit", profile = "work"
capabilities = ["observe", "interact"]
```

Trong `[model]`, `vision = true` / `pdf = true` khẳng định model nhận ảnh / PDF (mặc định theo hiểu biết của hame: model Claude và OpenAI đã biết thì có, các model khác coi là chỉ nhận văn bản).

Khi một lời gọi tool lặp lại y hệt và trả về y hệt ba lần liên tiếp, hoặc cùng một lời gọi thất bại bốn lần mà chưa lần nào đạt, kết quả kèm một ghi chú báo cho model biết nó đang kẹt (ghi chú này cũng được tính khi quyết định leo thang). Từ lần thứ năm liên tiếp giống hệt, lời gọi không được chạy nữa (kể cả lệnh thăm dò trạng thái lặp lại y nguyên). Ba request trước khi lượt hết số bước, model được báo để kịp tổng kết. Mỗi lần đổi model (leo thang, rồi trả lại ở lượt sau) được ghi vào log.

MCP: server stdio chạy trong sandbox; server HTTP được gọi từ tiến trình hame, xác thực bằng header tĩnh. Client nói được cả các bản có bắt tay (`initialize`, tới `2025-06-18`) lẫn bản không trạng thái `2026-07-28`: nó bắt tay kiểu cũ trước, và khi server từ chối thì hỏi `server/discover` rồi gửi phiên bản, danh tính cùng các header `Mcp-Method`, `Mcp-Name`, `Mcp-Param-*` trên từng request. Server dùng OAuth: `hame mcp login <tên>` tìm máy chủ cấp quyền từ chính server, tự đăng ký client, mở trình duyệt để bạn đăng nhập (authorization code với PKCE) và lưu token vào `~/.hame/mcp-auth/` (chỉ bạn đọc được); token được làm mới khi một phiên bắt đầu, không làm mới giữa phiên. Luồng này đã chạy trọn với một nhà cung cấp thật (server MCP của Notion, ngày 2026-11-14): đăng ký client, đăng nhập, dùng token, làm mới token hết hạn và đăng xuất. Với máy chủ cấp quyền không cho client tự đăng ký, đăng ký hame với nhà cung cấp rồi đặt `oauth_client_id` (và `oauth_redirect_port`, cổng của địa chỉ chuyển hướng `http://127.0.0.1:<cổng>/callback`) trong mục `[mcp.<tên>]`. Chưa hỗ trợ: những gì server muốn hỏi ngược lại (sampling, elicitation, roots) hay chạy thành tác vụ nền; gặp các trường hợp đó tool báo lỗi rõ ràng. Cả hai đường đã được kiểm với server dựng bằng SDK MCP chính thức (gói `mcp` 2.3.0), qua stdio và qua HTTP. Đặt `protocol = "2026-07-28"` trong mục `[mcp.<tên>]` để đi thẳng tới bản không trạng thái mà không thử bắt tay cũ.

Hooks của Claude Code: hame đọc cùng file `settings.json` và nói cùng giao thức (JSON qua stdin, mã thoát 2 để chặn, JSON trên stdout) cho các sự kiện `PreToolUse`, `PostToolUse`, `UserPromptSubmit` và `Stop`, với hook loại `command`, `matcher` và `if`. Tên tool và tham số được đổi sang dạng Claude Code (`Bash`, `Edit`, `file_path`…). Sự kiện và loại hook khác được báo một lần lúc khởi động rồi bỏ qua. Khác biệt cần biết: hook chạy lần lượt chứ không song song; `permissionDecision: "ask"` bị coi là chặn; `"allow"` không bỏ qua luật quyền của hame; `tool_response` chỉ có trường `output`; `transcript_path` trỏ tới log của hame, không cùng định dạng với Claude Code. Hook chạy ngoài sandbox, nên chỉ bật `"project"` với repo bạn tin.

Plugin của Claude Code: với mỗi plugin đã cài bằng `hame plugin install` (nằm trong `~/.hame/plugins`) và mỗi thư mục trong `claude_plugins`, hame nạp `skills/`, `commands/`, `agents/`, `hooks/hooks.json` (kể cả biến `${CLAUDE_PLUGIN_ROOT}`) và các server trong `.mcp.json` (stdio và HTTP; tool mang tên `mcp__<plugin>_<server>__<tool>`). Trình cài quét toàn bộ plugin như với skill và nói rõ khi plugin có hook hoặc server MCP. Nguồn cài có thể là một marketplace (thư mục hoặc kho git có `.claude-plugin/marketplace.json`): chọn plugin bằng `--plugin`; mục trỏ sang kho git khác được lấy đúng commit mà danh mục ghim. `hame plugin update` lấy lại từ nguồn đã ghi nhớ và quét lại trước khi thay; không có cập nhật tự động. Chưa hỗ trợ: các phần khác của plugin (output style, theme, LSP) và plugin phân phối bằng lệnh cài riêng.

Lệnh và loại subagent: file `commands/<tên>.md` trong `.hame/`, `.claude/`, `~/.hame/` hoặc trong plugin là một prompt gọi bằng `/<tên> tham số` (hỗ trợ `$ARGUMENTS`, `$1`…`$9`), dùng được với `hame run`, giao diện terminal và ACP. File `agents/<tên>.md` (front matter `name`, `description`, `tools`) định nghĩa một loại subagent mà model chọn qua tham số `agent` của tool `subagent`; nó chạy với chỉ dẫn trong file và, nếu có `tools`, chỉ với các tool đó.

Dự án có thể tự khai báo lệnh kiểm tra trong `.hame/config.toml` (chỉ mục `[verify]`; một repo không được phép nới sandbox). `AGENTS.md` và `CLAUDE.md` của dự án, cùng `~/.hame/AGENTS.md`, được nối vào prompt hệ thống. Skill đặt ở `.hame/skills/<tên>/SKILL.md`, `.claude/skills/` hoặc `~/.hame/skills/`.

`hame skill install` đọc mọi file của skill trước khi chép: ký tự vô hình, câu bảo model bỏ chỉ dẫn hoặc giấu người dùng, lệnh tải về rồi đưa thẳng vào shell, mã ẩn trong base64, đường dẫn tới khóa và file bí mật, symlink. Thấy dấu hiệu nguy hiểm thì không cài, trừ khi bạn thêm `--force`. Đây là so mẫu, không phải chứng minh: quét sạch nghĩa là không thấy các mánh đã biết, không có nghĩa skill an toàn.

Plugin mẫu và mô tả ABI: `plugins/wordcount`.

## Session, artifact và lưu giữ

Mỗi session là một thư mục trong `~/.hame/sessions/` chứa log (định dạng phiên bản 2) và, với các tool thao tác ngoài sandbox (trình duyệt, desktop), một nhật ký thao tác (`actions.v1.jsonl`). Ảnh và file mà tool trả về (ảnh chụp, file tải về, ảnh từ server MCP) được lưu một lần theo SHA-256 trong `~/.hame/artifacts/`, dùng chung cho mọi session, fork và session nhập vào; log chỉ giữ tham chiếu có kiểu. Giao diện terminal hiện mỗi thứ thành một dòng mô tả, giao diện web và client ACP hiện ảnh. Model được xem ít nhất 3 ảnh mới nhất trong kết quả tool; ảnh cũ hơn thành một ghi chú nêu tên artifact, và bị bỏ ba ảnh một lần để phần đầu prompt đã cache giữ nguyên giữa các bước. Khi mở lại một session, hame cảnh báo artifact bị thiếu hay hỏng; session vẫn chạy và model nhận một ghi chú thay vào chỗ đó. Không có gì bị xóa tự động: `hame sessions gc` xóa artifact không còn log nào tham chiếu và cũ hơn 24 giờ (`--older-than-hours N`, `--dry-run` để chỉ báo), cùng file tạm của những lần ghi bị gián đoạn. Xóa một thư mục session thì artifact của nó trở thành thu dọn được. Chưa có lệnh xuất; muốn chuyển session thì chép thư mục của nó cùng `~/.hame/artifacts/`. Sau crash, thao tác đã gửi mà chưa được xác nhận được ghi là `outcome_unknown` và model được dặn kiểm tra trạng thái trước khi thử lại; hame không tự lặp lại chúng. Fork mang theo mọi biên nhận đã chốt của session gốc, vì fork không hoàn tác được gì ngoài workspace.

Log do bản này ghi (định dạng 2) bị các bản trước 0.1 từ chối ("session format version 2 is newer than this build supports"); log cũ mở bình thường.

## Trace và đánh giá

`hame run --trace FILE` và `hame task --trace FILE` ghi một trace (`trace.v1.jsonl`): mỗi lượt, mỗi lần gọi model (kể cả lần thử lại và lần hỏng), mỗi lời gọi tool (kể cả các bước bên trong `script`, `batch` và workflow) và bước kiểm chứng là một span có thời gian và trạng thái, cùng một bản ghi usage cho mỗi lần gọi với số token của nhà cung cấp, đã chuẩn hóa và nguyên bản. `--trace session` đặt nó cạnh log session; `HAME_TRACE` đặt cho `hame run`. Trace chỉ chứa tên, mã, kích thước và thời gian, không bao giờ chứa đầu vào thô của tool hay nội dung lỗi của nhà cung cấp, và không bao giờ vào prompt. Không có gì được gửi đi: hame không có telemetry rời khỏi máy bạn.

`hame eval` đọc các trace đó để báo token, số request, số byte request và chi phí mỗi lần chạy. Chi phí lấy từ bảng giá có ghi ngày (model Anthropic, 2026-11-15); model không có trong bảng thì chi phí là không rõ, không bao giờ là 0. So sánh các cấu hình trên cùng tác vụ:

```sh
hame eval evals/pilot --arm base --arm lean=lean.toml --runs 3 --json out/pilot.json
hame eval evals/pilot --arm base --arm nodedupe --set nodedupe:context.dedupe_results=false
```

Các nhánh chạy xen kẽ; mỗi nhánh được so với nhánh đầu, từng tác vụ, kèm khoảng tin cậy bootstrap (`--bootstrap 2000`, `--seed 1`). Mỗi lần chạy ghi kết quả (`pass`, `fail`, `false_success` khi agent báo xong mà bộ chấm ẩn không đạt, `timeout`, `error`, `unsupported`), thời gian của agent, của bộ chấm và tổng. Định dạng manifest ở [evals/pilot/README.md](evals/pilot/README.md); số liệu đo được ở [docs/release/EVIDENCE.md](docs/release/EVIDENCE.md).

## Xử lý sự cố

Bắt đầu bằng `hame doctor`: mỗi dòng không phải `ok` đều nói cần làm gì.

- **`browser`: "Playwright for Python is not importable by python3".** Helper trình duyệt chạy `python3 -I` (chế độ cô lập), bỏ qua `PYTHONPATH` và thư mục site của người dùng, nên `pip install --user playwright` hay gói chỉ thấy qua `PYTHONPATH` đều không dùng được. Hãy cài Playwright vào chính trình thông dịch là `python3` trên `PATH` của hame. Với Python của Homebrew hay của hệ thống từ chối `pip install` (PEP 668), dùng môi trường ảo và đặt nó đầu `PATH` khi khởi động hame:

  ```sh
  python3 -m venv ~/.hame/venv
  ~/.hame/venv/bin/pip install playwright
  export PATH="$HOME/.hame/venv/bin:$PATH"
  python3 -I -c "import playwright.async_api" && echo ok   # đúng phép kiểm của hame
  ```
- **`browser`: "Google Chrome is not installed".** Cài Chrome, hoặc đặt `browser_channel = "chromium"` trong `[computer]` sau khi chạy `python3 -m playwright install chromium`.
- **`desktop`: `permission_denied`, hoặc doctor báo chưa cấp Accessibility.** macOS cấp Accessibility và Screen Recording cho ứng dụng đã khởi động hame (Terminal, iTerm, editor của bạn), không phải cho hame. Thêm ứng dụng đó vào System Settings > Privacy & Security > Accessibility (và Screen & System Audio Recording để chụp màn hình), rồi thoát hẳn và mở lại nó. Ảnh chụp chỉ có một màu bị từ chối là `blank_capture`; thường là do thiếu hoặc quyền Screen Recording đã cũ.
- **`desktop`: helper không build được.** Cài bộ công cụ dòng lệnh: `xcode-select --install`. Helper được lưu sẵn trong `~/.hame/helpers/` và build lại khi bản trong hame thay đổi.
- **`not_authorized: ... outside the granted scope`.** Thao tác cần một năng lực mà không quyền cấp nào phủ cho app, origin hay hồ sơ đó. Thêm quyền vào `[computer]` hoặc file tác vụ; chữ trên màn hình không cấp được quyền.
- **Model không thấy ảnh chụp ("not sent, ...").** Ảnh chỉ được gửi khi hame biết model nhận ảnh: model Claude, các họ gpt-4o, gpt-4.1, gpt-4.5, gpt-4-turbo, gpt-5, o1, o3, o4 của OpenAI, và qua máy chủ tương thích là các tên bắt đầu bằng `gemini` hoặc chứa `-vl`, `vision`. Các model khác, kể cả DeepSeek, bị coi là chỉ nhận văn bản và mỗi ảnh thành một ghi chú ngắn (ảnh vẫn được lưu). Nếu model của bạn nhận ảnh, đặt `vision = true` (và `pdf = true` cho PDF) trong `[model]`; nếu không, chọn model có thị giác hoặc để agent làm việc từ cấu trúc trang và dữ liệu accessibility.
- **`Workflow ... is not trusted in this exact version`.** Xem lại bằng `hame workflow show TÊN` rồi chạy `hame workflow trust TÊN`. File đã sửa phải tin dùng lại.
- **"session format version N is newer than this build supports".** Log được ghi bởi bản hame mới hơn; hãy dùng bản đó.

## Giới hạn

- **Điều khiển desktop chỉ có trên macOS** (14 trở lên) và mới được kiểm trên macOS 26.6.2 với TextEdit, Numbers 14.4 và Pages 14.4. Excel không cho accessibility thấy lưới ô; Word và Keynote chưa thử. Quyền `file_write` trên desktop không giới hạn theo thư mục (xem bảng quyền). Lệnh trong sandbox shell không gửi được Apple Events (`osascript`), không dùng được giao diện accessibility, không giả lập hay nghe lén phím chuột, không chụp màn hình và không dùng clipboard, nên ứng dụng khác chỉ được điều khiển qua tool desktop; chương trình cần window server (ứng dụng GUI, trình duyệt) không chạy được trong sandbox. Chưa có: cuộn, kéo thả, rê chuột, chọn mục trong menu bật lên theo giá trị, menu ngữ cảnh. `key` giả định bàn phím Mỹ (ANSI); ô nhập mật khẩu thường bỏ qua phím giả lập; không chụp được cửa sổ ở Space khác hay đang thu nhỏ.
- **Tool trình duyệt** điều khiển trình duyệt họ Chrome qua Playwright và mới được kiểm trên macOS arm64. Iframe và shadow root đóng không được mở ra; việc nhận ra commit là heuristic; request riêng của trang không bị lọc.
- **Kiểm chứng kết quả** chỉ thấy những gì điều kiện nêu ra: file tác vụ không kiểm một tác động phụ thì nó không được kiểm chứng. `desktop_state` đọc giá trị bị cắt ở 300 ký tự; tài liệu dài hãy kiểm trong file đã lưu. Việc nhận ra báo cáo thành công sai trong `hame eval` dựa vào cụm từ.
- **Ngân sách**: chỉ có bảng giá cho model Anthropic, nên với provider khác `max_cost_usd` dừng tác vụ ngay ở request đầu tiên.
- **Kiểm thử với nhà cung cấp thật** mới làm với Anthropic (ảnh trong kết quả tool và trong tin nhắn, ghi chú khi thiếu ảnh, cache prompt) bằng claude-haiku-5-5. Đường gửi ảnh và PDF của OpenAI, và PDF của Anthropic, mới được kiểm bằng dữ liệu mẫu đã ghi. Xem [docs/release/SUPPORT-MATRIX.md](docs/release/SUPPORT-MATRIX.md).
- Chưa có bộ cài hay binary dựng sẵn; hãy build từ mã nguồn. hame chưa qua kiểm toán bảo mật.

## Cấu trúc mã

| Crate | Việc |
|---|---|
| `hame-kernel` | Registry service, bus sự kiện, chuỗi middleware |
| `hame-session` | Schema sự kiện, kho JSONL, khôi phục sau crash, fork, projection; kho artifact; hợp đồng tác vụ, quyền cấp, quan sát và biên nhận; nhật ký thao tác |
| `hame-llm` | Adapter OpenAI Responses, Anthropic Messages và chat-completions (DeepSeek, máy chủ tương thích); gửi ảnh và PDF gốc; hồ sơ năng lực model |
| `hame-tools` | Tool có sẵn và trait `Tool` |
| `hame-agent` | Vòng turn/step, hook, checkpoint |
| `hame-sandbox` | Chính sách, Seatbelt (macOS), Landlock (Linux), mức toàn vẹn (Windows), worker, proxy egress |
| `hame-policy` | Cấu hình, luật quyền, hỏi phép, che secret |
| `hame-checkpoint` | Checkpoint workspace bằng kho git riêng |
| `hame-context` | Tràn đầu ra ra file, nén ngữ cảnh, nạp chỉ dẫn dự án |
| `hame-verify` | Cổng kiểm chứng trước khi kết thúc lượt; phát hiện lặp; leo thang model; điều kiện nghiệm thu, file tác vụ, ngân sách, workflow |
| `hame-browser` | Tool `browser` qua một helper Playwright chạy lâu dài |
| `hame-desktop` | Tool `desktop` qua một helper accessibility viết bằng Swift (macOS) |
| `hame-trace` | Trace lần chạy, tính usage, bảng giá có ghi ngày |
| `hame-ext` | MCP, LSP (chẩn đoán, tìm nơi dùng, đổi tên symbol), skills, hooks, plugin WebAssembly |
| `hame-multi` | Subagent, worktree, bảng việc, ngân sách chung |
| `hame-app` | Lắp một agent hoàn chỉnh từ cấu hình; nhập session |
| `hame-acp` | Máy chủ Agent Client Protocol; giao diện terminal cũng là một client của nó |
| `hame-tui`, `hame-web`, `hame-cli` | Các giao diện |
| `hame-eval` | Model giả phát lại bản ghi; manifest tác vụ, bản ghi lần chạy, so sánh nhánh và bootstrap |

## Phát triển

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Test của plugin cần `rustup target add wasm32-unknown-unknown`; một số test cần `python3` và `git`. Đặt `HAME_SCRIPT=<file.jsonl>` để `hame` phát lại phản hồi đã ghi thay vì gọi nhà cung cấp. Test trình duyệt tự bỏ qua khi thiếu Playwright và Chrome; test giao diện desktop phải bật riêng (`--ignored`). Công cụ phát hành (`scripts/docker-clean-install.sh`, `scripts/license-report.py`, `scripts/release-check.sh`) và tài liệu phát hành nằm trong [docs/release/](docs/release/).
