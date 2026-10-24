// 诊断用：命令行复现音视频倒放/变速/自写 MP4 muxer，打印每阶段耗时
// 用法: media_debug <reverse|speed|enc|mux> <input> <output> [speed]
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: media_debug <reverse|speed|enc|mux> <input> <output> [speed]");
        std::process::exit(1);
    }
    let mode = args[1].clone();
    let input = args[2].clone();
    let output = args.get(3).cloned().unwrap_or_default();
    let speed: f64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(3.0);

    if mode == "enc" {
        enc_probe(&input);
        return;
    }
    if mode == "mux" {
        mux_probe(&input, &output);
        return;
    }
    if mode == "probe" {
        probe_file(&input);
        return;
    }
    if mode == "decode" {
        let raw = args.iter().any(|a| a == "raw");
        decode_probe(&input, raw);
        return;
    }
    if mode == "dump" {
        dump_probe(&input);
        return;
    }
    if mode == "dec1" {
        decode_sample_probe(&input);
        return;
    }
    if mode == "aenc" {
        enum_audio_encoders();
        return;
    }
    if mode == "mkref" {
        make_ref_aac(&input, &output);
        return;
    }

    let t0 = Instant::now();
    let r = tauri::async_runtime::block_on(async move {
        match mode.as_str() {
            "reverse" => {
                neobox_lib::file_convert::reverse_media(input, output).await.map(|_| None)
            }
            "speed" => {
                neobox_lib::file_convert::change_media_speed(input, output, speed).await
            }
            _ => Err("unknown mode".to_string()),
        }
    });
    let ok = r.is_ok();
    match &r {
        Ok(w) => eprintln!("[media-debug] RESULT OK in {:?}, warning={:?}", t0.elapsed(), w),
        Err(e) => eprintln!("[media-debug] RESULT ERR after {:?}: {}", t0.elapsed(), e),
    }
    std::process::exit(if ok { 0 } else { 1 });
}

/// 直接实例化 H.264 编码器（IMFTransform）喂一帧，验证编码器组件本身是否正常
fn enc_probe(input: &str) {
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::Media::MediaFoundation::{
        MFCreateSourceReaderFromURL, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
        MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READERF_ENDOFSTREAM,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED};
    use windows::core::Interface;
    use windows::Win32::Media::MediaFoundation::IMFTransform;

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET);
        let t0 = Instant::now();

        // 1. 打开源，读第一帧 NV12
        let wide: Vec<u16> = input.encode_utf16().chain(std::iter::once(0)).collect();
        let reader = MFCreateSourceReaderFromURL(windows::core::PCWSTR(wide.as_ptr()), None)
            .expect("open reader");
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .expect("deselect all");
        reader
            .SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, true)
            .expect("select video");
        let mut req = MFCreateMediaType().expect("create type");
        req.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).expect("set major");
        req.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).expect("set sub");
        reader
            .SetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, None, &req)
            .expect("set nv12");
        let cur = reader
            .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
            .expect("get type");
        let fs = cur.GetUINT64(&MF_MT_FRAME_SIZE).expect("frame size");
        let fr = cur.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(0);
        let vw = (fs >> 32) as u32;
        let vh = (fs & 0xFFFF_FFFF) as u32;
        let fnum = (fr >> 32) as u32;
        let fden = (fr & 0xFFFF_FFFF) as u32;
        eprintln!("[enc] source {vw}x{vh} fps={fnum}/{fden}");
        let mut flags: u32 = 0;
        let mut ts: i64 = 0;
        let mut sample: Option<IMFSample> = None;
        reader
            .ReadSample(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                0,
                None,
                Some(&mut flags),
                Some(&mut ts),
                Some(&mut sample),
            )
            .expect("read sample");
        let sample = sample.expect("sample");
        let buf = sample.ConvertToContiguousBuffer().expect("contig");
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut maxlen: u32 = 0;
        let mut curlen: u32 = 0;
        buf.Lock(&mut ptr, Some(&mut maxlen), Some(&mut curlen)).expect("lock");
        eprintln!("[enc] first frame size={curlen} ts={ts}");
        let _ = buf.Unlock();
        drop(buf);
        eprintln!("[enc] opened+decoded first frame in {:?}", t0.elapsed());

        // 2. 直接实例化 Microsoft H.264 Encoder MFT
        let t1 = Instant::now();
        let encoder: IMFTransform = CoCreateInstance(&CMSH264EncoderMFT, None, CLSCTX_ALL)
            .expect("create encoder");
        eprintln!("[enc] encoder created in {:?}", t1.elapsed());

        // 输入类型 NV12（先设输出类型再设输入类型）
        let mut in_type = MFCreateMediaType().expect("in type");
        in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).expect("in major");
        in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).expect("in sub");
        in_type.SetUINT64(&MF_MT_FRAME_SIZE, fs).expect("in size");
        in_type.SetUINT64(&MF_MT_FRAME_RATE, fr).expect("in rate");
        in_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, 1920).expect("in stride");
        in_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .expect("in interlace");

        // 输出类型：用 GetOutputAvailableType 枚举，选 H.264 那个（自带完整属性）
        let mut chosen: Option<windows::Win32::Media::MediaFoundation::IMFMediaType> = None;
        for idx in 0..8u32 {
            match encoder.GetOutputAvailableType(0, idx) {
                Ok(t) => {
                    let sub = t.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
                    eprintln!("[enc] avail out type[{idx}] subtype={sub:?}");
                    if sub == MFVideoFormat_H264 {
                        chosen = Some(t);
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let mut out_type = chosen.expect("no H264 output type");
        // 补齐关键属性（编码器要求完整类型才能 SetOutputType）
        out_type.SetUINT64(&MF_MT_FRAME_SIZE, fs).expect("out size");
        out_type.SetUINT64(&MF_MT_FRAME_RATE, fr).expect("out rate");
        out_type.SetUINT32(&MF_MT_AVG_BITRATE, 8_000_000).expect("out bitrate");
        let _ = out_type.SetUINT32(&MF_MT_MPEG2_PROFILE, 66u32); // Baseline
        let _ = out_type.SetUINT32(&MF_MT_MPEG2_LEVEL, 40u32); // 4.0
        let _ = out_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32);
        let r = encoder.SetOutputType(0, &out_type, 0);
        eprintln!("[enc] SetOutputType(available+filled) => {r:?} ({:?})", t1.elapsed());
        if r.is_err() {
            std::process::exit(1);
        }

        let r = encoder.SetInputType(0, &in_type, 0);
        eprintln!("[enc] SetInputType => {r:?} ({:?})", t1.elapsed());
        if r.is_err() {
            std::process::exit(1);
        }

        encoder
            .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
            .expect("begin streaming");
        encoder
            .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
            .expect("start of stream");

        // 3. 循环：ProcessInput 帧 → ProcessOutput 收集（同步 MFT 驱动循环）
        let t2 = Instant::now();
        let mut total_in = 0usize;
        let mut total_out = 0usize;
        let mut total_out_bytes = 0u64;
        let mut need_drain = false;
        'outer: for _ in 0..64 {
            // 取下一帧（顺序读源视频流）
            let mut flags: u32 = 0;
            let mut ts2: i64 = 0;
            let mut nsample: Option<IMFSample> = None;
            let rr = reader
                .ReadSample(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut ts2),
                    Some(&mut nsample),
                );
            if rr.is_err() || flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 || nsample.is_none() {
                need_drain = true;
                break;
            }
            let ins = nsample.unwrap();
            // 紧凑化：源帧可能含 stride padding（3133440B = 1920x1088x1.5）
            // 编码器类型是 1920x1080，需要紧凑 NV12 = 3110400B
            let compacted: Option<IMFSample> = {
                let b = ins.ConvertToContiguousBuffer().expect("contig");
                let mut p: *mut u8 = std::ptr::null_mut();
                let mut m: u32 = 0;
                let mut c: u32 = 0;
                b.Lock(&mut p, Some(&mut m), Some(&mut c)).expect("lock");
                let need = (vw * vh * 3 / 2) as u32;
                let res = if c != need {
                    // 逐行拷贝前 vh 行（Y: vh*vw, UV: vh*vw/2）
                    let src = std::slice::from_raw_parts(p, c as usize);
                    let mut compact = vec![0u8; need as usize];
                    let row = vw as usize;
                    for y in 0..vh as usize {
                        compact[y * row..(y + 1) * row].copy_from_slice(&src[y * row..(y + 1) * row]);
                    }
                    let uv_src_off = c as usize - vh as usize * row; // 从尾部对齐 UV 区
                    let uv_start = (c as usize) - (vh as usize / 2) * row;
                    let uv_dst = (need as usize) - (vh as usize / 2) * row;
                    compact[uv_dst..].copy_from_slice(&src[uv_start..]);
                    drop(src);
                    let buf = windows::Win32::Media::MediaFoundation::MFCreateMemoryBuffer(need)
                        .expect("mem buf");
                    let mut pp: *mut u8 = std::ptr::null_mut();
                    let mut mm: u32 = 0;
                    let mut cc: u32 = 0;
                    buf.Lock(&mut pp, Some(&mut mm), Some(&mut cc)).expect("lock2");
                    std::ptr::copy_nonoverlapping(compact.as_ptr(), pp, need as usize);
                    let _ = buf.Unlock();
                    let s = windows::Win32::Media::MediaFoundation::MFCreateSample()
                        .expect("sample");
                    s.AddBuffer(&buf).expect("addbuf");
                    let _ = s.SetSampleTime(ts2);
                    let _ = s.SetSampleDuration(333333);
                    Some(s)
                } else {
                    let s = windows::Win32::Media::MediaFoundation::MFCreateSample()
                        .expect("sample");
                    s.AddBuffer(&b).expect("addbuf");
                    let _ = s.SetSampleTime(ts2);
                    let _ = s.SetSampleDuration(333333);
                    Some(s)
                };
                let _ = b.Unlock();
                res
            };
            let ins = compacted.unwrap();
            let r = encoder.ProcessInput(0, &ins, 0);
            if r.is_err() {
                eprintln!("[enc] ProcessInput err => {r:?} at in={total_in}");
                break;
            }
            total_in += 1;
            // 有输出才取（同步 MFT 用 GetOutputStatus 判断）
            loop {
                let st = encoder.GetOutputStatus().unwrap_or(0);
                if st == 0 {
                    break; // 无输出，继续喂帧
                }
                let mut out_status: u32 = 0;
                // 预分配 8MB 输出缓冲
                let obuf = MFCreateMemoryBuffer(8_000_000).expect("out mem buf");
                let mut op: *mut u8 = std::ptr::null_mut();
                let mut om: u32 = 0;
                let mut oc: u32 = 0;
                obuf.Lock(&mut op, Some(&mut om), Some(&mut oc)).expect("out lock");
                let _ = obuf.Unlock();
                let out_sample = MFCreateSample().expect("out sample");
                out_sample.AddBuffer(&obuf).expect("add out buf");
                let events = MFCreateCollection().expect("collection");
                let mut data = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: std::mem::ManuallyDrop::new(Some(out_sample)),
                    dwStatus: 0,
                    pEvents: std::mem::ManuallyDrop::new(Some(events)),
                };
                let mut buffers = [data];
                let r = encoder.ProcessOutput(0, &mut buffers, &mut out_status);
                match &r {
                    Ok(()) => {
                        if let Some(s) = buffers[0].pSample.take() {
                            let b = s.ConvertToContiguousBuffer().expect("out contig");
                            let mut p2: *mut u8 = std::ptr::null_mut();
                            let mut m2: u32 = 0;
                            let mut c2: u32 = 0;
                            b.Lock(&mut p2, Some(&mut m2), Some(&mut c2)).expect("out lock");
                            total_out_bytes += c2 as u64;
                            let _ = b.Unlock();
                            total_out += 1;
                        }
                    }
                    Err(e) => {
                        if e.code() == windows::core::HRESULT(0xC00D6D72u32 as i32) {
                            // MF_E_TRANSFORM_NEED_MORE_INPUT
                            break;
                        }
                        eprintln!("[enc] ProcessOutput err => {e:?} at in={total_in}");
                        break 'outer;
                    }
                }
            }
        }
        eprintln!(
            "[enc] loop: in={total_in} out={total_out} bytes={total_out_bytes} in {:?}",
            t2.elapsed()
        );

        // 4. Drain：输入完毕，请求剩余输出
        if need_drain {
            let _ = encoder.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
            loop {
                let mut out_status: u32 = 0;
                let mut out_sample = MFCreateSample().expect("out sample");
                let mut data = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: std::mem::ManuallyDrop::new(Some(out_sample)),
                    dwStatus: 0,
                    pEvents: std::mem::ManuallyDrop::new(None),
                };
                let mut buffers = [data];
                let r = encoder.ProcessOutput(0, &mut buffers, &mut out_status);
                match &r {
                    Ok(()) => {
                        if let Some(s) = buffers[0].pSample.take() {
                            let b = s.ConvertToContiguousBuffer().expect("out contig");
                            let mut p2: *mut u8 = std::ptr::null_mut();
                            let mut m2: u32 = 0;
                            let mut c2: u32 = 0;
                            b.Lock(&mut p2, Some(&mut m2), Some(&mut c2)).expect("out lock");
                            total_out_bytes += c2 as u64;
                            let _ = b.Unlock();
                            total_out += 1;
                        }
                    }
                    Err(e) => {
                        if e.code() == windows::core::HRESULT(0xC00D6D72u32 as i32) {
                            break; // drain 完成
                        }
                        eprintln!("[enc] drain ProcessOutput err => {e:?}");
                        break;
                    }
                }
            }
        }
        eprintln!(
            "[enc] FINAL: in={total_in} out={total_out} bytes={total_out_bytes} in {:?}",
            t2.elapsed()
        );

        let _ = MFShutdown();
        CoUninitialize();
        eprintln!("[enc] DONE total {:?}", t0.elapsed());
    }
}

