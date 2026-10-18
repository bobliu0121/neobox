//! 本地文件快速搜索（类似 Everything）。
//!
//! 索引策略：
//! - 整盘（NTFS）：通过 `FSCTL_ENUM_USN_DATA` 直接枚举 MFT，几秒内完成百万级文件索引；
//! - 指定文件夹 / 非 NTFS 卷 / USN 不可用：退化为 `walkdir` 递归扫描。
//!
//! 索引只保存文件名与父子关系（不保存完整路径），查询时按需拼接路径并读取元数据，
//! 在保持内存占用的同时做到输入即搜。索引仅存在于进程内，重启后重建。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

const COLLECT_CAP: usize = 4000;
const MAX_RESULTS_LIMIT: usize = 2000;
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;

// ────────────────────────────── 全局状态 ──────────────────────────────

static INDEX: Mutex<Option<Arc<SearchIndex>>> = Mutex::new(None);
static BUILDING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
static STATUS: Mutex<BuildStatus> = Mutex::new(BuildStatus {
    ready: false,
    indexing: false,
    entries: 0,
    dirs: 0,
    files: 0,
    roots: Vec::new(),
    built_at: 0,
    elapsed_ms: 0,
    scanned: 0,
    error: None,
});

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchIndexStatus {
    pub ready: bool,
    pub indexing: bool,
    pub entries: usize,
    pub dirs: usize,
    pub files: usize,
    pub roots: Vec<String>,
    pub built_at: i64,
    pub elapsed_ms: u64,
    pub scanned: u64,
    pub error: Option<String>,
}

struct BuildStatus {
    ready: bool,
    indexing: bool,
    entries: usize,
    dirs: usize,
    files: usize,
    roots: Vec<String>,
    built_at: i64,
    elapsed_ms: u64,
    scanned: u64,
    error: Option<String>,
}

