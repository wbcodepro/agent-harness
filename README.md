# zcode

`zcode` là một agent harness viết bằng Rust: một binary duy nhất chạy mô hình ngôn ngữ như một agent lập trình trong terminal, trong editor (ACP) hoặc trong trình duyệt.

Ba ý chính của thiết kế:

- **Log là nguồn sự thật.** Mỗi session là một file JSONL chỉ ghi thêm. Mọi thứ model nhìn thấy (prompt hệ thống, danh sách tool, tin nhắn) đều được dựng lại từ log, nên resume, fork, rewind, nén ngữ cảnh và phát lại đều dùng chung một cơ chế.
- **Tool không chạy trong tiến trình của harness.** Mỗi lần gọi tool chạy trong một tiến trình worker bị hệ điều hành giam (Seatbelt trên macOS, Landlock trên Linux). Không giam được thì từ chối chạy.
- **Mở rộng mà không nới quyền.** Plugin là WebAssembly và chỉ đọc được nơi bạn cấp; server MCP chạy trong cùng sandbox với tool.

## Cài đặt

Cần Rust 1.89 trở lên và `git`.

```sh
cargo build --release -p zcode-cli      # tạo target/release/zcode
```

Đặt API key vào biến môi trường hoặc file `~/.zcode/.env` (hoặc `.env` trong thư mục làm việc):

```sh
DEEPSEEK_API_KEY=...     # hoặc ANTHROPIC_API_KEY / CLAUDE_API_KEY, OPENAI_API_KEY
```

## Dùng

```sh
zcode                                  # giao diện terminal
zcode run "sửa lỗi làm test_stats.py thất bại"     # một lượt, không tương tác
zcode run --session <id> "tiếp tục"    # nối tiếp một session
zcode run --plan "..."                 # chỉ khảo sát và đề xuất kế hoạch, không sửa gì
zcode run --output json "..."          # một đối tượng JSON khi xong (stream-json: thêm từng sự kiện một dòng)
zcode web                              # giao diện web trên 127.0.0.1
zcode acp                              # máy chủ Agent Client Protocol cho editor
zcode sessions | show <id> | fork <id> | rewind <id> | recover <id>
zcode import dsh <session.jsonl>       # nhập session của DeepSeek Harness (v2–v4)
zcode eval evals/harder                # chạy bộ tác vụ, báo tỷ lệ đạt, thời gian, token (--keep <thư mục>: giữ log của tác vụ hỏng; --runs N: chạy N lần, báo tỷ lệ đạt từng tác vụ)
zcode eval evals/harder --command 'codex exec "$ZCODE_EVAL_PROMPT"'   # đo một agent khác trên cùng bộ
zcode skill install <thư mục | URL git> [--path thư/mục/con]   # quét rồi cài một skill vào ~/.zcode/skills
zcode skill list
zcode probe evals/harder --model <tên> # thử model với từng định dạng sửa file và bộ tool, in cấu hình tốt nhất
```

Chọn model bằng `--provider deepseek|openai|anthropic` và `--model <tên>`, hoặc `ZCODE_PROVIDER` / `ZCODE_MODEL`. Mặc định: `deepseek-flash` với DeepSeek, `claude-opus-5-5` với Anthropic; OpenAI cần chỉ rõ model.

Cuối mỗi lần `zcode run`, một dòng tổng kết in ra stderr: số token gửi đi, bao nhiêu phần trăm được đọc từ cache của nhà cung cấp, số token nhận về và số request. Nếu phần lớn prompt không được dùng lại giữa các request, zcode cảnh báo.

Trong giao diện terminal: `/plan`, `/rewind`, `/new`, `/quit`; Esc dừng lượt đang chạy. Gõ rồi Enter trong lúc một lượt đang chạy sẽ gửi lời nhắn đó cho chính lượt ấy: model nhận nó ở request kế tiếp, không phải đợi lượt kết thúc (qua ACP là thông báo mở rộng `_zcode/steer`).

### Trong editor (ACP)

`zcode acp` nói Agent Client Protocol qua stdin/stdout. Với Zed, thêm vào `settings.json` (theo tài liệu của Zed; cấu hình này chưa được thử trong Zed):

