# Báo cáo thị trường agent lập trình và hệ quả cho zcode

Ngày 2026-11-14. Tổng hợp từ bảy báo cáo nghiên cứu trong cùng thư mục, mỗi báo cáo do một agent riêng thực hiện bằng nguồn gốc (repo, tài liệu chính thức, `gh`, binary cài trên máy) và có dẫn nguồn từng ý:

| File | Nội dung |
|---|---|
| `codex-cli.md` | OpenAI Codex CLI: kiến trúc, sandbox, approval, tool, ngữ cảnh, mở rộng |
| `claude-code.md` | Claude Code: bề mặt, quyền, tool, ngữ cảnh, hooks/skills/subagent, tự động hóa |
| `other-harnesses.md` | 24 harness khác, bảng tính năng, xu hướng |
| `github-ecosystem.md` | Hệ sinh thái công cụ quanh agent trên GitHub, 12 nhóm |
| `techniques.md` | Kỹ thuật thiết kế agent do chính các đội xây dựng công bố |
| `token-optimisation.md` | Tối ưu token: kỹ thuật, số đo, bảng giá cache từng nhà cung cấp |
| `weak-model-parity.md` | Giúp model cũ/nhỏ làm chính xác việc khó |

Số liệu trong các báo cáo là của nguồn được dẫn; mục nào không kiểm chứng được đều ghi "unverified" trong file gốc.

## 1. Thị trường đang ở đâu

- **Quy mô.** DeepSeek Harness 280k sao sau ba tháng, OpenCode 227k, Claude Code 155k (repo không mở mã), Codex 135k, Pi 122k, Gemini CLI 109k. Aider, Roo Code, Continue, Plandex đã ngừng hoặc gần như ngừng.
- **Sáu xu hướng** (mỗi xu hướng có ít nhất hai ví dụ trong `other-harnesses.md`):
  1. Harness là máy chủ, giao diện là client (Kiro, OpenCode v2, Crush, Qwen, Codex app-server).
  2. ACP thắng ở ranh giới editor: 51 agent đăng ký.
  3. Sandbox cấp kernel cộng với model duyệt lệnh thay cho hỏi phép liên tục.
  4. Lõi tối giản, mở rộng được thắng lõi nhiều tính năng (Pi, DeepSeek Harness).
  5. Liền mạch từ máy cá nhân lên cloud, chạy cả đội agent.
  6. Rust và binary đơn cho CLI (Grok Build, Goose, Warp, ForgeCode).
- **Tính năng mặc định phải có cuối 2026:** MCP (kèm OAuth), `AGENTS.md`, `SKILL.md`, hooks tương thích Claude Code, subagent, plan mode, luật allow/ask/deny, tự nén ngữ cảnh, chế độ headless xuất JSON, máy chủ ACP, resume/fork, gói plugin, worktree, nhập cấu hình từ agent khác.

## 2. Hai đối thủ chính

**Codex CLI** (bản 0.169, khoảng 130 crate Rust)
- Mọi giao diện đều là client của một app-server JSON-RPC (Thread/Turn/Item), có sinh schema.
- Sandbox: Seatbelt trên macOS, bubblewrap cộng seccomp trên Linux; Windows có ba backend, trong đó bản cũ phải tạo tài khoản ẩn và luật tường lửa bằng quyền admin.
- Hồ sơ quyền ghép được theo đường dẫn và tên miền; `.git`, `.codex` luôn chỉ đọc kể cả trong vùng ghi được.
- `apply_patch` là tool dạng tự do bị ràng buộc bằng ngữ pháp; danh mục model quyết định biến thể tool cho từng model.
- Agent duyệt lệnh ("auto-review") thất bại thì đóng, có cầu dao khi bị từ chối liên tiếp.
- Điểm yếu: không còn undo hay checkpoint (yêu cầu khôi phục có 498 lượt ủng hộ), 9.521 issue mở gắn nhãn Windows, chỉ nói Responses API, nén ngữ cảnh mất thông tin.

