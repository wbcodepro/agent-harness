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
zcode web                              # giao diện web trên 127.0.0.1
zcode acp                              # máy chủ Agent Client Protocol cho editor
zcode sessions | show <id> | fork <id> | rewind <id> | recover <id>
zcode import dsh <session.jsonl>       # nhập session của DeepSeek Harness (v2–v4)
zcode eval evals/harder                # chạy bộ tác vụ, báo tỷ lệ đạt, thời gian, token
zcode eval evals/harder --command 'codex exec "$ZCODE_EVAL_PROMPT"'   # đo một agent khác trên cùng bộ
```

Chọn model bằng `--provider deepseek|openai|anthropic` và `--model <tên>`, hoặc `ZCODE_PROVIDER` / `ZCODE_MODEL`. Mặc định: `deepseek-flash` với DeepSeek, `claude-opus-5-5` với Anthropic; OpenAI cần chỉ rõ model.

Trong giao diện terminal: `/plan`, `/rewind`, `/new`, `/quit`; Esc dừng lượt đang chạy.

### Trong editor (ACP)

`zcode acp` nói Agent Client Protocol qua stdin/stdout. Với Zed, thêm vào `settings.json` (theo tài liệu của Zed; cấu hình này chưa được thử trong Zed):

```json
{ "agent_servers": { "zcode": { "command": "/đường/dẫn/tới/zcode", "args": ["acp"] } } }
```

Máy chủ được kiểm thử bằng runtime client chính thức của giao thức (`agent-client-protocol`), gồm cả luồng xin phép.

## Tool có sẵn

`read`, `write`, `edit`, `bash`, `glob`, `grep`, `repo_map`, `subagent`, `task_create` / `task_update` / `task_list`; thêm `skill` khi có skill và `diagnostics` khi có cấu hình language server.

Các lời gọi chỉ đọc mà model đưa ra cùng lúc được chạy song song. `subagent` giao việc cho một agent mới có ngữ cảnh riêng; với `isolate` nó làm trên một git worktree và nhánh riêng.

## An toàn

Mặc định (`workspace-write`):

- Chỉ ghi được trong thư mục làm việc và thư mục tạm.
- Không đọc được `~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.config/gh`…, dữ liệu riêng của zcode, và các file `.env`.
- Mạng chỉ đi qua proxy cục bộ tới danh sách tên miền cho phép.
- Biến môi trường của harness không tới được tool; secret chỉ tới khi bạn cấp (`[secrets] grant`), và giá trị của mọi secret bị thay bằng `[secret:TÊN]` trong đầu ra tool trước khi ghi log.
- Trước mỗi lượt và sau mỗi bước có thay đổi, workspace được lưu vào một kho git riêng ngoài dự án; `zcode rewind` khôi phục file và tạo session mới dừng ngay trước lượt đó.

`--sandbox read-only` cấm ghi; `--sandbox danger-full-access` bỏ giam, khi đó mọi tool không chỉ-đọc đều phải được bạn đồng ý.

Giới hạn cần biết:

- Thư mục tạm ghi được theo mặc định (`[sandbox] writable_tmp = false` để tắt). Dự án nằm trong `/tmp` sẽ ghi được sang các thư mục anh em.
- Trên Linux, Landlock không ẩn được file theo tên: `.env` trong workspace vẫn đọc được (giá trị vẫn bị che trong log); thư mục cha của một đường dẫn bị ẩn không liệt kê được; giới hạn mạng theo cổng TCP, UDP không bị chặn. Cần Linux 6.7 trở lên.
- Windows chỉ giam được việc **ghi**: tiến trình chạy tool bị hạ xuống mức toàn vẹn Low nên chỉ ghi được trong thư mục dự án và một thư mục tạm riêng (`AppData\LocalLow\zcode-tmp`). Tool vẫn đọc được mọi file bạn đọc được và vẫn ra mạng được, nên phải bật rõ ràng bằng `allow_partial = true` trong `[sandbox]`; không bật thì các chế độ giam từ chối khởi động. Thư mục dự án được gắn nhãn Low vĩnh viễn (chương trình Low khác của cùng người dùng cũng ghi được vào đó). Lệnh chạy qua tool `bash` trên Windows chưa được kiểm thử.
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

[sandbox]
mode = "workspace-write"        # read-only | workspace-write | danger-full-access
allow_partial = false           # Windows: chấp nhận chỉ giam việc ghi
writable = ["/data/cache"]
unreadable = ["/data/private"]

[network]
mode = "allowlist"              # off | allowlist | open
allow = ["github.com", "*.crates.io"]

[[rule]]                        # luật đầu tiên khớp sẽ thắng
tool = "bash"
pattern = "git push*"
action = "ask"                  # allow | ask | deny

[secrets]
grant = ["GITHUB_TOKEN"]

[context]
compact_at_tokens = 100000      # tóm tắt phần lịch sử cũ khi vượt ngưỡng
spill_at_bytes = 16000          # đầu ra dài hơn thì lưu ra file, chỉ xem trước

[verify]
commands = ["cargo test -q"]    # phải đạt trước khi một lượt có sửa file được kết thúc

[limits]
tree_tokens = 2000000           # ngân sách token cho agent và mọi subagent; 0 = không giới hạn
subagent_depth = 1

[mcp.docs]                      # server MCP (stdio); mặc định chạy trong sandbox
command = "docs-server"
args = ["--stdio"]

[lsp.rust]                      # language server cho tool `diagnostics`
command = "rust-analyzer"
extensions = ["rs"]

[[hook]]                        # lệnh chạy quanh mỗi lần gọi tool; thoát mã 2 = phản đối
event = "pre_tool"
tool = "bash"
command = "./scripts/check-command.sh"

[plugin.wc]                     # plugin WebAssembly và nơi nó được đọc
path = "plugins/wordcount.wasm"
read = ["docs"]
```

Dự án có thể tự khai báo lệnh kiểm tra trong `.zcode/config.toml` (chỉ mục `[verify]`; một repo không được phép nới sandbox). `AGENTS.md` và `CLAUDE.md` của dự án, cùng `~/.zcode/AGENTS.md`, được nối vào prompt hệ thống. Skill đặt ở `.zcode/skills/<tên>/SKILL.md`, `.claude/skills/` hoặc `~/.zcode/skills/`.

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
| `zcode-verify` | Cổng kiểm chứng trước khi kết thúc lượt |
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
