import { useEffect, useMemo, useRef, useState, type RefObject } from "react";
import {
  Check,
  Info,
  Keyboard,
  Monitor,
  Moon,
  MoreHorizontal,
  PanelLeftClose,
  PanelLeftOpen,
  Pencil,
  Pin,
  PinOff,
  Search,
  SquarePen,
  Sun,
  Trash2,
  X,
} from "lucide-react";
import type { SessionSummary } from "../model/types";
import { folderName, modKey, relativeTime } from "../state/util";
import styles from "./Sidebar.module.css";

export type Theme = "system" | "light" | "dark";

export interface SidebarProps {
  sessions: SessionSummary[] | null;
  activeId: string | null;
  unread: Set<string>;
  collapsed: boolean;
  drawer: boolean;
  searchRef: RefObject<HTMLInputElement | null>;
  theme: Theme;
  onOpen: (id: string) => void;
  onNew: () => void;
  onRename: (id: string, title: string | null) => void;
  onPin: (id: string, pinned: boolean) => void;
  onDelete: (id: string) => void;
  onCollapse: () => void;
  onClose: () => void;
  onTheme: (theme: Theme) => void;
  onShortcuts: () => void;
  onAbout: () => void;
}

const SHOWN = 5;

export function StatusGlyph({ status }: { status: SessionSummary["status"] }) {
  switch (status) {
    case "running":
      return <span className={styles.running} role="img" aria-label="running" />;
    case "needs_you":
      return <span className={styles.amber} role="img" aria-label="needs you" />;
    case "failed":
      return <span className={styles.red} role="img" aria-label="failed" />;
    case "verified":
      return <Check size={13} className={styles.verified} role="img" aria-label="verified" />;
    case "not_verified":
      return <X size={13} className={styles.unverified} role="img" aria-label="not verified" />;
    default:
      return <span className={styles.none} aria-hidden />;
  }
}

function Row({
  session,
  active,
  unread,
  props,
}: {
  session: SessionSummary;
  active: boolean;
  unread: boolean;
  props: SidebarProps;
}) {
  const [menu, setMenu] = useState(false);
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState(session.title ?? "");
  const menuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!menu) return;
    const close = (event: MouseEvent | KeyboardEvent) => {
      if (event instanceof KeyboardEvent ? event.key === "Escape" : !menuRef.current?.contains(event.target as Node)) setMenu(false);
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", close);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", close);
    };
  }, [menu]);
  const label = session.title ?? "New session";
  if (editing) {
    return (
      <li className={styles.row} data-active={active}>
        <form
          className={styles.rename}
          onSubmit={(event) => {
            event.preventDefault();
            props.onRename(session.id, title.trim() || null);
            setEditing(false);
          }}
        >
          <input
            autoFocus
            value={title}
            aria-label="Session title"
            onChange={(event) => setTitle(event.target.value)}
            onBlur={() => setEditing(false)}
            onKeyDown={(event) => event.key === "Escape" && setEditing(false)}
          />
        </form>
      </li>
    );
  }
  return (
    <li className={styles.row} data-active={active}>
      <button
        type="button"
        className={styles.rowMain}
        onClick={() => props.onOpen(session.id)}
        aria-current={active ? "page" : undefined}
        title={`${label}\n${session.cwd}`}
      >
        <StatusGlyph status={session.status} />
        <span className={`${styles.title} ${unread ? styles.unread : ""}`}>{label}</span>
        <span className={styles.time}>{relativeTime(session.last_activity)}</span>
      </button>
      <div className={styles.menuWrap} ref={menuRef}>
        <button
          type="button"
          className={styles.more}
          onClick={() => setMenu(!menu)}
          aria-label={`Actions for ${label}`}
          aria-haspopup="menu"
          aria-expanded={menu}
        >
          <MoreHorizontal size={15} aria-hidden />
        </button>
        {menu && (
          <div className={styles.menu} role="menu">
            <button
              role="menuitem"
              type="button"
              onClick={() => {
                setMenu(false);
                setTitle(session.title ?? "");
                setEditing(true);
              }}
            >
              <Pencil size={14} aria-hidden /> Rename
            </button>
            <button
              role="menuitem"
              type="button"
              onClick={() => {
                setMenu(false);
                props.onPin(session.id, !session.pinned);
              }}
            >
              {session.pinned ? <PinOff size={14} aria-hidden /> : <Pin size={14} aria-hidden />}
              {session.pinned ? "Unpin" : "Pin"}
            </button>
            <button
              role="menuitem"
              type="button"
              className={styles.danger}
              onClick={() => {
                setMenu(false);
                if (window.confirm(`Delete "${label}"? Its conversation log is removed; files it changed stay as they are.`)) {
                  props.onDelete(session.id);
                }
              }}
            >
              <Trash2 size={14} aria-hidden /> Delete
            </button>
          </div>
        )}
      </div>
    </li>
  );
}

function Group({ name, sessions, props }: { name: string; sessions: SessionSummary[]; props: SidebarProps }) {
  const [all, setAll] = useState(false);
  const shown = all ? sessions : sessions.slice(0, SHOWN);
  return (
    <section className={styles.group} aria-label={name}>
      <h2 className={styles.groupName}>{name}</h2>
      <ul className={styles.rows}>
        {shown.map((session) => (
          <Row key={session.id} session={session} active={session.id === props.activeId} unread={props.unread.has(session.id)} props={props} />
        ))}
      </ul>
      {sessions.length > SHOWN && (
        <button type="button" className={styles.showMore} onClick={() => setAll(!all)}>
          {all ? "Show less" : `Show ${sessions.length - SHOWN} more`}
        </button>
      )}
    </section>
  );
}

