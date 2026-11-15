import { useEffect, useRef, useState } from "react";
import { Bot, Folder, KeyRound, Menu, PanelRight, Shield, X } from "lucide-react";
import type { Enforced, ModeInfo, SessionStatus, Usage } from "../model/types";

/** The mode label when the host enforces asking. */
export function enforcedLabel(enforced: Enforced): string {
  return enforced === "ask-without-sandbox" ? "Asks first · no sandbox" : "Asks first";
}

export function enforcedDescription(enforced: Enforced): string {
  return enforced === "ask-without-sandbox"
    ? "This host asks before every change, whatever the mode; this session runs without the sandbox."
    : "This host asks before every change, whatever the mode.";
}
import { folderName, money, tokens } from "../state/util";
import styles from "./Header.module.css";

const STATUS: Record<SessionStatus | "new", { label: string; tone: string }> = {
  new: { label: "New", tone: "idle" },
  idle: { label: "Ready", tone: "idle" },
  running: { label: "Running", tone: "running" },
  needs_you: { label: "Needs you", tone: "warn" },
  failed: { label: "Failed", tone: "error" },
  verified: { label: "Verified", tone: "success" },
  not_verified: { label: "Not verified", tone: "error" },
};

export function ContextRing({
  context,
  usage,
  cost,
  model,
}: {
  context: { tokens: number; compact_at: number | null } | null;
  usage: Usage;
  cost: number | null;
  model: string | null;
}) {
  const [open, setOpen] = useState(false);
  const wrap = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (event: MouseEvent | KeyboardEvent) => {
      if (event instanceof KeyboardEvent ? event.key === "Escape" : !wrap.current?.contains(event.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", close);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", close);
    };
  }, [open]);
  const used = context?.tokens ?? 0;
  const limit = context?.compact_at ?? 0;
  const share = limit > 0 ? Math.min(1, used / limit) : 0;
  const radius = 8;
  const circumference = 2 * Math.PI * radius;
  const percent = Math.round(share * 100);
  return (
    <div className={styles.ringWrap} ref={wrap}>
      <button
        type="button"
        className={styles.ring}
        onClick={() => setOpen(!open)}
        aria-label={`Context ${percent}% used; show token use${cost !== null ? " and cost" : ""}`}
        aria-expanded={open}
      >
        <svg width="20" height="20" viewBox="0 0 20 20" aria-hidden>
          <circle cx="10" cy="10" r={radius} fill="none" stroke="var(--bg-3)" strokeWidth="2.5" />
          <circle
            cx="10"
            cy="10"
            r={radius}
            fill="none"
            stroke={share > 0.85 ? "var(--warn)" : "var(--accent)"}
            strokeWidth="2.5"
            strokeDasharray={`${share * circumference} ${circumference}`}
            strokeLinecap="round"
            transform="rotate(-90 10 10)"
          />
        </svg>
        <span className={styles.ringText}>{percent}%</span>
      </button>
      {open && (
        <div className={styles.popover} role="dialog" aria-label="Context and usage">
          <dl>
            <dt>Context</dt>
            <dd>
              {tokens(used)}
              {limit > 0 ? ` of ${tokens(limit)} before summarizing` : ""}
            </dd>
            <dt>Input</dt>
            <dd>{tokens(usage.input_tokens)}</dd>
            <dt>Cached input</dt>
            <dd>
              {tokens(usage.cache_read_tokens)} read · {tokens(usage.cache_write_tokens)} written
            </dd>
            <dt>Output</dt>
            <dd>{tokens(usage.output_tokens)}</dd>
            <dt>Cost</dt>
            <dd>{cost !== null ? money(cost) : "not known for this model"}</dd>
            {model && (
              <>
                <dt>Model</dt>
                <dd>{model}</dd>
              </>
            )}
          </dl>
        </div>
      )}
    </div>
  );
}

