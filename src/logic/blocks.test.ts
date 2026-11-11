import { test } from "node:test";
import assert from "node:assert/strict";
import { parseSegments, parseInfo, firstCode } from "./blocks.ts";

test("info 串的几种写法", () => {
  assert.deepEqual(parseInfo("ts path=src/a.ts"), { lang: "ts", path: "src/a.ts" });
  assert.deepEqual(parseInfo("rust:src/main.rs"), { lang: "rust", path: "src/main.rs" });
  assert.deepEqual(parseInfo("path=\"a b/c.py\""), { lang: "", path: "a b/c.py" });
  assert.deepEqual(parseInfo("src/x.css"), { lang: "", path: "src/x.css" });
  assert.deepEqual(parseInfo("python"), { lang: "python" });
  assert.deepEqual(parseInfo(""), { lang: "" });
});

test("文本与代码块交错", () => {
  const s = parseSegments("先说明\n\n```ts path=a.ts\nlet x = 1\n```\n\n结束");
  assert.equal(s.length, 3);
  assert.equal(s[1].kind, "code");
  if (s[1].kind === "code") {
    assert.equal(s[1].block.path, "a.ts");
    assert.equal(s[1].block.code, "let x = 1");
    assert.equal(s[1].closed, true);
  }
});

test("流式中未闭合的围栏", () => {
  const s = parseSegments("好的：\n```js\nconsole.l");
  const last = s[s.length - 1];
  assert.equal(last.kind, "code");
  if (last.kind === "code") assert.equal(last.closed, false);
  assert.equal(firstCode("```js\nab"), "ab");
});

test("没有代码块时 firstCode 为 null", () => {
  assert.equal(firstCode("纯文字"), null);
});