**Claude Code** (bản 2.1.323)
- Một binary, nhiều mặt tiền; SDK chính là binary đó.
- Bố cục prompt đặt cache lên hàng đầu: chỉ nối thêm, chụp prompt hệ thống theo hội thoại, hoãn nạp schema tool, fork dùng chung cache của cha.
- "Auto mode": luật tất định trước, rồi một model phân loại không nhìn kết quả tool (dương tính giả 0,4%, bỏ sót 17% hành động quá tay theo bài kỹ thuật của Anthropic).
- Sandbox bash có proxy theo danh sách tên miền và thay thế secret tại proxy; Windows gốc không có sandbox.
- 33 sự kiện hook, skills, subagent, workflow tất định, plugin, mods.
- Điểm yếu: mã đóng, chỉ model Anthropic, hạn mức mờ, checkpoint bỏ qua thay đổi do bash và subagent, sáu cơ chế điều phối chồng chéo.

## 3. Người dùng đang tự vá những gì

Xếp theo mức hoạt động trên GitHub (`github-ecosystem.md`):

1. Uốn hành vi và tri thức tái dùng: 42.275 repo skill tạo trong 2026.
2. Chi phí token và phình ngữ cảnh.
3. Hiểu codebase và nhớ qua phiên.
4. Chạy nhiều agent cùng lúc và từ xa.
5. Kỷ luật quy trình (spec, kế hoạch sống sót qua nén).
6. Nhìn thấy ứng dụng đang chạy (trình duyệt).
7. Quan sát chi phí và ngữ cảnh.
8. Review độc lập.
9. Cách ly và an toàn chuỗi cung ứng: sao thấp so với rủi ro, tức đang bị bỏ ngỏ.
10. Đánh giá harness và skill: ít chú ý nhất, có thể là điểm khác biệt.

Cảnh báo: một báo cáo quét 31.132 skill thấy 26,1% có lỗ hổng và 5,2% có dấu hiệu độc hại (số do README của công cụ quét công bố).

## 4. Tối ưu token: cái gì có bằng chứng

**Có bằng chứng mạnh**
- Giữ tiền tố ổn định để trúng cache: không có cache thì chi phí gấp 1,7 đến 7,3 lần (Anthropic đo nội bộ). Vòng lặp khỏe đọc khoảng 84% đầu vào từ cache.
- Sửa prompt hệ thống ở 100K ngữ cảnh đắt gấp 25 đến 50 lần gửi cùng nội dung như một tin nhắn.
- Che kết quả tool cũ (không tóm tắt) giảm nửa chi phí và giữ nguyên tỷ lệ giải (SWE-bench Verified, năm cấu hình model).
- Hoãn nạp định nghĩa tool: giảm khoảng 85% token định nghĩa; Cursor đo giảm 46,9% tổng token ở các lượt có dùng MCP.
- Điều khiển mức suy luận: Opus 5.5 ở mức low giải 87,4% với 0,11 USD/tác vụ, mức high 95,3% với 0,29 USD. Chạy low trước rồi chạy lại phần hỏng ở high đạt khoảng 97% với 0,17 USD.

**Phổ biến nhưng bằng chứng yếu hoặc âm**
- Bộ lọc đầu ra lệnh kiểu `rtk`: phép đo độc lập duy nhất cho kết quả từ giảm 5% đến tăng 17% chi phí, tỷ lệ đạt giảm nhẹ.
- Nén prompt kiểu LLMLingua: chi phí ròng tăng, độ chính xác sụp, và phá cache.
- Đổi model giữa hội thoại: mất cache, có thể đắt hơn để model lớn trả lời.
- Đội nhiều agent cho việc lập trình: 7 đến 15 lần token.
- Giới hạn `max_tokens` chặt: không cải thiện chi phí trên mỗi tác vụ giải được.

