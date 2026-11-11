import { gutter, GutterMarker, EditorView } from "@codemirror/view";
import { StateEffect, StateField, RangeSetBuilder, type Extension, type Text } from "@codemirror/state";
import { diff } from "@codemirror/merge";

// 相对 HEAD 的改动条：新增绿、修改黄、删除红。纯展示，不参与审阅流程。
export const setHead = StateEffect.define<string | null>();

class Bar extends GutterMarker {
  constructor(readonly kind: "add" | "mod" | "del") { super(); }
  eq(o: Bar) { return o.kind === this.kind; }
  toDOM() { const d = document.createElement("div"); d.className = `cm-gitbar ${this.kind}`; return d; }
}
const bars = { add: new Bar("add"), mod: new Bar("mod"), del: new Bar("del") };

interface GState { head: string | null; marks: ReturnType<typeof build> }

function build(head: string | null, doc: Text) {
  const b = new RangeSetBuilder<GutterMarker>();
  if (head === null) return b.finish();
  const changes = diff(head, doc.toString());
  const lineStarts = new Set<number>();
  const put = (line: number, m: Bar) => {
    if (line < 1 || line > doc.lines || lineStarts.has(line)) return;
    lineStarts.add(line);
    b.add(doc.line(line).from, doc.line(line).from, m);
  };
  const rows: [number, Bar][] = [];
  for (const c of changes) {
    const fromLine = doc.lineAt(Math.min(c.fromB, doc.length)).number;
    if (c.fromB === c.toB) rows.push([Math.max(1, fromLine), bars.del]);
    else {
      const toLine = doc.lineAt(Math.max(c.fromB, c.toB - 1)).number;
      const kind = c.fromA === c.toA ? bars.add : bars.mod;
      for (let l = fromLine; l <= toLine; l++) rows.push([l, kind]);
    }
  }
  rows.sort((a, z) => a[0] - z[0]);
  for (const [l, m] of rows) put(l, m);
  return b.finish();
}

const field = StateField.define<GState>({
  create: () => ({ head: null, marks: build(null, { lines: 0 } as Text) }),
  update(v, tr) {
    let head = v.head;
    let changed = false;
    for (const e of tr.effects) if (e.is(setHead)) { head = e.value; changed = true; }
    if (!changed && !tr.docChanged) return v;
    return { head, marks: build(head, tr.state.doc) };
  },
});

export function gitGutter(): Extension {
  return [
    field,
    gutter({ class: "cm-gitgutter", markers: (v: EditorView) => v.state.field(field).marks }),
  ];
}