impl BuildStatus {
    fn to_public(&self) -> SearchIndexStatus {
        SearchIndexStatus {
            ready: self.ready,
            indexing: self.indexing,
            entries: self.entries,
            dirs: self.dirs,
            files: self.files,
            roots: self.roots.clone(),
            built_at: self.built_at,
            elapsed_ms: self.elapsed_ms,
            scanned: self.scanned,
            error: self.error.clone(),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchProgress {
    pub indexing: bool,
    pub scanned: u64,
    pub message: String,
    pub done: bool,
    pub error: Option<String>,
}

// ────────────────────────────── 索引结构 ──────────────────────────────

struct DirNode {
    name: String,
    parent: Option<u32>,
    hidden: bool,
}

struct FileNode {
    name: String,
    dir: u32,
    hidden: bool,
}

struct SearchIndex {
    dirs: Vec<DirNode>,
    files: Vec<FileNode>,
}

impl SearchIndex {
    fn resolve_dir_path(&self, id: u32) -> String {
        let mut parts: Vec<&str> = Vec::new();
        let mut current = Some(id);
        let mut guard = 0usize;
        while let Some(dir_id) = current {
            let dir = match self.dirs.get(dir_id as usize) {
                Some(d) => d,
                None => break,
            };
            parts.push(dir.name.as_str());
            current = dir.parent;
            guard += 1;
            if guard > 256 {
                break;
            }
        }
        parts.reverse();
        let mut path = String::new();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 && !path.ends_with('\\') {
                path.push('\\');
            }
            path.push_str(part);
        }
        path
    }

    fn file_path(&self, file: &FileNode) -> String {
        let parent = self.resolve_dir_path(file.dir);
        if parent.ends_with('\\') {
            format!("{}{}", parent, file.name)
        } else {
            format!("{}\\{}", parent, file.name)
        }
    }

    fn search(&self, options: &SearchOptions) -> SearchResponse {
        let started = Instant::now();
        let query = options.query.trim().to_string();
        let case_sensitive = options.case_sensitive;
        let include_dirs = options.include_dirs;
        let include_files = options.include_files;
        let include_hidden = options.include_hidden;
        let extensions: HashSet<String> = options
            .extensions
            .iter()
            .map(|ext| ext.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|ext| !ext.is_empty())
            .collect();
        let max_results = options.max_results.clamp(1, MAX_RESULTS_LIMIT);

        let mut matches: Vec<MatchRef> = Vec::new();
        let mut total = 0usize;

        if query.is_empty() {
            return SearchResponse {
                total: 0,
                items: Vec::new(),
                took_ms: 0,
                truncated: false,
                index_entries: self.dirs.len() + self.files.len(),
            };
        }

        if include_dirs {
            for (i, dir) in self.dirs.iter().enumerate().skip(1) {
                if dir.hidden && !include_hidden {
                    continue;
                }
                if !name_matches(&dir.name, &query, case_sensitive) {
                    continue;
                }
                total += 1;
                if matches.len() < COLLECT_CAP {
                    matches.push(MatchRef {
                        is_dir: true,
                        id: i as u32,
                        score: match_score(&dir.name, &query, case_sensitive),
                    });
                }
            }
        }

        if include_files {
            for (i, file) in self.files.iter().enumerate() {
                if file.hidden && !include_hidden {
                    continue;
                }
                if !extensions.is_empty() {
                    let ext = file
                        .name
                        .rsplit_once('.')
                        .map(|(_, ext)| ext.to_ascii_lowercase());
                    match ext {
                        Some(ext) if extensions.contains(&ext) => {}
                        _ => continue,
                    }
                }
                if !name_matches(&file.name, &query, case_sensitive) {
                    continue;
                }
                total += 1;
                if matches.len() < COLLECT_CAP {
                    matches.push(MatchRef {
                        is_dir: false,
                        id: i as u32,
                        score: match_score(&file.name, &query, case_sensitive),
                    });
                }
            }
        }

        matches.sort_by(|a, b| {
            a.score
                .cmp(&b.score)
                .then_with(|| {
                    let name_a = self.match_name(a);
                    let name_b = self.match_name(b);
                    name_a
                        .len()
                        .cmp(&name_b.len())
                        .then_with(|| name_a.cmp(name_b))
                })
                .then_with(|| match (a.is_dir, b.is_dir) {
                    (false, true) => std::cmp::Ordering::Less,
                    (true, false) => std::cmp::Ordering::Greater,
                    _ => std::cmp::Ordering::Equal,
                })
        });

        let truncated = total > max_results;
        let items: Vec<SearchItem> = matches
            .into_iter()
            .take(max_results)
            .map(|entry| {
                if entry.is_dir {
                    let dir = &self.dirs[entry.id as usize];
                    SearchItem {
                        name: dir.name.clone(),
                        path: self.resolve_dir_path(entry.id),
                        is_dir: true,
                        size: None,
                        modified: None,
                        extension: None,
                        hidden: dir.hidden,
                    }
                } else {
                    let file = &self.files[entry.id as usize];
                    let path = self.file_path(file);
                    let (size, modified) = read_metadata(&path);
                    SearchItem {
                        name: file.name.clone(),
                        path,
                        is_dir: false,
                        size,
                        modified,
                        extension: file
                            .name
                            .rsplit_once('.')
                            .map(|(_, ext)| ext.to_ascii_lowercase()),
                        hidden: file.hidden,
                    }
                }
            })
            .collect();

        SearchResponse {
            total,
            items,
            took_ms: started.elapsed().as_millis() as u64,
            truncated,
            index_entries: self.dirs.len() + self.files.len(),
        }
    }

    fn match_name(&self, entry: &MatchRef) -> &str {
        if entry.is_dir {
            &self.dirs[entry.id as usize].name
        } else {
            &self.files[entry.id as usize].name
        }
    }
}

struct MatchRef {
    is_dir: bool,
    id: u32,
    score: u8,
}

fn read_metadata(path: &str) -> (Option<u64>, Option<i64>) {
    match std::fs::metadata(path) {
        Ok(meta) => {
            let modified = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs() as i64);
            (Some(meta.len()), modified)
        }
        Err(_) => (None, None),
    }
}

fn name_matches(name: &str, query: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        name.contains(query)
    } else {
        contains_ignore_ascii_case(name, query)
    }
}

