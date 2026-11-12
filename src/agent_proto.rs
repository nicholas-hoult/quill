//! Agent 工具调用的协议层（纯逻辑）：Anthropic 流式事件解析、历史消息 → API 消息、演示用的脚本化回合。
//! 单独测：解析错了，表现为工具调用的参数被截断（模型读错文件），或历史里的 tool_result 对不上 tool_use 导致 API 报 400。
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub enum ProtoEvent {
    Text(String),
    ToolUse { id: String, name: String, input: Value },
    /// end_turn / tool_use / max_tokens …
    Stop(String),
}

/// 逐行喂入 SSE，吐出事件。工具参数是分片的 JSON，要拼完整才能解析。
#[derive(Default)]
pub struct SseParser {
    tool: Option<(String, String, String)>,
}

impl SseParser {
    pub fn feed(&mut self, line: &str) -> Vec<ProtoEvent> {
        let Some(data) = line.strip_prefix("data:") else { return vec![] };
        let data = data.trim();
        if data.is_empty() { return vec![]; }
        let Ok(v) = serde_json::from_str::<Value>(data) else { return vec![] };
        match v["type"].as_str() {
            Some("content_block_start") if v["content_block"]["type"] == "tool_use" => {
                let cb = &v["content_block"];
                self.tool = Some((cb["id"].as_str().unwrap_or("").into(), cb["name"].as_str().unwrap_or("").into(), String::new()));
                vec![]
            }
            Some("content_block_delta") => match v["delta"]["type"].as_str() {
                Some("text_delta") => v["delta"]["text"].as_str().map(|t| vec![ProtoEvent::Text(t.into())]).unwrap_or_default(),
                Some("input_json_delta") => {
                    if let (Some(t), Some(p)) = (self.tool.as_mut(), v["delta"]["partial_json"].as_str()) { t.2.push_str(p); }
                    vec![]
                }
                _ => vec![],
            },
            Some("content_block_stop") => match self.tool.take() {
                Some((id, name, json)) => {
                    let input = if json.trim().is_empty() { json!({}) } else { serde_json::from_str(&json).unwrap_or(json!({})) };
                    vec![ProtoEvent::ToolUse { id, name, input }]
                }
                None => vec![],
            },
            Some("message_delta") => v["delta"]["stop_reason"].as_str().map(|r| vec![ProtoEvent::Stop(r.into())]).unwrap_or_default(),
            _ => vec![],
        }
    }
}

#[derive(Debug, Clone)]
pub struct HistCall {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub result: String,
}

#[derive(Debug, Clone)]
pub struct HistMsg {
    pub role: &'static str,
    pub text: String,
    pub calls: Vec<HistCall>,
}

/// 把界面里的对话历史转成 API 消息：助手消息带工具调用时，紧跟一条携带 tool_result 的用户消息。
pub fn api_messages(hist: &[HistMsg]) -> Vec<Value> {
    let mut out = vec![];
    for m in hist {
        if m.role == "user" {
            out.push(json!({ "role": "user", "content": m.text }));
            continue;
        }
        let mut blocks = vec![];
        if !m.text.trim().is_empty() { blocks.push(json!({ "type": "text", "text": m.text })); }
        for c in &m.calls { blocks.push(json!({ "type": "tool_use", "id": c.id, "name": c.name, "input": c.input })); }
        if blocks.is_empty() { continue; }
        out.push(json!({ "role": "assistant", "content": blocks }));
        if !m.calls.is_empty() {
            let results: Vec<Value> = m.calls.iter().map(|c| json!({ "type": "tool_result", "tool_use_id": c.id, "content": c.result })).collect();
            out.push(json!({ "role": "user", "content": results }));
        }
    }
    out
}

fn last_user_text(messages: &[Value]) -> String {
    messages.iter().filter(|m| m["role"] == "user").filter_map(|m| m["content"].as_str()).last().unwrap_or("").to_string()
}

/// 演示脚本：用户消息含 `[tool-demo]` 时，依次 列目录 → 读文件 → 提交编辑 → 总结。
/// 返回 None 表示不是演示请求，走普通 mock 回复。
pub fn mock_turn(messages: &[Value]) -> Option<Vec<ProtoEvent>> {
    // 找到触发演示的那条用户文本（工具回合里最后一条字符串型用户消息）
    if !last_user_text(messages).contains("[tool-demo]") { return None; }
    let results: Vec<&str> = messages.iter().filter(|m| m["role"] == "user").filter_map(|m| m["content"].as_array()).filter_map(|a| a.first()).filter_map(|b| b["content"].as_str()).collect();
    let call = |id: &str, name: &str, input: Value| ProtoEvent::ToolUse { id: id.into(), name: name.into(), input };
    Some(match results.len() {
        0 => vec![ProtoEvent::Text("我先看看项目里有什么。".into()), call("toolu_demo1", "list_dir", json!({ "path": "." })), ProtoEvent::Stop("tool_use".into())],
        1 => {
            let file = results[0].lines().find_map(|l| l.strip_prefix("file ")).unwrap_or("README.md").trim().to_string();
            vec![ProtoEvent::Text(format!("再读一下 {file}。")), call("toolu_demo2", "read_file", json!({ "path": file })), ProtoEvent::Stop("tool_use".into())]
        }
        2 => {
            // 上一步 read_file 的结果就是文件内容；找回路径用上一条助手消息里的 tool_use
            let path = messages.iter().rev().filter(|m| m["role"] == "assistant").filter_map(|m| m["content"].as_array()).flatten().find_map(|b| (b["name"] == "read_file").then(|| b["input"]["path"].as_str().unwrap_or("README.md").to_string())).unwrap_or_else(|| "README.md".into());
            let content = format!("// edited by Quill agent\n{}", results[1]);
            vec![ProtoEvent::Text("给它加一行注释，改动会先进入审阅。".into()), call("toolu_demo3", "edit_file", json!({ "path": path, "content": content })), ProtoEvent::Stop("tool_use".into())]
        }
        _ => vec![ProtoEvent::Text("改动已经放进编辑器等你审阅，接受或拒绝都可以。".into()), ProtoEvent::Stop("end_turn".into())],
    })
}

