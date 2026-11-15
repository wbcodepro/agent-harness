// How a tool call is shown: its category (which sets the icon and the
// work group summary) and a one-line description.

export type Category =
  | "read"
  | "search"
  | "edit"
  | "run"
  | "browser"
  | "desktop"
  | "subagent"
  | "web"
  | "mcp"
  | "other";

const CATEGORIES: Record<string, Category> = {
  read: "read",
  read_anchored: "read",
  view: "read",
  grep: "search",
  glob: "search",
  repo_map: "search",
  references: "search",
  diagnostics: "search",
  tool_search: "search",
  write: "edit",
  edit: "edit",
  edit_lines: "edit",
  apply_patch: "edit",
  rename_symbol: "edit",
  bash: "run",
  job_output: "run",
  script: "run",
  browser: "browser",
  desktop: "desktop",
  computer: "desktop",
  subagent: "subagent",
  send_message: "subagent",
  task: "subagent",
  web_fetch: "web",
  web_search: "web",
  fetch: "web",
};

export function categoryOf(name: string): Category {
  if (name.startsWith("mcp__")) return "mcp";
  return CATEGORIES[name] ?? "other";
}

function str(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() !== "" ? value : undefined;
}

function firstLine(text: string, max = 120): string {
  const line = text.split("\n").find((l) => l.trim() !== "") ?? "";
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}