/// 大小写不敏感子串匹配。ASCII 查询走字节窗口比较（不分配内存），
/// 非 ASCII 查询（如中文）直接按原文匹配，避免对整个索引做 to_lowercase。
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.is_ascii() {
        let hay = haystack.as_bytes();
        let pattern = needle.as_bytes();
        if pattern.len() > hay.len() {
            return false;
        }
        hay.windows(pattern.len())
            .any(|window| window.eq_ignore_ascii_case(pattern))
    } else {
        haystack.contains(needle)
    }
}

fn match_score(name: &str, query: &str, case_sensitive: bool) -> u8 {
    if case_sensitive {
        if name == query {
            return 0;
        }
        if name.starts_with(query) {
            return 1;
        }
        return 2;
    }
    if name.eq_ignore_ascii_case(query) && name.len() == query.len() {
        return 0;
    }
    if name.len() >= query.len()
        && name.as_bytes()[..query.len()].eq_ignore_ascii_case(query.as_bytes())
    {
        return 1;
    }
    2
}

// ────────────────────────────── IPC 类型 ──────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOptions {
    pub query: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default = "default_true")]
    pub include_dirs: bool,
    #[serde(default = "default_true")]
    pub include_files: bool,
    #[serde(default = "default_true")]
    pub include_hidden: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

fn default_true() -> bool {
    true
}

fn default_max_results() -> usize {
    500
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchItem {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub modified: Option<i64>,
    pub extension: Option<String>,
    pub hidden: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub total: usize,
    pub items: Vec<SearchItem>,
    pub took_ms: u64,
    pub truncated: bool,
    pub index_entries: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchDrive {
    pub letter: String,
    pub label: String,
    pub total: u64,
    pub free: u64,
    pub file_system: String,
}

// ────────────────────────────── 磁盘列表 ──────────────────────────────

#[tauri::command]
pub fn search_get_drives() -> Vec<SearchDrive> {
    #[cfg(windows)]
    {
        windows_drives()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(windows)]
fn windows_drives() -> Vec<SearchDrive> {
    use std::os::windows::ffi::OsStrExt;
    use winapi::um::fileapi::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    };
    use winapi::um::winbase::{DRIVE_FIXED, DRIVE_REMOVABLE};

    fn wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let mut drives = Vec::new();
    let mask = unsafe { GetLogicalDrives() };
    for index in 0..26u32 {
        if mask & (1 << index) == 0 {
            continue;
        }
        let letter = (b'A' + index as u8) as char;
        let root = format!("{}:\\", letter);
        let root_wide = wide(&root);
        let drive_type = unsafe { GetDriveTypeW(root_wide.as_ptr()) };
        if drive_type != DRIVE_FIXED && drive_type != DRIVE_REMOVABLE {
            continue;
        }
        let mut label_buffer = [0u16; 261];
        let mut fs_buffer = [0u16; 32];
        let mut serial = 0u32;
        let mut max_component = 0u32;
        let mut flags = 0u32;
        let ok = unsafe {
            GetVolumeInformationW(
                root_wide.as_ptr(),
                label_buffer.as_mut_ptr(),
                label_buffer.len() as u32,
                &mut serial,
                &mut max_component,
                &mut flags,
                fs_buffer.as_mut_ptr(),
                fs_buffer.len() as u32,
            )
        };
        let label = if ok != 0 {
            String::from_utf16_lossy(&label_buffer[..label_buffer.iter().position(|c| *c == 0).unwrap_or(0)])
        } else {
            String::new()
        };
        let file_system = if ok != 0 {
            String::from_utf16_lossy(&fs_buffer[..fs_buffer.iter().position(|c| *c == 0).unwrap_or(0)])
        } else {
            String::new()
        };
        let mut free_bytes = 0u64;
        let mut total_bytes = 0u64;
        let mut total_free = 0u64;
        let size_ok = unsafe {
            GetDiskFreeSpaceExW(
                root_wide.as_ptr(),
                &mut free_bytes as *mut u64 as *mut _,
                &mut total_bytes as *mut u64 as *mut _,
                &mut total_free as *mut u64 as *mut _,
            )
        };
        drives.push(SearchDrive {
            letter: format!("{}:", letter),
            label: if label.is_empty() {
                if drive_type == DRIVE_REMOVABLE {
                    "可移动磁盘".to_string()
                } else {
                    "本地磁盘".to_string()
                }
            } else {
                label
            },
            total: if size_ok != 0 { total_bytes } else { 0 },
            free: if size_ok != 0 { free_bytes } else { 0 },
            file_system,
        });
    }
    drives
}

// ────────────────────────────── 索引构建 ──────────────────────────────

#[tauri::command]
pub fn search_get_status() -> SearchIndexStatus {
    STATUS.lock().unwrap().to_public()
}

#[tauri::command]
pub async fn search_build_index(app: AppHandle, roots: Option<Vec<String>>) -> Result<SearchIndexStatus, String> {
    start_index_build(app, roots)
}

#[tauri::command]
pub fn search_cancel_index() {
    CANCEL.store(true, Ordering::SeqCst);
}

fn start_index_build(app: AppHandle, roots: Option<Vec<String>>) -> Result<SearchIndexStatus, String> {
    if BUILDING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(STATUS.lock().unwrap().to_public());
    }
    CANCEL.store(false, Ordering::SeqCst);

    let roots = normalize_roots(roots);
    if roots.is_empty() {
        BUILDING.store(false, Ordering::SeqCst);
        return Err("没有可索引的磁盘或目录".to_string());
    }

    {
        let mut status = STATUS.lock().unwrap();
        status.indexing = true;
        status.scanned = 0;
        status.error = None;
        status.roots = roots.clone();
    }
    emit_progress(&app, true, 0, "正在准备索引...".to_string(), false, None);

    let app_handle = app.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let result = build_index(&app_handle, &roots);
        let elapsed = started.elapsed().as_millis() as u64;
        match result {
            Ok(index) => {
                let dirs = index.dirs.len();
                let files = index.files.len();
                let built_at = unix_now();
                let arc = Arc::new(index);
                *INDEX.lock().unwrap() = Some(arc);
                {
                    let mut status = STATUS.lock().unwrap();
                    status.ready = true;
                    status.indexing = false;
                    status.entries = dirs + files;
                    status.dirs = dirs;
                    status.files = files;
                    status.built_at = built_at;
                    status.elapsed_ms = elapsed;
                    status.error = None;
                }
                BUILDING.store(false, Ordering::SeqCst);
                emit_progress(
                    &app_handle,
                    false,
                    (dirs + files) as u64,
                    format!("索引完成：{} 个目录，{} 个文件", dirs, files),
                    true,
                    None,
                );
            }
            Err(error) => {
                {
                    let mut status = STATUS.lock().unwrap();
                    status.indexing = false;
                    status.error = Some(error.clone());
                }
                BUILDING.store(false, Ordering::SeqCst);
                emit_progress(&app_handle, false, 0, error.clone(), true, Some(error));
            }
        }
    });

    Ok(STATUS.lock().unwrap().to_public())
}

