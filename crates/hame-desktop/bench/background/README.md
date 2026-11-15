# Background-execution probes

Standalone Swift programs that check what can be done to TextEdit, Numbers
and Pages without making them frontmost; results and conclusions are in
`docs/research/BACKGROUND-EXECUTION-2026-11-15.md` ("Verified on this Mac").

    swiftc -O -parse-as-library probe1.swift -o probe1 && ./probe1 /tmp/bgprobe

- `probe1.swift`: TextEdit launch, AX read/write, AX menu Save, `postToPid`
  text and cmd+s, save panel, capture (hidden, background, minimized);
  Numbers and Pages typing and capture. Its helpers are reused by the others.
- `probe4.swift`: whether typed text marks the document edited, AX Save in the
  background, and the same Save after `activate()` (control).
- `probe5.swift`: Pages caret plus typing, Numbers cell selection plus typing,
  and the Save As panel after `activate()`.

They open and quit the apps (discarding changes) and write only into the
output folder. Run them from a terminal with Accessibility and Screen
Recording permission. They never post mouse events.