/// Annex B（起始码）→ AVCC（4 字节大端长度前缀）；跳过 AUD（nal_type 9）
fn annexb_to_avcc(data: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        let mut nal_start = i;
        if data[i..].starts_with(&[0u8, 0, 0, 1]) {
            nal_start = i + 4;
        } else if data[i..].starts_with(&[0u8, 0, 1]) {
            nal_start = i + 3;
        }
        let mut j = nal_start;
        let mut next = data.len();
        while j + 3 < data.len() {
            if data[j] == 0
                && data[j + 1] == 0
                && (data[j + 2] == 1 || (data[j + 2] == 0 && data[j + 3] == 1))
            {
                next = j;
                break;
            }
            j += 1;
        }
        let nal = &data[nal_start..next];
        if !nal.is_empty() {
            let nal_type = nal[0] & 0x1F;
            if nal_type != 9 {
                out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                out.extend_from_slice(nal);
            }
        }
        i = next;
        if next >= data.len() {
            break;
        }
    }
    out
}

/// 从 Annex B 流提取 SPS（67）/ PPS（68）
fn extract_sps_pps(data: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut sps: Vec<u8> = Vec::new();
    let mut pps: Vec<u8> = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        let mut nal_start = i;
        if data[i..].starts_with(&[0u8, 0, 0, 1]) {
            nal_start = i + 4;
        } else if data[i..].starts_with(&[0u8, 0, 1]) {
            nal_start = i + 3;
        }
        let mut j = nal_start;
        let mut next = data.len();
        while j + 3 < data.len() {
            if data[j] == 0
                && data[j + 1] == 0
                && (data[j + 2] == 1 || (data[j + 2] == 0 && data[j + 3] == 1))
            {
                next = j;
                break;
            }
            j += 1;
        }
        let nal = &data[nal_start..next];
        if !nal.is_empty() {
            let t = nal[0] & 0x1F;
            if t == 7 {
                sps = nal.to_vec();
            } else if t == 8 {
                pps = nal.to_vec();
            }
        }
        i = next;
        if next >= data.len() {
            break;
        }
    }
    (sps, pps)
}

/// AudioSpecificConfig（AAC LC）—— 保留备用
fn asc_for(sr: u32, ch: u32) -> u16 {
    let idx = match sr {
        96000 => 0,
        88200 => 1,
        64000 => 2,
        48000 => 3,
        44100 => 4,
        32000 => 5,
        24000 => 6,
        22050 => 7,
        16000 => 8,
        12000 => 9,
        11025 => 10,
        8000 => 11,
        _ => 4,
    };
    let cc = match ch {
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        5 => 5,
        6 => 6,
        7 => 7,
        8 => 8,
        _ => 2,
    };
    ((2u16 << 11) | ((idx as u16) << 7) | ((cc as u16) << 3))
}

/// dac3 box 载荷（3 字节）：fscod(2) bsid(5) bsmod(3) acmod(3) lfeon(1) bit_rate_code(5) reserved(5)
fn dac3_cfg(sr: u32, ch: u32) -> Vec<u8> {
    let fscod = match sr {
        48000 => 0u8,
        44100 => 1,
        32000 => 2,
        _ => 0,
    };
    let bsid = 8u8;
    let bsmod = 0u8;
    let acmod = match ch {
        1 => 1u8, // mono
        3 => 3,   // LRC
        4 => 4,   // LCRS
        5 => 5,   // 5.0
        6 => 7,   // 5.1
        _ => 2,   // stereo
    };
    let lfeon = if ch == 6 { 1u8 } else { 0 };
    let brc = 15u8; // 384 kbps
    let b0 = (fscod << 6) | (bsid << 1) | (bsmod >> 2);
    let b1 = ((bsmod & 3) << 6) | (acmod << 3) | (lfeon << 2) | (brc >> 3);
    let b2 = ((brc & 7) << 5) | 0;
    vec![b0, b1, b2]
}

fn be32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn be64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn fourcc(out: &mut Vec<u8>, s: &[u8; 4]) {
    out.extend_from_slice(s);
}
/// MP4 box：先写 size 占位 + type，返回回填位置
fn box_start(out: &mut Vec<u8>, t: &[u8; 4]) -> usize {
    let pos = out.len();
    be32(out, 0);
    fourcc(out, t);
    pos
}
fn box_end(out: &mut Vec<u8>, pos: usize) {
    let size = (out.len() - pos) as u32;
    out[pos..pos + 4].copy_from_slice(&size.to_be_bytes());
}
/// ES descriptor 长度 varint
fn desc_len(out: &mut Vec<u8>, len: usize) {
    // 最多 4 字节 varint（7-bit groups, 大端, 高位续）
    let mut v = len;
    let mut tmp = [0u8; 4];
    let mut n = 0;
    tmp[n] = (v & 0x7F) as u8;
    v >>= 7;
    while v > 0 {
        n += 1;
        tmp[n] = ((v & 0x7F) as u8) | 0x80;
        v >>= 7;
    }
    for k in (0..=n).rev() {
        out.push(tmp[k]);
    }
}

/// 自写 MP4 muxer：H.264（avc1）+ AAC（mp4a）
struct Mp4Muxer {
    file: std::fs::File,
    vw: u32,
    vh: u32,
    v_fps_num: u32,
    v_fps_den: u32,
    v_sps: Vec<u8>,
    v_pps: Vec<u8>,
    v_sample_sizes: Vec<u32>,
    v_sample_offsets: Vec<u64>,
    sr: u32,
    ch: u32,
    a_sample_sizes: Vec<u32>,
    a_sample_offsets: Vec<u64>,
    a_cfg: Vec<u8>,
    a_codec: &'static str,
    a_frame: u32,
    ftyp_len: usize,
    data_start: u64,
    file_pos: u64,
    has_audio: bool,
}

impl Mp4Muxer {
    fn new(
        path: &str,
        vw: u32,
        vh: u32,
        fnum: u32,
        fden: u32,
        sr: u32,
        ch: u32,
        has_audio: bool,
        a_cfg: Vec<u8>,
        a_codec: &'static str,
        a_frame: u32,
    ) -> Result<Self, String> {
        let mut file = std::fs::File::create(path).map_err(|e| format!("创建输出文件失败: {e}"))?;
        let mut m = Mp4Muxer {
            file,
            vw,
            vh,
            v_fps_num: fnum,
            v_fps_den: fden,
            v_sps: Vec::new(),
            v_pps: Vec::new(),
            v_sample_sizes: Vec::new(),
            v_sample_offsets: Vec::new(),
            sr,
            ch,
            a_sample_sizes: Vec::new(),
            a_sample_offsets: Vec::new(),
            a_cfg,
            a_codec,
            a_frame,
            ftyp_len: 0,
            data_start: 0,
            file_pos: 0,
            has_audio,
        };
        m.write_ftyp();
        m.write_mdat_header();
        Ok(m)
    }