/// 终端 / 诊断演示：用户消息含 `[cmd-demo]`（命令成功）或 `[cmd-fail]`（命令失败）时，依次 运行命令 → 查看诊断 → 总结。
pub fn mock_cmd_turn(messages: &[Value]) -> Option<Vec<ProtoEvent>> {
    let text = last_user_text(messages);
    let fail = text.contains("[cmd-fail]");
    if !fail && !text.contains("[cmd-demo]") { return None; }
    let results = messages.iter().filter(|m| m["role"] == "user").filter_map(|m| m["content"].as_array()).filter_map(|a| a.first()).filter(|b| b["type"] == "tool_result").count();
    let call = |id: &str, name: &str, input: Value| ProtoEvent::ToolUse { id: id.into(), name: name.into(), input };
    Some(match results {
        0 => {
            let command = if fail { "echo 'error[E0425]: 找不到值 `x`' 1>&2; exit 2" } else { "echo '检查通过'; echo 'warning: 示例警告' 1>&2; ls | head -5" };
            vec![ProtoEvent::Text("我先在项目里跑一下检查。".into()), call("toolu_cmd1", "run_command", json!({ "command": command })), ProtoEvent::Stop("tool_use".into())]
        }
        1 => vec![ProtoEvent::Text("再看看语言服务器有没有报问题。".into()), call("toolu_cmd2", "diagnostics", json!({})), ProtoEvent::Stop("tool_use".into())],
        _ => vec![ProtoEvent::Text(if fail { "命令失败了，退出码 2。这是演示数据，真实模型会据此去改代码。".into() } else { "命令跑完了，诊断也看过了。".into() }), ProtoEvent::Stop("end_turn".into())],
    })
}

