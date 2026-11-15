import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { AssistantItem } from "./model/fold";
import type { ModeName, ServerInfo, SessionDetail, SessionSummary, StreamEvent } from "./model/types";
import { Api, ApiError } from "./state/api";
import { SessionStore, useSession } from "./state/session";
import { ServicesContext, blobUrls, folderName, isMod, storage, type Services } from "./state/util";
import type { Transport } from "./transport";
import { onDesktopEvent } from "./desktop";
import { ApprovalCard } from "./components/ApprovalCard";
import { Composer, type Attachment } from "./components/Composer";
import { Header } from "./components/Header";
import { Lightbox } from "./components/Media";
import { RightPanel, availableTabs, type PanelTab } from "./components/RightPanel";
import { About, Shortcuts } from "./components/Shortcuts";
import { Sidebar, type Theme } from "./components/Sidebar";
import { Transcript } from "./components/Transcript";
import styles from "./App.module.css";

const EXAMPLES = [
  "Explain how this project is organized",
  "Find a failing test and fix it",
  "Add a section about setup to the README",
];

function useMedia(query: string): boolean {
  const [matches, setMatches] = useState(() => typeof matchMedia === "function" && matchMedia(query).matches);
  useEffect(() => {
    const list = matchMedia(query);
    const change = () => setMatches(list.matches);
    list.addEventListener("change", change);
    change();
    return () => list.removeEventListener("change", change);
  }, [query]);
  return matches;
}

function useWidth(): number {
  const [width, setWidth] = useState(() => window.innerWidth);
  useEffect(() => {
    const change = () => setWidth(window.innerWidth);
    window.addEventListener("resize", change);
    return () => window.removeEventListener("resize", change);
  }, []);
  return width;
}

/**
 * Runs a request that may need confirming ("Full access"): the host answers
 * 428 with its warning and a one-time nonce; after the user agrees, the
 * same request goes again with the nonce. Resolves null when declined.
 */
async function confirmed<T>(run: (confirm?: string) => Promise<T>): Promise<T | null> {
  try {
    return await run();
  } catch (error) {
    if (error instanceof ApiError && error.confirm) {
      if (!window.confirm(error.message)) return null;
      return run(error.confirm);
    }
    throw error;
  }
}

function sessionFromHash(): string | null {
  const match = /^#\/s\/([A-Za-z0-9_-]{1,64})$/.exec(window.location.hash);
  return match ? match[1] : null;
}

