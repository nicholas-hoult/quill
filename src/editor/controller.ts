// 编辑器控制器：CodeMirror 视图不放进 React 渲染循环，每个标签页保留自己的 EditorState
// （撤销历史、选区、折叠都随之保留），切换时只做 setState，所以切标签是即时的。
import { EditorState, Compartment, EditorSelection, type Extension } from "@codemirror/state";
import {
  EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection, dropCursor,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching, indentOnInput, indentUnit, foldGutter, foldKeymap } from "@codemirror/language";
import { closeBrackets, closeBracketsKeymap, autocompletion, completionKeymap } from "@codemirror/autocomplete";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import { unifiedMergeView, getChunks, getOriginalDoc } from "@codemirror/merge";
import { ask } from "@tauri-apps/plugin-dialog";
import { api } from "../ipc.ts";
import { useStore } from "../store.ts";
import { quillTheme } from "./theme.ts";
import { languageFor } from "./languages.ts";
import { gitGutter, setHead } from "./gitGutter.ts";
import { langOf, relPath } from "../logic/lang.ts";

const reviewComp = new Compartment();
function chunkButton(type: "reject" | "accept", action: (e: MouseEvent) => void): HTMLElement {
  const b = document.createElement("button");
  b.className = `chunk-btn ${type}`;
  b.textContent = type === "accept" ? "接受" : "拒绝";
  b.onmousedown = (e) => { e.preventDefault(); action(e); };
  return b;
}
const reviewExt = (original: string): Extension =>
  unifiedMergeView({ original, mergeControls: chunkButton, gutter: true, highlightChanges: true, syntaxHighlightDeletions: true });

export interface StreamHandle {
  update(text: string): void;
  finish(): void;
  abort(): void;
}

class Controller {
  view: EditorView | null = null;
  host: HTMLElement | null = null;
  private states = new Map<string, EditorState>();
  private saved = new Map<string, string>();
  private scroll = new Map<string, number>();
  private reviewing = new Set<string>();
  private busy = false;
  onInline: (() => void) | null = null;

  mount(parent: HTMLElement) {
    this.host = parent;
    this.view = new EditorView({ state: EditorState.create({ doc: "" }), parent });
  }

  destroy() {
    this.host = null;
    this.view?.destroy();
    this.view = null;
  }

  private extensions(path: string): Extension[] {
    const wrap = langOf(path) === "markdown" ? [EditorView.lineWrapping] : [];
    return [
      lineNumbers(), foldGutter({ openText: "⌄", closedText: "›" }), gitGutter(),
      highlightActiveLine(), highlightActiveLineGutter(), history(),
      drawSelection({ cursorBlinkRate: 1200 }), dropCursor(), indentOnInput(), bracketMatching(), closeBrackets(),
      autocompletion(), highlightSelectionMatches(), indentUnit.of("  "), EditorState.tabSize.of(2),
      languageFor(path), quillTheme, wrap, reviewComp.of([]),
      keymap.of([
        { key: "Mod-s", preventDefault: true, run: () => { void this.save(); return true; } },
        { key: "Mod-k", preventDefault: true, run: () => { this.onInline?.(); return true; } },
        { key: "Ctrl-Enter", preventDefault: true, run: () => { this.onInline?.(); return true; } },
        ...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, ...foldKeymap, ...completionKeymap,
        indentWithTab,
      ]),
      EditorView.updateListener.of((u) => {
        const st = useStore.getState();
        if (u.docChanged && st.active) this.refreshDirty(st.active, u.state);
        if (u.docChanged || u.selectionSet) {
          const h = u.state.selection.main.head;
          const line = u.state.doc.lineAt(h);
          useStore.getState().set({ cursor: { line: line.number, col: h - line.from + 1 } });
        }
        if (st.active && this.reviewing.has(st.active)) {
          const n = getChunks(u.state)?.chunks.length ?? 0;
          if (n !== st.reviewChunks) useStore.getState().set({ reviewChunks: n });
          if (n === 0 && !this.busy) queueMicrotask(() => this.endReview("accept"));
        }
      }),
    ];
  }