/// 新建文件演示：用户消息含 `[new-demo]` 时，提交一次对新文件 `notes/by_agent.txt` 的 edit_file，然后总结。
pub fn mock_new_file_turn(messages: &[Value]) -> Option<Vec<ProtoEvent>> {
    if !last_user_text(messages).contains("[new-demo]") { return None; }
    let done = messages.iter().filter(|m| m["role"] == "user").filter_map(|m| m["content"].as_array()).filter_map(|a| a.first()).any(|b| b["type"] == "tool_result");
    Some(if done {
        vec![ProtoEvent::Text("新文件已经放进编辑器等你审阅。".into()), ProtoEvent::Stop("end_turn".into())]
    } else {
        vec![
            ProtoEvent::Text("我新建一个笔记文件。".into()),
            ProtoEvent::ToolUse { id: "toolu_new1".into(), name: "edit_file".into(), input: json!({ "path": "notes/by_agent.txt", "content": "由 Agent 新建\n" }) },
            ProtoEvent::Stop("tool_use".into()),
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(lines: &[&str]) -> Vec<ProtoEvent> {
        let mut p = SseParser::default();
        lines.iter().flat_map(|l| p.feed(l)).collect()
    }

    #[test]
    fn 文本与停止原因() {
        let ev = feed_all(&[
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你好"}}"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
            "event: ping",
        ]);
        assert_eq!(ev, vec![ProtoEvent::Text("你好".into()), ProtoEvent::Stop("end_turn".into())]);
    }

    #[test]
    fn 分片的工具参数拼完整后才发出() {
        let ev = feed_all(&[
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tu_1","name":"read_file","input":{}}}"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"pa"}}"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"th\": \"src/a.rs\"}"}}"#,
            r#"data: {"type":"content_block_stop","index":1}"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}"#,
        ]);
        assert_eq!(ev[0], ProtoEvent::ToolUse { id: "tu_1".into(), name: "read_file".into(), input: json!({ "path": "src/a.rs" }) });
        assert_eq!(ev[1], ProtoEvent::Stop("tool_use".into()));
    }

    #[test]
    fn 无参数的工具调用得到空对象() {
        let ev = feed_all(&[r#"data: {"type":"content_block_start","content_block":{"type":"tool_use","id":"t","name":"list_dir"}}"#, r#"data: {"type":"content_block_stop"}"#]);
        assert_eq!(ev, vec![ProtoEvent::ToolUse { id: "t".into(), name: "list_dir".into(), input: json!({}) }]);
    }

    #[test]
    fn 历史转_api_消息_tool_result_紧跟其后() {
        let h = vec![
            HistMsg { role: "user", text: "改一下".into(), calls: vec![] },
            HistMsg { role: "assistant", text: "先读文件".into(), calls: vec![HistCall { id: "a".into(), name: "read_file".into(), input: json!({"path":"x"}), result: "内容".into() }] },
            HistMsg { role: "assistant", text: "好了".into(), calls: vec![] },
            HistMsg { role: "assistant", text: "  ".into(), calls: vec![] },
        ];
        let m = api_messages(&h);
        assert_eq!(m.len(), 4, "空助手消息应被丢弃");
        assert_eq!(m[1]["content"][1]["type"], "tool_use");
        assert_eq!(m[2]["role"], "user");
        assert_eq!(m[2]["content"][0]["tool_use_id"], "a");
        assert_eq!(m[3]["content"][0]["text"], "好了");
    }

    #[test]
    fn 演示脚本四个回合() {
        let ask = json!({"role":"user","content":"帮我改 [tool-demo]"});
        let t0 = mock_turn(&[ask.clone()]).unwrap();
        assert!(matches!(&t0[1], ProtoEvent::ToolUse { name, .. } if name == "list_dir"));
        let a1 = json!({"role":"assistant","content":[{"type":"tool_use","id":"toolu_demo1","name":"list_dir","input":{}}]});
        let r1 = json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_demo1","content":"dir  src/\nfile app.ts"}]});
        let t1 = mock_turn(&[ask.clone(), a1.clone(), r1.clone()]).unwrap();
        assert!(matches!(&t1[1], ProtoEvent::ToolUse { name, input, .. } if name == "read_file" && input["path"] == "app.ts"));
        let a2 = json!({"role":"assistant","content":[{"type":"tool_use","id":"toolu_demo2","name":"read_file","input":{"path":"app.ts"}}]});
        let r2 = json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_demo2","content":"let a = 1;"}]});
        let t2 = mock_turn(&[ask.clone(), a1.clone(), r1.clone(), a2.clone(), r2.clone()]).unwrap();
        assert!(matches!(&t2[1], ProtoEvent::ToolUse { name, input, .. } if name == "edit_file" && input["path"] == "app.ts" && input["content"].as_str().unwrap().ends_with("let a = 1;")));
        let a3 = json!({"role":"assistant","content":[{"type":"tool_use","id":"toolu_demo3","name":"edit_file","input":{}}]});
        let r3 = json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_demo3","content":"ok"}]});
        let t3 = mock_turn(&[ask, a1, r1, a2, r2, a3, r3]).unwrap();
        assert_eq!(t3.last(), Some(&ProtoEvent::Stop("end_turn".into())));
        assert!(mock_turn(&[json!({"role":"user","content":"你好"})]).is_none());
    }

    #[test]
    fn 命令演示按结果数推进() {
        let u = |t: &str| json!({"role":"user","content":t});
        let tr = |id: &str, c: &str| json!({"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":c}]});
        assert!(mock_cmd_turn(&[u("你好")]).is_none());
        let t0 = mock_cmd_turn(&[u("跑一下 [cmd-demo]")]).unwrap();
        assert!(matches!(&t0[1], ProtoEvent::ToolUse { name, input, .. } if name == "run_command" && input["command"].as_str().unwrap().contains("检查通过")));
        let a = json!({"role":"assistant","content":[]});
        let t1 = mock_cmd_turn(&[u("[cmd-demo]"), a.clone(), tr("toolu_cmd1", "ok")]).unwrap();
        assert!(matches!(&t1[1], ProtoEvent::ToolUse { name, .. } if name == "diagnostics"));
        let t2 = mock_cmd_turn(&[u("[cmd-demo]"), a.clone(), tr("toolu_cmd1", "ok"), a, tr("toolu_cmd2", "无")]).unwrap();
        assert!(matches!(t2.last(), Some(ProtoEvent::Stop(s)) if s == "end_turn"));
        let f = mock_cmd_turn(&[u("[cmd-fail]")]).unwrap();
        assert!(matches!(&f[1], ProtoEvent::ToolUse { input, .. } if input["command"].as_str().unwrap().contains("exit 2")));
    }

    #[test]
    fn 新建文件演示() {
        let u = |t: &str| json!({"role":"user","content":t});
        assert!(mock_new_file_turn(&[u("你好")]).is_none());
        let t0 = mock_new_file_turn(&[u("[new-demo]")]).unwrap();
        assert!(matches!(&t0[1], ProtoEvent::ToolUse { name, input, .. } if name == "edit_file" && input["path"] == "notes/by_agent.txt"));
        let r = json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_new1","content":"ok"}]});
        let t1 = mock_new_file_turn(&[u("[new-demo]"), json!({"role":"assistant","content":[]}), r]).unwrap();
        assert!(matches!(t1.last(), Some(ProtoEvent::Stop(s)) if s == "end_turn"));
    }
}
