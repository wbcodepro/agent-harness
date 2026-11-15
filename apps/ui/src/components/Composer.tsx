import { useEffect, useLayoutEffect, useRef, useState, type ClipboardEvent, type DragEvent, type KeyboardEvent, type RefObject } from "react";
import { ArrowUp, Folder, Paperclip, Shield, Sparkles, Square, X } from "lucide-react";
import type { ModeName, ServerInfo } from "../model/types";
import type { Upload } from "../state/api";
import { folderName, storage } from "../state/util";
import styles from "./Composer.module.css";

export interface Attachment {
  id: string;
  name: string;
  status: "uploading" | "ready" | "failed";
  upload?: Upload;
  error?: string;
  preview?: string;
}

export interface ComposerProps {
  draftKey: string;
  running: boolean;
  queued: string[];
  /** Who queued each message: null for this computer, else a device's label. */
  queuedFrom?: (string | null)[];
  info: ServerInfo | null;
  cwd: string | null;
  /** Only a new session can choose its folder. */
  canChooseFolder: boolean;
  mode: ModeName;
  model: string | null;
  disabled?: boolean;
  autoFocus?: boolean;
  onSend: (text: string, attachments: Attachment[]) => Promise<boolean>;
  onStop: () => void;
  onWithdraw: (index: number, text: string) => void;
  onUpload: (file: File) => Promise<Upload>;
  onFolder: (folder: string) => void;
  onMode: (mode: ModeName) => void;
  onModel: (provider: string | null, model: string | null) => void;
  inputRef?: RefObject<HTMLTextAreaElement | null>;
}

let attachmentIds = 0;

