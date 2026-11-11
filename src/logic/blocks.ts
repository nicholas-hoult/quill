// 为什么单独测：模型回复边流式边解析，围栏可能没闭合、info 串写法不统一。
// 解析错了，UI 上表现为「应用」按钮找不到目标文件，或半截代码被当成正文。
export interface CodeBlock {
  lang: string;
  path?: string;
  code: string;
}

export type Segment =
  | { kind: "text"; text: string }
  | { kind: "code"; block: CodeBlock; closed: boolean };

/** 解析围栏 info 串：支持 `ts path=a/b.ts`、`ts:a/b.ts`、`path=a/b.ts`、`a/b.ts`。 */
export function parseInfo(info: string): { lang: string; path?: string } {
  const s = info.trim();
  if (!s) return { lang: "" };
  const kv = /(?:^|\s)(?:path|file|filename)=("[^"]+"|\S+)/.exec(s);
  const first = s.split(/\s+/)[0];
  if (kv) {
    const path = kv[1].replace(/^"|"$/g, "");
    const lang = first.includes("=") ? "" : first;
    return { lang, path };
  }
  const colon = /^([\w+#-]+):(.+)$/.exec(first);
  if (colon) return { lang: colon[1], path: colon[2] };
  if (first.includes("/") || /\.\w+$/.test(first)) return { lang: "", path: first };
  return { lang: first };
}

export function parseSegments(md: string): Segment[] {
  const out: Segment[] = [];
  const lines = md.split("\n");
  let text: string[] = [];
  let code: string[] | null = null;
  let info = "";
  const flushText = () => {
    const t = text.join("\n");
    if (t.trim()) out.push({ kind: "text", text: t.replace(/^\n+|\n+$/g, "") });
    text = [];
  };
  for (const line of lines) {
    if (code === null) {
      const m = /^```(.*)$/.exec(line);
      if (m) {
        flushText();
        code = [];
        info = m[1];
      } else text.push(line);
    } else if (/^```\s*$/.test(line)) {
      out.push({ kind: "code", block: { ...parseInfo(info), code: code.join("\n") }, closed: true });
      code = null;
    } else code.push(line);
  }
  if (code !== null) out.push({ kind: "code", block: { ...parseInfo(info), code: code.join("\n") }, closed: false });
  else flushText();
  return out;
}

/** 取第一个代码块的内容（Inline Assist 用），流式中未闭合也返回已有部分。 */
export function firstCode(md: string): string | null {
  const seg = parseSegments(md).find((s) => s.kind === "code");
  return seg && seg.kind === "code" ? seg.block.code : null;
}
