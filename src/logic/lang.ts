const MAP: Record<string, string> = {
  ts: "typescript", tsx: "tsx", js: "javascript", jsx: "jsx", mjs: "javascript", cjs: "javascript",
  json: "json", md: "markdown", markdown: "markdown", rs: "rust", py: "python",
  html: "html", htm: "html", css: "css", scss: "css", toml: "toml", yaml: "yaml", yml: "yaml",
  sh: "shell", zsh: "shell", go: "go", swift: "swift", java: "java", c: "c", h: "c", cpp: "cpp",
};

export function langOf(path: string): string {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return MAP[ext] ?? "text";
}

export function basename(p: string): string {
  return p.slice(p.lastIndexOf("/") + 1);
}

export function relPath(root: string, abs: string): string {
  return abs.startsWith(root + "/") ? abs.slice(root.length + 1) : abs;
}