export function Composer(props: ComposerProps) {
  const { draftKey, running, queued, info } = props;
  const [text, setText] = useState(() => storage.get(`hame.draft.${draftKey}`) ?? "");
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [dragging, setDragging] = useState(false);
  const local = useRef<HTMLTextAreaElement>(null);
  const area = props.inputRef ?? local;
  const file = useRef<HTMLInputElement>(null);
  const lastEscape = useRef(0);

  // Drafts are kept per session.
  useEffect(() => {
    setText(storage.get(`hame.draft.${draftKey}`) ?? "");
    setAttachments([]);
  }, [draftKey]);
  useEffect(() => {
    storage.set(`hame.draft.${draftKey}`, text ? text : null);
  }, [draftKey, text]);

  useLayoutEffect(() => {
    const element = area.current;
    if (!element) return;
    element.style.height = "auto";
    element.style.height = `${Math.min(element.scrollHeight, 240)}px`;
  }, [text, area]);

  useEffect(() => {
    if (props.autoFocus) area.current?.focus();
  }, [props.autoFocus, draftKey, area]);

  const add = (files: FileList | File[]) => {
    for (const picked of Array.from(files)) {
      const id = `att${++attachmentIds}`;
      const preview = picked.type.startsWith("image/") ? URL.createObjectURL(picked) : undefined;
      setAttachments((list) => [...list, { id, name: picked.name || "pasted image", status: "uploading", preview }]);
      props.onUpload(picked).then(
        (upload) => setAttachments((list) => list.map((a) => (a.id === id ? { ...a, status: "ready", upload } : a))),
        (error: Error) => setAttachments((list) => list.map((a) => (a.id === id ? { ...a, status: "failed", error: error.message } : a))),
      );
    }
  };

  const ready = attachments.filter((a) => a.status === "ready");
  const uploading = attachments.some((a) => a.status === "uploading");
  const canSend = !props.disabled && !uploading && (text.trim() !== "" || (!running && ready.length > 0));

  const send = async () => {
    if (!canSend) return;
    const ok = await props.onSend(text.trim(), running ? [] : ready);
    if (ok) {
      setText("");
      if (!running) setAttachments([]);
    }
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      void send();
    } else if (event.key === "Escape" && running) {
      const now = Date.now();
      if (now - lastEscape.current < 1200) {
        props.onStop();
        lastEscape.current = 0;
      } else {
        lastEscape.current = now;
      }
    }
  };

  const onPaste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    const files = Array.from(event.clipboardData.files);
    if (files.length) {
      event.preventDefault();
      add(files);
    }
  };

  const onDrop = (event: DragEvent) => {
    event.preventDefault();
    setDragging(false);
    if (event.dataTransfer.files.length) add(event.dataTransfer.files);
  };

  const models = info?.models;
  const modelValue = props.model ?? "";
  return (
    <div
      className={`${styles.composer} ${dragging ? styles.dragging : ""}`}
      onDragOver={(event) => {
        event.preventDefault();
        setDragging(true);
      }}
      onDragLeave={() => setDragging(false)}
      onDrop={onDrop}
    >
      {queued.length > 0 && (
        <ul className={styles.queue} aria-label="Queued messages">
          {queued.map((message, index) => (
            <li key={`${index}:${message}`}>
              <span className={styles.queuedTag}>
                {props.queuedFrom?.[index] ? `queued from ${props.queuedFrom[index]}` : "queued"}
              </span>
              <span className={styles.queuedText}>{message}</span>
              <button type="button" onClick={() => props.onWithdraw(index, message)} aria-label={`Remove queued message: ${message}`}>
                <X size={14} aria-hidden />
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className={styles.box}>
        {attachments.length > 0 && (
          <div className={styles.chips}>
            {attachments.map((attachment) => (
              <span key={attachment.id} className={styles.chip} data-status={attachment.status} title={attachment.error}>
                {attachment.preview ? <img src={attachment.preview} alt="" /> : <Paperclip size={13} aria-hidden />}
                <span className={styles.chipName}>{attachment.name}</span>
                {attachment.status === "uploading" && <span className={styles.spinner} aria-label="uploading" />}
                {attachment.status === "failed" && <span className={styles.chipError}>failed</span>}
                <button
                  type="button"
                  onClick={() => setAttachments((list) => list.filter((a) => a.id !== attachment.id))}
                  aria-label={`Remove ${attachment.name}`}
                >
                  <X size={13} aria-hidden />
                </button>
              </span>
            ))}
          </div>
        )}
        <textarea
          ref={area}
          className={styles.input}
          value={text}
          rows={1}
          placeholder={running ? "Add to what hame is doing…" : "Ask hame to do something…"}
          aria-label="Message"
          onChange={(event) => setText(event.target.value)}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
          disabled={props.disabled}
        />
        <div className={styles.controls}>
          <button type="button" className={styles.iconButton} onClick={() => file.current?.click()} aria-label="Attach files" disabled={props.disabled}>
            <Paperclip size={16} aria-hidden />
          </button>
          <input
            ref={file}
            type="file"
            multiple
            hidden
            onChange={(event) => {
              if (event.target.files) add(event.target.files);
              event.target.value = "";
            }}
          />
          <label className={styles.pill} title={props.cwd ?? undefined}>
            <Folder size={14} aria-hidden />
            {props.canChooseFolder && info ? (
              <select
                aria-label="Working folder"
                value={props.cwd ?? info.cwd}
                onChange={(event) => {
                  if (event.target.value === "__other") {
                    const folder = window.prompt("Folder on this computer (absolute path):", props.cwd ?? info.cwd);
                    if (folder) props.onFolder(folder.trim());
                  } else {
                    props.onFolder(event.target.value);
                  }
                }}
              >
                {[...new Set([props.cwd ?? info.cwd, ...info.folders])].map((folder) => (
                  <option key={folder} value={folder}>
                    {folderName(folder)}
                  </option>
                ))}
                <option value="__other">Other folder…</option>
              </select>
            ) : (
              <span className={styles.pillText} aria-label={`Working folder ${props.cwd ?? ""}`}>
                {folderName(props.cwd)}
              </span>
            )}
          </label>
          <label className={styles.pill} title={info?.modes.find((m) => m.name === props.mode)?.description}>
            <Shield size={14} aria-hidden />
            <select
              aria-label="Permission mode"
              value={props.mode}
              disabled={running || !info}
              onChange={(event) => {
                // Full access is confirmed by the host (a second request with its nonce).
                props.onMode(event.target.value as ModeName);
              }}
            >
              {(info?.modes ?? [{ name: props.mode, label: props.mode, description: "" }]).map((mode) => (
                <option key={mode.name} value={mode.name}>
                  {mode.label}
                </option>
              ))}
            </select>
          </label>
          <label className={`${styles.pill} ${styles.modelPill}`} title={`Model: ${props.model?.split(":").pop() ?? "default"}`}>
            <Sparkles size={14} aria-hidden />
            {models && !models.fixed && models.providers.some((p) => p.configured) ? (
              <select
                aria-label="Model"
                value={modelValue}
                disabled={running}
                onChange={(event) => {
                  const [provider, ...rest] = event.target.value.split(":");
                  const model = rest.join(":");
                  props.onModel(provider || null, model || null);
                }}
              >
                <option value="">{models.current ? `Default (${models.current.model})` : "Default model"}</option>
                {models.providers
                  .filter((provider) => provider.configured && provider.model)
                  .map((provider) => (
                    <optgroup key={provider.name} label={provider.name}>
                      <option value={`${provider.name}:${provider.model}`}>{provider.model}</option>
                    </optgroup>
                  ))}
                {props.model && !models.providers.some((p) => `${p.name}:${p.model}` === props.model) && (
                  <option value={props.model}>{props.model.split(":").pop()}</option>
                )}
              </select>
            ) : (
              <span className={styles.pillText}>{props.model?.split(":").pop() ?? (models?.fixed ? "Fixed model" : "Default model")}</span>
            )}
          </label>
          <span className={styles.spacer} />
          {running && (
            <button type="button" className={styles.stop} onClick={props.onStop} aria-label="Stop (Esc twice)" title="Stop (Esc twice)">
              <Square size={13} aria-hidden fill="currentColor" />
            </button>
          )}
          <button
            type="button"
            className={styles.send}
            onClick={() => void send()}
            disabled={!canSend}
            aria-label={running ? "Queue message" : "Send message"}
            title={running ? "Queue for the running turn" : "Send (Enter)"}
          >
            <ArrowUp size={16} aria-hidden />
          </button>
        </div>
      </div>
    </div>
  );
}
