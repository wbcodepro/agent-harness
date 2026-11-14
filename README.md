# hame

`hame` là một agent harness viết bằng Rust: một binary duy nhất chạy mô hình ngôn ngữ như một agent lập trình trong terminal, trong editor (ACP) hoặc trong trình duyệt.

Ba ý chính của thiết kế:

- **Log là nguồn sự thật.** Mỗi session là một file JSONL chỉ ghi thêm. Mọi thứ model nhìn thấy (prompt hệ thống, danh sách tool, tin nhắn) đều được dựng lại từ log, nên resume, fork, rewind, nén ngữ cảnh và phát lại đều dùng chung một cơ chế.
- **Tool không chạy trong tiến trình của harness.** Mỗi lần gọi tool chạy trong một tiến trình worker bị hệ điều hành giam (Seatbelt trên macOS, Landlock trên Linux). Không giam được thì từ chối chạy.
- **Mở rộng mà không nới quyền.** Plugin là WebAssembly và chỉ đọc được nơi bạn cấp; server MCP chạy trong cùng sandbox với tool.

## Cài đặt

Cần Rust 1.89 trở lên và `git`.

```sh
cargo build --release -p hame-cli      # tạo target/release/hame
```

Đặt API key vào biến môi trường hoặc file `~/.hame/.env` (hoặc `.env` trong thư mục làm việc):

```sh
DEEPSEEK_API_KEY=...     # hoặc ANTHROPIC_API_KEY / CLAUDE_API_KEY, OPENAI_API_KEY
```

## Dùng

```sh
hame                                  # giao diện terminal
hame run "sửa lỗi làm test_stats.py thất bại"     # một lượt, không tương tác
hame run --session <id> "tiếp tục"    # nối tiếp một session
hame run --plan "..."                 # chỉ khảo sát và đề xuất kế hoạch, không sửa gì
hame run --output json "..."          # một đối tượng JSON khi xong (stream-json: thêm từng sự kiện một dòng)
hame web                              # giao diện web trên 127.0.0.1
hame acp                              # máy chủ Agent Client Protocol cho editor
hame sessions | show <id> | fork <id> | rewind <id> | recover <id>
hame import dsh <session.jsonl>       # nhập session của DeepSeek Harness (v2–v4)
hame eval evals/harder                # chạy bộ tác vụ, báo tỷ lệ đạt, thời gian, token (--keep <thư mục>: giữ log của tác vụ hỏng; --runs N: chạy N lần, báo tỷ lệ đạt từng tác vụ)
hame eval evals/harder --command 'codex exec "$HAME_EVAL_PROMPT"'   # đo một agent khác trên cùng bộ
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

## Tool có sẵn

`read`, `write`, `edit`, `bash`, `glob`, `grep`, `repo_map`, `subagent`, `task_create` / `task_update` / `task_list`; thêm `skill` khi có skill và `diagnostics` khi có cấu hình language server. Với `edit_format = "apply-patch"`, tool `apply_patch` thay cho `edit` và `write`.

`write` từ chối ghi đè một file đã tồn tại mà model chưa đọc trong phiên (nó phải đọc trước, vì nội dung file thường là thứ đề bài phụ thuộc vào); khi ghi làm file nhỏ đi quá nửa, kết quả nói rõ là cả file đã bị thay. `edit` không tìm thấy đoạn cần thay thì chỉ ra khớp tới dòng nào và khác ở đâu. `grep` ghi kèm tên hàm hoặc lớp bao quanh mỗi dòng khớp. Sau mỗi lần `edit`, `write` hay `apply_patch`, nếu file nguồn đang hợp lệ trở nên sai cú pháp thì kết quả báo ngay dòng lỗi (Python được kiểm bằng `python3` khi có; Rust, JS, TS, Go bằng tree-sitter).

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

`--sandbox read-only` cấm ghi; `--sandbox danger-full-access` bỏ giam, khi đó mọi tool không chỉ-đọc đều phải được bạn đồng ý.

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
edit_format = "search-replace"  # search-replace | whole-file | apply-patch
max_tool_calls = 32             # số lời gọi tool tối đa trong một phản hồi
fuse_actions = true             # edit/write/apply_patch nhận tham số then_run: lệnh chạy ngay sau khi sửa, kết quả trả về cùng lời gọi
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
condense_output = true          # bỏ dòng test đạt và tiến trình build khỏi đầu ra lệnh; dòng lỗi và tổng kết giữ nguyên văn, bản đầy đủ lưu ra file
defer_tools_over = 20           # nhiều tool MCP/plugin hơn số này thì không khai báo cho model;
                                # model tìm bằng tool_search và chạy bằng tool_call

[verify]
commands = ["cargo test -q"]    # phải đạt trước khi một lượt có sửa file được kết thúc
checklist = false               # true: trước khi kết thúc, yêu cầu model rà lại từng điểm của đề bài
stop_on_red = true              # model định dừng khi lệnh test/build cuối cùng còn hỏng, hoặc khi vừa sửa mã nguồn
                                # mà chưa chạy gì: nhắc nó làm tiếp (tối đa 2 lần mỗi lượt)

[limits]
tree_tokens = 2000000           # ngân sách token cho agent và mọi subagent; 0 = không giới hạn
subagent_depth = 1

[mcp.docs]                      # server MCP (stdio); mặc định chạy trong sandbox
command = "docs-server"
args = ["--stdio"]

[mcp.tracker]                   # server MCP qua HTTP (streamable HTTP)
url = "https://mcp.example.com/mcp"
headers = { Authorization = "Bearer $TRACKER_TOKEN" }   # $TÊN lấy từ biến môi trường hoặc file .env

[lsp.rust]                      # language server cho tool `diagnostics`
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
```

