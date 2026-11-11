import { useState } from "react";
import { api, type Settings } from "../ipc.ts";
import { useStore, type ThemeMode } from "../store.ts";

const PROVIDERS: [Settings["provider"], string][] = [["mock", "演示"], ["anthropic", "Anthropic"], ["openai", "OpenAI 兼容"]];
const THEMES: [ThemeMode, string][] = [["system", "跟随系统"], ["dark", "深色"], ["light", "浅色"]];

export function SettingsModal() {
  const { settings, theme, set } = useStore();
  const [s, setS] = useState<Settings>(settings);
  const close = () => set({ settingsOpen: false });
  const save = async () => {
    await api.setSettings(s);
    set({ settings: s, settingsOpen: false });
    useStore.getState().toast("设置已保存");
  };
  return (
    <div className="scrim" onMouseDown={close} style={{ alignItems: "center", paddingTop: 0 }}>
      <div className="modal" onMouseDown={(e) => e.stopPropagation()}>
        <h3>设置</h3>
        <div className="frow"><label>主题</label>
          <div className="seg">{THEMES.map(([k, l]) => (
            <button key={k} className={theme === k ? "on" : ""} onClick={() => { set({ theme: k }); try { localStorage.setItem("quill.theme", k); } catch { /* 忽略 */ } }}>{l}</button>
          ))}</div>
        </div>
        <div className="frow"><label>模型服务</label>
          <div className="seg">{PROVIDERS.map(([k, l]) => (
            <button key={k} className={s.provider === k ? "on" : ""} onClick={() => setS({ ...s, provider: k, model: k === "openai" && s.model.startsWith("claude") ? "gpt-4o" : s.model })}>{l}</button>
          ))}</div>
        </div>
        {s.provider === "mock" ? <div className="note">演示模式不联网，只回放固定回复，用来体验交互。</div> : (
          <>
            <div className="frow"><label>API Key</label><input className="field" type="password" value={s.api_key} placeholder="sk-…" onChange={(e) => setS({ ...s, api_key: e.target.value })} /></div>
            <div className="frow"><label>模型</label><input className="field" value={s.model} onChange={(e) => setS({ ...s, model: e.target.value })} /></div>
            <div className="frow"><label>Base URL</label><input className="field" value={s.base_url} placeholder="留空使用官方地址" onChange={(e) => setS({ ...s, base_url: e.target.value })} /></div>
            <div className="note">Key 只保存在本机配置文件里，请求由 Rust 端发出，前端拿不到。</div>
          </>
        )}
        <div className="actions"><button className="btn" onClick={close}>取消</button><button className="btn primary" onClick={() => void save()}>保存</button></div>
      </div>
    </div>
  );
}