  private refreshDirty(path: string, state: EditorState) {
    const saved = this.saved.get(path) ?? "";
    const dirty = state.doc.length !== saved.length || state.doc.toString() !== saved;
    const st = useStore.getState();
    const t = st.tabs.find((x) => x.path === path);
    if (t && t.dirty !== dirty) st.set({ tabs: st.tabs.map((x) => (x.path === path ? { ...x, dirty } : x)) });
  }

  has(path: string) { return this.states.has(path); }

  /** 当前文本：已打开的取编辑器里的，没打开的读磁盘。 */
  async textOf(path: string): Promise<string> {
    const active = useStore.getState().active;
    if (path === active && this.view) return this.view.state.doc.toString();
    const s = this.states.get(path);
    if (s) return s.doc.toString();
    return api.readFile(path);
  }

  private show(path: string) {
    const v = this.view;
    if (!v) return;
    const cur = useStore.getState().active;
    if (cur && cur !== path) {
      this.states.set(cur, v.state);
      this.scroll.set(cur, v.scrollDOM.scrollTop);
    }
    const next = this.states.get(path)!;
    v.setState(next);
    requestAnimationFrame(() => { v.scrollDOM.scrollTop = this.scroll.get(path) ?? 0; });
    const h = next.selection.main.head;
    const line = next.doc.lineAt(h);
    const n = this.reviewing.has(path) ? (getChunks(next)?.chunks.length ?? 0) : 0;
    useStore.getState().set({ active: path, reviewChunks: n, cursor: { line: line.number, col: h - line.from + 1 } });
    if (this.host) {
      this.host.classList.remove("swap");
      void this.host.offsetWidth;
      this.host.classList.add("swap");
    }
    v.focus();
  }

  async open(path: string, line?: number): Promise<boolean> {
    const st = useStore.getState();
    if (!this.states.has(path)) {
      let text: string;
      try { text = await api.readFile(path); } catch (e) { st.toast(`无法打开：${String(e)}`, "error"); return false; }
      this.saved.set(path, text);
      this.states.set(path, EditorState.create({ doc: text, extensions: this.extensions(path) }));
      useStore.getState().set({ tabs: [...useStore.getState().tabs, { path, dirty: false }] });
      void this.loadHead(path);
    }
    this.show(path);
    if (line && this.view) {
      const l = this.view.state.doc.line(Math.min(Math.max(1, line), this.view.state.doc.lines));
      this.view.dispatch({ selection: EditorSelection.cursor(l.from), effects: EditorView.scrollIntoView(l.from, { y: "center" }) });
    }
    return true;
  }

  private async loadHead(path: string) {
    const root = useStore.getState().root;
    if (!root) return;
    const head = await api.gitHead(root, relPath(root, path)).catch(() => null);
    const s = this.states.get(path);
    if (useStore.getState().active === path && this.view) this.view.dispatch({ effects: setHead.of(head) });
    else if (s) this.states.set(path, s.update({ effects: setHead.of(head) }).state);
  }

  async save(path?: string) {
    const st = useStore.getState();
    const p = path ?? st.active;
    if (!p) return;
    const state = p === st.active && this.view ? this.view.state : this.states.get(p);
    if (!state) return;
    const text = state.doc.toString();
    try { await api.writeFile(p, text); } catch (e) { st.toast(`保存失败：${String(e)}`, "error"); return; }
    this.saved.set(p, text);
    this.refreshDirty(p, state);
    void this.loadHead(p);
    window.dispatchEvent(new Event("quill:saved"));
  }

  async close(path: string, force = false) {
    const st = useStore.getState();
    const tab = st.tabs.find((t) => t.path === path);
    if (!tab) return;
    if (tab.dirty && !force) {
      const ok = await ask("有未保存的修改，确定放弃吗？", { title: "关闭文件", kind: "warning", okLabel: "放弃修改", cancelLabel: "取消" });
      if (!ok) return;
    }
    const idx = st.tabs.findIndex((t) => t.path === path);
    const tabs = st.tabs.filter((t) => t.path !== path);
    this.states.delete(path); this.saved.delete(path); this.scroll.delete(path); this.reviewing.delete(path);
    if (st.active === path) {
      const next = tabs[Math.min(idx, tabs.length - 1)];
      if (next) { useStore.getState().set({ tabs }); this.show(next.path); }
      else {
        this.view?.setState(EditorState.create({ doc: "" }));
        useStore.getState().set({ tabs, active: null, reviewChunks: 0 });
      }
    } else useStore.getState().set({ tabs });
  }