Khi một lời gọi tool lặp lại y hệt và trả về y hệt ba lần liên tiếp, hoặc cùng một lời gọi thất bại bốn lần mà chưa lần nào đạt, kết quả kèm một ghi chú báo cho model biết nó đang kẹt (ghi chú này cũng được tính khi quyết định leo thang). Từ lần thứ năm liên tiếp giống hệt, lời gọi không được chạy nữa (kể cả lệnh thăm dò trạng thái lặp lại y nguyên). Ba request trước khi lượt hết số bước, model được báo để kịp tổng kết. Mỗi lần đổi model (leo thang, rồi trả lại ở lượt sau) được ghi vào log.

MCP: server stdio chạy trong sandbox; server HTTP được gọi từ tiến trình hame, xác thực bằng header tĩnh. Client nói được cả các bản có bắt tay (`initialize`, tới `2025-06-18`) lẫn bản không trạng thái `2026-07-28`: nó bắt tay kiểu cũ trước, và khi server từ chối thì hỏi `server/discover` rồi gửi phiên bản, danh tính cùng các header `Mcp-Method`, `Mcp-Name`, `Mcp-Param-*` trên từng request. Server dùng OAuth: `hame mcp login <tên>` tìm máy chủ cấp quyền từ chính server, tự đăng ký client, mở trình duyệt để bạn đăng nhập (authorization code với PKCE) và lưu token vào `~/.hame/mcp-auth/` (chỉ bạn đọc được); token được làm mới khi một phiên bắt đầu, không làm mới giữa phiên. Luồng này đã chạy trọn với một nhà cung cấp thật (server MCP của Notion, ngày 2026-11-14): đăng ký client, đăng nhập, dùng token, làm mới token hết hạn và đăng xuất. Với máy chủ cấp quyền không cho client tự đăng ký, đăng ký hame với nhà cung cấp rồi đặt `oauth_client_id` (và `oauth_redirect_port`, cổng của địa chỉ chuyển hướng `http://127.0.0.1:<cổng>/callback`) trong mục `[mcp.<tên>]`. Chưa hỗ trợ: những gì server muốn hỏi ngược lại (sampling, elicitation, roots) hay chạy thành tác vụ nền; gặp các trường hợp đó tool báo lỗi rõ ràng. Cả hai đường đã được kiểm với server dựng bằng SDK MCP chính thức (gói `mcp` 2.3.0), qua stdio và qua HTTP. Đặt `protocol = "2026-07-28"` trong mục `[mcp.<tên>]` để đi thẳng tới bản không trạng thái mà không thử bắt tay cũ.

Hooks của Claude Code: hame đọc cùng file `settings.json` và nói cùng giao thức (JSON qua stdin, mã thoát 2 để chặn, JSON trên stdout) cho các sự kiện `PreToolUse`, `PostToolUse`, `UserPromptSubmit` và `Stop`, với hook loại `command`, `matcher` và `if`. Tên tool và tham số được đổi sang dạng Claude Code (`Bash`, `Edit`, `file_path`…). Sự kiện và loại hook khác được báo một lần lúc khởi động rồi bỏ qua. Khác biệt cần biết: hook chạy lần lượt chứ không song song; `permissionDecision: "ask"` bị coi là chặn; `"allow"` không bỏ qua luật quyền của hame; `tool_response` chỉ có trường `output`; `transcript_path` trỏ tới log của hame, không cùng định dạng với Claude Code. Hook chạy ngoài sandbox, nên chỉ bật `"project"` với repo bạn tin.

