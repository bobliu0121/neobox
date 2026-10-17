# NeoBox 磁盘健康度检测 — 直读 SMART 方案设计

> 参考开源项目：**CrystalDiskInfo**（MIT License，https://crystalmark.info/）
> 核心移植源码：`CrystalDiskInfo-master/AtaSmart.cpp`、`AtaSmart.h`、`StorageQuery.h`、`NVMeInterpreter.cpp`
> 实现语言：Rust（Tauri 后端）+ React/TypeScript（前端）

## 1. 背景与目标

NeoBox 原有磁盘健康检测依赖 PowerShell（`Get-PhysicalDisk` / `Get-StorageReliabilityCounter`），存在以下问题：

- **启动慢**：每次刷新都要拉起 PowerShell 进程（秒级延迟）；
- **信息有限**：只能拿到 Windows 存储栈的 `HealthStatus` 字符串（Healthy/Warning/Unhealthy），无法获得坏扇区计数、SSD 磨损寿命等明细；
- **无健康度百分比**：无法像 CrystalDiskInfo 那样展示「健康度 xx%」。

本次改造的目标：

1. **弃用 PowerShell**，改为纯 Rust + Windows API 直读 SMART 数据；
2. 完整移植 CrystalDiskInfo 的 `CheckDiskStatus` 健康度判定逻辑；
3. 输出**健康度百分比**（NVMe / SSD 的寿命百分比，HDD 按状态映射）；
4. 前端展示「健康度 xx%」进度条，颜色随状态变化；
5. 编写本文档记录设计与对应关系。

## 2. 总体架构

```
DiskHealthPage.tsx ──invoke get_disk_health_info──▶ hardware.rs
                                                        │
                                    ┌───────────────────┴──────────────────┐
                                    │                                     │
                         wmi_query (COM 静态信息)                    smart.rs
                                    │                                ┌─────────┐
                                    │                                │ 读取层   │
                                    │                                │  ATA:   │
                                    │                                │ DFP_    │
                                    │                                │ RECEIVE │
                                    │                                │ _DRIVE  │
                                    │                                │ _DATA   │
                                    │                                │  NVMe:  │
                                    │                                │ IOCTL_  │
                                    │                                │ STORAGE │
                                    │                                │ _QUERY_ │
                                    │                                │ PROPERTY│
                                    │                                ├─────────┤
                                    │                                │ 解析层   │
                                    │                                │ parse_  │
                                    │                                │ attribu │
                                    │                                │ tes/    │
                                    │                                │ thresho │
                                    │                                │ lds     │
                                    │                                ├─────────┤
                                    │                                │ 判定层   │
                                    │                                │ check_  │
                                    │                                │ ata_    │
                                    │                                │ status /│
                                    │                                │ check_  │
                                    │                                │ nvme_   │
                                    │                                │ status  │
                                    │                                └─────────┘
                                    │
                         enumerate_volumes_by_disk (winapi 分区枚举)
```

- **读取层**：通过 `DeviceIoControl` 直接向磁盘发送 SMART 命令，读取 512 字节原始数据（对应 CDI `GetSmartAttributePd` / `GetSmartAttributeNVMeStorageQuery`）；
- **解析层**：按 `SMART_ATTRIBUTE`（12 字节/条）结构解析属性与阈值（对应 CDI `FillSmartData`）；
- **判定层**：移植 CDI `CheckDiskStatus`（L12522-12830），输出四级状态 + 寿命百分比。

## 3. SMART 数据来源

### 3.1 ATA（SATA 机械盘 / 固态盘）

对应 CDI `GetSmartAttributePd`（`AtaSmart.cpp` L7056-7109）：

```
DeviceIoControl(hDevice, DFP_RECEIVE_DRIVE_DATA, ...)
```

