# hame UI specification

One UI for web, desktop and mobile (`apps/ui`). Synthesized from studying
DeepSeek Harness (open source, MIT; its name, logo, colours and fonts are
not to be used), Claude (claude.ai, Claude Code desktop/web) and OpenAI
Codex (app, IDE extension, mobile). We take layout and interaction
patterns, never a product's name, logo, colour palette, icons or assets.
Notes behind each choice: the two research reports summarized in
`docs/design/APPS-AND-SYNC.md` §5.

## Principles

1. **Calm and dense.** A reading column, quiet chrome, one accent colour,
   hairline borders. Long agent runs stay readable because work is folded.
2. **Status is always visible.** Running, needs you, failed, verified: in
   the sidebar row, the session header and the composer.
3. **Safe by default.** Approvals are hard to miss; Enter allows only the
   narrowest scope; Esc rejects; "always" needs a click.
4. **Same everywhere.** Web, desktop and phone show the same components;
   the phone hides panels, never features.
5. **Keyboard first on desktop, thumb first on phone.**

## Layout

```
┌────────────┬───────────────────────────────────┬──────────────────┐
│ Sidebar    │ Session header (title, host, mode,│ Right panel      │
│ 280 px     │ status, subagents, context ring)  │ (tabs: Changes,  │
│ (56 px     ├───────────────────────────────────┤ Files, Terminal, │
│  rail when │ Transcript (reading column        │ Browser/Desktop  │
│  collapsed)│ clamp(680px, 64%, 920px))         │ screenshots,     │
│            │                                   │ Plan, Tasks)     │
│            ├───────────────────────────────────┤ 45% of viewport, │
│            │ Composer + control row            │ max 70%          │
└────────────┴───────────────────────────────────┴──────────────────┘
```

- Below 768 px: the sidebar becomes a drawer; the right panel opens
  full-screen as a sheet; the composer sticks to the bottom (safe-area
  insets); minimum supported width 390 px.
- The right panel never squeezes the transcript below 400 px; it shrinks
  to 300 px, then closes.

### Sidebar
- New session (Mod+N), search (Mod+K), sessions grouped by project folder
  (latest 5, "show more"), pinned first.
- Row: title, relative time, status glyph: running (shimmer), **needs you**
  (amber dot: approval or question pending), failed (red dot), verified
  (check, for `hame task`), unread (bold).
- Host switcher at the top when signed in: "This Mac" and other hosts with
  online dots; offline hosts greyed with "last seen".
- Bottom: account row (avatar, name, sync status), settings.

### Session header
Title (editable), host and working folder, permission mode, run status,
subagent count (opens Tasks), context ring (usage %, click for a
breakdown of tokens and cost).

### Transcript
- **User messages**: right-aligned bubble (radius 20), attachments as
  chips.
- **Assistant text**: Markdown (code blocks with copy, tables, links);
  streamed and painted once per frame.
- **Work groups**: consecutive reasoning and tool calls fold into one
  group with a summary header ("Read 4 files, edited 2, ran 3 commands"),
  live title with a shimmer while running, auto-folds when the turn ends;
  max height `min(400px, 50vh)` with internal scroll that follows new
  rows.
- **Tool rows**: icon by category (read, search, edit, run, browser,
  desktop, subagent, web, MCP), one-line description, duration; failures
  keep the icon and show a neutral capsule with a red dot and the exit
  code; expand for input/output.
- **Diffs**: card per file with `+N −M`, 3 context lines, `⋯` between
  hunks, folded after 9 rows; click opens it in Changes.
- **Screenshots and images**: thumbnails, open in a lightbox; desktop and
  browser screenshots also in the right panel.
- **Reasoning**: collapsed row showing its latest finished first line;
  expands to small tertiary text.
- **Plans**: a card with "Run with approvals", "Run, accept edits", "Keep
  planning"; full plan in the Plan tab.
- **Verification** (`hame task`): a verdict card (verified / not verified)
  listing each acceptance condition with its evidence.
- **Errors**: inline row with a red dot and the message; transient
  failures offer "Continue"; budget stops say which budget.