  /** 文件被删除 / 改名后，直接丢掉对应标签。 */
  dropPrefix(prefix: string) {
    for (const t of [...useStore.getState().tabs]) if (t.path === prefix || t.path.startsWith(prefix + "/")) void this.close(t.path, true);
  }

  // ---- AI 改动审阅 ----

  isReviewing(path: string) { return this.reviewing.has(path); }

  private startReview(): boolean {
    const v = this.view, p = useStore.getState().active;
    if (!v || !p) return false;
    if (!this.reviewing.has(p)) {
      v.dispatch({ effects: reviewComp.reconfigure(reviewExt(v.state.doc.toString())) });
      this.reviewing.add(p);
    }
    return true;
  }

  /** 把整个文件（或一段范围）替换为 AI 给出的内容，并进入逐块审阅。 */
  async applyEdit(path: string, text: string, range?: { from: number; to: number }) {
    if (!(await this.open(path)) || !this.view) return;
    this.busy = true;
    try {
      this.startReview();
      const v = this.view;
      const from = range?.from ?? 0, to = range?.to ?? v.state.doc.length;
      v.dispatch({ changes: { from, to, insert: text }, userEvent: "input.ai" });
      const first = getChunks(v.state)?.chunks[0];
      if (first) v.dispatch({ effects: EditorView.scrollIntoView(first.fromB, { y: "center" }) });
      useStore.getState().set({ reviewChunks: getChunks(v.state)?.chunks.length ?? 0 });
      if (!(getChunks(v.state)?.chunks.length)) { this.endReviewNow(); useStore.getState().toast("内容没有变化"); }
    } finally { this.busy = false; }
  }

  /** 流式写入：Inline Assist 边生成边替换选区。 */
  beginStream(from: number, to: number): StreamHandle | null {
    const v = this.view;
    if (!v || !this.startReview()) return null;
    this.busy = true;
    let end = to;
    let pending: string | null = null;
    let raf = 0;
    const flush = () => {
      raf = 0;
      if (pending === null || !this.view) return;
      const t = pending; pending = null;
      this.view.dispatch({ changes: { from, to: end, insert: t }, userEvent: "input.ai" });
      end = from + t.length;
    };
    return {
      update: (t) => { pending = t; if (!raf) raf = requestAnimationFrame(flush); },
      finish: () => {
        cancelAnimationFrame(raf); flush(); this.busy = false;
        const n = this.view ? (getChunks(this.view.state)?.chunks.length ?? 0) : 0;
        useStore.getState().set({ reviewChunks: n });
        if (n === 0) this.endReview("accept");
      },
      abort: () => { cancelAnimationFrame(raf); pending = null; this.busy = false; this.endReview("reject"); },
    };
  }

  private endReviewNow() {
    const p = useStore.getState().active;
    if (!p || !this.view) return;
    this.view.dispatch({ effects: reviewComp.reconfigure([]) });
    this.reviewing.delete(p);
    useStore.getState().set({ reviewChunks: 0 });
  }

  endReview(mode: "accept" | "reject") {
    const v = this.view, p = useStore.getState().active;
    if (!v || !p || !this.reviewing.has(p)) return;
    if (mode === "reject") {
      const orig = getOriginalDoc(v.state).toString();
      v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: orig } });
    }
    this.endReviewNow();
  }

  selection() {
    const v = this.view, path = useStore.getState().active;
    if (!v || !path) return null;
    const { from, to, head } = v.state.selection.main;
    const c = v.coordsAtPos(head);
    return { path, from, to, text: v.state.sliceDoc(from, to), coords: c ? { x: c.left, y: c.bottom } : null };
  }

  focus() { this.view?.focus(); }
}

export const ed = new Controller();
