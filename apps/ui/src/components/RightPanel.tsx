import { useEffect, useRef, useState } from "react";
import { Bot, FileText, X } from "lucide-react";
import type { ViewState } from "../model/fold";
import { duration } from "../state/util";
import { DiffCard, DiffStat } from "./DiffCard";
import { Thumb } from "./Media";
import styles from "./RightPanel.module.css";

export type PanelTab = "changes" | "files" | "terminal" | "screens" | "tasks";

const LABELS: Record<PanelTab, string> = {
  changes: "Changes",
  files: "Files",
  terminal: "Terminal",
  screens: "Screens",
  tasks: "Tasks",
};

function fileCount(n: number): string {
  return n === 1 ? "1 file" : `${n} files`;
}

/** The tabs that have something to show; Changes is always there. */
export function availableTabs(view: ViewState): PanelTab[] {
  const tabs: PanelTab[] = ["changes"];
  if (view.touched.length) tabs.push("files");
  if (view.commands.length) tabs.push("terminal");
  if (view.screens.length) tabs.push("screens");
  if (view.subagents.length) tabs.push("tasks");
  return tabs;
}

export function RightPanel({
  view,
  tab,
  focusPath,
  sheet,
  onTab,
  onClose,
}: {
  view: ViewState;
  tab: PanelTab;
  focusPath: string | null;
  sheet: boolean;
  onTab: (tab: PanelTab) => void;
  onClose: () => void;
}) {
  const [scope, setScope] = useState<"turn" | "all">("all");
  const body = useRef<HTMLDivElement>(null);
  const tabs = availableTabs(view);
  const current = tabs.includes(tab) ? tab : "changes";
  const lastTurn = view.changes.reduce((max, change) => Math.max(max, change.turn), 0);
  const changes = scope === "turn" ? view.changes.filter((change) => change.turn === lastTurn) : view.changes;
  const totals = changes.reduce((sum, change) => ({ added: sum.added + change.diff.added, removed: sum.removed + change.diff.removed }), {
    added: 0,
    removed: 0,
  });

  useEffect(() => {
    if (!focusPath || current !== "changes") return;
    const target = body.current?.querySelector(`[data-path="${CSS.escape(focusPath)}"]`);
    target?.scrollIntoView({ block: "start", behavior: "smooth" });
  }, [focusPath, current]);

  return (
    <aside className={`${styles.panel} ${sheet ? styles.sheet : ""}`} aria-label="Side panel">
      <div className={styles.tabs} role="tablist" aria-label="Side panel tabs">
        {tabs.map((name) => (
          <button
            key={name}
            type="button"
            role="tab"
            aria-selected={name === current}
            className={styles.tab}
            onClick={() => onTab(name)}
          >
            {LABELS[name]}
          </button>
        ))}
        <span className={styles.spacer} />
        <button type="button" className={styles.close} onClick={onClose} aria-label="Close the side panel">
          <X size={16} aria-hidden />
        </button>
      </div>
      <div className={styles.body} ref={body} role="tabpanel" aria-label={LABELS[current]}>
        {current === "changes" && (
          <>
            <div className={styles.toolbar}>
              <div className={styles.segmented} role="group" aria-label="Which changes">
                <button type="button" aria-pressed={scope === "turn"} onClick={() => setScope("turn")}>
                  Last turn
                </button>
                <button type="button" aria-pressed={scope === "all"} onClick={() => setScope("all")}>
                  All
                </button>
              </div>
              {changes.length > 0 && (
                <span className={styles.summary}>
                  {fileCount(new Set(changes.map((change) => change.diff.path)).size)} <DiffStat {...totals} />
                </span>
              )}
            </div>
            {changes.length === 0 ? (
              <p className={styles.empty}>No file changes {scope === "turn" ? "in the last turn" : "yet"}. Edits the agent makes show up here.</p>
            ) : (
              <div className={styles.stack}>
                {changes.map((change) => (
                  <div key={change.key} data-path={change.diff.path}>
                    <DiffCard diff={change.diff} folded={false} />
                  </div>
                ))}
              </div>
            )}
          </>
        )}
        {current === "files" && (
          <ul className={styles.files}>
            {view.touched.map((file) => (
              <li key={file.path}>
                <FileText size={14} aria-hidden />
                <span className={styles.filePath} title={file.path}>
                  {file.path}
                </span>
                <span className={styles.ops}>{file.ops.join(", ")}</span>
              </li>
            ))}
          </ul>
        )}
        {current === "terminal" && (
          <div className={styles.terminal}>
            {view.commands.map((command) => (
              <div key={command.key} className={styles.command}>
                <div className={styles.prompt}>
                  <span aria-hidden>$</span> {command.command}
                  {command.exitCode !== undefined && command.exitCode !== 0 && <span className={styles.exit}>exit {command.exitCode}</span>}
                  {command.running && <span className={styles.exit}>running…</span>}
                </div>
                {command.output && <pre>{command.output}</pre>}
              </div>
            ))}
          </div>
        )}
        {current === "screens" && (
          <div className={styles.screens}>
            {view.screens.map((screen) => (
              <figure key={screen.key}>
                <Thumb artifact={screen.artifact} large />
                <figcaption>
                  {screen.tool} · {new Date(screen.ts).toLocaleTimeString()}
                </figcaption>
              </figure>
            ))}
          </div>
        )}
        {current === "tasks" && (
          <ul className={styles.files}>
            {view.subagents.map((agent) => (
              <li key={agent.key}>
                <Bot size={14} aria-hidden />
                <span className={styles.filePath}>{agent.detail || agent.title}</span>
                <span className={styles.ops}>
                  {agent.status === "running" || agent.status === "waiting"
                    ? "running"
                    : agent.status === "failed"
                      ? "failed"
                      : agent.endTs
                        ? duration(agent.endTs - agent.startTs)
                        : agent.status}
                </span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </aside>
  );
}