    fn write_ftyp(&mut self) {
        let mut b = Vec::new();
        box_start(&mut b, b"ftyp");
        fourcc(&mut b, b"isom");
        be32(&mut b, 0);
        fourcc(&mut b, b"isom");
        fourcc(&mut b, b"iso2");
        fourcc(&mut b, b"avc1");
        fourcc(&mut b, b"mp41");
        box_end(&mut b, 0);
        use std::io::Write;
        let _ = self.file.write_all(&b);
        self.file_pos += b.len() as u64;
        self.ftyp_len = b.len();
    }

    fn write_mdat_header(&mut self) {
        let mut b = Vec::new();
        box_start(&mut b, b"mdat"); // size 占位，finish 时回填
        use std::io::Write;
        let _ = self.file.write_all(&b);
        self.file_pos += b.len() as u64;
        self.data_start = self.file_pos;
    }

    fn add_video(&mut self, annexb: &[u8]) -> Result<(), String> {
        use std::io::Write;
        if self.v_sps.is_empty() {
            let (s, p) = extract_sps_pps(annexb);
            self.v_sps = s;
            self.v_pps = p;
        }
        let avcc = annexb_to_avcc(annexb);
        let off = self.file_pos - self.data_start;
        self.v_sample_offsets.push(off);
        self.v_sample_sizes.push(avcc.len() as u32);
        self.file
            .write_all(&avcc)
            .map_err(|e| format!("写入视频样本失败: {e}"))?;
        self.file_pos += avcc.len() as u64;
        Ok(())
    }

    fn add_audio(&mut self, raw: &[u8]) -> Result<(), String> {
        use std::io::Write;
        let off = self.file_pos - self.data_start;
        self.a_sample_offsets.push(off);
        self.a_sample_sizes.push(raw.len() as u32);
        self.file
            .write_all(raw)
            .map_err(|e| format!("写入音频样本失败: {e}"))?;
        self.file_pos += raw.len() as u64;
        Ok(())
    }

    fn finish(mut self) -> Result<(), String> {
        use std::io::{Seek, Write};
        // 1. 回填 mdat size
        let mdat_size = (self.file_pos - self.data_start) + 8;
        let mdat_size_pos = self.ftyp_len as u64; // mdat size 字段在 ftyp 之后
        self.file
            .seek(std::io::SeekFrom::Start(mdat_size_pos))
            .map_err(|e| format!("定位 mdat 失败: {e}"))?;
        self.file
            .write_all(&(mdat_size as u32).to_be_bytes())
            .map_err(|e| format!("回填 mdat 失败: {e}"))?;
        self.file
            .seek(std::io::SeekFrom::Start(self.file_pos))
            .map_err(|e| format!("定位末尾失败: {e}"))?;

        // 2. 构建并写 moov
        let moov = self.build_moov();
        self.file
            .write_all(&moov)
            .map_err(|e| format!("写 moov 失败: {e}"))?;
        self.file.flush().map_err(|e| format!("刷新文件失败: {e}"))?;
        Ok(())
    }

    fn build_moov(&self) -> Vec<u8> {
        let mut moov = Vec::new();
        let mpos = box_start(&mut moov, b"moov");

        // 时长（100ns 单位）
        let v_dur = if self.v_fps_num > 0 {
            (self.v_sample_sizes.len() as u64) * 10_000_000u64 * (self.v_fps_den as u64)
                / (self.v_fps_num as u64)
        } else {
            0
        };
        let a_total_samples = (self.a_sample_sizes.len() as u64) * (self.a_frame as u64);
        let a_dur = if self.sr > 0 {
            a_total_samples * 10_000_000u64 / (self.sr as u64)
        } else {
            0
        };
        let total_dur = v_dur.max(a_dur);

        // mvhd
        {
            let p = box_start(&mut moov, b"mvhd");
            be32(&mut moov, 0); // version+flags
            be32(&mut moov, 0); // creation
            be32(&mut moov, 0); // modification
            be32(&mut moov, 10_000_000); // timescale (100ns)
            be32(&mut moov, total_dur.min(0xFFFF_FFFF) as u32);
            be32(&mut moov, 0x0001_0000); // rate
            be16(&mut moov, 0x0100); // volume
            be16(&mut moov, 0); // reserved
            be32(&mut moov, 0);
            be32(&mut moov, 0);
            // matrix
            for &v in &[
                0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000,
            ] {
                be32(&mut moov, v);
            }
            for _ in 0..6 {
                be32(&mut moov, 0);
            }
            be32(&mut moov, if self.has_audio { 3 } else { 2 }); // next_track_id
            box_end(&mut moov, p);
        }

        // 视频 trak
        if !self.v_sample_sizes.is_empty() {
            self.build_video_trak(&mut moov, v_dur);
        }
        // 音频 trak
        if !self.a_sample_sizes.is_empty() {
            self.build_audio_trak(&mut moov, a_dur, a_total_samples);
        }

        box_end(&mut moov, mpos);
        moov
    }

    fn build_video_trak(&self, moov: &mut Vec<u8>, v_dur: u64) {
        let tpos = box_start(moov, b"trak");
        // tkhd
        {
            let p = box_start(moov, b"tkhd");
            be32(moov, 0x0000_0003); // version0 + flags(3)
            be32(moov, 0);
            be32(moov, 0);
            be32(moov, 1); // track_id
            be32(moov, 0);
            be32(moov, v_dur.min(0xFFFF_FFFF) as u32);
            be32(moov, 0);
            be32(moov, 0);
            be16(moov, 0);
            be16(moov, 0);
            be16(moov, 0);
            be16(moov, 0);
            for &v in &[
                0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000,
            ] {
                be32(moov, v);
            }
            be32(moov, (self.vw << 16) | if self.vw > 0 { 0 } else { 0 });
            be32(moov, (self.vh << 16) | 0);
            box_end(moov, p);
        }
        // mdia
        {
            let p = box_start(moov, b"mdia");
            // mdhd
            let p2 = box_start(moov, b"mdhd");
            be32(moov, 0);
            be32(moov, 0);
            be32(moov, 0);
            be32(moov, 10_000_000);
            be32(moov, v_dur.min(0xFFFF_FFFF) as u32);
            be16(moov, 0x55C4);
            be16(moov, 0);
            box_end(moov, p2);
            // hdlr
            let p2 = box_start(moov, b"hdlr");
            be32(moov, 0);
            be32(moov, 0);
            fourcc(moov, b"vide");
            be32(moov, 0);
            be32(moov, 0);
            be32(moov, 0);
            moov.extend_from_slice(b"VideoHandler\0");
            box_end(moov, p2);
            // minf
            let p2 = box_start(moov, b"minf");
            // vmhd
            let p3 = box_start(moov, b"vmhd");
            be32(moov, 1);
            be16(moov, 0);
            be16(moov, 0);
            be16(moov, 0);
            be16(moov, 0);
            box_end(moov, p3);
            // dinf
            let p3 = box_start(moov, b"dinf");
            let p4 = box_start(moov, b"dref");
            be32(moov, 0);
            be32(moov, 1);
            let p5 = box_start(moov, b"url ");
            be32(moov, 1);
            box_end(moov, p5);
            box_end(moov, p4);
            box_end(moov, p3);
            // stbl
            let p3 = box_start(moov, b"stbl");
            // stsd
            let p4 = box_start(moov, b"stsd");
            be32(moov, 0);
            be32(moov, 1);
            let p5 = box_start(moov, b"avc1");
            be32(moov, 0); // reserved 6 + data_ref 2
            be32(moov, 1);
            be16(moov, 0);
            be16(moov, 0);
            be32(moov, 0);
            be32(moov, 0);
            be32(moov, 0);
            be16(moov, self.vw as u16);
            be16(moov, self.vh as u16);
            be32(moov, 0x0048_0000);
            be32(moov, 0x0048_0000);
            be32(moov, 0);
            be16(moov, 1);
            for _ in 0..32 {
                moov.push(0);
            }
            be16(moov, 0x18);
            be16(moov, 0xFFFF);
            // avcC
            let p6 = box_start(moov, b"avcC");
            moov.push(1);
            if self.v_sps.len() >= 4 {
                moov.push(self.v_sps[1]);
                moov.push(self.v_sps[2]);
                moov.push(self.v_sps[3]);
            } else {
                moov.push(66);
                moov.push(0);
                moov.push(40);
            }
            moov.push(0xFC | 0x03); // lengthSizeMinusOne=3
            moov.push(0xE0 | 1); // numSPS
            let sps_len = self.v_sps.len().min(0xFFFF) as u16;
            be16(moov, sps_len);
            moov.extend_from_slice(&self.v_sps);
            moov.push(1); // numPPS
            let pps_len = self.v_pps.len().min(0xFFFF) as u16;
            be16(moov, pps_len);
            moov.extend_from_slice(&self.v_pps);
            box_end(moov, p6);
            box_end(moov, p5);
            box_end(moov, p4);
            // stts
            let p4 = box_start(moov, b"stts");
            be32(moov, 0);
            be32(moov, 1);
            be32(moov, self.v_sample_sizes.len() as u32);
            let v_delta = if self.v_fps_num > 0 {
                (10_000_000u64 * (self.v_fps_den as u64) / (self.v_fps_num as u64)) as u32
            } else {
                333333
            };
            be32(moov, v_delta);
            box_end(moov, p4);
            // stsc
            let p4 = box_start(moov, b"stsc");
            be32(moov, 0);
            be32(moov, 1);
            be32(moov, 1);
            be32(moov, self.v_sample_sizes.len() as u32);
            be32(moov, 1);
            box_end(moov, p4);
            // stsz
            let p4 = box_start(moov, b"stsz");
            be32(moov, 0);
            be32(moov, 0);
            be32(moov, self.v_sample_sizes.len() as u32);
            for s in &self.v_sample_sizes {
                be32(moov, *s);
            }
            box_end(moov, p4);
            // co64
            let p4 = box_start(moov, b"co64");
            be32(moov, 0);
            be32(moov, self.v_sample_offsets.len() as u32);
            for o in &self.v_sample_offsets {
                be64(moov, *o);
            }
            box_end(moov, p4);
            box_end(moov, p3);
            box_end(moov, p2);
            box_end(moov, p);
        }
        box_end(moov, tpos);
    }

