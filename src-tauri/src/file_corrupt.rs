//! 文件随机损坏工具。
//!
//! # 用途
//!
//! 生成「可控损坏」的文件样本，用于测试文件修复 / 数据恢复 / 格式容错类工具的
//! 健壮性。真实场景里损坏往往只发生在文件局部（磁道坏块、传输截断），
//! 所以本工具的核心价值是**精确控制**：损坏多少、坏在哪、怎么坏、能否复现。
//!
//! # 安全约束（必须严格遵守）
//!
//! 1. **绝不碰原文件**：结果一律写到新路径（默认 `<原名>_corrupted.<扩展名>`）。
//!    本模块只有读取原文件的代码，没有任何写回原路径的分支。
//! 2. **拒绝系统关键目录**：误把系统文件弄坏会让机器无法启动，
//!    即使调用方传了这些路径也直接拒绝（见 [`is_protected_path`]）。
//! 3. **保留文件大小不变**：只替换字节内容，不插入/删除，符合真实损坏形态
//!    （长度变化通常是"截断"或"拼接"，属于另一类故障）。
//!
//! # 参数
//!
//! | 参数 | 含义 |
//! |---|---|
//! | `ratio` | 损坏比例（0~1），按文件大小的百分比决定改动字节数 |
//! | `bytes` | 精确损坏字节数（与 `ratio` 二选一，优先 `bytes`） |
//! | `start` / `end` | 限定损坏范围（字节偏移，`end` 为 None 表示到文件末尾） |
//! | `seed` | 随机种子：同一种子 + 同一文件 = 完全相同的损坏结果，便于复现对比 |
//! | `mode` | 损坏方式：随机字节 / 置零 / 置 0xFF |
//! | `keep_size` | 是否强制保持原大小（默认 true；置 false 时允许极小文件被整体改写） |

use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
/// 损坏方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorruptMode {
    /// 用随机字节覆盖（最接近真实损坏，磁道翻转/位错误）
    Random,
    /// 置零（模拟写入失败、全零块）
    Zero,
    /// 置 0xFF（模拟闪存擦除态、全一块）
    Ones,
}

impl Default for CorruptMode {
    fn default() -> Self {
        CorruptMode::Random
    }
}

/// 损坏请求参数
#[derive(Debug, Clone, Deserialize)]
pub struct CorruptRequest {
    /// 源文件绝对路径
    pub path: String,
    /// 输出路径；为空时自动生成 `<stem>_corrupted<suffix>`
    #[serde(default)]
    pub output: Option<String>,
    /// 损坏比例 0~1
    #[serde(default)]
    pub ratio: Option<f64>,
    /// 精确损坏字节数（优先于 ratio）
    #[serde(default)]
    pub bytes: Option<u64>,
    /// 损坏起始偏移
    #[serde(default)]
    pub start: Option<u64>,
    /// 损坏结束偏移（不含）
    #[serde(default)]
    pub end: Option<u64>,
    /// 随机种子
    #[serde(default)]
    pub seed: Option<u64>,
    /// 损坏方式
    #[serde(default)]
    pub mode: CorruptMode,
    /// 是否保持文件大小不变
    #[serde(default = "default_true")]
    pub keep_size: bool,
}

fn default_true() -> bool {
    true
}

/// 损坏结果（返回给前端用于展示与生成报告）
#[derive(Debug, Clone, Serialize, Default)]
pub struct CorruptResult {
    /// 实际写出的文件路径
    pub output_path: String,
    /// 原文件大小
    pub original_size: u64,
    /// 输出文件大小（应始终等于 original_size）
    pub output_size: u64,
    /// 实际被修改的字节数
    pub corrupted_bytes: u64,
    /// 实际损坏区间起点
    pub range_start: u64,
    /// 实际损坏区间终点
    pub range_end: u64,
    /// 使用的随机种子（前端可据此复现）
    pub seed: u64,
    /// 损坏方式
    pub mode: String,
    /// 人为损坏百分比（corrupted_bytes / 可损坏区间长度）
    pub percent: f64,
    /// 损坏连续块数量（相邻字节合并为一块）
    pub damaged_blocks: u64,
    /// 过程说明
    pub message: String,
}

/// 受保护的系统目录前缀（大小写不敏感）。
///
/// 为什么需要：本工具会真的改坏文件内容。如果用户误选了系统目录，
/// 可能把 Windows 自身文件弄坏导致无法开机。这里宁可拒绝，
/// 也不提供"反正有备份"的选项 —— 因为用户可能忘了自己开过覆盖。
const PROTECTED_PREFIXES: &[&str] = &[
    "c:\\windows",
    "c:\\program files",
    "c:\\program files (x86)",
    "c:\\programdata\\microsoft",
];