| 项目 | 值 | 来源 |
|------|-----|------|
| IOCTL | `DFP_RECEIVE_DRIVE_DATA = 0x0007C088` | CDI AtaSmart.h L409 |
| SMART 命令 | `bCommandReg = 0xB0` (SMART_CMD) | ntdddisk.h |
| 读属性 | `bFeaturesReg = 0xD0` (READ_ATTRIBUTES) | ntdddisk.h |
| 读阈值 | `bFeaturesReg = 0xD1` (READ_THRESHOLDS) | ntdddisk.h |
| 圆柱寄存器 | `bCylLowReg = 0x4F`, `bCylHighReg = 0xC2` | ntdddisk.h |
| 主盘目标 | `bDriveHeadReg = 0xA0` | CDI `AddDisk(..., 0xA0, ...)` |
| 缓冲大小 | `cBufferSize = 512` | CDI READ_ATTRIBUTE_BUFFER_SIZE |

**`SENDCMDINPARAMS` 输入布局**（32 字节，ntdddisk.h）：

```
offset 0  : DWORD  cBufferSize
offset 4  : IDEREGS irDriveRegs        // bFeaturesReg, bSectorCountReg, bSectorNumberReg,
                                       // bCylLowReg, bCylHighReg, bDriveHeadReg, bCommandReg, bReserved
offset 12 : BYTE   bDriveNumber
offset 13 : BYTE   bReserved[3]
offset 16 : DWORD  dwReserved[4]
```

**输出布局**：`SENDCMDOUTPARAMS`（16 字节 = `cBufferSize`(4) + `DRIVERSTATUS`(12)）之后紧接 512 字节 SMART 数据。实现中直接从输出缓冲 **偏移 16** 拷贝 512 字节（对应 CDI `memcpy_s(..., &sendCmdOutParam.SendCmdOutParam.bBuffer, 512)`）。

### 3.2 NVMe

对应 CDI `GetSmartAttributeNVMeStorageQuery`（`AtaSmart.cpp` L9010-9050）：

```
DeviceIoControl(hDevice, IOCTL_STORAGE_QUERY_PROPERTY, ...)
```

| 项目 | 值 | 来源 |
|------|-----|------|
| IOCTL | `IOCTL_STORAGE_QUERY_PROPERTY = 0x002D1400` | CDI StorageQuery.h |
| PropertyId | `StorageAdapterProtocolSpecificProperty = 49` | CDI StorageQuery.h L22-24 |
| QueryType | `PropertyStandardQuery = 0` | CDI StorageQuery.h |
| ProtocolType | `ProtocolTypeNvme = 3` | CDI StorageQuery.h L48 |
| DataType | `NVMeDataTypeLogPage = 2` | CDI StorageQuery.h L68 |
| LogPage ID | `2`（SMART / Health Information） | NVMe 规范 |
| ProtocolDataOffset | `40`（= sizeof(TStorageProtocolSpecificData)） | CDI L9026 |
| ProtocolDataLength | `4096` | CDI L9027 |

**结构布局**（`TStorageQueryWithBuffer`，8 + 40 + 4096 字节）：

```
offset 0  : STORAGE_PROPERTY_QUERY（8 字节）
offset 8  : STORAGE_PROTOCOL_SPECIFIC_DATA（40 字节）
offset 48 : BYTE Buffer[4096]        // NVMe 日志数据从偏移 48 开始
```

NVMe SMART/Health Info Log（512 字节）关键字段：

| 偏移 | 字段 | 说明 |
|------|------|------|
| 0 | Critical Warning | 非 0 表示控制器报告严重警告 |
| 1-2 | Temperature | 单位 Kelvin，`temp = raw[1] + raw[2]*256 - 273` |
| 3 | Available Spare | 可用备件百分比 |
| 4 | Spare Threshold | 备件阈值 |
| 5 | Percentage Used | 已用寿命百分比 → `Life = 100 - 该值` |
| 32 | Data Units Read | 累计读取 |
| 48 | Data Units Written | 累计写入 |
| 128-135 | Power On Hours | 通电小时数（小端 64 位） |

**NVMe 日志 → ATA 属性映射**（对应 CDI `NVMeSmartToATASmart`，`NVMeInterpreter.cpp` L169-189）：