DeepSeek đáng chú ý: token trúng cache chỉ bằng 2 đến 3% giá token trượt, nên tiền tố ổn định quan trọng ở đây hơn bất kỳ nhà cung cấp nào.

## 5. Model cũ hoặc nhỏ làm việc khó: harness kéo được bao nhiêu

- Harness ảnh hưởng tới model yếu nhiều hơn: Haiku 4.5 trên Terminal-Bench 2.0 dao động từ 13,9% đến 37,6% tùy harness (gấp 2,7 lần), Opus 4.6 từ 58,0% đến 81,8% (gấp 1,4 lần).
- Phần harness lấy lại được là các lỗi cơ học: sai định dạng sửa file, gọi tool hỏng, thiếu ngữ cảnh, dừng sai lúc.
  - Định dạng sửa file theo từng model: Qwen2.5-Coder-32B đạt 16,4% khi ghi cả file và 8,0% khi dùng diff. Tắt cơ chế áp dụng "khoan dung" làm lỗi sửa tăng 9 lần (Aider).
  - Sửa parser và tham số gọi tool: 34 lời gọi bị rơi giảm còn 1.
  - Cấp đủ đề bài: 11% lên 44%.
  - Quy trình cố định (định vị, sửa, kiểm): GPT-4o từ 18,33% lên 32,00% với 28% chi phí.
- Giới hạn: trên việc khó, harness đóng được khoảng một phần tư đến một nửa khoảng cách. Model cỡ nano vẫn ở 3 đến 12% dưới mọi harness đã thử. Phần còn lại cần model mạnh hơn, hoặc leo thang sang model mạnh do harness quyết định khi nào.
- Khuôn nặng làm hại model mạnh, nên cần hồ sơ năng lực theo từng model.

## 6. zcode đang đứng ở đâu

**Đã có, và là lợi thế so với thị trường**
- Checkpoint bằng kho git riêng bắt được cả thay đổi do bash: Codex không có undo, Claude Code bỏ qua thay đổi của bash.
- Sandbox trên cả ba hệ điều hành mà không cần quyền admin hay tài khoản ẩn (Windows mới giam được việc ghi).
- Log sự kiện chỉ ghi thêm, khôi phục sau crash, phát lại tất định.
- Ba giao thức model: OpenAI Responses, Anthropic Messages, chat-completions.
- Plugin WebAssembly chỉ đọc được nơi được cấp; server MCP chạy trong sandbox.
- ACP, MCP (stdio), skills, plan mode, subagent trên worktree, chẩn đoán LSP, cổng kiểm chứng, repo map tree-sitter, `zcode eval`.

**Thiếu so với mức mặc định của thị trường**
- Hooks tương thích Claude Code: zcode mới có 2 sự kiện, định dạng riêng.
- Đầu ra JSON cho chế độ headless.
- MCP qua HTTP kèm OAuth; bản đặc tả MCP 2026-07-28.
- Hoãn nạp tool MCP (tool search).
- Chèn tin nhắn khi lượt đang chạy (steering); lệnh chạy nền và PTY.
- Luật quyền hiểu cú pháp shell (tách lệnh ghép); bảo vệ `.git` trong vùng ghi được.
- Tool web; nhập cấu hình từ agent khác; bộ nhớ qua phiên.

## 7. Đề xuất thứ tự làm tiếp

Xếp theo bằng chứng về tác động trên mỗi đơn vị công sức.