    fn build_audio_trak(&self, moov: &mut Vec<u8>, a_dur: u64, a_total_samples: u64) {
        let tpos = box_start(moov, b"trak");
        // tkhd
        {
            let p = box_start(moov, b"tkhd");
            be32(moov, 0x0000_0003);
            be32(moov, 0);
            be32(moov, 0);
            be32(moov, 2);
            be32(moov, 0);
            be32(moov, a_dur.min(0xFFFF_FFFF) as u32);
            be32(moov, 0);
            be32(moov, 0);
            be16(moov, 0);
            be16(moov, 0);
            be16(moov, 0);
            be16(moov, 0);
            for &v in &[
                0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000,
            ] {
                be32(moov, v);
            }
            be32(moov, 0);
            be32(moov, 0);
            box_end(moov, p);
        }
        // mdia
        {
            let p = box_start(moov, b"mdia");
            // mdhd
            let p2 = box_start(moov, b"mdhd");
            be32(moov, 0);
            be32(moov, 0);
            be32(moov, 0);
            be32(moov, self.sr);
            be32(moov, a_total_samples.min(0xFFFF_FFFF) as u32);
            be16(moov, 0x55C4);
            be16(moov, 0);
            box_end(moov, p2);
            // hdlr
            let p2 = box_start(moov, b"hdlr");
            be32(moov, 0);
            be32(moov, 0);
            fourcc(moov, b"soun");
            be32(moov, 0);
            be32(moov, 0);
            be32(moov, 0);
            moov.extend_from_slice(b"SoundHandler\0");
            box_end(moov, p2);
            // minf
            let p2 = box_start(moov, b"minf");
            // smhd
            let p3 = box_start(moov, b"smhd");
            be32(moov, 0);
            be16(moov, 0);
            be16(moov, 0);
            box_end(moov, p3);
            // dinf
            let p3 = box_start(moov, b"dinf");
            let p4 = box_start(moov, b"dref");
            be32(moov, 0);
            be32(moov, 1);
            let p5 = box_start(moov, b"url ");
            be32(moov, 1);
            box_end(moov, p5);
            box_end(moov, p4);
            box_end(moov, p3);
            // stbl
            let p3 = box_start(moov, b"stbl");
            // stsd
            let p4 = box_start(moov, b"stsd");
            be32(moov, 0);
            be32(moov, 1);
            let p5 = box_start(moov, b"ac-3");
            be32(moov, 0);
            be16(moov, 0);
            be16(moov, 1);
            be16(moov, 0);
            be16(moov, 0);
            be32(moov, 0);
            be16(moov, self.ch as u16);
            be16(moov, 16);
            be16(moov, 0);
            be16(moov, 0);
            be32(moov, self.sr << 16);
            // dac3 box（解码器配置，3 字节）
            let p6 = box_start(moov, b"dac3");
            if self.a_cfg.len() >= 3 {
                moov.extend_from_slice(&self.a_cfg[..3]);
            } else {
                moov.extend_from_slice(&[0x10u8, 0x11, 0x40]);
            }
            box_end(moov, p6);
            box_end(moov, p5);
            box_end(moov, p4);
            // stts
            let p4 = box_start(moov, b"stts");
            be32(moov, 0);
            be32(moov, 1);
            be32(moov, self.a_sample_sizes.len() as u32);
            be32(moov, self.a_frame);
            box_end(moov, p4);
            // stsc
            let p4 = box_start(moov, b"stsc");
            be32(moov, 0);
            be32(moov, 1);
            be32(moov, 1);
            be32(moov, self.a_sample_sizes.len() as u32);
            be32(moov, 1);
            box_end(moov, p4);
            // stsz
            let p4 = box_start(moov, b"stsz");
            be32(moov, 0);
            be32(moov, 0);
            be32(moov, self.a_sample_sizes.len() as u32);
            for s in &self.a_sample_sizes {
                be32(moov, *s);
            }
            box_end(moov, p4);
            // co64
            let p4 = box_start(moov, b"co64");
            be32(moov, 0);
            be32(moov, self.a_sample_offsets.len() as u32);
            for o in &self.a_sample_offsets {
                be64(moov, *o);
            }
            box_end(moov, p4);
            box_end(moov, p3);
            box_end(moov, p2);
            box_end(moov, p);
        }
        box_end(moov, tpos);
    }
}

