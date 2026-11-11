import { create } from "zustand";
import type { Change, Settings } from "./ipc.ts";

export interface Tab { path: string; dirty: boolean }
export type SideView = "files" | "search" | "git";
export type ThemeMode = "system" | "dark" | "light";

export interface Toast { id: number; text: string; kind: "info" | "error" }

interface State {
  root: string | null;
  files: string[];
  changes: Change[];
  branch: string | null;
  tabs: Tab[];
  active: string | null;
  reviewChunks: number; // 当前标签页正在审阅的改动块数，0 = 不在审阅
  cursor: { line: number; col: number };
  sidebar: boolean;
  agent: boolean;
  terminal: boolean;
  sideView: SideView;
  sideW: number;
  agentW: number;
  termH: number;
  palette: null | "files" | "commands";
  settingsOpen: boolean;
  settings: Settings;
  theme: ThemeMode;
  toasts: Toast[];
  set: (p: Partial<State>) => void;
  toast: (text: string, kind?: Toast["kind"]) => void;
}

const num = (k: string, d: number) => {
  try { return Number(localStorage.getItem(k)) || d; } catch { return d; }
};

let toastSeq = 0;

export const useStore = create<State>((set, get) => ({
  root: null,
  files: [],
  changes: [],
  branch: null,
  tabs: [],
  active: null,
  reviewChunks: 0,
  cursor: { line: 1, col: 1 },
  sidebar: true,
  agent: true,
  terminal: false,
  sideView: "files",
  sideW: num("quill.sideW", 260),
  agentW: num("quill.agentW", 430),
  termH: num("quill.termH", 260),
  palette: null,
  settingsOpen: false,
  settings: { provider: "mock", api_key: "", base_url: "", model: "claude-sonnet-5-5" },
  theme: ((): ThemeMode => {
    try { return (localStorage.getItem("quill.theme") as ThemeMode) || "system"; } catch { return "system"; }
  })(),
  toasts: [],
  set: (p) => set(p),
  toast: (text, kind = "info") => {
    const id = ++toastSeq;
    set({ toasts: [...get().toasts, { id, text, kind }] });
    setTimeout(() => set({ toasts: get().toasts.filter((t) => t.id !== id) }), 3600);
  },
}));