- Turn footer: duration, tokens, cost (only when exact).
- Back-to-bottom button; follow output only while pinned to the bottom.

### Approval card
Replaces the composer while pending: amber strip "Waiting for your
approval", what will happen (tool, target, command or diff), buttons
**Allow once** (Enter), **Always allow for …** (scope shown), **Deny**
(Esc, optional reason). Shows which device/host asks when remote.

### Composer
- Auto-growing textarea (16 px font on touch), placeholder "Ask hame to do
  something…".
- `/` commands and skills picker, `@` files picker (paths from the host),
  attachments (paste or drop images and files; remote clients upload to
  the host), voice input on mobile (system dictation).
- Messages sent while a turn runs are queued as steering (shown as
  "queued", removable).
- Control row: host and folder, permission mode (Read only / Ask / Accept
  edits / Full access with confirmation), model picker (grouped by
  provider, effort sub-menu), send/stop button (Esc twice stops).
- Draft kept per session.

### Right panel tabs
Changes (diff by file, scopes: last turn / all), Files (tree + viewer for
`path:line` links), Terminal (command output of the session), Screens
(browser and desktop screenshots in order), Plan, Tasks (subagents and
background jobs with stop buttons). Tabs remember state per session.

### Settings
Account (sign-in methods, devices with fingerprints, revoke), Hosts
(pair a device: shows QR + SAS; approved devices), Models and keys
(providers configured on the host), Permissions defaults, Appearance
(theme, font size, density), Notifications, Keyboard shortcuts (Mod+/).

### Sign-in and pairing screens
- Sign in: email + password, "Continue with Google", "Continue with
  Apple", "Use a passkey"; sign up: name, email, password (strength
  meter), verification email; forgot password.
- Pair: on the host, a QR card with expiry; on the phone, a scanner; both
  show the 6-digit SAS; the host confirms.

### Empty states
New session: centred composer with the host/folder picker and 3 example
tasks; no sessions: short explanation of what hame can do on this
computer.

## Design tokens (hame's own)

```
--font-sans: system-ui, -apple-system, "Segoe UI", Roboto, sans-serif
--font-mono: ui-monospace, "SF Mono", "JetBrains Mono", Consolas, monospace
--text-body: 14px/22px   --text-small: 13px/20px   --text-code: 12.5px/19px
--text-h1: 20px  --text-h2: 17px  --text-h3: 15px   (weights 400/500/600)
--space: 4 8 12 16 20 24 32 48
--radius: control 10, card 14, bubble 18, sheet/composer 22, pill 999
--border: 0.5px hairline (1px on 1x displays)

Light                         Dark
--bg:        #FFFFFF          #0F1113
--bg-1:      #F7F7F8          #16191C
--bg-2:      #EFF0F2          #1D2125
--bg-3:      #E4E6E9          #262B30
--fg:        #16181B          #ECEDEE
--fg-2:      #4A4F57          #B4B8BE
--fg-3:      #7A808A          #858B94
--border:    #E2E4E8          #2A2F35
--accent:    #12805C (hame green)   #3FC79A
--accent-fg: #FFFFFF          #06130E
--warn:      #B7791F          #E3B341
--error:     #C93C37          #F0726B
--success:   #1E874B          #4CC38A
--diff-add:  #E6F4EA / #1A3A27     --diff-del: #FCE8E8 / #3D1F1F
```

Motion: 120–180 ms ease-out for panels; one shimmer animation for "live";
`prefers-reduced-motion` turns animations off.

Accessibility: WCAG AA contrast, visible focus rings (accent, 2 px),
keyboard reachable everything, ARIA live region for streamed text
announcements (polite), labels on icon buttons.

## Tech

React + TypeScript + Vite; CSS Modules on the tokens above (no Tailwind,
no component library); Markdown via `micromark` (GFM) rendered to React;
code highlighting via a small highlighter (`highlight.js` core with a few
languages), lazily loaded; virtualized transcript for long sessions
(`@tanstack/react-virtual`); icons: an MIT/ISC icon set (Lucide).
State: a small store per session (events by seq, folded view model);
transport interface with `local` (HTTP + SSE to the hame web server) and
`remote` (relay, `packages/sync-js`) implementations.
