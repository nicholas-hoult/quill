import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface Entry { name: string; path: string; is_dir: boolean }
export interface Change { path: string; status: "M" | "A" | "D" | "?" }
export interface Hit { path: string; line: number; text: string }
export interface Settings { provider: "mock" | "anthropic" | "openai"; api_key: string; base_url: string; model: string }
export interface Msg { role: "user" | "assistant"; content: string }

export const api = {
  readDir: (path: string) => invoke<Entry[]>("read_dir", { path }),
  readFile: (path: string) => invoke<string>("read_file", { path }),
  writeFile: (path: string, content: string) => invoke<void>("write_file", { path, content }),
  createEntry: (path: string, isDir: boolean) => invoke<void>("create_entry", { path, isDir }),
  renameEntry: (from: string, to: string) => invoke<void>("rename_entry", { from, to }),
  deleteEntry: (path: string) => invoke<void>("delete_entry", { path }),
  listFiles: (root: string) => invoke<string[]>("list_files", { root }),
  searchText: (root: string, query: string, caseSensitive: boolean) =>
    invoke<Hit[]>("search_text", { root, query, caseSensitive }),
  gitStatus: (root: string) => invoke<Change[]>("git_status", { root }),
  gitBranch: (root: string) => invoke<string | null>("git_branch", { root }),
  gitHead: (root: string, rel: string) => invoke<string | null>("git_head_content", { root, rel }),
  ptySpawn: (cwd: string, cols: number, rows: number) => invoke<number>("pty_spawn", { cwd, cols, rows }),
  ptyWrite: (id: number, data: string) => invoke<void>("pty_write", { id, data }),
  ptyResize: (id: number, cols: number, rows: number) => invoke<void>("pty_resize", { id, cols, rows }),
  ptyKill: (id: number) => invoke<void>("pty_kill", { id }),
  getSettings: () => invoke<Settings>("get_settings"),
  setSettings: (settings: Settings) => invoke<void>("set_settings", { settings }),
  aiStream: (id: string, system: string, messages: Msg[]) => invoke<void>("ai_stream", { id, system, messages }),
  aiCancel: (id: string) => invoke<void>("ai_cancel", { id }),
};

export { listen };