fn is_protected_path(path: &Path) -> bool {
    let s = path.to_string_lossy().to_ascii_lowercase();
    PROTECTED_PREFIXES.iter().any(|p| s.starts_with(p))
}

/// 默认输出路径：同目录下 `<stem>_corrupted<suffix>`。
///
/// 放同目录而不是临时目录，是因为用户通常要把损坏样本和原文件对照着用；
/// 若目标已存在则追加序号，绝不覆盖已有文件（避免误删上一次的结果）。
fn default_output_path(src: &Path) -> PathBuf {
    let parent = src.parent().unwrap_or_else(|| Path::new(""));
    let stem = src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
    let suffix = src.extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default();
    let mut candidate = parent.join(format!("{stem}_corrupted{suffix}"));
    let mut n = 2;
    while candidate.exists() {
        candidate = parent.join(format!("{stem}_corrupted_{n}{suffix}"));
        n += 1;
        if n > 9999 {
            break;
        }
    }
    candidate
}

/// 主入口：按请求生成损坏副本。
pub fn corrupt_file(req: &CorruptRequest) -> Result<CorruptResult, String> {
    let src = PathBuf::from(req.path.trim());
    if req.path.trim().is_empty() {
        return Err("未指定源文件".into());
    }
    if !src.is_file() {
        return Err(format!("源文件不存在或不是普通文件: {}", src.display()));
    }
    if is_protected_path(&src) {
        return Err("拒绝操作系统关键目录下的文件（可能损坏系统，导致无法开机）".into());
    }

    let meta = std::fs::metadata(&src).map_err(|e| format!("读取文件信息失败: {e}"))?;
    let size = meta.len();
    if size == 0 {
        return Err("文件为空（0 字节），没有可损坏的数据".into());
    }

    // 读出全部内容到内存。这里不用流式处理，原因：
    // 目标场景多为测试样本（MB 级以内），全量读入后可以精确控制"保留大小不变"，
    // 且避免边读边写导致的偏移计算错位。超大文件由前端提示分批处理。
    let mut data = std::fs::read(&src).map_err(|e| format!("读取源文件失败: {e}"))?;

    // ── 计算可损坏区间 ────────────────────────────────────────────
    let start = req.start.unwrap_or(0).min(size);
    let end = req.end.unwrap_or(size).min(size);
    if end <= start {
        return Err(format!(
            "损坏范围无效：起始 {start} 必须小于结束 {end}"
        ));
    }
    let span = end - start;

    // ── 计算要损坏的字节数 ────────────────────────────────────────
    let target = if let Some(b) = req.bytes {
        b
    } else {
        let ratio = req.ratio.unwrap_or(0.05);
        if !(0.0..=1.0).contains(&ratio) {
            return Err("损坏比例必须在 0~1 之间".into());
        }
        ((span as f64) * ratio).round() as u64
    };
    if target == 0 {
        return Err("损坏字节数为 0，请提高比例或指定更大的字节数".into());
    }
    // 目标字节数不能超过可损坏区间
    let count = target.min(span) as usize;

    // ── 选随机位置 ────────────────────────────────────────────────
    // 用显式 seed 的 StdRng：只要种子相同，同一版本内结果完全可复现
    // （不引入额外依赖，避免离线构建时缺 crate）。
    let seed = req.seed.unwrap_or_else(|| rand::thread_rng().gen::<u64>());
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);

    // 必须是「随机不重复位置」而非线性区间：真实损坏是离散的，
    // 整段写坏会变成另一类故障（大块丢失），不利于测试局部位错误的修复能力。
    let positions = sample_positions(&mut rng, span as usize, count);

    // 保留原始内容副本：用于精确统计「实际改变了多少字节」。
    // 置零/置全1 模式下，若目标字节本来就是目标值（文件中已有大量 0x00），
    // 写入前后内容相同 —— 等于没真正损坏。这样用户请求 200 字节时
    // 实际可能只改变了 199 个，会让"可控损坏比例"失真（实测复现过）。
    let original = data.clone();

    for off in &positions {
        let idx = start as usize + *off;
        match req.mode {
            CorruptMode::Random => data[idx] = rng.gen::<u8>(),
            CorruptMode::Zero => data[idx] = 0x00,
            CorruptMode::Ones => data[idx] = 0xFF,
        }
    }

    // 补足：把"写入后内容没变"的位置顺延到区间内其它未使用位置，
    // 直到实际改变数达到请求值（区间不够时尽力而为，并如实报告）。
    let mut actually_changed = 0u64;
    for off in &positions {
        let idx = start as usize + *off;
        if data[idx] != original[idx] {
            actually_changed += 1;
        }
    }
    if actually_changed < count as u64 {
        let mut used: std::collections::HashSet<usize> = positions.iter().copied().collect();
        let mut cursor = 0usize;
        let mut guard = 0usize;
        while actually_changed < count as u64 && guard < span as usize {
            guard += 1;
            while cursor < span as usize && used.contains(&cursor) {
                cursor += 1;
            }
            if cursor >= span as usize {
                break;
            }
            let idx = start as usize + cursor;
            let before = data[idx];
            match req.mode {
                CorruptMode::Random => data[idx] = rng.gen::<u8>(),
                CorruptMode::Zero => data[idx] = 0x00,
                CorruptMode::Ones => data[idx] = 0xFF,
            }
            used.insert(cursor);
            if data[idx] != before {
                actually_changed += 1;
            }
            cursor += 1;
        }
    }
    // 实际改变的字节数：这是对外报告与统计块数的唯一依据
    // （置零/全1 模式下可能少于请求值，见上面的补足逻辑）
    let corrupted = actually_changed;

    // 统计连续块数：把排序后的位置聚成相邻段，反映"碎裂程度"
    let mut blocks = 0u64;
    let mut prev: Option<usize> = None;
    for off in &positions {
        match prev {
            Some(p) if p + 1 == *off => {}
            _ => blocks += 1,
        }
        prev = Some(*off);
    }

    // ── 写出 ────────────────────────────────────────────────────
    let out = match req.output.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => {
            let p = PathBuf::from(p);
            if p == src {
                return Err("输出路径与源文件相同，会覆盖原文件，已拒绝".into());
            }
            if is_protected_path(&p) {
                return Err("拒绝写入系统关键目录".into());
            }
            p
        }
        None => default_output_path(&src),
    };
    if let Some(parent) = out.parent() {
        if !parent.exists() {
            return Err(format!("输出目录不存在: {}", parent.display()));
        }
    }

    // keep_size 场景下 data 长度本就等于 size（只替换不增删），
    // 这里保留开关是为了将来扩展；当前实现永远保持等长，因此额外断言一次。
    if req.keep_size && data.len() as u64 != size {
        return Err("内部错误：损坏后大小发生变化（不应发生）".into());
    }

    std::fs::write(&out, &data).map_err(|e| format!("写入输出文件失败: {e}"))?;

    // 复核：确保写出的大小与原文件一致（长度变化会被下游工具误判为截断）
    let out_size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    if out_size != size {
        return Err(format!(
            "输出大小异常：期望 {size} 字节，实际 {out_size} 字节"
        ));
    }

    let percent = if span > 0 {
        (corrupted as f64 / span as f64) * 100.0
    } else {
        0.0
    };

    Ok(CorruptResult {
        output_path: out.to_string_lossy().into_owned(),
        original_size: size,
        output_size: out_size,
        corrupted_bytes: corrupted,
        range_start: start,
        range_end: end,
        seed,
        mode: match req.mode {
            CorruptMode::Random => "random".into(),
            CorruptMode::Zero => "zero".into(),
            CorruptMode::Ones => "ones".into(),
        },
        percent,
        damaged_blocks: blocks,
        message: format!(
            "已就地替换 {} 字节（占损坏区间 {:.2}%），共 {} 个离散块，文件大小保持不变",
            corrupted, percent, blocks
        ),
    })
}