fn be16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// 自写 MP4 muxer + IMFTransform 编码器（H.264 + AAC）
fn mux_probe(input: &str, output: &str) {
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::Media::MediaFoundation::{
        MFCreateSourceReaderFromURL, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
        MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READERF_ENDOFSTREAM,
        MF_SOURCE_READER_FIRST_AUDIO_STREAM,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED};
    use windows::Win32::Media::MediaFoundation::IMFTransform;

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET);
        let t0 = Instant::now();

        // 1. 打开视频源（NV12）
        let wide: Vec<u16> = input.encode_utf16().chain(std::iter::once(0)).collect();
        let reader = MFCreateSourceReaderFromURL(windows::core::PCWSTR(wide.as_ptr()), None)
            .expect("open reader");
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .expect("deselect all");
        reader
            .SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, true)
            .expect("select video");
        let mut req = MFCreateMediaType().expect("create type");
        req.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).expect("set major");
        req.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).expect("set sub");
        reader
            .SetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, None, &req)
            .expect("set nv12");
        let cur = reader
            .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
            .expect("get type");
        let fs = cur.GetUINT64(&MF_MT_FRAME_SIZE).expect("frame size");
        let fr = cur.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(0);
        let vw = (fs >> 32) as u32;
        let vh = (fs & 0xFFFF_FFFF) as u32;
        let fnum = (fr >> 32) as u32;
        let fden = (fr & 0xFFFF_FFFF) as u32;
        eprintln!("[mux] {vw}x{vh} fps={fnum}/{fden}");

        // 2. H.264 编码器
        let encoder: IMFTransform = CoCreateInstance(&CMSH264EncoderMFT, None, CLSCTX_ALL)
            .expect("create encoder");
        let mut in_type = MFCreateMediaType().expect("in type");
        in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).expect("in major");
        in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).expect("in sub");
        in_type.SetUINT64(&MF_MT_FRAME_SIZE, fs).expect("in size");
        in_type.SetUINT64(&MF_MT_FRAME_RATE, fr).expect("in rate");
        in_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, vw).expect("in stride");
        in_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .expect("in interlace");
        let mut chosen: Option<IMFMediaType> = None;
        for idx in 0..8u32 {
            match encoder.GetOutputAvailableType(0, idx) {
                Ok(t) => {
                    let sub = t.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
                    if sub == MFVideoFormat_H264 {
                        chosen = Some(t);
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let mut out_type = chosen.expect("no H264 output type");
        out_type.SetUINT64(&MF_MT_FRAME_SIZE, fs).expect("out size");
        out_type.SetUINT64(&MF_MT_FRAME_RATE, fr).expect("out rate");
        out_type.SetUINT32(&MF_MT_AVG_BITRATE, 8_000_000).expect("out bitrate");
        let _ = out_type.SetUINT32(&MF_MT_MPEG2_PROFILE, 66u32);
        let _ = out_type.SetUINT32(&MF_MT_MPEG2_LEVEL, 40u32);
        let _ = out_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32);
        encoder.SetOutputType(0, &out_type, 0).expect("set out type");
        encoder.SetInputType(0, &in_type, 0).expect("set in type");
        encoder
            .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
            .expect("begin streaming");
        encoder
            .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
            .expect("start of stream");

        // 3. 音频（若有）：独立 reader + AAC 编码器
        let mut sample_rate: u32 = 44100;
        let mut channels: u32 = 2;
        let mut has_audio = false;
        let mut audio_reader: Option<windows::Win32::Media::MediaFoundation::IMFSourceReader> = None;
        let mut aac: Option<IMFTransform> = None;
        let mut alac_cfg_out: Option<Vec<u8>> = None;
        {
            let mut stream_idx = 0u32;
            loop {
                match reader.GetNativeMediaType(stream_idx, 0) {
                    Ok(t) => {
                        if let Ok(major) = t.GetGUID(&MF_MT_MAJOR_TYPE) {
                            if major == MFMediaType_Audio {
                                has_audio = true;
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
                stream_idx += 1;
            }
        }
        if has_audio {
            let a_w: Vec<u16> = input.encode_utf16().chain(std::iter::once(0)).collect();
            let ar = MFCreateSourceReaderFromURL(windows::core::PCWSTR(a_w.as_ptr()), None)
                .expect("open audio reader");
            ar.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .expect("deselect audio");
            ar.SetStreamSelection(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, true)
                .expect("select audio");
            let pcm_req = MFCreateMediaType().expect("pcm req");
            pcm_req
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                .expect("pcm major");
            pcm_req
                .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
                .expect("pcm sub");
            ar.SetCurrentMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, None, &pcm_req)
                .expect("set pcm");
            let pcm_type = ar
                .GetCurrentMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32)
                .expect("get pcm type");
            sample_rate = pcm_type
                .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
                .unwrap_or(44100);
            channels = pcm_type
                .GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)
                .unwrap_or(2);
            eprintln!("[mux] audio sr={sample_rate} ch={channels}");

            // Dolby AC-3 编码器（本机无 AAC/MP3；AC3 编码器+解码器均有）
            let ac3_clsid = windows::core::GUID::from_u128(0xAC3315C9_F481_45D7_826C_0B406C1F64B8);
            let alac_enc: IMFTransform =
                CoCreateInstance(&ac3_clsid, None, CLSCTX_ALL).expect("create ac3 encoder");
            // 输出类型：取第一个可用类型（无需知道 subtype GUID），补齐 sr/ch
            let mut a_chosen: Option<IMFMediaType> = None;
            for idx in 0..8u32 {
                match alac_enc.GetOutputAvailableType(0, idx) {
                    Ok(t) => {
                        let sub = t.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
                        eprintln!("[mux] ac3 avail[{idx}] sub={sub:?}");
                        if a_chosen.is_none() {
                            a_chosen = Some(t);
                        }
                        if sub != windows::core::GUID::zeroed() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let mut a_out = a_chosen.expect("no AC3 output type");
            a_out
                .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, sample_rate)
                .expect("ac3 sr");
            a_out
                .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)
                .expect("ac3 ch");
            let _ = a_out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 48_000); // 384kbps
            let r = alac_enc.SetOutputType(0, &a_out, 0);
            eprintln!("[mux] ac3 SetOutputType => {r:?}");
            if r.is_err() {
                has_audio = false;
            } else {
                let r = alac_enc.SetInputType(0, &pcm_type, 0);
                eprintln!("[mux] ac3 SetInputType => {r:?}");
                if r.is_err() {
                    has_audio = false;
                } else {
                    let _ = alac_enc.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
                    let _ = alac_enc.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);
                    // 手写 dac3 配置（fscod/bsid/bsmod/acmod/lfeon/bit_rate_code）
                    let cfg = dac3_cfg(sample_rate, channels);
                    eprintln!("[mux] dac3 cfg = {cfg:02X?}");
                    aac = Some(alac_enc);
                    audio_reader = Some(ar);
                    alac_cfg_out = Some(cfg);
                }
            }
        }

        // 4. 自写 MP4 muxer
        let mut mux = Mp4Muxer::new(
            output,
            vw,
            vh,
            fnum,
            fden,
            sample_rate,
            channels,
            has_audio,
            alac_cfg_out.unwrap_or_default(),
            "ac-3",
            1536,
        )
        .expect("muxer init");

        // 5. 视频循环
        let t1 = Instant::now();
        let mut in_frames = 0usize;
        let mut out_frames = 0usize;
        let mut need_drain = false;
        loop {
            let mut flags: u32 = 0;
            let mut ts: i64 = 0;
            let mut nsample: Option<IMFSample> = None;
            let rr = reader
                .ReadSample(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut ts),
                    Some(&mut nsample),
                );
            if rr.is_err() || flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 || nsample.is_none() {
                need_drain = true;
                break;
            }
            let ins0 = nsample.unwrap();
            let ins: IMFSample = {
                let b = ins0.ConvertToContiguousBuffer().expect("contig");
                let mut p: *mut u8 = std::ptr::null_mut();
                let mut m: u32 = 0;
                let mut c: u32 = 0;
                b.Lock(&mut p, Some(&mut m), Some(&mut c)).expect("lock");
                let need = (vw * vh * 3 / 2) as u32;
                let built = if c != need {
                    let src = std::slice::from_raw_parts(p, c as usize);
                    let mut compact = vec![0u8; need as usize];
                    let row = vw as usize;
                    for y in 0..vh as usize {
                        compact[y * row..(y + 1) * row].copy_from_slice(&src[y * row..(y + 1) * row]);
                    }
                    let uv_dst = need as usize - (vh as usize / 2) * row;
                    let uv_src = c as usize - (vh as usize / 2) * row;
                    compact[uv_dst..].copy_from_slice(&src[uv_src..]);
                    drop(src);
                    let buf = MFCreateMemoryBuffer(need).expect("mem buf");
                    let mut pp: *mut u8 = std::ptr::null_mut();
                    let mut mm: u32 = 0;
                    let mut cc: u32 = 0;
                    buf.Lock(&mut pp, Some(&mut mm), Some(&mut cc)).expect("lock2");
                    std::ptr::copy_nonoverlapping(compact.as_ptr(), pp, need as usize);
                    let _ = buf.Unlock();
                    let s = MFCreateSample().expect("sample");
                    s.AddBuffer(&buf).expect("addbuf");
                    let _ = s.SetSampleTime(ts);
                    let _ = s.SetSampleDuration(333333);
                    Some(s)
                } else {
                    let s = MFCreateSample().expect("sample");
                    s.AddBuffer(&b).expect("addbuf");
                    let _ = s.SetSampleTime(ts);
                    let _ = s.SetSampleDuration(333333);
                    Some(s)
                };
                let _ = b.Unlock();
                built.unwrap()
            };
            let r = encoder.ProcessInput(0, &ins, 0);
            if r.is_err() {
                eprintln!("[mux] ProcessInput err => {r:?}");
                break;
            }
            in_frames += 1;
            loop {
                let st = encoder.GetOutputStatus().unwrap_or(0);
                if st == 0 {
                    break;
                }
                let mut out_status: u32 = 0;
                let obuf = MFCreateMemoryBuffer(8_000_000).expect("out mem buf");
                let mut op: *mut u8 = std::ptr::null_mut();
                let mut om: u32 = 0;
                let mut oc: u32 = 0;
                obuf.Lock(&mut op, Some(&mut om), Some(&mut oc)).expect("out lock");
                let _ = obuf.Unlock();
                let out_sample = MFCreateSample().expect("out sample");
                out_sample.AddBuffer(&obuf).expect("add out buf");
                let events = MFCreateCollection().expect("collection");
                let mut data = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: std::mem::ManuallyDrop::new(Some(out_sample)),
                    dwStatus: 0,
                    pEvents: std::mem::ManuallyDrop::new(Some(events)),
                };
                let mut buffers = [data];
                let r = encoder.ProcessOutput(0, &mut buffers, &mut out_status);
                match &r {
                    Ok(()) => {
                        if let Some(s) = buffers[0].pSample.take() {
                            let b = s.ConvertToContiguousBuffer().expect("out contig");
                            let mut p2: *mut u8 = std::ptr::null_mut();
                            let mut m2: u32 = 0;
                            let mut c2: u32 = 0;
                            b.Lock(&mut p2, Some(&mut m2), Some(&mut c2)).expect("out lock");
                            let raw = std::slice::from_raw_parts(p2, c2 as usize);
                            mux.add_video(raw).expect("add video");
                            let _ = b.Unlock();
                            out_frames += 1;
                        }
                    }
                    Err(e) => {
                        if e.code() == windows::core::HRESULT(0xC00D6D72u32 as i32) {
                            break;
                        }
                        eprintln!("[mux] ProcessOutput err => {e:?} at in={in_frames}");
                        break;
                    }
                }
            }
        }
        // drain 视频
        if need_drain {
            let _ = encoder.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
            loop {
                let mut out_status: u32 = 0;
                let obuf = MFCreateMemoryBuffer(8_000_000).expect("out mem buf");
                let mut op: *mut u8 = std::ptr::null_mut();
                let mut om: u32 = 0;
                let mut oc: u32 = 0;
                obuf.Lock(&mut op, Some(&mut om), Some(&mut oc)).expect("out lock");
                let _ = obuf.Unlock();
                let out_sample = MFCreateSample().expect("out sample");
                out_sample.AddBuffer(&obuf).expect("add out buf");
                let events = MFCreateCollection().expect("collection");
                let mut data = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: std::mem::ManuallyDrop::new(Some(out_sample)),
                    dwStatus: 0,
                    pEvents: std::mem::ManuallyDrop::new(Some(events)),
                };
                let mut buffers = [data];
                let r = encoder.ProcessOutput(0, &mut buffers, &mut out_status);
                match &r {
                    Ok(()) => {
                        if let Some(s) = buffers[0].pSample.take() {
                            let b = s.ConvertToContiguousBuffer().expect("out contig");
                            let mut p2: *mut u8 = std::ptr::null_mut();
                            let mut m2: u32 = 0;
                            let mut c2: u32 = 0;
                            b.Lock(&mut p2, Some(&mut m2), Some(&mut c2)).expect("out lock");
                            let raw = std::slice::from_raw_parts(p2, c2 as usize);
                            mux.add_video(raw).expect("add video drain");
                            let _ = b.Unlock();
                            out_frames += 1;
                        }
                    }
                    Err(e) => {
                        if e.code() == windows::core::HRESULT(0xC00D6D72u32 as i32) {
                            break;
                        }
                        eprintln!("[mux] drain ProcessOutput err => {e:?}");
                        break;
                    }
                }
            }
        }
        eprintln!(
            "[mux] video in={in_frames} out={out_frames} in {:?}",
            t1.elapsed()
        );

        // 6. 音频循环（AAC）
        if let (Some(ar), Some(aac_enc)) = (audio_reader.as_ref(), aac.as_ref()) {
            let t2 = Instant::now();
            let mut a_in = 0usize;
            let mut a_out = 0usize;
            let spf = channels as usize * 2; // 每帧 PCM 字节（16bit）
            let mut pcm_buf: Vec<u8> = Vec::new();
            let mut a_done = false;
            // 读取全部 PCM 到内存（音频通常 <100MB）
            let mut all_pcm: Vec<u8> = Vec::new();
            loop {
                let mut flags: u32 = 0;
                let mut ts: i64 = 0;
                let mut nsample: Option<IMFSample> = None;
                let rr = ar.ReadSample(
                    MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut ts),
                    Some(&mut nsample),
                );
                if rr.is_err() || flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                if let Some(s) = nsample {
                    let b = s.ConvertToContiguousBuffer().expect("a contig");
                    let mut p: *mut u8 = std::ptr::null_mut();
                    let mut m: u32 = 0;
                    let mut c: u32 = 0;
                    b.Lock(&mut p, Some(&mut m), Some(&mut c)).expect("a lock");
                    if !p.is_null() && c > 0 {
                        let raw = std::slice::from_raw_parts(p, c as usize);
                        all_pcm.extend_from_slice(raw);
                    }
                    let _ = b.Unlock();
                }
            }
            eprintln!("[mux] pcm bytes={}", all_pcm.len());
            // 切 1536 帧块编码（AC-3 帧 = 1536 samples）
            let chunk_frames = 1536usize;
            let chunk_bytes = chunk_frames * spf;
            let mut off = 0usize;
            pcm_buf.clear();
            while off + chunk_bytes <= all_pcm.len() {
                let chunk = &all_pcm[off..off + chunk_bytes];
                let s = MFCreateSample().expect("a sample");
                let buf = MFCreateMemoryBuffer(chunk_bytes as u32).expect("a buf");
                let mut p: *mut u8 = std::ptr::null_mut();
                let mut m: u32 = 0;
                buf.Lock(&mut p, Some(&mut m), None).expect("a lock2");
                std::ptr::copy_nonoverlapping(chunk.as_ptr(), p, chunk_bytes);
                buf.SetCurrentLength(chunk_bytes as u32).expect("a len");
                let _ = buf.Unlock();
                s.AddBuffer(&buf).expect("a addbuf");
                let _ = s.SetSampleTime((a_in as i64) * 1024 * 10_000_000 / (sample_rate as i64));
                let _ = s.SetSampleDuration(1024 * 10_000_000 / (sample_rate as i64));
                let r = aac_enc.ProcessInput(0, &s, 0);
                if r.is_err() {
                    eprintln!("[mux] aac ProcessInput err => {r:?} at {a_in}");
                    break;
                }
                a_in += 1;
                // 取输出（同步 MFT：直接 ProcessOutput 驱动，不依赖 GetOutputStatus）
                loop {
                    let mut out_status: u32 = 0;
                    let obuf = MFCreateMemoryBuffer(64 * 1024).expect("a out buf");
                    let mut op: *mut u8 = std::ptr::null_mut();
                    let mut om: u32 = 0;
                    let mut oc: u32 = 0;
                    obuf.Lock(&mut op, Some(&mut om), Some(&mut oc)).expect("a out lock");
                    let _ = obuf.Unlock();
                    let out_sample = MFCreateSample().expect("a out sample");
                    out_sample.AddBuffer(&obuf).expect("a add out");
                    let events = MFCreateCollection().expect("a coll");
                    let mut data = MFT_OUTPUT_DATA_BUFFER {
                        dwStreamID: 0,
                        pSample: std::mem::ManuallyDrop::new(Some(out_sample)),
                        dwStatus: 0,
                        pEvents: std::mem::ManuallyDrop::new(Some(events)),
                    };
                    let mut buffers = [data];
                    let r = aac_enc.ProcessOutput(0, &mut buffers, &mut out_status);
                    match &r {
                        Ok(()) => {
                            if let Some(s) = buffers[0].pSample.take() {
                                let b = s.ConvertToContiguousBuffer().expect("a out contig");
                                let mut p2: *mut u8 = std::ptr::null_mut();
                                let mut m2: u32 = 0;
                                let mut c2: u32 = 0;
                                b.Lock(&mut p2, Some(&mut m2), Some(&mut c2)).expect("a out lock");
                                let raw = std::slice::from_raw_parts(p2, c2 as usize);
                                mux.add_audio(raw).expect("add audio");
                                let _ = b.Unlock();
                                a_out += 1;
                            }
                        }
                        Err(e) => {
                            if e.code() == windows::core::HRESULT(0xC00D6D72u32 as i32) {
                                break;
                            }
                            eprintln!("[mux] aac ProcessOutput err => {e:?}");
                            break;
                        }
                    }
                }
                off += chunk_bytes;
            }
            // drain 音频
            let _ = aac_enc.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
            loop {
                let mut out_status: u32 = 0;
                let obuf = MFCreateMemoryBuffer(64 * 1024).expect("a out buf");
                let mut op: *mut u8 = std::ptr::null_mut();
                let mut om: u32 = 0;
                let mut oc: u32 = 0;
                obuf.Lock(&mut op, Some(&mut om), Some(&mut oc)).expect("a out lock");
                let _ = obuf.Unlock();
                let out_sample = MFCreateSample().expect("a out sample");
                out_sample.AddBuffer(&obuf).expect("a add out");
                let events = MFCreateCollection().expect("a coll");
                let mut data = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: std::mem::ManuallyDrop::new(Some(out_sample)),
                    dwStatus: 0,
                    pEvents: std::mem::ManuallyDrop::new(Some(events)),
                };
                let mut buffers = [data];
                let r = aac_enc.ProcessOutput(0, &mut buffers, &mut out_status);
                match &r {
                    Ok(()) => {
                        if let Some(s) = buffers[0].pSample.take() {
                            let b = s.ConvertToContiguousBuffer().expect("a out contig");
                            let mut p2: *mut u8 = std::ptr::null_mut();
                            let mut m2: u32 = 0;
                            let mut c2: u32 = 0;
                            b.Lock(&mut p2, Some(&mut m2), Some(&mut c2)).expect("a out lock");
                            let raw = std::slice::from_raw_parts(p2, c2 as usize);
                            mux.add_audio(raw).expect("add audio drain");
                            let _ = b.Unlock();
                            a_out += 1;
                        }
                    }
                    Err(e) => {
                        if e.code() == windows::core::HRESULT(0xC00D6D72u32 as i32) {
                            break;
                        }
                        eprintln!("[mux] aac drain err => {e:?}");
                        break;
                    }
                }
            }
            eprintln!("[mux] audio in={a_in} out={a_out} in {:?}", t2.elapsed());
        }

        // 7. finish
        mux.finish().expect("finish mux");
        eprintln!("[mux] FINISHED total {:?}", t0.elapsed());
        let _ = MFShutdown();
        CoUninitialize();
    }
}

