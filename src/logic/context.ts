// 为什么单独测：@文件 引用决定模型能看到什么。
// 错了表现为「我明明 @ 了文件，模型却说没看到」，或把 email 里的 @ 当成文件。
export function findMentions(text: string, files: string[]): string[] {
  const out: string[] = [];
  const re = /(^|[^\w@])@([\w./\-]+)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text))) {
    const name = m[2].replace(/[.]+$/, "");
    const hit = files.find((f) => f === name) ?? files.find((f) => f.endsWith("/" + name));
    if (hit && !out.includes(hit)) out.push(hit);
  }
  return out;
}

export function buildUserMessage(text: string, files: { path: string; content: string }[]): string {
  if (!files.length) return text;
  const ctx = files.map((f) => `<file path="${f.path}">\n${f.content}\n</file>`).join("\n");
  return `${text}\n\n${ctx}`;
}

/** 历史里展示给人看的文本：去掉附带的文件正文。 */
export function stripContext(content: string): string {
  const i = content.indexOf('\n\n<file path="');
  return i >= 0 ? content.slice(0, i) : content;
}

export const SYSTEM_PROMPT = `你是 Quill，嵌在代码编辑器里的编程助手。用中文回答，简洁直接。
当你要修改文件时，输出该文件的**完整新内容**，放在一个围栏代码块里，围栏信息写成 \`\`\`语言 path=相对路径\`\`\`，例如 \`\`\`ts path=src/app.ts。
不要用省略号或"其余不变"代替未改动的部分。一次回复里每个文件最多一个代码块。`;

export const INLINE_PROMPT = `你是代码改写器。只输出改写后的代码，放进一个围栏代码块里，不要任何解释。保持原有缩进与风格。`;