```json
{ "agent_servers": { "zcode": { "command": "/đường/dẫn/tới/zcode", "args": ["acp"] } } }
```

Máy chủ được kiểm thử bằng runtime client chính thức của giao thức (`agent-client-protocol`), gồm cả luồng xin phép.

## Tool có sẵn

`read`, `write`, `edit`, `bash`, `glob`, `grep`, `repo_map`, `subagent`, `task_create` / `task_update` / `task_list`; thêm `skill` khi có skill và `diagnostics` khi có cấu hình language server. Với `edit_format = "apply-patch"`, tool `apply_patch` thay cho `edit` và `write`.

`write` từ chối ghi đè một file đã tồn tại mà model chưa đọc trong phiên (nó phải đọc trước, vì nội dung file thường là thứ đề bài phụ thuộc vào); khi ghi làm file nhỏ đi quá nửa, kết quả nói rõ là cả file đã bị thay. `edit` không tìm thấy đoạn cần thay thì chỉ ra khớp tới dòng nào và khác ở đâu. `grep` ghi kèm tên hàm hoặc lớp bao quanh mỗi dòng khớp. Sau mỗi lần `edit`, `write` hay `apply_patch`, nếu file nguồn đang hợp lệ trở nên sai cú pháp thì kết quả báo ngay dòng lỗi (Python được kiểm bằng `python3` khi có; Rust, JS, TS, Go bằng tree-sitter).

`edit` và `apply_patch` chấp nhận sai lệch về khoảng trắng, thụt lề và kiểu xuống dòng khi chỉ có đúng một chỗ khớp; dòng không đổi luôn giữ nguyên như trong file. Tham số gọi tool bị bọc trong code fence hoặc lẫn chữ được gỡ ra trước khi chạy.

Khi tool từ server MCP và plugin nhiều hơn `defer_tools_over`, chúng không nằm trong danh sách tool gửi cho model (mỗi tool khai báo tốn token ở mọi request). Model tìm bằng `tool_search` và chạy bằng `tool_call`; luật quyền và hook vẫn thấy tên thật của tool được gọi.

Các lời gọi chỉ đọc mà model đưa ra cùng lúc được chạy song song. `subagent` giao việc cho một agent mới có ngữ cảnh riêng; với `isolate` nó làm trên một git worktree và nhánh riêng.

## An toàn

Mặc định (`workspace-write`):

- Chỉ ghi được trong thư mục làm việc và thư mục tạm.
- Không đọc được `~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.config/gh`…, dữ liệu riêng của zcode, và các file `.env`.
- Mạng chỉ đi qua proxy cục bộ tới danh sách tên miền cho phép.
- Biến môi trường của harness không tới được tool; secret chỉ tới khi bạn cấp (`[secrets] grant`), và giá trị của mọi secret bị thay bằng `[secret:TÊN]` trong đầu ra tool trước khi ghi log.
- Trước mỗi lượt và sau mỗi bước có thay đổi, workspace được lưu vào một kho git riêng ngoài dự án; `zcode rewind` khôi phục file và tạo session mới dừng ngay trước lượt đó.
- Thay đổi mà một lần gọi tool gây ra cho `.git/hooks` và `.git/config` của repo ở thư mục làm việc bị hoàn tác ngay sau lần gọi đó và model được báo (`[sandbox] protect_git = false` để tắt). Đây là phát hiện rồi hoàn tác, không phải ngăn chặn: hook vẫn tồn tại trong lúc lệnh đang chạy. Repo do chính lần gọi đó tạo ra (`git init`) không được bảo vệ.
- Luật quyền cho `bash` được so với từng lệnh trong dòng (các phần ngăn bởi `;`, `&&`, `||`, `|`, phần trong `$(...)` và backtick, lệnh sau `sudo`/`env`…): luật `deny`/`ask` có hiệu lực dù lệnh nằm ở đâu trong dòng, còn luật `allow` chỉ áp dụng khi mọi lệnh trong dòng đều được cho phép. Bộ tách lệnh không phải là một trình phân tích shell đầy đủ.
- Với `[approval] reviewer = true`, một model thứ hai xem từng lời gọi cần xin phép (chỉ thấy tên tool, tham số và thư mục làm việc, không thấy hội thoại) và chỉ tự đồng ý khi lời gọi rõ ràng vô hại; mọi trường hợp khác, kể cả khi nó trả lời không rõ, vẫn được hỏi bạn. Mọi phán quyết được ghi vào `~/.zcode/approvals.jsonl`. Đây là một model, có thể phán sai: nó giảm số lần bị hỏi, không thay cho sandbox.