export function basename(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** The path to show next to a title that already names the file: only when it adds a folder. */
function where(path: string | undefined): string | undefined {
  return path && basename(path) !== path.replace(/[\\/]+$/, "") ? path : undefined;
}

export interface Described {
  /** What the row says, such as "Read lib.rs". */
  title: string;
  /** Monospace part shown after the title (a command, a path). */
  detail?: string;
  /** The file the call is about, when there is one. */
  path?: string;
}

/** A one-line description of a tool call from its name and input. */
export function describe(name: string, input: unknown): Described {
  const args = (typeof input === "object" && input !== null ? input : {}) as Record<string, unknown>;
  const path = str(args.path) ?? str(args.file_path);
  switch (name) {
    case "read":
    case "read_anchored":
    case "view":
      return { title: path ? `Read ${basename(path)}` : "Read a file", detail: where(path), path };
    case "grep":
      return { title: `Searched for ${JSON.stringify(firstLine(str(args.pattern) ?? "", 60))}`, detail: path };
    case "glob":
      return { title: "Listed files", detail: str(args.pattern) };
    case "repo_map":
      return { title: "Mapped the repository", detail: str(args.symbol) ?? path };
    case "references":
      return { title: "Found references", detail: str(args.symbol) ?? path };
    case "write":
      return { title: path ? `Wrote ${basename(path)}` : "Wrote a file", detail: where(path), path };
    case "edit":
    case "edit_lines":
      return { title: path ? `Edited ${basename(path)}` : "Edited a file", detail: where(path), path };
    case "apply_patch": {
      const patch = str(args.patch) ?? "";
      const files = patch.match(/^\*\*\* (Update|Add|Delete) File: /gm)?.length ?? 0;
      return { title: files === 1 ? "Patched 1 file" : `Patched ${files} files` };
    }
    case "rename_symbol":
      return { title: `Renamed ${str(args.symbol) ?? str(args.old_name) ?? "a symbol"}`, detail: path };
    case "bash": {
      const command = str(args.command) ?? "";
      return { title: "Ran", detail: firstLine(command, 160) };
    }
    case "job_output":
      return { title: args.kill ? "Stopped a background job" : "Checked a background job", detail: str(args.job) ?? str(args.id) };
    case "script":
      return { title: "Ran a script" };
    case "browser": {
      const action = str(args.action) ?? "used";
      return { title: `Browser: ${action}`, detail: str(args.url) ?? str(args.text) ?? str(args.selector) };
    }
    case "desktop":
    case "computer": {
      const action = str(args.action) ?? "used";
      return { title: `Desktop: ${action}`, detail: str(args.app) ?? str(args.text) };
    }
    case "subagent":
    case "task":
      return { title: "Subagent", detail: firstLine(str(args.task) ?? str(args.prompt) ?? str(args.description) ?? "", 120) };
    case "send_message":
      return { title: `Message to ${str(args.to) ?? "an agent"}`, detail: firstLine(str(args.text) ?? str(args.message) ?? "") };
    case "web_fetch":
    case "fetch":
      return { title: "Fetched", detail: str(args.url) };
    case "web_search":
      return { title: "Searched the web", detail: str(args.query) };
    default: {
      if (name.startsWith("mcp__")) {
        const [, server = "", tool = ""] = name.split("__");
        return { title: `${server}: ${tool}` };
      }
      const first = Object.values(args).find((value) => typeof value === "string") as string | undefined;
      return { title: name, detail: first ? firstLine(first, 100) : undefined };
    }
  }
}

/** The exit code a command's output reports, as `[exit code N]`. */
export function exitCode(output: string): number | undefined {
  const match = /\[exit code (\d+)/.exec(output);
  return match ? Number(match[1]) : undefined;
}

const PLURAL: Record<Category, [string, string]> = {
  read: ["Read {n} file", "Read {n} files"],
  search: ["searched {n} time", "searched {n} times"],
  edit: ["edited {n}", "edited {n}"],
  run: ["ran {n} command", "ran {n} commands"],
  browser: ["used the browser {n} time", "used the browser {n} times"],
  desktop: ["used the desktop {n} time", "used the desktop {n} times"],
  subagent: ["started {n} subagent", "started {n} subagents"],
  web: ["fetched {n} page", "fetched {n} pages"],
  mcp: ["called {n} tool", "called {n} tools"],
  other: ["used {n} tool", "used {n} tools"],
};

const ORDER: Category[] = ["read", "search", "edit", "run", "browser", "desktop", "subagent", "web", "mcp", "other"];

/**
 * A work group's summary header, such as "Read 4 files, edited 2, ran 3
 * commands". `rows` holds each call's category and, for file tools, its
 * path (files are counted once).
 */
export function summarize(rows: { category: Category; path?: string; failed?: boolean }[], thoughts = 0): string {
  const counts = new Map<Category, number>();
  const files = new Map<Category, Set<string>>();
  let failed = 0;
  for (const row of rows) {
    if (row.failed) failed++;
    if ((row.category === "read" || row.category === "edit") && row.path) {
      const set = files.get(row.category) ?? new Set();
      set.add(row.path);
      files.set(row.category, set);
      counts.set(row.category, set.size);
    } else {
      counts.set(row.category, (counts.get(row.category) ?? 0) + 1);
    }
  }
  const parts: string[] = [];
  for (const category of ORDER) {
    const n = counts.get(category);
    if (!n) continue;
    const [one, many] = PLURAL[category];
    parts.push((n === 1 ? one : many).replace("{n}", String(n)));
  }
  if (parts.length === 0) return thoughts > 0 ? "Thought" : "Working";
  let text = parts.join(", ");
  text = text[0].toUpperCase() + text.slice(1);
  if (failed > 0) text += ` · ${failed} failed`;
  return text;
}

/** A human description of an acceptance condition of a task. */
export function describePredicate(predicate: { kind: string } & Record<string, unknown>): string {
  const p = predicate as Record<string, unknown>;
  const s = (key: string) => String(p[key] ?? "");
  switch (predicate.kind) {
    case "file_exists":
      return `${s("path")} exists`;
    case "file_absent":
      return `${s("path")} does not exist`;
    case "file_contains":
      return `${s("path")} contains ${JSON.stringify(s("text"))}`;
    case "file_sha256":
      return `${s("path")} has hash ${s("sha256").slice(0, 12)}…`;
    case "json_value":
      return `${s("path")} ${s("pointer")} equals ${JSON.stringify(p.equals)}`;
    case "csv_shape":
      return `${s("path")} has ${p.rows ?? "?"} rows`;
    case "command":
      return `\`${s("command")}\` exits with ${p.exit ?? 0}`;
    case "browser_state":
      return `the browser shows ${p.text_contains ? JSON.stringify(p.text_contains) : `a URL with ${JSON.stringify(p.url_contains)}`}`;
    case "desktop_state":
      return `${s("app")} shows ${JSON.stringify(p.window_title_contains ?? p.value_contains)}`;
    default:
      return predicate.kind;
  }
}
