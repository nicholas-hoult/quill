//! 语法高亮：tree-sitter-highlight → 不重叠的 (字节范围, 类别) 列表。
//! 单独测：类别映射错了，表现为关键字和字符串颜色互换，或整行没有高亮。
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang { Rust, JavaScript, TypeScript, Tsx, Json, Python, Css, Bash, Go }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Keyword, String, Number, Function, Type, Property, Comment, Operator, Punct, Constant, Variable, Tag, Attribute, Escape }

const NAMES: &[&str] = &[
    "keyword", "string", "string.special", "number", "function", "function.builtin", "function.method", "function.macro", "type", "type.builtin",
    "constructor", "property", "comment", "operator", "punctuation", "punctuation.bracket", "punctuation.delimiter", "constant", "constant.builtin",
    "boolean", "variable", "variable.builtin", "variable.parameter", "tag", "attribute", "escape", "label", "module", "embedded", "string.escape",
];

fn kind_of(name: &str) -> Kind {
    match name {
        "keyword" => Kind::Keyword,
        "string" | "string.special" => Kind::String,
        "escape" | "string.escape" => Kind::Escape,
        "number" => Kind::Number,
        "function" | "function.builtin" | "function.method" | "function.macro" => Kind::Function,
        "type" | "type.builtin" | "constructor" | "module" | "label" => Kind::Type,
        "property" => Kind::Property,
        "comment" => Kind::Comment,
        "operator" => Kind::Operator,
        "punctuation" | "punctuation.bracket" | "punctuation.delimiter" => Kind::Punct,
        "constant" | "constant.builtin" | "boolean" => Kind::Constant,
        "tag" => Kind::Tag,
        "attribute" => Kind::Attribute,
        _ => Kind::Variable,
    }
}

pub fn lang_for_path(path: &str) -> Option<Lang> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => Lang::Rust,
        "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
        "ts" => Lang::TypeScript,
        "tsx" => Lang::Tsx,
        "json" => Lang::Json,
        "py" => Lang::Python,
        "css" => Lang::Css,
        "sh" | "bash" | "zsh" => Lang::Bash,
        "go" => Lang::Go,
        _ => return None,
    })
}

/// 围栏里的语言名（rs、ts、py…）
pub fn lang_for_name(name: &str) -> Option<Lang> {
    Some(match name.to_ascii_lowercase().as_str() {
        "rust" | "rs" => Lang::Rust,
        "javascript" | "js" | "jsx" => Lang::JavaScript,
        "typescript" | "ts" => Lang::TypeScript,
        "tsx" => Lang::Tsx,
        "json" => Lang::Json,
        "python" | "py" => Lang::Python,
        "css" => Lang::Css,
        "bash" | "sh" | "shell" | "zsh" => Lang::Bash,
        "go" => Lang::Go,
        _ => return None,
    })
}

pub fn lang_label(l: Option<Lang>) -> &'static str {
    match l {
        Some(Lang::Rust) => "Rust", Some(Lang::JavaScript) => "JavaScript", Some(Lang::TypeScript) => "TypeScript", Some(Lang::Tsx) => "TSX",
        Some(Lang::Json) => "JSON", Some(Lang::Python) => "Python", Some(Lang::Css) => "CSS", Some(Lang::Bash) => "Shell", Some(Lang::Go) => "Go",
        None => "纯文本",
    }
}