fn normalize_roots(roots: Option<Vec<String>>) -> Vec<String> {
    let requested: Vec<String> = match roots {
        Some(values) if !values.is_empty() => values,
        _ => {
            #[cfg(windows)]
            {
                windows_drives()
                    .into_iter()
                    .map(|drive| format!("{}\\", drive.letter))
                    .collect()
            }
            #[cfg(not(windows))]
            {
                Vec::new()
            }
        }
    };

    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for root in requested {
        let trimmed = root.trim().trim_end_matches(['\\', '/']).to_string();
        if trimmed.is_empty() {
            continue;
        }
        let normalized = if trimmed.len() == 2 && trimmed.ends_with(':') {
            format!("{}\\", trimmed)
        } else {
            trimmed
        };
        if !Path::new(&normalized).is_dir() {
            continue;
        }
        let key = normalized.to_ascii_lowercase();
        if seen.insert(key) {
            result.push(normalized);
        }
    }
    result
}

fn emit_progress(
    app: &AppHandle,
    indexing: bool,
    scanned: u64,
    message: String,
    done: bool,
    error: Option<String>,
) {
    let _ = app.emit(
        "search:progress",
        SearchProgress {
            indexing,
            scanned,
            message,
            done,
            error,
        },
    );
}

fn build_index(app: &AppHandle, roots: &[String]) -> Result<SearchIndex, String> {
    let mut dirs: Vec<DirNode> = Vec::new();
    let mut files: Vec<FileNode> = Vec::new();
    let mut scanned: u64 = 0;
    let mut last_emit = Instant::now();

    for root in roots {
        if CANCEL.load(Ordering::SeqCst) {
            return Err("索引已取消".to_string());
        }
        let message = format!("正在索引 {} ...", root);
        emit_progress(app, true, scanned, message, false, None);

        let drive_letter = drive_root_letter(root);
        let mut indexed = false;
        if let Some(letter) = drive_letter {
            if is_ntfs_volume(letter) {
                match enumerate_ntfs(
                    &letter,
                    app,
                    &mut dirs,
                    &mut files,
                    &mut scanned,
                    &mut last_emit,
                ) {
                    Ok(()) => indexed = true,
                    Err(error) => {
                        eprintln!("[Search] USN 枚举 {} 失败，回退到目录遍历：{}", letter, error);
                    }
                }
            }
        }
        if !indexed {
            walk_directory(root, &mut dirs, &mut files, &mut scanned, &mut last_emit, app)?;
        }
    }

    Ok(SearchIndex { dirs, files })
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn drive_root_letter(root: &str) -> Option<char> {
    let bytes = root.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        Some(bytes[0] as char)
    } else {
        None
    }
}

