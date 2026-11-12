//! 符号大纲：用 tree-sitter 语法树列出函数 / 类型 / 类等定义。
//! 错了表现为：大纲里漏了方法、嵌套层级错乱，或面包屑显示成了错误的函数名。
use crate::syntax::{Lang, ts_language};

#[derive(Clone, Debug, PartialEq)]
pub struct Symbol {
    pub name: String,
    /// 简短类型标签：fn / struct / enum / trait / impl / mod / class / interface / type
    pub kind: &'static str,
    pub line: usize,
    pub end_line: usize,
    pub depth: usize,
}

fn kind_of(lang: Lang, node: &str) -> Option<&'static str> {
    Some(match lang {
        Lang::Rust => match node {
            "function_item" => "fn", "struct_item" => "struct", "enum_item" => "enum", "trait_item" => "trait", "impl_item" => "impl",
            "mod_item" => "mod", "const_item" => "const", "type_item" => "type", "macro_definition" => "macro", _ => return None,
        },
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => match node {
            "function_declaration" | "generator_function_declaration" => "fn", "class_declaration" => "class", "method_definition" => "fn",
            "interface_declaration" => "interface", "type_alias_declaration" => "type", "enum_declaration" => "enum", _ => return None,
        },
        Lang::Python => match node { "function_definition" => "fn", "class_definition" => "class", _ => return None },
        Lang::Go => match node { "function_declaration" | "method_declaration" => "fn", "type_spec" => "type", _ => return None },
        _ => return None,
    })
}

fn name_of(node: tree_sitter::Node, src: &[u8]) -> Option<String> {
    let text = |n: tree_sitter::Node| n.utf8_text(src).ok().map(|s| s.to_string());
    if node.kind() == "impl_item" {
        // impl Trait for Type / impl Type
        let ty = node.child_by_field_name("type").and_then(text)?;
        return Some(match node.child_by_field_name("trait").and_then(text) { Some(t) => format!("{t} for {ty}"), None => ty });
    }
    node.child_by_field_name("name").and_then(text)
}

pub fn symbols(lang: Lang, text: &str) -> Vec<Symbol> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&ts_language(lang)).is_err() { return vec![]; }
    let Some(tree) = parser.parse(text, None) else { return vec![] };
    let mut out = vec![];
    fn walk(node: tree_sitter::Node, lang: Lang, src: &[u8], depth: usize, out: &mut Vec<Symbol>) {
        let mut d = depth;
        if let Some(kind) = kind_of(lang, node.kind()) {
            if let Some(name) = name_of(node, src) {
                out.push(Symbol { name, kind, line: node.start_position().row, end_line: node.end_position().row, depth });
                d += 1;
            }
        }
        let mut c = node.walk();
        for child in node.children(&mut c) { walk(child, lang, src, d, out); }
    }
    walk(tree.root_node(), lang, text.as_bytes(), 0, &mut out);
    out
}

/// 光标所在行的符号路径（由外到内），用于面包屑。
pub fn path_at(symbols: &[Symbol], line: usize) -> Vec<&Symbol> {
    let mut path: Vec<&Symbol> = vec![];
    for s in symbols {
        if s.line <= line && line <= s.end_line {
            while path.last().map(|p| p.depth >= s.depth).unwrap_or(false) { path.pop(); }
            path.push(s);
        }
    }
    path
}