| 映射索引 | 属性 Id | 来源 |
|---------|---------|------|
| Attribute[0] | 1 | Critical Warning（raw[0]） |
| Attribute[2] | 3 | Available Spare（raw[3]） |
| Attribute[3] | 4 | Spare Threshold（raw[4]） |

> 注：`CheckDiskStatus` 的 NVMe 分支仅用到 Attribute[0]/[2]/[3]，故解析层只需构造这 3 条。

## 4. 健康度判定逻辑（移植 CheckDiskStatus）

### 4.1 状态枚举

| 枚举 | 值 | 前端映射 |
|------|-----|---------|
| `DiskStatus::Unknown` | 0 | `health_status = "unknown"` |
| `DiskStatus::Good` | 1 | `health_status = "healthy"` |
| `DiskStatus::Caution` | 2 | `health_status = "warning"` |
| `DiskStatus::Bad` | 3 | `health_status = "unhealthy"` |

对应 CDI `AtaSmart.h` L303-307 `DISK_STATUS_UNKNOWN/GOOD/CAUTION/BAD`。

### 4.2 NVMe 判定（CDI L12529-12566）

1. **虚拟机排除**：型号以 `Parallels` / `VMware` / `QEMU` 开头 → `Unknown`；
2. **Critical Warning**：`Attribute[0].RawValue[0] > 0` → `Bad`；
3. **备件阈值**（`Attribute[3].RawValue[0]` 即 Spare Threshold）：
   - `== 0` 或 `> 100` → 阈值不可用，跳过；
   - `Attribute[2].RawValue[0] < Attribute[3].RawValue[0]`（可用备件 < 阈值）→ `Bad`；
   - `==` 且阈值 `!= 100` → `Caution`；
4. **寿命**：`Life > ThresholdFF(10)` → `Good`；否则 → `Caution`。

### 4.3 ATA 判定（HDD / SATA SSD）

**预检**（CDI L12568-12579）：
- 非 SSD 且无阈值（`!is_threshold_correct`）→ `Unknown`。

**遍历每个属性**：
1. **重复 ID 检测**（L12590-12597）：发现重复属性 Id → 整体 `Unknown`；
2. **关键属性范围**（L12622-12639），`CurrentValue < ThresholdValue` 且阈值非 0 → `error++`：

```
0x01~0x0D, 0x16, 0xBB~0xBD, 0xBF~0xC1, 0xC3~0xD1, 0xD3~0xD4,
0xDC~0xE4, 0xE6~0xE7, 0xF0, 0xFA, 0xFE
```

> 温度属性 `0xC2` 不参与 error 判定（L12610）；SSD 原始值 8 字节模式 `IsRawValues8` 也不参与（L12613）。

3. **05/C5/C6 坏扇区**（L12646-12682）：
   - 属性 `0x05`（重映射扇区）、`0xC5`（当前待映射扇区）、`0xC6`（离线不可校正扇区）；
   - 取小端 16 位原始值，与用户阈值比较（默认均为 1）：
     `raw >= Threshold05/C5/C6` → `caution = 1`（仅 HDD）；
   - RawValue[0..3] 全 `0xFF` 视为不可用跳过。

4. **SSD 寿命属性**（L12683-12791），命中即 `flagUnknown = FALSE` 并解析 Life：

| 属性 Id | 厂商 |
|---------|------|
| 0xA9 | Realtek / Kingston / Silicon Motion |
| 0xAD | KIOXIA |
| 0xB1 | Samsung |
| 0xBB | MTRON |
| 0xCA | Micron / Intel DC / Silicon Motion CVC |
| 0xD1 | Indilinx |
| 0xE7 | SandForce / Corsair / Kingston / SK Hynix / Realtek / SanDisk / SSSTC / Apacer / Phison / JMicron / Maxiotek / YMTC / SCY / Recadata / ADATA Industrial |
| 0xE8 | Plextor |
| 0xE9 | Intel / OCZ / SK Hynix / Sandisk Lenovo Helen Venus / Samsung 企业盘 |
| 0xE6 | WDC / SanDisk（特殊公式） |
| 0xC9 | SanDisk HP / HP Venus |

