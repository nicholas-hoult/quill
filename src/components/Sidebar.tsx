import { useCallback, useEffect, useRef, useState } from "react";
import { api, type Entry, type Hit } from "../ipc.ts";
import { useStore, type SideView } from "../store.ts";
import { ed } from "../editor/controller.ts";
import { Icon, FileIcon } from "./Icon.tsx";
import { basename, relPath } from "../logic/lang.ts";
import { ask } from "@tauri-apps/plugin-dialog";

function Node({ entry, depth, reload, version }: { entry: Entry; depth: number; reload: () => void; version: number }) {
  const [open, setOpen] = useState(false);
  const [kids, setKids] = useState<Entry[] | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [creating, setCreating] = useState<null | "file" | "dir">(null);
  const active = useStore((s) => s.active);
  const changes = useStore((s) => s.changes);
  const root = useStore((s) => s.root)!;
  const rel = relPath(root, entry.path);
  const st = changes.find((c) => c.path === rel || (entry.is_dir && c.path.startsWith(rel + "/")))?.status;

  const load = useCallback(() => api.readDir(entry.path).then(setKids).catch(() => setKids([])), [entry.path]);
  useEffect(() => { if (open) void load(); }, [open, version, load]);

  const menu = async (e: React.MouseEvent) => {
    e.preventDefault();
    const choice = await ask(`对「${entry.name}」操作`, { title: "文件", okLabel: "重命名", cancelLabel: "删除", kind: "info" });
    if (choice) setRenaming(true);
    else {
      const ok = await ask(`删除「${entry.name}」？此操作不可撤销。`, { title: "删除", kind: "warning", okLabel: "删除", cancelLabel: "取消" });
      if (ok) { await api.deleteEntry(entry.path); ed.dropPrefix(entry.path); reload(); }
    }
  };

  const submitRename = async (v: string) => {
    setRenaming(false);
    const name = v.trim();
    if (!name || name === entry.name) return;
    const to = entry.path.slice(0, entry.path.lastIndexOf("/") + 1) + name;
    try { await api.renameEntry(entry.path, to); ed.dropPrefix(entry.path); reload(); } catch (e) { useStore.getState().toast(String(e), "error"); }
  };

  const submitCreate = async (v: string) => {
    const kind = creating; setCreating(null);
    const name = v.trim();
    if (!name || !kind) return;
    const p = `${entry.path}/${name}`;
    try { await api.createEntry(p, kind === "dir"); await load(); reload(); if (kind === "file") void ed.open(p); } catch (e) { useStore.getState().toast(String(e), "error"); }
  };

  return (
    <div>
      <div
        className={`row${active === entry.path ? " sel" : ""}`}
        style={{ paddingLeft: 6 + depth * 14 }}
        onClick={() => (entry.is_dir ? setOpen(!open) : void ed.open(entry.path))}
        onContextMenu={(e) => void menu(e)}
        onDoubleClick={(e) => { if (!entry.is_dir) { e.stopPropagation(); } }}
      >
        <span className={`chev${open ? " open" : ""}`} style={{ visibility: entry.is_dir ? "visible" : "hidden" }}><Icon name="chevron" /></span>
        {entry.is_dir ? <Icon name="folder" className="" /> : <FileIcon name={entry.name} />}
        {renaming
          ? <input className="rename-input" autoFocus defaultValue={entry.name} onClick={(e) => e.stopPropagation()}
              onBlur={(e) => void submitRename(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); if (e.key === "Escape") setRenaming(false); }} />
          : <span className="name">{entry.name}</span>}
        {st ? <span className={`st ${st}`}>{st === "?" ? "U" : st}</span> : null}
        {entry.is_dir ? (
          <span className="icon-btn" style={{ width: 20, height: 20, marginLeft: st ? 0 : "auto", opacity: 0.0 }} onMouseEnter={(e) => (e.currentTarget.style.opacity = "1")} onMouseLeave={(e) => (e.currentTarget.style.opacity = "0")}
            title="新建文件" onClick={(e) => { e.stopPropagation(); setOpen(true); setCreating("file"); }}><Icon name="plus" /></span>
        ) : null}
      </div>
      {open ? (
        <div className="children">
          {creating ? (
            <div className="row" style={{ paddingLeft: 6 + (depth + 1) * 14 + 18 }}>
              <input className="rename-input" autoFocus placeholder="文件名（以 / 结尾创建文件夹）"
                onBlur={(e) => { const v = e.target.value; if (v.endsWith("/")) { setCreating("dir"); void submitCreate(v.slice(0, -1)); } else void submitCreate(v); }}
                onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); if (e.key === "Escape") setCreating(null); }} />
            </div>
          ) : null}
          {(kids ?? []).map((k) => <Node key={k.path} entry={k} depth={depth + 1} reload={reload} version={version} />)}
        </div>
      ) : null}
    </div>
  );
}

