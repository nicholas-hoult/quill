//! 抓取 `@网页` 引用的内容（#6）：阻塞式，放在后台线程里跑。
//! 错了表现为：发送后一直转圈（没有超时）、被一个巨大的页面撑爆内存、把图片 / 二进制当文字塞给模型、
//! 或抓取失败就吞掉整条消息（失败要如实告诉模型，消息照常发出）。

use crate::mentions;
use std::io::Read;
use std::time::Duration;

/// 抓取超时（整个请求）
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// 最多读这么多字节，超出部分丢掉
pub const MAX_BYTES: usize = 2 * 1024 * 1024;
/// 转成文字后最多保留的字符数（再长模型也看不过来，还费 token）
pub const MAX_CHARS: usize = 30_000;

/// 抓取一个网址，返回转成纯文字的内容。请求里不带任何用户数据（通用 User-Agent、没有 Cookie）。
pub fn fetch(url: &str, max_bytes: usize, timeout: Duration) -> Result<String, String> {
    mentions::url_allowed(url)?;
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(timeout)).build().into();
    let mut resp = agent.get(url).header("User-Agent", "Mozilla/5.0 (compatible; QuillFetch/1.0)").call().map_err(|e| format!("请求失败：{e}"))?;
    let status = resp.status().as_u16();
    if status >= 400 { return Err(format!("服务器返回 HTTP {status}")); }
    let ctype = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    if !mentions::is_textual(&ctype) {
        return Err(format!("不是文本内容（{}）", if ctype.is_empty() { "未知类型" } else { ctype.as_str() }));
    }
    let mut buf = Vec::new();
    resp.body_mut().as_reader().take(max_bytes as u64).read_to_end(&mut buf).map_err(|e| format!("读取失败：{e}"))?;
    let text = String::from_utf8_lossy(&buf).to_string();
    let text = if ctype.to_ascii_lowercase().contains("html") { mentions::html_to_text(&text) } else { text };
    Ok(mentions::truncate_chars(text.trim(), MAX_CHARS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    /// 起一个只回答一次的本地 HTTP 服务器，返回地址
    fn serve(response: Vec<u8>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut req = [0u8; 2048];
                let _ = s.read(&mut req);
                let _ = s.write_all(&response);
            }
        });
        format!("http://{addr}/p")
    }

    fn http(status: &str, ctype: &str, body: &[u8]) -> Vec<u8> {
        let mut v = format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn 抓取网页并转成文字() {
        let url = serve(http("200 OK", "text/html; charset=utf-8", "<html><script>x()</script><h1>标题</h1><p>正文 &amp; 更多</p></html>".as_bytes()));
        assert_eq!(fetch(&url, MAX_BYTES, TIMEOUT).unwrap(), "标题\n正文 & 更多");
    }

    #[test]
    fn 纯文本和_json_原样返回() {
        let url = serve(http("200 OK", "application/json", br#"{"a":1}"#));
        assert_eq!(fetch(&url, MAX_BYTES, TIMEOUT).unwrap(), r#"{"a":1}"#);
    }

    #[test]
    fn 二进制内容报错而不是塞给模型() {
        let url = serve(http("200 OK", "image/png", &[0x89, b'P', b'N', b'G']));
        let e = fetch(&url, MAX_BYTES, TIMEOUT).unwrap_err();
        assert!(e.contains("不是文本") && e.contains("image/png"), "{e}");
    }

    #[test]
    fn 错误状态码报错() {
        let url = serve(http("404 Not Found", "text/html", b"nope"));
        assert!(fetch(&url, MAX_BYTES, TIMEOUT).unwrap_err().contains("404"));
    }

    #[test]
    fn 超出字节上限只读前面一部分() {
        let body = "a".repeat(5000);
        let url = serve(http("200 OK", "text/plain", body.as_bytes()));
        let t = fetch(&url, 1000, TIMEOUT).unwrap();
        assert_eq!(t.len(), 1000);
    }

    #[test]
    fn 服务器不回应时按超时报错() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", l.local_addr().unwrap());
        let _hold = std::thread::spawn(move || { let _c = l.accept(); std::thread::sleep(Duration::from_secs(3)); });
        let t0 = std::time::Instant::now();
        assert!(fetch(&url, MAX_BYTES, Duration::from_millis(400)).is_err());
        assert!(t0.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn 不允许的协议在联网之前就拒绝() {
        assert!(fetch("file:///etc/passwd", MAX_BYTES, TIMEOUT).is_err());
        assert!(fetch("ftp://a.com", MAX_BYTES, TIMEOUT).is_err());
    }

    #[test]
    fn 连不上时报错() {
        // 端口 1 基本没人监听
        assert!(fetch("http://127.0.0.1:1/", MAX_BYTES, Duration::from_secs(2)).unwrap_err().contains("请求失败"));
    }
}
