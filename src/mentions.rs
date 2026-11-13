//! 对话里的引用（#6）：`@文件` / `@文件#符号` / `@对话[标题]` / `@网页地址` 的解析、候选和内容整理（纯逻辑）。
//! 错了表现为：@ 了却没附上、把邮箱当引用、选了符号却把整份文件塞给模型、网页里的脚本样式混进正文、
//! 或标题里带空格的对话选不中。

/// 输入框 / 消息里的一个引用记号
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// `@src/a.rs` 或只写文件名 `@a.rs`（解析成项目里的文件由调用方做）
    Path(String),
    /// `@src/a.rs#parse_args`
    Symbol { path: String, name: String },
    /// `@对话[标题]`
    Thread(String),
    /// `@https://…`
    Url(String),
}

const THREAD_OPEN: &str = "对话[";

fn is_path_char(c: char) -> bool { c.is_ascii_alphanumeric() || "_./-".contains(c) }
fn is_name_char(c: char) -> bool { c.is_alphanumeric() || c == '_' }

/// 网址末尾常跟着的标点，不属于网址
const URL_TRAIL: &[char] = &['.', ',', ';', ':', '!', '?', ')', '）', '，', '。', '；', '：', '！', '？', '」', '"', '\''];

/// 扫描文本里所有的 `@` 引用。`@` 前面是字母数字 / `_` / `@` 时不算（邮箱）。
pub fn parse(text: &str) -> Vec<Token> {
    let mut out: Vec<Token> = vec![];
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (k, &(i, c)) in chars.iter().enumerate() {
        if c != '@' { continue; }
        if k > 0 {
            let p = chars[k - 1].1;
            if p.is_ascii_alphanumeric() || p == '_' || p == '@' { continue; }
        }
        let rest = &text[i + 1..];
        let tok = if rest.starts_with("http://") || rest.starts_with("https://") {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let url = rest[..end].trim_end_matches(URL_TRAIL);
            if url.len() <= "https://".len() { None } else { Some(Token::Url(url.to_string())) }
        } else if let Some(body) = rest.strip_prefix(THREAD_OPEN) {
            body.find(']').map(|j| &body[..j]).filter(|t| !t.trim().is_empty()).map(|t| Token::Thread(t.trim().to_string()))
        } else {
            let end = rest.char_indices().find(|&(_, ch)| !is_path_char(ch)).map(|(j, _)| j).unwrap_or(rest.len());
            let path = rest[..end].trim_end_matches('.');
            let after = &rest[end..];
            if path.is_empty() { None }
            else if let Some(sym) = after.strip_prefix('#') {
                let n = sym.char_indices().find(|&(_, ch)| !is_name_char(ch)).map(|(j, _)| j).unwrap_or(sym.len());
                if n == 0 { Some(Token::Path(path.to_string())) } else { Some(Token::Symbol { path: path.to_string(), name: sym[..n].to_string() }) }
            } else { Some(Token::Path(path.to_string())) }
        };
        if let Some(t) = tok { if !out.contains(&t) { out.push(t); } }
    }
    out
}

/// 把 `@a.rs` 这样的名字对到项目里的文件：全路径优先，其次按文件名（`/a.rs` 结尾）
pub fn resolve_path<'a>(name: &str, files: &'a [String]) -> Option<&'a String> {
    files.iter().find(|f| f.as_str() == name).or_else(|| files.iter().find(|f| f.ends_with(&format!("/{name}"))))
}

