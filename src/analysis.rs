//! 后台语法分析：高亮区间 + 符号大纲 + 折叠范围，一次任务算完，结果带文本版本号送回界面线程。
//! 界面线程永远不做整文件解析：输入时先把旧高亮按编辑位置平移（`shift_spans`），分析结果回来再替换。
use crate::outline::{self, Symbol};
use crate::syntax::{self, Kind, Lang};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;
use std::sync::mpsc;

pub struct Analysis {
    pub version: u64,
    pub spans: Vec<(Range<usize>, Kind)>,
    pub symbols: Vec<Symbol>,
    pub folds: Vec<(usize, usize)>,
}

pub struct Job {
    /// 编辑器标识：同一个编辑器积压的任务只保留最新的
    pub id: u64,
    pub version: u64,
    pub lang: Lang,
    pub text: String,
    pub reply: async_channel::Sender<Analysis>,
}

static WORKER: OnceLock<mpsc::Sender<Job>> = OnceLock::new();

fn worker() -> &'static mpsc::Sender<Job> {
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new().name("syntax-worker".into()).spawn(move || {
            while let Ok(first) = rx.recv() {
                // 把积压的任务合并：每个编辑器只做最新的一个
                let mut latest: HashMap<u64, Job> = HashMap::new();
                latest.insert(first.id, first);
                while let Ok(j) = rx.try_recv() { latest.insert(j.id, j); }
                for (_, j) in latest {
                    let spans = if j.text.len() < 2_000_000 { syntax::highlight(j.lang, &j.text) } else { vec![] };
                    let (symbols, folds) = if j.text.len() < 1_000_000 { (outline::symbols(j.lang, &j.text), outline::fold_ranges(j.lang, &j.text)) } else { (vec![], vec![]) };
                    let _ = j.reply.send_blocking(Analysis { version: j.version, spans, symbols, folds });
                }
            }
        }).expect("无法启动语法分析线程");
        tx
    })
}

pub fn submit(job: Job) { let _ = worker().send(job); }

/// 一次编辑（在 `start` 处，原 `old_len` 字节变成 `new_len` 字节）之后，平移已有的高亮区间；
/// 与编辑区域重叠的区间丢弃（等后台结果补上）。
pub fn shift_spans(spans: &mut Vec<(Range<usize>, Kind)>, start: usize, old_len: usize, new_len: usize) {
    if old_len == new_len && old_len == 0 { return; }
    let old_end = start + old_len;
    let delta = new_len as isize - old_len as isize;
    spans.retain_mut(|(r, _)| {
        if r.end <= start { true }
        else if r.start >= old_end { r.start = (r.start as isize + delta) as usize; r.end = (r.end as isize + delta) as usize; true }
        else { false }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(a: usize, b: usize) -> (Range<usize>, Kind) { (a..b, Kind::Keyword) }

    #[test]
    fn 编辑之前的区间不动_之后的平移_重叠的丢弃() {
        let mut v = vec![sp(0, 3), sp(5, 9), sp(12, 15)];
        shift_spans(&mut v, 6, 1, 4); // 在 6 处把 1 字节换成 4 字节
        assert_eq!(v, vec![sp(0, 3), sp(15, 18)]); // 5..9 与编辑重叠被丢弃；12..15 → 15..18
    }

    #[test]
    fn 删除使后面的区间前移() {
        let mut v = vec![sp(10, 12)];
        shift_spans(&mut v, 2, 5, 0);
        assert_eq!(v, vec![sp(5, 7)]);
    }

    #[test]
    fn 在区间边界插入不影响前面的区间() {
        let mut v = vec![sp(0, 3), sp(3, 6)];
        shift_spans(&mut v, 3, 0, 2);
        assert_eq!(v, vec![sp(0, 3), sp(5, 8)]);
    }

    #[test]
    fn 工作线程产出与版本号一致的结果() {
        let (tx, rx) = async_channel::unbounded();
        submit(Job { id: 1, version: 7, lang: Lang::Rust, text: "fn a() {\n    1\n}\n".into(), reply: tx });
        let a = rx.recv_blocking().unwrap();
        assert_eq!(a.version, 7);
        assert!(!a.spans.is_empty());
        assert_eq!(a.symbols[0].name, "a");
        assert_eq!(a.folds, vec![(0, 2)]);
    }
}