fn build(lang: Lang) -> Option<HighlightConfiguration> {
    let (language, name, query): (tree_sitter::Language, &str, String) = match lang {
        Lang::Rust => (tree_sitter_rust::LANGUAGE.into(), "rust", tree_sitter_rust::HIGHLIGHTS_QUERY.to_string()),
        Lang::JavaScript => (tree_sitter_javascript::LANGUAGE.into(), "javascript", tree_sitter_javascript::HIGHLIGHT_QUERY.to_string()),
        Lang::TypeScript => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), "typescript",
            format!("{}\n{}", tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_typescript::HIGHLIGHTS_QUERY),
        ),
        Lang::Tsx => (
            tree_sitter_typescript::LANGUAGE_TSX.into(), "tsx",
            format!("{}\n{}", tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_typescript::HIGHLIGHTS_QUERY),
        ),
        Lang::Json => (tree_sitter_json::LANGUAGE.into(), "json", tree_sitter_json::HIGHLIGHTS_QUERY.to_string()),
        Lang::Python => (tree_sitter_python::LANGUAGE.into(), "python", tree_sitter_python::HIGHLIGHTS_QUERY.to_string()),
        Lang::Css => (tree_sitter_css::LANGUAGE.into(), "css", tree_sitter_css::HIGHLIGHTS_QUERY.to_string()),
        Lang::Bash => (tree_sitter_bash::LANGUAGE.into(), "bash", tree_sitter_bash::HIGHLIGHT_QUERY.to_string()),
        Lang::Go => (tree_sitter_go::LANGUAGE.into(), "go", tree_sitter_go::HIGHLIGHTS_QUERY.to_string()),
    };
    let mut cfg = HighlightConfiguration::new(language, name, &query, "", "").ok()?;
    cfg.configure(NAMES);
    Some(cfg)
}

thread_local! {
    static CONFIGS: RefCell<HashMap<Lang, Option<HighlightConfiguration>>> = RefCell::new(HashMap::new());
}

/// 对整段文本做高亮，返回按起点排序、互不重叠的区间（内层类别优先）。
pub fn highlight(lang: Lang, text: &str) -> Vec<(Range<usize>, Kind)> {
    CONFIGS.with(|c| {
        let mut map = c.borrow_mut();
        let Some(cfg) = map.entry(lang).or_insert_with(|| build(lang)).as_ref() else { return vec![] };
        let mut hl = Highlighter::new();
        let Ok(events) = hl.highlight(cfg, text.as_bytes(), None, None, |_| None) else { return vec![] };
        let mut out: Vec<(Range<usize>, Kind)> = vec![];
        let mut stack: Vec<Kind> = vec![];
        for ev in events {
            match ev {
                Ok(HighlightEvent::HighlightStart(h)) => stack.push(kind_of(NAMES.get(h.0).copied().unwrap_or(""))),
                Ok(HighlightEvent::HighlightEnd) => { stack.pop(); }
                Ok(HighlightEvent::Source { start, end }) => {
                    if let Some(&k) = stack.last() {
                        if start < end { out.push((start..end, k)); }
                    }
                }
                Err(_) => return out,
            }
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lang: Lang, src: &str) -> Vec<(String, Kind)> {
        highlight(lang, src).into_iter().map(|(r, k)| (src[r].to_string(), k)).collect()
    }

    #[test]
    fn rust_关键字与字符串() {
        let k = kinds(Lang::Rust, "fn main() { let s = \"hi\"; }");
        assert!(k.contains(&("fn".into(), Kind::Keyword)), "{k:?}");
        assert!(k.iter().any(|(t, kind)| t.contains("hi") && *kind == Kind::String), "{k:?}");
    }

    #[test]
    fn typescript_类型与关键字() {
        let k = kinds(Lang::TypeScript, "export interface User { id: number; }");
        assert!(k.iter().any(|(t, kind)| t == "export" && *kind == Kind::Keyword), "{k:?}");
        assert!(k.iter().any(|(t, kind)| t == "User" && *kind == Kind::Type), "{k:?}");
    }

    #[test]
    fn json_与_python_可用() {
        assert!(!highlight(Lang::Json, "{\"a\": 1}").is_empty());
        let k = kinds(Lang::Python, "def f():\n  return 1  # hi");
        assert!(k.iter().any(|(_, kind)| *kind == Kind::Comment), "{k:?}");
    }

    #[test]
    fn 区间不重叠且有序() {
        let sp = highlight(Lang::Rust, "fn a() -> i32 { 1 + 2 } // c");
        for w in sp.windows(2) { assert!(w[0].0.end <= w[1].0.start, "{sp:?}"); }
    }

    #[test]
    fn 扩展名识别() {
        assert_eq!(lang_for_path("src/a.rs"), Some(Lang::Rust));
        assert_eq!(lang_for_path("x.tsx"), Some(Lang::Tsx));
        assert_eq!(lang_for_path("README"), None);
        assert_eq!(lang_for_name("ts"), Some(Lang::TypeScript));
    }
}