/// 可折叠区域：(标题行, 结束行)，折叠时隐藏 标题行+1 ..= 结束行。同一标题行只保留最外层。
pub fn fold_ranges(lang: Lang, text: &str) -> Vec<(usize, usize)> {
    let kinds: &[&str] = match lang {
        Lang::Rust => &["block", "declaration_list", "field_declaration_list", "enum_variant_list", "match_block"],
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => &["statement_block", "class_body", "object", "array", "object_type", "interface_body", "switch_body", "enum_body"],
        Lang::Python => &["block"],
        Lang::Json => &["object", "array"],
        Lang::Css => &["block"],
        Lang::Go => &["block", "field_declaration_list", "interface_type"],
        Lang::Bash => &["compound_statement"],
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&ts_language(lang)).is_err() { return vec![]; }
    let Some(tree) = parser.parse(text, None) else { return vec![] };
    let mut best: std::collections::BTreeMap<usize, usize> = Default::default();
    fn walk(node: tree_sitter::Node, kinds: &[&str], python: bool, out: &mut std::collections::BTreeMap<usize, usize>) {
        if kinds.contains(&node.kind()) {
            let (s, e) = (node.start_position().row, node.end_position().row);
            // Python 的 block 从第一条语句开始，标题行是它的上一行
            let head = if python { s.saturating_sub(1) } else { s };
            if e > head + 1 || (e > head && !python) {
                let slot = out.entry(head).or_insert(e);
                if e > *slot { *slot = e; }
            }
        }
        let mut c = node.walk();
        for ch in node.children(&mut c) { walk(ch, kinds, python, out); }
    }
    walk(tree.root_node(), kinds, lang == Lang::Python, &mut best);
    best.into_iter().filter(|(h, e)| e > h).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RS: &str = "struct A { x: i32 }\nimpl A {\n    fn new() -> Self { A { x: 0 } }\n    fn get(&self) -> i32 { self.x }\n}\nfn main() {}\n";

    #[test]
    fn rust_符号与层级() {
        let s = symbols(Lang::Rust, RS);
        let v: Vec<_> = s.iter().map(|x| (x.name.as_str(), x.kind, x.depth, x.line)).collect();
        assert_eq!(v, vec![("A", "struct", 0, 0), ("A", "impl", 0, 1), ("new", "fn", 1, 2), ("get", "fn", 1, 3), ("main", "fn", 0, 5)]);
    }

    #[test]
    fn rust_trait_impl_名称() {
        let s = symbols(Lang::Rust, "impl Drop for Foo { fn drop(&mut self) {} }");
        assert_eq!(s[0].name, "Drop for Foo");
    }

    #[test]
    fn typescript_类与方法() {
        let s = symbols(Lang::TypeScript, "export interface User { id: number }\nclass C {\n  m() {}\n}\nfunction f() {}\n");
        let v: Vec<_> = s.iter().map(|x| (x.name.as_str(), x.kind, x.depth)).collect();
        assert_eq!(v, vec![("User", "interface", 0), ("C", "class", 0), ("m", "fn", 1), ("f", "fn", 0)]);
    }

    #[test]
    fn python_与_json() {
        let s = symbols(Lang::Python, "class K:\n    def a(self):\n        pass\n");
        assert_eq!(s.iter().map(|x| (x.name.as_str(), x.depth)).collect::<Vec<_>>(), vec![("K", 0), ("a", 1)]);
        assert!(symbols(Lang::Json, "{}").is_empty());
    }

    #[test]
    fn 折叠范围_rust() {
        let src = "fn a() {\n    let x = 1;\n    if x > 0 {\n        x\n    }\n}\nfn b() {}\n";
        let f = fold_ranges(Lang::Rust, src);
        assert_eq!(f, vec![(0, 5), (2, 4)]);
    }

    #[test]
    fn 折叠范围_python_与_json() {
        let f = fold_ranges(Lang::Python, "def f():\n    a = 1\n    return a\nx = 2\n");
        assert_eq!(f, vec![(0, 2)]);
        let j = fold_ranges(Lang::Json, "{\n  \"a\": [\n    1,\n    2\n  ]\n}\n");
        assert_eq!(j, vec![(0, 5), (1, 4)]);
        assert!(fold_ranges(Lang::Rust, "fn a() { 1 }").is_empty(), "单行块不可折叠");
    }

    #[test]
    fn 面包屑取光标所在的最内层() {
        let s = symbols(Lang::Rust, RS);
        let p: Vec<_> = path_at(&s, 3).iter().map(|x| x.name.as_str()).collect();
        assert_eq!(p, vec!["A", "get"]);
        assert!(path_at(&s, 4).iter().all(|x| x.name != "get"));
        assert!(path_at(&s, 99).is_empty());
    }
}
