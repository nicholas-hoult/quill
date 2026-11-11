import { Fragment, type ReactNode } from "react";
import { highlightTree } from "@lezer/highlight";
import { quillHighlight } from "../editor/theme.ts";
import { languageSupport, normalizeLang } from "../editor/languages.ts";
import { langOf } from "../logic/lang.ts";

/** 静态高亮：和编辑器共用同一套 HighlightStyle，所以聊天里的代码和编辑器里的配色完全一致。 */
export function Highlighted({ code, lang, path }: { code: string; lang: string; path?: string }) {
  const name = lang ? normalizeLang(lang) : path ? langOf(path) : "";
  const support = languageSupport(name);
  if (!support || code.length > 60_000) return <>{code}</>;
  const tree = support.language.parser.parse(code);
  const out: ReactNode[] = [];
  let pos = 0;
  highlightTree(tree, quillHighlight, (from, to, cls) => {
    if (from > pos) out.push(<Fragment key={out.length}>{code.slice(pos, from)}</Fragment>);
    out.push(<span key={out.length} className={cls}>{code.slice(from, to)}</span>);
    pos = to;
  });
  if (pos < code.length) out.push(<Fragment key={out.length}>{code.slice(pos)}</Fragment>);
  return <>{out}</>;
}