**Đợt 1: token và model yếu (hai trọng tâm bạn nêu)**
1. Đo và báo tỷ lệ trúng cache theo phiên; kiểm tra tiền tố ổn định cho từng nhà cung cấp; đặt khóa cache cho OpenAI.
2. Che kết quả tool cũ trước khi tóm tắt, theo lô lớn và thưa.
3. Hồ sơ năng lực theo model: định dạng sửa file (`apply_patch` cho GPT, search/replace mặc định, cả file cho model nhỏ), áp dụng khoan dung, sửa tham số gọi tool, giới hạn số lời gọi mỗi lượt, bộ tool rút gọn, tham số lấy mẫu.
4. Bộ thăm dò đo hồ sơ đó cho một model bất kỳ (đưa vào `zcode eval`).
5. Leo thang khi thất bại: mức suy luận thấp trước, hỏng thì cao; model rẻ trước, hỏng thì model mạnh, chỉ đổi ở ranh giới phiên hoặc subagent.
6. Danh sách kiểm trước khi kết thúc và phát hiện lặp.

Trạng thái đợt 1 (2026-11-14), đã có trong mã:

| Mục | Đã làm | Chưa làm |
|---|---|---|
| 1 | Dòng tổng kết token và tỷ lệ cache cuối `zcode run`, cảnh báo khi cache nguội; `prompt_cache_key` theo session cho OpenAI Responses | Kiểm tra tiền tố ổn định riêng cho từng nhà cung cấp |
| 2 | Sự kiện `context/masked`: xóa kết quả tool cũ khỏi ngữ cảnh theo lô lớn, thử trước khi tóm tắt, không gọi model. **Mặc định tắt** (`[context] clear_tool_results`), xem số đo bên dưới | Đo trên phiên dài thật với ngưỡng mặc định |
| 3 | `[model] tools`, `edit_format`, `max_tool_calls`; tool `apply_patch`; `edit` khoan dung với khoảng trắng; sửa tham số gọi tool | Tham số lấy mẫu theo model |
| 4 | `zcode probe` | Bộ tác vụ đủ khó để phân biệt các cấu hình |
| 5 | `escalate_to` / `escalate_effort` / `escalate_after`: model mạnh tiếp quản phần còn lại của lượt, ghi sự kiện `model/switched` | Leo thang riêng theo mức suy luận trên cùng model; chưa chạy với hai model thật |
| 6 | `[verify] checklist`; ghi chú khi lời gọi tool lặp y hệt ba lần | — |

Số đo của đợt 1 với model thật (`deepseek-flash`, mỗi cấu hình một hoặc hai lần chạy, nên chỉ là chỉ dấu):

- `zcode probe evals/harder`: cả bốn cấu hình đạt 8/8. Token cả bộ: search-replace 186.772; whole-file 163.649; apply-patch 168.714; whole-file với bộ tool rút gọn 116.937 (ít hơn khoảng 29% so với cùng định dạng với bộ tool đầy đủ).
- Xóa kết quả tool cũ, trên một tác vụ phải đọc tám file lớn rồi cộng số liệu, ngưỡng hạ xuống 14.000 token để ép kích hoạt: bật thì 133.642 và 197.482 token (14 và 19 request); tắt (chỉ tóm tắt) thì 153.858 và 90.033 token (14 và 10 request); không giới hạn ngữ cảnh thì 97.567 token (9 request). Mọi lần đều ra đáp án đúng. Model không ghi lại số liệu trước khi kết quả bị xóa nên phải đọc lại file. Kết quả này không ủng hộ việc bật mặc định, trái với kết luận của các nghiên cứu ở mục 4; tác vụ này bất lợi cho kỹ thuật (cần mọi kết quả cũ, ngưỡng rất thấp), nên cần đo lại trên phiên dài thật trước khi kết luận.
- Leo thang: với `claude-haiku-5-5` leo lên `claude-opus-5-5`, sau hai lần lệnh kiểm tra hỏng thì model mạnh tiếp quản giữa lượt, API không báo lỗi về khối suy luận.
- `prompt_cache_key` được OpenAI Responses chấp nhận; một tác vụ bảy request có 77% token đầu vào đọc từ cache (không có số đo trước khi thêm khóa để so).

### Model yếu: đo bằng `gpt-4o-mini`, sửa theo log thất bại