`--sandbox read-only` cấm ghi; `--sandbox danger-full-access` bỏ giam, khi đó mọi tool không chỉ-đọc đều phải được bạn đồng ý.

Giới hạn cần biết:

- Thư mục tạm ghi được theo mặc định (`[sandbox] writable_tmp = false` để tắt). Dự án nằm trong `/tmp` sẽ ghi được sang các thư mục anh em.
- Trên Linux, Landlock không ẩn được file theo tên: `.env` trong workspace vẫn đọc được (giá trị vẫn bị che trong log); thư mục cha của một đường dẫn bị ẩn không liệt kê được; giới hạn mạng theo cổng TCP, UDP không bị chặn. Cần Linux 6.7 trở lên.
- Windows chỉ giam được việc **ghi**: tiến trình chạy tool bị hạ xuống mức toàn vẹn Low nên chỉ ghi được trong thư mục dự án và một thư mục tạm riêng (`AppData\LocalLow\zcode-tmp`). Tool vẫn đọc được mọi file bạn đọc được và vẫn ra mạng được, nên phải bật rõ ràng bằng `allow_partial = true` trong `[sandbox]`; không bật thì các chế độ giam từ chối khởi động. Thư mục dự án được gắn nhãn Low vĩnh viễn (chương trình Low khác của cùng người dùng cũng ghi được vào đó). Phần Windows mới chỉ được kiểm trên máy của GitHub Actions (kể cả lệnh chạy qua tool `bash`), chưa trên một máy Windows có người dùng thật; xem `docs/WINDOWS.md`.
- Phần mềm này chưa qua kiểm toán bảo mật. Với mã không tin cậy, hãy chạy trong máy ảo hoặc container.

## Cấu hình

`~/.zcode/config.toml`. Mọi mục đều tùy chọn; khóa lạ là lỗi.

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
temperature = 0.0               # chỉ với API chat-completions (DeepSeek, máy chủ tương thích, model chạy tại chỗ)
escalate_to = "claude-fable-5-2"     # model mạnh hơn tiếp quản phần còn lại của lượt khi model chính kẹt
escalate_effort = "high"
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
grant = ["GITHUB_TOKEN"]

[context]
compact_at_tokens = 100000      # tóm tắt phần lịch sử cũ khi vượt ngưỡng
clear_tool_results = false      # true: vượt ngưỡng thì xóa kết quả tool cũ trước, chưa đủ mới tóm tắt
spill_at_bytes = 16000          # đầu ra dài hơn thì lưu ra file, chỉ xem trước
defer_tools_over = 20           # nhiều tool MCP/plugin hơn số này thì không khai báo cho model;
                                # model tìm bằng tool_search và chạy bằng tool_call

[verify]
commands = ["cargo test -q"]    # phải đạt trước khi một lượt có sửa file được kết thúc
checklist = false               # true: trước khi kết thúc, yêu cầu model rà lại từng điểm của đề bài
stop_on_red = true              # lệnh test/build cuối cùng model chạy bị hỏng mà nó định dừng: nhắc nó làm tiếp (tối đa 2 lần mỗi lượt)

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

MCP: server stdio chạy trong sandbox; server HTTP được gọi từ tiến trình zcode, xác thực bằng header tĩnh. Chưa hỗ trợ luồng đăng nhập OAuth, bản đặc tả không trạng thái `2026-07-28`, và yêu cầu do server khởi xướng (sampling, elicitation).

