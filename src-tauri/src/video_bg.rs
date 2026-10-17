/// 打开原生文件对话框选择视频文件，返回绝对路径
#[tauri::command]
pub fn pick_video_file() -> Result<Option<String>, String> {
    let file = rfd::FileDialog::new()
        .add_filter("视频文件 (mp4/webm)", &["mp4", "webm"])
        .pick_file();

    Ok(file.map(|p| p.to_string_lossy().to_string()))
}