`deepseek-flash` đạt hết bộ `evals/harder` nên không cho biết gì. `gpt-4o-mini` (model cũ, nhỏ) thì không, nên nó được dùng làm thước đo. Cách làm: chạy `zcode probe evals/harder --model gpt-4o-mini --keep <thư mục>`, đọc `session.v1.jsonl` của từng tác vụ hỏng, tìm chỗ harness đã làm model lạc hướng hoặc không ngăn nó, sửa, rồi chạy lại.

Tám tác vụ gốc của `evals/harder`, mỗi ô một lần chạy (nên sai số lớn; chỉ nên tin xu hướng chung):

| Cấu hình | Trước (commit `fc55dea`) | Sau (commit `02fb8d2`) |
|---|---|---|
| search-replace, bộ tool đầy đủ | 5/8, 929.184 token, 784 s | 8/8, 369.788 token, 434 s |
| whole-file, bộ tool đầy đủ | 4/8, 418.945 token, 501 s | 8/8, 440.584 token, 332 s |
| apply-patch, bộ tool đầy đủ | 4/8, 949.383 token, 640 s | 6/8, 980.552 token, 633 s |
| search-replace, bộ tool rút gọn | 6/8, 206.686 token, 273 s | 7/8, 375.863 token, 440 s |
| Tổng | 19/32 | 29/32 |

Trên cùng bản "sau", `deepseek-flash` và `claude-opus-5-5` vẫn đạt 8/8 (một lần chạy mỗi model), tức các thay đổi không làm model mạnh kém đi.

Những lỗi của harness tìm ra theo cách này, mỗi cái đều đã thấy trong log thật trước khi sửa:

| Hiện tượng trong log | Nguyên nhân phía harness | Sửa |
|---|---|---|
| Model trả lời `mod_37.py` thay vì `pkg/mod_37.py` | `grep`/`glob` in đường dẫn tương đối so với thư mục tìm, không dùng lại được | In đường dẫn theo thư mục làm việc |
| Model đoán tên hàm từ dòng `grep` khớp, không đọc file | `grep` không cho biết dòng khớp nằm trong hàm nào | Ghi kèm `[in <tên>, line N]` lấy từ cây cú pháp |
| Model ghi đè file test có sẵn bằng bài test tự bịa rồi sa lầy 50 bước | `write` thay cả file mà không cần đọc trước | Từ chối ghi đè file chưa đọc; báo khi file bị thu nhỏ quá nửa |
| Sửa đúng một ký tự nhưng thành `IndentationError` | Khớp lỏng giữ nguyên thụt lề sai của model khi model thụt thừa | Đưa phần thay thế về thụt lề của file theo cả hai chiều |
| Chạy test, thấy hỏng, liệt kê "bước tiếp theo" rồi dừng | Không có gì chặn lượt kết thúc khi lệnh kiểm tra cuối còn đỏ | `[verify] stop_on_red` nhắc làm tiếp, tối đa hai lần |
| Sửa file test và file dữ liệu của đề cho khớp chương trình | Prompt hệ thống không nói gì về việc này | Ba câu trong prompt hệ thống |
| `edit` báo "không tìm thấy" rồi model thử lại y nguyên | Thông báo không chỉ ra khác ở đâu | Nói khớp tới dòng nào và dòng nào khác |
| Chèn `import` bằng `old_string` rỗng bị từ chối, model chuyển sang `sed` làm hỏng file | Thông báo lỗi không chỉ cách làm đúng | Thông báo nói cách thêm dòng, kèm dòng đầu của file |
| Bản vá viết `@@ -19,7 +19,7 @@` áp vào sai chỗ | `apply_patch` coi số dòng là chữ để tìm | Dùng số dòng làm gợi ý vị trí |
| Cùng một lệnh test hỏng 18 lần liên tiếp | Chỉ phát hiện lặp khi kết quả giống hệt | Báo khi một lời gọi hỏng bốn lần chưa lần nào đạt |

