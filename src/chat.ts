import { create } from "zustand";
import { api, listen, type Msg } from "./ipc.ts";
import { useStore } from "./store.ts";
import { ed } from "./editor/controller.ts";
import { buildUserMessage, stripContext, SYSTEM_PROMPT } from "./logic/context.ts";
import { relPath } from "./logic/lang.ts";
import type { CodeBlock } from "./logic/blocks.ts";

export interface Message {
  id: string;
  role: "user" | "assistant";
  content: string;
  refs?: string[]; // 相对路径
  status?: "streaming" | "done" | "error";
  error?: string;
  applied?: Record<number, boolean>;
}
export interface Thread { id: string; title: string; messages: Message[]; updated: number }

interface ChatState {
  threads: Thread[];
  activeId: string;
  streamingId: string | null;
  newThread: () => void;
  switchThread: (id: string) => void;
  removeThread: (id: string) => void;
  send: (text: string, attach: string[]) => Promise<void>;
  stop: () => void;
  markApplied: (msgId: string, idx: number) => void;
}

const KEY = "quill.threads";
const uid = () => Math.random().toString(36).slice(2, 10);
const fresh = (): Thread => ({ id: uid(), title: "新对话", messages: [], updated: Date.now() });

function load(): Thread[] {
  try { const t = JSON.parse(localStorage.getItem(KEY) ?? "[]") as Thread[]; if (t.length) return t; } catch { /* 无存档 */ }
  return [fresh()];
}
function persist(threads: Thread[]) {
  try {
    const clean = threads.slice(0, 40).map((t) => ({ ...t, messages: t.messages.map((m) => (m.status === "streaming" ? { ...m, status: "done" as const } : m)) }));
    localStorage.setItem(KEY, JSON.stringify(clean));
  } catch { /* 存储满了就算了 */ }
}

const initial = load();

export const useChat = create<ChatState>((set, get) => {
  const patch = (msgId: string, f: (m: Message) => Message) =>
    set((s) => {
      const threads = s.threads.map((t) => (t.messages.some((m) => m.id === msgId) ? { ...t, updated: Date.now(), messages: t.messages.map((m) => (m.id === msgId ? f(m) : m)) } : t));
      return { threads };
    });
  return {
    threads: initial,
    activeId: initial[0].id,
    streamingId: null,
    newThread: () => {
      const cur = get().threads.find((t) => t.id === get().activeId);
      if (cur && cur.messages.length === 0) return; // 已经是空对话
      const t = fresh();
      set((s) => ({ threads: [t, ...s.threads], activeId: t.id }));
    },
    switchThread: (id) => set({ activeId: id }),
    removeThread: (id) => {
      set((s) => {
        let threads = s.threads.filter((t) => t.id !== id);
        if (!threads.length) threads = [fresh()];
        return { threads, activeId: s.activeId === id ? threads[0].id : s.activeId };
      });
    },
    markApplied: (msgId, idx) => patch(msgId, (m) => ({ ...m, applied: { ...m.applied, [idx]: true } })),
    stop: () => {
      const id = get().streamingId;
      if (!id) return;
      void api.aiCancel(id);
      patch(id, (m) => ({ ...m, status: "done" }));
      set({ streamingId: null });
    },
    send: async (text, attach) => {
      if (get().streamingId) return;
      const root = useStore.getState().root;
      const rels = attach.map((p) => (root ? relPath(root, p) : p));
      const files: { path: string; content: string }[] = [];
      for (let i = 0; i < attach.length; i++) {
        try { files.push({ path: rels[i], content: await ed.textOf(attach[i]) }); } catch { /* 读不到就跳过 */ }
      }
      const userMsg: Message = { id: uid(), role: "user", content: buildUserMessage(text, files), refs: files.map((f) => f.path) };
      const aiMsg: Message = { id: uid(), role: "assistant", content: "", status: "streaming" };
      const tid = get().activeId;
      set((s) => ({
        streamingId: aiMsg.id,
        threads: s.threads.map((t) => t.id === tid
          ? { ...t, updated: Date.now(), title: t.messages.length ? t.title : text.replace(/\s+/g, " ").slice(0, 26) || "新对话", messages: [...t.messages, userMsg, aiMsg] }
          : t),
      }));
      const thread = get().threads.find((t) => t.id === tid)!;
      const history: Msg[] = thread.messages
        .filter((m) => m.id !== aiMsg.id && !(m.role === "assistant" && m.status === "error"))
        .map((m) => ({ role: m.role, content: m.role === "user" && m.id !== userMsg.id ? stripContext(m.content) : m.content }));
      await api.aiStream(aiMsg.id, SYSTEM_PROMPT, history);
    },
  };
});

// ---- 流式事件：先攒进缓冲区，每帧合并一次，避免每个字都触发整棵树重渲染 ----
const buf = new Map<string, string>();
let raf = 0;
function flush() {
  raf = 0;
  const entries = [...buf.entries()];
  buf.clear();
  useChat.setState((s) => ({
    threads: s.threads.map((t) => t.messages.some((m) => buf.has(m.id) || entries.some(([id]) => id === m.id))
      ? { ...t, messages: t.messages.map((m) => { const e = entries.find(([id]) => id === m.id); return e ? { ...m, content: m.content + e[1] } : m; }) }
      : t),
  }));
}

let started = false;
export async function initChat() {
  if (started) return;
  started = true;
  await listen<{ id: string; delta: string }>("ai-chunk", (e) => {
    buf.set(e.payload.id, (buf.get(e.payload.id) ?? "") + e.payload.delta);
    if (!raf) raf = requestAnimationFrame(flush);
  });
  const finish = (id: string, err?: string) => {
    if (raf) { cancelAnimationFrame(raf); flush(); }
    useChat.setState((s) => ({
      streamingId: s.streamingId === id ? null : s.streamingId,
      threads: s.threads.map((t) => ({ ...t, messages: t.messages.map((m) => (m.id === id ? { ...m, status: err ? "error" : "done", error: err } : m)) })),
    }));
  };
  await listen<{ id: string }>("ai-done", (e) => finish(e.payload.id));
  await listen<{ id: string; message: string }>("ai-error", (e) => finish(e.payload.id, e.payload.message));
  useChat.subscribe((s) => persist(s.threads));
}

/** 把一个代码块应用到编辑器（进入逐块审阅）。返回是否成功。 */
export async function applyBlock(block: CodeBlock): Promise<boolean> {
  const st = useStore.getState();
  const root = st.root;
  let abs: string | null = null;
  let range: { from: number; to: number } | undefined;
  if (block.path) {
    const rel = block.path.replace(/^\.?\//, "");
    abs = root ? `${root}/${rel}` : rel;
    if (root && !st.files.includes(rel)) {
      try { await api.writeFile(abs, ""); window.dispatchEvent(new Event("quill:saved")); } catch (e) { st.toast(`无法创建文件：${String(e)}`, "error"); return false; }
    }
  } else {
    abs = st.active;
    const sel = ed.selection();
    if (sel && sel.path === abs && sel.from !== sel.to) range = { from: sel.from, to: sel.to };
  }
  if (!abs) { st.toast("没有可应用的目标文件：先打开一个文件，或让模型给出 path", "error"); return false; }
  await ed.applyEdit(abs, block.code, range);
  return true;
}