export function App({ transport }: { transport: Transport }) {
  const api = useMemo(() => new Api(transport), [transport]);
  const [info, setInfo] = useState<ServerInfo | null>(null);
  const [sessions, setSessions] = useState<SessionSummary[] | null>(null);
  const [activeId, setActiveId] = useState<string | null>(sessionFromHash);
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [draft, setDraft] = useState<{ cwd: string | null; mode: ModeName; model: string | null }>({ cwd: null, mode: "default", model: null });
  const [fatal, setFatal] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [theme, setTheme] = useState<Theme>(() => (storage.get("hame.theme") as Theme | null) ?? "system");
  const [collapsed, setCollapsed] = useState(() => storage.get("hame.sidebar") === "collapsed");
  const [drawer, setDrawer] = useState(false);
  const [panel, setPanel] = useState<{ open: boolean; tab: PanelTab; focus: string | null }>({ open: false, tab: "changes", focus: null });
  const [image, setImage] = useState<{ sha256: string; alt: string } | null>(null);
  const [shortcuts, setShortcuts] = useState(false);
  const [about, setAbout] = useState(false);
  const [announcement, setAnnouncement] = useState("");
  const [seen, setSeen] = useState<Record<string, number>>(() => {
    try {
      return JSON.parse(storage.get("hame.seen") ?? "{}") as Record<string, number>;
    } catch {
      return {};
    }
  });
  const searchRef = useRef<HTMLInputElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const mobile = useMedia("(max-width: 767px)");
  const width = useWidth();

  // Theme: the system's unless chosen; kept per viewer.
  useEffect(() => {
    if (theme === "system") document.documentElement.removeAttribute("data-theme");
    else document.documentElement.setAttribute("data-theme", theme);
    storage.set("hame.theme", theme === "system" ? null : theme);
  }, [theme]);

  useEffect(() => {
    storage.set("hame.sidebar", collapsed ? "collapsed" : null);
  }, [collapsed]);

  const flash = useCallback((message: string) => {
    setToast(message);
    setTimeout(() => setToast((current) => (current === message ? null : current)), 5000);
  }, []);

  const refreshSessions = useCallback(async () => {
    try {
      setSessions(await api.sessions());
    } catch (error) {
      if (error instanceof ApiError && error.status === 403) setFatal("This page's token was refused. Open the address `hame web` printed.");
    }
  }, [api]);

  useEffect(() => {
    api.info().then((loaded) => {
      setInfo(loaded);
      for (const warning of loaded.warnings ?? []) flash(warning);
    }, (error: Error) => {
      setFatal(error instanceof ApiError && error.status === 403 ? "This page's token was refused. Open the address `hame web` printed." : `Cannot reach hame: ${error.message}`);
    });
    void refreshSessions();
    const timer = setInterval(() => {
      if (document.visibilityState === "visible") void refreshSessions();
    }, 5000);
    return () => clearInterval(timer);
  }, [api, refreshSessions]);

  // Sessions that changed while not looked at are bold. The first list
  // seen counts as read.
  useEffect(() => {
    if (!sessions) return;
    setSeen((before) => {
      let changed = false;
      const next = { ...before };
      for (const session of sessions) {
        if (next[session.id] === undefined || session.id === activeId) {
          if (next[session.id] !== session.last_activity) {
            next[session.id] = session.last_activity;
            changed = true;
          }
        }
      }
      if (!changed) return before;
      storage.set("hame.seen", JSON.stringify(next));
      return next;
    });
  }, [sessions, activeId]);
  const unread = useMemo(
    () => new Set((sessions ?? []).filter((s) => s.id !== activeId && seen[s.id] !== undefined && s.last_activity > seen[s.id]).map((s) => s.id)),
    [sessions, seen, activeId],
  );

  // The open session's store.
  const store = useMemo(() => (activeId ? new SessionStore(activeId, transport) : null), [activeId, transport]);
  const loadDetail = useCallback(
    async (id: string) => {
      try {
        const next = await api.detail(id);
        setDetail((current) => (current && current.id !== id ? current : next));
      } catch (error) {
        if (error instanceof ApiError && error.status === 404) {
          flash("That session no longer exists.");
          setActiveId(null);
        }
      }
    },
    [api, flash],
  );
  useEffect(() => {
    if (!store) {
      setDetail(null);
      return;
    }
    store.onEvent = (event: StreamEvent) => {
      if (event.kind === "turn" || event.kind === "settings" || event.kind === "session" || event.kind === "approval") {
        void refreshSessions();
        if (event.kind !== "approval") void loadDetail(store.id);
      }
    };
    store.start();
    void loadDetail(store.id);
    return () => store.stop();
  }, [store, loadDetail, refreshSessions]);

  // Images of the session left behind are given back.
  useEffect(() => () => blobUrls.clear(), [activeId]);

  useEffect(() => {
    const hash = activeId ? `#/s/${activeId}` : "";
    if (window.location.hash !== hash) history.replaceState(null, "", window.location.pathname + window.location.search + hash);
  }, [activeId]);

  const { view, connection } = useSession(store);

  // Announce settled answers and approval questions to screen readers.
  const lastAssistant = [...view.items].reverse().find((item): item is AssistantItem => item.kind === "assistant");
  useEffect(() => {
    if (lastAssistant && !lastAssistant.streaming) setAnnouncement(`hame: ${lastAssistant.text.slice(0, 300)}`);
  }, [lastAssistant]);
  useEffect(() => {
    if (view.approvals[0]) setAnnouncement(`Approval needed: ${view.approvals[0].question}`);
  }, [view.approvals]);

  const openSession = useCallback(
    (id: string) => {
      setActiveId(id);
      setDrawer(false);
      setPanel((current) => ({ ...current, focus: null }));
    },
    [],
  );

  const newSession = useCallback(() => {
    setActiveId(null);
    setDrawer(false);
    setDraft((current) => ({ ...current, cwd: null }));
    setTimeout(() => inputRef.current?.focus(), 0);
  }, []);

  // The desktop shell: a clicked notification or hame:// link opens its
  // session; New Session in its menu or tray starts one.
  useEffect(
    () =>
      onDesktopEvent((event) => {
        if (event.action === "open") openSession(event.session);
        else newSession();
      }),
    [openSession, newSession],
  );

  // Keyboard shortcuts.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!isMod(event) || event.altKey || event.shiftKey) return;
      const key = event.key.toLowerCase();
      if (key === "k") {
        event.preventDefault();
        if (mobile) setDrawer(true);
        else setCollapsed(false);
        setTimeout(() => searchRef.current?.focus(), 0);
      } else if (key === "n") {
        event.preventDefault();
        newSession();
      } else if (key === "/") {
        event.preventDefault();
        setShortcuts((open) => !open);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [mobile, newSession]);

  useEffect(() => {
    if (!drawer) return;
    const onKey = (event: KeyboardEvent) => event.key === "Escape" && setDrawer(false);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [drawer]);

  const report = useCallback((error: unknown) => flash(error instanceof Error ? error.message : String(error)), [flash]);

  /** The open session, creating it from the draft when there is none yet. */
  const ensureSession = useCallback(async (): Promise<string> => {
    if (activeId) return activeId;
    const [provider, ...rest] = (draft.model ?? "").split(":");
    const created = await confirmed((confirm) =>
      api.create({
        cwd: draft.cwd ?? undefined,
        mode: draft.mode,
        provider: draft.model ? provider : undefined,
        model: draft.model ? rest.join(":") : undefined,
        confirm,
      }),
    );
    if (!created) throw new Error("Not started: Full access was not confirmed.");
    setActiveId(created.id);
    void refreshSessions();
    return created.id;
  }, [activeId, api, draft, refreshSessions]);

  const send = useCallback(
    async (text: string, attachments: Attachment[]): Promise<boolean> => {
      try {
        const id = await ensureSession();
        if (!activeId) storage.set("hame.draft.new", null);
        const prompt = () =>
          api.prompt(id, {
            text,
            images: attachments.filter((a) => a.upload?.kind === "image").map((a) => a.upload!.artifact!.sha256),
            files: attachments.filter((a) => a.upload?.kind === "file").map((a) => a.upload!.path!),
          });
        const steer = () => api.steer(id, text);
        // The turn may start or end between what this page knows and the
        // request: a refused queue becomes a prompt, a refused prompt is
        // queued.
        const [first, second] = view.running && activeId ? [steer, prompt] : [prompt, steer];
        try {
          await first();
        } catch (error) {
          if (!(error instanceof ApiError && error.status === 409)) throw error;
          await second();
        }
        return true;
      } catch (error) {
        report(error);
        return false;
      }
    },
    [ensureSession, view.running, activeId, api, report],
  );

  const answer = useCallback(
    (allow: boolean, always = false) => {
      const waiting = view.approvals[0];
      if (!waiting || !activeId || !store) return;
      store.local({ kind: "approval_resolved", id: waiting.id, allow });
      api.approve(activeId, waiting, allow, always).catch(report);
    },
    [view.approvals, activeId, store, api, report],
  );

  const changeMode = useCallback(
    (mode: ModeName) => {
      if (!activeId) {
        setDraft((current) => ({ ...current, mode }));
        return;
      }
      confirmed((confirm) => api.settings(activeId, { mode, confirm })).then(() => loadDetail(activeId), report);
    },
    [activeId, api, loadDetail, report],
  );

  const changeModel = useCallback(
    (provider: string | null, model: string | null) => {
      if (!activeId) {
        setDraft((current) => ({ ...current, model: provider && model ? `${provider}:${model}` : null }));
        return;
      }
      if (!provider || !model) return;
      api.settings(activeId, { provider, model }).then(() => loadDetail(activeId), report);
    },
    [activeId, api, loadDetail, report],
  );

  const rename = useCallback(
    (id: string, title: string | null) => {
      api.rename(id, { title }).then(() => {
        void refreshSessions();
        if (id === activeId) void loadDetail(id);
      }, report);
    },
    [api, activeId, refreshSessions, loadDetail, report],
  );

  const services: Services = useMemo(() => ({ api, session: activeId, openImage: setImage }), [api, activeId]);

  // The side panel takes room only when the transcript keeps 400 px.
  const sidebarWidth = mobile ? 0 : collapsed ? 56 : 280;
  const roomForPanel = width - sidebarWidth - 400;
  const panelWidth = Math.max(300, Math.min(width * 0.45, width * 0.7, roomForPanel));
  const panelFits = !mobile && roomForPanel >= 300;
  const panelShown = panel.open && (mobile || panelFits) && !!activeId;
  const panelAvailable = !!activeId && (mobile || panelFits);
  const tabs = availableTabs(view);

  const status = !activeId ? "new" : view.approvals.length ? "needs_you" : view.running ? "running" : (detail?.status ?? "idle");
  const modeName = activeId ? (view.settings?.mode ?? detail?.mode ?? "default") : draft.mode;
  const mode = info?.modes.find((m) => m.name === modeName);
  const model = activeId ? (detail?.provider && detail?.model ? `${detail.provider}:${detail.model}` : null) : draft.model;
  const title = activeId ? (sessions?.find((s) => s.id === activeId)?.title ?? detail?.title ?? "New session") : "New session";
  const cwd = activeId ? (view.cwd ?? detail?.cwd ?? null) : (draft.cwd ?? info?.cwd ?? null);
  const empty = !activeId || (view.items.length === 0 && connection !== "connecting");

  if (fatal) {
    return (
      <div className={styles.fatal} role="alert">
        <span className={styles.mark}>h</span>
        <p>{fatal}</p>
      </div>
    );
  }

  const composer =
    view.approvals.length > 0 ? (
      <ApprovalCard approval={view.approvals[0]} more={view.approvals.length - 1} onAnswer={answer} />
    ) : (
      <Composer
        draftKey={activeId ?? "new"}
        running={view.running}
        queued={view.queued}
        queuedFrom={view.queuedFrom}
        info={info}
        cwd={cwd}
        canChooseFolder={!activeId}
        mode={modeName}
        model={model}
        autoFocus={!mobile}
        inputRef={inputRef}
        onSend={send}
        onStop={() => activeId && api.cancel(activeId).catch(report)}
        onWithdraw={(index, text) => activeId && api.withdraw(activeId, index, text).catch(report)}
        onUpload={async (file) => {
          if (info && file.size > info.limits.upload_bytes) throw new Error(`larger than ${Math.round(info.limits.upload_bytes / 1048576)} MB`);
          return api.upload(await ensureSession(), file);
        }}
        onFolder={(folder) => setDraft((current) => ({ ...current, cwd: folder }))}
        onMode={changeMode}
        onModel={changeModel}
      />
    );

  return (
    <ServicesContext.Provider value={services}>
      <div
        className={styles.app}
        style={{
          gridTemplateColumns: mobile ? "1fr" : `${collapsed ? "var(--rail)" : "var(--sidebar)"} minmax(0, 1fr) ${panelShown ? `${panelWidth}px` : ""}`,
        }}
      >
        {(!mobile || drawer) && (
          <>
            {mobile && <div className={styles.scrim} onClick={() => setDrawer(false)} aria-hidden />}
            <div className={mobile ? styles.drawer : styles.sidebarCell}>
              <Sidebar
                sessions={sessions}
                activeId={activeId}
                unread={unread}
                collapsed={collapsed}
                drawer={mobile}
                searchRef={searchRef}
                theme={theme}
                onOpen={openSession}
                onNew={newSession}
                onRename={rename}
                onPin={(id, pinned) => api.rename(id, { pinned }).then(refreshSessions, report)}
                onDelete={(id) =>
                  api.remove(id).then(() => {
                    if (id === activeId) setActiveId(null);
                    void refreshSessions();
                  }, report)
                }
                onCollapse={() => setCollapsed(!collapsed)}
                onClose={() => setDrawer(false)}
                onTheme={setTheme}
                onShortcuts={() => setShortcuts(true)}
                onAbout={() => setAbout(true)}
              />
            </div>
          </>
        )}
        <main className={styles.main}>
          <Header
            title={title}
            canRename={!!activeId}
            cwd={cwd}
            mode={mode}
            enforced={detail?.enforced ?? (info?.enforced ? "ask" : null)}
            status={status}
            subagents={view.subagents.filter((agent) => agent.status === "running" || agent.status === "waiting").length}
            grants={view.grants}
            onRevoke={(tool) => activeId && api.revokeGrant(activeId, tool).catch(report)}
            context={detail ? detail.context : null}
            usage={view.usage}
            cost={detail?.cost_usd ?? null}
            model={detail?.model ?? null}
            panelOpen={panelShown}
            panelAvailable={panelAvailable}
            showMenu={mobile}
            connection={connection}
            onMenu={() => setDrawer(true)}
            onRename={(next) => activeId && rename(activeId, next)}
            onPanel={() => setPanel((current) => ({ ...current, open: !current.open }))}
            onTasks={() => setPanel({ open: true, tab: "tasks", focus: null })}
          />
          {empty ? (
            <div className={styles.emptyState}>
              <div className={styles.hero}>
                <h2>What should hame do?</h2>
                <p>
                  hame works in <strong title={cwd ?? undefined}>{folderName(cwd) || "this folder"}</strong> on this computer. It reads
                  the code, runs commands in a sandbox and asks before anything risky.
                </p>
              </div>
              <div className={styles.emptyComposer}>{composer}</div>
              <div className={styles.examples} aria-label="Examples">
                {EXAMPLES.map((example) => (
                  <button
                    key={example}
                    type="button"
                    onClick={() => {
                      const area = inputRef.current;
                      if (!area) return;
                      const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
                      setter?.call(area, example);
                      area.dispatchEvent(new Event("input", { bubbles: true }));
                      area.focus();
                    }}
                  >
                    {example}
                  </button>
                ))}
              </div>
            </div>
          ) : (
            <>
              <Transcript
                items={view.items}
                sessionKey={activeId ?? "new"}
                actions={{
                  onContinue: () => void send("Continue.", []),
                  onOpenChange: (path) => setPanel({ open: true, tab: "changes", focus: path }),
                }}
              />
              <div className={styles.dock}>{composer}</div>
            </>
          )}
        </main>
        {panelShown && (
          <RightPanel
            view={view}
            tab={tabs.includes(panel.tab) ? panel.tab : "changes"}
            focusPath={panel.focus}
            sheet={mobile}
            onTab={(tab) => setPanel((current) => ({ ...current, tab, focus: null }))}
            onClose={() => setPanel((current) => ({ ...current, open: false }))}
          />
        )}
      </div>
      {image && <Lightbox image={image} onClose={() => setImage(null)} />}
      {shortcuts && <Shortcuts onClose={() => setShortcuts(false)} />}
      {about && <About version={info?.version ?? null} onClose={() => setAbout(false)} />}
      {toast && (
        <div className={styles.toast} role="status">
          {toast}
        </div>
      )}
      <div className="sr-only" aria-live="polite">
        {announcement}
      </div>
    </ServicesContext.Provider>
  );
}