Ba tác vụ thêm sau đó (`spec-hidden-tests`, `large-file-fix`, `api-change`): `deepseek-flash` đạt 3/3; `gpt-4o-mini` 0/3 trước khi sửa lỗi thụt lề, sau đó `large-file-fix` 2/2, `api-change` 1/2, `spec-hidden-tests` 0/2 (model viết một lần rồi báo xong, sót quy tắc về không dùng chung đối tượng; bật `[verify] checklist` không cứu được, 0/3).

Điều chưa làm được: `rust-tokenizer` vẫn hỏng ở hai trên bốn cấu hình và là giới hạn của model, không thấy nguyên nhân phía harness. Mọi con số ở mục này là một hoặc vài lần chạy; để kết luận chắc cần chạy lặp nhiều lần hơn.

**Đợt 2: mức mặc định của thị trường**
7. Hooks và gói plugin theo định dạng Claude Code.
8. Đầu ra JSON cho headless; steering.
9. Hoãn nạp tool MCP; MCP qua HTTP.
10. Bảo vệ `.git`; luật quyền hiểu cú pháp shell.

Trạng thái đợt 2 (2026-11-14): mục 8 có `zcode run --output json|stream-json` và steering (nhắn cho lượt đang chạy, trong giao diện terminal và qua `_zcode/steer`); mục 10 đã làm (hoàn tác thay đổi vào `.git/hooks` và `.git/config`; luật quyền so với từng lệnh trong dòng shell). Mục 7: chạy được hooks định dạng Claude Code cho bốn sự kiện (`[compat] claude_hooks`), và nạp được gói plugin (`claude_plugins`: skills, hooks, server MCP; chưa có commands, agents, marketplace). Mục 9: đã hoãn nạp tool MCP/plugin (`tool_search` + `tool_call`, `[context] defer_tools_over`); MCP qua HTTP đã có (streamable HTTP, header tĩnh; đã thử với một server công khai thật), chưa có OAuth và bản đặc tả `2026-07-28`.

Trạng thái đợt 3 (2026-11-14): mục 11 đã có (`[approval] reviewer`; với `deepseek-flash` thật nó phán đúng 7/7 lệnh thử: cho phép `ls`, ghi và đọc file trong dự án; chuyển cho người dùng lệnh xóa ngoài dự án, `curl`, `git push` và mã bị mã hóa base64). Mục 14 đã có (`zcode skill install`, quét trước khi cài; đã thử với một skill công khai thật và một skill gài bẫy). Mục 12 (thay secret tại proxy) cần proxy chặn giữa TLS nên chưa làm; mục 13 (code mode) chưa làm.

**Đợt 3: khác biệt hóa**
11. Agent duyệt lệnh chỉ cho các lệnh muốn thoát sandbox.
12. Thay thế secret tại proxy.
13. Code mode cho chuỗi lời gọi tool.
14. Cài skill kèm quét an toàn.

## 8. Số đo của zcode đến nay

Bộ `evals/harder` (8 tác vụ), ba lần chạy mỗi cấu hình, mọi lần đều đạt 8/8:

| Model | Harness | Thời gian ba lần chạy |
|---|---|---|
| `claude-opus-5-5` | zcode | 93, 87, 91 giây |
| `claude-opus-5-5` | Claude Code | 129, 127, 131 giây |
| `gpt-6.1-sol` | zcode | 139, 144, 135 giây |
| `gpt-6.1-sol` | Codex CLI | 213, 232, 225 giây |

Bộ tác vụ này quá dễ để phân biệt chất lượng (mọi bên đều đạt hết) và mức suy luận mặc định của các harness chưa được cân bằng, nên con số chỉ nói về thời gian trên việc nhỏ. Theo `techniques.md`, bước tiếp theo đúng là bộ 20 đến 50 tác vụ lấy từ lỗi thật, chấm theo kết quả, chạy nhiều lần, và đo token trên mỗi tác vụ giải được.