/// 从 `0..len` 中随机抽取 `count` 个**不重复**位置，返回升序结果。
///
/// 为什么不用「逐个 gen_range 直到凑够」：当 count 接近 len 时会退化成
/// 接近无限循环（重复命中率高）。这里按 count 与 len 的关系选策略：
/// - 稀疏（count 远小于 len）：随机采样 + 去重，命中率高
/// - 稠密（count 超过一半）：生成全序列后部分洗牌，避免大量重复
fn sample_positions(rng: &mut impl Rng, len: usize, count: usize) -> Vec<usize> {
    let count = count.min(len);
    let mut out = Vec::with_capacity(count);

    if count * 2 <= len {
        let mut seen = std::collections::HashSet::with_capacity(count * 2);
        while out.len() < count {
            let p = rng.gen_range(0..len);
            if seen.insert(p) {
                out.push(p);
            }
        }
        out.sort_unstable();
    } else {
        // 稠密：构造 0..len 后做部分 Fisher-Yates，取前 count 个
        let mut all: Vec<usize> = (0..len).collect();
        for i in 0..count {
            let j = rng.gen_range(i..len);
            all.swap(i, j);
        }
        out.extend_from_slice(&all[..count]);
        out.sort_unstable();
    }
    out
}

/// Tauri 命令：按参数生成损坏副本，返回统计信息。
#[tauri::command]
pub async fn corrupt_file_random(req: CorruptRequest) -> Result<CorruptResult, String> {
    // 大文件读写是阻塞 IO，必须离开主线程，否则界面在损坏期间会完全无响应
    tokio::task::spawn_blocking(move || corrupt_file(&req))
        .await
        .map_err(|e| format!("损坏任务异常: {e}"))?
}

