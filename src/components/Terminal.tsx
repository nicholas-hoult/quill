import { useEffect, useRef } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { api, listen } from "../ipc.ts";
import { useStore } from "../store.ts";

function cssVar(n: string) { return getComputedStyle(document.documentElement).getPropertyValue(n).trim(); }

export function TerminalView() {
  const host = useRef<HTMLDivElement>(null);
  const root = useStore((s) => s.root)!;
  const theme = useStore((s) => s.theme);
  const termRef = useRef<XTerm | null>(null);

  useEffect(() => {
    const term = new XTerm({
      fontFamily: cssVar("--mono"), fontSize: 12.5, lineHeight: 1.35, cursorBlink: true, cursorStyle: "bar", allowTransparency: true,
      theme: { background: cssVar("--bg"), foreground: cssVar("--text"), cursor: cssVar("--accent"), selectionBackground: cssVar("--selection") },
    });
    termRef.current = term;
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host.current!);
    fit.fit();
    let id = 0, dead = false;
    const unsub: (() => void)[] = [];
    void (async () => {
      id = await api.ptySpawn(root, term.cols, term.rows);
      if (dead) { void api.ptyKill(id); return; }
      unsub.push(await listen<{ id: number; data: string }>("pty-data", (e) => { if (e.payload.id === id) term.write(e.payload.data); }));
      unsub.push(await listen<number>("pty-exit", (e) => { if (e.payload === id) term.write("\r\n\x1b[2m[进程已退出]\x1b[0m\r\n"); }));
    })();
    const d = term.onData((s) => { if (id) void api.ptyWrite(id, s); });
    const ro = new ResizeObserver(() => {
      try { fit.fit(); if (id) void api.ptyResize(id, term.cols, term.rows); } catch { /* 容器折叠时忽略 */ }
    });
    ro.observe(host.current!);
    return () => { dead = true; ro.disconnect(); d.dispose(); unsub.forEach((u) => u()); if (id) void api.ptyKill(id); term.dispose(); };
  }, [root]);

  useEffect(() => {
    const t = termRef.current;
    if (t) t.options.theme = { background: cssVar("--bg"), foreground: cssVar("--text"), cursor: cssVar("--accent"), selectionBackground: cssVar("--selection") };
  }, [theme]);

  return <div className="termbody" ref={host} />;
}
