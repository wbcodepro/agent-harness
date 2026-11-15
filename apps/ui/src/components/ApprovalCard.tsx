import { useEffect, useRef } from "react";
import { ShieldAlert } from "lucide-react";
import { diffsFromToolInput } from "../model/diff";
import { describe } from "../model/tools";
import type { ApprovalEvent } from "../model/types";
import { DiffCard } from "./DiffCard";
import styles from "./Composer.module.css";

/**
 * Takes the composer's place while a tool call waits for an answer. Enter
 * allows only this call; Esc denies; "always" needs a click.
 */
export function ApprovalCard({
  approval,
  more,
  onAnswer,
}: {
  approval: ApprovalEvent;
  more: number;
  onAnswer: (allow: boolean, always?: boolean) => void;
}) {
  const card = useRef<HTMLDivElement>(null);
  useEffect(() => {
    card.current?.focus();
  }, [approval.id]);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target as HTMLElement | null;
      const typing = target && (target.tagName === "TEXTAREA" || target.tagName === "INPUT" || target.isContentEditable);
      if (event.key === "Escape") {
        event.preventDefault();
        onAnswer(false);
      } else if (event.key === "Enter" && !event.shiftKey && !typing && target?.tagName !== "BUTTON") {
        event.preventDefault();
        onAnswer(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onAnswer]);

  const tool = approval.tool ?? "";
  const input = (approval.input ?? {}) as Record<string, unknown>;
  const diffs = tool ? diffsFromToolInput(tool, approval.input) : [];
  const described = tool ? describe(tool, approval.input) : null;
  const command = tool === "bash" && typeof input.command === "string" ? input.command : null;
  return (
    <div
      ref={card}
      className={styles.approval}
      role="alertdialog"
      aria-labelledby={`approval-${approval.id}`}
      aria-describedby={`approval-q-${approval.id}`}
      tabIndex={-1}
    >
      <div className={styles.strip} id={`approval-${approval.id}`}>
        <ShieldAlert size={15} aria-hidden />
        Waiting for your approval
        {more > 0 && <span className={styles.more}>+{more} more</span>}
      </div>
      <div className={styles.approvalBody}>
        <p className={styles.question} id={`approval-q-${approval.id}`}>
          {described ? (
            <>
              <strong>{tool}</strong> {described.detail ?? described.path ?? described.title}
            </>
          ) : (
            approval.question
          )}
        </p>
        {command && <pre className={styles.command}>{command}</pre>}
        {diffs.map((diff) => (
          <DiffCard key={diff.path} diff={diff} />
        ))}
        {!command && diffs.length === 0 && approval.input !== undefined && described && (
          <pre className={styles.command}>{JSON.stringify(approval.input, null, 2)}</pre>
        )}
      </div>
      <div className={styles.approvalActions}>
        <button type="button" className={styles.primary} onClick={() => onAnswer(true)}>
          Allow once <kbd>Enter</kbd>
        </button>
        {approval.scope && (
          <button type="button" className={styles.secondary} onClick={() => onAnswer(true, true)}>
            Always allow {approval.scope}
          </button>
        )}
        <button type="button" className={styles.secondary} onClick={() => onAnswer(false)}>
          Deny <kbd>Esc</kbd>
        </button>
      </div>
    </div>
  );
}
