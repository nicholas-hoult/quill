import { test } from "node:test";
import assert from "node:assert/strict";
import { findMentions, buildUserMessage, stripContext } from "./context.ts";

const files = ["src/app.ts", "src/lib/util.ts", "README.md"];

test("按全路径或文件名匹配 @", () => {
  assert.deepEqual(findMentions("看看 @src/app.ts 和 @util.ts，再 @README.md。", files), [
    "src/app.ts", "src/lib/util.ts", "README.md",
  ]);
});

test("邮箱里的 @ 不算，重复只计一次", () => {
  assert.deepEqual(findMentions("发给 a@b.com @app.ts @app.ts", files), ["src/app.ts"]);
});

test("上下文拼接后可还原展示文本", () => {
  const msg = buildUserMessage("改一下", [{ path: "a.ts", content: "x" }]);
  assert.ok(msg.includes('<file path="a.ts">'));
  assert.equal(stripContext(msg), "改一下");
  assert.equal(buildUserMessage("hi", []), "hi");
});
