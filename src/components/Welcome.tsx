import { useStore } from "../store.ts";
import { Icon } from "./Icon.tsx";

export function Welcome({ recents, onOpen, onPick }: { recents: string[]; onOpen: () => void; onPick: (p: string) => void }) {
  const set = useStore((s) => s.set);
  return (
    <div className="welcome">
      <div className="col">
        <h1>Quill<em>.</em></h1>
        <div className="tag">用对话改代码。每一处改动，你都看得见、收得回。</div>
        <div className="btns">
          <button className="btn primary" onClick={onOpen}><Icon name="folder" />打开文件夹</button>
          <button className="btn" onClick={() => set({ settingsOpen: true })}><Icon name="settings" />设置</button>
        </div>
        {recents.length ? <><h4>最近打开</h4>{recents.slice(0, 6).map((r) => (
          <button key={r} className="recent" onClick={() => onPick(r)}>{r.split("/").pop()}<span>{r.replace(/^\/Users\/[^/]+/, "~")}</span></button>
        ))}</> : null}
        <div className="keys">
          <kbd>⌘P</kbd><span>跳转到文件</span>
          <kbd>⌘⇧P</kbd><span>命令面板</span>
          <kbd>⌘L</kbd><span>呼出对话</span>
          <kbd>⌘K</kbd><span>就地改写选中代码</span>
        </div>
      </div>
    </div>
  );
}
