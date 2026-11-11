import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useStore } from "../store.ts";
import { useChat, applyBlock, type Message } from "../chat.ts";
import { Icon, FileIcon } from "./Icon.tsx";
import { Prose } from "./Prose.tsx";
import { Highlighted } from "./highlight.tsx";
import { parseSegments, type CodeBlock as Block } from "../logic/blocks.ts";
import { findMentions } from "../logic/context.ts";
import { fuzzyFilter } from "../logic/fuzzy.ts";
import { basename, relPath } from "../logic/lang.ts";
import { stripContext } from "../logic/context.ts";

function CodeBlock({ block, closed, applied, onApply }: { block: Block; closed: boolean; applied: boolean; onApply: () => void }) {
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const lines = block.code.split("\n").length;
  const long = lines > 18;
  return (
    <div className={`codeblock${closed ? "" : " streaming"}${open ? " open" : ""}`}>
      <div className="codehead">
        {block.path ? <FileIcon name={basename(block.path)} /> : null}
        <span className="path">{block.path ?? (block.lang || "code")}</span>
        {block.path && block.lang ? <span className="lang">{block.lang}</span> : null}
        <span className="spacer" />
        <button className="btn sm ghost" onClick={() => { void navigator.clipboard.writeText(block.code); setCopied(true); setTimeout(() => setCopied(false), 1400); }}>
          <Icon name={copied ? "check" : "copy"} />{copied ? "已复制" : "复制"}
        </button>
        {applied ? <span className="applied"><Icon name="check" />已应用</span>
          : <button className="btn sm primary" disabled={!closed} onClick={onApply}>应用</button>}
      </div>
      <pre className="codebody"><code><Highlighted code={block.code} lang={block.lang} path={block.path} /></code></pre>
      {long ? <button className="fold-more" onClick={() => setOpen(!open)}>{open ? "收起" : `展开全部 ${lines} 行`}</button> : null}
    </div>
  );
}

function Assistant({ m }: { m: Message }) {
  const segs = useMemo(() => parseSegments(m.content), [m.content]);
  const streaming = m.status === "streaming";
  const pathBlocks = segs.flatMap((s, i) => (s.kind === "code" && s.closed && s.block.path ? [{ i, block: s.block }] : []));
  const pending = pathBlocks.filter((b) => !m.applied?.[b.i]);
  const markApplied = useChat((s) => s.markApplied);
  const apply = async (i: number, b: Block) => { if (await applyBlock(b)) markApplied(m.id, i); };
  return (
    <div className="msg assistant">
      <div className={`avatar${streaming ? " live" : ""}`}><Icon name="agent" /></div>
      <div className="prose">
        {segs.length === 0 && streaming ? <span className="thinking"><i /><i /><i /></span> : null}
        {segs.map((s, i) => s.kind === "text"
          ? <Prose key={i} text={s.text} />
          : <CodeBlock key={i} block={s.block} closed={s.closed} applied={!!m.applied?.[i]} onApply={() => void apply(i, s.block)} />)}
        {streaming && segs.length > 0 ? <span className="caret" /> : null}
        {m.status === "error" ? <div className="err">出错了：{m.error}</div> : null}
        {!streaming && pending.length > 1 ? (
          <div className="suggest-bar">
            <span className="t">{pending.length} 个文件有修改建议</span>
            <button className="btn sm primary" onClick={async () => { for (const b of pending) await apply(b.i, b.block); }}>全部应用</button>
          </div>
        ) : null}
      </div>
    </div>
  );
}

function UserMsg({ m }: { m: Message }) {
  return (
    <div className="msg user">
      {m.refs?.length ? <div className="refs">{m.refs.map((r) => <span key={r} className="chip"><FileIcon name={basename(r)} />{basename(r)}</span>)}</div> : null}
      <div className="bubble">{stripContext(m.content)}</div>
    </div>
  );
}

const SUGGESTS = ["解释一下当前文件做了什么", "找出这个文件里潜在的 bug", "为当前文件补充单元测试", "把这段代码重构得更简洁"];

