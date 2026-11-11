import { test } from "node:test";
import assert from "node:assert/strict";
import { fuzzyScore, fuzzyFilter } from "./fuzzy.ts";

test("不匹配返回 null，空查询为 0", () => {
  assert.equal(fuzzyScore("xyz", "src/app.ts"), null);
  assert.equal(fuzzyScore("", "a"), 0);
});

test("文件名前缀优先于深层子序列", () => {
  const r = fuzzyFilter(["src/deep/snap.ts", "src/App.tsx", "README.md"], "app", (x) => x);
  assert.equal(r[0], "src/App.tsx");
});

test("连续匹配比分散匹配分数高", () => {
  const a = fuzzyScore("edit", "editor.ts")!;
  const b = fuzzyScore("edit", "e_d_i_t.ts")!;
  assert.ok(a > b);
});