**Life 计算**（CDI L12704-12719）：
- `FlagLifeRawValueIncrement`（增量型）：`Life = 100 - RawValue[0]`；
- `FlagLifeRawValue`（直读型）：`Life = RawValue[0]`；
- 默认：`Life = CurrentValue`；
- 截断到 `0-100`。

**Life 判定**（CDI L12724-12735）：
- `Life == 0` → `error = 1`；
- `Life <= ThresholdFF(10)` → `caution = 1`。

**0xE6 (WDC/SanDisk) 特例**（CDI L12737-12769）：
- `Life = 100 - RawValue[1]`；
- `Life == 0` → `error = 1`；`Life <= ThresholdFF` → `caution = 1`。

**汇总**（CDI L12814-12829）：

```
error > 0        → Bad
flagUnknown      → Unknown   （无任何可判定的阈值/寿命信息）
caution > 0      → Caution
否则             → Good
```

### 4.4 健康度百分比（health_percent）

| 盘类型 | 计算方式 |
|--------|---------|
| NVMe | `Life = 100 - PercentageUsed`（raw[5]），对应 CDI `AtaSmart.cpp` L148 |
| SATA SSD | 寿命属性解析出的 `Life`（0-100 截断） |
| HDD | 无寿命属性，按状态映射：`Good=100 / Caution=50 / Bad=0 / Unknown=None`（前端显示 `--`） |

## 5. 数据结构（Rust 定义）

```rust
// 对应 CDI NVMeInterpreter.h L15-23
#[repr(C)]
pub struct SmartAttribute {
    pub id: u8,
    pub status_flags: u16,
    pub current_value: u8,
    pub worst_value: u8,
    pub raw_value: [u8; 6],
    pub reserved: u8,
}

// 对应 CDI AtaSmart.h L468-473
#[repr(C)]
pub struct SmartThreshold {
    pub id: u8,
    pub threshold_value: u8,
    pub reserved: [u8; 10],
}

pub enum DiskStatus { Unknown = 0, Good, Caution, Bad }

pub struct SmartInfo {
    pub status: DiskStatus,
    pub life_percent: Option<u8>,     // 健康度百分比
    pub temperature_c: Option<i32>,
    pub power_on_hours: Option<u64>,
    pub is_nvme: bool,
    pub has_smart: bool,
    pub error: Option<String>,
}
```

前端 `DiskHealthInfo` 新增字段：

```ts
health_percent: number | null;   // 0-100 或 null（读取失败显示 "--"）
```

## 6. 阈值配置

默认值与 CrystalDiskInfo `HealthDlg`（`HealthDlg.cpp` L370-373）一致：

| 阈值 | 默认值 | 含义 |
|------|--------|------|
| `Threshold05` | 1 | 重映射扇区数 ≥ 该值 → Caution |
| `ThresholdC5` | 1 | 当前待映射扇区数 ≥ 该值 → Caution |
| `ThresholdC6` | 1 | 离线不可校正扇区数 ≥ 该值 → Caution |
| `ThresholdFF` | 10 | SSD/NVMe 寿命 ≤ 该值 → Caution |

> 注：CDI 支持在设置对话框（HealthDlg）中按型号持久化调整阈值（写入 ini）。当前 Rust 版本采用固定默认值，如需可调，可在后续增加配置文件支持。

## 7. 分区与静态信息（无 PowerShell）

### 7.1 静态磁盘信息

WMI COM 直调（复用 `wmi_query.rs`，非 PowerShell）：

```sql
SELECT Index, Model, Size, InterfaceType, SerialNumber, FirmwareRevision,
       MediaType, Status, PNPDeviceID FROM Win32_DiskDrive
```

按 `Index` 与 `PhysicalDriveN` 对齐。