/// 用系统 Media Foundation 打开 MP4，枚举流与时长（验证自写 muxer 输出可读）
fn probe_file(path: &str) {
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET);
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        match MFCreateSourceReaderFromURL(windows::core::PCWSTR(wide.as_ptr()), None) {
            Ok(reader) => {
                // 枚举所有流
                let mut idx = 0u32;
                loop {
                    match reader.GetNativeMediaType(idx, 0) {
                        Ok(t) => {
                            let major = t.GetGUID(&MF_MT_MAJOR_TYPE).unwrap_or_default();
                            let sub = t.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
                            let mut desc = String::new();
                            if major == MFMediaType_Video {
                                let fs = t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
                                let fr = t.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(0);
                                desc = format!(
                                    "video {}x{} fps={}/{} sub={sub:?}",
                                    (fs >> 32),
                                    (fs & 0xFFFF_FFFF),
                                    (fr >> 32),
                                    (fr & 0xFFFF_FFFF)
                                );
                            } else if major == MFMediaType_Audio {
                                let sr = t.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(0);
                                let ch = t.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(0);
                                let bits = t.GetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE).unwrap_or(0);
                                desc = format!("audio sr={sr} ch={ch} bits={bits} sub={sub:?}");
                            } else {
                                desc = format!("other major={major:?} sub={sub:?}");
                            }
                            eprintln!("[probe] stream[{idx}]: {desc}");
                            idx += 1;
                        }
                        Err(_) => break,
                    }
                }
                eprintln!("[probe] total streams = {idx}");
            }
            Err(e) => eprintln!("[probe] FAILED to open output: {e:?}"),
        }
        let _ = MFShutdown();
        CoUninitialize();
    }
}

/// 用系统 SourceReader 实际解码视频轨前 30 帧，验证输出视频是否真的可解
fn decode_probe(path: &str, raw: bool) {
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET);
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        match MFCreateSourceReaderFromURL(windows::core::PCWSTR(wide.as_ptr()), None) {
            Ok(reader) => {
                let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
                if reader
                    .SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, true)
                    .is_err()
                {
                    eprintln!("[decode] 无视频流");
                    let _ = MFShutdown();
                    CoUninitialize();
                    return;
                }
                let mut do_read = || {
                    let mut ok = 0u32;
                    for i in 0..30 {
                        let mut flags: u32 = 0;
                        let mut ts: i64 = 0;
                        let mut sample: Option<IMFSample> = None;
                        match reader.ReadSample(
                            MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                            0,
                            None,
                            Some(&mut flags),
                            Some(&mut ts),
                            Some(&mut sample),
                        ) {
                            Ok(_) => {
                                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                                    eprintln!("[decode] EOF at frame {i}");
                                    break;
                                }
                                if let Some(s) = sample {
                                    ok += 1;
                                    if let Ok(b) = s.ConvertToContiguousBuffer() {
                                        let mut p: *mut u8 = std::ptr::null_mut();
                                        let mut ml: u32 = 0;
                                        let mut cl: u32 = 0;
                                        if b
                                            .Lock(&mut p, Some(&mut ml), Some(&mut cl))
                                            .is_ok()
                                        {
                                            if i < 3 {
                                                eprintln!("[decode] frame {i}: {cl} bytes ts={ts}");
                                            }
                                            let _ = b.Unlock();
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("[decode] read error at frame {i}: {e}");
                                return;
                            }
                        }
                    }
                    eprintln!("[decode] RESULT: {ok}/30 frames read");
                };
                if raw {
                    for nidx in 0..8u32 {
                        if let Ok(t) = reader.GetNativeMediaType(nidx, 0) {
                            let major = t.GetGUID(&MF_MT_MAJOR_TYPE).unwrap_or_default();
                            if major != MFMediaType_Video {
                                continue;
                            }
                            let sub = t.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
                            let fs = t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
                            let fr = t.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(0);
                            let br = t.GetUINT32(&MF_MT_AVG_BITRATE).unwrap_or(0);
                            let pf = t.GetUINT32(&MF_MT_MPEG2_PROFILE).unwrap_or(0);
                            eprintln!("[decode] native video stream[{nidx}]: sub={sub:?} {}x{} fps={}/{} br={br} profile={pf}",
                                (fs >> 32), (fs & 0xFFFF_FFFF), (fr >> 32), (fr & 0xFFFF_FFFF));
                            let alc = t.GetUINT32(&MF_MT_MPEG2_LEVEL).unwrap_or(0);
                            eprintln!("[decode] MF_MT_MPEG2_LEVEL={alc}");
                            break;
                        }
                    }
                    do_read();
                    let _ = MFShutdown();
                    CoUninitialize();
                    return;
                }
                let mut req = MFCreateMediaType().expect("create type");
                req.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                    .expect("set major");
                let mut any_ok = false;
                for (name, sub) in [
                    ("NV12", MFVideoFormat_NV12),
                    ("YUY2", MFVideoFormat_YUY2),
                    ("RGB32", MFVideoFormat_RGB32),
                ] {
                    let _ = req.SetGUID(&MF_MT_SUBTYPE, &sub);
                    match reader.SetCurrentMediaType(
                        MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                        None,
                        &req,
                    ) {
                        Ok(_) => {
                            eprintln!("[decode] set {name} OK");
                            any_ok = true;
                            do_read();
                            break;
                        }
                        Err(e) => eprintln!("[decode] set {name} failed: {e}"),
                    }
                }
                if !any_ok {
                    eprintln!("[decode] 视频解码器无法输出任何测试格式");
                }
            }
            Err(e) => eprintln!("[decode] open failed: {e}"),
        }
        let _ = MFShutdown();
        CoUninitialize();
    }
}

