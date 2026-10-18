//! 极简本地静态文件服务器（仅监听 127.0.0.1），供 HTML 动态壁纸使用。
//!
//! 为什么需要它：Tauri 的 asset 协议把整条 Windows 路径 percent-encode 成
//! 一个 URL 段（`http://asset.localhost/C%3A%5Cdir%5Cindex.html`），浏览器做
//! 相对路径解析时会把最后一段整个替换掉，`style.css` 变成
//! `http://asset.localhost/style.css`，HTML 里的 CSS/JS/图片全部 404。
//! 用本地 HTTP 服务以 HTML 所在目录为根，相对路径、ES module、fetch 等
//! 浏览器语义就都正常了。
//!
//! 约束：只绑定回环地址、只服务指定根目录内的文件、只支持 GET/HEAD。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 运行中的静态服务器句柄；drop 时通知线程退出。
pub struct LocalServer {
    port: u16,
    stop: Arc<AtomicBool>,
}

impl LocalServer {
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// 在 127.0.0.1 的随机端口启动静态服务器，根目录为 `root`。
pub fn start(root: PathBuf) -> Result<LocalServer, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("启动 HTML 壁纸本地服务失败: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("读取本地服务端口失败: {e}"))?
        .port();
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("配置本地服务失败: {e}"))?;

    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = stop.clone();
    let root = Arc::new(root);

    std::thread::Builder::new()
        .name("wallpaper-http".into())
        .spawn(move || {
            while !stop_flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let root = root.clone();
                        // 每个请求一个短生命周期线程：壁纸页面并发很低
                        std::thread::spawn(move || {
                            let _ = handle(stream, &root);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(120));
                    }
                    Err(_) => break,
                }
            }
        })
        .map_err(|e| format!("启动 HTML 壁纸服务线程失败: {e}"))?;

    Ok(LocalServer { port, stop })
}

fn handle(mut stream: TcpStream, root: &Path) -> std::io::Result<()> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf)?;
    if n == 0 {
        return Ok(());
    }
    let head = String::from_utf8_lossy(&buf[..n]);
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or("/");

    if method != "GET" && method != "HEAD" {
        return write_response(&mut stream, 405, "text/plain; charset=utf-8", b"Method Not Allowed", method == "HEAD");
    }

    let path_part = target.split(['?', '#']).next().unwrap_or("/");
    let decoded = percent_decode(path_part);
    let rel = decoded.trim_start_matches('/');
    let mut rel_path = PathBuf::new();
    for comp in Path::new(rel).components() {
        match comp {
            Component::Normal(part) => rel_path.push(part),
            Component::CurDir => {}
            _ => {
                return write_response(&mut stream, 403, "text/plain; charset=utf-8", b"Forbidden", false);
            }
        }
    }
    if rel_path.as_os_str().is_empty() {
        rel_path.push("index.html");
    }

    let Ok(root_canon) = root.canonicalize() else {
        return write_response(&mut stream, 500, "text/plain; charset=utf-8", b"Invalid root", false);
    };
    let candidate = root_canon.join(&rel_path);
    let Ok(file_path) = candidate.canonicalize() else {
        return write_response(&mut stream, 404, "text/plain; charset=utf-8", b"Not Found", method == "HEAD");
    };
    if !file_path.starts_with(&root_canon) {
        return write_response(&mut stream, 403, "text/plain; charset=utf-8", b"Forbidden", false);
    }
    let file_path = if file_path.is_dir() {
        let index = file_path.join("index.html");
        if !index.is_file() {
            return write_response(&mut stream, 404, "text/plain; charset=utf-8", b"Not Found", method == "HEAD");
        }
        index
    } else {
        file_path
    };

    let body = std::fs::read(&file_path).unwrap_or_default();
    let mime = mime_for(&file_path);
    write_response(&mut stream, 200, mime, &body, method == "HEAD")
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    mime: &str,
    body: &[u8],
    head_only: bool,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    if !head_only {
        stream.write_all(body)?;
    }
    stream.flush()
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn mime_for(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "map" => "application/json; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" | "jfif" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}
