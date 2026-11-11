mod ai;
mod fs;
mod git;
mod pty;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(pty::Ptys::default())
        .manage(ai::Streams::default())
        .invoke_handler(tauri::generate_handler![
            fs::read_dir,
            fs::read_file,
            fs::write_file,
            fs::create_entry,
            fs::rename_entry,
            fs::delete_entry,
            fs::list_files,
            fs::search_text,
            git::git_status,
            git::git_branch,
            git::git_head_content,
            pty::pty_spawn,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_kill,
            ai::get_settings,
            ai::set_settings,
            ai::ai_stream,
            ai::ai_cancel,
        ])
        .run(tauri::generate_context!())
        .expect("启动失败");
}