const THEMES: { value: Theme; label: string; Icon: typeof Sun }[] = [
  { value: "system", label: "System theme", Icon: Monitor },
  { value: "light", label: "Light theme", Icon: Sun },
  { value: "dark", label: "Dark theme", Icon: Moon },
];

export function Sidebar(props: SidebarProps) {
  const [query, setQuery] = useState("");
  const groups = useMemo(() => {
    const needle = query.trim().toLowerCase();
    const matching = (props.sessions ?? []).filter(
      (session) => !needle || (session.title ?? "new session").toLowerCase().includes(needle) || session.cwd.toLowerCase().includes(needle),
    );
    const pinned = matching.filter((session) => session.pinned);
    const byFolder = new Map<string, SessionSummary[]>();
    for (const session of matching) {
      if (session.pinned) continue;
      const list = byFolder.get(session.cwd) ?? [];
      list.push(session);
      byFolder.set(session.cwd, list);
    }
    const result: { key: string; name: string; sessions: SessionSummary[] }[] = [];
    if (pinned.length) result.push({ key: "__pinned", name: "Pinned", sessions: pinned });
    for (const [cwd, sessions] of byFolder) result.push({ key: cwd, name: folderName(cwd) || cwd, sessions });
    return result;
  }, [props.sessions, query]);

  if (props.collapsed && !props.drawer) {
    return (
      <nav className={`${styles.sidebar} ${styles.rail}`} aria-label="Sessions">
        <button type="button" className={styles.railButton} onClick={props.onCollapse} aria-label="Show the session list">
          <PanelLeftOpen size={18} aria-hidden />
        </button>
        <button type="button" className={styles.railButton} onClick={props.onNew} aria-label={`New session (${modKey}+N)`}>
          <SquarePen size={18} aria-hidden />
        </button>
        <button
          type="button"
          className={styles.railButton}
          onClick={() => {
            props.onCollapse();
            setTimeout(() => props.searchRef.current?.focus(), 0);
          }}
          aria-label={`Search sessions (${modKey}+K)`}
        >
          <Search size={18} aria-hidden />
        </button>
      </nav>
    );
  }

  const current = THEMES.find((theme) => theme.value === props.theme) ?? THEMES[0];
  const next = THEMES[(THEMES.indexOf(current) + 1) % THEMES.length];
  return (
    <nav className={styles.sidebar} aria-label="Sessions">
      <div className={styles.top}>
        <span className={styles.brand}>
          <span className={styles.mark} aria-hidden>
            h
          </span>
          hame
        </span>
        <span className={styles.topActions}>
          <button type="button" className={styles.iconButton} onClick={props.onNew} aria-label={`New session (${modKey}+N)`} title={`New session (${modKey}+N)`}>
            <SquarePen size={17} aria-hidden />
          </button>
          {props.drawer ? (
            <button type="button" className={styles.iconButton} onClick={props.onClose} aria-label="Close the session list">
              <X size={17} aria-hidden />
            </button>
          ) : (
            <button type="button" className={styles.iconButton} onClick={props.onCollapse} aria-label="Collapse the session list">
              <PanelLeftClose size={17} aria-hidden />
            </button>
          )}
        </span>
      </div>
      <label className={styles.search}>
        <Search size={15} aria-hidden />
        <input
          ref={props.searchRef}
          type="search"
          placeholder="Search sessions"
          aria-label="Search sessions"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              setQuery("");
              (event.target as HTMLInputElement).blur();
            }
          }}
        />
        <kbd>{modKey}K</kbd>
      </label>
      <div className={styles.list}>
        {props.sessions === null ? null : props.sessions.length === 0 ? (
          <div className={styles.empty}>
            <p>No sessions yet.</p>
            <p>
              hame works in folders on this computer: it reads the code, runs commands in a sandbox and asks before anything risky. Start
              one with <strong>New session</strong>.
            </p>
          </div>
        ) : groups.length === 0 ? (
          <div className={styles.empty}>
            <p>No session matches “{query}”.</p>
          </div>
        ) : (
          groups.map((group) => <Group key={group.key} name={group.name} sessions={group.sessions} props={props} />)
        )}
      </div>
      <div className={styles.bottom}>
        <span className={styles.host} title="The hame on this computer">
          <span className={styles.online} aria-hidden /> This computer
        </span>
        <button type="button" className={styles.iconButton} onClick={props.onAbout} aria-label="About hame and open-source licenses" title="About">
          <Info size={16} aria-hidden />
        </button>
        <button type="button" className={styles.iconButton} onClick={props.onShortcuts} aria-label={`Keyboard shortcuts (${modKey}+/)`} title={`Keyboard shortcuts (${modKey}+/)`}>
          <Keyboard size={16} aria-hidden />
        </button>
        <button
          type="button"
          className={styles.iconButton}
          onClick={() => props.onTheme(next.value)}
          aria-label={`${current.label}; switch to ${next.label.toLowerCase()}`}
          title={current.label}
        >
          <current.Icon size={16} aria-hidden />
        </button>
      </div>
    </nav>
  );
}