function FileTree() {
  const root = useStore((s) => s.root)!;
  const [top, setTop] = useState<Entry[]>([]);
  const [version, setVersion] = useState(0);
  const reload = useCallback(() => { setVersion((v) => v + 1); window.dispatchEvent(new Event("quill:saved")); }, []);
  useEffect(() => { void api.readDir(root).then(setTop); }, [root, version]);
  useEffect(() => {
    const f = () => setVersion((v) => v + 1);
    window.addEventListener("quill:saved", f);
    return () => window.removeEventListener("quill:saved", f);
  }, []);
  return (
    <div className="tree">
      {top.map((e) => <Node key={e.path} entry={e} depth={0} reload={reload} version={version} />)}
      {top.length === 0 ? <div className="empty">空文件夹</div> : null}
    </div>
  );
}

function SearchPanel() {
  const root = useStore((s) => s.root)!;
  const [q, setQ] = useState("");
  const [hits, setHits] = useState<Hit[]>([]);
  const [busy, setBusy] = useState(false);
  const seq = useRef(0);
  useEffect(() => {
    if (!q.trim()) { setHits([]); return; }
    const my = ++seq.current;
    setBusy(true);
    const t = setTimeout(() => {
      void api.searchText(root, q, false).then((h) => { if (my === seq.current) { setHits(h); setBusy(false); } });
    }, 180);
    return () => clearTimeout(t);
  }, [q, root]);
  const groups = new Map<string, Hit[]>();
  for (const h of hits) groups.set(h.path, [...(groups.get(h.path) ?? []), h]);
  return (
    <>
      <div className="search-box"><input className="field" autoFocus placeholder="在项目中搜索" value={q} onChange={(e) => setQ(e.target.value)} /></div>
      <div className="tree">
        {q && !busy && hits.length === 0 ? <div className="empty">没有结果</div> : null}
        {[...groups].map(([p, hs]) => (
          <div key={p}>
            <div className="group-title">{basename(p)}<span>{p}</span></div>
            {hs.map((h) => (
              <button key={h.line + p} className="hit" onClick={() => void ed.open(`${root}/${p}`, h.line)}>
                <span className="ln">{h.line}</span>{h.text.trim()}
              </button>
            ))}
          </div>
        ))}
      </div>
    </>
  );
}

function GitPanel() {
  const { changes, root, branch } = useStore();
  return (
    <div className="tree">
      <div className="group-title">{branch ?? "非 Git 仓库"}<span>{changes.length} 处改动</span></div>
      {changes.map((c) => (
        <div key={c.path} className="row" onClick={() => void ed.open(`${root}/${c.path}`)}>
          <FileIcon name={basename(c.path)} /><span className="name">{c.path}</span><span className={`st ${c.status}`}>{c.status === "?" ? "U" : c.status}</span>
        </div>
      ))}
      {changes.length === 0 ? <div className="empty">工作区干净</div> : null}
    </div>
  );
}

export function Sidebar() {
  const { sideView, changes, set } = useStore();
  const tabs: [SideView, string][] = [["files", "文件"], ["search", "搜索"], ["git", "改动"]];
  return (
    <div className="inner">
      <div className="side-tabs">
        {tabs.map(([k, l]) => (
          <button key={k} className={`side-tab${sideView === k ? " on" : ""}`} onClick={() => set({ sideView: k })}>
            {l}{k === "git" && changes.length ? <span className="badge">{changes.length}</span> : null}
          </button>
        ))}
      </div>
      {sideView === "files" ? <FileTree /> : sideView === "search" ? <SearchPanel /> : <GitPanel />}
    </div>
  );
}