/// 编码一帧后 dump 输出样本属性（MF_NALU_LENGTH_INFORMATION / MF_MT_MPEG_SEQUENCE_HEADER）与原始字节
fn dump_probe(input: &str) {
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::Media::MediaFoundation::{
        MFCreateSourceReaderFromURL, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
        MF_SOURCE_READER_ALL_STREAMS,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED};
    use windows::core::Interface;

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET);

        let wide: Vec<u16> = input.encode_utf16().chain(std::iter::once(0)).collect();
        let reader = MFCreateSourceReaderFromURL(windows::core::PCWSTR(wide.as_ptr()), None)
            .expect("open reader");
        let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
        reader
            .SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, true)
            .expect("select video");
        let mut req = MFCreateMediaType().expect("create type");
        req.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).expect("set major");
        req.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).expect("set sub");
        reader
            .SetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, None, &req)
            .expect("set nv12");
        let cur = reader
            .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
            .expect("get type");
        let fs = cur.GetUINT64(&MF_MT_FRAME_SIZE).expect("frame size");
        let fr = cur.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(0);
        let vw = (fs >> 32) as u32;
        let vh = (fs & 0xFFFF_FFFF) as u32;
        eprintln!("[dump] source {vw}x{vh} fps={}", (fr >> 32));
        let mut flags: u32 = 0;
        let mut ts: i64 = 0;
        let mut sample: Option<IMFSample> = None;
        reader
            .ReadSample(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                0,
                None,
                Some(&mut flags),
                Some(&mut ts),
                Some(&mut sample),
            )
            .expect("read sample");
        let ins = sample.expect("sample");

        let encoder: IMFTransform = CoCreateInstance(&CMSH264EncoderMFT, None, CLSCTX_ALL)
            .expect("create encoder");
        let mut in_type = MFCreateMediaType().expect("in type");
        in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).expect("in major");
        in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).expect("in sub");
        in_type.SetUINT64(&MF_MT_FRAME_SIZE, fs).expect("in size");
        in_type.SetUINT64(&MF_MT_FRAME_RATE, fr).expect("in rate");
        in_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, 1920).expect("in stride");
        in_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .expect("in interlace");
        let mut chosen: Option<windows::Win32::Media::MediaFoundation::IMFMediaType> = None;
        for idx in 0..8u32 {
            match encoder.GetOutputAvailableType(0, idx) {
                Ok(t) => {
                    let sub = t.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
                    eprintln!("[dump] avail out type[{idx}] subtype={sub:?}");
                    if sub == MFVideoFormat_H264 {
                        chosen = Some(t);
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let mut out_type = chosen.expect("no H264 output type");
        out_type.SetUINT64(&MF_MT_FRAME_SIZE, fs).expect("out size");
        out_type.SetUINT64(&MF_MT_FRAME_RATE, fr).expect("out rate");
        out_type.SetUINT32(&MF_MT_AVG_BITRATE, 8_000_000).expect("out bitrate");
        let _ = out_type.SetUINT32(&MF_MT_MPEG2_PROFILE, 66u32);
        let _ = out_type.SetUINT32(&MF_MT_MPEG2_LEVEL, 40u32);
        let _ = out_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32);
        let rls = out_type.SetUINT32(&MF_NALU_LENGTH_SET, 1);
        eprintln!("[dump] set MF_NALU_LENGTH_SET=1 => {rls:?}");
        encoder.SetOutputType(0, &out_type, 0).expect("set out type");
        encoder.SetInputType(0, &in_type, 0).expect("set in type");

        // 输出类型更新后的 MPEG_SEQUENCE_HEADER
        let out_after = encoder.GetOutputCurrentType(0).expect("out current type");
        let seq = get_blob_attr(&out_after, &MF_MT_MPEG_SEQUENCE_HEADER);
        match &seq {
            Ok(s) => eprintln!(
                "[dump] MPEG_SEQUENCE_HEADER len={} first={:02x?}",
                s.len(),
                &s[..s.len().min(64)]
            ),
            Err(e) => eprintln!("[dump] no MPEG_SEQUENCE_HEADER: {e}"),
        }

        encoder
            .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
            .expect("begin streaming");
        encoder
            .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
            .expect("start of stream");

        // 喂 5 帧 + drain，收集输出样本，打印首个输出样本的 NALU 长度信息
        let mut first_done = false;
        let mut feed_count = 0;
        for _ in 0..5 {
            let mut flags: u32 = 0;
            let mut ts3: i64 = 0;
            let mut nsample: Option<IMFSample> = None;
            if reader
                .ReadSample(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut ts3),
                    Some(&mut nsample),
                )
                .is_err()
                || flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0
                || nsample.is_none()
            {
                break;
            }
            encoder
                .ProcessInput(0, &nsample.unwrap(), 0)
                .expect("process input");
            feed_count += 1;
            loop {
                let st = encoder.GetOutputStatus().unwrap_or(0);
                if st == 0 {
                    break;
                }
                let mut out_status: u32 = 0;
                let obuf = MFCreateMemoryBuffer(8_000_000).expect("out mem buf");
                let mut op: *mut u8 = std::ptr::null_mut();
                let mut om: u32 = 0;
                let mut oc: u32 = 0;
                obuf
                    .Lock(&mut op, Some(&mut om), Some(&mut oc))
                    .expect("out lock");
                let _ = obuf.Unlock();
                let out_sample = MFCreateSample().expect("out sample");
                out_sample.AddBuffer(&obuf).expect("add out buf");
                let events = MFCreateCollection().expect("collection");
                let mut data = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: std::mem::ManuallyDrop::new(Some(out_sample)),
                    dwStatus: 0,
                    pEvents: std::mem::ManuallyDrop::new(Some(events)),
                };
                let mut buffers = [data];
                match encoder.ProcessOutput(0, &mut buffers, &mut out_status) {
                    Ok(()) => {
                        if let Some(s) = buffers[0].pSample.take() {
                            if !first_done {
                                first_done = true;
                                let b = s
                                    .ConvertToContiguousBuffer()
                                    .expect("out contig");
                                let mut p2: *mut u8 = std::ptr::null_mut();
                                let mut m2: u32 = 0;
                                let mut c2: u32 = 0;
                                b.Lock(&mut p2, Some(&mut m2), Some(&mut c2))
                                    .expect("out lock");
                                eprintln!("[dump] sample bytes={c2}");
                                let raw = std::slice::from_raw_parts(p2, c2.min(96) as usize);
                                eprintln!(
                                    "[dump] first 96 bytes: {:02x?}",
                                    raw.iter().map(|x| *x).collect::<Vec<u8>>()
                                );
                                let _ = b.Unlock();
                            }
                        }
                    }
                    Err(e) => {
                        if e.code() == windows::core::HRESULT(0xC00D6D72u32 as i32) {
                            break;
                        }
                        eprintln!("[dump] ProcessOutput err: {e:?}");
                        break;
                    }
                }
            }
        }
        eprintln!("[dump] fed {feed_count} frames");
        // drain
        let _ = encoder.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
        loop {
            let mut out_status: u32 = 0;
            let obuf = MFCreateMemoryBuffer(8_000_000).expect("out mem buf");
            let mut op: *mut u8 = std::ptr::null_mut();
            let mut om: u32 = 0;
            let mut oc: u32 = 0;
            obuf
                .Lock(&mut op, Some(&mut om), Some(&mut oc))
                .expect("out lock");
            let _ = obuf.Unlock();
            let out_sample = MFCreateSample().expect("out sample");
            out_sample.AddBuffer(&obuf).expect("add out buf");
            let events = MFCreateCollection().expect("collection");
            let mut data = MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: std::mem::ManuallyDrop::new(Some(out_sample)),
                dwStatus: 0,
                pEvents: std::mem::ManuallyDrop::new(Some(events)),
            };
            let mut buffers = [data];
            match encoder.ProcessOutput(0, &mut buffers, &mut out_status) {
                Ok(()) => {
                    if let Some(s) = buffers[0].pSample.take() {
                        if !first_done {
                            first_done = true;
                            match get_blob_attr(&s, &MF_NALU_LENGTH_INFORMATION) {
                                Ok(blob) => {
                                    let n = blob.len() / 4;
                                    let mut lens: Vec<u32> = Vec::new();
                                    for j in 0..n {
                                        lens.push(u32::from_be_bytes([
                                            blob[j * 4],
                                            blob[j * 4 + 1],
                                            blob[j * 4 + 2],
                                            blob[j * 4 + 3],
                                        ]));
                                    }
                                    eprintln!("[dump] MF_NALU_LENGTH_INFORMATION(DWORDs): {lens:?}");
                                }
                                Err(e) => eprintln!("[dump] no MF_NALU_LENGTH_INFORMATION: {e}"),
                            }
                            let b = s.ConvertToContiguousBuffer().expect("out contig");
                            let mut p2: *mut u8 = std::ptr::null_mut();
                            let mut m2: u32 = 0;
                            let mut c2: u32 = 0;
                            b.Lock(&mut p2, Some(&mut m2), Some(&mut c2))
                                .expect("out lock");
                            eprintln!("[dump] sample bytes={c2}");
                            let raw = std::slice::from_raw_parts(p2, c2.min(96) as usize);
                            eprintln!(
                                "[dump] first 96 bytes: {:02x?}",
                                raw.iter().map(|x| *x).collect::<Vec<u8>>()
                            );
                            let _ = b.Unlock();
                        }
                    }
                }
                Err(e) => {
                    if e.code() == windows::core::HRESULT(0xC00D6D72u32 as i32) {
                        break;
                    }
                    eprintln!("[dump] drain ProcessOutput err: {e:?}");
                    break;
                }
            }
        }
        let _ = MFShutdown();
        CoUninitialize();
    }
}

/// IMFAttributes::GetBlob 的 0.58 两遍调用封装
fn get_blob_attr(
    attrs: &windows::Win32::Media::MediaFoundation::IMFAttributes,
    guid: &windows::core::GUID,
) -> Result<Vec<u8>, String> {
    unsafe {
        let sz = attrs
            .GetBlobSize(guid as *const _)
            .map_err(|e| format!("{e}"))?;
        let mut buf = vec![0u8; sz as usize];
        attrs
            .GetBlob(guid as *const _, &mut buf, None)
            .map_err(|e| format!("{e}"))?;
        Ok(buf)
    }
}

/// 枚举系统已注册的音频编码器 MFT
fn enum_audio_encoders() {
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, CoTaskMemFree, COINIT_MULTITHREADED};
    use windows::core::Interface;

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET);
        let mut out: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count: u32 = 0;
        let hr = MFTEnumEx(
            MFT_CATEGORY_AUDIO_ENCODER,
            MFT_ENUM_FLAG_ALL,
            None,
            None,
            &mut out,
            &mut count,
        );
        eprintln!("[aenc] MFTEnumEx hr={hr:?} count={count}");
        if hr.is_ok() {
            for i in 0..count as usize {
                let act = &*out.add(i);
                if let Some(a) = act.as_ref() {
                    let clsid = a
                        .GetGUID(&MFT_TRANSFORM_CLSID_Attribute)
                        .unwrap_or_default();
                    eprintln!("[aenc] #{i}: clsid={clsid:?}");
                    if let Ok(mft) = a.ActivateObject::<IMFTransform>() {
                        for t_idx in 0..4u32 {
                            match mft.GetOutputAvailableType(0, t_idx) {
                                Ok(t) => {
                                    let sub = t.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
                                    eprintln!("    out[{t_idx}] sub={sub:?}");
                                }
                                Err(_) => break,
                            }
                        }
                    }
                }
            }
            CoTaskMemFree(Some(out as *const core::ffi::c_void));
        }
        let _ = MFShutdown();
        CoUninitialize();
    }
}

