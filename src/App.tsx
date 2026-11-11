import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useStore } from "./store.ts";
import { api } from "./ipc.ts";
import { ed } from "./editor/controller.ts";
import { initChat } from "./chat.ts";
import { Icon, FileIcon } from "./components/Icon.tsx";
import { Sidebar } from "./components/Sidebar.tsx";
import { AgentPanel } from "./components/AgentPanel.tsx";
import { Palette, type Command } from "./components/Palette.tsx";
import { TerminalView } from "./components/Terminal.tsx";
import { InlineAssist, type InlineTarget } from "./components/InlineAssist.tsx";
import { SettingsModal } from "./components/Settings.tsx";
import { Welcome } from "./components/Welcome.tsx";
import { basename, relPath } from "./logic/lang.ts";
import { langOf } from "./logic/lang.ts";

const RECENT = "quill.recent";
const readRecents = (): string[] => { try { return JSON.parse(localStorage.getItem(RECENT) ?? "[]"); } catch { return []; } };

function Grip({ side, onDrag }: { side: "left" | "right"; onDrag: (x: number) => void }) {
  const [active, setActive] = useState(false);
  return (
    <div className={`grip${active ? " active" : ""}`} data-side={side}
      onPointerDown={(e) => {
        e.preventDefault();
        setActive(true);
        document.body.classList.add("resizing");
        const move = (ev: PointerEvent) => onDrag(ev.clientX);
        const up = () => { setActive(false); document.body.classList.remove("resizing"); window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", up); };
        window.addEventListener("pointermove", move);
        window.addEventListener("pointerup", up);
      }} />
  );
}

export function App() {
  const s = useStore();
  const { root, tabs, active, set } = s;
  const hostRef = useRef<HTMLDivElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const [inline, setInline] = useState<InlineTarget | null>(null);
  const [recents, setRecents] = useState<string[]>(readRecents);

  // ---- 主题 ----
  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: light)");
    const apply = () => document.documentElement.setAttribute("data-theme", s.theme === "system" ? (mq.matches ? "light" : "dark") : s.theme);
    apply();
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  }, [s.theme]);

  // ---- 初始化 ----
  useEffect(() => {
    ed.onInline = () => {
      const sel = ed.selection(), box = wrapRef.current?.getBoundingClientRect();
      if (!sel || !box) return;
      setInline({ path: sel.path, from: sel.from, to: sel.to, text: sel.text, x: (sel.coords?.x ?? box.left + 60) - box.left, y: (sel.coords?.y ?? box.top + 60) - box.top });
    };
    void initChat();
    void api.getSettings().then((st) => set({ settings: st }));
    const last = readRecents()[0];
    if (last) void openRoot(last);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 编辑器挂载点只在打开文件夹后才存在
  useEffect(() => {
    if (root && hostRef.current && ed.host !== hostRef.current) {
      ed.destroy();
      ed.mount(hostRef.current);
    }
  }, [root]);

  const refresh = useCallback(async () => {
    const r = useStore.getState().root;
    if (!r) return;
    const [files, changes, branch] = await Promise.all([api.listFiles(r), api.gitStatus(r), api.gitBranch(r)]);
    set({ files, changes, branch });
  }, [set]);

  useEffect(() => {
    const f = () => void refresh();
    window.addEventListener("quill:saved", f);
    window.addEventListener("focus", f);
    return () => { window.removeEventListener("quill:saved", f); window.removeEventListener("focus", f); };
  }, [refresh]);

  async function openRoot(path: string) {
    for (const t of [...useStore.getState().tabs]) await ed.close(t.path, true);
    set({ root: path, files: [], changes: [], branch: null, sideView: "files" });
    const list = [path, ...readRecents().filter((x) => x !== path)].slice(0, 10);
    try { localStorage.setItem(RECENT, JSON.stringify(list)); } catch { /* 忽略 */ }
    setRecents(list);
    void refresh();
  }

  const pickFolder = useCallback(async () => {
    const p = await openDialog({ directory: true, multiple: false });
    if (typeof p === "string") await openRoot(p);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ---- 命令与快捷键 ----
  const commands: Command[] = useMemo(() => [
    { id: "open", title: "打开文件夹…", keys: "⌘O", run: () => void pickFolder() },
    { id: "file", title: "跳转到文件", keys: "⌘P", run: () => set({ palette: "files" }) },
    { id: "save", title: "保存", keys: "⌘S", run: () => void ed.save() },
    { id: "side", title: "切换侧栏", keys: "⌘B", run: () => set({ sidebar: !useStore.getState().sidebar }) },
    { id: "term", title: "切换终端", keys: "⌘J", run: () => set({ terminal: !useStore.getState().terminal }) },
    { id: "agent", title: "切换 AI 对话", keys: "⌘L", run: () => set({ agent: !useStore.getState().agent }) },
    { id: "find", title: "在项目中搜索", keys: "⇧⌘F", run: () => set({ sidebar: true, sideView: "search" }) },
    { id: "git", title: "查看 Git 改动", run: () => set({ sidebar: true, sideView: "git" }) },
    { id: "inline", title: "就地改写（Inline Assist）", keys: "⌘K", run: () => ed.onInline?.() },
    { id: "settings", title: "打开设置", keys: "⌘,", run: () => set({ settingsOpen: true }) },
    { id: "theme", title: "切换深色 / 浅色", run: () => { const t = document.documentElement.getAttribute("data-theme") === "dark" ? "light" : "dark"; set({ theme: t }); try { localStorage.setItem("quill.theme", t); } catch { /* 忽略 */ } } },
    { id: "close", title: "关闭当前标签页", keys: "⌘W", run: () => { const a = useStore.getState().active; if (a) void ed.close(a); } },
  ], [pickFolder, set]);

  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (!e.metaKey) return;
      const st = useStore.getState();
      const k = e.key.toLowerCase();
      const hit = (fn: () => void) => { e.preventDefault(); fn(); };
      if (k === "p" && e.shiftKey) hit(() => set({ palette: "commands" }));
      else if (k === "p") hit(() => set({ palette: "files" }));
      else if (k === "f" && e.shiftKey) hit(() => set({ sidebar: true, sideView: "search" }));
      else if (k === "b") hit(() => set({ sidebar: !st.sidebar }));
      else if (k === "j") hit(() => set({ terminal: !st.terminal }));
      else if (k === "l") hit(() => { if (!st.agent) set({ agent: true }); setTimeout(() => window.dispatchEvent(new Event("quill:focus-agent")), 60); });
      else if (k === "o") hit(() => void pickFolder());
      else if (k === ",") hit(() => set({ settingsOpen: true }));
      else if (k === "w") hit(() => { if (st.active) void ed.close(st.active); });
      else if (k === "enter" && st.reviewChunks > 0) hit(() => ed.endReview("accept"));
      else if (e.altKey && (e.key === "ArrowRight" || e.key === "ArrowLeft") && st.tabs.length > 1) {
        hit(() => {
          const i = st.tabs.findIndex((t) => t.path === st.active);
          const n = (i + (e.key === "ArrowRight" ? 1 : -1) + st.tabs.length) % st.tabs.length;
          void ed.open(st.tabs[n].path);
        });
      }
    };
    window.addEventListener("keydown", h, true);
    return () => window.removeEventListener("keydown", h, true);
  }, [pickFolder, set]);

  const title = root ? basename(root) : "Quill";
  const speed = s.settings.provider === "mock" ? "演示模式" : s.settings.model;

  return (
    <div className="app">
      <div className="titlebar" data-tauri-drag-region>
        <button className={`icon-btn${s.sidebar ? " on" : ""}`} title="侧栏 ⌘B" onClick={() => set({ sidebar: !s.sidebar })}><Icon name="sidebar" /></button>
        <div className="title" data-tauri-drag-region><b>{title}</b>{active && root ? <> · {relPath(root, active)}</> : null}</div>
        <div className="spacer" data-tauri-drag-region />
        <button className="pill" onClick={() => set({ palette: "files" })}><Icon name="search" />搜索文件、运行命令<kbd>⌘P</kbd></button>
        <div className="spacer" data-tauri-drag-region />
        <button className={`icon-btn${s.terminal ? " on" : ""}`} title="终端 ⌘J" onClick={() => set({ terminal: !s.terminal })}><Icon name="terminal" /></button>
        <button className={`icon-btn${s.agent ? " on" : ""}`} title="AI 对话 ⌘L" onClick={() => set({ agent: !s.agent })}><Icon name="agent" /></button>
        <button className="icon-btn" title="设置 ⌘," onClick={() => set({ settingsOpen: true })}><Icon name="settings" /></button>
      </div>

      <div className="body">
        <div className={`pane left${s.sidebar && root ? "" : " closed"}`} style={{ width: s.sideW }}>
          {root ? <Sidebar /> : null}
        </div>
        {s.sidebar && root ? <Grip side="left" onDrag={(x) => { const w = Math.min(520, Math.max(180, x)); set({ sideW: w }); try { localStorage.setItem("quill.sideW", String(w)); } catch { /* 忽略 */ } }} /> : null}

        <div className="center">
          {!root ? <Welcome recents={recents} onOpen={() => void pickFolder()} onPick={(p) => void openRoot(p)} /> : (
            <>
              <div className="tabs">
                {tabs.map((t) => (
                  <div key={t.path} className={`tab${t.path === active ? " on" : ""}${t.dirty ? " dirty" : ""}`} onClick={() => void ed.open(t.path)}
                    onAuxClick={(e) => { if (e.button === 1) void ed.close(t.path); }}>
                    <FileIcon name={basename(t.path)} />
                    <span className="tname">{basename(t.path)}</span>
                    <span className="dot" />
                    <button className="x" onClick={(e) => { e.stopPropagation(); void ed.close(t.path); }}><Icon name="x" /></button>
                  </div>
                ))}
              </div>
              {active ? <div className="crumbs">{relPath(root, active).split("/").map((p, i, a) => <span key={i}>{i === a.length - 1 ? <b>{p}</b> : p}{i < a.length - 1 ? " ›" : ""}</span>)}</div> : null}
              <div className="editor-wrap" ref={wrapRef}>
                <div className={`cm-host${active ? "" : " hidden"}`} ref={hostRef} />
                {!active ? (
                  <div className="welcome" style={{ position: "absolute", inset: 0 }}>
                    <div className="col">
                      <h1 style={{ fontSize: 30 }}>选一个文件开始<em>.</em></h1>
                      <div className="tag">⌘P 跳转文件 · ⌘L 和 AI 对话</div>
                    </div>
                  </div>
                ) : null}
                {s.reviewChunks > 0 ? (
                  <div className="review-bar">
                    <span className="lbl">审阅 AI 改动 · <b>{s.reviewChunks}</b> 处</span>
                    <button className="btn sm" onClick={() => ed.endReview("reject")}>全部拒绝</button>
                    <button className="btn sm primary" onClick={() => ed.endReview("accept")}>全部接受 ⌘↵</button>
                  </div>
                ) : null}
                {inline ? <InlineAssist target={inline} onClose={() => { setInline(null); ed.focus(); }} /> : null}
              </div>
              <div className="hgrip" style={{ display: s.terminal ? "block" : "none" }}
                onPointerDown={(e) => {
                  e.preventDefault();
                  const startY = e.clientY, startH = s.termH;
                  document.body.classList.add("resizing");
                  const move = (ev: PointerEvent) => { const h = Math.min(640, Math.max(120, startH + startY - ev.clientY)); set({ termH: h }); };
                  const up = () => { document.body.classList.remove("resizing"); window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", up); };
                  window.addEventListener("pointermove", move); window.addEventListener("pointerup", up);
                }} />
              <div className={`termwrap${s.terminal ? "" : " closed"}`} style={{ height: s.termH }}>
                <div className="termhead"><Icon name="terminal" />终端<span className="spacer" /><button className="icon-btn" onClick={() => set({ terminal: false })}><Icon name="x" /></button></div>
                {s.terminal ? <TerminalView key={root} /> : null}
              </div>
            </>
          )}
        </div>

        {s.agent ? <Grip side="right" onDrag={(x) => { const w = Math.min(760, Math.max(320, window.innerWidth - x)); set({ agentW: w }); try { localStorage.setItem("quill.agentW", String(w)); } catch { /* 忽略 */ } }} /> : null}
        <div className={`pane right${s.agent ? "" : " closed"}`} style={{ width: s.agentW }}>
          <AgentPanel />
        </div>
      </div>

      <div className="statusbar">
        {s.branch ? <span className="item"><Icon name="branch" />{s.branch}</span> : null}
        {s.changes.length ? <button className="item" onClick={() => set({ sidebar: true, sideView: "git" })}>{s.changes.length} 处改动</button> : null}
        <span className="spacer" />
        {active ? <><span className="item">行 {s.cursor.line}，列 {s.cursor.col}</span><span className="item">{langOf(active)}</span></> : null}
        <button className="item" onClick={() => set({ settingsOpen: true })}><span className={`livedot${s.settings.provider === "mock" ? " mock" : ""}`} />{speed}</button>
      </div>

      {s.palette ? <Palette key={s.palette} commands={commands} /> : null}
      {s.settingsOpen ? <SettingsModal /> : null}
      <div className="toasts">{s.toasts.map((t) => <div key={t.id} className={`toast ${t.kind}`}>{t.text}</div>)}</div>
    </div>
  );
}