/// Tauri 命令：预估损坏规模（不写文件，供前端在参数变化时实时显示）。
#[tauri::command]
pub fn corrupt_estimate(path: String, ratio: Option<f64>, bytes: Option<u64>, start: Option<u64>, end: Option<u64>) -> Result<CorruptEstimate, String> {
    let src = PathBuf::from(path.trim());
    if !src.is_file() {
        return Err("源文件不存在或不是普通文件".into());
    }
    let size = std::fs::metadata(&src).map_err(|e| e.to_string())?.len();
    let s = start.unwrap_or(0).min(size);
    let e = end.unwrap_or(size).min(size);
    let span = e.saturating_sub(s);
    let target = if let Some(b) = bytes {
        b
    } else {
        ((span as f64) * ratio.unwrap_or(0.05)).round() as u64
    };
    Ok(CorruptEstimate {
        file_size: size,
        range_start: s,
        range_end: e,
        span,
        corrupt_bytes: target.min(span),
        percent: if span > 0 {
            (target.min(span) as f64 / span as f64) * 100.0
        } else {
            0.0
        },
    })
}

/// 预估结果
#[derive(Debug, Clone, Serialize, Default)]
pub struct CorruptEstimate {
    pub file_size: u64,
    pub range_start: u64,
    pub range_end: u64,
    pub span: u64,
    pub corrupt_bytes: u64,
    pub percent: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir() -> PathBuf {
        let d = std::env::temp_dir().join("neobox_corrupt_test");
        let _ = std::fs::create_dir_all(&d);
        d
    }

    #[test]
    fn keeps_size_and_only_touches_requested_bytes() {
        let dir = tmp_dir();
        let src = dir.join("sample.bin");
        let content: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&src, &content).unwrap();

        let req = CorruptRequest {
            path: src.to_string_lossy().into_owned(),
            output: None,
            ratio: None,
            bytes: Some(50),
            start: Some(100),
            end: Some(500),
            seed: Some(42),
            mode: CorruptMode::Zero,
            keep_size: true,
        };
        let r = corrupt_file(&req).unwrap();

        // 大小不变
        assert_eq!(r.output_size, 1000);
        assert_eq!(r.corrupted_bytes, 50);
        assert!(r.range_start >= 100 && r.range_end <= 500);

        // 对比字节：区间外的字节必须完全一致
        let out = std::fs::read(&r.output_path).unwrap();
        assert_eq!(out.len(), content.len());
        for i in 0..content.len() {
            if i < 100 || i >= 500 {
                assert_eq!(out[i], content[i], "区间外字节被改动: index={i}");
            }
        }
    }

    #[test]
    fn same_seed_is_reproducible() {
        let dir = tmp_dir();
        let src = dir.join("seed.bin");
        let content: Vec<u8> = (0..500u32).map(|i| (i % 256) as u8).collect();
        std::fs::write(&src, &content).unwrap();

        let mut outputs = Vec::new();
        for n in 0..2 {
            let req = CorruptRequest {
                path: src.to_string_lossy().into_owned(),
                output: Some(dir.join(format!("seed_out_{n}.bin")).to_string_lossy().into_owned()),
                ratio: Some(0.1),
                bytes: None,
                start: None,
                end: None,
                seed: Some(7),
                mode: CorruptMode::Random,
                keep_size: true,
            };
            let r = corrupt_file(&req).unwrap();
            outputs.push(std::fs::read(r.output_path).unwrap());
        }
        assert_eq!(outputs[0], outputs[1], "同种子结果不一致，无法复现");
    }

    #[test]
    fn refuses_protected_dirs() {
        let req = CorruptRequest {
            path: r"C:\Windows\System32\drivers\etc\hosts".into(),
            output: None,
            ratio: Some(0.1),
            bytes: None,
            start: None,
            end: None,
            seed: None,
            mode: CorruptMode::Random,
            keep_size: true,
        };
        assert!(corrupt_file(&req).is_err());
    }

    #[test]
    fn refuses_output_equals_input() {
        let dir = tmp_dir();
        let src = dir.join("same.bin");
        std::fs::write(&src, b"hello world").unwrap();
        let p = src.to_string_lossy().into_owned();
        let req = CorruptRequest {
            path: p.clone(),
            output: Some(p.clone()),
            ratio: Some(0.5),
            bytes: None,
            start: None,
            end: None,
            seed: Some(1),
            mode: CorruptMode::Zero,
            keep_size: true,
        };
        assert!(corrupt_file(&req).is_err(), "输出等于源时必须拒绝");
        // 源文件未被改动
        assert_eq!(std::fs::read(&src).unwrap(), b"hello world");
    }
}
