import { useEffect, useRef, useState } from "react";
import { api, listen } from "../ipc.ts";
import { ed, type StreamHandle } from "../editor/controller.ts";
import { useStore } from "../store.ts";
import { firstCode } from "../logic/blocks.ts";
import { INLINE_PROMPT } from "../logic/context.ts";
import { Icon } from "./Icon.tsx";
import { basename, langOf } from "../logic/lang.ts";

export interface InlineTarget { path: string; from: number; to: number; text: string; x: number; y: number }

/** Inline Assist：选区 + 指令 → 流式改写 → 进入逐块审阅（对标 Zed 的 Ctrl+Enter）。 */
export function InlineAssist({ target, onClose }: { target: InlineTarget; onClose: () => void }) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const idRef = useRef("");
  const handle = useRef<StreamHandle | null>(null);
  const unsubs = useRef<(() => void)[]>([]);

  useEffect(() => () => unsubs.current.forEach((u) => u()), []);

  const cancel = () => {
    if (busy) { void api.aiCancel(idRef.current); handle.current?.abort(); }
    onClose();
  };

  const run = async () => {
    const instruction = text.trim();
    if (!instruction || busy) return;
    setBusy(true);
    const id = "inline-" + Math.random().toString(36).slice(2, 8);
    idRef.current = id;
    const h = ed.beginStream(target.from, target.to);
    if (!h) { setBusy(false); return; }
    handle.current = h;
    let acc = "";
    unsubs.current.push(await listen<{ id: string; delta: string }>("ai-chunk", (e) => {
      if (e.payload.id !== id) return;
      acc += e.payload.delta;
      const code = firstCode(acc);
      if (code !== null) h.update(code);
    }));
    unsubs.current.push(await listen<{ id: string }>("ai-done", (e) => {
      if (e.payload.id !== id) return;
      if (firstCode(acc) === null) { h.abort(); useStore.getState().toast("模型没有返回代码"); } else h.finish();
      onClose();
    }));
    unsubs.current.push(await listen<{ id: string; message: string }>("ai-error", (e) => {
      if (e.payload.id !== id) return;
      h.abort(); useStore.getState().toast(e.payload.message, "error"); onClose();
    }));
    const where = `${basename(target.path)}（${langOf(target.path)}）`;
    const body = target.text
      ? `${instruction}\n\n选中的代码，来自 ${where}：\n\`\`\`\n${target.text}\n\`\`\``
      : `${instruction}\n\n这是要插入到 ${where} 光标处的新代码，只输出要插入的代码。\n选中的代码：\n\`\`\`\n\n\`\`\``;
    await api.aiStream(id, INLINE_PROMPT, [{ role: "user", content: body }]);
  };

  return (
    <div className="inline-assist" style={{ left: Math.max(16, target.x), top: target.y + 8 }}>
      <div className="row1">
        <span style={{ color: "var(--accent)", display: "grid" }}><Icon name="agent" /></span>
        <input autoFocus value={text} disabled={busy} placeholder={target.text ? "描述怎么改写选中的代码…" : "描述要在这里生成什么…"}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter" && !e.nativeEvent.isComposing) void run(); if (e.key === "Escape") cancel(); }} />
        <button className="icon-btn" onClick={cancel}><Icon name="x" /></button>
      </div>
      {busy ? <div className="shimmer" /> : <div className="hint"><span>↵ 生成</span><span>Esc 取消</span><span>生成后可逐块接受 / 拒绝</span></div>}
    </div>
  );
}