#[cfg(windows)]
fn is_ntfs_volume(letter: char) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use winapi::um::fileapi::GetVolumeInformationW;

    let root = format!("{}:\\", letter);
    let root_wide: Vec<u16> = std::ffi::OsStr::new(&root)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut fs_buffer = [0u16; 32];
    let ok = unsafe {
        GetVolumeInformationW(
            root_wide.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs_buffer.as_mut_ptr(),
            fs_buffer.len() as u32,
        )
    };
    if ok == 0 {
        return false;
    }
    let len = fs_buffer.iter().position(|c| *c == 0).unwrap_or(0);
    String::from_utf16_lossy(&fs_buffer[..len]).eq_ignore_ascii_case("NTFS")
}

#[cfg(not(windows))]
fn is_ntfs_volume(_letter: char) -> bool {
    false
}

// ────────────────────────────── NTFS / USN ──────────────────────────────

#[cfg(windows)]
fn enumerate_ntfs(
    letter: &char,
    app: &AppHandle,
    dirs: &mut Vec<DirNode>,
    files: &mut Vec<FileNode>,
    scanned: &mut u64,
    last_emit: &mut Instant,
) -> Result<(), String> {
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use winapi::shared::minwindef::{DWORD, LPVOID};
    use winapi::um::fileapi::{CreateFileW, OPEN_EXISTING};
    use winapi::um::handleapi::{CloseHandle, INVALID_HANDLE_VALUE};
    use winapi::um::ioapiset::DeviceIoControl;
    use winapi::um::winnt::{FILE_SHARE_READ, FILE_SHARE_WRITE, GENERIC_READ};

    const FSCTL_ENUM_USN_DATA: DWORD = 0x0009_00b3;
    const FSCTL_QUERY_USN_JOURNAL: DWORD = 0x0009_00f4;
    const ERROR_HANDLE_EOF: DWORD = 38;

    #[repr(C)]
    struct MftEnumDataV0 {
        start_file_reference_number: u64,
        low_usn: i64,
        high_usn: i64,
    }

    #[repr(C, align(8))]
    #[derive(Default)]
    struct UsnJournalDataV0 {
        usn_journal_id: u64,
        first_usn: i64,
        next_usn: i64,
        lowest_valid_usn: i64,
        max_usn: i64,
        maximum_size: u64,
        allocation_delta: u64,
    }

    let volume = format!("\\\\.\\{}:", letter);
    let volume_wide: Vec<u16> = std::ffi::OsStr::new(&volume)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            volume_wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null_mut(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!(
            "打开卷 {} 失败（错误码 {}）",
            volume,
            unsafe { winapi::um::errhandlingapi::GetLastError() }
        ));
    }

    let close = |handle: winapi::shared::ntdef::HANDLE| unsafe {
        CloseHandle(handle);
    };

    let mut journal = UsnJournalDataV0::default();
    let mut returned: DWORD = 0;
    let journal_ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_QUERY_USN_JOURNAL,
            ptr::null_mut(),
            0,
            &mut journal as *mut UsnJournalDataV0 as LPVOID,
            size_of::<UsnJournalDataV0>() as DWORD,
            &mut returned,
            ptr::null_mut(),
        )
    } != 0;

    let high_usn = if journal_ok && journal.next_usn > 0 {
        journal.next_usn
    } else {
        i64::MAX
    };

    let mut media = MftEnumDataV0 {
        start_file_reference_number: 0,
        low_usn: 0,
        high_usn,
    };

    let mut buffer = vec![0u8; 4 * 1024 * 1024];
    let mut raw_dirs: Vec<(u64, u64, String, bool)> = Vec::new();
    let mut raw_files: Vec<(u64, String, bool)> = Vec::new();

    loop {
        if CANCEL.load(Ordering::SeqCst) {
            close(handle);
            return Err("索引已取消".to_string());
        }
        let mut bytes_returned: DWORD = 0;
        let ok = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_ENUM_USN_DATA,
                &mut media as *mut MftEnumDataV0 as LPVOID,
                size_of::<MftEnumDataV0>() as DWORD,
                buffer.as_mut_ptr() as LPVOID,
                buffer.len() as DWORD,
                &mut bytes_returned,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            let last_error = unsafe { winapi::um::errhandlingapi::GetLastError() };
            if last_error == ERROR_HANDLE_EOF {
                break;
            }
            close(handle);
            return Err(format!("枚举 MFT 失败（错误码 {}）", last_error));
        }
        if bytes_returned < 8 {
            break;
        }

        let next_start = u64::from_ne_bytes(buffer[0..8].try_into().unwrap());
        let mut offset = 8usize;
        while offset + 60 <= bytes_returned as usize {
            let record_len =
                u32::from_ne_bytes(buffer[offset..offset + 4].try_into().unwrap()) as usize;
            if record_len < 60 || offset + record_len > bytes_returned as usize {
                break;
            }
            let major = u16::from_ne_bytes(buffer[offset + 4..offset + 6].try_into().unwrap());
            if major == 2 {
                let frn =
                    u64::from_ne_bytes(buffer[offset + 8..offset + 16].try_into().unwrap());
                let parent =
                    u64::from_ne_bytes(buffer[offset + 16..offset + 24].try_into().unwrap());
                let attrs = u32::from_ne_bytes(
                    buffer[offset + 52..offset + 56].try_into().unwrap(),
                );
                let name_len =
                    u16::from_ne_bytes(buffer[offset + 56..offset + 58].try_into().unwrap())
                        as usize;
                let name_offset =
                    u16::from_ne_bytes(buffer[offset + 58..offset + 60].try_into().unwrap())
                        as usize;
                if name_offset + name_len <= record_len {
                    let units: Vec<u16> = (0..name_len / 2)
                        .map(|i| {
                            let start = offset + name_offset + i * 2;
                            u16::from_ne_bytes(buffer[start..start + 2].try_into().unwrap())
                        })
                        .collect();
                    let name = String::from_utf16_lossy(&units);
                    if !name.is_empty() && name != "." && name != ".." {
                        let hidden = attrs & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0;
                        let is_dir = attrs & 0x10 != 0;
                        if is_dir {
                            if frn != 5 {
                                raw_dirs.push((frn, parent, name, hidden));
                            }
                        } else if !name.starts_with('$') {
                            raw_files.push((parent, name, hidden));
                        }
                        *scanned += 1;
                        if last_emit.elapsed().as_millis() >= 250 {
                            emit_progress(
                                app,
                                true,
                                *scanned,
                                format!("正在读取 {} 的 MFT 记录...", letter),
                                false,
                                None,
                            );
                            *last_emit = Instant::now();
                        }
                    }
                }
            }
            offset += record_len;
        }

        if next_start == 0 || next_start == media.start_file_reference_number {
            break;
        }
        media.start_file_reference_number = next_start;
    }
    close(handle);

    let mut dir_map: HashMap<u64, u32> = HashMap::with_capacity(raw_dirs.len() + 1);
    dirs.push(DirNode {
        name: format!("{}:", letter),
        parent: None,
        hidden: false,
    });
    dir_map.insert(5, 0);
    for (frn, _, name, hidden) in raw_dirs.iter() {
        let id = dirs.len() as u32;
        dirs.push(DirNode {
            name: name.clone(),
            parent: None,
            hidden: *hidden,
        });
        dir_map.insert(*frn, id);
    }
    for (frn, parent, _, _) in raw_dirs.iter() {
        if let Some(id) = dir_map.get(frn).copied() {
            let parent_id = dir_map.get(parent).copied().unwrap_or(0);
            if parent_id != id {
                dirs[id as usize].parent = Some(parent_id);
            }
        }
    }
    for (parent, name, hidden) in raw_files {
        let parent_id = dir_map.get(&parent).copied().unwrap_or(0);
        files.push(FileNode {
            name,
            dir: parent_id,
            hidden,
        });
    }

    emit_progress(
        app,
        true,
        *scanned,
        format!("{} 索引完成，共 {} 项", letter, raw_dirs.len() + files.len()),
        false,
        None,
    );
    Ok(())
}