/** Tools allowed for the rest of the session, each with a revoke button. */
function Grants({ grants, onRevoke }: { grants: string[]; onRevoke: (tool: string) => void }) {
  const [open, setOpen] = useState(false);
  const wrap = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (event: MouseEvent | KeyboardEvent) => {
      if (event instanceof KeyboardEvent ? event.key === "Escape" : !wrap.current?.contains(event.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", close);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", close);
    };
  }, [open]);
  if (grants.length === 0) return null;
  return (
    <div className={styles.ringWrap} ref={wrap}>
      <button
        type="button"
        className={styles.chip}
        onClick={() => setOpen(!open)}
        aria-expanded={open}
        aria-label={`${grants.length} ${grants.length === 1 ? "tool" : "tools"} allowed for this session; show and revoke`}
      >
        <KeyRound size={14} aria-hidden /> {grants.length}
      </button>
      {open && (
        <div className={styles.popover} role="dialog" aria-label="Allowed for this session">
          <p className={styles.popoverTitle}>Allowed without asking until the mode changes</p>
          <ul className={styles.grantList}>
            {grants.map((tool) => (
              <li key={tool}>
                <code>{tool}</code>
                <button type="button" onClick={() => onRevoke(tool)} aria-label={`Revoke ${tool}`}>
                  <X size={13} aria-hidden /> Revoke
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

export interface HeaderProps {
  title: string;
  canRename: boolean;
  cwd: string | null;
  mode: ModeInfo | undefined;
  /** What the host enforces whatever the mode (hame connect asks before every change). */
  enforced?: Enforced | null;
  status: SessionStatus | "new";
  subagents: number;
  grants: string[];
  onRevoke: (tool: string) => void;
  context: { tokens: number; compact_at: number | null } | null;
  usage: Usage;
  cost: number | null;
  model: string | null;
  panelOpen: boolean;
  panelAvailable: boolean;
  showMenu: boolean;
  connection: string;
  onMenu: () => void;
  onRename: (title: string | null) => void;
  onPanel: () => void;
  onTasks: () => void;
}

export function Header(props: HeaderProps) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(props.title);
  const status = STATUS[props.status];
  return (
    <header className={styles.header}>
      {props.showMenu && (
        <button type="button" className={styles.iconButton} onClick={props.onMenu} aria-label="Open the session list">
          <Menu size={18} aria-hidden />
        </button>
      )}
      <div className={styles.titleBlock}>
        {editing ? (
          <form
            onSubmit={(event) => {
              event.preventDefault();
              props.onRename(draft.trim() || null);
              setEditing(false);
            }}
          >
            <input
              className={styles.titleInput}
              autoFocus
              value={draft}
              aria-label="Session title"
              onChange={(event) => setDraft(event.target.value)}
              onBlur={() => setEditing(false)}
              onKeyDown={(event) => event.key === "Escape" && setEditing(false)}
            />
          </form>
        ) : (
          <h1 className={styles.title}>
            {props.canRename ? (
              <button
                type="button"
                onClick={() => {
                  setDraft(props.title);
                  setEditing(true);
                }}
                title="Rename"
              >
                {props.title}
              </button>
            ) : (
              props.title
            )}
          </h1>
        )}
        <div className={styles.meta}>
          {props.cwd && (
            <span className={styles.metaItem} title={props.cwd}>
              <Folder size={13} aria-hidden />
              {folderName(props.cwd)}
            </span>
          )}
          {props.enforced ? (
            <span className={styles.metaItem} title={enforcedDescription(props.enforced)}>
              <Shield size={13} aria-hidden />
              {enforcedLabel(props.enforced)}
            </span>
          ) : (
            props.mode && (
              <span className={styles.metaItem} title={props.mode.description}>
                <Shield size={13} aria-hidden />
                {props.mode.label}
              </span>
            )
          )}
          {props.connection === "reconnecting" && <span className={styles.metaItem}>reconnecting…</span>}
        </div>
      </div>
      <div className={styles.right}>
        <span className={`${styles.status} ${styles[status.tone]}`} role="status">
          <span className={styles.dot} aria-hidden />
          {status.label}
        </span>
        <Grants grants={props.grants} onRevoke={props.onRevoke} />
        {props.subagents > 0 && (
          <button type="button" className={styles.chip} onClick={props.onTasks} aria-label={`${props.subagents} subagents; open Tasks`}>
            <Bot size={14} aria-hidden /> {props.subagents}
          </button>
        )}
        {props.context && <ContextRing context={props.context} usage={props.usage} cost={props.cost} model={props.model} />}
        {props.panelAvailable && (
          <button
            type="button"
            className={styles.iconButton}
            onClick={props.onPanel}
            aria-label={props.panelOpen ? "Hide the side panel" : "Show the side panel"}
            aria-pressed={props.panelOpen}
          >
            <PanelRight size={18} aria-hidden />
          </button>
        )}
      </div>
    </header>
  );
}
