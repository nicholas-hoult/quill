import { useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "../store.ts";
import { ed } from "../editor/controller.ts";
import { fuzzyFilter } from "../logic/fuzzy.ts";
import { FileIcon } from "./Icon.tsx";
import { basename } from "../logic/lang.ts";

export interface Command { id: string; title: string; keys?: string; run: () => void }

export function Palette({ commands }: { commands: Command[] }) {
  const mode = useStore((s) => s.palette)!;
  const { files, root, set, tabs } = useStore();
  const [q, setQ] = useState("");
  const [idx, setIdx] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const close = () => set({ palette: null });

  const items = useMemo(() => {
    if (mode === "commands") return fuzzyFilter(commands, q, (c) => c.title).map((c) => ({ key: c.id, main: c.title, sub: "", keys: c.keys, run: c.run, file: "" }));
    const open = new Set(tabs.map((t) => t.path));
    const pool = q ? files : [...files].sort((a, b) => Number(open.has(`${root}/${b}`)) - Number(open.has(`${root}/${a}`)));
    return fuzzyFilter(pool, q, (f) => f, 60).map((f) => ({
      key: f, main: basename(f), sub: f.includes("/") ? f.slice(0, f.lastIndexOf("/")) : "", keys: undefined as string | undefined, file: f,
      run: () => void ed.open(`${root}/${f}`),
    }));
  }, [mode, q, files, commands, root, tabs]);

  useEffect(() => setIdx(0), [q, mode]);
  useEffect(() => { listRef.current?.querySelector(".pi.on")?.scrollIntoView({ block: "nearest" }); }, [idx]);

  const go = (i: number) => { const it = items[i]; if (!it) return; close(); setTimeout(it.run, 0); };

  return (
    <div className="scrim" onMouseDown={close}>
      <div className="palette" onMouseDown={(e) => e.stopPropagation()}>
        <input autoFocus value={q} placeholder={mode === "files" ? "按文件名搜索…" : "输入命令…"} onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") { e.preventDefault(); setIdx((idx + 1) % Math.max(items.length, 1)); }
            else if (e.key === "ArrowUp") { e.preventDefault(); setIdx((idx - 1 + items.length) % Math.max(items.length, 1)); }
            else if (e.key === "Enter") go(idx);
            else if (e.key === "Escape") close();
          }} />
        <div className="list" ref={listRef}>
          {items.map((it, i) => (
            <button key={it.key} className={`pi${i === idx ? " on" : ""}`} onMouseMove={() => setIdx(i)} onClick={() => go(i)}>
              {it.file ? <FileIcon name={it.main} /> : null}
              <span className="main">{it.main}</span>
              <span className="sub">{it.sub}</span>
              {it.keys ? <kbd>{it.keys}</kbd> : null}
            </button>
          ))}
          {items.length === 0 ? <div className="empty">没有匹配项</div> : null}
        </div>
      </div>
    </div>
  );
}