#[cfg(not(windows))]
fn enumerate_ntfs(
    _letter: &char,
    _app: &AppHandle,
    _dirs: &mut Vec<DirNode>,
    _files: &mut Vec<FileNode>,
    _scanned: &mut u64,
    _last_emit: &mut Instant,
) -> Result<(), String> {
    Err("仅支持 Windows".to_string())
}

// ────────────────────────────── 目录遍历 ──────────────────────────────

fn walk_directory(
    root: &str,
    dirs: &mut Vec<DirNode>,
    files: &mut Vec<FileNode>,
    scanned: &mut u64,
    last_emit: &mut Instant,
    app: &AppHandle,
) -> Result<(), String> {
    let root = root.trim_end_matches(['\\', '/']);
    let root_name = if root.is_empty() { "\\" } else { root };

    dirs.push(DirNode {
        name: root_name.to_string(),
        parent: None,
        hidden: false,
    });
    let mut dir_map: HashMap<PathBuf, u32> = HashMap::new();
    dir_map.insert(PathBuf::from(root_name), 0);

    for entry in walkdir::WalkDir::new(root_name)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if CANCEL.load(Ordering::SeqCst) {
            return Err("索引已取消".to_string());
        }
        let path = entry.path();
        if entry.depth() == 0 {
            continue;
        }
        let parent_path = match path.parent() {
            Some(parent) => parent,
            None => continue,
        };
        let parent_id = dir_map.get(parent_path).copied().unwrap_or(0);
        let name = entry.file_name().to_string_lossy().to_string();
        let hidden = is_hidden_path(path);
        let file_type = entry.file_type();
        if file_type.is_dir() {
            let id = dirs.len() as u32;
            dirs.push(DirNode {
                name,
                parent: Some(parent_id),
                hidden,
            });
            dir_map.insert(path.to_path_buf(), id);
        } else if file_type.is_file() {
            files.push(FileNode {
                name,
                dir: parent_id,
                hidden,
            });
        } else {
            continue;
        }
        *scanned += 1;
        if last_emit.elapsed().as_millis() >= 250 {
            emit_progress(
                app,
                true,
                *scanned,
                format!("正在扫描 {} ...", root_name),
                false,
                None,
            );
            *last_emit = Instant::now();
        }
    }

    emit_progress(
        app,
        true,
        *scanned,
        format!("{} 扫描完成", root_name),
        false,
        None,
    );
    Ok(())
}

