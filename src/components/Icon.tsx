const P: Record<string, string> = {
  sidebar: "M3 4.5h10v7H3zM6.5 4.5v7",
  agent: "M8 2.2l1.3 3.2 3.2 1.3-3.2 1.3L8 11.2 6.7 8 3.5 6.7l3.2-1.3zM12.2 10.6l.6 1.4 1.4.6-1.4.6-.6 1.4-.6-1.4-1.4-.6 1.4-.6z",
  terminal: "M3 4.5h10v7H3zM5.5 7l1.8 1.5L5.5 10M8.8 10h1.8",
  search: "M7 11.5a4.5 4.5 0 100-9 4.5 4.5 0 000 9zM10.4 10.4L13.5 13.5",
  chevron: "M6 4l4 4-4 4",
  x: "M4.5 4.5l7 7M11.5 4.5l-7 7",
  plus: "M8 3.5v9M3.5 8h9",
  send: "M8 12.5v-9M4.5 7L8 3.5 11.5 7",
  stop: "M5 5h6v6H5z",
  branch: "M5 3.5v9M5 10a3 3 0 003-3V6.5a1.8 1.8 0 011.8-1.8M11 4.7a1.2 1.2 0 100-2.4 1.2 1.2 0 000 2.4zM5 13.2a1.2 1.2 0 100-2.4 1.2 1.2 0 000 2.4zM5 5.2a1.2 1.2 0 100-2.4 1.2 1.2 0 000 2.4z",
  check: "M3.5 8.5l3 3 6-6.5",
  copy: "M5.5 5.5h7v7h-7zM3.5 10.5v-7h7",
  folder: "M2.5 4.5h4l1.2 1.5h5.8v6h-11z",
  sun: "M8 10.5a2.5 2.5 0 100-5 2.5 2.5 0 000 5zM8 2v1.3M8 12.7V14M2 8h1.3M12.7 8H14M3.8 3.8l.9.9M11.3 11.3l.9.9M3.8 12.2l.9-.9M11.3 4.7l.9-.9",
  settings: "M8 10a2 2 0 100-4 2 2 0 000 4zM8 2.2l.7 1.5 1.6.4 1.3-1 1.2 1.2-1 1.3.4 1.6 1.5.7v1.7l-1.5.7-.4 1.6 1 1.3-1.2 1.2-1.3-1-1.6.4-.7 1.5H6.6l-.7-1.5-1.6-.4-1.3 1-1.2-1.2 1-1.3-.4-1.6-1.5-.7V6.6l1.5-.7.4-1.6-1-1.3 1.2-1.2 1.3 1 1.6-.4.7-1.5z",
  trash: "M3.5 5h9M6.5 5V3.5h3V5M5 5l.5 8h5L11 5",
  file: "M4 2.5h5l3 3v8H4z",
  git: "M8 2.5l5.5 5.5L8 13.5 2.5 8z",
  refresh: "M12.5 8a4.5 4.5 0 11-1.4-3.3M12.5 3v2.5H10",
};

export function Icon({ name, className }: { name: keyof typeof P | string; className?: string }) {
  return (
    <svg className={"ic " + (className ?? "")} viewBox="0 0 16 16" aria-hidden>
      <path d={P[name] ?? ""} />
    </svg>
  );
}

const COLORS: Record<string, string> = {
  ts: "#5aa0e6", tsx: "#5aa0e6", js: "#e6c27a", jsx: "#e6c27a", json: "#c9a45a", md: "#8aa0b8", rs: "#e58a5a",
  py: "#6aa6d6", html: "#e5784f", css: "#8d7ae6", toml: "#a0a0a0", yaml: "#c97a7a", yml: "#c97a7a", sh: "#7fd08d",
  go: "#5fc9d6", swift: "#f2784f", lock: "#777",
};

export function FileIcon({ name }: { name: string }) {
  const ext = name.includes(".") ? name.split(".").pop()!.toLowerCase() : "";
  const label = (ext || name[0] || "?").slice(0, 2).toUpperCase();
  return (
    <span className="fico" style={{ background: COLORS[ext] ?? "var(--faint)" }}>
      {label}
    </span>
  );
}