Hooks của Claude Code: zcode đọc cùng file `settings.json` và nói cùng giao thức (JSON qua stdin, mã thoát 2 để chặn, JSON trên stdout) cho các sự kiện `PreToolUse`, `PostToolUse`, `UserPromptSubmit` và `Stop`, với hook loại `command`, `matcher` và `if`. Tên tool và tham số được đổi sang dạng Claude Code (`Bash`, `Edit`, `file_path`…). Sự kiện và loại hook khác được báo một lần lúc khởi động rồi bỏ qua. Khác biệt cần biết: hook chạy lần lượt chứ không song song; `permissionDecision: "ask"` bị coi là chặn; `"allow"` không bỏ qua luật quyền của zcode; `tool_response` chỉ có trường `output`; `transcript_path` trỏ tới log của zcode, không cùng định dạng với Claude Code. Hook chạy ngoài sandbox, nên chỉ bật `"project"` với repo bạn tin.

Plugin của Claude Code: với mỗi thư mục trong `claude_plugins`, zcode nạp `skills/`, `hooks/hooks.json` (kể cả biến `${CLAUDE_PLUGIN_ROOT}`) và các server trong `.mcp.json` (stdio và HTTP; tool mang tên `mcp__<plugin>_<server>__<tool>`). Lệnh (`commands/`), định nghĩa subagent (`agents/`) và marketplace chưa được hỗ trợ.

Dự án có thể tự khai báo lệnh kiểm tra trong `.zcode/config.toml` (chỉ mục `[verify]`; một repo không được phép nới sandbox). `AGENTS.md` và `CLAUDE.md` của dự án, cùng `~/.zcode/AGENTS.md`, được nối vào prompt hệ thống. Skill đặt ở `.zcode/skills/<tên>/SKILL.md`, `.claude/skills/` hoặc `~/.zcode/skills/`.

`zcode skill install` đọc mọi file của skill trước khi chép: ký tự vô hình, câu bảo model bỏ chỉ dẫn hoặc giấu người dùng, lệnh tải về rồi đưa thẳng vào shell, mã ẩn trong base64, đường dẫn tới khóa và file bí mật, symlink. Thấy dấu hiệu nguy hiểm thì không cài, trừ khi bạn thêm `--force`. Đây là so mẫu, không phải chứng minh: quét sạch nghĩa là không thấy các mánh đã biết, không có nghĩa skill an toàn.

Plugin mẫu và mô tả ABI: `plugins/wordcount`.

## Cấu trúc mã

| Crate | Việc |
|---|---|
| `zcode-kernel` | Registry service, bus sự kiện, chuỗi middleware |
| `zcode-session` | Schema sự kiện, kho JSONL, khôi phục sau crash, fork, projection |
| `zcode-llm` | Adapter OpenAI Responses, Anthropic Messages và chat-completions (DeepSeek, máy chủ tương thích) |
| `zcode-tools` | Tool có sẵn và trait `Tool` |
| `zcode-agent` | Vòng turn/step, hook, checkpoint |
| `zcode-sandbox` | Chính sách, Seatbelt (macOS), Landlock (Linux), mức toàn vẹn (Windows), worker, proxy egress |
| `zcode-policy` | Cấu hình, luật quyền, hỏi phép, che secret |
| `zcode-checkpoint` | Checkpoint workspace bằng kho git riêng |
| `zcode-context` | Tràn đầu ra ra file, nén ngữ cảnh, nạp chỉ dẫn dự án |
| `zcode-verify` | Cổng kiểm chứng trước khi kết thúc lượt; phát hiện lặp; leo thang model |
| `zcode-ext` | MCP, chẩn đoán LSP, skills, hooks, plugin WebAssembly |
| `zcode-multi` | Subagent, worktree, bảng việc, ngân sách chung |
| `zcode-app` | Lắp một agent hoàn chỉnh từ cấu hình; nhập session |
| `zcode-acp` | Máy chủ Agent Client Protocol; giao diện terminal cũng là một client của nó |
| `zcode-tui`, `zcode-web`, `zcode-cli` | Các giao diện |
| `zcode-eval` | Model giả phát lại bản ghi, cho test và eval |

## Phát triển

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Test của plugin cần `rustup target add wasm32-unknown-unknown`; một số test cần `python3` và `git`. Đặt `ZCODE_SCRIPT=<file.jsonl>` để `zcode` phát lại phản hồi đã ghi thay vì gọi nhà cung cấp.
