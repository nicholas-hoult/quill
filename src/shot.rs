//! 自截图：GPUI 没有截图 API，本机终端也没有屏幕录制权限。
//! 这里用 CoreGraphics 只截**本进程自己的窗口**（系统不要求权限），存成 PNG，供开发时肉眼检查界面。
//! 由环境变量 QUILL_SHOT 触发，正常使用不会走到这里。

#[cfg(target_os = "macos")]
pub fn capture_own_window(path: &str) -> Result<(), String> {
    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::display::{CGPoint, CGRect, CGSize};
    use core_graphics::window::{
        CGWindowListCopyWindowInfo, create_image, kCGNullWindowID, kCGWindowImageBoundsIgnoreFraming, kCGWindowListOptionIncludingWindow,
        kCGWindowListOptionOnScreenOnly,
    };

    let pid = std::process::id() as i64;
    let info = unsafe { CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID) };
    if info.is_null() { return Err("无法列出窗口".into()); }
    let arr: CFArray<CFDictionary<CFString, CFType>> = unsafe { CFArray::wrap_under_create_rule(info as _) };
    let key_pid = CFString::from_static_string("kCGWindowOwnerPID");
    let key_num = CFString::from_static_string("kCGWindowNumber");
    let key_layer = CFString::from_static_string("kCGWindowLayer");
    let mut found: Option<u32> = None;
    for d in arr.iter() {
        let get = |k: &CFString| d.find(k).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_i64());
        if get(&key_pid) == Some(pid) && get(&key_layer) == Some(0) {
            found = get(&key_num).map(|n| n as u32);
            break;
        }
    }
    let id = found.ok_or("没找到本进程的窗口（窗口不在屏幕上？）")?;
    let rect = CGRect::new(&CGPoint::new(f64::INFINITY, f64::INFINITY), &CGSize::new(0.0, 0.0));
    let img = create_image(rect, kCGWindowListOptionIncludingWindow, id, kCGWindowImageBoundsIgnoreFraming)
        .ok_or("CGWindowListCreateImage 返回空（可能被系统拦截）")?;
    let (w, h, bpr) = (img.width(), img.height(), img.bytes_per_row());
    let data = img.data();
    let bytes = data.bytes();
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = &bytes[y * bpr..y * bpr + w * 4];
        for px in row.chunks_exact(4) { rgba.extend_from_slice(&[px[2], px[1], px[0], 255]); }
    }
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().map_err(|e| e.to_string())?.write_image_data(&rgba).map_err(|e| e.to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn capture_own_window(_path: &str) -> Result<(), String> { Err("仅支持 macOS".into()) }
