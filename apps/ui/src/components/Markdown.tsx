// Markdown (CommonMark + GFM) through micromark, turned into React
// elements. micromark escapes raw HTML and drops dangerous link protocols;
// the conversion below keeps only known tags and attributes on top of that.

import { createElement, memo, useEffect, useMemo, useState, type ReactNode } from "react";
import { micromark } from "micromark";
import { gfm, gfmHtml } from "micromark-extension-gfm";
import { Check, Copy } from "lucide-react";
import styles from "./Markdown.module.css";

const ALLOWED = new Set([
  "p", "br", "hr", "em", "strong", "del", "code", "pre", "blockquote", "ul", "ol", "li",
  "h1", "h2", "h3", "h4", "h5", "h6", "a", "table", "thead", "tbody", "tr", "th", "td", "input", "sup", "sub", "section",
]);

type Highlighter = (code: string, language: string) => string | null;
let highlighter: Promise<Highlighter> | null = null;

function loadHighlighter(): Promise<Highlighter> {
  highlighter ??= import("./highlight").then((module) => module.highlight);
  return highlighter;
}

export function CodeBlock({ code, language, live }: { code: string; language: string; live?: boolean }) {
  const [html, setHtml] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (live || !language) return;
    let current = true;
    void loadHighlighter().then((highlight) => {
      if (current) setHtml(highlight(code, language));
    });
    return () => {
      current = false;
    };
  }, [code, language, live]);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard refused (insecure context); nothing to do.
    }
  };
  return (
    <div className={styles.code}>
      <div className={styles.codeBar}>
        <span>{language || "text"}</span>
        <button type="button" onClick={copy} aria-label={copied ? "Copied" : "Copy code"} className={styles.copy}>
          {copied ? <Check size={14} aria-hidden /> : <Copy size={14} aria-hidden />}
          <span>{copied ? "Copied" : "Copy"}</span>
        </button>
      </div>
      <pre>{html !== null && !live ? <code dangerouslySetInnerHTML={{ __html: html }} /> : <code>{code}</code>}</pre>
    </div>
  );
}

function convert(node: Node, key: string, live: boolean): ReactNode {
  if (node.nodeType === Node.TEXT_NODE) return node.textContent;
  if (node.nodeType !== Node.ELEMENT_NODE) return null;
  const element = node as Element;
  const tag = element.tagName.toLowerCase();
  const children = () => Array.from(element.childNodes).map((child, n) => convert(child, `${key}.${n}`, live));
  if (tag === "pre") {
    const code = element.querySelector("code");
    const language = /language-([\w+-]+)/.exec(code?.className ?? "")?.[1] ?? "";
    return <CodeBlock key={key} code={(code ?? element).textContent?.replace(/\n$/, "") ?? ""} language={language} live={live} />;
  }
  if (!ALLOWED.has(tag)) return <span key={key}>{children()}</span>;
  const props: Record<string, unknown> = { key };
  if (tag === "a") {
    const href = element.getAttribute("href") ?? "";
    if (/^(https?:|mailto:|#)/i.test(href)) {
      props.href = href;
      if (!href.startsWith("#")) {
        props.target = "_blank";
        props.rel = "noreferrer noopener";
      }
    }
  }
  if (tag === "input") {
    return <input key={key} type="checkbox" checked={element.hasAttribute("checked")} disabled readOnly aria-label="task item" />;
  }
  if (tag === "ol" && element.getAttribute("start")) props.start = Number(element.getAttribute("start"));
  if ((tag === "td" || tag === "th") && element.getAttribute("align")) props.style = { textAlign: element.getAttribute("align") };
  if (tag === "br" || tag === "hr") return createElement(tag, props);
  if (tag === "table") return <div key={key} className={styles.table}>{createElement("table", {}, children())}</div>;
  return createElement(tag, props, children());
}

export function renderMarkdown(text: string, live = false): ReactNode {
  const html = micromark(text, { extensions: [gfm()], htmlExtensions: [gfmHtml()] });
  const doc = new DOMParser().parseFromString(`<body>${html}</body>`, "text/html");
  return Array.from(doc.body.childNodes).map((child, n) => convert(child, String(n), live));
}

export const Markdown = memo(function Markdown({ text, live = false }: { text: string; live?: boolean }) {
  const content = useMemo(() => renderMarkdown(text, live), [text, live]);
  return <div className={styles.markdown}>{content}</div>;
});
