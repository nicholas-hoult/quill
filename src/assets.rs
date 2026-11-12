use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;

macro_rules! icons {
    ($($n:literal),* $(,)?) => {
        const ICONS: &[(&str, &[u8])] = &[$((concat!("icons/", $n, ".svg"), include_bytes!(concat!("../assets/icons/", $n, ".svg")))),*];
    };
}

icons!(
    "sidebar", "agent", "terminal", "search", "chevron", "chevron_down", "x", "plus", "send", "stop", "branch", "check", "copy",
    "folder", "folder_open", "file", "settings", "trash", "git", "sun", "alert", "undo",
);

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS.iter().find(|(p, _)| *p == path).map(|(_, b)| Cow::Borrowed(*b)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)).collect())
    }
}