/// 符号索引里的一项（行号 0 起，`end_line` 含）
#[derive(Debug, Clone, PartialEq)]
pub struct SymRef { pub path: String, pub name: String, pub kind: &'static str, pub line: usize, pub end_line: usize }

/// 候选菜单里的一项
#[derive(Debug, Clone, PartialEq)]
pub enum Cand {
    File(String),
    Symbol(SymRef),
    Thread(String),
    Url(String),
}

impl Cand {
    /// 选中后写进输入框的文字（不含开头的 @）
    pub fn insert_text(&self) -> String {
        match self {
            Cand::File(p) => p.clone(),
            Cand::Symbol(s) => format!("{}#{}", s.path, s.name),
            Cand::Thread(t) => format!("{THREAD_OPEN}{t}]"),
            Cand::Url(u) => u.clone(),
        }
    }
    /// 菜单里的类型标签
    pub fn tag(&self) -> &'static str {
        match self { Cand::File(_) => "文件", Cand::Symbol(_) => "符号", Cand::Thread(_) => "对话", Cand::Url(_) => "网页" }
    }
    /// 菜单里显示的主文字
    pub fn label(&self) -> String {
        match self {
            Cand::File(p) => p.clone(),
            Cand::Symbol(s) => format!("{}  {}", s.name, s.path),
            Cand::Thread(t) => t.clone(),
            Cand::Url(u) => u.clone(),
        }
    }
}

/// `@` 后面输入的查询是否还在「引用」里：不含空白；`对话[` 开头的标题里可以有空格，直到写上 `]`
pub fn query_open(q: &str) -> bool {
    if let Some(body) = q.strip_prefix(THREAD_OPEN) { return !body.contains(']'); }
    !q.chars().any(char::is_whitespace)
}

const MAX_FILES: usize = 5;
const MAX_SYMBOLS: usize = 4;
const MAX_THREADS: usize = 3;

/// 混合候选：文件 ≤5、符号 ≤4、对话 ≤3，再加网页项。
/// `q` 以 `http` 开头 → 只给网页项；含 `#` → 只在那个文件的符号里找；以 `对话` 开头 → 只列对话。
pub fn candidates(q: &str, files: &[String], symbols: &[SymRef], threads: &[String]) -> Vec<Cand> {
    let fuzzy_files = |query: &str, n: usize| -> Vec<String> {
        crate::fuzzy::fuzzy_filter(files, query, |s| s.as_str(), n).into_iter().map(|(f, _)| f.clone()).collect()
    };
    if q.starts_with("http://") || q.starts_with("https://") { return vec![Cand::Url(q.to_string())]; }
    if let Some((pq, sq)) = q.split_once('#') {
        let Some(file) = resolve_path(pq, files).cloned().or_else(|| fuzzy_files(pq, 1).into_iter().next()) else { return vec![] };
        let in_file: Vec<&SymRef> = symbols.iter().filter(|s| s.path == file).collect();
        return crate::fuzzy::fuzzy_filter(&in_file, sq, |s| s.name.as_str(), 8).into_iter().map(|(s, _)| Cand::Symbol((*s).clone())).collect();
    }
    if q.starts_with("对话") {
        let body = q.strip_prefix(THREAD_OPEN).unwrap_or(q.trim_start_matches("对话").trim_start_matches('['));
        return crate::fuzzy::fuzzy_filter(threads, body, |s| s.as_str(), 8).into_iter().map(|(t, _)| Cand::Thread(t.clone())).collect();
    }
    let mut out: Vec<Cand> = fuzzy_files(q, MAX_FILES).into_iter().map(Cand::File).collect();
    if !q.is_empty() {
        out.extend(crate::fuzzy::fuzzy_filter(symbols, q, |s| s.name.as_str(), MAX_SYMBOLS).into_iter().map(|(s, _)| Cand::Symbol(s.clone())));
    }
    out.extend(crate::fuzzy::fuzzy_filter(threads, q, |s| s.as_str(), MAX_THREADS).into_iter().map(|(t, _)| Cand::Thread(t.clone())));
    out
}

/// 符号的源码行（含起止行，行号 0 起），最多 `max_lines` 行，超出补一行说明。返回 (代码, 实际起始行号 1 起, 结束行号 1 起)
pub fn symbol_snippet(source: &str, line: usize, end_line: usize, max_lines: usize) -> (String, usize, usize) {
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    if lines.is_empty() || line >= lines.len() { return (String::new(), line + 1, line + 1); }
    let end = end_line.min(lines.len() - 1).max(line);
    let shown_end = end.min(line + max_lines.saturating_sub(1));
    let mut code: String = lines[line..=shown_end].concat();
    if shown_end < end {
        if !code.ends_with('\n') { code.push('\n'); }
        code.push_str(&format!("…（符号共 {} 行，只保留前 {} 行）", end - line + 1, max_lines));
    }
    (code, line + 1, shown_end + 1)
}