/// 手动解码实验：raw 读首样本（AVCC）→ 转 Annex B → 直接喂 H.264 解码器 MFT
fn decode_sample_probe(path: &str) {
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED};
    use windows::core::Interface;

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET);
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let reader = MFCreateSourceReaderFromURL(windows::core::PCWSTR(wide.as_ptr()), None)
            .expect("open reader");
        let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
        let _ = reader.SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, true);
        let mut flags: u32 = 0;
        let mut ts: i64 = 0;
        let mut sample: Option<IMFSample> = None;
        reader
            .ReadSample(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                0,
                None,
                Some(&mut flags),
                Some(&mut ts),
                Some(&mut sample),
            )
            .expect("read first sample");
        let s = sample.expect("no sample");
        let b = s.ConvertToContiguousBuffer().expect("contig");
        let mut p: *mut u8 = std::ptr::null_mut();
        let mut ml: u32 = 0;
        let mut cl: u32 = 0;
        b.Lock(&mut p, Some(&mut ml), Some(&mut cl)).expect("lock");
        let avcc = std::slice::from_raw_parts(p, cl as usize).to_vec();
        let _ = b.Unlock();
        eprintln!("[dec1] first sample avcc bytes={}", avcc.len());
        eprintln!("[dec1] first 96 bytes: {:02x?}", avcc[..avcc.len().min(96)].to_vec());

        // AVCC → Annex B（失败则保留原始）
        let mut annexb: Vec<u8> = Vec::new();
        let mut ok_parse = true;
        let mut i = 0usize;
        while i + 4 <= avcc.len() {
            let len = u32::from_be_bytes([avcc[i], avcc[i + 1], avcc[i + 2], avcc[i + 3]]) as usize;
            if len == 0 || i + 4 + len > avcc.len() {
                eprintln!("[dec1] bad avcc length {len} at {i}");
                ok_parse = false;
                break;
            }
            annexb.extend_from_slice(&[0u8, 0, 0, 1]);
            annexb.extend_from_slice(&avcc[i + 4..i + 4 + len]);
            i += 4 + len;
        }
        let feed: Vec<u8> = if ok_parse && annexb.len() > 100 {
            eprintln!("[dec1] annexb bytes={}", annexb.len());
            annexb
        } else {
            eprintln!("[dec1] using raw avcc sample directly ({})", avcc.len());
            avcc
        };

        // 创建解码器
        let dec_clsid = windows::core::GUID::from_u128(0x62ce7e72_4c71_4d20_b15d_452831a87d9d);
        let dec: IMFTransform = CoCreateInstance(&dec_clsid, None, CLSCTX_ALL)
            .expect("create h264 decoder");
        // 输入类型
        let mut in_t = MFCreateMediaType().expect("create in type");
        in_t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).expect("major");
        in_t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).expect("sub");
        in_t.SetUINT64(&MF_MT_FRAME_SIZE, (1920u64 << 32) | 1080).expect("fs");
        in_t.SetUINT64(&MF_MT_FRAME_RATE, ((30u64) << 32) | 1).expect("fr");
        in_t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).expect("ilace");
        let ir = dec.SetInputType(0, &in_t, 0);
        eprintln!("[dec1] set input type: {ir:?}");
        // 输出类型：先输入后枚举，找 NV12 或第一个
        let mut out_t: Option<IMFMediaType> = None;
        let mut first_t: Option<IMFMediaType> = None;
        for t_idx in 0..8u32 {
            match dec.GetOutputAvailableType(0, t_idx) {
                Ok(t) => {
                    if first_t.is_none() {
                        first_t = Some(t.clone());
                    }
                    if t.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default() == MFVideoFormat_NV12 {
                        out_t = Some(t);
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let out_t = out_t.or(first_t).expect("no decoder output type");
        let sub_o = out_t.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
        eprintln!("[dec1] decoder output subtype: {sub_o:?}");
        out_t.SetUINT64(&MF_MT_FRAME_SIZE, (1920u64 << 32) | 1080).expect("out fs");
        out_t.SetUINT64(&MF_MT_FRAME_RATE, ((30u64) << 32) | 1).expect("out fr");
        dec.SetOutputType(0, &out_t, 0).expect("set out");
        dec.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).expect("begin");
        dec.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).expect("start");
        // 构造样本
        let in_sample = MFCreateSample().expect("create sample");
        let buf = MFCreateMemoryBuffer(feed.len() as u32).expect("create buf");
        let mut bp: *mut u8 = std::ptr::null_mut();
        let mut bm: u32 = 0;
        buf.Lock(&mut bp, Some(&mut bm), None).expect("lock in");
        std::ptr::copy_nonoverlapping(feed.as_ptr(), bp, feed.len());
        buf.SetCurrentLength(feed.len() as u32).expect("set len");
        let _ = buf.Unlock();
        in_sample.AddBuffer(&buf).expect("add buf");
        in_sample.SetSampleTime(0).expect("set time");
        in_sample.SetSampleDuration(333_333).expect("set dur");
        let pi = dec.ProcessInput(0, &in_sample, 0);
        eprintln!("[dec1] process input: {pi:?}");
        // drain + 输出
        let _ = dec.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
        let mut got = 0u32;
        for _ in 0..8 {
            let mut out_status: u32 = 0;
            let obuf = MFCreateMemoryBuffer(8_000_000).expect("out mem");
            let mut op2: *mut u8 = std::ptr::null_mut();
            let mut om2: u32 = 0;
            let mut oc2: u32 = 0;
            obuf.Lock(&mut op2, Some(&mut om2), Some(&mut oc2)).expect("out lock");
            let _ = obuf.Unlock();
            let os = MFCreateSample().expect("out sample");
            os.AddBuffer(&obuf).expect("add out");
            let events = MFCreateCollection().expect("coll");
            let mut data = MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: std::mem::ManuallyDrop::new(Some(os)),
                dwStatus: 0,
                pEvents: std::mem::ManuallyDrop::new(Some(events)),
            };
            let mut buffers = [data];
            match dec.ProcessOutput(0, &mut buffers, &mut out_status) {
                Ok(()) => {
                    if let Some(ob) = buffers[0].pSample.take() {
                        let cb = ob.ConvertToContiguousBuffer().expect("out contig");
                        let mut p3: *mut u8 = std::ptr::null_mut();
                        let mut m3: u32 = 0;
                        let mut c3: u32 = 0;
                        cb.Lock(&mut p3, Some(&mut m3), Some(&mut c3)).expect("lock out2");
                        eprintln!("[dec1] decoded frame: {c3} bytes");
                        let _ = cb.Unlock();
                        got += 1;
                        break;
                    }
                }
                Err(e) => {
                    if e.code() == windows::core::HRESULT(0xC00D6D72u32 as i32) {
                        break;
                    }
                    eprintln!("[dec1] process output err: {e:?}");
                    break;
                }
            }
        }
        eprintln!("[dec1] RESULT: {got} decoded frames");
        let _ = MFShutdown();
        CoUninitialize();
    }
}

/// 用 Windows SinkWriter 原生生成 AAC 参考 MP4（SourceReader 解 PCM → SinkWriter 编码 AAC）
fn make_ref_aac(input: &str, output: &str) {
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
    use windows::core::Interface;

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET);
        let r = (|| -> Result<(), String> {
            let in_wide: Vec<u16> = input.encode_utf16().chain(std::iter::once(0)).collect();
            let reader = MFCreateSourceReaderFromURL(windows::core::PCWSTR(in_wide.as_ptr()), None)
                .map_err(|e| format!("创建 SourceReader 失败: {e}"))?;
            // 找到音频流
            let mut ai: Option<u32> = None;
            for i in 0..12u32 {
                match reader.GetCurrentMediaType(i) {
                    Ok(t) => {
                        let major = t.GetGUID(&MF_MT_MAJOR_TYPE).unwrap_or_default();
                        if major == MFMediaType_Audio {
                            ai = Some(i);
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let ai = ai.ok_or("输入文件没有音频流")?;
            reader
                .SetStreamSelection(ai, true)
                .map_err(|e| format!("选择音频流失败: {e}"))?;
            // 让 SourceReader 解码为 PCM
            let pcm = MFCreateMediaType().map_err(|e| format!("创建 PCM 类型失败: {e}"))?;
            pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                .map_err(|e| format!("设置类型失败: {e}"))?;
            pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
                .map_err(|e| format!("设置 PCM 失败: {e}"))?;
            reader
                .SetCurrentMediaType(ai, None, &pcm)
                .map_err(|e| format!("设置解码输出失败: {e}"))?;
            // SinkWriter
            let out_wide: Vec<u16> = output.encode_utf16().chain(std::iter::once(0)).collect();
            let sink = MFCreateSinkWriterFromURL(
                windows::core::PCWSTR(out_wide.as_ptr()),
                None,
                None,
            )
            .map_err(|e| format!("创建 SinkWriter 失败: {e}"))?;
            let amt = MFCreateMediaType().map_err(|e| format!("创建音频类型失败: {e}"))?;
            amt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                .map_err(|e| format!("设置类型失败: {e}"))?;
            amt.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)
                .map_err(|e| format!("设置 AAC 失败: {e}"))?;
            let sid = sink
                .AddStream(&amt)
                .map_err(|e| format!("AddStream 失败: {e}"))?;
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 0).map_err(|e| format!("创建属性失败: {e}"))?;
            attrs
                .as_ref()
                .ok_or("属性为空")?
                .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)
                .map_err(|e| format!("设置容器失败: {e}"))?;
            sink.SetInputMediaType(sid, &amt, attrs.as_ref())
                .map_err(|e| format!("SetInputMediaType 失败: {e}"))?;
            sink.BeginWriting()
                .map_err(|e| format!("BeginWriting 失败: {e}"))?;
            let mut count = 0u32;
            loop {
                let mut flags: u32 = 0;
                let mut ts: i64 = 0;
                let mut sample: Option<IMFSample> = None;
                match reader.ReadSample(ai, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample)) {
                    Ok(()) => {
                        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                            break;
                        }
                        if let Some(s) = sample {
                            sink.WriteSample(sid, &s)
                                .map_err(|e| format!("WriteSample 失败: {e}"))?;
                            count += 1;
                        }
                    }
                    Err(e) => {
                        eprintln!("[mkref] ReadSample err: {e:?}");
                        break;
                    }
                }
            }
            eprintln!("[mkref] wrote {count} samples");
            sink.Finalize().map_err(|e| format!("Finalize 失败: {e}"))?;
            Ok(())
        })();
        eprintln!("[mkref] RESULT: {r:?}");
        let _ = MFShutdown();
        CoUninitialize();
    }
}