### 7.2 分区信息

winapi 卷枚举（`enumerate_volumes_by_disk`）：

1. `GetLogicalDrives()` 枚举盘符位图；
2. `GetDriveTypeW()` 过滤固定盘（`DRIVE_FIXED`）；
3. `CreateFileW("\\\\.\\X:")` + `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` 映射卷 → 物理盘号；
4. `GetDiskFreeSpaceExW()` 获取容量/可用空间；
5. `GetVolumeInformationW()` 获取文件系统。

### 7.3 分区表信息

WMI COM 查询 `Win32_DiskPartition`（`DiskIndex, Type, BootPartition`）：
- `Type` 含 `GPT` → 分区风格 GPT，否则有分区 → MBR；
- `BootPartition == true` → 引导盘。

## 8. 前端展示

`src/pages/DiskHealthPage.tsx`：

- 每张磁盘卡片顶部新增「健康度」进度条 + 百分比文本；
- 颜色映射（`healthColor`）：`healthy→green`、`warning→yellow`、`unhealthy→red`、`unknown→gray`；
- `health_percent === null` 时显示 `--`；
- `HealthBadge` 增加 `unknown` 灰色分支；
- 文案新增 `diskHealth.health`（zh/en/zh-TW/ja/fr/de 六个语言文件）。

## 9. 文件清单

| 文件 | 动作 | 说明 |
|------|------|------|
| `src-tauri/src/smart.rs` | 新增 | SMART 读取 + 解析 + 判定核心模块 |
| `src-tauri/src/lib.rs` | 修改 | `mod smart;` |
| `src-tauri/src/hardware.rs` | 修改 | 重写 `get_disk_health_info`：去 PowerShell、接入 smart.rs、分区改 winapi、新增 `health_percent`；删除 `run_powershell`/`PsDiskHealthRaw`/`PowerShellError` |
| `src/pages/DiskHealthPage.tsx` | 修改 | 健康度进度条 + unknown 状态 |
| `src/locales/*.json`（6 个） | 修改 | `diskHealth.health` 文案 |
| `docs/disk-health-smart.md` | 新增 | 本文档 |

## 10. 错误处理与日志

- 打开 `\\.\PhysicalDriveN` 失败（通常因权限不足）→ 返回 `Unknown` + `error` 描述，前端显示 `--`；
- 非管理员运行：ATA/NVMe SMART 读取在部分系统上可能被拒绝，此时状态为 `Unknown` 且日志记录原因；
- NVMe 读取失败自动回退 ATA 路径；
- 日志按盘记录摘要（型号、NVMe 标志、状态、健康度、温度、通电小时），不打印完整 512 字节缓冲。

## 11. 性能说明

- 每块盘 2-3 次 IOCTL（属性数据 + 阈值 + NVMe 重试），单盘耗时 < 10ms；
- `get_disk_health_info` 运行在 `spawn_blocking` 线程池，不阻塞 Tauri 主线程；
- 每次刷新实时读取，无需缓存。

## 12. 与 CrystalDiskInfo 的差异（已知简化）

| 维度 | CrystalDiskInfo | NeoBox 移植 |
|------|-----------------|-------------|
| 厂商识别 | 基于型号/固件深度检测 `DiskVendorId` | 简化：仅按寿命属性 ID 判定，不区分厂商细节 |
| `FlagLife*` 标志 | 通过特征检测逐个设置 | 简化：`0xE6` 用 `100-Raw[1]`、`0xE7` 用 `100-Raw[0]`，其余用 `CurrentValue` |
| 虚拟机排除 | NVMe 型号匹配 | 保留 |
| 阈值可调 | 设置对话框 + ini 持久化 | 固定默认值 |
| 接口覆盖 | SATA/SCSI/CSMI/MegaRAID/桥接芯片等 | 仅 SATA(ATA) + NVMe |
| SMART 属性明细展示 | 完整 30 条属性表 | 暂未展示明细（仅健康度/温度/通电） |