#[cfg(windows)]
fn is_hidden_path(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    path.symlink_metadata()
        .map(|meta| meta.file_attributes() & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0)
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn is_hidden_path(path: &Path) -> bool {
    path.file_name()
        .map(|name| name.to_string_lossy().starts_with('.'))
        .unwrap_or(false)
}

// ────────────────────────────── 查询与打开 ──────────────────────────────

#[tauri::command]
pub async fn search_files(options: SearchOptions) -> Result<SearchResponse, String> {
    let index = INDEX
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "索引尚未建立，请先建立索引".to_string())?;
    tauri::async_runtime::spawn_blocking(move || index.search(&options))
        .await
        .map_err(|error| format!("搜索失败：{}", error))
}

#[tauri::command]
pub fn search_open_item(path: String) -> Result<(), String> {
    if !Path::new(&path).exists() {
        return Err("文件或目录不存在，可能已被移动或删除".to_string());
    }
    open_with_shell(&path)
}

#[cfg(windows)]
fn open_with_shell(path: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let operation = wide("open");
    let file = wide(path);
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result <= 32 {
        return Err(format!("无法打开（ShellExecute 返回 {}）", result));
    }
    Ok(())
}

#[cfg(not(windows))]
fn open_with_shell(path: &str) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn search_reveal_item(path: String) -> Result<(), String> {
    if !Path::new(&path).exists() {
        return Err("文件或目录不存在，可能已被移动或删除".to_string());
    }
    let argument = format!("/select,{}", path);
    std::process::Command::new("explorer")
        .arg(argument)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法打开资源管理器：{}", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_ascii_case_insensitively() {
        assert!(contains_ignore_ascii_case("HelloWorld.txt", "world"));
        assert!(contains_ignore_ascii_case("HelloWorld.txt", "HELLO"));
        assert!(!contains_ignore_ascii_case("HelloWorld.txt", "xyz"));
        assert!(contains_ignore_ascii_case("新建文件夹", "新建"));
    }

    #[test]
    fn index_walk_and_search() {
        let base = std::env::temp_dir().join(format!("neobox-search-test-{}", std::process::id()));
        let nested = base.join("docs").join("deep");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(base.join("alpha.txt"), b"a").unwrap();
        std::fs::write(nested.join("Beta-Report.PDF"), b"b").unwrap();

        let mut dirs = Vec::new();
        let mut files = Vec::new();
        let mut scanned = 0;
        // 直接使用目录遍历构建，避免测试环境要求管理员权限
        walk_directory_without_progress(&base.to_string_lossy(), &mut dirs, &mut files, &mut scanned);
        let index = SearchIndex { dirs, files };

        let response = index.search(&SearchOptions {
            query: "beta".to_string(),
            extensions: Vec::new(),
            include_dirs: true,
            include_files: true,
            include_hidden: true,
            case_sensitive: false,
            max_results: 50,
        });
        assert_eq!(response.total, 1);
        assert_eq!(response.items[0].name, "Beta-Report.PDF");

        let response = index.search(&SearchOptions {
            query: "report".to_string(),
            extensions: vec!["pdf".to_string()],
            include_dirs: false,
            include_files: true,
            include_hidden: true,
            case_sensitive: false,
            max_results: 50,
        });
        assert_eq!(response.total, 1);

        let response = index.search(&SearchOptions {
            query: "docs".to_string(),
            extensions: Vec::new(),
            include_dirs: true,
            include_files: false,
            include_hidden: true,
            case_sensitive: false,
            max_results: 50,
        });
        assert_eq!(response.total, 1);
        assert!(response.items[0].is_dir);

        std::fs::remove_dir_all(&base).ok();
    }

    // 测试用：与 walk_directory 相同的构建逻辑，但不发事件
    fn walk_directory_without_progress(
        root: &str,
        dirs: &mut Vec<DirNode>,
        files: &mut Vec<FileNode>,
        scanned: &mut u64,
    ) {
        let root = root.trim_end_matches(['\\', '/']);
        dirs.push(DirNode {
            name: root.to_string(),
            parent: None,
            hidden: false,
        });
        let mut dir_map: HashMap<PathBuf, u32> = HashMap::new();
        dir_map.insert(PathBuf::from(root), 0);
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            let path = entry.path();
            if entry.depth() == 0 {
                continue;
            }
            let parent_path = match path.parent() {
                Some(parent) => parent,
                None => continue,
            };
            let parent_id = dir_map.get(parent_path).copied().unwrap_or(0);
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.file_type().is_dir() {
                let id = dirs.len() as u32;
                dirs.push(DirNode {
                    name,
                    parent: Some(parent_id),
                    hidden: false,
                });
                dir_map.insert(path.to_path_buf(), id);
            } else if entry.file_type().is_file() {
                files.push(FileNode {
                    name,
                    dir: parent_id,
                    hidden: false,
                });
            }
            *scanned += 1;
        }
    }
}