// ---------- 对话引用 ----------

/// 把一段对话整理成给模型看的文字：只要人写的和助手的文字，不带附带的文件正文 / 工具结果；超长取开头。
/// `turns`：(是不是用户, 文字)
pub fn thread_text(turns: &[(bool, String)], max_chars: usize) -> String {
    let mut out = String::new();
    for (user, text) in turns {
        let t = text.trim();
        if t.is_empty() { continue; }
        out.push_str(if *user { "用户：" } else { "助手：" });
        out.push_str(t);
        out.push_str("\n\n");
    }
    truncate_chars(out.trim_end(), max_chars)
}

// ---------- 网页 ----------

/// 超出 `max` 个字符就截断并注明
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max { return s.to_string(); }
    let cut = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
    format!("{}\n…（内容过长，已截断）", &s[..cut])
}

const BLOCK_TAGS: &[&str] = &["p", "div", "br", "li", "ul", "ol", "h1", "h2", "h3", "h4", "h5", "h6", "tr", "section", "article", "header", "footer", "pre", "table", "blockquote", "hr", "main", "nav", "aside", "dt", "dd"];

fn decode_entity(e: &str) -> Option<String> {
    Some(match e {
        "amp" => "&".into(), "lt" => "<".into(), "gt" => ">".into(), "quot" => "\"".into(), "apos" => "'".into(), "nbsp" => " ".into(),
        "copy" => "©".into(), "hellip" => "…".into(), "mdash" => "—".into(), "ndash" => "–".into(),
        _ => {
            let n = e.strip_prefix('#')?;
            let code = if let Some(h) = n.strip_prefix(['x', 'X']) { u32::from_str_radix(h, 16).ok()? } else { n.parse().ok()? };
            char::from_u32(code)?.to_string()
        }
    })
}

/// HTML → 纯文本：整块去掉 script / style / 注释，去标签（块级标签换行），解码常见实体，折叠空白
pub fn html_to_text(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len() / 2);
    let mut i = 0;
    let b = html.as_bytes();
    let mut in_pre = false;
    while i < b.len() {
        if b[i] == b'<' {
            // 注释
            if lower[i..].starts_with("<!--") {
                i = lower[i + 4..].find("-->").map(|j| i + 4 + j + 3).unwrap_or(b.len());
                continue;
            }
            // script / style / noscript：连内容一起去掉
            let skip = ["script", "style", "noscript"].iter().find(|t| lower[i + 1..].starts_with(*t) && lower[i + 1 + t.len()..].starts_with(|c: char| c == '>' || c == ' ' || c == '/' || c == '\n' || c == '\t'));
            if let Some(tag) = skip {
                let close = format!("</{tag}");
                i = lower[i..].find(&close).and_then(|j| lower[i + j..].find('>').map(|k| i + j + k + 1)).unwrap_or(b.len());
                continue;
            }
            let Some(j) = html[i..].find('>') else { break };
            let closing = lower[i + 1..].starts_with('/');
            let inner = lower[i + 1..i + j].trim_start_matches('/').trim();
            let name: String = inner.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            if name == "pre" { in_pre = !closing; }
            if BLOCK_TAGS.contains(&name.as_str()) {
                // 块级标签只留一个换行：先去掉行尾空格，已经在行首就不重复
                while out.ends_with(' ') { out.pop(); }
                if !out.is_empty() && !out.ends_with('\n') { out.push('\n'); }
            }
            i += j + 1;
        } else if b[i] == b'&' {
            let rest = &html[i + 1..];
            match rest.find(';').filter(|&j| j <= 10).and_then(|j| decode_entity(&rest[..j]).map(|d| (d, j))) {
                Some((d, j)) => { out.push_str(&d); i += j + 2; }
                None => { out.push('&'); i += 1; }
            }
        } else {
            let ch = html[i..].chars().next().unwrap();
            // 源码里的换行 / 制表符按 HTML 语义当空白（<pre> 里保留换行）
            out.push(if !in_pre && (ch == '\n' || ch == '\r' || ch == '\t') { ' ' } else { ch });
            i += ch.len_utf8();
        }
    }
    // 折叠空白：行内连续空白 → 一个空格；行首尾去空；连续空行最多一个
    let mut lines: Vec<String> = vec![];
    for l in out.lines() {
        let t = l.split_whitespace().collect::<Vec<_>>().join(" ");
        if t.is_empty() && lines.last().map(|x: &String| x.is_empty()).unwrap_or(true) { continue; }
        lines.push(t);
    }
    lines.join("\n").trim().to_string()
}