Plugin của Claude Code: với mỗi plugin đã cài bằng `hame plugin install` (nằm trong `~/.hame/plugins`) và mỗi thư mục trong `claude_plugins`, hame nạp `skills/`, `commands/`, `agents/`, `hooks/hooks.json` (kể cả biến `${CLAUDE_PLUGIN_ROOT}`) và các server trong `.mcp.json` (stdio và HTTP; tool mang tên `mcp__<plugin>_<server>__<tool>`). Trình cài quét toàn bộ plugin như với skill và nói rõ khi plugin có hook hoặc server MCP. Nguồn cài có thể là một marketplace (thư mục hoặc kho git có `.claude-plugin/marketplace.json`): chọn plugin bằng `--plugin`; mục trỏ sang kho git khác được lấy đúng commit mà danh mục ghim. `hame plugin update` lấy lại từ nguồn đã ghi nhớ và quét lại trước khi thay; không có cập nhật tự động. Chưa hỗ trợ: các phần khác của plugin (output style, theme, LSP) và plugin phân phối bằng lệnh cài riêng.

Lệnh và loại subagent: file `commands/<tên>.md` trong `.hame/`, `.claude/`, `~/.hame/` hoặc trong plugin là một prompt gọi bằng `/<tên> tham số` (hỗ trợ `$ARGUMENTS`, `$1`…`$9`), dùng được với `hame run`, giao diện terminal và ACP. File `agents/<tên>.md` (front matter `name`, `description`, `tools`) định nghĩa một loại subagent mà model chọn qua tham số `agent` của tool `subagent`; nó chạy với chỉ dẫn trong file và, nếu có `tools`, chỉ với các tool đó.

Dự án có thể tự khai báo lệnh kiểm tra trong `.hame/config.toml` (chỉ mục `[verify]`; một repo không được phép nới sandbox). `AGENTS.md` và `CLAUDE.md` của dự án, cùng `~/.hame/AGENTS.md`, được nối vào prompt hệ thống. Skill đặt ở `.hame/skills/<tên>/SKILL.md`, `.claude/skills/` hoặc `~/.hame/skills/`.

`hame skill install` đọc mọi file của skill trước khi chép: ký tự vô hình, câu bảo model bỏ chỉ dẫn hoặc giấu người dùng, lệnh tải về rồi đưa thẳng vào shell, mã ẩn trong base64, đường dẫn tới khóa và file bí mật, symlink. Thấy dấu hiệu nguy hiểm thì không cài, trừ khi bạn thêm `--force`. Đây là so mẫu, không phải chứng minh: quét sạch nghĩa là không thấy các mánh đã biết, không có nghĩa skill an toàn.

Plugin mẫu và mô tả ABI: `plugins/wordcount`.

## Cấu trúc mã

| Crate | Việc |
|---|---|
| `hame-kernel` | Registry service, bus sự kiện, chuỗi middleware |
| `hame-session` | Schema sự kiện, kho JSONL, khôi phục sau crash, fork, projection |
| `hame-llm` | Adapter OpenAI Responses, Anthropic Messages và chat-completions (DeepSeek, máy chủ tương thích) |
| `hame-tools` | Tool có sẵn và trait `Tool` |
| `hame-agent` | Vòng turn/step, hook, checkpoint |
| `hame-sandbox` | Chính sách, Seatbelt (macOS), Landlock (Linux), mức toàn vẹn (Windows), worker, proxy egress |
| `hame-policy` | Cấu hình, luật quyền, hỏi phép, che secret |
| `hame-checkpoint` | Checkpoint workspace bằng kho git riêng |
| `hame-context` | Tràn đầu ra ra file, nén ngữ cảnh, nạp chỉ dẫn dự án |
| `hame-verify` | Cổng kiểm chứng trước khi kết thúc lượt; phát hiện lặp; leo thang model |
| `hame-ext` | MCP, chẩn đoán LSP, skills, hooks, plugin WebAssembly |
| `hame-multi` | Subagent, worktree, bảng việc, ngân sách chung |
| `hame-app` | Lắp một agent hoàn chỉnh từ cấu hình; nhập session |
| `hame-acp` | Máy chủ Agent Client Protocol; giao diện terminal cũng là một client của nó |
| `hame-tui`, `hame-web`, `hame-cli` | Các giao diện |
| `hame-eval` | Model giả phát lại bản ghi, cho test và eval |

## Phát triển

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Test của plugin cần `rustup target add wasm32-unknown-unknown`; một số test cần `python3` và `git`. Đặt `HAME_SCRIPT=<file.jsonl>` để `hame` phát lại phản hồi đã ghi thay vì gọi nhà cung cấp.
