import { useEffect, useRef } from "react";
import { FileText, ImageOff, X } from "lucide-react";
import type { ArtifactRef } from "../model/types";
import { useArtifactUrl, useServices } from "../state/util";
import styles from "./Media.module.css";

function label(artifact: ArtifactRef): string {
  const size = artifact.width && artifact.height ? ` ${artifact.width}×${artifact.height}` : "";
  return `${artifact.name ?? (artifact.capture?.target ? `screenshot of ${artifact.capture.target}` : "image")}${size}`;
}

/** A thumbnail that opens the image in the lightbox. */
export function Thumb({ artifact, large = false }: { artifact: ArtifactRef; large?: boolean }) {
  const { url, error } = useArtifactUrl(artifact.sha256);
  const { openImage } = useServices();
  const alt = label(artifact);
  if (error) {
    return (
      <span className={styles.missing} title={error}>
        <ImageOff size={14} aria-hidden /> image unavailable
      </span>
    );
  }
  return (
    <button
      type="button"
      className={`${styles.thumb} ${large ? styles.large : ""}`}
      onClick={() => openImage({ sha256: artifact.sha256, alt })}
      aria-label={`Open ${alt}`}
      style={artifact.width && artifact.height ? { aspectRatio: `${artifact.width} / ${artifact.height}` } : undefined}
    >
      {url ? <img src={url} alt={alt} loading="lazy" /> : <span className={styles.loading} />}
    </button>
  );
}

/** A stored file that is not an image: a download chip. */
export function FileChip({ artifact }: { artifact: ArtifactRef }) {
  const { url } = useArtifactUrl(artifact.sha256);
  const name = artifact.name ?? `file ${artifact.sha256.slice(0, 8)}`;
  return (
    <a className={styles.chip} href={url ?? undefined} download={name} aria-disabled={!url}>
      <FileText size={14} aria-hidden />
      <span>{name}</span>
      <span className={styles.meta}>{Math.max(1, Math.round(artifact.bytes / 1024))} KB</span>
    </a>
  );
}

export function Lightbox({ image, onClose }: { image: { sha256: string; alt: string }; onClose: () => void }) {
  const { url } = useArtifactUrl(image.sha256);
  const close = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const before = document.activeElement as HTMLElement | null;
    close.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      before?.focus?.();
    };
  }, [onClose]);
  return (
    <div className={styles.lightbox} role="dialog" aria-modal="true" aria-label={image.alt} onClick={onClose}>
      <button ref={close} type="button" className={styles.close} onClick={onClose} aria-label="Close image">
        <X size={18} aria-hidden />
      </button>
      {url && <img src={url} alt={image.alt} onClick={(event) => event.stopPropagation()} />}
      <div className={styles.caption}>{image.alt}</div>
    </div>
  );
}