/// 网址是否允许抓取：只允许 http / https，必须有主机名
pub fn url_allowed(url: &str) -> Result<(), String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).ok_or("只支持 http / https 地址")?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() || host.starts_with('.') { return Err("地址里没有主机名".into()); }
    Ok(())
}

/// Content-Type 是不是文本（可以转成文字给模型）：text/*、json、xml、xhtml
pub fn is_textual(content_type: &str) -> bool {
    let t = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    t.starts_with("text/") || t == "application/json" || t.ends_with("+json") || t.ends_with("+xml") || t == "application/xml" || t == "application/xhtml+xml"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Vec<String> { vec!["src/app.ts".into(), "src/lib/util.ts".into(), "README.md".into(), "src/main.rs".into()] }
    fn sym(path: &str, name: &str) -> SymRef { SymRef { path: path.into(), name: name.into(), kind: "fn", line: 0, end_line: 2 } }

    #[test]
    fn 解析四种记号() {
        let t = parse("看 @src/app.ts 和 @src/main.rs#parse_args，参考 @对话[修复 登录 bug] 以及 @https://example.com/a?b=1#c 。");
        assert_eq!(t, vec![
            Token::Path("src/app.ts".into()),
            Token::Symbol { path: "src/main.rs".into(), name: "parse_args".into() },
            Token::Thread("修复 登录 bug".into()),
            Token::Url("https://example.com/a?b=1#c".into()),
        ]);
    }

    #[test]
    fn 邮箱不算_重复只计一次() {
        assert_eq!(parse("发给 a@b.com @app.ts @app.ts"), vec![Token::Path("app.ts".into())]);
        assert_eq!(parse("x@@y"), vec![]);
    }

    #[test]
    fn 网址末尾的标点不算() {
        assert_eq!(parse("见 @https://a.com/x。"), vec![Token::Url("https://a.com/x".into())]);
        assert_eq!(parse("(@https://a.com/x), 和 @http://b.org."), vec![Token::Url("https://a.com/x".into()), Token::Url("http://b.org".into())]);
        assert_eq!(parse("@https://"), vec![]);
    }

    #[test]
    fn 对话记号要有结束括号和内容() {
        assert_eq!(parse("@对话[没写完"), vec![]);
        assert_eq!(parse("@对话[]"), vec![]);
        assert_eq!(parse("@对话[ 标题 ]"), vec![Token::Thread("标题".into())]);
    }

    #[test]
    fn 符号记号_井号后没有名字就只算文件() {
        assert_eq!(parse("@a.rs# 然后"), vec![Token::Path("a.rs".into())]);
        assert_eq!(parse("@a.rs#foo."), vec![Token::Symbol { path: "a.rs".into(), name: "foo".into() }]);
        assert_eq!(parse("@src/a.rs."), vec![Token::Path("src/a.rs".into())]);
    }

    #[test]
    fn 文件名按全路径或结尾匹配() {
        let f = files();
        assert_eq!(resolve_path("src/app.ts", &f).map(String::as_str), Some("src/app.ts"));
        assert_eq!(resolve_path("util.ts", &f).map(String::as_str), Some("src/lib/util.ts"));
        assert_eq!(resolve_path("nope.ts", &f), None);
    }

    #[test]
    fn 候选混合_文件_符号_对话() {
        let syms = vec![sym("src/main.rs", "parse_args"), sym("src/app.ts", "parseConfig")];
        let threads = vec!["解析参数的问题".to_string(), "无关".to_string()];
        let c = candidates("pars", &files(), &syms, &threads);
        assert!(c.iter().any(|x| matches!(x, Cand::Symbol(s) if s.name == "parse_args")), "{c:?}");
        assert!(c.iter().any(|x| matches!(x, Cand::Symbol(s) if s.name == "parseConfig")));
        // 文件在前
        let q_empty = candidates("", &files(), &syms, &threads);
        assert!(matches!(q_empty[0], Cand::File(_)));
        assert!(q_empty.iter().filter(|x| matches!(x, Cand::File(_))).count() <= MAX_FILES);
        // 空查询不列符号
        assert!(!q_empty.iter().any(|x| matches!(x, Cand::Symbol(_))));
    }

    #[test]
    fn 候选_网址_符号限定_对话限定() {
        assert_eq!(candidates("https://a.com/x", &files(), &[], &[]), vec![Cand::Url("https://a.com/x".into())]);
        let syms = vec![sym("src/main.rs", "parse_args"), sym("src/main.rs", "run"), sym("src/app.ts", "run")];
        // 井号：只在该文件的符号里找
        let c = candidates("main.rs#ru", &files(), &syms, &[]);
        assert_eq!(c, vec![Cand::Symbol(sym("src/main.rs", "run"))]);
        // 井号后空：该文件所有符号
        assert_eq!(candidates("main.rs#", &files(), &syms, &[]).len(), 2);
        assert!(candidates("不存在的文件#x", &files(), &syms, &[]).is_empty());
        // 对话限定
        let th = vec!["修复登录".to_string(), "重构".to_string()];
        assert_eq!(candidates("对话[登", &files(), &[], &th), vec![Cand::Thread("修复登录".into())]);
        assert_eq!(candidates("对话", &files(), &[], &th).len(), 2);
    }

    #[test]
    fn 候选的插入文字_标签() {
        assert_eq!(Cand::File("a.rs".into()).insert_text(), "a.rs");
        assert_eq!(Cand::Symbol(sym("src/a.rs", "f")).insert_text(), "src/a.rs#f");
        assert_eq!(Cand::Thread("修复 bug".into()).insert_text(), "对话[修复 bug]");
        assert_eq!(Cand::Url("https://x.y".into()).insert_text(), "https://x.y");
        assert_eq!(Cand::Thread("t".into()).tag(), "对话");
        // 插入后再解析，能还原成对应记号
        assert_eq!(parse(&format!("@{}", Cand::Thread("修复 bug".into()).insert_text())), vec![Token::Thread("修复 bug".into())]);
        assert_eq!(parse(&format!("@{}", Cand::Symbol(sym("src/a.rs", "f")).insert_text())), vec![Token::Symbol { path: "src/a.rs".into(), name: "f".into() }]);
    }

    #[test]
    fn 查询何时还算引用() {
        assert!(query_open("src/a"));
        assert!(query_open("main.rs#ru"));
        assert!(query_open("对话[修复 登"));
        assert!(!query_open("对话[修复 登录]"));
        assert!(!query_open("你好 世界"));
        assert!(query_open(""));
    }

    #[test]
    fn 符号片段只取符号的行() {
        let src = "line0\nfn a() {\n    1\n}\nline4\n";
        let (code, s, e) = symbol_snippet(src, 1, 3, 100);
        assert_eq!(code, "fn a() {\n    1\n}\n");
        assert_eq!((s, e), (2, 4));
        // 越界的结束行收回到文件末尾
        assert_eq!(symbol_snippet(src, 3, 99, 100).0, "}\nline4\n");
        // 起始行越界
        assert_eq!(symbol_snippet(src, 99, 100, 100).0, "");
        assert_eq!(symbol_snippet("", 0, 0, 10).0, "");
    }

    #[test]
    fn 符号片段超长截断并注明() {
        let src: String = (0..50).map(|i| format!("l{i}\n")).collect();
        let (code, s, e) = symbol_snippet(&src, 0, 49, 5);
        assert!(code.starts_with("l0\nl1\nl2\nl3\nl4\n"));
        assert!(code.contains("共 50 行") && code.contains("前 5 行"), "{code}");
        assert_eq!((s, e), (1, 5));
    }

    #[test]
    fn 对话文字_只要人写的和助手的() {
        let t = thread_text(&[(true, "你好".into()), (false, "  你好，有什么可以帮你  ".into()), (true, "  ".into())], 1000);
        assert_eq!(t, "用户：你好\n\n助手：你好，有什么可以帮你");
        assert_eq!(thread_text(&[], 10), "");
        assert!(thread_text(&[(true, "x".repeat(100))], 20).contains("已截断"));
    }

    #[test]
    fn 截断按字符() {
        assert_eq!(truncate_chars("你好世界", 4), "你好世界");
        let t = truncate_chars("你好世界", 2);
        assert!(t.starts_with("你好\n") && t.contains("已截断"));
    }

    #[test]
    fn html_去脚本样式注释和标签() {
        let h = "<html><head><title>标题</title><style>p{color:red}</style><script>var a = '<p>x</p>';</script></head><body><!-- 注释 --><h1>大标题</h1><p>第一段 <b>粗体</b> 文字</p><p>第二段</p></body></html>";
        let t = html_to_text(h);
        assert!(!t.contains("color:red") && !t.contains("var a") && !t.contains("注释") && !t.contains('<'), "{t}");
        assert_eq!(t, "标题\n大标题\n第一段 粗体 文字\n第二段");
    }

    #[test]
    fn html_实体与空白() {
        assert_eq!(html_to_text("a&nbsp;&nbsp;b &amp; c &lt;d&gt; &quot;e&quot; &#20013;&#x6587; &unknown; &"), "a b & c <d> \"e\" 中文 &unknown; &");
        assert_eq!(html_to_text("<p>a</p>\n\n\n<p>   b   \t c </p><br><br><br><p>d</p>"), "a\nb c\nd");
        assert_eq!(html_to_text(""), "");
        assert_eq!(html_to_text("纯文本没有标签"), "纯文本没有标签");
    }

    #[test]
    fn html_大小写标签与坏标签不崩() {
        assert_eq!(html_to_text("<SCRIPT>bad()</SCRIPT><P>ok</P>"), "ok");
        assert_eq!(html_to_text("<p>a</p><div"), "a"); // 没有结束的 <
        assert_eq!(html_to_text("<script>永远不结束"), "");
        assert_eq!(html_to_text("<!-- 没结束"), "");
        assert_eq!(html_to_text("<pre>  代码\n  缩进</pre>"), "代码\n缩进");
    }

    #[test]
    fn 网址限制() {
        assert!(url_allowed("https://a.com/x").is_ok());
        assert!(url_allowed("http://localhost:3000/").is_ok());
        assert!(url_allowed("file:///etc/passwd").is_err());
        assert!(url_allowed("ftp://a.com").is_err());
        assert!(url_allowed("javascript:alert(1)").is_err());
        assert!(url_allowed("https://").is_err());
        assert!(url_allowed("https:///path").is_err());
    }

    #[test]
    fn 内容类型判断() {
        assert!(is_textual("text/html; charset=utf-8"));
        assert!(is_textual("TEXT/plain"));
        assert!(is_textual("application/json"));
        assert!(is_textual("application/ld+json"));
        assert!(is_textual("application/atom+xml"));
        assert!(!is_textual("image/png"));
        assert!(!is_textual("application/pdf"));
        assert!(!is_textual("application/octet-stream"));
        assert!(!is_textual(""));
    }
}