export function AgentPanel() {
  const { threads, activeId, streamingId, send, stop, newThread, switchThread, removeThread } = useChat();
  const { files, active, root } = useStore();
  const thread = threads.find((t) => t.id === activeId) ?? threads[0];
  const [text, setText] = useState("");
  const [menu, setMenu] = useState(false);
  const [dropped, setDropped] = useState<string[]>([]); // 用户手动移除的自动附件
  const [extra, setExtra] = useState<string[]>([]);
  const [mention, setMention] = useState<{ q: string; idx: number } | null>(null);
  const ta = useRef<HTMLTextAreaElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  const activeRel = active && root ? relPath(root, active) : null;
  const auto = activeRel && !dropped.includes(activeRel) ? [activeRel] : [];
  const mentioned = findMentions(text, files);
  const attachRels = [...new Set([...auto, ...extra, ...mentioned])];

  useEffect(() => { setDropped([]); }, [active]);
  useEffect(() => { setExtra([]); }, [activeId]);

  useLayoutEffect(() => {
    const el = ta.current;
    if (el) { el.style.height = "0"; el.style.height = Math.min(el.scrollHeight, 220) + "px"; }
  }, [text]);

  useLayoutEffect(() => {
    const el = list.current;
    if (el && stick.current) el.scrollTo({ top: el.scrollHeight, behavior: streamingId ? "auto" : "smooth" });
  }, [thread.messages, streamingId]);

  useEffect(() => {
    const f = () => ta.current?.focus();
    window.addEventListener("quill:focus-agent", f);
    return () => window.removeEventListener("quill:focus-agent", f);
  }, []);

  const submit = () => {
    const t = text.trim();
    if (!t || streamingId || !root) return;
    const abs = attachRels.map((r) => `${root}/${r}`);
    setText(""); setMention(null); stick.current = true;
    void send(t, abs);
  };

  const onChange = (v: string) => {
    setText(v);
    const m = /(?:^|[^\w@])@([\w./\-]*)$/.exec(v.slice(0, ta.current?.selectionStart ?? v.length));
    setMention(m ? { q: m[1], idx: 0 } : null);
  };
  const cands = mention ? fuzzyFilter(files, mention.q, (x) => x, 8) : [];
  const pick = (f: string) => {
    const pos = ta.current?.selectionStart ?? text.length;
    const before = text.slice(0, pos).replace(/@[\w./\-]*$/, "");
    const after = text.slice(pos);
    setText(`${before}@${f} ${after}`);
    setExtra((e) => (e.includes(f) ? e : [...e, f]));
    setMention(null);
    ta.current?.focus();
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (mention && cands.length) {
      if (e.key === "ArrowDown") { e.preventDefault(); setMention({ ...mention, idx: (mention.idx + 1) % cands.length }); return; }
      if (e.key === "ArrowUp") { e.preventDefault(); setMention({ ...mention, idx: (mention.idx - 1 + cands.length) % cands.length }); return; }
      if (e.key === "Enter" || e.key === "Tab") { e.preventDefault(); pick(cands[mention.idx]); return; }
      if (e.key === "Escape") { setMention(null); return; }
    }
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) { e.preventDefault(); submit(); }
    if (e.key === "Escape" && streamingId) stop();
  };

  return (
    <div className="inner" style={{ position: "relative" }}>
      <div className="agent-head">
        <button className="thread-btn" onClick={() => setMenu(!menu)}>
          <span>{thread.title}</span><Icon name="chevron" className="" />
        </button>
        <span className="spacer" />
        <button className="icon-btn" title="新对话" onClick={() => { newThread(); setMenu(false); ta.current?.focus(); }}><Icon name="plus" /></button>
      </div>
      {menu ? (
        <>
          <div style={{ position: "fixed", inset: 0, zIndex: 29 }} onClick={() => setMenu(false)} />
          <div className="thread-menu">
            {threads.map((t) => (
              <button key={t.id} className={`thread-item${t.id === activeId ? " on" : ""}`} onClick={() => { switchThread(t.id); setMenu(false); }}>
                <span>{t.title}</span>
                <span className="del" role="button" onClick={(e) => { e.stopPropagation(); removeThread(t.id); }}><Icon name="trash" /></span>
              </button>
            ))}
          </div>
        </>
      ) : null}

      <div className="msgs" ref={list} onScroll={(e) => { const el = e.currentTarget; stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80; }}>
        {thread.messages.length === 0 ? (
          <div className="welcome-chat">
            <h2>想改点什么？</h2>
            <p>直接描述需求。用 <kbd>@</kbd> 引用文件，回复里的代码块可以一键应用并逐块审阅。</p>
            {SUGGESTS.map((s) => <button key={s} className="suggest" onClick={() => { setText(s); ta.current?.focus(); }}>{s}</button>)}
          </div>
        ) : thread.messages.map((m) => (m.role === "user" ? <UserMsg key={m.id} m={m} /> : <Assistant key={m.id} m={m} />))}
      </div>

      <div className="composer">
        {mention && cands.length ? (
          <div className="mention">
            {cands.map((f, i) => (
              <button key={f} className={`mi${i === mention.idx ? " on" : ""}`} onMouseEnter={() => setMention({ ...mention, idx: i })} onMouseDown={(e) => { e.preventDefault(); pick(f); }}>
                <FileIcon name={basename(f)} />{f}
              </button>
            ))}
          </div>
        ) : null}
        {attachRels.length ? (
          <div className="chips">
            {attachRels.map((r) => (
              <span key={r} className="chip fixed"><FileIcon name={basename(r)} />{basename(r)}
                <button onClick={() => { setExtra((e) => e.filter((x) => x !== r)); setDropped((d) => [...d, r]); setText((t) => t.split(`@${r}`).join("")); }}><Icon name="x" /></button>
              </span>
            ))}
          </div>
        ) : null}
        <textarea ref={ta} value={text} rows={1} placeholder={root ? "向 Quill 提问，或描述要做的修改…" : "先打开一个文件夹"} onChange={(e) => onChange(e.target.value)} onKeyDown={onKey} />
        <div className="bar">
          <span>↵ 发送 · ⇧↵ 换行</span>
          <span className="spacer" />
          {streamingId
            ? <button className="send stop" title="停止 (Esc)" onClick={stop}><Icon name="stop" /></button>
            : <button className="send" disabled={!text.trim() || !root} title="发送" onClick={submit}><Icon name="send" /></button>}
        </div>
      </div>
    </div>
  );
}
