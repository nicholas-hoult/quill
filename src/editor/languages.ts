import type { Extension } from "@codemirror/state";
import type { LanguageSupport } from "@codemirror/language";
import { javascript } from "@codemirror/lang-javascript";
import { json } from "@codemirror/lang-json";
import { markdown } from "@codemirror/lang-markdown";
import { rust } from "@codemirror/lang-rust";
import { python } from "@codemirror/lang-python";
import { html } from "@codemirror/lang-html";
import { css } from "@codemirror/lang-css";
import { langOf } from "../logic/lang.ts";

export function languageSupport(lang: string): LanguageSupport | null {
  switch (lang) {
    case "typescript": return javascript({ typescript: true });
    case "tsx": return javascript({ typescript: true, jsx: true });
    case "javascript": return javascript();
    case "jsx": return javascript({ jsx: true });
    case "json": return json();
    case "markdown": return markdown();
    case "rust": return rust();
    case "python": return python();
    case "html": return html();
    case "css": return css();
    default: return null;
  }
}

/** 围栏里的语言别名（ts、js、py…）归一到 langOf 的名字。 */
export function normalizeLang(l: string): string {
  const m: Record<string, string> = { ts: "typescript", js: "javascript", py: "python", rs: "rust", md: "markdown", sh: "shell", yml: "yaml" };
  const k = l.toLowerCase();
  return m[k] ?? k;
}

export function languageFor(path: string): Extension {
  return languageSupport(langOf(path)) ?? [];
}
